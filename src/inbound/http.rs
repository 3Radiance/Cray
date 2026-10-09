use crate::inbound::Inbound;
use crate::{ConnectRequest, ConnectResult};
use anyhow::{Result, anyhow};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const MAX_HEADER_BYTES: usize = 16 * 1024;

pub struct HttpRequest;

impl Inbound for HttpRequest {
    async fn handshake<S>(&self, client: &mut S) -> Result<ConnectRequest>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send,
    {
        let mut buff = Vec::with_capacity(1024);
        let mut tmp = [0u8; 1024];

        loop {
            let n = client.read(&mut tmp).await?;
            if n == 0 {
                return Err(anyhow!("connection closed"));
            }
            buff.extend_from_slice(&tmp[..n]);

            if buff.len() > MAX_HEADER_BYTES {
                return Err(anyhow!("request headers too large"));
            }

            if let Some((req, used)) = parse(&buff)? {
                if used != buff.len() {
                    return Err(anyhow!("unexpected data after CONNECT"));
                }
                return Ok(req);
            }
        }
    }

    async fn reply<S>(&self, client: &mut S, ok: ConnectResult) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send,
    {
        let resp: &[u8] = match ok {
            ConnectResult::Ok => b"HTTP/1.1 200 Connection Established\r\n\r\n",
            ConnectResult::Timeout => b"HTTP/1.1 504 Gateway Timeout\r\n\r\n",
            _ => b"HTTP/1.1 502 Bad Gateway\r\n\r\n",
        };
        client.write_all(resp).await?;
        client.flush().await?;
        Ok(())
    }
}

fn parse(buff: &[u8]) -> Result<Option<(ConnectRequest, usize)>> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut req = httparse::Request::new(&mut headers);

    match req.parse(buff) {
        Ok(httparse::Status::Complete(used)) => {
            if req.method != Some("CONNECT") {
                return Err(anyhow!("only CONNECT is supported"));
            }
            let host = req
                .headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case("host"))
                .ok_or_else(|| anyhow!("no Host header"))?;
            let host = std::str::from_utf8(host.value)?.trim();

            let (addr, port) = match host.rsplit_once(':') {
                Some((a, p)) if !p.contains(']') => {
                    (a, p.parse::<u16>().map_err(|_| anyhow!("bad port"))?)
                }
                _ => return Err(anyhow!("failed split Host header: {}", host)),
            };

            Ok(Some((
                ConnectRequest {
                    addr: addr.trim_matches(['[', ']']).to_string(),
                    port,
                },
                used,
            )))
        }
        Ok(httparse::Status::Partial) => Ok(None),
        Err(e) => Err(anyhow!("failed to parse request: {:?}", e)),
    }
}
