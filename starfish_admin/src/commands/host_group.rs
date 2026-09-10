use crate::admin_args::HostGroupOp;
use crate::commands::{find_host_group, find_user, table};
use anyhow::{Context, bail};
use starfish_db::{Host, HostGroup, HostGroupUser};
use std::collections::BTreeSet;
use tabled::Tabled;
use toasty::Db;

pub async fn run(db: &mut Db, op: &HostGroupOp) -> anyhow::Result<()> {
    match op {
        HostGroupOp::Add { name } => add(db, name).await,
        HostGroupOp::List { verbose } => list(db, *verbose).await,
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

    println!("Added host group '{}' ({})", group.name, group.id);

    Ok(())
}

/// A row of `host-group list --verbose`. These are counts of what belongs to
/// the group, not fields of `HostGroup`.
#[derive(Tabled)]
#[tabled(rename_all = "Upper Title Case")]
struct HostGroupRow {
    name: String,
    hosts: usize,
    users: usize,
    security_groups: usize,
}

async fn list(db: &mut Db, verbose: bool) -> anyhow::Result<()> {
    let groups = HostGroup::all()
        .order_by(HostGroup::fields().name().asc())
        .exec(db)
        .await
        .context("Unable to read host groups")?;

    if !verbose {
        for group in groups {
            println!("{}", group.name);
        }

        return Ok(());
    }

    let mut rows = Vec::with_capacity(groups.len());

    for group in groups {
        let hosts = Host::filter_by_host_group_id(group.id)
            .exec(db)
            .await
            .context("Unable to read the group's hosts")?;
        let users = HostGroupUser::filter_by_host_group_id(group.id)
            .exec(db)
            .await
            .context("Unable to read the group's users")?;

        // Security groups exist only by being named on a membership, so the
        // group's set of them is the union across its users.
        let security_groups: BTreeSet<&str> = users
            .iter()
            .flat_map(|user| user.security_groups.iter().map(String::as_str))
            .collect();

        rows.push(HostGroupRow {
            name: group.name,
            hosts: hosts.len(),
            users: users.len(),
            security_groups: security_groups.len(),
        });
    }

    println!("{}", table(rows));

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
