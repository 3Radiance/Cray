use anyhow::{Result, bail};
use bytes::{BufMut, BytesMut};
use cray_core::ConnectRequest;
use std::net::IpAddr;
use std::str::FromStr;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub async fn client_handshake<W>(uuid: &[u8], wire: &mut W, port: u16, addr: &str) -> Result<()>
where
    W: AsyncRead + AsyncWrite + Unpin + Send + Sync,
{
    let mut buf = BytesMut::new();
    buf.put_u8(0); // Version
    buf.put(uuid); // uuid
    buf.put_u8(0); // addons len
    buf.put_u8(1); // cmd 1 = tcp 2 = udp
    buf.put_u16(port); // port

    match IpAddr::from_str(addr) {
        Ok(IpAddr::V4(ipv4)) => {
            buf.put_u8(1); // ATYP 1 = IPv4
            buf.put(&ipv4.octets()[..]); // 4 bytes IP
        }
        Ok(IpAddr::V6(ipv6)) => {
            buf.put_u8(4); // ATYP 4 = IPv6
            buf.put(&ipv6.octets()[..]); // 16 bytes IP
        }
        Err(_) => {
            // If not IP, then this is a domain name (ATYP 3)
            buf.put_u8(3); // ATYP 3 = Domain
            buf.put_u8(addr.len() as u8); // domain length
            buf.put(addr.as_bytes()); // domain bytes
        }
    }

    wire.write_all(&buf).await?;
    Ok(())
}
pub async fn server_handshake<P>(uuid: &[u8], plain: &mut P) -> Result<ConnectRequest>
where
    P: AsyncRead + AsyncWrite + Unpin + Send + Sync,
{
    let mut skip_buf = [0u8; 1];
    plain.read_exact(&mut skip_buf).await?;

    let mut recv_uuid = [0u8; 16];
    plain.read_exact(&mut recv_uuid).await?;

    if recv_uuid != uuid {
        bail!("Invalid UUID")
    }

    let mut skip_buf = [0u8; 2];
    plain.read_exact(&mut skip_buf).await?;

    let mut port = [0u8; 2];
    plain.read_exact(&mut port).await?;

    let mut atyp = [0u8; 1];
    plain.read_exact(&mut atyp).await?;

    let addr = match atyp[0] {
        1 => {
            let mut addr = [0u8; 4];
            plain.read_exact(&mut addr).await?;
            IpAddr::from(addr).to_string()
        }
        4 => {
            let mut addr = [0u8; 16];
            plain.read_exact(&mut addr).await?;
            IpAddr::from(addr).to_string()
        }
        3 => {
            let mut addr_len = [0u8; 1];
            plain.read_exact(&mut addr_len).await?;
            let addr_len = addr_len[0];
            let mut addr = vec![0u8; addr_len as usize];
            plain.read_exact(&mut addr).await?;
            String::from_utf8(addr).unwrap()
        }
        _ => bail!("Invalid ATYP"),
    };

    Ok(ConnectRequest {
        addr,
        port: u16::from_be_bytes(port),
    })
}
