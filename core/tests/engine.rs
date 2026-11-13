use anyhow::Result;
use cray_core::*;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

static OUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static INIT_LOG: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn init_log(tag: &str) -> String {
    INIT_LOG
        .lock()
        .unwrap()
        .get(tag)
        .cloned()
        .unwrap_or_default()
}

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

struct Wrap;
impl Layer for Wrap {
    fn ports(&self) -> Ports {
        Ports::WRAP
    }
    fn call<'a>(&'a self, i: Input, _cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move { Ok(Output::Stream(i.stream()?)) })
    }
}

struct Dst;
impl Layer for Dst {
    fn ports(&self) -> Ports {
        Ports::SOURCE
    }
    fn requires(&self) -> Vec<Key> {
        vec![key::<ConnectRequest>()]
    }
    fn provides(&self) -> Vec<Key> {
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

struct Term;
impl Layer for Term {
    fn ports(&self) -> Ports {
        Ports::TERMINAL
    }
    fn call<'a>(&'a self, i: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let mut s = i.stream()?;
            let mut buf = Vec::new();
            s.read_to_end(&mut buf).await?;

            let dialer = cx.get::<Dialer>().expect("dialer in cx").clone();
            let mut out = dialer
                .dial_to(
                    cx,
                    ConnectRequest {
                        addr: "example.com".into(),
                        port: 443,
                    },
                )
                .await?;
            out.read_to_end(&mut buf).await?;

            OUT.lock().unwrap().extend_from_slice(&buf);
            Ok(Output::Done)
        })
    }
}

fn b<L: Layer>(l: L) -> Result<Box<dyn Layer>> {
    Ok(Box::new(l))
}

// --- init layers -----------------------------------------------------------

/// Payload the init layer leaves in the cx for downstream layers: the tag of
/// the test config, used both as the log key and as the runtime value.
struct Stamp(String);

/// INIT layer: takes nothing, gives nothing, appends "init|" to its tag's log
/// cell, puts a Stamp into the cx and passes the input through untouched.
struct Init(String);
impl Layer for Init {
    fn ports(&self) -> Ports {
        Ports::INIT
    }
    fn provides(&self) -> Vec<Key> {
        vec![key::<Stamp>()]
    }
    fn call<'a>(&'a self, i: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            cx.put(Stamp(self.0.clone()));
            INIT_LOG
                .lock()
                .unwrap()
                .entry(self.0.clone())
                .or_default()
                .push_str("init|");
            Ok(Output::Skip(i))
        })
    }
}

/// Wrap layer that requires the Stamp the init layer provides, marking its log
/// cell, proving both build-time ordering and runtime cx visibility.
struct Reader;
impl Layer for Reader {
    fn ports(&self) -> Ports {
        Ports::WRAP
    }
    fn requires(&self) -> Vec<Key> {
        vec![key::<Stamp>()]
    }
    fn call<'a>(&'a self, i: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let tag = cx.get::<Stamp>().expect("stamp in cx").0.clone();
            INIT_LOG
                .lock()
                .unwrap()
                .entry(tag)
                .or_default()
                .push_str("seen|");
            Ok(Output::Stream(i.stream()?))
        })
    }
}

/// Dial-side source: needs no cx values, just hands out a stream.
struct FakeDial;
impl Layer for FakeDial {
    fn ports(&self) -> Ports {
        Ports::SOURCE
    }
    fn call<'a>(&'a self, _i: Input, _cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let (a, mut b) = duplex(64);
            b.write_all(b"dialed").await?;
            b.shutdown().await?;
            Ok(Output::Stream(Box::pin(a)))
        })
    }
}

/// Terminal that pipes the stream into its tag's INIT_LOG cell instead of OUT,
/// so init tests never interfere with `runs_chain_and_dials` polling OUT.
struct InitTerm;
impl Layer for InitTerm {
    fn ports(&self) -> Ports {
        Ports::TERMINAL
    }
    fn call<'a>(&'a self, i: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let tag = cx.get::<Stamp>().expect("stamp in cx").0.clone();
            let mut s = i.stream()?;
            let mut buf = Vec::new();
            s.read_to_end(&mut buf).await?;
            INIT_LOG
                .lock()
                .unwrap()
                .entry(tag)
                .or_default()
                .push_str(&String::from_utf8_lossy(&buf));
            Ok(Output::Done)
        })
    }
}

fn reg() -> Registry {
    let mut r = Registry::new();
    r.add("src", |_, _| b(Src));
    r.add("wrap", |_, _| b(Wrap));
    r.add("dst", |_, _| b(Dst));
    r.add("term", |_, _| b(Term));
    r.add("init", |p, _| {
        b(Init(
            p.get("tag")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_owned(),
        ))
    });
    r.add("reader", |_, _| b(Reader));
    r.add("fakedial", |_, _| b(FakeDial));
    r.add("initterm", |_, _| b(InitTerm));
    r
}

async fn wait_init(tag: &str, want: &str) {
    for _ in 0..500 {
        if init_log(tag) == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "timed out waiting for {tag:?} == {want:?}, got {:?}",
        init_log(tag)
    );
}

#[tokio::test]
async fn init_layers_run_first_and_pass_input_through() {
    let toml = r#"
[[inbound.layers]]
type = "init"
tag = "a"
[[inbound.layers]]
type = "init"
tag = "a2"
[[inbound.layers]]
type = "src"
[[inbound.layers]]
type = "initterm"
[[outbound.layers]]
type = "fakedial"
"#;
    let engine = Engine::build(&AppConfig::parse(toml).unwrap(), &reg())
        .map_err(|e| e.to_string())
        .unwrap();
    engine.run().await.unwrap();
    // both inits ran (in config order) before anything else touched the flow;
    // the terminal sees the stream because Skip passed it through both inits.
    // only the last init's stamp survives in the cx, so the terminal logs a2.
    wait_init("a", "init|").await;
    wait_init("a2", "init|hello").await;
}

#[tokio::test]
async fn init_layer_mid_chain_is_transparent() {
    let toml = r#"
[[inbound.layers]]
type = "src"
[[inbound.layers]]
type = "init"
tag = "b"
[[inbound.layers]]
type = "initterm"
[[outbound.layers]]
type = "fakedial"
"#;
    let engine = Engine::build(&AppConfig::parse(toml).unwrap(), &reg())
        .map_err(|e| e.to_string())
        .unwrap();
    engine.run().await.unwrap();
    // init received the stream mid-chain and Skip'd it intact to the terminal
    wait_init("b", "init|hello").await;
}

#[tokio::test]
async fn init_provides_are_visible_downstream() {
    let toml = r#"
[[inbound.layers]]
type = "init"
tag = "c"
[[inbound.layers]]
type = "src"
[[inbound.layers]]
type = "reader"
[[inbound.layers]]
type = "initterm"
[[outbound.layers]]
type = "fakedial"
"#;
    let engine = Engine::build(&AppConfig::parse(toml).unwrap(), &reg())
        .map_err(|e| e.to_string())
        .unwrap();
    engine.run().await.unwrap();
    // init logged first, then the reader saw the stamp the init put in the cx
    wait_init("c", "init|seen|hello").await;
}

#[test]
fn init_provides_are_enforced() {
    // no init layer before the reader: the build must reject it
    let toml = r#"
[[inbound.layers]]
type = "src"
[[inbound.layers]]
type = "reader"
[[inbound.layers]]
type = "initterm"
[[outbound.layers]]
type = "fakedial"
"#;
    match Engine::build(&AppConfig::parse(toml).unwrap(), &reg()) {
        Ok(_) => panic!("expected a build error"),
        Err(e) => {
            let e = format!("{e:#}");
            assert!(e.contains("requires") && e.contains("Stamp"), "{e}");
        }
    }
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
    let engine = Engine::build(&AppConfig::parse(GOOD).unwrap(), &reg())
        .map_err(|e| e.to_string())
        .unwrap();
    engine.run().await.unwrap();

    for _ in 0..100 {
        if OUT.lock().unwrap().as_slice() == b"helloexample.com" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "unexpected output: {:?}",
        String::from_utf8_lossy(&OUT.lock().unwrap())
    );
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
    fn ports(&self) -> Ports {
        Ports::WRAP
    }
    fn requires(&self) -> Vec<Key> {
        vec![key::<ConnectRequest>()]
    }
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
            assert!(
                e.contains("requires") && e.contains("ConnectRequest"),
                "{e}"
            );
        }
    }
}
