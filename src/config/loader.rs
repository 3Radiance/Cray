use crate::config::appconfig::AppConfig;
use crate::config::http2::Http2Config;
use crate::config::tls::TlsConfig;
use crate::fingerprint::http2::builder::http2_builder;
use crate::fingerprint::tls::builder::{set_alpn_select_callback, tls_builder};
use crate::fingerprint::tls::cert::CertPair;
use anyhow::Result;
use btls::ssl::{SslAcceptor, SslConnector, SslMethod};
use dashmap::DashMap;
use std::sync::{Arc, OnceLock};

pub static CONFIG: OnceLock<AppConfig> = OnceLock::new();
pub static TLS_CONNECTORS: OnceLock<DashMap<ProfileKey, Arc<SslConnector>>> = OnceLock::new();
pub static HTTP2_CONNECTORS: OnceLock<DashMap<ProfileKey, Arc<http2::client::Builder>>> =
    OnceLock::new();
pub static TLS_PROFILES: OnceLock<DashMap<ProfileKey, Arc<TlsConfig>>> = OnceLock::new();
pub static HTTP2_PROFILES: OnceLock<DashMap<ProfileKey, Arc<Http2Config>>> = OnceLock::new();

type CertKey = (String, String);
type ProfileKey = String;

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

fn tls_cache() -> &'static DashMap<ProfileKey, Arc<TlsConfig>> {
    TLS_PROFILES.get_or_init(DashMap::new)
}

fn http2_cache() -> &'static DashMap<ProfileKey, Arc<Http2Config>> {
    HTTP2_PROFILES.get_or_init(DashMap::new)
}

pub fn get_or_load_tls_acceptor(
    cert_path: &str,
    key_path: &str,
    config_path: &str,
) -> Result<Arc<SslAcceptor>> {
    let key = (cert_path.to_string(), key_path.to_string());
    let cache = global_cert_cache();

    if let Some(acceptor) = cache.get(&key) {
        return Ok(Arc::clone(&acceptor));
    }

    let pair = CertPair::load(cert_path, key_path)?;
    let tls_config = get_tls(config_path)?;
    let mut builder = SslAcceptor::mozilla_intermediate(SslMethod::tls())?;

    set_alpn_select_callback(&mut builder, tls_config.encode_alpn_wire());

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

fn tls_connector_cache() -> &'static DashMap<ProfileKey, Arc<SslConnector>> {
    TLS_CONNECTORS.get_or_init(DashMap::new)
}

pub async fn get_or_build_tls_connector(profile_path: &str) -> Result<Arc<SslConnector>> {
    let cache = tls_connector_cache();
    if let Some(c) = cache.get(profile_path) {
        return Ok(Arc::clone(&c));
    }
    let built = Arc::new(tls_builder(profile_path).await?);
    cache.insert(profile_path.to_string(), Arc::clone(&built));
    Ok(built)
}

fn get_http2_connector_cache() -> &'static DashMap<ProfileKey, Arc<http2::client::Builder>> {
    HTTP2_CONNECTORS.get_or_init(DashMap::new)
}

pub fn get_or_build_http2(profile_path: &str) -> Result<Arc<http2::client::Builder>> {
    let cache = get_http2_connector_cache();
    if let Some(c) = cache.get(profile_path) {
        return Ok(Arc::clone(&c));
    }
    let built = Arc::new(http2_builder(profile_path)?);
    cache.insert(profile_path.to_string(), Arc::clone(&built));
    Ok(built)
}

pub fn get_http2(profile_path: &str) -> Result<Arc<Http2Config>> {
    let cache = http2_cache();
    if let Some(c) = cache.get(profile_path) {
        return Ok(Arc::clone(&c));
    }
    let built = Arc::new(Http2Config::load(profile_path)?);
    cache.insert(profile_path.to_string(), Arc::clone(&built));
    Ok(built)
}

pub fn get_tls(profile_path: &str) -> Result<Arc<TlsConfig>> {
    let cache = tls_cache();
    if let Some(c) = cache.get(profile_path) {
        return Ok(Arc::clone(&c));
    }
    let built = Arc::new(TlsConfig::load(profile_path)?);
    cache.insert(profile_path.to_string(), Arc::clone(&built));
    Ok(built)
}
