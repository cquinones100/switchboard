//! `switchboard claude-context`: Switchboard's part of Claude's instructions
//! (`assets/switchboard.md`: the notes app and the tab's own note,
//! `switchboard tabs`, talking to another tab and naming one, a
//! repository's own skills), printed as a SessionStart hook's
//! `additionalContext`, which Claude Code adds to the conversation as a session starts, resumes, clears or compacts. New with
//! the plugin (src/setup/plugin.rs), whose hooks run it, so the text
//! reaches the Claude sessions in Switchboard's panes without an import in
//! your own `~/.claude/CLAUDE.md`, as the old way (and the tmux version)
//! has it.
//!
//! Only an interactive session gets it. A headless `claude -p` started from
//! a pane (a skill's, a script's, one of Switchboard's own that kept the
//! plugin) answers a question of its own, and the instructions would only
//! cost it tokens: Claude Code names how it was started in its hooks'
//! `CLAUDE_CODE_ENTRYPOINT` (`cli` for the interactive one, `sdk-cli` and
//! the like for the rest), and a session's file says so too
//! (src/agent/sessions.rs's `is_headless`, by the hook's `session_id`).
//!
//! The text is the one compiled into the binary, so a new build brings its
//! own with the next session. As with `switchboard hook`, Claude Code adds
//! the output to the conversation and waits on it, so it reads the hook's
//! JSON with a bound, prints only the hook's JSON or nothing (a panic's
//! message, or a failed write to a pipe Claude has closed, says nothing),
//! and always exits 0.
//!
//! A session in a tab with a note of its own (`@tab_note`, else
//! `@tab_note_last`, as Notes picks the tab's note) is also pointed at that
//! note by its path and told to read it before its first reply: a note is
//! in no conversation until Claude reads it, so a standing rule kept there
//! would be missed by each new session in the tab, whatever repository it
//! works in. Only the path goes through the hook, never the note's words,
//! and the `tab_note_context` setting turns it off. The server is asked
//! with `send_quiet`'s bound, so one that has stopped answering costs the
//! session start a second at most, and the session gets the part alone.
//!
//! A session in a pane is told its tab's number too, as `switchboard tabs`
//! numbers it, and the tab's label: Claude Code's own name for a session
//! (`switchboard-66`) ends in digits of its own, which a session that
//! didn't know its tab's number took for one, and a message to another tab
//! says which tab it comes from by that number.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Value, json};

use crate::agent::sessions;
use crate::mux::socket;
use crate::notes::{serve::in_notes, store};
use crate::settings;
use crate::util;

/// Switchboard's part, as compiled in.
pub const PART: &str = include_str!("../../assets/switchboard.md");

/// Switchboard's part: the clone's, else the compiled one (src/assets.rs).
pub fn part() -> std::borrow::Cow<'static, str> {
    crate::assets::text("switchboard.md", PART)
}

/// The most of the hook's JSON read.
const INPUT_MAX: u64 = 1 << 20;

/// The hook's answer: the part as SessionStart's added context, with the
/// tab's number and label after it when the session is in a tab, and the
/// tab's note pointed at when it has one.
pub fn answer(tab: Option<(&str, &str)>, note: Option<&Path>) -> String {
    let mut part = part().into_owned();
    if let Some((number, label)) = tab {
        part.push_str(&format!(
            "\n# This tab\n\nThis session is in tab {number}, {label}. Say so by that number when you write to another tab.\n"
        ));
    }
    let context = match note {
        Some(note) => format!(
            "{part}\n# This tab's note\n\nThis tab's note is `{}`. Read it before your first reply: \
             the standing rules it keeps apply to this session too, whatever repository it works in.\n",
            note.display()
        ),
        None => part,
    };
    json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": context,
        }
    })
    .to_string()
}

/// The note of the tab this pane is in, by its file: `@tab_note`, else
/// `@tab_note_last`, while its file is in the notes directory. Nothing
/// outside a pane, with the setting off, or when the server doesn't answer.
fn tab_note() -> Option<PathBuf> {
    if !socket::in_pane() || !settings::is_on("tab_note_context") {
        return None;
    }
    let pane = std::env::var("SWITCHBOARD_PANE").ok();
    let dir = store::notes_dir();
    for option in ["@tab_note", "@tab_note_last"] {
        let (code, out, _) =
            socket::ask_quiet(vec!["show".into(), option.into()], pane.clone(), None)?;
        let rel = String::from_utf8_lossy(&out).trim().to_string();
        if code == 0 && in_notes(&rel) && dir.join(&rel).is_file() {
            return Some(dir.join(rel));
        }
    }
    None
}

/// The number and label of the tab this pane is in: the tab the server says
/// owns the pane (`display tab`, which finds a pane parked behind the diff
/// too), then its number and label from `list`, as `switchboard tabs` reads
/// them. Nothing outside a pane or when the server doesn't answer.
fn this_tab() -> Option<(String, String)> {
    if !socket::in_pane() {
        return None;
    }
    let pane = std::env::var("SWITCHBOARD_PANE").ok()?;
    let (code, out, _) = socket::ask_quiet(
        vec!["display".into(), "tab".into()],
        Some(pane.clone()),
        None,
    )?;
    let tab = String::from_utf8_lossy(&out).trim().to_string();
    if code != 0 || tab.is_empty() {
        return None;
    }
    let (code, out, _) = socket::ask_quiet(vec!["list".into()], Some(pane), None)?;
    if code != 0 {
        return None;
    }
    tab_of(&serde_json::from_slice(&out).ok()?, &tab)
}

/// The number and label of the tab `id` (`@3`) in the server's `list`.
fn tab_of(list: &Value, id: &str) -> Option<(String, String)> {
    let tabs = list["sessions"]
        .as_array()?
        .iter()
        .flat_map(|session| session["tabs"].as_array().into_iter().flatten());
    for tab in tabs {
        if tab["id"] == id {
            let label = tab["label"].as_str().unwrap_or_default();
            return Some((util::js_string(&tab["index"]), label.replace('\n', " ")));
        }
    }
    None
}

/// Whether a session started as `entrypoint` (Claude Code's
/// `CLAUDE_CODE_ENTRYPOINT`, when set) is an interactive one.
pub fn interactive(entrypoint: Option<&str>) -> bool {
    entrypoint.is_none_or(|e| e.is_empty() || e == "cli")
}

/// The line that puts the queue's stand-ins first on the PATH of
/// Claude's commands.
pub fn stand_ins_line(bin: &Path) -> String {
    format!(
        "export PATH={}:\"$PATH\"\n",
        crate::worktree::shell_quote(&bin.to_string_lossy())
    )
}

/// In a pane, puts the stand-ins for cargo, npm and pnpm (src/cmd/slot.rs)
/// first on the PATH of every command Claude runs, through the file Claude
/// Code reads its commands' environment from after your shell's startup
/// files, which may put the real ones first again (rustup's
/// `~/.cargo/bin`). Headless sessions too, since a skill's `claude -p`
/// builds as well. Run by `switchboard hook SessionStart`, which both
/// ways of setting Claude up (the plugin and `~/.claude`) install.
pub fn stand_ins_first() {
    let Some(file) = std::env::var_os("CLAUDE_ENV_FILE") else {
        return;
    };
    let bin = crate::cmd::slot::bin_dir();
    if std::env::var_os("SWITCHBOARD_PANE").is_none() || !bin.is_dir() {
        return;
    }
    let line = stand_ins_line(&bin);
    let file = PathBuf::from(file);
    if std::fs::read_to_string(&file).is_ok_and(|text| text.contains(&line)) {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

pub fn main(_args: Vec<String>) -> ExitCode {
    std::panic::set_hook(Box::new(|_| {}));
    let _ = std::panic::catch_unwind(|| {
        if !interactive(std::env::var("CLAUDE_CODE_ENTRYPOINT").ok().as_deref()) {
            return;
        }
        let mut input = Vec::new();
        if socket::stdin_is_data() {
            let _ = std::io::stdin()
                .lock()
                .take(INPUT_MAX)
                .read_to_end(&mut input);
        }
        let session = serde_json::from_slice::<Value>(&input)
            .ok()
            .and_then(|v| {
                v.get("session_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        if !session.is_empty()
            && sessions::is_headless(&sessions::read_all(&sessions::sessions_dir()), &session)
        {
            return;
        }
        let mut out = std::io::stdout().lock();
        let tab = this_tab();
        let tab = tab.as_ref().map(|(n, l)| (n.as_str(), l.as_str()));
        let _ = out.write_all(answer(tab, tab_note().as_deref()).as_bytes());
        let _ = out.write_all(b"\n");
        let _ = out.flush();
    });
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_is_session_start_s_context_holding_the_whole_part() {
        let answer: Value = serde_json::from_str(&answer(None, None)).expect("JSON");
        assert_eq!(
            answer["hookSpecificOutput"]["hookEventName"],
            "SessionStart"
        );
        assert_eq!(answer["hookSpecificOutput"]["additionalContext"], PART);
        assert_eq!(answer.as_object().unwrap().len(), 1, "nothing else");
        // Claude Code writes a context over 10,000 characters to a file
        // rather than adding it.
        assert!(PART.chars().count() < 10_000);
        assert!(PART.contains("switchboard tabs"));
        // Another tab's Claude is reached by the address `tabs` prints, not
        // by a row's directory, which `ListAgents` doesn't show.
        assert!(PART.contains("`uds:<socket>`"));
        assert!(!PART.contains("row's directory"));
        assert!(!PART.contains("tmux"));
    }

    #[test]
    fn a_tab_with_a_note_has_it_pointed_at_after_the_part() {
        let answer: Value =
            serde_json::from_str(&answer(None, Some(Path::new("/n/Plans/A b.md")))).expect("JSON");
        let context = answer["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        let added = context.strip_prefix(PART).expect("the part first");
        assert!(
            added.contains("This tab's note is `/n/Plans/A b.md`"),
            "{added}"
        );
        assert!(added.contains("before your first reply"), "{added}");
        assert!(context.chars().count() < 10_000);
    }

    #[test]
    fn a_session_in_a_tab_is_told_its_number_and_label() {
        let answer: Value =
            serde_json::from_str(&answer(Some(("6", "Design")), None)).expect("JSON");
        let context = answer["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        let added = context.strip_prefix(PART).expect("the part first");
        assert!(
            added.contains("This session is in tab 6, Design."),
            "{added}"
        );
    }

    #[test]
    fn the_tab_is_found_by_its_id() {
        let list = json!({"sessions": [{"tabs": [
            {"id": "@1", "index": 1, "label": "One"},
            {"id": "@66", "index": 6, "label": "switchboard"},
        ]}]});
        assert_eq!(
            tab_of(&list, "@66"),
            Some(("6".to_string(), "switchboard".to_string()))
        );
        assert_eq!(tab_of(&list, "@3"), None);
    }

    #[test]
    fn only_an_interactive_session_gets_it() {
        assert!(interactive(Some("cli")));
        assert!(interactive(None), "an older Claude Code that doesn't say");
        assert!(!interactive(Some("sdk-cli")));
        assert!(!interactive(Some("sdk-ts")));
        assert!(!interactive(Some("claude-vscode")));
    }
}
