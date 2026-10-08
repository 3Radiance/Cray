use crate::inbound::Inbound;
use anyhow::Result;
use std::future::Future;
use tokio::io::{AsyncRead, AsyncWrite};

pub trait Outbound: Send + Sync {
    fn connect<I: Inbound, S>(&self, client: &mut I) -> impl Future<Output = Result<S>> + Send
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static;
}
