use crate::config::appconfig::{AppConfig, Role};
use crate::fingerprint::tls::builder::set_alpn_select_callback;
use crate::fingerprint::tls::cert::CertPair;
use anyhow::{Result, anyhow};
use btls::ssl::{SslAcceptor, SslMethod};
use dashmap::DashMap;
use std::sync::{Arc, OnceLock};

pub static CONFIG: OnceLock<AppConfig> = OnceLock::new();
type CertKey = (String, String);

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

fn global_cert_cache() -> &'static DashMap<CertKey, Arc<SslAcceptor>> {
    static CACHE: OnceLock<DashMap<CertKey, Arc<SslAcceptor>>> = OnceLock::new();
    CACHE.get_or_init(DashMap::new)
}

pub fn get_or_load_acceptor(cert_path: &str, key_path: &str) -> Result<Arc<SslAcceptor>> {
    let key = (cert_path.to_string(), key_path.to_string());
    let cache = global_cert_cache();
    let loader = get_config();

    if let Some(acceptor) = cache.get(&key) {
        return Ok(Arc::clone(&acceptor));
    }

    let pair = CertPair::load(cert_path, key_path)?;
    let mut builder = SslAcceptor::mozilla_intermediate(SslMethod::tls())?;
    if let Some(tls) = &loader.tls {
        set_alpn_select_callback(&mut builder, tls.encode_alpn_wire());
    } else {
        return Err(anyhow!("TLS configuration not found!"));
    }

    if let Some(leaf) = pair.chain.first() {
        builder.set_certificate(leaf)?;
    }

    for cert in pair.chain.iter().skip(1) {
        builder.add_extra_chain_cert(cert.clone())?;
    }

    builder.set_private_key(&pair.key)?;
    builder.check_private_key()?;

    let acceptor = Arc::new(builder.build());
    cache.insert(key, Arc::clone(&acceptor));

    Ok(acceptor)
}
