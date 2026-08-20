#[cfg(any(target_os = "macos", target_os = "windows"))]
use anyhow::Context;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use anyhow::bail;
use serde::Serialize;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use tokio::process::Command;

use crate::SystemProxyConfig;

#[derive(Debug, Clone, Serialize)]
pub struct SystemProxyStatus {
    pub supported: bool,
    pub enabled: bool,
    pub network_service: String,
}

#[derive(Debug, Clone)]
pub struct SystemProxyController {
    network_service: String,
    host: String,
    port: u16,
}

impl SystemProxyController {
    pub fn new(config: &SystemProxyConfig, host: String, port: u16) -> Self {
        Self {
            network_service: config.network_service.clone(),
            host,
            port,
        }
    }

    pub fn network_service(&self) -> &str {
        &self.network_service
    }

    pub async fn enable(&self) -> anyhow::Result<()> {
        #[cfg(target_os = "macos")]
        {
            self.run(&[
                "-setwebproxy",
                &self.network_service,
                &self.host,
                &self.port.to_string(),
            ])
            .await?;
            self.run(&[
                "-setsecurewebproxy",
                &self.network_service,
                &self.host,
                &self.port.to_string(),
            ])
            .await?;
            self.run(&["-setwebproxystate", &self.network_service, "on"])
                .await?;
            return self
                .run(&["-setsecurewebproxystate", &self.network_service, "on"])
                .await;
        }

        #[cfg(target_os = "windows")]
        {
            self.write_windows_value("ProxyServer", "REG_SZ", &self.windows_proxy_server())
                .await?;
            return self
                .write_windows_value("ProxyEnable", "REG_DWORD", "1")
                .await;
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        bail!("automatic system proxy control is not supported on this operating system")
    }

    pub async fn disable(&self) -> anyhow::Result<()> {
        #[cfg(target_os = "macos")]
        {
            self.run(&["-setwebproxystate", &self.network_service, "off"])
                .await?;
            return self
                .run(&["-setsecurewebproxystate", &self.network_service, "off"])
                .await;
        }

        #[cfg(target_os = "windows")]
        {
            return self
                .write_windows_value("ProxyEnable", "REG_DWORD", "0")
                .await;
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        bail!("automatic system proxy control is not supported on this operating system")
    }

    #[cfg(target_os = "macos")]
    pub async fn status(&self) -> anyhow::Result<SystemProxyStatus> {
        let web = self.read_proxy_state("-getwebproxy").await?;
        let secure_web = self.read_proxy_state("-getsecurewebproxy").await?;
        Ok(SystemProxyStatus {
            supported: true,
            enabled: web && secure_web,
            network_service: self.network_service.clone(),
        })
    }

    #[cfg(target_os = "windows")]
    pub async fn status(&self) -> anyhow::Result<SystemProxyStatus> {
        Ok(SystemProxyStatus {
            supported: true,
            enabled: self.read_windows_proxy_enabled().await?,
            network_service: "Windows user proxy".to_owned(),
        })
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    pub async fn status(&self) -> anyhow::Result<SystemProxyStatus> {
        Ok(SystemProxyStatus {
            supported: false,
            enabled: false,
            network_service: self.network_service.clone(),
        })
    }

    #[cfg(target_os = "macos")]
    async fn read_proxy_state(&self, option: &str) -> anyhow::Result<bool> {
        let output = Command::new("/usr/sbin/networksetup")
            .args([option, &self.network_service])
            .output()
            .await
            .context("failed to run macOS networksetup")?;
        if !output.status.success() {
            anyhow::bail!(
                "networksetup failed for service {}: {}",
                self.network_service,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).contains("Enabled: Yes"))
    }

    #[cfg(target_os = "macos")]
    async fn run(&self, args: &[&str]) -> anyhow::Result<()> {
        let output = Command::new("/usr/sbin/networksetup")
            .args(args)
            .output()
            .await
            .context("failed to run macOS networksetup")?;
        if output.status.success() {
            return Ok(());
        }
        anyhow::bail!(
            "networksetup failed for service {}: {}",
            self.network_service,
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }

    #[cfg(target_os = "windows")]
    fn windows_proxy_server(&self) -> String {
        format!(
            "http={host}:{port};https={host}:{port}",
            host = self.host,
            port = self.port
        )
    }

    #[cfg(target_os = "windows")]
    async fn write_windows_value(&self, name: &str, value_type: &str, value: &str) -> anyhow::Result<()> {
        let output = Command::new("reg.exe")
            .args([
                "add",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
                "/v",
                name,
                "/t",
                value_type,
                "/d",
                value,
                "/f",
            ])
            .output()
            .await
            .context("failed to run Windows reg.exe")?;
        if output.status.success() {
            return Ok(());
        }
        anyhow::bail!(
            "failed to update Windows system proxy: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }

    #[cfg(target_os = "windows")]
    async fn read_windows_proxy_enabled(&self) -> anyhow::Result<bool> {
        let output = Command::new("reg.exe")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
                "/v",
                "ProxyEnable",
            ])
            .output()
            .await
            .context("failed to run Windows reg.exe")?;
        if !output.status.success() {
            return Ok(false);
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .any(|value| value.eq_ignore_ascii_case("0x1")))
    }
}
