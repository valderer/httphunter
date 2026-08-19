use anyhow::bail;
#[cfg(target_os = "macos")]
use anyhow::Context;
use serde::Serialize;
#[cfg(target_os = "macos")]
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
        self.run(&["-setsecurewebproxystate", &self.network_service, "on"])
            .await
    }

    pub async fn disable(&self) -> anyhow::Result<()> {
        self.run(&["-setwebproxystate", &self.network_service, "off"])
            .await?;
        self.run(&["-setsecurewebproxystate", &self.network_service, "off"])
            .await
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

    #[cfg(not(target_os = "macos"))]
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
            bail!(
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
        bail!(
            "networksetup failed for service {}: {}",
            self.network_service,
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }

    #[cfg(not(target_os = "macos"))]
    async fn run(&self, _args: &[&str]) -> anyhow::Result<()> {
        bail!("automatic system proxy control is currently implemented for macOS only")
    }
}
