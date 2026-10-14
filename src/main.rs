use anyhow::{Result, anyhow, bail};
use clap::Parser;
use cray::config::appconfig::{LayerConfig, Role};
use cray::config::loader::{get_config, load};
use cray::config::validate::validate;
use cray::inbound::{tcp::Tcp, unix::Unix};
use cray::transport::BoxedStream;
use cray::{Listener, chain, log_tag};
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    #[arg(short, long, default_value = "config.toml")]
    pub config: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    load(&args.config)?;
    cray::logging::init_logging();

    let cfg = get_config();
    validate(cfg)?;

    log_tag!(info, "CFG", "Role: {:?}", cfg.role);
    log_tag!(info, "CFG", "Inbound: {:#?}", cfg.inbound.layers);
    log_tag!(info, "CFG", "Outbound: {:#?}", cfg.outbound.layers);

    let (first, rest) = cfg
        .inbound
        .layers
        .split_first()
        .ok_or_else(|| anyhow!("inbound.layers is empty"))?;

    let transports: &'static [LayerConfig] = match cfg.role {
        Role::Server => &rest[..rest.len() - 1],
        Role::Client => rest,
    };

    match first {
        LayerConfig::Tcp { addr, .. } => {
            let addr = addr
                .as_deref()
                .ok_or_else(|| anyhow!("tcp inbound needs addr"))?;
            serve(Tcp::bind(addr).await?, transports, addr).await
        }
        LayerConfig::Unix { addr, .. } => {
            let addr = addr
                .as_deref()
                .ok_or_else(|| anyhow!("unix inbound needs addr"))?;
            serve(Unix::bind(addr)?, transports, addr).await
        }
        _ => bail!("first inbound layer must be tcp or unix"),
    }
}

async fn serve<L: Listener>(
    listener: L,
    transports: &'static [LayerConfig],
    addr: &str,
) -> Result<()>
where
    L::Stream: Sync,
{
    log_tag!(info, "INB", "Listening on {}", addr);
    loop {
        let (stream, _peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                log_tag!(warn, "INB", "accept failed: {}", e);
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        tokio::spawn(async move {
            let s: BoxedStream = Box::pin(stream);
            if let Err(e) = chain::run(transports, s).await {
                log_tag!(error, "CHAIN", "{}", e);
            }
        });
    }
}
