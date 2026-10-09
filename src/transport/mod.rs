use anyhow::Result;
use std::future::Future;
use tokio::io::{AsyncRead, AsyncWrite};

pub mod http;
pub mod http2;
pub mod tls;

pub trait Transport: Send + Sync {
    type Context;
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    fn client<W>(
        &mut self,
        wire: W,
        ctx: &mut Self::Context,
    ) -> impl Future<Output = Result<Self::Stream>> + Send
    where
        W: AsyncRead + AsyncWrite + Unpin + Send;

    fn server<P>(
        &mut self,
        plain: P,
        ctx: &mut Self::Context,
    ) -> impl Future<Output = Result<Self::Stream>> + Send
    where
        P: AsyncRead + AsyncWrite + Unpin + Send;
}
