//! `claude-here`: resumes a Claude Code session from the current git
//! worktree (or the current directory outside a repository). Ported from
//! `bin/claude-here`.
//!
//! With one session that ran here it resumes it at once; with several it
//! shows a picker, each session by when it was last active and the last
//! thing you typed in it. On a terminal the picker is moved through with the
//! arrows or j/k and Enter picks (Esc, q or C-c cancels); with stdin piped
//! it is a numbered prompt (Enter for the most recent).
//!
//! ```text
//! claude-here [--print] [extra args passed through to claude]
//!   --print  print every matching session instead of resuming one
//! ```
//!
//! Claude Code keeps each conversation under `~/.claude/projects/<dir>/`,
//! `<dir>` being the directory it started in with every character but a
//! letter or digit made `-`. That is lossy (`-foo` is both `/foo` and
//! `.foo`), so the candidate directories are found by that prefix and each
//! session is confirmed by the directories its transcript records. A
//! transcript is read from its end, a growing piece at a time, since one can
//! run to hundreds of megabytes.
//!
//! The picker draws on stderr, so `--print` stays pipeable on stdout, and
//! SIGINT is ignored while Claude runs, since Ctrl-C reaches Claude from the
//! terminal itself. Unlike the Node version, a SIGTERM or SIGHUP sent to this
//! process alone isn't passed on to Claude.

use std::fs::{self, File};
use std::io::{IsTerminal, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::sidebar::text;
use crate::{paths, util};

/// How much of a transcript's end is read first; four times more each time
/// what is looked for isn't in it.
const TAIL_START: u64 = 256 * 1024;

static SESSION_FILE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.jsonl$").unwrap()
});
static ESCAPES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07\x1b]*(?:\x07|\x1b\\)?|.)").unwrap()
});

/// Status goes to stderr so `--print`'s output stays pipeable on stdout.
fn status(message: &str) {
    eprintln!("{message}");
}

/// A path as printed: its control characters shown as `?`, since a
/// folder's name may hold a terminal escape.
fn printable(text: &str) -> String {
    text.chars()
        .map(|c| if is_control(c) { '?' } else { c })
        .collect()
}

fn is_control(c: char) -> bool {
    matches!(c as u32, 0..=0x1f | 0x7f..=0x9f)
}

/// A directory as Claude Code names its project folder: every character
/// that isn't an ASCII letter or digit made `-`.
pub fn encode_prefix(dir: &str) -> String {
    dir.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// What a transcript says at its end: the last directory, the last
/// activity in milliseconds and the last thing you typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tail {
    pub cwd: Option<String>,
    pub last_active_ms: Option<i64>,
    pub last_message: Option<String>,
}

/// What a prompt shows in the picker: the text you typed, with terminal
/// escapes and other control characters a pasted prompt may hold dropped,
/// so the picker draws text, never commands. Meta entries, tool results and
/// the `<command-*>` and `<system-reminder>` messages Claude Code records
/// give nothing.
fn prompt_text(entry: &Value) -> Option<String> {
    if entry.get("type").and_then(Value::as_str) != Some("user")
        || entry.get("isMeta").is_some_and(util::truthy)
    {
        return None;
    }
    let content = entry.pointer("/message/content");
    let raw = match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    };
    let stripped = ESCAPES.replace_all(&raw, "");
    let spaced: String = stripped
        .chars()
        .map(|c| if is_control(c) { ' ' } else { c })
        .collect();
    let cleaned = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    (!cleaned.is_empty()
        && !cleaned.starts_with('<')
        && !cleaned.starts_with("[Request interrupted"))
    .then_some(cleaned)
}

/// Milliseconds since the epoch of an ISO time, as `Date.parse` reads one.
fn parse_time(text: &str) -> Option<i64> {
    text.parse::<jiff::Timestamp>()
        .ok()
        .map(|t| t.as_millisecond())
}

/// The last `length` bytes of a file of `size`, from the start of a whole
/// line unless the read reaches the start of the file; and whether it did.
fn tail_of(file: &mut File, size: u64, length: u64) -> std::io::Result<(String, bool)> {
    let start = size.saturating_sub(length);
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity((size - start) as usize);
    file.take(size - start).read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    if start == 0 {
        return Ok((text, true));
    }
    let from = text.find('\n').map_or(text.len(), |at| at + 1);
    Ok((text[from..].to_string(), false))
}

/// A session's last activity, last directory and last prompt, read from
/// the end of its transcript, a growing piece at a time. The last entry
/// with a timestamp is the last activity, not the file's modified time:
/// Claude Code appends bookkeeping entries with none whenever a session is
/// merely opened. The last directory, not the first, since a session can
/// outlive the folder it started in. A transcript that can't be read reads
/// as having nothing, so it is left out rather than stopping the search.
pub fn read_session_tail(path: &Path) -> Tail {
    let Ok(mut file) = File::open(path) else {
        return Tail::default();
    };
    let Ok(size) = file.metadata().map(|m| m.len()) else {
        return Tail::default();
    };
    let mut length = TAIL_START;
    loop {
        let Ok((text, whole)) = tail_of(&mut file, size, length) else {
            return Tail::default();
        };
        let mut found = Tail::default();
        for line in text.split('\n').rev() {
            if found.cwd.is_some() && found.last_active_ms.is_some() && found.last_message.is_some()
            {
                break;
            }
            let Ok(entry) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if found.cwd.is_none()
                && let Some(cwd) = entry
                    .get("cwd")
                    .and_then(Value::as_str)
                    .filter(|c| !c.is_empty())
            {
                found.cwd = Some(cwd.to_string());
            }
            if found.last_active_ms.is_none() {
                found.last_active_ms = entry
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .and_then(parse_time);
            }
            if found.last_message.is_none() {
                found.last_message = prompt_text(&entry);
            }
        }
        let all =
            found.cwd.is_some() && found.last_active_ms.is_some() && found.last_message.is_some();
        if all || whole {
            return found;
        }
        length *= 4;
    }
}

/// The first directory a transcript names, where its session started, read
/// from its start. One that can't be read has none.
pub fn read_first_cwd(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    use std::io::BufRead;
    for line in std::io::BufReader::new(file).lines() {
        let Ok(line) = line else {
            return None;
        };
        if !line.contains("\"cwd\":") {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<Value>(&line)
            && let Some(cwd) = entry
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty())
        {
            return Some(cwd.to_string());
        }
    }
    None
}

/// How long ago a session was active, as the picker says it.
pub fn format_relative_time(time_ms: i64, now_ms: i64) -> String {
    let minutes = (now_ms - time_ms).div_euclid(60_000);
    if minutes < 1 {
        return "just now".into();
    }
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    if days < 2 {
        return "yesterday".into();
    }
    if days < 7 {
        return format!("{days}d ago");
    }
    local_date(time_ms)
}

/// A local date as `toLocaleDateString` writes one in English: 6/12/2026.
pub fn local_date(ms: i64) -> String {
    let at = util::local_at_ms(ms);
    format!("{}/{}/{}", at.month(), at.day(), at.year())
}

/// A local time as `toLocaleString` writes one in English:
/// 6/12/2026, 3:04:05 PM.
fn local_date_time(ms: i64) -> String {
    let at = util::local_at_ms(ms);
    let hour = at.hour();
    let (twelve, half) = match hour {
        0 => (12, "AM"),
        1..=11 => (hour, "AM"),
        12 => (12, "PM"),
        _ => (hour - 12, "PM"),
    };
    format!(
        "{}, {twelve}:{:02}:{:02} {half}",
        local_date(ms),
        at.minute(),
        at.second()
    )
}

/// An epoch time as `toISOString` writes one: 2026-06-12T15:04:05.000Z.
fn iso(ms: i64) -> String {
    let at = jiff::Timestamp::from_millisecond(ms)
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
        .to_zoned(jiff::tz::TimeZone::UTC);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        at.year(),
        at.month(),
        at.day(),
        at.hour(),
        at.minute(),
        at.second(),
        ms.rem_euclid(1000)
    )
}

/// A folder a session can be resumed in: there, and readable.
fn usable(dir: &str) -> bool {
    let Ok(c) = std::ffi::CString::new(dir) else {
        return false;
    };
    // SAFETY: access only reads the path it is given.
    let readable = unsafe { libc::access(c.as_ptr(), libc::R_OK | libc::X_OK) } == 0;
    readable && fs::metadata(dir).is_ok_and(|m| m.is_dir())
}

fn parent(dir: &str) -> Option<String> {
    let p = Path::new(dir).parent()?;
    Some(p.to_string_lossy().into_owned())
}

/// `claude --resume` finds a conversation only within the project directory
/// encoded from where it runs, which is where the session started, so it is
/// resumed from there; or, when that folder has since been renamed, from the
/// folder its last directory is in (or is) that encodes the same.
fn resume_dir(first: Option<&str>, last: &str, project_dir: &Path) -> Option<String> {
    let encoded = project_dir.file_name()?.to_string_lossy().into_owned();
    let mut candidates: Vec<String> = first.map(str::to_string).into_iter().collect();
    let mut at = Some(last.to_string());
    while let Some(dir) = at {
        at = parent(&dir).filter(|p| *p != dir);
        candidates.push(dir);
    }
    candidates
        .into_iter()
        .find(|dir| encode_prefix(dir) == encoded && usable(dir))
}

/// A session that ran here, and where to resume it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub file_path: PathBuf,
    pub project_dir: PathBuf,
    pub last_active_ms: i64,
    pub last_message: Option<String>,
    /// Where `claude --resume` finds it.
    pub cwd: String,
}

/// The sessions that ran in `target` or a folder of it, last active first.
/// One that moved into a worktree after it started in the main checkout
/// stays under the main checkout's project, which isn't read for the
/// worktree, and the worktrees nested under the target (its
/// `.claude/worktrees`) are checkouts with sessions of their own, left out.
pub fn find_sessions(target: &str, projects: &Path) -> Vec<Session> {
    let prefix = encode_prefix(target);
    let nested_prefix = encode_prefix(&format!("{target}/.claude/worktrees"));
    let mut project_dirs: Vec<PathBuf> = fs::read_dir(projects)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|name| *name == prefix || name.starts_with(&format!("{prefix}-")))
                .filter(|name| {
                    *name != nested_prefix && !name.starts_with(&format!("{nested_prefix}-"))
                })
                .map(|name| projects.join(name))
                .collect()
        })
        .unwrap_or_default();
    project_dirs.sort();

    let mut files: Vec<(String, PathBuf, PathBuf, i64)> = Vec::new();
    for dir in &project_dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !SESSION_FILE.is_match(&name) {
                continue;
            }
            let path = entry.path();
            // Followed through a link, so one to nothing is left out.
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as i64);
            let id = name.trim_end_matches(".jsonl").to_string();
            files.push((id, path, dir.clone(), mtime));
        }
    }
    status(&format!(
        "found {} session file{} across {} candidate project director{}",
        files.len(),
        if files.len() == 1 { "" } else { "s" },
        project_dirs.len(),
        if project_dirs.len() == 1 { "y" } else { "ies" }
    ));
    if files.is_empty() {
        return Vec::new();
    }

    status("verifying session working directories");
    let nested = format!("{target}/.claude/worktrees/");
    let ran_here = |dir: &str| {
        !dir.starts_with(&nested) && (dir == target || dir.starts_with(&format!("{target}/")))
    };
    let mut found: Vec<Session> = files
        .into_iter()
        .filter_map(|(id, file_path, project_dir, mtime)| {
            let tail = read_session_tail(&file_path);
            let last = tail.cwd.filter(|c| ran_here(c))?;
            let first = read_first_cwd(&file_path);
            let cwd = resume_dir(first.as_deref(), &last, &project_dir)?;
            Some(Session {
                id,
                file_path,
                project_dir,
                last_active_ms: tail.last_active_ms.unwrap_or(mtime),
                last_message: tail.last_message,
                cwd,
            })
        })
        .collect();
    found.sort_by_key(|s| std::cmp::Reverse(s.last_active_ms));
    found
}

/// Each session's line in the picker: when, then what you last typed.
fn session_lines(sessions: &[Session]) -> Vec<String> {
    let now = util::now_ms();
    let times: Vec<String> = sessions
        .iter()
        .map(|s| format_relative_time(s.last_active_ms, now))
        .collect();
    let width = times.iter().map(|t| t.chars().count()).max().unwrap_or(0);
    sessions
        .iter()
        .zip(times)
        .map(|(s, time)| {
            // By character, so a cut never splits an emoji or other wide one.
            let message: String = s
                .last_message
                .as_deref()
                .unwrap_or("(no user messages)")
                .chars()
                .take(80)
                .collect();
            format!("{time:<width$}   {message}")
        })
        .collect()
}

/// The numbered prompt, for piped stdin, which can't enter raw mode. The
/// end of the input is the most recent.
fn choose_from_pipe(lines: &[String]) -> Option<usize> {
    let number_width = lines.len().to_string().len();
    for (i, line) in lines.iter().enumerate() {
        status(&format!("  {:>number_width$}. {line}", i + 1));
    }
    Some(crate::worktree::choose_numbered(
        "Select session [1]: ",
        lines.len(),
    ))
}

/// The raw-mode picker, drawn on stderr. Lines are cut to the terminal's
/// width so each stays one row, and only as many as fit its height are
/// drawn, scrolling with the selection, since the redraw moves the cursor
/// up by the line count, which a line that wraps or scrolls off the top
/// would throw off. A resize lays it out again from a cleared screen.
fn choose_interactive(lines: &[String]) -> Option<usize> {
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, read};
    use crossterm::terminal;

    struct Layout {
        rows: Vec<String>,
        shown: usize,
        hint: Option<String>,
    }
    let layout = |lines: &[String]| {
        let (columns, height) = terminal::size().unwrap_or((80, 24));
        let (columns, height) = (columns as usize, height as usize);
        // The "❯ " before each takes two columns, and one is kept spare.
        let rows: Vec<String> = lines
            .iter()
            .map(|l| text::cut(l, columns.saturating_sub(3).max(1)))
            .collect();
        let with_hint = height >= 3;
        let shown = rows
            .len()
            .min(height.saturating_sub(if with_hint { 2 } else { 1 }))
            .max(1);
        let hint = with_hint.then(|| {
            let more = if shown < rows.len() {
                format!(" ({} sessions)", rows.len())
            } else {
                String::new()
            };
            text::cut(
                &format!("↑/↓ or j/k to move, Enter to select, Esc to cancel{more}"),
                columns.saturating_sub(1).max(1),
            )
        });
        Layout { rows, shown, hint }
    };
    let mut out = std::io::stderr();
    let mut l = layout(lines);
    let mut selected = 0usize;
    let mut top = 0usize;
    let render = |out: &mut std::io::Stderr, l: &Layout, selected: usize, top: &mut usize| {
        *top = (*top)
            .max((selected + 1).saturating_sub(l.shown))
            .min(selected)
            .min(l.rows.len().saturating_sub(l.shown));
        let mut frame = String::new();
        for (j, row) in l.rows.iter().skip(*top).take(l.shown).enumerate() {
            let line = if *top + j == selected {
                format!("\x1b[36m❯ \x1b[1m{row}\x1b[0m")
            } else {
                format!("  {row}")
            };
            frame.push_str(&format!("\x1b[2K{line}\r\n"));
        }
        if let Some(hint) = &l.hint {
            frame.push_str(&format!("\x1b[2K\x1b[2m{hint}\x1b[0m\r\n"));
        }
        let _ = out.write_all(frame.as_bytes());
        let _ = out.flush();
    };

    if terminal::enable_raw_mode().is_err() {
        return choose_from_pipe(lines);
    }
    let _ = out.write_all(b"\x1b[?25l");
    render(&mut out, &l, selected, &mut top);
    let chosen = loop {
        let Ok(event) = read() else {
            break None;
        };
        match event {
            Event::Resize(..) => {
                l = layout(lines);
                let _ = out.write_all(b"\x1b[2J\x1b[H");
                render(&mut out, &l, selected, &mut top);
            }
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                match key.code {
                    KeyCode::Enter => break Some(selected),
                    KeyCode::Esc | KeyCode::Char('q') => break None,
                    KeyCode::Char('c') if ctrl => break None,
                    KeyCode::Up | KeyCode::Char('k') => {
                        selected = (selected + l.rows.len() - 1) % l.rows.len();
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        selected = (selected + 1) % l.rows.len();
                    }
                    _ => continue,
                }
                let up = l.shown + usize::from(l.hint.is_some());
                let _ = out.write_all(format!("\x1b[{up}A").as_bytes());
                render(&mut out, &l, selected, &mut top);
            }
            _ => {}
        }
    };
    let _ = out.write_all(b"\x1b[?25h");
    let _ = out.flush();
    let _ = terminal::disable_raw_mode();
    chosen
}

/// The top of the git worktree here, else the current directory.
fn target_dir() -> String {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    if let Ok(out) = out
        && out.status.success()
    {
        // Only git's newline taken off: a folder's name may end in a space.
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let text = text.strip_suffix('\n').unwrap_or(&text);
        return text.strip_suffix('\r').unwrap_or(text).to_string();
    }
    std::env::current_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".into())
}

pub fn main(args: Vec<String>) -> ExitCode {
    let target = target_dir();
    status(&format!(
        "searching for claude sessions in {}",
        printable(&target)
    ));
    let sessions = find_sessions(&target, &paths::claude_dir().join("projects"));
    if sessions.is_empty() {
        eprintln!("No Claude sessions found for {}", printable(&target));
        return ExitCode::from(1);
    }

    if args.iter().any(|a| a == "--print") {
        let mut out = std::io::stdout().lock();
        for s in &sessions {
            let _ = writeln!(
                out,
                "{}\t{}\t{}",
                s.id,
                iso(s.last_active_ms),
                s.file_path.display()
            );
        }
        return ExitCode::SUCCESS;
    }

    let chosen = if sessions.len() == 1 {
        Some(0)
    } else {
        let lines = session_lines(&sessions);
        if std::io::stdin().is_terminal() {
            choose_interactive(&lines)
        } else {
            choose_from_pipe(&lines)
        }
    };
    let Some(chosen) = chosen else {
        status("cancelled");
        return ExitCode::from(130);
    };
    let session = &sessions[chosen];
    status(&format!(
        "session found: {} (last active {})",
        session.id,
        local_date_time(session.last_active_ms)
    ));

    // Resumed from the session's own directory, not the target: `claude
    // --resume` finds the conversation within the project directory encoded
    // from where it runs.
    status(&format!(
        "opening{}",
        if session.cwd == target {
            String::new()
        } else {
            format!(" in {}", printable(&session.cwd))
        }
    ));
    let child = Command::new("claude")
        .arg("--resume")
        .arg(&session.id)
        .args(&args)
        .current_dir(&session.cwd)
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(e) => {
            eprintln!(
                "Couldn't start claude in {}: {}",
                printable(&session.cwd),
                printable(&e.to_string())
            );
            return ExitCode::from(1);
        }
    };
    // SAFETY: ignoring SIGINT in this process alone, after Claude has
    // started with the default, so Ctrl-C at the terminal reaches Claude and
    // doesn't end this process first.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
    }
    match child.wait() {
        Ok(status) => match status.code() {
            Some(code) => ExitCode::from(code.clamp(0, 255) as u8),
            None => ExitCode::from(1),
        },
        Err(_) => ExitCode::from(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_lose_escapes_and_what_claude_code_adds() {
        let entry = serde_json::json!({ "type": "user", "message": { "content": [
            { "type": "text", "text": "follow-up\n  \u{1b}[2J\u{1b}[Hquestion\u{7}" }
        ] } });
        assert_eq!(prompt_text(&entry).as_deref(), Some("follow-up question"));
        let command =
            serde_json::json!({ "type": "user", "message": { "content": "<command-name>/clear" } });
        assert_eq!(prompt_text(&command), None);
        let meta =
            serde_json::json!({ "type": "user", "isMeta": true, "message": { "content": "x" } });
        assert_eq!(prompt_text(&meta), None);
    }

    #[test]
    fn times_as_javascript_writes_them() {
        assert_eq!(iso(1_781_276_645_123), "2026-06-12T15:04:05.123Z");
        assert_eq!(printable("a\u{1b}b"), "a?b");
        assert_eq!(encode_prefix("/a.b/c"), "-a-b-c");
    }
}
