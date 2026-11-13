use cray_core::{AppConfig, Engine, Registry};
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

fn conf_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../conf")
}

fn tmpdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cray-profiles-test-{tag}-{}-{:x}",
        std::process::id(),
        rand::random::<u64>()
    ));
    std::fs::create_dir_all(&dir).expect("tempdir");
    dir
}

type Profile = BTreeMap<&'static str, String>;

fn render(p: &Profile) -> String {
    p.iter().map(|(k, v)| format!("{k} = {v}\n")).collect()
}

fn tls_ok() -> Profile {
    let mut p = Profile::new();
    p.insert(
        "cipher_suites",
        r#""TLS_AES_128_GCM_SHA256\nTLS_AES_256_GCM_SHA384\nTLS_CHACHA20_POLY1305_SHA256\nECDHE-ECDSA-AES128-GCM-SHA256\nECDHE-RSA-AES128-GCM-SHA256""#.into(),
    );
    p.insert("curves", r#""X25519\nP-256\nP-384""#.into());
    p.insert(
        "signature_algorithms",
        r#""ecdsa_secp256r1_sha256\nrsa_pss_rsae_sha256\nrsa_pkcs1_sha256""#.into(),
    );
    p.insert("alpn", r#"["h2"]"#.into());
    p.insert("alps", "false".into());
    p
}

fn h2_ok() -> String {
    r#"
settings_order = ["enable_push", "header_table_size", "max_concurrent_streams", "initial_window_size", "max_frame_size"]
pseudo_headers_order = [":method", ":authority", ":scheme", ":path"]
headers_order = ["accept", "user-agent"]

[settings]
header_table_size = 65535
enable_push = false
max_concurrent_streams = 100
initial_window_size = 65535
max_frame_size = 16384

[http_headers]
"accept" = "*/*"
"user-agent" = "test"
"#
    .into()
}

fn h2_with_top(extra: &str) -> String {
    format!("{extra}\n{}", h2_ok())
}

fn h2_with_setting(key: &str, value: &str) -> String {
    h2_ok()
        .lines()
        .map(|l| {
            if l.trim_start().starts_with(&format!("{key} ")) {
                format!("{key} = {value}")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_client(tls: &str, h2: &str) -> Result<(), String> {
    let dir = tmpdir("case");
    let (tls_path, h2_path) = (dir.join("tls.toml"), dir.join("http2.toml"));
    std::fs::write(&tls_path, tls).unwrap();
    std::fs::write(&h2_path, h2).unwrap();
    build_client_paths(tls_path.to_str().unwrap(), h2_path.to_str().unwrap())
}

fn build_client_paths(tls_path: &str, h2_path: &str) -> Result<(), String> {
    let cfg = format!(
        r#"
[[inbound.layers]]
type = "tcp"
addr = "127.0.0.1:0"

[[inbound.layers]]
type = "http-proxy"

[[outbound.layers]]
type = "tcp"
addr = "127.0.0.1:1"

[[outbound.layers]]
type = "tls"
sni = "example.com"
tls_path = '{tls_path}'
http2_path = '{h2_path}'

[[outbound.layers]]
type = "http2"
path = "/tun"
host = "example.com"
method = "POST"
config = '{h2_path}'

[[outbound.layers]]
type = "vless"
uuid = "11111111-1111-4111-8111-111111111111"
"#
    );
    engine_build(&cfg)
}

fn engine_build(cfg_text: &str) -> Result<(), String> {
    let res = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
        let cfg = AppConfig::parse(cfg_text).map_err(|e| format!("{e:#}"))?;
        let mut reg = Registry::new();
        cray_layers::register(&mut reg);
        Engine::build(&cfg, &reg)
            .map(|_| ())
            .map_err(|e| format!("{e:#}"))
    }));
    match res {
        Ok(r) => r,
        Err(p) => {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            Err(format!("PANIC: {msg}"))
        }
    }
}

fn expect_rejected(case: &str, r: Result<(), String>, must_name: &str) {
    match r {
        Ok(()) => panic!("[{case}] profile was accepted, expected an error naming '{must_name}'"),
        Err(e) if e.starts_with("PANIC") => {
            panic!("[{case}] build panicked instead of returning an error: {e}")
        }
        Err(e) => assert!(
            e.contains(must_name),
            "[{case}] error does not name '{must_name}':\n  {e}"
        ),
    }
}

#[test]
fn baseline_profiles_are_accepted() {
    build_client(&render(&tls_ok()), &h2_ok()).expect("baseline profiles must build");
}

#[test]
fn shipped_fingerprint_profiles_are_accepted() {
    let dir = conf_dir().join("fingerprint/tls");
    let h2 = conf_dir().join("http2.toml");
    let mut n = 0;
    for entry in std::fs::read_dir(&dir).expect("conf/fingerprint/tls") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "toml") {
            n += 1;
            build_client_paths(path.to_str().unwrap(), h2.to_str().unwrap())
                .unwrap_or_else(|e| panic!("shipped profile {} rejected: {e}", path.display()));
        }
    }
    assert!(n > 0, "no shipped tls profiles found in {}", dir.display());
}

#[test]
fn shipped_example_client_config_is_accepted() {
    let conf = conf_dir();
    let text = std::fs::read_to_string(conf.join("client.toml")).expect("conf/client.toml");
    let text = text.replace("\"./", &format!("\"{}/", conf.display()));
    engine_build(&text).unwrap_or_else(|e| panic!("conf/client.toml rejected: {e}"));
}

#[test]
fn tls_bad_values_are_named() {
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "unknown cipher",
            "cipher_suites",
            r#""TLS_AES_128_GCM_SHA256\nNOT-A-CIPHER""#,
            "NOT-A-CIPHER",
        ),
        ("unknown curve", "curves", r#""X25519\nP-999""#, "P-999"),
        (
            "unknown sigalg",
            "signature_algorithms",
            r#""rsa_pss_rsae_sha256\nrsa_foo_sha1""#,
            "rsa_foo_sha1",
        ),
        (
            "unknown extension",
            "extensions_order",
            r#""server_name\nnope_ext""#,
            "nope_ext",
        ),
        (
            "unknown cert compression",
            "cert_compression",
            r#"["brotli", "lz4"]"#,
            "lz4",
        ),
        (
            "bad delegated credential",
            "delegated_credentials",
            r#""ecdsa_foo""#,
            "ecdsa_foo",
        ),
        (
            "record_size_limit overflow",
            "record_size_limit",
            "70000",
            "record_size_limit",
        ),
        ("wrong type", "cipher_suites", "5", "cipher_suites"),
        (
            "only tls1.3 suites",
            "cipher_suites",
            r#"["TLS_AES_128_GCM_SHA256"]"#,
            "cipher_suites",
        ),
        (
            "typo in key",
            "cipher_suits",
            r#""TLS_AES_128_GCM_SHA256""#,
            "cipher_suits",
        ),
    ];
    let mut failed = Vec::new();
    for (case, key, val, must_name) in cases {
        let mut p = tls_ok();
        p.insert(key, (*val).into());
        let r = build_client(&render(&p), &h2_ok());
        if std::panic::catch_unwind(AssertUnwindSafe(|| expect_rejected(case, r, must_name)))
            .is_err()
        {
            failed.push(*case);
        }
    }
    assert!(
        failed.is_empty(),
        "cases not rejected with a clear error: {failed:?}"
    );
}

#[test]
fn tls_lists_accept_array_and_multiline_forms() {
    let mut p = tls_ok();
    p.insert(
        "cipher_suites",
        r#"["TLS_AES_128_GCM_SHA256", "ECDHE-ECDSA-AES128-GCM-SHA256 # trailing comment"]"#.into(),
    );
    build_client(&render(&p), &h2_ok()).expect("array form must build");
}
#[test]
fn tls_alps_requires_http2_profile() {
    let dir = tmpdir("alps");
    let tls_path = dir.join("tls.toml");
    let mut p = tls_ok();
    p.insert("alps", "true".into());
    std::fs::write(&tls_path, render(&p)).unwrap();
    let cfg = format!(
        r#"
[[inbound.layers]]
type = "tcp"
addr = "127.0.0.1:0"
[[inbound.layers]]
type = "http-proxy"
[[outbound.layers]]
type = "tcp"
addr = "127.0.0.1:1"
[[outbound.layers]]
type = "tls"
sni = "example.com"
tls_path = '{}'
[[outbound.layers]]
type = "vless"
uuid = "11111111-1111-4111-8111-111111111111"
"#,
        tls_path.display()
    );
    expect_rejected("alps without http2_path", engine_build(&cfg), "http2_path");
}

#[test]
fn tls_missing_profile_file_names_the_path() {
    let r = build_client_paths("/no/such/tls.toml", "/no/such/http2.toml");
    expect_rejected("missing file", r, "/no/such/tls.toml");
}

#[test]
fn tls_alps_accepts_xhttp2_profile() {
    let dir = tmpdir("alps-xh2");
    let tls_path = dir.join("tls.toml");
    let xh2_path = dir.join("xhttp2.toml");
    let mut p = tls_ok();
    p.insert("alps", "true".into());
    std::fs::write(&tls_path, render(&p)).unwrap();
    std::fs::write(
        &xh2_path,
        r#"
settings_order = ["header_table_size", "max_frame_size"]

[settings]
header_table_size = 65535
enable_push = false
max_frame_size = 16384

[uplink]
headers_order = ["x-session-id"]

[uplink.http_headers]
"x-session-id" = "id"

[downlink]
headers_order = ["x-session-id"]

[downlink.http_headers]
"x-session-id" = "id"
"#,
    )
    .unwrap();
    let cfg = format!(
        r#"
[[inbound.layers]]
type = "tcp"
addr = "127.0.0.1:0"
[[inbound.layers]]
type = "http-proxy"
[[outbound.layers]]
type = "tcp"
addr = "127.0.0.1:1"
[[outbound.layers]]
type = "tls"
sni = "example.com"
tls_path = '{tls_path}'
http2_path = '{xh2_path}'
[[outbound.layers]]
type = "vless"
uuid = "11111111-1111-4111-8111-111111111111"
"#,
        tls_path = tls_path.display(),
        xh2_path = xh2_path.display()
    );
    engine_build(&cfg).expect("xhttp2 profile must build for alps");
}

// #[test]
// fn generated_wreq_emulation_profiles_are_accepted() {
//     let root = conf_dir().join("fingerprint");
//     let h2_default = conf_dir().join("http2.toml");
//     let mut checked = 0;
//     let dirs = std::fs::read_dir(&root).expect("conf/fingerprint");
//     for browser in dirs {
//         let bdir = browser.unwrap().path();
//         if !bdir.is_dir() || bdir.file_name().unwrap() == "tls" {
//             continue;
//         }
//         for sub in std::fs::read_dir(&bdir).expect("browser dir") {
//             let sub = sub.unwrap().path();
//             let tls_path = sub.join("tls.toml");
//             if !tls_path.is_file() {
//                 continue;
//             }
//             let h2_path = match sub.join("http2.toml") {
//                 p if p.is_file() => p,
//                 _ => h2_default.clone(),
//             };
//             checked += 1;
//             build_client_paths(tls_path.to_str().unwrap(), h2_path.to_str().unwrap())
//                 .unwrap_or_else(|e| panic!("wreq profile {} rejected: {e}", sub.display()));
//         }
//     }
//     assert!(checked > 100, "expected ~139 wreq emulation profiles, found {checked}");
// }

#[test]
fn plain_http2_profile_rejects_xhttp2_sections() {
    let r = build_client(
        &render(&tls_ok()),
        &h2_with_top("[uplink]\nheaders_order = [\"x-session-id\"]"),
    );
    expect_rejected("uplink in plain profile", r, "uplink");
}

#[test]
fn http2_bad_values_are_named() {
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "unknown pseudo header",
            h2_ok().replace(r#"":path"]"#, r#"":foo"]"#),
            ":foo",
        ),
        (
            "unknown setting in order",
            h2_ok().replace(r#""max_frame_size"]"#, r#""bogus_setting"]"#),
            "bogus_setting",
        ),
        (
            "max_frame_size below 16384",
            h2_with_setting("max_frame_size", "100"),
            "max_frame_size",
        ),
        (
            "max_frame_size above 2^24-1",
            h2_with_setting("max_frame_size", "16777216"),
            "max_frame_size",
        ),
        (
            "initial_window_size above 2^31-1",
            h2_with_setting("initial_window_size", "4294967295"),
            "initial_window_size",
        ),
        (
            "even initial_stream_id",
            h2_with_top("initial_stream_id = 2"),
            "initial_stream_id",
        ),
        (
            "invalid header name",
            h2_ok().replace(r#""user-agent"]"#, r#""bad header"]"#),
            "bad header",
        ),
        (
            "invalid header value",
            h2_ok().replace(r#""user-agent" = "test""#, "\"user-agent\" = \"a\\nb\""),
            "user-agent",
        ),
        (
            "typo in key",
            h2_with_top("settngs_order = []"),
            "settngs_order",
        ),
    ];
    let mut failed = Vec::new();
    for (case, profile, must_name) in &cases {
        let r = build_client(&render(&tls_ok()), profile);
        if std::panic::catch_unwind(AssertUnwindSafe(|| expect_rejected(case, r, must_name)))
            .is_err()
        {
            failed.push(*case);
        }
    }
    assert!(
        failed.is_empty(),
        "cases not rejected with a clear error: {failed:?}"
    );
}
