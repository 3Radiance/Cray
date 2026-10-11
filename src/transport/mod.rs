use anyhow::Result;
use bytes::Bytes;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;

pub mod http;
pub mod http2;
pub mod tls;

pub trait Transport: Send + Sync {
    type Context;
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    type Incoming: Incoming;

    fn client<W>(
        &mut self,
        wire: W,
        ctx: &mut Self::Context,
    ) -> impl Future<Output = Result<Self::Stream>> + Send
    where
        W: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static;

    fn server<P>(
        &mut self,
        plain: P,
        ctx: &mut Self::Context,
    ) -> impl Future<Output = Result<Self::Incoming>> + Send
    where
        P: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static;
}

pub trait AsyncReadWrite: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncReadWrite for T {}

pub type BoxedStream = Pin<Box<dyn AsyncReadWrite + Send + Sync + 'static>>;

pub trait ChunkRx: Unpin + Send {
    fn poll_chunk(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<Option<Bytes>>>;
}
pub trait ChunkTx: Unpin + Send {
    fn poll_send(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>>;
    fn poll_finish(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>>;
}

pub struct ChunkStream<R, T> {
    rx: R,
    tx: T,
    buf: Bytes,
    eof: bool,
}

impl<R: ChunkRx, T: ChunkTx> AsyncRead for ChunkStream<R, T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            if !self.buf.is_empty() {
                let n = self.buf.len().min(out.remaining());
                out.put_slice(&self.buf.split_to(n));
                return Poll::Ready(Ok(()));
            }
            if self.eof {
                return Poll::Ready(Ok(()));
            }
            match std::task::ready!(self.rx.poll_chunk(cx))? {
                Some(c) => self.buf = c,
                None => self.eof = true,
            }
        }
    }
}

impl<R: ChunkRx, T: ChunkTx> AsyncWrite for ChunkStream<R, T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        b: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.tx.poll_send(cx, b)
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.tx.poll_finish(cx)
    }
}

impl<R, T> ChunkStream<R, T> {
    pub fn new(rx: R, tx: T) -> Self {
        Self {
            rx,
            tx,
            buf: Bytes::new(),
            eof: false,
        }
    }
}

pub trait Incoming: Send {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    fn accept(&mut self) -> impl Future<Output = Option<Self::Stream>> + Send;
}

pub struct Once<S>(Option<S>);
impl<S> Once<S> {
    pub fn new(s: S) -> Self {
        Self(Some(s))
    }
}
impl<S: AsyncRead + AsyncWrite + Unpin + Send + 'static> Incoming for Once<S> {
    type Stream = S;
    async fn accept(&mut self) -> Option<S> {
        self.0.take()
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send + 'static> Incoming for mpsc::Receiver<S> {
    type Stream = S;
    async fn accept(&mut self) -> Option<S> {
        self.recv().await
    }
}
