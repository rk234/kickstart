mod cli;
mod config;
mod exec;
mod orchestrator;

use std::sync::Arc;

use clap::Parser;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::{
    cli::Cli,
    config::IgniteConfig,
    exec::{ServiceEvent, monitor_srv},
    orchestrator::Orchestrator,
    // exec::{monitor_srv, spawn_srv},
};

fn spawn_fulfilled(
    orchestrator: &mut Orchestrator,
    config: &IgniteConfig,
    tx: UnboundedSender<ServiceEvent>,
) {
    for srv in orchestrator.fulfilled() {
        let service = Arc::new(config.services[&srv].clone());
        match exec::spawn_srv(&srv, &service) {
            Ok(child) => {
                println!(
                    "[orchestrator]: spawned {} [{}]",
                    service.name.clone().unwrap_or(srv.clone()),
                    srv
                );

                orchestrator.mark_running(&srv);

                let tx = tx.clone();
                tokio::spawn(async move {
                    monitor_srv(srv, service, child, tx).await;
                });
            }
            Err(e) => eprintln!(
                "[orchestrator]: failed to spawn {} [{}]: {}",
                service.name.clone().unwrap_or(srv.clone()),
                srv,
                e
            ),
        }
    }
}

async fn dispatch(cli: &Cli, config: &IgniteConfig) {
    match cli.command {
        cli::Commands::Up => {
            let mut orchestrator = orchestrator::Orchestrator::new(&config.services);

            let (tx, mut rx): (
                UnboundedSender<ServiceEvent>,
                UnboundedReceiver<ServiceEvent>,
            ) = tokio::sync::mpsc::unbounded_channel();

            spawn_fulfilled(&mut orchestrator, config, tx.clone());

            while let Some(msg) = rx.recv().await {
                println!("msg: {:?}", msg);

                match msg {
                    ServiceEvent::Exit(key, _) => {
                        orchestrator.mark_ended(&key);
                    }
                    ServiceEvent::Ready(key) => {
                        orchestrator.mark_ready(&key);
                    }
                    ServiceEvent::Failed(key, _) => {
                        orchestrator.mark_ended(&key);
                    }
                    ServiceEvent::Output(_, _, _) => {}
                }

                spawn_fulfilled(&mut orchestrator, config, tx.clone());

                if orchestrator.all_ended() {
                    break;
                }
            }
        }
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
