use anyhow::{Context, Result};
use clap::Parser;
use cray_core::{AppConfig, Engine, Registry};

#[derive(Parser)]
struct Args {
    #[arg(long, short)]
    config: String,

    #[arg(long)]
    check: bool,
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

    let built = AppConfig::load(&args.config).and_then(|cfg| {
        Engine::build(&cfg, &reg)
            .context("build chain")
            .map(|engine| (cfg, engine))
    });

    if args.check {
        return match built {
            Ok((cfg, _)) => {
                println!(
                    "{}: ok ({} inbound layers, {} outbound layers)",
                    args.config,
                    cfg.inbound.layers.len(),
                    cfg.outbound.layers.len()
                );
                Ok(())
            }
            Err(e) => {
                eprintln!("{}: {e:#}", args.config);
                std::process::exit(1);
            }
        };
    }

    built?.1.run().await
}
