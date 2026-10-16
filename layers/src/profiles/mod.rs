mod http2;
mod tls;

pub use http2::Http2Config;
pub use tls::TlsConfig;

use crate::fingerprint::http2::builder::http2_builder;
use crate::fingerprint::tls::builder::{set_alpn_select_callback, tls_builder};
use crate::fingerprint::tls::cert::CertPair;
use anyhow::Result;
use btls::ssl::{SslAcceptor, SslConnector, SslMethod};
use cray_core::log_tag;
use dashmap::DashMap;
use std::sync::{Arc, OnceLock};

type CertKey = (String, String, Vec<u8>);
type ProfileKey = String;

fn cert_cache() -> &'static DashMap<CertKey, Arc<SslAcceptor>> {
    static CACHE: OnceLock<DashMap<CertKey, Arc<SslAcceptor>>> = OnceLock::new();
    CACHE.get_or_init(DashMap::new)
}

fn tls_cache() -> &'static DashMap<ProfileKey, Arc<TlsConfig>> {
    static CACHE: OnceLock<DashMap<ProfileKey, Arc<TlsConfig>>> = OnceLock::new();
    CACHE.get_or_init(DashMap::new)
}

fn http2_cache() -> &'static DashMap<ProfileKey, Arc<Http2Config>> {
    static CACHE: OnceLock<DashMap<ProfileKey, Arc<Http2Config>>> = OnceLock::new();
    CACHE.get_or_init(DashMap::new)
}

fn tls_connector_cache() -> &'static DashMap<ProfileKey, Arc<SslConnector>> {
    static CACHE: OnceLock<DashMap<ProfileKey, Arc<SslConnector>>> = OnceLock::new();
    CACHE.get_or_init(DashMap::new)
}

fn http2_builder_cache() -> &'static DashMap<ProfileKey, Arc<::http2::client::Builder>> {
    static CACHE: OnceLock<DashMap<ProfileKey, Arc<::http2::client::Builder>>> = OnceLock::new();
    CACHE.get_or_init(DashMap::new)
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

pub fn get_http2(profile_path: &str) -> Result<Arc<Http2Config>> {
    let cache = http2_cache();
    if let Some(c) = cache.get(profile_path) {
        return Ok(Arc::clone(&c));
    }
    let built = Arc::new(Http2Config::load(profile_path)?);
    cache.insert(profile_path.to_string(), Arc::clone(&built));
    Ok(built)
}

pub async fn get_or_build_tls_connector(profile_path: &str) -> Result<Arc<SslConnector>> {
    let cache = tls_connector_cache();
    if let Some(c) = cache.get(profile_path) {
        log_tag!(
            debug,
            "TLS",
            "Using cached TLS connector for profile {}",
            profile_path
        );
        return Ok(Arc::clone(&c));
    }
    log_tag!(
        debug,
        "TLS",
        "Building TLS connector for profile {}",
        profile_path
    );
    let built = Arc::new(tls_builder(profile_path).await?);
    cache.insert(profile_path.to_string(), Arc::clone(&built));
    Ok(built)
}

pub fn get_or_build_http2(profile_path: &str) -> Result<Arc<::http2::client::Builder>> {
    let cache = http2_builder_cache();
    if let Some(c) = cache.get(profile_path) {
        log_tag!(
            debug,
            "H2",
            "Using cached HTTP/2 builder for profile {}",
            profile_path
        );
        return Ok(Arc::clone(&c));
    }
    log_tag!(
        debug,
        "H2",
        "Building HTTP/2 builder for profile {}",
        profile_path
    );
    let built = Arc::new(http2_builder(profile_path)?);
    cache.insert(profile_path.to_string(), Arc::clone(&built));
    Ok(built)
}

pub fn get_or_load_tls_acceptor(
    cert_path: &str,
    key_path: &str,
    alpn_wire: &[u8],
) -> Result<Arc<SslAcceptor>> {
    let key = (
        cert_path.to_string(),
        key_path.to_string(),
        alpn_wire.to_vec(),
    );
    let cache = cert_cache();

    if let Some(acceptor) = cache.get(&key) {
        log_tag!(
            debug,
            "TLS",
            "Using cached acceptor for cert {} and key {}",
            cert_path,
            key_path
        );
        return Ok(Arc::clone(&acceptor));
    }

    log_tag!(
        debug,
        "TLS",
        "Building TLS acceptor for cert {} and key {}",
        cert_path,
        key_path
    );
    let pair = CertPair::load(cert_path, key_path)?;
    let mut builder = SslAcceptor::mozilla_intermediate(SslMethod::tls())?;

    set_alpn_select_callback(&mut builder, alpn_wire.to_vec());

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
