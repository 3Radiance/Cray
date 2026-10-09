use anyhow::Result;
use std::future::Future;
use std::pin::Pin;
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
        W: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    fn server<P>(
        &mut self,
        plain: P,
        ctx: &mut Self::Context,
    ) -> impl Future<Output = Result<Self::Stream>> + Send
    where
        P: AsyncRead + AsyncWrite + Unpin + Send + 'static;
}

pub trait AsyncReadWrite: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncReadWrite for T {}

pub type BoxedStream = Pin<Box<dyn AsyncReadWrite + 'static>>;
