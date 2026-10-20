# Frame authentication: the `auth` layer

`auth` is a plain wrapper: it takes any stream and returns a stream, with no
sockets and no knowledge of the transport or the protocol above and below it.
It works in `inbound.layers` and in `outbound.layers`; behavior is defined only
by `Side`, there are no per-side settings.

Every frame on the wire is authenticated with a shared key. This is not a
handshake and not a key exchange: the key lives in the config on both ends:

```
[ tag:32 | len:2 | payload ]
```

- `tag` — HMAC-SHA256 over `len || payload`, all 32 bytes of the computation
  go on the wire (v1 framing);
- `len` — payload length, u16 big-endian, at most 16384 bytes per frame;
- one key, identical on both sides.

There is no handshake in the usual sense: the connection does not stop to
check "the first message", either on the reader side or as a separate
back-and-forth. Instead the reader walks the frames with limits in place:
34 bytes for the header and up to 16 KiB for the frame payload, and only bytes
that pass the tag check ever emerge above. Unverified bytes never leave the
reader.

## Where to put it

`auth` sits above encryption and below the shapers:

    tcp → tls → auth → pad → protocol

Crypto is paid once per frame and covers everything above it in the stack:
`pad` framing and the protocol are inside the authenticated frame. Over TLS the
frame format is invisible to DPI (everything there just looks like TLS data),
so `auth` adds no format here, only content verification.

Below `pad`: the `pad` layer cannot sign anything, so `auth` under it is
mandatory wherever the padding reader trusts lengths.

## Config

```toml
[[outbound.layers]]
type = "auth"
key = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
```

- `key` — exactly 64 hex characters, that is 32 bytes. Generate one with the
  built-in binary `cargo run --bin gen_key` (source:
  `cray/src/bin/gen_key.rs`), or with any other tool, e.g.
  `openssl rand -hex 32`.
- The key matches on both ends.
- A non-hex string, a length other than 32 bytes, or unknown config fields is
  a load-time error; the proxy refuses to start.

## Writing

- Every write from above is split into frames of at most 16 KiB. A frame is
  built in a single pass (header, `len`, payload) and then handed to the
  stream piece by piece. Large writes produce several consecutive frames.
- The tag is deterministic over the content: two identical writes produce
  identical frames. The layer adds no randomness on the wire.

## Reading

- The reader collects the 34-byte header, then exactly the declared payload
  length, and only after the tag check hands bytes upward. While the tag is
  unchecked, not a single byte of the frame leaves the reader.
- One frame — one HMAC check, every frame is verified on its own.
- An oversized length (declared `len > 16384`) is rejected before any payload
  is read at all.
- EOF inside a header or a payload is an error and tears the read down. EOF
  exactly on a frame boundary is a normal close, not an error.
- The wire length is the only trust before verification: the payload is taken
  exactly as declared, up to 16 KiB, and the tag is checked afterwards.

## What v1 lacks

- **Replay protection.** The tag is deterministic (`len || payload`), so an
  active attacker on the channel can replay a recorded session in full: frames
  carry neither a nonce nor a timestamp, and the reader has nothing to compare.
  The fix is planned as the next step: `nonce + ts`, a masked timestamp, and a
  nonce cache.
- **AEAD mode** (`cipher = "chacha20"`): the plan is a record of the form
  `[nonce:16 | ts':8 | len:2 | AEAD(payload)]`, where the AEAD tag is not
  written out separately; AEAD itself punishes forgery. btls already ships
  XChaCha20-Poly1305 and the needed primitives.
- **Logging.** The layer is silent for now.

## Crypto engine

HMAC-SHA256 and tag comparison run through btls (BoringSSL): the one-shot
`btls::hash::hmac_sha256` and constant-time `btls::memcmp::eq`, with no
RustCrypto dependency. In v1 the tag field is fixed (32 bytes), so the lengths
always match at the comparison. Keep in mind for later: `memcmp::eq` panics
when lengths differ, so if the format ever leaves the fixed tag, the length
must be validated before the comparison.
