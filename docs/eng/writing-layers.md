# Writing Layers

Layers are the building blocks of Cray. Each layer does one thing: accepts a
connection, transforms a stream, or implements a protocol. Layers are
composable — the engine chains them based on your config, and validates the
chain when the program starts.

This page walks through the layer contract, a complete small layer, Ports,
`Cx`, timeouts, logging and testing. For the poll-level details of
`AsyncRead`/`AsyncWrite` wrappers, see [stream-wrappers.md](stream-wrappers.md).

[Читать на русском ->](../ru/writing-layers.md)

## The layer contract

Every layer implements the `Layer` trait in `core/src/api.rs`:

| Method | Purpose | Default |
|--------|---------|---------|
| `ports()` | What the layer takes and gives; the engine validates chain order with it | required |
| `requires()` | `Cx` keys the layer needs from earlier layers | empty |
| `provides()` | `Cx` keys the layer puts for other layers | empty |
| `default_timeout()` | Timeout for `call()` when the config is silent | `None` |
| `call()` | The main logic: consume `Input`, return `Output` | required |

`Input` is either `Input::None` (nothing yet — this is what a source layer
receives) or `Input::Stream(BoxedStream)`. `Output` is one of `Stream` (one
stream to hand to the next layer), `Streams` (a connection acceptor — each
accepted stream starts a new run of the rest of the chain), `Done` (terminal
layer finished) or `Consumed`.

Sessions stay stateless from the engine's point of view: layers that need
shared state (session tables, counters) own it internally.

## Layer anatomy

Every layer consists of **three parts**:

| Part | Purpose |
|------|---------|
| **Config struct** | Deserializes TOML parameters from the config file |
| **Layer struct** | Holds runtime state; implements the `Layer` trait |
| **`build()` function** | Factory that parses config and constructs the layer |

All real layers live in `layers/src`, grouped by kind:
`transport/`, `proxy/`, `protocol/`, `wrappers/`, plus `tcp.rs`, `unix.rs`.

## Step-by-step: the `fragment` layer

Let's walk through a real small layer, `fragment` from
[`layers/src/wrappers/fragment.rs`](../../layers/src/wrappers/fragment.rs).
It takes a stream and cuts every write into smaller pieces.

### 1. Define the config struct

```rust
use serde::Deserialize;

use crate::deserialize_from_str;

#[derive(Debug, Deserialize, Clone)]
pub struct FragStage {
    pub bytes: Option<u64>,          // stage upper bound, in total written bytes
    #[serde(deserialize_with = "deserialize_from_str")]
    pub size: FragSize,              // parsed from a TOML string like "600-1000"
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FragmentCfg {
    stages: Option<Vec<FragStage>>,
}
```

The config struct maps directly to the TOML table of the layer:

```toml
[[outbound.layers]]
type = "fragment"
stages = [
    { bytes = 2000, size = "1-1" },
    { size = "600-1000" },
]
```

Notes:

- `#[serde(deny_unknown_fields)]` rejects unknown fields — typos fail with a
  message instead of being ignored.
- `deserialize_from_str` is a helper in `layers/src/lib.rs` that parses a
  string value (here `"1-1"`) through the type's `FromStr` impl and reports
  the error as a config error.
- `params.clone().try_into()?` in `build()` is how a factory turns the TOML
  table into this struct: `deny_unknown_fields` makes the table strictly
  match.

### 2. Define the layer struct

```rust
pub struct Fragment {
    stages: Vec<FragStage>,
}
```

No `Side` field here — `fragment` behaves the same inbound and outbound. See
[Side-Specific Behavior](#side-specific-behavior) below for when you do need
one.

### 3. Implement the `Layer` trait

```rust
use anyhow::Result;
use cray_core::{BoxFuture, BoxedStream, Cx, Input, Layer, Output, Ports};

impl Layer for Fragment {
    fn ports(&self) -> Ports {
        Ports::WRAP // takes one stream, gives one stream
    }

    fn call<'a>(&'a self, input: Input, _cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let stream: BoxedStream = input.stream()?;
            Ok(Output::Stream(Box::pin(FragmentStream::new(
                stream,
                self.stages.clone(),
            ))))
        })
    }
}
```

`input.stream()?` unwraps `Input::Stream` and fails with a clear error if the
chain gave this layer nothing. `requires()`, `provides()` and
`default_timeout()` have default implementations, so a layer that needs
nothing may leave them out.

### 4. Write the `build()` function

```rust
use anyhow::{Result, bail};
use cray_core::{BuildCx, Layer};

pub fn build(params: &toml::Table, _bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: FragmentCfg = params.clone().try_into()?;
    let stages = cfg
        .stages
        .ok_or_else(|| anyhow::anyhow!("stages is required"))?;
    if stages.is_empty() {
        bail!("stages must not be empty");
    }
    Ok(Box::new(Fragment { stages }))
}
```

Everything that can be rejected without running — unknown fields, missing
required fields, out-of-range sizes — belongs here, not in `call()`. The
engine builds all chains at startup and fails fast, so a bad config never
reaches `call()`.

### 5. Register the layer

Layer names are keys in a `Registry`, populated in
[`layers/src/lib.rs`](../../layers/src/lib.rs):

```rust
pub fn register(r: &mut Registry) {
    // ...
    r.add("xhttp2", transport::xhttp2::build);
    r.add("pipe", protocol::pipe::build);
}
```

Create the module file (for example `layers/src/wrappers/fragment.rs`) and
add `pub mod fragment;` to the parent `mod.rs`. The binary in `cray/src/main.rs`
calls `cray_layers::register()`, so nothing else is needed. Registering a
duplicate name panics at startup.

The name in `r.add("...")` is the `type = "..."` value in TOML. Layer names
are lowercase, hyphen-separated.

## Understanding `Ports`

`ports()` tells the engine what your layer does. A `Ports` value is a
`(takes, gives)` pair over `Port::{Nothing, Stream, Streams, Done}`. Choose
the right constant:

| Constant | Takes | Gives | Use Case |
|----------|-------|-------|----------|
| `Ports::SOURCE` | `Nothing` | `Stream` | Produces a single connection: outbound socket dialing, or an inbound source that yields one stream instead of accepting many |
| `Ports::LISTENER` | `Nothing` | `Streams` | Accepts incoming connections continuously (inbound listen) |
| `Ports::WRAP` | `Stream` | `Stream` | Transforms one stream into another |
| `Ports::ACCEPT` | `Stream` | `Streams` | Multiplexing: accepts requests on the stream and exposes them as separate streams |
| `Ports::TERMINAL` | `Stream` | `Done` | Protocol handler — consumes the stream |

Real layers:

- `tcp` / `unix` are `SOURCE` outbound and `LISTENER` inbound;
- `tls` is `WRAP` — takes a stream, gives a (de)crypted stream, both ways;
- `http-proxy` is `WRAP` — CONNECT through the previous stream, both ways;
- `http2` / `xhttp2` are `WRAP` outbound (one request → one virtual stream)
  and `ACCEPT` inbound (accept requests, each matching one gives a virtual
  stream);
- `vless` and `pipe` are `TERMINAL`.

Chain rules the engine enforces at startup:

- the first layer must take `Nothing`;
- every neighboring pair must link up (`Stream`/`Streams` → `Stream`);
- the last inbound layer must still feed a stream — anything that gives
  `Done` in the middle of a list is an error;
- protocol layers (`gives = Done`) must be contiguous at the end of the list,
  and exactly one of them is the terminal;
- every outbound layer must give a single stream, so it can be used for
  dialing.

## Using `requires()`, `provides()` and `Cx`

Layers communicate through the shared `Cx` (context) object. It maps type
keys to values: `key::<T>()` returns the `Key` for type `T`, `cx.put(v)`
stores a value by its type, `cx.get::<T>()` reads it back. Values must be
`Send + Sync`; there is no fixed schema, any type works, and the engine
checks at startup that every `requires()` is satisfied by some earlier
`provides()` in the chain.

**Example**: a handshake layer produces session keys, a crypto layer
consumes them:

```rust
// Define your shared struct
pub struct SessionKeys {
    pub send_key: [u8; 32],
    pub recv_key: [u8; 32],
}

use anyhow::{anyhow, Result};
use cray_core::{BoxFuture, Cx, Input, Output};

// Producer layer — stores keys in Cx
impl Layer for Handshake {
    fn provides(&self) -> Vec<cray_core::Key> {
        vec![cray_core::key::<SessionKeys>()]
    }

    fn call<'a>(&'a self, input: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let mut stream = input.stream()?;
            let mut keys = SessionKeys { send_key: [0; 32], recv_key: [0; 32] };
            do_handshake(&mut stream, &mut keys).await?;
            cx.put(keys);  // now available to downstream layers
            Ok(Output::Stream(stream))
        })
    }
}

// Consumer layer — reads keys from Cx
impl Layer for Crypto {
    fn requires(&self) -> Vec<cray_core::Key> {
        vec![cray_core::key::<SessionKeys>()]
    }

    fn call<'a>(&'a self, input: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let keys = cx
                .get::<SessionKeys>()
                .ok_or_else(|| anyhow!("no SessionKeys in Cx"))?;
            let stream = input.stream()?;
            Ok(Output::Stream(stream)) // ... use keys in a wrapper here ...
        })
    }
}
```

(`do_handshake` here stands for your own handshake code.)

### Built-in context keys

Two types live in the context out of the box, both defined in
`core/src/api.rs` and `core/src/dial.rs`:

- **`ConnectRequest`** — the target address for this connection
  (`addr: String`, `port: u16`). Layers earlier in the chain put the actual
  value into the context during the run: `http-proxy` inbound parses an HTTP
  CONNECT request and puts the parsed target there. At startup the engine
  already knows where the value will come from for outbound layers: the
  dialer copies the context and inserts the target when the outbound chain
  is dialed (`Dialer::dial_to`), so outbound layers and dial stages may
  declare `ConnectRequest` in `requires()`. `tcp` outbound with no `addr`
  dials it. A terminal layer may require it only if something earlier in its
  chain provides it.
- **`Dialer`** — how to create an outbound connection. The engine injects
  `Dialer` for the terminal layer and for intermediate stages itself, no
  layer needs to declare it in `provides()`. `Dialer::dial_to(cx, target)`
  dials the target through the wrapped outbound chain and puts its
  `ConnectRequest` into the stage's copy of the context.

## Side-Specific Behavior

`build()` receives a `BuildCx { side: Side }` — `Side::Inbound` or
`Side::Outbound`. Storing `side` in your struct is not required in general
(`pipe`, for example, does not keep it); store it if your layer behaves
differently inbound and outbound.

Layer structs that do store it branch in `ports()`, `requires()` and
`call()`:

```rust
fn ports(&self) -> Ports {
    match self.side {
        Side::Inbound => Ports::LISTENER,
        Side::Outbound => Ports::SOURCE,
    }
}

fn requires(&self) -> Vec<Key> {
    match self.side {
        Side::Outbound if self.addr.is_none() => vec![key::<ConnectRequest>()],
        _ => Vec::new(),
    }
}
```

(from `layers/src/tcp.rs` — real code)

Validate config against the side in `build()`:

```rust
pub fn build(params: &toml::Table, bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: HTTPConnectCfg = params.clone().try_into()?;
    if cfg.addr.is_none() && bcx.side == Side::Outbound {
        return Err(anyhow!("Client HTTPConnect Need addr"));
    }
    // ...
}
```

(from `layers/src/proxy/http.rs` — real code)

## Default timeout and timeouts in the config

The engine applies a timeout around layer `call()`s in the inbound and
outbound chains and in the dial chain (`core/src/engine.rs`,
`core/src/dial.rs`):

- the layer declares a default via `default_timeout()`:

  ```rust
  fn default_timeout(&self) -> Option<Duration> {
      Some(Duration::from_secs(5))
  }
  ```
- a per-layer `timeout = <seconds>` in TOML overrides it;
- a named preset from the top-level `[timeouts]` table also overrides it
  (`timeout = "handshake"` → `timeouts.handshake` in seconds);
- `timeout = 0` disables the timeout for this layer.

The default timeout applies only when the config is silent for the layer.
On timeout, `call()` is cancelled and the connection is dropped with a
`timed out after …` error.

## Logging

Use tagged logging for consistent output:

```rust
use cray_core::log_tag;

log_tag!(debug, "HTTP", "CONNECT {}:{}", cr.addr, cr.port);
log_tag!(trace, "JITTER", "write {n}");
log_tag!(warn, "INB", "accept failed: {}", e);
```

The tag goes both into the `tracing` target and as a `[TAG]` prefix into the
message text. Use your layer's name in uppercase with no separator
(`FRAG` for `fragment`). A tag may be matched in `RUST_LOG`:

```bash
RUST_LOG=info,TLS=debug,FRAG=trace ./cray --config client.toml
```

Tags in use across the codebase:

| Tag | Who logs under it |
|-----|-------------------|
| `CHAIN` | `core` — engine, chain building, dialer |
| `INB` | inbound listeners (`tcp`, `unix`) |
| `HTTP` | `http-proxy` |
| `TLS` / `CERT` | `tls`, TLS builder and certificates |
| `H2` | `http2` |
| `XH2` | `xhttp2` |
| `VLESS` | `vless` |
| `JITTER` / `FRAG` / `PAD` | `jitter`, `fragment`, `pad` |

## How to test a layer

Integration tests live in `layers/tests/` (`pad.rs`, `auth.rs`, `http2.rs`,
`tls.rs`, `xhttp2.rs`, `reorder.rs`, `gc.rs`, `profiles.rs`). The canonical
pattern for wrapper layers is [`layers/tests/pad.rs`](../../layers/tests/pad.rs):

1. **Build the layer the way the engine does** — via the real registry, so
   the test also verifies registration:

   ```rust
   use cray_core::*;
   use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

   fn try_pad(stages: &str, side: Side) -> anyhow::Result<Box<dyn Layer>> {
       let mut reg = Registry::new();
       cray_layers::register(&mut reg);
       let f = reg.get("pad").expect("pad registered");
       let params: toml::Table = format!("stages = [{stages}]").parse().unwrap();
       f(&params, &BuildCx { side })
   }
   ```

2. **Run the layer over a duplex pair** — call the layer directly and wrap
   both ends, then assert the roundtrip:

   ```rust
   async fn roundtrip(stages: &str, msg: &[u8]) -> Vec<u8> {
       let (a, b) = duplex(65536);
       let mut up = wrap(&*pad_layer(stages, Side::Outbound), Box::pin(a) as BoxedStream).await;
       let mut dn = wrap(&*pad_layer(stages, Side::Inbound), Box::pin(b) as BoxedStream).await;

       up.write_all(msg).await.unwrap();
       up.shutdown().await.unwrap();

       let mut out = Vec::new();
       dn.read_to_end(&mut out).await.unwrap();
       out
   }
   ```

3. **Test the wire format directly** — where a layer adds framing, take the
   raw side of the duplex and parse its bytes, as in `pad.rs`
   (`frames(&wire)`, `wire_of(...)`).
4. **Test build-time validation** — call the factory with bad parameters and
   match on the error text, as in `build_err(...)` in `pad.rs`.
5. **Force partial writes** — a small duplex buffer (100 bytes) makes every
   frame write partial, which is how `pad.rs` exercises its draining loop
   (`survives_a_tiny_pipe`).

Run the suite with `cargo test --workspace`, or one crate with
`cargo test -p cray-layers`.

## Checklist for a new layer

- code: one module under `layers/src/`, registered in `layers/src/lib.rs`
- tests in `layers/tests/`
- a page in `docs/eng/` **and** `docs/ru/`
- updated `README.md` / `README.ru.md`, if the layer changes the list of
  capabilities

The process-level requirements (branches, commit format) are in
[CONTRIBUTING.md](../../CONTRIBUTING.md).
