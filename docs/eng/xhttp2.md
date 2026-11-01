# The `xhttp2` Layer (Split HTTP/2)

The `xhttp2` layer implements a split-stream HTTP/2 transport over a single connection:
1. **Upload (client ──▶ server):** Request with the configured method (typically `POST`). The client writes outgoing bytes into the request body, and the server reads them (`RecvStream`).
2. **Download (server ──▶ client):** Request with `GET`. The server sends a response and streams bytes into the response body (`SendStream`), while the client reads them (`ResponseFuture`).

Both streams are correlated on the server using a unique, random 32-character hex Session ID (`sid`) passed inside a custom HTTP header.

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

# Server (inbound)
[[inbound.layers]]
type = "xhttp2"
host = "example.com"
path = "/split-tun"
method = "POST"
config = "conf/http2.toml"
```

### Parameters

| Field | Type | Description |
|---|---|---|
| `host` | string | Required. Expected host / `:authority`. On server, requests targeting other hosts are rejected with the profile's `reject_status` (404 by default). |
| `path` | string | Required. Request `:path`. Requests with other paths receive `reject_status`. |
| `method` | string | Required. HTTP method for uplink (`POST`, `PUT`, etc.). Downlink leg always uses `GET`. |
| `config` | string | Required. Path to the HTTP/2 profile (needed by both sides to identify the Session ID header). |

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

## Architecture and Streaming

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
