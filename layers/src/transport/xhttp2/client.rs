use super::{Mode, XH2, XH2Rx, XH2Tx};
use crate::crypto::id::generate_session_id;
use crate::profiles::{get_or_build_http2, get_xhttp2};
use crate::transport::chunk::ChunkStream;
use anyhow::{Result, anyhow, bail};
use bytes::Bytes;
use cray_core::{BoxedStream, log_tag};
use http::Method;
use http::{HeaderMap, Request};

pub async fn client_handshake(wire: BoxedStream, h2: &XH2, mode: &Mode) -> Result<BoxedStream> {
    let sock = match mode {
        Mode::StreamUP => stream_up(wire, h2).await?,
        Mode::PacketUP => bail!("not suported"),
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
    let builder = get_or_build_http2(profile)?;
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
