//! `IPC` request handling for the daemon
//!
//! Connections are served by their own tasks; they ask the event loop for a
//! [`Snapshot`] or hand it a switch [`Command`], so a slow client never stalls the
//! loop and the loop never blocks on a client.

use std::sync::Arc;
use std::time::Duration;

use color_eyre::eyre::Result;
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{error, info};

use super::event_loop::{Command, ManualSwitch, PauseRequest, Snapshot};
use crate::daemon_manager::DaemonManager;
use crate::ipc::{self, IpcServer, Request, Response, WindowInfo};

/// How long a switch request waits for the activation before replying "in progress".
/// Below the client's 10 s read timeout.
pub(crate) const SWITCH_REPLY_TIMEOUT: Duration = Duration::from_secs(8);
/// Pause after a failed `accept` (e.g. out of file descriptors) to avoid spinning
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(100);

/// Facts about the daemon that never change while it runs
#[derive(Debug, Clone)]
pub(crate) struct ServerInfo {
    pub version: String,
    pub started: std::time::Instant,
    pub daemon_manager: DaemonManager,
}

/// Serve `IPC` connections until the returned task is aborted (which drops the
/// server and removes the socket)
pub(crate) fn spawn(
    server: IpcServer,
    commands: mpsc::Sender<Command>,
    info: Arc<ServerInfo>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let Some(stream) = server.accept().await else {
                tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                continue;
            };
            let commands = commands.clone();
            let info = Arc::clone(&info);
            tokio::spawn(async move {
                if let Err(e) = handle_connection(stream, &commands, &info).await {
                    // Health checks connect and hang up without sending anything
                    let is_health_check = e.chain().any(|cause| {
                        cause
                            .downcast_ref::<std::io::Error>()
                            .is_some_and(|io| io.kind() == std::io::ErrorKind::UnexpectedEof)
                    });
                    if !is_health_check {
                        error!("IPC request handling error: {e:#}");
                    }
                }
            });
        }
    })
}

/// Read one request, answer it
///
/// # Errors
/// Returns an error if reading the request or writing the response fails.
pub(crate) async fn handle_connection(
    mut stream: UnixStream,
    commands: &mpsc::Sender<Command>,
    info: &ServerInfo,
) -> Result<()> {
    let request = match ipc::read_request(&mut stream).await? {
        Ok(request) => request,
        Err(e) => {
            // Most likely a newer client talking to this (older) daemon: tell it so
            // instead of dropping the connection
            info!("Unsupported IPC request: {e}");
            let reply = ipc::unsupported_request(&info.version, &e);
            return ipc::write_response(&mut stream, &reply).await;
        }
    };

    if matches!(request, Request::Shutdown) {
        info!("Shutdown requested via IPC");
        ipc::write_response(&mut stream, &Response::Ok {
            message: "Daemon shutting down...".to_string(),
        })
        .await?;
        let _ = commands.send(Command::Shutdown).await;
        return Ok(());
    }

    let response = respond(request, commands, info).await;
    ipc::write_response(&mut stream, &response).await
}

fn shutting_down() -> Response {
    Response::Error {
        message: "Daemon is shutting down".to_string(),
    }
}

async fn query(commands: &mpsc::Sender<Command>) -> Option<Snapshot> {
    let (tx, rx) = oneshot::channel();
    commands.send(Command::Query(tx)).await.ok()?;
    rx.await.ok()
}

async fn switch(commands: &mpsc::Sender<Command>, request: ManualSwitch) -> Response {
    let (reply, rx) = oneshot::channel();
    if commands
        .send(Command::Switch { request, reply })
        .await
        .is_err()
    {
        return shutting_down();
    }
    match tokio::time::timeout(SWITCH_REPLY_TIMEOUT, rx).await {
        Ok(Ok(response)) => response,
        Ok(Err(_)) => shutting_down(),
        Err(_) => Response::Ok {
            message: "Switch in progress (taking longer than expected)".to_string(),
        },
    }
}

async fn set_paused(commands: &mpsc::Sender<Command>, request: PauseRequest) -> Response {
    let (reply, rx) = oneshot::channel();
    if commands
        .send(Command::Pause { request, reply })
        .await
        .is_err()
    {
        return shutting_down();
    }
    rx.await.unwrap_or_else(|_| shutting_down())
}

async fn respond(
    request: Request,
    commands: &mpsc::Sender<Command>,
    info: &ServerInfo,
) -> Response {
    match request {
        Request::GetManagerInfo => Response::ManagerInfo {
            daemon_manager: info.daemon_manager,
        },
        Request::SetSink { sink } => switch(commands, ManualSwitch::Set(sink)).await,
        Request::NextSink => switch(commands, ManualSwitch::Next).await,
        Request::PrevSink => switch(commands, ManualSwitch::Prev).await,
        Request::SetSource { source } => switch(commands, ManualSwitch::SetSource(source)).await,
        Request::Pause => set_paused(commands, PauseRequest::Pause).await,
        Request::Resume => set_paused(commands, PauseRequest::Resume).await,
        Request::TogglePause => set_paused(commands, PauseRequest::Toggle).await,
        // Answered (before shutting down) by `handle_connection`
        Request::Shutdown => Response::Ok {
            message: "Daemon shutting down...".to_string(),
        },
        Request::Status => match query(commands).await {
            Some(snapshot) => status_response(&snapshot, info),
            None => shutting_down(),
        },
        Request::ListWindows => match query(commands).await {
            Some(snapshot) => Response::Windows {
                windows: snapshot.windows,
            },
            None => shutting_down(),
        },
        Request::TestRule { pattern } => match query(commands).await {
            Some(snapshot) => test_rule_response(&snapshot.windows, pattern),
            None => shutting_down(),
        },
    }
}

fn status_response(snapshot: &Snapshot, info: &ServerInfo) -> Response {
    // Only a sink pwsw applied or saw: the rules' wish is not reported as applied
    let current_sink = snapshot.applied.as_deref().map(|current| {
        snapshot
            .config
            .sinks
            .iter()
            .find(|s| s.name == current)
            .map_or_else(|| current.to_string(), |s| s.desc.clone())
    });
    let current_source = snapshot.applied_source.as_deref().map(|current| {
        snapshot
            .config
            .endpoint_desc(crate::config::Endpoint::Source, current)
    });
    Response::Status {
        version: info.version.clone(),
        protocol: ipc::PROTOCOL_VERSION,
        uptime_secs: info.started.elapsed().as_secs(),
        current_sink_fallback: current_sink.is_some() && snapshot.fallback,
        current_sink,
        active_window: snapshot.active_window.clone(),
        tracked_windows: snapshot.tracked_windows,
        paused: snapshot.paused,
        current_source_fallback: current_source.is_some() && snapshot.source_fallback,
        current_source,
        sources_configured: snapshot.sources_configured,
    }
}

fn test_rule_response(windows: &[WindowInfo], pattern: String) -> Response {
    let regex = match crate::config::compile_rule_regex(&pattern) {
        Ok(regex) => regex,
        Err(e) => {
            return Response::Error {
                message: format!("Invalid regex pattern: {e}"),
            };
        }
    };
    let matches = windows
        .iter()
        .filter_map(|w| {
            let matched_on = match (regex.is_match(&w.app_id), regex.is_match(&w.title)) {
                (true, true) => "both",
                (true, false) => "app_id",
                (false, true) => "title",
                (false, false) => return None,
            };
            Some(WindowInfo {
                id: w.id,
                app_id: w.app_id.clone(),
                title: w.title.clone(),
                matched_on: Some(matched_on.to_string()),
                // The rule the window matches now, so the output shows its sinks
                tracked: w.tracked.clone(),
            })
        })
        .collect();
    Response::RuleMatches { pattern, matches }
}
