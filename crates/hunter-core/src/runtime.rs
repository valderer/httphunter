use std::{net::SocketAddr, sync::Arc};

use anyhow::Context;
use serde::Serialize;
use tokio::{
    net::TcpListener,
    sync::{watch, Mutex},
    task::JoinHandle,
};

use crate::{proxy_engine, AppConfig, HttpSession, MemoryStore, SessionSummary};

#[derive(Debug, Clone, Serialize)]
pub struct CaptureRuntimeStatus {
    pub running: bool,
    pub listen: SocketAddr,
    pub mitm_enabled: bool,
    pub capture_enabled: bool,
}

struct RuntimeState {
    shutdown: Option<watch::Sender<bool>>,
    task: Option<JoinHandle<anyhow::Result<()>>>,
}

impl RuntimeState {
    fn stopped() -> Self {
        Self {
            shutdown: None,
            task: None,
        }
    }
}

pub struct HunterRuntime {
    config: AppConfig,
    listen: SocketAddr,
    mitm_enabled: bool,
    store: Arc<MemoryStore>,
    state: Mutex<RuntimeState>,
}

impl HunterRuntime {
    pub fn new(config: AppConfig, listen: SocketAddr, mitm_enabled: bool) -> Self {
        let store = Arc::new(MemoryStore::new(config.capture.enabled));
        Self {
            config,
            listen,
            mitm_enabled,
            store,
            state: Mutex::new(RuntimeState::stopped()),
        }
    }

    pub fn store(&self) -> Arc<MemoryStore> {
        Arc::clone(&self.store)
    }

    pub async fn start(&self) -> anyhow::Result<CaptureRuntimeStatus> {
        let mut state = self.state.lock().await;
        if state.task.is_some() {
            return Ok(self.status_from(true));
        }

        let listener = TcpListener::bind(self.listen)
            .await
            .with_context(|| format!("failed to bind proxy listener at {}", self.listen))?;
        let (shutdown, shutdown_rx) = watch::channel(false);
        let config = self.config.clone();
        let store = Arc::clone(&self.store);
        let mitm_enabled = self.mitm_enabled;
        let task = tokio::spawn(async move {
            proxy_engine::serve(listener, config, mitm_enabled, store, shutdown_rx).await
        });
        state.shutdown = Some(shutdown);
        state.task = Some(task);
        Ok(self.status_from(true))
    }

    pub async fn stop(&self) -> anyhow::Result<CaptureRuntimeStatus> {
        let (shutdown, task) = {
            let mut state = self.state.lock().await;
            (state.shutdown.take(), state.task.take())
        };

        if let Some(shutdown) = shutdown {
            let _ = shutdown.send(true);
        }
        if let Some(task) = task {
            task.await.context("proxy runtime task panicked")??;
        }
        Ok(self.status_from(false))
    }

    pub async fn status(&self) -> CaptureRuntimeStatus {
        let state = self.state.lock().await;
        self.status_from(state.task.is_some())
    }

    pub async fn sessions(&self) -> Vec<HttpSession> {
        self.store.list().await
    }

    pub async fn session_summaries(&self) -> Vec<SessionSummary> {
        self.store.summaries().await
    }

    pub async fn session(&self, id: &str) -> Option<HttpSession> {
        let id = id.parse().ok()?;
        self.store.get(id).await
    }

    pub async fn session_count(&self) -> usize {
        self.store.len().await
    }

    pub async fn clear_sessions(&self) {
        self.store.clear().await;
    }

    pub fn set_capture_enabled(&self, enabled: bool) {
        self.store.set_enabled(enabled);
    }

    fn status_from(&self, running: bool) -> CaptureRuntimeStatus {
        CaptureRuntimeStatus {
            running,
            listen: self.listen,
            mitm_enabled: self.mitm_enabled,
            capture_enabled: self.store.is_enabled(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runtime_reports_its_initial_stopped_state() {
        let runtime = HunterRuntime::new(
            AppConfig::default(),
            "127.0.0.1:18080".parse().unwrap(),
            true,
        );
        let status = runtime.status().await;
        assert!(!status.running);
        assert!(status.mitm_enabled);
        assert!(!status.capture_enabled);
    }
}
