//! `tls` layer: BoringSSL handshake with a fully configurable ClientHello.
//! Client and server are both WRAP (Stream -> Stream): the server handshake
//! runs per accepted connection, fan-out stays upstream.
//! Handshake logic ported from old-cray `transport/tls`.

use crate::fingerprint::tls::builder::set_alps;
use crate::profiles::{get_http2, get_or_build_tls_connector, get_or_load_tls_acceptor, get_tls};
use anyhow::{Result, bail};
use btls::ssl::Ssl;
use cray_core::{BoxFuture, BoxedStream, BuildCx, Cx, Input, Layer, Output, Ports, Side, log_tag};
use serde::Deserialize;
use std::pin::Pin;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_btls::SslStream;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TlsCfg {
    sni: Option<String>,
    cert: Option<String>,
    key: Option<String>,
    tls_path: Option<String>,
    http2_path: Option<String>,
    alpn: Option<Vec<String>>,
}

pub struct Tls {
    cfg: TlsCfg,
    side: Side,
    alpn_wire: Vec<u8>,
}

async fn client_handshake<W>(
    wire: W,
    sni: &str,
    tls_path: &str,
    http2_path: Option<&str>,
) -> Result<BoxedStream>
where
    W: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    log_tag!(
        trace,
        "TLS",
        "Starting client TLS handshake for SNI {} using profile {}",
        sni,
        tls_path
    );

    let connector = get_or_build_tls_connector(tls_path).await?;
    let config = get_tls(tls_path)?;
    let mut ssl = connector.configure()?.into_ssl(sni)?;
    if config.alps == Some(true) {
        let http2_path = http2_path.ok_or_else(|| {
            anyhow::anyhow!(
                "tls profile '{tls_path}' enables 'alps' but the layer has no 'http2_path'"
            )
        })?;
        let http2 = get_http2(http2_path)?;
        set_alps(&mut ssl, http2, config);
    }
    let boxed_wire: BoxedStream = Box::pin(wire);
    let mut remote = SslStream::new(ssl, boxed_wire)?;
    Pin::new(&mut remote).connect().await?;
    log_tag!(trace, "TLS", "TLS client handshake completed for {}", sni);
    Ok(Box::pin(remote))
}

async fn server_handshake<P>(
    plain: P,
    cert: &str,
    key: &str,
    alpn_wire: &[u8],
) -> Result<BoxedStream>
where
    P: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    log_tag!(
        trace,
        "TLS",
        "Starting server TLS handshake with cert {} and key {}",
        cert,
        key
    );
    let acceptor = get_or_load_tls_acceptor(cert, key, alpn_wire)?;
    let ssl = Ssl::new(acceptor.context())?;
    let boxed_plain: BoxedStream = Box::pin(plain);
    let mut tls_stream = SslStream::new(ssl, boxed_plain)?;

    if let Err(e) = Pin::new(&mut tls_stream).accept().await {
        log_tag!(error, "TLS", "Handshake failed: {}", e);
        return Err(e.into());
    }

    log_tag!(trace, "TLS", "TLS server handshake completed");
    Ok(Box::pin(tls_stream))
}

impl Layer for Tls {
    fn ports(&self) -> Ports {
        Ports::WRAP
    }

    fn default_timeout(&self) -> Option<Duration> {
        Some(Duration::from_secs(6))
    }

    fn call<'a>(&'a self, input: Input, _cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let stream = input.stream()?;
            let out = match self.side {
                Side::Outbound => {
                    let (sni, tls_path) = (
                        self.cfg.sni.as_deref().expect("checked at build"),
                        self.cfg.tls_path.as_deref().expect("checked at build"),
                    );
                    client_handshake(stream, sni, tls_path, self.cfg.http2_path.as_deref()).await?
                }
                Side::Inbound => {
                    let (cert, key) = (
                        self.cfg.cert.as_deref().expect("checked at build"),
                        self.cfg.key.as_deref().expect("checked at build"),
                    );
                    server_handshake(stream, cert, key, &self.alpn_wire).await?
                }
            };
            Ok(Output::Stream(out))
        })
    }
}

pub fn build(params: &toml::Table, bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: TlsCfg = params.clone().try_into()?;
    match bcx.side {
        Side::Outbound => {
            if cfg.sni.is_none() || cfg.tls_path.is_none() {
                bail!(
                    "tls outbound requires 'sni' and 'tls_path' (+ 'http2_path' when the profile enables alps)"
                );
            }
            let tls_path = cfg.tls_path.as_deref().expect("checked above");
            let tls = get_tls(tls_path)?;
            if tls.alps == Some(true) {
                match cfg.http2_path.as_deref() {
                    None => bail!(
                        "tls profile '{tls_path}' enables 'alps' but the layer has no 'http2_path'"
                    ),
                    Some(hp) => {
                        get_http2(hp)?;
                    }
                }
            }
        }
        Side::Inbound => {
            if cfg.cert.is_none() || cfg.key.is_none() {
                bail!("tls inbound requires 'cert' and 'key'");
            }
            if cfg.tls_path.is_some() {
                log_tag!(
                    warn,
                    "TLS",
                    "tls_path is ignored on inbound, pass 'alpn' directly"
                );
            }
        }
    }
    let alpn_wire = match (&cfg.alpn, &cfg.tls_path, bcx.side) {
        (Some(list), _, Side::Inbound) => encode_alpn_wire(list),
        _ => Vec::new(),
    };
    Ok(Box::new(Tls {
        cfg,
        side: bcx.side,
        alpn_wire,
    }))
}

fn encode_alpn_wire(alpn: &[String]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for proto in alpn {
        let len = proto.len();
        if len > 0 && len <= 255 {
            bytes.push(len as u8);
            bytes.extend_from_slice(proto.as_bytes());
        }
    }
    bytes
}
