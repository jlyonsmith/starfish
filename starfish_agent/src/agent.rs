//! The agent's connection to the controller.
//!
//! The agent keeps one WebSocket open, applies whatever configuration arrives
//! on it, and checks in on a timer. Losing the connection is expected rather
//! than exceptional, so the loop reconnects with a backoff instead of exiting.

use crate::agent_config::AgentConfig;
use anyhow::{Context, bail};
use futures_util::{SinkExt, StreamExt};
use starfish_msg::{
    AgentMsg, ControllerMsg, Heartbeat, Hello, HostConfig, ItemReport, ProtocolErrorKind, Status,
    SyncReport,
};
use std::time::Duration;
use tokio::signal;
use tokio_websockets::{ClientBuilder, Connector, Message};

/// How long to wait before the first reconnection attempt.
const RECONNECT_DELAY: Duration = Duration::from_secs(1);

/// The ceiling the reconnect delay doubles up to.
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(60);

/// How a connection to the controller ended.
enum Outcome {
    /// It ran normally until the controller went away.
    Closed,

    /// The controller refused this agent. Reconnecting immediately would only
    /// get the same answer.
    Rejected,
}

pub struct Agent {
    config: AgentConfig,
}

impl Agent {
    pub fn new(config: &AgentConfig) -> Self {
        env_logger::builder().filter_level(config.log_level).init();

        log::info!(
            "{} v{} starting",
            env!("CARGO_PKG_DESCRIPTION"),
            env!("CARGO_PKG_VERSION"),
        );

        Agent {
            config: config.clone(),
        }
    }

    /// Stays connected to the controller until interrupted.
    pub async fn run(&self) -> anyhow::Result<()> {
        let (program, args) = self.config.sync_command();

        log::info!(
            "Controller is {}, heartbeat every {}s",
            self.config.controller_url,
            self.config.heartbeat_secs
        );
        log::info!(
            "Changes are applied by {}",
            std::iter::once(program.as_str())
                .chain(args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" ")
        );

        // Other crates in this workspace pull rustls in with more than one
        // crypto provider, which leaves it unable to pick one on its own and
        // panicking the first time a `wss://` URL is dialled. Choosing here
        // makes it explicit. It is process wide, so a second call would fail,
        // and that is not an error worth reporting.
        let _ = rustls::crypto::ring::default_provider().install_default();

        // Built once rather than per attempt: it reads the system trust store,
        // so a broken one should be an immediate startup failure rather than a
        // surprise on some later reconnect.
        let connector = Connector::new().context("Unable to set up TLS")?;

        let mut delay = RECONNECT_DELAY;

        loop {
            // The last configuration applied, so a controller that re-sends one
            // the host already has does not cause pointless work. It is reset
            // per connection because a controller restart renumbers.
            let mut applied: Option<u64> = None;

            let outcome = tokio::select! {
                _ = signal::ctrl_c() => {
                    log::info!("Stopping agent");
                    return Ok(());
                }

                outcome = self.connected(&connector, &mut applied) => outcome,
            };

            match outcome {
                Ok(Outcome::Closed) => {
                    log::info!("The controller closed the connection");
                    delay = RECONNECT_DELAY;
                }
                // The backoff deliberately keeps growing here. A key the
                // controller does not know is fixed in the database, not on
                // this host, so retrying every second would do nothing but
                // hammer the controller until somebody notices.
                Ok(Outcome::Rejected) => {}
                Err(err) => log::warn!("Disconnected from the controller: {err:#}"),
            }

            log::debug!("Reconnecting in {delay:?}");

            tokio::select! {
                _ = signal::ctrl_c() => {
                    log::info!("Stopping agent");
                    return Ok(());
                }
                _ = tokio::time::sleep(delay) => {}
            }

            delay = (delay * 2).min(MAX_RECONNECT_DELAY);
        }
    }

    /// Connects, introduces itself, then serves the connection until it drops.
    async fn connected(
        &self,
        connector: &Connector,
        applied: &mut Option<u64>,
    ) -> anyhow::Result<Outcome> {
        let url = self.config.controller_url.as_str();

        let (mut ws, _response) = ClientBuilder::new()
            .uri(url)
            .with_context(|| format!("'{url}' is not a valid controller address"))?
            .connector(connector)
            .connect()
            .await
            .with_context(|| format!("Unable to connect to {url}"))?;

        log::info!("Connected to {url}");

        let hostname = hostname();

        send(
            &mut ws,
            &AgentMsg::Hello(Hello {
                protocol_version: starfish_msg::PROTOCOL_VERSION,
                agent_key: self.config.agent_key.clone(),
                hostname: hostname.clone(),
                agent_version: env!("CARGO_PKG_VERSION").to_string(),
            }),
        )
        .await?;

        let interval = self.config.heartbeat_interval();
        let mut heartbeat = tokio::time::interval(interval);

        // The first tick is immediate, and the Hello above has just told the
        // controller we are here.
        heartbeat.tick().await;

        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    log::debug!("Sending a heartbeat, next in {interval:?}");

                    send(&mut ws, &AgentMsg::Heartbeat(Heartbeat { next_in: interval })).await?;
                }

                incoming = ws.next() => {
                    let Some(msg) = incoming else {
                        return Ok(Outcome::Closed);
                    };

                    let msg = msg.context("Unable to read from the controller")?;

                    if msg.is_close() {
                        return Ok(Outcome::Closed);
                    }

                    // Pings are answered by the WebSocket layer, so only binary
                    // frames carry anything for us.
                    if !msg.is_binary() {
                        continue;
                    }

                    let msg: ControllerMsg = starfish_msg::from_slice(msg.as_payload())
                        .context("Unable to decode a controller message")?;

                    if let Some(outcome) = self.handle(&mut ws, msg, applied).await? {
                        return Ok(outcome);
                    }
                }
            }
        }
    }

    /// Handles one controller message. `Some` means the connection should
    /// close, and says how it ended.
    async fn handle(
        &self,
        ws: &mut Socket,
        msg: ControllerMsg,
        applied: &mut Option<u64>,
    ) -> anyhow::Result<Option<Outcome>> {
        match msg {
            ControllerMsg::Config(config) => {
                if applied.is_some_and(|last| config.generation <= last) {
                    log::debug!(
                        "Ignoring generation {}, already applied {}",
                        config.generation,
                        applied.unwrap()
                    );

                    return Ok(None);
                }

                let generation = config.generation;
                let report = self.apply(config).await?;

                send(ws, &AgentMsg::SyncReport(report)).await?;

                *applied = Some(generation);

                Ok(None)
            }

            ControllerMsg::HeartbeatAck => {
                log::trace!("Heartbeat acknowledged");

                Ok(None)
            }

            ControllerMsg::Error(err) => {
                log::error!("The controller rejected us: {}", err.message);

                // The agent keeps retrying either way, so that fixing the
                // database is enough to bring the host back without anyone
                // logging into it. It just backs off first.
                let rejected = matches!(
                    err.kind,
                    ProtocolErrorKind::UnknownAgentKey | ProtocolErrorKind::UnsupportedVersion
                );

                Ok(rejected.then_some(Outcome::Rejected))
            }
        }
    }

    /// Hands the configuration to the privileged helper and waits for its
    /// report.
    ///
    /// The agent has no privileges of its own, so this is the only way it can
    /// change anything. It runs off the async runtime because the helper is a
    /// blocking child process.
    async fn apply(&self, config: HostConfig) -> anyhow::Result<SyncReport> {
        log::info!(
            "Applying generation {} with {} group(s) and {} user(s)",
            config.generation,
            config.groups.len(),
            config.users.len()
        );

        let command = self.config.sync_command();
        let bytes = starfish_msg::to_vec(&config).context("Unable to encode the configuration")?;

        let outcome = tokio::task::spawn_blocking(move || run_helper(command, bytes))
            .await
            .context("The synchronization task panicked")?;

        let report = match outcome {
            Ok(report) => report,
            Err(err) => {
                log::error!("Unable to apply generation {}: {err:#}", config.generation);

                // The controller is told which host could not be configured and
                // why, rather than simply hearing nothing back. Whoever is
                // reading its log is the person who can fix this.
                failure_report(&config, &format!("{err:#}"))
            }
        };

        let failures = report
            .groups
            .iter()
            .chain(report.users.iter())
            .filter(|item| item.status.is_failure())
            .count();

        if failures == 0 {
            log::info!("Applied generation {}", report.generation);
        } else {
            log::error!(
                "Applied generation {} with {failures} failure(s)",
                report.generation
            );
        }

        Ok(report)
    }
}

/// Runs the privileged helper, writing the configuration to its standard input
/// and reading the report from its standard output.
fn run_helper(
    (program, args): (String, Vec<String>),
    bytes: Vec<u8>,
) -> anyhow::Result<SyncReport> {
    let output = duct::cmd(&program, &args)
        .stdin_bytes(bytes)
        .stdout_capture()
        .stderr_capture()
        .unchecked()
        .run()
        .with_context(|| format!("Unable to run {program}"))?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();

    if !output.status.success() {
        let status = match output.status.code() {
            Some(code) => format!("exit code {code}"),
            None => "killed by a signal".to_string(),
        };

        if stderr.is_empty() {
            bail!("{program} failed with {status}");
        }

        bail!("{program} failed with {status}: {stderr}");
    }

    // The helper logs nothing on success, so anything here is worth passing on.
    if !stderr.is_empty() {
        log::warn!("{program}: {stderr}");
    }

    starfish_msg::from_slice(&output.stdout)
        .with_context(|| format!("Unable to decode the report from {program}"))
}

/// A report saying that nothing could be applied.
///
/// Used when the helper could not be run at all, so the controller learns that
/// the host is unconfigured instead of the failure going only to the agent's
/// own log.
fn failure_report(config: &HostConfig, message: &str) -> SyncReport {
    let failed = |name: &str| ItemReport {
        name: name.to_string(),
        status: Status::Failed {
            message: message.to_string(),
        },
    };

    SyncReport {
        hostname: config.hostname.clone(),
        generation: config.generation,
        groups: config
            .groups
            .iter()
            .map(|group| failed(&group.name))
            .collect(),
        users: config.users.iter().map(|user| failed(&user.name)).collect(),
    }
}

type Socket =
    tokio_websockets::WebSocketStream<tokio_websockets::MaybeTlsStream<tokio::net::TcpStream>>;

async fn send(ws: &mut Socket, msg: &AgentMsg) -> anyhow::Result<()> {
    let bytes = starfish_msg::to_vec(msg).context("Unable to encode a message")?;

    ws.send(Message::binary(bytes))
        .await
        .context("Unable to send to the controller")
}

/// This host's name, for the controller's logs. The controller identifies the
/// host by its agent key, so a name it cannot read is not fatal.
fn hostname() -> String {
    duct::cmd!("hostname")
        .stdout_capture()
        .stderr_capture()
        .unchecked()
        .run()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|hostname| !hostname.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}
