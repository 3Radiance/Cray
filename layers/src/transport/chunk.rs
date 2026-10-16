//! Byte-chunk adapter: independent read-half / write-half over any message stream.
//! Ported verbatim from old-cray `transport` core.

use bytes::Bytes;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

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
