//! `switchboard claude-context`: Switchboard's part of Claude's instructions
//! (`assets/switchboard.md`: the notes app, `switchboard tabs`, talking to
//! another tab and naming one, a repository's own skills), printed as a
//! SessionStart hook's `additionalContext`, which Claude Code adds to the
//! conversation as a session starts, resumes, clears or compacts. New with
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

use std::io::{Read, Write};
use std::process::ExitCode;

use serde_json::{Value, json};

use crate::agent::sessions;
use crate::mux::socket;

/// Switchboard's part, as compiled in.
pub const PART: &str = include_str!("../../assets/switchboard.md");

/// The most of the hook's JSON read.
const INPUT_MAX: u64 = 1 << 20;

/// The hook's answer: the part as SessionStart's added context.
pub fn answer() -> String {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": PART,
        }
    })
    .to_string()
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
        let _ = out.write_all(answer().as_bytes());
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
        let answer: Value = serde_json::from_str(&answer()).expect("JSON");
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
        assert!(!PART.contains("tmux"));
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
