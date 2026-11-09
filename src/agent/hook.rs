//! `switchboard hook <event>`: what Claude Code's hooks run, with the hook's
//! JSON on stdin (tmux/agent-state.sh's `set` and `notification`). In a pane
//! it hands the event to the server, which marks the pane; outside one it
//! does only what agent-state.sh did outside tmux: tells the Slack relay,
//! and follows a conversation moved into the background back to its tab.
//!
//! Claude Code waits on its hooks, runs this before and after every tool
//! call, and adds some hooks' output to the conversation, so it prints
//! nothing, waits at most a second on a server that doesn't answer, and
//! always exits 0: no server, a pane the server doesn't know, JSON that
//! can't be read, even a panic, and Claude goes on as if nothing had run.
//! The one exception is autopilot's hands-free mode (src/agent/hands_free.rs):
//! for PreToolUse and Stop in a pane, the server may answer with a JSON
//! object for Claude Code to read (a question denied, a stop blocked), which
//! is printed as it came; any other answer prints nothing. A Stop in a pane
//! is told to the Slack relay only after the server's answer, and not at
//! all when the server blocked it.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::{Map, Value};

use super::sessions;
use super::state::QUESTION_KINDS;
use crate::mux::socket;
use crate::{paths, util};

/// `switchboard hook <event>`.
pub fn main(args: Vec<String>) -> ExitCode {
    // A panic's message would reach the conversation; the default hook
    // printing it is the only output this could make.
    std::panic::set_hook(Box::new(|_| {}));
    let _ = std::panic::catch_unwind(|| run(&args));
    ExitCode::SUCCESS
}

fn run(args: &[String]) {
    // The relay's own Haiku sessions (bin/slack-relay) inherit these hooks;
    // their turns are Slack calls, not work anyone is waiting on.
    if std::env::var_os("SLACK_RELAY_HELPER").is_some_and(|v| !v.is_empty()) {
        return;
    }
    let input = read_input();
    let payload = serde_json::from_slice::<Value>(&input)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()));
    let event = args
        .first()
        .filter(|e| !e.is_empty())
        .cloned()
        .or_else(|| text(&payload, "hook_event_name"))
        .unwrap_or_default();
    if event.is_empty() {
        return;
    }
    let session = text(&payload, "session_id").unwrap_or_default();
    // A headless Claude (a review loop's `claude -p`) inherits the tab's
    // SWITCHBOARD_PANE and runs these hooks too; its turns are not the tab's,
    // so they mark nothing and tell the relay nothing.
    let files = if session.is_empty() {
        Vec::new()
    } else {
        sessions::read_all(&sessions::sessions_dir())
    };
    if sessions::is_headless(&files, &session) {
        return;
    }
    let pane = std::env::var("SWITCHBOARD_PANE")
        .ok()
        .filter(|p| !p.is_empty());
    let answers = pane.is_some() && matches!(event.as_str(), "PreToolUse" | "Stop");
    // A Stop the server may block is told to the relay only once it stands,
    // so a hands-free turn sent back to carry on isn't reported done.
    let stop_waits = answers && event == "Stop";
    if !stop_waits {
        tell_relay(&event, &payload);
    }
    let mut command = vec!["hook".to_string(), event.clone()];
    if pane.is_none() {
        // A conversation moved into the background runs in a job with no
        // pane of its own, while the tab it came from keeps a parked Claude
        // whose session names the job. Its hooks are marked on that tab, so
        // the tab shows what the conversation is doing rather than idle.
        let pids = sessions::parked_chain(&files, &session);
        if pids.is_empty() {
            return;
        }
        let list: Vec<String> = pids.iter().map(u32::to_string).collect();
        command.push("--parked".into());
        command.push(list.join(","));
    }
    let answer = socket::ask_quiet(command, pane, Some(input))
        .filter(|_| answers)
        .and_then(|(code, out, _)| (code == 0).then(|| answer_json(&out)).flatten());
    if stop_waits && answer.is_none() {
        tell_relay(&event, &payload);
    }
    if let Some(json) = answer {
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(json.as_bytes());
        let _ = stdout.flush();
    }
}

/// The server's answer to a hook, when it is a JSON object, the only kind
/// Claude Code reads from a hook; anything else is no answer.
fn answer_json(out: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(out).ok()?.trim();
    serde_json::from_str::<Value>(text)
        .ok()
        .filter(Value::is_object)
        .map(|_| text.to_string())
}

/// The hook's JSON: stdin when it is a pipe or a file. One run by hand at a
/// terminal reads nothing, rather than waiting for input that won't come.
fn read_input() -> Vec<u8> {
    let mut input = Vec::new();
    if socket::stdin_is_data() {
        let _ = std::io::stdin().lock().read_to_end(&mut input);
    }
    input
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

/// The relay's events file (lib/slack-relay/config.js's `EVENTS_FILE`),
/// which it keeps while it runs and removes only when stopped on purpose, so
/// a crashed relay picks up where it left off.
fn relay_events() -> PathBuf {
    paths::home_join(".local/state/slack-relay/events.jsonl")
}

/// What the relay hears of a hook: a prompt taken, a turn finished, or
/// Claude waiting on you. The relay wants these from every session, outside
/// the workspace too, so it is told before anything asks for a pane.
fn relay_kind(event: &str, payload: &Value) -> Option<&'static str> {
    match event {
        "UserPromptSubmit" => Some("prompt"),
        "Stop" => Some("done"),
        "PermissionRequest" => Some("waiting"),
        "Notification" => payload
            .get("notification_type")
            .and_then(Value::as_str)
            .filter(|k| QUESTION_KINDS.contains(k))
            .map(|_| "waiting"),
        _ => None,
    }
}

/// The event's line, as agent-state.sh's `jq -c` wrote it: the session and
/// its transcript, which say what happened, and for a prompt its first 300
/// characters, for a finished turn its final message, since the transcript
/// can lag behind the Stop hook.
fn relay_line(kind: &str, payload: &Value, now: i64) -> String {
    let field = |key: &str| payload.get(key).cloned().unwrap_or(Value::Null);
    let mut line = Map::new();
    line.insert("event".into(), Value::from(kind));
    line.insert("time".into(), Value::from(now));
    for key in ["session_id", "transcript_path", "cwd"] {
        line.insert(key.into(), field(key));
    }
    // jq's `if .prompt then`: anything but null and false.
    let present = |v: &Value| !matches!(v, Value::Null | Value::Bool(false));
    match field("prompt") {
        Value::String(p) => {
            line.insert(
                "prompt".into(),
                Value::from(p.chars().take(300).collect::<String>()),
            );
        }
        v if present(&v) => {
            line.insert("prompt".into(), v);
        }
        _ => {}
    }
    let last = field("last_assistant_message");
    if present(&last) {
        line.insert("last".into(), last);
    }
    format!("{}\n", util::json_compact(&Value::Object(line)))
}

/// Appends the hook's event to the relay's file while the relay runs.
/// Without the file nothing is written, and it is never made here: one made
/// after the relay stopped would read as a relay that crashed.
fn tell_relay(event: &str, payload: &Value) {
    let Some(kind) = relay_kind(event, payload) else {
        return;
    };
    let file = relay_events();
    if !file.is_file() {
        return;
    }
    if let Ok(mut f) = OpenOptions::new().append(true).open(&file) {
        // One write, so lines from sessions finishing at once don't mix.
        let _ = f.write_all(relay_line(kind, payload, util::now_secs()).as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_a_json_object_is_passed_on_to_claude_code() {
        assert_eq!(
            answer_json(b"{\"decision\":\"block\"}\n").as_deref(),
            Some("{\"decision\":\"block\"}")
        );
        assert_eq!(answer_json(b""), None);
        assert_eq!(answer_json(b"[1]"), None);
        assert_eq!(answer_json(b"not json"), None);
    }

    #[test]
    fn the_relay_hears_of_prompts_turns_and_waits() {
        assert_eq!(relay_kind("UserPromptSubmit", &json!({})), Some("prompt"));
        assert_eq!(relay_kind("Stop", &json!({})), Some("done"));
        assert_eq!(relay_kind("PermissionRequest", &json!({})), Some("waiting"));
        assert_eq!(
            relay_kind(
                "Notification",
                &json!({"notification_type": "elicitation_dialog"})
            ),
            Some("waiting")
        );
        assert_eq!(
            relay_kind("Notification", &json!({"notification_type": "idle_prompt"})),
            None
        );
        assert_eq!(relay_kind("PreToolUse", &json!({})), None);
    }

    #[test]
    fn a_relay_line_is_what_jq_wrote() {
        let prompt = "é".repeat(400);
        let line = relay_line(
            "prompt",
            &json!({"session_id": "s1", "cwd": "/w", "prompt": prompt, "tool_name": "x"}),
            1700000000,
        );
        let expected_prompt = "é".repeat(300);
        assert_eq!(
            line,
            format!(
                "{{\"event\":\"prompt\",\"time\":1700000000,\"session_id\":\"s1\",\"transcript_path\":null,\"cwd\":\"/w\",\"prompt\":\"{expected_prompt}\"}}\n"
            )
        );
        let done = relay_line(
            "done",
            &json!({"session_id": "s1", "transcript_path": "/t.jsonl", "cwd": "/w", "last_assistant_message": "All set."}),
            5,
        );
        assert_eq!(
            done,
            "{\"event\":\"done\",\"time\":5,\"session_id\":\"s1\",\"transcript_path\":\"/t.jsonl\",\"cwd\":\"/w\",\"last\":\"All set.\"}\n"
        );
    }
}
