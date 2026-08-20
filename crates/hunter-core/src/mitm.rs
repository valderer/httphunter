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
    proxy_engine::{
        is_hop_by_hop, is_hop_by_hop_name, static_captured_response, ProxyBody,
    },
    CaStore, EditableRequest, HeaderEntry, HttpSession, InterceptAction, MemoryStore,
    RequestRecord, ResponseRecord, SharedTrafficController,
};

type UpstreamSender = http1::SendRequest<ProxyBody>;

pub(crate) async fn replay_https(
    request: EditableRequest,
    store: Arc<MemoryStore>,
    config: &crate::AppConfig,
) -> crate::ReplayResult {
    let started_at = chrono::Utc::now();
    let uri = match request.url.parse::<http::Uri>() {
        Ok(uri) => uri,
        Err(_) => return crate::proxy_engine::failed_replay(started_at, request, store, "invalid HTTPS URL").await,
    };
    let Some(authority) = uri.authority().map(|authority| authority.to_string()) else {
        return crate::proxy_engine::failed_replay(started_at, request, store, "HTTPS URL requires a host").await;
    };
    let host = uri.host().unwrap_or_default().to_owned();
    let port = uri.port_u16().unwrap_or(443);
    let upstream = match tokio::time::timeout(
        Duration::from_millis(config.proxy.connect_timeout_ms),
        TcpStream::connect((host.as_str(), port)),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => return crate::proxy_engine::failed_replay(started_at, request, store, &error.to_string()).await,
        Err(_) => return crate::proxy_engine::failed_replay(started_at, request, store, "replay connection timed out").await,
    };
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let connector = TlsConnector::from(Arc::new(
        ClientConfig::builder().with_root_certificates(roots).with_no_client_auth(),
    ));
    let server_name = match ServerName::try_from(host.clone()) {
        Ok(name) => name,
        Err(error) => return crate::proxy_engine::failed_replay(started_at, request, store, &error.to_string()).await,
    };
    let upstream_tls = match connector.connect(server_name, upstream).await {
        Ok(stream) => stream,
        Err(error) => return crate::proxy_engine::failed_replay(started_at, request, store, &error.to_string()).await,
    };
    let (mut sender, connection) = match http1::handshake(TokioIo::new(upstream_tls)).await {
        Ok(connection) => connection,
        Err(error) => return crate::proxy_engine::failed_replay(started_at, request, store, &error.to_string()).await,
    };
    tokio::spawn(async move { let _ = connection.await; });
    let path = uri.path_and_query().cloned().unwrap_or_else(|| http::uri::PathAndQuery::from_static("/"));
    let mut builder = Request::builder().method(request.method.as_str()).uri(path).header(header::HOST, authority);
    for header in &request.headers {
        if !header.name.eq_ignore_ascii_case("host") && !is_hop_by_hop_name(&header.name) {
            builder = builder.header(header.name.as_str(), header.value.as_str());
        }
    }
    let upstream_request = match builder.body(Full::new(hyper::body::Bytes::from(request.body.clone()))) {
        Ok(request) => request,
        Err(error) => return crate::proxy_engine::failed_replay(started_at, request, store, &error.to_string()).await,
    };
    let response = match tokio::time::timeout(Duration::from_millis(config.proxy.request_timeout_ms), sender.send_request(upstream_request)).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => return crate::proxy_engine::failed_replay(started_at, request, store, &error.to_string()).await,
        Err(_) => return crate::proxy_engine::failed_replay(started_at, request, store, "replay request timed out").await,
    };
    let status = response.status().as_u16();
    let headers = response.headers().iter().filter(|(name, _)| !is_hop_by_hop(name)).map(|(name, value)| HeaderEntry::new(name.as_str(), value)).collect();
    let body = match response.into_body().collect().await {
        Ok(body) => body.to_bytes().to_vec(),
        Err(error) => return crate::proxy_engine::failed_replay(started_at, request, store, &error.to_string()).await,
    };
    crate::proxy_engine::completed_replay(started_at, request, status, headers, body, store, None).await
}

pub async fn serve(
    upgraded: hyper::upgrade::Upgraded,
    authority: String,
    ca: CaStore,
    store: Arc<MemoryStore>,
    traffic: SharedTrafficController,
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
        let traffic = Arc::clone(&traffic);
        let authority = authority.clone();
        async move {
            Ok::<_, std::convert::Infallible>(
                forward_request(request, authority, peer, sender, store, traffic, timeout).await,
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
    traffic: SharedTrafficController,
    timeout: Duration,
) -> Response<ProxyBody> {
    let started_at = chrono::Utc::now();
    let method = request.method().clone();
    let relative_uri = request.uri().clone();
    tracing::info!(%peer, method = %method, uri = %relative_uri, "HTTPS request decrypted");
    let upstream_host = upstream_host_header(&authority);
    let (parts, body) = request.into_parts();
    let body = match body.collect().await {
        Ok(body) => body.to_bytes(),
        Err(error) => return text_response(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    let request_headers: Vec<_> = parts
        .headers
        .iter()
        .filter(|(name, _)| !is_hop_by_hop(name))
        .map(|(name, value)| HeaderEntry::new(name.as_str(), value))
        .collect();

    let display_url = normalized_https_url(&authority, &relative_uri);
    let mut editable = EditableRequest {
        method: method.to_string(),
        url: display_url,
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
    let edited_uri = match editable.url.parse::<http::Uri>() {
        Ok(uri) if is_same_https_target(&uri, &authority) => uri
            .path_and_query()
            .cloned()
            .unwrap_or_else(|| http::uri::PathAndQuery::from_static("/")),
        _ => return text_response(StatusCode::BAD_REQUEST, "HTTPS interception cannot change the target host"),
    };
    let mut builder = Request::builder()
        .method(editable.method.as_str())
        .uri(edited_uri)
        .header(header::HOST, upstream_host.as_str());
    for header in &editable.headers {
        if !header.name.eq_ignore_ascii_case("host") {
            builder = builder.header(header.name.as_str(), header.value.as_str());
        }
    }
    let upstream_request = match builder.body(Full::new(hyper::body::Bytes::from(
        editable.body.clone(),
    ))) {
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
    let session = HttpSession::completed(
        started_at,
        peer,
        RequestRecord::new(
            editable.method,
            editable.url,
            editable.headers,
            editable.body,
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

fn is_same_https_target(uri: &http::Uri, tunnel_authority: &str) -> bool {
    let Ok(tunnel) = tunnel_authority.parse::<http::uri::Authority>() else {
        return false;
    };
    uri.scheme_str() == Some("https")
        && uri
            .host()
            .is_some_and(|host| host.eq_ignore_ascii_case(tunnel.host()))
        && uri.port_u16().unwrap_or(443) == tunnel.port_u16().unwrap_or(443)
}

#[cfg(test)]
mod tests {
    use super::is_same_https_target;

    #[test]
    fn accepts_default_https_port_with_or_without_explicit_port() {
        assert!(is_same_https_target(
            &"https://example.com/path".parse().unwrap(),
            "example.com:443"
        ));
        assert!(is_same_https_target(
            &"https://example.com:443/path".parse().unwrap(),
            "example.com"
        ));
    }

    #[test]
    fn rejects_a_different_https_target() {
        assert!(!is_same_https_target(
            &"https://other.example/path".parse().unwrap(),
            "example.com:443"
        ));
        assert!(!is_same_https_target(
            &"https://example.com:8443/path".parse().unwrap(),
            "example.com:443"
        ));
    }
}
