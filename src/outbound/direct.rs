use crate::{ConnectRequest, outbound::Outbound};
use anyhow::Result;
use tokio::net::TcpStream;

pub struct Direct;
impl Outbound for Direct {
    type Stream = TcpStream;

    async fn dial(&self, t: &ConnectRequest) -> Result<TcpStream> {
        let s = TcpStream::connect((t.addr.as_str(), t.port)).await?;
        s.set_nodelay(true)?;
        Ok(s)
    }
}
