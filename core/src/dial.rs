use crate::api::{BoxedStream, ConnectRequest, Input, Output};
use crate::chain::{Chain, Entry};
use crate::cx::Cx;
use crate::engine::with_timeout;
use crate::log_tag;
use anyhow::{Result, anyhow, bail};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub enum Dialer {
    Chain(Arc<Chain>),
    Stage {
        below: Arc<Dialer>,
        stage: Arc<Entry>,
    },
    Once(Arc<Mutex<Option<BoxedStream>>>),
}
impl Dialer {
    pub async fn dial_to(&self, cx: &Cx, target: ConnectRequest) -> Result<BoxedStream> {
        match self {
            Dialer::Chain(chain) => {
                let mut cx = cx.clone();
                cx.put(target);
                let mut input = Input::None;
                for e in &chain.entries {
                    match with_timeout(&e.name, e.timeout, e.layer.call(input, &mut cx)).await? {
                        Output::Stream(s) => input = Input::Stream(s),
                        _ => bail!("dial chain: layer '{}' must give a single stream", e.name),
                    }
                }
                input.stream()
            }
            Dialer::Stage { below, stage } => {
                let (a, b) = tokio::io::duplex(32 * 1024);
                let mut c = cx.clone();
                c.put(target);
                c.put((**below).clone());
                let st = stage.clone();
                tokio::spawn(async move {
                    if let Err(e) = st.layer.call(Input::Stream(Box::pin(b)), &mut c).await {
                        log_tag!(error, "CHAIN", "stage '{}': {:#}", st.name, e);
                    }
                });
                Ok(Box::pin(a))
            }
            Dialer::Once(slot) => slot
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| anyhow!("Once dialer: stream already taken")),
        }
    }
}
