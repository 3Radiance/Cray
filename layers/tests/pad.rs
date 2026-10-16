use cray_core::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

const KEY: &str = "e451904254878ec3aa963892e03a3e3341c423c6a27bc1070f7ec28134f89b87";
const OTHER_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn pad_layer(key: &str, stages: &str, side: Side) -> Box<dyn Layer> {
    let mut reg = Registry::new();
    cray_layers::register(&mut reg);
    let f = reg.get("pad").expect("pad registered");
    let params: toml::Table = format!("key = \"{key}\"\nstages = [{stages}]")
        .parse()
        .unwrap();
    f(&params, &BuildCx { side }).expect("pad builds")
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
    let mut up = wrap(
        &*pad_layer(KEY, stages, Side::Outbound),
        Box::pin(a) as BoxedStream,
    )
    .await;
    let mut dn = wrap(
        &*pad_layer(KEY, stages, Side::Inbound),
        Box::pin(b) as BoxedStream,
    )
    .await;

    up.write_all(msg).await.unwrap();
    up.shutdown().await.unwrap();

    let mut out = Vec::new();
    dn.read_to_end(&mut out).await.unwrap();
    out
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
    let msg = b"auth only, no padding";
    let out = roundtrip(r#"{ upto = "0" }"#, msg).await;
    assert_eq!(out, msg);
}

#[tokio::test]
async fn wire_is_multiples_of_frame() {
    let (a, b) = duplex(65536);
    let mut up = wrap(
        &*pad_layer(KEY, r#"{ size = "1400" }"#, Side::Outbound),
        Box::pin(a) as BoxedStream,
    )
    .await;
    let mut raw = b;

    up.write_all(&[7u8; 100]).await.unwrap();
    up.shutdown().await.unwrap();

    let mut wire = Vec::new();
    raw.read_to_end(&mut wire).await.unwrap();
    assert_eq!(wire.len(), 1400, "100 bytes must inflate to one 1400 frame");
}

#[tokio::test]
async fn wrong_key_fails() {
    let (a, b) = duplex(65536);
    let mut up = wrap(
        &*pad_layer(KEY, r#"{ size = "1400" }"#, Side::Outbound),
        Box::pin(a) as BoxedStream,
    )
    .await;
    let mut dn = wrap(
        &*pad_layer(OTHER_KEY, r#"{ size = "1400" }"#, Side::Inbound),
        Box::pin(b) as BoxedStream,
    )
    .await;

    up.write_all(b"secret").await.unwrap();
    up.shutdown().await.unwrap();

    let mut out = Vec::new();
    let r = dn.read_to_end(&mut out).await;
    assert!(r.is_err(), "wrong key must fail tag check");
}
