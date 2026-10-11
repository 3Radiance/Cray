use crate::config::appconfig::AppConfig;
use crate::fingerprint::tls::cert::CertPair;
use crate::log_tag;
use anyhow::Result;
use std::sync::OnceLock;

pub static CONFIG: OnceLock<AppConfig> = OnceLock::new();
pub static CA: OnceLock<CertPair> = OnceLock::new();

pub fn load(path: &str) -> Result<()> {
    let config = AppConfig::load(path)?;
    let ca = if let Some(tls_config) = config.tls.as_ref() {
        if let (Some(cert_path), Some(key_path)) =
            (tls_config.cert.as_ref(), tls_config.key.as_ref())
        {
            match CertPair::load(cert_path, key_path) {
                Ok(pair) => Some(pair),
                Err(e) => {
                    log_tag!(warn, "TLS", "Failed to load certificates: {}", e);
                    None
                }
            }
        } else {
            log_tag!(warn, "TLS", "Cert or key path missing in TLS config");
            None
        }
    } else {
        log_tag!(
            warn,
            "TLS",
            "TLS config not found, skipping certificate loading"
        );
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
