use std::{io::Write, sync::Arc, time::Duration};

use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    watch,
};

use crate::{
    config::KickstartConfig,
    exec::{self, OutputStream, ServiceEvent, monitor_srv},
    log::{Level, Logger},
    orchestrator::{EndOutcome, Orchestrator},
};

const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(10);

pub async fn run(config: &KickstartConfig) {
    let mut logger = Logger::stdout();
    let warns = config.validate();
    if !warns.is_empty() {
        for w in warns {
            logger.log("error", Level::Error, w.to_string().as_str());
        }
        return;
    }

    let mut orchestrator = Orchestrator::new(&config.services);
    let mut shutting_down = false;

    logger.register(config.services.keys().map(String::as_str));

    let (event_tx, mut event_rx): (
        UnboundedSender<ServiceEvent>,
        UnboundedReceiver<ServiceEvent>,
    ) = tokio::sync::mpsc::unbounded_channel();

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let mut spawned = Vec::new();
    spawned.extend(spawn_fulfilled(
        &mut orchestrator,
        config,
        &mut logger,
        event_tx.clone(),
        shutdown_rx.clone(),
    ));

    loop {
        let msg = tokio::select! {
            msg = event_rx.recv() => match msg {
                Some(msg) => msg,
                None => break
            },
            _ = tokio::signal::ctrl_c() => {
                shutting_down = true;
                match shutdown_tx.send(true) {
                    Ok(_) => {
                        orchestrator.skip_pending();
                        logger.orchestrator(Level::Notice, "shutting down");
                    },
                    Err(_) => {
                        logger.orchestrator(Level::Error, "failed to send shutdown signal!");
                    }
                };
                continue;
            }
        };

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
                    logger.orchestrator(Level::Error, &format!("{key} exited with {status}"));
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

        if !shutting_down {
            spawned.extend(spawn_fulfilled(
                &mut orchestrator,
                config,
                &mut logger,
                event_tx.clone(),
                shutdown_rx.clone(),
            ));
        }

        if orchestrator.all_ended_or_skipped() {
            break;
        }
    }
}

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
    config: &KickstartConfig,
    logger: &mut Logger<W>,
    tx: UnboundedSender<ServiceEvent>,
    shutdown_rx: watch::Receiver<bool>,
) -> Vec<(String, u32)> {
    let mut spawned = Vec::new();
    for srv in orchestrator.fulfilled() {
        let service = Arc::new(config.services[&srv].clone());
        let name = service.name.clone().unwrap_or(srv.clone());
        match exec::spawn_srv(&srv, &service) {
            Ok(child) => {
                logger.orchestrator(Level::Info, &format!("spawned {name} [{srv}]"));

                orchestrator.mark_running(&srv);

                if let Some(pid) = child.id() {
                    spawned.push((srv.clone(), pid));
                }
                let tx = tx.clone();
                let shutdown_rx = shutdown_rx.clone();
                tokio::spawn(async move {
                    monitor_srv(srv, service, child, tx, shutdown_rx, SHUTDOWN_GRACE_PERIOD).await;
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

    spawned
}
