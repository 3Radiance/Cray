use super::{Channel, Mode, Pair, XH2, XH2Rx, XH2Tx, req_host};
use crate::profiles::{Xhttp2Config, get_xhttp2};
use crate::transport::chunk::ChunkStream;
use anyhow::{Result, anyhow, bail};
use bytes::Bytes;
use cray_core::{BoxedIncoming, BoxedStream, log_tag};
use http::{HeaderMap, Method, Response};
use std::sync::Arc;
use tokio::sync::mpsc;

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

fn build_reject_response(
    cfg: &Xhttp2Config,
    body: Option<&[u8]>,
) -> anyhow::Result<Response<()>> {
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
        Mode::PacketUP => bail!("not suported"),
    };

    Ok(sock)
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
    let headers = get_xhttp2(&h2.profile.as_deref().expect("checked at build"))?;

    let mut conn = builder
        .handshake::<_, Bytes>(plain)
        .await
        .map_err(|e| anyhow!("XH2 handshake failed: {e}"))?;

    let (tx, rx) = mpsc::channel(64);
    let (method, path, authority) = (h2.method.clone(), h2.path.clone(), h2.authority.clone());
    log_tag!(debug, "XH2", "XHTTP/2 listener ready");

    let pair = Arc::clone(&h2.pair);
    let post_header = headers
        .apply_http_headers(&h2.method, &HeaderMap::new())?
        .into_iter()
        .find(|(_, v)| v == "id")
        .map(|(k, _)| k)
        .ok_or(anyhow!("XH2 requires an id header"))?;

    let get_header = headers
        .apply_http_headers(&Method::GET, &HeaderMap::new())?
        .into_iter()
        .find(|(_, v)| v == "id")
        .map(|(k, _)| k)
        .ok_or(anyhow!("XH2 requires an id header"))?;

    let reject_body = headers.reject_body.clone();
    let headers = Arc::clone(&headers);

    tokio::spawn(async move {
        while let Some(res) = conn.accept().await {
            let (req, mut respond) = match res {
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

            match req.method() {
                &Method::GET => {
                    let host_ok =
                        req_host(&req).is_some_and(|h| h.eq_ignore_ascii_case(&authority));
                    if req.uri().path() != path || !host_ok {
                        log_tag!(
                            trace,
                            "XH2",
                            "rejecting request for path '{}' host {:?} method {}",
                            req.uri().path(),
                            req_host(&req),
                            req.method()
                        );
                        let Ok(r) =
                            build_reject_response(&headers, reject_body.as_deref())
                        else {
                            continue;
                        };
                        let _ = match reject_body.as_deref() {
                            Some(body) => {
                                let Ok(mut stream) = respond.send_response(r, false) else {
                                    continue;
                                };
                                stream.send_data(Bytes::copy_from_slice(body), true)
                            }
                            None => respond.send_response(r, true).map(|_| ()),
                        };
                        continue;
                    }
                    let sid = req
                        .headers()
                        .get(get_header.as_str())
                        .and_then(|v| v.to_str().ok())
                        .filter(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()));

                    let Some(sid) = sid else {
                        log_tag!(
                            trace,
                            "XH2",
                            "missing/invalid sid header on {} request",
                            req.method()
                        );
                        let Ok(r) =
                            build_reject_response(&headers, reject_body.as_deref())
                        else {
                            continue;
                        };
                        let _ = match reject_body.as_deref() {
                            Some(body) => {
                                let Ok(mut stream) = respond.send_response(r, false) else {
                                    continue;
                                };
                                stream.send_data(Bytes::copy_from_slice(body), true)
                            }
                            None => respond.send_response(r, true).map(|_| ()),
                        };
                        continue;
                    };

                    let mut guard = pair.lock().await;

                    match guard.remove(sid) {
                        Some(Pair::Read(post_res)) => {
                            log_tag!(trace, "XH2", "paired stream (GET arrived second)");

                            let Ok(r) = build_response(&headers, &Method::GET, sid) else {
                                continue;
                            };
                            let Ok(r2) = build_response(&headers, &method, sid) else {
                                continue;
                            };

                            let Ok(send_stream) = respond.send_response(r, false) else {
                                continue;
                            };
                            let (req, mut post_resp) = post_res;
                            let _ = post_resp.send_response(r2, true);

                            let stream: BoxedStream =
                                Box::pin(ChunkStream::new(XH2Rx::Body(req), XH2Tx(send_stream)));

                            drop(guard);
                            if tx.send(stream).await.is_err() {
                                break;
                            }
                        }
                        _ => {
                            log_tag!(trace, "XH2", "parked GET half, waiting for POST");
                            guard.insert(sid.to_string(), Pair::Write((req.into_body(), respond)));
                            continue;
                        }
                    }
                }
                m if m == &method => {
                    let host_ok =
                        req_host(&req).is_some_and(|h| h.eq_ignore_ascii_case(&authority));
                    if req.uri().path() != path || !host_ok {
                        log_tag!(
                            trace,
                            "XH2",
                            "rejecting request for path '{}' host {:?} method {}",
                            req.uri().path(),
                            req_host(&req),
                            req.method()
                        );
                        let Ok(r) =
                            build_reject_response(&headers, reject_body.as_deref())
                        else {
                            continue;
                        };
                        let _ = match reject_body.as_deref() {
                            Some(body) => {
                                let Ok(mut stream) = respond.send_response(r, false) else {
                                    continue;
                                };
                                stream.send_data(Bytes::copy_from_slice(body), true)
                            }
                            None => respond.send_response(r, true).map(|_| ()),
                        };
                        continue;
                    }

                    let sid = req
                        .headers()
                        .get(post_header.as_str())
                        .and_then(|v| v.to_str().ok())
                        .filter(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()));

                    let Some(sid) = sid else {
                        log_tag!(
                            trace,
                            "XH2",
                            "missing/invalid sid header on {} request",
                            req.method()
                        );
                        let Ok(r) =
                            build_reject_response(&headers, reject_body.as_deref())
                        else {
                            continue;
                        };
                        let _ = match reject_body.as_deref() {
                            Some(body) => {
                                let Ok(mut stream) = respond.send_response(r, false) else {
                                    continue;
                                };
                                stream.send_data(Bytes::copy_from_slice(body), true)
                            }
                            None => respond.send_response(r, true).map(|_| ()),
                        };
                        continue;
                    };

                    let mut guard = pair.lock().await;

                    match guard.remove(sid) {
                        Some(Pair::Write(get_respond)) => {
                            log_tag!(trace, "XH2", "paired stream (POST arrived second)");

                            let Ok(r) = build_response(&headers, &Method::GET, sid) else {
                                continue;
                            };

                            let (_, mut getrespond) = get_respond;

                            let Ok(send_stream) = getrespond.send_response(r, false) else {
                                continue;
                            };

                            let Ok(get_ok) = build_response(&headers, &method, sid) else {
                                continue;
                            };

                            let _ = respond.send_response(get_ok, true);

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
                            log_tag!(trace, "XH2", "parked POST half, waiting for GET");
                            guard.insert(sid.to_string(), Pair::Read((req.into_body(), respond)));
                            continue;
                        }
                    }
                }
                _ => {
                    log_tag!(trace, "XH2", "unexpected method {}", req.method());
                    let r = Response::builder().status(404).body(()).unwrap();
                    let _ = respond.send_response(r, true);
                    continue;
                }
            }
        }
    });

    Ok(Box::new(Channel(rx)))
}
