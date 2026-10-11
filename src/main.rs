use anyhow::Result;
use clap::Parser;
use cray::ConnectRequest;
use cray::ConnectResult;
use cray::Listener;
use cray::config::appconfig::{LayerConfig, Role};
use cray::config::loader::{get_config, load};
use cray::inbound::{Inbound, tcp::Tcp, unix::Unix};
use cray::log_tag;
use cray::outbound::Outbound;
use cray::protocol::Protocol;
use cray::transport::{BoxedStream, Transport};
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    #[arg(short, long, default_value = "config.toml")]
    pub config: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    load(&args.config)?;
    cray::logging::init_logging();

    let config = get_config();

    log_tag!(info, "CFG", "Role: {:?}", config.role);
    log_tag!(info, "CFG", "Inbound: {:#?}", config.inbound.layers);
    log_tag!(info, "CFG", "Outbound: {:#?}", config.outbound.layers);

    inbound().await?;

    Ok(())
}

pub async fn inbound() -> Result<()> {
    let config = get_config();
    let first_layer = &config.inbound.layers[0];
    match first_layer {
        LayerConfig::Tcp { addr } => {
            let bind_addr = addr.as_ref().expect("TCP Inbound needs an address");
            let listener = Tcp::bind(bind_addr).await?;
            log_tag!(info, "INB", "Listening on {}", bind_addr);
            loop {
                let (stream, _peer) = listener.accept().await?;
                let boxed_stream: BoxedStream = Box::pin(stream);

                tokio::spawn(async move {
                    if let Err(e) = transport(boxed_stream).await {
                        eprintln!("Inbound connection error: {}", e);
                    }
                });
            }
        }
        LayerConfig::Unix { addr } => {
            let bind_addr = addr.as_ref().expect("Unix Inbound needs an address");
            let listener = Unix::bind(bind_addr)?;
            log_tag!(info, "INB", "Listening on {}", bind_addr);
            loop {
                let (stream, _peer) = listener.accept().await?;
                let boxed_stream: BoxedStream = Box::pin(stream);

                tokio::spawn(async move {
                    if let Err(e) = transport(boxed_stream).await {
                        eprintln!("Inbound connection error: {}", e);
                    }
                });
            }
        }
        _ => anyhow::bail!("First inbound layer must be Tcp or Unix"),
    }
}

async fn transport(mut stream: BoxedStream) -> Result<()> {
    let config = get_config();
    let layers = &config.inbound.layers[1..];
    let mut connect_request: Arc<Option<ConnectRequest>> = Arc::new(None);

    for layer in layers {
        match layer {
            LayerConfig::Http2 { path, method, host } => {
                let mut h2 = cray::transport::http2::H2 {
                    method: method.clone().unwrap_or("POST".to_string()).parse()?,
                    authority: host.clone().unwrap_or_default(),
                    path: path.clone().unwrap_or_else(|| "/".to_string()),
                };
                let mut virtual_streams_rx = h2.server(stream, &mut "".to_string()).await?;
                while let Some(virtual_stream) = virtual_streams_rx.recv().await {
                    let value = connect_request.clone();
                    tokio::spawn(async move {
                        if let Err(e) = finish(virtual_stream, value).await {
                            log_tag!(error, "CHAIN", "Virtual stream error: {}", e);
                        }
                    });
                }
                return Ok(());
            }
            LayerConfig::HttpConnect => {
                let http_inbound = cray::inbound::http_connect::HttpRequest;
                let request = http_inbound.handshake(&mut stream).await?;
                connect_request = Arc::new(Some(request));
                http_inbound.reply(&mut stream, ConnectResult::Ok).await?;
            }

            LayerConfig::Tls { sni } => {
                let mut tls = cray::transport::tls::Tls;
                let mut incoming = tls.server(stream, &mut "".to_string()).await?;
                use cray::transport::Incoming;
                let tls_stream = incoming
                    .accept()
                    .await
                    .expect("TLS Once Incoming should always yield a stream");
                stream = Box::pin(tls_stream);
            }
            _ => {}
        }
    }
    finish(stream, connect_request).await
}

async fn finish(stream: BoxedStream, request: Arc<Option<ConnectRequest>>) -> Result<()> {
    let config = get_config();
    let mut protocol = config
        .inbound
        .layers
        .iter()
        .chain(config.outbound.layers.iter())
        .find_map(|layer| match layer {
            LayerConfig::Tsf { magic } => match magic {
                Some(magic) => Some(cray::protocol::tsf::Tsf { magic: *magic }),
                None => None,
            },
            _ => None,
        })
        .expect("No valid protocol layer found in configuration");
    let mut ctx = ();

    match config.role {
        Role::Client => {
            let req = if let Some(r) = (*request).as_ref() {
                r
            } else {
                return Err(anyhow::anyhow!("No ConnectRequest found"));
            };

            let outbound_stream = cray::outbound::default::DefaultOut;
            let outbound_stream = outbound_stream.connect(&req).await?;

            protocol
                .client(stream, outbound_stream, &req, &mut ctx)
                .await?;
        }
        Role::Server => {
            protocol.server(stream, &mut ctx).await?;
        }
    }

    Ok(())
}
