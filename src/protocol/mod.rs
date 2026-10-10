use crate::ConnectRequest;
use anyhow::Result;
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
