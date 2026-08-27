use crate::controller::Controller;
use anyhow::Context;
use futures_util::{SinkExt, StreamExt};
use starfish_db::Host;
use starfish_msg::{
    AgentMsg, ControllerMsg, Heartbeat, ProtocolError, ProtocolErrorKind, SyncReport,
};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tokio_websockets::{Message, ServerBuilder, WebSocketStream};

/// How long an agent has to send its [`Hello`] before the connection is
/// dropped, so a socket that connects and says nothing cannot hold a slot.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// The longest heartbeat interval an agent may ask for. Without a ceiling a
/// broken agent could promise to check in next year and never look overdue.
const MAX_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Slack added to the interval an agent promises before its next heartbeat
/// counts as late, so ordinary scheduling jitter does not flag a healthy host.
const HEARTBEAT_GRACE: Duration = Duration::from_secs(60);

/// Runs one agent connection from the WebSocket handshake to disconnect.
///
/// The transport is whatever the listener handed over: a plain TCP stream, or a
/// TLS one when the controller is serving `wss://`.
pub async fn serve<S>(
    controller: Arc<Controller>,
    conn: S,
    peer: SocketAddr,
    cancel: CancellationToken,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let ws = match ServerBuilder::new().accept(conn).await {
        Ok((_request, ws)) => ws,
        Err(err) => {
            log::warn!("WebSocket handshake with {peer} failed: {err}");
            return;
        }
    };

    if let Err(err) = run(controller, ws, peer, cancel).await {
        log::warn!("Agent connection from {peer} ended: {err:#}");
    }
}

async fn run<S>(
    controller: Arc<Controller>,
    ws: WebSocketStream<S>,
    peer: SocketAddr,
    cancel: CancellationToken,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sink, mut stream) = ws.split();

    // Everything written to the agent goes through this channel, so the
    // registry can push configuration from another task without sharing the
    // sink.
    let (reply_tx, mut reply_rx) = mpsc::channel::<ControllerMsg>(4);

    let writer = tokio::spawn(async move {
        while let Some(msg) = reply_rx.recv().await {
            let bytes = match starfish_msg::to_vec(&msg) {
                Ok(bytes) => bytes,
                Err(err) => {
                    log::error!("Unable to encode a message for {peer}: {err}");
                    continue;
                }
            };

            if sink.send(Message::binary(bytes)).await.is_err() {
                break;
            }
        }

        let _ = sink.close().await;
    });

    let outcome = authenticate(&controller, &mut stream, &reply_tx, peer).await;

    let host = match outcome {
        Ok(host) => host,
        Err(err) => {
            // The agent gets told why before the connection goes away.
            let _ = reply_tx
                .send(ControllerMsg::Error(err.protocol_error()))
                .await;

            drop(reply_tx);
            let _ = writer.await;

            return Err(err.into());
        }
    };

    log::info!("Agent for host '{}' connected from {peer}", host.hostname);

    let (session_id, mut agent_rx) = controller.registry().register(host.id, &host.hostname);

    let result = session(
        &controller,
        &host,
        &mut stream,
        &reply_tx,
        &mut agent_rx,
        cancel,
    )
    .await;

    controller.registry().unregister(host.id, session_id);

    drop(reply_tx);
    let _ = writer.await;

    log::info!("Agent for host '{}' disconnected", host.hostname);

    result
}

/// Reads the agent's [`Hello`] and resolves it to a host.
async fn authenticate(
    controller: &Controller,
    stream: &mut (impl StreamExt<Item = Result<Message, tokio_websockets::Error>> + Unpin),
    reply_tx: &mpsc::Sender<ControllerMsg>,
    peer: SocketAddr,
) -> Result<Host, AuthError> {
    let msg = tokio::time::timeout(HELLO_TIMEOUT, next_agent_msg(stream))
        .await
        .map_err(|_| AuthError::NoHello)?
        .map_err(AuthError::Malformed)?
        .ok_or(AuthError::NoHello)?;

    let AgentMsg::Hello(hello) = msg else {
        return Err(AuthError::NotHello);
    };

    if hello.protocol_version != starfish_msg::PROTOCOL_VERSION {
        return Err(AuthError::UnsupportedVersion(hello.protocol_version));
    }

    let host = controller
        .host_for_agent_key(hello.agent_key.as_str())
        .await
        .map_err(AuthError::Lookup)?
        .ok_or_else(|| AuthError::UnknownKey(hello.hostname.clone()))?;

    if host.hostname != hello.hostname {
        // Not fatal. The key decides which host this is; a mismatch usually
        // means the machine was renamed and the database is now stale.
        log::warn!(
            "Agent at {peer} calls itself '{}' but its key belongs to host '{}'",
            hello.hostname,
            host.hostname
        );
    }

    send_config(controller, &host, reply_tx).await?;

    Ok(host)
}

/// Handles messages until the agent goes away or the server shuts down.
async fn session(
    controller: &Controller,
    host: &Host,
    stream: &mut (impl StreamExt<Item = Result<Message, tokio_websockets::Error>> + Unpin),
    reply_tx: &mpsc::Sender<ControllerMsg>,
    agent_rx: &mut mpsc::Receiver<ControllerMsg>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),

            // Configuration pushed by an administrator or by a reconnect
            // elsewhere in the controller.
            pushed = agent_rx.recv() => {
                let Some(msg) = pushed else {
                    // The registry dropped this session, which happens when a
                    // newer connection for the same host replaced it.
                    return Ok(());
                };

                if reply_tx.send(msg).await.is_err() {
                    return Ok(());
                }
            }

            incoming = next_agent_msg(stream) => {
                let Some(msg) = incoming? else {
                    return Ok(());
                };

                if !handle(controller, host, msg, reply_tx).await? {
                    return Ok(());
                }
            }
        }
    }
}

/// Handles one message from an agent. Returns `false` when the connection
/// should close.
async fn handle(
    controller: &Controller,
    host: &Host,
    msg: AgentMsg,
    reply_tx: &mpsc::Sender<ControllerMsg>,
) -> anyhow::Result<bool> {
    match msg {
        AgentMsg::Heartbeat(heartbeat) => {
            record_heartbeat(controller, host, &heartbeat).await?;

            Ok(reply_tx.send(ControllerMsg::HeartbeatAck).await.is_ok())
        }

        AgentMsg::ConfigRequest => {
            log::debug!("Host '{}' asked for its configuration", host.hostname);

            match send_config(controller, host, reply_tx).await {
                Ok(()) => Ok(true),
                Err(AuthError::Disconnected) => Ok(false),
                Err(err) => Err(err.into()),
            }
        }

        AgentMsg::SyncReport(report) => {
            log_sync_report(host, &report);

            Ok(true)
        }

        AgentMsg::Hello(_) => {
            log::warn!(
                "Host '{}' sent a second Hello, closing the connection",
                host.hostname
            );

            let _ = reply_tx
                .send(ControllerMsg::Error(ProtocolError {
                    kind: ProtocolErrorKind::NotAuthenticated,
                    message: "Already authenticated on this connection".to_string(),
                }))
                .await;

            Ok(false)
        }
    }
}

async fn send_config(
    controller: &Controller,
    host: &Host,
    reply_tx: &mpsc::Sender<ControllerMsg>,
) -> Result<(), AuthError> {
    let config = controller
        .host_config(host)
        .await
        .map_err(AuthError::Config)?;

    log::debug!(
        "Sending host '{}' generation {} with {} group(s) and {} user(s)",
        host.hostname,
        config.generation,
        config.groups.len(),
        config.users.len()
    );

    reply_tx
        .send(ControllerMsg::Config(config))
        .await
        .map_err(|_| AuthError::Disconnected)
}

/// Records that the host is alive and when its next heartbeat is due.
async fn record_heartbeat(
    controller: &Controller,
    host: &Host,
    heartbeat: &Heartbeat,
) -> anyhow::Result<()> {
    let interval = heartbeat.next_in.min(MAX_HEARTBEAT_INTERVAL);

    if interval < heartbeat.next_in {
        log::warn!(
            "Host '{}' asked for a {:?} heartbeat interval, capping it at {:?}",
            host.hostname,
            heartbeat.next_in,
            MAX_HEARTBEAT_INTERVAL
        );
    }

    let now = jiff::Timestamp::now();
    let due = now
        .checked_add(jiff::SignedDuration::try_from(interval + HEARTBEAT_GRACE)?)
        .context("Heartbeat interval overflows a timestamp")?;

    let mut db = controller.db();

    Host::update_by_id(host.id)
        .contacted_at(Some(now))
        .next_heartbeat_at(Some(due))
        .exec(&mut db)
        .await
        .context("Unable to record heartbeat")?;

    log::trace!("Heartbeat from host '{}', next due {due}", host.hostname);

    Ok(())
}

fn log_sync_report(host: &Host, report: &SyncReport) {
    let failures: Vec<_> = report
        .groups
        .iter()
        .chain(report.users.iter())
        .filter(|item| item.status.is_failure())
        .collect();

    if failures.is_empty() {
        log::info!(
            "Host '{}' applied generation {} ({} group(s), {} user(s))",
            host.hostname,
            report.generation,
            report.groups.len(),
            report.users.len()
        );

        return;
    }

    log::error!(
        "Host '{}' reported {} failure(s) applying generation {}",
        host.hostname,
        failures.len(),
        report.generation
    );

    for item in failures {
        if let starfish_msg::Status::Failed { message } = &item.status {
            log::error!("  {}: {message}", item.name);
        }
    }
}

/// Reads the next protocol message, skipping WebSocket frames that carry none.
///
/// Returns `Ok(None)` when the agent closed the connection.
async fn next_agent_msg(
    stream: &mut (impl StreamExt<Item = Result<Message, tokio_websockets::Error>> + Unpin),
) -> anyhow::Result<Option<AgentMsg>> {
    while let Some(msg) = stream.next().await {
        let msg = msg.context("Unable to read from the agent")?;

        if msg.is_close() {
            return Ok(None);
        }

        // Pings are answered by the WebSocket layer, so only binary frames
        // carry anything for us.
        if !msg.is_binary() {
            continue;
        }

        return Ok(Some(
            starfish_msg::from_slice(msg.as_payload()).context("Unable to decode agent message")?,
        ));
    }

    Ok(None)
}

/// Why a connection never became an authenticated session.
#[derive(Debug, thiserror::Error)]
enum AuthError {
    #[error("The agent did not send a Hello")]
    NoHello,

    #[error("The agent sent another message before its Hello")]
    NotHello,

    #[error("The agent speaks protocol version {0}, this controller speaks {ours}", ours = starfish_msg::PROTOCOL_VERSION)]
    UnsupportedVersion(u32),

    #[error("No host is registered with the key the agent claiming to be '{0}' sent")]
    UnknownKey(String),

    #[error(transparent)]
    Malformed(anyhow::Error),

    #[error(transparent)]
    Lookup(anyhow::Error),

    #[error(transparent)]
    Config(anyhow::Error),

    #[error("The agent disconnected")]
    Disconnected,
}

impl AuthError {
    /// The error to report to the agent. Lookup and configuration failures are
    /// the controller's problem, so the agent is told only that, not the
    /// internals.
    fn protocol_error(&self) -> ProtocolError {
        let kind = match self {
            AuthError::NoHello | AuthError::NotHello | AuthError::Disconnected => {
                ProtocolErrorKind::NotAuthenticated
            }
            AuthError::UnsupportedVersion(_) => ProtocolErrorKind::UnsupportedVersion,
            AuthError::UnknownKey(_) => ProtocolErrorKind::UnknownAgentKey,
            AuthError::Malformed(_) => ProtocolErrorKind::Malformed,
            AuthError::Lookup(_) | AuthError::Config(_) => ProtocolErrorKind::Internal,
        };

        let message = match self {
            AuthError::Lookup(_) | AuthError::Config(_) => {
                "The controller was unable to build this host's configuration".to_string()
            }
            other => other.to_string(),
        };

        ProtocolError { kind, message }
    }
}
