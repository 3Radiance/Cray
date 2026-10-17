use cray_core::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

fn try_pad(stages: &str, side: Side) -> anyhow::Result<Box<dyn Layer>> {
    let mut reg = Registry::new();
    cray_layers::register(&mut reg);
    let f = reg.get("pad").expect("pad registered");
    let params: toml::Table = format!("stages = [{stages}]").parse().unwrap();
    f(&params, &BuildCx { side })
}

fn pad_layer(stages: &str, side: Side) -> Box<dyn Layer> {
    try_pad(stages, side).expect("pad builds")
}

fn build_err(stages: &str) -> String {
    match try_pad(stages, Side::Outbound) {
        Ok(_) => panic!("expected a build error for {stages}"),
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
        _ => panic!("pad must give Stream"),
    }
}

async fn roundtrip(stages: &str, msg: &[u8]) -> Vec<u8> {
    let (a, b) = duplex(65536);
    let mut up = wrap(&*pad_layer(stages, Side::Outbound), Box::pin(a) as BoxedStream).await;
    let mut dn = wrap(&*pad_layer(stages, Side::Inbound), Box::pin(b) as BoxedStream).await;

    up.write_all(msg).await.unwrap();
    up.shutdown().await.unwrap();

    let mut out = Vec::new();
    dn.read_to_end(&mut out).await.unwrap();
    out
}

/// What `pad` puts on the wire for `msg`, read from the raw side.
async fn wire_of(stages: &str, msg: &[u8]) -> Vec<u8> {
    let (a, mut raw) = duplex(1 << 20);
    let mut up = wrap(&*pad_layer(stages, Side::Outbound), Box::pin(a) as BoxedStream).await;
    up.write_all(msg).await.unwrap();
    up.shutdown().await.unwrap();
    let mut wire = Vec::new();
    raw.read_to_end(&mut wire).await.unwrap();
    wire
}

/// Splits the wire into (payload_len, pad_len) per frame.
fn frames(mut wire: &[u8]) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    while !wire.is_empty() {
        let p = u16::from_be_bytes([wire[0], wire[1]]) as usize;
        let q = u16::from_be_bytes([wire[2], wire[3]]) as usize;
        v.push((p, q));
        wire = &wire[4 + p + q..];
    }
    v
}

#[tokio::test]
async fn fixed_size_roundtrip() {
    let msg: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
    let out = roundtrip(r#"{ size = "1400" }"#, &msg).await;
    assert_eq!(out, msg);
}

#[tokio::test]
async fn upto_roundtrip() {
    let msg = b"hello pad, small message";
    let out = roundtrip(r#"{ upto = "1400" }"#, msg).await;
    assert_eq!(out, msg);
}

#[tokio::test]
async fn no_pad_roundtrip() {
    let msg = b"no padding";
    let out = roundtrip(r#"{ upto = "0" }"#, msg).await;
    assert_eq!(out, msg);
}

#[tokio::test]
async fn no_pad_adds_only_the_header() {
    // 100 bytes + 4 bytes of header, not a 16 KiB frame
    let wire = wire_of(r#"{ upto = "0" }"#, &[7u8; 100]).await;
    assert_eq!(wire.len(), 104);
    let wire = wire_of(r#"{ size = "0" }"#, &[7u8; 100]).await;
    assert_eq!(wire.len(), 104);
}

#[tokio::test]
async fn small_write_inflates_to_one_frame() {
    let wire = wire_of(r#"{ size = "1400" }"#, &[7u8; 100]).await;
    assert_eq!(wire.len(), 1400, "100 bytes must inflate to one 1400 frame");
    assert_eq!(frames(&wire), vec![(100, 1296)]);
}

#[tokio::test]
async fn percent_pads_on_top_of_arrival() {
    // 10 bytes + 50% = 5 bytes of padding, and it must not fail on a tiny write
    let wire = wire_of(r#"{ size = "50%" }"#, &[1u8; 10]).await;
    assert_eq!(frames(&wire), vec![(10, 5)]);
}

#[tokio::test]
async fn stages_count_payload_bytes() {
    // stage 0 (frames of 200) lasts until 1000 payload bytes went through, then frames of 400
    let stages = r#"{ bytes = 1000, size = "200" }, { size = "400" }"#;
    let wire = wire_of(stages, &vec![9u8; 3000]).await;
    let mut sent = 0usize;
    for (p, q) in frames(&wire) {
        let want = if sent < 1000 { 200 } else { 400 };
        let is_tail = sent + p == 3000 && 4 + p + q < want; // never happens: frames are padded to size
        assert!(!is_tail);
        assert_eq!(4 + p + q, want, "frame after {sent} payload bytes");
        sent += p;
    }
    assert_eq!(sent, 3000);
}

#[tokio::test]
async fn survives_a_tiny_pipe() {
    // a 100-byte pipe forces partial writes of every frame
    let (a, b) = duplex(100);
    let stages = r#"{ size = "1400" }"#;
    let mut up = wrap(&*pad_layer(stages, Side::Outbound), Box::pin(a) as BoxedStream).await;
    let mut dn = wrap(&*pad_layer(stages, Side::Inbound), Box::pin(b) as BoxedStream).await;

    let msg: Vec<u8> = (0..100_000u32).map(|i| (i % 253) as u8).collect();
    let want = msg.clone();
    let reader = tokio::spawn(async move {
        let mut out = Vec::new();
        dn.read_to_end(&mut out).await.unwrap();
        out
    });
    up.write_all(&msg).await.unwrap();
    up.shutdown().await.unwrap();
    assert_eq!(reader.await.unwrap(), want);
}

#[tokio::test]
async fn empty_decoy_frames_are_skipped() {
    let (mut raw, b) = duplex(4096);
    let mut dn = wrap(&*pad_layer(r#"{ size = "1400" }"#, Side::Inbound), Box::pin(b) as BoxedStream).await;

    // frame with no payload and 10 bytes of padding, then a real frame
    raw.write_all(&[0, 0, 0, 10]).await.unwrap();
    raw.write_all(&[0xAA; 10]).await.unwrap();
    raw.write_all(&[0, 2, 0, 0]).await.unwrap();
    raw.write_all(b"hi").await.unwrap();
    raw.shutdown().await.unwrap();

    let mut out = Vec::new();
    dn.read_to_end(&mut out).await.unwrap();
    assert_eq!(out, b"hi");
}

#[tokio::test]
async fn oversized_header_is_rejected() {
    let (mut raw, b) = duplex(4096);
    let mut dn = wrap(&*pad_layer(r#"{ size = "1400" }"#, Side::Inbound), Box::pin(b) as BoxedStream).await;
    raw.write_all(&[0xFF, 0xFF, 0, 0]).await.unwrap();
    let mut out = Vec::new();
    assert!(dn.read_to_end(&mut out).await.is_err());
}

#[tokio::test]
async fn eof_inside_a_frame_is_an_error() {
    let (mut raw, b) = duplex(4096);
    let mut dn = wrap(&*pad_layer(r#"{ size = "1400" }"#, Side::Inbound), Box::pin(b) as BoxedStream).await;
    raw.write_all(&[0, 10, 0, 0]).await.unwrap();
    raw.write_all(b"short").await.unwrap(); // promised 10 bytes, sent 5
    raw.shutdown().await.unwrap();
    let mut out = Vec::new();
    assert!(dn.read_to_end(&mut out).await.is_err());
}

#[test]
fn config_validation() {
    // the header is 4 bytes, so a frame must be at least 5 (or 0 = no padding)
    assert!(build_err(r#"{ size = "4" }"#).contains("must be >"));
    assert!(build_err(r#"{ size = "1-3" }"#).contains("must be >"));
    assert!(build_err(r#"{ upto = "2" }"#).contains("must be >"));
    assert!(try_pad(r#"{ size = "5" }"#, Side::Outbound).is_ok());
    assert!(build_err(r#"{ size = "99999" }"#).contains("must be <="));
    assert!(build_err(r#"{ size = "1400", upto = "1400" }"#).contains("either"));
    assert!(build_err(r#"{ bytes = 10 }"#).contains("either"));
    // a middle stage without `bytes` would hide every later stage
    assert!(build_err(r#"{ size = "1200" }, { size = "1400" }"#).contains("only the last"));
    assert!(build_err(r#"{ bytes = 500, size = "1200" }, { bytes = 100, size = "1400" }"#).contains("increasing"));
    assert!(try_pad(r#"{ bytes = 500, size = "1200-1400" }, { size = "1400" }"#, Side::Outbound).is_ok());
}

#[test]
fn old_key_option_is_rejected() {
    let mut reg = Registry::new();
    cray_layers::register(&mut reg);
    let f = reg.get("pad").unwrap();
    let params: toml::Table = "key = \"00\"\nstages = [{ size = \"1400\" }]".parse().unwrap();
    assert!(f(&params, &BuildCx { side: Side::Outbound }).is_err());
}
