//! What differs between macOS and Linux, behind one set of functions, so the
//! rest never asks which it runs on. macOS answers through the commands it
//! ships with (`security`, `pbcopy`, `osascript`, `ioreg`, `caffeinate`,
//! `lsappinfo`, `open`); Linux through the desktop's own D-Bus services
//! (the Secret Service, notifications, logind) and the tools a desktop ships
//! with, so nothing has to be installed for any of it. `switchboard platform <what>` gives the shell scripts the same.

use std::path::PathBuf;

use anyhow::Result;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as os;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as os;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("Switchboard runs on macOS and Linux");

/// "macOS" or "Linux", for words shown to the person.
pub const NAME: &str = os::NAME;

/// A secret stored for `service` (the Keychain's generic password with that
/// service; on Linux the Secret Service's item with that `service`
/// attribute, else `~/.config/switchboard/secrets.json`), trimmed, or `None`.
pub fn secret(service: &str) -> Option<String> {
    os::secret(service, None)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// As `secret`, for the item stored under `account` as well.
pub fn secret_for(service: &str, account: &str) -> Option<String> {
    os::secret(service, Some(account))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Stores a secret for `service` and `account`, replacing one there. Gives
/// where it went, in words.
pub fn set_secret(service: &str, account: &str, value: &str) -> Result<String> {
    os::set_secret(service, account, value)
}

/// Puts text on the desktop's clipboard. Where there is no clipboard to reach
/// (over SSH, a Linux with no desktop), this fails, and the server hands the
/// text to its clients' terminals instead (OSC 52), which put it on the
/// clipboard of the computer you are at.
pub fn copy(text: &str) -> Result<()> {
    os::copy(text)
}

/// The clipboard's text, or `None`.
pub fn paste() -> Option<String> {
    os::paste()
}

/// A desktop notification (a macOS banner), titled `title`. Never fails: a
/// desktop with no notifications shows none.
pub fn notify(title: &str, text: &str) {
    os::notify(title, text)
}

/// Whether you're at the computer, as it can be read here: the time since
/// the last keyboard or mouse input and whether the screen is locked, either
/// `None` when it can't be read.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Presence {
    pub idle_ms: Option<f64>,
    pub locked: Option<bool>,
}

/// Reads the idle time and the lock. Blocking: call it from
/// `tokio::task::spawn_blocking` on a runtime. Where the desktop can't say
/// (a Linux with no desktop session, WSL), the idle time is `None`, and the
/// server goes by the last key pressed in any of its clients instead.
pub fn presence() -> Presence {
    os::presence()
}

/// Keeps the computer from sleeping while it is held (`caffeinate -is` on
/// macOS, a logind sleep inhibitor on Linux); dropped, it lets it sleep.
pub struct KeepAwake {
    #[allow(dead_code)]
    inner: os::KeepAwake,
}

/// Keeps the computer awake for as long as the value lives, or `None` when
/// that can't be done here.
pub fn keep_awake(why: &str) -> Option<KeepAwake> {
    os::keep_awake(why).map(|inner| KeepAwake { inner })
}

/// Claude Code's managed settings, the file an organization's policy puts
/// on the computer for every user of it: `/Library/Application Support/
/// ClaudeCode/managed-settings.json` on macOS, `/etc/claude-code/
/// managed-settings.json` on Linux. `SWITCHBOARD_CLAUDE_MANAGED_SETTINGS`
/// names another file in its place, which the tests set, so none reads the
/// computer's own.
pub fn claude_managed_settings() -> PathBuf {
    match std::env::var_os("SWITCHBOARD_CLAUDE_MANAGED_SETTINGS") {
        Some(f) if !f.is_empty() => PathBuf::from(f),
        _ => PathBuf::from(os::CLAUDE_MANAGED_SETTINGS),
    }
}

/// Chrome's `Local State` file, which lists its profiles, where Chrome (or
/// Chromium) keeps it here.
pub fn chrome_local_state() -> Option<PathBuf> {
    os::chrome_local_state()
}

/// The app in front, by name, or `None` when it can't be told (always on
/// Linux): for waiting until a link handed to the browser has opened.
pub fn front_app() -> Option<String> {
    os::front_app()
}

/// Opens a file or link with what the desktop opens it with (`open`;
/// `xdg-open`, or Windows' own under WSL). Links Switchboard opens go
/// through its Chrome-profile chooser (`browser.json`) first.
pub fn open(target: &str) -> Result<()> {
    os::open(target)
}

/// Shows a file in its folder in the desktop's file manager (`open -R`,
/// Finder with the file chosen; on Linux the folder opened), for the media
/// picker's Ctrl-O.
pub fn reveal(file: &std::path::Path) -> Result<()> {
    os::reveal(file)
}

/// Where secrets go on a computer with no keyring to keep them: a file only
/// its owner can read, `{ "<service>": "<secret>" }`.
pub fn secrets_file() -> PathBuf {
    crate::paths::config_dir().join("secrets.json")
}

/// Saves the clipboard's image to `file` as a PNG, for a picture pasted
/// into a note (src/notes/images.rs): macOS's pasteboard, through
/// JavaScript for Automation; Wayland's or X's clipboard tool on Linux.
/// Whether there was an image to save.
pub fn paste_image(file: &std::path::Path) -> bool {
    os::paste_image(file)
}

/// Shows a file without leaving the terminal for an app where the desktop
/// can (Quick Look on macOS); elsewhere opens it as `open` does.
pub fn preview(file: &std::path::Path) -> Result<()> {
    os::preview(file)
}

/// A terminal app whose own preferences can make Option act as Alt, which
/// every `M-` key needs on a Mac.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalApp {
    /// macOS's Terminal: its default profile's "Use Option as Meta key".
    Terminal,
    /// iTerm2: its default profile's Option keys sending Esc+.
    ITerm,
}

/// Whether `app`'s default profile makes Option act as Alt, with that
/// profile's name; `None` where its preferences can't be read (it was never
/// opened, or this isn't a Mac).
pub fn option_as_alt(app: TerminalApp) -> Option<(bool, String)> {
    os::option_as_alt(app)
}

/// Makes Option act as Alt in `app`'s default profile, through macOS's own
/// preferences (`defaults export` and `import`, as setup.sh did), so the
/// app takes the change in its next window, or once it starts again.
pub fn set_option_as_alt(app: TerminalApp) -> Result<()> {
    os::set_option_as_alt(app)
}
