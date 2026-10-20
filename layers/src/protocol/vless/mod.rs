mod handshake;

use anyhow::{Result, anyhow};
use cray_core::{
    BoxFuture, BoxedStream, BuildCx, ConnectRequest, Cx, Dialer, Input, Key, Layer, Output, Ports,
    Side, key, log_tag,
};
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncWrite};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VlessCfg {
    uuid: Option<String>,
}

pub fn build(params: &toml::Table, bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: VlessCfg = params.clone().try_into()?;
    let uuid = match cfg.uuid {
        Some(u) => u,
        None => return Err(anyhow!("Vless need uuid")),
    };
    let uuid = *uuid::Uuid::parse_str(&uuid)
        .map_err(|_| anyhow!("Invalid UUID format"))?
        .to_owned()
        .as_bytes();

    Ok(Box::new(Vless {
        uuid: uuid,
        side: bcx.side,
    }))
}

pub struct Vless {
    side: Side,
    uuid: [u8; 16],
}

impl Layer for Vless {
    fn ports(&self) -> Ports {
        Ports::TERMINAL
    }

    fn requires(&self) -> Vec<Key> {
        match self.side {
            Side::Outbound => vec![key::<ConnectRequest>()],
            _ => Vec::new(),
        }
    }

    fn provides(&self) -> Vec<Key> {
        match self.side {
            Side::Inbound => vec![key::<ConnectRequest>()],
            _ => Vec::new(),
        }
    }

    fn call<'a>(&'a self, input: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            match self.side {
                Side::Inbound => {
                    let stream: BoxedStream = input.stream()?;
                    let dialer: &Dialer = cx
                        .get::<Dialer>()
                        .ok_or_else(|| anyhow!("no Dialer in Cx"))?;

                    self.server(stream, dialer, cx).await?;
                    Ok(Output::Done)
                }
                Side::Outbound => {
                    let stream: BoxedStream = input.stream()?;
                    let dialer: &Dialer = cx
                        .get::<Dialer>()
                        .ok_or_else(|| anyhow!("no Dialer in Cx"))?;
                    let t = cx
                        .get::<ConnectRequest>()
                        .ok_or_else(|| anyhow!("no ConnectRequest in Cx"))?;
                    let wire = dialer.dial_to(cx, t.clone()).await?;

                    self.client(stream, wire, t, cx).await?;

                    Ok(Output::Done)
                }
            }
        })
    }
}

impl Vless {
    async fn client<'a, C, W>(
        &self,
        mut plain: C,
        mut wire: W,
        t: &ConnectRequest,
        _cx: &'a Cx,
    ) -> Result<()>
    where
        C: AsyncRead + AsyncWrite + Unpin + Send + Sync,
        W: AsyncRead + AsyncWrite + Unpin + Send + Sync,
    {
        log_tag!(
            debug,
            "VLESS",
            "Client VLESS relay started for target {}:{}",
            t.addr,
            t.port
        );
        handshake::client_handshake(&self.uuid, &mut wire, t.port, &t.addr).await?;

        tokio::io::copy_bidirectional(&mut plain, &mut wire).await?;
        Ok(())
    }

    async fn server<'a, W>(&self, mut wire: W, dialer: &Dialer, cx: &'a Cx) -> Result<()>
    where
        W: AsyncRead + AsyncWrite + Unpin + Send + Sync,
    {
        log_tag!(debug, "VLESS", "Server VLESS relay started");
        let req = handshake::server_handshake(&self.uuid, &mut wire).await?;
        log_tag!(debug, "VLESS", "Parsed upstream target: {:?}", req);

        let mut plain = dialer.dial_to(cx, req).await?;

        tokio::io::copy_bidirectional(&mut plain, &mut wire).await?;

        Ok(())
    }
}
