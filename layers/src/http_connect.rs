use anyhow::{Result, anyhow};
use cray_core::{
    BoxFuture, BoxedStream, BuildCx, ConnectRequest, Cx, Input, Key, Layer, Output, Ports, Side,
    key, log_tag,
};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Duration;

pub const MAX_HEADER_BYTES: usize = 16 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HTTPConnectCfg {
    addr: Option<String>,
}

pub fn build(params: &toml::Table, bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: HTTPConnectCfg = params.clone().try_into()?;
    if cfg.addr.is_none() && bcx.side == Side::Outbound {
        return Err(anyhow!("Client HTTPConnect Need addr"));
    }
    Ok(Box::new(HTTPConnect {
        addr: cfg.addr.unwrap_or_default(),
        side: bcx.side,
    }))
}

pub struct HTTPConnect {
    addr: String,
    side: Side,
}

impl Layer for HTTPConnect {
    fn ports(&self) -> Ports {
        Ports::WRAP
    }

    fn requires(&self) -> Vec<Key> {
        Vec::new()
    }

    fn default_timeout(&self) -> Option<Duration> {
        Some(Duration::from_secs(5))
    }

    fn provides(&self) -> Vec<Key> {
        match self.side {
            Side::Inbound => vec![key::<ConnectRequest>()],
            _ => Vec::new(),
        }
    }

    fn call<'a>(&'a self, input: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            match self.side {
                Side::Inbound => {
                    let mut s = input.stream()?;
                    let cr = self.server(&mut s).await?;
                    log_tag!(debug, "HTTP", "CONNECT {}:{}", cr.addr, cr.port);
                    cx.put(cr);
                    Ok(Output::Stream(s))
                }
                Side::Outbound => {
                    log_tag!(debug, "HTTP", "CONNECT via proxy to {}", self.addr);
                    let mut s = input.stream()?;
                    self.client(&mut s, &self.addr).await?;
                    Ok(Output::Stream(s))
                }
            }
        })
    }
}

impl HTTPConnect {
    async fn server(&self, s: &mut BoxedStream) -> Result<ConnectRequest> {
        let mut buff = Vec::with_capacity(1024);
        let mut tmp = [0u8; 1024];

        loop {
            let n = s.read(&mut tmp).await?;
            if n == 0 {
                return Err(anyhow!("connection closed"));
            }
            buff.extend_from_slice(&tmp[..n]);

            if buff.len() > MAX_HEADER_BYTES {
                return Err(anyhow!("request headers too large"));
            }

            if let Some((req, used)) = server_parse(&buff)? {
                if used != buff.len() {
                    return Err(anyhow!("unexpected data after CONNECT"));
                }
                s.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                    .await?;
                return Ok(req);
            }
        }
    }

    async fn client(&self, s: &mut BoxedStream, host: &str) -> Result<()> {
        s.write_all(format!("CONNECT {} HTTP/1.1\r\nHost: {}\r\n\r\n", host, host).as_bytes())
            .await?;

        let mut buff = Vec::new();
        let mut tmp = [0u8; 1024];
        loop {
            let n = s.read(&mut tmp).await?;
            if n == 0 {
                return Err(anyhow!("connection closed"));
            }
            buff.extend_from_slice(&tmp[..n]);

            if buff.len() > MAX_HEADER_BYTES {
                return Err(anyhow!("request headers too large"));
            }

            if let Some(()) = client_parse(&buff)? {
                return Ok(());
            }
        }
    }
}

fn server_parse(buff: &[u8]) -> Result<Option<(ConnectRequest, usize)>> {
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

fn client_parse(buff: &[u8]) -> Result<Option<()>> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut req = httparse::Response::new(&mut headers);

    match req.parse(buff) {
        Ok(httparse::Status::Complete(_)) => {
            if req.code != Some(200) {
                return Err(anyhow!("expected 200 response"));
            }
            Ok(Some(()))
        }
        Ok(httparse::Status::Partial) => Ok(None),
        Err(e) => Err(anyhow!("failed to parse response: {:?}", e)),
    }
}
