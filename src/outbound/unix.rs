use crate::{ConnectRequest, outbound::Outbound};
use anyhow::Result;
use tokio::net::UnixStream;

pub struct Unix;
impl Outbound for Unix {
    type Stream = UnixStream;

    async fn connect(&self, t: &ConnectRequest) -> Result<Self::Stream> {
        let s = UnixStream::connect(t.addr.as_str()).await?; // stub
        Ok(s)
    }
}
