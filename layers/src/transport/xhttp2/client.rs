use super::{Mode, XH2, XH2Rx, XH2Tx};
use crate::crypto::id::generate_session_id;
use crate::profiles::{Xhttp2Config, get_or_build_xhttp2_builder, get_xhttp2};
use crate::transport::chunk::{ChunkStream, ChunkTx};
use anyhow::{Result, anyhow};
use bytes::Bytes;
use cray_core::{BoxedStream, log_tag};
use futures_util::Sink;
use http::Method;
use http::{HeaderMap, Request};
use http2::client::SendRequest;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::mpsc;
use tokio_util::sync::PollSender;

pub const MAX_BODY: usize = 16 * 1024;

pub async fn client_handshake(wire: BoxedStream, h2: &XH2, mode: &Mode) -> Result<BoxedStream> {
    let sock = match mode {
        Mode::StreamUP => stream_up(wire, h2).await?,
        Mode::PacketUP => packet_up(wire, h2).await?,
    };

    Ok(sock)
}

async fn stream_up(wire: BoxedStream, h2: &XH2) -> Result<BoxedStream> {
    let profile = h2.profile.as_deref().expect("checked at build");
    log_tag!(
        trace,
        "XH2",
        "Starting XHTTP/2 client handshake for profile {}",
        profile
    );
    let builder = get_or_build_xhttp2_builder(profile)?;
    let http2 = get_xhttp2(profile)?;
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
    for (k, v) in http2.apply_http_headers(&h2.method, &HeaderMap::new())? {
        if v == "id" {
            post_req = post_req.header(k.as_str(), &sid);
        } else {
            post_req = post_req.header(k.as_str(), v.as_ref());
        }
    }
    let mut get_req = Request::builder()
        .method("GET")
        .uri(format!("https://{}{}", h2.authority, h2.path));
    for (k, v) in http2.apply_http_headers(&Method::GET, &HeaderMap::new())? {
        if v == "id" {
            get_req = get_req.header(k.as_str(), &sid);
        } else {
            get_req = get_req.header(k.as_str(), v.as_ref());
        }
    }

    let (_post_resp, post_send) = sr.send_request(post_req.body(())?, false)?;
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

async fn packet_up(wire: BoxedStream, h2: &XH2) -> Result<BoxedStream> {
    let profile = h2.profile.as_deref().expect("checked at build");
    log_tag!(
        trace,
        "XH2",
        "Starting XHTTP/2 client handshake for profile {}",
        profile
    );
    let builder = get_or_build_xhttp2_builder(profile)?;
    let http2 = get_xhttp2(profile)?;
    let sid = generate_session_id();

    let (send_req, conn) = builder
        .handshake::<_, Bytes>(wire)
        .await
        .map_err(|e| anyhow!("XH2 handshake failed: {e}"))?;

    tokio::spawn(async move {
        let _ = conn.await;
    });

    let mut sr = send_req.ready().await?;

    let mut get_req = Request::builder()
        .method("GET")
        .uri(format!("https://{}{}", h2.authority, h2.path));
    for (k, v) in http2.apply_http_headers(&Method::GET, &HeaderMap::new())? {
        if v == "id" {
            get_req = get_req.header(k.as_str(), &sid);
        } else {
            get_req = get_req.header(k.as_str(), v.as_ref());
        }
    }

    let (get_resp, _) = sr.send_request(get_req.body(())?, true)?;
    let (tx, rx) = mpsc::channel(64);

    let xh2 = h2.clone();
    tokio::spawn(async move {
        if let Err(e) = post_seq(sr.clone(), rx, http2.clone(), sid, xh2).await {
            log_tag!(warn, "XH2", "packet_up post_seq task failed: {e:#}");
        }
    });

    Ok(Box::pin(ChunkStream::new(
        XH2Rx::Pending(get_resp),
        XH2TxPacketUp::new(tx),
    )))
}

async fn post_seq(
    mut sr: SendRequest<Bytes>,
    mut rx: mpsc::Receiver<Bytes>,
    h2: Arc<Xhttp2Config>,
    sid: String,
    xh2: XH2,
) -> Result<()> {
    let mut seq: u64 = 0;
    while let Some(mut buf) = rx.recv().await {
        if buf.is_empty() {
            sr = sr.ready().await?;
            let req = build_post(&h2, &xh2, &sid, &seq.to_string())?;
            let (resp, mut send) = sr.send_request(req, false)?;
            let _ = send.send_data(Bytes::new(), true);
            hold_stream_refs(resp, send);
            drop(sr);
            break;
        }

        while buf.len() > MAX_BODY {
            sr = sr.ready().await?;
            let chunk = buf.split_to(MAX_BODY);

            let req = build_post(&h2, &xh2, &sid, &seq.to_string())?;
            let (resp, mut send) = sr.send_request(req, false)?;
            let _ = send.send_data(chunk, true);
            hold_stream_refs(resp, send);
            seq += 1;
        }

        if !buf.is_empty() {
            sr = sr.ready().await?;
            let req = build_post(&h2, &xh2, &sid, &seq.to_string())?;
            let (resp, mut send) = sr.send_request(req, false)?;
            let _ = send.send_data(buf, true);
            hold_stream_refs(resp, send);
            seq += 1;
        }
    }
    Ok(())
}

fn hold_stream_refs(resp: http2::client::ResponseFuture, send: http2::SendStream<Bytes>) {
    tokio::spawn(async move {
        let _ = resp.await;
        drop(send);
    });
}

fn build_post(h2: &Xhttp2Config, xh2: &XH2, sid: &str, seq: &str) -> Result<Request<()>> {
    let mut req = Request::builder()
        .method(&xh2.method)
        .uri(format!("https://{}{}", xh2.authority, xh2.path));
    for (k, v) in h2.apply_http_headers(&xh2.method, &HeaderMap::new())? {
        if v == "id" {
            req = req.header(k.as_str(), sid);
        } else if v == "seq" {
            req = req.header(k.as_str(), seq);
        } else {
            req = req.header(k.as_str(), v.as_ref())
        }
    }
    Ok(req.body(())?)
}

pub struct XH2TxPacketUp {
    sender: PollSender<Bytes>,
}

impl XH2TxPacketUp {
    pub fn new(tx: mpsc::Sender<Bytes>) -> Self {
        Self {
            sender: PollSender::new(tx),
        }
    }
}

impl ChunkTx for XH2TxPacketUp {
    fn poll_send(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        match Pin::new(&mut self.sender).poll_ready(cx) {
            Poll::Ready(Ok(())) => {
                let bytes = Bytes::copy_from_slice(buf);
                let len = bytes.len();
                if let Err(e) = Pin::new(&mut self.sender).start_send(bytes) {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        format!("XH2 mpsc channel closed: {e}"),
                    )));
                }

                Poll::Ready(Ok(len))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                format!("XH2 mpsc send error: {e}"),
            ))),
            Poll::Pending => Poll::Pending,
        }
    }
    fn poll_finish(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.sender.poll_reserve(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(_)) => return Poll::Ready(Ok(())),
            Poll::Ready(Ok(())) => {}
        }

        match self.sender.send_item(Bytes::new()) {
            Ok(()) => Poll::Ready(Ok(())),
            Err(e) => Poll::Ready(Err(io::Error::other(e))),
        }
    }
}
