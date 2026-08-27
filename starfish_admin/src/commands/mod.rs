//! One module per group of commands, plus the lookups they share.

pub mod host;
pub mod host_group;
pub mod refresh;
pub mod security_group;
pub mod user;

use anyhow::Context;
use starfish_db::{Host, HostGroup, SecurityGroup, User};
use toasty::Db;

/// Finds a user by login name.
pub async fn find_user(db: &mut Db, alias: &str) -> anyhow::Result<User> {
    User::filter_by_alias(alias)
        .first()
        .exec(db)
        .await
        .context("Unable to look up the user")?
        .with_context(|| format!("There is no user named '{alias}'"))
}

/// Finds a host group by name.
pub async fn find_host_group(db: &mut Db, name: &str) -> anyhow::Result<HostGroup> {
    HostGroup::filter_by_name(name)
        .first()
        .exec(db)
        .await
        .context("Unable to look up the host group")?
        .with_context(|| format!("There is no host group named '{name}'"))
}

/// Finds a host by name.
pub async fn find_host(db: &mut Db, hostname: &str) -> anyhow::Result<Host> {
    Host::filter_by_hostname(hostname)
        .first()
        .exec(db)
        .await
        .context("Unable to look up the host")?
        .with_context(|| format!("There is no host named '{hostname}'"))
}

/// Finds a security group by name within a host group. Names are only unique
/// inside a host group, so both are needed.
pub async fn find_security_group(
    db: &mut Db,
    host_group: &HostGroup,
    name: &str,
) -> anyhow::Result<SecurityGroup> {
    SecurityGroup::filter_by_host_group_id_and_name(host_group.id, name)
        .first()
        .exec(db)
        .await
        .context("Unable to look up the security group")?
        .with_context(|| {
            format!(
                "Host group '{}' has no security group named '{name}'",
                host_group.name
            )
        })
}
