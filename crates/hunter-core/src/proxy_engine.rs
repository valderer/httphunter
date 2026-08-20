use std::{
    convert::Infallible,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use http::{header, HeaderValue, Method, Request, Response, StatusCode, Uri};
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming, server::conn::http1, service::service_fn, upgrade};
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::{TokioExecutor, TokioIo},
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::watch,
};

use crate::mitm;
use crate::{
    AppConfig, CaStore, EditableRequest, HeaderEntry, HttpSession, InterceptAction, MemoryStore,
    ReplayResult, RequestRecord, ResponseRecord, SharedTrafficController,
};

pub(crate) type ProxyBody = Full<hyper::body::Bytes>;
type HttpClient = Client<HttpConnector, ProxyBody>;

pub(crate) async fn replay(
    request: EditableRequest,
    store: Arc<MemoryStore>,
    config: &AppConfig,
) -> ReplayResult {
    let started_at = chrono::Utc::now();
    let uri = match request.url.parse::<Uri>() {
        Ok(uri) => uri,
        Err(_) => return failed_replay(started_at, request, store, "invalid request URL").await,
    };
    if uri.scheme_str() == Some("https") {
        return crate::mitm::replay_https(request, store, config).await;
    }
    if uri.scheme_str() != Some("http") {
        return failed_replay(started_at, request, store, "only HTTP and HTTPS URLs can be replayed").await;
    }
    let mut builder = Request::builder().method(request.method.as_str()).uri(uri);
    for header in &request.headers {
        if !header.name.eq_ignore_ascii_case("host") && !is_hop_by_hop_name(&header.name) {
            builder = builder.header(header.name.as_str(), header.value.as_str());
        }
    }
    let upstream = match builder.body(Full::new(hyper::body::Bytes::from(request.body.clone()))) {
        Ok(request) => request,
        Err(error) => return failed_replay(started_at, request, store, &error.to_string()).await,
    };
    let client = build_client(config);
    let response = match tokio::time::timeout(
        Duration::from_millis(config.proxy.request_timeout_ms),
        client.request(upstream),
    )
    .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => return failed_replay(started_at, request, store, &error.to_string()).await,
        Err(_) => return failed_replay(started_at, request, store, "replay request timed out").await,
    };
    let status = response.status();
    let headers = response.headers().clone();
    let body = match response.into_body().collect().await {
        Ok(body) => body.to_bytes().to_vec(),
        Err(error) => return failed_replay(started_at, request, store, &error.to_string()).await,
    };
    let header_entries = headers
        .iter()
        .filter(|(name, _)| !is_hop_by_hop(name))
        .map(|(name, value)| HeaderEntry::new(name.as_str(), value))
        .collect();
    completed_replay(started_at, request, status.as_u16(), header_entries, body, store, None).await
}

pub(crate) async fn completed_replay(
    started_at: chrono::DateTime<chrono::Utc>,
    request: EditableRequest,
    status: u16,
    headers: Vec<HeaderEntry>,
    body: Vec<u8>,
    store: Arc<MemoryStore>,
    error: Option<String>,
) -> ReplayResult {
    let mime_type = headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("content-type"))
        .map(|header| header.value.clone());
    let session = HttpSession::completed(
        started_at,
        "127.0.0.1:0".parse().expect("static localhost address"),
        RequestRecord::new(request.method, request.url, request.headers, request.body),
        ResponseRecord::new(status, headers, mime_type, body),
    );
    store.insert(session.clone()).await;
    ReplayResult { session, error }
}

pub(crate) async fn failed_replay(
    started_at: chrono::DateTime<chrono::Utc>,
    request: EditableRequest,
    store: Arc<MemoryStore>,
    error: &str,
) -> ReplayResult {
    completed_replay(
        started_at,
        request,
        599,
        vec![HeaderEntry { name: "content-type".to_owned(), value: "text/plain; charset=utf-8".to_owned() }],
        error.as_bytes().to_vec(),
        store,
        Some(error.to_owned()),
    )
    .await
}

pub async fn serve(
    listener: TcpListener,
    config: AppConfig,
    mitm_enabled: bool,
    store: Arc<MemoryStore>,
    traffic: SharedTrafficController,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let listen = listener
        .local_addr()
        .context("failed to read proxy listener address")?;
    let client = build_client(&config);
    let timeout = Duration::from_millis(config.proxy.request_timeout_ms);
    let client = Arc::new(client);
    let ca = CaStore::default()?;
    let mitm_exclude = config.proxy.mitm_exclude.clone();
    let allow_lan_clients = config.proxy.allow_lan_clients;

    tracing::info!(%listen, "HTTP proxy listening");

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, peer) = result.context("failed to accept client connection")?;
                let client = Arc::clone(&client);
                let store = Arc::clone(&store);
                let traffic = Arc::clone(&traffic);
                let ca = ca.clone();
                let mitm_exclude = mitm_exclude.clone();
                if allow_lan_clients && !is_private_network_client(peer.ip()) {
                    tracing::warn!(%peer, "rejected non-private client for LAN proxy");
                    continue;
                }
                tokio::spawn(async move {
                    if let Err(error) = serve_client(stream, peer, client, store, traffic, ca, mitm_enabled, mitm_exclude, timeout).await {
                        tracing::debug!(%peer, %error, "client connection ended with an error");
                    }
                });
            }
            changed = shutdown.changed() => {
                changed.context("proxy shutdown signal closed unexpectedly")?;
                if !*shutdown.borrow() {
                    continue;
                }
                tracing::info!("shutting down proxy");
                return Ok(());
            }
        }
    }
}

fn is_private_network_client(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || address == Ipv4Addr::UNSPECIFIED
        }
        IpAddr::V6(address) => {
            address.is_loopback()
                || address.is_unicast_link_local()
                || (address.segments()[0] & 0xfe00) == 0xfc00
                || address == Ipv6Addr::UNSPECIFIED
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_private_network_client;
    use std::net::IpAddr;

    #[test]
    fn accepts_private_and_local_clients() {
        for address in [
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "127.0.0.1",
            "169.254.10.1",
            "::1",
            "fe80::1",
            "fd00::1",
        ] {
            assert!(is_private_network_client(address.parse::<IpAddr>().unwrap()));
        }
    }

    #[test]
    fn rejects_public_clients() {
        for address in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            assert!(!is_private_network_client(address.parse::<IpAddr>().unwrap()));
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
    traffic: SharedTrafficController,
    ca: CaStore,
    mitm_enabled: bool,
    mitm_exclude: Vec<String>,
    timeout: Duration,
) -> Result<()> {
    let io = TokioIo::new(stream);
    let service = service_fn(move |request| {
        let client = Arc::clone(&client);
        let store = Arc::clone(&store);
        let traffic = Arc::clone(&traffic);
        let ca = ca.clone();
        let mitm_exclude = mitm_exclude.clone();
        async move {
            Ok::<_, Infallible>(
                handle_request(
                    request,
                    peer,
                    client,
                    store,
                    traffic,
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
    traffic: SharedTrafficController,
    ca: CaStore,
    mitm_enabled: bool,
    mitm_exclude: Vec<String>,
    timeout: Duration,
) -> Response<ProxyBody> {
    let started_at = chrono::Utc::now();
    let request_uri = request.uri().to_string();
    tracing::info!(%peer, method = %request.method(), uri = %request.uri(), "request received");

    if request.method() == Method::CONNECT {
        return handle_connect(
            request,
            peer,
            store,
            traffic,
            ca,
            mitm_enabled,
            mitm_exclude,
            timeout,
        )
        .await;
    }

    let (parts, body) = request.into_parts();
    let body = match body.collect().await {
        Ok(body) => body.to_bytes(),
        Err(error) => {
            tracing::warn!(%peer, %error, "failed to read request body");
            return text_response(StatusCode::BAD_REQUEST, "failed to read request body");
        }
    };
    let request_headers: Vec<_> = parts
        .headers
        .iter()
        .filter(|(name, _)| !is_hop_by_hop(name))
        .map(|(name, value)| HeaderEntry::new(name.as_str(), value))
        .collect();

    let mut editable = EditableRequest {
        method: parts.method.to_string(),
        url: request_uri,
        headers: request_headers,
        body: body.to_vec(),
    };
    if let Some(rule) = traffic.matching_mock(&editable).await {
        let (status, headers, body) = crate::traffic::mock_response(&rule);
        return static_captured_response(started_at, peer, editable, status, headers, body, store).await;
    }
    if let Some(resolution) = traffic.intercept(editable.clone()).await {
        editable = resolution.request;
        if matches!(resolution.action, InterceptAction::Drop) {
            return static_captured_response(
                started_at,
                peer,
                editable,
                StatusCode::FORBIDDEN,
                vec![HeaderEntry { name: "content-type".to_owned(), value: "text/plain; charset=utf-8".to_owned() }],
                b"request dropped by httphunter".to_vec(),
                store,
            ).await;
        }
    }
    let uri = match editable.url.parse::<Uri>().ok().and_then(|uri| absolute_uri(&uri).ok()) {
        Some(uri) => uri,
        None => return text_response(StatusCode::BAD_REQUEST, "request URL must be absolute"),
    };
    let mut builder = Request::builder().method(editable.method.as_str()).uri(uri);
    for header in &editable.headers {
        if !header.name.eq_ignore_ascii_case("host") && !is_hop_by_hop_name(&header.name) {
            builder = builder.header(header.name.as_str(), header.value.as_str());
        }
    }

    let upstream_request = match builder.body(Full::new(hyper::body::Bytes::from(
        editable.body.clone(),
    ))) {
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
        RequestRecord::new(editable.method, editable.url, editable.headers, editable.body),
        ResponseRecord::new(status.as_u16(), response_headers, mime_type, body_for_capture(&body)),
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
    traffic: SharedTrafficController,
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
                        mitm::serve(upgraded, authority, ca, store, traffic, peer, timeout).await
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

pub(crate) fn is_hop_by_hop_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
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

pub(crate) async fn static_captured_response(
    started_at: chrono::DateTime<chrono::Utc>,
    peer: SocketAddr,
    request: EditableRequest,
    status: StatusCode,
    headers: Vec<HeaderEntry>,
    body: Vec<u8>,
    store: Arc<MemoryStore>,
) -> Response<ProxyBody> {
    let mime_type = headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("content-type"))
        .map(|header| header.value.clone());
    let session = HttpSession::completed(
        started_at,
        peer,
        RequestRecord::new(request.method, request.url, request.headers, request.body),
        ResponseRecord::new(status.as_u16(), headers.clone(), mime_type, body.clone()),
    );
    store.insert(session).await;
    response_from_entries(status, &headers, body)
}

pub(crate) fn response_from_entries(
    status: StatusCode,
    headers: &[HeaderEntry],
    body: Vec<u8>,
) -> Response<ProxyBody> {
    let mut response = Response::builder().status(status);
    for header in headers {
        if !is_hop_by_hop_name(&header.name) {
            response = response.header(header.name.as_str(), header.value.as_str());
        }
    }
    response
        .body(Full::new(hyper::body::Bytes::from(body)))
        .unwrap_or_else(|_| text_response(StatusCode::BAD_GATEWAY, "failed to build response"))
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
