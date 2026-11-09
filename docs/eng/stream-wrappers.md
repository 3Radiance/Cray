# Stream Wrappers: implementing `AsyncRead` / `AsyncWrite`

Wrappers transform streams by implementing `AsyncRead` and/or `AsyncWrite` on
a struct that holds the inner stream. All of them live in
[`layers/src/wrappers/`](../../layers/src/wrappers/): `jitter`, `fragment`,
`pad` (write shaping), `auth` (framed verification). The pattern is
consistent across all wrappers, so this page walks through it with real code.

[Читать на русском ->](../ru/stream-wrappers.md)

The layer body itself stays small — it takes a stream, wraps it and hands the
wrapped stream onward:

```rust
// layers/src/wrappers/jitter.rs
fn call<'a>(&'a self, input: Input, _cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
    Box::pin(async move {
        let stream: BoxedStream = input.stream()?;
        Ok(Output::Stream(Box::pin(JitterStream::new(
            stream,
            self.stages.clone(),
        ))))
    })
}
```

## Basic pattern

```rust
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub struct MyStream<S> {
    inner: S,
    // ... your state ...
}

// Pass-through read (jitter, fragment and pad don't modify reads; auth
// does — it strips its own framing, see its state machine)
impl<S: AsyncRead + Unpin> AsyncRead for MyStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

// Transform writes
impl<S: AsyncWrite + Unpin> AsyncWrite for MyStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        // Transform buf, then forward to inner
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
```

## Key rules

1. **`Unpin` bound** — the inner stream must be `Unpin` to call
   `Pin::new(&mut self.inner)`.
2. **Propagate `Poll::Pending`** — if inner returns `Pending`, pass it on;
   if you already produced bytes in this poll, report them instead of
   pending again (see `fragment` below).
3. **Track partial writes** — `poll_write` may write fewer bytes than
   requested; return the actual count and resume from there next call.
4. **Use the `ready!` macro** — for sequential async steps inside one poll;
   it returns `Poll::Pending` early when a step is not ready yet.

## Example: delaying writes (from `jitter.rs`)

`JitterStream` sleeps a sampled delay before forwarding each write. The
single `Box<Sleep>` field is armed and reused across polls:

```rust
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
                    self.slept = true;
                } else {
                    self.sleep.as_mut().reset(Instant::now() + d);
                    self.armed = true;
                }
            }
            if self.armed {
                ready!(self.sleep.as_mut().poll(cx));  // wait for the timer
                self.armed = false;
                self.slept = true;
            }
        }

        let r = ready!(Pin::new(&mut self.inner).poll_write(cx, buf));
        self.slept = false;  // reset for the next write

        if let Ok(n) = r {
            self.written += n as u64;
        }
        Poll::Ready(r)
    }
    // ... flush and shutdown pass through
}
```

## Example: splitting writes (from `fragment.rs`)

`FragmentStream` slices one write into pieces. It loops over the remaining
bytes and must report a partial write when the inner stream turns `Pending`
mid-way:

```rust
impl<S: AsyncWrite + Unpin> AsyncWrite for FragmentStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut offset = 0;

        while offset < buf.len() {
            let current_buf = &buf[offset..];

            while let Some(up) = self.stages[self.idx].bytes {
                if self.written >= up && self.idx + 1 < self.stages.len() {
                    self.idx += 1;
                } else {
                    break;
                }
            }

            let st = &self.stages[self.idx];
            let take = match st.bytes {
                Some(up) if self.written >= up => current_buf.len(),
                Some(up) => st
                    .size
                    .sample(current_buf.len())
                    .min((up - self.written) as usize)
                    .max(1),
                None => st.size.sample(current_buf.len()),
            }
            .min(current_buf.len());

            match Pin::new(&mut self.inner).poll_write(cx, &current_buf[..take]) {
                Poll::Ready(Ok(n)) => {
                    if n == 0 {
                        return Poll::Ready(Ok(offset));
                    }
                    self.written += n as u64;
                    offset += n;
                    if n < take {
                        break;  // partial write, stop here
                    }
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => {
                    if offset > 0 {
                        return Poll::Ready(Ok(offset));  // report what we wrote
                    } else {
                        return Poll::Pending;
                    }
                }
            }
        }

        Poll::Ready(Ok(offset))
    }
    // ...
}
```

## Example: framing with an internal buffer (from `pad.rs`)

`pad` wraps each write `[payload_len:2 | pad_len:2 | payload | pad]`. A
wrapper that needs a frame in memory first keeps an internal write buffer
(`wbuf`) plus an offset (`woff`), and drains it into the inner stream before
each new write:

```rust
pub struct PadStream<S> {
    inner: S,
    wbuf: Vec<u8>,      // pending bytes to write
    woff: usize,        // offset into wbuf
    // ... other state ...
}

impl<S: AsyncWrite + Unpin> PadStream<S> {
    fn poll_drain(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.woff < self.wbuf.len() {
            let n = ready!(
                Pin::new(&mut self.inner).poll_write(cx, &self.wbuf[self.woff..])
            )?;
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

        // First, drain any pending bytes
        ready!(this.poll_drain(cx))?;

        // Build the frame in the buffer
        let n = this.make_frame(buf);

        // Try to send immediately
        if let Poll::Ready(Err(e)) = this.poll_drain(cx) {
            return Poll::Ready(Err(e));
        }

        Poll::Ready(Ok(n))  // report the original buf length as consumed
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
```

Note that `poll_write` reports `n` — the length of the **original**, unframed
buffer — because frame bytes live in `wbuf`, not in `buf`.

## Example: stateful reads (from `pad.rs`)

The reader of a framing protocol walks a state machine per
`poll_read` call, reading into temporary buffers and only forwarding
complete payloads to the caller:

```rust
#[derive(Clone, Copy)]
enum ReadState {
    Hdr { got: usize },                     // reading the header
    Payload { left: usize, pad: usize },    // reading the payload
    Pad { left: usize },                    // skipping padding
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
                    // read into the temp header buffer
                    let m = {
                        let mut rb = ReadBuf::new(&mut this.hdr[got..]);
                        ready!(Pin::new(&mut this.inner).poll_read(cx, &mut rb))?;
                        rb.filled().len()
                    };

                    if m == 0 {
                        return if got == 0 {
                            Poll::Ready(Ok(()))  // clean EOF between frames
                        } else {
                            Poll::Ready(Err(eof("header")))
                        };
                    }
                    // ... advance state, parse lengths, move to Payload ...
                }
                // ... Payload copies into the caller's ReadBuf and returns;
                // Pad reads and discards padding ...
            }
        }
    }
}
```

`ReadBuf::new(&mut this.hdr[got..])` keeps the outer buffer untouched until a
real payload piece is ready — the caller never sees padding bytes.

## Testing

The `layers/tests/pad.rs` suite exercises exactly the hard cases described
above: roundtrips over a duplex pair, wire-format checks against the raw side,
a tiny 100-byte duplex buffer that forces partial writes of every frame
(`survives_a_tiny_pipe`), mid-frame EOF (`eof_inside_a_frame_is_an_error`)
and empty frames on the wire (`empty_decoy_frames_are_skipped`). See
[writing-layers.md](writing-layers.md#how-to-test-a-layer) for the pattern.
