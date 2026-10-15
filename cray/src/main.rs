use anyhow::{Context, Result};
use clap::Parser;
use cray_core::{AppConfig, Engine, Registry};

#[derive(Parser)]
struct Args {
    #[arg(long, short)]
    config: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();

    let mut reg = Registry::new();
    cray_layers::register(&mut reg);

    let cfg = AppConfig::load(&args.config)?;
    let engine = Engine::build(&cfg, &reg).context("build chain")?;
    engine.run().await
}
