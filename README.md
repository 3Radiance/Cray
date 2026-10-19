<p align="left">
  <img src="./cray.jpg" width="20%" alt="crays-logo" style="border-radius: 16px; box-shadow: 0 4px 20px rgba(0,0,0,0.15);">
</p>

# Cray

English | [Русский](README.ru.md)

A custom proxy engine written in Rust.

Cray is built around a **modular, layered architecture**: transport and protocol are independent of each other. You can write your own protocol without worrying about how bytes travel across the network, or write your own transport without worrying about the protocol. Layers can be combined into any pipeline directly in the config:

```text
tcp → custom protocol
tcp → tls → custom protocol
tcp → tls → http2 → custom protocol
```

There are still relatively few built-in layers, but every new protocol or transport increases the number of possible pipelines.

> 🚧 The project is under active development. The core architectural idea is already working, but there are still many gaps (see [Status](#status)).

---

## How It Works

The configuration is a TOML file with two layer chains:

- **`inbound.layers`**: how to accept a connection;
- **`outbound.layers`**: how to connect onward.

There is no `client`/`server` flag anymore: behavior follows position. Inbound layers listen and accept, outbound layers dial. A terminal (protocol) layer at the end of outbound means this side initiates (client); at the end of inbound, it means this side accepts (server).

**Chain rules:**
- The first layer is the connection source (`tcp` or `unix`);
- The remaining layers are transports, each transforming one stream into another;
- **The protocol is always the last layer** in the chain and terminates the connection.

### Layers

| Layer | Client | Server |
|---|---|---|
| `tcp` | Inbound: listens on `addr`. Outbound: connects to `addr`. | Inbound: listens on `addr`. Outbound: without `addr`, connects directly to the destination (like “freedom/direct”). |
| `unix` | Inbound: listens on `addr`. Outbound: connects to `addr`. | Inbound: listens on `addr`. Outbound: connects to `addr`. |
| `http-proxy` | Accepts HTTP CONNECT from the browser and extracts the destination address. | Proxy chaining: sends CONNECT to a fixed `addr` through the previous stream. |
| `tls` | Client handshake: `sni` and a configurable fingerprint. | Acceptor: performs a handshake using your certificates. |
| `http2` | Opens a POST/PUT request and exposes a **virtual stream**. | HTTP/2 server: accepts streams and passes each onward as a separate stream. |
| `jitter` | Adds a random delay before every write; see [docs/eng/shaping.md](docs/eng/shaping.md). | Same, for the traffic the server sends. |
| `fragment` | Cuts every write into smaller pieces; see [docs/eng/shaping.md](docs/eng/shaping.md). | Same, for the traffic the server sends. |
| `vless` | Protocol: VLESS-style header with UUID authentication (TCP). | Same. |
| `pad` | Size equalization, a pure shaper (`stages`, no keys; see [docs/eng/shaping.md](docs/eng/shaping.md)). | Same. |
| `auth` | Frame authentication: an HMAC-SHA256 tag over every record, one shared key, no handshake; see [docs/eng/auth.md](docs/eng/auth.md). | Same (identical key on both ends). |

Placement matters for `pad`: `tcp → tls → pad` hides padding inside the encryption (recommended). `tcp → pad → tls` also works, but exposes the framing to DPI. `pad` does not authenticate; put the `auth` layer under it (`tcp → tls → auth → pad → protocol`; see [docs/eng/auth.md](docs/eng/auth.md)).

The `http2` layer, or any multiplexing transport, takes a socket and exposes a virtual stream. The protocol therefore does not care what is underneath it: raw TCP, TLS, or HTTP/2. It simply reads from and writes to a stream.

### Example: Client

```toml
# 1. Inbound: listen on a local port and parse HTTP CONNECT
[[inbound.layers]]
type = "tcp"
addr = "127.0.0.1:10810"

[[inbound.layers]]
type = "http-proxy"

# 2. Outbound: tunnel to the server
[[outbound.layers]]
type = "tcp"
addr = "127.0.0.1:10811"

[[outbound.layers]]
type = "tls"
sni = "domain.com"
tls_path = "./tls.toml"
http2_path = "./http2.toml"   # read only when alps = true

[[outbound.layers]]
type = "http2"
path = "/tun"
host = "idk.com"
method = "POST"
config = "./http2.toml"

[[outbound.layers]]
type = "vless"     # the protocol is always last
uuid = "11111111-1111-4111-8111-111111111111"   # placeholder, generate your own
```

What happens:

1. `tcp` in inbound listens on the specified address (every inbound source does this).
2. `http-proxy` extracts the destination address from the request and passes it to the protocol so it can label packets. This completes the inbound chain. Instead of HTTP CONNECT, you could write your own entry layer, such as SOCKS5; its only job would be to extract the destination address.
3. In outbound, `tcp` creates a socket and connects it to the server, `tls` encrypts it, `http2` wraps it in a virtual stream, and the protocol operates on that stream.

### Example: Server

```toml
[[inbound.layers]]
type = "tcp"
addr = "127.0.0.1:10811"

[[inbound.layers]]
type = "tls"
cert = "/path/to/fullchain.pem"
key = "/path/to/privkey.pem"
alpn = ["h2"]

[[inbound.layers]]
type = "http2"
path = "/tun"
host = "idk.com"
method = "POST"

[[inbound.layers]]
type = "vless"
uuid = "11111111-1111-4111-8111-111111111111"   # the same UUID as on the client

[[outbound.layers]]
type = "tcp"        # no address: connect to the destination requested by the protocol
```

The same layers are used here, but they act as acceptors:

- `tls` accepts the socket, performs a handshake using your certificates, and passes it onward;
- `http2` is an HTTP/2 server: it accepts only requests whose `method`, `path` and `host` match, and returns 404 to others;
- the **server-side protocol function** is then called and given the stream;
- `tcp` in outbound, when no address is specified, connects to the destination provided by the protocol.

---

## Fingerprint Configuration

Both TLS and HTTP/2 can be configured in full. Cray lets you set values manually; presets are not available yet.

```toml
# tls.toml
cipher_suites = [
    "TLS_AES_128_GCM_SHA256",
    "TLS_AES_256_GCM_SHA384",
    "TLS_CHACHA20_POLY1305_SHA256",
    "ECDHE-ECDSA-AES128-GCM-SHA256",
    "ECDHE-RSA-AES128-GCM-SHA256",
]
alpn = ["h2"]
curves = ["X25519", "P-256", "P-384"]
signature_algorithms = [
    "rsa_pkcs1_sha256",
    "rsa_pkcs1_sha384",
    "ecdsa_secp256r1_sha256",
]
permute_extensions = true
```

Fingerprints live in separate profile files that the layers point to: `tls_path` (TLS) and `config` (HTTP/2). The HTTP/2 profile, `http2.toml`, describes SETTINGS, pseudo-header and header ordering, and priorities.

For a detailed description of all fields, see [docs/eng/tls.md](docs/eng/tls.md) and [docs/eng/http2.md](docs/eng/http2.md).

On the server, the `tls` layer takes `alpn` directly; `tls_path` is ignored inbound (a warning is logged).

---

## Modules

| Module | Purpose |
|---|---|
| `core/` | The engine: layer contract (`Layer`), `Registry`, chain building and validation, the `drive` runner. Knows no layer names. |
| `layers/` | All layers, grouped by kind: `transport/` (`tls`, `http2`), `proxy/` (`http-proxy`), `protocol/` (`vless`), `wrapers/` (`jitter`, `fragment`), plus `tcp`, `unix`, `fingerprint/` builders and `profiles/` loading. A new layer is one file plus one `register` line. |
| `cray/` | The binary: parses args, registers layers, builds the engine, runs. |

---

## Running

Building requires Rust, `cmake`, and a C/C++ compiler (TLS is built on BoringSSL through `btls`).

```bash
cargo build --release
cd conf          # profile paths such as ./tls.toml are relative to the working directory
../target/release/cray --config client.toml
../target/release/cray --config server.toml
```

The example configs and profiles live in [`conf/`](conf/).

Generate your own UUID with `cargo run -p cray --bin gen_uuid` and put it into the `uuid` field of the `vless` layer on both sides. The UUIDs in the examples are placeholders, do not use them.

### Logging

The default level is `info`. Set `RUST_LOG` to change it, globally or per tag:

```bash
RUST_LOG=info ./cray --config server.toml
RUST_LOG=info,TLS=debug,FRAG=trace ./cray --config client.toml
```

Tags: `CHAIN`, `INB`, `HTTP`, `TLS`, `VLESS`, `JITTER`, `FRAG`. Per-connection details are logged at `debug` and `trace`, so `info` stays quiet.

---

## Status

Working:
- `tcp`, `unix`, `tls`, `http2`, and `http-proxy` layers, plus the `jitter`, `fragment` and `pad` shaping wrappers;
- The `auth` layer: per-frame HMAC-SHA256 authentication (v1 — tag only, no replay protection yet); see [docs/eng/auth.md](docs/eng/auth.md).
- Building client and server layer chains from config and validating layer order;
- The `vless` protocol (TCP command, UUID authentication).

> ⚠️ `vless` follows the VLESS header layout only partly and is **not interoperable** with Xray, sing-box or other VLESS clients yet. Differences: domain and IPv6 address types use the SOCKS5-style codes 3 and 4 (VLESS uses 2 and 3), the server sends no response header, request addons are assumed to be empty, and only the TCP command is supported. A Cray client works with a Cray server. The UUID is the only credential, and there is no replay protection yet.

**Plans:**
1. Full VLESS wire compatibility (see the warning above), UDP and Mux commands, and replay protection (nonce cache).
2. `auth`: replay protection (nonce cache and a masked timestamp, no handshake
   still) and an AEAD record mode (`[nonce:16 | ts':8 | len:2 | AEAD(payload)]`).
3. Two-level verification: level 1 on the nginx side (HMAC cookie, timestamp; if no cookie is present, serve a genuine static website), level 2 in the protocol (on rejection, show a mirror of the real error page rather than a TCP RST).
4. A declarative protocol description language (DSL) for people who do not want to write Rust code. Rust authors retain full flexibility; the DSL is limited to a closed set of verifiers.
5. Custom transports and protocols as extensions, HTTP/1.1 and WebSocket support, and UDP.
6. Connection multiplexing (last priority).
7. `xhttp`: split uplink/downlink over two legs with independent stacks.
   Each leg (`up`/`down`) is a full sub-chain with its own transports and
   shaping, so the two directions can look different on the wire:

```toml
[[outbound.layers]]
type = "xhttp-h2"
host = "idk.com"
path = "/tun"
method = "POST"
config = "conf/http2.toml"
# Each leg is a full sub-chain: any layers are allowed inside except extra
# transports (h2/ws — tcp/tls don't count) and protocols. Shaping (jitter,
# fragment, pad), alternative encryption instead of/alongside tls, even
# routing legs through different proxies via http-proxy:
up = [
  { type = "tcp", addr = "up.example.com:443" },
  { type = "http-proxy", addr = "xhttp.server.com:443" },
  { type = "tls", sni = "xhttp.server.com", tls_path = "./tls.toml" },
]
down = [
  { type = "tcp", addr = "down.example.com:443" },
  { type = "tls", sni = "down.example.com", tls_path = "./tls.toml" },
]
```

