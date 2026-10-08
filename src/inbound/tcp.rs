use crate::inbound::Listener;
use anyhow::Result;
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};

pub struct TcpInbound {
    listener: TcpListener,
}

impl TcpInbound {
    pub async fn bind(addr: &str) -> Result<Self> {
        Ok(Self {
            listener: TcpListener::bind(addr).await?,
        })
    }
}

impl Listener for TcpInbound {
    type Stream = TcpStream;

    async fn accept(&self) -> Result<(TcpStream, SocketAddr)> {
        let (stream, peer) = self.listener.accept().await?;
        stream.set_nodelay(true)?;
        Ok((stream, peer))
    }
}
