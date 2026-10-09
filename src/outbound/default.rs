use crate::{ConnectRequest, outbound::Outbound};
use anyhow::Result;
use tokio::net::TcpStream;

pub struct Default;
impl Outbound for Default {
    type Stream = TcpStream;

    async fn connect(&self, t: &ConnectRequest) -> Result<Self::Stream> {
        let s = TcpStream::connect((t.addr.as_str(), t.port)).await?;
        s.set_nodelay(true)?;
        Ok(s)
    }
}
