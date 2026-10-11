use anyhow::Result;
use std::future::Future;
use tokio::io::{AsyncRead, AsyncWrite};

pub mod default;

pub trait Outbound: Send + Sync {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    type Context;

    fn connect(&self, t: &Self::Context) -> impl Future<Output = Result<Self::Stream>> + Send;
}
