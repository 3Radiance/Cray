use anyhow::Result;
use cray_core::log_tag;
use cray_core::{BoxFuture, BoxedStream, BuildCx, Cx, Incoming, Input, Layer, Output, Ports, Side};
use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnixCfg {
    addr: String,
}

pub struct Unix {
    addr: String,
    side: Side,
}

struct UnixIncoming {
    listener: tokio::net::UnixListener,
}

impl Incoming for UnixIncoming {
    fn accept(&mut self) -> BoxFuture<'_, Option<BoxedStream>> {
        Box::pin(async move {
            loop {
                match self.listener.accept().await {
                    Ok((s, peer)) => {
                        log_tag!(debug, "INB", "Accepted Unix connection from {:?}", peer);
                        return Some(Box::pin(s) as BoxedStream);
                    }
                    Err(e) => {
                        log_tag!(warn, "INB", "accept failed: {}", e);
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        })
    }
}

impl Layer for Unix {
    fn ports(&self) -> Ports {
        match self.side {
            Side::Inbound => Ports::LISTENER,
            Side::Outbound => Ports::SOURCE,
        }
    }

    fn requires(&self) -> Vec<cray_core::Key> {
        Vec::new()
    }

    fn default_timeout(&self) -> Option<Duration> {
        Some(Duration::from_secs(5))
    }

    fn call<'a>(&'a self, _input: Input, _cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            match self.side {
                Side::Inbound => {
                    let _ = std::fs::remove_file(&self.addr);
                    let listener = tokio::net::UnixListener::bind(&self.addr)?;
                    log_tag!(info, "INB", "Listening on {}", self.addr);
                    Ok(Output::Streams(Box::new(UnixIncoming { listener })))
                }
                Side::Outbound => {
                    let s = tokio::net::UnixStream::connect(&self.addr).await?;
                    Ok(Output::Stream(Box::pin(s)))
                }
            }
        })
    }
}

pub fn build(params: &toml::Table, bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: UnixCfg = params.clone().try_into()?;

    Ok(Box::new(Unix {
        addr: cfg.addr,
        side: bcx.side,
    }))
}
