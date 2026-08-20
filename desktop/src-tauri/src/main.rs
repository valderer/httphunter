use std::{
    io::Read,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    sync::Arc,
};

#[cfg(target_os = "macos")]
use std::process::Command;

use flate2::read::GzDecoder;
use hunter_core::{
    AppConfig, ApplicationInfo, CaStore, CaptureRuntimeStatus, EditableRequest, HttpSession,
    HunterRuntime, InterceptResolution, MockRule, PendingIntercept, ReplayResult, SessionSummary,
    SystemProxyController, SystemProxyStatus,
};
use serde::Serialize;
use tauri::State;
use tokio::sync::Mutex;

#[derive(Serialize)]
struct CertificateInfo {
    path: String,
    exists: bool,
}

#[derive(Serialize)]
struct MobileCaptureStatus {
    enabled: bool,
    listen: String,
    lan_addresses: Vec<String>,
}

struct RuntimeController {
    config: AppConfig,
    runtime: Arc<HunterRuntime>,
    mobile_capture: bool,
}

struct DesktopState {
    runtime: Mutex<RuntimeController>,
    system_proxy: SystemProxyController,
}

impl DesktopState {
    fn new() -> Self {
        let mut config = AppConfig::default();
        config.capture.enabled = true;
        config.proxy.mitm = true;
        let runtime = Arc::new(HunterRuntime::new(
            config.clone(),
            config.proxy.listen,
            config.proxy.mitm,
        ));
        Self {
            runtime: Mutex::new(RuntimeController {
                config: config.clone(),
                runtime,
                mobile_capture: false,
            }),
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
    let controller = state.runtime.lock().await;
    Ok(controller.runtime.status().await)
}

#[tauri::command]
async fn start_capture(state: State<'_, DesktopState>) -> Result<CaptureRuntimeStatus, String> {
    let controller = state.runtime.lock().await;
    controller.runtime.set_capture_enabled(true);
    let status = controller
        .runtime
        .start()
        .await
        .map_err(|error| error.to_string())?;
    if let Err(error) = state.system_proxy.enable().await {
        let _ = controller.runtime.stop().await;
        return Err(format!(
            "proxy started but system proxy could not be enabled: {error}"
        ));
    }
    Ok(status)
}

#[tauri::command]
async fn stop_capture(state: State<'_, DesktopState>) -> Result<CaptureRuntimeStatus, String> {
    let proxy_error = state.system_proxy.disable().await.err();
    let controller = state.runtime.lock().await;
    controller.runtime.set_capture_enabled(false);
    let runtime_status = controller
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
    let controller = state.runtime.lock().await;
    Ok(controller.runtime.session_count().await)
}

#[tauri::command]
async fn clear_sessions(state: State<'_, DesktopState>) -> Result<(), String> {
    let controller = state.runtime.lock().await;
    controller.runtime.clear_sessions().await;
    Ok(())
}

#[tauri::command]
async fn list_sessions(state: State<'_, DesktopState>) -> Result<Vec<SessionSummary>, String> {
    let controller = state.runtime.lock().await;
    Ok(controller.runtime.session_summaries().await)
}

#[tauri::command]
async fn get_session(
    id: String,
    state: State<'_, DesktopState>,
) -> Result<Option<HttpSession>, String> {
    let controller = state.runtime.lock().await;
    Ok(controller
        .runtime
        .session(&id)
        .await
        .map(decode_response_body))
}

#[tauri::command]
async fn replay_request(
    request: EditableRequest,
    state: State<'_, DesktopState>,
) -> Result<ReplayResult, String> {
    let controller = state.runtime.lock().await;
    Ok(controller.runtime.replay(request).await)
}

#[tauri::command]
async fn intercept_enabled(state: State<'_, DesktopState>) -> Result<bool, String> {
    let controller = state.runtime.lock().await;
    Ok(controller.runtime.traffic().intercept_enabled().await)
}

#[tauri::command]
async fn set_intercept_enabled(
    enabled: bool,
    state: State<'_, DesktopState>,
) -> Result<(), String> {
    let controller = state.runtime.lock().await;
    controller
        .runtime
        .traffic()
        .set_intercept_enabled(enabled)
        .await;
    Ok(())
}

#[tauri::command]
async fn pending_intercepts(
    state: State<'_, DesktopState>,
) -> Result<Vec<PendingIntercept>, String> {
    let controller = state.runtime.lock().await;
    Ok(controller.runtime.traffic().pending().await)
}

#[tauri::command]
async fn resolve_intercept(
    id: String,
    resolution: InterceptResolution,
    state: State<'_, DesktopState>,
) -> Result<bool, String> {
    let controller = state.runtime.lock().await;
    Ok(controller.runtime.traffic().resolve(&id, resolution).await)
}

#[tauri::command]
async fn list_mock_rules(state: State<'_, DesktopState>) -> Result<Vec<MockRule>, String> {
    let controller = state.runtime.lock().await;
    Ok(controller.runtime.traffic().rules().await)
}

#[tauri::command]
async fn save_mock_rule(
    rule: MockRule,
    state: State<'_, DesktopState>,
) -> Result<MockRule, String> {
    let controller = state.runtime.lock().await;
    Ok(controller.runtime.traffic().save_rule(rule).await)
}

#[tauri::command]
async fn delete_mock_rule(id: String, state: State<'_, DesktopState>) -> Result<bool, String> {
    let controller = state.runtime.lock().await;
    Ok(controller.runtime.traffic().delete_rule(&id).await)
}

#[tauri::command]
async fn mobile_capture_status(
    state: State<'_, DesktopState>,
) -> Result<MobileCaptureStatus, String> {
    let controller = state.runtime.lock().await;
    Ok(mobile_status(&controller))
}

#[tauri::command]
async fn set_mobile_capture(
    enabled: bool,
    state: State<'_, DesktopState>,
) -> Result<MobileCaptureStatus, String> {
    let mut controller = state.runtime.lock().await;
    if controller.mobile_capture == enabled {
        return Ok(mobile_status(&controller));
    }

    let was_running = controller.runtime.status().await.running;
    if was_running {
        controller
            .runtime
            .stop()
            .await
            .map_err(|error| format!("failed to stop the current proxy: {error}"))?;
    }

    let mut next_config = controller.config.clone();
    next_config.proxy.listen = proxy_listen_address(enabled);
    next_config.proxy.allow_lan_clients = enabled;
    let next_runtime = Arc::new(HunterRuntime::with_store_and_traffic(
        next_config.clone(),
        next_config.proxy.listen,
        next_config.proxy.mitm,
        controller.runtime.store(),
        controller.runtime.traffic(),
    ));

    if was_running {
        if let Err(error) = next_runtime.start().await {
            let restore_error = controller.runtime.start().await.err();
            let detail = restore_error
                .map(|restore| format!("; failed to restore the original proxy: {restore}"))
                .unwrap_or_default();
            return Err(format!("failed to switch proxy listener: {error}{detail}"));
        }
    }

    controller.config = next_config;
    controller.runtime = next_runtime;
    controller.mobile_capture = enabled;
    Ok(mobile_status(&controller))
}

fn proxy_listen_address(allow_lan_clients: bool) -> SocketAddr {
    let ip = if allow_lan_clients {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    } else {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    };
    SocketAddr::new(ip, 8080)
}

fn mobile_status(controller: &RuntimeController) -> MobileCaptureStatus {
    MobileCaptureStatus {
        enabled: controller.mobile_capture,
        listen: controller.config.proxy.listen.to_string(),
        lan_addresses: local_network_addresses(),
    }
}

fn local_network_addresses() -> Vec<String> {
    #[cfg(target_os = "macos")]
    if let Some(address) = macos_wifi_address() {
        return vec![address];
    }

    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
        return Vec::new();
    };
    if socket.connect("1.1.1.1:80").is_err() {
        return Vec::new();
    }
    match socket.local_addr().ok().map(|address| address.ip()) {
        Some(IpAddr::V4(address)) if address.is_private() || address.is_link_local() => {
            vec![address.to_string()]
        }
        _ => Vec::new(),
    }
}

#[cfg(target_os = "macos")]
fn macos_wifi_address() -> Option<String> {
    let output = Command::new("/usr/sbin/networksetup")
        .args(["-listallhardwareports"])
        .output()
        .ok()?;
    let hardware_ports = String::from_utf8(output.stdout).ok()?;
    let wifi_device = hardware_ports.split("\n\n").find_map(|section| {
        section
            .lines()
            .any(|line| line.trim() == "Hardware Port: Wi-Fi")
            .then(|| {
                section
                    .lines()
                    .find_map(|line| line.trim().strip_prefix("Device: ").map(str::to_owned))
            })
            .flatten()
    })?;
    let output = Command::new("/usr/sbin/ipconfig")
        .args(["getifaddr", &wifi_device])
        .output()
        .ok()?;
    let address = String::from_utf8(output.stdout).ok()?.trim().parse().ok()?;
    let IpAddr::V4(address) = address else {
        return None;
    };
    (address.is_private() || address.is_link_local()).then(|| address.to_string())
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
            get_session,
            replay_request,
            intercept_enabled,
            set_intercept_enabled,
            pending_intercepts,
            resolve_intercept,
            list_mock_rules,
            save_mock_rule,
            delete_mock_rule,
            mobile_capture_status,
            set_mobile_capture
        ])
        .run(tauri::generate_context!())
        .expect("failed to run httphunter desktop application");
}
