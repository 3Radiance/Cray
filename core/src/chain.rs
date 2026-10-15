use crate::api::{BuildCx, Layer, Port};
use crate::config::{AppConfig, LayerSpec};
use crate::cx::Key;
use crate::registry::Registry;
use anyhow::{Result, anyhow, bail, ensure};
use std::{collections::HashSet, time::Duration};

pub struct Entry {
    pub name: String,
    pub layer: Box<dyn Layer>,
    pub timeout: Option<Duration>,
}

pub struct Chain {
    pub entries: Vec<Entry>,
}

pub(crate) fn build_entries(
    specs: &[LayerSpec],
    bcx: &BuildCx,
    reg: &Registry,
    cfg: &AppConfig,
) -> Result<Vec<Entry>> {
    specs
        .iter()
        .map(|s| {
            let f = reg.get(&s.name).ok_or_else(|| {
                anyhow!(
                    "unknown layer '{}' (known: {})",
                    s.name,
                    reg.names().join(", ")
                )
            })?;
            let layer = f(&s.params, bcx).map_err(|e| anyhow!("layer '{}': {e}", s.name))?;
            let timeout = cfg.resolve_timeout(s.timeout.as_ref(), layer.default_timeout())?;
            Ok(Entry {
                name: s.name.clone(),
                layer,
                timeout,
            })
        })
        .collect()
}

fn feeds(gives: Port, takes: Port) -> bool {
    matches!((gives, takes), (Port::Stream | Port::Streams, Port::Stream))
}
pub(crate) fn check_links(entries: &[Entry], label: &str) -> Result<()> {
    let first = entries
        .first()
        .ok_or_else(|| anyhow!("{label}: no layers"))?;
    ensure!(
        first.layer.ports().takes == Port::Nothing,
        "{label}: first layer '{}' must be a source (tcp, unix...)",
        first.name
    );
    for w in entries.windows(2) {
        let (a, b) = (w[0].layer.ports(), w[1].layer.ports());
        ensure!(
            feeds(a.gives, b.takes),
            "{label}: layer '{}' gives {:?}, but '{}' takes {:?}",
            w[0].name,
            a.gives,
            w[1].name,
            b.takes
        );
    }
    Ok(())
}

pub(crate) fn feeds_stream(gives: Port) -> bool {
    feeds(gives, Port::Stream)
}

pub(crate) fn check_requires(
    entries: &[Entry],
    provided: &mut HashSet<Key>,
    label: &str,
) -> Result<()> {
    for e in entries {
        for k in e.layer.requires() {
            if !provided.contains(&k) {
                bail!(
                    "{label}: layer '{}' requires {} but nothing before it provides it",
                    e.name,
                    k.name
                );
            }
        }
        provided.extend(e.layer.provides());
    }
    Ok(())
}
