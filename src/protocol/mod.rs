use crate::{ConnectRequest, outbound::Outbound};
use anyhow::Result;
use std::future::Future;
use tokio::io::{AsyncRead, AsyncWrite};

pub trait Protocol: Send + Sync {
    type Context;

    fn client<C, W>(
        &mut self,
        plain: C,
        wire: W,
        t: &ConnectRequest,
        ctx: &mut Self::Context,
    ) -> impl Future<Output = Result<()>> + Send
    where
        C: AsyncRead + AsyncWrite + Unpin + Send,
        W: AsyncRead + AsyncWrite + Unpin + Send;

    fn server<W, D>(
        &mut self,
        wire: W,
        dialer: &D,
        ctx: &mut Self::Context,
    ) -> impl Future<Output = Result<()>> + Send
    where
        W: AsyncRead + AsyncWrite + Unpin + Send,
        D: Outbound;
}
