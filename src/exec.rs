use std::{
    fmt::Display,
    future::pending,
    net::SocketAddr,
    process::{ExitStatus, Stdio},
    sync::mpsc::Sender,
    time::Duration,
};

use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader, Lines},
    net::TcpStream,
    process::{Child, Command},
    time::{sleep, timeout},
};

use crate::config::{ReadyCondition, Service};

impl Service {
    fn create_command_args(&self) -> Option<(String, Vec<String>)> {
        let toks = shlex::split(&self.command)?;
        let (prog, args) = toks.split_first()?;

        Some((prog.to_owned(), args.to_owned()))
    }
}

#[derive(Debug)]
enum SpawnError {
    CmdParseErr,
    ExecErr(std::io::Error),
}

impl Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CmdParseErr => write!(f, "command parsing error"),
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

pub fn spawn_srv(key: &str, srv: &Service) -> Result<Child, SpawnError> {
    let (prog, args) = srv.create_command_args().ok_or(SpawnError::CmdParseErr)?;
    let child = Command::new(prog)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    Ok(child)
}

#[derive(Debug)]
enum ServiceEvent {
    Ready(String),
    Exit(String, ExitStatus),
    Output(String, String),
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

pub async fn monitor_srv(id: String, srv: &Service, proc: &mut Child, tx: Sender<ServiceEvent>) {
    let mut stdout = proc.stdout.take().map(|out| BufReader::new(out).lines());
    let mut stderr = proc.stderr.take().map(|err| BufReader::new(err).lines());

    let ready = await_ready(&srv.ready_condition);
    tokio::pin!(ready);

    let mut is_ready = false;
    let mut status: Option<ExitStatus> = None;

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
                Some(line) => ServiceEvent::Output(id.clone(), line),
                None => { stdout = None; continue; }
            },
            line = next_line(&mut stderr) => match line {
                Some(line) => ServiceEvent::Output(id.clone(), line),
                None => { stderr = None; continue; }
            },
            waited = proc.wait(), if status.is_none() => {
                let Ok(waited) = waited else { return };
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
            }
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
    use std::{net::TcpListener, sync::mpsc, time::Instant};

    fn service(command: &str, ready_condition: ReadyCondition) -> Service {
        Service {
            name: None,
            command: command.to_owned(),
            ready_condition,
            depends_on: None,
        }
    }

    async fn run(command: &str, ready_condition: ReadyCondition) -> Vec<ServiceEvent> {
        let srv = service(command, ready_condition);
        let mut proc = spawn_srv("test", &srv).unwrap();
        let (tx, rx) = mpsc::channel();
        monitor_srv("test".to_owned(), &srv, &mut proc, tx).await;
        rx.into_iter().collect()
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
                ServiceEvent::Output(id, line) => Some((id.as_str(), line.as_str())),
                _ => None,
            })
            .collect();
        lines.sort();
        assert_eq!(lines, [("test", "one"), ("test", "two")]);

        assert!(matches!(events.first(), Some(ServiceEvent::Ready(_))));
        assert!(matches!(events.last(), Some(ServiceEvent::Exit(_, _))));
    }

    #[tokio::test]
    async fn ready_fires_once_and_only_on_a_matching_exit_code() {
        let events = run("sh -c 'exit 7'", ReadyCondition::ExitCode(7)).await;
        assert_eq!(ready_ids(&events), ["test"]);

        let events = run("sh -c 'exit 1'", ReadyCondition::ExitCode(7)).await;
        assert!(ready_ids(&events).is_empty());
        assert!(matches!(events.last(), Some(ServiceEvent::Exit(_, _))));
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
}
