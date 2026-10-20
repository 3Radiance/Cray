use crate::fingerprint::tls::{
    builder::{set_alpn_select_callback, set_alps},
    cert::CertPair,
};
use crate::profiles::{get_http2, get_or_build_tls_connector, get_or_load_tls_acceptor, get_tls};
use anyhow::{Context, Result, anyhow, bail};
use btls::ssl::{NameType, SniError, Ssl, SslAcceptor, SslContext, SslMethod};
use cray_core::{BoxFuture, BoxedStream, BuildCx, Cx, Input, Layer, Output, Ports, Side, log_tag};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_btls::SslStream;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TlsCfg {
    sni: Option<String>,
    certs: Option<Vec<TlsLayer>>,
    default: Option<String>,
    tls_path: Option<String>,
    http2_path: Option<String>,
    alpn: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]

struct TlsLayer {
    sni: String,
    cert: String,
    key: String,
}

pub struct Tls {
    cfg: TlsCfg,
    side: Side,
    bundle: Option<SniBundle>,
}

struct SniBundle {
    base: Arc<SslContext>,
    by_sni: Arc<HashMap<String, Arc<SslContext>>>,
}

fn build_bundle(certs: &[TlsLayer], default: &str, alpn: &[u8]) -> Result<SniBundle> {
    let mut by_sni = HashMap::new();
    for e in certs.iter().filter(|x| x.sni != default) {
        let a = get_or_load_tls_acceptor(e.cert.as_str(), e.key.as_str(), alpn)?;
        by_sni.insert(e.sni.clone(), Arc::new(a.context().to_owned()));
    }

    let map = Arc::new(by_sni);
    let mut b = SslAcceptor::mozilla_intermediate(SslMethod::tls())?;
    set_alpn_select_callback(&mut b, alpn.to_vec());
    let d = certs
        .iter()
        .find(|d| d.sni == default)
        .with_context(|| anyhow!("tls inbound dont found default cert"))?;
    let pair = CertPair::load(&d.cert, &d.key)?;
    if let Some(leaf) = pair.chain.first() {
        b.set_certificate(leaf)?;
    }
    for cert in pair.chain.iter().skip(1) {
        b.add_extra_chain_cert(cert.clone())?;
    }
    b.set_private_key(&pair.key)?;
    b.check_private_key()?;

    b.set_servername_callback({
        let map = Arc::clone(&map);
        move |ssl, _alert| match ssl.servername(NameType::HOST_NAME) {
            Some(name) => match map.get(name) {
                Some(ctx) => ssl.set_ssl_context(ctx).map_err(|_| SniError::ALERT_FATAL),
                None => Ok(()),
            },
            None => Ok(()),
        }
    });

    Ok(SniBundle {
        base: Arc::new(b.build().context().to_owned()),
        by_sni: map,
    })
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

async fn server_handshake<P>(plain: P, bundle: &SniBundle) -> Result<BoxedStream>
where
    P: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    log_tag!(trace, "TLS", "Starting server TLS handshake");

    let ssl = Ssl::new(&*bundle.base)?;
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
                    server_handshake(stream, self.bundle.as_ref().expect("checked at build"))
                        .await?
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
            if cfg.certs.is_none() {
                bail!("tls inbound requires 'certs'");
            }
            if cfg.default.is_none() {
                bail!("tls inbound requires 'default'");
            }
            let certs = cfg.certs.as_ref().unwrap();
            let default = cfg.default.as_ref().unwrap();
            if certs.iter().all(|x| x.sni.as_str() != default.as_str()) {
                bail!("tls inbound default '{default}' is not in 'certs'");
            }
            let mut seen: HashSet<&str> = HashSet::new();
            for e in certs {
                if !seen.insert(e.sni.as_str()) {
                    bail!("tls inbound: duplicate sni '{}' in 'certs'", e.sni);
                }
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
    let bundle = match bcx.side {
        Side::Inbound => Some(build_bundle(
            cfg.certs.as_ref().unwrap(),
            cfg.default.as_ref().unwrap(),
            &alpn_wire,
        )?),
        Side::Outbound => None,
    };
    Ok(Box::new(Tls {
        cfg,
        side: bcx.side,
        bundle,
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
