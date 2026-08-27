use anyhow::Context;
use clap::Parser;

mod admin_args;
mod commands;

use admin_args::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = match AdminArgs::try_parse() {
        Ok(m) => m,
        Err(err) => {
            // Help and version come back as an error
            eprintln!("{err}");
            return Ok(());
        }
    };

    // Refreshing only talks to the controller, so it does not need, and should
    // not insist on, a working database connection.
    if let Command::Refresh { hostname } = &args.command {
        return commands::refresh::run(&args.socket, hostname.as_deref()).await;
    }

    let database_url =
        starfish_db::connection_url(&args.postgres_server, args.password_file.as_deref())?;

    if let Some(warning) = starfish_db::tls_warning(&database_url) {
        eprintln!("warning: {warning}");
    }

    let mut db = toasty::Db::builder()
        .models(toasty::models!(starfish_db::*))
        .connect(database_url.as_ref())
        .await
        .with_context(|| {
            format!(
                "Unable to connect to {}",
                starfish_db::redact(&database_url)
            )
        })?;

    match &args.command {
        Command::InitDb => {
            db.push_schema()
                .await
                .context("Unable to create the database schema")?;

            println!("Database schema created");
        }
        Command::User { op } => commands::user::run(&mut db, op).await?,
        Command::HostGroup { op } => commands::host_group::run(&mut db, op).await?,
        Command::SecurityGroup { op } => commands::security_group::run(&mut db, op).await?,
        Command::Host { op } => commands::host::run(&mut db, op).await?,
        Command::Refresh { .. } => unreachable!("handled before connecting to the database"),
    }

    Ok(())
}
