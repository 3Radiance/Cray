use super::chunk::{ChunkRx, ChunkStream, ChunkTx};
use crate::crypto::id::generate_session_id;
use crate::profiles::{get_http2, get_or_build_http2};
use anyhow::{Result, anyhow, bail};
use bytes::Bytes;
use cray_core::{
    BoxFuture, BoxedIncoming, BoxedStream, BuildCx, Cx, Incoming as CoreIncoming, Input, Layer,
    Output, Ports, Side, log_tag,
};
use http::{HeaderMap, Method, Request, Response};
use http2::client::ResponseFuture;
use http2::server::SendResponse;
use http2::{RecvStream, SendStream};
use serde::Deserialize;
use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct XH2Cfg {
    path: Option<String>,
    host: Option<String>,
    method: Option<String>,
    config: Option<String>,
}

pub struct XH2 {
    method: http::Method,
    authority: String,
    path: String,
    profile: Option<String>,
    side: Side,
    pair: Arc<Mutex<HashMap<String, Pair>>>,
}

pub enum Pair {
    Write(SendResponse<Bytes>),
    Read(RecvStream),
}

async fn client_handshake(wire: BoxedStream, h2: &XH2) -> Result<BoxedStream> {
    let profile = h2.profile.as_deref().expect("checked at build");
    log_tag!(
        trace,
        "XH2",
        "Starting XHTTP/2 client handshake for profile {}",
        profile
    );
    let builder = get_or_build_http2(profile)?;
    let http2 = get_http2(profile)?;
    let sid = generate_session_id();

    let (send_req, conn) = builder
        .handshake::<_, Bytes>(wire)
        .await
        .map_err(|e| anyhow!("XH2 handshake failed: {e}"))?;

    tokio::spawn(async move {
        let _ = conn.await;
    });

    let mut sr = send_req.ready().await?;

    let mut post_req = Request::builder()
        .method(h2.method.clone())
        .uri(format!("https://{}{}", h2.authority, h2.path));
    for (k, v) in http2.apply_http_headers(&HeaderMap::new())? {
        if v == "id" {
            post_req = post_req.header(k, &sid);
        } else {
            post_req = post_req.header(k, v);
        }
    }
    let (_post_resp, post_send) = sr.send_request(post_req.body(())?, false)?;

    let mut get_req = Request::builder()
        .method("GET")
        .uri(format!("https://{}{}", h2.authority, h2.path));
    for (k, v) in http2.apply_http_headers(&HeaderMap::new())? {
        if v == "id" {
            get_req = get_req.header(k, &sid);
        } else {
            get_req = get_req.header(k, v);
        }
    }

    let (get_resp, _send_stream) = sr.send_request(get_req.body(())?, true)?;

    drop(sr);

    log_tag!(
        trace,
        "XH2",
        "XHTTP/2 client connected to {}{}",
        h2.authority,
        h2.path
    );
    Ok(Box::pin(ChunkStream::new(
        XH2Rx::Pending(get_resp),
        XH2Tx(post_send),
    )))
}

struct Channel(mpsc::Receiver<BoxedStream>);

impl CoreIncoming for Channel {
    fn accept(&mut self) -> cray_core::BoxFuture<'_, Option<BoxedStream>> {
        Box::pin(async move { self.0.recv().await })
    }
}

async fn server_handshake(plain: BoxedStream, h2: &XH2) -> Result<BoxedIncoming> {
    log_tag!(
        trace,
        "XH2",
        "Starting XHTTP/2 server for path {} method {}",
        h2.path,
        h2.method
    );
    let mut builder = http2::server::Builder::new();
    builder
        .max_concurrent_streams(128)
        .initial_window_size(1 << 20)
        .initial_connection_window_size(4 << 20);
    let headers = get_http2(&h2.profile.as_deref().expect("checked at build"))?;
    let id_header = headers
        .apply_http_headers(&HeaderMap::new())?
        .into_iter()
        .find(|(_, v)| v == "id")
        .map(|(k, _)| k)
        .ok_or(anyhow!("XH2 requires an id header"))?;

    let mut conn = builder
        .handshake::<_, Bytes>(plain)
        .await
        .map_err(|e| anyhow!("XH2 handshake failed: {e}"))?;

    let (tx, rx) = mpsc::channel(64);
    let (method, path, authority) = (h2.method.clone(), h2.path.clone(), h2.authority.clone());
    log_tag!(debug, "XH2", "XHTTP/2 listener ready");

    let pair = Arc::clone(&h2.pair);

    tokio::spawn(async move {
        while let Some(res) = conn.accept().await {
            let Ok((req, mut respond)) = res else { break };

            match req.method() {
                &Method::GET => {
                    if req.uri().path() != path || req.uri().host().map_or(true, |h| h != authority)
                    {
                        let r = Response::builder().status(404).body(()).unwrap();
                        let _ = respond.send_response(r, true);
                        continue;
                    }
                    let sid = req
                        .headers()
                        .get(id_header.clone())
                        .and_then(|v| v.to_str().ok())
                        .filter(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()));

                    let Some(sid) = sid else {
                        let r = Response::builder().status(404).body(()).unwrap();
                        let _ = respond.send_response(r, true);
                        continue;
                    };

                    let mut guard = pair.lock().await;

                    match guard.remove(sid) {
                        Some(Pair::Read(recv_stream)) => {
                            let r = Response::builder()
                                .status(200)
                                .header("content-type", "application/octet-stream")
                                .body(())
                                .unwrap();

                            let Ok(send_stream) = respond.send_response(r, false) else {
                                continue;
                            };

                            let stream: BoxedStream = Box::pin(ChunkStream::new(
                                XH2Rx::Body(recv_stream),
                                XH2Tx(send_stream),
                            ));

                            drop(guard);
                            if tx.send(stream).await.is_err() {
                                break;
                            }
                        }
                        _ => {
                            guard.insert(sid.to_string(), Pair::Write(respond));
                            continue;
                        }
                    }
                }
                m if m == &method => {
                    if req.uri().path() != path || req.uri().host().map_or(true, |h| h != authority)
                    {
                        let r = Response::builder().status(404).body(()).unwrap();
                        let _ = respond.send_response(r, true);
                        continue;
                    }

                    let sid = req
                        .headers()
                        .get(id_header.clone())
                        .and_then(|v| v.to_str().ok())
                        .filter(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()));

                    let Some(sid) = sid else {
                        let r = Response::builder().status(404).body(()).unwrap();
                        let _ = respond.send_response(r, true);
                        continue;
                    };

                    let mut guard = pair.lock().await;

                    match guard.remove(sid) {
                        Some(Pair::Write(mut get_respond)) => {
                            let r = Response::builder()
                                .status(200)
                                .header("content-type", "application/octet-stream")
                                .body(())
                                .unwrap();

                            let Ok(send_stream) = get_respond.send_response(r, false) else {
                                continue;
                            };

                            let post_ok = Response::builder().status(200).body(()).unwrap();
                            let _ = respond.send_response(post_ok, true);

                            let stream: BoxedStream = Box::pin(ChunkStream::new(
                                XH2Rx::Body(req.into_body()),
                                XH2Tx(send_stream),
                            ));

                            drop(guard);
                            if tx.send(stream).await.is_err() {
                                break;
                            }
                        }
                        _ => {
                            guard.insert(sid.to_string(), Pair::Read(req.into_body()));
                            continue;
                        }
                    }
                }
                _ => {
                    let r = Response::builder().status(404).body(()).unwrap();
                    let _ = respond.send_response(r, true);
                    continue;
                }
            }
        }
    });

    Ok(Box::new(Channel(rx)))
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
                Side::Inbound => Ok(Output::Streams(server_handshake(stream, self).await?)),
                Side::Outbound => Ok(Output::Stream(client_handshake(stream, self).await?)),
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
    Ok(Box::new(XH2 {
        method,
        authority: host,
        path,
        profile: cfg.config,
        side: bcx.side,
        pair: Arc::new(Mutex::new(HashMap::new())),
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
