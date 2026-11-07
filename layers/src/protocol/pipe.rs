use anyhow::{Result, anyhow};
use cray_core::{
    BoxFuture, BuildCx, ConnectRequest, Cx, Dialer, Input, Key, Layer, Output, Ports, key,
};

pub fn build(_params: &toml::Table, _bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    Ok(Box::new(Pipe))
}

pub struct Pipe;

impl Layer for Pipe {
    fn ports(&self) -> Ports {
        Ports::TERMINAL
    }
    fn requires(&self) -> Vec<Key> {
        vec![key::<ConnectRequest>(), key::<Dialer>()]
    }

    fn call<'a>(&'a self, input: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let mut client = input.stream()?;
            let dialer: &Dialer = cx
                .get::<Dialer>()
                .ok_or_else(|| anyhow!("Dialer dont have"))?;
            let con = cx
                .get::<ConnectRequest>()
                .ok_or_else(|| anyhow!("ConnectRequest dont have"))?;
            let mut stream = dialer.dial_to(cx, con.clone()).await?;

            tokio::io::copy_bidirectional(&mut client, &mut stream).await?;

            Ok(Output::Done)
        })
    }
}
