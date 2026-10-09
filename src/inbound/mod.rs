use crate::{ConnectRequest, ConnectResult};
use anyhow::Result;
use std::future::Future;
use tokio::io::{AsyncRead, AsyncWrite};

pub mod http_connect;
pub mod tcp;
pub mod unix;

pub trait Inbound: Send + Sync {
    fn handshake<S>(&self, client: &mut S) -> impl Future<Output = Result<ConnectRequest>> + Send
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    fn reply<S>(
        &self,
        client: &mut S,
        ok: ConnectResult,
    ) -> impl Future<Output = Result<()>> + Send
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static;
}
