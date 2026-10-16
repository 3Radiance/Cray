use crate::{
    crypto::{parse_key, tag as hmac_tag, verify as hmac_verify},
    deserialize_opt_from_str,
};
use anyhow::Result;
use cray_core::log_tag;
use cray_core::{BoxFuture, BoxedStream, BuildCx, Cx, Input, Layer, Output, Ports};
use rand::Rng;
use serde::Deserialize;
use std::io;
use std::pin::Pin;
use std::str::FromStr;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PadCfg {
    key: Option<String>,
    stages: Option<Vec<PadStage>>,
}

pub fn build(params: &toml::Table, _bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: PadCfg = params.clone().try_into()?;
    let key = cfg.key.ok_or_else(|| anyhow::anyhow!("key is required"))?;
    let key = parse_key(&key)?;
    let stages = cfg
        .stages
        .ok_or_else(|| anyhow::anyhow!("stages is required"))?;
    anyhow::ensure!(stages.len() > 0, "stages must not be empty");
    for stage in &stages {
        stage.validate()?;
    }
    Ok(Box::new(Pad { key, stages }))
}

pub struct Pad {
    key: [u8; 32],
    stages: Vec<PadStage>,
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
                self.key,
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
pub struct PadStage {
    pub bytes: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_opt_from_str")]
    pub size: Option<PadSize>,
    #[serde(default, deserialize_with = "deserialize_opt_from_str")]
    pub upto: Option<PadUpTo>,
}

#[derive(Debug, Deserialize, Clone)]
pub enum PadUpTo {
    Fixed(usize),
    Range(usize, usize),
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

impl PadStage {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.size.is_some() ^ self.upto.is_some(),
            "pad stage: set either 'size' or 'upto', not both and not none"
        );
        Ok(())
    }
}

pub struct PadStream<S> {
    inner: S,
    key: [u8; 32],
    stages: Vec<PadStage>,
    idx: usize,
    written: u64,
    wseq: u64,
    rseq: u64,
    wbuf: Vec<u8>,
    rstate: ReadState,
    rbuf: Vec<u8>,
    pending: Vec<u8>,
    poff: usize,
}

const HDR: usize = 32 + 2 + 2;
const MAX_FRAME_BODY: usize = 16 * 1024;

enum ReadState {
    Hdr,
    Body { payload: usize, pad: usize },
}

impl<S> PadStream<S> {
    pub fn new(inner: S, stages: Vec<PadStage>, key: [u8; 32]) -> Self {
        Self {
            inner,
            key,
            stages,
            idx: 0,
            written: 0,
            wseq: 0,
            rseq: 0,
            wbuf: Vec::new(),
            rstate: ReadState::Hdr,
            rbuf: Vec::with_capacity(HDR),
            pending: Vec::new(),
            poff: 0,
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

    fn frame_size(&mut self, arrival: usize) -> Option<usize> {
        self.advance_stage();
        let st = &self.stages[self.idx];
        let mut rng = rand::rng();
        if let Some(size) = &st.size {
            let s = match size {
                PadSize::Fixed(0) => return Some(0),
                PadSize::Fixed(n) => *n,
                PadSize::Range(min, max) => rng.random_range(*min..=*max),

                PadSize::Percent(p) => arrival + arrival * (*p as usize) / 100,
            };
            Some(s.max(1))
        } else if let Some(upto) = &st.upto {
            let cap = match upto {
                PadUpTo::Fixed(0) => return Some(0),
                PadUpTo::Fixed(n) => *n,
                PadUpTo::Range(min, max) => rng.random_range(*min..=*max),
            };
            Some(cap.max(1))
        } else {
            None
        }
    }
}

impl<S: AsyncWrite + Unpin> PadStream<S> {
    fn poll_drain(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while !self.wbuf.is_empty() {
            let wire_len = self.wbuf.len();
            match Pin::new(&mut self.inner).poll_write(cx, &self.wbuf) {
                Poll::Ready(Ok(n)) => {
                    self.wbuf.drain(..n);
                    if !self.wbuf.is_empty() {
                        return Poll::Pending;
                    }
                    self.written += wire_len as u64;
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        }
        Poll::Ready(Ok(()))
    }

    fn make_frame(&mut self, buf: &[u8]) -> Result<(Vec<u8>, usize)> {
        let s = self
            .frame_size(buf.len())
            .ok_or_else(|| anyhow::anyhow!("pad: stage has neither size nor upto"))?;
        let cap = if s == 0 {
            MAX_FRAME_BODY
        } else {
            if s <= HDR {
                anyhow::bail!("pad: frame size must be > {HDR} (check stage size/upto)");
            }
            (s - HDR).min(MAX_FRAME_BODY)
        };
        let n = buf.len().min(cap);
        let pad_len = cap - n;

        let mut hdr = [0u8; 4];
        hdr[0..2].copy_from_slice(&(n as u16).to_be_bytes());
        hdr[2..4].copy_from_slice(&(pad_len as u16).to_be_bytes());
        let mut mac_msg = Vec::with_capacity(8 + 4 + n);
        mac_msg.extend_from_slice(&self.wseq.to_be_bytes());
        mac_msg.extend_from_slice(&hdr);
        mac_msg.extend_from_slice(&buf[..n]);
        let tag = hmac_tag(&self.key, &mac_msg);
        self.wseq += 1;

        let mut frame = Vec::with_capacity(HDR + n + pad_len);
        frame.extend_from_slice(&tag);
        frame.extend_from_slice(&hdr);
        frame.extend_from_slice(&buf[..n]);
        frame.resize(HDR + n + pad_len, 0);
        rand::rng().fill(&mut frame[HDR + n..]);
        log_tag!(
            trace,
            "PAD",
            "frame: stage {}, wire {}, payload {}, pad {}",
            self.idx,
            HDR + n + pad_len,
            n,
            pad_len
        );
        Ok((frame, n))
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for PadStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if this.poff < this.pending.len() {
                let n = (this.pending.len() - this.poff).min(buf.remaining());
                buf.put_slice(&this.pending[this.poff..this.poff + n]);
                this.poff += n;
                if this.poff == this.pending.len() {
                    this.pending.clear();
                    this.poff = 0;
                }
                return Poll::Ready(Ok(()));
            }

            let want = match this.rstate {
                ReadState::Hdr => HDR,
                ReadState::Body { payload, pad } => HDR + payload + pad,
            };
            while this.rbuf.len() < want {
                let mut tmp = [0u8; 8192];
                let mut rb = ReadBuf::new(&mut tmp);
                ready!(Pin::new(&mut this.inner).poll_read(cx, &mut rb))?;
                let n = rb.filled().len();
                if n == 0 {
                    if this.rbuf.is_empty() && matches!(this.rstate, ReadState::Hdr) {
                        return Poll::Ready(Ok(()));
                    }
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "pad: eof inside frame",
                    )));
                }
                this.rbuf.extend_from_slice(rb.filled());
            }

            match this.rstate {
                ReadState::Hdr => {
                    let payload = u16::from_be_bytes([this.rbuf[32], this.rbuf[33]]) as usize;
                    let pad = u16::from_be_bytes([this.rbuf[34], this.rbuf[35]]) as usize;
                    if payload > MAX_FRAME_BODY || pad > MAX_FRAME_BODY {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "pad: frame too large",
                        )));
                    }
                    this.rstate = ReadState::Body { payload, pad };
                }
                ReadState::Body { payload, pad } => {
                    let frame_len = HDR + payload + pad;
                    let body: Vec<u8> = this.rbuf[..frame_len].to_vec();
                    this.rbuf.drain(..frame_len);
                    let (tag, rest) = body.split_at(32);
                    let (hdr, rest) = rest.split_at(4);
                    let (payload_bytes, _) = rest.split_at(payload);

                    let mut mac_msg = Vec::with_capacity(8 + 4 + payload);
                    mac_msg.extend_from_slice(&this.rseq.to_be_bytes());
                    mac_msg.extend_from_slice(hdr);
                    mac_msg.extend_from_slice(payload_bytes);
                    if hmac_verify(&this.key, &mac_msg, tag).is_err() {
                        log_tag!(warn, "PAD", "bad tag, closing");
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "pad: bad tag",
                        )));
                    }
                    this.rseq += 1;
                    this.rstate = ReadState::Hdr;
                    this.pending = payload_bytes.to_vec();
                    this.poff = 0;
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
        let mut offset = 0;

        while offset < buf.len() {
            if this.wbuf.is_empty() {
                let (frame, n) = this
                    .make_frame(&buf[offset..])
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
                debug_assert!(n > 0);
                offset += n;
                this.wbuf = frame;
            }
            let wire_len = this.wbuf.len();

            match Pin::new(&mut this.inner).poll_write(cx, &this.wbuf) {
                Poll::Ready(Ok(n)) => {
                    this.wbuf.drain(..n);
                    if !this.wbuf.is_empty() {
                        break;
                    }
                    this.written += wire_len as u64;
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => break,
            }
        }

        if offset > 0 {
            Poll::Ready(Ok(offset))
        } else if this.wbuf.is_empty() {
            Poll::Ready(Ok(0))
        } else {
            let wire_len = this.wbuf.len();
            match Pin::new(&mut this.inner).poll_write(cx, &this.wbuf) {
                Poll::Ready(Ok(n)) => {
                    this.wbuf.drain(..n);
                    if this.wbuf.is_empty() {
                        this.written += wire_len as u64;
                        Poll::Ready(Ok(0))
                    } else {
                        Poll::Pending
                    }
                }
                Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
                Poll::Pending => Poll::Pending,
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.poll_drain(cx).is_pending() {
            return Poll::Pending;
        }
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.poll_drain(cx).is_pending() {
            return Poll::Pending;
        }
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}
