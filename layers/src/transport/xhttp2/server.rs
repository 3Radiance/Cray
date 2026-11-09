use super::client::MAX_BODY;
use super::{Channel, Mode, Pair, XH2, XH2Rx, XH2Tx, req_host};
use super::{ChunkRx, reorder::Reorder};
use crate::profiles::{Xhttp2Config, get_xhttp2};
use crate::transport::chunk::ChunkStream;
use anyhow::{Result, anyhow};
use bytes::{Bytes, BytesMut};
use cray_core::{BoxedIncoming, BoxedStream, log_tag};
use dashmap::DashMap;
use http::Request;
use http::{HeaderMap, Method, Response};
use http2::{RecvStream, server::SendResponse};
use std::io;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context, Poll};
use tokio::sync::{Mutex, mpsc};
use tokio::time::{Duration, Instant, interval};

pub trait Expired: Send + Sync {
    fn expired(&self, timeout: Duration) -> bool;
}

fn build_response(cfg: &Xhttp2Config, method: &Method, sid: &str) -> anyhow::Result<Response<()>> {
    let mut b = Response::builder().status(200);
    let mut content_type = false;
    for (k, v) in cfg.apply_response_headers(method)? {
        if k == http::header::CONTENT_TYPE {
            content_type = true;
        }
        if v == "id" {
            b = b.header(k.as_str(), sid);
        } else {
            b = b.header(k.as_str(), v.as_ref());
        }
    }
    if !content_type {
        b = b.header("content-type", "application/octet-stream");
    }
    Ok(b.body(()).unwrap())
}

fn build_reject_response(cfg: &Xhttp2Config, body: Option<&[u8]>) -> anyhow::Result<Response<()>> {
    let mut b = Response::builder().status(cfg.reject_status.unwrap_or(404));
    let mut content_type = false;
    for (k, v) in cfg.apply_reject_headers()? {
        if k == http::header::CONTENT_TYPE {
            content_type = true;
        }
        b = b.header(k.as_str(), v.as_ref());
    }
    if body.is_some() && !content_type {
        b = b.header("content-type", "text/html");
    }
    Ok(b.body(()).unwrap())
}

pub async fn server_handshake(plain: BoxedStream, h2: &XH2, mode: &Mode) -> Result<BoxedIncoming> {
    let sock = match mode {
        Mode::StreamUP => stream_up(plain, h2).await?,
        Mode::PacketUP => packet_up(plain, h2).await?,
    };

    Ok(sock)
}

fn find_id_header(cfg: &Xhttp2Config, method: &Method) -> Result<String> {
    cfg.apply_http_headers(method, &HeaderMap::new())?
        .into_iter()
        .find(|(_, v)| v == "id")
        .map(|(k, _)| k.as_str().to_owned())
        .ok_or(anyhow!("XH2 requires an id header"))
}

fn route_sid(
    req: &Request<RecvStream>,
    path: &str,
    authority: &str,
    header: &str,
) -> Option<String> {
    let host_ok = req_host(req).is_some_and(|h| h.eq_ignore_ascii_case(authority));
    if req.uri().path() != path || !host_ok {
        log_tag!(
            trace,
            "XH2",
            "rejecting request for path '{}' host {:?} method {}",
            req.uri().path(),
            req_host(req),
            req.method()
        );
        return None;
    }

    let sid = req
        .headers()
        .get(header)
        .and_then(|v| v.to_str().ok())
        .filter(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()));

    if sid.is_none() {
        log_tag!(
            trace,
            "XH2",
            "missing/invalid sid header on {} request",
            req.method()
        );
    }
    sid.map(str::to_owned)
}

fn reject(cfg: &Xhttp2Config, mut respond: SendResponse<Bytes>) {
    let body = cfg.reject_body.as_deref();
    let Ok(r) = build_reject_response(cfg, body) else {
        return;
    };
    let _ = match body {
        Some(body) => match respond.send_response(r, false) {
            Ok(mut stream) => stream.send_data(Bytes::copy_from_slice(body), true),
            Err(e) => Err(e),
        },
        None => respond.send_response(r, true).map(|_| ()),
    };
}

fn method_not_allowed(method: &Method, mut respond: SendResponse<Bytes>) {
    log_tag!(trace, "XH2", "unexpected method {}", method);
    let r = Response::builder().status(405).body(()).unwrap();
    let _ = respond.send_response(r, true);
}

async fn deliver(tx: &mpsc::Sender<BoxedStream>, stream: BoxedStream) -> bool {
    tx.send(stream).await.is_ok()
}

async fn on_get(
    mut respond: SendResponse<Bytes>,
    sid: String,
    cfg: Arc<Xhttp2Config>,
    tx: &mpsc::Sender<BoxedStream>,
) {
    let pair = get_or_gen_pair(&sid);
    let mut guard = pair.state.lock().await;

    let Ok(r) = build_response(&*cfg, &Method::GET, &sid) else {
        return;
    };

    let state = std::mem::replace(&mut *guard, Pair::Done);

    match state {
        Pair::Post(post_body) => {
            log_tag!(trace, "XH2", "paired stream (GET arrived second)");

            let Ok(send_stream) = respond.send_response(r, false) else {
                return;
            };

            let stream: BoxedStream =
                Box::pin(ChunkStream::new(XH2Rx::Body(post_body), XH2Tx(send_stream)));
            drop(guard);
            let _ = deliver(tx, stream).await;
            pair.done.store(true, Ordering::Relaxed);
        }
        Pair::Waiting => {
            log_tag!(trace, "XH2", "parked GET half, waiting for POST");
            let Ok(send_stream) = respond.send_response(r, false) else {
                return;
            };
            *guard = Pair::Get(send_stream);
            drop(guard);
        }
        Pair::Get(x) => {
            *guard = Pair::Get(x);
            drop(guard);
        }
        Pair::Done => return,
    }
}

async fn on_post(
    req: Request<RecvStream>,
    mut respond: SendResponse<Bytes>,
    sid: String,
    cfg: Arc<Xhttp2Config>,
    method: Method,
    tx: &mpsc::Sender<BoxedStream>,
) {
    let pair = get_or_gen_pair(&sid);
    let mut guard = pair.state.lock().await;
    let Ok(r) = build_response(&*cfg, &method, &sid) else {
        return;
    };

    let state = std::mem::replace(&mut *guard, Pair::Done);

    match state {
        Pair::Get(send_stream) => {
            log_tag!(trace, "XH2", "paired stream (POST arrived second)");

            let Ok(_) = respond.send_response(r, false) else {
                return;
            };

            let stream: BoxedStream = Box::pin(ChunkStream::new(
                XH2Rx::Body(req.into_body()),
                XH2Tx(send_stream),
            ));
            drop(guard);
            let _ = deliver(tx, stream).await;
            pair.done.store(true, Ordering::Relaxed);
        }
        Pair::Waiting => {
            log_tag!(trace, "XH2", "parked POST half, waiting for GET");
            if respond.send_response(r, true).is_err() {
                return;
            }
            *guard = Pair::Post(req.into_body());
            drop(guard);
        }
        Pair::Post(x) => {
            *guard = Pair::Post(x);
            drop(guard);
        }
        Pair::Done => return,
    }
}

pub struct PairEntry {
    pub created: Instant,
    state: Mutex<Pair>,
    pub done: AtomicBool,
}

impl PairEntry {
    pub fn new() -> Self {
        Self {
            created: Instant::now(),
            state: Mutex::new(Pair::Waiting),
            done: AtomicBool::new(false),
        }
    }
}

impl Expired for PairEntry {
    fn expired(&self, timeout: Duration) -> bool {
        self.done.load(Ordering::Relaxed) || self.created.elapsed() > timeout
    }
}

fn pair_cache() -> &'static DashMap<String, Arc<PairEntry>> {
    static CACHE: OnceLock<DashMap<String, Arc<PairEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| {
        tokio::spawn(async move { session_gc(pair_cache()).await });
        DashMap::new()
    })
}

fn get_or_gen_pair(sid: &str) -> Arc<PairEntry> {
    pair_cache()
        .entry(sid.to_string())
        .or_insert_with(|| Arc::new(PairEntry::new()))
        .value()
        .clone()
}

async fn stream_up(plain: BoxedStream, h2: &XH2) -> Result<BoxedIncoming> {
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
    let cfg = get_xhttp2(h2.profile.as_deref().expect("checked at build"))?;

    let mut conn = builder
        .handshake::<_, Bytes>(plain)
        .await
        .map_err(|e| anyhow!("XH2 handshake failed: {e}"))?;

    let (tx, rx) = mpsc::channel(64);
    let (method, path, authority) = (h2.method.clone(), h2.path.clone(), h2.authority.clone());
    log_tag!(debug, "XH2", "XHTTP/2 listener ready");

    let post_header = find_id_header(&cfg, &h2.method)?;
    let get_header = find_id_header(&cfg, &Method::GET)?;

    tokio::spawn(async move {
        while let Some(res) = conn.accept().await {
            let (req, respond) = match res {
                Ok(v) => v,
                Err(e) => {
                    log_tag!(trace, "XH2", "accept failed / connection closed: {e:#}");
                    break;
                }
            };
            log_tag!(
                trace,
                "XH2",
                "accept stream: method {} path '{}' host {:?}",
                req.method(),
                req.uri().path(),
                req_host(&req)
            );

            let is_get = req.method() == Method::GET;
            if !is_get && req.method() != method {
                method_not_allowed(req.method(), respond);
                continue;
            }

            let header = if is_get { &get_header } else { &post_header };
            let Some(sid) = route_sid(&req, &path, &authority, header) else {
                reject(&cfg, respond);
                continue;
            };

            if is_get {
                let tx = tx.clone();
                let cfg = cfg.clone();
                tokio::spawn(async move { on_get(respond, sid, cfg, &tx).await })
            } else {
                let method = method.clone();
                let tx = tx.clone();
                let cfg = cfg.clone();
                tokio::spawn(async move { on_post(req, respond, sid, cfg, method, &tx).await })
            };
        }
    });

    Ok(Box::new(Channel(rx)))
}

pub struct Session {
    pub created: Instant,
    reorder: Mutex<Reorder>,
    out_tx: mpsc::Sender<Bytes>,
    out_rx: Mutex<Option<mpsc::Receiver<Bytes>>>,
    pub last_used: AtomicU64,
    pub get_attached: AtomicBool,
}

impl Session {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(64);
        Self {
            created: Instant::now(),
            reorder: Mutex::new(Reorder::new()),
            out_tx: tx,
            out_rx: Mutex::new(Some(rx)),
            last_used: AtomicU64::new(0),
            get_attached: AtomicBool::new(false),
        }
    }
}

impl Expired for Session {
    fn expired(&self, timeout: Duration) -> bool {
        !self.get_attached.load(Ordering::Relaxed)
            && self.created.elapsed().saturating_sub(Duration::from_millis(
                self.last_used.load(Ordering::Relaxed),
            )) > timeout
    }
}

fn session_cache() -> &'static DashMap<String, Arc<Session>> {
    static CACHE: OnceLock<DashMap<String, Arc<Session>>> = OnceLock::new();
    CACHE.get_or_init(|| {
        tokio::spawn(async move { session_gc(session_cache()).await });
        DashMap::new()
    })
}

fn get_or_gen_session(sid: &String) -> Arc<Session> {
    session_cache()
        .entry(sid.to_owned())
        .or_insert_with(|| Arc::new(Session::new()))
        .value()
        .clone()
}

fn find_seq_header(cfg: &Xhttp2Config, method: &Method) -> Result<String> {
    cfg.apply_http_headers(method, &HeaderMap::new())?
        .into_iter()
        .find(|(_, v)| v == "seq")
        .map(|(k, _)| k.as_str().to_owned())
        .ok_or(anyhow!("XH2 requires an seq header"))
}

async fn packet_up(plain: BoxedStream, h2: &XH2) -> Result<BoxedIncoming> {
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
    let cfg = get_xhttp2(h2.profile.as_deref().expect("checked at build"))?;

    let mut conn = builder
        .handshake::<_, Bytes>(plain)
        .await
        .map_err(|e| anyhow!("XH2 handshake failed: {e}"))?;

    let (tx, rx) = mpsc::channel(64);
    let (method, path, authority) = (h2.method.clone(), h2.path.clone(), h2.authority.clone());
    log_tag!(debug, "XH2", "XHTTP/2 listener ready");

    let post_header = find_id_header(&cfg, &h2.method)?;
    let get_header = find_id_header(&cfg, &Method::GET)?;
    let seq_header = find_seq_header(&cfg, &h2.method)?;

    tokio::spawn(async move {
        while let Some(res) = conn.accept().await {
            let (req, respond) = match res {
                Ok(v) => v,
                Err(e) => {
                    log_tag!(trace, "XH2", "accept failed / connection closed: {e:#}");
                    break;
                }
            };
            log_tag!(
                trace,
                "XH2",
                "accept stream: method {} path '{}' host {:?}",
                req.method(),
                req.uri().path(),
                req_host(&req)
            );

            let is_get = req.method() == Method::GET;
            if !is_get && req.method() != method {
                method_not_allowed(req.method(), respond);
                continue;
            }

            let header = if is_get { &get_header } else { &post_header };
            let Some(sid) = route_sid(&req, &path, &authority, header) else {
                reject(&cfg, respond);
                continue;
            };
            let session = get_or_gen_session(&sid);

            if is_get {
                let cfg = cfg.clone();
                let tx = tx.clone();
                tokio::spawn(async move { get_handle(session, sid, respond, cfg, tx).await });
            } else {
                let method = method.clone();
                let cfg = cfg.clone();
                let seq_header = seq_header.clone();
                tokio::spawn(async move {
                    if let Err(e) = post_handle(
                        session,
                        sid,
                        req,
                        respond,
                        cfg,
                        seq_header.as_str(),
                        &method,
                    )
                    .await
                    {
                        log_tag!(debug, "XH2", "Err post handle {e}")
                    }
                });
            };
        }
    });

    Ok(Box::new(Channel(rx)))
}

pub struct XH2RxPacketUp(Option<mpsc::Receiver<Bytes>>, String);

impl ChunkRx for XH2RxPacketUp {
    fn poll_chunk(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<Option<Bytes>>> {
        if let Some(rx) = self.0.as_mut() {
            match rx.poll_recv(cx) {
                Poll::Ready(Some(bytes)) => Poll::Ready(Ok(Some(bytes))),
                Poll::Ready(None) => Poll::Ready(Ok(None)),
                Poll::Pending => Poll::Pending,
            }
        } else {
            Poll::Ready(Ok(None))
        }
    }
}

impl Drop for XH2RxPacketUp {
    fn drop(&mut self) {
        session_cache().remove(&self.1);
        if let Some(rx) = self.0.take() {
            drop(rx);
        }
    }
}

async fn get_handle(
    session: Arc<Session>,
    sid: String,
    mut respond: http2::server::SendResponse<Bytes>,
    cfg: Arc<Xhttp2Config>,
    out: mpsc::Sender<BoxedStream>,
) {
    let Some(rx) = session.out_rx.lock().await.take() else {
        reject(&cfg, respond);
        return;
    };
    let Ok(r) = build_response(&cfg, &Method::GET, &sid) else {
        return;
    };
    let Ok(send_stream) = respond.send_response(r, false) else {
        return;
    };

    let stream: BoxedStream = Box::pin(ChunkStream::new(
        XH2RxPacketUp(Some(rx), sid),
        XH2Tx(send_stream),
    ));

    deliver(&out, stream).await;
    session.get_attached.store(true, Ordering::Relaxed);
    session.last_used.store(
        session.created.elapsed().as_millis() as u64,
        Ordering::Relaxed,
    )
}

async fn post_handle(
    session: Arc<Session>,
    sid: String,
    req: Request<RecvStream>,
    mut respond: http2::server::SendResponse<Bytes>,
    cfg: Arc<Xhttp2Config>,
    seq: &str,
    method: &Method,
) -> Result<()> {
    let seq = req
        .headers()
        .get(seq)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    let Some(seq) = seq else {
        reject(&cfg, respond);
        return Err(anyhow!("Reject ,dont have seq hdr"));
    };

    let mut body = req.into_body();
    let mut buf = BytesMut::new();
    while let Some(chunk) = body.data().await {
        let chunk = chunk?;
        body.flow_control().release_capacity(chunk.len())?;
        if buf.len() + chunk.len() > MAX_BODY {
            reject(&cfg, respond);
            return Err(anyhow!("Reject buf overflow"));
        }
        buf.extend_from_slice(&chunk);
    }

    let buf = buf.freeze();
    let mut guard = session.reorder.lock().await;
    match guard.push(seq, buf) {
        Ok(_) => {}
        Err(_) => {
            drop(guard);
            session_cache().remove(&sid);
            reject(&cfg, respond);
            return Err(anyhow!("Reject, invalid seq"));
        }
    };
    for chunk in &guard.buf {
        if chunk.is_empty() {
            session_cache().remove(&sid);
            break;
        }
        if session.out_tx.send(chunk.to_owned()).await.is_err() {
            session_cache().remove(&sid);
            break;
        }
    }
    drop(guard);

    let Ok(r) = build_response(&cfg, method, &sid) else {
        return Err(anyhow!("failed to build"));
    };
    let Ok(_) = respond.send_response(r, true) else {
        return Err(anyhow!("failed to send"));
    };

    session.last_used.store(
        session.created.elapsed().as_millis() as u64,
        Ordering::Relaxed,
    );

    Ok(())
}

pub async fn session_gc<E: Expired>(cache: &DashMap<String, Arc<E>>) {
    let mut interval = interval(Duration::from_secs(15));
    let timeout = Duration::from_secs(30);
    let mut vec: Vec<String> = Vec::with_capacity(64);

    loop {
        interval.tick().await;

        gc_iteration(cache, timeout, &mut vec).await;
    }
}

pub async fn gc_iteration<E: Expired>(
    cache: &DashMap<String, Arc<E>>,
    timeout: Duration,
    vec: &mut Vec<String>,
) {
    vec.clear();

    for entry in cache.iter() {
        let sid = entry.key();
        let session = entry.value();

        if session.expired(timeout) {
            vec.push(sid.clone());
        }
    }

    for sid in vec {
        cache.remove(sid);
        log_tag!(debug, "XH2", "Session timeout delete {sid}")
    }
}
