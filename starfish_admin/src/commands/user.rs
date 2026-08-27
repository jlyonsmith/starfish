use crate::admin_args::{UserOp, UserRef};
use crate::commands::find_user;
use anyhow::{Context, bail};
use starfish_db::{HostGroup, HostGroupUser, SecurityGroup, SshKey, User, UserSecurityGroup};
use toasty::Db;

pub async fn run(db: &mut Db, op: &UserOp) -> anyhow::Result<()> {
    match op {
        UserOp::Add {
            alias,
            first_name,
            last_name,
            email,
            ssh_keys,
        } => add(db, alias, first_name, last_name, email, ssh_keys).await,
        UserOp::List { verbose } => list(db, *verbose).await,
        UserOp::Show { user } => show(db, user).await,
        UserOp::Update {
            user,
            first_name,
            last_name,
            email,
        } => update(db, user, first_name, last_name, email).await,
        UserOp::Remove { user } => remove(db, user).await,
        UserOp::AddKey { user, name, key } => add_key(db, user, name, key).await,
        UserOp::RemoveKey { user, name } => remove_key(db, user, name).await,
    }
}

async fn add(
    db: &mut Db,
    alias: &str,
    first_name: &str,
    last_name: &str,
    email: &str,
    ssh_keys: &[(String, String)],
) -> anyhow::Result<()> {
    let mut builder = User::create()
        .alias(alias)
        .first_name(first_name)
        .last_name(last_name)
        .email(email);

    if !ssh_keys.is_empty() {
        builder = builder.ssh_keys(
            ssh_keys
                .iter()
                .map(|(name, key)| SshKey::create().name(name).key(key))
                .collect::<Vec<_>>(),
        );
    }

    let user = builder.exec(db).await.context("Unable to add the user")?;

    println!("Added user '{}' ({})", user.alias, user.id);

    Ok(())
}

async fn list(db: &mut Db, verbose: bool) -> anyhow::Result<()> {
    let users = User::all()
        .order_by(User::fields().alias().asc())
        .exec(db)
        .await
        .context("Unable to read users")?;

    for user in users {
        if verbose {
            println!(
                "{}\t{} {}\t{}",
                user.alias, user.first_name, user.last_name, user.email
            );
        } else {
            println!("{}", user.alias);
        }
    }

    Ok(())
}

async fn show(db: &mut Db, user_ref: &UserRef) -> anyhow::Result<()> {
    let user = find_user(db, &user_ref.alias).await?;

    println!("alias:      {}", user.alias);
    println!("name:       {} {}", user.first_name, user.last_name);
    println!("email:      {}", user.email);

    let keys = SshKey::filter_by_user_id(user.id)
        .exec(db)
        .await
        .context("Unable to read the user's SSH keys")?;

    println!("ssh keys:   {}", keys.len());

    for key in &keys {
        println!("  {}\t{}", key.name, key.key);
    }

    let memberships = HostGroupUser::filter_by_user_id(user.id)
        .exec(db)
        .await
        .context("Unable to read the user's host groups")?;

    println!("host groups:");

    for membership in &memberships {
        let group = HostGroup::get_by_id(db, membership.host_group_id)
            .await
            .context("Unable to read a host group")?;

        let mut flags = Vec::new();

        if membership.is_sudoer {
            flags.push("sudo");
        }

        if membership.is_admin {
            flags.push("admin");
        }

        println!(
            "  {}{}",
            group.name,
            if flags.is_empty() {
                String::new()
            } else {
                format!(" ({})", flags.join(", "))
            }
        );
    }

    let security_groups = UserSecurityGroup::filter_by_user_id(user.id)
        .exec(db)
        .await
        .context("Unable to read the user's security groups")?;

    println!("security groups:");

    for membership in &security_groups {
        let group = SecurityGroup::get_by_id(db, membership.security_group_id)
            .await
            .context("Unable to read a security group")?;
        let host_group = HostGroup::get_by_id(db, group.host_group_id)
            .await
            .context("Unable to read a host group")?;

        println!("  {}/{}", host_group.name, group.name);
    }

    Ok(())
}

async fn update(
    db: &mut Db,
    user_ref: &UserRef,
    first_name: &Option<String>,
    last_name: &Option<String>,
    email: &Option<String>,
) -> anyhow::Result<()> {
    if first_name.is_none() && last_name.is_none() && email.is_none() {
        bail!("Nothing to update; pass at least one of --first-name, --last-name or --email");
    }

    let user = find_user(db, &user_ref.alias).await?;
    let mut update = User::update_by_id(user.id);

    if let Some(first_name) = first_name {
        update = update.first_name(first_name);
    }

    if let Some(last_name) = last_name {
        update = update.last_name(last_name);
    }

    if let Some(email) = email {
        update = update.email(email);
    }

    update.exec(db).await.context("Unable to update the user")?;

    println!("Updated user '{}'", user.alias);

    Ok(())
}

async fn remove(db: &mut Db, user_ref: &UserRef) -> anyhow::Result<()> {
    let user = find_user(db, &user_ref.alias).await?;

    // Nothing in the schema cascades, so the rows that point at this user have
    // to go first or they would be left dangling.
    SshKey::delete_by_user_id(db, user.id)
        .await
        .context("Unable to remove the user's SSH keys")?;
    UserSecurityGroup::delete_by_user_id(db, user.id)
        .await
        .context("Unable to remove the user's security group memberships")?;
    HostGroupUser::delete_by_user_id(db, user.id)
        .await
        .context("Unable to remove the user's host group memberships")?;

    User::delete_by_id(db, user.id)
        .await
        .context("Unable to remove the user")?;

    println!("Removed user '{}'", user.alias);

    Ok(())
}

async fn add_key(db: &mut Db, user_ref: &UserRef, name: &str, key: &str) -> anyhow::Result<()> {
    let user = find_user(db, &user_ref.alias).await?;

    let existing = SshKey::filter_by_user_id(user.id)
        .exec(db)
        .await
        .context("Unable to read the user's SSH keys")?;

    if existing.iter().any(|existing| existing.name == name) {
        bail!("User '{}' already has a key named '{name}'", user.alias);
    }

    SshKey::create()
        .user_id(user.id)
        .name(name)
        .key(key)
        .exec(db)
        .await
        .context("Unable to add the SSH key")?;

    println!("Added key '{name}' to user '{}'", user.alias);

    Ok(())
}

async fn remove_key(db: &mut Db, user_ref: &UserRef, name: &str) -> anyhow::Result<()> {
    let user = find_user(db, &user_ref.alias).await?;

    let keys = SshKey::filter_by_user_id(user.id)
        .exec(db)
        .await
        .context("Unable to read the user's SSH keys")?;

    let key = keys
        .iter()
        .find(|key| key.name == name)
        .with_context(|| format!("User '{}' has no key named '{name}'", user.alias))?;

    SshKey::delete_by_id(db, key.id)
        .await
        .context("Unable to remove the SSH key")?;

    println!("Removed key '{name}' from user '{}'", user.alias);

    Ok(())
}
