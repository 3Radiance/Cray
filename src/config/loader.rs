use crate::config::appconfig::AppConfig;
use crate::fingerprint::tls::cert::CertPair;
use anyhow::Result;
use std::sync::OnceLock;

pub static CONFIG: OnceLock<AppConfig> = OnceLock::new();
pub static CA: OnceLock<CertPair> = OnceLock::new();

pub fn load(path: &str) -> Result<()> {
    let config = AppConfig::load(path)?;
    let ca = CertPair::load(
        config
            .tls
            .as_ref()
            .expect("TLS config not found")
            .cert
            .as_ref()
            .expect("Cert not found"),
        config
            .tls
            .as_ref()
            .expect("TLS config not found")
            .key
            .as_ref()
            .expect("Key not found"),
    )?;
    CA.set(ca)
        .map_err(|_| anyhow::anyhow!("CA already loaded"))?;
    CONFIG
        .set(config)
        .map_err(|_| anyhow::anyhow!("Config already loaded"))
}

pub fn get_config() -> &'static AppConfig {
    CONFIG
        .get()
        .expect("Config is not initialized! Call init_config() first.")
}

pub fn get_ca() -> &'static CertPair {
    CA.get()
        .expect("CA is not initialized! Call init_config() first.")
}
