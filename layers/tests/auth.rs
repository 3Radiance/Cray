use cray_core::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

const KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const HDR: usize = 34; // tag(32) + len(2)

fn try_auth_key(key: &str, side: Side) -> anyhow::Result<Box<dyn Layer>> {
    let mut reg = Registry::new();
    cray_layers::register(&mut reg);
    let f = reg.get("auth").expect("auth registered");
    let params: toml::Table = format!("key = \"{key}\"").parse().unwrap();
    f(&params, &BuildCx { side })
}

fn auth_layer(side: Side) -> Box<dyn Layer> {
    try_auth_key(KEY, side).expect("auth builds")
}

fn build_err(key: &str) -> String {
    match try_auth_key(key, Side::Outbound) {
        Ok(_) => panic!("expected a build error for key {key:?}"),
        Err(e) => format!("{e:#}"),
    }
}

async fn wrap(layer: &dyn Layer, stream: BoxedStream) -> BoxedStream {
    match layer
        .call(Input::Stream(stream), &mut Cx::default())
        .await
        .unwrap()
    {
        Output::Stream(s) => s,
        _ => panic!("auth must give Stream"),
    }
}

/// Two auth-wrapped ends of one pipe, (writer, reader).
async fn pair() -> (BoxedStream, BoxedStream) {
    let (a, b) = duplex(65536);
    let up = wrap(&*auth_layer(Side::Outbound), Box::pin(a) as BoxedStream).await;
    let dn = wrap(&*auth_layer(Side::Inbound), Box::pin(b) as BoxedStream).await;
    (up, dn)
}

async fn roundtrip(msg: &[u8]) -> Vec<u8> {
    let (mut up, mut dn) = pair().await;
    up.write_all(msg).await.unwrap();
    up.shutdown().await.unwrap();
    let mut out = Vec::new();
    dn.read_to_end(&mut out).await.unwrap();
    out
}

/// For messages larger than the pipe: reads run concurrently so writes cannot stall.
async fn roundtrip_big(msg: &[u8], pipe: usize) -> Vec<u8> {
    let (a, b) = duplex(pipe);
    let mut up = wrap(&*auth_layer(Side::Outbound), Box::pin(a) as BoxedStream).await;
    let mut dn = wrap(&*auth_layer(Side::Inbound), Box::pin(b) as BoxedStream).await;
    let want = msg.to_vec();
    let reader = tokio::spawn(async move {
        let mut out = Vec::new();
        dn.read_to_end(&mut out).await.unwrap();
        out
    });
    up.write_all(msg).await.unwrap();
    up.shutdown().await.unwrap();
    assert_eq!(reader.await.unwrap(), want);
    want
}

/// What `auth` puts on the wire for `msg`, read from the raw side.
async fn wire_of(msg: &[u8]) -> Vec<u8> {
    wire_of_pipe(msg, 1 << 20).await
}

/// Same, but with a small pipe so the writer sees backpressure while the raw
/// reader consumes concurrently.
async fn wire_of_pipe(msg: &[u8], pipe: usize) -> Vec<u8> {
    let (a, mut raw) = duplex(pipe);
    let mut up = wrap(&*auth_layer(Side::Outbound), Box::pin(a) as BoxedStream).await;
    let reader = tokio::spawn(async move {
        let mut wire = Vec::new();
        raw.read_to_end(&mut wire).await.unwrap();
        wire
    });
    up.write_all(msg).await.unwrap();
    up.shutdown().await.unwrap();
    reader.await.unwrap()
}

/// Reads from a raw (unauthenticated) writer into an auth reader.
async fn raw_read(raw_wire: &[u8]) -> anyhow::Result<Vec<u8>> {
    let (mut raw, b) = duplex(4096);
    let mut dn = wrap(&*auth_layer(Side::Inbound), Box::pin(b) as BoxedStream).await;
    raw.write_all(raw_wire).await.unwrap();
    raw.shutdown().await.unwrap();
    let mut out = Vec::new();
    dn.read_to_end(&mut out).await?;
    Ok(out)
}

/// A 34-byte header whose `len` field sits at [32..34].
fn header_with_len(len: u16) -> [u8; HDR] {
    let mut h = [0u8; HDR];
    h[32..34].copy_from_slice(&len.to_be_bytes());
    h
}

#[tokio::test]
async fn small_roundtrip() {
    let msg = b"hello auth, small message";
    assert_eq!(roundtrip(msg).await, msg);
}

#[tokio::test]
async fn wire_is_tag_len_payload() {
    let wire = wire_of(b"12345").await;
    assert_eq!(wire.len(), HDR + 5, "one frame: tag(32)+len(2)+payload(5)");
    assert_eq!(
        u16::from_be_bytes(wire[32..34].try_into().unwrap()),
        5,
        "length field must be big-endian, at [32..34]"
    );
    assert_eq!(&wire[HDR..], b"12345");
}

#[tokio::test]
async fn multi_frame_roundtrip() {
    // > 16 KiB forces the writer to split across frames and the reader to
    // walk Out -> Hdr -> Payload more than once.
    let msg: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    roundtrip_big(&msg, 65536).await;
}

#[tokio::test]
async fn tmp_find_threshold() {
    for k in 1..=8u32 {
        let msg: Vec<u8> = (0..(16_384 * k)).map(|i| (i % 251) as u8).collect();
        let len = msg.len();
        let (a, b) = duplex(65536);
        let mut up = wrap(&*auth_layer(Side::Outbound), Box::pin(a) as BoxedStream).await;
        let mut dn = wrap(&*auth_layer(Side::Inbound), Box::pin(b) as BoxedStream).await;
        let reader = tokio::spawn(async move {
            let mut out = Vec::new();
            let r = dn.read_to_end(&mut out).await;
            (r.is_ok(), out.len())
        });
        up.write_all(&msg).await.unwrap();
        up.shutdown().await.unwrap();
        let (ok, n) = reader.await.unwrap();
        eprintln!("k={k} len={len} reader_ok={ok} got_len={n}");
        assert!(ok, "failed at k={k} ({len} bytes)");
        assert_eq!(n, len, "wrong payload length at k={k}");
    }
}

#[tokio::test]
async fn tmp_wire_structure() {
    for &pipe in &[1 << 20, 65536, 100usize] {
        let msg: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let wire = wire_of_pipe(&msg, pipe).await;
        let mut off = 0usize;
        let mut frames = 0usize;
        let mut total = 0usize;
        while off < wire.len() {
            assert!(
                off + 34 <= wire.len(),
                "pipe {pipe}: truncated header at {off}"
            );
            let len = u16::from_be_bytes(wire[off + 32..off + 34].try_into().unwrap()) as usize;
            assert!(
                len <= 16_384,
                "pipe {pipe}: bad len {len} at frame {frames}"
            );
            assert!(
                off + 34 + len <= wire.len(),
                "pipe {pipe}: truncated payload at {off}"
            );
            off += 34 + len;
            total += len;
            frames += 1;
        }
        eprintln!(
            "pipe={pipe} frames={frames} wire_len={} total_payload={total}",
            wire.len()
        );
        assert_eq!(
            total,
            msg.len(),
            "pipe {pipe}: payload bytes must sum to msg len"
        );
        assert_eq!(
            wire.len(),
            msg.len() + frames * 34,
            "pipe {pipe}: overhead = 34 per frame"
        );
        let mut p = 0usize;
        let mut off = 0usize;
        for _ in 0..frames {
            let len = u16::from_be_bytes(wire[off + 32..off + 34].try_into().unwrap()) as usize;
            assert_eq!(
                &wire[off + 34..off + 34 + len],
                &msg[p..p + len],
                "pipe {pipe}: frame payload mismatch"
            );
            p += len;
            off += 34 + len;
        }
    }
}

#[tokio::test]
async fn tiny_pipe_survives_partial_io() {
    // a 100-byte pipe forces partial writes (woff) and partial reads (truncate)
    let msg: Vec<u8> = (0..100_000u32).map(|i| (i % 253) as u8).collect();
    roundtrip_big(&msg, 100).await;
}

#[tokio::test]
async fn tampered_payload_is_rejected() {
    let mut wire = wire_of(b"auth test").await;
    assert_eq!(wire.len(), HDR + 9);
    wire[HDR] ^= 0xFF; // flip the first payload byte, the tag no longer covers it
    assert!(raw_read(&wire).await.is_err());
}

#[tokio::test]
async fn wrong_key_is_rejected() {
    let (a, b) = duplex(4096);
    let mut up = wrap(&*auth_layer(Side::Outbound), Box::pin(a) as BoxedStream).await;
    let other = "ff".repeat(32);
    let mut dn = wrap(
        &*try_auth_key(&other, Side::Inbound).unwrap(),
        Box::pin(b) as BoxedStream,
    )
    .await;
    up.write_all(b"secret").await.unwrap();
    up.shutdown().await.unwrap();
    let mut out = Vec::new();
    assert!(dn.read_to_end(&mut out).await.is_err());
}

#[tokio::test]
async fn oversized_length_is_rejected() {
    // len > 16 KiB must be dropped before any payload is buffered
    let hdr = header_with_len(20_000);
    assert!(raw_read(&hdr).await.is_err());
}

#[tokio::test]
async fn eof_inside_a_frame_is_an_error() {
    // promises 10 bytes, sends 5, then closes
    let mut wire = header_with_len(10).to_vec();
    wire.extend_from_slice(&[0u8; 5]);
    assert!(raw_read(&wire).await.is_err());
}

#[tokio::test]
async fn clean_eof_is_not_an_error() {
    let (raw, b) = duplex(4096);
    let mut dn = wrap(&*auth_layer(Side::Inbound), Box::pin(b) as BoxedStream).await;
    drop(raw); // no bytes at all
    let mut out = Vec::new();
    assert_eq!(dn.read_to_end(&mut out).await.unwrap(), 0);
    assert!(out.is_empty());
}

#[test]
fn config_validation() {
    // a wrong-length key is caught at build time
    assert!(build_err("00").contains("32 bytes"));
    // non-hex is caught too
    assert!(build_err(&"zz".repeat(32)).contains("hex"));
    // the real key builds
    assert!(try_auth_key(KEY, Side::Outbound).is_ok());
}
