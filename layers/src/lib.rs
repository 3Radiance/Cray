//! Все слои. Зависит от cray-core, ядро про этот крейт ничего не знает.
//! Новый слой = новый файл + одна строка в `register`.

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

mod http_connect;
mod tcp;
mod unix;
// mod tls;
// mod http2;
mod fragment;
mod jitter;
// mod pad;
mod kit;
mod vless;

pub fn register(r: &mut Registry) {
    r.add("tcp", tcp::build);
    r.add("unix", unix::build);
    r.add("fragment", fragment::build);
    r.add("jitter", jitter::build);
    r.add("http_connect", http_connect::build);
    r.add("vless", vless::build);
    // r.add("pad", pad::build);
}
