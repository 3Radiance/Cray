use crate::outbound::Outbound;
use crate::outbound::default::DefaultOut;

use anyhow::Result;
use bytes::{BufMut, Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::ConnectRequest;
use crate::protocol::Protocol;

pub struct tsf {
    len_packet: u32,
    magic: u16,
    addr_type: u16,
    addr_len: u16,
    addr: BytesMut,
    port: u16,
    payload_len: u32,
    payload: BytesMut,
}

impl Protocol for tsf {
    type Context = ();
    type Outcome = ();

    async fn client<C, W>(
        &mut self,
        plain: C,
        wire: W,
        t: &ConnectRequest,
        ctx: &mut Self::Context,
    ) -> Result<Self::Outcome>
    where
        C: AsyncRead + AsyncWrite + Unpin + Send + Sync,
        W: AsyncRead + AsyncWrite + Unpin + Send + Sync,
    {
        let (mut p_r, p_w) = tokio::io::split(plain);
        let (w_r, mut w_w) = tokio::io::split(wire);

        let client_to_wire = async {
            let mut buf = BytesMut::new();
            let mut temp = [0u8; 8192];
            let addr_bytes = t.addr.as_bytes();

            loop {
                let n = p_r.read(&mut temp).await?;
                if n == 0 {
                    break;
                }

                let len_packet = (2 + 2 + 2 + addr_bytes.len() + 2 + 4) as u32 + n as u32;

                buf.clear();
                buf.put_u32(len_packet);
                buf.put_u16(self.magic);
                buf.put_u16(0x2);
                buf.put_u16(addr_bytes.len() as u16);
                buf.put(addr_bytes);
                buf.put_u16(t.port);
                buf.put_u32(n as u32);
                buf.put(&temp[..n]);

                w_w.write_all(&buf).await?;
            }

            Ok::<(), anyhow::Error>(())
        };

        let wire_to_client = async {
            let mut tokio_p_w = p_w;
            let mut tokio_w_r = w_r;

            tokio::io::copy(&mut tokio_w_r, &mut tokio_p_w).await?;
            Ok::<(), anyhow::Error>(())
        };

        tokio::try_join!(client_to_wire, wire_to_client)?;

        Ok(())
    }

    async fn server<W>(&mut self, mut wire: W, ctx: &mut Self::Context) -> Result<Self::Outcome>
    where
        W: AsyncRead + AsyncWrite + Unpin + Send,
    {
        let (addr_str, port, initial_payload) = {
            let len_packet = wire.read_u32().await?;

            let mut packet = vec![0u8; len_packet as usize];
            wire.read_exact(&mut packet).await?;

            use bytes::Buf;
            let mut buf = std::io::Cursor::new(packet);

            let _magic = buf.get_u16();
            let _addr_type = buf.get_u16();
            let addr_len = buf.get_u16();

            let mut addr_bytes = vec![0u8; addr_len as usize];
            buf.copy_to_slice(&mut addr_bytes);
            let addr_str = String::from_utf8_lossy(&addr_bytes).to_string();

            let port = buf.get_u16();
            let payload_len = buf.get_u32();

            let mut payload = vec![0u8; payload_len as usize];
            buf.copy_to_slice(&mut payload);

            (addr_str, port, payload)
        };

        let connect = ConnectRequest {
            addr: addr_str,
            port,
        };
        let out = DefaultOut;
        let mut plain = out.connect(&connect).await?;

        if !initial_payload.is_empty() {
            plain.write_all(&initial_payload).await?;
        }

        let (mut p_r, mut p_w) = tokio::io::split(plain);
        let (mut w_r, mut w_w) = tokio::io::split(wire);

        let wire_to_plain = async {
            loop {
                let len_packet = match w_r.read_u32().await {
                    Ok(len) => len,
                    Err(_) => break,
                };

                let mut packet = vec![0u8; len_packet as usize];
                if w_r.read_exact(&mut packet).await.is_err() {
                    break;
                }

                use bytes::Buf;
                let mut buf = std::io::Cursor::new(packet);

                let _magic = buf.get_u16();
                let _addr_type = buf.get_u16();
                let addr_len = buf.get_u16();
                buf.advance(addr_len as usize);
                let _port = buf.get_u16();

                let payload_len = buf.get_u32();
                let mut payload = vec![0u8; payload_len as usize];
                buf.copy_to_slice(&mut payload);

                if p_w.write_all(&payload).await.is_err() {
                    break;
                }
            }
            Ok::<(), anyhow::Error>(())
        };

        let plain_to_wire = async {
            tokio::io::copy(&mut p_r, &mut w_w).await?;
            Ok::<(), anyhow::Error>(())
        };

        tokio::try_join!(wire_to_plain, plain_to_wire)?;

        Ok(())
    }
}
