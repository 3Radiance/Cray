use crate::{ConnectRequest, outbound::Outbound};
use anyhow::Result;
use tokio::net::TcpStream;

pub struct DefaultOut;
impl Outbound for DefaultOut {
    type Stream = TcpStream;
    type Context = ConnectRequest;

    async fn connect(&self, t: &Self::Context) -> Result<Self::Stream> {
        let s = TcpStream::connect((t.addr.as_str(), t.port)).await?;
        s.set_nodelay(true)?;
        Ok(s)
    }
}
