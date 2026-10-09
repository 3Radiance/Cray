use crate::config::http2::Http2Config;
use crate::config::tls::TlsConfig;
use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct AppConfig {
    pub tls: TlsConfig,
    pub http2: Http2Config,
}

impl AppConfig {
    pub fn load(path: &str) -> Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let config: AppConfig = toml::from_str(&contents)?;
        Ok(config)
    }
}
