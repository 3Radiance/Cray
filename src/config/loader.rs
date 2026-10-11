use crate::config::appconfig::{AppConfig, Role};
use crate::fingerprint::tls::cert::CertPair;
use anyhow::{Result, anyhow};
use std::sync::OnceLock;

pub static CONFIG: OnceLock<AppConfig> = OnceLock::new();
pub static CA: OnceLock<CertPair> = OnceLock::new();

pub fn load(path: &str) -> Result<()> {
    let config = AppConfig::load(path)?;
    let ca = if config.role == Role::Server {
        let tls_config = config
            .tls
            .as_ref()
            .ok_or_else(|| anyhow!("TLS config is required for Server role"))?;
        let cert_path = tls_config
            .cert
            .as_ref()
            .ok_or_else(|| anyhow!("Cert path missing in TLS config"))?;
        let key_path = tls_config
            .key
            .as_ref()
            .ok_or_else(|| anyhow!("Key path missing in TLS config"))?;

        Some(CertPair::load(cert_path, key_path)?)
    } else {
        None
    };

    if let Some(caa) = ca {
        CA.set(caa)
            .map_err(|_| anyhow::anyhow!("CA already loaded"))?;
    }

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
