# The `xhttp2` Layer (Split HTTP/2)

The `xhttp2` layer implements a split-stream HTTP/2 transport over a single connection:
1. **Upload (client ──▶ server):** Request with the configured method (typically `POST`). The client writes outgoing bytes into the request body, and the server reads them (`RecvStream`).
2. **Download (server ──▶ client):** Request with `GET`. The server sends a response and streams bytes into the response body (`SendStream`), while the client reads them (`ResponseFuture`).

Both streams are correlated on the server using a unique, random 32-character hex Session ID (`sid`) passed inside a custom HTTP header.

The layer has two modes, selected by the required `mode` parameter:

- **`StreamUp`** — the upload is a single long-lived `POST` whose body is streamed. Lowest overhead, but it needs a path that does not buffer request bodies (see [Behind a CDN or reverse proxy](#behind-a-cdn-or-reverse-proxy)).
- **`PacketUp`** — every chunk of the upload is sent as its own short `POST` carrying a sequence number, and the server puts the packets back in order. Slower, but it works through intermediaries that buffer request bodies (for example a CDN).

---

## Custom Session ID Header (`id`)

The `xhttp2` layer **does not hardcode the header name** used for the Session ID. The header name is determined entirely by the HTTP/2 profile (`config`), where the special value `"id"` is assigned to the chosen header in `[http_headers]`:

```toml
# conf/http2.toml
[http_headers]
"x-session-id" = "id"          # Cray substitutes the generated Session ID here
"user-agent"   = "Mozilla/5.0 (Windows NT 10.0; Win64; x64)"
"accept"       = "*/*"
```

### How it works:
- **The value `"id"`** acts as a placeholder for Cray.
- **On client (`outbound`):** Before dispatching the `POST` and `GET` requests, the layer generates a random 32-hex character ID (`sid`) and replaces the value of this header with it.
- **On server (`inbound`):** The server inspects the profile for the header mapped to `"id"` and retrieves the Session ID from incoming client requests to pair the `POST` and `GET` streams.
- **Header name can be arbitrary**, for example:
  - `"x-session-id" = "id"`
  - `"x-correlation-id" = "id"`
  - `"sec-ch-ua-session" = "id"`
- **⚠️ Requirement:** Both client and server **must** specify a header with value `"id"` for the uplink leg (the layer's method) and for the downlink leg (`GET`). The header is looked up in the corresponding profile
  section (`[uplink]`/`[downlink]`) and falls back to the common top-level
  `headers_order`/`[http_headers]` set. If a leg has no header mapped to `"id"`, layer configuration fails with: `XHTTP2 needs a header with the value id (.../uplink|downlink)`.

---

## Sequence Number Header (`seq`, PacketUp only)

In `PacketUp` mode the profile must also map a header to the special value `"seq"` in the uplink leg. The client replaces it with the packet's sequence number (a decimal `u64`, starting at `0` and growing by one per `POST`); the server uses it to restore the order:

```toml
[uplink.http_headers]
"x-session-id" = "id"
"x-seq"        = "seq"
```

- The header name is arbitrary, just like for `id`.
- Only the uplink leg carries it; the `GET` has no sequence number.
- Server: if the uplink leg has no `"seq"` header, the connection fails with `XH2 requires an seq header` (checked when a connection is accepted, not at startup). Client: nothing is checked, but without the header the server rejects every `POST`, so keep both profiles in sync.
- A request with a missing or non-numeric `seq` is rejected like any other malformed request (see [Rejections](#rejections)).

---

## Layer Configuration

The `xhttp2` layer is configured symmetrically for client and server:

```toml
# Client (outbound)
[[outbound.layers]]
type = "xhttp2"
host = "example.com"
path = "/split-tun"
method = "POST"
config = "conf/http2.toml"
mode = "StreamUp"   # or "PacketUp"

# Server (inbound)
[[inbound.layers]]
type = "xhttp2"
host = "example.com"
path = "/split-tun"
method = "POST"
config = "conf/http2.toml"
mode = "StreamUp"   # or "PacketUp"
```

### Parameters

| Field | Type | Description |
|---|---|---|
| `host` | string | Required. Expected host / `:authority`. On server, requests targeting other hosts are rejected with the profile's `reject_status` (404 by default). |
| `path` | string | Required. Request `:path`. Requests with other paths receive `reject_status`. |
| `method` | string | Required. HTTP method for uplink (`POST`, `PUT`, etc.). Downlink leg always uses `GET`. |
| `config` | string | Required. Path to the HTTP/2 profile (needed by both sides to identify the Session ID header). |
| `mode` | string | Required. `StreamUp` or `PacketUp` (case-sensitive). Must be the same on client and server. A missing or unknown value fails layer configuration (`need mode` / `unknown mode <value>`). |

---

## Profile: uplink and downlink, responses and rejections

Request headers can be defined separately for the uplink leg (`[uplink]` — any
method except `GET`) and for the downlink leg (`[downlink]` — always `GET`). The
old flat format still works: top-level `headers_order` and `[http_headers]`
serve as a common fallback for both legs. Inside each leg the same
`headers_order`/`http_headers` apply, and partially defined legs take the
missing part from the top level (`[uplink]` also has the `[post]`/`[put]`
aliases, `[downlink]` — `[get]`):

```toml
[uplink]
headers_order = ["x-session-id", "user-agent", "content-type"]

[uplink.http_headers]
"x-session-id" = "id"
"user-agent" = "Mozilla/5.0"
"content-type" = "application/grpc"

[downlink]
headers_order = ["x-session-id", "user-agent"]

[downlink.http_headers]
"x-session-id" = "id"
"user-agent" = "Mozilla/5.0"
```

### Server responses

- `[uplink.response]` / `[downlink.response]` — headers of the successful (200)
  responses for the corresponding request. Same format: `headers_order` +
  `http_headers`.
- If a leg has no `response`, the top-level `[response]` table is used as the
  common fallback; if that is absent too, the response contains only
  `content-type: application/octet-stream`.
- In response headers (like in request headers) a value of `"id"` is replaced
  with the Session ID.
- If `content-type` is not set in the response headers, the layer adds
  `application/octet-stream`.

### Rejections

Requests that do not match the path, host or method, or that carry a missing or
malformed Session ID, are rejected:

- `reject_status` — the rejection code, any value 100–999 (`404` by default); 
  an invalid code fails profile loading.
- `[reject]` — reject headers in the same format; without the table the
  rejection stays bare (status only).
- `body_path` inside `[reject]` — path to a file (usually HTML) served entirely
  as the rejection body. The file is read once when the profile is loaded; a
  missing file is a configuration error. With a body set and no explicit
  `content-type`, `text/html` is used.

```toml
reject_status = 505

[response]
headers_order = ["server"]

[response.http_headers]
server = "nginx"

[uplink.response]
headers_order = ["content-type", "data"]

[uplink.response.http_headers]
content-type = "application/grpc"
data = "id"               # replaced with the Session ID

[reject]
headers_order = ["server", "content-type"]
body_path = "conf/reject.html"

[reject.http_headers]
server = "nginx"
content-type = "text/html; charset=utf-8"
```

`reject_status`, `[reject]` and `[response]` also work in the plain `http2`
layer (a single common set, no uplink/downlink split; see
[http2.md](http2.md)).

---

## Architecture: StreamUp

```
Client (Outbound)                                        Server (Inbound)
─────────────────                                        ────────────────
[ChunkStream]                                            [ChunkStream]
    │                                                        │
    ├─▶ TX: POST /split-tun (request body)  ───▶ RX (read)  ─┤
    │       Header: x-session-id: <sid>                      │
    │                                                        │
    └─◀ RX: GET  /split-tun (response body) ◀─── TX (write) ─┘
            Header: x-session-id: <sid>
```

- Request arrival order on the server is independent: if `POST` arrives first, its readable stream is buffered until `GET` arrives. If `GET` arrives first, its responder awaits the incoming `POST`.
- Data streams chunk by chunk (`ChunkStream`) without full in-memory buffering.
- HTTP/2 flow control windows are released dynamically as bytes are consumed.

---

## Architecture: PacketUp

```
Client (Outbound)                                           Server (Inbound)
─────────────────                                           ────────────────
[ChunkStream]                                               [ChunkStream]
    │                                                           │
    ├─▶ TX: POST seq=0 ┐                                        │
    │       POST seq=1 ├─ in parallel, any arrival order ─▶ Reorder (per sid) ─▶ RX
    │       POST seq=2 ┘                                        │
    │                                                           │
    └─◀ RX: GET (one long response body) ◀───────────── TX (write) ─┘
```

- The client sends the `GET` once. The server answers `200` immediately (headers from `[downlink.response]`) and streams the whole download in that response body; it never waits for a `POST`.
- Every write is cut into pieces of at most 16 KiB. Each piece is its own `POST` with its own `seq` and a closed request body (`END_STREAM`). Concurrency is bounded by HTTP/2 `max_concurrent_streams`: before the next `POST` the client waits for a free stream slot.
- The server keeps one session per `sid` in a process-wide table, so the `POST`s and the `GET` of one session may arrive on different HTTP/2 connections (this is what happens behind a reverse proxy that opens a connection per request).
- Packets go through a per-session reorder buffer: a packet with the expected `seq` is delivered at once together with every consecutive packet already buffered; a packet from the future is buffered until the gap is filled.
- The server answers a `POST` with an empty `200` only after its packet has been accepted by the reorder buffer.
- End of stream: the client sends a `POST` with an empty body and the next `seq` (FIN). After all earlier data is delivered, the server closes the session and the read side sees EOF.
- Errors: a missing `seq`, or a body larger than 16 KiB, is rejected with `reject_status`. A duplicate or already delivered `seq` is rejected too, and in addition the session is closed.

---

## Behind a CDN or reverse proxy

An intermediary can break each leg in its own way. Notes from running `xhttp2` behind Cloudflare → nginx → Cray:

- **Buffered request bodies (`StreamUp`).** A CDN may hold a `POST` body until it ends, so the server never sees the stream. `content-type = "application/grpc"` on the uplink `POST` (with gRPC enabled on the CDN) makes Cloudflare stream it. Put it **only** into `[uplink.http_headers]`: on the `GET` it makes the request look like broken gRPC and the `GET` never reaches the origin. If the CDN buffers anyway, use `PacketUp`.
- **Caching of the `GET`.** A CDN caches by file extension (Cloudflare caches `.css` and similar by default) and the cache key is just the URL, so your `x-session-id` does not matter: a cached copy is served and the tunnel `GET` never reaches the server. The symptom is that `POST`s arrive and the `GET` does not. To prevent it:
  - answer the `GET` with `cache-control: no-store` (`[downlink.response.http_headers]`);
  - never serve a cacheable decoy (`max-age`, `immutable`) from the same URL: any visitor without a session would put it into the CDN cache;
  - add a CDN rule that bypasses the cache for the tunnel path (Cloudflare: a Cache Rule with *Bypass cache*, or a Cache Response Rule that adds `no-store`) and purge the URL afterwards;
  - or use a path without a cacheable extension.

  Check: `curl -s -o /dev/null -D - --http2 -H "x-session-id: $(openssl rand -hex 16)" https://host/path | grep -iE 'cf-cache-status|age'` — the status must not be `HIT`.
- **Buffered download.** Recommended headers for the downlink response: `content-type: text/event-stream` and `x-accel-buffering: no` (see `conf/xhttp2.toml`). In nginx also set `proxy_buffering off;` and `proxy_request_buffering off;`.
- **Masquerade in front of the layer.** A proxy may serve a decoy page to requests without a session; just make sure the decoy is not cacheable (see above) and that the session header name used by the proxy matches the one in the profile for **both** legs.
