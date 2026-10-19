use crate::crypto::hmac::{self, parse_key};
use anyhow::Result;
use cray_core::{BoxFuture, BoxedStream, BuildCx, Cx, Input, Layer, Output, Ports};
use serde::Deserialize;
use std::cmp::min;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub const HDR: usize = 32 + 2;
const MAX_FRAME_BODY: usize = 16 * 1024;
const SCRATCH: usize = 4096;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthCfg {
    key: String,
}

pub fn build(params: &toml::Table, _bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: AuthCfg = params.clone().try_into()?;
    let key = parse_key(&cfg.key)?;
    Ok(Box::new(Auth { key }))
}

pub struct Auth {
    key: [u8; 32],
}

impl Layer for Auth {
    fn ports(&self) -> Ports {
        Ports::WRAP
    }

    fn call<'a>(&'a self, input: Input, _cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let stream: BoxedStream = input.stream()?;
            Ok(Output::Stream(Box::pin(AuthStream::new(stream, self.key))))
        })
    }
}

pub enum ReadState {
    Hdr { got: usize },
    Payload { left: usize },
    Out { off: usize },
}

pub struct AuthStream<S> {
    inner: S,
    key: [u8; 32],
    wbuf: Vec<u8>,
    woff: usize,
    hdr: [u8; HDR],
    rstate: ReadState,
    frame: Vec<u8>,
}

impl<S> AuthStream<S> {
    fn new(inner: S, key: [u8; 32]) -> Self {
        Self {
            inner,
            key,
            wbuf: Vec::new(),
            woff: 0,
            rstate: ReadState::Hdr { got: 0 },
            hdr: [0; HDR],
            frame: Vec::new(),
        }
    }

    fn make_frame(&mut self, buf: &[u8]) -> Result<usize> {
        let n = buf.len().min(MAX_FRAME_BODY) as u16;
        self.wbuf.resize(32 + 2 + n as usize, 0);
        self.wbuf[32..34].copy_from_slice(&n.to_be_bytes());
        self.wbuf[34..].copy_from_slice(&buf[..n as usize]);
        let tag = hmac::tag(&self.key, &self.wbuf[32..])?;
        self.wbuf[0..32].copy_from_slice(&tag);
        Ok(n as usize)
    }
}
impl<S: AsyncWrite + Unpin> AuthStream<S> {
    fn poll_drain(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.woff < self.wbuf.len() {
            let n = ready!(Pin::new(&mut self.inner).poll_write(cx, &self.wbuf[self.woff..]))?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.woff += n;
        }
        self.wbuf.clear();
        self.woff = 0;
        Poll::Ready(Ok(()))
    }
}

fn eof(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        format!("auth: eof inside {what}"),
    )
}

impl<S: AsyncRead + Unpin> AsyncRead for AuthStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            match this.rstate {
                ReadState::Hdr { got } => {
                    let m = {
                        let mut rb = ReadBuf::new(&mut this.hdr[got..]);
                        ready!(Pin::new(&mut this.inner).poll_read(cx, &mut rb))?;
                        rb.filled().len()
                    };
                    if m == 0 {
                        return if got == 0 {
                            Poll::Ready(Ok(()))
                        } else {
                            Poll::Ready(Err(eof("header")))
                        };
                    }
                    let got = got + m;
                    if got < HDR {
                        this.rstate = ReadState::Hdr { got };
                        continue;
                    }
                    let len = u16::from_be_bytes(this.hdr[32..34].try_into().unwrap()) as usize;
                    if len > MAX_FRAME_BODY {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "auth: frame too large",
                        )));
                    }
                    this.frame.clear();
                    this.rstate = ReadState::Payload { left: len };
                }
                ReadState::Payload { left } => {
                    if left == 0 {
                        let mut msg = Vec::with_capacity(2 + this.frame.len());
                        msg.extend_from_slice(&this.hdr[32..34]);
                        msg.extend_from_slice(&this.frame);
                        if let Err(_) = hmac::verify(&this.key, &msg, &this.hdr[0..32]) {
                            return Poll::Ready(Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "auth: bad tag",
                            )));
                        }
                        this.rstate = ReadState::Out { off: 0 };
                        continue;
                    }
                    let take = left.min(SCRATCH);
                    let m = {
                        let mut scratch = [0u8; SCRATCH];
                        let mut rb = ReadBuf::new(&mut scratch[..take]);
                        ready!(Pin::new(&mut this.inner).poll_read(cx, &mut rb))?;
                        let m = rb.filled().len();
                        if m == 0 {
                            return Poll::Ready(Err(eof("payload")));
                        }
                        this.frame.extend_from_slice(&rb.filled()[..m]);
                        m
                    };
                    this.rstate = ReadState::Payload { left: left - m };
                }
                ReadState::Out { off } => {
                    if off == this.frame.len() {
                        this.rstate = ReadState::Hdr { got: 0 };
                        continue;
                    }
                    if buf.remaining() == 0 {
                        return Poll::Ready(Ok(()));
                    }
                    let n = min(this.frame.len() - off, buf.remaining());
                    buf.put_slice(&this.frame[off..off + n]);
                    let off = off + n;
                    this.rstate = ReadState::Out { off };
                    return Poll::Ready(Ok(()));
                }
            }
        }
    }
}
impl<S: AsyncWrite + Unpin> AsyncWrite for AuthStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        ready!(this.poll_drain(cx))?;
        let n = this.make_frame(buf).map_err(io::Error::other)?;
        if let Poll::Ready(Err(e)) = this.poll_drain(cx) {
            return Poll::Ready(Err(e));
        }
        Poll::Ready(Ok(n))
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_drain(cx))?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_drain(cx))?;
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}
