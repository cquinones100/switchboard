//! What each Claude session in the workspace is doing, as tmux/agent-state.sh
//! kept it in pane options (`@agent_state`, `@agent_since`, `@agent_activity`,
//! `@agent_wait`, `@agent_loop`, `@agent_phase`, `@agent_compact_from`): here
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

use std::collections::HashMap;
use std::time::Instant;

use serde_json::{Value, json};

use crate::mux::{PaneId, TabId};

/// What a pane's Claude is doing. Ordered by how much it needs you, so the
/// most urgent of several is their `max`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
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

/// One pane's Claude: its state and what goes with it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
    /// The loop the session runs (review, ci, queue or deploy), from the
    /// moment its skill is asked for until a new prompt or session: the turns
    /// between a loop's rounds are the loop's too.
    pub loop_name: Option<String>,
    /// The step a loop says it is at (queue-and-clean's landing), set by the
    /// loop and ended by a new prompt or session.
    pub phase: Option<String>,
    /// How it was before an automatic compaction began, to hand it back
    /// after: `None` when it was working (nothing to hand back), `Some(None)`
    /// when it was idle, else its state and since.
    compact_from: Option<Option<(State, i64)>>,
}

impl PaneAgent {
    fn is_empty(&self) -> bool {
        self.state.is_none()
            && self.activity.is_none()
            && self.wait.is_none()
            && self.loop_name.is_none()
            && self.phase.is_none()
            && self.compact_from.is_none()
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
            field("skill").unwrap_or_default()
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

    /// Whether this is a new prompt or a new or ended session, which ends a
    /// loop, its phase and a compaction cut short.
    fn begins_anew(&self) -> bool {
        matches!(
            self.event.as_str(),
            "UserPromptSubmit" | "SessionStart" | "SessionEnd"
        )
    }
}

/// The skill a prompt starts with (`/review-loop now` gives `review-loop`),
/// as `^\s*/([a-z-]+)` captured it.
fn slash_command(prompt: &str) -> String {
    let Some(rest) = prompt.trim_start().strip_prefix('/') else {
        return String::new();
    };
    rest.chars()
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

/// A tab that needs you, as M-a walks them: pinned first, then waiting, then
/// done, each oldest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stop {
    /// 0 pinned, 1 waiting, 2 done.
    pub rank: u8,
    pub since: i64,
    pub tab: TabId,
    /// The pane to focus: the Claude that needs you, else the tab's own.
    pub pane: PaneId,
    pub session: String,
}

pub const PINNED: u8 = 0;
pub const WAITING: u8 = 1;
pub const DONE: u8 = 2;

impl Stop {
    fn key(&self) -> (u8, i64, TabId) {
        (self.rank, self.since, self.tab)
    }
}

/// Where M-a's walk got to: the tab it last went to, and that stop's place in
/// the order, kept as a place rather than a tab, since a done tab stops being
/// done the moment it is seen and is then no longer in the list to count from.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Walk {
    tab: TabId,
    key: (u8, i64, TabId),
}

/// Every pane's Claude, by pane.
#[derive(Debug, Default)]
pub struct Agents {
    panes: HashMap<PaneId, PaneAgent>,
    walk: Option<Walk>,
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
    }

    /// Applies a hook to its pane, `front` saying whether the pane's tab is in
    /// front in a session someone is looking at, at `now` (seconds). Gives
    /// whether the pane's state changed.
    pub fn apply(&mut self, pane: PaneId, hook: &Hook, now: i64, front: bool) -> bool {
        let Some(mut state) = hook.marks() else {
            return false;
        };
        // Compaction: Claude is at work on it, whatever the tab showed, and a
        // session it restarts (SessionStart from compact) is the same one
        // going on, so it keeps its state and loop. After a /compact you
        // typed, nothing follows, so the tab is idle again. An automatic one
        // hands the tab back as it was before: working in the middle of a
        // turn, idle or done when Claude compacted while idle.
        if hook.event == "SessionStart" && hook.source == "compact" {
            return false;
        }
        let agent = self.panes.entry(pane).or_default();
        let mut handed_back = false;
        match hook.event.as_str() {
            "PreCompact" => {
                agent.compact_from = match agent.state {
                    Some(State::Working) => None,
                    other => Some(other.map(|s| (s, agent.since))),
                };
            }
            "PostCompact" => {
                let before = agent.compact_from.take();
                state = if hook.source == "manual" {
                    None
                } else {
                    match before {
                        // As it was, since included, so a finished tab keeps
                        // its age.
                        Some(Some((s, since))) => {
                            handed_back = agent.state != Some(s);
                            agent.state = Some(s);
                            agent.since = since;
                            Some(s)
                        }
                        // A look while it compacted has already made it idle.
                        Some(None) => None,
                        None => Some(State::Working),
                    }
                };
            }
            _ => {}
        }
        // A question to you is a wait from the moment it is asked.
        if hook.event == "PreToolUse" && hook.tool == "AskUserQuestion" {
            state = Some(State::Waiting);
        }
        let changed = set_state(agent, state, now, front) || handed_back;
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
        let changed = set_state(agent, Some(State::Waiting), now, front);
        if agent.is_empty() {
            self.panes.remove(&pane);
        }
        changed
    }

    /// A tab's panes, in front for a second: a finished one has been seen,
    /// and one seen while it compacts comes back idle rather than finished.
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
            if matches!(agent.compact_from, Some(Some((State::Done, _)))) {
                agent.compact_from = Some(None);
            }
            if agent.is_empty() {
                self.panes.remove(p);
            }
        }
        changed
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
        if agent.is_empty() {
            self.panes.remove(&pane);
        }
        Some(result)
    }

    /// M-a: which of `stops` (in their order) to go to from the tab `here`,
    /// keeping where the walk got to. Each press goes to the stop after the
    /// one it last went to, wrapping at the end, so pressing it again and
    /// again walks the whole list; a tab stays waiting after a visit until
    /// you answer it, so "the most urgent tab" would keep coming back to the
    /// same one. From a finished tab in the walk, a tab pinned or waiting
    /// comes first. `None` when nothing needs you, or only `here` does.
    pub fn next_stop(&mut self, stops: &[Stop], here: TabId) -> Option<usize> {
        if stops.is_empty() {
            return None;
        }
        let walk = self.walk.as_ref().filter(|w| w.tab == here);
        let away = || stops.iter().position(|s| s.tab != here);
        let to = match walk {
            Some(w) => {
                let urgent = (w.key.0 == DONE)
                    .then(|| stops.iter().position(|s| s.rank < DONE && s.tab != here))
                    .flatten();
                urgent
                    .or_else(|| stops.iter().position(|s| s.key() > w.key))
                    .unwrap_or(0)
            }
            None => away().unwrap_or(0),
        };
        let stop = &stops[to];
        if stop.tab == here {
            return None;
        }
        self.walk = Some(Walk {
            tab: stop.tab,
            key: stop.key(),
        });
        Some(to)
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
        // A compaction cut short by a new prompt or session leaves nothing to
        // hand back, and a loop's step is set again by the loop as it goes on.
        agent.compact_from = None;
        agent.phase = None;
    }
    match state {
        None | Some(State::Done) => {
            agent.activity = None;
            agent.wait = None;
        }
        Some(State::Working) => {
            agent.wait = None;
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
    fn an_automatic_compaction_while_idle_hands_the_tab_back() {
        let mut a = Agents::default();
        on(&mut a, "Stop", json!({}), 100);
        on(&mut a, "PreCompact", json!({"trigger": "auto"}), 110);
        assert_eq!(a.state(P), Some(State::Working));
        on(&mut a, "PostCompact", json!({"trigger": "auto"}), 120);
        let agent = a.get(P).unwrap();
        assert_eq!(agent.state, Some(State::Done));
        assert_eq!(agent.since, 100, "a finished tab keeps its age");
        // From idle, it goes back to idle.
        let mut a = Agents::default();
        on(&mut a, "PreCompact", json!({"trigger": "auto"}), 110);
        on(&mut a, "PostCompact", json!({"trigger": "auto"}), 120);
        assert_eq!(a.state(P), None);
    }

    #[test]
    fn a_finished_tab_seen_while_it_compacts_comes_back_idle() {
        let mut a = Agents::default();
        on(&mut a, "Stop", json!({}), 100);
        on(&mut a, "PreCompact", json!({"trigger": "auto"}), 110);
        a.seen(&[P]);
        assert_eq!(a.state(P), Some(State::Working), "working stays");
        on(&mut a, "PostCompact", json!({"trigger": "auto"}), 120);
        assert_eq!(a.state(P), None);
    }

    #[test]
    fn a_compaction_you_typed_leaves_the_tab_idle() {
        let mut a = Agents::default();
        on(&mut a, "Stop", json!({}), 100);
        on(&mut a, "PreCompact", json!({"trigger": "manual"}), 110);
        on(&mut a, "PostCompact", json!({"trigger": "manual"}), 120);
        assert!(a.get(P).is_none());
    }

    #[test]
    fn a_new_prompt_cuts_a_compaction_short() {
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

    fn stop(rank: u8, since: i64, tab: u32) -> Stop {
        Stop {
            rank,
            since,
            tab: TabId(tab),
            pane: PaneId(tab * 10),
            session: "main".into(),
        }
    }

    #[test]
    fn the_walk_goes_round_what_needs_you() {
        let mut a = Agents::default();
        let stops = vec![stop(WAITING, 10, 1), stop(WAITING, 20, 2)];
        // From elsewhere, the first; then each press the next, wrapping.
        assert_eq!(a.next_stop(&stops, TabId(9)), Some(0));
        assert_eq!(a.next_stop(&stops, TabId(1)), Some(1));
        assert_eq!(a.next_stop(&stops, TabId(2)), Some(0));
        // Pressed in a tab the walk didn't go to, it starts again from the
        // top, passing over the tab it is pressed in.
        assert_eq!(a.next_stop(&stops, TabId(2)), Some(0));
        assert_eq!(a.next_stop(&stops, TabId(9)), Some(0));
        // A done tab leaves the list once seen; pressed there, the walk
        // carries on from its place rather than from the top.
        let with_done = vec![stop(DONE, 4, 2), stop(DONE, 5, 3), stop(DONE, 6, 4)];
        assert_eq!(a.next_stop(&with_done, TabId(2)), Some(1));
        let seen = vec![stop(DONE, 4, 2), stop(DONE, 6, 4)];
        assert_eq!(a.next_stop(&seen, TabId(3)), Some(1));
    }

    #[test]
    fn from_a_finished_tab_a_waiting_one_comes_first() {
        let mut a = Agents::default();
        let stops = vec![stop(PINNED, 0, 4), stop(DONE, 5, 3), stop(DONE, 6, 5)];
        assert_eq!(a.next_stop(&stops, TabId(4)), Some(1));
        // From the finished tab 3, the pinned tab 4 first.
        assert_eq!(a.next_stop(&stops, TabId(3)), Some(0));
    }

    #[test]
    fn nothing_to_walk_to_goes_nowhere() {
        let mut a = Agents::default();
        assert_eq!(a.next_stop(&[], TabId(1)), None);
        assert_eq!(a.next_stop(&[stop(WAITING, 1, 1)], TabId(1)), None);
    }
}
