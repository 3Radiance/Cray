# TLS Fingerprint Customization

The client-side `tls` layer builds its ClientHello entirely from a TLS profile
file that the layer points to: cipher order, extensions and their order, curves,
signature algorithms, ALPN, GREASE, certificate compression. Every byte of the
ClientHello that fingerprinting sees is under your control. The stack is
BoringSSL via `btls` / `tokio-btls` (`src/fingerprint/tls/`).

The server-side `tls` layer is a plain acceptor. It is **not** fingerprint-shaped:
it only needs a certificate, a key and an ALPN list.

## Where the settings live

The TLS fingerprint is a **separate profile file** that the `tls` layer points
to with `tls_path`. The profile contains only top-level keys (there is no
`[tls]` header). The layer itself carries the connection parameters:

```toml
# client.toml
[[outbound.layers]]
type = "tls"
sni = "example.com"
tls_path = "./tls.toml"        # the profile described in this document
http2_path = "./http2.toml"    # read only when alps = true
```

```toml
# tls.toml (the profile)
cipher_suites = [ ... ]
# ...
```

```toml
# server.toml
[[inbound.layers]]
type = "tls"
cert = "/path/to/fullchain.pem"
key = "/path/to/privkey.pem"
tls_path = "./tls.toml"        # on the server only `alpn` is read from it
```

Layer fields:

| Field | Client | Server |
|---|---|---|
| `sni` | required: the name sent in the ClientHello and checked against the certificate | not used |
| `tls_path` | required: the fingerprint profile | required: only `alpn` is read |
| `http2_path` | required, but the file is read only when `alps = true`; otherwise any placeholder string works | not used |
| `cert`, `key` | not used | required: PEM certificate chain and private key |

How the files are handled:

- Relative paths are resolved against the process working directory, not
  against the config file.
- A profile is loaded the first time a connection needs it and is cached by
  path. Errors in a profile show up on the first connection, not at startup,
  and edits need a restart.
- Keys that are not part of the profile are ignored silently, so a misspelled
  key simply has no effect.
- A missing required layer field is reported when a connection arrives.

TOML has no `null`. To leave an optional field unset, **omit the key**.

## Connection flow

Client (`outbound.layers`):

```
tcp socket ──▶ tls layer ──▶ next layer (http2, protocol, ...)
                 1. take the SslConnector for `tls_path` (built once per profile file, cached)
                 2. set SNI from the layer, attach ALPS payload if alps = true (reads http2_path)
                 3. handshake with the profile, 6 second timeout
                 4. verify the server certificate, return the encrypted stream
```

Server (`inbound.layers`):

```
tcp socket ──▶ tls layer ──▶ next layer (http2, protocol, ...)
                 1. SslAcceptor (mozilla_intermediate) with cert chain and key (built once per cert/key pair)
                 2. ALPN select callback, client preference order
                 3. handshake, 6 second timeout
```

The ALPN callback walks the client's list in order and picks the first protocol
that also appears in the server's `alpn`. If nothing matches, no ALPN is
negotiated.

The client connector is assembled in `tls_builder`
(`src/fingerprint/tls/builder.rs`) in this order: Chromium root store → cipher
suites → ALPN → curves → signature algorithms → record size limit →
certificate compression → GREASE → extension permutation → OCSP → SCT →
session ticket → delegated credentials → extension order. ALPS is attached per
connection.

### Server certificate verification

The client verifies the server certificate against the Chromium root store and
checks the host name against `sni`. Use a publicly trusted certificate (for
example Let's Encrypt) whose name matches `sni`. Self-signed or private CAs are
not supported yet.

## Example configuration

```toml
# tls.toml
cipher_suites = [
    "TLS_AES_128_GCM_SHA256",
    "TLS_AES_256_GCM_SHA384",
    "TLS_CHACHA20_POLY1305_SHA256",
    "ECDHE-ECDSA-AES128-GCM-SHA256",
    "ECDHE-RSA-AES128-GCM-SHA256",
    "ECDHE-ECDSA-AES256-GCM-SHA384",
    "ECDHE-RSA-AES256-GCM-SHA384",
]
alpn = ["h2", "http/1.1"]
curves = ["X25519", "P-256", "P-384"]
signature_algorithms = [
    "ecdsa_secp256r1_sha256",
    "rsa_pss_rsae_sha256",
    "rsa_pkcs1_sha256",
    "ecdsa_secp384r1_sha384",
    "rsa_pss_rsae_sha384",
    "rsa_pkcs1_sha384",
]
cert_compression = ["brotli"]
permute_extensions = false
grease_enabled = false
status_request = false
signed_certificate_timestamp = false
session_ticket = false
alps = false
# extensions_order = [ ... ]    omit unless you need a fixed order
# delegated_credentials = [ ... ]
# record_size_limit = 16385
```

`cipher_suites`, `curves` and `signature_algorithms` are required in practice:
if one is omitted, an empty list is passed to BoringSSL, which rejects it and
the connection fails.

### `cipher_suites`: strict order, two blocks

The list is joined with `:` and passed to `set_cipher_list`, with
`set_preserve_tls13_cipher_list(true)` so your order survives verbatim.
BoringSSL keeps TLS 1.3 and TLS 1.2 suites in two separate internal lists. They
can never be interleaved: BoringSSL always emits them as two contiguous blocks
(TLS 1.3 first, then TLS 1.2), however they are mixed in the config. Real
browsers behave the same way, so this is not a limitation to work around.

TLS 1.3 suites use IANA-style `TLS_<AEAD>_<HASH>` names. For TLS 1.2 use the
OpenSSL-style short names (`ECDHE-ECDSA-AES128-GCM-SHA256`) as in the examples.

TLS 1.3:
```
TLS_AES_128_GCM_SHA256
TLS_AES_256_GCM_SHA384
TLS_CHACHA20_POLY1305_SHA256
```

TLS 1.2:
```
ECDHE-ECDSA-AES128-GCM-SHA256
ECDHE-ECDSA-AES256-GCM-SHA384
ECDHE-RSA-AES128-GCM-SHA256
ECDHE-RSA-AES256-GCM-SHA384
ECDHE-ECDSA-CHACHA20-POLY1305
ECDHE-RSA-CHACHA20-POLY1305
ECDHE-ECDSA-AES128-SHA
ECDHE-RSA-AES128-SHA
ECDHE-RSA-AES128-SHA256
ECDHE-ECDSA-AES256-SHA
ECDHE-RSA-AES256-SHA
TLS_RSA_WITH_AES_128_GCM_SHA256
TLS_RSA_WITH_AES_256_GCM_SHA384
TLS_RSA_WITH_AES_128_CBC_SHA
TLS_RSA_WITH_AES_256_CBC_SHA
TLS_RSA_WITH_3DES_EDE_CBC_SHA
TLS_PSK_WITH_AES_128_CBC_SHA
TLS_PSK_WITH_AES_256_CBC_SHA
ECDHE-PSK-AES128-CBC-SHA
ECDHE-PSK-AES256-CBC-SHA
ECDHE-PSK-CHACHA20-POLY1305
```

### `alpn`: protocol list

Encoded into the length-prefixed wire format and offered to the server on the
client, or used as the accepted list on the server. Entries that are empty or
longer than 255 bytes are skipped silently. If `alpn` is omitted, no ALPN is
sent or accepted, and the `http2` layer cannot be used over this TLS
connection. **On the server, set `alpn = ["h2"]` in the profile that `tls_path` points to
when the chain contains an `http2` layer.**

### `curves`: supported groups, in order

Passed to `set_curves_list`. Full BoringSSL vocabulary:
```
P-256
P-384
P-521
X25519
X25519Kyber768Draft00
X25519MLKEM768
MLKEM1024
```

### `signature_algorithms`: in order

Passed to `set_sigalgs_list`. Full vocabulary:
```
rsa_pkcs1_md5_sha1
rsa_pkcs1_sha1
rsa_pkcs1_sha256
rsa_pkcs1_sha256_legacy
rsa_pkcs1_sha384
rsa_pkcs1_sha512
ecdsa_secp256r1_sha256
ecdsa_secp384r1_sha384
ecdsa_secp521r1_sha512
rsa_pss_rsae_sha256
rsa_pss_rsae_sha384
rsa_pss_rsae_sha512
ed25519
```

### `extensions_order`: exact extension positions

Controls the position of each TLS extension in the outgoing ClientHello
(`set_extension_permutation` under the hood). Every extension that is actually
active in the handshake (enabled via `cipher_suites`, `curves`,
`signature_algorithms`, `alpn`, `cert_compression`, `record_size_limit`, and so
on) should be listed here. **If an active extension is left out, its position
is undefined**: it may be dropped, appended or placed anywhere, and upstream
documents nothing here. Always list everything you enable.

Supported names (some accept aliases):

```
server_name
status_request
ec_point_formats
signature_algorithms
alpn
padding
signed_certificate_timestamp
extended_master_secret
quic_transport_parameters_legacy
quic_transport_parameters_standard (or quic_transport_parameters)
cert_compression
session_ticket
supported_groups
pre_shared_key
early_data
supported_versions
cookie
psk_key_exchange_modes
certificate_authorities
signature_algorithms_cert
key_share
renegotiation_info
delegated_credentials
application_settings
application_settings_old
encrypted_client_hello (or ech)
next_proto_neg (or npn)
channel_id
record_size_limit
```

An unknown name fails the connection with `unknown extension: <name>`.

`renegotiation_info` (RFC 5746) is not toggleable: BoringSSL always sends it,
like every modern browser. It only needs a position in this list.

### `cert_compression`: with real decompressors

Each name registers a working decompressor, so the handshake completes even if
the server sends a compressed certificate:

```
brotli
zlib
zstd
```

Unknown names are a hard error, not a silent skip.

### Boolean toggles

- **`permute_extensions`**: randomizes extension order on every handshake
  (Chrome behavior). **Never combine it with a populated `extensions_order`**:
  a fixed order and a random order are contradictory, so pick one per profile.
  Chrome style: `true` plus GREASE and no fixed order. Firefox style: fixed
  order and `false`.
- **`grease_enabled`**: injects GREASE values (random unknown cipher and
  extension codepoints). JA3/JA4 hashers normally strip GREASE, so this changes
  the wire bytes without changing the hash.
- **`status_request`**: OCSP stapling request. Browsers send it by default.
- **`signed_certificate_timestamp`**: Certificate Transparency SCT extension.
  Also on by default in browsers.
- **`session_ticket`**: whether the `session_ticket` extension is sent. The
  connector is built per connection, so no session resumption happens; this
  only controls the extension's presence in the fingerprint.
- **`alps`**: Application-Layer Protocol Settings. Sends an ALPS payload for
  `h2`, encoded from the same `settings` and `settings_order` that drive the
  real HTTP/2 SETTINGS frame (see [http2.md](http2.md)). Browsers use that
  exact encoding, so there is no separate ALPS config. The HTTP/2 profile is
  read from the file named by `http2_path` on the TLS layer. If `settings` or
  `settings_order` is missing, the payload is empty.

### `delegated_credentials`: opt-in extension

Enables the TLS `delegated_credentials` extension. The value is a list of
signature scheme names that a delegated credential may use. Omit the key to
leave the extension out.

```
ecdsa_secp256r1_sha256
ecdsa_secp384r1_sha384
ecdsa_secp521r1_sha512
ed25519
ecdsa_sha1
```

### `record_size_limit`: integer

Advertises the maximum TLS record size the client accepts (the
`record_size_limit` extension, 16-bit value). Omit the key to leave the
extension out.

## ECH (not implemented yet)

`enable_ech`, `enable_ech_grease` and `doh` are accepted by the config parser
but are **currently ignored**: nothing in the connector reads them, so they do
not add or remove an ECH extension. If you need an `encrypted_client_hello`
position in the ClientHello, list it in `extensions_order`; the ECH payload
itself is not produced yet. ECH lookup over DoH is planned.

## JA3 / JA4 notes

- JA3 hashes the ordered cipher list, extensions list, curves and point
  formats. All four are controlled here; point formats follow the BoringSSL
  defaults for the chosen curves.
- JA4 additionally folds in ALPN, SNI presence (the `server_name` position in
  `extensions_order`) and counts.
- GREASE codepoints are stripped by the hashers before hashing, so
  `grease_enabled` affects middlebox behavior, not the hash.

## Gotchas

- `cipher_suites`, `curves` and `signature_algorithms` must be set. An omitted
  list becomes an empty list, and BoringSSL rejects it.
- `http2_path` must be present on the client `tls` layer even when `alps` is
  `false`. It is only read when `alps = true`, so a placeholder is fine then.
- `extensions_order` must list **every** active extension, or positions are
  undefined. When in doubt, capture a real browser Hello (for example with
  `tls.peet.ws` or Wireshark) and mirror it.
- `permute_extensions = true` together with `extensions_order` is a
  contradictory config. Pick one.
- A server using an `http2` layer needs `alpn = ["h2"]` in the profile its `tls`
  layer points to; otherwise the HTTP/2 handshake fails.
- The server's acceptor is cached by `cert` and `key` only. If two `tls` layers
  share the same pair but use different `tls_path` files, the first profile wins.
- Unknown keys in a profile are ignored without any warning. If a setting seems
  to have no effect, check the spelling of the key first.
- The server certificate must be trusted by the Chromium root store and match
  `sni`.
- One profile applies per config file. To present a different fingerprint,
  run another instance with another config.