//! What each Claude session is talking about, read from Claude Code's own
//! files and nothing else (lib/sidebar/claude.js): no processes, so it can
//! run on every refresh. Claude Code writes `~/.claude/sessions/<pid>.json`
//! for each process it runs, naming its session, the name it was given and
//! where it started, and keeps each conversation as a transcript under
//! `~/.claude/projects/`. Neither is documented, so if one changes shape the
//! sidebar quietly loses what it read from it and nothing else is affected.
//!
//! A pane's session is found by its process, where claude.js found it by the
//! `tmux` field Claude Code wrote from `TMUX_PANE`: the session file whose
//! process is in the pane's foreground process group. A conversation moved
//! into the background is carried on by a job whose session file has the
//! parked session's `parkedJobId` as its `jobId`, and the tab shows the job's
//! conversation, as the agent state marks the tab with the job's state.
//!
//! Each transcript is read only as far as it has grown since the last look
//! (`Reader::background_work`), and its last turn only when its size or time
//! has changed (`Reader::last_turn`), so a refresh reads a few new lines.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;

use indexmap::IndexMap;
use regex::Regex;
use serde_json::Value;

/// What a session file says, as far as the sidebar needs it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionInfo {
    pub pid: Option<u32>,
    pub session_id: String,
    pub cwd: Option<String>,
    pub name: Option<String>,
    /// How it got its name: `derived` (from the first prompt) isn't shown.
    pub name_source: Option<String>,
    /// When its process started, in milliseconds since the epoch.
    pub started_at: Option<i64>,
    pub entrypoint: String,
    pub job_id: String,
    pub parked_job_id: String,
}

impl SessionInfo {
    fn from_json(v: &Value) -> SessionInfo {
        let text = |key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
        let pid = match v.get("pid") {
            Some(Value::Number(n)) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
            Some(Value::String(s)) => s.trim().parse().ok(),
            _ => None,
        };
        SessionInfo {
            pid,
            session_id: text("sessionId").unwrap_or_default(),
            cwd: text("cwd"),
            name: text("name"),
            name_source: text("nameSource"),
            started_at: v.get("startedAt").and_then(Value::as_f64).map(|n| n as i64),
            entrypoint: text("entrypoint").unwrap_or_default(),
            job_id: text("jobId").unwrap_or_default(),
            parked_job_id: text("parkedJobId").unwrap_or_default(),
        }
    }

    /// A headless Claude's (`entrypoint` `sdk-cli`), such as one a review
    /// loop's script starts: its turns aren't the tab's.
    pub fn headless(&self) -> bool {
        self.entrypoint.starts_with("sdk")
    }
}

/// The conversation a pane shows: the session in front of it, or the
/// background job it parked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionRef {
    pub session_id: String,
    pub cwd: Option<String>,
    /// The name it was given by you or a skill, not one derived.
    pub name: Option<String>,
    pub started_at: Option<i64>,
}

/// The session shown for a pane, from the sessions whose processes are in
/// front on it: its own interactive Claude wins over a headless one that is
/// there too, and a conversation it moved into the background is followed
/// to the job that carries it on.
pub fn session_of(all: &[SessionInfo], front: &[&SessionInfo]) -> Option<SessionRef> {
    let session = front
        .iter()
        .filter(|s| !s.session_id.is_empty())
        .min_by_key(|s| s.headless())?;
    let mut current = *session;
    let mut seen: HashSet<&str> = HashSet::from([current.session_id.as_str()]);
    while !current.parked_job_id.is_empty() {
        let Some(job) = all.iter().find(|o| {
            o.job_id == current.parked_job_id
                && !o.session_id.is_empty()
                && !seen.contains(o.session_id.as_str())
        }) else {
            break;
        };
        seen.insert(&job.session_id);
        current = job;
    }
    let named = session
        .name_source
        .as_deref()
        .is_some_and(|s| s != "derived")
        .then(|| session.name.clone())
        .flatten();
    Some(SessionRef {
        session_id: current.session_id.clone(),
        cwd: current.cwd.clone().or_else(|| session.cwd.clone()),
        name: named,
        started_at: current.started_at.or(session.started_at),
    })
}

/// Claude Code keeps a project's transcripts in a directory named after its
/// path with every character but a letter or digit turned into `-`.
pub fn project_dir_name(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

// ── Background work ─────────────────────────────────────────────────────────

/// The skills that loop in the background, told apart by what their
/// background commands run: while one of a loop's commands runs, the work is
/// that loop, and its round is how many of them have started since its skill
/// last began, so many to a round. A review loop runs codex-review twice a
/// round; a CI loop watches the checks once a round, starting again after
/// each fix; queue-and-clean watches the merge queue (its watch also reads
/// the checks, so it is tested first), or its landing waits for its landing
/// lock. deploy-test, after a merge, waits for the deploy, which marks its
/// rounds; preview-test runs deploy-test's scripts on a PR's preview, so its
/// entry only marks where its rounds start.
struct LoopDef {
    kind: &'static str,
    skill: &'static str,
    command: &'static LazyLock<Regex>,
    per_round: usize,
}

static QUEUE_COMMAND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"mergeQueueEntry|land-lock\.js wait").expect("a regex"));
static REVIEW_COMMAND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"codex-review").expect("a regex"));
static CI_COMMAND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"gh pr checks").expect("a regex"));
static DEPLOY_COMMAND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"wait-deploy\.js").expect("a regex"));

static LOOPS: [LoopDef; 5] = [
    LoopDef {
        kind: "queue",
        skill: "queue-and-clean",
        command: &QUEUE_COMMAND,
        per_round: 1,
    },
    LoopDef {
        kind: "review",
        skill: "review-loop",
        command: &REVIEW_COMMAND,
        per_round: 2,
    },
    LoopDef {
        kind: "ci",
        skill: "ci-loop",
        command: &CI_COMMAND,
        per_round: 1,
    },
    LoopDef {
        kind: "deploy",
        skill: "deploy-test",
        command: &DEPLOY_COMMAND,
        per_round: 1,
    },
    LoopDef {
        kind: "deploy",
        skill: "preview-test",
        command: &DEPLOY_COMMAND,
        per_round: 1,
    },
];

fn loop_of(command: &str) -> Option<&'static str> {
    LOOPS
        .iter()
        .find(|l| l.command.is_match(command))
        .map(|l| l.kind)
}

/// A self-paced loop's wakeup counts for this long after it was due, since a
/// busy session fires it late.
const WAKEUP_GRACE_MS: i64 = 5 * 60_000;
/// A recurring cron job lasts this long at most, Claude Code's own limit.
const CRON_LIFE_MS: i64 = 7 * 24 * 60 * 60_000;
/// How much of a transcript is read at a time.
const READ_CHUNK: u64 = 16 * 1024 * 1024;

static STARTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:Command running in background with ID|agentId): ([A-Za-z0-9_-]+)")
        .expect("a regex")
});
// A command that outran its timeout and was moved to the background says so
// at the start of its own result.
static MOVED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^Command did not complete within its \d+s timeout and was moved to the background \(ID: ([A-Za-z0-9_-]+)\)").expect("a regex")
});
static MONITOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Monitor started \(task ([A-Za-z0-9_-]+), expires in (\d+)m").expect("a regex")
});
static TASK_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<task-id>([A-Za-z0-9_-]+)</task-id>").expect("a regex"));
static CRON_JOB: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Scheduled [A-Za-z0-9_-]* ?job ([A-Za-z0-9_]+)").expect("a regex")
});
// Only a line that may say something about background work is parsed.
static MAY_MATTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"background with ID|moved to the background|agentId|<task-id>|run_in_background|review-loop|ci-loop|queue-and-clean|deploy-test|preview-test|"Monitor"|Monitor started|"TaskStop"|"ScheduleWakeup"|"CronCreate"|"CronDelete"|Scheduled [A-Za-z0-9_-]* ?job"#).expect("a regex")
});
static LOCAL_COMMAND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*<(local-command-|command-name>|command-message>|bash-)").expect("a regex")
});
static NOTIFICATION_END: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"</task-notification>(\\n|\s)*$").expect("a regex"));

/// The fields of a notification before its summary, and before it closes,
/// so one with no status never takes the next one's as its own.
fn head_of(block: &str) -> &str {
    let before = block.split("<summary>").next().unwrap_or("");
    before.split("</task-notification>").next().unwrap_or("")
}

/// Whether a notification ends its task. Claude Code writes a task's status,
/// when it has one (completed, failed, killed), among the fields before its
/// summary, and a watch's event after it, with no status. So a status before
/// the summary ends the task, whatever the summary quotes; with none, an
/// event keeps it going and anything else ends it.
fn ends_task(block: &str) -> bool {
    head_of(block).contains("<status>") || !block.contains("<event>")
}

/// The notifications a message or a queued command carries, each a block:
/// the first `<task-notification>`, and each that follows a closed one, as
/// when several arrive together. One inside another's text (its summary
/// quoting one) isn't a notification of its own. Each block runs from its
/// own start to the next notification's, so a summary that mentions the
/// opening tag doesn't cut it short before its event.
fn notification_blocks(text: &str) -> Vec<&str> {
    const OPEN: &str = "<task-notification>";
    let parts: Vec<&str> = text.split(OPEN).collect();
    let mut starts = Vec::new();
    let mut at = parts[0].len();
    for (i, part) in parts.iter().enumerate().skip(1) {
        if i == 1 || NOTIFICATION_END.is_match(parts[i - 1]) {
            starts.push(at);
        }
        at += OPEN.len() + part.len();
    }
    starts
        .iter()
        .enumerate()
        .map(|(i, &start)| {
            let end = starts.get(i + 1).copied().unwrap_or(text.len());
            &text[start + OPEN.len()..end]
        })
        .collect()
}

fn ended_in(text: &str, ended: &mut HashSet<String>) {
    for block in notification_blocks(text) {
        if let Some(id) = TASK_ID.captures(head_of(block)).map(|c| c[1].to_string())
            && ends_task(block)
        {
            ended.insert(id);
        }
    }
}

/// A five-field cron expression's field matching a value (`*`, `5`, `1-5`,
/// `*/5`, `3-59/5`, lists of them).
fn cron_matches(field: &str, value: i64, (low, high): (i64, i64)) -> bool {
    field.split(',').any(|part| {
        let (range, step) = part.split_once('/').unwrap_or((part, "1"));
        let Ok(step) = step.parse::<i64>() else {
            return false;
        };
        let (a, b) = if range == "*" {
            (Some(low), Some(high))
        } else {
            let mut ends = range.split('-');
            (
                ends.next().and_then(|n| n.parse().ok()),
                ends.next().map(|n| n.parse().ok()).unwrap_or(None),
            )
        };
        let Some(a) = a else {
            return false;
        };
        let end = b.unwrap_or(if part.contains('/') { high } else { a });
        step != 0 && value >= a && value <= end && (value - a) % step == 0
    })
}

/// When a five-field cron expression (minute hour day month weekday, in
/// local time, as CronCreate takes it) next matches after `from` (ms),
/// looked for a minute at a time over the next eight days; `None` if it
/// never does.
pub fn next_cron(cron: &str, from: i64) -> Option<i64> {
    let fields: Vec<&str> = cron.split_whitespace().collect();
    if fields.len() != 5 {
        return None;
    }
    const RANGES: [(i64, i64); 5] = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 6)];
    let start = crate::util::local_at_ms(from);
    let mut t = start.with().second(0).subsec_nanosecond(0).build().ok()?;
    for _ in 0..8 * 24 * 60 {
        t = t.checked_add(jiff::SignedDuration::from_mins(1)).ok()?;
        let values = [
            i64::from(t.minute()),
            i64::from(t.hour()),
            i64::from(t.day()),
            i64::from(t.month()),
            i64::from(t.weekday().to_sunday_zero_offset()),
        ];
        if values
            .iter()
            .zip(fields.iter())
            .zip(RANGES)
            .all(|((v, f), r)| cron_matches(f, *v, r))
        {
            return Some(t.timestamp().as_millisecond());
        }
    }
    None
}

/// A loop's prompt, short: a /loop's own command, else its first sentence.
pub fn loop_name(prompt: &str) -> String {
    static LOOP: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^/loop\s+(\d+[smhd]\s+)?").expect("a regex"));
    let text = LOOP.replace(prompt, "");
    let text = text.trim();
    if text.starts_with('/') {
        return text
            .split(char::is_whitespace)
            .next()
            .unwrap_or("")
            .to_string();
    }
    // Up to a space after a full stop, or a line's end.
    let mut prev = '\0';
    for (i, c) in text.char_indices() {
        if c == '\n' || (c.is_whitespace() && prev == '.') {
            return text[..i].to_string();
        }
        prev = c;
    }
    text.to_string()
}

/// A running loop: its kind (`review`, `ci`, `queue`, `deploy`) and round.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopRun {
    pub kind: &'static str,
    pub round: usize,
}

/// A loop on a timer between its ticks: its prompt, short, and when it next
/// runs (ms).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scheduled {
    pub prompt: String,
    pub at: i64,
}

/// What a session has running in the background, as `backgroundWork` gave
/// it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Work {
    /// Background commands, agents and watches still running.
    pub count: usize,
    /// The loop they are, while one runs.
    pub loop_run: Option<LoopRun>,
    /// The Monitor watches running, by description.
    pub monitors: Vec<String>,
    /// A self-paced or timed loop waiting for its next tick.
    pub scheduled: Option<Scheduled>,
    /// The message of a turn an API error cut short, while nothing has been
    /// said since, and when it came (ms).
    pub api_error: Option<(String, i64)>,
}

#[derive(Clone, Debug)]
struct Task {
    at: i64,
    loop_kind: Option<&'static str>,
    monitor: Option<String>,
    until: Option<i64>,
}

#[derive(Clone, Debug)]
struct Cron {
    prompt: String,
    cron: String,
    recurring: bool,
    at: i64,
}

#[derive(Clone, Debug)]
struct Wakeup {
    prompt: String,
    at: i64,
    set: i64,
}

/// What has been read of one transcript.
#[derive(Clone, Debug, Default)]
struct Seen {
    offset: u64,
    rest: Vec<u8>,
    started: IndexMap<String, Task>,
    ended: HashSet<String>,
    commands: HashMap<String, String>,
    watches: HashMap<String, String>,
    loop_at: HashMap<&'static str, i64>,
    cron_calls: HashMap<String, Value>,
    crons: IndexMap<String, Cron>,
    wakeup: Option<Wakeup>,
    api_error: Option<(String, i64)>,
}

fn timestamp_ms(entry: &Value) -> i64 {
    entry
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|t| t.parse::<jiff::Timestamp>().ok())
        .map(|t| t.as_millisecond())
        .unwrap_or(0)
}

fn str_of(v: Option<&Value>) -> &str {
    v.and_then(Value::as_str).unwrap_or("")
}

/// A JSON value as JavaScript's `String()` writes it, for a field read as
/// text whatever it is.
fn string_of(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(v) => crate::util::js_string(v),
    }
}

impl Seen {
    /// One line of a transcript.
    fn line(&mut self, line: &str) {
        // A turn cut short by an API error ("API Error: Connection lost
        // mid-response"): Claude Code records it as a message of its own and
        // runs no Stop hook, so the pane can be left looking busy. It stands
        // until the conversation goes on (anything said after it, by you or
        // Claude), checked by text so the lines between aren't all parsed. A
        // subagent's lines (isSidechain) are its own, not the conversation's.
        let sidechain = line.contains("\"isSidechain\":true");
        if !sidechain && line.contains("\"isApiErrorMessage\":true") {
            if let Ok(entry) = serde_json::from_str::<Value>(line)
                && str_of(entry.get("type")) == "assistant"
                && entry.get("isApiErrorMessage") == Some(&Value::Bool(true))
                && entry.get("isSidechain") != Some(&Value::Bool(true))
            {
                let text = entry
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .and_then(|parts| {
                        parts
                            .iter()
                            .find(|p| str_of(p.get("type")) == "text")
                            .and_then(|p| p.get("text").and_then(Value::as_str))
                    })
                    .filter(|t| !t.is_empty())
                    .unwrap_or("API error");
                let first = text.split('\n').next().unwrap_or("").to_string();
                self.api_error = Some((first, timestamp_ms(&entry)));
            }
            return;
        }
        // Parsed only while an error stands, and only a line that may say
        // something: a user or assistant entry of the conversation itself.
        if self.api_error.is_some()
            && !sidechain
            && (line.contains("\"type\":\"user\"") || line.contains("\"type\":\"assistant\""))
            && let Ok(entry) = serde_json::from_str::<Value>(line)
        {
            // A slash command as typed, its output, a shell command run with
            // ! and its output, a note Claude Code adds (isMeta) or a
            // background task's report doesn't carry the conversation on by
            // itself: a prompt does, and so does Claude's answer.
            let kind = str_of(entry.get("type"));
            let local = entry
                .pointer("/message/content")
                .and_then(Value::as_str)
                .is_some_and(|c| LOCAL_COMMAND.is_match(c));
            let report = str_of(entry.pointer("/origin/kind")) == "task-notification";
            let flag = |k: &str| entry.get(k) == Some(&Value::Bool(true));
            if (kind == "user" || kind == "assistant")
                && !flag("isSidechain")
                && !flag("isMeta")
                && !local
                && !report
            {
                self.api_error = None;
            }
        }
        if !MAY_MATTER.is_match(line) {
            return;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let kind = str_of(entry.get("type"));
        // A task's end, wherever Claude Code wrote it: in a user message, or,
        // in newer versions, as a queued command in an attachment entry. Only
        // a queued command marked as a task's notification: a message you
        // typed mid-turn is queued the same way, marked as a prompt.
        let attachment = entry.get("attachment");
        let queued = attachment
            .filter(|a| {
                str_of(a.get("type")) == "queued_command"
                    && (str_of(a.get("commandMode")) == "task-notification"
                        || str_of(a.pointer("/origin/kind")) == "task-notification")
            })
            .and_then(|a| a.get("prompt").and_then(Value::as_str))
            .unwrap_or("");
        if kind != "user" && kind != "assistant" && !queued.is_empty() {
            ended_in(queued, &mut self.ended);
            return;
        }
        // The conversation alone: Claude Code also writes copies of each
        // request (api-request-blob) with no time of their own, which would
        // start every task again at time 0, before the session.
        if kind != "user" && kind != "assistant" {
            return;
        }
        let at = timestamp_ms(&entry);
        let content = entry
            .pointer("/message/content")
            .cloned()
            .unwrap_or(Value::String(String::new()));
        let text = content.to_string();
        let parts: Vec<Value> = content.as_array().cloned().unwrap_or_default();
        // A loop's skill starting: typed as /<skill>, or run by the Skill tool.
        for l in &LOOPS {
            if kind == "user"
                && text.contains(&format!("<command-name>/{}</command-name>", l.skill))
            {
                self.loop_at.insert(l.kind, at);
            }
        }
        for part in &parts {
            let ptype = str_of(part.get("type"));
            let name = str_of(part.get("name"));
            let id = str_of(part.get("id")).to_string();
            let input = part.get("input");
            let field = |k: &str| input.and_then(|i| i.get(k));
            if ptype == "tool_use" {
                if name == "Skill"
                    && let Some(l) = LOOPS.iter().find(|l| str_of(field("skill")) == l.skill)
                {
                    self.loop_at.insert(l.kind, at);
                }
                // What each background command runs, by its tool call.
                if field("run_in_background").is_some_and(crate::util::truthy) {
                    let command = field("command")
                        .filter(|v| !v.is_null())
                        .or_else(|| field("prompt"));
                    self.commands.insert(id.clone(), string_of(command));
                }
                if name == "Monitor" {
                    let description = field("description")
                        .filter(|v| !v.is_null())
                        .map(|v| string_of(Some(v)))
                        .unwrap_or_else(|| "a watch".into());
                    self.watches.insert(id.clone(), description);
                }
                if name == "TaskStop"
                    && let Some(task) = field("task_id").filter(|v| crate::util::truthy(v))
                {
                    self.ended.insert(string_of(Some(task)));
                }
                if name == "CronCreate" {
                    self.cron_calls.insert(
                        id.clone(),
                        input.cloned().unwrap_or(Value::Object(Default::default())),
                    );
                }
                if name == "CronDelete"
                    && let Some(job) = field("id").filter(|v| crate::util::truthy(v))
                {
                    self.crons.shift_remove(&string_of(Some(job)));
                }
                if name == "ScheduleWakeup" && kind == "assistant" {
                    self.wakeup = if field("stop").is_some_and(crate::util::truthy) {
                        None
                    } else {
                        let delay = field("delaySeconds").and_then(Value::as_f64).unwrap_or(0.0);
                        Some(Wakeup {
                            prompt: string_of(field("prompt")),
                            at: at + (delay * 1000.0) as i64,
                            set: at,
                        })
                    };
                }
            }
            if ptype == "tool_result" {
                let use_id = str_of(part.get("tool_use_id"));
                if let Some(call) = self.cron_calls.get(use_id) {
                    let result = part
                        .get("content")
                        .cloned()
                        .unwrap_or(Value::String(String::new()));
                    if let Some(job) = CRON_JOB
                        .captures(&result.to_string())
                        .map(|c| c[1].to_string())
                    {
                        let cron = Cron {
                            prompt: string_of(call.get("prompt")),
                            cron: string_of(call.get("cron")),
                            recurring: call.get("recurring") != Some(&Value::Bool(false)),
                            at,
                        };
                        self.crons.insert(job, cron);
                    }
                }
            }
        }
        let notification = str_of(entry.pointer("/origin/kind")) == "task-notification"
            || text.contains("<task-notification>");
        if notification {
            // A watch's events keep it going; anything else about a task
            // ends it. Each text part on its own, as written, since several
            // notifications can come as separate parts of one message.
            match &content {
                Value::String(s) => ended_in(s, &mut self.ended),
                _ => {
                    for part in parts.iter().filter(|p| str_of(p.get("type")) == "text") {
                        ended_in(str_of(part.get("text")), &mut self.ended);
                    }
                }
            }
            return;
        }
        // Only in the result of the call that starts one: a command's output
        // can quote these words, and would otherwise read as a task of this
        // session's.
        for part in parts
            .iter()
            .filter(|p| str_of(p.get("type")) == "tool_result")
        {
            let use_id = str_of(part.get("tool_use_id"));
            let result_value = part
                .get("content")
                .cloned()
                .unwrap_or(Value::String(String::new()));
            let result = result_value.to_string();
            let command = self.commands.get(use_id).cloned();
            let first_text = match &result_value {
                Value::String(s) => s.as_str(),
                other => other
                    .get(0)
                    .and_then(|p| p.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or(""),
            };
            if let Some(moved) = MOVED.captures(first_text).map(|c| c[1].to_string())
                && !self.started.contains_key(&moved)
            {
                self.started.insert(
                    moved,
                    Task {
                        at,
                        loop_kind: None,
                        monitor: None,
                        until: None,
                    },
                );
            }
            // Only a call made to run in the background: a subagent run in
            // the foreground also names its agentId, once it has finished.
            if let Some(command) = &command {
                for m in STARTED.captures_iter(&result) {
                    let id = m[1].to_string();
                    if !self.started.contains_key(&id) {
                        self.started.insert(
                            id,
                            Task {
                                at,
                                loop_kind: loop_of(command),
                                monitor: None,
                                until: None,
                            },
                        );
                    }
                }
            }
            if let Some(description) = self.watches.get(use_id).cloned()
                && let Some(watch) = MONITOR.captures(&result)
            {
                let minutes: i64 = watch[2].parse().unwrap_or(0);
                self.started.insert(
                    watch[1].to_string(),
                    Task {
                        at,
                        loop_kind: None,
                        monitor: Some(description),
                        until: Some(at + minutes * 60_000),
                    },
                );
            }
        }
    }

    /// What is running, for a session whose process started at `since`
    /// (ms): a task dies with its process, so one started before doesn't
    /// count.
    fn work(&self, since: i64, now: i64) -> Work {
        let running: Vec<&Task> = self
            .started
            .iter()
            .filter(|(id, t)| {
                t.at >= since && !self.ended.contains(*id) && t.until.is_none_or(|u| now <= u)
            })
            .map(|(_, t)| t)
            .collect();
        let monitors = running.iter().filter_map(|t| t.monitor.clone()).collect();
        let running_loop = LOOPS
            .iter()
            .find(|l| running.iter().any(|t| t.loop_kind == Some(l.kind)));
        let from_cron = self
            .crons
            .values()
            .filter(|job| job.at >= since && now < job.at + CRON_LIFE_MS)
            .filter_map(|job| {
                let at = next_cron(&job.cron, if job.recurring { now } else { job.at })?;
                (job.recurring || now < at + WAKEUP_GRACE_MS).then(|| Scheduled {
                    prompt: loop_name(&job.prompt),
                    at,
                })
            })
            .min_by_key(|s| s.at);
        let scheduled = match &self.wakeup {
            Some(w) if w.set >= since && now < w.at + WAKEUP_GRACE_MS => Some(Scheduled {
                prompt: loop_name(&w.prompt),
                at: w.at,
            }),
            _ => from_cron,
        };
        let api_error = self.api_error.clone().filter(|(_, at)| *at >= since);
        let loop_run = running_loop.map(|l| {
            let from = since.max(self.loop_at.get(l.kind).copied().unwrap_or(0));
            let runs = self
                .started
                .values()
                .filter(|t| t.loop_kind == Some(l.kind) && t.at >= from)
                .count();
            LoopRun {
                kind: l.kind,
                round: runs.div_ceil(l.per_round).max(1),
            }
        });
        Work {
            count: running.len(),
            loop_run,
            monitors,
            scheduled,
            api_error,
        }
    }
}

// ── The last turn ───────────────────────────────────────────────────────────

/// A session's last turn: your prompt, when you gave it (ms), what Claude
/// said last, how many tool calls it made, and the one it is stopped on (a
/// permission prompt or a question), in a few words.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Turn {
    pub prompt: String,
    pub at: Option<i64>,
    pub reply: Option<String>,
    pub tools: usize,
    pub pending: Option<String>,
}

/// The prompt as you typed it: a slash command as `/name args`, pasted text
/// unwrapped, a background task's report named as one.
pub fn prompt_text(entry: &Value) -> Option<String> {
    static COMMAND: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<command-name>(.*?)</command-name>").expect("a regex"));
    static ARGS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<command-args>(.*?)</command-args>").expect("a regex"));
    static PASTED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)<pasted_content\b[^>]*>\n?(.*?)\n?</pasted_content\b[^>]*>")
            .expect("a regex")
    });
    let content = entry.pointer("/message/content")?;
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) if !parts.iter().any(|p| str_of(p.get("type")) == "tool_result") => {
            parts
                .iter()
                .filter(|p| str_of(p.get("type")) == "text")
                .map(|p| str_of(p.get("text")))
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => return None,
    };
    if text.trim().is_empty() {
        return None;
    }
    if str_of(entry.pointer("/origin/kind")) == "task-notification" {
        return Some("(a background task finished)".into());
    }
    if let Some(command) = COMMAND.captures(&text) {
        let args = ARGS
            .captures(&text)
            .map(|c| c[1].trim().to_string())
            .unwrap_or_default();
        return Some(format!("{} {args}", command[1].trim()).trim().to_string());
    }
    let trimmed = text.trim();
    if trimmed.starts_with("<local-command-") || trimmed.starts_with("<bash-") {
        return None;
    }
    Some(PASTED.replace_all(&text, "$1").trim().to_string())
}

/// A tool call in a few words, for a turn stopped on one.
pub fn tool_text(part: &Value) -> String {
    let name = str_of(part.get("name"));
    let input = part.get("input");
    if name == "AskUserQuestion" {
        return input
            .and_then(|i| i.pointer("/questions/0/question"))
            .and_then(Value::as_str)
            .unwrap_or("a question")
            .to_string();
    }
    let detail = [
        "command",
        "file_path",
        "path",
        "url",
        "pattern",
        "description",
    ]
    .iter()
    .find_map(|k| input.and_then(|i| i.get(*k)).filter(|v| !v.is_null()))
    .map(|v| string_of(Some(v)))
    .unwrap_or_default();
    if detail.is_empty() {
        name.to_string()
    } else {
        format!("{name}: {}", detail.split('\n').next().unwrap_or(""))
    }
}

/// The last turn in a transcript's text (or its end).
pub fn parse_turn(text: &str) -> Option<Turn> {
    let flag = |e: &Value, k: &str| e.get(k) == Some(&Value::Bool(true));
    let entries: Vec<Value> = text
        .split('\n')
        .filter(|l| l.starts_with('{'))
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| !flag(e, "isSidechain") && !flag(e, "isMeta"))
        .collect();
    let start = entries
        .iter()
        .rposition(|e| str_of(e.get("type")) == "user" && prompt_text(e).is_some())?;
    let mut said: Vec<String> = Vec::new();
    let mut calls: IndexMap<String, (Value, bool)> = IndexMap::new();
    for entry in &entries[start + 1..] {
        let parts = entry
            .pointer("/message/content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        match str_of(entry.get("type")) {
            "assistant" => {
                for part in parts {
                    match str_of(part.get("type")) {
                        "text" => {
                            let t = str_of(part.get("text")).trim();
                            if !t.is_empty() {
                                said.push(t.to_string());
                            }
                        }
                        "tool_use" => {
                            calls.insert(str_of(part.get("id")).to_string(), (part, false));
                        }
                        _ => {}
                    }
                }
            }
            "user" => {
                for part in parts
                    .iter()
                    .filter(|p| str_of(p.get("type")) == "tool_result")
                {
                    if let Some(call) = calls.get_mut(str_of(part.get("tool_use_id"))) {
                        call.1 = true;
                    }
                }
            }
            _ => {}
        }
    }
    let pending = calls
        .values()
        .rev()
        .find(|(_, answered)| !answered)
        .map(|(part, _)| tool_text(part));
    let at = timestamp_ms(&entries[start]);
    Some(Turn {
        prompt: prompt_text(&entries[start]).unwrap_or_default(),
        at: (at != 0).then_some(at),
        reply: said.pop(),
        tools: calls.len(),
        pending,
    })
}

/// How much of a transcript's end is read for its last turn first, and at
/// most: a transcript can be tens of megabytes.
const TAIL: u64 = 512 * 1024;
const MAX_TAIL: u64 = 16 * 1024 * 1024;

// ── Reading, with what was read kept ────────────────────────────────────────

/// Claude Code's files as last read: the session files by their modified
/// time, each transcript as far as it has been read, and each last turn by
/// its transcript's size and time, so a refresh reads only what changed.
#[derive(Default)]
pub struct Reader {
    sessions: HashMap<PathBuf, (Option<SystemTime>, Option<SessionInfo>)>,
    transcripts: HashMap<String, PathBuf>,
    seen: HashMap<PathBuf, Seen>,
    turns: HashMap<PathBuf, (u64, Option<SystemTime>, Option<Turn>)>,
}

impl Reader {
    /// Every session file in `dir`, each read again only when it changed.
    pub fn sessions(&mut self, dir: &Path) -> Vec<SessionInfo> {
        let Ok(entries) = fs::read_dir(dir) else {
            self.sessions.clear();
            return Vec::new();
        };
        let files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        self.sessions.retain(|p, _| files.contains(p));
        let mut out = Vec::new();
        for file in files {
            let mtime = fs::metadata(&file).and_then(|m| m.modified()).ok();
            if mtime.is_none() {
                continue;
            }
            let cached = self.sessions.get(&file).filter(|(t, _)| *t == mtime);
            let info = match cached {
                Some((_, info)) => info.clone(),
                None => {
                    let info = crate::util::read_json(&file)
                        .filter(Value::is_object)
                        .map(|v| SessionInfo::from_json(&v));
                    self.sessions.insert(file.clone(), (mtime, info.clone()));
                    info
                }
            };
            out.extend(info);
        }
        out
    }

    /// A session's transcript: in its project's directory, else found once
    /// among every project's and remembered.
    pub fn transcript(&mut self, claude_dir: &Path, session: &SessionRef) -> Option<PathBuf> {
        let projects = claude_dir.join("projects");
        let file_name = format!("{}.jsonl", session.session_id);
        if let Some(cwd) = &session.cwd {
            let direct = projects.join(project_dir_name(cwd)).join(&file_name);
            if direct.exists() {
                return Some(direct);
            }
        }
        if let Some(found) = self.transcripts.get(&session.session_id)
            && found.exists()
        {
            return Some(found.clone());
        }
        let found = fs::read_dir(&projects)
            .ok()?
            .flatten()
            .map(|d| d.path().join(&file_name))
            .find(|f| f.exists())?;
        self.transcripts
            .insert(session.session_id.clone(), found.clone());
        Some(found)
    }

    /// What a session has running in the background: commands and agents
    /// Claude started in the background since its process started (`since`,
    /// ms) that no task notification has reported ended, its Monitor
    /// watches, the loop they are, a loop waiting on a timer, and a turn an
    /// API error cut short. The transcript is read only as far as it has
    /// grown since the last look; an error reading it is given back, the
    /// work then reading as none.
    pub fn background_work(&mut self, file: &Path, since: i64, now: i64) -> std::io::Result<Work> {
        let meta = fs::metadata(file)?;
        if meta.is_dir() {
            return Err(std::io::ErrorKind::IsADirectory.into());
        }
        let size = meta.len();
        let seen = self.seen.entry(file.to_path_buf()).or_default();
        if size < seen.offset {
            *seen = Seen::default();
        }
        if size > seen.offset {
            let mut f = fs::File::open(file)?;
            f.seek(SeekFrom::Start(seen.offset))?;
            while seen.offset < size {
                let take = (size - seen.offset).min(READ_CHUNK);
                let mut chunk = Vec::with_capacity(take as usize);
                (&mut f).take(take).read_to_end(&mut chunk)?;
                if chunk.is_empty() {
                    break;
                }
                seen.offset += chunk.len() as u64;
                let mut bytes = std::mem::take(&mut seen.rest);
                bytes.extend_from_slice(&chunk);
                let mut lines: Vec<&[u8]> = bytes.split(|&b| b == b'\n').collect();
                let rest = lines.pop().unwrap_or(&[]).to_vec();
                for line in lines {
                    seen.line(&String::from_utf8_lossy(line));
                }
                seen.rest = rest;
            }
        }
        Ok(seen.work(since, now))
    }

    /// A transcript's last turn, read from its end: the last `TAIL` bytes,
    /// then more while no prompt turns up in them. Kept until the file's
    /// size or time changes.
    pub fn last_turn(&mut self, file: &Path) -> Option<Turn> {
        let meta = fs::metadata(file).ok()?;
        let (size, mtime) = (meta.len(), meta.modified().ok());
        if let Some((s, t, turn)) = self.turns.get(file)
            && *s == size
            && *t == mtime
        {
            return turn.clone();
        }
        let mut f = fs::File::open(file).ok()?;
        let mut length = TAIL;
        let turn = loop {
            let take = length.min(size);
            let mut buf = Vec::with_capacity(take as usize);
            f.seek(SeekFrom::Start(size - take)).ok()?;
            (&mut f).take(take).read_to_end(&mut buf).ok()?;
            let turn = parse_turn(&String::from_utf8_lossy(&buf));
            if turn.is_some() || take == size || length >= MAX_TAIL {
                break turn;
            }
            length *= 4;
        };
        self.turns
            .insert(file.to_path_buf(), (size, mtime, turn.clone()));
        turn
    }

    /// Forgets what was read of transcripts no session uses any more.
    pub fn retain(&mut self, files: &HashSet<PathBuf>) {
        self.seen.retain(|f, _| files.contains(f));
        self.turns.retain(|f, _| files.contains(f));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A transcript in a temporary directory of the test's own.
    struct Transcript {
        dir: PathBuf,
        file: PathBuf,
    }

    impl Transcript {
        fn new(name: &str) -> Transcript {
            let dir = std::env::temp_dir().join(format!(
                "sb-claude-{}-{name}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            let file = dir.join("t.jsonl");
            fs::write(&file, "").unwrap();
            Transcript { dir, file }
        }

        fn add(&self, entries: &[Value]) {
            use std::io::Write;
            let mut f = fs::OpenOptions::new()
                .append(true)
                .open(&self.file)
                .unwrap();
            for e in entries {
                writeln!(f, "{e}").unwrap();
            }
        }
    }

    impl Drop for Transcript {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn iso(ms: i64) -> String {
        jiff::Timestamp::from_millisecond(ms).unwrap().to_string()
    }

    fn started(id: &str, ms: i64) -> Vec<Value> {
        vec![
            json!({"type": "assistant", "timestamp": iso(ms), "message": {"content": [{"type": "tool_use", "id": format!("use-{id}"), "name": "Bash", "input": {"command": "npm test", "run_in_background": true}}]}}),
            json!({"type": "user", "timestamp": iso(ms), "message": {"content": [{"type": "tool_result", "tool_use_id": format!("use-{id}"), "content": format!("Command running in background with ID: {id}. Output is being written to: /tmp/x")}]}}),
        ]
    }

    fn ended(id: &str, ms: i64) -> Value {
        json!({"type": "user", "timestamp": iso(ms), "origin": {"kind": "task-notification"}, "message": {"content": format!("<task-notification>\n<task-id>{id}</task-id>\n<status>completed</status>\n</task-notification>")}})
    }

    fn notice(ms: i64, content: &str) -> Value {
        json!({"type": "user", "timestamp": iso(ms), "origin": {"kind": "task-notification"}, "message": {"content": content}})
    }

    #[test]
    fn background_tasks_run_until_a_notification_ends_them() {
        let t = Transcript::new("bg");
        let now = crate::util::now_ms();
        let since = now - 10_000;
        let mut r = Reader::default();
        let count = |r: &mut Reader| r.background_work(&t.file, since, now).unwrap().count;
        t.add(&started("old1", now - 60_000));
        t.add(&started("done1", now - 5_000));
        t.add(&[ended("done1", now)]);
        t.add(&started("run1", now - 4_000));
        t.add(&started("run2", now - 3_000));
        assert_eq!(count(&mut r), 2, "the old one died with the last process");
        t.add(&[ended("run2", now)]);
        assert_eq!(count(&mut r), 1, "read as it grows");
        t.add(&[ended("run1", now)]);
        assert_eq!(count(&mut r), 0);

        t.add(&started("run3", now - 2_000));
        assert_eq!(count(&mut r), 1);
        // A watch's event keeps it going; a status only quoted in a summary
        // or an event isn't the notification's own.
        t.add(&[notice(now, "<task-notification>\n<task-id>run3</task-id>\n<summary>a line</summary>\n<event>NEW: x</event>\n</task-notification>")]);
        assert_eq!(count(&mut r), 1);
        t.add(&[notice(now, "<task-notification>\n<task-id>run3</task-id>\n<summary>Monitor event: \"<status>completed</status>\"</summary>\n<event>checks=pending</event>\n</task-notification>")]);
        assert_eq!(count(&mut r), 1);
        t.add(&[notice(now, "<task-notification>\n<task-id>run3</task-id>\n<summary>Quoted output:\n<status>completed</status>\n</summary>\n<event>checks=pending\n<status>done</status></event>\n</task-notification>")]);
        assert_eq!(count(&mut r), 1);
        // A summary mentioning the opening tag, in a queued command.
        t.add(&[json!({"type": "attachment", "timestamp": iso(now), "attachment": {"type": "queued_command", "commandMode": "task-notification", "prompt": "<task-notification>\n<task-id>run3</task-id>\n<summary>Found <task-notification> in output</summary>\n<event>NEW: x</event>\n</task-notification>"}})]);
        assert_eq!(count(&mut r), 1);
        // One watch's end sent with another's event: each read as its own.
        t.add(&started("m1", now - 1_000));
        t.add(&[notice(now, "<task-notification>\n<task-id>m1</task-id>\n<summary>Monitor source ended.</summary>\n</task-notification>\n<task-notification>\n<task-id>run3</task-id>\n<summary>Monitor event</summary>\n<event>changed</event>\n</task-notification>")]);
        assert_eq!(count(&mut r), 1, "m1 ended, run3 still going");
        // One quoted in another's summary ends nothing.
        t.add(&started("other", now - 1_000));
        t.add(&[notice(now, "<task-notification>\n<task-id>other</task-id>\n<status>completed</status>\n<summary>Quoted: <task-notification><task-id>run3</task-id><status>completed</status></task-notification></summary>\n</task-notification>")]);
        assert_eq!(count(&mut r), 1);
        // Two as separate parts of one message each end theirs.
        t.add(&started("part1", now - 1_000));
        t.add(&started("part2", now - 1_000));
        t.add(&[json!({"type": "user", "timestamp": iso(now), "origin": {"kind": "task-notification"}, "message": {"content": [{"type": "text", "text": "<task-notification>\n<task-id>part1</task-id>\n<status>completed</status>\n</task-notification>"}, {"type": "text", "text": "<task-notification>\n<task-id>part2</task-id>\n<status>completed</status>\n</task-notification>"}]}})]);
        assert_eq!(count(&mut r), 1);
        t.add(&started("two1", now - 1_000));
        t.add(&started("two2", now - 1_000));
        t.add(&[notice(now, "<task-notification>\n<task-id>two1</task-id>\n<status>completed</status>\n</task-notification>\n<task-notification>\n<task-id>two2</task-id>\n<status>completed</status>\n</task-notification>")]);
        assert_eq!(count(&mut r), 1);
        // A message typed mid-turn quoting a notification ends nothing.
        t.add(&[json!({"type": "attachment", "timestamp": iso(now), "attachment": {"type": "queued_command", "commandMode": "prompt", "prompt": "Explain this: <task-notification><task-id>run3</task-id><status>completed</status></task-notification>"}})]);
        assert_eq!(count(&mut r), 1);
        t.add(&[json!({"type": "attachment", "timestamp": iso(now), "attachment": {"type": "queued_command", "commandMode": "task-notification", "origin": {"kind": "task-notification"}, "prompt": "<task-notification>\n<task-id>run3</task-id>\n<status>completed</status>\n<summary>Background command \"Find <event> tags\" completed</summary>\n</task-notification>"}})]);
        assert_eq!(count(&mut r), 0);

        // A subagent run in the foreground is nothing left running; one in
        // the background is.
        t.add(&[
            json!({"type": "assistant", "timestamp": iso(now), "message": {"content": [{"type": "tool_use", "id": "use-fg", "name": "Agent", "input": {"prompt": "look"}}]}}),
            json!({"type": "user", "timestamp": iso(now), "message": {"content": [{"type": "tool_result", "tool_use_id": "use-fg", "content": "Done. agentId: fg1 (internal ID)"}]}}),
        ]);
        assert_eq!(count(&mut r), 0);
        t.add(&[
            json!({"type": "assistant", "timestamp": iso(now), "message": {"content": [{"type": "tool_use", "id": "use-bg", "name": "Agent", "input": {"prompt": "look", "run_in_background": true}}]}}),
            json!({"type": "user", "timestamp": iso(now), "message": {"content": [{"type": "tool_result", "tool_use_id": "use-bg", "content": "Started. agentId: bg1"}]}}),
        ]);
        assert_eq!(count(&mut r), 1);
        t.add(&[ended("bg1", now)]);
        assert_eq!(count(&mut r), 0);
        // A command moved to the background runs on; one only quoting the
        // words starts nothing.
        t.add(&[
            json!({"type": "user", "timestamp": iso(now), "message": {"content": [{"type": "tool_result", "tool_use_id": "use-slow", "content": "Command did not complete within its 120s timeout and was moved to the background (ID: slow1). Output is being written to: /tmp/x"}]}}),
            json!({"type": "user", "timestamp": iso(now), "message": {"content": [{"type": "tool_result", "tool_use_id": "use-grep", "content": "log: Command did not complete within its 120s timeout and was moved to the background (ID: quoted1)"}]}}),
        ]);
        assert_eq!(count(&mut r), 1);
        t.add(&[ended("slow1", now)]);
        assert_eq!(count(&mut r), 0);
    }

    #[test]
    fn a_request_copied_untimed_starts_nothing_again() {
        let t = Transcript::new("blob");
        let now = crate::util::now_ms();
        let call = json!({"type": "tool_use", "id": "c1", "name": "Bash", "input": {"command": "codex-review --base main", "run_in_background": true}});
        let start = json!({"type": "tool_result", "tool_use_id": "c1", "content": "Command running in background with ID: rev1. Output is being written to: /tmp/x"});
        t.add(&[
            json!({"type": "assistant", "timestamp": iso(now - 5_000), "message": {"content": [call]}}),
            json!({"type": "user", "timestamp": iso(now - 4_000), "message": {"content": [start]}}),
            json!({"type": "api-request-blob", "message": {"content": [call, start]}}),
        ]);
        let work = Reader::default()
            .background_work(&t.file, now - 60_000, now)
            .unwrap();
        assert_eq!(work.count, 1);
        assert_eq!(work.loop_run.unwrap().kind, "review");
    }

    #[test]
    fn an_api_error_stands_until_the_conversation_goes_on() {
        let t = Transcript::new("api");
        let now = crate::util::now_ms();
        let at = |ago: i64| iso(now - ago);
        t.add(&[
            json!({"type": "user", "timestamp": at(9_000), "message": {"content": "fix the flaky test"}}),
            json!({"type": "assistant", "timestamp": at(8_000), "message": {"content": [{"type": "text", "text": "Looking."}]}}),
            json!({"type": "assistant", "timestamp": at(7_000), "isApiErrorMessage": true, "error": "server_error", "message": {"content": [{"type": "text", "text": "API Error: Connection lost mid-response. The response above may be incomplete."}]}}),
        ]);
        let mut r = Reader::default();
        let work = r.background_work(&t.file, now - 60_000, now).unwrap();
        let (text, when) = work.api_error.unwrap();
        assert_eq!(
            text,
            "API Error: Connection lost mid-response. The response above may be incomplete."
        );
        assert_eq!(when, now - 7_000);
        assert_eq!(
            r.background_work(&t.file, now - 1_000, now)
                .unwrap()
                .api_error,
            None,
            "not one from before the session"
        );
        t.add(&[
            json!({"type": "progress", "timestamp": at(3_000), "data": {"type": "agent_progress", "message": {"type": "assistant"}}}),
            json!({"type": "assistant", "isSidechain": true, "timestamp": at(2_000), "message": {"content": [{"type": "text", "text": "a subagent"}]}}),
            json!({"type": "user", "timestamp": at(2_600), "message": {"content": "<command-message>cost</command-message><command-name>/cost</command-name>"}}),
            json!({"type": "user", "timestamp": at(2_500), "message": {"content": "<local-command-stdout>Total cost: $1</local-command-stdout>"}}),
            json!({"type": "user", "isMeta": true, "timestamp": at(2_400), "message": {"content": "Caveat"}}),
            json!({"type": "user", "origin": {"kind": "task-notification"}, "timestamp": at(2_300), "message": {"content": "<task-notification>done</task-notification>"}}),
        ]);
        assert!(
            r.background_work(&t.file, now - 60_000, now)
                .unwrap()
                .api_error
                .is_some()
        );
        t.add(&[
            json!({"type": "user", "timestamp": at(1_000), "message": {"content": "try again"}}),
        ]);
        assert_eq!(
            r.background_work(&t.file, now - 60_000, now)
                .unwrap()
                .api_error,
            None
        );
        let nested = Transcript::new("nested");
        nested.add(&[json!({"type": "progress", "timestamp": at(2_000), "data": {"type": "agent_progress", "message": {"type": "assistant", "isApiErrorMessage": true}}})]);
        assert_eq!(
            r.background_work(&nested.file, now - 60_000, now)
                .unwrap()
                .api_error,
            None
        );
    }

    #[test]
    fn a_monitor_watch_runs_through_its_events_until_it_expires() {
        let t = Transcript::new("watch");
        let now = crate::util::now_ms();
        let at = |ago: i64| iso(now - ago);
        t.add(&[
            json!({"type": "assistant", "timestamp": at(5_000), "message": {"content": [{"type": "tool_use", "id": "w1", "name": "Monitor", "input": {"description": "CI on #12", "command": "gh pr checks 12"}}]}}),
            json!({"type": "user", "timestamp": at(5_000), "message": {"content": [{"type": "tool_result", "tool_use_id": "w1", "content": "Monitor started (task m1, expires in 30m unless the source ends first)"}]}}),
            json!({"type": "user", "timestamp": at(1_000), "origin": {"kind": "task-notification"}, "message": {"content": "<task-notification>\n<task-id>m1</task-id>\n<summary>Monitor event: \"CI\"</summary>\n<event>checks=pending</event>\n</task-notification>"}}),
        ]);
        let mut r = Reader::default();
        let work = r.background_work(&t.file, now - 60_000, now).unwrap();
        assert_eq!(
            (work.count, work.monitors.clone()),
            (1, vec!["CI on #12".to_string()])
        );
        assert_eq!(work.loop_run, None);
        // Past its expiry it is over, whatever was said.
        let later = r
            .background_work(&t.file, now - 60_000, now + 31 * 60_000)
            .unwrap();
        assert_eq!(later.count, 0);
        t.add(&[json!({"type": "user", "timestamp": at(0), "origin": {"kind": "task-notification"}, "message": {"content": "<task-notification>\n<task-id>m1</task-id>\n<summary>Monitor expired after 30m with no events delivered.</summary>\n</task-notification>"}})]);
        assert_eq!(
            r.background_work(&t.file, now - 60_000, now).unwrap(),
            Work::default()
        );
    }

    #[test]
    fn a_loop_in_the_background_reads_as_that_loop_with_its_round() {
        let t = Transcript::new("loops");
        let now = crate::util::now_ms();
        let at = |ago: i64| iso(now - ago);
        let run = |use_id: &str, task: &str, command: &str, ago: i64| {
            vec![
                json!({"type": "assistant", "timestamp": at(ago), "message": {"content": [{"type": "tool_use", "id": use_id, "name": "Bash", "input": {"command": command, "run_in_background": true}}]}}),
                json!({"type": "user", "timestamp": at(ago), "message": {"content": [{"type": "tool_result", "tool_use_id": use_id, "content": format!("Command running in background with ID: {task}. Output is being written to: /tmp/x")}]}}),
            ]
        };
        let end = |task: &str| ended(task, now);
        let review = "node .claude/skills/review-loop/scripts/codex-review.cjs --base abc";
        t.add(&[json!({"type": "user", "timestamp": at(9_000), "message": {"content": "<command-name>/review-loop</command-name>\n<command-args></command-args>"}})]);
        t.add(&run("u1", "r1a", review, 8_000));
        t.add(&run("u2", "r1b", review, 8_000));
        t.add(&[end("r1a"), end("r1b")]);
        t.add(&run("u3", "r2a", review, 4_000));
        t.add(&run("u4", "r2b", review, 4_000));
        let mut r = Reader::default();
        let work = |r: &mut Reader| r.background_work(&t.file, now - 60_000, now).unwrap();
        let w = work(&mut r);
        assert_eq!(
            (w.count, w.loop_run),
            (
                2,
                Some(LoopRun {
                    kind: "review",
                    round: 2
                })
            )
        );
        let ci = "checks=$(gh pr checks 12 --json name,bucket)";
        t.add(&[end("r2a"), end("r2b")]);
        t.add(&[json!({"type": "user", "timestamp": at(3_000), "message": {"content": "<command-name>/ci-loop</command-name>"}})]);
        t.add(&run("u5", "ci1", ci, 2_500));
        t.add(&[end("ci1")]);
        t.add(&run("u6", "ci2", ci, 1_000));
        assert_eq!(
            work(&mut r).loop_run,
            Some(LoopRun {
                kind: "ci",
                round: 2
            })
        );
        t.add(&[end("ci2")]);
        t.add(&run(
            "u7",
            "q1",
            "gh api graphql -f query='{ mergeQueueEntry }'; gh pr checks 12",
            500,
        ));
        assert_eq!(
            work(&mut r).loop_run,
            Some(LoopRun {
                kind: "queue",
                round: 1
            })
        );
        t.add(&[end("q1")]);
        t.add(&run("u9", "q2", "node land-lock.js wait", 300));
        assert_eq!(
            work(&mut r).loop_run,
            Some(LoopRun {
                kind: "queue",
                round: 2
            })
        );
        t.add(&[end("q2")]);
        t.add(&run("u8", "x1", "sleep 100", 100));
        let w = work(&mut r);
        assert_eq!((w.count, w.loop_run), (1, None));
    }

    #[test]
    fn a_self_paced_loop_waits_for_its_wakeup_and_a_stop_ends_it() {
        let t = Transcript::new("wakeup");
        let now = crate::util::now_ms();
        let wakeup = |ago: i64, input: Value| json!({"type": "assistant", "timestamp": iso(now - ago), "message": {"content": [{"type": "tool_use", "id": format!("s{ago}"), "name": "ScheduleWakeup", "input": input}]}});
        t.add(&[wakeup(
            60_000,
            json!({"delaySeconds": 900, "prompt": "/loop /oncall"}),
        )]);
        let mut r = Reader::default();
        assert_eq!(
            r.background_work(&t.file, now - 120_000, now)
                .unwrap()
                .scheduled,
            Some(Scheduled {
                prompt: "/oncall".into(),
                at: now - 60_000 + 900_000
            })
        );
        assert_eq!(
            r.background_work(&t.file, now - 30_000, now)
                .unwrap()
                .scheduled,
            None,
            "scheduled by an earlier process"
        );
        t.add(&[wakeup(20_000, json!({"stop": true}))]);
        assert_eq!(
            r.background_work(&t.file, now - 120_000, now).unwrap(),
            Work::default()
        );
        fs::write(&t.file, "").unwrap();
        t.add(&[wakeup(
            30 * 60_000,
            json!({"delaySeconds": 600, "prompt": "/loop /oncall"}),
        )]);
        assert_eq!(
            r.background_work(&t.file, now - 60 * 60_000, now)
                .unwrap()
                .scheduled,
            None,
            "overdue, so the loop ended"
        );
    }

    #[test]
    fn a_cron_job_is_a_loop_until_cron_delete_names_it() {
        let t = Transcript::new("cron");
        let now = crate::util::now_ms();
        let prompt = "Watch the reply to the DM. Read the channel, and act on it.";
        t.add(&[
            json!({"type": "assistant", "timestamp": iso(now - 60_000), "message": {"content": [{"type": "tool_use", "id": "c1", "name": "CronCreate", "input": {"cron": "*/5 * * * *", "recurring": true, "prompt": prompt}}]}}),
            json!({"type": "user", "timestamp": iso(now - 60_000), "message": {"content": [{"type": "tool_result", "tool_use_id": "c1", "content": "Scheduled recurring job a03c0408 (*/5 * * * *). Next run: soon."}]}}),
        ]);
        let mut r = Reader::default();
        let scheduled = r
            .background_work(&t.file, now - 120_000, now)
            .unwrap()
            .scheduled
            .unwrap();
        assert_eq!(scheduled.prompt, "Watch the reply to the DM.");
        assert_eq!(Some(scheduled.at), next_cron("*/5 * * * *", now));
        assert!(scheduled.at > now && scheduled.at <= now + 5 * 60_000);
        t.add(&[json!({"type": "assistant", "timestamp": iso(now - 1_000), "message": {"content": [{"type": "tool_use", "id": "c2", "name": "CronDelete", "input": {"id": "a03c0408"}}]}})]);
        assert_eq!(
            r.background_work(&t.file, now - 120_000, now).unwrap(),
            Work::default()
        );
    }

    #[test]
    fn cron_expressions_match_in_local_time() {
        let local = |y: i16, mo: i8, d: i8, h: i8, mi: i8, s: i8| {
            jiff::civil::datetime(y, mo, d, h, mi, s, 0)
                .to_zoned(jiff::tz::TimeZone::system())
                .unwrap()
                .timestamp()
                .as_millisecond()
        };
        // A Monday, 8:59:30.
        assert_eq!(
            next_cron("0 9 * * 1-5", local(2026, 10, 26, 8, 59, 30)),
            Some(local(2026, 10, 26, 9, 0, 0))
        );
        assert_eq!(
            next_cron("3-59/5 * * * *", local(2026, 10, 26, 16, 9, 0)),
            Some(local(2026, 10, 26, 16, 13, 0))
        );
        assert_eq!(next_cron("nonsense", 0), None);
        assert_eq!(loop_name("/loop 5m /babysit-prs"), "/babysit-prs");
        assert_eq!(
            loop_name("Check the build. Then report."),
            "Check the build."
        );
        assert_eq!(loop_name("one line\nanother"), "one line");
    }

    #[test]
    fn the_last_turn_is_the_last_prompt_what_claude_said_and_the_call_it_waits_on() {
        let lines = [
            json!({"type": "user", "message": {"content": "first prompt"}, "timestamp": "2026-01-01T00:00:00Z"}),
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "old reply"}]}}),
            json!({"type": "user", "message": {"content": "<command-message>shape</command-message><command-name>/shape</command-name><command-args>CHA-1</command-args>"}, "timestamp": "2026-01-01T00:01:00Z"}),
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Reading it."}, {"type": "tool_use", "id": "a", "name": "Read", "input": {"file_path": "/x"}}]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "a", "content": "..."}]}}),
            json!({"type": "assistant", "isSidechain": true, "message": {"content": [{"type": "text", "text": "a subagent"}]}}),
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Ready to run the tests."}, {"type": "tool_use", "id": "b", "name": "Bash", "input": {"command": "npm test\nmore"}}]}}),
        ];
        let text = lines
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let turn = parse_turn(&text).unwrap();
        assert_eq!(turn.prompt, "/shape CHA-1");
        assert_eq!(turn.reply.as_deref(), Some("Ready to run the tests."));
        assert_eq!(turn.tools, 2);
        assert_eq!(turn.pending.as_deref(), Some("Bash: npm test"));
        assert_eq!(
            turn.at,
            Some(
                "2026-01-01T00:01:00Z"
                    .parse::<jiff::Timestamp>()
                    .unwrap()
                    .as_millisecond()
            )
        );
        // Pasted text unwrapped; a report named as one; a question by its words.
        assert_eq!(
            prompt_text(
                &json!({"message": {"content": "<pasted_content id=1>\nhello\n</pasted_content>"}})
            )
            .as_deref(),
            Some("hello")
        );
        assert_eq!(
            prompt_text(
                &json!({"origin": {"kind": "task-notification"}, "message": {"content": "x"}})
            )
            .as_deref(),
            Some("(a background task finished)")
        );
        assert_eq!(
            prompt_text(
                &json!({"message": {"content": "<local-command-stdout>x</local-command-stdout>"}})
            ),
            None
        );
        assert_eq!(
            tool_text(
                &json!({"name": "AskUserQuestion", "input": {"questions": [{"question": "Which layout?"}]}})
            ),
            "Which layout?"
        );
        assert_eq!(tool_text(&json!({"name": "Glob", "input": {}})), "Glob");
    }

    #[test]
    fn a_panes_session_is_its_own_claude_followed_to_a_parked_job() {
        let file = |pid, id: &str, entry: &str, job: &str, parked: &str| SessionInfo {
            pid: Some(pid),
            session_id: id.into(),
            cwd: Some("/work".into()),
            entrypoint: entry.into(),
            job_id: job.into(),
            parked_job_id: parked.into(),
            ..SessionInfo::default()
        };
        let own = file(121, "own", "cli", "", "");
        let headless = file(999, "headless", "sdk-cli", "", "");
        let all = vec![own.clone(), headless.clone()];
        assert_eq!(
            session_of(&all, &[&headless, &own]).unwrap().session_id,
            "own"
        );
        assert_eq!(session_of(&all, &[]), None);
        let parked = file(10, "tab", "cli", "", "j1");
        let job = file(20, "job", "cli", "j1", "");
        let all = vec![parked.clone(), job];
        assert_eq!(session_of(&all, &[&parked]).unwrap().session_id, "job");
        // A name given, not derived, is shown.
        let named = SessionInfo::from_json(
            &json!({"pid": 111, "sessionId": "s1", "cwd": "/work", "name": "Login retry", "nameSource": "user", "startedAt": 1_700_000_000_000_i64}),
        );
        assert_eq!(
            session_of(std::slice::from_ref(&named), &[&named]),
            Some(SessionRef {
                session_id: "s1".into(),
                cwd: Some("/work".into()),
                name: Some("Login retry".into()),
                started_at: Some(1_700_000_000_000),
            })
        );
        assert_eq!(project_dir_name("/Users/me/my.repo"), "-Users-me-my-repo");
    }

    #[test]
    fn the_reader_finds_a_transcript_and_its_last_turn() {
        let t = Transcript::new("reader");
        let claude = t.dir.join(".claude");
        fs::create_dir_all(claude.join("projects/-work")).unwrap();
        fs::create_dir_all(claude.join("sessions")).unwrap();
        fs::write(
            claude.join("sessions/111.json"),
            json!({"pid": 111, "sessionId": "s1", "cwd": "/work"}).to_string(),
        )
        .unwrap();
        fs::write(claude.join("sessions/bad.json"), "not json").unwrap();
        fs::write(
            claude.join("projects/-work/s1.jsonl"),
            format!(
                "{}\n{}\n",
                json!({"type": "user", "message": {"content": "fix it"}}),
                json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Fixed."}]}})
            ),
        )
        .unwrap();
        let mut r = Reader::default();
        let sessions = r.sessions(&claude.join("sessions"));
        assert_eq!(sessions.len(), 1);
        let s = session_of(&sessions, &[&sessions[0]]).unwrap();
        let file = r.transcript(&claude, &s).unwrap();
        assert_eq!(r.last_turn(&file).unwrap().reply.as_deref(), Some("Fixed."));
        // Found among the projects when its directory names another.
        let elsewhere = SessionRef {
            session_id: "s1".into(),
            cwd: Some("/moved".into()),
            ..SessionRef::default()
        };
        assert_eq!(r.transcript(&claude, &elsewhere), Some(file.clone()));
        // A transcript that can't be read is an error, not a panic.
        fs::create_dir_all(claude.join("projects/-work/broken.jsonl")).unwrap();
        assert!(
            r.background_work(&claude.join("projects/-work/broken.jsonl"), 0, 0)
                .is_err()
        );
    }
}
