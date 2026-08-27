use crate::admin_args::SecurityGroupOp;
use crate::commands::{find_host_group, find_security_group, find_user};
use anyhow::{Context, bail};
use starfish_db::{HostGroup, HostGroupUser, SecurityGroup, UserSecurityGroup};
use toasty::Db;

pub async fn run(db: &mut Db, op: &SecurityGroupOp) -> anyhow::Result<()> {
    match op {
        SecurityGroupOp::Add { host_group, name } => add(db, host_group, name).await,
        SecurityGroupOp::List { host_group } => list(db, host_group.as_deref()).await,
        SecurityGroupOp::Remove { host_group, name } => remove(db, host_group, name).await,
        SecurityGroupOp::AddUser {
            host_group,
            name,
            alias,
        } => add_user(db, host_group, name, alias).await,
        SecurityGroupOp::RemoveUser {
            host_group,
            name,
            alias,
        } => remove_user(db, host_group, name, alias).await,
    }
}

async fn add(db: &mut Db, host_group: &str, name: &str) -> anyhow::Result<()> {
    let group = find_host_group(db, host_group).await?;

    SecurityGroup::create()
        .host_group_id(group.id)
        .name(name)
        .exec(db)
        .await
        .context("Unable to add the security group")?;

    println!("Added security group '{name}' to host group '{host_group}'");

    Ok(())
}

async fn list(db: &mut Db, host_group: Option<&str>) -> anyhow::Result<()> {
    let groups = match host_group {
        Some(name) => vec![find_host_group(db, name).await?],
        None => HostGroup::all()
            .order_by(HostGroup::fields().name().asc())
            .exec(db)
            .await
            .context("Unable to read host groups")?,
    };

    for group in groups {
        let security_groups = SecurityGroup::filter_by_host_group_id(group.id)
            .order_by(SecurityGroup::fields().name().asc())
            .exec(db)
            .await
            .context("Unable to read security groups")?;

        for security_group in security_groups {
            println!("{}/{}", group.name, security_group.name);
        }
    }

    Ok(())
}

async fn remove(db: &mut Db, host_group: &str, name: &str) -> anyhow::Result<()> {
    let group = find_host_group(db, host_group).await?;
    let security_group = find_security_group(db, &group, name).await?;

    UserSecurityGroup::delete_by_security_group_id(db, security_group.id)
        .await
        .context("Unable to remove the group's memberships")?;

    SecurityGroup::delete_by_id(db, security_group.id)
        .await
        .context("Unable to remove the security group")?;

    println!("Removed security group '{host_group}/{name}'");

    Ok(())
}

async fn add_user(db: &mut Db, host_group: &str, name: &str, alias: &str) -> anyhow::Result<()> {
    let group = find_host_group(db, host_group).await?;
    let security_group = find_security_group(db, &group, name).await?;
    let user = find_user(db, alias).await?;

    // A security group only reaches a host through the host group, so a user
    // who has no account there would silently never get it.
    HostGroupUser::filter_by_host_group_id_and_user_id(group.id, user.id)
        .first()
        .exec(db)
        .await
        .context("Unable to look up the host group membership")?
        .with_context(|| {
            format!("'{alias}' is not in host group '{host_group}', so add them there first")
        })?;

    let existing =
        UserSecurityGroup::filter_by_user_id_and_security_group_id(user.id, security_group.id)
            .first()
            .exec(db)
            .await
            .context("Unable to look up the membership")?;

    if existing.is_some() {
        bail!("'{alias}' is already in security group '{host_group}/{name}'");
    }

    UserSecurityGroup::create()
        .user_id(user.id)
        .security_group_id(security_group.id)
        .exec(db)
        .await
        .context("Unable to add the user to the security group")?;

    println!("Added '{alias}' to security group '{host_group}/{name}'");

    Ok(())
}

async fn remove_user(db: &mut Db, host_group: &str, name: &str, alias: &str) -> anyhow::Result<()> {
    let group = find_host_group(db, host_group).await?;
    let security_group = find_security_group(db, &group, name).await?;
    let user = find_user(db, alias).await?;

    UserSecurityGroup::filter_by_user_id_and_security_group_id(user.id, security_group.id)
        .first()
        .exec(db)
        .await
        .context("Unable to look up the membership")?
        .with_context(|| format!("'{alias}' is not in security group '{host_group}/{name}'"))?;

    UserSecurityGroup::delete_by_user_id_and_security_group_id(db, user.id, security_group.id)
        .await
        .context("Unable to remove the user from the security group")?;

    println!("Removed '{alias}' from security group '{host_group}/{name}'");

    Ok(())
}
