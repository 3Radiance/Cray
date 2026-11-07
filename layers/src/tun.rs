use anyhow::{Result, bail};
use cray_core::{BoxFuture, BuildCx, Cx, Input, Layer, Output, Ports, Side};
use serde::Deserialize;
use tokio_tun::TunBuilder;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TunCfg {
    name: Option<String>,
    address: Option<String>,
    prefix: Option<String>,
}

pub fn build(params: &toml::Table, bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: TunCfg = params.clone().try_into()?;
    if bcx.side == Side::Outbound {
        bail!("tun Outbound dont sup");
    }
    if cfg.address.is_none() {
        bail!("tun layer need address example: 10.0.0.1")
    }
    if cfg.name.is_none() {
        bail!("tun layer need interface name example: tun0 or wlan1 (For stealth)")
    }
    if cfg.prefix.is_none() {
        bail!("tun layer need prefix example: 24")
    }

    Ok(Box::new(Tun {
        name: cfg.name.unwrap(),
        address: cfg.address.unwrap(),
        prefix: cfg.prefix.unwrap().parse().unwrap(),
    }))
}

struct Tun {
    name: String,
    address: String,
    prefix: u8,
}

impl Layer for Tun {
    fn ports(&self) -> Ports {
        Ports::SOURCE
    }

    fn call<'a>(&'a self, _input: Input, _cx: &'a mut Cx) -> BoxFuture<'a, anyhow::Result<Output>> {
        Box::pin(async move {
            let mut tun = TunBuilder::new()
                .name(self.name.as_str())
                .up()
                .address(self.address.parse().unwrap(), self.prefix)
                .build()?;

            Ok(Output::Stream(Box::pin(tun.pop().unwrap())))
        })
    }
}
