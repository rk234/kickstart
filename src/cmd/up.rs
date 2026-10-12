use std::{io::Write, time::Duration};

use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    watch,
};

use crate::{
    config::KickstartConfig,
    exec::{self, OutputStream, ServiceEvent, monitor_srv},
    log::{Level, Logger},
    orchestrator::{EndOutcome, Orchestrator},
    registry::{Registry, ServiceId},
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

    let registry = Registry::new(config);
    let mut orchestrator = Orchestrator::new(&registry);
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
        &registry,
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
            ServiceEvent::Exit(id, status) => {
                let key = registry.id(id);
                match orchestrator.mark_ended(id) {
                    EndOutcome::Failed { skipped } => {
                        logger.orchestrator(
                            Level::Error,
                            &format!("{key} exited with {status} before becoming ready"),
                        );
                        log_skipped(&mut logger, &registry, key, skipped);
                    }
                    EndOutcome::Ended if status.success() => {
                        logger.orchestrator(Level::Info, &format!("{key} exited"));
                    }
                    EndOutcome::Ended => {
                        logger.orchestrator(Level::Error, &format!("{key} exited with {status}"));
                    }
                }
            }
            ServiceEvent::Ready(id) => {
                let key = registry.id(id);
                logger.orchestrator(Level::Info, &format!("{key} is ready"));
                orchestrator.mark_ready(id);
            }
            ServiceEvent::Failed(id, err) => {
                let key = registry.id(id);
                logger.orchestrator(Level::Error, &format!("{key} failed: {err}"));
                let skipped = orchestrator.mark_failed(id);
                log_skipped(&mut logger, &registry, key, skipped);
            }
            ServiceEvent::Output(id, stream, line) => {
                let level = match stream {
                    OutputStream::StdOut => Level::Info,
                    OutputStream::StdErr => Level::Notice,
                };
                logger.log(registry.id(id), level, &line);
            }
        }

        if !shutting_down {
            spawned.extend(spawn_fulfilled(
                &mut orchestrator,
                &registry,
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

fn log_skipped<W: Write>(
    logger: &mut Logger<W>,
    registry: &Registry,
    srv: &str,
    skipped: Vec<ServiceId>,
) {
    for skipped in skipped {
        let skipped = registry.id(skipped);
        logger.orchestrator(
            Level::Notice,
            &format!("skipping {skipped}: dependency {srv} failed"),
        );
    }
}

fn spawn_fulfilled<W: Write>(
    orchestrator: &mut Orchestrator,
    registry: &Registry,
    logger: &mut Logger<W>,
    tx: UnboundedSender<ServiceEvent>,
    shutdown_rx: watch::Receiver<bool>,
) -> Vec<(ServiceId, u32)> {
    let mut spawned = Vec::new();
    for id in orchestrator.fulfilled() {
        let srv = registry.id(id);
        let service = registry.service(id);
        let name = service.name.as_deref().unwrap_or(srv);
        match exec::spawn_srv(service) {
            Ok(child) => {
                logger.orchestrator(Level::Info, &format!("spawned {name} [{srv}]"));

                orchestrator.mark_running(id);

                if let Some(pid) = child.id() {
                    spawned.push((id, pid));
                }
                tokio::spawn(monitor_srv(
                    id,
                    service.ready_condition,
                    child,
                    tx.clone(),
                    shutdown_rx.clone(),
                    SHUTDOWN_GRACE_PERIOD,
                ));
            }
            Err(e) => {
                logger.orchestrator(
                    Level::Error,
                    &format!("failed to spawn {name} [{srv}]: {e}"),
                );

                let skipped = orchestrator.mark_failed(id);
                log_skipped(logger, registry, srv, skipped);
            }
        }
    }

    spawned
}
