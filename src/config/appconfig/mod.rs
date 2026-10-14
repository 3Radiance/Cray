use crate::transport::fragment::FragStage;
use crate::transport::jitter::JitterStage;
use anyhow::{Result, anyhow, ensure};
use serde::Deserialize;
use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

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
        timeout: Option<TimeoutRef>,
    },
    HttpConnect,
    Http2 {
        path: Option<String>,
        host: Option<String>,
        method: Option<String>,
        config: Option<String>,
        timeout: Option<TimeoutRef>,
    },
    Vless {
        uuid: Option<String>,
    },
    Tls {
        sni: Option<String>,
        cert: Option<String>,
        key: Option<String>,
        tls_path: Option<String>,
        http2_path: Option<String>,
        timeout: Option<TimeoutRef>,
    },
    Unix {
        addr: Option<String>,
        timeout: Option<TimeoutRef>,
    },
    Jitter {
        stages: Option<Vec<JitterStage>>,
    },
    Fragment {
        stages: Option<Vec<FragStage>>,
    },
}
impl LayerConfig {
    pub fn is_protocol(&self) -> bool {
        matches!(self, LayerConfig::Vless { .. })
    }
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct AppConfig {
    pub role: Role,
    pub inbound: InboundConfig,
    pub outbound: OutboundConfig,
    #[serde(default)]
    pub timeouts: HashMap<String, u64>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum TimeoutRef {
    Secs(u64),
    Named(String),
}

impl AppConfig {
    pub fn load(path: &str) -> Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let config: AppConfig = toml::from_str(&contents)?;
        Ok(config)
    }

    pub fn resolve_timeout(
        &self,
        field: Option<&TimeoutRef>,
        default_secs: u64,
    ) -> Result<Duration> {
        let secs = match field {
            None => default_secs,
            Some(TimeoutRef::Secs(s)) => *s,
            Some(TimeoutRef::Named(name)) => *self
                .timeouts
                .get(name)
                .ok_or_else(|| anyhow!("unknown timeout preset `{name}`"))?,
        };
        ensure!(secs > 0, "timeout must be above 0 seconds");
        Ok(Duration::from_secs(secs))
    }

    pub fn layer_timeout(layer: &LayerConfig) -> Option<&TimeoutRef> {
        match layer {
            LayerConfig::Tcp { timeout, .. }
            | LayerConfig::Unix { timeout, .. }
            | LayerConfig::Tls { timeout, .. }
            | LayerConfig::Http2 { timeout, .. } => timeout.as_ref(),
            _ => None,
        }
    }
}

pub async fn with_timeout<T, F>(what: &str, dur: Duration, fut: F) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    match tokio::time::timeout(dur, fut).await {
        Ok(res) => res,
        Err(_) => Err(anyhow!("{what} timed out after {}s", dur.as_secs())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(timeouts: &[(&str, u64)]) -> AppConfig {
        AppConfig {
            role: Role::Client,
            inbound: InboundConfig { layers: vec![] },
            outbound: OutboundConfig { layers: vec![] },
            timeouts: timeouts
                .iter()
                .map(|(k, v)| (k.to_string(), *v))
                .collect(),
        }
    }

    #[test]
    fn timeout_ref_from_int_and_string() {
        #[derive(Deserialize)]
        struct W {
            timeout: TimeoutRef,
        }
        let w: W = toml::from_str("timeout = 5").unwrap();
        assert!(matches!(w.timeout, TimeoutRef::Secs(5)));
        let w: W = toml::from_str(r#"timeout = "tls""#).unwrap();
        assert!(matches!(w.timeout, TimeoutRef::Named(ref n) if n == "tls"));
    }

    #[test]
    fn resolve_defaults_and_presets() {
        let cfg = cfg_with(&[("tls", 10)]);
        assert_eq!(
            cfg.resolve_timeout(None, 6).unwrap(),
            Duration::from_secs(6)
        );
        assert_eq!(
            cfg.resolve_timeout(Some(&TimeoutRef::Secs(3)), 6)
                .unwrap(),
            Duration::from_secs(3)
        );
        assert_eq!(
            cfg.resolve_timeout(Some(&TimeoutRef::Named("tls".into())), 6)
                .unwrap(),
            Duration::from_secs(10)
        );
        assert!(cfg
            .resolve_timeout(Some(&TimeoutRef::Named("nope".into())), 6)
            .is_err());
        assert!(cfg.resolve_timeout(Some(&TimeoutRef::Secs(0)), 6).is_err());
    }

    #[test]
    fn full_layer_config_parses() {
        let cfg: AppConfig = toml::from_str(
            r#"
role = "client"
[timeouts]
tls = 10
[[inbound.layers]]
type = "tcp"
addr = "127.0.0.1:10810"
[[inbound.layers]]
type = "http_connect"
[[outbound.layers]]
type = "tcp"
addr = "127.0.0.1:10811"
timeout = "tls"
[[outbound.layers]]
type = "tls"
sni = "example.com"
tls_path = "./tls.toml"
http2_path = "./http2.toml"
timeout = 7
[[outbound.layers]]
type = "vless"
uuid = "11111111-1111-4111-8111-111111111111"
"#,
        )
        .unwrap();
        assert_eq!(cfg.timeouts["tls"], 10);
        assert_eq!(
            cfg.resolve_timeout(AppConfig::layer_timeout(&cfg.outbound.layers[0]), 5)
                .unwrap(),
            Duration::from_secs(10)
        );
        assert_eq!(
            cfg.resolve_timeout(AppConfig::layer_timeout(&cfg.outbound.layers[1]), 6)
                .unwrap(),
            Duration::from_secs(7)
        );
    }
}
