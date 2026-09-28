mod cli;
mod config;
mod exec;
mod log;
mod orchestrator;

use std::{io::Write, sync::Arc};

use clap::Parser;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::{
    cli::Cli,
    config::IgniteConfig,
    exec::{OutputStream, ServiceEvent, monitor_srv},
    log::{Level, Logger},
    orchestrator::{EndOutcome, Orchestrator},
};

fn log_skipped<W: Write>(logger: &mut Logger<W>, srv: &str, skipped: Vec<String>) {
    for skipped in skipped {
        logger.orchestrator(
            Level::Notice,
            &format!("skipping {skipped}: dependency {srv} failed"),
        );
    }
}

fn spawn_fulfilled<W: Write>(
    orchestrator: &mut Orchestrator,
    config: &IgniteConfig,
    logger: &mut Logger<W>,
    tx: UnboundedSender<ServiceEvent>,
) {
    for srv in orchestrator.fulfilled() {
        let service = Arc::new(config.services[&srv].clone());
        let name = service.name.clone().unwrap_or(srv.clone());
        match exec::spawn_srv(&srv, &service) {
            Ok(child) => {
                logger.orchestrator(Level::Info, &format!("spawned {name} [{srv}]"));

                orchestrator.mark_running(&srv);

                let tx = tx.clone();
                tokio::spawn(async move {
                    monitor_srv(srv, service, child, tx).await;
                });
            }
            Err(e) => {
                logger.orchestrator(
                    Level::Error,
                    &format!("failed to spawn {name} [{srv}]: {e}"),
                );

                let skipped = orchestrator.mark_failed(&srv);
                log_skipped(logger, &srv, skipped);
            }
        }
    }
}

async fn dispatch(cli: &Cli, config: &IgniteConfig) {
    match cli.command {
        cli::Commands::Up => {
            let mut orchestrator = Orchestrator::new(&config.services);
            let mut logger = Logger::stdout();
            logger.register(config.services.keys().map(String::as_str));

            let (tx, mut rx): (
                UnboundedSender<ServiceEvent>,
                UnboundedReceiver<ServiceEvent>,
            ) = tokio::sync::mpsc::unbounded_channel();

            spawn_fulfilled(&mut orchestrator, config, &mut logger, tx.clone());

            while let Some(msg) = rx.recv().await {
                match msg {
                    ServiceEvent::Exit(key, status) => match orchestrator.mark_ended(&key) {
                        EndOutcome::Failed { skipped } => {
                            logger.orchestrator(
                                Level::Error,
                                &format!("{key} exited with {status} before becoming ready"),
                            );
                            log_skipped(&mut logger, &key, skipped);
                        }
                        EndOutcome::Ended if status.success() => {
                            logger.orchestrator(Level::Info, &format!("{key} exited"));
                        }
                        EndOutcome::Ended => {
                            logger
                                .orchestrator(Level::Error, &format!("{key} exited with {status}"));
                        }
                    },
                    ServiceEvent::Ready(key) => {
                        logger.orchestrator(Level::Info, &format!("{key} is ready"));
                        orchestrator.mark_ready(&key);
                    }
                    ServiceEvent::Failed(key, err) => {
                        logger.orchestrator(Level::Error, &format!("{key} failed: {err}"));
                        let skipped = orchestrator.mark_failed(&key);
                        log_skipped(&mut logger, &key, skipped);
                    }
                    ServiceEvent::Output(key, stream, line) => {
                        let level = match stream {
                            OutputStream::StdOut => Level::Info,
                            OutputStream::StdErr => Level::Notice,
                        };
                        logger.log(&key, level, &line);
                    }
                }

                spawn_fulfilled(&mut orchestrator, config, &mut logger, tx.clone());

                if orchestrator.all_ended_or_skipped() {
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
