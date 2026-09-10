//! Drives the real `starfishd` binary the way an agent and the admin tool do.
//!
//! These tests need a PostgreSQL database, named by `STARFISH_TEST_DATABASE_URL`.
//! They skip when it is unset. **The database is dropped and recreated**, so
//! point the variable at one kept for testing:
//!
//! ```text
//! STARFISH_TEST_DATABASE_URL=postgresql://postgres@localhost:5432/starfish_test \
//!     cargo test -p starfishd --test end_to_end
//! ```

use futures_util::{SinkExt, StreamExt};
use starfish_db::{Host, HostGroup, HostGroupUser, SshKey, User};
use starfish_msg::{
    AdminRequest, AdminResponse, AgentKey, AgentMsg, ControllerMsg, ControllerMsg::HeartbeatAck,
    Heartbeat, Hello, HostConfig, ItemReport, ProtocolErrorKind, Status, SyncReport, frame,
};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio::net::{TcpStream, UnixStream};
use tokio_websockets::{ClientBuilder, MaybeTlsStream, Message, WebSocketStream};

const HOSTNAME: &str = "web-1";
const PORT: u16 = 19611;

/// Long enough for a loaded machine to get the controller listening, short
/// enough that a genuine failure does not hang the suite.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

type Agent = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The whole flow lives in one test because `starfishd` refuses to run twice at
/// once, so two tests each starting a controller would fight over the lock.
#[tokio::test]
async fn controller_serves_agents_and_admin_commands() {
    let Some(database_url) = std::env::var("STARFISH_TEST_DATABASE_URL").ok() else {
        eprintln!("skipping: STARFISH_TEST_DATABASE_URL is not set");
        return;
    };

    let (agent_key, mut db) = seed(&database_url).await;

    let socket = std::env::temp_dir().join(format!("starfish-test-{}.sock", std::process::id()));
    let mut controller = start(&database_url, &socket);

    let result = run_checks(&agent_key, &socket, &mut db).await;

    let _ = controller.kill();
    let _ = controller.wait();

    result.unwrap();
}

async fn run_checks(
    agent_key: &AgentKey,
    socket: &std::path::Path,
    db: &mut toasty::Db,
) -> anyhow::Result<()> {
    // An unrecognised key is refused rather than silently ignored.
    let mut stranger = connect().await;
    hello(&mut stranger, &AgentKey::generate()?).await;

    match recv(&mut stranger).await {
        ControllerMsg::Error(err) => assert_eq!(err.kind, ProtocolErrorKind::UnknownAgentKey),
        other => panic!("expected an unknown key error, got {other:?}"),
    }

    // A recognised key gets the host's configuration without having to ask.
    let mut agent = connect().await;
    hello(&mut agent, agent_key).await;

    let config = expect_config(&mut agent).await;

    check_config(&config);

    // Heartbeats are acknowledged.
    send(
        &mut agent,
        &AgentMsg::Heartbeat(Heartbeat {
            next_in: Duration::from_secs(300),
        }),
    )
    .await;

    assert!(matches!(recv(&mut agent).await, HeartbeatAck));

    // The ack alone does not prove the host's liveness was recorded, so check
    // the row the controller was supposed to update.
    let host = Host::get_by_hostname(db, HOSTNAME).await?;
    let contacted = host.contacted_at.expect("no contact time was recorded");
    let due = host
        .next_heartbeat_at
        .expect("no next heartbeat was recorded");

    assert_eq!(
        due.as_second() - contacted.as_second(),
        360,
        "the next heartbeat should be due the promised 300s later, plus the 60s grace"
    );

    // A sync report is accepted, including one carrying failures.
    send(
        &mut agent,
        &AgentMsg::SyncReport(SyncReport {
            hostname: HOSTNAME.to_string(),
            generation: config.generation,
            groups: vec![ItemReport {
                name: "developers".to_string(),
                status: Status::Created,
            }],
            users: vec![ItemReport {
                name: "ada".to_string(),
                status: Status::Failed {
                    message: "useradd exited with 1".to_string(),
                },
            }],
        }),
    )
    .await;

    // An agent can ask for its configuration again.
    send(&mut agent, &AgentMsg::ConfigRequest).await;

    let again = expect_config(&mut agent).await;

    check_config(&again);
    assert!(
        again.generation > config.generation,
        "a later push must carry a later generation, got {} then {}",
        config.generation,
        again.generation
    );

    // The admin socket pushes a fresh configuration to the connected agent.
    let mut admin = UnixStream::connect(socket).await?;

    frame::write(
        &mut admin,
        &AdminRequest::Refresh {
            hostname: Some(HOSTNAME.to_string()),
        },
    )
    .await?;

    let response: AdminResponse = frame::read(&mut admin).await?.expect("no admin response");

    match response {
        AdminResponse::Refreshed { notified, offline } => {
            assert_eq!(notified, vec![HOSTNAME.to_string()]);
            assert!(offline.is_empty(), "unexpected offline hosts: {offline:?}");
        }
        other => panic!("expected a refresh, got {other:?}"),
    }

    let pushed = expect_config(&mut agent).await;

    check_config(&pushed);

    // Refreshing an unknown host is an error, not a silent success.
    frame::write(
        &mut admin,
        &AdminRequest::Refresh {
            hostname: Some("not-a-host".to_string()),
        },
    )
    .await?;

    let response: AdminResponse = frame::read(&mut admin).await?.expect("no admin response");

    assert!(
        matches!(response, AdminResponse::Error { .. }),
        "expected an error, got {response:?}"
    );

    Ok(())
}

/// Checks the configuration against what `seed` put in the database.
fn check_config(config: &HostConfig) {
    assert_eq!(config.hostname, HOSTNAME);

    let groups: Vec<&str> = config.groups.iter().map(|g| g.name.as_str()).collect();

    assert_eq!(groups, vec!["deploy", "developers"]);

    let users: Vec<&str> = config.users.iter().map(|u| u.name.as_str()).collect();

    assert_eq!(users, vec!["ada", "jls"]);

    let ada = &config.users[0];

    assert!(!ada.is_sudoer);
    assert_eq!(ada.groups, vec!["developers"]);
    assert!(ada.ssh_keys.is_empty());

    let jls = &config.users[1];

    assert!(jls.is_sudoer);
    assert_eq!(jls.groups, vec!["deploy", "developers"]);
    assert_eq!(jls.full_name, "John Lyon-Smith");
    assert_eq!(jls.ssh_keys.len(), 1);
    assert_eq!(jls.ssh_keys[0].name, "laptop");
}

/// Rebuilds the database and fills it with one host group, two users and one
/// host. Returns the host's agent key.
async fn seed(database_url: &str) -> (AgentKey, toasty::Db) {
    // `reset_db` drops and recreates the database, which closes the pooled
    // connection the handle that issued it is holding, so the seeding below
    // needs a handle opened afterwards.
    let reset = toasty::Db::builder()
        .models(toasty::models!(starfish_db::*))
        .connect(database_url)
        .await
        .expect("unable to connect to the test database");

    reset
        .reset_db()
        .await
        .expect("unable to reset the database");

    drop(reset);

    let mut db = toasty::Db::builder()
        .models(toasty::models!(starfish_db::*))
        .connect(database_url)
        .await
        .expect("unable to reconnect to the test database");

    db.push_schema().await.expect("unable to create the schema");

    let group = HostGroup::create()
        .name("web")
        .exec(&mut db)
        .await
        .expect("unable to create the host group");

    let jls = User::create()
        .alias("jls")
        .email("john@lyon-smith.org")
        .first_name("John")
        .last_name("Lyon-Smith")
        .exec(&mut db)
        .await
        .expect("unable to create a user");

    let ada = User::create()
        .alias("ada")
        .email("ada@example.com")
        .first_name("Ada")
        .last_name("Lovelace")
        .exec(&mut db)
        .await
        .expect("unable to create a user");

    SshKey::create()
        .user_id(jls.id)
        .name("laptop")
        .key("ssh-ed25519 AAAAC3Nz-jls-laptop")
        .exec(&mut db)
        .await
        .expect("unable to create an SSH key");

    // jls is in both groups, ada only in developers, so the test can tell that
    // memberships are per user rather than per host group, and that the groups
    // the host is sent are the union of the two.
    for (user, is_sudoer, security_groups) in [
        (
            &jls,
            true,
            vec!["developers".to_string(), "deploy".to_string()],
        ),
        (&ada, false, vec!["developers".to_string()]),
    ] {
        HostGroupUser::create()
            .host_group_id(group.id)
            .user_id(user.id)
            .is_sudoer(is_sudoer)
            .security_groups(security_groups)
            .exec(&mut db)
            .await
            .expect("unable to add a user to the host group");
    }

    let agent_key = AgentKey::generate().expect("unable to generate an agent key");

    Host::create()
        .host_group_id(group.id)
        .hostname(HOSTNAME)
        .info("test host")
        .agent_key(agent_key.as_str())
        .exec(&mut db)
        .await
        .expect("unable to create the host");

    (agent_key, db)
}

fn start(database_url: &str, socket: &std::path::Path) -> Child {
    // A config file that does not exist, so the test never picks up whatever is
    // installed at the default path on this machine.
    let config = std::env::temp_dir().join("starfish-test-no-such.conf");

    Command::new(env!("CARGO_BIN_EXE_starfishd"))
        .arg("--config")
        .arg(&config)
        .arg("--sql-server")
        .arg(database_url)
        .arg("--listen")
        .arg(format!("127.0.0.1:{PORT}"))
        .arg("--admin-socket")
        .arg(socket)
        .arg("--log-level")
        .arg("debug")
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("unable to start starfishd")
}

/// Connects as an agent would, waiting for the controller to come up.
async fn connect() -> Agent {
    let deadline = std::time::Instant::now() + STARTUP_TIMEOUT;

    loop {
        let attempt = ClientBuilder::new()
            .uri(&format!("ws://127.0.0.1:{PORT}"))
            .expect("bad uri")
            .connect()
            .await;

        match attempt {
            Ok((agent, _response)) => return agent,
            Err(err) if std::time::Instant::now() < deadline => {
                let _ = err;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(err) => panic!("unable to connect to the controller: {err}"),
        }
    }
}

async fn hello(agent: &mut Agent, agent_key: &AgentKey) {
    send(
        agent,
        &AgentMsg::Hello(Hello {
            protocol_version: starfish_msg::PROTOCOL_VERSION,
            agent_key: agent_key.clone(),
            hostname: HOSTNAME.to_string(),
            agent_version: "test".to_string(),
        }),
    )
    .await;
}

async fn send(agent: &mut Agent, msg: &AgentMsg) {
    let bytes = starfish_msg::to_vec(msg).expect("unable to encode");

    agent
        .send(Message::binary(bytes))
        .await
        .expect("unable to send to the controller");
}

async fn recv(agent: &mut Agent) -> ControllerMsg {
    let deadline = tokio::time::sleep(REPLY_TIMEOUT);

    tokio::pin!(deadline);

    loop {
        tokio::select! {
            _ = &mut deadline => panic!("the controller did not reply within {REPLY_TIMEOUT:?}"),

            item = agent.next() => {
                let msg = item
                    .expect("the controller closed the connection")
                    .expect("unable to read from the controller");

                if msg.is_binary() {
                    return starfish_msg::from_slice(msg.as_payload()).expect("unable to decode");
                }
            }
        }
    }
}

async fn expect_config(agent: &mut Agent) -> HostConfig {
    match recv(agent).await {
        ControllerMsg::Config(config) => config,
        other => panic!("expected a configuration, got {other:?}"),
    }
}
