use clap::Parser;
use sqlx::postgres::PgPoolOptions;

#[derive(Parser)]
enum Operation {
    // Insert a user into the database
    InsertUser {
        #[clap(long)]
        alias: String,
        #[clap(long)]
        email: String,
        #[clap(long)]
        first_name: String,
        #[clap(long)]
        last_name: String,
    },
    UpdateUser {
        id: i32,
        #[clap(long)]
        alias: Option<String>,
        #[clap(long)]
        email: Option<String>,
        #[clap(long)]
        first_name: Option<String>,
        #[clap(long)]
        last_name: Option<String>,
    },
    GetUser {
        id: i32,
    },
    AllUsers,
    DeleteUser {
        id: i32,
    },
}

#[derive(Parser)]
struct Cli {
    #[clap(subcommand)]
    op: Operation,
}

#[derive(sqlx::FromRow, Debug)]
struct User {
    id: i64,
    alias: String,
    email: String,
    first_name: String,
    last_name: String,
    created_at: jiff_sqlx::Timestamp,
    modified_at: jiff_sqlx::Timestamp,
}

/// Fetch a single row by ID
async fn get_user_by_id(pool: &sqlx::PgPool, id: i32) -> Result<Option<User>, sqlx::Error> {
    let user = sqlx::query_as::<_, User>("SELECT * FROM \"user\" WHERE id = $1")
        .bind(id)
        .fetch_optional(pool) // Returns Ok(None) if not found
        .await?;

    Ok(user)
}

/// Fetch all user rows
async fn get_all_users(pool: &sqlx::PgPool) -> Result<Vec<User>, sqlx::Error> {
    let users = sqlx::query_as::<_, User>(
        "SELECT id, alias, email, first_name, last_name, created_at, modified_at FROM \"user\"",
    )
    .fetch_all(pool)
    .await?;

    Ok(users)
}

/// Insert a new user
async fn insert_user(
    pool: &sqlx::PgPool,
    alias: &str,
    email: &str,
    first_name: &str,
    last_name: &str,
) -> Result<User, sqlx::Error> {
    let user = sqlx::query_as::<_, User>("INSERT INTO \"user\" (\"alias\", \"email\", \"first_name\", \"last_name\") VALUES ($1, $2, $3, $4) RETURNING *")
        .bind(alias)
        .bind(email)
        .bind(first_name)
        .bind(last_name)
        .fetch_one(pool)
        .await?;

    Ok(user)
}

/// Update an existing user
async fn update_user(
    pool: &sqlx::PgPool,
    id: i32,
    alias: Option<&str>,
    email: Option<&str>,
    first_name: Option<&str>,
    last_name: Option<&str>,
) -> Result<User, sqlx::Error> {
    let user = sqlx::query_as::<_, User>("UPDATE \"user\" SET \"alias\" = COALESCE($2, \"alias\"), \"email\" = COALESCE($3, \"email\"), \"first_name\" = COALESCE($4, \"first_name\"), \"last_name\" = COALESCE($5, \"last_name\") WHERE id = $1 RETURNING *")
        .bind(id)
        .bind(alias)
        .bind(email)
        .bind(first_name)
        .bind(last_name)
        .fetch_one(pool)
        .await?;

    Ok(user)
}

/// Delete a user
async fn delete_user(pool: &sqlx::PgPool, id: i32) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM \"user\" WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Print a user
fn print_user(user: &User) {
    println!(
        "id: {}, alias: {}, email: {}, first_name: {}, last_name: {}, created_at: {}, modified_at: {}",
        user.id,
        user.alias,
        user.email,
        user.first_name,
        user.last_name,
        user.created_at.to_jiff(),
        user.modified_at.to_jiff(),
    );
}

#[tokio::main]
async fn main() -> Result<(), sqlx::Error> {
    let cli = Cli::parse();

    let database_url = "postgres://postgres@localhost/starfish";

    // Create a connection pool
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await?;

    // Insert a user
    match cli.op {
        Operation::InsertUser {
            alias,
            email,
            first_name,
            last_name,
        } => {
            let user = insert_user(&pool, &alias, &email, &first_name, &last_name).await?;
            print_user(&user);
        }
        Operation::UpdateUser {
            id,
            alias,
            email,
            first_name,
            last_name,
        } => {
            let user = update_user(
                &pool,
                id,
                alias.as_deref(),
                email.as_deref(),
                first_name.as_deref(),
                last_name.as_deref(),
            )
            .await?;
            print_user(&user);
        }
        Operation::AllUsers => {
            let users = get_all_users(&pool).await?;
            for user in users {
                print_user(&user);
            }
        }
        Operation::GetUser { id } => {
            let user = get_user_by_id(&pool, id).await?;
            if let Some(user) = user {
                print_user(&user);
            } else {
                println!("User not found");
            }
        }
        Operation::DeleteUser { id } => {
            delete_user(&pool, id).await?;
            println!("User deleted");
        }
    }

    Ok(())
}
