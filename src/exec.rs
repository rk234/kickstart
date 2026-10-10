use std::{
    fmt::Display,
    future::pending,
    net::SocketAddr,
    process::{ExitStatus, Stdio},
    sync::Arc,
    time::Duration,
};

use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader, Lines},
    net::TcpStream,
    process::{Child, Command},
    sync::{mpsc::UnboundedSender, watch},
    time::{sleep, timeout},
};

use crate::config::{ReadyCondition, Service};

#[derive(Debug)]
pub enum SpawnError {
    ExecErr(std::io::Error),
}

impl Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExecErr(err) => write!(f, "exec error: {}", err),
        }
    }
}

impl From<std::io::Error> for SpawnError {
    fn from(value: std::io::Error) -> Self {
        Self::ExecErr(value)
    }
}

impl std::error::Error for SpawnError {}

pub fn spawn_srv(_key: &str, srv: &Service) -> Result<Child, SpawnError> {
    let child = Command::new("sh")
        .arg("-c")
        .arg(&srv.command)
        .process_group(0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    Ok(child)
}

#[derive(Debug)]
pub enum OutputStream {
    StdOut,
    StdErr,
}

#[derive(Debug)]
pub enum ServiceEvent {
    Ready(String),
    Exit(String, ExitStatus),
    Failed(String, String),
    Output(String, OutputStream, String),
}

const PORT_PROBE_TIMEOUT: Duration = Duration::from_millis(200);
const PORT_RETRY_INTERVAL: Duration = Duration::from_millis(100);

fn exited_with_code(status: ExitStatus, expected: i32) -> bool {
    status.code() == Some(expected)
}

async fn port_accepting(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    matches!(
        timeout(PORT_PROBE_TIMEOUT, TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
}

async fn await_port(port: u16) {
    while !port_accepting(port).await {
        sleep(PORT_RETRY_INTERVAL).await;
    }
}

async fn await_ready(cond: &ReadyCondition) {
    match *cond {
        ReadyCondition::Immediate => {}
        ReadyCondition::Delay(secs) => sleep(Duration::from_millis(secs)).await,
        ReadyCondition::Port(port) => await_port(port).await,
        ReadyCondition::ExitCode(_) => pending().await,
    }
}

async fn next_line<R>(lines: &mut Option<Lines<BufReader<R>>>) -> Option<String>
where
    R: AsyncRead + Unpin,
{
    match lines {
        Some(lines) => lines.next_line().await.ok().flatten(),
        None => pending().await,
    }
}

pub async fn monitor_srv(
    id: String,
    srv: Arc<Service>,
    mut proc: Child,
    tx: UnboundedSender<ServiceEvent>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut stdout = proc.stdout.take().map(|out| BufReader::new(out).lines());
    let mut stderr = proc.stderr.take().map(|err| BufReader::new(err).lines());

    let ready = await_ready(&srv.ready_condition);
    tokio::pin!(ready);

    let pgid = proc.id();
    let mut is_ready = false;
    let mut status: Option<ExitStatus> = None;
    let mut signalled = false;

    loop {
        if let Some(status) = status
            && stdout.is_none()
            && stderr.is_none()
        {
            let _ = tx.send(ServiceEvent::Exit(id, status));
            return;
        }

        let event = tokio::select! {
            line = next_line(&mut stdout) => match line {
                Some(line) => ServiceEvent::Output(id.clone(), OutputStream::StdOut, line),
                None => { stdout = None; continue; }
            },
            line = next_line(&mut stderr) => match line {
                Some(line) => ServiceEvent::Output(id.clone(), OutputStream::StdErr, line),
                None => { stderr = None; continue; }
            },
            waited = proc.wait(), if status.is_none() => {
                let waited = match waited {
                    Ok(waited) => waited,
                    // The child is unreachable, so no exit status is coming:
                    // report the failure rather than leaving the supervisor
                    // waiting on a service that can never end.
                    Err(err) => {
                        let _ = tx.send(ServiceEvent::Failed(id, err.to_string()));
                        return;
                    }
                };
                status = Some(waited);

                match srv.ready_condition {
                    ReadyCondition::ExitCode(code)
                        if !is_ready && exited_with_code(waited, code) =>
                    {
                        is_ready = true;
                        ServiceEvent::Ready(id.clone())
                    }
                    _ => continue,
                }
            },
            _ = shutdown.wait_for(|&stop| stop), if !signalled && status.is_none() => {
                signalled = true;
                if let Some(pgid) = pgid {
                    unsafe {
                        libc::killpg(pgid as i32, libc::SIGTERM);
                    }
                }
                continue;
            },
            () = &mut ready, if !is_ready => {
                is_ready = true;
                ServiceEvent::Ready(id.clone())
            }
        };

        if tx.send(event).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::TcpListener, os::unix::process::ExitStatusExt, time::Instant};
    use tokio::{sync::mpsc, task::JoinHandle};

    fn service(command: &str, ready_condition: ReadyCondition) -> Service {
        Service {
            name: None,
            command: command.to_owned(),
            ready_condition,
            depends_on: Vec::new(),
        }
    }

    async fn run(command: &str, ready_condition: ReadyCondition) -> Vec<ServiceEvent> {
        let srv = Arc::new(service(command, ready_condition));
        let proc = spawn_srv("test", &srv).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        // Held for the whole run: dropping the sender counts as a shutdown.
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);

        monitor_srv("test".to_owned(), srv, proc, tx, shutdown_rx).await;

        collect(&mut rx).await
    }

    /// Drains events until the monitor returns. The monitor owns the sender,
    /// so the channel closes when it does.
    async fn collect(rx: &mut mpsc::UnboundedReceiver<ServiceEvent>) -> Vec<ServiceEvent> {
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    }

    /// Spawns a monitored service and hands back the shutdown switch, so the
    /// test decides when (and whether) to stop it.
    fn start(
        command: &str,
    ) -> (
        mpsc::UnboundedReceiver<ServiceEvent>,
        watch::Sender<bool>,
        JoinHandle<()>,
    ) {
        let srv = Arc::new(service(command, ReadyCondition::Immediate));
        let proc = spawn_srv("test", &srv).unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let monitor = tokio::spawn(monitor_srv("test".to_owned(), srv, proc, tx, shutdown_rx));
        (rx, shutdown_tx, monitor)
    }

    /// Consumes events until the service prints `line`, so a test can wait for
    /// the shell to reach a known point before shutting it down.
    async fn wait_for_line(rx: &mut mpsc::UnboundedReceiver<ServiceEvent>, line: &str) {
        let seen = async {
            while let Some(event) = rx.recv().await {
                if let ServiceEvent::Output(_, OutputStream::StdOut, out) = event
                    && out == line
                {
                    return true;
                }
            }
            false
        };
        let seen = timeout(Duration::from_secs(5), seen).await;
        assert!(matches!(seen, Ok(true)), "service never printed {line:?}");
    }

    async fn collect_after_shutdown(
        rx: &mut mpsc::UnboundedReceiver<ServiceEvent>,
        monitor: JoinHandle<()>,
    ) -> Vec<ServiceEvent> {
        let events = timeout(Duration::from_secs(5), collect(rx))
            .await
            .expect("service outlived shutdown");
        monitor.await.unwrap();
        events
    }

    fn exit_signal(events: &[ServiceEvent]) -> Option<i32> {
        match events.last() {
            Some(ServiceEvent::Exit(_, status)) => status.signal(),
            other => panic!("expected a final Exit, got {other:?}"),
        }
    }

    fn ready_ids(events: &[ServiceEvent]) -> Vec<&str> {
        events
            .iter()
            .filter_map(|event| match event {
                ServiceEvent::Ready(id) => Some(id.as_str()),
                _ => None,
            })
            .collect()
    }

    /// A privileged port nothing can be listening on: binding it needs root,
    /// so no concurrent test can make it accept connections mid-probe. Probing
    /// a released ephemeral port instead would race whoever the kernel hands it
    /// to next.
    const CLOSED_PORT: u16 = 1;

    #[tokio::test]
    async fn port_ready_resolves_once_the_port_accepts() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        assert!(port_accepting(port).await);
        assert!(
            timeout(Duration::from_secs(5), await_port(port))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn port_ready_pends_while_nothing_listens() {
        assert!(!port_accepting(CLOSED_PORT).await);

        let waited = timeout(Duration::from_millis(300), await_port(CLOSED_PORT)).await;
        assert!(waited.is_err(), "readiness fired on a closed port");
    }

    #[test]
    fn exit_code_matches_only_the_expected_code() {
        let status = std::process::Command::new("sh")
            .args(["-c", "exit 3"])
            .status()
            .unwrap();

        assert!(exited_with_code(status, 3));
        assert!(!exited_with_code(status, 0));
    }

    #[tokio::test]
    async fn reports_output_from_both_streams_before_exit() {
        let events = run("sh -c 'echo one; echo two >&2'", ReadyCondition::Immediate).await;

        let mut lines: Vec<(&str, &str)> = events
            .iter()
            .filter_map(|event| match event {
                ServiceEvent::Output(id, _, line) => Some((id.as_str(), line.as_str())),
                _ => None,
            })
            .collect();
        lines.sort();
        assert_eq!(lines, [("test", "one"), ("test", "two")]);

        assert!(matches!(events.first(), Some(ServiceEvent::Ready(_))));
        assert!(matches!(events.last(), Some(ServiceEvent::Exit(_, _))));
    }

    fn stdout_lines(events: &[ServiceEvent]) -> Vec<&str> {
        events
            .iter()
            .filter_map(|event| match event {
                ServiceEvent::Output(_, OutputStream::StdOut, line) => Some(line.as_str()),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn commands_support_shell_sequencing() {
        let events = run(
            "echo a && echo b; false && echo skipped; echo c",
            ReadyCondition::Immediate,
        )
        .await;
        assert_eq!(stdout_lines(&events), ["a", "b", "c"]);
    }

    #[tokio::test]
    async fn service_leads_its_own_process_group() {
        let events = run("ps -o pgid= -p $$; echo $$", ReadyCondition::Immediate).await;
        let lines = stdout_lines(&events);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].trim(), lines[1].trim());
    }

    #[tokio::test]
    async fn ready_fires_once_and_only_on_a_matching_exit_code() {
        let events = run("sh -c 'exit 7'", ReadyCondition::ExitCode(7)).await;
        assert_eq!(ready_ids(&events), ["test"]);

        let events = run("sh -c 'exit 1'", ReadyCondition::ExitCode(7)).await;
        assert!(ready_ids(&events).is_empty());
        assert!(matches!(events.last(), Some(ServiceEvent::Exit(_, _))));
    }

    /// Exit and Failed are the events a monitor ends on.
    fn is_terminal(event: &ServiceEvent) -> bool {
        matches!(event, ServiceEvent::Exit(_, _) | ServiceEvent::Failed(_, _))
    }

    #[tokio::test]
    async fn every_run_reports_exactly_one_terminal_event() {
        for (command, cond) in [
            ("sh -c 'echo hi'", ReadyCondition::Immediate),
            ("sh -c 'exit 2'", ReadyCondition::ExitCode(0)),
            (
                "sh -c 'echo out; echo err >&2; exit 1'",
                ReadyCondition::Delay(10),
            ),
        ] {
            let events = run(command, cond).await;
            let terminal = events.iter().filter(|e| is_terminal(e)).count();
            assert_eq!(terminal, 1, "unexpected terminal count for `{}`", command);
            assert!(events.last().is_some_and(is_terminal));
        }
    }

    #[tokio::test]
    async fn monitor_runs_as_an_independent_spawned_task() {
        // Owning its Service and Child is what makes the monitor 'static, so
        // the supervisor can spawn one per service instead of driving them all.
        let srv = Arc::new(service("sh -c 'echo hi'", ReadyCondition::Immediate));
        let proc = spawn_srv("test", &srv).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);

        let handle = tokio::spawn(monitor_srv("test".to_owned(), srv, proc, tx, shutdown_rx));

        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        handle.await.unwrap();

        assert!(events.last().is_some_and(is_terminal));
    }

    #[tokio::test]
    async fn monitor_gives_up_once_the_receiver_is_gone() {
        let srv = Arc::new(service(
            "sh -c 'echo hi; sleep 1'",
            ReadyCondition::Immediate,
        ));
        let proc = spawn_srv("test", &srv).unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        drop(rx);
        // Kept alive so the monitor can only stop because the receiver is
        // gone, not because a dropped sender reads as a shutdown.
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);

        let monitor = monitor_srv("test".to_owned(), srv, proc, tx, shutdown_rx);
        assert!(
            timeout(Duration::from_millis(500), monitor).await.is_ok(),
            "monitor outlived its receiver"
        );
    }

    #[tokio::test]
    async fn ready_waits_for_the_delay_to_pass() {
        // The service has to outlive its delay: one that exits first reaches
        // the terminal Exit before the timer fires, and never reports ready.
        let started = Instant::now();
        let events = run("sleep 1", ReadyCondition::Delay(100)).await;
        assert_eq!(ready_ids(&events), ["test"]);

        let ready_at = events
            .iter()
            .position(|event| matches!(event, ServiceEvent::Ready(_)))
            .unwrap();
        let exit_at = events
            .iter()
            .position(|event| matches!(event, ServiceEvent::Exit(_, _)))
            .unwrap();

        assert!(ready_at < exit_at);
        assert!(started.elapsed() >= Duration::from_secs(1));
    }

    #[tokio::test]
    async fn shutdown_terminates_a_running_service() {
        let (mut rx, shutdown_tx, monitor) = start("sleep 30");

        shutdown_tx.send(true).unwrap();

        let events = collect_after_shutdown(&mut rx, monitor).await;
        assert_eq!(exit_signal(&events), Some(libc::SIGTERM));
    }

    #[tokio::test]
    async fn shutdown_reaches_the_whole_process_group() {
        // The background sleep inherits stdout, so the stream only closes (and
        // Exit only arrives) once it dies too, not just the shell above it.
        let (mut rx, shutdown_tx, monitor) = start("sleep 30 & echo started; wait");
        wait_for_line(&mut rx, "started").await;

        shutdown_tx.send(true).unwrap();

        let events = collect_after_shutdown(&mut rx, monitor).await;
        assert_eq!(exit_signal(&events), Some(libc::SIGTERM));
    }

    #[tokio::test]
    async fn shutdown_drains_output_and_reports_the_exit() {
        // A service that cleans up on SIGTERM: its last words and real exit
        // status must still come through after the signal is sent.
        let (mut rx, shutdown_tx, monitor) =
            start("trap 'echo stopping; exit 0' TERM; echo started; while :; do sleep 0.1; done");
        wait_for_line(&mut rx, "started").await;

        shutdown_tx.send(true).unwrap();

        let events = collect_after_shutdown(&mut rx, monitor).await;
        assert_eq!(stdout_lines(&events), ["stopping"]);
        assert!(matches!(
            events.last(),
            Some(ServiceEvent::Exit(_, status)) if status.success()
        ));
    }

    #[tokio::test]
    async fn dropping_the_shutdown_sender_stops_the_service() {
        // If the supervisor goes away, its services must not be left running.
        let (mut rx, shutdown_tx, monitor) = start("sleep 30");

        drop(shutdown_tx);

        let events = collect_after_shutdown(&mut rx, monitor).await;
        assert_eq!(exit_signal(&events), Some(libc::SIGTERM));
    }
}
