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
- **⚠️ Requirement:** Both client and server **must** specify a header with value `"id"` in their profile. If no header mapped to `"id"` is present, the handshake fails with: `XH2 requires an id header`.

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
| `host` | string | Required. Expected host / `:authority`. On server, requests targeting other hosts receive 404. |
| `path` | string | Required. Request `:path`. Requests with other paths receive 404. |
| `method` | string | Required. HTTP method for uplink (`POST`, `PUT`, etc.). Downlink leg always uses `GET`. |
| `config` | string | Required. Path to the HTTP/2 profile (needed by both sides to identify the Session ID header). |

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
