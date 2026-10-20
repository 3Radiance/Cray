# TLS Fingerprint Customization

The outbound `tls` layer builds its ClientHello entirely from a TLS profile
file that the layer points to: cipher order, extensions and their order, curves,
signature algorithms, ALPN, GREASE, certificate compression. Every byte of the
ClientHello that fingerprinting sees is under your control. The stack is
BoringSSL via `btls` / `tokio-btls` (`layers/src/fingerprint/tls/`).

The inbound `tls` layer terminates TLS and picks the certificate by the
connection's SNI: a list of `domain -> certificate` pairs lives in `certs`, and
`default` is the certificate served to clients with a different or missing SNI.

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
certs = [
    { sni = "up.example.com",   cert = "/path/to/up.pem",   key = "/path/to/up.key" },
    { sni = "down.example.com", cert = "/path/to/down.pem", key = "/path/to/down.key" },
]
default = "up.example.com"     # for a miss or an absent SNI
alpn = ["h2"]                  # on inbound ALPN comes from the layer, tls_path is ignored
```

Layer fields:

| Field | outbound | inbound |
|---|---|---|
| `sni` | required: the name sent in the ClientHello and checked against the certificate | not used |
| `tls_path` | required: the fingerprint profile | ignored (a warning is logged) |
| `alpn` | not used | ALPN protocols to accept, e.g. `["h2"]` |
| `http2_path` | required, but the file is read only when `alps = true`; otherwise any placeholder string works | not used |
| `certs`, `default` | not used | required: a list of `{ sni, cert, key }` entries and, among them, the `sni` served when SNI is missing or does not match |
| `timeout` | seconds (`timeout = 6`) or preset name (`timeout = "tls"`); default 6 | same; default 6 |

How the files are handled:

- Relative paths are resolved against the process working directory, not
  against the config file.
- Profiles are loaded and validated at startup (the `tls` layer reads its
  `tls_path`, and `http2_path` when the profile enables `alps`), and cached
  by path. Edits need a restart.
- Keys that are not part of the profile are ignored silently, so a misspelled
  key simply has no effect.
- A missing required layer field is reported at startup, before listening.

TOML has no `null`. To leave an optional field unset, **omit the key**.

## SNI-based certificate selection (inbound)

The certificate is chosen by the SNI from the ClientHello:

- the match against `sni` in `certs` is exact; wildcard names (`*.example.com`)
  are not supported yet;
- when SNI is absent or matches nothing, the `default` certificate is served;
- all pairs are loaded and validated at startup: paths exist, PEM parses, the
  key matches the certificate, `sni` entries are unique, `default` refers to an
  existing entry. Any of these errors keeps the proxy from starting;
- the same `cert`/`key` used by several entries is not loaded twice: contexts
  are cached by the triple `cert`, `key`, `alpn` list;
- the choice is injected mid-handshake (`set_ssl_context` from the servername
  callback). From the outside the connection is terminated by one ordinary SSL;
  the selection logic is invisible to the peer.

## Timeouts

The `timeout` field exists on the `tcp`, `unix`, `tls` and `http2` layers. The
value is either seconds as a number or a preset name from the `[timeouts]` table
at the top of the config:

```toml
[timeouts]
tcp = 5
tls = 10
h2 = 4

[[outbound.layers]]
type = "tls"
sni = "example.com"
tls_path = "./tls.toml"
http2_path = "./http2.toml"
timeout = "tls"
```

The timeout covers the setup phase only: connecting for `tcp`/`unix`,
the handshake for `tls`/`http2`. It never limits the connection lifetime.
When the field is omitted, built-in defaults apply: `tcp`/`unix` — 5 s,
`tls` — 6 s, `http2` — 5 s. An unknown preset name or zero fails config
loading, so the proxy refuses to start.

## Connection flow

Outbound (`outbound.layers`):

```
tcp socket ──▶ tls layer ──▶ next layer (http2, protocol, ...)
                 1. take the SslConnector for `tls_path` (built once per profile file, cached)
                 2. set SNI from the layer, attach ALPS payload if alps = true (reads http2_path)
                 3. handshake with the profile, 6 second timeout by default (see "Timeouts")
                 4. verify the server certificate, return the encrypted stream
```

Inbound (`inbound.layers`):

```
tcp socket ──▶ tls layer ──▶ next layer (http2, protocol, ...)
                 1. SslAcceptor (mozilla_intermediate): the default pair + an SNI -> SslContext table
                    (contexts for every pair in certs, cache keyed by cert/key/alpn)
                 2. ALPN select callback, client preference order
                 3. mid-handshake: SNI from the ClientHello picks the context;
                    miss or no SNI -> the default context stays
                 4. handshake, 6 second timeout by default (see "Timeouts")
```

The ALPN callback walks the client's list in order and picks the first protocol
that also appears in inbound's `alpn`. If nothing matches, no ALPN is
negotiated.

The outbound connector is assembled in `tls_builder`
(`layers/src/fingerprint/tls/builder.rs`) in this order: Chromium root store → cipher
suites → ALPN → curves → signature algorithms → record size limit →
certificate compression → GREASE → extension permutation → OCSP → SCT →
session ticket → delegated credentials → extension order. ALPS is attached per
connection.

### Server certificate verification (outbound)

The outbound verifies the server certificate against the Chromium root store and
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

Encoded into the length-prefixed wire format and offered to the server by the
outbound layer, or used as the accepted list on inbound. Entries that are empty or
longer than 255 bytes are skipped silently. If `alpn` is omitted, no ALPN is
sent or accepted, and the `http2` layer cannot be used over this TLS
connection. **On inbound, set `alpn = ["h2"]` on the `tls` layer itself
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

Advertises the maximum TLS record size the outbound accepts (the
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
- `http2_path` must be present on the outbound `tls` layer even when `alps` is
  `false`. It is only read when `alps = true`, so a placeholder is fine then.
- `extensions_order` must list **every** active extension, or positions are
  undefined. When in doubt, capture a real browser Hello (for example with
  `tls.peet.ws` or Wireshark) and mirror it.
- `permute_extensions = true` together with `extensions_order` is a
  contradictory config. Pick one.
- Inbound using an `http2` layer needs `alpn = ["h2"]` on its `tls`
  layer; otherwise the HTTP/2 handshake fails.
- Inbound acceptor contexts are cached by `cert`, `key` and the `alpn` list; per connection the
  context matching the ClientHello SNI is switched in.
- Unknown keys in a profile are ignored without any warning. If a setting seems
  to have no effect, check the spelling of the key first.
- The server certificate must be trusted by the Chromium root store and match
  `sni`.
- One profile applies per config file. To present a different fingerprint,
  run another instance with another config.