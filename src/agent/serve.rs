//! The agent state on the server: a hook marks its pane, focus clears what
//! you have seen, the keys walk to what needs you, and a second Claude in a
//! tab gets a tab of its own. What tmux/agent-state.sh did with tmux calls
//! (`set -p`, `display -p`, `list-panes`), done on the server's own model,
//! in the server task.
//!
//! Whether a pane runs Claude is read from its foreground processes and
//! Claude Code's session files, without starting a process. It is asked
//! only when a pane's state changes and a second after a focus, never on
//! every tool call, since most tool calls change nothing.

use std::time::{Duration, Instant};

use serde_json::Value;

use super::sessions;
use super::state::{DONE, PINNED, QUESTION_KINDS, State, Stop, WAITING};
use crate::mux::server::{ClientId, Event, Reply, Server};
use crate::mux::{PaneId, TabId};
use crate::{settings, util};

/// How long a tab must stay in front for its finished mark to count as seen
/// (agent-state.sh's `AGENT_SEEN_DELAY`): passing over a tab, as moving the
/// sidebar's cursor does, leaves it.
pub const SEEN_DELAY: Duration = Duration::from_secs(1);

/// How often every pane is checked for a Claude that ended without saying
/// so, at most: each check reads every marked pane's processes, and holding
/// a key that switches tabs would otherwise check once per tab passed.
const SWEEP_EVERY: Duration = Duration::from_secs(1);

/// An option's value, read from the file now rather than from the cache:
/// these are read on rare events (a notification, a session starting, a
/// look at a tab), so a change made by hand applies at once.
fn setting(key: &str) -> Value {
    settings::setting(key).unwrap_or(Value::Null)
}

fn setting_on(key: &str) -> bool {
    setting(key) != Value::Bool(false)
}

impl Server {
    /// `hook <event> [--parked <pid>,...]`, the hook's JSON on stdin, from
    /// `switchboard hook`: marks the caller's pane, or with `--parked` the
    /// pane in front of the first of those Claude processes (a backgrounded
    /// conversation's tab). Always answers success: a hook has nothing to
    /// say back.
    pub fn agent_hook(
        &mut self,
        args: &[String],
        caller: Option<PaneId>,
        stdin: Option<&[u8]>,
    ) -> Reply {
        let payload = stdin
            .and_then(|b| serde_json::from_slice::<Value>(b).ok())
            .unwrap_or(Value::Null);
        let event = args.first().map(String::as_str).unwrap_or("");
        let parked: Vec<u32> = args
            .iter()
            .position(|a| a == "--parked")
            .and_then(|i| args.get(i + 1))
            .map(|list| {
                list.split(',')
                    .filter_map(|p| p.trim().parse().ok())
                    .collect()
            })
            .unwrap_or_default();
        let Some(pane) = caller.or_else(|| self.parked_pane(&parked)) else {
            return Reply::ok("");
        };
        let Some(tab) = self.panes.get(&pane).map(|s| s.tab) else {
            return Reply::ok("");
        };
        let hook = super::Hook::read(event, &payload);
        let front = self.tab_in_front(tab);
        let now = util::now_secs();
        let changed = if hook.event == "Notification" {
            // Permission prompts and questions are Claude waiting on you. The
            // PermissionRequest hook has usually said so already, since this
            // one only comes once a prompt has waited about six seconds.
            let waits =
                QUESTION_KINDS.contains(&hook.kind.as_str()) && self.agents.wait(pane, now, front);
            self.banner(tab, &hook.message);
            waits
        } else {
            self.agents.apply(pane, &hook, now, front)
        };
        if changed {
            // A pane that has gone back to a shell keeps no state, so a
            // session that died without SessionEnd leaves no mark behind.
            let panes = self.tab_panes(tab);
            self.sweep(&panes);
            self.redraw_tab(tab);
            // The first tab to wait on you, or to finish, while you look
            // elsewhere brings its tip on M-a, once.
            if !front {
                match self.agents.state(pane) {
                    Some(State::Waiting) => {
                        self.tip("waiting", None);
                    }
                    Some(State::Done) => {
                        self.tip("done", None);
                    }
                    _ => {}
                }
            }
        }
        if hook.event == "SessionStart"
            && hook.source != "compact"
            && setting_on("one_claude_per_tab")
        {
            self.one_claude(pane);
        }
        Reply::ok("")
    }

    /// The pane in front of the first of these processes: the tab's parked
    /// Claude is the pane's program, or a program its shell started, in the
    /// process group in front on the pane's terminal.
    fn parked_pane(&self, pids: &[u32]) -> Option<PaneId> {
        pids.iter().find_map(|&pid| {
            let group = process_group(pid);
            self.panes
                .iter()
                .find(|(_, slot)| {
                    slot.pane.pid() == Some(pid)
                        || match group {
                            Some(g) => slot.pane.foreground_pgid() == Some(g),
                            None => slot.pane.foreground_pids().contains(&pid),
                        }
                })
                .map(|(id, _)| *id)
        })
    }

    /// Whether a tab is the one in front in a session someone is looking at
    /// (tmux's `window_active` and `session_attached`).
    fn tab_in_front(&self, tab: TabId) -> bool {
        self.session_of_tab(tab)
            .is_some_and(|s| s.current == tab && self.clients.values().any(|c| c.session == s.name))
    }

    fn tab_panes(&self, tab: TabId) -> Vec<PaneId> {
        self.tabs
            .get(&tab)
            .map(|t| t.layout.panes())
            .unwrap_or_default()
    }

    /// Whether a pane runs Claude: a process in front on its terminal has a
    /// session file (tmux/claude-pane.sh).
    pub fn runs_claude(&self, pane: PaneId) -> bool {
        self.panes.get(&pane).is_some_and(|slot| {
            sessions::runs_claude(&sessions::sessions_dir(), &slot.pane.foreground_pids())
        })
    }

    /// A tab's state, as its status line badge shows it.
    pub fn tab_agent(&self, tab: TabId) -> Option<State> {
        self.agents.rollup(&self.tab_panes(tab))
    }

    /// A pane's record for `switchboard list`, or null.
    pub fn agent_json(&self, pane: PaneId) -> Value {
        self.agents
            .get(pane)
            .map(super::PaneAgent::json)
            .unwrap_or(Value::Null)
    }

    /// Every status line showing this tab draws again: the badges and counts
    /// are on all of a session's clients, whichever tab they show.
    fn redraw_tab(&mut self, tab: TabId) {
        if let Some(name) = self.session_of_tab(tab).map(|s| s.name.clone()) {
            self.dirty_session(&name);
        }
    }

    /// Drops the state of each of these panes no longer running Claude.
    /// Gives whether any changed.
    fn sweep(&mut self, panes: &[PaneId]) -> bool {
        let mut changed = false;
        for pane in panes {
            if self.agents.state(*pane).is_some() && !self.runs_claude(*pane) {
                changed |= self.agents.drop_state(*pane);
            }
        }
        changed
    }

    /// A tab was focused: a second later, if it is still in front, it has
    /// been seen (the pane-focus-in hook's `agent-state.sh seen`).
    pub fn agent_focus(&mut self, tab: TabId) {
        self.send_later(SEEN_DELAY, Event::AgentSeen(tab));
    }

    /// A second after a tab was focused: still in front, its finished panes
    /// have been seen, unless the done_clears setting keeps the mark until
    /// the next prompt, and a reminder come due is done with. Then every
    /// marked pane, in every tab, is checked for a Claude that ended without
    /// saying so.
    pub fn agent_seen(&mut self, tab: TabId) {
        if self.tab_in_front(tab) {
            if setting("done_clears") != "prompt" {
                let panes = self.tab_panes(tab);
                if self.agents.seen(&panes) {
                    self.redraw_tab(tab);
                }
            }
            // A reminder come due (tmux/remind.sh) is done with once you are
            // at its tab.
            let now = util::now_secs();
            if let Some(t) = self.tabs.get_mut(&tab)
                && t.opts
                    .get("@remind_at")
                    .and_then(|at| at.trim().parse::<i64>().ok())
                    .is_some_and(|at| at <= now)
            {
                for option in ["@remind_at", "@remind_note", "@remind_said"] {
                    t.opts.remove(option);
                }
                self.redraw_tab(tab);
            }
        }
        if self
            .agents
            .swept
            .is_some_and(|at| at.elapsed() < SWEEP_EVERY)
        {
            return;
        }
        self.agents.swept = Some(Instant::now());
        let panes = &self.panes;
        self.agents.retain(|p| panes.contains_key(&p));
        for pane in self.agents.marked() {
            if self.sweep(&[pane])
                && let Some(tab) = self.panes.get(&pane).map(|s| s.tab)
            {
                self.redraw_tab(tab);
            }
        }
    }

    /// A banner (platform::notify) naming the tab, so you know where to go;
    /// none with the notifications setting off. Sent off the server task,
    /// since macOS's takes a process.
    fn banner(&self, tab: TabId, message: &str) {
        if !setting_on("notifications") {
            return;
        }
        let Some(s) = self.session_of_tab(tab) else {
            return;
        };
        let index = s.tabs.iter().position(|t| *t == tab).unwrap_or(0) + 1;
        let name = self
            .tabs
            .get(&tab)
            .map(|t| t.display_name().to_string())
            .unwrap_or_default();
        let title = format!("Claude Code: {index}:{name}");
        let text = if message.is_empty() {
            "Claude Code needs your attention".to_string()
        } else {
            message.to_string()
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::spawn_blocking(move || crate::platform::notify(&title, &text));
        }
    }

    /// One Claude to a tab (tmux/one-claude.sh): a session starting in a tab
    /// that already runs one moves to a tab of its own, since the sidebar,
    /// mission control, the Slack relay, M-c and M-m each take a tab's Claude
    /// to be one pane. Claude Code's SessionStart hook can't refuse a
    /// session, so the pane is moved instead. Only a pane running Claude
    /// moves: a `claude -p` started elsewhere may carry a pane's
    /// SWITCHBOARD_PANE too.
    fn one_claude(&mut self, pane: PaneId) {
        if !self.runs_claude(pane) {
            return;
        }
        let Some(tab) = self.panes.get(&pane).map(|s| s.tab) else {
            return;
        };
        let other = self
            .tab_panes(tab)
            .into_iter()
            .any(|p| p != pane && self.runs_claude(p));
        if !other {
            return;
        }
        let session = self.session_of_tab(tab).map(|s| s.name.clone());
        match self.break_pane(pane) {
            Ok(_) => {
                // Said on a client showing that session, the one used last;
                // with none, to no one.
                if let Some(client) = session.and_then(|s| self.latest_client_of(&s)) {
                    self.say(
                        Some(client),
                        "That tab already had a Claude, so this session has a tab of its own. (Settings: One Claude to a tab)",
                    );
                }
            }
            Err(e) => eprintln!("one Claude to a tab: {e}"),
        }
    }

    /// M-a: the next tab that needs you, as agent-state.sh's `next` walked
    /// them without the sidebar's daemon: pinned, then waiting, then done,
    /// oldest first within each, across every session. With nowhere to go,
    /// it says so. Pressing it is learning it, so its tip isn't shown after.
    pub fn next_needs_you(&mut self, client: ClientId) {
        self.tip_seen("waiting");
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        let Some(here) = self.session(&session).map(|s| s.current) else {
            return;
        };
        let stops = self.needs_you();
        let Some(i) = self.agents.next_stop(&stops, here) else {
            let words = if stops.is_empty() {
                "Nothing needs you."
            } else {
                "This is the only tab that needs you."
            };
            self.say(Some(client), words);
            return;
        };
        let stop = stops[i].clone();
        // The tab's pane first, so switching sessions lands on it rather
        // than on whatever was last focused there.
        self.focus_pane(stop.pane);
        if stop.session != session {
            self.switch_client(client, &stop.session);
        }
    }

    /// Every tab that needs you, in M-a's order.
    fn needs_you(&self) -> Vec<Stop> {
        let mut stops = Vec::new();
        for s in &self.sessions {
            for id in &s.tabs {
                let Some(tab) = self.tabs.get(id) else {
                    continue;
                };
                let leader = self.agents.leader(&tab.layout.panes());
                let pinned = tab.opts.get("@pinned").is_some_and(|v| pinned(v));
                let (rank, since) = match leader {
                    _ if pinned => (PINNED, 0),
                    Some((_, State::Waiting, since)) => (WAITING, since),
                    Some((_, State::Done, since)) => (DONE, since),
                    _ => continue,
                };
                stops.push(Stop {
                    rank,
                    since,
                    tab: *id,
                    pane: leader.map(|(p, _, _)| p).unwrap_or(tab.active),
                    session: s.name.clone(),
                });
            }
        }
        stops.sort_by_key(|s| (s.rank, s.since, s.tab));
        stops
    }

    /// M-c: into the pane running Claude in this tab (tmux/claude-focus.sh),
    /// or, with none, saying so.
    pub fn focus_claude(&mut self, tab: TabId) {
        match self
            .tab_panes(tab)
            .into_iter()
            .find(|p| self.runs_claude(*p))
        {
            Some(pane) => self.focus_pane(pane),
            None => self.say(None, "No Claude in this tab"),
        }
    }

    /// M-f: pins the tab, or unpins it. A pin of its own, not Claude's
    /// "done": visiting the tab doesn't clear it, and pinned tabs lead M-a's
    /// walk (and the sidebar's Needs you). The first pin brings its tip.
    pub fn toggle_pin(&mut self, tab: TabId) {
        let Some(t) = self.tabs.get_mut(&tab) else {
            return;
        };
        let pinning = !t.opts.get("@pinned").is_some_and(|v| pinned(v));
        if pinning {
            t.opts.insert("@pinned".into(), "1".into());
        } else {
            t.opts.remove("@pinned");
        }
        self.redraw_tab(tab);
        if pinning {
            self.tip("pinned", None);
        }
    }

    /// One of a pane's `@agent_` options, for `show`; `None` for any other.
    pub fn agent_option(&self, pane: PaneId, name: &str) -> Option<Option<String>> {
        self.agents.option(pane, name)
    }

    /// Sets one of a pane's `@agent_` options, for `set` (land-lock marks
    /// its phase this way); `None` for any other option.
    pub fn set_agent_option(
        &mut self,
        pane: PaneId,
        name: &str,
        value: Option<&str>,
    ) -> Option<Result<(), String>> {
        let result = self
            .agents
            .set_option(pane, name, value, util::now_secs())?;
        if let Some(tab) = self.panes.get(&pane).map(|s| s.tab) {
            self.redraw_tab(tab);
        }
        Some(result)
    }
}

/// A process's group, one call to the kernel, so finding a parked Claude's
/// pane reads no pane's whole process list; `None` when it can't be read,
/// and the processes are read instead.
fn process_group(pid: u32) -> Option<u32> {
    let pid = libc::pid_t::try_from(pid).ok().filter(|p| *p > 0)?;
    // SAFETY: getpgid only reads the kernel's process table; a pid that
    // isn't there is an error, not undefined.
    let group = unsafe { libc::getpgid(pid) };
    u32::try_from(group).ok().filter(|g| *g > 0)
}

/// Whether an `@pinned` value is set, as tmux's `#{@pinned}` read it: not
/// empty and not 0.
fn pinned(value: &str) -> bool {
    !value.is_empty() && value != "0"
}
