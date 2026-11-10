use anyhow::{Result, anyhow, bail};
use cray_core::{BoxFuture, BuildCx, Cx, Input, Layer, Output, Ports, Side};
use serde::Deserialize;
use std::net::IpAddr;
use std::os::fd::OwnedFd;
use std::sync::Mutex;
#[cfg(not(target_os = "android"))]
use tokio_tun::TunBuilder;
use tokio_tun::TunQueue;

pub static TUN_FD: Mutex<Option<OwnedFd>> = Mutex::new(None);

pub fn set_tun_fd(fd: OwnedFd) {
    *TUN_FD.lock().unwrap() = Some(fd);
}

pub fn clear_tun_fd() {
    *TUN_FD.lock().unwrap() = None;
}

pub fn take_tun_fd() -> Option<OwnedFd> {
    TUN_FD.lock().unwrap().take()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TunCfg {
    name: Option<String>,
    address: Option<IpAddr>,
    prefix: Option<u8>,
    mode: String,
    mtu: Option<u16>,
}

pub fn build(params: &toml::Table, bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: TunCfg = params.clone().try_into()?;
    if bcx.side == Side::Outbound {
        bail!("tun dont work with outbound");
    }
    let mode = match cfg.mode.as_str() {
        #[cfg(not(target_os = "android"))]
        "Create" => Mode::Create,
        "Fd" => Mode::Fd,
        #[cfg(target_os = "android")]
        m => bail!(
            "mode {m} is not available on Android (Create handwritten is not implemented yet)"
        ),
        #[cfg(not(target_os = "android"))]
        m => bail!("unknown mode: {m}"),
    };

    match mode {
        #[cfg(not(target_os = "android"))]
        Mode::Create => {
            if cfg.name.is_none()
                || cfg.address.is_none()
                || cfg.prefix.is_none()
                || cfg.mtu.is_none()
            {
                bail!("tun mode create need Fields")
            }
            Ok(Box::new(Tun {
                name: cfg.name,
                address: cfg.address,
                prefix: cfg.prefix,
                mode,
                mtu: Some(cfg.mtu.unwrap() as i32),
            }))
        }

        Mode::Fd => Ok(Box::new(Tun {
            name: None,
            address: None,
            prefix: None,
            mtu: None,
            mode,
        })),
    }
}

pub enum Mode {
    #[cfg(not(target_os = "android"))]
    Create,
    Fd,
}

struct Tun {
    name: Option<String>,
    address: Option<IpAddr>,
    prefix: Option<u8>,
    mtu: Option<i32>,
    mode: Mode,
}

impl Layer for Tun {
    fn ports(&self) -> Ports {
        Ports::SOURCE
    }

    fn call<'a>(&'a self, _input: Input, _cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            match self.mode {
                #[cfg(not(target_os = "android"))]
                Mode::Create => {
                    let mut tun = TunBuilder::new()
                        .name(self.name.as_deref().unwrap())
                        .up()
                        .address(self.address.unwrap(), self.prefix.unwrap())
                        .mtu(self.mtu.unwrap())
                        .build()?;

                    Ok(Output::Stream(Box::pin(tun.pop().unwrap())))
                }
                Mode::Fd => {
                    let tun = take_tun_fd();
                    if tun.is_none() {
                        return Err(anyhow!("Tun fd is none"));
                    }
                    let tun = tun.unwrap();
                    let asynctun = TunQueue::new(tun)?;
                    Ok(Output::Stream(Box::pin(asynctun)))
                }
            }
        })
    }
}
