use crate::Listener;
use anyhow::Result;
use std::net::SocketAddr;
use tokio::net::{TcpListener as TokioTcpListener, TcpStream};

pub struct TcpListener {
    listener: TokioTcpListener,
}

impl TcpListener {
    pub async fn bind<A: tokio::net::ToSocketAddrs>(addr: A) -> Result<Self> {
        Ok(Self {
            listener: TokioTcpListener::bind(addr).await?,
        })
    }
}

impl Listener for TcpListener {
    type Stream = TcpStream;

    async fn accept(&self) -> Result<(TcpStream, SocketAddr)> {
        let (stream, peer) = self.listener.accept().await?;
        stream.set_nodelay(true)?;
        Ok((stream, peer))
    }
}
