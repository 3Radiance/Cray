use crate::config::appconfig::{LayerConfig, Role};
use crate::config::loader::get_config;
use crate::inbound::Inbound;
use crate::outbound::{Outbound, default::DefaultOut};
use crate::protocol::*;
use crate::transport::{BoxedStream, Incoming, Transport, http2::H2, tls::Tls};
use crate::{ConnectRequest, ConnectResult, log_tag};
use anyhow::{Result, anyhow, bail};
use std::{future::Future, pin::Pin, sync::Arc};

type BoxFut = Pin<Box<dyn Future<Output = Result<()>> + Send>>;
type Req = Arc<Option<ConnectRequest>>;

macro_rules! fan {
    ($incoming:expr, $rest:expr, $req:expr) => {{
        let mut incoming = $incoming;
        while let Some(s) = incoming.accept().await {
            let s: BoxedStream = Box::pin(s);
            let req = $req.clone();
            let rest = $rest;
            tokio::spawn(async move {
                if let Err(e) = run_with(rest, s, req).await {
                    log_tag!(error, "CHAIN", "{}", e);
                }
            });
        }
        Ok(())
    }};
}

pub fn run(layers: &'static [LayerConfig], stream: BoxedStream) -> BoxFut {
    run_with(layers, stream, Arc::new(None))
}

fn run_with(layers: &'static [LayerConfig], mut stream: BoxedStream, req: Req) -> BoxFut {
    Box::pin(async move {
        let Some((layer, rest)) = layers.split_first() else {
            return finish(stream, req).await;
        };

        match layer {
            LayerConfig::HttpConnect => {
                let inb = crate::inbound::http_connect::HttpRequest;
                let r = inb.handshake(&mut stream).await?;
                inb.reply(&mut stream, ConnectResult::Ok).await?;
                run_with(rest, stream, Arc::new(Some(r))).await
            }
            LayerConfig::Tls {
                cert,
                key,
                tls_path,
                ..
            } => {
                let (cert_path, key_path, config) = match (
                    cert.as_deref(),
                    key.as_deref(),
                    tls_path.as_deref(),
                ) {
                    (Some(c), Some(k), Some(config_path)) => (c, k, config_path),
                    _ => {
                        bail!(
                            "TLS layer requires 'cert', 'key' and 'config' paths to be specified!"
                        )
                    }
                };
                let mut tls = Tls;
                let mut ctx = (
                    cert_path.to_string(),
                    key_path.to_string(),
                    config.to_owned(),
                );
                let incoming = tls.server(stream, &mut ctx).await?;
                fan!(incoming, rest, req)
            }
            LayerConfig::Http2 {
                path, method, host, ..
            } => {
                let (path, host, method) =
                    match (path.as_deref(), host.as_deref(), method.as_deref()) {
                        (Some(p), Some(h), Some(m)) => (p, h, m),
                        _ => {
                            bail!("Http2 layer requires 'path', 'host', 'method' to be specified!")
                        }
                    };
                let mut h2 = H2 {
                    method: method.parse()?,
                    authority: host.to_string(),
                    path: path.to_string(),
                };
                let incoming = h2.server(stream, &mut ()).await?;
                fan!(incoming, rest, req)
            }
            other => bail!("layer {:?} is not valid in this chain", other),
        }
    })
}

async fn finish(stream: BoxedStream, req: Req) -> Result<()> {
    let cfg = get_config();
    let proto_layer = match cfg.role {
        Role::Client => cfg.outbound.layers.last(),
        Role::Server => cfg.inbound.layers.last(),
    };

    let mut proto = if let Some(layer) = proto_layer {
        proto_builder(layer)?
    } else {
        bail!("No protocol layer found");
    };

    let mut ctx = ();

    match cfg.role {
        Role::Client => {
            let r = (*req)
                .as_ref()
                .ok_or_else(|| anyhow!("no ConnectRequest"))?;
            let out = DefaultOut.connect(r).await?;
            proto.client(stream, out, r, &mut ctx).await?;
        }
        Role::Server => {
            proto.server(stream, &mut ctx).await?;
        }
    }
    Ok(())
}
