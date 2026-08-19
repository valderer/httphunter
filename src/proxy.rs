use std::{convert::Infallible, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use http::{header, HeaderValue, Method, Request, Response, StatusCode, Uri};
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming, server::conn::http1, service::service_fn, upgrade};
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::{TokioExecutor, TokioIo},
};
use tokio::net::{TcpListener, TcpStream};

use crate::api;
use crate::ca::CaStore;
use crate::capture::{HeaderEntry, HttpSession, MemoryStore, RequestRecord, ResponseRecord};
use crate::config::AppConfig;
use crate::mitm;
use crate::system_proxy::SystemProxyController;

pub(crate) type ProxyBody = Full<hyper::body::Bytes>;
type HttpClient = Client<HttpConnector, ProxyBody>;

pub async fn run(listen: SocketAddr, config: &AppConfig, mitm_enabled: bool) -> Result<()> {
    let listener = TcpListener::bind(listen)
        .await
        .with_context(|| format!("failed to bind proxy listener at {listen}"))?;
    let client = build_client(config);
    let timeout = Duration::from_millis(config.proxy.request_timeout_ms);
    let client = Arc::new(client);
    let store = Arc::new(MemoryStore::new(config.capture.enabled));
    let ca = CaStore::default()?;
    let mitm_exclude = config.proxy.mitm_exclude.clone();
    let system_proxy =
        SystemProxyController::new(&config.system_proxy, listen.ip().to_string(), listen.port());

    if config.api.enabled {
        let api_store = Arc::clone(&store);
        let api_listen = config.api.listen;
        tokio::spawn(async move {
            if let Err(error) = api::run(api_listen, api_store, system_proxy).await {
                tracing::error!(%error, "local API stopped");
            }
        });
    }

    tracing::info!(%listen, "HTTP proxy listening");

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, peer) = result.context("failed to accept client connection")?;
                let client = Arc::clone(&client);
                let store = Arc::clone(&store);
                let ca = ca.clone();
                let mitm_exclude = mitm_exclude.clone();
                tokio::spawn(async move {
                    if let Err(error) = serve_client(stream, peer, client, store, ca, mitm_enabled, mitm_exclude, timeout).await {
                        tracing::debug!(%peer, %error, "client connection ended with an error");
                    }
                });
            }
            result = tokio::signal::ctrl_c() => {
                result.context("failed to listen for shutdown signal")?;
                tracing::info!("shutting down proxy");
                return Ok(());
            }
        }
    }
}

fn build_client(config: &AppConfig) -> HttpClient {
    let mut connector = HttpConnector::new();
    connector.set_connect_timeout(Some(Duration::from_millis(config.proxy.connect_timeout_ms)));
    connector.enforce_http(false);
    Client::builder(TokioExecutor::new()).build(connector)
}

async fn serve_client(
    stream: TcpStream,
    peer: SocketAddr,
    client: Arc<HttpClient>,
    store: Arc<MemoryStore>,
    ca: CaStore,
    mitm_enabled: bool,
    mitm_exclude: Vec<String>,
    timeout: Duration,
) -> Result<()> {
    let io = TokioIo::new(stream);
    let service = service_fn(move |request| {
        let client = Arc::clone(&client);
        let store = Arc::clone(&store);
        let ca = ca.clone();
        let mitm_exclude = mitm_exclude.clone();
        async move {
            Ok::<_, Infallible>(
                handle_request(
                    request,
                    peer,
                    client,
                    store,
                    ca,
                    mitm_enabled,
                    mitm_exclude,
                    timeout,
                )
                .await,
            )
        }
    });

    http1::Builder::new()
        .preserve_header_case(true)
        .title_case_headers(true)
        .serve_connection(io, service)
        .with_upgrades()
        .await
        .context("HTTP/1.1 connection failed")?;
    Ok(())
}

async fn handle_request(
    request: Request<Incoming>,
    peer: SocketAddr,
    client: Arc<HttpClient>,
    store: Arc<MemoryStore>,
    ca: CaStore,
    mitm_enabled: bool,
    mitm_exclude: Vec<String>,
    timeout: Duration,
) -> Response<ProxyBody> {
    let started_at = chrono::Utc::now();
    let request_method = request.method().to_string();
    let request_uri = request.uri().to_string();
    tracing::info!(%peer, method = %request.method(), uri = %request.uri(), "request received");

    if request.method() == Method::CONNECT {
        return handle_connect(
            request,
            peer,
            store,
            ca,
            mitm_enabled,
            mitm_exclude,
            timeout,
        )
        .await;
    }

    let uri = match absolute_uri(request.uri()) {
        Ok(uri) => uri,
        Err(message) => return text_response(StatusCode::BAD_REQUEST, &message),
    };

    let (parts, body) = request.into_parts();
    let body = match body.collect().await {
        Ok(body) => body.to_bytes(),
        Err(error) => {
            tracing::warn!(%peer, %error, "failed to read request body");
            return text_response(StatusCode::BAD_REQUEST, "failed to read request body");
        }
    };
    let request_body_for_capture = body.to_vec();

    let request_headers = parts
        .headers
        .iter()
        .filter(|(name, _)| !is_hop_by_hop(name))
        .map(|(name, value)| HeaderEntry::new(name.as_str(), value))
        .collect();

    let mut builder = Request::builder().method(parts.method).uri(uri);
    for (name, value) in &parts.headers {
        if !is_hop_by_hop(name) && name != header::HOST {
            builder = builder.header(name, value);
        }
    }

    let upstream_request = match builder.body(Full::new(body)) {
        Ok(request) => request,
        Err(error) => {
            tracing::warn!(%peer, %error, "failed to build upstream request");
            return text_response(StatusCode::BAD_REQUEST, "failed to build upstream request");
        }
    };

    let upstream_response = match tokio::time::timeout(timeout, client.request(upstream_request))
        .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            tracing::warn!(%peer, %error, "upstream request failed");
            return text_response(StatusCode::BAD_GATEWAY, "upstream request failed");
        }
        Err(_) => return text_response(StatusCode::GATEWAY_TIMEOUT, "upstream request timed out"),
    };

    let status = upstream_response.status();
    let headers = upstream_response.headers().clone();
    let body = match upstream_response.into_body().collect().await {
        Ok(body) => body.to_bytes(),
        Err(error) => {
            tracing::warn!(%peer, %error, "failed to read upstream response body");
            return text_response(
                StatusCode::BAD_GATEWAY,
                "failed to read upstream response body",
            );
        }
    };

    let response_headers = headers
        .iter()
        .filter(|(name, _)| !is_hop_by_hop(name))
        .map(|(name, value)| HeaderEntry::new(name.as_str(), value))
        .collect();
    let mime_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    let session = HttpSession::completed(
        started_at,
        peer,
        RequestRecord::new(
            request_method,
            request_uri,
            request_headers,
            request_body_for_capture,
        ),
        ResponseRecord::new(
            status.as_u16(),
            response_headers,
            mime_type,
            body_for_capture(&body),
        ),
    );
    tracing::debug!(session_id = %session.id, "captured HTTP session");
    store.insert(session).await;

    let mut response = Response::builder().status(status);
    for (name, value) in &headers {
        if !is_hop_by_hop(name) {
            response = response.header(name, value);
        }
    }
    response
        .body(Full::new(body))
        .unwrap_or_else(|_| text_response(StatusCode::BAD_GATEWAY, "failed to build response"))
}

async fn handle_connect(
    request: Request<Incoming>,
    peer: SocketAddr,
    store: Arc<MemoryStore>,
    ca: CaStore,
    mitm_enabled: bool,
    mitm_exclude: Vec<String>,
    timeout: Duration,
) -> Response<ProxyBody> {
    let authority = match request.uri().authority() {
        Some(authority) => authority.to_string(),
        None => return text_response(StatusCode::BAD_REQUEST, "CONNECT requires host:port"),
    };

    if mitm_enabled && !is_excluded(&authority, &mitm_exclude) {
        let on_upgrade = upgrade::on(request);
        tokio::spawn(async move {
            match on_upgrade.await {
                Ok(upgraded) => {
                    if let Err(error) =
                        mitm::serve(upgraded, authority, ca, store, peer, timeout).await
                    {
                        tracing::warn!(%peer, %error, "MITM tunnel failed");
                    }
                }
                Err(error) => tracing::warn!(%peer, %error, "client upgrade failed"),
            }
        });
        return Response::builder()
            .status(StatusCode::OK)
            .body(Full::new(hyper::body::Bytes::new()))
            .expect("static CONNECT response must be valid");
    }

    if mitm_enabled {
        tracing::info!(%peer, %authority, "MITM bypassed for excluded host; using CONNECT tunnel");
    }

    let upstream = match tokio::time::timeout(timeout, TcpStream::connect(&authority)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            tracing::warn!(%peer, %authority, %error, "CONNECT upstream failed");
            return text_response(StatusCode::BAD_GATEWAY, "CONNECT upstream failed");
        }
        Err(_) => return text_response(StatusCode::GATEWAY_TIMEOUT, "CONNECT timed out"),
    };

    let on_upgrade = upgrade::on(request);
    tokio::spawn(async move {
        match on_upgrade.await {
            Ok(upgraded) => {
                let mut client = TokioIo::new(upgraded);
                let mut upstream = upstream;
                if let Err(error) = tokio::io::copy_bidirectional(&mut client, &mut upstream).await
                {
                    tracing::debug!(%peer, %authority, %error, "CONNECT tunnel closed");
                }
            }
            Err(error) => {
                tracing::debug!(%peer, %authority, %error, "client upgrade failed");
            }
        }
    });

    Response::builder()
        .status(StatusCode::OK)
        .body(Full::new(hyper::body::Bytes::new()))
        .expect("static CONNECT response must be valid")
}

fn is_excluded(authority: &str, excludes: &[String]) -> bool {
    let host = authority
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(authority)
        .trim_matches(['[', ']'])
        .to_ascii_lowercase();
    excludes.iter().any(|pattern| {
        let pattern = pattern.trim().to_ascii_lowercase();
        host == pattern || host.ends_with(&format!(".{pattern}"))
    })
}

fn body_for_capture(body: &hyper::body::Bytes) -> Vec<u8> {
    body.to_vec()
}

fn absolute_uri(uri: &Uri) -> Result<Uri, String> {
    if uri.scheme().is_some() && uri.authority().is_some() {
        return Ok(uri.clone());
    }

    Err("proxy requests must use an absolute URI, for example http://example.com/".to_owned())
}

pub(crate) fn is_hop_by_hop(name: &header::HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn text_response(status: StatusCode, message: &str) -> Response<ProxyBody> {
    Response::builder()
        .status(status)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        )
        .body(Full::new(hyper::body::Bytes::from(message.to_owned())))
        .expect("static proxy error response must be valid")
}
