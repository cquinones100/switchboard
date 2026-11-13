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

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Value, json};

use crate::agent::sessions;
use crate::mux::socket;
use crate::notes::{serve::in_notes, store};
use crate::settings;

/// Switchboard's part, as compiled in.
pub const PART: &str = include_str!("../../assets/switchboard.md");

/// Switchboard's part: the clone's, else the compiled one (src/assets.rs).
pub fn part() -> std::borrow::Cow<'static, str> {
    crate::assets::text("switchboard.md", PART)
}

/// The most of the hook's JSON read.
const INPUT_MAX: u64 = 1 << 20;

/// The hook's answer: the part as SessionStart's added context, with the
/// tab's note pointed at when it has one.
pub fn answer(note: Option<&Path>) -> String {
    let part = part();
    let context = match note {
        Some(note) => format!(
            "{part}\n# This tab's note\n\nThis tab's note is `{}`. Read it before your first reply: \
             the standing rules it keeps apply to this session too, whatever repository it works in.\n",
            note.display()
        ),
        None => part.into_owned(),
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

/// Whether a session started as `entrypoint` (Claude Code's
/// `CLAUDE_CODE_ENTRYPOINT`, when set) is an interactive one.
pub fn interactive(entrypoint: Option<&str>) -> bool {
    entrypoint.is_none_or(|e| e.is_empty() || e == "cli")
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
        let _ = out.write_all(answer(tab_note().as_deref()).as_bytes());
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
        let answer: Value = serde_json::from_str(&answer(None)).expect("JSON");
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
            serde_json::from_str(&answer(Some(Path::new("/n/Plans/A b.md")))).expect("JSON");
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
    fn only_an_interactive_session_gets_it() {
        assert!(interactive(Some("cli")));
        assert!(interactive(None), "an older Claude Code that doesn't say");
        assert!(!interactive(Some("sdk-cli")));
        assert!(!interactive(Some("sdk-ts")));
        assert!(!interactive(Some("claude-vscode")));
    }
}
