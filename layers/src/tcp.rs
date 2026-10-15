use anyhow::{Result, bail};
use cray_core::log_tag;
use cray_core::{
    BoxFuture, BoxedStream, BuildCx, ConnectRequest, Cx, Incoming, Input, Key, Layer, Output,
    Ports, Side, key,
};
use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TcpCfg {
    addr: Option<String>,
}

pub struct Tcp {
    addr: Option<String>,
    side: Side,
}

struct TcpIncoming {
    listener: tokio::net::TcpListener,
}

impl Incoming for TcpIncoming {
    fn accept(&mut self) -> BoxFuture<'_, Option<BoxedStream>> {
        Box::pin(async move {
            loop {
                match self.listener.accept().await {
                    Ok((s, peer)) => {
                        let _ = s.set_nodelay(true);
                        log_tag!(debug, "INB", "Accepted TCP connection from {}", peer);
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

impl Layer for Tcp {
    fn ports(&self) -> Ports {
        match self.side {
            Side::Inbound => Ports::LISTENER, // Nothing -> Streams
            Side::Outbound => Ports::SOURCE,  // Nothing -> Stream
        }
    }

    fn requires(&self) -> Vec<Key> {
        match self.side {
            Side::Outbound if self.addr.is_none() => vec![key::<ConnectRequest>()],
            _ => Vec::new(),
        }
    }

    fn default_timeout(&self) -> Option<Duration> {
        Some(Duration::from_secs(5))
    }

    fn call<'a>(&'a self, _input: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            match self.side {
                Side::Inbound => {
                    let addr = self.addr.clone().expect("checked at build");
                    let listener = tokio::net::TcpListener::bind(&addr).await?;
                    log_tag!(info, "INB", "Listening on {}", addr);
                    Ok(Output::Streams(Box::new(TcpIncoming { listener })))
                }
                Side::Outbound => {
                    let s = if let Some(fixed) = &self.addr {
                        tokio::net::TcpStream::connect(fixed).await?
                    } else {
                        let t = cx
                            .get::<ConnectRequest>()
                            .ok_or_else(|| anyhow::anyhow!("no ConnectRequest in Cx"))?;
                        tokio::net::TcpStream::connect((t.addr.as_str(), t.port)).await?
                    };
                    s.set_nodelay(true)?;
                    Ok(Output::Stream(Box::pin(s)))
                }
            }
        })
    }
}

pub fn build(params: &toml::Table, bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: TcpCfg = params.clone().try_into()?;
    if bcx.side == Side::Inbound && cfg.addr.is_none() {
        bail!("tcp inbound needs 'addr'");
    }
    Ok(Box::new(Tcp {
        addr: cfg.addr,
        side: bcx.side,
    }))
}
