//! Checks applied before anything on the host is touched.
//!
//! This runs as root, so it is the last place a bad name can be stopped. The
//! agent hands over whatever the controller sent, and the controller hands over
//! whatever is in the database, so nothing upstream can be assumed to have
//! checked it.

use anyhow::bail;

/// The longest a Linux user or group name may be.
const MAX_NAME_LEN: usize = 32;

/// Accounts below this belong to the distribution, not to Starfish. Refusing to
/// touch them keeps a bad configuration from turning `root` or `daemon` into a
/// Starfish managed account.
pub const MIN_MANAGED_UID: u32 = 1000;

/// Checks a user or group name.
///
/// Names go onto a command line as positional arguments, so a name starting
/// with `-` would be read as an option: `useradd` given a name of `-o` is a
/// very different command. The character set below is the conservative
/// intersection of what Ubuntu accepts and what is unambiguous as an argument.
pub fn name(kind: &str, name: &str) -> anyhow::Result<()> {
    if name.is_empty() {
        bail!("{kind} name is empty");
    }

    if name.len() > MAX_NAME_LEN {
        bail!("{kind} name '{name}' is longer than {MAX_NAME_LEN} characters");
    }

    let mut chars = name.chars();
    let first = chars.next().expect("the name is not empty");

    if !first.is_ascii_lowercase() && first != '_' {
        bail!("{kind} name '{name}' must start with a lowercase letter or an underscore");
    }

    for c in chars {
        if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '_' && c != '-' {
            bail!(
                "{kind} name '{name}' may only hold lowercase letters, digits, underscores and hyphens"
            );
        }
    }

    Ok(())
}

/// Checks a full name before it becomes a `GECOS` field.
///
/// `GECOS` is colon separated inside `/etc/passwd`, so a colon or a newline in
/// one would corrupt the file.
pub fn full_name(full_name: &str) -> anyhow::Result<()> {
    if let Some(bad) = full_name
        .chars()
        .find(|c| *c == ':' || *c == '\n' || c.is_control())
    {
        bail!("Full name '{full_name}' holds a character that is not allowed: {bad:?}");
    }

    Ok(())
}

/// Refuses to manage an account the distribution owns.
pub fn managed_uid(user: &str, uid: u32) -> anyhow::Result<()> {
    if uid < MIN_MANAGED_UID {
        bail!("'{user}' is a system account (uid {uid}) and will not be managed");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_names() {
        for good in ["ada", "jls", "_svc", "web-1", "a", "user_2"] {
            name("User", good).unwrap_or_else(|err| panic!("rejected '{good}': {err}"));
        }
    }

    #[test]
    fn rejects_names_that_would_be_read_as_options() {
        for bad in ["-o", "--uid=0", "-rf"] {
            assert!(name("User", bad).is_err(), "accepted '{bad}'");
        }
    }

    #[test]
    fn rejects_names_with_paths_spaces_or_shell_characters() {
        for bad in ["../root", "a b", "a;b", "a/b", "a$b", "a\nb", "Ada", ""] {
            assert!(name("User", bad).is_err(), "accepted '{bad}'");
        }
    }

    #[test]
    fn rejects_an_over_long_name() {
        assert!(name("User", &"a".repeat(MAX_NAME_LEN + 1)).is_err());
        name("User", &"a".repeat(MAX_NAME_LEN)).unwrap();
    }

    #[test]
    fn rejects_a_full_name_that_would_corrupt_passwd() {
        assert!(full_name("Ada:Lovelace").is_err());
        assert!(full_name("Ada\nLovelace").is_err());
        full_name("Ada Lovelace").unwrap();
        full_name("Seán Ó Briain").unwrap();
    }

    #[test]
    fn refuses_system_accounts() {
        assert!(managed_uid("root", 0).is_err());
        assert!(managed_uid("daemon", 1).is_err());
        assert!(managed_uid("nobody", 999).is_err());
        managed_uid("ada", MIN_MANAGED_UID).unwrap();
    }
}
