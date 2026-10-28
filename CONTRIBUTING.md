# Contributing to Cray

Thanks for your interest in contributing! 🎉

[Читать на русском ->](./CONTRIBUTING.ru.md)

## Getting Started

1. Fork the repository
2. Create a branch for your feature or fix:
   ```bash
   git checkout -b feature/my-layer
   ```
3. Make your changes and commit them
4. Push the branch and open a Pull Request

## Development Environment

Building requires Rust, `cmake`, and a C/C++ compiler (TLS is built on BoringSSL through `btls`).

```bash
cargo build --release
cargo test --workspace
```

## Code Style

- Formatting is enforced with `rustfmt` — run `cargo fmt --all` before committing
- Lints are enforced with `clippy` — run `cargo clippy --workspace` to keep warnings at zero
- CI runs both checks on every push

---

## Writing a Custom Layer

Layers are the building blocks of Cray. Each layer does one thing: accepts a connection, transforms a stream, or implements a protocol. Layers are composable — the engine chains them based on your config.

### Layer Anatomy

Every layer consists of **three parts**:

| Part | Purpose |
|------|---------|
| **Config struct** | Deserializes TOML parameters from the config file |
| **Layer struct** | Holds runtime state; implements the `Layer` trait |
| **`build()` function** | Factory that parses config and constructs the layer |

### Step-by-Step: Creating a Layer

Let's create a simple layer called `delay` that adds a fixed delay before forwarding data.

#### 1. Define the Config Struct

```rust
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]  // Rejects unknown fields — catches typos early
struct DelayCfg {
    millis: Option<u64>,        // Optional field
}
```

The config struct maps directly to the TOML table in your config file:

```toml
[[outbound.layers]]
type = "delay"
millis = 100
```

#### 2. Define the Layer Struct

```rust
use cray_core::{BuildCx, Layer, Ports, Side};
use std::time::Duration;

pub struct Delay {
    millis: u64,
    side: Side,  // Stored at build time — determines behavior in call()
}
```

The `side` field is **required**. It tells the layer whether it's running as inbound or outbound, so `call()` can dispatch to the right logic.

#### 3. Implement the `Layer` Trait

```rust
use cray_core::{BoxFuture, BoxedStream, Cx, Input, Output};
use anyhow::Result;

impl Layer for Delay {
    // What this layer does to the stream
    fn ports(&self) -> Ports {
        Ports::WRAP  // Takes a stream, gives a stream
    }

    // Declare what this layer needs from the context
    fn requires(&self) -> Vec<cray_core::Key> {
        Vec::new()  // This layer needs nothing
    }

   // Declare what this layer provides to other layers via Cx
    fn provides(&self) -> Vec<cray_core::Key> {
        Vec::new()  // This layer provides nothing
    }

    // The main logic — called when the chain reaches this layer
    fn call<'a>(&'a self, input: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            match self.side {
                Side::Inbound => {
                    // Inbound logic: accept/process incoming
                    let stream: BoxedStream = input.stream()?;
                    // ... transform ...
                    Ok(Output::Stream(stream))
                }
                Side::Outbound => {
                    // Outbound logic: dial/connect onward
                    let stream: BoxedStream = input.stream()?;
                    // ... transform ...
                    Ok(Output::Stream(stream))
                }
            }
        })
    }
}
```

#### Sharing Custom Data Through Cx

The `Cx` context is not limited to built-in types — you can pass any `Send + Sync` struct between layers. This lets you link layers that know nothing about each other directly.
**Example**: a handshake layer produces session keys, a crypto layer consumes them:
```rust
// Define your shared struct
pub struct SessionKeys {
    pub send_key: [u8; 32],
    pub recv_key: [u8; 32],
}

// Producer layer — stores keys in Cx
impl Layer for Handshake {
    fn provides(&self) -> Vec<Key> {
        vec![key::<SessionKeys>()]
    }

    fn call<'a>(&'a self, input: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let stream = input.stream()?;
            let keys = do_handshake(stream).await?;
            cx.put(keys);  // Now available to downstream layers
            Ok(Output::Stream(stream))
        })
    }
}

// Consumer layer — reads keys from Cx
impl Layer for Crypto {
    fn requires(&self) -> Vec<Key> {
        vec![key::<SessionKeys>()]
    }

    fn call<'a>(&'a self, input: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let keys = cx.get::<SessionKeys>()
                .ok_or_else(|| anyhow!("no SessionKeys in Cx"))?;
            // ... use keys ...
        })
    }
}
```
There are no restrictions on what you can store — any struct, enum, or collection works as long as it's `Send + Sync + 'static`. The engine validates at startup that all requires() are satisfied by some provides() earlier in the chain.

#### 4. Write the `build()` Function

```rust
pub fn build(params: &toml::Table, bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: DelayCfg = params.clone().try_into()?;
    let millis = cfg.millis.unwrap_or(50);  // Default value

    Ok(Box::new(Delay {
        millis,
        side: bcx.side,  // Engine tells us which side we're on
    }))
}
```

#### 5. Register the Layer

Add your layer to `layers/src/lib.rs`:

```rust
pub fn register(r: &mut Registry) {
    // ... existing layers ...
    r.add("delay", wrappers::delay::build);  // Add this line
}
```

Create the module file at `layers/src/wrappers/delay.rs` and add `pub mod delay;` to `layers/src/wrappers/mod.rs`.

---

## Understanding `Ports`

`ports()` tells the engine what your layer does. Choose the right constant:

| Constant | Takes | Gives | Use Case |
|----------|-------|-------|----------|
| `Ports::SOURCE` | Nothing | `Stream` | Creates a connection (outbound dial) |
| `Ports::LISTENER` | Nothing | `Streams` | Accepts connections (inbound listen) |
| `Ports::WRAP` | `Stream` | `Stream` | Transforms one stream into another |
| `Ports::ACCEPT` | `Stream` | `Streams` | Accepts requests on a stream, produces multiple |
| `Ports::TERMINAL` | `Stream` | `Done` | Protocol handler — consumes the stream |

**Examples:**
- `tcp` uses `SOURCE` (outbound) / `LISTENER` (inbound)
- `tls` uses `WRAP` — takes encrypted stream, gives decrypted
- `http2` uses `ACCEPT` — takes a connection, gives virtual streams
- `vless` uses `TERMINAL` — handles the protocol, then done

---

## Using `requires()` and `provides()`

Layers communicate through the shared `Cx` (context) object:

```rust
// If your layer needs a ConnectRequest (e.g., to know where to connect):
fn requires(&self) -> Vec<Key> {
    match self.side {
        Side::Outbound => vec![key::<ConnectRequest>()],
        _ => Vec::new(),
    }
}

// Access it in call():
let req = cx.get::<ConnectRequest>()
    .ok_or_else(|| anyhow!("no ConnectRequest in Cx"))?;
```

Available context keys:
- `ConnectRequest` — target address/port (set by inbound protocol layers)
- `Dialer` — used by terminal layers to create outbound connections

---

## Side-Specific Behavior

Store `side` in your struct and branch in `call()`:

```rust
fn call<'a>(&'a self, input: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
    Box::pin(async move {
        match self.side {
            Side::Inbound => self.accept(input, cx).await,
            Side::Outbound => self.dial(input, cx).await,
        }
    })
}
```

Validate config in `build()` based on side:

```rust
pub fn build(params: &toml::Table, bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: MyCfg = params.clone().try_into()?;
    match bcx.side {
        Side::Inbound => {
            if cfg.cert.is_none() {
                bail!("my-layer inbound requires 'cert'");
            }
        }
        Side::Outbound => {
            if cfg.sni.is_none() {
                bail!("my-layer outbound requires 'sni'");
            }
        }
    }
    // ...
}
```

---

## Implementing `AsyncRead` / `AsyncWrite`

Wrappers transform streams by implementing `AsyncRead` and/or `AsyncWrite`. The pattern is consistent across all wrappers (`jitter`, `fragment`, `pad`).

### Basic Pattern

```rust
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub struct MyStream<S> {
    inner: S,
    // ... your state ...
}

impl<S> MyStream<S> {
    pub fn new(inner: S) -> Self {
        Self { inner }
    }
}

// Pass-through read (most wrappers don't modify reads)
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

### Key Rules

1. **`Unpin` bound** — inner stream must be `Unpin` to call `Pin::new(&mut self.inner)`
2. **Return `Poll::Pending`** — if inner returns `Pending`, propagate it
3. **Track partial writes** — `poll_write` may write fewer bytes than requested; return actual count
4. **Use `ready!` macro** — for sequential async operations within a single poll

### Example: Delaying Writes (from `jitter.rs`)

```rust
impl<S: AsyncWrite + Unpin> AsyncWrite for JitterStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        // Check if we need to sleep first
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
                ready!(self.sleep.as_mut().poll(cx));  // Wait for timer
                self.armed = false;
                self.slept = true;
            }
        }

        // Forward to inner stream
        let r = ready!(Pin::new(&mut self.inner).poll_write(cx, buf));
        self.slept = false;  // Reset for next write

        if let Ok(n) = r {
            self.written += n as u64;
        }
        Poll::Ready(r)
    }
    // ... flush and shutdown pass through
}
```

### Example: Splitting Writes (from `fragment.rs`)

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
            let take = self.calculate_take(current_buf.len());

            match Pin::new(&mut self.inner).poll_write(cx, &current_buf[..take]) {
                Poll::Ready(Ok(n)) => {
                    if n == 0 {
                        return Poll::Ready(Ok(offset));
                    }
                    self.written += n as u64;
                    offset += n;
                    if n < take {
                        break;  // Partial write, stop here
                    }
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => {
                    // If we wrote something, report it; else propagate Pending
                    return if offset > 0 {
                        Poll::Ready(Ok(offset))
                    } else {
                        Poll::Pending
                    };
                }
            }
        }

        Poll::Ready(Ok(offset))
    }
    // ...
}
```

### Example: Framing with Internal Buffer (from `pad.rs`)

When your wrapper needs to buffer data (e.g., add headers, padding), use an internal write buffer:

```rust
pub struct PadStream<S> {
    inner: S,
    wbuf: Vec<u8>,      // Pending bytes to write
    woff: usize,        // Offset into wbuf
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

        // First, drain any pending bytes
        ready!(this.poll_drain(cx))?;

        // Build frame in buffer
        let n = this.make_frame(buf);

        // Try to send immediately
        if let Poll::Ready(Err(e)) = this.poll_drain(cx) {
            return Poll::Ready(Err(e));
        }

        Poll::Ready(Ok(n))  // Report original buf length as consumed
    }
}
```

### Example: Stateful Reads (from `pad.rs`)

For protocols with framing, track read state:

```rust
enum ReadState {
    Hdr { got: usize },           // Reading header
    Payload { left: usize, pad: usize },  // Reading payload
    Pad { left: usize },          // Skipping padding
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
                    // Read into temp buffer
                    let mut rb = ReadBuf::new(&mut this.hdr[got..]);
                    ready!(Pin::new(&mut this.inner).poll_read(cx, &mut rb))?;
                    let m = rb.filled().len();

                    if m == 0 {
                        return if got == 0 {
                            Poll::Ready(Ok(()))  // Clean EOF
                        } else {
                            Poll::Ready(Err(eof("header")))
                        };
                    }
                    // ... update state ...
                }
                // ... handle other states ...
            }
        }
    }
}
```

---

## Adding a Timeout

```rust
fn default_timeout(&self) -> Option<Duration> {
    Some(Duration::from_secs(10))
}
```

---

## Logging

Use tagged logging for consistent output:

```rust
use cray_core::log_tag;

log_tag!(debug, "DELAY", "Processing connection");
log_tag!(trace, "DELAY", "Detailed info: {:?}", data);
log_tag!(warn, "DELAY", "Something odd: {}", e);
```

Available tags: `CHAIN`, `INB`, `HTTP`, `TLS`, `VLESS`, `JITTER`, `FRAG`. Use your layer's name in uppercase.

---

## PR Guidelines

- Keep PRs focused: one feature or fix per PR
- Add or update tests for changes where it makes sense
- Update the README (and `README.ru.md`) if your change affects the docs
- Run `cargo fmt --all` and `cargo clippy --workspace` before pushing

## License

By contributing, you agree that your contributions will be licensed under the [GPL-3.0-or-later](./LICENSE) license.

---

See the list of contributors in [CONTRIBUTORS.md](./CONTRIBUTORS.md).
