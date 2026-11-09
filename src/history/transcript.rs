//! What a tab's history reads from its Claude session's transcript: the
//! moments the timeline marks (your turns, commits, review rounds, the
//! answers you gave at questions, compactions) and the context the session
//! used over time. New with UX 2.0's board 6, with no tmux-version file
//! behind it.
//!
//! Claude Code keeps each conversation as JSON lines under
//! `~/.claude/projects/`, which isn't documented; a line this doesn't
//! recognise is passed over, so a change of shape loses a lane's marks and
//! nothing else. A transcript can be tens of megabytes, so it is read off
//! the server's task (`read_from`) and only as far as it has grown since
//! the last read, a whole line at a time.
//!
//! The context a reply used is its prompt's tokens (`input_tokens` and the
//! cache's two) over the model's window, which the transcript doesn't say:
//! the window is worked out from what the statusline last wrote for the
//! session (its used share against the last reply's tokens), else taken as
//! 200k tokens, or a million once a reply used more than that.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::sidebar::claude::prompt_text;

/// A commit made in the session, by the `git commit` that made it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub at: i64,
    /// Its short hash, from git's `[branch abc1234] subject`; empty when the
    /// output didn't say.
    pub hash: String,
}

/// A round of `switchboard codex-review`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Round {
    pub at: i64,
    /// Its number in the session, from 1.
    pub number: usize,
    /// It found nothing, when its JSON came back in the call's result.
    pub clean: bool,
}

/// A question asked with AskUserQuestion and what you chose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// When it was asked, and when you answered.
    pub asked: i64,
    pub at: i64,
    pub chose: String,
}

/// A compaction, with the context before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compaction {
    pub at: i64,
    /// The prompt's tokens before it, as Claude Code recorded them.
    pub before: u64,
}

/// What the timeline marks, read from a transcript. Times are seconds
/// since the epoch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lanes {
    /// The transcript's first entry with a time: where the span starts.
    pub first: Option<i64>,
    pub turns: Vec<i64>,
    pub commits: Vec<Commit>,
    pub rounds: Vec<Round>,
    pub answers: Vec<Answer>,
    pub compactions: Vec<Compaction>,
    /// Each reply's prompt tokens, as it came.
    pub context: Vec<(i64, u64)>,
    /// The calls not answered yet, by id: (tool, asked at, round number),
    /// for a result that comes in a later read.
    pending: Vec<(String, Pending)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Pending {
    Commit,
    Round(usize),
    Ask(i64),
}

/// The window a session's context is measured against, in tokens.
pub const WINDOW: u64 = 200_000;
pub const LARGE_WINDOW: u64 = 1_000_000;

impl Lanes {
    /// The window: from the statusline's last `used` share against the
    /// last reply's tokens when it has one, else by the most a reply used.
    pub fn window(&self, used: Option<f64>) -> u64 {
        let last = self.context.last().map(|(_, t)| *t).unwrap_or(0);
        if let Some(used) = used.filter(|u| *u > 1.0)
            && last > 0
        {
            let implied = last as f64 * 100.0 / used;
            // The nearer of the two windows Claude models have.
            return if (implied - WINDOW as f64).abs() < (implied - LARGE_WINDOW as f64).abs() {
                WINDOW
            } else {
                LARGE_WINDOW
            };
        }
        if self.context.iter().any(|(_, t)| *t > WINDOW) {
            LARGE_WINDOW
        } else {
            WINDOW
        }
    }

    /// Takes in the lines of what the transcript has grown by.
    pub fn take(&mut self, text: &str) {
        for line in text.split('\n').filter(|l| l.starts_with('{')) {
            if let Ok(entry) = serde_json::from_str::<Value>(line) {
                self.entry(&entry);
            }
        }
    }

    fn entry(&mut self, entry: &Value) {
        let flag = |k: &str| entry.get(k) == Some(&Value::Bool(true));
        if flag("isSidechain") {
            return;
        }
        let at = seconds(entry);
        if at == 0 {
            return;
        }
        self.first.get_or_insert(at);
        if flag("isMeta") {
            return;
        }
        match str_of(entry.get("type")) {
            "system" if str_of(entry.get("subtype")) == "compact_boundary" => {
                let before = entry
                    .pointer("/compactMetadata/preTokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                self.compactions.push(Compaction { at, before });
            }
            // A message typed while Claude works is queued as an attachment
            // rather than a user entry: a turn of yours as well, unlike a
            // background task's notification queued the same way.
            "attachment"
                if entry.pointer("/attachment/type").and_then(Value::as_str)
                    == Some("queued_command")
                    && by_you(entry.pointer("/attachment/origin/kind"))
                    && entry
                        .pointer("/attachment/commandMode")
                        .and_then(Value::as_str)
                        .is_none_or(|m| m == "prompt") =>
            {
                self.turns.push(at);
            }
            "assistant" => self.reply(entry, at),
            "user" => self.user(entry, at),
            _ => {}
        }
    }

    fn reply(&mut self, entry: &Value, at: i64) {
        if let Some(usage) = entry.pointer("/message/usage") {
            let n = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0);
            let tokens =
                n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens");
            // Each part of a message is an entry of its own, with the
            // message's usage again.
            if tokens > 0 && self.context.last() != Some(&(at, tokens)) {
                self.context.push((at, tokens));
            }
        }
        let parts = entry
            .pointer("/message/content")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for part in parts.iter().filter(|p| str_of(p.get("type")) == "tool_use") {
            let id = str_of(part.get("id")).to_string();
            let name = str_of(part.get("name"));
            let command = part
                .pointer("/input/command")
                .and_then(Value::as_str)
                .unwrap_or("");
            let pending = if name == "AskUserQuestion" {
                Pending::Ask(at)
            } else if name == "Bash" && is_review(command) {
                let number = self.rounds.len() + 1;
                self.rounds.push(Round {
                    at,
                    number,
                    clean: false,
                });
                Pending::Round(number)
            } else if name == "Bash" && is_commit(command) {
                Pending::Commit
            } else {
                continue;
            };
            self.pending.push((id, pending));
        }
    }

    fn user(&mut self, entry: &Value, at: i64) {
        let content = entry.pointer("/message/content");
        let results: Vec<&Value> = content
            .and_then(Value::as_array)
            .map(|parts| {
                parts
                    .iter()
                    .filter(|p| str_of(p.get("type")) == "tool_result")
                    .collect()
            })
            .unwrap_or_default();
        if results.is_empty() {
            if by_you(entry.pointer("/origin/kind")) && prompt_text(entry).is_some() {
                self.turns.push(at);
            }
            return;
        }
        for result in results {
            let id = str_of(result.get("tool_use_id"));
            let Some(i) = self.pending.iter().position(|(p, _)| p == id) else {
                continue;
            };
            let (_, pending) = self.pending.remove(i);
            let failed = result.get("is_error") == Some(&Value::Bool(true));
            let text = result_text(result);
            match pending {
                // git's summary line says the commit was made, whatever came
                // after it in the same call (a push refused).
                Pending::Commit => {
                    if let Some(hash) = commit_hash(&text) {
                        self.commits.push(Commit { at, hash });
                    }
                }
                Pending::Round(number) => {
                    if let Some(round) = self.rounds.iter_mut().find(|r| r.number == number) {
                        round.clean = !failed && review_clean(&text);
                    }
                }
                Pending::Ask(asked) if !failed => {
                    let chose = answers(&text);
                    if !chose.is_empty() {
                        self.answers.push(Answer { asked, at, chose });
                    }
                }
                _ => {}
            }
        }
    }
}

/// What a transcript has grown by since `offset`, a whole line at a time:
/// the text and the offset to read from next. Off the server's task.
pub fn read_from(file: &Path, offset: u64) -> Option<(String, u64)> {
    let mut f = std::fs::File::open(file).ok()?;
    let size = f.metadata().ok()?.len();
    // A transcript rewritten shorter is read again from its start.
    let offset = if size < offset { 0 } else { offset };
    f.seek(SeekFrom::Start(offset)).ok()?;
    let mut bytes = Vec::new();
    f.take(size - offset).read_to_end(&mut bytes).ok()?;
    let whole = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    bytes.truncate(whole);
    Some((
        String::from_utf8_lossy(&bytes).into_owned(),
        offset + whole as u64,
    ))
}

/// An entry you typed, by its origin: none, or `human`, never a background
/// task's notification or a subagent's report (`peer`), which Claude Code
/// writes as user entries and queues the same way.
fn by_you(kind: Option<&Value>) -> bool {
    kind.and_then(Value::as_str).is_none_or(|k| k == "human")
}

fn seconds(entry: &Value) -> i64 {
    entry
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|t| t.parse::<jiff::Timestamp>().ok())
        .map(|t| t.as_second())
        .unwrap_or(0)
}

fn str_of(v: Option<&Value>) -> &str {
    v.and_then(Value::as_str).unwrap_or("")
}

/// A tool result's text, whether it is a string or a list of parts.
fn result_text(result: &Value) -> String {
    match result.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// A Bash command that runs `git commit`, git's own options before it or
/// not (`git -C dir commit`, `git -c key=value commit`), not one that only
/// mentions it.
pub fn is_commit(command: &str) -> bool {
    static COMMIT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)(?:^|[;&|(]\s*)git(?:\s+-[^\s=]+(?:=\S+|\s+[^-\s]\S*)?)*\s+commit\b")
            .expect("a regex")
    });
    COMMIT.is_match(command)
}

/// A Bash command that runs a review round: `codex-review` run as a
/// command (`switchboard codex-review …`, or the link by its name), not one
/// that only mentions it, asks for its help or for a patch id.
fn is_review(command: &str) -> bool {
    static REVIEW: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)(?:^|[;&|(]\s*)(?:\S*switchboard\s+)?codex-review\b").expect("a regex")
    });
    REVIEW.is_match(command)
        && !["--patch-id", "--help", " -h"]
            .iter()
            .any(|flag| command.contains(flag))
}

/// The hash in git commit's first line, `[main abc1234] subject`.
fn commit_hash(text: &str) -> Option<String> {
    static HASH: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^\[[^\]\n]*?\b([0-9a-f]{7,40})\]").expect("a regex"));
    HASH.captures(text).map(|c| c[1].to_string())
}

/// Whether a round's JSON says it found nothing.
fn review_clean(text: &str) -> bool {
    let Some(start) = text.find('{') else {
        return false;
    };
    let Some(end) = text.rfind('}') else {
        return false;
    };
    serde_json::from_str::<Value>(&text[start..=end])
        .ok()
        .and_then(|v| v.get("findings").and_then(Value::as_array).map(Vec::len))
        == Some(0)
}

/// What you chose at a question, from AskUserQuestion's result
/// (`User has answered your questions: "Q"="A", "Q2"="B". ...`).
fn answers(text: &str) -> String {
    static PAIR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#""[^"]*"="([^"]*)""#).expect("a regex"));
    PAIR.captures_iter(text)
        .map(|c| c[1].to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(v: Value) -> String {
        format!("{v}\n")
    }

    fn at(minute: u32) -> String {
        format!("2026-11-08T13:{minute:02}:00Z")
    }

    fn secs(minute: u32) -> i64 {
        at(minute).parse::<jiff::Timestamp>().unwrap().as_second()
    }

    fn bash(minute: u32, id: &str, command: &str, tokens: u64) -> String {
        line(json!({
            "type": "assistant", "timestamp": at(minute),
            "message": {
                "usage": {"input_tokens": 10, "cache_read_input_tokens": tokens - 10},
                "content": [{"type": "tool_use", "id": id, "name": "Bash", "input": {"command": command}}]
            }
        }))
    }

    fn result(minute: u32, id: &str, text: &str) -> String {
        line(json!({
            "type": "user", "timestamp": at(minute),
            "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": text}]}
        }))
    }

    #[test]
    fn reads_turns_commits_rounds_and_answers() {
        let mut text = String::new();
        text += &line(
            json!({"type": "user", "timestamp": at(0), "message": {"content": "fix the bug"}}),
        );
        text += &bash(2, "c1", "git add -A && git commit -m 'Fix it'", 20_000);
        text += &result(3, "c1", "[main 3f2a91c] Fix it\n 1 file changed");
        text += &bash(4, "r1", "switchboard codex-review --notes n.md", 30_000);
        text += &result(
            9,
            "r1",
            r#"{"summary":"x","findings":[{"severity":"medium"}]}"#,
        );
        text += &bash(10, "r2", "switchboard codex-review --notes n.md", 40_000);
        text += &result(14, "r2", r#"{"summary":"x","findings":[]}"#);
        text += &line(json!({
            "type": "assistant", "timestamp": at(15),
            "message": {"content": [{"type": "tool_use", "id": "q1", "name": "AskUserQuestion", "input": {}}]}
        }));
        text += &result(
            21,
            "q1",
            r#"User has answered your questions: "Fix it?"="1. Fix it". You can now continue."#,
        );
        text += &bash(22, "e1", "echo git commit is not run here", 41_000);
        text += &line(
            json!({"type": "user", "timestamp": at(23), "isMeta": true, "message": {"content": "caveat"}}),
        );
        let mut lanes = Lanes::default();
        lanes.take(&text);
        assert_eq!(lanes.first, Some(secs(0)));
        assert_eq!(lanes.turns, vec![secs(0)]);
        assert_eq!(
            lanes.commits,
            vec![Commit {
                at: secs(3),
                hash: "3f2a91c".into()
            }]
        );
        assert_eq!(
            lanes.rounds,
            vec![
                Round {
                    at: secs(4),
                    number: 1,
                    clean: false
                },
                Round {
                    at: secs(10),
                    number: 2,
                    clean: true
                },
            ]
        );
        assert_eq!(
            lanes.answers,
            vec![Answer {
                asked: secs(15),
                at: secs(21),
                chose: "1. Fix it".into()
            }]
        );
        assert_eq!(
            lanes.context.iter().map(|(_, t)| *t).collect::<Vec<_>>(),
            vec![20_000, 30_000, 40_000, 41_000]
        );
    }

    #[test]
    fn a_commit_with_gits_own_options_before_it_counts() {
        assert!(is_commit("git commit -m x"));
        assert!(is_commit("git -C /w commit -m x"));
        assert!(is_commit("git -c commit.gpgsign=false commit -m x"));
        assert!(is_commit("git --no-pager commit -m x"));
        assert!(is_commit("git add -A && git commit -m x"));
        assert!(!is_commit("echo git commit"));
        assert!(!is_commit("git log --grep commit"));
    }

    #[test]
    fn only_a_review_run_counts_as_a_round() {
        assert!(is_review("switchboard codex-review --notes n.md"));
        assert!(is_review("cd /w && switchboard codex-review --base x"));
        assert!(is_review("codex-review --notes n.md"));
        assert!(!is_review("rg -n codex-review src"));
        assert!(!is_review("switchboard codex-review --help"));
        assert!(!is_review("switchboard codex-review --patch-id a b"));
    }

    #[test]
    fn a_commit_counts_when_a_later_command_in_its_call_fails() {
        let mut lanes = Lanes::default();
        lanes.take(&bash(2, "c1", "git commit -m x && git push", 1000));
        lanes.take(&line(json!({
            "type": "user", "timestamp": at(3),
            "message": {"content": [{"type": "tool_result", "tool_use_id": "c1", "is_error": true,
                "content": "[main a1c3e52] x\nerror: failed to push"}]}
        })));
        assert_eq!(lanes.commits.len(), 1);
    }

    #[test]
    fn a_message_typed_while_claude_works_is_a_turn() {
        let mut lanes = Lanes::default();
        lanes.take(&line(
            json!({"type": "attachment", "timestamp": at(5), "attachment": {
            "type": "queued_command", "commandMode": "prompt", "prompt": "also do x"}}),
        ));
        lanes.take(&line(json!({"type": "attachment", "timestamp": at(6), "attachment": {
            "type": "queued_command", "commandMode": "task-notification", "prompt": "<task-notification/>"}})));
        lanes.take(&line(
            json!({"type": "attachment", "timestamp": at(7), "attachment": {
            "type": "queued_command", "prompt": "and y"}}),
        ));
        lanes.take(&line(
            json!({"type": "user", "timestamp": at(8), "origin": {"kind": "peer"},
            "message": {"content": "An agent reported back"}}),
        ));
        assert_eq!(lanes.turns, vec![secs(5), secs(7)]);
    }

    #[test]
    fn a_result_in_a_later_read_finds_its_call() {
        let mut lanes = Lanes::default();
        lanes.take(&bash(2, "c1", "git -C /w commit -m x", 1000));
        lanes.take(&result(3, "c1", "[b a1c3e52] x"));
        assert_eq!(lanes.commits.len(), 1);
        assert_eq!(lanes.commits[0].hash, "a1c3e52");
    }

    #[test]
    fn a_compaction_is_marked_with_the_context_before_it() {
        let mut lanes = Lanes::default();
        lanes.take(&line(json!({
            "type": "system", "subtype": "compact_boundary", "timestamp": at(30),
            "compactMetadata": {"trigger": "auto", "preTokens": 132_000}
        })));
        assert_eq!(
            lanes.compactions,
            vec![Compaction {
                at: secs(30),
                before: 132_000
            }]
        );
    }

    #[test]
    fn the_window_follows_the_statusline_else_the_largest_reply() {
        let mut lanes = Lanes {
            context: vec![(1, 100_000)],
            ..Lanes::default()
        };
        assert_eq!(lanes.window(None), WINDOW);
        assert_eq!(lanes.window(Some(50.0)), WINDOW);
        assert_eq!(lanes.window(Some(10.0)), LARGE_WINDOW);
        lanes.context.push((2, 300_000));
        assert_eq!(lanes.window(None), LARGE_WINDOW);
    }

    #[test]
    fn reads_whole_lines_only_and_goes_on_from_there() {
        let dir = std::env::temp_dir().join(format!("sb-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("t.jsonl");
        std::fs::write(&file, "{\"a\":1}\n{\"b\":").unwrap();
        let (text, next) = read_from(&file, 0).unwrap();
        assert_eq!(text, "{\"a\":1}\n");
        assert_eq!(next, 8);
        std::fs::write(&file, "{\"a\":1}\n{\"b\":2}\n").unwrap();
        let (text, next) = read_from(&file, next).unwrap();
        assert_eq!(text, "{\"b\":2}\n");
        assert_eq!(next, 16);
        let _ = std::fs::remove_dir_all(dir);
    }
}
