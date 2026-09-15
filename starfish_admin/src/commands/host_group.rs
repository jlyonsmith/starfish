use crate::admin_args::HostGroupOp;
use crate::commands::{DEFAULT_TABLE_STYLE, find_host_group, find_user, format_time};
use anyhow::{Context, bail};
use starfish_db::{Host, HostGroup, HostGroupUser, User};
use std::collections::BTreeSet;
use std::iter;
use tabled::Table;
use toasty::Db;

pub async fn run(db: &mut Db, op: &HostGroupOp) -> anyhow::Result<()> {
    match op {
        HostGroupOp::Add { name } => add(db, name).await,
        HostGroupOp::List {} => list(db).await,
        HostGroupOp::Show { name } => show(db, name).await,
        HostGroupOp::Remove { name } => remove(db, name).await,
        HostGroupOp::AddUser {
            name,
            alias,
            sudoer,
            security_groups,
        } => add_user(db, name, alias, *sudoer, security_groups).await,
        HostGroupOp::RemoveUser { name, alias } => remove_user(db, name, alias).await,
    }
}

async fn add(db: &mut Db, name: &str) -> anyhow::Result<()> {
    let group = HostGroup::create()
        .name(name)
        .exec(db)
        .await
        .context("Unable to add the host group")?;

    println!("Added host group '{}'", group.name);

    Ok(())
}

async fn list(db: &mut Db) -> anyhow::Result<()> {
    #[derive(tabled::Tabled)]
    #[tabled(rename_all = "Upper Title Case")]
    struct HostGroupRow<'a> {
        name: &'a str,
        num_hosts: usize,
        hosts: String,
        num_users: usize,
        users: String,
        #[tabled(display("format_time"))]
        created_at: &'a jiff::Timestamp,
        #[tabled(display("format_time"))]
        updated_at: &'a jiff::Timestamp,
    }

    let groups = HostGroup::all()
        .order_by(HostGroup::fields().name().asc())
        .exec(db)
        .await
        .context("Unable to read host groups")?;
    let mut rows = Vec::with_capacity(groups.len());

    for group in &groups {
        let hosts = Host::filter_by_host_group_id(group.id)
            .exec(db)
            .await
            .context("Unable to read the group's hosts")?;
        let host_group_users = HostGroupUser::filter_by_host_group_id(group.id)
            .exec(db)
            .await
            .context("Unable to read the group's users")?;
        let mut users = Vec::<String>::with_capacity(host_group_users.len());

        for host_group_user in &host_group_users {
            let user = User::get_by_id(db, host_group_user.user_id)
                .await
                .context("Unable to read the user")?;
            users.push(user.alias.clone());
        }

        rows.push(HostGroupRow {
            name: &group.name,
            num_hosts: host_group_users.len(),
            hosts: hosts
                .iter()
                .map(|h| h.hostname.clone())
                .collect::<Vec<_>>()
                .join(", "),
            num_users: host_group_users.len(),
            users: users.join(", "),
            created_at: &group.created_at,
            updated_at: &group.updated_at,
        });
    }

    let mut table = Table::new(rows);

    println!("{}", table.with(DEFAULT_TABLE_STYLE));

    Ok(())
}

async fn show(db: &mut Db, name: &str) -> anyhow::Result<()> {
    #[derive(tabled::Tabled)]
    #[tabled(rename_all = "Upper Title Case")]
    struct HostGroupRow<'a> {
        name: &'a str,
        #[tabled(display("format_time"))]
        created_at: &'a jiff::Timestamp,
        #[tabled(display("format_time"))]
        updated_at: &'a jiff::Timestamp,
    }

    #[derive(tabled::Tabled)]
    #[tabled(rename_all = "Upper Title Case")]
    struct HostRow<'a> {
        hostname: &'a str,
        #[tabled(display("format_time"))]
        created_at: &'a jiff::Timestamp,
        #[tabled(display("format_time"))]
        updated_at: &'a jiff::Timestamp,
    }

    #[derive(tabled::Tabled)]
    #[tabled(rename_all = "Upper Title Case")]
    struct UserRow<'a> {
        alias: String,
        security_groups: String,
        sudoer: &'static str,
        #[tabled(display("format_time"))]
        created_at: &'a jiff::Timestamp,
        #[tabled(display("format_time"))]
        updated_at: &'a jiff::Timestamp,
    }

    let group = find_host_group(db, name).await?;
    let row = HostGroupRow {
        name: &group.name,
        created_at: &group.created_at,
        updated_at: &group.updated_at,
    };
    let mut table = Table::new(iter::once(row));

    println!("{}\n", table.with(DEFAULT_TABLE_STYLE));

    let hosts = Host::filter_by_host_group_id(group.id)
        .exec(db)
        .await
        .context("Unable to read the group's hosts")?;

    let mut table = Table::new(hosts.iter().map(|host| HostRow {
        hostname: host.hostname.as_str(),
        created_at: &host.created_at,
        updated_at: &host.updated_at,
    }));

    println!("{}\n", table.with(DEFAULT_TABLE_STYLE));

    let host_group_users = HostGroupUser::filter_by_host_group_id(group.id)
        .exec(db)
        .await
        .context("Unable to read the group's users")?;
    let mut rows = Vec::<UserRow>::with_capacity(host_group_users.len());

    for host_group_user in &host_group_users {
        let user = User::get_by_id(db, host_group_user.user_id)
            .await
            .context("Cannot get user")?;

        rows.push(UserRow {
            alias: user.alias.clone(),
            security_groups: host_group_user.security_groups.join(", "),
            sudoer: if host_group_user.is_sudoer {
                "Yes"
            } else {
                "No"
            },
            created_at: &host_group_user.created_at,
            updated_at: &host_group_user.updated_at,
        });
    }

    let mut table = Table::new(rows);

    println!("{}", table.with(DEFAULT_TABLE_STYLE));

    Ok(())
}

async fn remove(db: &mut Db, name: &str) -> anyhow::Result<()> {
    let group = find_host_group(db, name).await?;

    // Removing a group out from under its hosts would leave them pointing at
    // nothing, so say so instead of doing it.
    let hosts = Host::filter_by_host_group_id(group.id)
        .exec(db)
        .await
        .context("Unable to read the group's hosts")?;

    if !hosts.is_empty() {
        bail!(
            "Host group '{name}' still has {} host(s); remove or move them first",
            hosts.len()
        );
    }

    // The memberships carry the group's security groups, so they go with them.
    HostGroupUser::delete_by_host_group_id(db, group.id)
        .await
        .context("Unable to remove the group's users")?;

    HostGroup::delete_by_id(db, group.id)
        .await
        .context("Unable to remove the host group")?;

    println!("Removed host group '{name}'");

    Ok(())
}

async fn add_user(
    db: &mut Db,
    name: &str,
    alias: &str,
    sudoer: bool,
    security_groups: &[String],
) -> anyhow::Result<()> {
    let group = find_host_group(db, name).await?;
    let user = find_user(db, alias).await?;

    // A group named twice on the command line is still one group, and sorting
    // keeps the stored set independent of the order it was typed in.
    let security_groups: Vec<String> = security_groups
        .iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .cloned()
        .collect();

    let existing = HostGroupUser::filter_by_host_group_id_and_user_id(group.id, user.id)
        .first()
        .exec(db)
        .await
        .context("Unable to look up the membership")?;

    // Repeating the command with different flags should change them rather
    // than fail on the composite key.
    if existing.is_some() {
        HostGroupUser::update_by_host_group_id_and_user_id(group.id, user.id)
            .is_sudoer(sudoer)
            .security_groups(security_groups)
            .exec(db)
            .await
            .context("Unable to update the membership")?;

        println!("Updated '{alias}' in host group '{name}'");

        return Ok(());
    }

    HostGroupUser::create()
        .host_group_id(group.id)
        .user_id(user.id)
        .is_sudoer(sudoer)
        .security_groups(security_groups)
        .exec(db)
        .await
        .context("Unable to add the user to the host group")?;

    println!("Added '{alias}' to host group '{name}'");

    Ok(())
}

async fn remove_user(db: &mut Db, name: &str, alias: &str) -> anyhow::Result<()> {
    let group = find_host_group(db, name).await?;
    let user = find_user(db, alias).await?;

    HostGroupUser::filter_by_host_group_id_and_user_id(group.id, user.id)
        .first()
        .exec(db)
        .await
        .context("Unable to look up the membership")?
        .with_context(|| format!("'{alias}' is not in host group '{name}'"))?;

    // The user's security groups within this host group live on the row, so
    // they go with it.
    HostGroupUser::delete_by_host_group_id_and_user_id(db, group.id, user.id)
        .await
        .context("Unable to remove the user from the host group")?;

    println!("Removed '{alias}' from host group '{name}'");

    Ok(())
}
