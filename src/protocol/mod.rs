use crate::ConnectRequest;
use crate::config::appconfig::LayerConfig;
use anyhow::{Result, bail};
use std::future::Future;
use tokio::io::{AsyncRead, AsyncWrite};

pub mod tsf;

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
        LayerConfig::Tsf { magic: Some(magic) } => {
            Ok(ProtoVariant::Tsf(tsf::Tsf { magic: *magic }))
        }
        _ => bail!("Invalid protocol layer"),
    }
}

pub enum ProtoVariant {
    Tsf(tsf::Tsf),
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
            ProtoVariant::Tsf(p) => Box::pin(p.client(plain, wire, t, ctx)),
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
            ProtoVariant::Tsf(p) => Box::pin(p.server(wire, ctx)),
            // ProtoVariant::Сustom_proto(p) => Box::pin(p.server(wire, ctx)),
        }
    }
}
