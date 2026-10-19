use crate::deserialize_opt_from_str;
use anyhow::Result;
use cray_core::log_tag;
use cray_core::{BoxFuture, BoxedStream, BuildCx, Cx, Input, Layer, Output, Ports};
use rand::Rng;
use serde::Deserialize;
use std::io;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

const HDR: usize = 2 + 2;
const MAX_FRAME_BODY: usize = 16 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PadCfg {
    stages: Option<Vec<PadStage>>,
}

pub fn build(params: &toml::Table, _bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: PadCfg = params.clone().try_into()?;
    let stages = cfg
        .stages
        .ok_or_else(|| anyhow::anyhow!("stages is required"))?;
    validate_stages(&stages)?;
    Ok(Box::new(Pad {
        stages: stages.into(),
    }))
}

pub struct Pad {
    stages: Arc<[PadStage]>,
}

impl Layer for Pad {
    fn ports(&self) -> Ports {
        Ports::WRAP
    }

    fn call<'a>(&'a self, input: Input, _cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let stream: BoxedStream = input.stream()?;
            log_tag!(debug, "PAD", "new stream, {} stages", self.stages.len());
            Ok(Output::Stream(Box::pin(PadStream::new(
                stream,
                self.stages.clone(),
            ))))
        })
    }
}

#[derive(Debug, Deserialize, Clone)]
pub enum PadSize {
    Fixed(usize),
    Range(usize, usize),
    Percent(u8),
}

#[derive(Debug, Deserialize, Clone)]
pub enum PadUpTo {
    Fixed(usize),
    Range(usize, usize),
}

#[derive(Debug, Deserialize, Clone)]
pub struct PadStage {
    pub bytes: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_opt_from_str")]
    pub size: Option<PadSize>,
    #[serde(default, deserialize_with = "deserialize_opt_from_str")]
    pub upto: Option<PadUpTo>,
}

impl FromStr for PadUpTo {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if let Some((a, b)) = s.split_once('-') {
            let min: usize = a
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid pad upto min: '{}'", a))?;
            let max: usize = b
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid pad upto max: '{}'", b))?;
            anyhow::ensure!(
                min <= max,
                "pad upto min ({}) must be <= max ({})",
                min,
                max
            );
            anyhow::ensure!(min > 0, "pad upto min must be > 0");
            return Ok(PadUpTo::Range(min, max));
        }
        let n: usize = s
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid pad upto value: '{}'", s))?;
        Ok(PadUpTo::Fixed(n))
    }
}

impl FromStr for PadSize {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if let Some(pct) = s.strip_suffix('%') {
            let p: u8 = pct
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid pad size: '{}'", pct))?;
            anyhow::ensure!(
                p > 0 && p <= 100,
                "pad size percent must be 1..=100, got {}",
                p
            );
            return Ok(PadSize::Percent(p));
        }
        if let Some((a, b)) = s.split_once('-') {
            let min: usize = a
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid pad size: '{}'", a))?;
            let max: usize = b
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid pad size: '{}'", b))?;
            anyhow::ensure!(
                min <= max,
                "pad size min ({}) must be <= max ({})",
                min,
                max
            );
            anyhow::ensure!(min > 0, "pad size min must be > 0");
            return Ok(PadSize::Range(min, max));
        }
        let n: usize = s
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid pad size: '{}'", s))?;
        Ok(PadSize::Fixed(n))
    }
}

fn check_frame_size(n: usize) -> Result<()> {
    anyhow::ensure!(
        n > HDR,
        "pad: frame size {n} must be > {HDR} (the header), or 0 for no padding"
    );
    anyhow::ensure!(
        n <= HDR + MAX_FRAME_BODY,
        "pad: frame size {n} must be <= {}",
        HDR + MAX_FRAME_BODY
    );
    Ok(())
}

impl PadSize {
    fn check(&self) -> Result<()> {
        match *self {
            PadSize::Fixed(0) | PadSize::Percent(_) => Ok(()),
            PadSize::Fixed(n) => check_frame_size(n),
            PadSize::Range(min, max) => {
                check_frame_size(min)?;
                check_frame_size(max)
            }
        }
    }
}

impl PadUpTo {
    fn check(&self) -> Result<()> {
        match *self {
            PadUpTo::Fixed(0) => Ok(()),
            PadUpTo::Fixed(n) => check_frame_size(n),
            PadUpTo::Range(min, max) => {
                check_frame_size(min)?;
                check_frame_size(max)
            }
        }
    }
}

impl PadStage {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.size.is_some() ^ self.upto.is_some(),
            "pad stage: set either 'size' or 'upto', not both and not none"
        );
        if let Some(s) = &self.size {
            s.check()?;
        }
        if let Some(u) = &self.upto {
            u.check()?;
        }
        Ok(())
    }
}

fn validate_stages(stages: &[PadStage]) -> Result<()> {
    anyhow::ensure!(!stages.is_empty(), "stages must not be empty");
    for st in stages {
        st.validate()?;
    }
    let mut prev = 0u64;
    for (i, st) in stages.iter().enumerate() {
        match st.bytes {
            None => anyhow::ensure!(
                i + 1 == stages.len(),
                "pad stage {i}: only the last stage may omit 'bytes'"
            ),
            Some(b) => {
                anyhow::ensure!(b > prev, "pad stage {i}: 'bytes' must be increasing");
                prev = b;
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum ReadState {
    Hdr { got: usize },
    Payload { left: usize, pad: usize },
    Pad { left: usize },
}

pub struct PadStream<S> {
    inner: S,
    stages: Arc<[PadStage]>,
    idx: usize,
    written: u64,
    wbuf: Vec<u8>,
    woff: usize,
    hdr: [u8; HDR],
    rstate: ReadState,
}

impl<S> PadStream<S> {
    pub fn new(inner: S, stages: Arc<[PadStage]>) -> Self {
        Self {
            inner,
            stages,
            idx: 0,
            written: 0,
            wbuf: Vec::new(),
            woff: 0,
            hdr: [0; HDR],
            rstate: ReadState::Hdr { got: 0 },
        }
    }

    fn advance_stage(&mut self) {
        while let Some(up) = self.stages[self.idx].bytes {
            if self.written >= up && self.idx + 1 < self.stages.len() {
                self.idx += 1;
            } else {
                break;
            }
        }
    }

    fn frame_size(&mut self, arrival: usize) -> usize {
        self.advance_stage();
        let st = &self.stages[self.idx];
        let mut rng = rand::rng();
        if let Some(size) = &st.size {
            match size {
                PadSize::Fixed(n) => *n,
                PadSize::Range(min, max) => rng.random_range(*min..=*max),
                PadSize::Percent(p) => HDR + arrival + arrival.saturating_mul(*p as usize) / 100,
            }
        } else if let Some(upto) = &st.upto {
            match upto {
                PadUpTo::Fixed(n) => *n,
                PadUpTo::Range(min, max) => rng.random_range(*min..=*max),
            }
        } else {
            0
        }
    }

    fn make_frame(&mut self, buf: &[u8]) -> usize {
        let s = self.frame_size(buf.len());
        let (n, pad_len) = if s == 0 {
            (buf.len().min(MAX_FRAME_BODY), 0)
        } else {
            let cap = s.saturating_sub(HDR).clamp(1, MAX_FRAME_BODY);
            let n = buf.len().min(cap);
            (n, cap - n)
        };
        self.written += n as u64;

        self.wbuf.clear();
        self.woff = 0;
        self.wbuf.reserve(HDR + n + pad_len);
        self.wbuf.extend_from_slice(&(n as u16).to_be_bytes());
        self.wbuf.extend_from_slice(&(pad_len as u16).to_be_bytes());
        self.wbuf.extend_from_slice(&buf[..n]);
        let start = self.wbuf.len();
        self.wbuf.resize(start + pad_len, 0);
        rand::rng().fill(&mut self.wbuf[start..]);
        log_tag!(
            trace,
            "PAD",
            "frame: stage {}, wire {}, payload {}, pad {}",
            self.idx,
            HDR + n + pad_len,
            n,
            pad_len
        );
        n
    }
}

impl<S: AsyncWrite + Unpin> PadStream<S> {
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
        format!("pad: eof inside {what}"),
    )
}

impl<S: AsyncRead + Unpin> AsyncRead for PadStream<S> {
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
                    let payload = u16::from_be_bytes([this.hdr[0], this.hdr[1]]) as usize;
                    let pad = u16::from_be_bytes([this.hdr[2], this.hdr[3]]) as usize;
                    if payload > MAX_FRAME_BODY || pad > MAX_FRAME_BODY {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "pad: frame too large",
                        )));
                    }
                    this.rstate = ReadState::Payload { left: payload, pad };
                }
                ReadState::Payload { left, pad } => {
                    if left == 0 {
                        this.rstate = ReadState::Pad { left: pad };
                        continue;
                    }
                    if buf.remaining() == 0 {
                        return Poll::Ready(Ok(()));
                    }
                    let take = left.min(buf.remaining());
                    let m = {
                        let mut rb = ReadBuf::new(buf.initialize_unfilled_to(take));
                        ready!(Pin::new(&mut this.inner).poll_read(cx, &mut rb))?;
                        rb.filled().len()
                    };
                    if m == 0 {
                        return Poll::Ready(Err(eof("payload")));
                    }
                    buf.advance(m);
                    this.rstate = ReadState::Payload {
                        left: left - m,
                        pad,
                    };
                    return Poll::Ready(Ok(()));
                }
                ReadState::Pad { left } => {
                    if left == 0 {
                        this.rstate = ReadState::Hdr { got: 0 };
                        continue;
                    }
                    let mut tmp = [0u8; 2048];
                    let take = left.min(tmp.len());
                    let m = {
                        let mut rb = ReadBuf::new(&mut tmp[..take]);
                        ready!(Pin::new(&mut this.inner).poll_read(cx, &mut rb))?;
                        rb.filled().len()
                    };
                    if m == 0 {
                        return Poll::Ready(Err(eof("padding")));
                    }
                    this.rstate = ReadState::Pad { left: left - m };
                }
            }
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for PadStream<S> {
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
        let n = this.make_frame(buf);

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
