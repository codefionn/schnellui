//! The app id: the reverse-DNS name the desktop knows this program by.
//!
//! [`set_app_id`] names every window opened afterwards (the Wayland app id and the X11
//! class) and, on Linux outside a sandbox, registers the id with the desktop portal.
//! Portals store permissions such as camera access per app id. An unsandboxed program
//! that never registers shares the empty id with every other one, so a "no" given to
//! any of them refuses it too, without a dialog.

use std::fmt;
use std::sync::OnceLock;

static APP_ID: OnceLock<String> = OnceLock::new();
static PORTAL_ID: OnceLock<String> = OnceLock::new();

/// Why [`set_app_id`] did not fully apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppIdError {
    /// Not a reverse-DNS id such as `org.example.App`. Nothing was set.
    Invalid(String),
    /// An id was set before. It stays.
    AlreadySet(String),
    /// Windows take the id, but the portal refused it, for example because another
    /// portal call came first on the connection. Portal permissions stay shared.
    Portal(String),
}

impl fmt::Display for AppIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(id) => write!(f, "{id:?} is not a reverse-DNS app id"),
            Self::AlreadySet(id) => write!(f, "the app id is already {id}"),
            Self::Portal(why) => write!(f, "the desktop portal did not take the app id: {why}"),
        }
    }
}

impl std::error::Error for AppIdError {}

/// Sets the app id once per process. Call it first in `main`: the portal takes it only
/// as the first call on its connection, so before any clipboard, file chooser, settings
/// or capture portal use.
pub fn set_app_id(id: &str) -> Result<(), AppIdError> {
    if !is_valid_app_id(id) {
        return Err(AppIdError::Invalid(id.to_owned()));
    }
    if APP_ID.set(id.to_owned()).is_err() {
        return Err(AppIdError::AlreadySet(
            app_id().unwrap_or_default().to_owned(),
        ));
    }
    if super::os::register_app_id(id).map_err(AppIdError::Portal)? {
        let _ = PORTAL_ID.set(id.to_owned());
    }
    Ok(())
}

/// The id given to [`set_app_id`], if any.
pub fn app_id() -> Option<&'static str> {
    APP_ID.get().map(String::as_str)
}

/// The id the desktop portal keeps this process's permissions under, when it took
/// ours. `None` on Linux outside a sandbox means the empty id unsandboxed programs
/// share; it is also `None` inside a sandbox and where there is no such portal.
pub fn portal_app_id() -> Option<&'static str> {
    PORTAL_ID.get().map(String::as_str)
}

/// A D-Bus style reverse-DNS name: at least two dot-separated elements of ASCII
/// letters, digits, `_` and `-`, none empty or starting with a digit, 255 bytes at most.
pub fn is_valid_app_id(id: &str) -> bool {
    id.len() <= 255
        && id.split('.').count() >= 2
        && id.split('.').all(|part| {
            !part.is_empty()
                && !part.starts_with(|c: char| c.is_ascii_digit())
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_ids_are_reverse_dns_names() {
        for good in [
            "org.example.App",
            "eu.codefionn.gacast",
            "a.b",
            "io.x-y.z_1",
        ] {
            assert!(is_valid_app_id(good), "{good}");
        }
        let long = format!("a.{}", "b".repeat(254));
        for bad in [
            "gacast",
            "",
            "a..b",
            ".a.b",
            "a.b.",
            "org.1example",
            "a.b c",
            "ä.b",
        ] {
            assert!(!is_valid_app_id(bad), "{bad}");
        }
        assert!(!is_valid_app_id(&long));
    }

    #[test]
    fn an_invalid_id_sets_nothing() {
        assert_eq!(
            set_app_id("no dots"),
            Err(AppIdError::Invalid("no dots".into()))
        );
        assert_eq!(app_id(), None);
        assert_eq!(portal_app_id(), None);
    }
}
