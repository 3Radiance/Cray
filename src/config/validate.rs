use crate::config::appconfig::{AppConfig, LayerConfig, Role};
use anyhow::{Result, bail};

#[derive(Clone, Copy)]
enum Proto {
    Required,
    Forbidden,
    Optional,
}

fn check(name: &str, layers: &[LayerConfig], proto: Proto) -> Result<()> {
    match layers.first() {
        Some(LayerConfig::Tcp { .. } | LayerConfig::Unix { .. }) => {}
        _ => bail!("{name}: first layer must be tcp or unix"),
    }

    let protos: Vec<usize> = layers
        .iter()
        .enumerate()
        .filter(|(_, l)| l.is_protocol())
        .map(|(i, _)| i)
        .collect();

    if protos.len() > 1 {
        bail!("{name}: only one protocol layer is allowed");
    }
    if let Some(&i) = protos.first() {
        if i != layers.len() - 1 {
            bail!("{name}: protocol must be the last layer");
        }
    }
    match (proto, protos.is_empty()) {
        (Proto::Required, true) => bail!("{name}: protocol layer is required"),
        (Proto::Forbidden, false) => bail!("{name}: protocol layer is not allowed here"),
        _ => Ok(()),
    }
}

pub fn validate(cfg: &AppConfig) -> Result<()> {
    match cfg.role {
        // клиент: inbound это вход браузера, outbound это туннель до сервера
        Role::Client => {
            check("inbound", &cfg.inbound.layers, Proto::Forbidden)?;
            check("outbound", &cfg.outbound.layers, Proto::Required)
        }
        // сервер: inbound это приём туннеля, outbound это выход в интернет
        Role::Server => {
            check("inbound", &cfg.inbound.layers, Proto::Required)?;
            check("outbound", &cfg.outbound.layers, Proto::Optional)
        }
    }
}
