use crate::config::appconfig::{LayerConfig, with_timeout};
use crate::log_tag;
use crate::transport;
use crate::{
    ConnectRequest,
    config::loader::get_config,
    outbound::Outbound,
    transport::BoxedStream,
    transport::Transport,
    transport::{
        fragment,
        fragment::{FragSize, FragStage},
        jitter,
        jitter::{JitterDelay, JitterStage},
    },
};
use anyhow::{Result, bail};
use tokio::net::{TcpStream, UnixStream};
use tokio::time::Duration;

pub struct DefaultOut;
impl Outbound for DefaultOut {
    type Stream = BoxedStream;
    type Context = ConnectRequest;

    async fn connect(&self, t: &Self::Context) -> Result<Self::Stream> {
        let config = get_config();

        let first_layer = &config.outbound.layers[0];

        let mut stream: BoxedStream = match first_layer {
            LayerConfig::Tcp { addr, timeout } => {
                let dur = config.resolve_timeout(timeout.as_ref(), 5)?;
                match addr {
                    Some(fixed_addr) => {
                        let s = with_timeout("tcp connect", dur, async {
                            TcpStream::connect(fixed_addr).await.map_err(anyhow::Error::from)
                        })
                        .await?;
                        s.set_nodelay(true)?;
                        Box::pin(s)
                    }
                    None => {
                        let s = with_timeout("tcp connect", dur, async {
                            TcpStream::connect((t.addr.as_str(), t.port))
                                .await
                                .map_err(anyhow::Error::from)
                        })
                        .await?;
                        s.set_nodelay(true)?;
                        Box::pin(s)
                    }
                }
            }
            LayerConfig::Unix { addr, timeout } => match addr {
                Some(fixed_addr) => {
                    let dur = config.resolve_timeout(timeout.as_ref(), 5)?;
                    let s = with_timeout("unix connect", dur, async {
                        UnixStream::connect(fixed_addr).await.map_err(anyhow::Error::from)
                    })
                    .await?;
                    Box::pin(s)
                }
                None => {
                    anyhow::bail!("Unix socket address not specified");
                }
            },

            _ => anyhow::bail!("First layer must be Tcp or Unix"),
        };

        if config.outbound.layers.len() > 1 {
            for layer in config.outbound.layers.iter().skip(1) {
                log_tag!(debug, "CHAIN", "Processing Outbound layer: {:?}", layer);
                stream = match layer {
                    LayerConfig::Tls {
                        sni,
                        tls_path,
                        http2_path,
                        timeout,
                        ..
                    } => {
                        let (sni, config, h2_config) = match (
                            sni.as_deref(),
                            tls_path.as_deref(),
                            http2_path.as_deref(),
                        ) {
                            (Some(sni_val), Some(config_path), Some(http2_path)) => {
                                (sni_val, config_path, http2_path)
                            }
                            _ => {
                                bail!(
                                    "TLS layer requires 'sni', 'config' and 'http2_path' to be specified!"
                                )
                            }
                        };
                        let dur = get_config().resolve_timeout(timeout.as_ref(), 6)?;
                        let mut tls = transport::tls::Tls;
                        let mut ctx = (sni.to_owned(), config.to_owned(), h2_config.to_owned());
                        let tls_stream =
                            with_timeout("tls handshake", dur, tls.client(stream, &mut ctx))
                                .await?;
                        Box::pin(tls_stream)
                    }
                    LayerConfig::Http2 {
                        path,
                        host,
                        method,
                        config: h2_profile,
                        timeout,
                    } => {
                        let (path, host, method, profile) = match (
                            path.as_deref(),
                            host.as_deref(),
                            method.as_deref(),
                            h2_profile.as_deref(),
                        ) {
                            (Some(p), Some(h), Some(m), Some(c)) => (p, h, m, c),
                            _ => {
                                bail!(
                                    "Http2 layer requires 'path', 'host', 'method' and 'config' to be specified!"
                                )
                            }
                        };
                        let dur = get_config().resolve_timeout(timeout.as_ref(), 5)?;
                        let mut h2 = transport::http2::H2 {
                            method: method.parse().unwrap(),
                            authority: host.to_string(),
                            path: path.to_string(),
                        };
                        let mut profile = profile.to_owned();
                        let h2_stream =
                            with_timeout("h2 handshake", dur, h2.client(stream, &mut profile))
                                .await?;
                        Box::pin(h2_stream)
                    }
                    LayerConfig::Jitter { stages } => {
                        let s = match stages.as_deref() {
                            Some(s) => s.to_vec(),
                            None => vec![JitterStage {
                                bytes: None,
                                delay: JitterDelay::Fixed(Duration::ZERO),
                            }],
                        };
                        let stream: BoxedStream = jitter::wrap(stream, s);
                        Box::pin(stream)
                    }
                    LayerConfig::Fragment { stages } => {
                        let s = match stages.as_deref() {
                            Some(s) => s.to_vec(),
                            None => vec![FragStage {
                                bytes: None,
                                size: FragSize::Fixed(1400),
                            }],
                        };
                        let stream: BoxedStream = fragment::wrap(stream, s);
                        Box::pin(stream)
                    }
                    _ => stream,
                };
            }
        }
        Ok(stream)
    }
}
