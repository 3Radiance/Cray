use crate::config::loader::{
    get_http2, get_or_build_tls_connector, get_or_load_tls_acceptor, get_tls,
};
use crate::fingerprint::tls::builder::set_alps;
use crate::transport::{BoxedStream, Once, Transport};
use anyhow::Result;
use btls::ssl::Ssl;
use std::pin::Pin;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_btls::SslStream;

pub struct Tls;
impl Transport for Tls {
    type Stream = SslStream<BoxedStream>;
    type Incoming = Once<SslStream<BoxedStream>>;
    type Context = (String, String, String);
    type ServerContext = (String, String, String);

    async fn client<W>(&mut self, wire: W, ctx: &mut Self::Context) -> Result<Self::Stream>
    where
        W: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
    {
        let (sni, config_path, http2_path) = ctx;
        crate::log_tag!(info, "TLS", "Starting client TLS handshake for SNI {} using profile {}", sni, config_path);

        let connector = get_or_build_tls_connector(config_path).await?;
        let config = get_tls(config_path)?;
        let mut ssl = connector.configure()?.into_ssl(sni)?;
        if let Some(alps) = config.alps {
            if alps {
                let http2 = get_http2(http2_path)?;

                set_alps(&mut ssl, http2, config);
            }
        }
        let boxed_wire: BoxedStream = Box::pin(wire);
        let mut remote = SslStream::new(ssl, boxed_wire)?;
        tokio::time::timeout(Duration::from_secs(6), Pin::new(&mut remote).connect()).await??;
        crate::log_tag!(info, "TLS", "TLS client handshake completed for {}", sni);
        Ok(remote)
    }

    async fn server<P>(&mut self, plain: P, ctx: &mut Self::ServerContext) -> Result<Self::Incoming>
    where
        P: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
    {
        let (cert_path, key_path, tls_path) = ctx;
        crate::log_tag!(info, "TLS", "Starting server TLS handshake with cert {} and key {} using profile {}", cert_path, key_path, tls_path);
        let acceptor = get_or_load_tls_acceptor(cert_path, key_path, tls_path)?;
        let ssl = Ssl::new(acceptor.context())?;
        let boxed_plain: BoxedStream = Box::pin(plain);
        let mut tls_stream = SslStream::new(ssl, boxed_plain)?;

        if let Err(e) =
            tokio::time::timeout(Duration::from_secs(6), Pin::new(&mut tls_stream).accept()).await?
        {
            crate::log_tag!(error, "TLS", "Handshake failed: {}", e);
            return Err(e.into());
        }

        crate::log_tag!(info, "TLS", "TLS server handshake completed");
        Ok(Once::new(tls_stream))
    }
}
