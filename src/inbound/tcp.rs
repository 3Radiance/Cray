use crate::Listener;
use anyhow::Result;
use std::net::SocketAddr;
use tokio::net::{TcpListener as TokioTcpListener, TcpStream};

pub struct Tcp {
    listener: TokioTcpListener,
}

impl Tcp {
    pub async fn bind<A: tokio::net::ToSocketAddrs>(addr: A) -> Result<Self> {
        Ok(Self {
            listener: TokioTcpListener::bind(addr).await?,
        })
    }
}

impl Listener for Tcp {
    type Stream = TcpStream;
    type Addr = SocketAddr;

    async fn accept(&self) -> Result<(Self::Stream, Self::Addr)> {
        let (stream, peer) = self.listener.accept().await?;
        stream.set_nodelay(true)?;
        Ok((stream, peer))
    }
}
