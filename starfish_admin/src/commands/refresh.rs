use anyhow::Context;
use starfish_msg::{AdminRequest, AdminResponse, frame};
use std::path::Path;
use tokio::net::UnixStream;

/// Asks the controller to rebuild configuration and push it to its agents.
pub async fn run(socket: &Path, hostname: Option<&str>) -> anyhow::Result<()> {
    let mut conn = UnixStream::connect(socket).await.with_context(|| {
        format!(
            "Unable to reach the controller on {}. Is starfishd running?",
            socket.display()
        )
    })?;

    frame::write(
        &mut conn,
        &AdminRequest::Refresh {
            hostname: hostname.map(str::to_string),
        },
    )
    .await
    .context("Unable to send the refresh request")?;

    let response: AdminResponse = frame::read(&mut conn)
        .await
        .context("Unable to read the controller's reply")?
        .context("The controller closed the connection without replying")?;

    match response {
        AdminResponse::Refreshed { notified, offline } => {
            for hostname in &notified {
                println!("{hostname}: refreshed");
            }

            // Not a failure. A host with no agent connected picks the change up
            // the next time it connects.
            for hostname in &offline {
                println!("{hostname}: offline, will update when its agent connects");
            }

            if notified.is_empty() && offline.is_empty() {
                println!("No hosts to refresh");
            }

            Ok(())
        }

        AdminResponse::Error { message } => {
            anyhow::bail!("The controller refused the request: {message}")
        }
    }
}
