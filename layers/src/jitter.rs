use crate::deserialize_from_str;
use anyhow::{Result, bail};
use cray_core::log_tag;
use cray_core::{BoxFuture, BoxedStream, BuildCx, Cx, Input, Layer, Output, Ports};
use serde::Deserialize;
use std::io;
use std::pin::Pin;
use std::str::FromStr;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::{Duration, Instant, Sleep};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JitterCfg {
    stages: Option<Vec<JitterStage>>,
}

pub struct Jitter {
    stages: Vec<JitterStage>,
}

impl Layer for Jitter {
    fn ports(&self) -> Ports {
        Ports::WRAP
    }

    fn call<'a>(&'a self, input: Input, _cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let stream: BoxedStream = input.stream()?;
            Ok(Output::Stream(Box::pin(JitterStream::new(
                stream,
                self.stages.clone(),
            ))))
        })
    }
}

#[derive(Debug, Deserialize, Clone)]
pub enum JitterDelay {
    Fixed(Duration),
    Range(Duration, Duration),
}

#[derive(Debug, Deserialize, Clone)]
pub struct JitterStage {
    pub bytes: Option<u64>,
    #[serde(deserialize_with = "deserialize_from_str")]
    pub delay: JitterDelay,
}

pub struct JitterStream<S> {
    inner: S,
    stages: Vec<JitterStage>,
    idx: usize,
    written: u64,
    slept: bool,
    armed: bool,
    sleep: Pin<Box<Sleep>>,
    rng: fastrand::Rng,
}

impl<S> JitterStream<S> {
    pub fn new(inner: S, stages: Vec<JitterStage>) -> Self {
        let initial_sleep = Box::pin(tokio::time::sleep(Duration::ZERO));
        Self {
            inner,
            stages: stages.into(),
            idx: 0,
            written: 0,
            slept: false,
            armed: false,
            sleep: initial_sleep,
            rng: fastrand::Rng::new(),
        }
    }

    fn current_delay(&mut self) -> Duration {
        while let Some(up) = self.stages[self.idx].bytes {
            if self.written >= up && self.idx + 1 < self.stages.len() {
                self.idx += 1;
            } else {
                break;
            }
        }
        let st = &self.stages[self.idx];
        st.delay.sample(&mut self.rng)
    }
}

impl JitterDelay {
    pub fn sample(&self, rng: &mut fastrand::Rng) -> Duration {
        match self {
            JitterDelay::Fixed(d) => *d,
            JitterDelay::Range(min, max) => {
                let range_ms = max.as_millis().saturating_sub(min.as_millis()) as u64;
                if range_ms == 0 {
                    return *min;
                }
                *min + Duration::from_millis(rng.u64(0..=range_ms))
            }
        }
    }
}

impl FromStr for JitterDelay {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((a, b)) = s.split_once('-') {
            let min = a
                .trim()
                .parse::<u64>()
                .map_err(|_| anyhow::anyhow!("invalid jitter min: '{}'", a))?;
            let max = b
                .trim()
                .parse::<u64>()
                .map_err(|_| anyhow::anyhow!("invalid jitter max: '{}'", b))?;
            anyhow::ensure!(min <= max, "jitter min ({}) must be <= max ({})", min, max);
            Ok(JitterDelay::Range(
                Duration::from_millis(min),
                Duration::from_millis(max),
            ))
        } else {
            let ms = s
                .trim()
                .parse::<u64>()
                .map_err(|_| anyhow::anyhow!("invalid jitter value: '{}'", s))?;
            Ok(JitterDelay::Fixed(Duration::from_millis(ms)))
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for JitterStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for JitterStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if !self.slept {
            if !self.armed {
                let d = self.current_delay();
                if d.is_zero() {
                    log_tag!(trace, "JITTER", "zero delay for stage index {}, skipping sleep", self.idx);
                    self.slept = true;
                } else {
                    log_tag!(trace, "JITTER", "Delay for stage index {}, sleeping for {:?}", self.idx, d);
                    self.sleep.as_mut().reset(Instant::now() + d);
                    self.armed = true;
                }
            }
            if self.armed {
                ready!(self.sleep.as_mut().poll(cx));
                self.armed = false;
                self.slept = true;
            }
        }

        let r = ready!(Pin::new(&mut self.inner).poll_write(cx, buf));
        self.slept = false;

        if let Ok(n) = r {
            log_tag!(trace, "JITTER", "write {n}");
            self.written += n as u64;
        }
        Poll::Ready(r)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

pub fn build(params: &toml::Table, _bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: JitterCfg = params.clone().try_into()?;
    let stages = cfg
        .stages
        .ok_or_else(|| anyhow::anyhow!("stages is required"))?;
    if stages.is_empty() {
        bail!("stages must not be empty");
    }
    Ok(Box::new(Jitter { stages }))
}
