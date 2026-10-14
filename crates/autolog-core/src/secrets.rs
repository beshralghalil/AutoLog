//! Keeping API tokens out of the config file.
//!
//! Secrets go to the operating system's own store — Credential Manager on
//! Windows, the Secret Service on Linux. Where no store can be reached, they
//! are held for the session only and the user is told, because writing a token
//! to a file AutoLog controls would be worse than asking for it again.

use std::collections::HashMap;
use std::sync::Mutex;

use zeroize::Zeroize as _;

use crate::error::{UserError, UserFacing};

/// Where this app's credentials live in the system store.
const SERVICE: &str = "AutoLog";
/// The service name the Python version wrote under, read once so that an
/// upgrade does not make everybody retype their tokens.
const LEGACY_SERVICE: &str = "jira_worklog_app";

/// A credential value that is wiped from memory when dropped and never prints
/// itself, so it cannot reach a log line by accident.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Hand over the value. Named to make a reviewer look twice.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_empty() {
            f.write_str("Secret(empty)")
        } else {
            f.write_str("Secret(redacted)")
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum SecretError {
    #[error("no credential store is available: {0}")]
    Unavailable(String),
    #[error("the credential store refused the request: {0}")]
    Refused(String),
}

impl UserFacing for SecretError {
    fn to_user_error(&self) -> UserError {
        match self {
            Self::Unavailable(reason) => UserError::new(
                "Credentials cannot be saved",
                "No system credential store is available, so tokens will be kept only until AutoLog closes.",
            )
            .with_hint(if cfg!(target_os = "linux") {
                "Install and unlock a keyring such as GNOME Keyring or KWallet to have them remembered."
            } else {
                "Check that Windows Credential Manager is running."
            })
            .with_detail(reason.clone()),
            Self::Refused(reason) => UserError::new(
                "Credential store refused",
                "The system credential store would not answer.",
            )
            .with_hint("It may be locked. Unlock your keyring and try again.")
            .with_detail(reason.clone()),
        }
    }
}

/// The system credential store, with a session-only fallback.
#[derive(Debug)]
pub struct SecretStore {
    /// Why the system store is unusable, when it is.
    unavailable: Option<SecretError>,
    /// Values held in memory only, used when there is no system store.
    session: Mutex<HashMap<String, Secret>>,
}

impl Default for SecretStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore {
    pub fn new() -> Self {
        let unavailable = keyring::Entry::store_status()
            .as_ref()
            .err()
            .map(|error| SecretError::Unavailable(error.to_string()));

        if let Some(error) = &unavailable {
            tracing::warn!("{error}; secrets will not outlive this session");
        }

        Self {
            unavailable,
            session: Mutex::new(HashMap::new()),
        }
    }

    /// The reason secrets will not be remembered, if that is the case. The GUI
    /// shows this once rather than failing silently.
    pub fn unavailable(&self) -> Option<&SecretError> {
        self.unavailable.as_ref()
    }

    /// Read a saved secret, falling back to what the Python version stored.
    pub fn get(&self, consumer: &str, field: &str) -> Secret {
        let key = key(consumer, field);

        if self.unavailable.is_some() {
            return self
                .session
                .lock()
                .map(|session| session.get(&key).cloned().unwrap_or_default())
                .unwrap_or_default();
        }

        read(SERVICE, &key)
            .or_else(|| read(LEGACY_SERVICE, &key))
            .or_else(|| {
                // Credentials saved before consumers were namespaced.
                (consumer == "jira")
                    .then(|| read(LEGACY_SERVICE, field))
                    .flatten()
            })
            .unwrap_or_default()
    }

    /// Save a secret, or forget it when the value is blank.
    pub fn set(&self, consumer: &str, field: &str, value: &Secret) -> Result<(), SecretError> {
        let key = key(consumer, field);

        if self.unavailable.is_some() {
            let mut session = self.session.lock().map_err(|error| {
                SecretError::Refused(format!("session store poisoned: {error}"))
            })?;
            if value.is_empty() {
                session.remove(&key);
            } else {
                session.insert(key, value.clone());
            }
            return Ok(());
        }

        let entry = keyring::Entry::new(SERVICE, &key)
            .map_err(|error| SecretError::Refused(error.to_string()))?;

        if value.is_empty() {
            return match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(error) => Err(SecretError::Refused(error.to_string())),
            };
        }

        entry
            .set_password(value.expose())
            .map_err(|error| SecretError::Refused(error.to_string()))
    }
}

fn key(consumer: &str, field: &str) -> String {
    format!("{consumer}.{field}")
}

fn read(service: &str, key: &str) -> Option<Secret> {
    let entry = keyring::Entry::new(service, key).ok()?;
    match entry.get_password() {
        Ok(password) => Some(Secret::new(password)),
        Err(keyring::Error::NoEntry) => None,
        Err(error) => {
            tracing::debug!(service, "could not read a saved credential: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_never_prints_what_it_holds() {
        let secret = Secret::new("s3cr3t-token");
        assert_eq!(format!("{secret:?}"), "Secret(redacted)");
        assert!(!format!("{secret:?}").contains("s3cr3t"));
        assert_eq!(format!("{:?}", Secret::default()), "Secret(empty)");
    }

    #[test]
    fn whitespace_does_not_count_as_a_value() {
        assert!(Secret::new("   ").is_empty());
        assert!(!Secret::new(" x ").is_empty());
    }

    #[test]
    fn keys_are_namespaced_by_consumer() {
        assert_eq!(key("kimai", "api_token"), "kimai.api_token");
    }

    /// Without a system store the API still works; it just forgets at exit.
    #[test]
    fn the_session_fallback_round_trips() {
        let store = SecretStore {
            unavailable: Some(SecretError::Unavailable("test".to_owned())),
            session: Mutex::new(HashMap::new()),
        };

        assert!(store.get("kimai", "api_token").is_empty());
        store
            .set("kimai", "api_token", &Secret::new("abc"))
            .unwrap();
        assert_eq!(store.get("kimai", "api_token").expose(), "abc");

        store.set("kimai", "api_token", &Secret::default()).unwrap();
        assert!(store.get("kimai", "api_token").is_empty());
        assert!(store.unavailable().is_some());
    }
}
