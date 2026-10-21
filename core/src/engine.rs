use crate::api::{BoxFuture, BuildCx, ConnectRequest, Input, Output, Port, Side};
use crate::chain::{Chain, Entry, build_entries, check_links, check_requires, feeds_stream};
use crate::config::AppConfig;
use crate::cx::{Cx, Key, key};
use crate::dial::Dialer;
use crate::log_tag;
use crate::registry::Registry;
use anyhow::{Result, anyhow, bail, ensure};
use std::{
    collections::HashSet,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};

pub struct Engine {
    head: Arc<Chain>,
    head_stages: Vec<Arc<Entry>>,
    terminal: Entry,
    dialer: Dialer,
}

pub(crate) async fn with_timeout<T>(
    what: &str,
    dur: Option<Duration>,
    fut: impl Future<Output = Result<T>>,
) -> Result<T> {
    match dur {
        None => fut.await,
        Some(d) => tokio::time::timeout(d, fut)
            .await
            .map_err(|_| anyhow!("{what}: timed out after {d:?}"))?,
    }
}

fn is_proto(e: &Entry) -> bool {
    e.layer.ports().gives == Port::Done
}

fn split_protocols(entries: &mut Vec<Entry>, label: &str) -> Result<Vec<Entry>> {
    let at = entries
        .iter()
        .position(|e| is_proto(e))
        .unwrap_or(entries.len());
    let protos = entries.split_off(at);
    ensure!(
        protos.iter().all(|e| is_proto(e)),
        "{label}: protocol layers must be contiguous at the end of the list"
    );
    Ok(protos)
}

impl Engine {
    pub fn build(cfg: &AppConfig, reg: &Registry) -> Result<Engine> {
        let bcx = |side| BuildCx { side };
        let mut head = build_entries(&cfg.inbound.layers, &bcx(Side::Inbound), reg, cfg)?;
        let mut dial = build_entries(&cfg.outbound.layers, &bcx(Side::Outbound), reg, cfg)?;

        let mut head_protos = split_protocols(&mut head, "inbound")?;
        let dial_protos = split_protocols(&mut dial, "outbound")?;

        let (terminal, head_stages, dial_stages) = match (head_protos.pop(), dial_protos.is_empty())
        {
            (Some(t), _) => (t, head_protos, dial_protos),
            (None, false) => {
                let mut p = dial_protos;
                let t = p.remove(0);
                (t, Vec::new(), p)
            }
            (None, true) => bail!("no terminal (protocol) layer in inbound or outbound"),
        };
        ensure!(
            terminal.layer.ports().takes == Port::Stream,
            "terminal layer '{}' must take a stream",
            terminal.name
        );

        check_links(&head, "inbound")?;
        check_links(&dial, "outbound")?;
        ensure!(
            feeds_stream(head.last().expect("checked").layer.ports().gives),
            "inbound: last layer '{}' must give streams for the terminal",
            head.last().expect("checked").name
        );
        for e in &dial {
            ensure!(
                e.layer.ports().gives == Port::Stream,
                "outbound: layer '{}' must give a single stream (it is used for dialing)",
                e.name
            );
        }

        let mut provided: HashSet<Key> = HashSet::new();
        check_requires(&head, &mut provided, "inbound")?;
        provided.extend(terminal.layer.provides());

        let mut term_env = provided.clone();
        term_env.insert(key::<Dialer>());
        check_requires(std::slice::from_ref(&terminal), &mut term_env, "terminal")?;

        let mut stage_env = provided.clone();
        stage_env.insert(key::<Dialer>());
        stage_env.insert(key::<ConnectRequest>());
        for s in head_stages.iter().chain(dial_stages.iter()) {
            ensure!(
                s.layer.ports().takes == Port::Stream,
                "stage '{}' must take a stream",
                s.name
            );
            check_requires(std::slice::from_ref(s), &mut stage_env.clone(), "stage")?;
        }

        let mut dial_provided = provided.clone();
        dial_provided.insert(key::<ConnectRequest>());
        check_requires(&dial, &mut dial_provided, "outbound")?;

        let mut dialer = Dialer::Chain(Arc::new(Chain { entries: dial }));
        for p in dial_stages.into_iter().rev() {
            dialer = Dialer::Stage {
                below: Arc::new(dialer),
                stage: Arc::new(p),
            };
        }

        Ok(Engine {
            head: Arc::new(Chain { entries: head }),
            head_stages: head_stages.into_iter().map(Arc::new).collect(),
            terminal,
            dialer,
        })
    }

    pub async fn run(self) -> Result<()> {
        drive(Arc::new(self), 0, Input::None, Cx::default()).await
    }
}

fn drive(eng: Arc<Engine>, i: usize, input: Input, mut cx: Cx) -> BoxFuture<'static, Result<()>> {
    Box::pin(async move {
        let Some(e) = eng.head.entries.get(i) else {
            let mut input = input;
            for st in &eng.head_stages {
                let (a, b) = tokio::io::duplex(32 * 1024);
                let mut c = cx.clone();
                c.put(Dialer::Once(Arc::new(Mutex::new(Some(Box::pin(b))))));
                let st = st.clone();
                let inp = std::mem::replace(&mut input, Input::Stream(Box::pin(a)));
                tokio::spawn(async move {
                    if let Err(e) = st.layer.call(inp, &mut c).await {
                        log_tag!(error, "CHAIN", "stage '{}': {:#}", st.name, e);
                    }
                });
            }
            cx.put(eng.dialer.clone());
            return match eng.terminal.layer.call(input, &mut cx).await? {
                Output::Done => Ok(()),
                _ => bail!("terminal layer '{}' must give Done", eng.terminal.name),
            };
        };

        log_tag!(debug, "CHAIN", "calling layer: {}", e.name);
        let out = with_timeout(&e.name, e.timeout, e.layer.call(input, &mut cx)).await?;

        match out {
            Output::Stream(s) => drive(eng, i + 1, Input::Stream(s), cx).await,
            Output::Streams(mut inc) => {
                while let Some(s) = inc.accept().await {
                    let (eng, cx) = (eng.clone(), cx.clone());
                    tokio::spawn(async move {
                        if let Err(err) = drive(eng, i + 1, Input::Stream(s), cx).await {
                            log_tag!(error, "CHAIN", "connection: {:#}", err);
                        }
                    });
                }
                Ok(())
            }
            Output::Done => bail!(
                "layer '{}' gave Done in the middle of the chain",
                eng.head.entries[i].name
            ),
            Output::Consumed => Ok(()),
        }
    })
}
