use crate::ConnectRequest;
use anyhow::Result;
use std::future::Future;
use tokio::io::{AsyncRead, AsyncWrite};

pub mod default;
pub mod unix;

pub trait Outbound: Send + Sync {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    fn connect(&self, t: &ConnectRequest) -> impl Future<Output = Result<Self::Stream>> + Send;
}
