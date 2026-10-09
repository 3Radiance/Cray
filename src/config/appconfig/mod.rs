use crate::config::http2::Http2Config;
use crate::config::tls::TlsConfig;
use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Client,
    Server,
}
#[derive(Debug, Deserialize, Clone)]
pub struct InboundConfig {
    pub layers: Vec<LayerConfig>,
}
#[derive(Debug, Deserialize, Clone)]
pub struct OutboundConfig {
    pub layers: Vec<LayerConfig>,
}
#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayerConfig {
    Tcp {
        addr: Option<String>,
    },
    HttpConnect,
    Http2 {
        path: Option<String>,
        host: Option<String>,
    },
    Tsf {
        uuid: Option<String>,
    },
    Tls {
        sni: Option<String>,
    },
}
#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct AppConfig {
    pub role: Role,
    pub inbound: InboundConfig,
    pub outbound: OutboundConfig,

    #[serde(default)]
    pub tls: Option<TlsConfig>,
    #[serde(default)]
    pub http2: Option<Http2Config>,
}
impl AppConfig {
    pub fn load(path: &str) -> Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let config: AppConfig = toml::from_str(&contents)?;
        Ok(config)
    }
}
