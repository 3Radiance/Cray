use crate::outbound::Outbound;
use crate::outbound::default::DefaultOut;

use anyhow::Result;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::ConnectRequest;
use crate::protocol::Protocol;

pub mod handshake;

pub struct Vless {
    pub uuid: [u8; 16],
}

impl Protocol for Vless {
    type Context = ();
    type Outcome = ();

    async fn client<C, W>(
        &mut self,
        mut plain: C,
        mut wire: W,
        t: &ConnectRequest,
        _ctx: &mut Self::Context,
    ) -> Result<Self::Outcome>
    where
        C: AsyncRead + AsyncWrite + Unpin + Send + Sync,
        W: AsyncRead + AsyncWrite + Unpin + Send + Sync,
    {
        crate::log_tag!(
            debug,
            "VLESS",
            "Client VLESS relay started for target {}:{}",
            t.addr,
            t.port
        );

        handshake::client_handshake(&self.uuid, &mut wire, t.port, &t.addr).await?;

        tokio::io::copy_bidirectional(&mut plain, &mut wire).await?;
        Ok(())
    }

    async fn server<W>(&mut self, mut wire: W, _ctx: &mut Self::Context) -> Result<Self::Outcome>
    where
        W: AsyncRead + AsyncWrite + Unpin + Send + Sync,
    {
        crate::log_tag!(debug, "VLESS", "Server VLESS relay started");
        let req = handshake::server_handshake(&self.uuid, &mut wire).await?;
        crate::log_tag!(debug, "VLESS", "Parsed upstream target: {:?}", req);

        let out = DefaultOut;
        let mut plain = out.connect(&req).await?;

        tokio::io::copy_bidirectional(&mut plain, &mut wire).await?;

        Ok(())
    }
}
