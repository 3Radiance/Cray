use std::io;
use std::pin::Pin;
use std::str::FromStr;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::transport::jitter::lcg_rand;

#[derive(Debug, Clone)]
pub enum FragSize {
    Fixed(usize),
    Range(usize, usize),
    Percent(u8),
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
    frag_size: FragSize,
}

impl<S> FragmentStream<S> {
    pub fn new(inner: S, frag_size: FragSize) -> Self {
        Self { inner, frag_size }
    }
}

pub fn wrap<S>(inner: S, frag_size: FragSize) -> Pin<Box<FragmentStream<S>>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    Box::pin(FragmentStream::new(inner, frag_size))
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
        let size = self.frag_size.sample(buf.len());
        crate::log_tag!(
            trace,
            "FRAG",
            "write {} bytes, fragment size {}",
            buf.len(),
            size
        );
        Pin::new(&mut self.inner).poll_write(cx, &buf[..size])
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
