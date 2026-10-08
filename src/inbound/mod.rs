use crate::ConnectRequest;
use anyhow::Result;
use std::net::SocketAddr;
use tokio::io::{AsyncRead, AsyncWrite};

pub mod http;
pub mod tcp;

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

pub trait Listener: Send + Sync {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    fn accept(&self) -> impl Future<Output = Result<(Self::Stream, SocketAddr)>> + Send;
}

pub enum ConnectResult {
    Ok,
    HostUnreachable,
    ConnectionRefused,
    Timeout,
    Other,
}
