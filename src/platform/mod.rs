//! What differs between macOS and Linux, behind one set of functions, so the
//! rest never asks which it runs on. macOS answers through the commands it
//! ships with (`security`, `pbcopy`, `osascript`, `ioreg`, `caffeinate`,
//! `lsappinfo`, `open`, and `sandbox-exec` for a sealed try, whose rules
//! are `sandbox`'s); Linux through the desktop's own D-Bus services
//! (the Secret Service, notifications, logind) and the tools a desktop ships
//! with, so nothing has to be installed for any of it, which is why Linux
//! has no sealed try. `switchboard platform <what>` gives the shell scripts
//! the same.

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

pub mod sandbox;

/// "macOS" or "Linux", for words shown to the person.
pub const NAME: &str = os::NAME;

/// What a menu calls showing a file in the desktop's file manager
/// (`reveal`): "Open in Finder" on macOS.
pub const REVEAL_LABEL: &str = os::REVEAL_LABEL;

/// What a menu calls opening a file with what the desktop opens it with
/// (`open`): "Open in Preview" on macOS, a picture's viewer there.
pub const VIEWER_LABEL: &str = os::VIEWER_LABEL;

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
/// (a Linux with no desktop session, WSL), the idle time is `None`. Either
/// way the server and the Slack relay fold in the last key pressed in any
/// of the server's clients (src/presence.rs), which the desktop doesn't see
/// from a client on another computer.
/// `SWITCHBOARD_PRESENCE=unknown` has the desktop say nothing, as the tests'
/// servers run, so a test goes by what it typed whatever the computer's own
/// idle time (a CI Mac nobody touches is always away);
/// `SWITCHBOARD_PRESENCE=<idle ms>[,locked]` has it say that, for a test of
/// a computer left alone while the workspace is used from another.
pub fn presence() -> Presence {
    if let Some(said) = std::env::var_os("SWITCHBOARD_PRESENCE") {
        let said = said.to_string_lossy();
        if said == "unknown" {
            return Presence::default();
        }
        if let Some(p) = presence_said(&said) {
            return p;
        }
    }
    os::presence()
}

/// A reading given as `<idle ms>` or `<idle ms>,locked`.
fn presence_said(said: &str) -> Option<Presence> {
    let (idle, locked) = match said.split_once(',') {
        Some((idle, "locked")) => (idle, true),
        Some(_) => return None,
        None => (said, false),
    };
    Some(Presence {
        idle_ms: Some(idle.parse().ok()?),
        locked: Some(locked),
    })
}

/// Keeps the computer from sleeping while it is held (`caffeinate -is` on
/// macOS, a logind sleep inhibitor on Linux); dropped, it lets it sleep.
/// Held by the Slack relay while it runs, and by the server while tabs work
/// (src/awake).
pub struct KeepAwake {
    #[allow(dead_code)]
    inner: Option<os::KeepAwake>,
}

impl KeepAwake {
    /// One that holds nothing, for the unit tests.
    #[cfg(test)]
    pub fn stand_in() -> KeepAwake {
        KeepAwake { inner: None }
    }
}

/// Keeps the computer awake for as long as the value lives, or `None` when
/// that can't be done here. Blocking (it starts `caffeinate`, or asks
/// logind): on the server, call it off its task.
pub fn keep_awake(why: &str) -> Option<KeepAwake> {
    os::keep_awake(why).map(|inner| KeepAwake { inner: Some(inner) })
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

/// The folders rustup's `cargo` is in when it isn't on the PATH, in the
/// order they are looked in: `$CARGO_HOME/bin` (else `~/.cargo/bin`), where
/// rustup.rs installs it, then where Homebrew's rustup keeps it here.
pub fn rustup_dirs() -> Vec<PathBuf> {
    let cargo_home = match std::env::var_os("CARGO_HOME") {
        Some(h) if !h.is_empty() => PathBuf::from(h),
        _ => crate::paths::home_join(".cargo"),
    };
    std::iter::once(cargo_home.join("bin"))
        .chain(os::RUSTUP_DIRS.iter().map(PathBuf::from))
        .collect()
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
/// its owner can read, `{ "<service>": "<secret>" }`. Always on this
/// computer, never in a repository your files are kept in (src/files).
pub fn secrets_file() -> PathBuf {
    crate::paths::local_config_dir().join("secrets.json")
}

/// Saves the clipboard's image to `file` as a PNG, for a picture pasted
/// into a note (crates/notes/src/images.rs): macOS's pasteboard, through
/// JavaScript for Automation; Wayland's or X's clipboard tool on Linux, or
/// Windows' clipboard through PowerShell under WSL.
/// Whether there was an image to save.
pub fn paste_image(file: &std::path::Path) -> bool {
    os::paste_image(file)
}

/// Shows a file without leaving the terminal for an app where the desktop
/// can (Quick Look on macOS); elsewhere opens it as `open` does.
pub fn preview(file: &std::path::Path) -> Result<()> {
    os::preview(file)
}

/// Writes a video's first frame to `out` as a PNG no larger than `size`
/// pixels either way, for the Slack view's videos: Quick Look's thumbnail
/// (`qlmanage -t`) on macOS; Linux has no tool it can count on, so none.
/// Whether it wrote one.
pub fn video_frame(video: &std::path::Path, out: &std::path::Path, size: u32) -> bool {
    os::video_frame(video, out, size)
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

/// Whether a program can be run sealed here (`switchboard try --sealed`):
/// macOS's own `sandbox-exec`. Linux has no sandbox without installing
/// another program, which the error says.
pub fn sandbox_ready() -> Result<(), String> {
    os::sandbox_ready()
}

/// The words that run `program` under the sandbox profile in the file
/// `profile` (src/platform/sandbox.rs writes its rules), everything it
/// starts in it too; the program's own arguments follow. Its environment is
/// the caller's to make from nothing.
pub fn sandboxed(
    profile: &std::path::Path,
    program: &std::path::Path,
) -> Result<Vec<String>, String> {
    os::sandboxed(profile, program)
}

/// Your per-user temporary and cache folders, as a real path, where the
/// computer keeps them apart from `/tmp` (macOS's `DARWIN_USER_DIR`'s
/// parent), which a sealed try may not read.
pub fn user_dir() -> Option<PathBuf> {
    os::user_dir()
}

/// For a sealed try's build: the folders put first on its PATH and the
/// environment it adds, so nothing in the sandbox finds a tool through a
/// shim that reads or writes outside it (on macOS the C compiler's and
/// git's own folders, not /usr/bin's xcrun shims, and the SDK's path).
pub fn sealed_build() -> (Vec<PathBuf>, Vec<(String, String)>) {
    os::sealed_build()
}

/// Makes Option act as Alt in `app`'s default profile, through macOS's own
/// preferences (`defaults export` and `import`, as setup.sh did), so the
/// app takes the change in its next window, or once it starts again.
pub fn set_option_as_alt(app: TerminalApp) -> Result<()> {
    os::set_option_as_alt(app)
}

/// Clones the folder `from` as `to`, which must not exist, copy-on-write, so
/// the copy takes no space until one side changes a file: APFS's
/// `clonefile` on macOS, `FICLONE` file by file on Linux (Btrfs, XFS). Where
/// the filesystem can't, or the two are on different volumes, it fails,
/// leaving nothing at `to`; it never falls back to a plain copy.
pub fn clone_tree(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    os::clone_tree(from, to)
}

// ── Tailscale ───────────────────────────────────────────────────────────────

/// Tailscale's command line: `SWITCHBOARD_TAILSCALE` where the tests set
/// it, else `tailscale` on the PATH, else where the platform's app keeps it
/// (on macOS the app's own binary, which runs as its command line). Only
/// the phone app (src/phone) uses Tailscale.
fn tailscale_bin() -> Result<PathBuf> {
    if let Some(bin) = std::env::var_os("SWITCHBOARD_TAILSCALE").filter(|b| !b.is_empty()) {
        return Ok(PathBuf::from(bin));
    }
    crate::util::which("tailscale")
        .or_else(|| {
            os::TAILSCALE_PATHS
                .iter()
                .map(PathBuf::from)
                .find(|p| p.is_file())
        })
        .ok_or_else(|| anyhow::anyhow!("Tailscale isn't installed"))
}

fn tailscale(args: &[&str]) -> Result<String> {
    crate::util::run(&tailscale_bin()?.to_string_lossy(), args)
}

/// `tailscale status --json`: whether it runs, this computer's addresses
/// and name on the tailnet, and the names it may get certificates for.
pub fn tailscale_status() -> Result<serde_json::Value> {
    Ok(serde_json::from_str(&tailscale(&["status", "--json"])?)?)
}

/// `tailscale cert`: the certificate for `domain` (this computer's
/// `*.ts.net` name), its chain and its key, as PEM printed to stdout
/// (`-` for both files). Printed rather than written to a file of ours,
/// since the App Store's Tailscale is sandboxed and can't write outside
/// its own container.
pub fn tailscale_cert(domain: &str) -> Result<String> {
    tailscale(&["cert", "--cert-file=-", "--key-file=-", domain])
}

/// `tailscale whois --json <ip>`: the device at a tailnet address and who
/// is signed in on it.
pub fn tailscale_whois(ip: &str) -> Result<serde_json::Value> {
    Ok(serde_json::from_str(&tailscale(&["whois", "--json", ip])?)?)
}
