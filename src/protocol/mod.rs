use anyhow::Result;
use bytes::BytesMut;
use std::future::Future;
use tokio::io::{AsyncRead, AsyncWrite};

pub mod test;

pub trait Protocol: Send + Sync {
    fn wrap(&mut self, plain: &[u8]) -> Result<Self>
    where
        Self: Sized;

    fn unwrap(&mut self, inp: &mut BytesMut) -> Result<Self>
    where
        Self: Sized;

    fn tunnel<C, U>(&mut self, client: C, upstream: U) -> impl Future<Output = Result<()>> + Send
    where
        C: AsyncRead + AsyncWrite + Unpin + Send,
        U: AsyncRead + AsyncWrite + Unpin + Send;
}
