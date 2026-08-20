//! Shared application contracts and state used by httphunter frontends.
//!
//! Proxy, certificate, and platform-control modules move here gradually.
//! Keeping capture state here prevents the desktop UI from depending on Axum.

use std::{
    collections::VecDeque,
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

mod config;
pub use config::{
    AppConfig, CaptureConfig, LoggingConfig, PrivacyConfig, ProxyConfig, SystemProxyConfig,
};

mod ca;
pub use ca::CaStore;

mod mitm;
mod proxy_engine;
mod runtime;
pub use runtime::{CaptureRuntimeStatus, HunterRuntime};

mod traffic;
pub use traffic::{
    EditableRequest, InterceptAction, InterceptResolution, MockRule, PendingIntercept,
    ReplayResult, SharedTrafficController, TrafficController,
};

mod system_proxy;
pub use system_proxy::{SystemProxyController, SystemProxyStatus};

pub const APP_NAME: &str = "httphunter";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplicationInfo {
    pub name: String,
    pub version: String,
}

impl ApplicationInfo {
    pub fn current(version: impl Into<String>) -> Self {
        Self {
            name: APP_NAME.to_owned(),
            version: version.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCompletedEvent {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeaderEntry {
    pub name: String,
    pub value: String,
}

impl HeaderEntry {
    pub fn new(name: &str, value: &http::HeaderValue) -> Self {
        Self {
            name: name.to_owned(),
            value: value.to_str().unwrap_or("<non-utf8>").to_owned(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestRecord {
    pub method: String,
    pub url: String,
    pub headers: Vec<HeaderEntry>,
    pub body: Vec<u8>,
}

impl RequestRecord {
    pub fn new(method: String, url: String, headers: Vec<HeaderEntry>, body: Vec<u8>) -> Self {
        Self {
            method,
            url,
            headers,
            body,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseRecord {
    pub status: u16,
    pub headers: Vec<HeaderEntry>,
    pub mime_type: Option<String>,
    pub body: Vec<u8>,
}

impl ResponseRecord {
    pub fn new(
        status: u16,
        headers: Vec<HeaderEntry>,
        mime_type: Option<String>,
        body: Vec<u8>,
    ) -> Self {
        Self {
            status,
            headers,
            mime_type,
            body,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpSession {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub client: SocketAddr,
    pub request: RequestRecord,
    pub response: ResponseRecord,
    pub duration_ms: i64,
}

impl HttpSession {
    pub fn completed(
        started_at: DateTime<Utc>,
        client: SocketAddr,
        request: RequestRecord,
        response: ResponseRecord,
    ) -> Self {
        let completed_at = Utc::now();
        let duration_ms = (completed_at - started_at).num_milliseconds();
        Self {
            id: Uuid::new_v4(),
            started_at,
            completed_at,
            client,
            request,
            response,
            duration_ms,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub started_at: DateTime<Utc>,
    pub method: String,
    pub url: String,
    pub host: String,
    pub path: String,
    pub status: u16,
    pub mime_type: Option<String>,
    pub response_size: usize,
    pub duration_ms: i64,
}

impl From<&HttpSession> for SessionSummary {
    fn from(session: &HttpSession) -> Self {
        let (host, path) = split_display_url(&session.request.url);
        Self {
            id: session.id.to_string(),
            started_at: session.started_at,
            method: session.request.method.clone(),
            url: session.request.url.clone(),
            host,
            path,
            status: session.response.status,
            mime_type: session.response.mime_type.clone(),
            response_size: session.response.body.len(),
            duration_ms: session.duration_ms,
        }
    }
}

fn split_display_url(url: &str) -> (String, String) {
    match url.parse::<http::Uri>() {
        Ok(uri) => {
            let host = uri
                .authority()
                .map(|authority| authority.to_string())
                .unwrap_or_default();
            let path = uri
                .path_and_query()
                .map(|path| path.to_string())
                .unwrap_or_else(|| "/".to_owned());
            (host, path)
        }
        Err(_) => (String::new(), url.to_owned()),
    }
}

#[derive(Debug, Clone)]
pub struct MemoryStore {
    sessions: Arc<RwLock<VecDeque<HttpSession>>>,
    enabled: Arc<AtomicBool>,
    capacity: usize,
}

impl MemoryStore {
    pub fn new(enabled: bool) -> Self {
        Self::with_capacity(enabled, 10_000)
    }

    pub fn with_capacity(enabled: bool, capacity: usize) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(VecDeque::new())),
            enabled: Arc::new(AtomicBool::new(enabled)),
            capacity,
        }
    }

    pub async fn insert(&self, session: HttpSession) {
        if !self.is_enabled() {
            return;
        }
        let mut sessions = self.sessions.write().await;
        if sessions.len() >= self.capacity {
            sessions.pop_front();
        }
        sessions.push_back(session);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    pub async fn list(&self) -> Vec<HttpSession> {
        self.sessions.read().await.iter().cloned().collect()
    }

    pub async fn len(&self) -> usize {
        self.sessions.read().await.len()
    }

    pub async fn get(&self, id: Uuid) -> Option<HttpSession> {
        self.sessions
            .read()
            .await
            .iter()
            .find(|session| session.id == id)
            .cloned()
    }

    pub async fn summaries(&self) -> Vec<SessionSummary> {
        self.sessions
            .read()
            .await
            .iter()
            .map(SessionSummary::from)
            .collect()
    }

    pub async fn clear(&self) {
        self.sessions.write().await.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disabled_store_does_not_record_sessions() {
        let store = MemoryStore::new(false);
        store
            .insert(HttpSession::completed(
                Utc::now(),
                "127.0.0.1:8080".parse().expect("valid socket address"),
                RequestRecord::new(
                    "GET".to_owned(),
                    "https://example.com".to_owned(),
                    vec![],
                    vec![],
                ),
                ResponseRecord::new(200, vec![], None, vec![]),
            ))
            .await;
        assert!(store.list().await.is_empty());
    }

    #[tokio::test]
    async fn store_discards_oldest_session_at_capacity() {
        let store = MemoryStore::with_capacity(true, 1);
        let client = "127.0.0.1:8080".parse().expect("valid socket address");
        let first = HttpSession::completed(
            Utc::now(),
            client,
            RequestRecord::new(
                "GET".to_owned(),
                "https://first.example".to_owned(),
                vec![],
                vec![],
            ),
            ResponseRecord::new(200, vec![], None, vec![]),
        );
        let first_id = first.id;
        store.insert(first).await;
        store
            .insert(HttpSession::completed(
                Utc::now(),
                client,
                RequestRecord::new(
                    "GET".to_owned(),
                    "https://second.example".to_owned(),
                    vec![],
                    vec![],
                ),
                ResponseRecord::new(200, vec![], None, vec![]),
            ))
            .await;

        let sessions = store.list().await;
        assert_eq!(sessions.len(), 1);
        assert_ne!(sessions[0].id, first_id);
        assert_eq!(sessions[0].request.url, "https://second.example");
    }
}
