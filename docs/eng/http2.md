# HTTP/2 Fingerprint Customization 

HTTP/2 fingerprinting (the Akamai-style hash over SETTINGS, window sizes,
priorities and header order) is fully configurable through an HTTP/2 profile file
that the `http2` layer points to with `config`. The client-side `http2` layer shapes everything the
network can see: the connection preface, the SETTINGS frame, stream
priorities, pseudo-header order and header order
(`src/fingerprint/http2/`).

Unlike a forwarding proxy, Cray **originates** the HTTP/2 request itself.
There are no incoming browser headers to pass through: the request is built
from the layer parameters (`method`, `host`, `path`) and the header
configuration in the profile.

## How the layer works

Client (`outbound.layers`):

```
tls stream ──▶ http2 layer ──▶ virtual stream ──▶ protocol
                 1. HTTP/2 handshake with the profiled builder (built once per profile
                    file and cached, 5 second timeout by default, `timeout` field)
                 2. one request: method + :authority + :path from the layer,
                    headers from the profile, END_STREAM is NOT set on HEADERS
                 3. the request body is the upload direction,
                    the response body is the download direction
```

Server (`inbound.layers`):

```
tls stream ──▶ http2 layer ──▶ one virtual stream per accepted request ──▶ protocol
                 1. HTTP/2 server handshake (5 second timeout by default, `timeout` field)
                 2. every request whose method, path and host match is answered 200
                    (content-type: application/octet-stream),
                    anything else is answered 404
                 3. each accepted request becomes an independent byte stream
```

The layer takes the connection and hands a **virtual stream** outward. For the
protocol on top it is a plain byte stream, the same as a TCP socket: bytes
written go out as DATA frames in the request body (client) or the response body
(server), and bytes read come from the opposite direction. Flow control
(`WINDOW_UPDATE`) is handled inside the layer. Closing the write side sends an
empty DATA frame with `END_STREAM`.

The profile shapes the client side only. The server ignores it and uses fixed
settings (128 concurrent streams, 1 MiB stream window, 4 MiB connection
window).

## Layer parameters

```toml
[[outbound.layers]]
type = "http2"
path = "/tun"
host = "example.com"
method = "POST"
config = "./http2.toml"
```

| Field | Client | Server |
|---|---|---|
| `path` | required: request `:path` | required: requests must match, others get 404 |
| `method` | required: `POST` or `PUT` | required: requests must match, others get 404 |
| `host` | required: request `:authority` | required: requests must match, others get 404 |
| `config` | required: path to the HTTP/2 profile | not used |
| `timeout` | seconds (`timeout = 5`) or preset name (`timeout = "h2"`); default 5 | same; default 5 |
| scheme | always `https` | n/a |

The profile is a separate file with only top-level keys (there is no `[http2]`
header). Relative paths are resolved against the process working directory. The
profile is loaded on the first connection and cached by path, so errors in it
show up then and edits need a restart. Keys that are not part of the profile are
ignored silently.

`timeout` covers only the layer handshake, not the connection lifetime. A preset
name resolves through the `[timeouts]` table at the top of the config (details
and defaults in the "Timeouts" section of [tls.md](tls.md)):

```toml
[timeouts]
h2 = 4

[[outbound.layers]]
type = "http2"
path = "/tun"
host = "example.com"
method = "POST"
config = "./http2.toml"
timeout = "h2"
```

## Example profile (`http2.toml`)

```toml
# http2.toml
connection_window_update = 15663105
settings_order = [
    "HEADER_TABLE_SIZE",
    "ENABLE_PUSH",
    "INITIAL_WINDOW_SIZE",
    "MAX_FRAME_SIZE",
]
pseudo_headers_order = [":method", ":authority", ":scheme", ":path"]
headers_order = ["user-agent", "accept", "accept-language", "priority"]
# initial_stream_id = 3             omit to derive it

[settings]
header_table_size = 65536
enable_push = false
initial_window_size = 6291456
max_frame_size = 16384

[headers_priority]
exclusive = true
depends_on = 0
weight = 255

[http_headers]
user-agent = "Mozilla/5.0 (...)"
accept = "*/*"
accept-language = "en-US,en;q=0.6"
priority = "u=0, i"
```

TOML has no `null`. To leave an optional field unset, omit the key. Write
scalar keys before the first table header.

### `settings`: SETTINGS frame values

Values advertised in the SETTINGS frame. Omitted optional keys are not sent.
`enable_push` is a plain boolean and must be present whenever `[settings]`
exists:

| Field | Meaning |
|---|---|
| `header_table_size` | HPACK dynamic table size. Omit to leave it out. |
| `enable_push` | Server push on/off. Always sent. |
| `max_concurrent_streams` | Max concurrently active streams. Omit to leave it out. |
| `initial_window_size` | Per-stream flow-control window. |
| `max_frame_size` | Max frame payload size. |
| `max_header_list_size` | Max header list size. Omit to leave it out. |

If the whole `[settings]` table is missing, no values are customized and
the library defaults apply.

### `settings_order`: SETTINGS parameter order

Order of parameters in the outgoing SETTINGS frame. Only parameters present in
`settings` are emitted, in exactly this order:

```
HEADER_TABLE_SIZE
ENABLE_PUSH
MAX_CONCURRENT_STREAMS
INITIAL_WINDOW_SIZE
MAX_FRAME_SIZE
MAX_HEADER_LIST_SIZE
```

Names are case-insensitive. An unknown name fails the connection when the
builder is created, so a typo shows up on the first `h2` connection instead of
silently producing a wrong fingerprint.

### `connection_window_update`: connection window

The initial connection-level flow-control window, independent from the
per-stream `initial_window_size`. If omitted, the library default is used. Real
browsers use distinctive large values here (for example `15663105` for
Firefox), so this field matters more than it looks.

### `initial_stream_id`: first request stream

Explicit first client-initiated stream ID. When omitted, it is derived:

- no priority frames: `1`;
- with priority frames: the highest configured priority `stream_id` plus 2
  (priority frames occupy stream IDs before the first real request).

When you set it manually alongside `priority_frames`, pick an unused odd ID
past them. A collision breaks the connection.

### `priority_frames`: PRIORITY frames before the request

PRIORITY frames sent before the request stream (the classic Firefox priority
tree signal), written as an array of tables:

```toml
[[priority_frames]]
stream_id = 3
exclusive = false
depends_on = 0
weight = 201

[[priority_frames]]
stream_id = 5
exclusive = false
depends_on = 0
weight = 101
```

| Field | Meaning |
|---|---|
| `stream_id` | Stream the priority attaches to (client streams are odd). |
| `exclusive` | Whether the dependency is exclusive. |
| `depends_on` | Parent stream ID (`0` = connection root). |
| `weight` | Raw value, 0 to 255. |

Omit the key or leave it empty for no priority frames. With the example above
and no `initial_stream_id`, the request stream becomes `7`.

### `headers_priority`: HEADERS stream dependency

Priority attached to the request HEADERS frame:

```toml
[headers_priority]
exclusive = true
depends_on = 0
weight = 255
```

Same fields as a priority frame, minus `stream_id`. Omit the table for no
custom dependency.

### `pseudo_headers_order`: pseudo-header order

Order of `:method`, `:path`, `:authority`, `:scheme` in the request. An unknown
name fails the connection. Browsers differ characteristically here; Firefox
sends `:method, :authority, :scheme, :path`:

```toml
pseudo_headers_order = [":method", ":authority", ":scheme", ":path"]
```

Set it explicitly. The values themselves come from the layer (`method`,
`host`, `path`) and the fixed `https` scheme.

### `headers_order` and `http_headers`: request headers

The request header list is built from these two fields only:

1. Walk `headers_order` in order. For each name, take its value from
   `http_headers`. A name that is listed in the order but has no value in
   `http_headers` is **omitted**, because there is no incoming request to copy
   it from.
2. Append the headers that are present in `http_headers` but not in
   `headers_order`. Their relative order is **not defined**.

```toml
headers_order = ["user-agent", "accept", "accept-language", "priority"]

[http_headers]
user-agent = "Mozilla/5.0"
accept = "*/*"
accept-language = "en-US,en;q=0.6"
priority = "u=0, i"
```

Rules that follow from this:

- **List every header in `headers_order`.** Anything left out is appended in an
  undefined order.
- **Use lowercase keys in `http_headers`.** Names are looked up in lowercase,
  so a key such as `User-Agent` is never matched and the header is dropped.
- Values must be valid HTTP header values; an invalid name or value fails the
  connection.
- The layer adds no headers of its own, not even `content-type` or
  `content-length`. Add what you want to appear in `http_headers`.

## Streaming

Bodies stream in both directions and are never buffered as a whole. One tunnel
is one HTTP/2 stream, and one HTTP/2 connection carries one tunnel from the
client for now (multiplexing several tunnels over one connection is planned).

The client does not wait for the response headers before the first write.
They are read lazily on the first read from the stream, so a protocol that
sends first and listens later cannot deadlock against a server that replies
only after reading.

## ALPS coupling

When `alps = true` in the TLS profile, the same `settings` and `settings_order` are
re-encoded as the TLS ALPS payload for `h2` (see [tls.md](tls.md)). Browsers
use that exact encoding, so one profile drives both places on the wire. The TLS layer finds it through
`http2_path`; point that at the same file as `config` on the `http2` layer. Keep
them in sync with the browser you imitate: a mismatch between the SETTINGS
frame and the ALPS payload is itself a fingerprint signal.

## Gotchas

- `settings_order` and `pseudo_headers_order` are validated: unknown names fail
  the connection instead of degrading silently. Check the spelling against the
  vocabularies above.
- `enable_push` must be present whenever the `[settings]` table exists.
- `settings` values outside the protocol range (for example an absurd
  `max_frame_size`) are rejected by the HTTP/2 stack at handshake time.
- `initial_stream_id` colliding with `priority_frames` IDs kills the
  connection. Prefer to omit it and let it derive.
- `http_headers` keys must be lowercase, and every header must be listed in
  `headers_order` to get a defined position.
- The server accepts only requests whose method, path and host match the layer;
  everything else gets 404. A client and a server with different `path`,
  `method` or `host` values never connect.
- The server compares `host` with the host part of `:authority`, without the
  port. Write `host = "example.com"`, not `"example.com:443"`. Behind a reverse
  proxy the value is whatever the proxy forwards as `:authority`.
- The client `http2` layer needs `config`. The TLS layer needs `http2_path`
  only when `alps = true`; point both at the same file.
- Unknown keys are ignored without any warning (this includes the former
  `end_stream_on_headers` key). If a setting seems to have no effect, check the
  spelling of the key first.
- To verify the result, capture the client's traffic (Wireshark) or log the
  JA3/JA4 and HTTP/2 fingerprint on the server side (for example in nginx),
  and compare it with a capture of the real browser you imitate.