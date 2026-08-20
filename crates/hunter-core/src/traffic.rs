use std::{collections::HashMap, io::Write, sync::Arc};

use flate2::{write::GzEncoder, Compression};
use http::StatusCode;
use serde::{Deserialize, Serialize};
use tokio::sync::{oneshot, Mutex, RwLock};
use uuid::Uuid;

use crate::HeaderEntry;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditableRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<HeaderEntry>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockRule {
    pub id: String,
    pub enabled: bool,
    pub method: String,
    pub url_pattern: String,
    pub status: u16,
    pub headers: Vec<HeaderEntry>,
    pub body: Vec<u8>,
}

impl MockRule {
    pub fn new(mut rule: Self) -> Self {
        if rule.id.is_empty() {
            rule.id = Uuid::new_v4().to_string();
        }
        rule
    }

    pub fn matches(&self, request: &EditableRequest) -> bool {
        self.enabled
            && (self.method.trim().is_empty()
                || self.method.eq_ignore_ascii_case(&request.method))
            && !self.url_pattern.trim().is_empty()
            && request
                .url
                .to_ascii_lowercase()
                .contains(&self.url_pattern.trim().to_ascii_lowercase())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingIntercept {
    pub id: String,
    pub request: EditableRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InterceptAction {
    Forward,
    Drop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterceptResolution {
    pub action: InterceptAction,
    pub request: EditableRequest,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReplayResult {
    pub session: crate::HttpSession,
    pub error: Option<String>,
}

#[derive(Default)]
struct TrafficState {
    intercept_enabled: bool,
    rules: Vec<MockRule>,
}

pub struct TrafficController {
    state: RwLock<TrafficState>,
    pending: Mutex<HashMap<String, PendingRequest>>,
}

struct PendingRequest {
    request: EditableRequest,
    sender: oneshot::Sender<InterceptResolution>,
}

impl TrafficController {
    pub fn new() -> Self {
        Self {
            state: RwLock::new(TrafficState::default()),
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub async fn intercept_enabled(&self) -> bool {
        self.state.read().await.intercept_enabled
    }

    pub async fn set_intercept_enabled(&self, enabled: bool) {
        self.state.write().await.intercept_enabled = enabled;
    }

    pub async fn rules(&self) -> Vec<MockRule> {
        self.state.read().await.rules.clone()
    }

    pub async fn save_rule(&self, rule: MockRule) -> MockRule {
        let rule = MockRule::new(rule);
        let mut state = self.state.write().await;
        if let Some(existing) = state.rules.iter_mut().find(|existing| existing.id == rule.id) {
            *existing = rule.clone();
        } else {
            state.rules.push(rule.clone());
        }
        rule
    }

    pub async fn delete_rule(&self, id: &str) -> bool {
        let mut state = self.state.write().await;
        let before = state.rules.len();
        state.rules.retain(|rule| rule.id != id);
        state.rules.len() != before
    }

    pub async fn matching_mock(&self, request: &EditableRequest) -> Option<MockRule> {
        self.state
            .read()
            .await
            .rules
            .iter()
            .find(|rule| rule.matches(request))
            .cloned()
    }

    pub async fn intercept(&self, request: EditableRequest) -> Option<InterceptResolution> {
        if !self.intercept_enabled().await {
            return None;
        }
        let id = Uuid::new_v4().to_string();
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(
            id.clone(),
            PendingRequest {
                request,
                sender,
            },
        );
        let result = receiver.await.ok();
        self.pending.lock().await.remove(&id);
        result
    }

    pub async fn pending(&self) -> Vec<PendingIntercept> {
        self.pending
            .lock()
            .await
            .iter()
            .map(|(id, pending)| PendingIntercept {
                id: id.clone(),
                request: pending.request.clone(),
            })
            .collect()
    }

    pub async fn resolve(&self, id: &str, resolution: InterceptResolution) -> bool {
        self.pending
            .lock()
            .await
            .remove(id)
            .is_some_and(|pending| pending.sender.send(resolution).is_ok())
    }
}

pub fn mock_response(rule: &MockRule) -> (StatusCode, Vec<HeaderEntry>, Vec<u8>) {
    let status = StatusCode::from_u16(rule.status).unwrap_or(StatusCode::OK);
    let mut headers: Vec<_> = rule
        .headers
        .iter()
        .filter(|header| !header.name.eq_ignore_ascii_case("content-length"))
        .cloned()
        .collect();
    let body = if uses_gzip(&headers) {
        gzip(&rule.body).unwrap_or_else(|error| {
            tracing::warn!(%error, "failed to gzip mock response body");
            rule.body.clone()
        })
    } else {
        rule.body.clone()
    };
    headers.push(HeaderEntry {
        name: "content-length".to_owned(),
        value: body.len().to_string(),
    });
    (status, headers, body)
}

fn uses_gzip(headers: &[HeaderEntry]) -> bool {
    headers.iter().any(|header| {
        header.name.eq_ignore_ascii_case("content-encoding")
            && header
                .value
                .split(',')
                .any(|encoding| encoding.trim().eq_ignore_ascii_case("gzip"))
    })
}

fn gzip(body: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(body)?;
    encoder.finish()
}

impl Default for TrafficController {
    fn default() -> Self {
        Self::new()
    }
}

pub type SharedTrafficController = Arc<TrafficController>;

#[cfg(test)]
mod tests {
    use super::*;

    fn request(url: &str) -> EditableRequest {
        EditableRequest {
            method: "GET".to_owned(),
            url: url.to_owned(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    #[tokio::test]
    async fn mock_rules_match_method_and_url() {
        let controller = TrafficController::new();
        controller
            .save_rule(MockRule {
                id: String::new(),
                enabled: true,
                method: "GET".to_owned(),
                url_pattern: "/api/profile".to_owned(),
                status: 200,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            })
            .await;
        assert!(controller
            .matching_mock(&request("https://example.com/api/profile"))
            .await
            .is_some());
        assert!(controller
            .matching_mock(&request("https://example.com/api/other"))
            .await
            .is_none());
    }

    #[test]
    fn mock_response_gzips_body_and_sets_compressed_length() {
        let rule = MockRule {
            id: String::new(),
            enabled: true,
            method: String::new(),
            url_pattern: "/".to_owned(),
            status: 200,
            headers: vec![
                HeaderEntry {
                    name: "content-encoding".to_owned(),
                    value: "gzip".to_owned(),
                },
                HeaderEntry {
                    name: "content-length".to_owned(),
                    value: "1".to_owned(),
                },
            ],
            body: br#"{"mocked":true}"#.to_vec(),
        };

        let (_, headers, body) = mock_response(&rule);
        assert_ne!(body, rule.body);
        assert_eq!(
            headers
                .iter()
                .filter(|header| header.name.eq_ignore_ascii_case("content-length"))
                .count(),
            1
        );
        assert_eq!(
            headers
                .iter()
                .find(|header| header.name.eq_ignore_ascii_case("content-length"))
                .map(|header| header.value.parse::<usize>().unwrap()),
            Some(body.len())
        );

        let mut decoder = flate2::read::GzDecoder::new(body.as_slice());
        let mut decoded = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut decoded).unwrap();
        assert_eq!(decoded, rule.body);
    }

    #[tokio::test]
    async fn intercepted_request_can_be_forwarded() {
        let controller = Arc::new(TrafficController::new());
        controller.set_intercept_enabled(true).await;
        let waiting = Arc::clone(&controller);
        let task = tokio::spawn(async move { waiting.intercept(request("https://example.com/")) .await });
        tokio::task::yield_now().await;
        let pending = controller.pending().await;
        assert_eq!(pending.len(), 1);
        assert!(controller
            .resolve(
                &pending[0].id,
                InterceptResolution {
                    action: InterceptAction::Forward,
                    request: pending[0].request.clone(),
                },
            )
            .await);
        assert!(matches!(task.await.unwrap().unwrap().action, InterceptAction::Forward));
    }
}
