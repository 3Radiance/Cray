//! Проверка адаптеров kit на игрушечных Transport и Protocol, через настоящий Engine.

use anyhow::Result;
use cray_core::*;
use cray_layers::kit::{DialCtx, Once, Protocol, ProtocolLayer, Transport, TransportLayer, layer};
use std::{future::Future, sync::Mutex, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, duplex};

// ── игрушечный Transport: пропускает поток как есть ──
struct ToyT;
impl Transport for ToyT {
    type Context = ();
    type ServerContext = ();
    type Stream = BoxedStream;
    type Incoming = Once<BoxedStream>;

    async fn client<W>(&mut self, wire: W, _ctx: &mut ()) -> Result<BoxedStream>
    where
        W: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
    {
        Ok(Box::pin(wire))
    }

    async fn server<P>(&mut self, plain: P, _ctx: &mut ()) -> Result<Once<BoxedStream>>
    where
        P: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
    {
        Ok(Once::new(Box::pin(plain)))
    }
}

#[tokio::test]
async fn transport_server_gives_streams() -> Result<()> {
    let l = TransportLayer::new(Side::Inbound, || ToyT, || (), || ())
        .with_default_timeout(Duration::from_secs(3));
    assert_eq!(l.ports().gives, Port::Streams);
    assert_eq!(l.default_timeout(), Some(Duration::from_secs(3)));

    let (a, mut b) = duplex(64);
    let Output::Streams(mut inc) = l
        .call(Input::Stream(Box::pin(a)), &mut Cx::default())
        .await?
    else {
        panic!("server transport must give Streams")
    };
    b.write_all(b"ping").await?;
    let mut s = inc.accept().await.expect("one stream");
    let mut buf = [0u8; 4];
    s.read_exact(&mut buf).await?;
    assert_eq!(&buf, b"ping");
    assert!(inc.accept().await.is_none());
    Ok(())
}

#[tokio::test]
async fn transport_client_gives_stream() -> Result<()> {
    let l = TransportLayer::new(Side::Outbound, || ToyT, || (), || ());
    assert_eq!(l.ports().gives, Port::Stream);

    let (a, mut b) = duplex(64);
    let Output::Stream(mut s) = l
        .call(Input::Stream(Box::pin(a)), &mut Cx::default())
        .await?
    else {
        panic!("client transport must give Stream")
    };
    s.write_all(b"pong").await?;
    let mut buf = [0u8; 4];
    b.read_exact(&mut buf).await?;
    assert_eq!(&buf, b"pong");
    Ok(())
}

// ── игрушечный Protocol ──
static OUT_S: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static OUT_C: Mutex<Vec<u8>> = Mutex::new(Vec::new());

struct ToyP;
impl Protocol for ToyP {
    type Context = DialCtx;
    type Outcome = ();

    async fn client<C, W>(
        &mut self,
        mut plain: C,
        mut wire: W,
        _t: &ConnectRequest,
        _ctx: &mut DialCtx,
    ) -> Result<()>
    where
        C: AsyncRead + AsyncWrite + Unpin + Send + Sync,
        W: AsyncRead + AsyncWrite + Unpin + Send + Sync,
    {
        let mut buf = Vec::new();
        plain.read_to_end(&mut buf).await?;
        wire.read_to_end(&mut buf).await?;
        OUT_C.lock().unwrap().extend_from_slice(&buf);
        Ok(())
    }

    async fn server<W>(&mut self, mut wire: W, ctx: &mut DialCtx) -> Result<()>
    where
        W: AsyncRead + AsyncWrite + Unpin + Send + Sync,
    {
        let mut buf = Vec::new();
        wire.read_to_end(&mut buf).await?;
        let mut out = ctx
            .connect(ConnectRequest {
                addr: "example.com".into(),
                port: 443,
            })
            .await?;
        out.read_to_end(&mut buf).await?;
        OUT_S.lock().unwrap().extend_from_slice(&buf);
        Ok(())
    }
}

// ── фейковые слои вокруг ──
struct OneShot(Option<BoxedStream>);
impl Incoming for OneShot {
    fn accept(&mut self) -> BoxFuture<'_, Option<BoxedStream>> {
        Box::pin(async move { self.0.take() })
    }
}

struct Src;
impl Layer for Src {
    fn ports(&self) -> Ports {
        Ports::LISTENER
    }
    fn call<'a>(&'a self, _i: Input, _cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let (a, mut b) = duplex(64);
            b.write_all(b"hello").await?;
            b.shutdown().await?;
            Ok(Output::Streams(Box::new(OneShot(Some(Box::pin(a))))))
        })
    }
}

// req: кладёт ConnectRequest в Cx (роль http_connect)
struct Req;
impl Layer for Req {
    fn ports(&self) -> Ports {
        Ports::WRAP
    }
    fn provides(&self) -> Vec<Key> {
        vec![key::<ConnectRequest>()]
    }
    fn call<'a>(&'a self, i: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            cx.put(ConnectRequest {
                addr: "target.test".into(),
                port: 80,
            });
            Ok(Output::Stream(i.stream()?))
        })
    }
}

// dst: outbound-источник, пишет адрес цели из Cx
struct Dst;
impl Layer for Dst {
    fn ports(&self) -> Ports {
        Ports::SOURCE
    }
    fn requires(&self) -> Vec<Key> {
        vec![key::<ConnectRequest>()]
    }
    fn call<'a>(&'a self, _i: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let addr = cx
                .get::<ConnectRequest>()
                .expect("target in cx")
                .addr
                .clone();
            let (a, mut b) = duplex(64);
            b.write_all(addr.as_bytes()).await?;
            b.shutdown().await?;
            Ok(Output::Stream(Box::pin(a)))
        })
    }
}

fn reg() -> Registry {
    let mut r = Registry::new();
    r.add("src", |_, _| Ok(layer(Src)));
    r.add("req", |_, _| Ok(layer(Req)));
    r.add("dst", |_, _| Ok(layer(Dst)));
    r.add("proto", |_, bcx| {
        Ok(layer(ProtocolLayer::new(bcx.side, || ToyP, |d| d)))
    });
    r
}

async fn wait_for(buf: &Mutex<Vec<u8>>, want: &[u8]) {
    for _ in 0..100 {
        if buf.lock().unwrap().as_slice() == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "unexpected output: {:?}",
        String::from_utf8_lossy(&buf.lock().unwrap())
    );
}

#[tokio::test]
async fn protocol_server_dials_through_dialctx() {
    let cfg = AppConfig::parse(
        r#"
[[inbound.layers]]
type = "src"
[[inbound.layers]]
type = "proto"
[[outbound.layers]]
type = "dst"
"#,
    )
    .unwrap();
    let engine = Engine::build(&cfg, &reg())
        .map_err(|e| e.to_string())
        .unwrap();
    engine.run().await.unwrap();
    wait_for(&OUT_S, b"helloexample.com").await;
}

#[tokio::test]
async fn protocol_client_gets_wire_from_dialer() {
    let cfg = AppConfig::parse(
        r#"
[[inbound.layers]]
type = "src"
[[inbound.layers]]
type = "req"
[[outbound.layers]]
type = "dst"
[[outbound.layers]]
type = "proto"
"#,
    )
    .unwrap();
    let engine = Engine::build(&cfg, &reg())
        .map_err(|e| e.to_string())
        .unwrap();
    engine.run().await.unwrap();
    wait_for(&OUT_C, b"hellotarget.test").await;
}

#[test]
fn client_protocol_without_connect_request_is_rejected() {
    // client без слоя, который кладёт ConnectRequest: должно отвалиться при сборке, а не на первом соединении
    let cfg = AppConfig::parse(
        r#"
[[inbound.layers]]
type = "src"
[[outbound.layers]]
type = "dst"
[[outbound.layers]]
type = "proto"
"#,
    )
    .unwrap();
    match Engine::build(&cfg, &reg()) {
        Ok(_) => panic!("expected a build error"),
        Err(e) => {
            let e = format!("{e:#}");
            assert!(
                e.contains("requires") && e.contains("ConnectRequest"),
                "{e}"
            );
        }
    }
}

#[allow(dead_code)]
fn _assert_send<F: Future + Send>(_: F) {}
