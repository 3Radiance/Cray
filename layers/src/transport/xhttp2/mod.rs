use super::{
    chunk::{ChunkRx, ChunkTx},
    xhttp2::server::session_gc,
};
use crate::profiles::{get_or_build_xhttp2_builder, get_xhttp2};
use anyhow::{Result, anyhow, bail};
use bytes::Bytes;
use cray_core::{
    BoxFuture, BoxedStream, BuildCx, Cx, Incoming as CoreIncoming, Input, Layer, Output, Ports,
    Side,
};
use http::{HeaderMap, Method};
use http2::client::ResponseFuture;
use http2::{RecvStream, SendStream};
use serde::Deserialize;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use std::time::Duration;
use tokio::sync::mpsc;

pub mod client;
pub mod reorder;
pub mod server;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct XH2Cfg {
    path: Option<String>,
    host: Option<String>,
    method: Option<String>,
    config: Option<String>,
    mode: Option<String>,
}

#[derive(Clone)]
pub struct XH2 {
    method: http::Method,
    authority: String,
    path: String,
    profile: Option<String>,
    side: Side,
    mode: Mode,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    StreamUP,
    PacketUP,
}

pub enum Pair {
    Get(SendStream<Bytes>),
    Post(RecvStream),
    Waiting,
    Done,
}

fn req_host(req: &http::Request<http2::RecvStream>) -> Option<String> {
    let uri_host = req.uri().host().map(|h| h.to_ascii_lowercase());

    let header_host = req
        .headers()
        .get("host")
        .and_then(|v| v.to_str().ok())
        .map(|h| h.split(':').next().unwrap_or(h).to_ascii_lowercase());

    uri_host.or(header_host)
}

pub struct Channel(mpsc::Receiver<BoxedStream>);

impl CoreIncoming for Channel {
    fn accept(&mut self) -> cray_core::BoxFuture<'_, Option<BoxedStream>> {
        Box::pin(async move { self.0.recv().await })
    }
}

impl Layer for XH2 {
    fn ports(&self) -> Ports {
        match self.side {
            Side::Outbound => Ports::WRAP,
            Side::Inbound => Ports::ACCEPT,
        }
    }

    fn default_timeout(&self) -> Option<Duration> {
        Some(Duration::from_secs(5))
    }

    fn call<'a>(&'a self, input: Input, _cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let stream = input.stream()?;
            match self.side {
                Side::Inbound => Ok(Output::Streams(
                    server::server_handshake(stream, self, &self.mode).await?,
                )),
                Side::Outbound => Ok(Output::Stream(
                    client::client_handshake(stream, self, &self.mode).await?,
                )),
            }
        })
    }
}

pub fn build(params: &toml::Table, bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: XH2Cfg = params.clone().try_into()?;
    let (Some(path), Some(host), Some(method)) = (cfg.path, cfg.host, cfg.method) else {
        bail!("XHTTP2 requires 'path', 'host' and 'method'");
    };

    if cfg.config.is_none() {
        bail!("XHTTP2 requires 'config' (profile path)");
    }

    let method: http::Method = method
        .parse()
        .map_err(|e| anyhow!("XHTTP2: invalid method '{method}': {e}"))?;

    if method == Method::GET {
        bail!("XHTTP2 method must not be GET");
    }

    let h2 = get_xhttp2(cfg.config.as_ref().unwrap())?;
    if !h2
        .apply_http_headers(&method, &HeaderMap::new())?
        .into_iter()
        .any(|(_, v)| v == "id")
    {
        bail!("XHTTP2 needs a header with the value id ({method} / uplink)");
    }
    if !h2
        .apply_http_headers(&Method::GET, &HeaderMap::new())?
        .into_iter()
        .any(|(_, v)| v == "id")
    {
        bail!("XHTTP2 needs a header with the value id (GET / downlink)");
    }

    if bcx.side == Side::Outbound {
        let profile = cfg.config.as_deref().expect("checked above");
        get_or_build_xhttp2_builder(profile)
            .map_err(|e| anyhow!("xhttp2 profile '{profile}': {e:#}"))?;
    }

    let mode = match cfg.mode.as_deref() {
        Some("StreamUp") => Mode::StreamUP,
        Some("PacketUp") => Mode::PacketUP,
        Some(m) => bail!("unknown mode {m}"),
        None => bail!("need mode"),
    };

    tokio::spawn(async move { session_gc().await });
    Ok(Box::new(XH2 {
        method,
        authority: host,
        path,
        profile: cfg.config,
        side: bcx.side,
        mode,
    }))
}

enum XH2Rx {
    Pending(ResponseFuture),
    Body(RecvStream),
    Done,
}

impl ChunkRx for XH2Rx {
    fn poll_chunk(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<Option<Bytes>>> {
        loop {
            match self {
                XH2Rx::Pending(fut) => {
                    let resp = ready!(Pin::new(fut).poll(cx)).map_err(io::Error::other)?;
                    if !resp.status().is_success() {
                        return Poll::Ready(Err(io::Error::other(format!(
                            "status {}",
                            resp.status()
                        ))));
                    }
                    *self = XH2Rx::Body(resp.into_body());
                }
                XH2Rx::Body(body) => match ready!(body.poll_data(cx)) {
                    Some(Ok(chunk)) => {
                        let _ = body.flow_control().release_capacity(chunk.len());
                        return Poll::Ready(Ok(Some(chunk)));
                    }
                    Some(Err(e)) => return Poll::Ready(Err(io::Error::other(e))),
                    None => {
                        *self = XH2Rx::Done;
                        return Poll::Ready(Ok(None));
                    }
                },
                XH2Rx::Done => return Poll::Ready(Ok(None)),
            }
        }
    }
}

struct XH2Tx(SendStream<Bytes>);

impl ChunkTx for XH2Tx {
    fn poll_send(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.0.reserve_capacity(buf.len());
        match ready!(self.0.poll_capacity(cx)) {
            None => Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
            Some(Err(e)) => Poll::Ready(Err(io::Error::other(e))),
            Some(Ok(n)) => {
                if n == 0 {
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                let n = n.min(buf.len());
                self.0
                    .send_data(Bytes::copy_from_slice(&buf[..n]), false)
                    .map_err(io::Error::other)?;
                Poll::Ready(Ok(n))
            }
        }
    }
    fn poll_finish(&mut self, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.0
            .send_data(Bytes::new(), true)
            .map_err(io::Error::other)?;
        Poll::Ready(Ok(()))
    }
}
