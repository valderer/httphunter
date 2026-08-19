use std::{io::Read, sync::Arc};

use flate2::read::GzDecoder;
use hunter_core::{
    AppConfig, ApplicationInfo, CaStore, CaptureRuntimeStatus, HttpSession, HunterRuntime,
    SessionSummary, SystemProxyController, SystemProxyStatus,
};
use serde::Serialize;
use tauri::State;

#[derive(Serialize)]
struct CertificateInfo {
    path: String,
    exists: bool,
}

struct DesktopState {
    runtime: Arc<HunterRuntime>,
    system_proxy: SystemProxyController,
}

impl DesktopState {
    fn new() -> Self {
        let mut config = AppConfig::default();
        config.capture.enabled = true;
        config.proxy.mitm = true;
        let runtime = HunterRuntime::new(config.clone(), config.proxy.listen, config.proxy.mitm);
        Self {
            runtime: Arc::new(runtime),
            system_proxy: SystemProxyController::new(
                &config.system_proxy,
                config.proxy.listen.ip().to_string(),
                config.proxy.listen.port(),
            ),
        }
    }
}

#[tauri::command]
fn application_info() -> ApplicationInfo {
    ApplicationInfo::current(env!("CARGO_PKG_VERSION"))
}

#[tauri::command]
fn certificate_info() -> Result<CertificateInfo, String> {
    let store = CaStore::default().map_err(|error| error.to_string())?;
    Ok(CertificateInfo {
        path: store.cert_path().display().to_string(),
        exists: store.cert_path().is_file(),
    })
}

#[tauri::command]
fn generate_certificate() -> Result<CertificateInfo, String> {
    let store = CaStore::default().map_err(|error| error.to_string())?;
    store.generate(false).map_err(|error| error.to_string())?;
    Ok(CertificateInfo {
        path: store.cert_path().display().to_string(),
        exists: true,
    })
}

#[tauri::command]
async fn capture_status(state: State<'_, DesktopState>) -> Result<CaptureRuntimeStatus, String> {
    Ok(state.runtime.status().await)
}

#[tauri::command]
async fn start_capture(state: State<'_, DesktopState>) -> Result<CaptureRuntimeStatus, String> {
    state.runtime.set_capture_enabled(true);
    let status = state
        .runtime
        .start()
        .await
        .map_err(|error| error.to_string())?;
    if let Err(error) = state.system_proxy.enable().await {
        let _ = state.runtime.stop().await;
        return Err(format!(
            "proxy started but system proxy could not be enabled: {error}"
        ));
    }
    Ok(status)
}

#[tauri::command]
async fn stop_capture(state: State<'_, DesktopState>) -> Result<CaptureRuntimeStatus, String> {
    let proxy_error = state.system_proxy.disable().await.err();
    let runtime_status = state
        .runtime
        .stop()
        .await
        .map_err(|error| error.to_string())?;
    if let Some(error) = proxy_error {
        return Err(format!(
            "capture stopped but system proxy could not be disabled: {error}"
        ));
    }
    Ok(runtime_status)
}

#[tauri::command]
async fn system_proxy_status(state: State<'_, DesktopState>) -> Result<SystemProxyStatus, String> {
    state
        .system_proxy
        .status()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn session_count(state: State<'_, DesktopState>) -> Result<usize, String> {
    Ok(state.runtime.session_count().await)
}

#[tauri::command]
async fn clear_sessions(state: State<'_, DesktopState>) -> Result<(), String> {
    state.runtime.clear_sessions().await;
    Ok(())
}

#[tauri::command]
async fn list_sessions(state: State<'_, DesktopState>) -> Result<Vec<SessionSummary>, String> {
    Ok(state.runtime.session_summaries().await)
}

#[tauri::command]
async fn get_session(
    id: String,
    state: State<'_, DesktopState>,
) -> Result<Option<HttpSession>, String> {
    Ok(state.runtime.session(&id).await.map(decode_response_body))
}

fn decode_response_body(mut session: HttpSession) -> HttpSession {
    if has_gzip_content_encoding(&session.response.headers) {
        if let Some(body) = decompress_gzip(&session.response.body) {
            session.response.body = body;
        }
    }
    session
}

fn has_gzip_content_encoding(headers: &[hunter_core::HeaderEntry]) -> bool {
    headers.iter().any(|header| {
        header.name.eq_ignore_ascii_case("content-encoding")
            && header
                .value
                .split(',')
                .any(|encoding| encoding.trim().eq_ignore_ascii_case("gzip"))
    })
}

fn decompress_gzip(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut decoder = GzDecoder::new(bytes);
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed).ok()?;
    Some(decompressed)
}

fn main() {
    tauri::Builder::default()
        .manage(DesktopState::new())
        .invoke_handler(tauri::generate_handler![
            application_info,
            certificate_info,
            generate_certificate,
            capture_status,
            system_proxy_status,
            start_capture,
            stop_capture,
            session_count,
            clear_sessions,
            list_sessions,
            get_session
        ])
        .run(tauri::generate_context!())
        .expect("failed to run httphunter desktop application");
}
