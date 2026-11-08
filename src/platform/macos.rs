//! macOS, through the commands it ships with.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::LazyLock;

use anyhow::{Result, anyhow};
use regex::Regex;

use super::Presence;
use crate::util;

pub const NAME: &str = "macOS";

pub fn secret(service: &str, account: Option<&str>) -> Option<String> {
    let mut args = vec!["find-generic-password"];
    if let Some(a) = account {
        args.extend(["-a", a]);
    }
    args.extend(["-s", service, "-w"]);
    util::try_run("security", args)
}

pub fn set_secret(service: &str, account: &str, value: &str) -> Result<String> {
    util::run(
        "security",
        [
            "add-generic-password",
            "-U",
            "-a",
            account,
            "-s",
            service,
            "-w",
            value,
        ],
    )?;
    Ok(format!("the Keychain, as {service}"))
}

pub fn copy(text: &str) -> Result<()> {
    util::run_with_input(&mut Command::new("pbcopy"), text.as_bytes())
        .map(|_| ())
        .map_err(|_| anyhow!("Couldn't copy: no pbcopy."))
}

pub fn paste() -> Option<String> {
    util::try_run("pbpaste", [] as [&str; 0])
}

pub fn notify(title: &str, text: &str) {
    // The text and title as arguments, so neither is read as AppleScript.
    let _ = Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 1 of argv) with title (item 2 of argv)",
            "-e",
            "end run",
            text,
            title,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

// macOS keeps the time since the last keyboard or mouse input (HIDIdleTime, in
// nanoseconds, on IOHIDSystem), and marks the console session locked
// (CGSSessionScreenIsLocked on the IORegistry's root) while the lock screen is
// up; ioreg reads both without any permission.
static IDLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""HIDIdleTime" = (\d+)"#).unwrap());
static LOCKED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""CGSSessionScreenIsLocked"\s*=\s*Yes"#).unwrap());

pub fn presence() -> Presence {
    let idle_ms = util::try_run("ioreg", ["-c", "IOHIDSystem", "-d", "4"])
        .and_then(|out| IDLE.captures(&out).and_then(|c| c[1].parse::<f64>().ok()))
        .map(|ns| ns / 1e6);
    let locked = util::try_run("ioreg", ["-n", "Root", "-d", "1"]).map(|out| LOCKED.is_match(&out));
    Presence { idle_ms, locked }
}

pub struct KeepAwake(Child);

impl Drop for KeepAwake {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn keep_awake(_why: &str) -> Option<KeepAwake> {
    // -w: caffeinate ends with this process, however it ends.
    Command::new("caffeinate")
        .args(["-is", "-w", &std::process::id().to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
        .map(KeepAwake)
}

pub const CLAUDE_MANAGED_SETTINGS: &str =
    "/Library/Application Support/ClaudeCode/managed-settings.json";

pub fn chrome_local_state() -> Option<PathBuf> {
    Some(crate::paths::home_join(
        "Library/Application Support/Google/Chrome/Local State",
    ))
}

pub fn front_app() -> Option<String> {
    // From macOS's Launch Services.
    util::try_run(
        "/bin/sh",
        ["-c", r#"lsappinfo info -only name "$(lsappinfo front)""#],
    )
    .map(|s| s.trim().to_string())
}

pub fn open(target: &str) -> Result<()> {
    util::run("open", [target]).map(|_| ())
}

pub fn reveal(file: &std::path::Path) -> Result<()> {
    util::run("open", ["-R", &file.to_string_lossy()]).map(|_| ())
}

/// The pasteboard's PNG, or its TIFF made a PNG, as a screenshot copied
/// puts both there (notes/lua/notes/images.lua's `grab`). AppleScript's
/// `the clipboard as «class PNGf»` can't read an image the screenshot tool
/// hands over only when asked.
const GRAB: &str = r#"
ObjC.import("AppKit");
function run(argv) {
  const pb = $.NSPasteboard.generalPasteboard;
  let data = pb.dataForType("public.png");
  if (data.isNil()) {
    const tiff = pb.dataForType("public.tiff");
    if (tiff.isNil()) return "none";
    const rep = $.NSBitmapImageRep.imageRepWithData(tiff);
    if (rep.isNil()) return "none";
    data = rep.representationUsingTypeProperties($.NSBitmapImageFileTypePNG, $({}));
  }
  return data.writeToFileAtomically(argv[0], true) ? "ok" : "none";
}
"#;

pub fn paste_image(file: &std::path::Path) -> bool {
    let path = file.to_string_lossy();
    util::try_run("osascript", ["-l", "JavaScript", "-e", GRAB, &path])
        .is_some_and(|out| out.trim() == "ok")
        && file.is_file()
}

/// Quick Look, without leaving the terminal for an app.
pub fn preview(file: &std::path::Path) -> Result<()> {
    Command::new("qlmanage")
        .arg("-p")
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| anyhow!("Couldn't show it: {e}"))
}

// ── Option as Alt ───────────────────────────────────────────────────────────

const PLISTBUDDY: &str = "/usr/libexec/PlistBuddy";

/// An app's preferences, exported to a file of their own for PlistBuddy to
/// read and change, removed when dropped.
struct Exported {
    domain: &'static str,
    file: PathBuf,
}

impl Exported {
    fn of(app: super::TerminalApp) -> Option<Exported> {
        let domain = match app {
            super::TerminalApp::Terminal => "com.apple.Terminal",
            super::TerminalApp::ITerm => "com.googlecode.iterm2",
        };
        let file =
            std::env::temp_dir().join(format!("switchboard-{domain}-{}.plist", std::process::id()));
        let exported = Exported { domain, file };
        util::run(
            "defaults",
            ["export", domain, &exported.file.to_string_lossy()],
        )
        .ok()?;
        Some(exported)
    }

    /// PlistBuddy's answer to one command, trimmed.
    fn ask(&self, command: &str) -> Option<String> {
        util::try_run(PLISTBUDDY, ["-c", command, &self.file.to_string_lossy()])
            .map(|s| s.trim().to_string())
    }

    /// Sets an entry, adding it as `kind` when it isn't there.
    fn set(&self, at: &str, kind: &str, value: &str) -> Result<()> {
        let file = self.file.to_string_lossy();
        if util::run(PLISTBUDDY, ["-c", &format!("Set {at} {value}"), &file]).is_err() {
            util::run(
                PLISTBUDDY,
                ["-c", &format!("Add {at} {kind} {value}"), &file],
            )?;
        }
        Ok(())
    }

    fn import(&self) -> Result<()> {
        util::run(
            "defaults",
            ["import", self.domain, &self.file.to_string_lossy()],
        )
        .map(|_| ())
    }
}

impl Drop for Exported {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
    }
}

/// PlistBuddy wants a space in a key escaped.
fn plist_key(key: &str) -> String {
    key.replace(' ', "\\ ")
}

/// The default profile's entries that make Option act as Alt, its name,
/// and whether they are set.
fn option_entries(
    prefs: &Exported,
    app: super::TerminalApp,
) -> Option<(Vec<String>, String, bool)> {
    match app {
        super::TerminalApp::Terminal => {
            let profile = prefs.ask("Print :Default\\ Window\\ Settings")?;
            let at = format!(
                ":Window\\ Settings:{}:useOptionAsMetaKey",
                plist_key(&profile)
            );
            let set = prefs.ask(&format!("Print {at}")).as_deref() == Some("true");
            Some((vec![at], profile, set))
        }
        super::TerminalApp::ITerm => {
            let guid = prefs.ask("Print :Default\\ Bookmark\\ Guid")?;
            let mut i = 0;
            loop {
                let found = prefs.ask(&format!("Print :New\\ Bookmarks:{i}:Guid"))?;
                if found == guid {
                    let at = format!(":New\\ Bookmarks:{i}");
                    let name = prefs
                        .ask(&format!("Print {at}:Name"))
                        .unwrap_or_else(|| "Default".into());
                    let keys: Vec<String> =
                        ["Option\\ Key\\ Sends", "Right\\ Option\\ Key\\ Sends"]
                            .iter()
                            .map(|k| format!("{at}:{k}"))
                            .collect();
                    let set = keys
                        .iter()
                        .all(|k| prefs.ask(&format!("Print {k}")).as_deref() == Some("2"));
                    return Some((keys, name, set));
                }
                i += 1;
            }
        }
    }
}

pub fn option_as_alt(app: super::TerminalApp) -> Option<(bool, String)> {
    let prefs = Exported::of(app)?;
    let (_, profile, set) = option_entries(&prefs, app)?;
    Some((set, profile))
}

pub fn set_option_as_alt(app: super::TerminalApp) -> Result<()> {
    let prefs = Exported::of(app).ok_or_else(|| anyhow!("couldn't read its preferences"))?;
    let (entries, _, _) =
        option_entries(&prefs, app).ok_or_else(|| anyhow!("couldn't find its default profile"))?;
    for at in entries {
        match app {
            super::TerminalApp::Terminal => prefs.set(&at, "bool", "true")?,
            super::TerminalApp::ITerm => prefs.set(&at, "integer", "2")?,
        }
    }
    prefs.import()
}
