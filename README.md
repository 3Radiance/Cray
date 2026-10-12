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

The configuration is a TOML file. First, specify the role, `client` or `server`, so the proxy can distinguish their functions and method calls. Then define two layer chains:

- **`inbound.layers`**: how to accept a connection;
- **`outbound.layers`**: how to connect onward.

The role changes the meaning of the layers: on the client, inbound is the browser-facing entry point, while outbound is the tunnel to the server; on the server, inbound accepts the tunnel, while outbound connects to the internet.

**Chain rules:**
- The first layer is the connection source (`tcp` or `unix`);
- The remaining layers are transports, each transforming one stream into another;
- **The protocol is always the last layer** in the chain and terminates the connection.

### Layers

| Layer | Client | Server |
|---|---|---|
| `tcp` | Inbound: listens on `addr`. Outbound: connects to `addr`. | Inbound: listens on `addr`. Outbound: without `addr`, connects directly to the destination (like “freedom/direct”). |
| `unix` | Listens on a Unix socket. | Listens on a Unix socket. |
| `http_connect` | Accepts HTTP CONNECT from the browser and extracts the destination address. | Not used. |
| `tls` | Client handshake: `sni` and a configurable fingerprint. | Acceptor: performs a handshake using your certificates. |
| `http2` | Opens a POST/PUT request and exposes a **virtual stream**. | HTTP/2 server: accepts streams and passes each onward as a separate stream. |
| `tsf` | Protocol (test). | Protocol (test). |

The `http2` layer, or any multiplexing transport, takes a socket and exposes a virtual stream. The protocol therefore does not care what is underneath it: raw TCP, TLS, or HTTP/2. It simply reads from and writes to a stream.

### Example: Client

```toml
role = "client"

# 1. Inbound: listen on a local port and parse HTTP CONNECT
[[inbound.layers]]
type = "tcp"
addr = "127.0.0.1:10810"

[[inbound.layers]]
type = "http_connect"

# 2. Outbound: tunnel to the server
[[outbound.layers]]
type = "tcp"
addr = "127.0.0.1:10811"

[[outbound.layers]]
type = "tls"
sni = "domain.com"

[[outbound.layers]]
type = "http2"
path = "/tun"
host = "idk.com"
method = "POST"

[[outbound.layers]]
type = "tsf"      # the protocol is always last
magic = 432
```

What happens:

1. `tcp` in inbound listens on the specified address (every inbound layer does this, regardless of role).
2. `http_connect` extracts the destination address from the request and passes it to the protocol so it can label packets. This completes the inbound chain. Instead of HTTP CONNECT, you could write your own entry layer, such as SOCKS5; its only job would be to extract the destination address.
3. In outbound, `tcp` creates a socket and connects it to the server, `tls` encrypts it, `http2` wraps it in a virtual stream, and the protocol operates on that stream.

### Example: Server

```toml
role = "server"

[[inbound.layers]]
type = "tcp"
addr = "127.0.0.1:10811"

[[inbound.layers]]
type = "tls"
cert = "/path/to"
key = "/path/to"

[[inbound.layers]]
type = "http2"
path = "/tun"
host = "idk.com"
method = "POST"

[[inbound.layers]]
type = "tsf"
magic = 432

[[outbound.layers]]
type = "tcp"        # no address: connect to the destination requested by the protocol

[tls]
alpn = ["h2"]
```

The same layers are used here, but they act as acceptors:

- `tls` accepts the socket, performs a handshake using your certificates, and passes it onward;
- `http2` is an HTTP/2 server: it accepts only requests whose `host` and `path` match, and returns 404 to others (`method` is not checked yet);
- the **server-side protocol function** is then called and given the stream;
- `tcp` in outbound, when no address is specified, connects to the destination provided by the protocol.

---

## Fingerprint Configuration

Both TLS and HTTP/2 can be configured in full. Cray lets you set values manually; presets are not available yet.

```toml
[tls]
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

The `[http2]` section describes the HTTP/2 fingerprint: SETTINGS, pseudo-header and header ordering, and priorities.

For a detailed description of all fields, see [docs/TLS.md](docs/eng/tls.md) and [docs/HTTP2.md](docs/eng/http2.md).

On the server, HTTP/2 requires `alpn = ["h2"]` in `[tls]`.

---

## Modules

| Module | Purpose |
|---|---|
| `inbound/` | Inbound layers: `tcp`, `unix`, `http_connect`, as well as custom ones (e.g. SOCKS5). |
| `transport/` | Transport layers: `tls`, `http2`. |
| `protocol/` | Protocol: the final layer in the chain. |
| `outbound/` | Outbound: a direct connection or a chain of layers. |
| `auth/` | Basic cryptography and authentication methods (under development). |
| `config/` | Configuration loading and validation. |
| `fingerprint/` | TLS/HTTP2 builders. |

---

## Running

Building requires Rust, `cmake`, and a C/C++ compiler (TLS is built on BoringSSL through `btls`).

```bash
cargo build --release
./target/release/cray --config client.toml
./target/release/cray --config server.toml
```

---

## Status

Working:
- `tcp`, `unix`, `tls`, `http2`, and `http_connect` layers;
- Building client and server layer chains from config and validating layer order;
- The test protocol `tsf`.

> ⚠️ `tsf` is a **test protocol with no authentication**. Do not use it on real networks.

**Plans:**
1. A proper protocol with authentication and replay protection (nonce cache).
2. `auth/`: verifiers and generators.
3. Two-level verification: level 1 on the nginx side (HMAC cookie, timestamp; if no cookie is present, serve a genuine static website), level 2 in the protocol (on rejection, show a mirror of the real error page rather than a TCP RST).
4. A declarative protocol description language (DSL) for people who do not want to write Rust code. Rust authors retain full flexibility; the DSL is limited to a closed set of verifiers.
5. Custom transports and protocols as extensions, HTTP/1.1 and WebSocket support, and UDP.
6. Connection multiplexing (last priority).
