use crate::outbound::Outbound;
use anyhow::Result;
use std::path::PathBuf;
use tokio::net::UnixStream;

pub struct Unix;
impl Outbound for Unix {
    type Stream = UnixStream;
    type Context = PathBuf;

    async fn connect(&self, t: &Self::Context) -> Result<Self::Stream> {
        let s = UnixStream::connect(t).await?;
        Ok(s)
    }
}
