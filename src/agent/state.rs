//! What each Claude session in the workspace is doing, as tmux/agent-state.sh
//! kept it in pane options (`@agent_state`, `@agent_since`, `@agent_activity`,
//! `@agent_wait`, `@agent_loop`, `@agent_phase`): here
//! a typed record per pane, changed only by the server task, with nothing in
//! it that reads a file or starts a process, so every rule can be tested on
//! its own.
//!
//! A pane is working, waiting on you, or done (finished and not yet looked
//! at), or has no state at all. Its tab shows the most urgent of its panes'
//! states (waiting, then done, then working), as the window's `@agent` did.
//! The same state set again changes nothing, so `since` keeps when it began:
//! a permission prompt says "waiting" twice (PermissionRequest, then the
//! Notification hook about six seconds later), and every tool call says
//! "working", and neither should look like a new wait or a new turn.
//!
//! A wait belongs to whoever asked: the conversation itself, or a subagent
//! (the hook's `agent_id`). A subagent runs its tools while the
//! conversation waits on a question, so only the one waiting ends its wait
//! by going on with its tools; a new prompt, a finished turn or a session
//! starting or ending ends any.
//!
//! A wait belongs to its call too (the hook's `tool_use_id`): the
//! conversation's other calls end while it waits (a backgrounded agent
//! returning, a monitor firing), and a call ending ends the wait only when it
//! is the one waited on, or when either hook names no call, as before waits
//! had calls.
//!
//! In auto mode a permission request is work until it reaches you: the hook
//! fires before auto mode decides, and a call it allows runs with nothing
//! more said until it ends, so the request's words are kept aside and the
//! pane waits only once the Notification of a prompt shown to you comes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::mux::PaneId;

/// What a pane's Claude is doing. Ordered by how much it needs you, so the
/// most urgent of several is their `max`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum State {
    Working,
    Done,
    Waiting,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Working => "working",
            State::Done => "done",
            State::Waiting => "waiting",
        }
    }

    pub fn parse(name: &str) -> Option<State> {
        match name {
            "working" => Some(State::Working),
            "done" => Some(State::Done),
            "waiting" => Some(State::Waiting),
            _ => None,
        }
    }
}

/// The Notification hook's kinds that are Claude waiting on you. The idle
/// reminder comes a minute after a turn ended, which "done" already shows,
/// and the rest (sign-in, quota) only get the banner.
pub const QUESTION_KINDS: &[&str] = &[
    "permission_prompt",
    "elicitation_dialog",
    "elicitation_url_dialog",
    "agent_needs_input",
];

/// The skills that are loops, by the name the sidebar shows them under.
const LOOPS: &[(&str, &str)] = &[
    ("review-loop", "review"),
    ("ci-loop", "ci"),
    ("queue-and-clean", "queue"),
    ("deploy-test", "deploy"),
    ("preview-test", "deploy"),
];

/// Who a wait belongs to: the conversation itself, or a subagent by its
/// `agent_id`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Waiter {
    Conversation,
    Agent(String),
}

/// A permission request auto mode may still allow: its words, who asked
/// and the call it is for, kept until a prompt shown to you makes it a wait.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asked {
    pub wait: String,
    pub by: Waiter,
    pub call: Option<String>,
}

/// One pane's Claude: its state and what goes with it. Serialized only to
/// hand it to a new build with its pane (src/mux/upgrade.rs).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaneAgent {
    pub state: Option<State>,
    /// When it got to its state, in seconds since the epoch; 0 with none.
    pub since: i64,
    /// The tool call in a few words ("Run the test suite", "editing
    /// render.js"), set as each tool starts and kept until the next.
    pub activity: Option<String>,
    /// What it waits on ("permission: Bash git push", "asks: Which
    /// layout?"), while it waits.
    pub wait: Option<String>,
    /// Whose wait it is, while the pane waits; `None` while nothing
    /// waits, or for a wait only a Notification said, whose hook may not
    /// say who asked, which any tool call ends, as before waits had owners.
    pub wait_by: Option<Waiter>,
    /// The call the wait is on (its `tool_use_id`), when the hook that
    /// began the wait named one: another call ending leaves the wait.
    pub wait_call: Option<String>,
    /// When the conversation itself last ran a hook (ms), which tells work
    /// you interrupted from work a prompt came after.
    pub heard: i64,
    /// When the conversation itself last ran a hook that could have
    /// followed an answer (ms: any but another call ending, or one that
    /// marks nothing), which tells a question its transcript shows
    /// unanswered from one asked before.
    pub went_on: i64,
    /// The loop the session runs (review, ci, queue or deploy), from the
    /// moment its skill is asked for until a new prompt or session: the turns
    /// between a loop's rounds are the loop's too.
    pub loop_name: Option<String>,
    /// The step a loop says it is at (queue-and-clean's landing), set by the
    /// loop and ended by a new prompt or session.
    pub phase: Option<String>,
    /// A permission request made in auto mode while the pane works, until
    /// its Notification says it reached you or the call goes on.
    pub asked: Option<Asked>,
}

impl PaneAgent {
    fn is_empty(&self) -> bool {
        self.state.is_none()
            && self.activity.is_none()
            && self.wait.is_none()
            && self.wait_by.is_none()
            && self.wait_call.is_none()
            && self.loop_name.is_none()
            && self.phase.is_none()
            && self.asked.is_none()
    }

    /// The pane's record, as `switchboard list` shows it.
    pub fn json(&self) -> Value {
        json!({
            "state": self.state.map(State::name),
            "since": (self.since > 0).then_some(self.since),
            "activity": self.activity,
            "wait": self.wait,
            "loop": self.loop_name,
            "phase": self.phase,
        })
    }
}

/// What a hook said, read out of its JSON: agent-state.sh's `DETAILS`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hook {
    /// `UserPromptSubmit`, `PreToolUse`, `Stop`, ...
    pub event: String,
    pub tool: String,
    /// The tool call in a few words, for `PreToolUse`.
    pub activity: String,
    /// What a permission request or a question waits on.
    pub wait: String,
    /// The loop a prompt or the Skill tool asks for.
    pub loop_name: String,
    /// What started a session (startup, resume, clear, compact) or a
    /// compaction (manual, auto).
    pub source: String,
    /// A Notification's kind and text.
    pub kind: String,
    pub message: String,
    pub session_id: String,
    /// The subagent the hook fires in (`agent_id`); empty for the
    /// conversation itself.
    pub agent: String,
    /// The tool call the hook fires for (`tool_use_id`); empty when it
    /// names none.
    pub call: String,
    /// The session's permission mode (`default`, `auto`, ...).
    pub permission_mode: String,
}

impl Hook {
    /// Reads a hook's JSON. Anything missing or of the wrong type reads as
    /// empty, so a hook whose JSON can't be read still marks its state.
    pub fn read(event: &str, payload: &Value) -> Hook {
        let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
        let event = if event.is_empty() {
            text(payload, "hook_event_name").unwrap_or_default()
        } else {
            event.to_string()
        };
        let tool = text(payload, "tool_name").unwrap_or_default();
        let input = payload.get("tool_input").cloned().unwrap_or(Value::Null);
        let field = |key: &str| text(&input, key);
        let first_line = |s: String| s.split('\n').next().unwrap_or("").to_string();
        let base = |s: String| s.rsplit('/').next().unwrap_or("").to_string();
        let activity = match tool.as_str() {
            "" => String::new(),
            "Bash" => field("description")
                .or_else(|| field("command").map(first_line))
                .unwrap_or_default(),
            "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => format!(
                "editing {}",
                base(
                    field("file_path")
                        .or_else(|| field("notebook_path"))
                        .unwrap_or_default()
                )
            ),
            "Read" => format!("reading {}", base(field("file_path").unwrap_or_default())),
            "Grep" | "Glob" => format!("searching {}", field("pattern").unwrap_or_default()),
            "WebSearch" | "WebFetch" => "searching the web".into(),
            "Agent" | "Task" => format!("agent: {}", field("description").unwrap_or_default()),
            "Skill" => format!("/{}", field("skill").unwrap_or_default()),
            "TodoWrite" => "planning".into(),
            t if t.starts_with("mcp__") => {
                let rest = t.strip_prefix("mcp__").unwrap_or(t);
                let rest = rest.strip_prefix("claude_ai_").unwrap_or(rest);
                rest.replacen("__", ": ", 1).replace('_', " ")
            }
            t => t.to_string(),
        };
        let wait = if event == "PermissionRequest" {
            let command = field("command")
                .map(|c| format!(" {}", first_line(c)))
                .unwrap_or_default();
            short(&format!("permission: {tool}{command}"), 80)
        } else if tool == "AskUserQuestion" {
            let question = input
                .get("questions")
                .and_then(|q| q.get(0))
                .and_then(|q| q.get("question"))
                .and_then(Value::as_str)
                .unwrap_or("");
            short(&format!("asks: {question}"), 80)
        } else {
            String::new()
        };
        let skill = if event == "UserPromptSubmit" {
            text(payload, "prompt")
                .map(|p| slash_command(&p))
                .unwrap_or_default()
        } else if tool == "Skill" {
            field("skill")
                .map(|s| crate::setup::plugin::bare(&s).to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let loop_name = LOOPS
            .iter()
            .find(|(s, _)| *s == skill)
            .map(|(_, l)| l.to_string())
            .unwrap_or_default();
        let source = text(payload, "source")
            .or_else(|| text(payload, "trigger"))
            .unwrap_or_default();
        Hook {
            event: one_line(&event),
            tool: one_line(&tool),
            activity: one_line(&short(&activity, 60)),
            wait: one_line(&wait),
            loop_name,
            source: one_line(&source),
            kind: text(payload, "notification_type").unwrap_or_default(),
            message: text(payload, "message").unwrap_or_default(),
            session_id: text(payload, "session_id").unwrap_or_default(),
            agent: text(payload, "agent_id").unwrap_or_default(),
            call: text(payload, "tool_use_id").unwrap_or_default(),
            permission_mode: text(payload, "permission_mode").unwrap_or_default(),
        }
    }

    /// Who the hook fires for: the conversation itself or a subagent.
    pub fn waiter(&self) -> Waiter {
        if self.agent.is_empty() {
            Waiter::Conversation
        } else {
            Waiter::Agent(self.agent.clone())
        }
    }

    /// The state a hook marks, from the event Claude Code's hooks run it for
    /// (agent-state.sh's `set` argument in claude/switchboard-settings.json):
    /// `Some(None)` to clear it, `None` for an event that marks nothing.
    fn marks(&self) -> Option<Option<State>> {
        match self.event.as_str() {
            "UserPromptSubmit" | "PreToolUse" | "PostToolUse" | "PostToolUseFailure"
            | "PreCompact" | "PostCompact" => Some(Some(State::Working)),
            "PermissionRequest" => Some(Some(State::Waiting)),
            "Stop" => Some(Some(State::Done)),
            "SessionStart" | "SessionEnd" => Some(None),
            _ => None,
        }
    }

    /// Whether the conversation running this hook could have followed an
    /// answer to its question: any hook that marks a state but a call other
    /// than a question ending, which a backgrounded agent returning is.
    fn follows_an_answer(&self) -> bool {
        let another_call_ended =
            matches!(self.event.as_str(), "PostToolUse" | "PostToolUseFailure")
                && !self.tool.is_empty()
                && self.tool != "AskUserQuestion";
        self.marks().is_some() && !another_call_ended
    }

    /// Whether this is a new prompt or a new or ended session, which ends a
    /// loop and its phase.
    fn begins_anew(&self) -> bool {
        matches!(
            self.event.as_str(),
            "UserPromptSubmit" | "SessionStart" | "SessionEnd"
        )
    }
}

/// The skill a prompt starts with (`/review-loop now` gives `review-loop`),
/// as `^\s*/([a-z-]+)` captured it, Switchboard's plugin's name before it
/// left out (`/switchboard:review-loop`, src/setup/plugin.rs).
fn slash_command(prompt: &str) -> String {
    let Some(rest) = prompt.trim_start().strip_prefix('/') else {
        return String::new();
    };
    crate::setup::plugin::bare(rest)
        .chars()
        .take_while(|c| c.is_ascii_lowercase() || *c == '-')
        .collect()
}

/// Cut to `n` characters, the last an ellipsis.
fn short(text: &str, n: usize) -> String {
    if text.chars().count() > n {
        let mut cut: String = text.chars().take(n - 1).collect();
        cut.push('…');
        cut
    } else {
        text.to_string()
    }
}

/// Tabs and line breaks as spaces, so what is shown stays on one line.
fn one_line(text: &str) -> String {
    text.replace(['\t', '\n', '\u{1f}'], " ")
}

/// Every pane's Claude, by pane.
#[derive(Debug, Default)]
pub struct Agents {
    panes: HashMap<PaneId, PaneAgent>,
    /// The directory each pane's Claude last said it works in (a hook's
    /// `cwd`, which follows its Bash tool's `cd`, where its own process
    /// stays where it started), with the process group in front on the pane
    /// when it said so: once another program is in front, it no longer
    /// counts.
    dirs: HashMap<PaneId, (u32, PathBuf)>,
    /// When every marked pane was last checked for a Claude still running.
    pub(super) swept: Option<Instant>,
}

impl Agents {
    pub fn get(&self, pane: PaneId) -> Option<&PaneAgent> {
        self.panes.get(&pane)
    }

    pub fn state(&self, pane: PaneId) -> Option<State> {
        self.panes.get(&pane).and_then(|a| a.state)
    }

    /// Every pane's record, for handing them to a new build.
    pub fn records(&self) -> Vec<(PaneId, PaneAgent)> {
        let mut all: Vec<(PaneId, PaneAgent)> =
            self.panes.iter().map(|(p, a)| (*p, a.clone())).collect();
        all.sort_by_key(|(p, _)| *p);
        all
    }

    /// A pane's record as a build before this one had it. One from a build
    /// that kept no `went_on` takes it from the last hook heard, as that
    /// build did.
    pub fn put(&mut self, pane: PaneId, mut agent: PaneAgent) {
        if agent.went_on == 0 {
            agent.went_on = agent.heard;
        }
        if !agent.is_empty() {
            self.panes.insert(pane, agent);
        }
    }

    /// The panes that have a state.
    pub fn marked(&self) -> Vec<PaneId> {
        let mut panes: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|(_, a)| a.state.is_some())
            .map(|(p, _)| *p)
            .collect();
        panes.sort();
        panes
    }

    /// A tab's state: the most urgent of its panes'.
    pub fn rollup(&self, panes: &[PaneId]) -> Option<State> {
        panes.iter().filter_map(|p| self.state(*p)).max()
    }

    /// The pane holding a tab's state, the longest there among equals, and
    /// when it got there.
    pub fn leader(&self, panes: &[PaneId]) -> Option<(PaneId, State, i64)> {
        panes
            .iter()
            .filter_map(|p| {
                let a = self.panes.get(p)?;
                Some((*p, a.state?, a.since))
            })
            .max_by(|a, b| a.1.cmp(&b.1).then(b.2.cmp(&a.2)))
    }

    /// Forgets the panes that are gone.
    pub fn retain(&mut self, alive: impl Fn(PaneId) -> bool) {
        self.panes.retain(|p, _| alive(*p));
        self.dirs.retain(|p, _| alive(*p));
    }

    /// A pane's Claude, in front as process group `group`, said it works in
    /// `dir`. Gives whether that changed the directory kept for the pane.
    pub fn moved(&mut self, pane: PaneId, group: u32, dir: &Path) -> bool {
        let entry = (group, dir.to_path_buf());
        self.dirs.insert(pane, entry.clone()) != Some(entry)
    }

    /// A pane's Claude ended: its directory no longer counts. Gives whether
    /// one was kept.
    pub fn forget_dir(&mut self, pane: PaneId) -> bool {
        self.dirs.remove(&pane).is_some()
    }

    /// The directory a pane's Claude last said it works in, while the same
    /// process group is in front on the pane.
    pub fn dir(&self, pane: PaneId, group: u32) -> Option<&Path> {
        self.dirs
            .get(&pane)
            .filter(|(g, _)| *g == group)
            .map(|(_, d)| d.as_path())
    }

    /// Applies a hook to its pane, `front` saying whether the pane's tab is in
    /// front in a session someone is looking at, at `now` (seconds). Gives
    /// whether the pane's state changed.
    pub fn apply(&mut self, pane: PaneId, hook: &Hook, now: i64, front: bool) -> bool {
        let Some(mut state) = hook.marks() else {
            return false;
        };
        // Compaction. A session it restarts (SessionStart from compact) is the
        // same one going on, so it keeps its state and loop. A /compact you
        // typed has Claude at work on it, whatever the tab showed, and nothing
        // follows it, so the tab is idle again after. An automatic one leaves
        // the state as it is and only says what a working pane is doing:
        // Claude Code also writes a compaction's summary ahead of time, in the
        // background (as a turn ends, while idle, or mid-turn), running
        // PreCompact with no PostCompact after it, and a compaction while idle
        // can take that summary up behind a PreCompact of its own. A mark made
        // at PreCompact would stay on an idle tab until its next turn ended.
        if hook.event == "SessionStart" && hook.source == "compact" {
            return false;
        }
        let compaction = matches!(hook.event.as_str(), "PreCompact" | "PostCompact");
        if compaction && hook.source != "manual" && self.state(pane) != Some(State::Working) {
            return false;
        }
        // A tool call by someone other than whoever waits (a background
        // agent's while the conversation asks you something) is no answer.
        // A new question is a wait of its own, which takes the wait over.
        let asks = hook.event == "PreToolUse" && hook.tool == "AskUserQuestion";
        let tool_event = matches!(
            hook.event.as_str(),
            "PreToolUse" | "PostToolUse" | "PostToolUseFailure"
        );
        if tool_event
            && !asks
            && let Some(agent) = self.panes.get(&pane)
            && agent.state == Some(State::Waiting)
            && agent
                .wait_by
                .as_ref()
                .is_some_and(|by| *by != hook.waiter())
        {
            return false;
        }
        // Nor is another call ending while the wait is on one: the
        // conversation's own backgrounded agent returning, a monitor firing.
        let call_ended = matches!(hook.event.as_str(), "PostToolUse" | "PostToolUseFailure");
        if call_ended
            && !hook.call.is_empty()
            && let Some(agent) = self.panes.get(&pane)
            && agent.state == Some(State::Waiting)
            && agent.wait_call.as_ref().is_some_and(|c| *c != hook.call)
        {
            return false;
        }
        let agent = self.panes.entry(pane).or_default();
        // A request auto mode may allow is work until a prompt reaches you.
        // One made while the pane already waits on you leaves that wait.
        if hook.event == "PermissionRequest" && hook.permission_mode == "auto" {
            if agent.state == Some(State::Waiting) {
                return false;
            }
            agent.asked = Some(Asked {
                wait: hook.wait.clone(),
                by: hook.waiter(),
                call: (!hook.call.is_empty()).then(|| hook.call.clone()),
            });
            let changed = set_state(agent, Some(State::Working), now, front);
            details(agent, Some(State::Working), hook);
            return changed;
        }
        if hook.event == "PostCompact" && hook.source == "manual" {
            state = None;
        }
        // A question to you is a wait from the moment it is asked.
        if asks {
            state = Some(State::Waiting);
        }
        let changed = set_state(agent, state, now, front);
        // Whoever asked last owns the wait, as its words are the ones shown.
        if (asks || hook.event == "PermissionRequest") && agent.state == Some(State::Waiting) {
            agent.wait_by = Some(hook.waiter());
            agent.wait_call = (!hook.call.is_empty()).then(|| hook.call.clone());
        }
        details(agent, state, hook);
        if agent.is_empty() {
            self.panes.remove(&pane);
        }
        changed
    }

    /// Marks a pane waiting on you (a Notification's question). Gives whether
    /// its state changed.
    pub fn wait(&mut self, pane: PaneId, now: i64, front: bool) -> bool {
        let agent = self.panes.entry(pane).or_default();
        let already = agent.state == Some(State::Waiting);
        let changed = set_state(agent, Some(State::Waiting), now, front);
        // A request auto mode kept as work reached you after all, unless
        // the pane already waited on something else.
        if let Some(asked) = agent.asked.take()
            && !already
        {
            agent.wait = Some(asked.wait);
            agent.wait_by = Some(asked.by);
            agent.wait_call = asked.call;
        }
        if agent.is_empty() {
            self.panes.remove(&pane);
        }
        changed
    }

    /// A pane waiting on a permission request whose hook named no tool
    /// (`permission: ` alone) takes the words of the call `call` its
    /// transcript shows unanswered, when the wait is the conversation's own
    /// (a subagent's calls are in a transcript of their own) and on that
    /// call, or on none named while `only` says it is the one call
    /// unanswered. Gives whether its wait changed.
    pub fn name_wait(&mut self, pane: PaneId, call: &str, only: bool, words: &str) -> bool {
        let Some(agent) = self.panes.get_mut(&pane) else {
            return false;
        };
        let unnamed = agent.state == Some(State::Waiting)
            && agent.wait.as_deref().map(str::trim_end) == Some("permission:")
            && agent.wait_by == Some(Waiter::Conversation)
            && agent.wait_call.as_ref().map_or(only, |c| c == call);
        if !unnamed || words.trim().is_empty() {
            return false;
        }
        agent.wait = Some(one_line(&short(&format!("permission: {words}"), 80)));
        true
    }

    /// The conversation itself ran `hook` at `at` (ms), kept while the
    /// pane has a record.
    pub fn heard(&mut self, pane: PaneId, hook: &Hook, at: i64) {
        if let Some(agent) = self.panes.get_mut(&pane) {
            agent.heard = agent.heard.max(at);
            if hook.follows_an_answer() {
                agent.went_on = agent.went_on.max(at);
            }
        }
    }

    /// A question the pane's transcript shows unanswered, asked at `at`
    /// (ms) by the call `call`: a wait on that call, unless the pane waits
    /// already or the conversation has since run a hook that could have
    /// followed its answer, which says the transcript is behind (the answer
    /// not written yet) or the hook marked it itself; another call ending
    /// since (a backgrounded agent returning) says neither. For a question
    /// whose hook never came. Gives whether its state changed.
    pub fn asked(
        &mut self,
        pane: PaneId,
        at: i64,
        call: &str,
        question: &str,
        now: i64,
        front: bool,
    ) -> bool {
        let agent = self.panes.entry(pane).or_default();
        if agent.state == Some(State::Waiting) || agent.went_on >= at {
            if agent.is_empty() {
                self.panes.remove(&pane);
            }
            return false;
        }
        let changed = set_state(agent, Some(State::Waiting), now, front);
        agent.wait_by = Some(Waiter::Conversation);
        agent.wait_call = (!call.is_empty()).then(|| call.to_string());
        agent.wait = Some(one_line(&short(&format!("asks: {question}"), 80)));
        changed
    }

    /// A tab's panes, in front for a second: a finished one has been seen.
    /// Gives whether a state changed.
    pub fn seen(&mut self, panes: &[PaneId]) -> bool {
        let mut changed = false;
        for p in panes {
            let Some(agent) = self.panes.get_mut(p) else {
                continue;
            };
            if agent.state == Some(State::Done) {
                agent.state = None;
                agent.since = 0;
                changed = true;
            }
            if agent.is_empty() {
                self.panes.remove(p);
            }
        }
        changed
    }

    /// Claude Code said the pane's session went idle at `at_ms` (ms), its
    /// session file's status: a turn can end with no hook, as a /compact you
    /// typed that failed or that you cancelled does (no PostCompact), so a
    /// pane still working from before then, with no hook from the
    /// conversation since, goes idle. What it was doing and its loop are
    /// kept, as for an interruption. A wait is left to its own hooks and the
    /// transcript (`interrupted`): Claude Code may not yet say it waits when
    /// the question's hook comes. Gives whether it changed.
    pub fn went_idle(&mut self, pane: PaneId, at_ms: i64) -> bool {
        let Some(agent) = self.panes.get_mut(&pane) else {
            return false;
        };
        if agent.state != Some(State::Working)
            || agent.since > at_ms.div_euclid(1000)
            || agent.heard > at_ms
        {
            return false;
        }
        agent.state = None;
        agent.since = 0;
        agent.asked = None;
        if agent.is_empty() {
            self.panes.remove(&pane);
        }
        true
    }

    /// A turn you interrupted at `at_ms` (ms), which runs no hook, so the
    /// pane would stay as it was until the next prompt (agent-state.sh's
    /// `interrupted`, run by the sidebar daemon from the transcript): a
    /// permission request denied, or Esc pressed on it, leaves it waiting,
    /// and Esc pressed while it works (a command auto mode let run) leaves
    /// it working. It goes idle unless it has moved on since: it isn't
    /// waiting or working, its state began after (a new request or prompt),
    /// or, working, the conversation has run a hook since (a prompt typed
    /// after it, which keeps the state's old `since`). What it was doing and
    /// its loop are kept. Gives whether it changed.
    pub fn interrupted(&mut self, pane: PaneId, at_ms: i64) -> bool {
        let Some(agent) = self.panes.get_mut(&pane) else {
            return false;
        };
        let began_by_then = agent.since <= at_ms.div_euclid(1000);
        let left_as_it_was = match agent.state {
            Some(State::Waiting) => began_by_then,
            Some(State::Working) => began_by_then && agent.heard <= at_ms,
            _ => false,
        };
        if !left_as_it_was {
            return false;
        }
        agent.state = None;
        agent.since = 0;
        agent.wait = None;
        agent.wait_by = None;
        agent.wait_call = None;
        agent.asked = None;
        if agent.is_empty() {
            self.panes.remove(&pane);
        }
        true
    }

    /// Drops the state of a pane no longer running Claude, so a session that
    /// died without its SessionEnd hook leaves no mark behind. What it was
    /// doing is kept, as agent-state.sh kept it.
    pub fn drop_state(&mut self, pane: PaneId) -> bool {
        let Some(agent) = self.panes.get_mut(&pane) else {
            return false;
        };
        let changed = agent.state.take().is_some();
        agent.since = 0;
        agent.wait_by = None;
        agent.wait_call = None;
        agent.asked = None;
        if agent.is_empty() {
            self.panes.remove(&pane);
        }
        changed
    }

    /// One of a pane's `@agent_` options, by its tmux name, as `show` reads
    /// it; `None` for a name that isn't one of them.
    pub fn option(&self, pane: PaneId, name: &str) -> Option<Option<String>> {
        let a = self.panes.get(&pane);
        Some(match name {
            "@agent_state" => a.and_then(|a| a.state).map(|s| s.name().to_string()),
            "@agent_since" => a.filter(|a| a.since > 0).map(|a| a.since.to_string()),
            "@agent_activity" => a.and_then(|a| a.activity.clone()),
            "@agent_wait" => a.and_then(|a| a.wait.clone()),
            "@agent_loop" => a.and_then(|a| a.loop_name.clone()),
            "@agent_phase" => a.and_then(|a| a.phase.clone()),
            _ => return None,
        })
    }

    /// Sets one of a pane's `@agent_` options (`None` unsets it), as `set`
    /// does, for scripts that mark a pane themselves (land-lock's phase).
    /// `None` for a name that isn't one of them.
    pub fn set_option(
        &mut self,
        pane: PaneId,
        name: &str,
        value: Option<&str>,
        now: i64,
    ) -> Option<Result<(), String>> {
        // Only the names `option` reads; any other is a plain pane option.
        self.option(pane, name)?;
        let agent = self.panes.entry(pane).or_default();
        let text = value.map(str::to_string);
        let result = match name {
            "@agent_state" => match value {
                None => {
                    agent.state = None;
                    agent.since = 0;
                    Ok(())
                }
                Some(v) => match State::parse(v) {
                    Some(s) => {
                        if agent.state != Some(s) {
                            agent.state = Some(s);
                            agent.since = now;
                        }
                        Ok(())
                    }
                    None => Err(format!("@agent_state is working, waiting or done, not {v}")),
                },
            },
            "@agent_since" => match value {
                None => {
                    agent.since = 0;
                    Ok(())
                }
                Some(v) => v
                    .trim()
                    .parse()
                    .map(|s| agent.since = s)
                    .map_err(|_| format!("@agent_since is seconds since the epoch, not {v}")),
            },
            "@agent_activity" => {
                agent.activity = text;
                Ok(())
            }
            "@agent_wait" => {
                agent.wait = text;
                Ok(())
            }
            "@agent_loop" => {
                agent.loop_name = text;
                Ok(())
            }
            _ => {
                agent.phase = text;
                Ok(())
            }
        };
        if agent.state != Some(State::Waiting) {
            agent.wait_by = None;
            agent.wait_call = None;
        }
        if agent.is_empty() {
            self.panes.remove(&pane);
        }
        Some(result)
    }
}

/// Puts a pane in a state (`None` to clear it): the same state again changes
/// nothing, and finishing in front of you is not news, so the tab you are
/// looking at in a session someone is attached to never gets "done". Gives
/// whether it changed.
fn set_state(agent: &mut PaneAgent, state: Option<State>, now: i64, front: bool) -> bool {
    if agent.state == state {
        return false;
    }
    let state = if state == Some(State::Done) && front {
        None
    } else {
        state
    };
    let changed = agent.state != state;
    agent.state = state;
    agent.since = if state.is_some() { now } else { 0 };
    changed
}

/// Keeps what a pane is doing in step with its state: a new prompt or a
/// finished turn clears both activity and wait, work clears the wait and
/// records the tool call, and a wait records what it is for. `state` is the
/// one the hook asked for, before "done" in front of you became nothing,
/// since either way the turn is over.
fn details(agent: &mut PaneAgent, state: Option<State>, hook: &Hook) {
    if !hook.loop_name.is_empty() {
        agent.loop_name = Some(hook.loop_name.clone());
    } else if hook.begins_anew() {
        agent.loop_name = None;
    }
    if hook.begins_anew() {
        // A loop's step is set again by the loop as it goes on.
        agent.phase = None;
    }
    // A request kept aside ends with its call going on, or the turn.
    let tool_event = matches!(
        hook.event.as_str(),
        "PreToolUse" | "PostToolUse" | "PostToolUseFailure"
    );
    let ends_asked = match &agent.asked {
        Some(asked) if tool_event => {
            asked.by == hook.waiter()
                && (hook.call.is_empty() || asked.call.as_ref().is_none_or(|c| *c == hook.call))
        }
        Some(_) => matches!(
            hook.event.as_str(),
            "UserPromptSubmit" | "Stop" | "SessionStart" | "SessionEnd"
        ),
        None => false,
    };
    if ends_asked {
        agent.asked = None;
    }
    match state {
        None | Some(State::Done) => {
            agent.activity = None;
            agent.wait = None;
            agent.wait_by = None;
            agent.wait_call = None;
        }
        Some(State::Working) => {
            agent.wait = None;
            agent.wait_by = None;
            agent.wait_call = None;
            match hook.event.as_str() {
                "UserPromptSubmit" | "PostCompact" => agent.activity = None,
                "PreCompact" => agent.activity = Some("compacting context".into()),
                "PreToolUse" if !hook.activity.is_empty() => {
                    agent.activity = Some(hook.activity.clone());
                }
                _ => {}
            }
        }
        Some(State::Waiting) => {
            if !hook.wait.is_empty() {
                agent.wait = Some(hook.wait.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: PaneId = PaneId(1);

    #[test]
    fn a_reported_directory_counts_only_while_its_program_is_in_front() {
        let mut agents = Agents::default();
        assert!(agents.moved(P, 40, Path::new("/repo/wt")));
        assert!(!agents.moved(P, 40, Path::new("/repo/wt")));
        assert_eq!(agents.dir(P, 40), Some(Path::new("/repo/wt")));
        assert_eq!(agents.dir(P, 41), None);
        assert!(agents.moved(P, 40, Path::new("/repo")));
        assert_eq!(agents.dir(P, 40), Some(Path::new("/repo")));
        assert!(agents.forget_dir(P));
        assert_eq!(agents.dir(P, 40), None);
        assert!(!agents.forget_dir(P));
        agents.moved(P, 40, Path::new("/repo"));
        agents.retain(|_| false);
        assert_eq!(agents.dir(P, 40), None);
    }

    fn hook(event: &str, payload: Value) -> Hook {
        Hook::read(event, &payload)
    }

    fn on(agents: &mut Agents, event: &str, payload: Value, now: i64) -> bool {
        agents.apply(P, &hook(event, payload), now, false)
    }

    #[test]
    fn each_hook_marks_its_state() {
        let mut a = Agents::default();
        for (event, want) in [
            ("UserPromptSubmit", Some(State::Working)),
            ("PermissionRequest", Some(State::Waiting)),
            ("PostToolUse", Some(State::Working)),
            ("PostToolUseFailure", Some(State::Working)),
            ("Stop", Some(State::Done)),
            ("PreToolUse", Some(State::Working)),
            ("SessionEnd", None),
            ("PostToolUse", Some(State::Working)),
            ("SessionStart", None),
        ] {
            on(&mut a, event, json!({}), 100);
            assert_eq!(a.state(P), want, "after {event}");
        }
        // An event no hook marks with changes nothing.
        on(&mut a, "UserPromptSubmit", json!({}), 100);
        assert!(!on(&mut a, "SubagentStop", json!({}), 200));
        assert_eq!(a.state(P), Some(State::Working));
    }

    #[test]
    fn the_same_state_again_keeps_when_it_began() {
        let mut a = Agents::default();
        assert!(on(&mut a, "PermissionRequest", json!({}), 100));
        assert!(!a.wait(P, 106, false));
        assert_eq!(a.get(P).unwrap().since, 100);
        assert!(on(&mut a, "PostToolUse", json!({}), 110));
        assert!(!on(&mut a, "PreToolUse", json!({}), 120));
        assert_eq!(a.get(P).unwrap().since, 110);
    }

    #[test]
    fn only_whoever_waits_ends_the_wait_by_going_on() {
        let mut a = Agents::default();
        let ask = json!({"tool_name": "AskUserQuestion", "tool_input": {"questions": [{"question": "Which?"}]}});
        on(&mut a, "UserPromptSubmit", json!({}), 100);
        assert!(on(&mut a, "PreToolUse", ask, 110));
        // A background agent's tools while the conversation asks you.
        let sub = json!({"agent_id": "a1", "tool_name": "Bash", "tool_input": {"command": "ls"}});
        assert!(!on(&mut a, "PreToolUse", sub.clone(), 111));
        assert!(!on(&mut a, "PostToolUse", sub.clone(), 112));
        assert_eq!(a.state(P), Some(State::Waiting));
        assert_eq!(a.get(P).unwrap().wait.as_deref(), Some("asks: Which?"));
        // The answer: the question's own call ends.
        assert!(on(
            &mut a,
            "PostToolUse",
            json!({"tool_name": "AskUserQuestion"}),
            130
        ));
        assert_eq!(a.state(P), Some(State::Working));
        // A subagent's permission request is its wait; the conversation's
        // own calls don't end it, the subagent going on does.
        let request =
            json!({"agent_id": "a1", "tool_name": "Bash", "tool_input": {"command": "git push"}});
        assert!(on(&mut a, "PermissionRequest", request, 140));
        assert!(!on(
            &mut a,
            "PostToolUse",
            json!({"tool_name": "Read"}),
            141
        ));
        assert!(on(&mut a, "PostToolUse", sub.clone(), 150));
        // A question asked while a subagent waits takes the wait over.
        on(&mut a, "PermissionRequest", json!({"agent_id": "a1"}), 154);
        let ask = json!({"tool_name": "AskUserQuestion", "tool_input": {"questions": [{"question": "And?"}]}});
        on(&mut a, "PreToolUse", ask, 155);
        assert!(!on(&mut a, "PostToolUse", sub, 156));
        assert_eq!(a.get(P).unwrap().wait.as_deref(), Some("asks: And?"));
        on(&mut a, "PostToolUse", json!({}), 157);
        // Two subagents asking: the later one's request is the one shown,
        // and its going on ends the wait.
        on(&mut a, "PermissionRequest", json!({"agent_id": "a1"}), 158);
        on(&mut a, "PermissionRequest", json!({"agent_id": "a2"}), 159);
        assert!(!on(&mut a, "PostToolUse", json!({"agent_id": "a1"}), 160));
        assert!(on(&mut a, "PostToolUse", json!({"agent_id": "a2"}), 161));
        // An automatic compaction leaves a subagent's wait with the subagent.
        on(&mut a, "PermissionRequest", json!({"agent_id": "a1"}), 162);
        assert!(!on(&mut a, "PreCompact", json!({"trigger": "auto"}), 163));
        assert!(!on(&mut a, "PostCompact", json!({"trigger": "auto"}), 164));
        assert_eq!(a.state(P), Some(State::Waiting));
        assert!(!on(&mut a, "PostToolUse", json!({}), 165));
        assert!(on(&mut a, "PostToolUse", json!({"agent_id": "a1"}), 166));
        assert_eq!(a.get(P).unwrap().wait_by, None);
        // A wait only a Notification said may be anyone's: any call ends it.
        on(&mut a, "PostToolUse", json!({}), 151);
        assert!(a.wait(P, 152, false));
        assert!(on(&mut a, "PostToolUse", json!({"agent_id": "a1"}), 153));
        // A finished turn ends any wait.
        on(&mut a, "PermissionRequest", json!({"agent_id": "a1"}), 160);
        on(&mut a, "Stop", json!({}), 170);
        assert_eq!(a.state(P), Some(State::Done));
    }

    /// A hook of the conversation's as the server takes it: applied, then
    /// heard, at `now` seconds.
    fn runs(agents: &mut Agents, event: &str, payload: Value, now: i64) -> bool {
        let hook = hook(event, payload);
        let changed = agents.apply(P, &hook, now, false);
        agents.heard(P, &hook, now * 1000);
        changed
    }

    fn ask(call: &str) -> Value {
        json!({"tool_name": "AskUserQuestion", "tool_use_id": call, "tool_input": {"questions": [{"question": "Which?"}]}})
    }

    #[test]
    fn a_question_stays_a_wait_while_another_call_of_the_conversation_ends() {
        let mut a = Agents::default();
        let agent = json!({"tool_name": "Agent", "tool_use_id": "a1", "tool_input": {"description": "Security review"}});
        runs(
            &mut a,
            "UserPromptSubmit",
            json!({"prompt": "/queue-and-clean"}),
            100,
        );
        runs(&mut a, "PreToolUse", agent.clone(), 101);
        assert!(runs(&mut a, "PreToolUse", ask("q1"), 110));
        // The backgrounded agent returns: the conversation's own hook, for
        // another call than the question.
        assert!(!runs(&mut a, "PostToolUse", agent.clone(), 120));
        assert!(!runs(&mut a, "PostToolUseFailure", agent, 121));
        let pane = a.get(P).unwrap();
        assert_eq!((pane.state, pane.since), (Some(State::Waiting), 110));
        assert_eq!(pane.wait.as_deref(), Some("asks: Which?"));
        assert_eq!(pane.loop_name.as_deref(), Some("queue"));
        // A notification marks nothing new, and ends nothing.
        assert!(!a.wait(P, 122, false));
        // The question's own call ending is the answer.
        assert!(runs(&mut a, "PostToolUse", ask("q1"), 130));
        let pane = a.get(P).unwrap();
        assert_eq!(pane.state, Some(State::Working));
        assert_eq!(
            (pane.wait.as_deref(), pane.wait_call.as_deref()),
            (None, None)
        );

        // Going on with another tool, a new prompt and a finished turn each
        // end a wait on a call, as does a call ending that names none.
        for (event, payload, want) in [
            (
                "PreToolUse",
                json!({"tool_name": "Read", "tool_use_id": "r1"}),
                State::Working,
            ),
            ("UserPromptSubmit", json!({}), State::Working),
            ("Stop", json!({}), State::Done),
            ("PostToolUse", json!({"tool_name": "Agent"}), State::Working),
        ] {
            runs(&mut a, "PreToolUse", ask("q2"), 140);
            assert_eq!(a.state(P), Some(State::Waiting));
            runs(&mut a, event, payload, 150);
            assert_eq!(a.state(P), Some(want), "after {event}");
            assert_eq!(a.get(P).unwrap().wait_call, None, "after {event}");
        }

        // A permission request's wait is its call's too, when it names one.
        let push = json!({"tool_name": "Bash", "tool_use_id": "b1", "tool_input": {"command": "git push"}});
        runs(&mut a, "PermissionRequest", push.clone(), 160);
        assert!(!runs(
            &mut a,
            "PostToolUse",
            json!({"tool_name": "Monitor", "tool_use_id": "m1"}),
            161
        ));
        assert!(runs(&mut a, "PostToolUse", push, 162));
        // A new question takes the wait over, with its call.
        runs(&mut a, "PreToolUse", ask("q3"), 170);
        runs(&mut a, "PreToolUse", ask("q4"), 171);
        assert!(!runs(&mut a, "PostToolUse", ask("q3"), 172));
        assert!(runs(&mut a, "PostToolUse", ask("q4"), 173));
    }

    #[test]
    fn a_question_the_transcript_shows_waits_unless_a_hook_came_since() {
        let mut a = Agents::default();
        runs(&mut a, "UserPromptSubmit", json!({}), 100);
        // Asked after the conversation's last hook: its hook never came.
        assert!(a.asked(P, 105_000, "q1", "Which layout?", 106, false));
        assert_eq!(a.state(P), Some(State::Waiting));
        assert_eq!(
            a.get(P).unwrap().wait.as_deref(),
            Some("asks: Which layout?")
        );
        assert!(!a.asked(P, 105_000, "q1", "Which layout?", 107, false));
        // A backgrounded agent returning leaves the wait, which is its
        // call's.
        let agent = json!({"tool_name": "Agent", "tool_use_id": "a1"});
        assert!(!runs(&mut a, "PostToolUse", agent.clone(), 110));
        assert_eq!(a.get(P).unwrap().since, 106);
        // Answered: the hook comes before the transcript has the answer.
        runs(&mut a, "PostToolUse", ask("q1"), 120);
        assert!(!a.asked(P, 105_000, "q1", "Which layout?", 121, false));
        assert_eq!(a.state(P), Some(State::Working));

        // A question asked before the agent returned, whose hook never came
        // and which no read of the transcript had shown yet: the agent's
        // return doesn't hide it.
        runs(&mut a, "PostToolUse", agent, 135);
        assert!(a.asked(P, 130_000, "q2", "And now?", 136, false));
        // One asked before a hook that followed an answer was answered.
        runs(&mut a, "PreToolUse", json!({"tool_name": "Read"}), 140);
        assert!(!a.asked(P, 130_000, "q2", "And now?", 141, false));
        // A hook that marks nothing (a subagent stopping) says nothing
        // about an answer either.
        runs(&mut a, "SubagentStop", json!({}), 150);
        assert!(a.asked(P, 145_000, "q3", "Last?", 151, false));

        // An idle pane with no record waits on a question too.
        let mut b = Agents::default();
        assert!(b.asked(P, 5, "", "Go?", 1, false));
        // A record a build before this one kept takes what it heard as
        // when it went on.
        let mut c = Agents::default();
        c.put(
            P,
            PaneAgent {
                state: Some(State::Working),
                heard: 200_000,
                ..PaneAgent::default()
            },
        );
        assert!(!c.asked(P, 150_000, "q", "Old?", 201, false));
    }

    #[test]
    fn a_request_auto_mode_may_allow_is_work_until_it_reaches_you() {
        let mut a = Agents::default();
        let test = json!({"tool_name": "Bash", "tool_use_id": "b1", "permission_mode": "auto",
            "tool_input": {"command": "cargo test", "description": "Run the tests"}});
        on(&mut a, "PreToolUse", test.clone(), 100);
        assert!(!on(&mut a, "PermissionRequest", test.clone(), 101));
        let agent = a.get(P).unwrap();
        assert_eq!(agent.state, Some(State::Working));
        assert_eq!(agent.activity.as_deref(), Some("Run the tests"));
        assert_eq!(agent.wait, None);
        // The prompt reached you: its Notification makes it a wait.
        assert!(a.wait(P, 107, false));
        let agent = a.get(P).unwrap();
        assert_eq!(agent.wait.as_deref(), Some("permission: Bash cargo test"));
        assert_eq!(agent.wait_by, Some(Waiter::Conversation));
        assert_eq!(agent.wait_call.as_deref(), Some("b1"));
        assert_eq!(agent.asked, None);
        on(&mut a, "PostToolUse", test.clone(), 120);
        assert_eq!(a.state(P), Some(State::Working));

        // Allowed: the call ends before any Notification, and what was asked
        // goes with it.
        on(&mut a, "PreToolUse", test.clone(), 130);
        on(&mut a, "PermissionRequest", test.clone(), 131);
        assert!(!on(&mut a, "PostToolUse", test.clone(), 140));
        let agent = a.get(P).unwrap();
        assert_eq!(agent.state, Some(State::Working));
        assert_eq!(agent.asked, None);
        // A compaction in the background keeps what was asked.
        on(&mut a, "PermissionRequest", test.clone(), 141);
        on(&mut a, "PreCompact", json!({"trigger": "auto"}), 142);
        assert!(a.get(P).unwrap().asked.is_some());
        on(&mut a, "PostToolUse", test.clone(), 143);
        // A Notification after that is a wait as before, with no words.
        assert!(a.wait(P, 150, false));
        assert_eq!(a.get(P).unwrap().wait, None);

        // One made while a question waits leaves the question's wait.
        on(&mut a, "PreToolUse", ask("q1"), 160);
        on(
            &mut a,
            "PermissionRequest",
            json!({"agent_id": "s1", "tool_name": "Bash",
            "permission_mode": "auto", "tool_input": {"command": "ls"}}),
            161,
        );
        assert_eq!(a.get(P).unwrap().asked, None);
        a.wait(P, 167, false);
        assert_eq!(a.get(P).unwrap().wait.as_deref(), Some("asks: Which?"));
        assert_eq!(a.get(P).unwrap().wait_by, Some(Waiter::Conversation));

        // Another mode waits at once.
        let mut b = Agents::default();
        let push = json!({"tool_name": "Bash", "permission_mode": "default",
            "tool_input": {"command": "git push"}});
        assert!(on(&mut b, "PermissionRequest", push, 100));
        assert_eq!(b.state(P), Some(State::Waiting));
        assert_eq!(
            b.get(P).unwrap().wait.as_deref(),
            Some("permission: Bash git push")
        );
    }

    #[test]
    fn a_permission_wait_with_no_tool_takes_the_transcripts_words() {
        let mut a = Agents::default();
        assert!(!a.name_wait(P, "", true, "Bash: ls"), "nothing waits");
        // A subagent's, or one on another call, is left alone.
        on(&mut a, "PermissionRequest", json!({"agent_id": "s1"}), 90);
        assert!(!a.name_wait(P, "", true, "Bash: ls"));
        on(
            &mut a,
            "PermissionRequest",
            json!({"tool_use_id": "t1"}),
            95,
        );
        assert!(!a.name_wait(P, "t2", true, "Bash: ls"));
        on(&mut a, "PermissionRequest", json!({}), 100);
        assert_eq!(a.get(P).unwrap().wait.as_deref(), Some("permission: "));
        assert!(
            !a.name_wait(P, "", false, "Bash: ls"),
            "one of several unanswered calls"
        );
        assert!(a.name_wait(P, "", true, "Bash: sleep 30"));
        assert_eq!(
            a.get(P).unwrap().wait.as_deref(),
            Some("permission: Bash: sleep 30")
        );
        assert!(!a.name_wait(P, "", true, "Read: x"), "a named wait is kept");
        on(&mut a, "PreToolUse", ask("q"), 110);
        assert!(!a.name_wait(P, "", true, "Read: x"));
        assert_eq!(a.get(P).unwrap().wait.as_deref(), Some("asks: Which?"));
    }

    #[test]
    fn finishing_in_front_of_you_is_not_news() {
        let mut a = Agents::default();
        on(&mut a, "UserPromptSubmit", json!({}), 100);
        assert!(a.apply(P, &hook("Stop", json!({})), 110, true));
        assert_eq!(a.state(P), None);
        assert!(a.get(P).is_none(), "nothing is left of a finished turn");
        // Waiting is news wherever you look.
        a.apply(P, &hook("PermissionRequest", json!({})), 120, true);
        assert_eq!(a.state(P), Some(State::Waiting));
    }

    #[test]
    fn a_tab_shows_its_most_urgent_pane() {
        let mut a = Agents::default();
        let (p1, p2, p3) = (PaneId(1), PaneId(2), PaneId(3));
        a.apply(p1, &hook("UserPromptSubmit", json!({})), 100, false);
        assert_eq!(a.rollup(&[p1, p2, p3]), Some(State::Working));
        a.apply(p2, &hook("Stop", json!({})), 100, false);
        assert_eq!(a.rollup(&[p1, p2, p3]), Some(State::Done));
        a.apply(p3, &hook("PermissionRequest", json!({})), 100, false);
        assert_eq!(a.rollup(&[p1, p2, p3]), Some(State::Waiting));
        assert_eq!(a.rollup(&[p1, p2]), Some(State::Done));
        assert_eq!(a.rollup(&[PaneId(9)]), None);
        assert_eq!(a.leader(&[p1, p2, p3]), Some((p3, State::Waiting, 100)));
    }

    #[test]
    fn what_claude_is_doing_follows_its_hooks() {
        let mut a = Agents::default();
        on(
            &mut a,
            "UserPromptSubmit",
            json!({"prompt": "  /review-loop then push"}),
            100,
        );
        assert_eq!(a.get(P).unwrap().loop_name.as_deref(), Some("review"));
        on(
            &mut a,
            "PreToolUse",
            json!({"tool_name": "Bash", "tool_input": {"command": "cargo test\n--all", "description": "Run the\ttests"}}),
            101,
        );
        assert_eq!(a.get(P).unwrap().activity.as_deref(), Some("Run the tests"));
        // PostToolUse keeps the activity until the next tool starts.
        on(&mut a, "PostToolUse", json!({"tool_name": "Bash"}), 102);
        assert_eq!(a.get(P).unwrap().activity.as_deref(), Some("Run the tests"));
        on(
            &mut a,
            "PermissionRequest",
            json!({"tool_name": "Bash", "tool_input": {"command": "git push\norigin"}}),
            103,
        );
        let agent = a.get(P).unwrap();
        assert_eq!(agent.wait.as_deref(), Some("permission: Bash git push"));
        assert_eq!(
            agent.loop_name.as_deref(),
            Some("review"),
            "the loop goes on"
        );
        on(&mut a, "PostToolUse", json!({}), 104);
        assert_eq!(a.get(P).unwrap().wait, None, "work clears the wait");
        on(&mut a, "Stop", json!({}), 105);
        let agent = a.get(P).unwrap();
        assert_eq!((agent.activity.as_ref(), agent.wait.as_ref()), (None, None));
        assert_eq!(agent.loop_name.as_deref(), Some("review"));
        // Any other prompt ends the loop.
        on(&mut a, "UserPromptSubmit", json!({"prompt": "thanks"}), 106);
        assert_eq!(a.get(P).unwrap().loop_name, None);
        // The Skill tool starts one too.
        on(
            &mut a,
            "PreToolUse",
            json!({"tool_name": "Skill", "tool_input": {"skill": "queue-and-clean"}}),
            107,
        );
        let agent = a.get(P).unwrap();
        assert_eq!(agent.loop_name.as_deref(), Some("queue"));
        assert_eq!(agent.activity.as_deref(), Some("/queue-and-clean"));
        // Named with Switchboard's plugin's name, typed or by the tool.
        on(
            &mut a,
            "UserPromptSubmit",
            json!({"prompt": "/switchboard:review-loop all"}),
            108,
        );
        assert_eq!(a.get(P).unwrap().loop_name.as_deref(), Some("review"));
        on(
            &mut a,
            "PreToolUse",
            json!({"tool_name": "Skill", "tool_input": {"skill": "switchboard:ci-loop"}}),
            109,
        );
        assert_eq!(a.get(P).unwrap().loop_name.as_deref(), Some("ci"));
    }

    #[test]
    fn a_question_is_a_wait_from_the_moment_it_is_asked() {
        let mut a = Agents::default();
        on(&mut a, "UserPromptSubmit", json!({}), 100);
        on(
            &mut a,
            "PreToolUse",
            json!({"tool_name": "AskUserQuestion", "tool_input": {"questions": [{"question": "Which layout?"}]}}),
            101,
        );
        let agent = a.get(P).unwrap();
        assert_eq!(agent.state, Some(State::Waiting));
        assert_eq!(agent.wait.as_deref(), Some("asks: Which layout?"));
    }

    #[test]
    fn a_new_prompt_or_session_ends_a_loops_phase() {
        let mut a = Agents::default();
        a.set_option(P, "@agent_phase", Some("landing"), 1);
        a.set_option(P, "@agent_loop", Some("queue"), 1);
        on(&mut a, "PostToolUse", json!({}), 100);
        assert_eq!(a.get(P).unwrap().phase.as_deref(), Some("landing"));
        on(&mut a, "SessionStart", json!({"source": "startup"}), 101);
        assert!(a.get(P).is_none());
    }

    #[test]
    fn tool_calls_in_a_few_words() {
        let activity = |tool: &str, input: Value| {
            hook(
                "PreToolUse",
                json!({"tool_name": tool, "tool_input": input}),
            )
            .activity
        };
        assert_eq!(
            activity("Edit", json!({"file_path": "/a/b/render.js"})),
            "editing render.js"
        );
        assert_eq!(
            activity("NotebookEdit", json!({"notebook_path": "x/n.ipynb"})),
            "editing n.ipynb"
        );
        assert_eq!(
            activity("Read", json!({"file_path": "/x/y.rs"})),
            "reading y.rs"
        );
        assert_eq!(
            activity("Grep", json!({"pattern": "fn main"})),
            "searching fn main"
        );
        assert_eq!(activity("WebFetch", json!({})), "searching the web");
        assert_eq!(
            activity("Task", json!({"description": "Find the bug"})),
            "agent: Find the bug"
        );
        assert_eq!(activity("TodoWrite", json!({})), "planning");
        assert_eq!(
            activity("mcp__claude_ai_Linear__get_issue", json!({})),
            "Linear: get issue"
        );
        assert_eq!(
            activity("mcp__github__list_prs", json!({})),
            "github: list prs"
        );
        assert_eq!(activity("Fly", json!({})), "Fly");
        assert_eq!(activity("Bash", json!({"command": "ls\nrm"})), "ls");
        let long = activity("Bash", json!({"description": "x".repeat(100)}));
        assert_eq!(long.chars().count(), 60);
        assert!(long.ends_with('…'));
        // Input of the wrong shape reads as empty rather than failing.
        assert_eq!(activity("Read", json!("nonsense")), "reading ");
        assert_eq!(hook("PreToolUse", json!([1, 2])).tool, "");
    }

    #[test]
    fn the_event_comes_from_the_json_when_not_given() {
        assert_eq!(
            hook("", json!({"hook_event_name": "Stop"})).marks(),
            Some(Some(State::Done))
        );
    }

    #[test]
    fn a_session_restarted_by_compaction_goes_on() {
        let mut a = Agents::default();
        on(
            &mut a,
            "UserPromptSubmit",
            json!({"prompt": "/ci-loop"}),
            100,
        );
        assert!(!on(
            &mut a,
            "SessionStart",
            json!({"source": "compact"}),
            101
        ));
        let agent = a.get(P).unwrap();
        assert_eq!(agent.state, Some(State::Working));
        assert_eq!(agent.since, 100);
        assert_eq!(agent.loop_name.as_deref(), Some("ci"));
    }

    #[test]
    fn an_automatic_compaction_mid_turn_stays_working() {
        let mut a = Agents::default();
        on(&mut a, "UserPromptSubmit", json!({}), 100);
        on(&mut a, "PreCompact", json!({"trigger": "auto"}), 110);
        assert_eq!(
            a.get(P).unwrap().activity.as_deref(),
            Some("compacting context")
        );
        on(&mut a, "PostCompact", json!({"trigger": "auto"}), 120);
        let agent = a.get(P).unwrap();
        assert_eq!(agent.state, Some(State::Working));
        assert_eq!(agent.since, 100);
        assert_eq!(agent.activity, None);
    }

    #[test]
    fn an_automatic_compaction_while_idle_leaves_the_tab_as_it_was() {
        // A turn ends, Claude Code writes a summary ahead of time (PreCompact
        // alone), and later compacts while idle, taking that summary up
        // behind a PreCompact of its own.
        let mut a = Agents::default();
        on(&mut a, "Stop", json!({}), 100);
        assert!(!on(&mut a, "PreCompact", json!({"trigger": "auto"}), 101));
        assert_eq!(a.state(P), Some(State::Done));
        assert!(!on(&mut a, "PreCompact", json!({"trigger": "auto"}), 3300));
        assert!(!on(
            &mut a,
            "SessionStart",
            json!({"source": "compact"}),
            3390
        ));
        assert!(!on(&mut a, "PostCompact", json!({"trigger": "auto"}), 3390));
        let agent = a.get(P).unwrap();
        assert_eq!(
            (agent.state, agent.since),
            (Some(State::Done), 100),
            "a finished tab keeps its age"
        );
        assert_eq!(agent.activity, None);
        // Seen, it is idle, and an idle tab stays idle.
        assert!(a.seen(&[P]));
        on(&mut a, "PreCompact", json!({"trigger": "auto"}), 3400);
        on(&mut a, "PostCompact", json!({"trigger": "auto"}), 3410);
        assert!(a.get(P).is_none());
    }

    #[test]
    fn a_summary_written_ahead_while_claude_waits_leaves_it_waiting() {
        let mut a = Agents::default();
        on(&mut a, "UserPromptSubmit", json!({}), 100);
        on(
            &mut a,
            "PermissionRequest",
            json!({"tool_name": "Bash", "tool_input": {"command": "git push"}}),
            110,
        );
        assert!(!on(&mut a, "PreCompact", json!({"trigger": "auto"}), 111));
        let agent = a.get(P).unwrap();
        assert_eq!((agent.state, agent.since), (Some(State::Waiting), 110));
        assert_eq!(agent.wait.as_deref(), Some("permission: Bash git push"));
    }

    #[test]
    fn a_compaction_you_typed_leaves_the_tab_idle() {
        let mut a = Agents::default();
        on(&mut a, "Stop", json!({}), 100);
        on(&mut a, "PreCompact", json!({"trigger": "manual"}), 110);
        on(&mut a, "PostCompact", json!({"trigger": "manual"}), 120);
        assert!(a.get(P).is_none());
        // One that fails, or that you cancel, runs no PostCompact: Claude
        // Code saying the session went idle after it hands the tab back.
        on(&mut a, "PreCompact", json!({"trigger": "manual"}), 130);
        a.heard(
            P,
            &hook("PreCompact", json!({"trigger": "manual"})),
            130_000,
        );
        assert_eq!(a.state(P), Some(State::Working));
        assert!(!a.went_idle(P, 129_000), "idle from before it began");
        assert!(a.went_idle(P, 131_200));
        let agent = a.get(P).unwrap();
        assert_eq!((agent.state, agent.since), (None, 0));
        assert_eq!(agent.activity.as_deref(), Some("compacting context"));
    }

    #[test]
    fn claude_saying_it_went_idle_ends_only_work_nothing_has_followed() {
        let mut a = Agents::default();
        on(&mut a, "UserPromptSubmit", json!({}), 100);
        a.heard(P, &hook("UserPromptSubmit", json!({})), 100_400);
        on(&mut a, "PreToolUse", json!({"tool_name": "Read"}), 101);
        a.heard(P, &hook("PreToolUse", json!({})), 101_200);
        // Idle as the turn began, written before the prompt's hook came.
        assert!(!a.went_idle(P, 100_300));
        assert!(!a.went_idle(P, 101_100), "a hook came after it");
        assert_eq!(a.state(P), Some(State::Working));
        // A wait is its own hooks' and the transcript's to end.
        on(&mut a, "PermissionRequest", json!({}), 105);
        assert!(!a.went_idle(P, 106_000));
        assert_eq!(a.state(P), Some(State::Waiting));
        // A finished turn stays finished.
        on(&mut a, "Stop", json!({}), 110);
        assert!(!a.went_idle(P, 111_000));
        assert_eq!(a.state(P), Some(State::Done));
        assert!(!a.went_idle(PaneId(9), 111_000));
    }

    #[test]
    fn a_prompt_sent_while_it_compacts_idle_works_on() {
        let mut a = Agents::default();
        on(&mut a, "Stop", json!({}), 100);
        on(&mut a, "PreCompact", json!({"trigger": "auto"}), 110);
        on(&mut a, "UserPromptSubmit", json!({}), 115);
        on(&mut a, "PostCompact", json!({"trigger": "auto"}), 120);
        assert_eq!(a.state(P), Some(State::Working));
    }

    #[test]
    fn seeing_a_tab_clears_its_finished_panes() {
        let mut a = Agents::default();
        let (p1, p2) = (PaneId(1), PaneId(2));
        a.apply(p1, &hook("Stop", json!({})), 100, false);
        a.apply(p2, &hook("PermissionRequest", json!({})), 100, false);
        assert!(a.seen(&[p1, p2]));
        assert_eq!(a.state(p1), None);
        assert_eq!(
            a.state(p2),
            Some(State::Waiting),
            "a wait stays until answered"
        );
        assert!(!a.seen(&[p1, p2]));
    }

    #[test]
    fn a_pane_no_longer_running_claude_loses_its_state() {
        let mut a = Agents::default();
        on(
            &mut a,
            "UserPromptSubmit",
            json!({"prompt": "/deploy-test"}),
            100,
        );
        assert_eq!(a.marked(), vec![P]);
        assert!(a.drop_state(P));
        assert_eq!(a.state(P), None);
        assert!(a.marked().is_empty());
        assert!(!a.drop_state(P));
    }

    #[test]
    fn a_wait_you_interrupted_is_handed_back_unless_the_pane_moved_on() {
        let mut a = Agents::default();
        on(
            &mut a,
            "UserPromptSubmit",
            json!({"prompt": "/ci-loop"}),
            90,
        );
        on(
            &mut a,
            "PermissionRequest",
            json!({"tool_name": "Bash", "tool_input": {"command": "git push"}}),
            100,
        );
        assert!(
            !a.interrupted(P, 99_999),
            "a request made after it waits on"
        );
        assert_eq!(a.state(P), Some(State::Waiting));
        assert!(a.interrupted(P, 100_000));
        let agent = a.get(P).unwrap();
        assert_eq!(
            (agent.state, agent.since, agent.wait.as_ref()),
            (None, 0, None)
        );
        assert_eq!(agent.loop_name.as_deref(), Some("ci"), "the loop goes on");
        assert!(!a.interrupted(P, 100_000), "once");
        // Working again since, it has moved on.
        on(&mut a, "UserPromptSubmit", json!({}), 110);
        assert!(!a.interrupted(P, 109_999));
        assert_eq!(a.state(P), Some(State::Working));
        assert!(!a.interrupted(PaneId(9), 120_000));
    }

    #[test]
    fn work_you_interrupted_is_handed_back_unless_a_prompt_came_after() {
        let mut a = Agents::default();
        on(
            &mut a,
            "UserPromptSubmit",
            json!({"prompt": "/ci-loop"}),
            100,
        );
        on(
            &mut a,
            "PreToolUse",
            json!({"tool_name": "Bash", "tool_input": {"command": "sleep 120"}}),
            100,
        );
        a.heard(P, &hook("UserPromptSubmit", json!({})), 100_500);
        assert!(a.interrupted(P, 130_000));
        let agent = a.get(P).unwrap();
        assert_eq!((agent.state, agent.since), (None, 0));
        assert_eq!(agent.loop_name.as_deref(), Some("ci"), "the loop goes on");

        // A prompt sent after the interruption works on.
        on(&mut a, "UserPromptSubmit", json!({}), 140);
        a.heard(P, &hook("UserPromptSubmit", json!({})), 140_000);
        assert!(!a.interrupted(P, 139_000));
        assert_eq!(a.state(P), Some(State::Working));

        // Never handed back, the pane still works from before; a prompt
        // typed after the interruption keeps that `since`, but its hook was
        // heard after it.
        a.heard(P, &hook("UserPromptSubmit", json!({})), 150_000);
        assert!(!a.interrupted(P, 145_000));
        assert_eq!(a.state(P), Some(State::Working));
    }

    #[test]
    fn options_read_and_set_by_their_tmux_names() {
        let mut a = Agents::default();
        assert_eq!(
            a.set_option(P, "@agent_state", Some("waiting"), 50),
            Some(Ok(()))
        );
        assert_eq!(a.option(P, "@agent_state"), Some(Some("waiting".into())));
        assert_eq!(a.option(P, "@agent_since"), Some(Some("50".into())));
        assert!(matches!(
            a.set_option(P, "@agent_state", Some("asleep"), 50),
            Some(Err(_))
        ));
        assert_eq!(a.set_option(P, "@agent_state", None, 50), Some(Ok(())));
        assert!(a.get(P).is_none());
        assert_eq!(a.set_option(P, "@tab_name", Some("x"), 50), None);
        assert_eq!(a.option(P, "@other"), None);
        assert!(a.get(P).is_none());
    }
}
