use anyhow::Result;
use cray_core::{BoxFuture, BoxedStream, Cx, Dialer, Layer};
use cray_core::{ConnectRequest, Incoming as DynIncoming};
use std::future::Future;
use tokio::io::{AsyncRead, AsyncWrite};

pub trait Incoming: Send {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    fn accept(&mut self) -> impl Future<Output = Option<Self::Stream>> + Send;
}

pub trait Transport: Send + Sync {
    type Incoming: Incoming;

    fn client<'a, W>(
        &mut self,
        wire: W,
        cx: &'a Cx,
    ) -> impl Future<Output = Result<BoxedStream>> + Send
    where
        W: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static;

    fn server<'a, P>(
        &mut self,
        plain: P,
        cx: &'a Cx,
    ) -> impl Future<Output = Result<Self::Incoming>> + Send
    where
        P: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static;
}

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

pub struct BoxIncoming<I>(pub I);

impl<I> DynIncoming for BoxIncoming<I>
where
    I: Incoming,
    I::Stream: Sync,
{
    fn accept(&mut self) -> BoxFuture<'_, Option<BoxedStream>> {
        Box::pin(async move {
            let s = self.0.accept().await?;
            Some(Box::pin(s) as BoxedStream)
        })
    }
}

pub fn layer<L: Layer>(l: L) -> Box<dyn Layer> {
    Box::new(l)
}
