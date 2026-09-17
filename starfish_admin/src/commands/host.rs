use crate::admin_args::HostOp;
use crate::commands::{DEFAULT_TABLE_STYLE, find_host, find_host_group, format_time};
use anyhow::{Context, bail};
use starfish_db::{Host, HostGroup};
use starfish_msg::AgentKey;
use std::iter;
use tabled::{Table, derive::display};
use toasty::Db;

/// Whether the host's agent has checked in when it said it would.
fn get_health(next_heartbeat_at: &Option<jiff::Timestamp>) -> String {
    let Some(next_heartbeat_at) = next_heartbeat_at else {
        return "Unknown".to_string();
    };

    if *next_heartbeat_at < jiff::Timestamp::now() {
        "Overdue".to_string()
    } else {
        "OK".to_string()
    }
}

fn get_contacted_at(contacted_at: &Option<jiff::Timestamp>) -> String {
    if let Some(contacted_at) = contacted_at {
        format_time(contacted_at)
    } else {
        "Never".to_string()
    }
}

pub async fn run(db: &mut Db, op: &HostOp) -> anyhow::Result<()> {
    match op {
        HostOp::Add {
            hostname,
            host_group,
            info,
        } => add(db, hostname, host_group, info).await,
        HostOp::List {} => list(db).await,
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

    println!("Added host '{}' to '{host_group}'", host.hostname);
    println!();
    println!("Agent key: {agent_key}");
    println!();
    println!(
        "Put this in the host's /etc/starfish_agent.conf as `agent_key`. It is shown again by `starfish-admin host show {hostname}`."
    );

    Ok(())
}

async fn list(db: &mut Db) -> anyhow::Result<()> {
    #[derive(tabled::Tabled)]
    #[tabled(rename_all = "Upper Title Case")]
    struct HostRow<'a> {
        hostname: &'a str,
        host_group: String,
        health: String,
        contacted_at: String,
        #[tabled(display("format_time"))]
        created_at: &'a jiff::Timestamp,
        #[tabled(display("format_time"))]
        updated_at: &'a jiff::Timestamp,
        #[tabled(display("display::wrap", 25))]
        info: &'a str,
    }

    let hosts = Host::all()
        .order_by(Host::fields().hostname().asc())
        .exec(db)
        .await
        .context("Unable to read hosts")?;
    let mut rows = Vec::<HostRow<'_>>::with_capacity(hosts.len());

    for host in &hosts {
        let group = HostGroup::get_by_id(db, host.host_group_id)
            .await
            .context("Unable to read the host's group")?;

        rows.push(HostRow {
            hostname: &host.hostname,
            host_group: group.name.clone(),
            info: &host.info,
            health: get_health(&host.next_heartbeat_at).to_string(),
            contacted_at: get_contacted_at(&host.contacted_at).to_string(),
            created_at: &host.created_at,
            updated_at: &host.updated_at,
        });
    }

    let mut table = Table::new(rows);

    println!("{}", table.with(DEFAULT_TABLE_STYLE));

    Ok(())
}

async fn show(db: &mut Db, hostname: &str) -> anyhow::Result<()> {
    #[derive(tabled::Tabled)]
    #[tabled(rename_all = "Upper Title Case")]
    struct HostRow<'a> {
        hostname: &'a str,
        host_group: String,
        agent_key: &'a str,
        health: String,
        contacted_at: String,
        #[tabled(display("format_time"))]
        created_at: &'a jiff::Timestamp,
        #[tabled(display("format_time"))]
        updated_at: &'a jiff::Timestamp,
        #[tabled(display("display::wrap", 25))]
        info: &'a str,
    }

    let host = find_host(db, hostname).await?;
    let group = HostGroup::get_by_id(db, host.host_group_id)
        .await
        .context("Unable to read the host's group")?;
    let row = HostRow {
        hostname: &host.hostname,
        host_group: group.name.clone(),
        info: &host.info,
        agent_key: &host.agent_key,
        health: get_health(&host.next_heartbeat_at).to_string(),
        contacted_at: get_contacted_at(&host.contacted_at).to_string(),
        created_at: &host.created_at,
        updated_at: &host.updated_at,
    };
    let mut table = Table::new(iter::once(row));

    println!("{}", table.with(DEFAULT_TABLE_STYLE));

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

    println!("New agent key for '{hostname}': {agent_key}\n");
    println!(
        "The agent on this host cannot reconnect until its configuration is updated with this key."
    );

    Ok(())
}
