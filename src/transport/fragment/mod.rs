use crate::transport::jitter::deserialize_from_str;
use serde::Deserialize;
use std::io;
use std::pin::Pin;
use std::str::FromStr;
use std::task::ready;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::log_tag;
use crate::transport::jitter::lcg_rand;

#[derive(Debug, Deserialize, Clone)]
pub enum FragSize {
    Fixed(usize),
    Range(usize, usize),
    Percent(u8),
}

#[derive(Debug, Deserialize, Clone)]
pub struct FragStage {
    pub bytes: Option<u64>,
    #[serde(deserialize_with = "deserialize_from_str")]
    pub size: FragSize,
}

impl FragSize {
    pub fn sample(&self, buf_len: usize) -> usize {
        let n = match self {
            FragSize::Fixed(n) => *n,
            FragSize::Range(min, max) => {
                let range = max.saturating_sub(*min) as u64;
                if range == 0 {
                    *min
                } else {
                    *min + (lcg_rand() % range) as usize
                }
            }
            FragSize::Percent(pct) => {
                let n = buf_len * (*pct as usize) / 100;
                n.max(1)
            }
        };
        n.max(1).min(buf_len)
    }
}

impl FromStr for FragSize {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if let Some(pct) = s.strip_suffix('%') {
            let p: u8 = pct
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid fragment percent: '{}'", pct))?;
            anyhow::ensure!(
                p > 0 && p <= 100,
                "fragment percent must be 1..=100, got {}",
                p
            );
            return Ok(FragSize::Percent(p));
        }
        if let Some((a, b)) = s.split_once('-') {
            let min: usize = a
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid fragment min: '{}'", a))?;
            let max: usize = b
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid fragment max: '{}'", b))?;
            anyhow::ensure!(
                min <= max,
                "fragment min ({}) must be <= max ({})",
                min,
                max
            );
            anyhow::ensure!(min > 0, "fragment min must be > 0");
            return Ok(FragSize::Range(min, max));
        }
        let n: usize = s
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid fragment size: '{}'", s))?;
        anyhow::ensure!(n > 0, "fragment size must be > 0");
        Ok(FragSize::Fixed(n))
    }
}

pub struct FragmentStream<S> {
    inner: S,
    stages: Vec<FragStage>,
    idx: usize,
    written: u64,
}

impl<S> FragmentStream<S> {
    pub fn new(inner: S, stages: Vec<FragStage>) -> Self {
        Self {
            inner,
            stages,
            idx: 0,
            written: 0,
        }
    }
}

pub fn wrap<S>(inner: S, stages: Vec<FragStage>) -> Pin<Box<FragmentStream<S>>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    Box::pin(FragmentStream::new(inner, stages))
}

impl<S: AsyncRead + Unpin> AsyncRead for FragmentStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for FragmentStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        while let Some(up) = self.stages[self.idx].bytes {
            if self.written >= up && self.idx + 1 < self.stages.len() {
                self.idx += 1
            } else {
                break;
            }
        }
        let st = &self.stages[self.idx];
        let take = match st.bytes {
            Some(up) if self.written >= up => buf.len(),
            Some(up) => st
                .size
                .sample(buf.len())
                .min((up - self.written) as usize)
                .max(1),
            None => st.size.sample(buf.len()),
        }
        .min(buf.len());
        log_tag!(
            trace,
            "FRAG",
            "stage: {}, written: {}, buf_len: {}, take: {}",
            self.idx,
            self.written,
            buf.len(),
            take
        );
        let n = ready!(Pin::new(&mut self.inner).poll_write(cx, &buf[..take]))?;
        self.written += n as u64;
        log_tag!(trace, "FRAG", "write {}", n);
        Poll::Ready(Ok(n))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
