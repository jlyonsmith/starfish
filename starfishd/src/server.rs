use crate::admin_socket;
use crate::agent_session;
use crate::controller::Controller;
use crate::server_config::ServerConfig;
use crate::tls;
use anyhow::Context;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::signal;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;

/// How long a TLS handshake may take before the connection is dropped, so a
/// client that opens a socket and stalls cannot tie up a task indefinitely.
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Server {
    config: ServerConfig,
}

impl Server {
    pub fn new(config: &ServerConfig) -> Self {
        env_logger::builder().filter_level(config.log_level).init();

        log::info!(
            "{} v{} starting",
            env!("CARGO_PKG_DESCRIPTION"),
            env!("CARGO_PKG_VERSION"),
        );

        Server {
            config: config.clone(),
        }
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        let cancel = CancellationToken::new();

        let (database_url, tls_warning) = self.config.database_url()?;

        if let Some(warning) = tls_warning {
            log::warn!("{warning}");
        }

        // Logged redacted: the URL may now be carrying a password read from a
        // file, and there is no reason for it to reach the log.
        log::info!(
            "Connecting to SQL server {}",
            starfish_db::redact(&database_url)
        );

        let db = toasty::Db::builder()
            .models(toasty::models!(starfish_db::*))
            .connect(database_url.as_ref())
            .await
            .context("Unable to connect to the database")?;

        ensure_schema(&db).await?;

        let controller = Arc::new(Controller::new(db));

        // Loaded before binding so a missing or mismatched certificate stops
        // startup rather than failing on the first agent that connects.
        let acceptor = match self.config.tls()? {
            Some((cert, key)) => Some(
                tls::acceptor(cert, key).context("Unable to set up TLS for the agent listener")?,
            ),
            None => None,
        };

        let listener = TcpListener::bind(self.config.listen)
            .await
            .with_context(|| format!("Unable to listen on {}", self.config.listen))?;

        log::info!(
            "Listening for agents on {} ({})",
            self.config.listen,
            if acceptor.is_some() {
                "wss, TLS"
            } else {
                "ws, no TLS"
            }
        );

        let admin = tokio::spawn(admin_socket::listen(
            controller.clone(),
            self.config.admin_socket.clone(),
            self.config.admin_socket_group.clone(),
            cancel.clone(),
        ));

        let agents = tokio::spawn(accept_agents(
            controller.clone(),
            listener,
            acceptor,
            cancel.clone(),
        ));

        tokio::select! {
            _ = signal::ctrl_c() => log::info!("Stopping server"),
            result = admin => report("The administration socket", result),
            result = agents => report("The agent listener", result),
        }

        cancel.cancel();

        log::info!(
            "Stopped with {} agent(s) connected",
            controller.registry().connected_count()
        );

        Ok(())
    }
}

/// Creates the schema if the database does not have it yet.
///
/// `push_schema` issues plain `CREATE TABLE` statements, so it succeeds only
/// against an empty database. Reading from a table first tells us whether this
/// is the first run, which keeps every later start from failing on a schema
/// that is already there.
///
/// This creates a missing schema; it does not migrate an existing one. A model
/// change after the first run needs the tables updated separately.
async fn ensure_schema(db: &toasty::Db) -> anyhow::Result<()> {
    let mut probe = db.clone();

    if starfish_db::Host::all()
        .first()
        .exec(&mut probe)
        .await
        .is_ok()
    {
        log::debug!("The database schema is already present");

        return Ok(());
    }

    log::info!("Creating the database schema");

    db.push_schema()
        .await
        .context("Unable to create the database schema")
}

/// Accepts agent connections until cancelled, handling each on its own task so
/// one slow handshake cannot hold up the rest.
async fn accept_agents(
    controller: Arc<Controller>,
    listener: TcpListener,
    acceptor: Option<TlsAcceptor>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),

            accepted = listener.accept() => {
                let (conn, peer) = match accepted {
                    Ok(accepted) => accepted,
                    Err(err) => {
                        log::warn!("Unable to accept an agent connection: {err}");
                        continue;
                    }
                };

                let controller = controller.clone();
                let cancel = cancel.clone();

                match acceptor.clone() {
                    // The handshake runs on the connection's own task so a slow
                    // or stalled peer cannot hold up the accept loop.
                    Some(acceptor) => {
                        tokio::spawn(serve_tls(controller, acceptor, conn, peer, cancel));
                    }
                    None => {
                        tokio::spawn(agent_session::serve(controller, conn, peer, cancel));
                    }
                }
            }
        }
    }
}

/// Completes the TLS handshake, then serves the agent over the encrypted
/// stream.
async fn serve_tls(
    controller: Arc<Controller>,
    acceptor: TlsAcceptor,
    conn: TcpStream,
    peer: std::net::SocketAddr,
    cancel: CancellationToken,
) {
    let handshake = tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(conn));

    let stream = match handshake.await {
        Ok(Ok(stream)) => stream,
        Ok(Err(err)) => {
            log::warn!("TLS handshake with {peer} failed: {err}");
            return;
        }
        Err(_) => {
            log::warn!("TLS handshake with {peer} timed out");
            return;
        }
    };

    agent_session::serve(controller, stream, peer, cancel).await;
}

/// Logs a listener task that stopped on its own, which only happens on an error
/// serious enough to bring the whole controller down.
fn report(what: &str, result: Result<anyhow::Result<()>, tokio::task::JoinError>) {
    match result {
        Ok(Ok(())) => log::info!("{what} stopped"),
        Ok(Err(err)) => log::error!("{what} failed: {err:#}"),
        Err(err) => log::error!("{what} panicked: {err}"),
    }
}
