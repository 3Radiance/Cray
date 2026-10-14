use crate::config::loader::{get_http2, get_or_build_http2};
use crate::transport::{BoxedStream, ChunkRx, ChunkStream, ChunkTx, Transport};
use anyhow::{Result, anyhow};
use bytes::Bytes;
use http::{HeaderMap, Request, Response};
use http2::client::ResponseFuture;
use http2::{RecvStream, SendStream};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};

pub struct H2 {
    pub method: http::Method,
    pub authority: String,
    pub path: String,
}

impl Transport for H2 {
    type Stream = BoxedStream;
    type Incoming = mpsc::Receiver<BoxedStream>;
    type Context = String;
    type ServerContext = ();

    async fn client<W>(&mut self, wire: W, ctx: &mut Self::Context) -> Result<Self::Stream>
    where
        W: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        crate::log_tag!(
            trace,
            "H2",
            "Starting HTTP/2 client handshake for profile {}",
            ctx
        );
        let builder = get_or_build_http2(ctx)?;
        let http2 = get_http2(ctx)?;

        let (send_req, conn) = timeout(Duration::from_secs(5), builder.handshake::<_, Bytes>(wire))
            .await
            .map_err(|_| anyhow!("h2 handshake timeout"))??;

        tokio::spawn(async move {
            let _ = conn.await;
        });

        let mut req = Request::builder()
            .method(self.method.clone())
            .uri(format!("https://{}{}", self.authority, self.path));
        for (k, v) in http2.apply_http_headers(&HeaderMap::new())? {
            req = req.header(k, v);
        }

        let mut send_req = send_req.ready().await?;
        let (resp_fut, send_stream) = send_req.send_request(req.body(())?, false)?;
        drop(send_req);

        crate::log_tag!(
            trace,
            "H2",
            "HTTP/2 client connected to {}{}",
            self.authority,
            self.path
        );
        Ok(Box::pin(ChunkStream::new(
            H2Rx::Pending(resp_fut),
            H2Tx(send_stream),
        )))
    }

    async fn server<P>(
        &mut self,
        plain: P,
        _ctx: &mut Self::ServerContext,
    ) -> Result<Self::Incoming>
    where
        P: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        crate::log_tag!(
            trace,
            "H2",
            "Starting HTTP/2 server for path {} method {}",
            self.path,
            self.method
        );
        let mut builder = http2::server::Builder::new();
        builder
            .max_concurrent_streams(128)
            .initial_window_size(1 << 20)
            .initial_connection_window_size(4 << 20);

        let mut conn = timeout(Duration::from_secs(5), builder.handshake::<_, Bytes>(plain))
            .await
            .map_err(|_| anyhow!("h2 handshake timeout"))??;

        let (tx, rx) = mpsc::channel(64);
        let (method, path, authority) = (
            self.method.clone(),
            self.path.clone(),
            self.authority.clone(),
        );
        crate::log_tag!(debug, "H2", "HTTP/2 listener ready");

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

        Ok(rx)
    }
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
