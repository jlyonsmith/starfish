use crate::admin_args::HostOp;
use crate::commands::{find_host, find_host_group};
use anyhow::{Context, bail};
use starfish_db::{Host, HostGroup};
use starfish_msg::AgentKey;
use toasty::Db;

pub async fn run(db: &mut Db, op: &HostOp) -> anyhow::Result<()> {
    match op {
        HostOp::Add {
            hostname,
            host_group,
            info,
        } => add(db, hostname, host_group, info).await,
        HostOp::List { verbose } => list(db, *verbose).await,
        HostOp::Show { hostname } => show(db, hostname).await,
        HostOp::Update {
            hostname,
            info,
            host_group,
        } => update(db, hostname, info, host_group).await,
        HostOp::Remove { hostname } => remove(db, hostname).await,
        HostOp::Rekey { hostname } => rekey(db, hostname).await,
    }
}

async fn add(db: &mut Db, hostname: &str, host_group: &str, info: &str) -> anyhow::Result<()> {
    let group = find_host_group(db, host_group).await?;
    let agent_key = AgentKey::generate().context("Unable to generate an agent key")?;

    let host = Host::create()
        .hostname(hostname)
        .host_group_id(group.id)
        .info(info)
        .agent_key(agent_key.as_str())
        .exec(db)
        .await
        .context("Unable to add the host")?;

    println!(
        "Added host '{}' ({}) to '{host_group}'",
        host.hostname, host.id
    );
    println!();
    println!("Agent key: {agent_key}");
    println!();
    println!("Put this in the host's /etc/starfish_agent.conf as `agent_key`.");
    println!("It is shown again by `starfish-admin host show --hostname {hostname}`.");

    Ok(())
}

async fn list(db: &mut Db, verbose: bool) -> anyhow::Result<()> {
    let hosts = Host::all()
        .order_by(Host::fields().hostname().asc())
        .exec(db)
        .await
        .context("Unable to read hosts")?;

    for host in hosts {
        if !verbose {
            println!("{}", host.hostname);
            continue;
        }

        let group = HostGroup::get_by_id(db, host.host_group_id)
            .await
            .context("Unable to read the host's group")?;

        println!(
            "{}\t{}\t{}\t{}",
            host.hostname,
            group.name,
            health(&host),
            host.info
        );
    }

    Ok(())
}

async fn show(db: &mut Db, hostname: &str) -> anyhow::Result<()> {
    let host = find_host(db, hostname).await?;
    let group = HostGroup::get_by_id(db, host.host_group_id)
        .await
        .context("Unable to read the host's group")?;

    println!("hostname:   {}", host.hostname);
    println!("host group: {}", group.name);
    println!("info:       {}", host.info);
    println!("agent key:  {}", host.agent_key);
    println!("health:     {}", health(&host));

    match host.contacted_at {
        Some(contacted_at) => println!("last seen:  {contacted_at}"),
        None => println!("last seen:  never"),
    }

    if let Some(next_heartbeat_at) = host.next_heartbeat_at {
        println!("due by:     {next_heartbeat_at}");
    }

    Ok(())
}

async fn update(
    db: &mut Db,
    hostname: &str,
    info: &Option<String>,
    host_group: &Option<String>,
) -> anyhow::Result<()> {
    if info.is_none() && host_group.is_none() {
        bail!("Nothing to update; pass at least one of --info or --host-group");
    }

    let host = find_host(db, hostname).await?;
    let mut update = Host::update_by_id(host.id);

    if let Some(info) = info {
        update = update.info(info);
    }

    if let Some(host_group) = host_group {
        let group = find_host_group(db, host_group).await?;

        update = update.host_group_id(group.id);
    }

    update.exec(db).await.context("Unable to update the host")?;

    println!("Updated host '{hostname}'");

    Ok(())
}

async fn remove(db: &mut Db, hostname: &str) -> anyhow::Result<()> {
    let host = find_host(db, hostname).await?;

    Host::delete_by_id(db, host.id)
        .await
        .context("Unable to remove the host")?;

    println!("Removed host '{hostname}'");

    Ok(())
}

async fn rekey(db: &mut Db, hostname: &str) -> anyhow::Result<()> {
    let host = find_host(db, hostname).await?;
    let agent_key = AgentKey::generate().context("Unable to generate an agent key")?;

    Host::update_by_id(host.id)
        .agent_key(agent_key.as_str())
        .exec(db)
        .await
        .context("Unable to update the host's agent key")?;

    println!("New agent key for '{hostname}': {agent_key}");
    println!();
    println!("The agent on this host cannot reconnect until its configuration");
    println!("is updated with this key.");

    Ok(())
}

/// Whether the host's agent has checked in when it said it would.
fn health(host: &Host) -> &'static str {
    let Some(next_heartbeat_at) = host.next_heartbeat_at else {
        return "never seen";
    };

    if next_heartbeat_at < jiff::Timestamp::now() {
        "overdue"
    } else {
        "ok"
    }
}
