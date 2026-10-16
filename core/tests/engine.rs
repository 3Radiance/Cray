use anyhow::Result;
use cray_core::*;
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

static OUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());

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

struct Wrap;
impl Layer for Wrap {
    fn ports(&self) -> Ports { Ports::WRAP }
    fn call<'a>(&'a self, i: Input, _cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move { Ok(Output::Stream(i.stream()?)) })
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
            b.write_all(addr.as_bytes()).await?;
            b.shutdown().await?;
            Ok(Output::Stream(Box::pin(a)))
        })
    }
}

struct Term;
impl Layer for Term {
    fn ports(&self) -> Ports { Ports::TERMINAL }
    fn call<'a>(&'a self, i: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let mut s = i.stream()?;
            let mut buf = Vec::new();
            s.read_to_end(&mut buf).await?;

            let dialer = cx.get::<Dialer>().expect("dialer in cx").clone();
            let mut out = dialer.dial_to(cx, ConnectRequest { addr: "example.com".into(), port: 443 }).await?;
            out.read_to_end(&mut buf).await?;

            OUT.lock().unwrap().extend_from_slice(&buf);
            Ok(Output::Done)
        })
    }
}

fn b<L: Layer>(l: L) -> Result<Box<dyn Layer>> { Ok(Box::new(l)) }

fn reg() -> Registry {
    let mut r = Registry::new();
    r.add("src", |_, _| b(Src));
    r.add("wrap", |_, _| b(Wrap));
    r.add("dst", |_, _| b(Dst));
    r.add("term", |_, _| b(Term));
    r
}

const GOOD: &str = r#"
[[inbound.layers]]
type = "src"
[[inbound.layers]]
type = "wrap"
[[inbound.layers]]
type = "term"
[[outbound.layers]]
type = "dst"
"#;

#[tokio::test]
async fn runs_chain_and_dials() {
    let engine = Engine::build(&AppConfig::parse(GOOD).unwrap(), &reg()).map_err(|e| e.to_string()).unwrap();
    engine.run().await.unwrap();

    for _ in 0..100 {
        if OUT.lock().unwrap().as_slice() == b"helloexample.com" { return; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("unexpected output: {:?}", String::from_utf8_lossy(&OUT.lock().unwrap()));
}

fn build_err(toml: &str) -> String {
    match Engine::build(&AppConfig::parse(toml).unwrap(), &reg()) {
        Ok(_) => panic!("expected a build error"),
        Err(e) => format!("{e:#}"),
    }
}

#[test]
fn unknown_layer_is_rejected() {
    let e = build_err(&GOOD.replace("\"wrap\"", "\"nope\""));
    assert!(e.contains("unknown layer 'nope'"), "{e}");
}

#[test]
fn wrong_order_is_rejected() {
    let toml = r#"
[[inbound.layers]]
type = "wrap"
[[inbound.layers]]
type = "term"
[[outbound.layers]]
type = "dst"
"#;
    let e = build_err(toml);
    assert!(e.contains("must be a source"), "{e}");
}

#[test]
fn missing_terminal_is_rejected() {
    let e = build_err(&GOOD.replace("type = \"term\"", "type = \"wrap\""));
    assert!(e.contains("no terminal"), "{e}");
}

#[test]
fn dial_chain_must_give_single_stream() {
    let toml = r#"
[[inbound.layers]]
type = "src"
[[inbound.layers]]
type = "term"
[[outbound.layers]]
type = "src"
"#;
    let e = build_err(toml);
    assert!(e.contains("must give a single stream"), "{e}");
}

struct Needy;
impl Layer for Needy {
    fn ports(&self) -> Ports { Ports::WRAP }
    fn requires(&self) -> Vec<Key> { vec![key::<ConnectRequest>()] }
    fn call<'a>(&'a self, i: Input, _cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move { Ok(Output::Stream(i.stream()?)) })
    }
}

#[test]
fn missing_requires_is_rejected() {
    let mut r = reg();
    r.add("needy", |_, _| b(Needy));
    let toml = GOOD.replace("type = \"wrap\"", "type = \"needy\"");
    match Engine::build(&AppConfig::parse(&toml).unwrap(), &r) {
        Ok(_) => panic!("expected a build error"),
        Err(e) => {
            let e = format!("{e:#}");
            assert!(e.contains("requires") && e.contains("ConnectRequest"), "{e}");
        }
    }
}
