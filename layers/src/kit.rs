use anyhow::Result;
use cray_core::{ConnectRequest, Cx, Dialer};
use std::future::Future;
use tokio::io::{AsyncRead, AsyncWrite};

pub trait Protocol: Send + Sync {
    fn client<'a, C, W>(
        &self,
        plain: C,
        wire: W,
        t: &ConnectRequest,
        cx: &'a Cx,
    ) -> impl Future<Output = Result<()>> + Send
    where
        C: AsyncRead + AsyncWrite + Unpin + Send + Sync,
        W: AsyncRead + AsyncWrite + Unpin + Send + Sync;

    fn server<'a, W>(
        &self,
        wire: W,
        dialer: &Dialer,
        cx: &'a Cx,
    ) -> impl Future<Output = Result<()>> + Send
    where
        W: AsyncRead + AsyncWrite + Unpin + Send + Sync;
}
