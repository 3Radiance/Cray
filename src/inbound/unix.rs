use crate::Listener;
use anyhow::Result;
use tokio::net::{UnixListener as TokioUnixListener, UnixStream, unix::SocketAddr};

pub struct Unix {
    listener: TokioUnixListener,
}

impl Unix {
    pub fn bind<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
        let listener = TokioUnixListener::bind(path)?;
        crate::log_tag!(info, "LISTEN", "Unix listener bound at {}", path.display());
        Ok(Self { listener })
    }
}

impl Listener for Unix {
    type Stream = UnixStream;
    type Addr = SocketAddr;

    async fn accept(&self) -> Result<(Self::Stream, Self::Addr)> {
        let (stream, peer) = self.listener.accept().await?;
        crate::log_tag!(info, "INB", "Accepted Unix connection from {:?}", peer);
        Ok((stream, peer))
    }
}
