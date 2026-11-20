use cray_core::{Factory, Registry};
use inventory;
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
pub mod fingerprint;
pub mod profiles;
pub mod protocol;
pub mod proxy;
pub mod tcp;
pub mod transport;
pub mod tun;
pub mod tun_tcp;
pub mod unix;
pub mod wrappers;

pub struct LayerEntry {
    pub name: &'static str,
    pub factory: Factory,
}

inventory::collect!(LayerEntry);

pub fn register_all(r: &mut Registry) {
    for entry in inventory::iter::<LayerEntry> {
        r.add(entry.name, entry.factory);
    }
}

#[macro_export]
macro_rules! submit_layer {
    ($name:expr, $factory:expr) => {
        $crate::inventory::submit!($crate::LayerEntry {
            name: $name,
            factory: $factory,
        });
    };
}

pub fn register(r: &mut Registry) {
    register_all(r);
}
