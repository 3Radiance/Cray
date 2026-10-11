pub mod auth;
pub mod config;
pub mod fingerprint;
pub mod inbound;
pub mod logging;
pub mod outbound;
pub mod protocol;
pub mod transport;

use anyhow::Result;
use std::future::Future;
use tokio::io::{AsyncRead, AsyncWrite};

#[derive(Clone)]
pub struct ConnectRequest {
    pub addr: String,
    pub port: u16,
}

pub enum ConnectResult {
    Ok,
    HostUnreachable,
    ConnectionRefused,
    Timeout,
    Other,
}

pub trait Listener: Send + Sync {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    type Addr: Send + 'static;

    fn accept(&self) -> impl Future<Output = Result<(Self::Stream, Self::Addr)>> + Send;
}
