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

use super::state::{QUESTION_KINDS, State};
use super::{hands_free, sessions};
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
    /// conversation's tab). Always answers success, saying nothing but in a
    /// tab of autopilot's hands-free mode, where a question box or a turn
    /// ending on a question is answered for Claude Code to read
    /// (`hands_free_answer`).
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
        let mut hook = super::Hook::read(event, &payload);
        let answer = if caller.is_some() && hook.agent.is_empty() {
            self.hands_free_answer(tab, &hook, &payload)
        } else {
            None
        };
        if answer.is_some() {
            // Answered for Claude, so it goes on working: a question denied
            // marks no wait, a stop blocked no finish.
            if hook.event == "Stop" {
                return Reply::ok(answer.unwrap_or_default());
            }
            hook.tool.clear();
            hook.wait.clear();
        }
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
        if hook.agent.is_empty() {
            self.agents.heard(pane, util::now_ms());
        }
        self.history_hook(pane, &hook, &payload);
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
        if hook.event == "SessionStart" && hook.source != "compact" {
            // The first Claude to start in a worktree brings the tip on its
            // PR's keys, with GitHub on (agent-state.sh's SessionStart).
            if setting_on("github") && !self.tip_was_seen("worktree") {
                let dir = payload
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(std::path::PathBuf::from)
                    .or_else(|| self.panes.get(&pane).and_then(|s| s.pane.cwd()));
                if dir.is_some_and(|d| in_worktree(&d)) {
                    self.tip("worktree", None);
                }
            }
            if setting_on("one_claude_per_tab") {
                self.one_claude(pane);
            }
        }
        Reply::ok(answer.unwrap_or_default())
    }

    /// What a hook in a tab of autopilot's hands-free mode is answered
    /// with (src/agent/hands_free.rs): a question box denied unless marked
    /// always asked, a turn ending on a question blocked at most `NUDGES`
    /// times; nothing once the tab has pushed or landed, nor for anything
    /// else.
    fn hands_free_answer(
        &mut self,
        tab: TabId,
        hook: &super::Hook,
        payload: &Value,
    ) -> Option<String> {
        use crate::autopilot::{AUTOLAND, HANDS_FREE, LEFT, NUDGED, PUSHED};
        let t = self.tabs.get_mut(&tab)?;
        // The setting read at the hook too, so turning it off counts at once.
        if t.opts.get(HANDS_FREE).is_none_or(|v| v != "1") || !setting_on("autopilot_handsfree") {
            return None;
        }
        // Never once landed: pushed (`@autopilot_pushed`, or
        // `@autopilot_autoland`, set as the landing pushes) or back in the
        // main clone (`@autopilot_left`); what comes after, such as applying
        // the change, is asked as usual.
        if [LEFT, AUTOLAND, PUSHED]
            .iter()
            .any(|m| t.opts.contains_key(*m))
        {
            return None;
        }
        match hook.event.as_str() {
            "PreToolUse" if hook.tool == "AskUserQuestion" => {
                (!hands_free::always_asked(payload)).then(hands_free::deny_question)
            }
            "Stop" if hands_free::sends_back(payload) => {
                let nudged: u32 = t.opts.get(NUDGED).and_then(|n| n.parse().ok()).unwrap_or(0);
                if nudged >= hands_free::NUDGES {
                    return None;
                }
                t.opts.insert(NUDGED.into(), (nudged + 1).to_string());
                self.dirty_tab(tab);
                Some(hands_free::block_stop())
            }
            _ => None,
        }
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
    pub(crate) fn tab_in_front(&self, tab: TabId) -> bool {
        self.session_of_tab(tab)
            .is_some_and(|s| s.current == tab && self.clients.values().any(|c| c.session == s.name))
    }

    /// A tab's panes, a Claude parked behind the diff or lazygit included:
    /// the tab still shows what it is doing, and looking at the tab sees it
    /// finished, as looking at a tmux window did whatever was in front.
    pub(crate) fn tab_panes(&self, tab: TabId) -> Vec<PaneId> {
        self.panes_with_parked(tab)
    }

    /// Whether a pane runs Claude, by its program's name or a session file
    /// in front (tmux/claude-pane.sh, `sessions::pane_runs_claude`).
    pub fn runs_claude(&self, pane: PaneId) -> bool {
        self.panes
            .get(&pane)
            .is_some_and(|slot| sessions::pane_runs_claude(&sessions::sessions_dir(), &slot.pane))
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
            // A tab of autopilot, landed and back in the main
            // clone, has been seen, so it closes once you leave it
            // (src/autopilot).
            if let Some(t) = self.tabs.get_mut(&tab)
                && t.opts.contains_key("@autopilot_left")
                && !t.opts.contains_key("@autopilot_seen")
            {
                t.opts.insert("@autopilot_seen".into(), "1".into());
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

    /// M-a: the next tab that needs you, in the sidebar's order (its Needs
    /// you, then the notifications), as agent-state.sh's `next` walked the
    /// list the sidebar's daemon wrote: src/sidebar has the walk. Pressing
    /// it is learning it, so its tip isn't shown after.
    pub fn next_needs_you(&mut self, client: ClientId) {
        self.tip_seen("waiting");
        self.sidebar_next_needs_you(client);
    }

    /// M-A: the next idle tab, in the sidebar's Idle order (src/sidebar has
    /// the walk); with the idle_walk setting off, the key reaches the pane.
    pub fn next_idle(&mut self, client: ClientId) {
        if !settings::is_on("idle_walk") {
            self.key_back_to_pane(client, &crate::mux::keys::Action::NextIdle);
            return;
        }
        self.sidebar_next_idle(client);
    }

    /// M-c: into the pane running Claude in this tab (tmux/claude-focus.sh),
    /// or, with none, saying so.
    pub fn focus_claude(&mut self, tab: TabId) {
        // Only one in the layout: a parked one is reached by quitting
        // what stands in its place.
        let shown = self.tabs.get(&tab).map(|t| t.layout.panes());
        match shown
            .unwrap_or_default()
            .into_iter()
            .find(|p| self.runs_claude(*p))
        {
            Some(pane) => self.focus_pane(pane),
            None => {
                // UX 2.0: M-c is the header's Claude key, so with the diff,
                // the PR or the ticket in Claude's place it closes that,
                // which gives Claude its place back (src/diff/in_place.rs).
                let in_place = settings::is_on("ux2")
                    .then(|| self.tabs.get(&tab).map(|t| t.active))
                    .flatten()
                    .filter(|p| {
                        self.panes
                            .get(p)
                            .is_some_and(|s| s.opts.contains_key("@in_place"))
                    });
                match in_place {
                    Some(tool) => self.kill_pane(tool),
                    None => self.say(None, "No Claude in this tab"),
                }
            }
        }
    }

    /// M-y: the latest reply of the tab's Claude to the clipboard
    /// (tmux/reply-copy.sh), from any pane of the tab, the sidebar included:
    /// its Claude pane, as M-c finds it, or the one a tool standing in its
    /// place (M-d, lazygit) has parked, and that pane's transcript as the
    /// sidebar last found it, read from its end off the server's task
    /// (`sidebar::claude::last_reply`). A box says how many lines were
    /// copied, and that Claude is still working when it is, or that the tab
    /// has no Claude or no reply yet. With the copy_reply setting off, the
    /// key reaches the pane.
    pub fn copy_reply(&mut self, client: ClientId, tab: TabId) {
        if !settings::is_on("copy_reply") {
            self.key_back_to_pane(client, &crate::mux::keys::Action::CopyReply);
            return;
        }
        let Some(pane) = self
            .panes_with_parked(tab)
            .into_iter()
            .find(|p| self.runs_claude(*p))
        else {
            self.say(Some(client), "No Claude in this tab");
            return;
        };
        let transcript = |server: &Server| {
            server
                .sidebar
                .panes
                .get(&pane)
                .and_then(|read| read.transcript.clone())
        };
        // A session started since the sidebar's last look is looked for now.
        if transcript(self).is_none() {
            self.sidebar_read();
        }
        let Some(file) = transcript(self) else {
            self.say(Some(client), "Couldn't find this tab's Claude session");
            return;
        };
        let working = self
            .agents
            .get(pane)
            .is_some_and(|a| a.state == Some(State::Working));
        self.off_task(
            client,
            move || crate::sidebar::claude::last_reply(&file),
            move |server, client, reply| {
                let Some(reply) = reply else {
                    server.say(Some(client), "Claude hasn't replied yet in this tab");
                    return;
                };
                server.to_clipboard(Some(client), &reply);
                let lines = reply.lines().count().max(1);
                let mut words = format!(
                    "Copied Claude's latest reply ({lines} line{})",
                    if lines == 1 { "" } else { "s" }
                );
                if working {
                    words.push_str(", written while it is still working");
                }
                server.say(Some(client), &words);
            },
        );
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

    /// A turn you interrupted on a permission request (denied, or Esc on
    /// it), at `at` seconds, read from the pane's transcript by the sidebar
    /// (`claude::Work::interrupted_at`), since no hook runs for it: the pane
    /// waiting since then or before goes idle, for the tab bar, the sidebar,
    /// M-a and the Slack relay, and one that has moved on since is left
    /// alone. A Claude parked behind the diff or lazygit too, since its
    /// request can be denied from the Slack relay. Gives whether it changed.
    pub fn agent_interrupted(&mut self, pane: PaneId, at: i64) -> bool {
        if !self.agents.interrupted(pane, at) {
            return false;
        }
        if let Some(tab) = self.panes.get(&pane).map(|s| s.tab) {
            self.redraw_tab(tab);
        }
        true
    }

    /// A question the pane's transcript shows unanswered, asked at `at`
    /// (ms), read by the sidebar (`claude::Turn::asks`): the pane waits on
    /// it, unless the conversation's own hooks have said more since
    /// (`Agents::asked`). For a question whose hook never reached the
    /// server, which would leave the tab looking busy while Claude waits on
    /// you. Gives whether it changed.
    pub fn agent_asked(&mut self, pane: PaneId, at: i64, question: &str) -> bool {
        let Some(tab) = self.panes.get(&pane).map(|s| s.tab) else {
            return false;
        };
        let front = self.tab_in_front(tab);
        if !self
            .agents
            .asked(pane, at, question, util::now_secs(), front)
        {
            return false;
        }
        self.redraw_tab(tab);
        true
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

/// Whether a directory is in a worktree the worktree commands made, under a
/// repository's `.claude/worktrees/` (agent-state.sh's
/// `*/.claude/worktrees/*`).
fn in_worktree(dir: &std::path::Path) -> bool {
    dir.to_string_lossy().contains("/.claude/worktrees/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_worktree_is_under_a_repositorys_claude_worktrees() {
        assert!(in_worktree(Path::new("/r/app/.claude/worktrees/eng-1-fix")));
        assert!(in_worktree(Path::new("/r/app/.claude/worktrees/eng-1/src")));
        assert!(!in_worktree(Path::new("/r/app/.claude")));
        assert!(!in_worktree(Path::new("/r/app")));
    }
}
