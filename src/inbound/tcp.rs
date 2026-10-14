use crate::Listener;
use anyhow::Result;
use std::net::SocketAddr;
use tokio::net::{TcpListener as TokioTcpListener, TcpStream};

pub struct Tcp {
    listener: TokioTcpListener,
}

impl Tcp {
    pub async fn bind<A: tokio::net::ToSocketAddrs>(addr: A) -> Result<Self> {
        let listener = TokioTcpListener::bind(addr).await?;
        crate::log_tag!(info, "LISTEN", "TCP listener bound");
        Ok(Self { listener })
    }
}

impl Listener for Tcp {
    type Stream = TcpStream;
    type Addr = SocketAddr;

    async fn accept(&self) -> Result<(Self::Stream, Self::Addr)> {
        let (stream, peer) = self.listener.accept().await?;
        stream.set_nodelay(true)?;
        crate::log_tag!(debug, "INB", "Accepted TCP connection from {}", peer);
        Ok((stream, peer))
    }
}
