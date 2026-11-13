#!/usr/bin/env python3
"""Convert wreq-util emulation profiles into cray TOML fingerprints.

Writes conf/fingerprint/<browser>/<wreq-profile>/{tls.toml,http2.toml}.
"""
import re, os
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent 

SRC = ROOT / "tools" / "profile"
OUT = ROOT / "conf" / "fingerprint"
ALPN = ["h2", "http/1.1"]

CIPHER_MAP = {
    "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256": "ECDHE-ECDSA-AES128-GCM-SHA256",
    "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256": "ECDHE-RSA-AES128-GCM-SHA256",
    "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384": "ECDHE-ECDSA-AES256-GCM-SHA384",
    "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384": "ECDHE-RSA-AES256-GCM-SHA384",
    "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256": "ECDHE-ECDSA-CHACHA20-POLY1305",
    "TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256": "ECDHE-RSA-CHACHA20-POLY1305",
    "TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA": "ECDHE-ECDSA-AES256-SHA",
    "TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA": "ECDHE-ECDSA-AES128-SHA",
    "TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA": "ECDHE-RSA-AES128-SHA",
    "TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA": "ECDHE-RSA-AES256-SHA",
    "TLS_RSA_WITH_AES_128_GCM_SHA256": "AES128-GCM-SHA256",
    "TLS_RSA_WITH_AES_256_GCM_SHA384": "AES256-GCM-SHA384",
    "TLS_RSA_WITH_AES_128_CBC_SHA": "AES128-SHA",
    "TLS_RSA_WITH_AES_256_CBC_SHA": "AES256-SHA",
    "TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA384": "ECDHE-ECDSA-AES256-SHA384",
    "TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA256": "ECDHE-ECDSA-AES128-SHA256",
    "TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA384": "ECDHE-RSA-AES256-SHA384",
    "TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA256": "ECDHE-RSA-AES128-SHA256",
    "TLS_RSA_WITH_AES_256_CBC_SHA256": "AES256-SHA256",
    "TLS_RSA_WITH_AES_128_CBC_SHA256": "AES128-SHA256",
    "TLS_ECDHE_ECDSA_WITH_3DES_EDE_CBC_SHA": "ECDHE-ECDSA-DES-CBC3-SHA",
    "TLS_ECDHE_RSA_WITH_3DES_EDE_CBC_SHA": "ECDHE-RSA-DES-CBC3-SHA",
    "TLS_RSA_WITH_3DES_EDE_CBC_SHA": "DES-CBC3-SHA",
}
UNSUPPORTED = {"TLS_AES_128_CCM_SHA256", "TLS_AES_256_CCM_8_SHA256"}
# btls(-sys) vendored BoringSSL has no MLDSA TLS signature algorithms
NO_MLDSA = {"mldsa44", "mldsa65", "mldsa87"}

def ciphers(s):
    out, dropped = [], []
    for c in s.split(":"):
        (dropped if c in UNSUPPORTED else out).append(CIPHER_MAP.get(c, c))
    return out, dropped

def uniq(s):
    seen = set()
    return [a for a in s.split(":") if not (a in seen or seen.add(a))]

def uniq_no_mldsa(s):
    """uniq() + drop MLDSA sigalgs unsupported by btls BoringSSL."""
    out, dropped = [], []
    seen = set()
    for a in s.split(":"):
        if a in seen:
            continue
        seen.add(a)
        if a in NO_MLDSA:
            dropped.append(a)
            continue
        out.append(a)
    return out, dropped

def toml_str(s):
    return '"%s"' % s.replace("\\", "\\\\").replace('"', '\\"')

def toml_list(items):
    return "[\n" + ",\n".join("    %s" % toml_str(i) for i in items) + ",\n]"

# ---------------------------------------------------------------- tables

CHROME_CIPHER = (
    "TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:"
    "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256:TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256:"
    "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384:TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384:"
    "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256:TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256:"
    "TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA:TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA:"
    "TLS_RSA_WITH_AES_128_GCM_SHA256:TLS_RSA_WITH_AES_256_GCM_SHA384:"
    "TLS_RSA_WITH_AES_128_CBC_SHA:TLS_RSA_WITH_AES_256_CBC_SHA")
CHROME_SIGALGS = ("ecdsa_secp256r1_sha256:rsa_pss_rsae_sha256:rsa_pkcs1_sha256:"
    "ecdsa_secp384r1_sha384:rsa_pss_rsae_sha384:rsa_pkcs1_sha384:"
    "rsa_pss_rsae_sha512:rsa_pkcs1_sha512")
CHROME_NEW_SIGALGS = ("mldsa44:mldsa65:mldsa87:ecdsa_secp256r1_sha256:rsa_pss_rsae_sha256:"
    "rsa_pkcs1_sha256:ecdsa_secp384r1_sha384:rsa_pss_rsae_sha384:rsa_pkcs1_sha384:"
    "rsa_pss_rsae_sha512:rsa_pkcs1_sha512")

FF_CIPHER = {
    1: "TLS_AES_128_GCM_SHA256:TLS_CHACHA20_POLY1305_SHA256:TLS_AES_256_GCM_SHA384:"
       "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256:TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256:"
       "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256:TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256:"
       "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384:TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384:"
       "TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA:TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA:"
       "TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA:TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA:"
       "TLS_RSA_WITH_AES_128_GCM_SHA256:TLS_RSA_WITH_AES_256_GCM_SHA384:"
       "TLS_RSA_WITH_AES_128_CBC_SHA:TLS_RSA_WITH_AES_256_CBC_SHA",
    2: "TLS_AES_128_GCM_SHA256:TLS_CHACHA20_POLY1305_SHA256:TLS_AES_256_GCM_SHA384:"
       "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256:TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256:"
       "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256:TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256:"
       "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384:TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384:"
       "TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA:TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA:"
       "TLS_RSA_WITH_AES_128_GCM_SHA256:TLS_RSA_WITH_AES_256_GCM_SHA384:"
       "TLS_RSA_WITH_AES_128_CBC_SHA:TLS_RSA_WITH_AES_256_CBC_SHA",
    3: "TLS_AES_128_GCM_SHA256:TLS_CHACHA20_POLY1305_SHA256:TLS_AES_256_GCM_SHA384:"
       "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256:TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256:"
       "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256:TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256:"
       "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384:TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384:"
       "TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA:TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA:"
       "TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA:TLS_RSA_WITH_AES_128_GCM_SHA256:"
       "TLS_RSA_WITH_AES_256_GCM_SHA384:TLS_RSA_WITH_AES_128_CBC_SHA:TLS_RSA_WITH_AES_256_CBC_SHA",
}
FF_CURVES = {
    1: "X25519:P-256:P-384:P-521:ffdhe2048:ffdhe3072",
    2: "X25519MLKEM768:X25519:P-256:P-384:P-521:ffdhe2048:ffdhe3072",
}
FF_SIGALGS = ("ecdsa_secp256r1_sha256:ecdsa_secp384r1_sha384:ecdsa_secp521r1_sha512:"
    "rsa_pss_rsae_sha256:rsa_pss_rsae_sha384:rsa_pss_rsae_sha512:"
    "rsa_pkcs1_sha256:rsa_pkcs1_sha384:rsa_pkcs1_sha512:ecdsa_sha1:rsa_pkcs1_sha1")
FF_EXT = ("server_name extended_master_secret renegotiation_info supported_groups "
          "ec_point_formats session_ticket alpn status_request delegated_credentials "
          "certificate_timestamp key_share supported_versions signature_algorithms "
          "psk_key_exchange_modes record_size_limit cert_compression ech").split()
FF_DELEGATED = ("ecdsa_secp256r1_sha256:ecdsa_secp384r1_sha384:ecdsa_secp521r1_sha512:"
    "ecdsa_sha1")

SAFARI_CURVES = {1: "X25519:P-256:P-384:P-521", 2: "X25519MLKEM768:X25519:P-256:P-384:P-521"}
SAFARI_CIPHER = {
    1: "TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:"
       "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384:TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256:"
       "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256:TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384:"
       "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256:TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256:"
       "TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA384:TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA256:"
       "TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA:TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA:"
       "TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA384:TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA256:"
       "TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA:TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA:"
       "TLS_RSA_WITH_AES_256_GCM_SHA384:TLS_RSA_WITH_AES_128_GCM_SHA256:"
       "TLS_RSA_WITH_AES_256_CBC_SHA256:TLS_RSA_WITH_AES_128_CBC_SHA256:"
       "TLS_RSA_WITH_AES_256_CBC_SHA:TLS_RSA_WITH_AES_128_CBC_SHA:"
       "TLS_ECDHE_ECDSA_WITH_3DES_EDE_CBC_SHA:TLS_ECDHE_RSA_WITH_3DES_EDE_CBC_SHA:"
       "TLS_RSA_WITH_3DES_EDE_CBC_SHA",
    2: "TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:"
       "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384:TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256:"
       "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256:TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384:"
       "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256:TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256:"
       "TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA:TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA:"
       "TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA:TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA:"
       "TLS_RSA_WITH_AES_256_GCM_SHA384:TLS_RSA_WITH_AES_128_GCM_SHA256:"
       "TLS_RSA_WITH_AES_256_CBC_SHA:TLS_RSA_WITH_AES_128_CBC_SHA:"
       "TLS_ECDHE_ECDSA_WITH_3DES_EDE_CBC_SHA:TLS_ECDHE_RSA_WITH_3DES_EDE_CBC_SHA:"
       "TLS_RSA_WITH_3DES_EDE_CBC_SHA",
    3: "TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:TLS_AES_128_GCM_SHA256:"
       "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384:TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256:"
       "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256:TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384:"
       "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256:TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256:"
       "TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA:TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA:"
       "TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA:TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA:"
       "TLS_RSA_WITH_AES_256_GCM_SHA384:TLS_RSA_WITH_AES_128_GCM_SHA256:"
       "TLS_RSA_WITH_AES_256_CBC_SHA:TLS_RSA_WITH_AES_128_CBC_SHA:"
       "TLS_ECDHE_ECDSA_WITH_3DES_EDE_CBC_SHA:TLS_ECDHE_RSA_WITH_3DES_EDE_CBC_SHA:"
       "TLS_RSA_WITH_3DES_EDE_CBC_SHA",
}
SAFARI_SIGALGS = {
    1: "ecdsa_secp256r1_sha256:rsa_pss_rsae_sha256:rsa_pkcs1_sha256:"
       "ecdsa_secp384r1_sha384:ecdsa_sha1:rsa_pss_rsae_sha384:rsa_pss_rsae_sha384:"
       "rsa_pkcs1_sha384:rsa_pss_rsae_sha512:rsa_pkcs1_sha512:rsa_pkcs1_sha1",
    2: "ecdsa_secp256r1_sha256:rsa_pss_rsae_sha256:rsa_pkcs1_sha256:"
       "ecdsa_secp384r1_sha384:rsa_pss_rsae_sha384:rsa_pss_rsae_sha384:"
       "rsa_pkcs1_sha384:rsa_pss_rsae_sha512:rsa_pkcs1_sha512:rsa_pkcs1_sha1",
}

OKHTTP_CIPHER = (
    "TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:"
    "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256:TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256:"
    "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384:TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384:"
    "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256:TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256:"
    "TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA:TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA:"
    "TLS_RSA_WITH_AES_128_GCM_SHA256:TLS_RSA_WITH_AES_256_GCM_SHA384:"
    "TLS_RSA_WITH_AES_128_CBC_SHA:TLS_RSA_WITH_AES_256_CBC_SHA:TLS_RSA_WITH_3DES_EDE_CBC_SHA")
OKHTTP_CURVES = "X25519:P-256:P-384"
OKHTTP_SIGALGS = ("ecdsa_secp256r1_sha256:rsa_pss_rsae_sha256:rsa_pkcs1_sha256:"
    "ecdsa_secp384r1_sha384:rsa_pss_rsae_sha384:rsa_pkcs1_sha384:"
    "rsa_pss_rsae_sha512:rsa_pkcs1_sha512:rsa_pkcs1_sha1")

OPERA_CURVES = "X25519MLKEM768:X25519:P-256:P-384"

# ---------------------------------------------------------------- rust parsing

def _skip_string(s, k):
    """s[k] region starts a string (raw or plain). Return end index (exclusive) of full literal."""
    n = len(s)
    if s.startswith('r#"', k):
        e = s.index('"#', k + 3)
        return e + 2
    e = k + 1
    while e < n and s[e] != '"':
        if s[e] == '\\':
            e += 1
        e += 1
    return e + 1

def _find_close(s, i):
    """i points at '('; return index of matching close paren (string-aware)."""
    d, k, n = 1, i + 1, len(s)
    while k < n:
        ch = s[k]
        if s.startswith('r#"', k) or (ch == '"' and s[k-1:k] not in ('\\',)):
            k = _skip_string(s, k)
            continue
        if ch in "([{":
            d += 1
        elif ch in ")]}":
            d -= 1
            if d == 0:
                return k
        k += 1
    raise ValueError("unbalanced")

def _top_split(s):
    """s is the content directly inside one opening delimiter; split at depth-1 commas."""
    segs, cur, d, k = [], [], 1, 0
    n = len(s)
    while k < n:
        ch = s[k]
        if s.startswith('r#"', k):
            e = _skip_string(s, k)
            cur.append(s[k:e])
            k = e
            continue
        if ch == '"':
            e = _skip_string(s, k)
            cur.append(s[k:e])
            k = e
            continue
        if ch in "([{":
            d += 1
        elif ch in ")]}":
            d -= 1
        elif ch == "," and d == 1:
            segs.append("".join(cur).strip())
            cur = []
            k += 1
            continue
        cur.append(ch)
        k += 1
    segs.append("".join(cur).strip())
    return segs

def parse_mods(path):
    text = open(path).read()
    mods = []
    for m in re.finditer(r"mod_generator!\(", text):
        close = _find_close(text, m.end() - 1)
        chunks = _top_split(text[m.end():close])
        chunks = [c for c in chunks if c]
        if not chunks:
            continue
        mods.append(chunks)
        m.start  # keep iter semantics
        # continue scanning after this mod
    return mods

def strings_in(s):
    vals = []
    k, n = 0, len(s)
    while k < n:
        if s.startswith('r#"', k):
            e = s.index('"#', k + 3)
            vals.append(s[k + 3:e])
            k = e + 2
            continue
        if s[k] == '"':
            e = k + 1
            buf = []
            while e < n and s[e] != '"':
                if s[e] == '\\':
                    esc = s[e + 1]
                    buf.append({'n': '\n', 't': '\t', 'r': '\r', '"': '"', '\\': '\\'}.get(esc, esc))
                    e += 2
                    continue
                buf.append(s[e])
                e += 1
            vals.append("".join(buf))
            k = e + 1
            continue
        k += 1
    return vals

def os_list_of(chunk):
    """chunk is the `[ (OS, secch, ua), ... ]` segment. -> [(os, secch_or_None, ua), ...]"""
    res = []
    lb = chunk.index("[")
    close = _find_close(chunk, lb)
    inner = chunk[lb + 1:close]
    for grp in _top_split(inner):
        if not grp or not grp.startswith("("):
            continue
        gc = _find_close(grp, 0)
        parts = _top_split(grp[1:gc])
        parts = [p for p in parts if p]
        if not parts:
            continue
        os_name = parts[0]
        os_name = "MacOS" if os_name == "MacOs" else os_name
        strs = []
        for p in parts[1:]:
            strs.extend(strings_in(p))
        if len(strs) >= 2:
            res.append((os_name, strs[0], strs[1]))
        elif len(strs) == 1:
            res.append((os_name, None, strs[0]))
    return res

def resolve(c, mods, idx):
    guard = 0
    while c[idx].endswith("::build_emulation"):
        src = c[idx].split("::")[0]
        c = next(m for m in mods if m[0] == src)
        guard += 1
        if guard > 12:
            raise RuntimeError("alias loop")
    return c

# ---------------------------------------------------------------- headers

PLATFORMS = {"MacOS": '"macOS"', "Windows": '"Windows"', "Linux": '"Linux"',
             "Android": '"Android"', "IOS": '"iOS"'}

def chrome_headers(kind, os_name, secch, ua):
    mobile = "?1" if os_name in ("Android", "IOS") else "?0"
    order, vals = [], {}
    order += ["sec-ch-ua", "sec-ch-ua-mobile", "sec-ch-ua-platform"]
    vals["sec-ch-ua"] = secch
    vals["sec-ch-ua-mobile"] = mobile
    vals["sec-ch-ua-platform"] = PLATFORMS[os_name]
    if kind == "chrome152":
        order += ["accept-language"]
        vals["accept-language"] = "en-US,en;q=0.9"
    order += ["upgrade-insecure-requests", "user-agent", "accept"]
    vals["upgrade-insecure-requests"] = "1"
    vals["user-agent"] = ua
    vals["accept"] = ("text/html,application/xhtml+xml,application/xml;q=0.9,"
                      "image/avif,image/webp,image/apng,*/*;q=0.8,"
                      "application/signed-exchange;v=b3;q=0.7")
    order += ["sec-fetch-site", "sec-fetch-mode", "sec-fetch-user", "sec-fetch-dest"]
    vals["sec-fetch-site"] = "none"
    vals["sec-fetch-mode"] = "navigate"
    vals["sec-fetch-user"] = "?1"
    vals["sec-fetch-dest"] = "document"
    enc = "gzip, deflate, br, zstd" if ("zstd" in kind or kind == "chrome152") else "gzip, deflate, br"
    order += ["accept-encoding"]
    vals["accept-encoding"] = enc
    if kind != "chrome152":
        order += ["accept-language"]
        vals["accept-language"] = "en-US,en;q=0.9"
    if "priority" in kind or kind == "chrome152":
        order += ["priority"]
        vals["priority"] = "u=0, i"
    return order, vals

def firefox_headers(kind, ua):
    order, vals = [], {}
    order += ["user-agent", "accept"]
    vals["user-agent"] = ua
    vals["accept"] = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"
    order += ["accept-language", "accept-encoding"]
    vals["accept-language"] = "en-US,en;q=0.9" if "q09" in kind else "en-US,en;q=0.5"
    vals["accept-encoding"] = "gzip, deflate, br, zstd" if "zstd" in kind else "gzip, deflate, br"
    order += ["upgrade-insecure-requests"]
    vals["upgrade-insecure-requests"] = "1"
    order += ["sec-fetch-dest", "sec-fetch-mode", "sec-fetch-site", "sec-fetch-user"]
    vals["sec-fetch-dest"] = "document"
    vals["sec-fetch-mode"] = "navigate"
    vals["sec-fetch-site"] = "none"
    vals["sec-fetch-user"] = "?1"
    if kind != "plain":
        order += ["priority"]
        vals["priority"] = "u=0, i"
    order += ["te"]
    vals["te"] = "trailers"
    return order, vals

def safari_headers(kind, ua):
    if kind == "safari15":
        return (["user-agent", "accept", "accept-language", "accept-encoding"],
                {"user-agent": ua,
                 "accept": "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
                 "accept-language": "en-US,en;q=0.9",
                 "accept-encoding": "gzip, deflate, br"})
    if kind == "safari16_17":
        return (["accept", "sec-fetch-site", "accept-encoding", "sec-fetch-mode",
                 "user-agent", "accept-language", "sec-fetch-dest"],
                {"accept": "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
                 "sec-fetch-site": "none", "accept-encoding": "gzip, deflate, br",
                 "sec-fetch-mode": "navigate", "user-agent": ua,
                 "accept-language": "en-US,en;q=0.9", "sec-fetch-dest": "document"})
    return (["sec-fetch-dest", "user-agent", "accept", "sec-fetch-site",
             "sec-fetch-mode", "accept-language", "priority", "accept-encoding"],
            {"sec-fetch-dest": "document", "user-agent": ua,
             "accept": "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
             "sec-fetch-site": "none", "sec-fetch-mode": "navigate",
             "accept-language": "en-US,en;q=0.9", "priority": "u=0, i",
             "accept-encoding": "gzip, deflate, br"})

def okhttp_headers(ua):
    return (["accept", "accept-language", "user-agent", "accept-encoding"],
            {"accept": "*/*", "accept-language": "en-US,en;q=0.9", "user-agent": ua,
             "accept-encoding": "gzip"})

# ---------------------------------------------------------------- file renderer

NOTES = []
def n(m):
    NOTES.append(m)

def write_profile(browser, name, tls_pairs, h2_pairs, oses, h2_notes):
    d = os.path.join(OUT, browser, name)
    os.makedirs(d, exist_ok=True)

    def hdr():
        h = ["# wreq-util emulation profile: %s/%s" % (browser, name),
             "# UA by OS ('*' = default/first):"]
        for o, secch, ua in oses:
            h.append("#   %-8s %s" % (o.lower(), (ua if ua else (secch or ""))))
        h.append("")
        return h

    t = hdr()
    for key, v in tls_pairs:
        if isinstance(v, list):
            t.append("%s = %s" % (key, toml_list(v)))
        elif isinstance(v, bool):
            t.append("%s = %s" % (key, "true" if v else "false"))
        elif isinstance(v, int):
            t.append("%s = %s" % (key, v))
        elif isinstance(v, str):
            if re.match(r"^tls1(\.[0-3])?$", v) or v in ("sorted", "shuffled"):
                t.append('%s = "%s"' % (key, v))
            else:
                t.append("# NOTE: %s" % v)
    open(os.path.join(d, "tls.toml"), "w").write("\n".join(t) + "\n")

    h = hdr()
    # TOML: everything after a `[settings]` header belongs to that table,
    # so render [settings] last.
    delayed = None
    for key, v in h2_pairs:
        if key == "settings":
            delayed = v
            continue
        if key == "headers_priority":
            dep, w, ex = v
            h.append("headers_priority = { exclusive = %s, depends_on = %d, weight = %d }"
                     % ("true" if ex else "false", dep, w))
        elif isinstance(v, list):
            if key == "priority_frames":
                h.append("priority_frames = [" + ", ".join(
                    "{ stream_id = %d, depends_on = %d, weight = %d, exclusive = %s }"
                    % (s, dep, w, "true" if ex else "false")
                    for (s, dep, w, ex) in v) + "]")
            elif key == "http_headers" or (v and isinstance(v[0], tuple) and len(v[0]) == 2):
                # http_headers: table of name = value
                h.append("[http_headers]")
                for k2, v2 in v:
                    if v2 is None:
                        continue
                    h.append("%s = %s" % (k2, toml_str(v2)))
            else:
                h.append("%s = %s" % (key, toml_list(v)))
        elif isinstance(v, int):
            h.append("%s = %s" % (key, v))
        elif isinstance(v, str):
            h.append("# NOTE: %s" % v)
    if delayed is not None:
        h.append("[settings]")
        for k2, v2 in delayed:
            if isinstance(v2, bool):
                h.append("%s = %s" % (k2, "true" if v2 else "false"))
            else:
                h.append("%s = %s" % (k2, v2))
    for x in h2_notes:
        h.append("# NOTE: %s" % x)
    open(os.path.join(d, "http2.toml"), "w").write("\n".join(h) + "\n")

# ---------------------------------------------------------------- chrome / edge / opera

def chrome_tls(variant, curves_list, sig, chrome9, trust_anchors=None):
    cl, dropped = ciphers(CHROME_CIPHER)
    sig, sig_dropped = uniq_no_mldsa(sig)
    kw = [("cipher_suites", cl), ("alpn", ALPN),
          ("curves", curves_list),
          ("signature_algorithms", sig),
          ("cert_compression", ["brotli"]),
          ("grease_enabled", True), ("status_request", True),
          ("signed_certificate_timestamp", True), ("alps", True),
          ("alps_use_new_codepoint", variant >= 7),
          ("aes_hw_override", True),
          ("min_tls_version", "tls1.2"), ("max_tls_version", "tls1.3")]
    if trust_anchors:
        kw.append(("trust_anchors", trust_anchors))
    if variant in (3, 4, 5, 6, 7, 8, 9):
        kw.append(("permute_extensions", True))
    if variant != 1:
        kw.append(("enable_ech_grease", True))
    for x in (
        "chrome/edge/opera: wreq pre_shared_key=true не выразимо в cray TlsConfig" if variant in (5, 6, 7, 8, 9) else None,
        "chrome 152/154: grease_sigalgs_enabled — поля нет в cray (fork-btls only)" if chrome9 else None,
    ):
        if x:
            n(x)
    if sig_dropped:
        n("wreq NEW_SIGALGS: в btls(-sys) 0.5.6 BoringSSL нет ML-DSA sigalgs — убраны: " + ", ".join(sig_dropped))
    return kw, dropped + sig_dropped

def chrome_h2(hv):
    base = ["header_table_size", "enable_push", "max_concurrent_streams",
            "initial_window_size", "max_frame_size", "max_header_list_size"]
    setv = [("header_table_size", 65536)]
    if hv != 3:
        setv.append(("max_concurrent_streams", 1000))
    if hv in (2, 3):
        setv.insert(1, ("enable_push", False))
    else:
        # wreq не задаёт enable_push (не шлётся); поле обязательно по схеме —
        # включаем формально, в settings_order не добавляем
        setv.append(("enable_push", True))
    setv += [("initial_window_size", 6291456), ("max_header_list_size", 262144)]
    enabled = set(k for k, _ in setv if k != "enable_push" or hv in (2, 3))
    order = [x for x in base if x in enabled]
    # wreq держит оба идентификатора в settings_order, значения не задаёт —
    # cray их пропускает (не отправляет), порядок в фрейме не меняется
    order += ["enable_connect_protocol", "no_rfc7540_priorities"]
    return order, setv, 15728640 - 65535, (0, 255, True), \
           [":method", ":authority", ":scheme", ":path"]

def emit_chrome_family():
    mods = parse_mods(os.path.join(SRC, "chrome.rs"))
    specs = {  # root -> (chrome_variant, curves_list, sigalgs, chrome9)
        "v100": (1, ["X25519", "P-256", "P-384"], CHROME_SIGALGS, False),
        "v105": (2, ["X25519", "P-256", "P-384"], CHROME_SIGALGS, False),
        "v106": (3, ["X25519", "P-256", "P-384"], CHROME_SIGALGS, False),
        "v116": (4, ["X25519", "P-256", "P-384"], CHROME_SIGALGS, False),
        "v117": (5, ["X25519", "P-256", "P-384"], CHROME_SIGALGS, False),
        "v118": (4, ["X25519", "P-256", "P-384"], CHROME_SIGALGS, False),
        "v124": (6, ["X25519Kyber768Draft00", "X25519", "P-256", "P-384"], CHROME_SIGALGS, False),
        "v131": (6, ["X25519MLKEM768", "X25519", "P-256", "P-384"], CHROME_SIGALGS, False),
        "v132": (7, ["X25519MLKEM768", "X25519", "P-256", "P-384"], CHROME_SIGALGS, False),
        "v150": (8, ["X25519MLKEM768", "X25519", "P-256", "P-384"], CHROME_NEW_SIGALGS, False),
        "v152": (9, ["X25519MLKEM768", "X25519", "P-256", "P-384"], CHROME_NEW_SIGALGS, True),
        "v154": (9, ["X25519MLKEM768", "X25519", "P-256", "P-384"], CHROME_NEW_SIGALGS, True),
    }
    # chrome 152/153 shuffle trust anchor ids per client config, 154 sorts them
    trust = {"v152": "shuffled", "v154": "sorted"}
    h2specs = {"v100": 1, "v105": 1, "v106": 2, "v116": 2, "v117": 3, "v118": 3,
               "v124": 3, "v131": 3, "v132": 3, "v150": 3, "v152": 3, "v154": 3}
    for m in mods:
        name = m[0]
        tm = resolve(m, mods, 1)
        root = tm[0]
        variant, curves, sig, c9 = specs[root]
        hv = h2specs[root]
        order, setv, cwu, hprio, pseudo = chrome_h2(hv)
        oses = os_list_of(m[-1])
        o_def, secch, ua = oses[0]
        header = m[2] if m[2].startswith("header_initializer") else m[3]
        kind = header.replace("header_initializer", "").strip("(),'")
        if kind == "":
            kind = "plain"
        hdr_order, vals = chrome_headers(kind, o_def, secch, ua)
        tls_pairs, dropped = chrome_tls(variant, curves, sig, c9, trust.get(root))
        h2 = [("settings_order", order), ("settings", setv),
              ("connection_window_update", cwu),
              ("pseudo_headers_order", pseudo), ("headers_priority", hprio),
              ("headers_order", hdr_order), ("http_headers", list(vals.items()))]
        notes = []
        if dropped:
            notes.append("removed unsupported ciphers: " + ", ".join(dropped))
        write_profile("chrome", name, tls_pairs, h2, oses, notes)

def emit_opera():
    mods = parse_mods(os.path.join(SRC, "opera.rs"))
    cl, _ = ciphers(CHROME_CIPHER)
    kw = [("cipher_suites", cl), ("alpn", ALPN),
          ("curves", OPERA_CURVES.split(":")),
          ("signature_algorithms", uniq(CHROME_SIGALGS)),
          ("cert_compression", ["brotli"]),
          ("grease_enabled", True), ("status_request", True),
          ("signed_certificate_timestamp", True), ("alps", True),
          ("alps_use_new_codepoint", False),
          ("aes_hw_override", True),
          ("min_tls_version", "tls1.2"), ("max_tls_version", "tls1.3"),
          ("permute_extensions", True), ("enable_ech_grease", True)]
    n("opera: pre_shared_key=true (psk) — не выразимо в cray TlsConfig")
    order, setv, cwu, hprio, pseudo = chrome_h2(3)  # opera = push false, no conc
    for m in mods:
        oses = os_list_of(m[-1])
        o_def, secch, ua = oses[0]
        hdr_order, vals = chrome_headers("zstd_priority", o_def, secch, ua)
        h2 = [("settings_order", order), ("settings", setv),
              ("connection_window_update", cwu),
              ("pseudo_headers_order", pseudo), ("headers_priority", hprio),
              ("headers_order", hdr_order), ("http_headers", list(vals.items()))]
        write_profile("opera", m[0], kw, h2, oses, [])

# ---------------------------------------------------------------- firefox

FF_H2 = {
    1: dict(table=65536, push=False, conc=None, window=131072, frame=16384,
            hprio=(0, 41, False), frames=None, sid=3),
    2: dict(table=65536, push=None, conc=None, window=131072, frame=16384,
            hprio=(13, 41, False), sid=15,
            frames=[(3, 0, 200, False), (5, 0, 100, False),
                    (7, 0, 0, False), (9, 7, 0, False),
                    (11, 3, 0, False), (13, 0, 240, False)]),
    3: dict(table=65536, push=False, conc=0, window=131072, frame=16384,
            hprio=(0, 41, False), frames=None, sid=3),
    4: dict(table=4096, push=False, conc=None, window=32768, frame=16384,
            hprio=(0, 41, False), frames=None, sid=3),
}
FF_BASE_ORDER = ["header_table_size", "enable_push", "max_concurrent_streams",
                 "initial_window_size", "max_frame_size", "max_header_list_size",
                 "enable_connect_protocol", "no_rfc7540_priorities"]
FF_KEY_SHARES = {
    1: ["X25519", "P-256"],
    2: ["X25519MLKEM768", "X25519", "P-256"],
}

def firefox_tls(variant, cipher_n, curves_n, key_shares_n):
    cl, _ = ciphers(FF_CIPHER[cipher_n])
    kw = [("cipher_suites", cl), ("alpn", ALPN),
          ("curves", FF_CURVES[curves_n].split(":")),
          ("signature_algorithms", uniq(FF_SIGALGS)),
          ("extensions_order", FF_EXT),
          ("status_request", True),
          ("session_ticket", variant not in (3, 6)),
          ("delegated_credentials", FF_DELEGATED.split(":")),
          ("record_size_limit", 0x4001),
          ("aes_hw_override", True),
          ("min_tls_version", "tls1.2"), ("max_tls_version", "tls1.3")]
    if key_shares_n:
        kw.append(("key_shares", FF_KEY_SHARES[key_shares_n]))
    if variant in (1, 4, 5, 6):
        kw.append(("cert_compression", ["zlib", "brotli", "zstd"]))
    if variant != 2:
        kw.append(("enable_ech_grease", True))
    if variant in (4, 6):
        kw.append(("signed_certificate_timestamp", True))
    if variant in (1, 4, 5, 6):
        n("firefox tls_options!(%d): pre_shared_key/psk_skip_session_tickets/psk_dhe_ke — не выразимо в cray" % variant)
    if key_shares_n:
        n("firefox KEY_SHARES_%d: random_aes_hw_override — поля нет в cray (fork-btls only)" % key_shares_n)
    return kw

def emit_firefox():
    mods = parse_mods(os.path.join(SRC, "firefox.rs"))
    specs = {}
    for m in mods:
        tls_e = m[1].strip().strip(",")
        if "::build_emulation" in tls_e:
            continue
        mv = re.match(r"tls_options!\((\d), CIPHER_LIST_(\d), CURVES_(\d)(?:, KEY_SHARES_(\d))?",
                      tls_e)
        assert mv, (m[0], tls_e)
        h2n = int(re.match(r"http2_options!\((\d)", m[2].strip().strip(",")).group(1))
        specs[m[0]] = dict(v=int(mv.group(1)), c=int(mv.group(2)), u=int(mv.group(3)),
                           k=int(mv.group(4)) if mv.group(4) else 0, h=h2n)
    for m in mods:
        name = m[0]
        tm = resolve(m, mods, 1)
        sp = specs[tm[0]]
        hm = tm if len(m) == 4 else resolve(m, mods, 2)
        hh = FF_H2[specs[hm[0]]["h"]]
        tls_pairs = firefox_tls(sp["v"], sp["c"], sp["u"], sp["k"])
        sv, order = [], []
        if hh["table"] is not None:
            sv.append(("header_table_size", hh["table"]))
        if hh["push"] is not None:
            sv.append(("enable_push", hh["push"]))
            order.append("enable_push")
        else:
            sv.append(("enable_push", True))
        if hh["conc"] is not None:
            sv.append(("max_concurrent_streams", hh["conc"]))
        order.append("max_concurrent_streams") if hh["conc"] is not None else None
        sv.append(("initial_window_size", hh["window"]))
        order.append("initial_window_size")
        sv.append(("max_frame_size", hh["frame"]))
        order.append("max_frame_size")
        # header_table_size is always set for firefox profiles
        order.append("header_table_size")
        # wreq keeps both identifiers in settings_order with no value set;
        # cray skips ids without a value, so the frame order is unchanged
        order += ["enable_connect_protocol", "no_rfc7540_priorities"]
        # reorder to wreq base order: table, push, conc, window, frame, ..., ecp, nrp
        order = sorted(set(order), key=FF_BASE_ORDER.index)
        oses = os_list_of(m[-1])
        o_def, secch, ua = oses[0]
        is_alias = len(m) == 4
        kind = (m[2] if is_alias else m[3])
        kind = kind.replace("header_initializer", "").strip("(),'")
        if kind == "":
            kind = "plain"
        hdr_order, vals = firefox_headers(kind, ua)
        h2 = [("settings_order", order), ("settings", sv),
              ("connection_window_update", 12517377),
              ("initial_stream_id", hh["sid"]),
              ("pseudo_headers_order", [":method", ":path", ":authority", ":scheme"]),
              ("headers_priority", hh["hprio"])]
        if hh["frames"]:
            h2.append(("priority_frames", hh["frames"]))
        h2 += [("headers_order", hdr_order), ("http_headers", list(vals.items()))]
        notes = []
        if hh["push"] is None:
            notes.append("wreq: enable_push не задан (не шлётся); тут enable_push=true обязателен по схеме, но не включён в settings_order")
        write_profile("firefox", name, tls_pairs, h2, oses, notes)

# ---------------------------------------------------------------- safari

SAF_H2 = {
    1: dict(res=(2097152, 10551295), push=None, so=1, po=1),
    2: dict(res=(2097152, 10551295), push=False, so=1, po=1),
    3: dict(res=(2097152, 10485760), push=False, so=2, po=2, nrp=True, ecp=True),
    4: dict(res=(4194304, 10551295), push=None, so=1, po=1),
    5: dict(res=(4194304, 10551295), push=False, so=1, po=1),
    6: dict(res=(2097152, 10485760), push=False, so=2, po=2, nrp=True),
}
SAF_ORDER = {
    1: ["header_table_size", "enable_push", "initial_window_size", "max_concurrent_streams",
        "max_frame_size", "max_header_list_size", "enable_connect_protocol", "no_rfc7540_priorities"],
    2: ["header_table_size", "enable_push", "max_concurrent_streams", "initial_window_size",
        "max_frame_size", "max_header_list_size", "enable_connect_protocol", "no_rfc7540_priorities"],
}
SAF_PSEUDO = {
    1: [":method", ":scheme", ":path", ":authority"],
    2: [":method", ":scheme", ":authority", ":path"],
}
SAF_HEADER_KINDS = {
    "header_initializer_for_15": "safari15",
    "header_initializer_for_16_17": "safari16_17",
    "header_initializer_for_18": "safari18",
}

def safari_tls(variant, c, s, cu):
    cl, _ = ciphers(SAFARI_CIPHER[c])
    kw = [("cipher_suites", cl), ("alpn", ALPN),
          ("curves", SAFARI_CURVES[cu].split(":")),
          ("signature_algorithms", uniq(SAFARI_SIGALGS[s])),
          ("cert_compression", ["zlib"]),
          ("grease_enabled", True), ("status_request", True),
          ("signed_certificate_timestamp", True), ("session_ticket", False),
          ("min_tls_version", "tls1.2" if variant == 3 else "tls1.0"),
          ("max_tls_version", "tls1.3")]
    if variant == 3:
        n("safari 26: preserve_tls13_cipher_list=true — семантика встроена в cray set_cipher_suites")
    return kw

def emit_safari():
    mods = parse_mods(os.path.join(SRC, "safari.rs"))
    specs = {}
    for m in mods:
        tls_e = m[1].strip().strip(",")
        if "::build_emulation" in tls_e:
            continue  # aliases inherit TLS/H2 from the resolution target
        mv = re.match(r"tls_options!\((\d)(?:, (\w+))?(?:, (\w+))?(?:, (\w+))?\)", tls_e)
        assert mv, (m[0], tls_e)
        v = int(mv.group(1))
        toks = [t for t in mv.groups()[1:] if t]
        # defaults from SafariTlsConfig builder: sigalgs=SIGALGS_LIST_1, curves=CURVES_1
        c = next((int(re.search(r"CIPHER_LIST_(\d)", t).group(1)) for t in toks if "CIPHER_LIST_" in t), None)
        s = next((int(re.search(r"SIGALGS_LIST_(\d)", t).group(1)) for t in toks if "SIGALGS_LIST_" in t), 1)
        cu = next((int(re.search(r"CURVES_(\d)", t).group(1)) for t in toks if "CURVES_" in t), 1)
        assert c is not None, (m[0], tls_e)
        h2n = int(re.match(r"http2_options!\((\d)", m[2].strip().strip(",")).group(1))
        specs[m[0]] = (v, c, s, cu, h2n)
    for m in mods:
        name = m[0]
        tm = resolve(m, mods, 1)
        v, c, s, cu, h2n = specs[tm[0]]
        hh = SAF_H2[h2n]
        tls_pairs = safari_tls(v, c, s, cu)
        w, conn = hh["res"]
        sv = [("max_concurrent_streams", 100), ("initial_window_size", w)]
        if hh.get("ecp"):
            sv.append(("enable_connect_protocol", True))
        if hh.get("nrp"):
            sv.append(("no_rfc7540_priorities", True))
        order = [x for x in SAF_ORDER[hh["so"]]
                 if dict(sv).get(x) is not None
                 or x in ("enable_connect_protocol", "no_rfc7540_priorities")]
        if hh["push"] is False:
            sv.insert(0, ("enable_push", False))
            if "enable_push" in order:
                pass
            else:
                order = ["enable_push"] + [x for x in order if x != "enable_push"]
        else:
            sv.insert(0, ("enable_push", True))
            order = [x for x in order if x != "enable_push"]
        # chunks: root = [name, tls, http2, header_fn, ua]; alias = [name, alias, header_fn, ua]
        is_alias = len(m) == 4
        header_tok = m[2] if is_alias else m[3]
        ua = strings_in(m[-1])[0]
        kind = SAF_HEADER_KINDS[header_tok]
        oses = [("MacOS", None, ua)]
        hdr_order, vals = safari_headers(kind, oses[0][2])
        h2 = [("settings_order", order), ("settings", sv),
              ("connection_window_update", conn - 65535),
              ("pseudo_headers_order", SAF_PSEUDO[hh["po"]]),
              ("headers_order", hdr_order), ("http_headers", list(vals.items()))]
        notes = []
        if hh["push"] is None:
            notes.append("wreq: enable_push не задан (не шлётся); тут включён формально — не в settings_order")
        write_profile("safari", name, tls_pairs, h2, oses, notes)

# ---------------------------------------------------------------- okhttp

def emit_okhttp():
    mods = parse_mods(os.path.join(SRC, "okhttp.rs"))
    for m in mods:
        tls_e = m[1].strip().strip(",")
        if tls_e == "CIPHER_LIST":
            cl = OKHTTP_CIPHER
        elif tls_e.upper().startswith("JOIN"):
            cl = ":".join(re.findall(r"TLS_[A-Z0-9_]+", tls_e))
        else:
            cl = OKHTTP_CIPHER
        ua = strings_in(m[2])[0]
        oses = [("Android", None, ua)]
        cl2, dropped = ciphers(cl)
        tls_pairs = [("cipher_suites", cl2), ("alpn", ALPN),
                     ("curves", OKHTTP_CURVES.split(":")),
                     ("signature_algorithms", uniq(OKHTTP_SIGALGS)),
                     ("status_request", True),
                     ("aes_hw_override", True),
                     ("min_tls_version", "tls1.2"), ("max_tls_version", "tls1.3")]
        order_h, vals = okhttp_headers(ua)
        h2 = [("settings_order", ["header_table_size", "max_concurrent_streams",
                                  "initial_window_size", "max_frame_size",
                                  "max_header_list_size",
                                  "enable_connect_protocol", "no_rfc7540_priorities"]),
              ("settings", [("enable_push", True), ("initial_window_size", 16777216)]),
              ("connection_window_update", 16777216 - 65535),
              ("pseudo_headers_order", [":method", ":path", ":authority", ":scheme"]),
              ("headers_order", order_h), ("http_headers", list(vals.items()))]
        notes = []
        if dropped:
            notes.append("removed unsupported TLS1.3 ciphers: " + ", ".join(dropped))
        notes.append("wreq okhttp: enable_push не задан (не шлётся); тут включён формально — не в settings_order")
        write_profile("okhttp", m[0], tls_pairs, h2, oses, notes)

# ---------------------------------------------------------------- main

def main():
    emit_chrome_family()
    emit_opera()
    emit_firefox()
    emit_safari()
    emit_okhttp()
    seen = set()
    print("== notes ==")
    for x in NOTES:
        if x not in seen:
            seen.add(x)
            print("-", x)

if __name__ == "__main__":
    main()
