
use crate::api::{BoxFuture, BuildCx, Input, Output, Port, Side};
use crate::chain::{Chain, Entry, build_entries, check_links, check_requires, feeds_stream};
use crate::config::AppConfig;
use crate::cx::{Cx, Key, key};
use crate::dial::Dialer;
use crate::log_tag;
use crate::registry::Registry;
use anyhow::{Result, anyhow, bail, ensure};
use std::{collections::HashSet, future::Future, sync::Arc, time::Duration};

pub struct Engine {
    head: Arc<Chain>,
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

fn take_terminal(entries: &mut Vec<Entry>) -> Option<Entry> {
    let is_terminal = entries.last().is_some_and(|e| e.layer.ports().gives == Port::Done);
    if is_terminal { entries.pop() } else { None }
}

impl Engine {
    pub fn build(cfg: &AppConfig, reg: &Registry) -> Result<Engine> {
        let bcx = |side| BuildCx { side };
        let mut head = build_entries(&cfg.inbound.layers, &bcx(Side::Inbound), reg, cfg)?;
        let mut dial = build_entries(&cfg.outbound.layers, &bcx(Side::Outbound), reg, cfg)?;

        let terminal = match (take_terminal(&mut head), take_terminal(&mut dial)) {
            (Some(t), None) | (None, Some(t)) => t,
            (None, None) => bail!("no terminal (protocol) layer: expected it as the last layer of inbound or outbound"),
            (Some(_), Some(_)) => bail!("terminal (protocol) layer found in both inbound and outbound"),
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
        provided.insert(key::<crate::api::ConnectRequest>());
        let mut with_dialer = provided.clone();
        with_dialer.insert(key::<Dialer>());
        check_requires(std::slice::from_ref(&terminal), &mut with_dialer, "terminal")?;
        check_requires(&dial, &mut provided, "outbound")?;

        Ok(Engine {
            head: Arc::new(Chain { entries: head }),
            terminal,
            dialer: Dialer { chain: Arc::new(Chain { entries: dial }) },
        })
    }

    pub async fn run(self) -> Result<()> {
        drive(Arc::new(self), 0, Input::None, Cx::default()).await
    }
}

fn drive(eng: Arc<Engine>, i: usize, input: Input, mut cx: Cx) -> BoxFuture<'static, Result<()>> {
    Box::pin(async move {
        let Some(e) = eng.head.entries.get(i) else {
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
            Output::Done => bail!("layer '{}' gave Done in the middle of the chain", eng.head.entries[i].name),
        }
    })
}
