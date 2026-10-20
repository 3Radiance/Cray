use crate::cx::{Cx, Key};
use anyhow::{Result, bail};
use std::{future::Future, pin::Pin, time::Duration};
use tokio::io::{AsyncRead, AsyncWrite};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait AsyncReadWrite: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncReadWrite for T {}
pub type BoxedStream = Pin<Box<dyn AsyncReadWrite + Send + Sync + 'static>>;

pub trait Incoming: Send {
    fn accept(&mut self) -> BoxFuture<'_, Option<BoxedStream>>;
}
pub type BoxedIncoming = Box<dyn Incoming>;

#[derive(Debug, Clone)]
pub struct ConnectRequest {
    pub addr: String,
    pub port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Inbound,
    Outbound,
}

#[derive(Debug, Clone, Copy)]
pub struct BuildCx {
    pub side: Side,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Port {
    Nothing,
    Stream,
    Streams,
    Done,
}

#[derive(Debug, Clone, Copy)]
pub struct Ports {
    pub takes: Port,
    pub gives: Port,
}

impl Ports {
    pub const SOURCE: Ports = Ports {
        takes: Port::Nothing,
        gives: Port::Stream,
    };
    pub const LISTENER: Ports = Ports {
        takes: Port::Nothing,
        gives: Port::Streams,
    };
    pub const WRAP: Ports = Ports {
        takes: Port::Stream,
        gives: Port::Stream,
    };
    pub const ACCEPT: Ports = Ports {
        takes: Port::Stream,
        gives: Port::Streams,
    };
    pub const TERMINAL: Ports = Ports {
        takes: Port::Stream,
        gives: Port::Done,
    };
}

pub enum Input {
    None,
    Stream(BoxedStream),
}

impl Input {
    pub fn stream(self) -> Result<BoxedStream> {
        match self {
            Input::Stream(s) => Ok(s),
            Input::None => bail!("layer expected an input stream"),
        }
    }
}

pub enum Output {
    Stream(BoxedStream),
    Streams(BoxedIncoming),
    Done,
    Consumed,
}

pub trait Layer: Send + Sync + 'static {
    fn ports(&self) -> Ports;

    fn requires(&self) -> Vec<Key> {
        Vec::new()
    }

    fn provides(&self) -> Vec<Key> {
        Vec::new()
    }

    fn default_timeout(&self) -> Option<Duration> {
        None
    }

    fn call<'a>(&'a self, input: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>>;
}

pub type Factory = fn(&toml::Table, &BuildCx) -> Result<Box<dyn Layer>>;
