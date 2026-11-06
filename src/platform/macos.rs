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
