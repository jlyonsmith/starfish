use crate::admin_args::UserOp;
use crate::commands::{DEFAULT_TABLE_STYLE, find_user, format_time};
use anyhow::{Context, bail};
use starfish_db::{HostGroup, HostGroupUser, SshKey, User};
use std::iter;
use tabled::{Table, derive::display};
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
        UserOp::List {} => list(db).await,
        UserOp::Show { alias } => show(db, alias).await,
        UserOp::Update {
            alias,
            new_alias,
            first_name,
            last_name,
            email,
        } => update(db, alias, new_alias, first_name, last_name, email).await,
        UserOp::Remove { alias } => remove(db, alias).await,
        UserOp::AddKey { alias, name, key } => add_ssh_key(db, alias, name, key).await,
        UserOp::RemoveKey { alias, name } => remove_ssh_key(db, alias, name).await,
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

    println!("Added user '{}'", user.alias);

    Ok(())
}

async fn list(db: &mut Db) -> anyhow::Result<()> {
    #[derive(tabled::Tabled)]
    #[tabled(rename_all = "Upper Title Case")]
    struct UserListRow<'a> {
        alias: &'a str,
        first_name: &'a str,
        last_name: &'a str,
        host_groups: String,
        #[tabled(display("format_time"))]
        created_at: &'a jiff::Timestamp,
        #[tabled(display("format_time"))]
        updated_at: &'a jiff::Timestamp,
    }

    let users = User::all()
        .order_by(User::fields().alias().asc())
        .exec(db)
        .await
        .context("Unable to read users")?;
    let mut rows = Vec::<UserListRow<'_>>::with_capacity(users.len());

    for user in &users {
        let host_group_users = HostGroupUser::filter_by_user_id(user.id)
            .exec(db)
            .await
            .context("Unable to read the user's host groups")?;
        let mut group_names = Vec::<String>::new();

        for host_group_user in host_group_users {
            let group = HostGroup::get_by_id(db, host_group_user.host_group_id)
                .await
                .context("Unable to read a host group")?;

            group_names.push(group.name.clone());
        }

        rows.push(UserListRow {
            alias: &user.alias,
            first_name: &user.first_name,
            last_name: &user.last_name,
            host_groups: group_names.join(", "),
            created_at: &user.created_at,
            updated_at: &user.updated_at,
        });
    }

    let mut table = Table::new(rows);

    println!("{}\n", table.with(DEFAULT_TABLE_STYLE));

    Ok(())
}

async fn show(db: &mut Db, alias: &str) -> anyhow::Result<()> {
    #[derive(tabled::Tabled)]
    #[tabled(rename_all = "Upper Title Case")]
    struct UserRow<'a> {
        alias: &'a str,
        first_name: &'a str,
        last_name: &'a str,
        email: &'a str,
        #[tabled(display("format_time"))]
        created_at: &'a jiff::Timestamp,
        #[tabled(display("format_time"))]
        updated_at: &'a jiff::Timestamp,
    }

    #[derive(tabled::Tabled)]
    #[tabled(rename_all = "Upper Title Case")]
    struct UserHostGroupRow<'a> {
        host_group: String,
        sudoer: &'a str,
        security_groups: String,
        #[tabled(display("format_time"))]
        created_at: &'a jiff::Timestamp,
        #[tabled(display("format_time"))]
        updated_at: &'a jiff::Timestamp,
    }

    #[derive(tabled::Tabled)]
    #[tabled(rename_all = "Upper Title Case")]
    struct SshKeyRow<'a> {
        name: &'a str,
        #[tabled(display("display::wrap", 25))]
        ssh_key: &'a str,
        #[tabled(display("format_time"))]
        created_at: &'a jiff::Timestamp,
        #[tabled(display("format_time"))]
        updated_at: &'a jiff::Timestamp,
    }

    let user = find_user(db, alias).await?;
    let row = UserRow {
        alias: &user.alias,
        first_name: &user.first_name,
        last_name: &user.last_name,
        email: &user.email,
        created_at: &user.created_at,
        updated_at: &user.updated_at,
    };
    let mut table = Table::new(iter::once(row));

    println!("{}\n", table.with(DEFAULT_TABLE_STYLE));

    let keys = SshKey::filter_by_user_id(user.id)
        .exec(db)
        .await
        .context("Unable to read the user's SSH keys")?;

    let mut table = Table::new(keys.iter().map(|k| SshKeyRow {
        name: &k.name,
        ssh_key: &k.key,
        created_at: &k.created_at,
        updated_at: &k.updated_at,
    }));

    println!("{}\n", table.with(DEFAULT_TABLE_STYLE));

    let host_group_users = HostGroupUser::filter_by_user_id(user.id)
        .exec(db)
        .await
        .context("Unable to read the user's host groups")?;
    let mut rows = Vec::<UserHostGroupRow>::new();

    for host_group_user in &host_group_users {
        let group = HostGroup::get_by_id(db, host_group_user.host_group_id)
            .await
            .context("Unable to read a host group")?;

        rows.push(UserHostGroupRow {
            host_group: group.name.clone(),
            security_groups: host_group_user
                .security_groups
                .iter()
                .map(|g| g.clone())
                .collect::<Vec<String>>()
                .join(", "),
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

async fn update(
    db: &mut Db,
    alias: &str,
    new_alias: &Option<String>,
    first_name: &Option<String>,
    last_name: &Option<String>,
    email: &Option<String>,
) -> anyhow::Result<()> {
    if first_name.is_none() && last_name.is_none() && email.is_none() {
        bail!("Nothing to update; pass at least one of --first-name, --last-name or --email");
    }

    let user = find_user(db, alias).await?;
    let mut update = User::update_by_id(user.id);

    if let Some(new_alias) = new_alias {
        update = update.alias(new_alias)
    }

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

async fn remove(db: &mut Db, alias: &str) -> anyhow::Result<()> {
    let user = find_user(db, alias).await?;

    // Nothing in the schema cascades, so the rows that point at this user have
    // to go first or they would be left dangling.
    SshKey::delete_by_user_id(db, user.id)
        .await
        .context("Unable to remove the user's SSH keys")?;
    HostGroupUser::delete_by_user_id(db, user.id)
        .await
        .context("Unable to remove the user's host group memberships")?;

    User::delete_by_id(db, user.id)
        .await
        .context("Unable to remove the user")?;

    println!("Removed user '{}'", user.alias);

    Ok(())
}

async fn add_ssh_key(db: &mut Db, alias: &str, name: &str, key: &str) -> anyhow::Result<()> {
    let user = find_user(db, alias).await?;

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

async fn remove_ssh_key(db: &mut Db, alias: &str, name: &str) -> anyhow::Result<()> {
    let user = find_user(db, alias).await?;

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
