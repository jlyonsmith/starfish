use anyhow::Context;
use clap::Parser;
use starfish_db::User;

mod admin_args;

use admin_args::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = match AdminArgs::try_parse() {
        Ok(m) => m,
        Err(err) => {
            // Help and version come back as an error
            eprintln!("{}", err.to_string());
            return Ok(());
        }
    };

    let mut db = toasty::Db::builder()
        .models(toasty::models!(starfish_db::*))
        .connect(&args.postgres_server.to_string())
        .await
        .context("Unable to connect to database")?;

    match &args.entity {
        Entity::System { op } => match op {
            SystemOp::CreateDatabase => {
                db.push_schema().await?;
                println!("Database created successfully");
            }
        },
        Entity::User { op } => match op {
            UserOp::Add {
                alias,
                first_name,
                last_name,
                email,
                ssh_keys,
            } => {
                let mut user_builder = toasty::create!(starfish_db::User {
                    alias: alias.clone(),
                    first_name: first_name.clone(),
                    last_name: last_name.clone(),
                    email: email.clone(),
                });

                if let Some(ssh_keys) = ssh_keys {
                    user_builder = user_builder.ssh_keys(
                        ssh_keys
                            .iter()
                            .map(|tuple| {
                                starfish_db::SshKey::create()
                                    .name(tuple.0.clone())
                                    .key(tuple.1.clone())
                            })
                            .collect::<Vec<_>>(),
                    );
                }

                let user = user_builder.exec(&mut db).await?;

                println!("User '{}' (id: {}) added successfully", user.alias, user.id);
            }
            UserOp::Remove { alias } => {
                User::delete_by_alias(&mut db, alias)
                    .await
                    .context(format!("Could not find user {}", alias))?;

                println!("User '{}' removed successfully", alias);
            }
            UserOp::List { verbose } => {
                let users = User::all()
                    .order_by(User::fields().alias().asc())
                    .exec(&mut db)
                    .await?;

                for user in users {
                    if *verbose {
                        println!(
                            "alias: {}, id: {}, first_name: {}, last_name: {}, email: {}",
                            user.alias, user.id, user.first_name, user.last_name, user.email
                        );
                    } else {
                        println!("{}", user.alias);
                    }
                }
            }
            UserOp::Update { .. } => {}
        },
        Entity::Host { op } => match op {
            HostOp::Add { .. } => {}
            HostOp::Remove { .. } => {}
            HostOp::Update { .. } => {}
            HostOp::List { .. } => {}
        },
        Entity::HostGroup { op } => match op {
            HostGroupOp::Add { .. } => {}
            HostGroupOp::Remove { .. } => {}
            HostGroupOp::Update { .. } => {}
            HostGroupOp::List { .. } => {}
        },
    }

    Ok(())
}
