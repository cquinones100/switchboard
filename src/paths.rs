//! Where things are. Everything finds itself through `~` (`$HOME`), as the
//! Node scripts did with `os.homedir()`, so the tests (and a try of a work in
//! progress) can give a run a home of its own. The binary needs nothing
//! beside it: what the repository holds for it (the settings schema, the
//! skills) is compiled in from `assets/`.

use std::env;
use std::path::{Path, PathBuf};

/// `$HOME`, read every time, so a test or a try that changes it is followed.
pub fn home() -> PathBuf {
    match env::var_os("HOME") {
        Some(h) if !h.is_empty() => PathBuf::from(h),
        _ => env::home_dir().unwrap_or_else(|| PathBuf::from("/")),
    }
}

/// A path under the home directory: `home_join(".config/switchboard")`.
pub fn home_join(rel: impl AsRef<Path>) -> PathBuf {
    home().join(rel)
}

/// Someone's own files: `~/.config/switchboard`.
pub fn config_dir() -> PathBuf {
    home_join(".config/switchboard")
}

/// Switchboard's own state: `SWITCHBOARD_STATE_DIR`, else
/// `~/.local/state/switchboard`. The server's socket is in it, so it is kept
/// short: a Unix socket's path can't be over 104 bytes on macOS.
pub fn state_dir() -> PathBuf {
    match env::var_os("SWITCHBOARD_STATE_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => home_join(".local/state/switchboard"),
    }
}

/// `~/.claude`.
pub fn claude_dir() -> PathBuf {
    home_join(".claude")
}

/// `~/.cache/switchboard`.
pub fn cache_dir() -> PathBuf {
    home_join(".cache/switchboard")
}

/// A path with the home directory's first occurrence shown as `~`, as the
/// Node scripts wrote `file.replace(os.homedir(), "~")`.
pub fn tilde(path: impl AsRef<Path>) -> String {
    let text = path.as_ref().to_string_lossy().into_owned();
    let home = home().to_string_lossy().into_owned();
    if home.is_empty() || home == "/" {
        return text;
    }
    text.replacen(&home, "~", 1)
}
