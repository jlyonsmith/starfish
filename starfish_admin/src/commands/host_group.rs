use crate::admin_args::HostGroupOp;
use crate::commands::{find_host_group, find_user};
use anyhow::{Context, bail};
use starfish_db::{Host, HostGroup, HostGroupUser, SecurityGroup, UserSecurityGroup};
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
            admin,
        } => add_user(db, name, alias, *sudoer, *admin).await,
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

async fn list(db: &mut Db, verbose: bool) -> anyhow::Result<()> {
    let groups = HostGroup::all()
        .order_by(HostGroup::fields().name().asc())
        .exec(db)
        .await
        .context("Unable to read host groups")?;

    for group in groups {
        if !verbose {
            println!("{}", group.name);
            continue;
        }

        let hosts = Host::filter_by_host_group_id(group.id)
            .exec(db)
            .await
            .context("Unable to read the group's hosts")?;
        let users = HostGroupUser::filter_by_host_group_id(group.id)
            .exec(db)
            .await
            .context("Unable to read the group's users")?;
        let security_groups = SecurityGroup::filter_by_host_group_id(group.id)
            .exec(db)
            .await
            .context("Unable to read the group's security groups")?;

        println!(
            "{}\t{} host(s)\t{} user(s)\t{} security group(s)",
            group.name,
            hosts.len(),
            users.len(),
            security_groups.len()
        );
    }

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

    let security_groups = SecurityGroup::filter_by_host_group_id(group.id)
        .exec(db)
        .await
        .context("Unable to read the group's security groups")?;

    for security_group in &security_groups {
        UserSecurityGroup::delete_by_security_group_id(db, security_group.id)
            .await
            .context("Unable to remove security group memberships")?;
    }

    SecurityGroup::delete_by_host_group_id(db, group.id)
        .await
        .context("Unable to remove the group's security groups")?;
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
    admin: bool,
) -> anyhow::Result<()> {
    let group = find_host_group(db, name).await?;
    let user = find_user(db, alias).await?;

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
            .is_admin(admin)
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
        .is_admin(admin)
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

    HostGroupUser::delete_by_host_group_id_and_user_id(db, group.id, user.id)
        .await
        .context("Unable to remove the user from the host group")?;

    // The user's security groups within this host group are meaningless now
    // that they have no account on its hosts.
    let security_groups = SecurityGroup::filter_by_host_group_id(group.id)
        .exec(db)
        .await
        .context("Unable to read the group's security groups")?;

    for security_group in &security_groups {
        UserSecurityGroup::delete_by_user_id_and_security_group_id(db, user.id, security_group.id)
            .await
            .context("Unable to remove a security group membership")?;
    }

    println!("Removed '{alias}' from host group '{name}'");

    Ok(())
}
