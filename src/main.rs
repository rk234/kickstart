mod cli;
mod cmd;
mod config;
mod exec;
mod log;
mod orchestrator;

use crate::{cli::Cli, config::IgniteConfig};
use clap::Parser;

async fn dispatch(cli: &Cli, config: &IgniteConfig) {
    match cli.command {
        cli::Commands::Up => cmd::up::run(config).await,
        cli::Commands::Services => {
            for name in config.services.keys() {
                println!("{}", name);
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let cli = cli::Cli::parse();

    let config = match config::load_config(cli.config.as_deref()) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("{}", err);
            std::process::exit(1);
        }
    };

    dispatch(&cli, &config).await;
}
