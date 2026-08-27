use crate::{Error, Result};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::str::FromStr;

/// The alphabet an agent key is drawn from. 62 characters, so a key carries
/// just under 96 bits of entropy.
const ALPHABET: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// The key an agent authenticates to the controller with.
///
/// The controller generates one per host and stores it on the host record; the
/// agent reads it from its configuration and sends it in [`crate::Hello`].
#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct AgentKey(String);

impl AgentKey {
    /// Length of a key in characters.
    pub const LEN: usize = 16;

    /// Generates a new key from the operating system's random source.
    pub fn generate() -> Result<Self> {
        // `ALPHABET` divides 248 evenly, so rejecting bytes at or above it
        // leaves every character equally likely.
        const CEILING: u8 = (256 / ALPHABET.len() * ALPHABET.len()) as u8;

        let mut key = String::with_capacity(Self::LEN);
        let mut buf = [0u8; Self::LEN];

        while key.len() < Self::LEN {
            getrandom::fill(&mut buf)?;

            for byte in buf {
                if byte >= CEILING {
                    continue;
                }

                key.push(ALPHABET[byte as usize % ALPHABET.len()] as char);

                if key.len() == Self::LEN {
                    break;
                }
            }
        }

        Ok(Self(key))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for AgentKey {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        if s.len() != Self::LEN || !s.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(Error::InvalidAgentKey(s.to_string()));
        }

        Ok(Self(s.to_string()))
    }
}

impl<'de> Deserialize<'de> for AgentKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;

        s.parse().map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for AgentKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Keys are credentials, so the debug output is redacted. Use
/// [`AgentKey::as_str`] when a key genuinely needs to be printed.
impl fmt::Debug for AgentKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AgentKey(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_distinct_alphanumeric_keys() {
        let a = AgentKey::generate().unwrap();
        let b = AgentKey::generate().unwrap();

        assert_eq!(a.as_str().len(), AgentKey::LEN);
        assert!(a.as_str().bytes().all(|b| b.is_ascii_alphanumeric()));
        assert_ne!(a, b);
    }

    #[test]
    fn round_trips_through_a_string() {
        let key = AgentKey::generate().unwrap();

        assert_eq!(key.as_str().parse::<AgentKey>().unwrap(), key);
    }

    #[test]
    fn rejects_malformed_keys() {
        for bad in ["", "short", "0123456789abcdefg", "0123456789abcde-"] {
            assert!(bad.parse::<AgentKey>().is_err(), "accepted '{bad}'");
        }
    }

    #[test]
    fn hides_the_key_from_debug_output() {
        let key = AgentKey::generate().unwrap();

        assert!(!format!("{key:?}").contains(key.as_str()));
    }
}
