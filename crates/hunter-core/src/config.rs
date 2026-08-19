use std::{
    fs,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
};

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub proxy: ProxyConfig,
    pub capture: CaptureConfig,
    pub privacy: PrivacyConfig,
    pub logging: LoggingConfig,
    pub api: ApiConfig,
    pub system_proxy: SystemProxyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxyConfig {
    pub listen: SocketAddr,
    pub mitm: bool,
    pub mitm_exclude: Vec<String>,
    pub connect_timeout_ms: u64,
    pub request_timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureConfig {
    pub enabled: bool,
    pub max_body_bytes: usize,
    pub store_binary: bool,
    pub store_compressed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacyConfig {
    pub redact_headers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    pub enabled: bool,
    pub listen: SocketAddr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SystemProxyConfig {
    pub network_service: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            proxy: ProxyConfig::default(),
            capture: CaptureConfig::default(),
            privacy: PrivacyConfig::default(),
            logging: LoggingConfig::default(),
            api: ApiConfig::default(),
            system_proxy: SystemProxyConfig::default(),
        }
    }
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080),
            mitm: false,
            mitm_exclude: Vec::new(),
            connect_timeout_ms: 10_000,
            request_timeout_ms: 30_000,
        }
    }
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_body_bytes: 10 * 1024 * 1024,
            store_binary: true,
            store_compressed: true,
        }
    }
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            redact_headers: [
                "authorization",
                "cookie",
                "set-cookie",
                "proxy-authorization",
                "x-api-key",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
        }
    }
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9090),
        }
    }
}

impl Default for SystemProxyConfig {
    fn default() -> Self {
        Self {
            network_service: "Wi-Fi".to_owned(),
        }
    }
}

impl AppConfig {
    pub fn load(path: Option<&str>) -> anyhow::Result<Self> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let path = PathBuf::from(path);
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("failed to parse config file {}", path.display()))
    }

    pub fn load_from_path(path: &Path) -> anyhow::Result<Self> {
        Self::load(path.to_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_uses_localhost_proxy_and_api() {
        let config = AppConfig::default();
        assert_eq!(config.proxy.listen, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(config.api.listen, "127.0.0.1:9090".parse().unwrap());
        assert!(config.api.enabled);
    }
}
