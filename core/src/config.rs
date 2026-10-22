use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::{collections::HashMap, time::Duration};

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum TimeoutRef {
    Secs(u64),
    Named(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct LayerSpec {
    #[serde(rename = "type")]
    pub name: String,
    pub timeout: Option<TimeoutRef>,
    #[serde(flatten)]
    pub params: toml::Table,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChainConfig {
    pub layers: Vec<LayerSpec>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub inbound: ChainConfig,
    pub outbound: ChainConfig,
    #[serde(default)]
    pub timeouts: HashMap<String, u64>,
}

impl AppConfig {
    pub fn load(path: &str) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("read {path}"))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        Ok(toml::from_str(text)?)
    }

    pub fn resolve_timeout(
        &self,
        t: Option<&TimeoutRef>,
        default: Option<Duration>,
    ) -> Result<Option<Duration>> {
        match t {
            None => Ok(default),
            Some(TimeoutRef::Secs(0)) => Ok(None),
            Some(TimeoutRef::Secs(n)) => Ok(Some(Duration::from_secs(*n))),
            Some(TimeoutRef::Named(name)) => self
                .timeouts
                .get(name)
                .map(|n| Some(Duration::from_secs(*n)))
                .ok_or_else(|| anyhow!("unknown timeout preset '{name}'")),
        }
    }
}
