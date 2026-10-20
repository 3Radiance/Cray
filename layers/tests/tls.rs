use btls::asn1::{Asn1Integer, Asn1Time};
use btls::bn::BigNum;
use btls::ec::{EcGroup, EcKey};
use btls::hash::MessageDigest;
use btls::nid::Nid;
use btls::pkey::{PKey, Private};
use btls::ssl::{SslConnector, SslMethod, SslVerifyMode};
use btls::x509::{X509, X509NameBuilder};
use cray_core::*;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};
use tokio_btls::SslStream;

/// Self-signed cert for `cn` written to PEM files in `dir`; returns (cert, key) paths.
fn self_signed(dir: &Path, cn: &str) -> (PathBuf, PathBuf) {
    let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).expect("P-256 group");
    let key: PKey<Private> =
        PKey::from_ec_key(EcKey::generate(&group).expect("ec keygen")).expect("pkey from ec");

    let mut name = X509NameBuilder::new().expect("name builder");
    name.append_entry_by_text("CN", cn).expect("append cn");
    let name = name.build();

    let mut cert = X509::builder().expect("x509 builder");
    cert.set_version(2).expect("version");
    let serial =
        Asn1Integer::from_bn(&BigNum::from_u32(fastrand::u32(1..1 << 31)).expect("bignum"))
            .expect("serial");
    cert.set_serial_number(&serial).expect("serial");
    cert.set_not_before(&Asn1Time::days_from_now(0).expect("not before"))
        .expect("set not before");
    cert.set_not_after(&Asn1Time::days_from_now(1).expect("not after"))
        .expect("set not after");
    cert.set_issuer_name(&name).expect("set issuer");
    cert.set_subject_name(&name).expect("set subject");
    cert.set_pubkey(&key).expect("set pubkey");
    cert.sign(&key, MessageDigest::sha256()).expect("sign");
    let cert = cert.build();

    let cert_path = dir.join(format!("{cn}.pem"));
    let key_path = dir.join(format!("{cn}.key"));
    std::fs::write(&cert_path, cert.to_pem().expect("cert pem")).expect("write cert");
    std::fs::write(&key_path, key.private_key_to_pem_pkcs8().expect("key pem")).expect("write key");
    (cert_path, key_path)
}

fn tmpdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cray-tls-test-{tag}-{}-{:x}",
        std::process::id(),
        fastrand::u64(..)
    ));
    std::fs::create_dir_all(&dir).expect("tempdir");
    dir
}

fn try_build(params: &toml::Table, side: Side) -> anyhow::Result<Box<dyn Layer>> {
    let mut reg = Registry::new();
    cray_layers::register(&mut reg);
    let f = reg.get("tls").expect("tls registered");
    f(params, &BuildCx { side })
}

fn build_err(toml_str: &str) -> String {
    let params: toml::Table = toml_str.parse().expect("valid toml");
    match try_build(&params, Side::Inbound) {
        Ok(_) => panic!("expected a build error for:\n{toml_str}"),
        Err(e) => format!("{e:#}"),
    }
}

/// CN from the certificate the server sent us.
fn peer_cn(stream: &SslStream<tokio::io::DuplexStream>) -> String {
    let cert = stream.ssl().peer_certificate().expect("peer cert received");
    let entry = cert
        .subject_name()
        .entries_by_nid(Nid::COMMONNAME)
        .next()
        .expect("CN entry");
    entry.data().as_utf8().expect("CN utf8").to_string()
}

/// Fixture: certs for `up.test` and `down.test`; default = up.test, alpn = h2.
fn sni_params(dir: &Path) -> toml::Table {
    let (up_c, up_k) = self_signed(dir, "up.test");
    let (down_c, down_k) = self_signed(dir, "down.test");
    format!(
        r#"
certs = [
    {{ sni = "up.test", cert = "{}", key = "{}" }},
    {{ sni = "down.test", cert = "{}", key = "{}" }},
]
default = "up.test"
alpn = ["h2"]
"#,
        up_c.display(),
        up_k.display(),
        down_c.display(),
        down_k.display(),
    )
    .parse()
    .expect("valid toml")
}

/// Client TLS handshake with a given SNI; verification is off (self-signed local certs).
/// Returns the handshake result.
fn connect_client(
    client: tokio::io::DuplexStream,
    sni: &str,
) -> tokio::task::JoinHandle<SslStream<tokio::io::DuplexStream>> {
    let sni = sni.to_string();
    tokio::spawn(async move {
        let mut conn = SslConnector::builder(SslMethod::tls()).expect("connector builder");
        conn.set_verify(SslVerifyMode::NONE);
        conn.set_alpn_protos(b"\x02h2").expect("alpn protos");
        let ssl = conn
            .build()
            .configure()
            .expect("connector configure")
            .into_ssl(&sni)
            .expect("into ssl");
        let mut stream = SslStream::new(ssl, client).expect("ssl stream");
        Pin::new(&mut stream)
            .connect()
            .await
            .expect("client handshake");
        stream
    })
}

// ---------------------------------------------------------------- build

#[test]
fn inbound_requires_certs() {
    assert!(
        build_err("default = \"up.test\"").contains("requires 'certs'"),
        "missing 'certs' must fail the build"
    );
}

#[test]
fn inbound_requires_default() {
    let e = build_err("certs = [{ sni = \"up.test\", cert = \"a\", key = \"b\" }]");
    assert!(
        e.contains("requires 'default'"),
        "missing 'default' must fail the build, got: {e}"
    );
}

#[test]
fn default_must_exist_in_certs() {
    let e = build_err(
        "certs = [{ sni = \"up.test\", cert = \"a\", key = \"b\" }]\ndefault = \"missing.test\"",
    );
    assert!(
        e.contains("missing.test") && e.contains("is not in 'certs'"),
        "unknown default must name it and fail the build, got: {e}"
    );
}

#[test]
fn duplicate_sni_rejected() {
    let e = build_err(
        "certs = [
            { sni = \"up.test\", cert = \"a\", key = \"b\" },
            { sni = \"up.test\", cert = \"c\", key = \"d\" },
        ]
        default = \"up.test\"",
    );
    assert!(
        e.contains("duplicate sni 'up.test'"),
        "duplicate sni must fail the build, got: {e}"
    );
}

// ------------------------------------------------------------- handshake

#[tokio::test]
async fn selects_cert_per_sni() {
    let dir = tmpdir("per-sni");
    let params = sni_params(&dir);
    let tls = try_build(&params, Side::Inbound).expect("tls layer builds");

    for (sni, want) in [("up.test", "up.test"), ("down.test", "down.test")] {
        let (a, b) = duplex(64 * 1024);
        let client = connect_client(b, sni);

        let mut server = match tls
            .call(Input::Stream(Box::pin(a)), &mut Cx::default())
            .await
            .expect("server handshake")
        {
            Output::Stream(s) => s,
            _ => panic!("tls must give Stream"),
        };
        let mut client = client.await.expect("client task joined");

        assert_eq!(
            client.ssl().selected_alpn_protocol(),
            Some(b"h2".as_slice())
        );
        assert_eq!(peer_cn(&client), want, "cert must match handshake SNI");

        // plaintext flows both ways after the handshake
        client.write_all(b"ping").await.expect("client write");
        let mut buf = [0u8; 4];
        server.read_exact(&mut buf).await.expect("server read");
        assert_eq!(&buf, b"ping");
        server.write_all(b"pong").await.expect("server write");
        let mut buf = [0u8; 4];
        client.read_exact(&mut buf).await.expect("client read");
        assert_eq!(&buf, b"pong");
    }
}

#[tokio::test]
async fn unknown_sni_falls_back_to_default() {
    let dir = tmpdir("default-sni");
    let params = sni_params(&dir);
    let tls = try_build(&params, Side::Inbound).expect("tls layer builds");

    let (a, b) = duplex(64 * 1024);
    let client = connect_client(b, "other.test");

    let mut server = match tls
        .call(Input::Stream(Box::pin(a)), &mut Cx::default())
        .await
        .expect("server handshake")
    {
        Output::Stream(s) => s,
        _ => panic!("tls must give Stream"),
    };
    let mut client = client.await.expect("client task joined");

    assert_eq!(
        peer_cn(&client),
        "up.test",
        "unknown SNI gets the default cert"
    );
    assert_eq!(
        client.ssl().selected_alpn_protocol(),
        Some(b"h2".as_slice())
    );

    client.write_all(b"ping").await.expect("client write");
    let mut buf = [0u8; 4];
    server.read_exact(&mut buf).await.expect("server read");
    assert_eq!(&buf, b"ping");
}

#[tokio::test]
async fn missing_sni_falls_back_to_default() {
    let dir = tmpdir("no-sni");
    let params = sni_params(&dir);
    let tls = try_build(&params, Side::Inbound).expect("tls layer builds");

    let (a, b) = duplex(64 * 1024);
    // No SNI at all: connector without a domain, i.e. no server name extension.
    let plain = tokio::spawn(async move {
        let mut conn = SslConnector::builder(SslMethod::tls()).expect("connector builder");
        conn.set_verify(SslVerifyMode::NONE);
        conn.set_alpn_protos(b"\x02h2").expect("alpn protos");
        let ssl = conn
            .build()
            .configure()
            .expect("configure")
            .use_server_name_indication(false)
            .verify_hostname(false)
            .into_ssl("")
            .expect("into ssl");
        let mut stream = SslStream::new(ssl, b).expect("ssl stream");
        Pin::new(&mut stream)
            .connect()
            .await
            .expect("client handshake");
        stream
    });

    let mut server = match tls
        .call(Input::Stream(Box::pin(a)), &mut Cx::default())
        .await
        .expect("server handshake")
    {
        Output::Stream(s) => s,
        _ => panic!("tls must give Stream"),
    };
    let mut client = plain.await.expect("client task joined");

    assert_eq!(peer_cn(&client), "up.test", "no SNI gets the default cert");
    client.write_all(b"ping").await.expect("client write");
    let mut buf = [0u8; 4];
    server.read_exact(&mut buf).await.expect("server read");
    assert_eq!(&buf, b"ping");
}
