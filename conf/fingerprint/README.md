# wreq-util emulation profiles → cray fingerprint TOML

[Читать на русском](README.ru.md)

A set of ready-made TLS/HTTP2 profiles for the `tls` and `http2` layers.

This is **not a copy of wreq**. The values (ciphers, curves, extension order,
SETTINGS, headers) are ported from
[wreq-util](https://github.com/0x676e67/wreq-util) (`src/emulate/profile`),
where they were matched against live browsers — but beyond that these are
standalone TOML profiles in cray's own format
(`layers/src/profiles/{tls.rs,http2.rs}` → TlsConfig / Http2Config). The cray
settings format is deliberately custom and flexible: any profile field can be
edited by hand without deferring to wreq. The profiles themselves exist for
convenience — they are presets and starting points, not a hard dependency:
if you need your own fingerprint, take the closest file and tweak the values
however you like.

> **⚠ Generated files.** Every `.toml` here was produced by `tools/gen.py`
> from the wreq-util sources: nobody wrote them by hand or captured them from
> real browsers. The converter is not perfect, so individual values may contain
> mistakes or typos. The test below only proves that a profile is accepted by
> `Engine::build`, not that the fingerprint matches the real browser. Compare a
> profile with a real capture before relying on it, and fix systematic problems
> in `tools/gen.py`.

## Structure

    conf/fingerprint/<browser>/<profile>/
        tls.toml     → TLS part (TlsConfig)
        http2.toml   → HTTP/2 part (Http2Config), including headers_order/[http_headers]

Folder names = module ids in wreq-util
(`chrome.rs`, `firefox.rs`, `safari.rs`, `opera.rs`, `okhttp.rs`).
Alias modules (e.g. `v133 → v132::build_emulation`) inherit TLS and SETTINGS
from the root profile but have their own UA/headers. Edge lives in `chrome/`
(next to the root versions), Opera is a separate folder.

## Profile counts

| folder   | profiles | contents                                             |
|----------|----------|------------------------------------------------------|
| chrome/  | 65       | v100..v154 + edge101..edge148                         |
| firefox/ | 21       | ff109..ff152, ff_android_135, ff_private_*            |
| safari/  | 29       | safari15_3..safari26_4, safari_ios_*, safari_ipad_*   |
| opera/   | 16       | opera116..opera131                                    |
| okhttp/  | 8        | okhttp3_9..okhttp5                                    |

## How to validate

    cargo test -p cray-layers --test profiles
    # the generated_wreq_emulation_profiles_are_accepted test runs every
    # folder through the full Engine::build

## Conversion notes

Fields that cannot be expressed in the cray schema are marked with `# NOTE:`
comments right in the files. There are few of them:

- `TLS_AES_128_CCM_SHA256` / `TLS_AES_256_CCM_8_SHA256` (okhttp3_13) were
  dropped: btls BoringSSL does not support them.
- `mldsa44/65/87` (chrome v150+) were dropped: btls(-sys) 0.5.6 BoringSSL has
  no ML-DSA sigalgs (that API exists only in the fork revision of btls used
  by wreq).
- `pre_shared_key` (PSK) is not expressible: it needs a client session cache,
  which neither btls-0.5.6 nor cray has yet.
- `grease_sigalgs_enabled` / `random_aes_hw_override` (chrome 152/154,
  firefox) — fork-btls APIs, not available in stock btls.
- `connection_window_update` = `initial_connection_window_size - 65535`
  (the cray field is the WINDOW_UPDATE size, not the full window).
- `enable_push` is a mandatory bool in `[settings]`; when wreq does not set
  it (i.e. never sends it in SETTINGS), it is still written out, but left out
  of `settings_order`, so it never reaches the frame.
- `enable_connect_protocol` / `no_rfc7540_priorities` are present in
  `settings_order` without values in every family (same as wreq — an id
  without a value is not sent); only Safari 18+ assigns a value (both `true`).

Everything else from the old "missing" list is now supported by the schema:
`key_shares`, `min_tls_version`/`max_tls_version`, `alps_use_new_codepoint`,
`trust_anchors` (sorted/shuffled), `aes_hw_override`, explicit
`initial_stream_id`, `priority_frames`, ALPS.

## Generation

The profiles are generated from the wreq-util sources:

    tools/fetch_wreq_profiles.sh   # copies src/emulate/profile into tools/profile/
    python3 tools/gen.py           # parses the Rust sources, writes conf/fingerprint/**

`fetch_wreq_profiles.sh` takes wreq-util at a pinned revision
(`917d2581d6761a94db66466caa6c2eb4d4cdbd43`, the one the committed files were
generated from); pass another commit or tag as the first argument to try a
newer one. `tools/profile/` is git-ignored and is not part of this repository.
Hand-editing the files is fine and expected — but regenerating wipes manual
edits, so persistent differences should go into `tools/gen.py` instead.

## License and attribution

The profile values come from [wreq-util](https://github.com/0x676e67/wreq-util),
© 0x676e67. At the pinned revision it is released under the Apache License 2.0;
older releases (for example 3.0.0-rc.x) were LGPL-3.0, so check the license of
the revision you generate from. Cray itself is GPL-3.0, which Apache-2.0 code
is compatible with. If you redistribute these files, keep this attribution.
