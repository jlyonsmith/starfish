//! One module per group of commands, plus the lookups they share.

pub mod host;
pub mod host_group;
pub mod refresh;
pub mod user;

use anyhow::Context;
use starfish_db::{Host, HostGroup, User};
use tabled::{Table, Tabled, settings::Style};
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

/// Renders `rows` as the `list` commands' shared table: column titles over a
/// rule, and no other decoration. Columns are sized to their contents.
pub fn table<T: Tabled>(rows: impl IntoIterator<Item = T>) -> Table {
    let mut table = Table::new(rows);

    table.with(Style::psql());
    table
}
