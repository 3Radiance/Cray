use crate::ConnectionData;
use anyhow::{Result, anyhow};
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

const MAX_HEADER_BYTES: usize = 16 * 1024;

async fn http_handle(mut client: TcpStream) -> Result<(TcpStream, ConnectionData)> {
    let mut buff = Vec::new();
    let mut buf = [0u8; 1024];
    let mut http: ConnectionData;
    loop {
        let n = client.read(&mut buf).await?;
        if n == 0 {
            return Err(anyhow!("connection closed"));
        }
        buff.extend_from_slice(&buf[..n]);

        if buff.len() > MAX_HEADER_BYTES {
            return Err(anyhow!("request headers too large"));
        }

        match http_parse(&buff) {
            Ok(Some(data)) => {
                http = data;
                break;
            }
            Ok(None) => continue,
            Err(e) => return Err(e),
        }
    }

    Ok((client, http))
}

fn http_parse(buff: &[u8]) -> Result<Option<ConnectionData>> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut req = httparse::Request::new(&mut headers);

    match req.parse(buff) {
        Ok(httparse::Status::Complete(_)) => {
            let host = req
                .headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case("host"))
                .ok_or_else(|| anyhow!("no Host header"))?;
            let host = std::str::from_utf8(host.value)?.trim();

            let (addr, port) = match host.rsplit_once(':') {
                Some((a, p)) if !p.contains(']') => (a, p.parse::<u16>()?),
                _ => (host, 443),
            };

            Ok(Some(ConnectionData {
                addr: addr.to_string(),
                port,
            }))
        }
        Ok(httparse::Status::Partial) => Ok(None),
        Err(e) => Err(anyhow!("failed to parse request: {:?}", e)),
    }
}
