use crate::controller::Controller;
use anyhow::{Context, bail};
use starfish_msg::{AdminRequest, AdminResponse, frame};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use tokio_util::sync::CancellationToken;

/// With no group configured, only the user the controller runs as may talk to
/// the socket. A refresh makes the controller act, so this is not a read-only
/// endpoint.
const SOCKET_MODE_PRIVATE: u32 = 0o600;

/// With a group configured, its members may talk to the socket as well. This is
/// what lets administrators with their own accounts run `starfish-admin
/// refresh` without being root.
const SOCKET_MODE_GROUP: u32 = 0o660;

/// Serves `starfish_admin` requests on a Unix domain socket until cancelled.
///
/// The socket file is removed on the way out so a restart is not blocked by a
/// leftover from an unclean shutdown.
pub async fn listen(
    controller: Arc<Controller>,
    path: PathBuf,
    group: Option<String>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let listener = bind(&path, group.as_deref())?;

    match &group {
        Some(group) => log::info!(
            "Listening for administration commands on {} (group {group})",
            path.display()
        ),
        None => log::info!(
            "Listening for administration commands on {}",
            path.display()
        ),
    }

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,

            accepted = listener.accept() => {
                let (conn, _addr) = match accepted {
                    Ok(accepted) => accepted,
                    Err(err) => {
                        log::warn!("Unable to accept an administration connection: {err}");
                        continue;
                    }
                };

                let controller = controller.clone();

                tokio::spawn(async move {
                    if let Err(err) = handle(controller, conn).await {
                        log::warn!("Administration command failed: {err:#}");
                    }
                });
            }
        }
    }

    remove_socket(&path);

    Ok(())
}

fn bind(path: &Path, group: Option<&str>) -> anyhow::Result<UnixListener> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Unable to create {}", parent.display()))?;
    }

    // Binding fails if the path exists, and a socket left by a killed process
    // would block every restart. Removing one that nothing is listening on is
    // safe; if another controller is live, `single_instance` has already
    // stopped us before we get here.
    remove_socket(path);

    let listener = UnixListener::bind(path)
        .with_context(|| format!("Unable to listen on {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        // Narrowed first, then handed to the group, so there is never a moment
        // where the group can reach a socket it is not yet meant to.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(SOCKET_MODE_PRIVATE))
            .with_context(|| format!("Unable to set permissions on {}", path.display()))?;

        if let Some(group) = group {
            let gid = group_id(group)?;

            std::os::unix::fs::chown(path, None, Some(gid))
                .with_context(|| format!("Unable to give {} to group '{group}'", path.display()))?;

            std::fs::set_permissions(path, std::fs::Permissions::from_mode(SOCKET_MODE_GROUP))
                .with_context(|| format!("Unable to set permissions on {}", path.display()))?;
        }
    }

    Ok(listener)
}

/// Looks a group up by name.
#[cfg(unix)]
fn group_id(group: &str) -> anyhow::Result<u32> {
    let name = std::ffi::CString::new(group)
        .with_context(|| format!("'{group}' is not a usable group name"))?;

    // Safe to call here: this runs once, during startup, before any other task
    // could be using the returned buffer.
    let entry = unsafe { libc::getgrnam(name.as_ptr()) };

    if entry.is_null() {
        bail!("There is no group named '{group}'");
    }

    Ok(unsafe { (*entry).gr_gid })
}

fn remove_socket(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => log::warn!("Unable to remove {}: {err}", path.display()),
    }
}

/// Reads requests from one admin connection until it closes.
async fn handle(controller: Arc<Controller>, mut conn: UnixStream) -> anyhow::Result<()> {
    while let Some(request) = frame::read::<_, AdminRequest>(&mut conn).await? {
        let response = match request {
            AdminRequest::Refresh { hostname } => {
                log::info!(
                    "Refresh requested for {}",
                    hostname.as_deref().unwrap_or("every host")
                );

                match controller.refresh(hostname.as_deref()).await {
                    Ok(response) => response,
                    Err(err) => {
                        log::error!("Refresh failed: {err:#}");

                        AdminResponse::Error {
                            message: format!("{err:#}"),
                        }
                    }
                }
            }
        };

        frame::write(&mut conn, &response).await?;
    }

    Ok(())
}
