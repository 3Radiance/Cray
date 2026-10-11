use crate::config::loader::{get_ca, get_config};
use crate::fingerprint::tls::builder::{create_ssl_acceptor, set_alps, tls_builder};
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
    type Context = String;

    async fn client<W>(&mut self, wire: W, sni: &mut Self::Context) -> Result<Self::Stream>
    where
        W: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
    {
        let loader = get_config();
        let http2 = if let Some(http2) = &loader.http2 {
            http2
        } else {
            return Err(anyhow::anyhow!("HTTP/2 configuration not found"));
        };

        let tls = if let Some(tls) = &loader.tls {
            tls
        } else {
            return Err(anyhow::anyhow!("TLS configuration not found"));
        };

        let connector = tls_builder().await?;
        let mut ssl = connector.configure()?.into_ssl(sni)?;
        set_alps(&mut ssl, http2, tls);
        let boxed_wire: BoxedStream = Box::pin(wire);
        let mut remote = SslStream::new(ssl, boxed_wire)?;
        tokio::time::timeout(Duration::from_secs(6), Pin::new(&mut remote).connect()).await??;
        Ok(remote)
    }

    async fn server<P>(&mut self, plain: P, _ctx: &mut Self::Context) -> Result<Self::Incoming>
    where
        P: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
    {
        let loader = get_config();
        let ca = get_ca();
        let alpn = if let Some(tls) = &loader.tls {
            tls.encode_alpn_wire()
        } else {
            b"\x08http/1.1".to_vec()
        };

        let acceptor = create_ssl_acceptor(ca, alpn)?;
        let ssl = Ssl::new(acceptor.context())?;
        let boxed_plain: BoxedStream = Box::pin(plain);
        let mut tls_stream = SslStream::new(ssl, boxed_plain)?;

        if let Err(e) =
            tokio::time::timeout(Duration::from_secs(6), Pin::new(&mut tls_stream).accept()).await?
        {
            crate::log_tag!(error, "TLS", "Handshake failed: {}", e);
            return Err(e.into());
        }

        Ok(Once::new(tls_stream))
    }
}
