//! The services Switchboard reads for you, over their own APIs through `net`:
//! GitHub (`github`) and Linear (`linear`). Each finds its credentials the
//! way a person already keeps them, refuses outright while its setting
//! (`github`, `linear`) is off, so turning a service off in Settings means
//! nothing asks it, and turns its failures into `net::ServiceError`s a view
//! can explain.
//!
//! What both share is here: the settings check, and a stored secret read off
//! the async runtime and kept a while, since reading the Keychain is a
//! process on macOS and the security agent inspects every one.

pub mod github;
pub mod linear;

use std::sync::Mutex;

use anyhow::Result;

use crate::net::{ErrorKind, ServiceError};
use crate::{platform, settings, util};

/// Fails with `ErrorKind::Off` while the on/off setting `key` is off.
pub(crate) fn ensure_on(key: &str, service: &str) -> Result<()> {
    if settings::is_on(key) {
        return Ok(());
    }
    Err(ServiceError::new(
        ErrorKind::Off,
        service,
        format!(
            "{service} is turned off in Settings (Integrations → {service}), so nothing asks it."
        ),
    )
    .into())
}

/// A credential found somewhere slow to look (a command, the Keychain), kept
/// for `KEEP_MS` so a lookup every few seconds doesn't look again each time,
/// and forgotten at once when the service refuses it, so a new one is picked
/// up on the next request.
pub(crate) struct Remembered {
    found: Mutex<Option<(Credential, i64)>>,
}

const KEEP_MS: i64 = 10 * 60 * 1000;

/// A token or key, and where it came from, for a message about it.
#[derive(Clone, Debug)]
pub(crate) struct Credential {
    pub value: String,
    pub from: String,
}

impl Remembered {
    pub const fn new() -> Remembered {
        Remembered {
            found: Mutex::new(None),
        }
    }

    pub fn get(&self) -> Option<Credential> {
        let found = self.found.lock().unwrap_or_else(|e| e.into_inner());
        found
            .as_ref()
            .filter(|(_, at)| util::now_ms() - at < KEEP_MS)
            .map(|(credential, _)| credential.clone())
    }

    pub fn set(&self, credential: &Credential) {
        *self.found.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((credential.clone(), util::now_ms()));
    }

    pub fn forget(&self) {
        *self.found.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// A stored secret (`platform::secret`), read on a blocking thread, since it
/// is a Keychain command on macOS and a D-Bus round trip on Linux.
pub(crate) async fn stored_secret(service: &str) -> Option<String> {
    let service = service.to_string();
    tokio::task::spawn_blocking(move || platform::secret(&service))
        .await
        .ok()
        .flatten()
}

/// An environment variable's value, trimmed, when it is set and not empty.
pub(crate) fn env_value(var: &str) -> Option<String> {
    std::env::var(var)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}
