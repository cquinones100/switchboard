//! The services Switchboard reads for you, over their own APIs through `net`:
//! GitHub (`github`) and Linear (`linear`). Each finds its credentials the
//! way a person already keeps them, refuses outright while its setting
//! (`github`, `linear`) is off, so turning a service off in Settings means
//! the workspace asks it nothing by itself, and turns its failures into
//! `net::ServiceError`s a view can explain.
//!
//! The settings are about what the workspace does on its own: its lookups,
//! views and keys. A program you run yourself (`codex-review`, `wt`, `wtpr`,
//! `wait-deploy`, `wt-jump`, `worktree-done`) asks the service whatever they
//! say, as the tmux version's scripts did through gh and curl, which never
//! read them; such a program calls `run_by_you` first.
//!
//! What both share is here: the settings check, and a stored secret read off
//! the async runtime and kept a while, since reading the Keychain is a
//! process on macOS and the security agent inspects every one.

pub mod github;
pub mod linear;

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;

use crate::net::{ErrorKind, ServiceError};
use crate::{platform, settings, util};

// Set for the whole of a program someone runs themselves, never in the server.
static RUN_BY_YOU: AtomicBool = AtomicBool::new(false);

/// Marks this process as a program someone runs themselves, whose requests
/// the `github` and `linear` settings don't stop.
pub fn run_by_you() {
    RUN_BY_YOU.store(true, Ordering::Relaxed);
}

/// Fails with `ErrorKind::Off` while the on/off setting `key` is off, unless
/// this is a program someone runs themselves (`run_by_you`).
pub(crate) fn ensure_on(key: &str, service: &str) -> Result<()> {
    if RUN_BY_YOU.load(Ordering::Relaxed) || settings::is_on(key) {
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
