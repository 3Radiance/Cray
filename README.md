<p align="left">
  <img src="./cray.jpg" width="20%" alt="crays-logo" style="border-radius: 16px; box-shadow: 0 4px 20px rgba(0,0,0,0.15);">
</p>

# Cray

English | [Русский](README.ru.md)

[![CI](https://github.com/3Radiance/Cray/actions/workflows/ci.yml/badge.svg)](https://github.com/3Radiance/Cray/actions/workflows/ci.yml)

**A composable proxy engine written in Rust.**

Cray builds a connection out of independent layers declared in a TOML file: a connection source, any number of transports, and a protocol at the end. Transport and protocol know nothing about each other, so a new protocol works over every transport, and a new transport carries every protocol.

```text
tcp → protocol
tcp → tls → protocol
tcp → tls → http2 → protocol
tcp → tls → xhttp2 → auth → protocol
```

> 🚧 Under active development. The layered core works; see [Status](#status) for what is still missing.

---

## Overview

### What it is

A proxy engine rather than a finished VPN: a small core that runs a chain of layers, and a set of layers built on top of it. The same binary acts as a client or as a server depending only on the config.

### Why

In most proxy cores the protocol is tied to a transport: a new transport means re-implementing the protocol, and what the traffic looks like on the wire is decided by the author. Cray separates the two and moves every decision that shapes the traffic into configuration.

### Principles

- **Composable.** Layers combine freely. A protocol reads and writes a stream and does not care what is underneath: raw TCP, TLS or HTTP/2.
- **Explicit.** Header names, fingerprints, response and rejection pages are values in the config and in profile files, not constants in the code.
- **Control.** Cray provides mechanisms, not policies: which layers to chain, how traffic is shaped and what it looks like on the wire is decided in your config.

---

## Concepts

### Chains and roles

The configuration is a TOML file with two layer chains:

- **`inbound.layers`**: how to accept a connection;
- **`outbound.layers`**: how to connect onward.

Role follows position, there is no client/server switch. Inbound layers listen and accept, outbound layers dial. A protocol layer at the end of `outbound.layers` means this side initiates; at the end of `inbound.layers` it means this side accepts.

**Chain rules:**
- the first layer is the connection source (`tcp` or `unix`);
- the remaining layers are transports, each turning one stream into another;
- **the protocol is always the last layer** and terminates the connection.

### Layers

| Layer | outbound | inbound |
|---|---|---|
| `tcp` | Dials `addr`; without `addr`, connects directly to the destination (like “freedom/direct”). | Listens on `addr`. |
| `unix` | Connects to `addr`. | Listens on `addr`. |
| `http-proxy` | Proxy chaining: sends CONNECT to a fixed `addr` through the previous stream. | Accepts HTTP CONNECT from the browser and extracts the destination address. |
| `tls` | Outbound handshake: `sni` and a configurable fingerprint. | Terminates TLS: picks the certificate by SNI (`certs`/`default`) and performs the handshake. |
| `http2` | Opens one POST/PUT request and exposes a **virtual stream**. | Accepts HTTP/2 requests and passes each matching one onward as a separate virtual stream; everything else gets a configurable rejection (status, headers, HTML page). |
| `xhttp2` | Splits a tunnel across upload (`POST`/`PUT`) and download (`GET`) requests and exposes one **virtual stream**. `mode`: `StreamUp` (one streamed `POST`) or `PacketUp` (one `POST` per chunk, ordered by a sequence number). | Pairs the requests of a session by the configured session-ID header regardless of arrival order; in `PacketUp` also restores the packet order. |
| `jitter` | Adds a random delay before every write; see [docs/eng/shaping.md](docs/eng/shaping.md). | Same, on writes of this side. |
| `fragment` | Cuts every write into smaller pieces; see [docs/eng/shaping.md](docs/eng/shaping.md). | Same, on writes of this side. |
| `pad` | Size equalization, a pure shaper (`stages`, no keys; see [docs/eng/shaping.md](docs/eng/shaping.md)). | Same. |
| `auth` | Frame authentication: an HMAC-SHA256 tag over every record, one shared key, no handshake; see [docs/eng/auth.md](docs/eng/auth.md). | Same (identical key on both ends). |
| `vless` | Protocol: wraps the stream into VLESS framing — a UUID-authenticated header (TCP command), the target address, then the payload. | Protocol: unwraps VLESS framing, verifies the UUID, dials the target and pipes the two ends together. |
| `pipe` | — | Local protocol for testing and a local transparent proxy without a server: no config; reads the target address from the context (`ConnectRequest`, provided e.g. by `http-proxy`), dials it through the outbound chain (`Dialer`) and pipes the two ends together until EOF. Requires both of those keys from `Cx`, nothing else. |

Placement matters for `pad`: `tcp → tls → pad` hides padding inside the encryption (recommended). `tcp → pad → tls` also works, but exposes the framing to DPI. `pad` does not authenticate; put the `auth` layer under it (`tcp → tls → auth → pad → protocol`; see [docs/eng/auth.md](docs/eng/auth.md)).

`http2`, `xhttp2` and any other multiplexing transport take a socket and expose a virtual stream, so the protocol on top reads and writes a plain byte stream.

---

## Examples

### Client

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
2. `http-proxy` extracts the destination address from the request and passes it to the protocol so it can label packets. This completes the inbound chain. Instead of HTTP CONNECT you could write your own entry layer, such as SOCKS5; its only job would be to extract the destination address.
3. In outbound, `tcp` creates a socket and connects it to the server, `tls` encrypts it, `http2` wraps it in a virtual stream, and the protocol operates on that stream.

### Server

```toml
[[inbound.layers]]
type = "tcp"
addr = "127.0.0.1:10811"

[[inbound.layers]]
type = "tls"
certs = [
    { sni = "domain.com", cert = "/path/to/fullchain.pem", key = "/path/to/privkey.pem" },
]
default = "domain.com"
alpn = ["h2"]

[[inbound.layers]]
type = "http2"
path = "/tun"
host = "idk.com"
method = "POST"

[[inbound.layers]]
type = "vless"
uuid = "11111111-1111-4111-8111-111111111111"   # the same UUID as in the outbound config

[[outbound.layers]]
type = "tcp"        # no address: connect to the destination requested by the protocol
```

The same layers act as acceptors here:

- `tls` accepts the socket, picks the certificate by the ClientHello SNI (`certs`/`default`), performs the handshake and passes it onward. `alpn` is set directly on the layer;
- `http2` is an HTTP/2 server: it accepts only requests whose `method`, `path` and `host` match and rejects the rest (404 by default, configurable in the profile);
- the **inbound protocol** is then given the stream;
- `tcp` in outbound, when no address is specified, connects to the destination provided by the protocol.

### Splitting a tunnel with `xhttp2`

`xhttp2` replaces `http2` when a single streamed request is a problem, for example behind a CDN that buffers request bodies. The client below sends the download as one long `GET` and the upload as many short `POST`s; the server sits behind a reverse proxy that terminates TLS.

```toml
# client
[[outbound.layers]]
type = "tcp"
addr = "cdn.example.com:443"

[[outbound.layers]]
type = "tls"
sni = "cdn.example.com"
tls_path = "./tls.toml"

[[outbound.layers]]
type = "xhttp2"
host = "cdn.example.com"
path = "/api/sync"
method = "POST"
config = "./xhttp2.toml"   # defines the session-ID and sequence headers
mode = "PacketUp"          # or "StreamUp"

[[outbound.layers]]
type = "vless"
uuid = "11111111-1111-4111-8111-111111111111"
```

```toml
# server (behind a reverse proxy)
[[inbound.layers]]
type = "tcp"
addr = "127.0.0.1:10811"

[[inbound.layers]]
type = "xhttp2"
host = "cdn.example.com"
path = "/api/sync"
method = "POST"
config = "./xhttp2.toml"
mode = "PacketUp"          # must match the client

[[inbound.layers]]
type = "vless"
uuid = "11111111-1111-4111-8111-111111111111"

[[outbound.layers]]
type = "tcp"
```

The modes, the `id` and `seq` header markers and the notes on running behind a CDN are in [docs/eng/xhttp2.md](docs/eng/xhttp2.md). A sample profile is in [`conf/xhttp2.toml`](conf/xhttp2.toml).

---

## Profiles

TLS and HTTP/2 can be configured in full. Cray lets you set values manually; presets are not available yet.

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

Fingerprints live in separate profile files that the layers point to: `tls_path` (TLS) and `config` (HTTP/2 and `xhttp2`). An HTTP/2 profile describes SETTINGS, pseudo-header and header ordering, priorities, and, on the server, the responses and rejection pages. Outbound layers load and validate their profiles when the chain is built, so a broken profile stops the program at startup.

Field-by-field references: [docs/eng/tls.md](docs/eng/tls.md), [docs/eng/http2.md](docs/eng/http2.md) and [docs/eng/xhttp2.md](docs/eng/xhttp2.md).

On the server the `tls` layer takes `alpn` directly; `tls_path` is ignored inbound (a warning is logged).

---

## Project Structure

| Path | Purpose |
|---|---|
| `core/` | The engine: layer contract (`Layer`), `Registry`, chain building and validation, the `drive` runner. Knows no layer names. |
| `layers/` | All layers, grouped by kind: `transport/` (`tls`, `http2`, `xhttp2`), `proxy/` (`http-proxy`), `protocol/` (`vless`, `pipe`), `wrappers/` (`jitter`, `fragment`, `pad`, `auth`), plus `tcp`, `unix`, `fingerprint/` builders and `profiles/` loading. A new layer is one file plus one `register` line. |
| `cray/` | The binary: parses args, registers layers, builds the engine, runs. |
| `conf/` | Example configs and profiles. |
| `docs/` | Layer and profile references in English (`eng/`) and Russian (`ru/`). |

---

## Build and Run

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

Tags: `CHAIN`, `INB`, `HTTP`, `TLS`, `CERT`, `H2`, `XH2`, `VLESS`, `JITTER`, `FRAG`, `PAD`. Per-connection details are logged at `debug` and `trace`, so `info` stays quiet.

---

## Status

### Working

- Layers `tcp`, `unix`, `http-proxy`, `tls`, `http2`, `xhttp2`, `pipe` and `vless`, and the `jitter`, `fragment`, `pad` and `auth` wrappers.
- Building inbound and outbound chains from the config and validating the layer order.
- `http2`: a profiled HTTP/2 client and a server with configurable responses and rejections; see [docs/eng/http2.md](docs/eng/http2.md).
- `xhttp2`: upload and download split across separate HTTP/2 requests, correlated by a session-ID header of your choice. `StreamUp` streams the upload in one `POST`; `PacketUp` sends it as ordered `POST` packets restored by a per-session reorder buffer, and has been run through Cloudflare and nginx. Idle and never-paired sessions are expired by a background GC. See [docs/eng/xhttp2.md](docs/eng/xhttp2.md).
- `auth`: per-frame HMAC-SHA256 authentication; see [docs/eng/auth.md](docs/eng/auth.md).
- `vless`: TCP command with UUID authentication.

### Known limitations

> ⚠️ `vless` follows the VLESS header layout only partly and is **not interoperable** with Xray, sing-box or other VLESS clients yet. Differences: domain and IPv6 address types use the SOCKS5-style codes 3 and 4 (VLESS uses 2 and 3), the server sends no response header, request addons are assumed to be empty, and only the TCP command is supported. A Cray client works with a Cray server. The UUID is the only credential, and there is no replay protection yet.

- `auth` v1 carries only the tag: no replay protection yet.
- `xhttp2` `PacketUp`: the server does not yet limit the reorder buffer or the number of sessions. Idle and never-paired sessions are expired by the server GC. Every write goes out as its own request immediately — write-through by design, no write buffering.
- TLS and HTTP/2 presets are not available, fingerprints are set by hand.

### Roadmap

1. `xhttp2` hardening: limits for the reorder buffer and the session table.
2. Full VLESS wire compatibility (see the warning above), UDP and Mux commands, and replay protection (nonce cache).
3. `auth`: replay protection (nonce cache and a masked timestamp, still no handshake) and an AEAD record mode (`[nonce:16 | ts':8 | len:2 | AEAD(payload)]`).
4. Two-level verification: level 1 on the nginx side (HMAC cookie, timestamp; if no cookie is present, serve a genuine static website), level 2 in the protocol (on rejection, show a mirror of the real error page rather than a TCP RST).
5. A declarative protocol description language (DSL) for people who do not want to write Rust code. Rust authors retain full flexibility; the DSL is limited to a closed set of verifiers.
6. Custom transports and protocols as extensions, HTTP/1.1 and WebSocket support, and UDP.
7. Connection multiplexing (last priority).
8. Independent stacks per leg of a split transport. Each leg (`up`/`down`) would be a full sub-chain with its own transports and shaping, so the two directions can look different on the wire:

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

---

## Contributors

- **Maintainer:** [Three_Radiance](https://github.com/3Radiance) — author and maintainer

See the full list in [CONTRIBUTORS.md](CONTRIBUTORS.md) and the contribution guide in [CONTRIBUTING.md](CONTRIBUTING.md). All contributions are licensed under [GPL-3.0-or-later](LICENSE).

---

## License

[![License: GPL-3.0-or-later](https://img.shields.io/badge/License-GPL--3.0--or--later-blue.svg)](LICENSE)

Cray is licensed under the **GNU General Public License v3.0 or later**.

This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. It is distributed in the hope that it will be useful, but **without any warranty**; see the [full license text](LICENSE) for details.

