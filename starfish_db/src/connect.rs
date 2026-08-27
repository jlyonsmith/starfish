//! Building a database connection without putting the password on a command
//! line.
//!
//! A URL passed as an argument shows up in `ps` output for every user on the
//! machine, and in shell history. These helpers keep the password in a file
//! that only its owner can read, and keep it out of the log once it is loaded.

use anyhow::{Context, bail};
use std::path::Path;
use url::Url;

/// Hosts where an unencrypted connection never leaves the machine, so there is
/// nothing for TLS to protect against.
const LOCAL_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

/// The `sslmode` values that authenticate the server rather than merely
/// encrypting the connection.
const VERIFIED_SSL_MODES: [&str; 2] = ["verify-ca", "verify-full"];

/// Builds the URL to connect with, taking the password from `password_file` if
/// one was given.
///
/// A password already in `url` is left alone, so an existing setup keeps
/// working, but a file takes precedence over it.
pub fn connection_url(url: &Url, password_file: Option<&Path>) -> anyhow::Result<Url> {
    let Some(path) = password_file else {
        return Ok(url.clone());
    };

    let password = read_password(path)?;
    let mut url = url.clone();

    // `set_password` percent encodes, so a password holding `@`, `/` or `:`
    // cannot break the URL apart.
    url.set_password(Some(&password))
        .map_err(|()| anyhow::anyhow!("Unable to put a password into '{}'", redact(&url)))?;

    Ok(url)
}

/// The URL with any password removed, safe to log.
pub fn redact(url: &Url) -> String {
    if url.password().is_none() {
        return url.to_string();
    }

    let mut url = url.clone();

    // Failing to redact would mean logging the password, so the whole URL is
    // dropped rather than risk it.
    if url.set_password(Some("REDACTED")).is_err() {
        return "<unprintable database URL>".to_string();
    }

    url.to_string()
}

/// A warning about connecting to a remote database without verifying it, or
/// `None` when there is nothing to say.
///
/// `sslmode` defaults to `prefer`, which uses TLS when the server offers it but
/// accepts plaintext and never checks who it is talking to. That is worth
/// pointing out rather than leaving to be discovered.
pub fn tls_warning(url: &Url) -> Option<String> {
    let host = url.host_str()?;

    if LOCAL_HOSTS.contains(&host) {
        return None;
    }

    let ssl_mode = url
        .query_pairs()
        .find(|(key, _)| key == "sslmode")
        .map(|(_, value)| value.into_owned())
        .unwrap_or_else(|| "prefer".to_string());

    if VERIFIED_SSL_MODES.contains(&ssl_mode.as_str()) {
        return None;
    }

    Some(format!(
        "Connecting to '{host}' with sslmode={ssl_mode}, which does not verify the server. \
         Add ?sslmode=verify-full&sslrootcert=system to the URL, or sslrootcert=<path> for an \
         internal CA."
    ))
}

/// Reads a password from a file, insisting nobody else can read it.
fn read_password(path: &Path) -> anyhow::Result<String> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("Unable to read the password file {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = metadata.permissions().mode();

        // A password readable by the group or by everyone is not a secret, and
        // silently going along with it defeats the point of the file.
        if mode & 0o077 != 0 {
            bail!(
                "The password file {} is mode {:o}; it must not be readable by anyone else (chmod 600)",
                path.display(),
                mode & 0o777
            );
        }
    }

    let password = std::fs::read_to_string(path)
        .with_context(|| format!("Unable to read the password file {}", path.display()))?;

    // A trailing newline is what any editor leaves behind, and is not part of
    // the password.
    let password = password.trim_end_matches(['\n', '\r']);

    if password.is_empty() {
        bail!("The password file {} is empty", path.display());
    }

    Ok(password.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn leaves_a_url_alone_when_there_is_no_password_file() {
        let original = url("postgresql://starfish@db.example.com/starfish");

        assert_eq!(connection_url(&original, None).unwrap(), original);
    }

    #[test]
    fn takes_the_password_from_a_file() {
        let dir = tempdir();
        let path = dir.join("password");

        write_private(&path, "hunter2\n");

        let url = connection_url(
            &url("postgresql://starfish@db.example.com/starfish"),
            Some(&path),
        )
        .unwrap();

        assert_eq!(url.password(), Some("hunter2"));
    }

    #[test]
    fn encodes_a_password_that_would_otherwise_break_the_url() {
        let dir = tempdir();
        let path = dir.join("password");

        write_private(&path, "p@ss:w/ord");

        let url = connection_url(
            &url("postgresql://starfish@db.example.com/starfish"),
            Some(&path),
        )
        .unwrap();

        // Round tripping through the string form is what actually matters,
        // since that is what gets handed to the driver.
        let reparsed = Url::parse(url.as_str()).unwrap();

        assert_eq!(reparsed.password(), Some("p%40ss%3Aw%2Ford"));
        assert_eq!(reparsed.host_str(), Some("db.example.com"));
    }

    #[test]
    fn refuses_a_password_file_others_can_read() {
        let dir = tempdir();
        let path = dir.join("password");

        write_private(&path, "hunter2");
        set_mode(&path, 0o644);

        let err = connection_url(&url("postgresql://db/starfish"), Some(&path)).unwrap_err();

        assert!(
            format!("{err:#}").contains("chmod 600"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn refuses_an_empty_password_file() {
        let dir = tempdir();
        let path = dir.join("password");

        write_private(&path, "\n");

        assert!(connection_url(&url("postgresql://db/starfish"), Some(&path)).is_err());
    }

    #[test]
    fn keeps_the_password_out_of_the_redacted_form() {
        let redacted = redact(&url(
            "postgresql://starfish:hunter2@db.example.com/starfish",
        ));

        assert!(!redacted.contains("hunter2"), "{redacted}");
        assert!(redacted.contains("db.example.com"), "{redacted}");
    }

    #[test]
    fn says_nothing_about_a_url_with_no_password() {
        let plain = "postgresql://starfish@db.example.com/starfish";

        assert_eq!(redact(&url(plain)), plain);
    }

    #[test]
    fn warns_about_an_unverified_remote_connection() {
        assert!(tls_warning(&url("postgresql://db.example.com/starfish")).is_some());
        assert!(
            tls_warning(&url("postgresql://db.example.com/starfish?sslmode=require")).is_some()
        );
        assert!(
            tls_warning(&url("postgresql://db.example.com/starfish?sslmode=disable")).is_some()
        );
    }

    #[test]
    fn says_nothing_about_a_verified_or_local_connection() {
        assert!(
            tls_warning(&url(
                "postgresql://db.example.com/starfish?sslmode=verify-full&sslrootcert=system"
            ))
            .is_none()
        );
        assert!(tls_warning(&url("postgresql://localhost:5432/starfish")).is_none());
        assert!(tls_warning(&url("postgresql://127.0.0.1:5432/starfish")).is_none());
    }

    fn tempdir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "starfish-db-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));

        std::fs::create_dir_all(&dir).unwrap();

        dir
    }

    fn write_private(path: &Path, contents: &str) {
        std::fs::write(path, contents).unwrap();
        set_mode(path, 0o600);
    }

    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
}
