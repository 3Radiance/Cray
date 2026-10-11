use crate::config::appconfig::LayerConfig;
use crate::transport;
use crate::{
    ConnectRequest, config::loader::get_config, outbound::Outbound, transport::BoxedStream,
    transport::Transport,
};
use anyhow::Result;
use tokio::net::{TcpStream, UnixStream};

pub struct DefaultOut;
impl Outbound for DefaultOut {
    type Stream = BoxedStream;
    type Context = ConnectRequest;

    async fn connect(&self, t: &Self::Context) -> Result<Self::Stream> {
        let config = get_config();

        let first_layer = &config.outbound.layers[0];

        let mut stream: BoxedStream = match first_layer {
            LayerConfig::Tcp { addr } => match addr {
                Some(fixed_addr) => {
                    let s = TcpStream::connect(fixed_addr).await?;
                    s.set_nodelay(true)?;
                    Box::pin(s)
                }
                None => {
                    let s = TcpStream::connect((t.addr.as_str(), t.port)).await?;
                    s.set_nodelay(true)?;
                    Box::pin(s)
                }
            },
            LayerConfig::Unix { addr } => match addr {
                Some(fixed_addr) => {
                    let s = UnixStream::connect(fixed_addr).await?;
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
                stream = match layer {
                    LayerConfig::Tls { sni } => match sni {
                        Some(sni) => {
                            let mut tls = transport::tls::Tls;
                            let tls_stream = tls.client(stream, &mut sni.to_owned()).await?;
                            Box::pin(tls_stream)
                        }
                        None => anyhow::bail!("SNI not specified for TLS"),
                    },
                    LayerConfig::Http2 { path, host, method } => {
                        let path = path.as_deref().unwrap_or("/");
                        let host = host.as_deref().unwrap_or(t.addr.as_str());
                        let method = match method {
                            Some(m) => m.parse().unwrap_or(http::Method::POST),
                            None => http::Method::POST,
                        };

                        let mut h2 = transport::http2::H2 {
                            method,
                            authority: host.to_string(),
                            path: path.to_string(),
                        };

                        let h2_stream = h2.client(stream, &mut "".to_string()).await?;
                        Box::pin(h2_stream)
                    }
                    _ => stream,
                };
            }
        }
        Ok(stream)
    }
}
