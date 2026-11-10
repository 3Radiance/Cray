use anyhow::{Result, anyhow, bail};
use cray_core::{BoxFuture, BuildCx, Cx, Input, Layer, Output, Ports, Side};
use serde::Deserialize;
use std::net::SocketAddr;
use std::os::fd::AsFd;
use std::os::fd::BorrowedFd;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::net::TcpSocket;

pub static PROTECT: Mutex<Option<Arc<dyn SocketProtect>>> = Mutex::new(None);

pub fn set_protect<T: SocketProtect + 'static>(protect: T) {
    *PROTECT.lock().unwrap() = Some(Arc::new(protect));
}

pub fn get_protect() -> Option<Arc<dyn SocketProtect>> {
    PROTECT.lock().unwrap().clone()
}

pub fn clear_protect() {
    *PROTECT.lock().unwrap() = None
}

pub trait SocketProtect: Send + Sync {
    fn protect(&self, fd: BorrowedFd<'_>) -> Result<()>;
}

pub fn build(params: &toml::Table, bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: TunTcpCfg = params.clone().try_into()?;
    if bcx.side == Side::Inbound {
        bail!("tun-tcp its only outbound");
    }

    let soc: SocketAddr = cfg.addr.parse()?;
    Ok(Box::new(TunTcp { addr: soc }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TunTcpCfg {
    addr: String,
}

pub struct TunTcp {
    addr: SocketAddr,
}

impl Layer for TunTcp {
    fn ports(&self) -> Ports {
        Ports::SOURCE
    }

    fn call<'a>(&'a self, _input: Input, _cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let socket = TcpSocket::new_v4()?;
            if let Some(protect) = get_protect() {
                protect.protect(socket.as_fd())?;
            } else {
                return Err(anyhow!("Protect dont found exit"));
            }
            let stream = socket.connect(self.addr).await?;

            Ok(Output::Stream(Box::pin(stream)))
        })
    }
}
