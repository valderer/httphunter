use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use http::{header, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming, client::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use rustls::{
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName},
    ClientConfig, RootCertStore, ServerConfig,
};
use tokio::{net::TcpStream, sync::Mutex};
use tokio_rustls::{TlsAcceptor, TlsConnector};

use crate::{
    ca::CaStore,
    capture::{HeaderEntry, HttpSession, MemoryStore, RequestRecord, ResponseRecord},
    proxy::{is_hop_by_hop, ProxyBody},
};

type UpstreamSender = http1::SendRequest<ProxyBody>;

pub async fn serve(
    upgraded: hyper::upgrade::Upgraded,
    authority: String,
    ca: CaStore,
    store: Arc<MemoryStore>,
    peer: SocketAddr,
    timeout: Duration,
) -> Result<()> {
    let host = authority
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(authority.as_str())
        .trim_matches(['[', ']'])
        .to_owned();
    let (cert_der, key_der) = ca
        .leaf_for_host(&host)
        .context("failed to generate certificate for client TLS")?;

    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(cert_der)],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der)),
        )?;
    let acceptor = TlsAcceptor::from(Arc::new(server_config));
    let client_tls = acceptor
        .accept(TokioIo::new(upgraded))
        .await
        .context("client TLS handshake failed")?;

    let upstream = tokio::time::timeout(timeout, TcpStream::connect(&authority))
        .await
        .context("upstream TLS connection timed out")??;

    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let client_config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(client_config));
    let server_name = ServerName::try_from(host.clone())?;
    let upstream_tls = connector
        .connect(server_name, upstream)
        .await
        .context("upstream TLS handshake failed")?;

    let (sender, connection) = http1::handshake(TokioIo::new(upstream_tls))
        .await
        .context("upstream HTTP/1.1 handshake failed")?;
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            tracing::debug!(%error, "upstream MITM connection closed");
        }
    });

    let sender = Arc::new(Mutex::new(sender));
    let service = service_fn(move |request| {
        let sender = Arc::clone(&sender);
        let store = Arc::clone(&store);
        let authority = authority.clone();
        async move {
            Ok::<_, std::convert::Infallible>(
                forward_request(request, authority, peer, sender, store, timeout).await,
            )
        }
    });

    hyper::server::conn::http1::Builder::new()
        .serve_connection(TokioIo::new(client_tls), service)
        .await
        .context("MITM client HTTP connection failed")?;
    Ok(())
}

async fn forward_request(
    request: Request<Incoming>,
    authority: String,
    peer: SocketAddr,
    sender: Arc<Mutex<UpstreamSender>>,
    store: Arc<MemoryStore>,
    timeout: Duration,
) -> Response<ProxyBody> {
    let started_at = chrono::Utc::now();
    let method = request.method().clone();
    let relative_uri = request.uri().clone();
    tracing::info!(%peer, method = %method, uri = %relative_uri, "HTTPS request decrypted");
    let target_uri = relative_uri.clone();
    let upstream_host = upstream_host_header(&authority);
    let (parts, body) = request.into_parts();
    let body = match body.collect().await {
        Ok(body) => body.to_bytes(),
        Err(error) => return text_response(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    let request_headers = parts
        .headers
        .iter()
        .filter(|(name, _)| !is_hop_by_hop(name))
        .map(|(name, value)| HeaderEntry::new(name.as_str(), value))
        .collect();

    let mut builder = Request::builder()
        .method(method)
        .uri(target_uri)
        .header(header::HOST, upstream_host.as_str());
    for (name, value) in &parts.headers {
        if !is_hop_by_hop(name) && name != header::HOST {
            builder = builder.header(name, value);
        }
    }
    let upstream_request = match builder.body(Full::new(body.clone())) {
        Ok(request) => request,
        Err(error) => return text_response(StatusCode::BAD_REQUEST, &error.to_string()),
    };

    let upstream_response = {
        let mut sender = sender.lock().await;
        match tokio::time::timeout(timeout, sender.send_request(upstream_request)).await {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => return text_response(StatusCode::BAD_GATEWAY, &error.to_string()),
            Err(_) => {
                return text_response(StatusCode::GATEWAY_TIMEOUT, "upstream request timed out")
            }
        }
    };
    let status = upstream_response.status();
    let headers = upstream_response.headers().clone();
    let response_body = match upstream_response.into_body().collect().await {
        Ok(body) => body.to_bytes(),
        Err(error) => return text_response(StatusCode::BAD_GATEWAY, &error.to_string()),
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
    let display_url = normalized_https_url(&authority, &relative_uri);
    let session = HttpSession::completed(
        started_at,
        peer,
        RequestRecord::new(
            parts.method.to_string(),
            display_url,
            request_headers,
            body.to_vec(),
        ),
        ResponseRecord::new(
            status.as_u16(),
            response_headers,
            mime_type,
            response_body.to_vec(),
        ),
    );
    tracing::debug!(session_id = %session.id, "captured HTTPS MITM session");
    store.insert(session).await;

    let mut response = Response::builder().status(status);
    for (name, value) in &headers {
        if !is_hop_by_hop(name) {
            response = response.header(name, value);
        }
    }
    response
        .body(Full::new(response_body))
        .unwrap_or_else(|_| text_response(StatusCode::BAD_GATEWAY, "failed to build response"))
}

fn text_response(status: StatusCode, message: &str) -> Response<ProxyBody> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Full::new(hyper::body::Bytes::from(message.to_owned())))
        .expect("static MITM error response must be valid")
}

fn upstream_host_header(authority: &str) -> String {
    match authority.rsplit_once(':') {
        Some((host, port)) if port == "443" => host.to_owned(),
        _ => authority.to_owned(),
    }
}

fn normalized_https_url(authority: &str, uri: &http::Uri) -> String {
    let display_authority = match authority.rsplit_once(':') {
        Some((host, port)) if port == "443" => host,
        _ => authority,
    };
    format!("https://{}{}", display_authority, uri)
}
