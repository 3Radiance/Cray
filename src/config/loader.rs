use crate::config::appconfig::AppConfig;
use anyhow::Result;
use std::sync::OnceLock;

pub static CONFIG: OnceLock<AppConfig> = OnceLock::new();

pub fn load(path: &str) -> Result<()> {
    let config = AppConfig::load(path)?;
    CONFIG
        .set(config)
        .map_err(|_| anyhow::anyhow!("Config already loaded"))
}

pub fn get_config() -> &'static AppConfig {
    CONFIG
        .get()
        .expect("Config is not initialized! Call init_config() first.")
}
