//! `switchboard statusline`: Claude Code's statusline, given its JSON on
//! stdin. Ported from claude/statusline-command.sh, which
//! `assets/claude-settings.json` now names in its place.
//!
//! It prints the model in the project's accent color (src/tabs, so the
//! Claude statusline and the tab agree on a project's color), then, outside
//! the workspace, the directory and branch, which inside it the tab already
//! shows, and the context used, colored by how full it is. With UX 2.0 on
//! (`ux2`) and mission control's Sessions view on (`view_sessions`) it
//! leaves the context out inside the workspace, where that view shows it; a
//! plain terminal has no Sessions view, so there it stays.
//!
//! It also keeps what Claude Code tells the statusline alone, for mission
//! control's Sessions and Usage views (src/usage/sessions.rs), under
//! `paths::state_dir()`: `context/<session>` (`<percent>\t<model>`), and
//! `usage/spend-<session>` (`<day>\t<spent that day>\t<the session's cost so
//! far>\t<the Claude process's pid>`) with `usage/limits` (the account's
//! `rate_limits`, the same for every session). Each is written only when it
//! changes, and only its owner can read it, as the script's `umask 077` had
//! it.
//!
//! The day's share is kept across days and restarts. Within one process the
//! cost the statusline is given dips and recovers, so a lower cost from the
//! same pid adds nothing and keeps the higher one; only a new pid, from the
//! session file naming this session, counts from nothing. A record from
//! before the pid was kept may hold a share counted many times over, so
//! today's is cut to the session's cost so far, and since it can't tell
//! whether this draw comes from the process that wrote it, a lower cost adds
//! nothing and becomes the cost to count on from. That is the script's awk,
//! step for step (`next_spend`).

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::Value;

use crate::agent::sessions;
use crate::tabs::{self, git};
use crate::usage::sessions::{context_dir, safe_id, usage_dir};
use crate::{paths, util};

const RESET: &str = "\x1b[0m";

/// `switchboard statusline`.
pub fn main(_args: Vec<String>) -> ExitCode {
    let mut input = Vec::new();
    let _ = std::io::stdin().lock().read_to_end(&mut input);
    let payload: Value = serde_json::from_slice(&input).unwrap_or(Value::Null);
    let in_workspace = std::env::var_os("SWITCHBOARD").is_some_and(|v| !v.is_empty());
    keep(
        &payload,
        &paths::state_dir(),
        &util::ymd(&util::local_now()),
    );
    let sessions_show_the_context =
        crate::settings::is_on("ux2") && crate::settings::is_on("view_sessions");
    let line = line(&payload, in_workspace, sessions_show_the_context);
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(line.as_bytes());
    let _ = out.flush();
    ExitCode::SUCCESS
}

/// A field as `jq -r` printed it: a string as it is, anything else as JSON,
/// `null` when missing.
fn raw(v: &Value) -> String {
    util::js_string(v)
}

/// A field as `jq -r '... // empty'` printed it: nothing for null or false.
fn or_empty(v: &Value) -> Option<String> {
    match v {
        Value::Null | Value::Bool(false) => None,
        other => Some(raw(other)),
    }
}

/// What the statusline prints.
pub fn line(payload: &Value, in_workspace: bool, sessions_show_the_context: bool) -> String {
    let model = raw(&payload["model"]["display_name"]);
    let raw_cwd = match &payload["workspace"]["current_dir"] {
        Value::String(s) => s.clone(),
        other => raw(other),
    };
    let dir = PathBuf::from(&raw_cwd);
    let place = git::locate(&dir);
    let color = tabs::statusline_color(&tabs::color::color_key(
        &tabs::color::dir_name(&dir),
        place.as_ref(),
    ));
    let accent = format!("\x1b[38;2;{};{};{}m", color.r, color.g, color.b);

    let mut output = format!("{accent}{model}{RESET}");
    // Inside the workspace the tab already names the directory and branch,
    // so only the model and context are shown there (the model alone with
    // UX 2.0, whose Sessions view shows the context); in a plain terminal this
    // is the only place they appear.
    if !in_workspace {
        output.push_str(&format!(" | {accent}{}{RESET}", tilde_start(&raw_cwd)));
        if let Some(p) = &place {
            output.push_str(&format!(" | {accent}{}{RESET}", p.branch));
        }
    }
    if in_workspace && sessions_show_the_context {
        return output;
    }
    match used(payload) {
        Some(used) => format!(
            "{output} | Context: {}{used:.1}% used{RESET}",
            context_color(used)
        ),
        None => output,
    }
}

/// The context used, in percent, when Claude Code gave it.
fn used(payload: &Value) -> Option<f64> {
    let text = or_empty(&payload["context_window"]["used_percentage"])?;
    awk_number(&text).filter(|_| !text.is_empty())
}

/// The context's color, from white through green, yellow and orange to red
/// as it fills, independent of the accent.
fn context_color(used: f64) -> &'static str {
    // printf "%.0f" rounds as Rust's formatting does.
    let whole: i64 = format!("{used:.0}").parse().unwrap_or(0);
    match whole {
        ..20 => "\x1b[97m",
        20..40 => "\x1b[32m",
        40..60 => "\x1b[38;5;178m",
        60..80 => "\x1b[38;5;208m",
        _ => "\x1b[38;5;160m",
    }
}

/// The directory with a leading home written `~`, as `sed "s|^$HOME|~|"`.
fn tilde_start(dir: &str) -> String {
    let home = paths::home().to_string_lossy().into_owned();
    match dir.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => dir.to_string(),
    }
}

/// The number at the start of a text, as awk reads a field: `1.5x` is 1.5,
/// and text with no number at its start is 0.
fn awk_number(text: &str) -> Option<f64> {
    let t = text.trim_start();
    let mut end = 0;
    let mut seen_digit = false;
    let mut seen_point = false;
    for (i, c) in t.char_indices() {
        match c {
            '0'..='9' => {
                seen_digit = true;
                end = i + 1;
            }
            '.' if !seen_point => {
                seen_point = true;
                end = i + 1;
            }
            '-' | '+' if i == 0 => end = 1,
            _ => break,
        }
    }
    if !seen_digit {
        return Some(0.0);
    }
    t[..end].parse::<f64>().ok()
}

/// Writes what the Sessions and Usage views read, each file only when it
/// changes.
pub fn keep(payload: &Value, state: &Path, today: &str) {
    let session = or_empty(&payload["session_id"]).unwrap_or_default();
    if !safe_id(&session) {
        return;
    }
    if let Some(used) = or_empty(&payload["context_window"]["used_percentage"]) {
        let model = raw(&payload["model"]["display_name"]);
        let line = format!("{used}\t{model}");
        write_changed(&context_dir(state).join(&session), &line);
    }
    let Some(cost) = or_empty(&payload["cost"]["total_cost_usd"]) else {
        return;
    };
    if cost.is_empty() || !cost.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return;
    }
    let usage = usage_dir(state);
    let spend_file = usage.join(format!("spend-{session}"));
    let old = fs::read_to_string(&spend_file).unwrap_or_default();
    let pid = pid_of(&sessions::sessions_dir(), &session).unwrap_or_default();
    let new = next_spend(&old, today, &cost, &pid);
    if new != old {
        write_changed(&spend_file, &new);
    }
    if let Some(limits) = payload
        .get("rate_limits")
        .filter(|l| !matches!(l, Value::Null | Value::Bool(false)))
    {
        write_changed(&usage.join("limits"), &util::json_compact(limits));
    }
}

/// The pid of the Claude process whose session file names this session:
/// its file's name, as the script took it from `grep -l`.
fn pid_of(dir: &Path, session: &str) -> Option<String> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    files.into_iter().find_map(|file| {
        let v = util::read_json(&file)?;
        (v.get("sessionId").and_then(Value::as_str) == Some(session))
            .then(|| file.file_stem()?.to_str().map(str::to_string))?
    })
}

/// The spend file's next text, from its text now: the script's awk.
pub fn next_spend(old: &str, today: &str, cost: &str, pid: &str) -> String {
    let record = old.lines().last().unwrap_or("");
    let fields: Vec<&str> = if record.is_empty() {
        Vec::new()
    } else {
        record.split('\t').collect()
    };
    let field = |i: usize| fields.get(i).copied().unwrap_or("");
    let num = |s: &str| awk_number(s).unwrap_or(0.0);
    let (day, last_text, was) = (field(0), field(2), field(3));
    let (mut spent, last) = (num(field(1)), num(last_text));
    if day != today {
        spent = 0.0;
    } else if fields.len() == 3 && spent > last {
        spent = last;
    }
    let pid = if pid.is_empty() { was } else { pid };
    let mut cost_text = cost;
    let mut delta = num(cost) - last;
    if !was.is_empty() && pid != was {
        delta = num(cost);
    } else if delta < 0.0 {
        delta = 0.0;
        if fields.len() != 3 {
            cost_text = last_text;
        }
    }
    format!("{today}\t{:.4}\t{cost_text}\t{pid}", spent + delta)
}

/// Writes a file only its owner can read, in a folder only its owner can,
/// unless it already holds `text`.
fn write_changed(file: &Path, text: &str) {
    if fs::read_to_string(file).is_ok_and(|now| now == text) {
        return;
    }
    if let Some(dir) = file.parent() {
        let _ = fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir);
    }
    let tmp = file.with_extension(format!("{}.tmp", std::process::id()));
    let written = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| f.write_all(text.as_bytes()));
    if written.is_ok() {
        let _ = fs::rename(&tmp, file);
    } else {
        let _ = fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_day_s_share_is_counted_as_the_awk_counted_it() {
        let day = "2026-11-06";
        // Nothing yet: the whole cost so far.
        assert_eq!(
            next_spend("", day, "1.5", "100"),
            "2026-11-06\t1.5000\t1.5\t100"
        );
        // The same process, a higher cost: the difference added.
        assert_eq!(
            next_spend("2026-11-06\t1.5000\t1.5\t100", day, "2.25", "100"),
            "2026-11-06\t2.2500\t2.25\t100"
        );
        // A dip from the same process adds nothing and keeps the higher one.
        assert_eq!(
            next_spend("2026-11-06\t2.2500\t2.25\t100", day, "2.0", "100"),
            "2026-11-06\t2.2500\t2.25\t100"
        );
        // A new process counts from nothing.
        assert_eq!(
            next_spend("2026-11-06\t2.2500\t2.25\t100", day, "0.5", "200"),
            "2026-11-06\t2.7500\t0.5\t200"
        );
        // A new day starts the share again.
        assert_eq!(
            next_spend("2026-11-05\t9.0000\t3\t100", day, "3.5", "100"),
            "2026-11-06\t0.5000\t3.5\t100"
        );
        // A record from before the pid was kept: today's cut to the cost so
        // far, and a lower cost becomes the one to count on from.
        assert_eq!(
            next_spend("2026-11-06\t40\t3", day, "4", ""),
            "2026-11-06\t4.0000\t4\t"
        );
        assert_eq!(
            next_spend("2026-11-06\t2\t3", day, "1", "300"),
            "2026-11-06\t2.0000\t1\t300"
        );
        // No pid found keeps the one written.
        assert_eq!(
            next_spend("2026-11-06\t1.0000\t1\t100", day, "1.25", ""),
            "2026-11-06\t1.2500\t1.25\t100"
        );
    }

    #[test]
    fn the_line_names_the_model_and_the_context() {
        let payload = json!({
            "model": { "display_name": "Opus" },
            "workspace": { "current_dir": "/nowhere/at-all" },
            "context_window": { "used_percentage": 42.34 }
        });
        let color = tabs::statusline_color("at-all");
        let accent = format!("\x1b[38;2;{};{};{}m", color.r, color.g, color.b);
        assert_eq!(line(&payload, true, true), format!("{accent}Opus{RESET}"));
        assert_eq!(
            line(&payload, true, false),
            format!("{accent}Opus{RESET} | Context: \x1b[38;5;178m42.3% used{RESET}")
        );
        for sessions in [true, false] {
            assert_eq!(
                line(&payload, false, sessions),
                format!(
                    "{accent}Opus{RESET} | {accent}/nowhere/at-all{RESET} | Context: \x1b[38;5;178m42.3% used{RESET}"
                )
            );
        }
        let bare =
            json!({ "model": { "display_name": "Haiku" }, "workspace": { "current_dir": "/x" } });
        for (in_workspace, sessions) in [(true, true), (true, false), (false, true), (false, false)]
        {
            assert!(!line(&bare, in_workspace, sessions).contains("Context"));
        }
    }

    #[test]
    fn the_context_s_color_steps_up_as_it_fills() {
        assert_eq!(context_color(5.0), "\x1b[97m");
        assert_eq!(context_color(19.4), "\x1b[97m");
        assert_eq!(context_color(20.0), "\x1b[32m");
        assert_eq!(context_color(79.4), "\x1b[38;5;208m");
        assert_eq!(context_color(95.0), "\x1b[38;5;160m");
    }
}
