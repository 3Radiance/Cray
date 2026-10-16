use cray_core::Registry;
use serde::Deserialize;

pub(crate) fn deserialize_from_str<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    T::from_str(&s).map_err(serde::de::Error::custom)
}

mod fingerprint;
mod profiles;
mod proxy;
mod tcp;
mod transport;
mod unix;
mod wrappers;
// mod pad;
mod kit;
mod protocol;

pub fn register(r: &mut Registry) {
    r.add("tcp", tcp::build);
    r.add("unix", unix::build);
    r.add("fragment", wrappers::fragment::build);
    r.add("jitter", wrappers::jitter::build);
    r.add("http-proxy", proxy::http::build);
    r.add("tls", transport::tls::build);
    r.add("http2", transport::http2::build);
    r.add("vless", protocol::vless::build);
    // r.add("pad", pad::build);
}
