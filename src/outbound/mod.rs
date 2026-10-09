use crate::ConnectRequest;
use anyhow::Result;
use std::future::Future;
use tokio::io::{AsyncRead, AsyncWrite};

pub mod direct;

pub trait Outbound: Send + Sync {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    fn dial(&self, t: &ConnectRequest) -> impl Future<Output = Result<Self::Stream>> + Send;
}
