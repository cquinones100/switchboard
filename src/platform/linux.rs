//! Linux, through the desktop's D-Bus services (the Secret Service,
//! notifications, logind, GNOME's and KDE's idle monitors) and the tools a
//! desktop ships with, secrets falling back to a private file where there is
//! no keyring.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Result, anyhow};
use serde_json::Value;
use zbus::{Connection, OwnedFd, OwnedObjectPath, OwnedVariant, Proxy, Variant};

use super::Presence;
use crate::{paths, util};

pub const NAME: &str = "Linux";

// ── Secrets ─────────────────────────────────────────────────────────────────

const SECRETS: &str = "org.freedesktop.secrets";
const SECRETS_PATH: &str = "/org/freedesktop/secrets";
const SECRETS_SERVICE: &str = "org.freedesktop.Secret.Service";

/// The Secret Service's item with these attributes, when the desktop has a
/// keyring (GNOME Keyring, KWallet) and it is unlocked; a locked one isn't
/// asked to unlock, since nothing here can answer its prompt.
fn secret_service_get(attributes: &HashMap<&str, &str>) -> Option<String> {
    zbus::block_on(async {
        let conn = Connection::session().await.ok()?;
        let service = Proxy::new(&conn, SECRETS, SECRETS_PATH, SECRETS_SERVICE)
            .await
            .ok()?;
        let (_, session): (OwnedVariant, OwnedObjectPath) = service
            .call("OpenSession", &("plain", Variant::from("")))
            .await
            .ok()?;
        let (unlocked, _locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) =
            service.call("SearchItems", &(attributes,)).await.ok()?;
        let item = unlocked.into_iter().next()?;
        let secrets: HashMap<OwnedObjectPath, (OwnedObjectPath, Vec<u8>, Vec<u8>, String)> =
            service
                .call("GetSecrets", &(vec![item.clone()], &session))
                .await
                .ok()?;
        let _ = close_session(&conn, &session).await;
        let (_, _, value, _) = secrets.get(&item)?;
        String::from_utf8(value.clone()).ok()
    })
}

async fn close_session(conn: &Connection, session: &OwnedObjectPath) -> zbus::Result<()> {
    let proxy = Proxy::new(
        conn,
        SECRETS,
        session.as_str(),
        "org.freedesktop.Secret.Session",
    )
    .await?;
    proxy.call::<_, _, ()>("Close", &()).await
}

fn secret_service_set(attributes: &HashMap<&str, &str>, label: &str, value: &str) -> Result<()> {
    zbus::block_on(async {
        let conn = Connection::session().await?;
        let service = Proxy::new(&conn, SECRETS, SECRETS_PATH, SECRETS_SERVICE).await?;
        let (_, session): (OwnedVariant, OwnedObjectPath) = service
            .call("OpenSession", &("plain", Variant::from("")))
            .await?;
        let collection = Proxy::new(
            &conn,
            SECRETS,
            "/org/freedesktop/secrets/aliases/default",
            "org.freedesktop.Secret.Collection",
        )
        .await?;
        let mut properties: HashMap<&str, Variant> = HashMap::new();
        properties.insert("org.freedesktop.Secret.Item.Label", Variant::from(label));
        properties.insert(
            "org.freedesktop.Secret.Item.Attributes",
            Variant::from(attributes.clone()),
        );
        let secret = (
            session.clone(),
            Vec::<u8>::new(),
            value.as_bytes().to_vec(),
            "text/plain",
        );
        let (_, prompt): (OwnedObjectPath, OwnedObjectPath) = collection
            .call("CreateItem", &(properties, secret, true))
            .await?;
        let _ = close_session(&conn, &session).await;
        if prompt.as_str() != "/" {
            return Err(anyhow!("the keyring is locked"));
        }
        Ok(())
    })
}

fn file_secrets() -> serde_json::Map<String, Value> {
    util::read_json_object(super::secrets_file())
}

pub fn secret(service: &str, account: Option<&str>) -> Option<String> {
    let mut attributes = HashMap::from([("service", service)]);
    if let Some(a) = account {
        attributes.insert("account", a);
    }
    secret_service_get(&attributes).or_else(|| {
        file_secrets()
            .get(service)
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

pub fn set_secret(service: &str, account: &str, value: &str) -> Result<String> {
    let attributes = HashMap::from([("service", service), ("account", account)]);
    if secret_service_set(&attributes, &format!("Switchboard: {service}"), value).is_ok() {
        return Ok(format!("the keyring, as {service}"));
    }
    // No keyring to keep it (a server, WSL): a file only you can read.
    let file = super::secrets_file();
    let mut secrets = file_secrets();
    secrets.insert(service.to_string(), Value::from(value));
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension("json.tmp");
    util::write_private(
        &tmp,
        format!("{}\n", util::json_pretty(&Value::Object(secrets))),
        false,
    )?;
    std::fs::rename(&tmp, &file)?;
    Ok(paths::tilde(&file))
}

// ── The clipboard ───────────────────────────────────────────────────────────

fn wsl() -> bool {
    std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .is_ok_and(|r| r.to_lowercase().contains("microsoft"))
}

fn env_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

/// The clipboard tool this desktop has, with its arguments for copying and
/// pasting: Wayland's, X's, or Windows' under WSL.
fn clipboard_tool() -> Option<(
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
)> {
    let tools: [(&str, &[&str], &[&str], bool); 4] = [
        ("wl-copy", &[], &[], env_set("WAYLAND_DISPLAY")),
        (
            "xclip",
            &["-selection", "clipboard"],
            &["-selection", "clipboard", "-o"],
            env_set("DISPLAY"),
        ),
        (
            "xsel",
            &["--clipboard", "--input"],
            &["--clipboard", "--output"],
            env_set("DISPLAY"),
        ),
        ("clip.exe", &[], &[], wsl()),
    ];
    tools
        .into_iter()
        .find(|(name, _, _, usable)| *usable && util::which(name).is_some())
        .map(|(n, c, p, _)| (n, c, p))
}

pub fn copy(text: &str) -> Result<()> {
    let (name, args, _) = clipboard_tool().ok_or_else(|| anyhow!("no clipboard here"))?;
    util::run_with_input(Command::new(name).args(args), text.as_bytes()).map(|_| ())
}

pub fn paste() -> Option<String> {
    match clipboard_tool() {
        Some(("wl-copy", _, _)) => util::try_run("wl-paste", ["-n"]),
        Some(("clip.exe", _, _)) => util::try_run(
            "powershell.exe",
            ["-NoProfile", "-Command", "Get-Clipboard"],
        )
        .map(|t| t.replace("\r\n", "\n").trim_end_matches('\n').to_string()),
        Some((name, _, args)) => util::try_run(name, args),
        None => None,
    }
}

// ── Notifications ───────────────────────────────────────────────────────────

pub fn notify(title: &str, text: &str) {
    let _ = zbus::block_on(async {
        let conn = Connection::session().await?;
        let proxy = Proxy::new(
            &conn,
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
        )
        .await?;
        let hints: HashMap<&str, Variant> = HashMap::new();
        proxy
            .call::<_, _, u32>(
                "Notify",
                &(
                    "Switchboard",
                    0u32,
                    "",
                    title,
                    text,
                    Vec::<&str>::new(),
                    hints,
                    -1i32,
                ),
            )
            .await
    });
}

// ── Presence ────────────────────────────────────────────────────────────────

pub fn presence() -> Presence {
    zbus::block_on(async {
        let session = Connection::session().await.ok();
        let system = Connection::system().await.ok();
        let logind = match &system {
            Some(conn) => Proxy::new(
                conn,
                "org.freedesktop.login1",
                "/org/freedesktop/login1/session/auto",
                "org.freedesktop.login1.Session",
            )
            .await
            .ok(),
            None => None,
        };
        let mut idle_ms = None;
        let mut screensaver_active = None;
        if let Some(conn) = &session {
            // GNOME's idle monitor, to the millisecond.
            if let Ok(p) = Proxy::new(
                conn,
                "org.gnome.Mutter.IdleMonitor",
                "/org/gnome/Mutter/IdleMonitor/Core",
                "org.gnome.Mutter.IdleMonitor",
            )
            .await
            {
                idle_ms = p
                    .call::<_, _, u64>("GetIdletime", &())
                    .await
                    .ok()
                    .map(|ms| ms as f64);
            }
            // KDE's (and others') screensaver, to the second.
            if let Ok(p) = Proxy::new(
                conn,
                "org.freedesktop.ScreenSaver",
                "/org/freedesktop/ScreenSaver",
                "org.freedesktop.ScreenSaver",
            )
            .await
            {
                if idle_ms.is_none() {
                    idle_ms = p
                        .call::<_, _, u32>("GetSessionIdleTime", &())
                        .await
                        .ok()
                        .map(|s| s as f64 * 1000.0);
                }
                screensaver_active = p.call::<_, _, bool>("GetActive", &()).await.ok();
            }
        }
        let mut locked = None;
        if let Some(p) = &logind {
            locked = p.get_property::<bool>("LockedHint").await.ok();
            if idle_ms.is_none() {
                // logind only knows idle past the desktop's own threshold.
                if let (Ok(idle), Ok(since)) = (
                    p.get_property::<bool>("IdleHint").await,
                    p.get_property::<u64>("IdleSinceHint").await,
                ) {
                    idle_ms = Some(if idle && since > 0 {
                        ((util::now_ms() * 1000 - since as i64).max(0) / 1000) as f64
                    } else {
                        0.0
                    });
                }
            }
        }
        Presence {
            idle_ms,
            locked: locked.or(screensaver_active),
        }
    })
}

// ── Staying awake ───────────────────────────────────────────────────────────

/// A logind sleep inhibitor: the computer stays awake while its descriptor is open.
pub struct KeepAwake(#[allow(dead_code)] OwnedFd);

pub fn keep_awake(why: &str) -> Option<KeepAwake> {
    zbus::block_on(async {
        let conn = Connection::system().await.ok()?;
        let manager = Proxy::new(
            &conn,
            "org.freedesktop.login1",
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
        )
        .await
        .ok()?;
        let fd: OwnedFd = manager
            .call("Inhibit", &("sleep:idle", "Switchboard", why, "block"))
            .await
            .ok()?;
        Some(KeepAwake(fd))
    })
}

// ── Apps ────────────────────────────────────────────────────────────────────

pub const CLAUDE_MANAGED_SETTINGS: &str = "/etc/claude-code/managed-settings.json";

pub fn chrome_local_state() -> Option<PathBuf> {
    [
        "google-chrome",
        "chromium",
        "google-chrome-beta",
        "google-chrome-unstable",
    ]
    .iter()
    .map(|dir| paths::home_join(format!(".config/{dir}/Local State")))
    .find(|p| p.is_file())
    .or_else(|| Some(paths::home_join(".config/google-chrome/Local State")))
}

pub fn front_app() -> Option<String> {
    None
}

pub fn open(target: &str) -> Result<()> {
    let quiet = |name: &str, args: &[&str]| {
        Command::new(name)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    if wsl() {
        if util::which("wslview").is_some() && quiet("wslview", &[target]) {
            return Ok(());
        }
        // Windows opens a link, or a file by its Windows path.
        let windows_path = if target.contains("://") {
            Some(target.to_string())
        } else {
            util::try_run("wslpath", ["-w", target]).map(|p| p.trim().to_string())
        };
        if let Some(t) = windows_path {
            // explorer.exe exits 1 even when it opened it.
            if Command::new("explorer.exe")
                .arg(&t)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok()
            {
                return Ok(());
            }
        }
    }
    if util::which("xdg-open").is_some() && quiet("xdg-open", &[target]) {
        return Ok(());
    }
    if util::which("gio").is_some() && quiet("gio", &["open", target]) {
        return Ok(());
    }
    Err(anyhow!("nothing here opens {target}: install xdg-utils"))
}

/// The file's folder, opened as `open` opens anything: Linux's file
/// managers have no one way to be told which file to choose in it.
pub fn reveal(file: &std::path::Path) -> Result<()> {
    let folder = file.parent().unwrap_or(file);
    open(&folder.to_string_lossy())
}

/// The clipboard's PNG, through Wayland's or X's clipboard tool.
pub fn paste_image(file: &std::path::Path) -> bool {
    let out = match clipboard_tool() {
        Some(("wl-copy", _, _)) => Command::new("wl-paste")
            .args(["--type", "image/png"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok(),
        Some(("xclip", _, _)) => Command::new("xclip")
            .args(["-selection", "clipboard", "-t", "image/png", "-o"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok(),
        _ => None,
    };
    match out {
        Some(out) if out.status.success() && out.stdout.starts_with(b"\x89PNG") => {
            std::fs::write(file, out.stdout).is_ok()
        }
        _ => false,
    }
}

/// Linux has no Quick Look: the file opens in what the desktop opens it with.
pub fn preview(file: &std::path::Path) -> Result<()> {
    open(&file.to_string_lossy())
}

// ── Option as Alt ───────────────────────────────────────────────────────────

/// Option is a Mac's key: a Linux terminal sends Alt as it is.
pub fn option_as_alt(_app: super::TerminalApp) -> Option<(bool, String)> {
    None
}

pub fn set_option_as_alt(_app: super::TerminalApp) -> Result<()> {
    Err(anyhow!("Option as Alt is a Mac terminal's setting"))
}

// ── The sandbox ─────────────────────────────────────────────────────────────

// Linux has namespaces and seccomp, but a sandbox made of them is a program
// of its own to install (bubblewrap, firejail), which Switchboard doesn't
// ask for, so someone else's code isn't run sealed here.
pub fn sandbox_ready() -> Result<(), String> {
    Err("--sealed needs a sandbox, and Linux has none without installing another program (macOS has sandbox-exec built in), so someone else's code isn't run here: read it with switchboard pr-risk, review it with switchboard codex-review -C, and try it on a Mac".into())
}

pub fn sandboxed(
    _profile: &std::path::Path,
    _program: &std::path::Path,
) -> Result<Vec<String>, String> {
    sandbox_ready().map(|_| Vec::new())
}

pub fn user_dir() -> Option<PathBuf> {
    None
}

pub fn sealed_build() -> (Vec<PathBuf>, Vec<(String, String)>) {
    (Vec::new(), Vec::new())
}
