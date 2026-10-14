use std::io;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, ready};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::{Sleep, sleep};

#[derive(Debug, Clone)]
pub enum JitterDelay {
    Fixed(Duration),
    Range(Duration, Duration),
}

impl JitterDelay {
    pub fn sample(&self) -> Duration {
        match self {
            JitterDelay::Fixed(d) => *d,
            JitterDelay::Range(min, max) => {
                let range_ms = max.as_millis().saturating_sub(min.as_millis()) as u64;
                if range_ms == 0 {
                    return *min;
                }
                *min + Duration::from_millis(lcg_rand() % range_ms)
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

pub struct JitterStream<S> {
    inner: S,
    delay: JitterDelay,
    sleep: Option<Pin<Box<Sleep>>>,
}

impl<S> JitterStream<S> {
    pub fn new(inner: S, delay: JitterDelay) -> Self {
        Self {
            inner,
            delay,
            sleep: None,
        }
    }
}

pub fn wrap<S>(inner: S, delay: JitterDelay) -> Pin<Box<JitterStream<S>>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    Box::pin(JitterStream::new(inner, delay))
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
        if self.sleep.is_none() {
            let dur = self.delay.sample();
            crate::log_tag!(
                trace,
                "JITTER",
                "write {} bytes, sleeping {:?}",
                buf.len(),
                dur
            );
            self.sleep = Some(Box::pin(sleep(dur)));
        }

        {
            let sl = self.sleep.as_mut().unwrap();
            ready!(sl.as_mut().poll(cx));
        }

        self.sleep = None;
        crate::log_tag!(
            trace,
            "JITTER",
            "sleep done, forwarding {} bytes",
            buf.len()
        );
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

pub(crate) fn lcg_rand() -> u64 {
    static STATE: AtomicU64 = AtomicU64::new(0);

    let mut state = STATE.load(Ordering::Relaxed);
    if state == 0 {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0xdeadbeef_cafebabe);
        state = seed | 1;
    }

    state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    STATE.store(state, Ordering::Relaxed);

    state >> 33
}
