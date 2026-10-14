use crate::ConnectRequest;
use crate::config::appconfig::LayerConfig;
use anyhow::{Result, bail};
use std::future::Future;
use std::str::FromStr;
use tokio::io::{AsyncRead, AsyncWrite};
use uuid::Uuid;

pub mod vless;

pub trait Protocol: Send + Sync {
    type Context;
    type Outcome;

    fn client<C, W>(
        &mut self,
        plain: C,
        wire: W,
        t: &ConnectRequest,
        ctx: &mut Self::Context,
    ) -> impl Future<Output = Result<Self::Outcome>> + Send
    where
        C: AsyncRead + AsyncWrite + Unpin + Send + Sync,
        W: AsyncRead + AsyncWrite + Unpin + Send + Sync;

    fn server<W>(
        &mut self,
        wire: W,
        ctx: &mut Self::Context,
    ) -> impl Future<Output = Result<Self::Outcome>> + Send
    where
        W: AsyncRead + AsyncWrite + Unpin + Send + Sync;
}

pub fn proto_builder(layer: &LayerConfig) -> Result<ProtoVariant> {
    match layer {
        LayerConfig::Vless { uuid } => match uuid {
            Some(uuid) => {
                let uuid = Uuid::from_str(uuid)?;
                Ok(ProtoVariant::Vless(vless::Vless {
                    uuid: uuid.as_bytes().to_owned(),
                }))
            }
            None => bail!("Vless uuid is not provided"),
        },
        _ => bail!("Invalid protocol layer"),
    }
}

pub enum ProtoVariant {
    Vless(vless::Vless),
}

impl Protocol for ProtoVariant {
    type Context = ();
    type Outcome = ();

    fn client<C, W>(
        &mut self,
        plain: C,
        wire: W,
        t: &ConnectRequest,
        ctx: &mut Self::Context,
    ) -> impl Future<Output = Result<Self::Outcome>> + Send
    where
        C: AsyncRead + AsyncWrite + Unpin + Send + Sync,
        W: AsyncRead + AsyncWrite + Unpin + Send + Sync,
    {
        match self {
            ProtoVariant::Vless(p) => Box::pin(p.client(plain, wire, t, ctx)),
            // ProtoVariant::Custom_proto(p) => Box::pin(p.client(plain, wire, t, ctx)),
        }
    }

    fn server<W>(
        &mut self,
        wire: W,
        ctx: &mut Self::Context,
    ) -> impl Future<Output = Result<Self::Outcome>> + Send
    where
        W: AsyncRead + AsyncWrite + Unpin + Send + Sync,
    {
        match self {
            ProtoVariant::Vless(p) => Box::pin(p.server(wire, ctx)),
            // ProtoVariant::Сustom_proto(p) => Box::pin(p.server(wire, ctx)),
        }
    }
}
