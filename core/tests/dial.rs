use anyhow::{Result, bail};
use cray_core::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

static OUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());

fn out() -> String {
    String::from_utf8_lossy(&OUT.lock().unwrap()).into_owned()
}

struct OneShot(Option<BoxedStream>);
impl Incoming for OneShot {
    fn accept(&mut self) -> BoxFuture<'_, Option<BoxedStream>> {
        Box::pin(async move { self.0.take() })
    }
}

struct Src;
impl Layer for Src {
    fn ports(&self) -> Ports { Ports::LISTENER }
    fn call<'a>(&'a self, _i: Input, _cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let (a, mut b) = duplex(64);
            b.write_all(b"hello").await?;
            b.shutdown().await?;
            Ok(Output::Streams(Box::new(OneShot(Some(Box::pin(a))))))
        })
    }
}

struct Dst;
impl Layer for Dst {
    fn ports(&self) -> Ports { Ports::SOURCE }
    fn requires(&self) -> Vec<Key> { vec![key::<ConnectRequest>()] }
    fn call<'a>(&'a self, _i: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let addr = cx.get::<ConnectRequest>().expect("target in cx").addr.clone();
            let (a, mut b) = duplex(64);
            // the transport answers with the dialed address once a request
            // arrives, so stages can both write to and read from the dial
            tokio::spawn(async move {
                let mut req = Vec::new();
                let _ = b.read_to_end(&mut req).await;
                let _ = b.write_all(addr.as_bytes()).await;
                let _ = b.shutdown().await;
            });
            Ok(Output::Stream(Box::pin(a)))
        })
    }
}

/// Terminal: reads what came from above, dials through the cx dialer, pokes
/// the protocol side and pipes the reply back into OUT as "{tag}|<input>|<reply>".
struct Term(&'static str);
impl Layer for Term {
    fn ports(&self) -> Ports { Ports::TERMINAL }
    fn call<'a>(&'a self, i: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let mut s = i.stream()?;
            let mut buf = Vec::new();
            s.read_to_end(&mut buf).await?;

            let dialer = cx.get::<Dialer>().expect("dialer in cx").clone();
            let mut out = dialer
                .dial_to(cx, ConnectRequest { addr: "example.com".into(), port: 443 })
                .await?;
            out.write_all(b"ping").await?;
            out.shutdown().await?;
            let mut resp = Vec::new();
            out.read_to_end(&mut resp).await?;

            let mut got = format!("{}|", self.0).into_bytes();
            got.extend_from_slice(&buf);
            got.extend_from_slice(b"|");
            got.extend_from_slice(&resp);
            OUT.lock().unwrap().extend_from_slice(&got);
            Ok(Output::Done)
        })
    }
}

/// Protocol stage on the dial side: reads the bytes fed to it from the dialer,
/// dials the layer below and pipes the reply back with its tag appended.
struct Prox(&'static str);
impl Layer for Prox {
    fn ports(&self) -> Ports { Ports::TERMINAL }
    fn call<'a>(&'a self, i: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let mut up = i.stream()?;
            let mut req = Vec::new();
            up.read_to_end(&mut req).await?;

            let dialer = cx.get::<Dialer>().expect("dialer in cx").clone();
            let mut down = dialer
                .dial_to(cx, ConnectRequest { addr: "example.com".into(), port: 443 })
                .await?;
            down.write_all(&req).await?;
            down.shutdown().await?;
            let mut resp = Vec::new();
            down.read_to_end(&mut resp).await?;

            up.write_all(&resp).await?;
            up.write_all(self.0.as_bytes()).await?;
            Ok(Output::Done)
        })
    }
}

/// Head stage: reads the stream coming from above and forwards it through the
/// one-shot dialer to the next stage / terminal, prefixed with its tag.
struct Relay(&'static str);
impl Layer for Relay {
    fn ports(&self) -> Ports { Ports::TERMINAL }
    fn call<'a>(&'a self, i: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let mut up = i.stream()?;
            let mut buf = Vec::new();
            up.read_to_end(&mut buf).await?;

            let dialer = cx.get::<Dialer>().expect("dialer in cx").clone();
            let mut d = dialer
                .dial_to(cx, ConnectRequest { addr: "stage-pipe".into(), port: 0 })
                .await?;
            let msg = format!("{}{}", self.0, String::from_utf8_lossy(&buf)).into_bytes();
            d.write_all(&msg).await?;
            Ok(Output::Done)
        })
    }
}

struct NoInput;
impl Layer for NoInput {
    fn ports(&self) -> Ports { Ports { takes: Port::Nothing, gives: Port::Done } }
    fn call<'a>(&'a self, _i: Input, _cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move { bail!("never called") })
    }
}

struct Secret;

struct NeedyProto;
impl Layer for NeedyProto {
    fn ports(&self) -> Ports { Ports::TERMINAL }
    fn requires(&self) -> Vec<Key> { vec![key::<Secret>()] }
    fn call<'a>(&'a self, i: Input, _cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move { Ok(Output::Stream(i.stream()?)) })
    }
}

fn b<L: Layer>(l: L) -> Result<Box<dyn Layer>> { Ok(Box::new(l)) }

fn reg() -> Registry {
    let mut r = Registry::new();
    r.add("src", |_, _| b(Src));
    r.add("dst", |_, _| b(Dst));
    r.add("term", |_, _| b(Term("term")));
    r.add("p1", |_, _| b(Prox("[p1]")));
    r.add("p2", |_, _| b(Prox("[p2]")));
    r.add("p3", |_, _| b(Prox("[p3]")));
    r.add("p4", |_, _| b(Prox("[p4]")));
    r.add("h1", |_, _| b(Relay("h1.")));
    r.add("h2", |_, _| b(Relay("h2.")));
    r.add("noinput", |_, _| b(NoInput));
    r.add("needyproto", |_, _| b(NeedyProto));
    r
}

fn build_err(toml: &str) -> String {
    match Engine::build(&AppConfig::parse(toml).unwrap(), &reg()) {
        Ok(_) => panic!("expected a build error"),
        Err(e) => format!("{e:#}"),
    }
}

async fn run(toml: &str) {
    let engine = Engine::build(&AppConfig::parse(toml).unwrap(), &reg())
        .map_err(|e| e.to_string())
        .unwrap();
    engine.run().await.unwrap();
}

/// Connections are driven in a spawned task, so run() may return before the
/// terminal finished writing out its captured bytes.
async fn wait_for(needle: &str) {
    for _ in 0..500 {
        if out().contains(needle) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {needle:?}, got {:?}", out());
}

// --- driving -------------------------------------------------------------

#[tokio::test]
async fn inbound_terminal_dials_through_outbound_stage() {
    let toml = r#"
[[inbound.layers]]
type = "src"
[[inbound.layers]]
type = "term"
[[outbound.layers]]
type = "dst"
[[outbound.layers]]
type = "p1"
"#;
    run(toml).await;
    wait_for("term|hello|example.com[p1]").await;
}

#[tokio::test]
async fn first_outbound_proto_is_terminal_the_rest_are_dial_stages() {
    let toml = r#"
[[inbound.layers]]
type = "src"
[[outbound.layers]]
type = "dst"
[[outbound.layers]]
type = "term"
[[outbound.layers]]
type = "p4"
"#;
    run(toml).await;
    wait_for("term|hello|example.com[p4]").await;
}

#[tokio::test]
async fn dial_stages_nest_from_last_listed_down_to_the_transport() {
    let toml = r#"
[[inbound.layers]]
type = "src"
[[inbound.layers]]
type = "term"
[[outbound.layers]]
type = "dst"
[[outbound.layers]]
type = "p2"
[[outbound.layers]]
type = "p3"
"#;
    run(toml).await;
    wait_for("term|hello|example.com[p3][p2]").await;
}

#[tokio::test]
async fn head_stages_relays_upstream_to_the_last_proto() {
    let toml = r#"
[[inbound.layers]]
type = "src"
[[inbound.layers]]
type = "h1"
[[inbound.layers]]
type = "h2"
[[inbound.layers]]
type = "term"
[[outbound.layers]]
type = "dst"
"#;
    run(toml).await;
    // h1 sees the raw upstream, its once-pipe feeds h2, h2's feeds the terminal
    // (the last proto on the inbound side), which then dials outbound as usual.
    wait_for("term|h2.h1.hello|example.com").await;
}

// --- build validation ----------------------------------------------------

#[test]
fn inbound_protos_must_be_contiguous_at_the_end() {
    let toml = r#"
[[inbound.layers]]
type = "src"
[[inbound.layers]]
type = "term"
[[inbound.layers]]
type = "dst"
[[outbound.layers]]
type = "dst"
"#;
    let e = build_err(toml);
    assert!(
        e.contains("inbound: protocol layers must be contiguous at the end of the list"),
        "{e}"
    );
}

#[test]
fn outbound_protos_must_be_contiguous_at_the_end() {
    let toml = r#"
[[inbound.layers]]
type = "src"
[[inbound.layers]]
type = "term"
[[outbound.layers]]
type = "dst"
[[outbound.layers]]
type = "p1"
[[outbound.layers]]
type = "dst"
"#;
    let e = build_err(toml);
    assert!(
        e.contains("outbound: protocol layers must be contiguous at the end of the list"),
        "{e}"
    );
}

#[test]
fn stage_must_take_a_stream() {
    let toml = r#"
[[inbound.layers]]
type = "src"
[[inbound.layers]]
type = "noinput"
[[inbound.layers]]
type = "term"
[[outbound.layers]]
type = "dst"
"#;
    let e = build_err(toml);
    assert!(e.contains("stage 'noinput' must take a stream"), "{e}");
}

#[test]
fn stage_requires_are_checked() {
    let toml = r#"
[[inbound.layers]]
type = "src"
[[inbound.layers]]
type = "needyproto"
[[inbound.layers]]
type = "term"
[[outbound.layers]]
type = "dst"
"#;
    let e = build_err(toml);
    assert!(e.contains("stage"), "{e}");
    assert!(e.contains("requires"), "{e}");
    assert!(e.contains("Secret"), "{e}");
}

// --- dialer primitives ---------------------------------------------------

#[tokio::test]
async fn once_dialer_hands_out_its_stream_exactly_once() {
    let (a, mut b) = duplex(64);
    let dial = Dialer::Once(Arc::new(Mutex::new(Some(Box::pin(a)))));
    let cx = Cx::default();

    let mut s = dial
        .dial_to(&cx, ConnectRequest { addr: "x".into(), port: 0 })
        .await
        .expect("first dial");
    s.write_all(b"ping").await.unwrap();
    s.shutdown().await.unwrap();
    let mut buf = Vec::new();
    b.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"ping");

    let err = match dial
        .dial_to(&cx, ConnectRequest { addr: "x".into(), port: 0 })
        .await
    {
        Ok(_) => panic!("second dial must fail"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("already taken"), "{err:#}");
}
