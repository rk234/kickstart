mod cli;
mod cmd;
mod config;
mod exec;
mod log;
mod orchestrator;
mod registry;

use crate::{cli::Cli, config::KickstartConfig};
use clap::Parser;

async fn dispatch(cli: &Cli, config: &KickstartConfig) {
    match cli.command {
        cli::Commands::Up => cmd::up::run(config).await,
        cli::Commands::Services { json } => cmd::services::run(config, json),
    }
}

fn is_not_found(err: &dotenvy::Error) -> bool {
    matches!(err, dotenvy::Error::Io(e) if e.kind() == std::io::ErrorKind::NotFound)
}

fn main() {
    let cli = cli::Cli::parse();

    let env_path = cli.env.clone().unwrap_or_else(|| String::from(".env"));

    if let Err(e) = dotenvy::from_filename(&env_path)
        && (cli.env.is_some() || !is_not_found(&e))
    {
        eprintln!("failed to load .env at {}: {}", env_path, e);
        std::process::exit(1);
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime")
        .block_on(run(cli));
}

async fn run(cli: Cli) {
    let env = std::env::vars().collect();

    let config = match config::load_config(cli.config.as_deref(), &env) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("{}", err);
            std::process::exit(1);
        }
    };

    dispatch(&cli, &config).await;
}
