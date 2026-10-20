use crate::api::{BoxedStream, ConnectRequest, Input, Output};
use crate::chain::Chain;
use crate::cx::Cx;
use crate::engine::with_timeout;
use anyhow::{Result, bail};
use std::sync::Arc;

#[derive(Clone)]
pub struct Dialer {
    pub(crate) chain: Arc<Chain>,
}

impl Dialer {
    pub async fn dial_to(&self, cx: &Cx, target: ConnectRequest) -> Result<BoxedStream> {
        let mut cx = cx.clone();
        cx.put(target);

        let mut input = Input::None;
        for e in &self.chain.entries {
            match with_timeout(&e.name, e.timeout, e.layer.call(input, &mut cx)).await? {
                Output::Stream(s) => input = Input::Stream(s),
                Output::Consumed => {
                    bail!("layer '{}' consumed the stream in the dial chain", e.name)
                }
                _ => bail!("dial chain: layer '{}' must give a single stream", e.name),
            }
        }
        input.stream()
    }
}
