use clap::Parser;
use cray_core::{AppConfig, Engine, Registry};
use std::process::ExitCode;

#[derive(Parser)]
#[command(about = "Validate cray configs (and the profiles they reference) without running them")]
struct Args {
    #[arg(required = true)]
    configs: Vec<String>,
}

fn check(path: &str, reg: &Registry) -> anyhow::Result<()> {
    let cfg = AppConfig::load(path)?;
    Engine::build(&cfg, reg)?;
    Ok(())
}

fn main() -> ExitCode {
    let args = Args::parse();

    let mut reg = Registry::new();
    cray_layers::register(&mut reg);

    let mut failed = 0;
    for path in &args.configs {
        match check(path, &reg) {
            Ok(()) => println!("ok    {path}"),
            Err(e) => {
                failed += 1;
                println!("FAIL  {path}\n      {e:#}");
            }
        }
    }

    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        println!("\n{failed} of {} config(s) invalid", args.configs.len());
        ExitCode::FAILURE
    }
}
