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

pub(crate) fn deserialize_opt_from_str<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
    D: serde::Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    opt.map(|s| T::from_str(&s).map_err(serde::de::Error::custom))
        .transpose()
}

pub mod crypto;
mod fingerprint;
mod profiles;
mod protocol;
mod proxy;
mod tcp;
mod transport;
mod unix;
mod wrappers;

pub fn register(r: &mut Registry) {
    r.add("tcp", tcp::build);
    r.add("unix", unix::build);
    r.add("fragment", wrappers::fragment::build);
    r.add("jitter", wrappers::jitter::build);
    r.add("http-proxy", proxy::http::build);
    r.add("tls", transport::tls::build);
    r.add("http2", transport::http2::build);
    r.add("vless", protocol::vless::build);
    r.add("pad", wrappers::pad::build);
    r.add("auth", wrappers::auth::build);
    r.add("xhttp2", transport::xhttp2::build);
    // r.add("xhttp", transport::xhttp::build);
    // r.add("byedpi", wrappers::byedpi::build);
    // r.add("direct", protocol::direct::build);
    // r.add("split", wrappers::split::build);
}
