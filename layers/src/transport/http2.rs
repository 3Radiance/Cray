use super::chunk::{ChunkRx, ChunkStream, ChunkTx};
use crate::profiles::{get_http2, get_or_build_http2};
use anyhow::{Result, anyhow, bail};
use bytes::Bytes;
use cray_core::{
    BoxFuture, BoxedIncoming, BoxedStream, BuildCx, Cx, Incoming as CoreIncoming, Input, Layer,
    Output, Ports, Side, log_tag,
};
use http::{HeaderMap, Request, Response};
use http2::client::ResponseFuture;
use http2::{RecvStream, SendStream};
use serde::Deserialize;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct H2Cfg {
    path: Option<String>,
    host: Option<String>,
    method: Option<String>,
    config: Option<String>,
}

pub struct H2 {
    method: http::Method,
    authority: String,
    path: String,
    profile: Option<String>,
    side: Side,
}

async fn client_handshake(wire: BoxedStream, h2: &H2) -> Result<BoxedStream> {
    let profile = h2.profile.as_deref().expect("checked at build");
    log_tag!(
        trace,
        "H2",
        "Starting HTTP/2 client handshake for profile {}",
        profile
    );
    let builder = get_or_build_http2(profile)?;
    let http2 = get_http2(profile)?;

    let (send_req, conn) = builder
        .handshake::<_, Bytes>(wire)
        .await
        .map_err(|e| anyhow!("h2 handshake failed: {e}"))?;

    tokio::spawn(async move {
        let _ = conn.await;
    });

    let mut req = Request::builder()
        .method(h2.method.clone())
        .uri(format!("https://{}{}", h2.authority, h2.path));
    for (k, v) in http2.apply_http_headers(&HeaderMap::new())? {
        req = req.header(k, v);
    }

    let mut send_req = send_req.ready().await?;
    let (resp_fut, send_stream) = send_req.send_request(req.body(())?, false)?;
    drop(send_req);

    log_tag!(
        trace,
        "H2",
        "HTTP/2 client connected to {}{}",
        h2.authority,
        h2.path
    );
    Ok(Box::pin(ChunkStream::new(
        H2Rx::Pending(resp_fut),
        H2Tx(send_stream),
    )))
}

struct Channel(mpsc::Receiver<BoxedStream>);

impl CoreIncoming for Channel {
    fn accept(&mut self) -> cray_core::BoxFuture<'_, Option<BoxedStream>> {
        Box::pin(async move { self.0.recv().await })
    }
}

async fn server_handshake(plain: BoxedStream, h2: &H2) -> Result<BoxedIncoming> {
    log_tag!(
        trace,
        "H2",
        "Starting HTTP/2 server for path {} method {}",
        h2.path,
        h2.method
    );
    let mut builder = http2::server::Builder::new();
    builder
        .max_concurrent_streams(128)
        .initial_window_size(1 << 20)
        .initial_connection_window_size(4 << 20);

    let mut conn = builder
        .handshake::<_, Bytes>(plain)
        .await
        .map_err(|e| anyhow!("h2 handshake failed: {e}"))?;

    let (tx, rx) = mpsc::channel(64);
    let (method, path, authority) = (h2.method.clone(), h2.path.clone(), h2.authority.clone());
    log_tag!(debug, "H2", "HTTP/2 listener ready");

    tokio::spawn(async move {
        while let Some(res) = conn.accept().await {
            let Ok((req, mut respond)) = res else { break };

            if req.method() != method
                || req.uri().path() != path
                || req.uri().host().map_or(true, |h| h != authority)
            {
                let r = Response::builder().status(404).body(()).unwrap();
                let _ = respond.send_response(r, true);
                continue;
            }

            let r = Response::builder()
                .status(200)
                .header("content-type", "application/octet-stream")
                .body(())
                .unwrap();
            let Ok(send) = respond.send_response(r, false) else {
                continue;
            };

            let stream: BoxedStream =
                Box::pin(ChunkStream::new(H2Rx::Body(req.into_body()), H2Tx(send)));

            match tx.try_send(stream) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {}
                Err(mpsc::error::TrySendError::Closed(_)) => break,
            }
        }
    });

    Ok(Box::new(Channel(rx)))
}

impl Layer for H2 {
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
                Side::Inbound => Ok(Output::Streams(server_handshake(stream, self).await?)),
                Side::Outbound => Ok(Output::Stream(client_handshake(stream, self).await?)),
            }
        })
    }
}

pub fn build(params: &toml::Table, bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: H2Cfg = params.clone().try_into()?;
    let (Some(path), Some(host), Some(method)) = (cfg.path, cfg.host, cfg.method) else {
        bail!("http2 requires 'path', 'host' and 'method'");
    };

    if bcx.side == Side::Outbound && cfg.config.is_none() {
        bail!("http2 outbound requires 'config' (profile path)");
    }

    let method: http::Method = method
        .parse()
        .map_err(|e| anyhow!("http2: invalid method '{method}': {e}"))?;
    Ok(Box::new(H2 {
        method,
        authority: host,
        path,
        profile: cfg.config,
        side: bcx.side,
    }))
}

enum H2Rx {
    Pending(ResponseFuture),
    Body(RecvStream),
    Done,
}

impl ChunkRx for H2Rx {
    fn poll_chunk(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<Option<Bytes>>> {
        loop {
            match self {
                H2Rx::Pending(fut) => {
                    let resp = ready!(Pin::new(fut).poll(cx)).map_err(io::Error::other)?;
                    if !resp.status().is_success() {
                        return Poll::Ready(Err(io::Error::other(format!(
                            "status {}",
                            resp.status()
                        ))));
                    }
                    *self = H2Rx::Body(resp.into_body());
                }
                H2Rx::Body(body) => match ready!(body.poll_data(cx)) {
                    Some(Ok(chunk)) => {
                        let _ = body.flow_control().release_capacity(chunk.len());
                        return Poll::Ready(Ok(Some(chunk)));
                    }
                    Some(Err(e)) => return Poll::Ready(Err(io::Error::other(e))),
                    None => {
                        *self = H2Rx::Done;
                        return Poll::Ready(Ok(None));
                    }
                },
                H2Rx::Done => return Poll::Ready(Ok(None)),
            }
        }
    }
}

struct H2Tx(SendStream<Bytes>);

impl ChunkTx for H2Tx {
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
