//! The programs on the demo's PATH in place of yours, `switchboard
//! demo-stand-in <name>`: each a two-line script in the demo's
//! `~/.local/bin` (a script, not a link, so nothing in its home leads out of
//! it), first on its PATH, so whatever the workspace runs by name reaches
//! these rather than your desktop, Keychain, gh or Claude.
//!
//! - `claude`: the stand-in Claude (`super::agent`); `claude -p` and
//!   `claude mcp …`, which would reach a model or a connector, are refused.
//! - `gh`: answers `auth token` (and `auth status`) with the demo's token
//!   and refuses the rest, as the tests' stand-in refuses a call it wasn't
//!   told of.
//! - the openers (`open`, `xdg-open`, `gio`, WSL's `wslview` and
//!   `explorer.exe`, Chrome), `osascript` and
//!   `notify-send`: what they were asked is written to the demo's
//!   `desktop-log`, a line each, and nothing opens.
//! - the clipboard (`pbcopy`/`pbpaste`, `wl-copy`/`wl-paste`, `xclip`): the
//!   demo's own `clipboard` file.
//! - `security`: `find-generic-password` reads the demo's `secrets.json`,
//!   never your Keychain; `add-generic-password` writes it.
//! - `caffeinate` and `qlmanage`: do nothing.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use serde_json::{Map, Value};

use crate::util;
use crate::worktree::shell_quote;

/// Writes the stand-ins' scripts, and `switchboard` itself, into the demo's
/// `~/.local/bin`, each running `bin`.
pub fn install(home: &Path, bin: &Path) -> Result<()> {
    let dir = home.join(".local/bin");
    fs::create_dir_all(&dir)?;
    let bin = shell_quote(&bin.to_string_lossy());
    let write = |name: &str, body: String| -> Result<()> {
        let file = dir.join(name);
        fs::write(&file, body)?;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755))?;
        Ok(())
    };
    write("switchboard", format!("#!/bin/sh\nexec {bin} \"$@\"\n"))?;
    for name in super::STAND_INS {
        write(
            name,
            format!("#!/bin/sh\nexec {bin} demo-stand-in {name} \"$@\"\n"),
        )?;
    }
    Ok(())
}

fn home() -> PathBuf {
    crate::paths::home()
}

fn log(what: &str, args: &[String]) {
    let file = super::desktop_log(&home());
    if let Some(dir) = file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(file) {
        let _ = writeln!(f, "{}\t{what}\t{}", util::now_ms(), args.join(" "));
    }
}

fn clipboard() -> PathBuf {
    super::own_state(&home()).join("clipboard")
}

fn copy_in() -> ExitCode {
    let mut text = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut text);
    match fs::write(clipboard(), text) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

fn paste() -> ExitCode {
    let _ = std::io::stdout().write_all(&fs::read(clipboard()).unwrap_or_default());
    ExitCode::SUCCESS
}

fn refuse(name: &str, args: &[String], why: &str) -> ExitCode {
    eprintln!(
        "{name} (the demo's stand-in): {why}; it was asked: {}",
        args.join(" ")
    );
    ExitCode::FAILURE
}

/// `switchboard demo-stand-in <name> [args]`.
pub fn main(args: Vec<String>) -> ExitCode {
    let Some((name, rest)) = args.split_first() else {
        eprintln!("Usage: switchboard demo-stand-in <program> [args]");
        return ExitCode::from(2);
    };
    let words: Vec<&str> = rest.iter().map(String::as_str).collect();
    match name.as_str() {
        "claude" => claude(rest),
        "gh" => match words.as_slice() {
            ["auth", "token", ..] => {
                println!("{}", super::fixtures::GITHUB_TOKEN);
                ExitCode::SUCCESS
            }
            ["auth", "status", ..] => {
                println!(
                    "github.com\n  ✓ Logged in to github.com account {} (demo)",
                    super::fixtures::LOGIN
                );
                ExitCode::SUCCESS
            }
            _ => refuse("gh", rest, "the demo answers only `gh auth token`"),
        },
        "pbcopy" | "wl-copy" => copy_in(),
        "pbpaste" | "wl-paste" => paste(),
        "xclip" if words.contains(&"-o") => paste(),
        "xclip" => copy_in(),
        "security" => security(rest),
        "caffeinate" | "qlmanage" => ExitCode::SUCCESS,
        "osascript" if words.contains(&"JavaScript") => {
            refuse("osascript", rest, "the demo has no clipboard pictures")
        }
        "open"
        | "xdg-open"
        | "gio"
        | "wslview"
        | "explorer.exe"
        | "osascript"
        | "notify-send"
        | "google-chrome"
        | "google-chrome-stable"
        | "chromium"
        | "chromium-browser" => {
            log(name, rest);
            ExitCode::SUCCESS
        }
        _ => refuse(name, rest, "no such stand-in"),
    }
}

fn claude(args: &[String]) -> ExitCode {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    if words.contains(&"--version") || words.contains(&"-v") {
        println!("2.1.0 (Claude Code, the demo's stand-in)");
        return ExitCode::SUCCESS;
    }
    if words.iter().any(|w| matches!(*w, "-p" | "--print")) || words.first() == Some(&"mcp") {
        return refuse(
            "claude",
            args,
            "the demo has no Claude to ask: it plays a script and reaches no model",
        );
    }
    let script = words
        .iter()
        .position(|w| *w == "--demo-script")
        .and_then(|i| words.get(i + 1))
        .copied()
        .unwrap_or("default");
    super::agent::run(script)
}

fn secrets_file() -> PathBuf {
    home().join(".config/switchboard/secrets.json")
}

/// `security find-generic-password [-a account] -s service -w` from the
/// demo's `secrets.json`, exiting 44 (`errSecItemNotFound`'s exit) for a
/// secret it hasn't; `add-generic-password` keeps one there.
fn security(args: &[String]) -> ExitCode {
    let value_of = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let service = value_of("-s").unwrap_or_default();
    let mut secrets: Map<String, Value> = util::read_json_object(secrets_file());
    match args.first().map(String::as_str) {
        Some("find-generic-password") => match secrets.get(&service).and_then(Value::as_str) {
            Some(value) => {
                println!("{value}");
                ExitCode::SUCCESS
            }
            None => {
                eprintln!(
                    "security: SecKeychainSearchCopyNext: The specified item could not be found in the keychain."
                );
                ExitCode::from(44)
            }
        },
        Some("add-generic-password") => {
            let Some(value) = value_of("-w") else {
                return refuse("security", args, "no -w value");
            };
            secrets.insert(service, Value::String(value));
            match util::write_atomic(secrets_file(), util::json_pretty(&Value::Object(secrets))) {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => ExitCode::FAILURE,
            }
        }
        Some("delete-generic-password") => {
            secrets.remove(&service);
            let _ = util::write_atomic(secrets_file(), util::json_pretty(&Value::Object(secrets)));
            ExitCode::SUCCESS
        }
        _ => refuse("security", args, "the demo has no Keychain"),
    }
}
