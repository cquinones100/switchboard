//! The sidebar on the server: what bin/tmux-sidebar's daemon did, done in
//! the server task on its own model. Each tab's model is put together when
//! a client is drawn, from what the server holds (labels, colors, the agent
//! state, the facts the sources keep, the tab's options) and from what was
//! last read of Claude Code's files, which is read again every tick and
//! after a hook (`sidebar_read`), never while drawing. The daemon read tmux
//! in one `list-panes` per refresh for the same reason.
//!
//! Focus is the session's, as a sidebar pane's was its window's: M-s moves
//! it into the sidebar and back out (tmux.conf's binding), and while it is
//! there the keys the daemon's `sidebarKey` took move a cursor through the
//! tabs, each tab coming to the front as the cursor reaches it, with the
//! layout held still (`frozen`) so tabs don't jump about as you look at
//! them. Leaving the sidebar keeps the tab you are on.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use serde_json::Value;

use super::model::{self, MergeState, Rules, TabLoop, TabModel, TabState};
use super::peek::Peek;
use super::render::{self, Filter, Notice, Owner, View};
use super::walk::{Next, Place, Stop, Why};
use super::{Drawn, PaneRead, SidebarEvent, claude, config, notices, text};
use crate::agent::State;
use crate::facts::{Merge, PrState};
use crate::items::Target;
use crate::mux::compose::colors;
use crate::mux::input::{Key, KeyCode, Mods, Mouse, MouseButton, MouseKind};
use crate::mux::keys::Action;
use crate::mux::server::{ClientId, Event, Server, Session};
use crate::mux::{PaneId, TabId};
use crate::ui::{Outcome, PickItem, Picker, Theme};
use crate::{paths, settings, util};

mod history;
mod window;

/// How often the spinner turns.
const SPIN: Duration = Duration::from_millis(text::SPINNER_MS);

/// The sidebar's keys, for `?` (render.js's `HELP.sidebar`, less what comes
/// with mission control), and mission control's Keys view.
pub const KEYS: &[(&str, &str)] = &[
    ("j k", "show a tab"),
    ("g G", "first, last"),
    ("^d ^u", "half a screen down, up"),
    ("↵ l", "go to the tab"),
    ("h esc q ^c", "back to your pane"),
    ("1-9", "go to that tab"),
    ("/", "find a tab by title (↵ go, esc clear)"),
    (
        "tab ⇧tab",
        "next, last tab (with tab_focus off: mission control)",
    ),
    ("M-space", "mission control"),
    ("space", "peek at it"),
    ("a", "answer it, or send it a message"),
    ("n", "to the notifications, when there are any"),
    ("t", "new tab running Claude there (M-t)"),
    ("i", "its full title (M-T)"),
    ("d L", "its diff, or lazygit, in its pane's place"),
    ("M-x C-x", "run that key there"),
    ("M-s", "back to your pane"),
    ("?", "these keys"),
];

/// The shades the sidebar is drawn in: the light ones when the theme is
/// light, the canvas's for a truecolor terminal (src/status/theme.rs), as
/// the boxes are.
fn theme() -> Theme {
    crate::status::theme::theme()
}

/// A process's group, one call to the kernel; `None` for one gone.
fn process_group(pid: u32) -> Option<u32> {
    let pid = libc::pid_t::try_from(pid).ok().filter(|p| *p > 0)?;
    // SAFETY: getpgid only reads the kernel's process table; a pid that
    // isn't there is an error, not undefined.
    let group = unsafe { libc::getpgid(pid) };
    u32::try_from(group).ok().filter(|g| *g > 0)
}

/// A label's PR in front (`#12 name`) and the rest.
fn split_pr(label: &str) -> (Option<u64>, String) {
    if let Some(rest) = label.strip_prefix('#')
        && let Some((num, name)) = rest.split_once(' ')
        && let Ok(n) = num.parse::<u64>()
    {
        return (Some(n), name.to_string());
    }
    (None, label.to_string())
}

impl Server {
    // ── Its place on the screen ─────────────────────────────────────────────

    /// Reads sidebar.json, at the start and whenever it changes.
    pub fn sidebar_load(&mut self) {
        let seen = std::fs::metadata(config::config_file())
            .and_then(|m| m.modified())
            .ok();
        if self.sidebar.config_seen != Some(seen) {
            self.sidebar.config_seen = Some(seen);
            self.sidebar.config = config::Config::read();
        }
    }

    /// The columns the sidebar takes in a session `cols` wide, its line
    /// beside the panes included: none with the sidebar setting off or a
    /// window narrower than sidebar.json's `minWindowWidth`.
    pub fn sidebar_columns(&self, cols: u16) -> u16 {
        if settings::is_on("sidebar") {
            self.sidebar.config.columns_in(cols)
        } else {
            0
        }
    }

    /// Whether a session shows its sidebar now: it has room for one and its
    /// tab in front isn't zoomed (a zoomed pane covers it, as a zoomed tmux
    /// pane covered the sidebar's).
    pub fn sidebar_shown(&self, session: &str) -> bool {
        self.session(session)
            .is_some_and(|s| s.sidebar > 0 && self.tabs.get(&s.current).is_none_or(|t| !t.zoomed))
    }

    /// Gives each session the sidebar its size and the settings allow, laying
    /// its tabs out again where that changed.
    pub fn fit_sidebars(&mut self) {
        let wanted: Vec<(String, u16)> = self
            .sessions
            .iter()
            .map(|s| (s.name.clone(), self.sidebar_columns(s.cols)))
            .collect();
        for (name, columns) in wanted {
            let Some(s) = self.session_mut(&name) else {
                continue;
            };
            if s.sidebar == columns {
                continue;
            }
            s.sidebar = columns;
            let tabs = s.tabs.clone();
            for t in tabs {
                self.relayout(t);
            }
            if columns == 0 {
                self.sidebar_leave(&name);
            }
            self.dirty_session(&name);
        }
    }

    // ── Reading ─────────────────────────────────────────────────────────────

    /// Asks Haiku for a sentence on each pane's last turn that has ended
    /// (a reply, no call waiting, its Claude neither working nor waiting),
    /// once a turn, in a task of its own, with Settings' `turn_summaries`
    /// and `ux2` on, the cards that show it being UX 2.0's
    /// (src/mission/summary.rs). A turn begun over a day ago is left as
    /// it is, so a server starting doesn't send every old tab's.
    fn summarize_turns(&mut self, reads: &std::collections::HashMap<PaneId, PaneRead>) {
        if !settings::is_on("turn_summaries") || !settings::is_on("ux2") {
            return;
        }
        let day_ago = util::now_ms() - 86_400_000;
        let away = self.away();
        for (pane, read) in reads {
            let (Some(session), Some(turn)) = (&read.session, &read.turn) else {
                continue;
            };
            // What was kept for the session shows whatever the turn's age.
            self.mission.summaries.load(&session.session_id);
            let (Some(at), Some(reply)) = (turn.at, &turn.reply) else {
                continue;
            };
            let ended = turn.pending.is_none()
                && !matches!(
                    self.agents.state(*pane),
                    Some(State::Working | State::Waiting)
                );
            // Held while you're away (`pause_when_away`), as the lookups
            // are, and asked once you're back.
            if !ended
                || at < day_ago
                || away
                || !self.mission.summaries.ask_now(&session.session_id, at)
            {
                continue;
            }
            let program = util::which("claude").unwrap_or_else(|| "claude".into());
            let (session, prompt, reply) = (
                session.session_id.clone(),
                turn.prompt.clone(),
                reply.clone(),
            );
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let text = crate::mission::summary::ask(
                    &program,
                    &crate::mission::summary::dir(),
                    &prompt,
                    &reply,
                )
                .await;
                let _ = tx.send(Event::Mission(crate::mission::MissionEvent::Summary {
                    session,
                    turn_at: at,
                    text,
                }));
            });
        }
    }

    /// Reads, for every pane, the Claude conversation in front of it and what
    /// it runs in the background, from Claude Code's files: the session files
    /// (each read again only when it changed), each process's group (one
    /// call to the kernel), and each transcript as far as it has grown.
    pub fn sidebar_read(&mut self) {
        let now = util::now_ms();
        let claude_dir = paths::claude_dir();
        let reader = &mut self.sidebar.reader;
        let sessions = reader.sessions(&claude_dir.join("sessions"));
        let groups: Vec<Option<u32>> = sessions
            .iter()
            .map(|s| s.pid.and_then(process_group))
            .collect();
        let mut reads = std::collections::HashMap::new();
        let mut files = std::collections::HashSet::new();
        for (id, slot) in &self.panes {
            let front = slot.pane.foreground_pgid();
            let in_front: Vec<&claude::SessionInfo> = sessions
                .iter()
                .zip(&groups)
                .filter(|(_, g)| g.is_some() && **g == front)
                .map(|(s, _)| s)
                .collect();
            let mut read = PaneRead {
                session: claude::session_of(&sessions, &in_front),
                command: slot.pane.foreground_command(),
                claude: !in_front.is_empty()
                    || crate::agent::sessions::named_like_claude(&slot.pane),
                ..PaneRead::default()
            };
            if let Some(session) = &read.session
                && let Some(file) = reader.transcript(&claude_dir, session)
            {
                files.insert(file.clone());
                // A transcript that can't be read leaves the tab shown
                // without what it would have added, never the others.
                if let Ok(work) =
                    reader.background_work(&file, session.started_at.unwrap_or(0), now)
                {
                    read.work = work;
                }
                read.turn = reader.last_turn(&file);
                read.said_at = std::fs::metadata(&file)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64);
                read.transcript = Some(file);
            }
            reads.insert(*id, read);
        }
        reader.retain(&files);
        let interrupted: Vec<(PaneId, i64)> = reads
            .iter()
            .filter_map(|(p, r)| Some((*p, r.work.interrupted_at?)))
            .collect();
        // When Claude Code says each pane's session went idle.
        let idle: Vec<(PaneId, i64)> = reads
            .iter()
            .filter_map(|(p, r)| Some((*p, r.session.as_ref()?.idle_since?)))
            .collect();
        // A question asked since the session started, still unanswered.
        let asks: Vec<(PaneId, String, i64, String)> = reads
            .iter()
            .filter_map(|(p, r)| {
                let (question, at, call) = r.turn.as_ref()?.asks.clone()?;
                let started = r.session.as_ref()?.started_at.unwrap_or(0);
                (at >= started).then_some((*p, question, at, call))
            })
            .collect();
        // The call each pane's turn is stopped on, for a wait whose hook
        // named no tool.
        let pending: Vec<(PaneId, String, bool, String)> = reads
            .iter()
            .filter_map(|(p, r)| {
                let turn = r.turn.as_ref()?;
                let only = turn.unanswered == 1;
                Some((*p, turn.pending_call.clone(), only, turn.pending.clone()?))
            })
            .collect();
        let changed = reads != self.sidebar.panes;
        self.summarize_turns(&reads);
        self.sidebar.panes = reads;
        self.sidebar.stale = false;
        self.sidebar_hand_back(&interrupted);
        for (pane, at) in idle {
            if self.agent_went_idle(pane, at) {
                self.dirty_sidebars();
            }
        }
        for (pane, question, at, call) in asks {
            if self.agent_asked(pane, at, &call, &question) {
                self.dirty_sidebars();
            }
        }
        for (pane, call, only, words) in pending {
            if self.agent_name_wait(pane, &call, only, &words) {
                self.dirty_sidebars();
            }
        }
        if changed {
            self.dirty_sidebars();
        }
    }

    /// A turn you interrupted, waiting on a request or working, runs no
    /// hook, so its pane is left as it was: each pane whose transcript says so is handed
    /// back as idle (`agent_interrupted`, the daemon's `handBack`), once for
    /// each interruption (`at`, ms), a parked one included, as every pane is
    /// read.
    fn sidebar_hand_back(&mut self, interrupted: &[(PaneId, i64)]) {
        let panes = &self.panes;
        self.sidebar
            .handed_back
            .retain(|p, _| panes.contains_key(p));
        for &(pane, at) in interrupted {
            if self.sidebar.handed_back.get(&pane) != Some(&at) && self.agent_interrupted(pane, at)
            {
                self.sidebar.handed_back.insert(pane, at);
                self.dirty_sidebars();
            }
        }
    }

    /// Every client showing a sidebar draws again.
    fn dirty_sidebars(&mut self) {
        let ids: Vec<ClientId> = self.sidebar.drawn.keys().copied().collect();
        for id in ids {
            if let Some(c) = self.clients.get_mut(&id) {
                c.dirty = true;
            }
        }
    }

    /// Every tick: sidebar.json read again when it changed, each session's
    /// sidebar fitted to its size and the settings, the panes read again,
    /// and the tips only the sidebar can tell are due; what was kept for a
    /// client, session or pane that has gone is let go. The sidebars draw
    /// again, for the times they show ("waiting on you 4m").
    pub fn sidebar_tick(&mut self) {
        self.sidebar_load();
        self.fit_sidebars();
        self.sidebar_read();
        self.sidebar_tips();
        let clients = &self.clients;
        self.sidebar.drawn.retain(|id, _| clients.contains_key(id));
        self.sidebar.hover.retain(|id, _| clients.contains_key(id));
        let sessions = &self.sessions;
        self.sidebar
            .sides
            .retain(|name, _| sessions.iter().any(|s| &s.name == name));
        let panes = &self.panes;
        self.sidebar.output_at.retain(|p, _| panes.contains_key(p));
        self.dirty_sidebars();
    }

    /// Before drawing: read again when a hook has come since the last read.
    pub fn sidebar_before_draw(&mut self) {
        if self.sidebar.stale && self.clients.values().any(|c| c.dirty) {
            self.sidebar_read();
        }
    }

    /// The tips only the sidebar can tell are due, asked once a run: the
    /// first merged tab and the first tab looping in the background (the
    /// sidebar's own comes when it is first focused).
    fn sidebar_tips(&mut self) {
        let tabs = self.sidebar_all_tabs();
        let due = [
            (
                "merged",
                tabs.iter().any(|t| t.merge == Some(MergeState::Merged)),
            ),
            (
                "loop",
                tabs.iter()
                    .any(|t| t.state == TabState::Background && t.tab_loop.is_some()),
            ),
        ];
        for (name, now) in due {
            if now && self.sidebar.tips_asked.insert(name) {
                self.tip(name, None);
            }
        }
    }

    // ── The model ───────────────────────────────────────────────────────────

    /// A session's tabs as the sidebar and mission control see them, in the
    /// session's order.
    pub fn sidebar_tabs(&self, session: &str) -> Vec<TabModel> {
        let Some(s) = self.session(session) else {
            return Vec::new();
        };
        let attached = self.clients.values().any(|c| c.session == s.name);
        // The Notifications window is no tab (src/sidebar/notices.rs).
        s.tabs
            .iter()
            .enumerate()
            .filter(|(_, id)| !self.is_notices_tab(**id))
            .filter_map(|(i, id)| self.tab_model(s, attached, *id, i + 1))
            .collect()
    }

    /// Every session's tabs, one list.
    pub fn sidebar_all_tabs(&self) -> Vec<TabModel> {
        self.sessions
            .iter()
            .flat_map(|s| self.sidebar_tabs(&s.name))
            .collect()
    }

    /// The repository a session's sidebar is filtered to, while Settings'
    /// sidebar_repo_filter is on.
    pub(crate) fn sidebar_repo(&self, session: &str) -> Option<&std::path::Path> {
        if !settings::is_on("sidebar_repo_filter") {
            return None;
        }
        self.sidebar.sides.get(session)?.repo.as_deref()
    }

    /// A session's tabs as its sidebar groups them: those of another
    /// repository than its filter marked `elsewhere`, for Other repos.
    pub(crate) fn sidebar_marked(&self, session: &str) -> Vec<TabModel> {
        let mut tabs = self.sidebar_tabs(session);
        if let Some(repo) = self.sidebar_repo(session) {
            for t in &mut tabs {
                t.elsewhere = t.repo_dir.as_deref() != Some(repo);
            }
        }
        tabs
    }

    /// Every session's tabs as M-a and M-A walk them: those under a
    /// session's Other repos left out of Needs you and Idle.
    fn sidebar_walked_tabs(&self) -> Vec<TabModel> {
        self.sessions
            .iter()
            .flat_map(|s| self.sidebar_marked(&s.name))
            .collect()
    }

    /// Whether a session's Other repos is folded to one line: while its
    /// filter is on, draws an Other repos group (the held layout's
    /// included), and wasn't unfolded;
    /// never while a / search is on, so its matches are all reachable.
    pub(crate) fn sidebar_folded(&self, session: &str, tabs: &[TabModel]) -> bool {
        let searching = self
            .sidebar
            .sides
            .get(session)
            .filter(|s| s.focused)
            .and_then(|s| s.filter.as_ref())
            .is_some_and(|f| !f.text.trim().is_empty());
        self.sidebar_repo(session).is_some()
            && !searching
            && !self.sidebar.sides.get(session).is_some_and(|s| s.repo_open)
            && {
                let frozen = self
                    .sidebar
                    .sides
                    .get(session)
                    .and_then(|s| s.frozen.as_ref());
                model::grouped(tabs, frozen, &Rules::now())
                    .iter()
                    .any(|g| g.stage == model::Stage::OtherRepos)
            }
    }

    /// One tab (model.js's `tabsFrom` for one window).
    fn tab_model(&self, s: &Session, attached: bool, id: TabId, index: usize) -> Option<TabModel> {
        let tab = self.tabs.get(&id)?;
        let (pr, name) = split_pr(&self.tab_label(id));
        // A note item's tab shows its whole first line, which the sidebar
        // wraps to its three lines, while its name is that item cut short.
        let name = self.tab_full_title(id).unwrap_or(name);
        let panes = tab.layout.panes();
        // Its Claude is looked for behind a tool in its pane's place too
        // (the diff, lazygit, a view), so the tab keeps the stage its
        // Claude is in rather than falling to Other.
        let with_parked = self.tab_panes(id);
        // The pane whose state the tab shows, and since when: the longest
        // there of those in that state; else the first running Claude.
        let leader = self.agents.leader(&with_parked);
        let read_of = |p: PaneId| self.sidebar.panes.get(&p);
        let claude_pane = leader.map(|(p, _, _)| p).or_else(|| {
            with_parked
                .iter()
                .copied()
                .find(|p| read_of(*p).is_some_and(|r| r.session.is_some()))
                .or_else(|| {
                    with_parked
                        .iter()
                        .copied()
                        .find(|p| read_of(*p).is_some_and(|r| r.claude))
                })
        });
        let read = claude_pane.and_then(read_of);
        let session = read.and_then(|r| r.session.clone());
        let base = match leader.map(|(_, state, _)| state) {
            Some(State::Waiting) => TabState::Waiting,
            Some(State::Done) => TabState::Done,
            Some(State::Working) => TabState::Working,
            None => TabState::Idle,
        };
        // Read for a working tab too, since a turn an API error cut short
        // leaves its pane marked working (no Stop hook runs); its background
        // work counts only for a tab between turns.
        let mut work = match read {
            Some(r) if session.is_some() && base != TabState::Waiting => r.work.clone(),
            _ => claude::Work::default(),
        };
        // A working tab's loop counts only for its round, which its card
        // shows.
        let round = work.loop_run.as_ref().map_or(0, |r| r.round);
        if base == TabState::Working {
            work = claude::Work {
                api_error: work.api_error,
                ..claude::Work::default()
            };
        }
        let stopped = work.api_error.clone();
        // A turn that ends while only watches run is Done, under Needs you,
        // until you see it (the hook state goes Idle), as a question is
        // Waiting whatever runs behind it; then Monitoring.
        let finished_over_watches = base == TabState::Done
            && work.count > 0
            && work.monitors.len() == work.count
            && work.loop_run.is_none()
            && work.scheduled.is_none();
        let state = if stopped.is_some() {
            TabState::Waiting
        } else if finished_over_watches {
            base
        } else if work.count > 0 || work.scheduled.is_some() {
            TabState::Background
        } else {
            base
        };
        let agent = claude_pane.and_then(|p| self.agents.get(p));
        let opt = |k: &str| tab.opts.get(k).filter(|v| !v.is_empty()).cloned();
        let since = if let Some((_, at)) = &stopped {
            Some(at.div_euclid(1000)).filter(|s| *s > 0)
        } else if base == TabState::Idle {
            // When it last did anything: a Claude tab's last word in its
            // conversation, else the last output to its pane.
            read.and_then(|r| r.said_at)
                .or_else(|| self.sidebar.output_at.get(&tab.active).copied())
        } else {
            leader.map(|(_, _, since)| since).filter(|s| *s > 0)
        };
        let mut t = TabModel::new(id, index, &name);
        t.session = s.name.clone();
        t.attached = attached;
        t.current = s.current == id;
        t.zoomed = tab.zoomed;
        t.own_name = self.tab_own_name(id).map(str::to_string);
        t.color = self.tab_color(id);
        t.state = state;
        t.pinned = opt("@pinned").is_some_and(|v| v != "0");
        t.remind_at = opt("@remind_at")
            .and_then(|v| v.trim().parse::<i64>().ok())
            .filter(|n| *n != 0);
        t.remind_note = opt("@remind_note");
        // A landed tab made yours (`@autopilot_yours`) is a plain tab here.
        t.autopilot = opt("@autopilot")
            .and_then(|v| v.trim().parse::<f64>().ok())
            .is_some_and(|n| n != 0.0)
            && opt("@autopilot_yours").is_none();
        t.autopilot_landed = t.autopilot && opt("@autopilot_left").is_some();
        t.autopilot_autoland = t.autopilot && opt("@autopilot_autoland").is_some();
        // Notes beside this tab's work, or, on Notes' own tab while it is
        // away, where it went (model.js's notesHere, notesAway, notesOwner).
        let marked = |p: &PaneId, name: &str| {
            self.panes
                .get(p)
                .and_then(|slot| slot.opts.get(name))
                .is_some_and(|v| !v.is_empty() && v != "0")
        };
        t.notes_here = panes.len() > 1 && panes.iter().any(|p| marked(p, "@notes_pane"));
        t.notes_away = panes.iter().any(|p| marked(p, "@notes_placeholder"));
        if t.notes_away {
            t.notes_owner = self.notes_owner(&s.name);
        }
        t.background = work.count;
        t.monitors = work.monitors.clone();
        t.round = round;
        t.tab_loop = work
            .loop_run
            .clone()
            .map(TabLoop::Run)
            .or_else(|| work.scheduled.clone().map(TabLoop::Scheduled));
        let some = |v: &Option<String>| v.clone().filter(|v| !v.is_empty());
        t.activity = agent.and_then(|a| some(&a.activity));
        t.wait = stopped
            .map(|(text, _)| text)
            .or_else(|| agent.and_then(|a| some(&a.wait)));
        t.ask = agent
            .filter(|a| a.wait.is_some())
            .and_then(|a| a.ask.clone());
        t.doing = agent
            .filter(|a| a.state == Some(crate::agent::State::Working))
            .and_then(|a| a.doing.clone());
        t.context = agent.and_then(|a| a.context.as_ref()).map(|c| c.used);
        t.loop_skill = agent.and_then(|a| some(&a.loop_name));
        t.phase = agent.and_then(|a| some(&a.phase));
        t.since = since;
        t.path = self.tab_path(id).map(std::path::Path::to_path_buf);
        t.repo = t.path.as_deref().and_then(|p| self.sources.origin_kept(p));
        t.repo_dir = self
            .tab_names
            .shown
            .get(&id)
            .and_then(|shown| shown.repo_dir.clone());
        t.command = read_of(tab.active).and_then(|r| r.command.clone());
        t.panes = panes;
        t.active = Some(tab.active);
        t.claude_pane = claude_pane;
        t.claude = session;
        t.turn = read.and_then(|r| r.turn.clone());
        // Shown only while the setting is on, whatever was kept before.
        t.summary = read
            .filter(|_| settings::is_on("turn_summaries"))
            .and_then(|r| Some((r.session.as_ref()?, r.turn.as_ref()?.at?)))
            .and_then(|(s, at)| self.mission.summaries.get(&s.session_id, at))
            .map(str::to_string);
        // The PR its label shows, else the one GitHub says its branch has.
        t.pr = pr;
        t.pr_repo = self.tab_pr_repo(id).map(ToString::to_string);
        // What GitHub and the landing queue say of it, as prs.js's mergeOf
        // and landing.js's landingOf gave it; none of GitHub's with the
        // github setting off.
        let facts = self.facts.get(&id);
        if let Some(p) = facts
            .and_then(|f| f.pr.as_ref())
            .filter(|_| settings::is_on("github"))
        {
            // The repository GitHub answered from, with its number, when the
            // tab's name didn't say (a gone worktree's branch found in a
            // fork's upstream).
            if t.pr_repo.is_none() && t.pr.is_none_or(|n| n == p.number) {
                t.pr_repo = Some(p.repo.clone());
            }
            t.pr = t.pr.or(Some(p.number));
            t.pr_state = Some(p.state);
            t.merge = match p.merge {
                Some(Merge::Merged) => Some(MergeState::Merged),
                Some(Merge::Queued { position }) => {
                    t.position = Some(position);
                    Some(MergeState::Queued)
                }
                None if p.state == PrState::Merged => Some(MergeState::Merged),
                None => None,
            };
            t.checks = p.checks.filter(|_| p.state != PrState::Merged);
            t.review = p.review_decision.clone();
            t.size = Some((p.additions, p.deletions));
        }
        t.landing = facts.and_then(|f| f.landing);
        // Landed straight on a branch (`land-lock landed`): Merged, as a
        // merged PR is, unless GitHub answered for a PR of its own.
        // Only while the tab is still in the landed repository's main
        // clone, on its branch then, read from git's files.
        if t.pr_state.is_none()
            && let Some(mark) = opt(model::LANDED).as_deref().and_then(model::landed_mark)
            && t.path
                .as_deref()
                .and_then(crate::tabs::git::locate)
                .is_some_and(|p| model::landed_here(&mark, &p))
        {
            t.landed_on = Some((mark.hash, mark.branch));
            t.merge = Some(MergeState::Merged);
        }
        Some(t)
    }

    /// The notifications the sources have given, each source's list in the
    /// order the kinds are shown (`notices::ORDER`): none with the
    /// sidebar_notifications setting off.
    pub(crate) fn notice_lists(&self) -> Vec<(&str, &[crate::items::Item])> {
        if !settings::is_on("sidebar_notifications") {
            return Vec::new();
        }
        let mut lists: Vec<(&str, &[crate::items::Item])> = self
            .notices
            .iter()
            .filter(|(_, items)| !items.is_empty())
            .map(|(source, items)| (source.as_str(), items.as_slice()))
            .collect();
        lists.sort_by_key(|(source, _)| notices::rank(source));
        lists
    }

    /// The notifications in the sidebar's shape: a row for each kind, with
    /// how many there are (`notices::summary`), named by its first, so the
    /// tabs keep the room however many come in; the Notifications window
    /// lists each.
    fn sidebar_notices(&self) -> Vec<Notice> {
        self.notice_lists()
            .into_iter()
            .map(|(source, items)| {
                let (title, detail) = notices::summary(source, items);
                Notice {
                    owner: Owner::Notice {
                        source: source.to_string(),
                        id: items[0].id.clone(),
                    },
                    title,
                    detail,
                    // The newest one's time; a meeting's is when it starts,
                    // not when it came.
                    time: (source != "meeting")
                        .then(|| items.iter().filter_map(|i| i.time).max())
                        .flatten()
                        .map(|ms| ms.div_euclid(1000)),
                }
            })
            .collect()
    }

    /// A session's tabs as its sidebar lists them: all, or those whose name
    /// has its / filter's text.
    fn sidebar_listed(&self, session: &str) -> Vec<TabModel> {
        let text = self
            .sidebar
            .sides
            .get(session)
            .filter(|s| s.focused)
            .and_then(|s| s.filter.as_ref())
            .map(|f| f.text.trim().to_lowercase())
            .unwrap_or_default();
        let tabs = self.sidebar_marked(session);
        if text.is_empty() {
            return tabs;
        }
        tabs.into_iter()
            .filter(|t| t.name.to_lowercase().contains(&text))
            .collect()
    }

    /// The tabs in the order a session's sidebar lists them, those under
    /// a folded Other repos left out.
    fn sidebar_order(&self, session: &str) -> Vec<TabId> {
        let r = Rules::now();
        let tabs = self.sidebar_listed(session);
        let folded = self.sidebar_folded(session, &tabs);
        let frozen = self
            .sidebar
            .sides
            .get(session)
            .and_then(|s| s.frozen.as_ref());
        // By the groups as drawn, the held layout's included, so a tab
        // that moved repository while you browse is where it shows.
        model::grouped(&tabs, frozen, &r)
            .into_iter()
            .filter(|g| !(folded && g.stage == model::Stage::OtherRepos))
            .flat_map(|g| g.tabs)
            .map(|t| t.id)
            .collect()
    }

    /// Whether a tab is under a session's Other repos as the sidebar draws
    /// it, the held layout's group included, whatever a / search shows.
    fn sidebar_in_other_repos(&self, session: &str, tab: TabId) -> bool {
        let tabs = self.sidebar_marked(session);
        let frozen = self
            .sidebar
            .sides
            .get(session)
            .and_then(|s| s.frozen.as_ref());
        model::grouped(&tabs, frozen, &Rules::now())
            .iter()
            .any(|g| g.stage == model::Stage::OtherRepos && g.tabs.iter().any(|t| t.id == tab))
    }

    /// Whether a session's sidebar draws Other repos folded, a stop of
    /// its own after the last tab.
    fn sidebar_fold_stop(&self, session: &str) -> bool {
        let tabs = self.sidebar_listed(session);
        self.sidebar_folded(session, &tabs)
    }

    // ── Drawing ─────────────────────────────────────────────────────────────

    /// Draws a session's sidebar into a client's frame, on the left of the
    /// panes, with the line between them in the accent color while it has
    /// the focus; gives where it went, or `None` when it isn't shown.
    pub fn sidebar_draw(&self, buf: &mut Buffer, s: &Session, client: ClientId) -> Option<Drawn> {
        if !self.sidebar_shown(&s.name) {
            return None;
        }
        let status = crate::status::lines();
        let rows = s
            .rows
            .saturating_sub(status)
            .min(buf.area.height.saturating_sub(status));
        let width = (s.sidebar - 1).min(buf.area.width);
        let area = Rect::new(0, 0, width, rows).intersection(buf.area);
        if area.is_empty() {
            return None;
        }
        let r = Rules::now();
        let side = self.sidebar.sides.get(&s.name);
        let focused = side.is_some_and(|x| x.focused);
        let tabs = self.sidebar_listed(&s.name);
        let folded = self.sidebar_folded(&s.name, &tabs);
        let repo_name = self.sidebar_repo(&s.name).map(|d| {
            d.file_name()
                .unwrap_or(d.as_os_str())
                .to_string_lossy()
                .into_owned()
        });
        let notices = self.sidebar_notices();
        // Each plugin that asks for a list under the tabs (plugins.js's
        // `sidebar` count), from what its last run gave.
        let sections: Vec<render::Section> = self
            .usage
            .plugins
            .iter()
            .filter(|p| p.sidebar > 0)
            .map(|p| render::Section::of(&p.title, &p.items, p.error.as_deref(), p.sidebar))
            .collect();
        let view = View {
            focused,
            cursor: side.and_then(|x| x.cursor.as_ref()),
            frame: self.sidebar.frame,
            here: s.current,
            filter: side.and_then(|x| x.filter.as_ref()).filter(|_| focused),
            top: side.filter(|x| x.focused).and_then(|x| x.top),
            keep: side.and_then(|x| x.keep),
            hover: self.sidebar.hover.get(&client),
            awake: self.awake.holding(),
            repo: repo_name.as_deref(),
            folded,
        };
        let out = render::sidebar_lines(
            &tabs,
            side.and_then(|x| x.frozen.as_ref()),
            &notices,
            &sections,
            &s.name,
            area.width as usize,
            area.height as usize,
            &view,
            &r,
            &theme(),
        );
        render::draw(buf, area, &out.lines);
        if width < buf.area.width {
            let line = Style::new().fg(if focused {
                colors::ACCENT
            } else {
                crate::status::theme::palette().border
            });
            for y in 0..rows {
                buf[(width, y)].set_symbol("│").set_style(line);
            }
        }
        Some(Drawn {
            area,
            owners: out.owners,
            scroll: out.scroll,
            spins: out.spins,
        })
    }

    /// After a client is drawn: where its sidebar went, kept for the mouse
    /// and C-d; the list's place, kept while it has the focus, and kept for
    /// the next tab in front to hold to (`View::keep`); and the spinner's
    /// next turn, timed only while something spins.
    pub fn sidebar_drawn(&mut self, client: ClientId, session: &str, drawn: Option<Drawn>) {
        let Some(d) = drawn else {
            self.sidebar.drawn.remove(&client);
            return;
        };
        let side = self.sidebar.side(session);
        if side.focused {
            side.top = Some(d.scroll.top);
        }
        side.keep = Some(d.scroll.top);
        let spins = d.spins;
        self.sidebar.drawn.insert(client, d);
        if spins && !self.sidebar.spin_pending {
            self.sidebar.spin_pending = true;
            self.send_later(SPIN, Event::Sidebar(SidebarEvent::Spin));
        }
    }

    /// What the sidebar's timers say.
    pub fn sidebar_event(&mut self, event: SidebarEvent) {
        match event {
            SidebarEvent::Spin => {
                self.sidebar.spin_pending = false;
                self.sidebar.frame = self.sidebar.frame.wrapping_add(1);
                let spinning: Vec<ClientId> = self
                    .sidebar
                    .drawn
                    .iter()
                    .filter(|(_, d)| d.spins)
                    .map(|(id, _)| *id)
                    .collect();
                for id in spinning {
                    if let Some(c) = self.clients.get_mut(&id) {
                        c.dirty = true;
                    }
                }
            }
            SidebarEvent::HistoryRead(read) => self.notice_history_read(read),
            SidebarEvent::HistoryWritten => self.sidebar.history_writing = false,
        }
    }

    /// Whether a client's terminal should report every mouse movement for
    /// the sidebar: while it shows one and hover_highlight is on.
    pub fn sidebar_wants_motion(&self, client: ClientId) -> bool {
        self.sidebar.drawn.contains_key(&client) && settings::is_on("hover_highlight")
    }

    // ── Focus ───────────────────────────────────────────────────────────────

    /// Focus into a session's sidebar, its cursor on `cursor` or the tab you
    /// are in, the tabs held where they are while you browse them.
    pub fn sidebar_enter(&mut self, session: &str, cursor: Option<Owner>) {
        if !self.sidebar_shown(session) {
            return;
        }
        let Some(here) = self.session(session).map(|s| s.current) else {
            return;
        };
        let r = Rules::now();
        let frozen = model::frozen_from(&self.sidebar_marked(session), &r);
        let side = self.sidebar.side(session);
        if !side.focused {
            side.frozen = Some(frozen);
            side.top = None;
        }
        side.focused = true;
        // In the Notifications window, the cursor starts on its notification.
        let shown = self
            .is_notices_tab(here)
            .then(|| self.sidebar.windows.at.get(session).cloned())
            .flatten();
        let side = self.sidebar.side(session);
        side.cursor = Some(cursor.or(shown).unwrap_or(Owner::Tab(here)));
        self.sidebar_cursor_back(session);
        self.dirty_session(session);
        if self.sidebar.tips_asked.insert("sidebar") {
            let client = self.latest_client_of(session);
            self.tip("sidebar", client);
        }
    }

    /// Focus back out of a session's sidebar, to the tab in front's pane.
    pub fn sidebar_leave(&mut self, session: &str) {
        let Some(side) = self.sidebar.sides.get_mut(session) else {
            return;
        };
        if !side.focused {
            return;
        }
        side.focused = false;
        side.frozen = None;
        side.filter = None;
        side.top = None;
        self.dirty_session(session);
    }

    /// A pane of this tab took the focus: its session's sidebar gives it up.
    pub fn sidebar_leave_tab(&mut self, tab: TabId) {
        if let Some(name) = self.session_of_tab(tab).map(|s| s.name.clone()) {
            self.sidebar_leave(&name);
        }
    }

    /// M-s: into the sidebar, or back out of it. A zoomed pane is unzoomed
    /// first, as moving to the sidebar pane unzoomed a tmux window; with no
    /// sidebar to go to, it says why.
    fn sidebar_toggle_focus(&mut self, client: ClientId) {
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        if self.sidebar.focused(&session) {
            self.sidebar_leave(&session);
            return;
        }
        let Some(s) = self.session(&session) else {
            return;
        };
        if s.sidebar == 0 {
            let why = if settings::is_on("sidebar") {
                "This window is too narrow for the sidebar."
            } else {
                "The sidebar is off. (Settings: Sidebar, or \"Turn the sidebar on or off\" in M-p)"
            };
            self.say(Some(client), why);
            return;
        }
        let current = s.current;
        if let Some(t) = self.tabs.get_mut(&current)
            && t.zoomed
        {
            t.zoomed = false;
            self.relayout(current);
        }
        self.sidebar_enter(&session, None);
    }

    /// "Turn the sidebar on or off" (the palette's): the sidebar setting.
    fn sidebar_toggle(&mut self, client: ClientId) {
        let on = settings::is_on("sidebar");
        if let Err(e) = settings::write_setting("sidebar", Value::Bool(!on)) {
            self.say(Some(client), &format!("Couldn't change the setting: {e}"));
            return;
        }
        settings::reload_settings();
        self.fit_sidebars();
    }

    /// Before a key's action runs: with the sidebar focused, an action on
    /// "the pane" acts on your pane, so the sidebar gives up the focus first
    /// (tmux.conf's `leave-sidebar`); M-a, M-1 to M-9 and the boxes keep it.
    /// Moving between panes from the sidebar comes back out to the right, and
    /// goes nowhere else. Gives whether the action is done with, and whether
    /// the sidebar had the focus.
    pub fn sidebar_before_action(&mut self, client: ClientId, action: &Action) -> (bool, bool) {
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return (false, false);
        };
        match action {
            Action::FocusSidebar => {
                self.sidebar_toggle_focus(client);
                return (true, false);
            }
            Action::ToggleSidebar => {
                self.sidebar_toggle(client);
                return (true, false);
            }
            _ => {}
        }
        if !self.sidebar.focused(&session) {
            return (false, false);
        }
        match action {
            Action::SelectPane(dir) => {
                if *dir == crate::mux::Direction::Right {
                    self.sidebar_leave(&session);
                }
                (true, true)
            }
            Action::NextNeedsYou
            | Action::NextIdle
            | Action::SelectTab(_)
            | Action::Palette
            | Action::ListKeys
            | Action::Detach
            | Action::ResetTips
            | Action::TogglePin
            | Action::CopyReply
            // C-w's next key moves from the sidebar, and M-0's tab keeps
            // the cursor in a sidebar, as M-1 to M-9 do (src/extras).
            | Action::Extra(crate::extras::Command::PaneNav | crate::extras::Command::GoToTab) => {
                (false, true)
            }
            _ => {
                self.sidebar_leave(&session);
                (false, true)
            }
        }
    }

    /// After a key's action: M-1 to M-9 pressed from the sidebar leave its
    /// cursor on the tab they went to (sidebar.sh refocus).
    pub fn sidebar_after_action(&mut self, client: ClientId, was_focused: bool, action: &Action) {
        if !was_focused || !matches!(action, Action::SelectTab(_)) {
            return;
        }
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        let Some(here) = self.session(&session).map(|s| s.current) else {
            return;
        };
        if let Some(side) = self.sidebar.sides.get_mut(&session)
            && side.focused
        {
            side.cursor = Some(Owner::Tab(here));
            // A tab under a folded Other repos unfolds it, so the cursor
            // is on a tab the sidebar lists.
            if self.sidebar_in_other_repos(&session, here) {
                self.sidebar.side(&session).repo_open = true;
            }
            self.dirty_session(&session);
        }
    }

    // ── Keys ────────────────────────────────────────────────────────────────

    /// A key while the sidebar has the focus (after the bindings); gives
    /// whether the sidebar took it.
    pub fn sidebar_key(&mut self, client: ClientId, key: Key, raw: &[u8]) -> bool {
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return false;
        };
        if !self.sidebar.focused(&session) {
            return false;
        }
        if !self.sidebar_shown(&session) {
            self.sidebar_leave(&session);
            return false;
        }
        self.sidebar_key_in(client, &session, key, raw);
        true
    }

    /// Text pasted while the sidebar has the focus goes into a filter being
    /// typed, or nowhere. Gives whether the sidebar took it.
    pub fn sidebar_paste(&mut self, client: ClientId, pasted: &str) -> bool {
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return false;
        };
        if !self.sidebar.focused(&session) {
            return false;
        }
        let side = self.sidebar.side(&session);
        if let Some(f) = side.filter.as_mut().filter(|f| f.typing) {
            f.text.push_str(&pasted.replace(['\n', '\r'], " "));
            self.sidebar_refilter(&session);
        }
        true
    }

    /// The cursor to the first tab a changed filter leaves.
    fn sidebar_refilter(&mut self, session: &str) {
        let first = self.sidebar_order(session).first().copied();
        let here = self.session(session).map(|s| s.current);
        if let Some(t) = first.or(here) {
            self.sidebar.side(session).cursor = Some(Owner::Tab(t));
        }
        self.dirty_session(session);
    }

    /// Moves the cursor to the `to`th stop: a tab, which comes to the front
    /// with the focus still in the sidebar, or past the last tab, one of the
    /// notifications.
    fn sidebar_move(&mut self, session: &str, to: isize) {
        let order = self.sidebar_order(session);
        let notices: Vec<Owner> = self
            .sidebar_notices()
            .into_iter()
            .map(|n| n.owner)
            .collect();
        let typing = self
            .sidebar
            .sides
            .get(session)
            .and_then(|s| s.filter.as_ref())
            .is_some_and(|f| f.typing);
        // A folded Other repos is a stop of its own after the last tab.
        let fold = !typing && self.sidebar_fold_stop(session);
        let stops = order.len() + usize::from(fold);
        if to >= stops as isize && !notices.is_empty() && !typing {
            let i = ((to - stops as isize) as usize).min(notices.len() - 1);
            self.sidebar.side(session).cursor = Some(notices[i].clone());
            // Onto a notification, the Notifications window comes to the
            // front on it, as a tab would (notice_list setting).
            if settings::is_on("notice_list") {
                self.show_notices(session, &notices[i]);
            }
            self.dirty_session(session);
            return;
        }
        if fold && to >= order.len() as isize {
            self.sidebar.side(session).cursor = Some(Owner::OtherRepos);
            self.dirty_session(session);
            return;
        }
        if order.is_empty() {
            return;
        }
        let tab = order[to.clamp(0, order.len() as isize - 1) as usize];
        self.sidebar.side(session).cursor = Some(Owner::Tab(tab));
        if self.session(session).is_some_and(|s| s.current != tab) {
            self.select_tab(session, tab);
        }
        self.dirty_session(session);
    }

    /// Tab and Shift-Tab on the notifications with the Notifications
    /// window: the next or last of them one by one, the window following; up
    /// from the first goes back to the last tab, and down from the last stays.
    fn sidebar_notice_step(&mut self, session: &str, tabs: usize, cursor: &Owner, by: isize) {
        let entries = self.notice_entries();
        let at = entries
            .iter()
            .position(|(o, _)| o == cursor)
            .unwrap_or_else(|| self.notice_cursor(session, &entries)) as isize;
        let to = at + by;
        if to < 0 {
            self.sidebar_move(session, tabs as isize - 1);
            return;
        }
        if let Some((owner, _)) = entries.get(to as usize) {
            self.sidebar_notice_to(session, owner);
        }
    }

    /// j and k with UX 2.0, whose notifications sit above the tabs: up
    /// from the first tab onto the last kind of notification, down from the
    /// last kind onto the first tab, and no further either way, a kind's row
    /// at a time, the Notifications window on that kind's first.
    fn sidebar_step_ux2(
        &mut self,
        session: &str,
        tabs: usize,
        on_notice: Option<usize>,
        at: isize,
        by: isize,
    ) {
        let notices = self.sidebar_notices().len();
        match on_notice {
            None if by < 0 && at == 0 && notices > 0 => {
                self.sidebar_move(session, (tabs + notices) as isize - 1);
            }
            None => self.sidebar_move(session, (at + by).clamp(0, tabs.max(1) as isize - 1)),
            Some(i) => {
                let to = i as isize + by;
                if to >= notices as isize {
                    self.sidebar_move(session, 0);
                } else if to >= 0 {
                    self.sidebar_move(session, tabs as isize + to);
                }
            }
        }
    }

    /// The sidebar's cursor and the Notifications window's onto one
    /// notification.
    fn sidebar_notice_to(&mut self, session: &str, owner: &Owner) {
        let w = &mut self.sidebar.windows;
        w.at.insert(session.to_string(), owner.clone());
        w.back.remove(session);
        self.sidebar.side(session).cursor = Some(owner.clone());
        self.dirty_session(session);
    }

    /// Goes to a tab from the sidebar, the focus into its pane.
    fn sidebar_go(&mut self, session: &str, tab: TabId) {
        // A tab of Other repos gone to (a / search's match) keeps the
        // group unfolded, so the tab in front stays listed.
        if self.sidebar_in_other_repos(session, tab) {
            self.sidebar.side(session).repo_open = true;
        }
        self.sidebar_leave(session);
        if let Some(s) = self.session_of_tab(tab).map(|s| s.name.clone()) {
            self.select_tab(&s, tab);
        }
    }

    fn sidebar_key_in(&mut self, client: ClientId, session: &str, key: Key, raw: &[u8]) {
        let order = self.sidebar_order(session);
        let notices: Vec<Owner> = self
            .sidebar_notices()
            .into_iter()
            .map(|n| n.owner)
            .collect();
        let Some(here) = self.session(session).map(|s| s.current) else {
            return;
        };
        // Any key puts out what the mouse lit.
        if self.sidebar.hover.remove(&client).is_some() {
            self.dirty_session(session);
        }
        let side = self.sidebar.side(session).clone();
        let cursor = side.cursor.clone().unwrap_or(Owner::Tab(here));
        // A kind's row holds the cursor while it is on any of its
        // notifications, as the Notifications window goes through them.
        let on_notice = notices.iter().position(|o| o.same_row(&cursor));
        let by_item = on_notice.is_some() && settings::is_on("notice_list");
        // A folded Other repos is a stop after the last tab, before the
        // notifications.
        let typing = side.filter.as_ref().is_some_and(|f| f.typing);
        let fold = !typing && self.sidebar_fold_stop(session);
        let stops = order.len() + usize::from(fold);
        let on_fold = fold && cursor == Owner::OtherRepos;
        let at = match on_notice {
            Some(i) => stops + i,
            None if on_fold => order.len(),
            None => order
                .iter()
                .position(|t| Owner::Tab(*t) == cursor)
                .unwrap_or(0),
        } as isize;
        let k = key.normalized();
        let none = k.mods.is_empty();
        let ctrl = k.mods == Mods::CTRL;
        let tab_focus = settings::is_on("tab_focus");
        let chosen = order.get(at.max(0) as usize).copied();

        // Typing a / filter: letters go into it and the cursor to the first
        // tab it leaves; the arrows move, Enter goes to the chosen tab, and
        // Esc clears it.
        if side.filter.as_ref().is_some_and(|f| f.typing) {
            match k.code {
                KeyCode::Up if none => self.sidebar_move(session, at - 1),
                KeyCode::Down if none => self.sidebar_move(session, at + 1),
                KeyCode::Char('p') if ctrl => self.sidebar_move(session, at - 1),
                KeyCode::Char('n') if ctrl => self.sidebar_move(session, at + 1),
                KeyCode::Tab | KeyCode::BackTab if tab_focus && !order.is_empty() => {
                    let n = order.len() as isize;
                    let by = if k.code == KeyCode::Tab { 1 } else { -1 };
                    self.sidebar_move(session, (at + by).rem_euclid(n));
                }
                KeyCode::Enter => {
                    let go = order
                        .iter()
                        .find(|t| Owner::Tab(**t) == cursor)
                        .or(order.first())
                        .copied();
                    if let Some(t) = go {
                        self.sidebar_go(session, t);
                    } else {
                        self.sidebar.side(session).filter = None;
                        self.dirty_session(session);
                    }
                }
                KeyCode::Esc => {
                    self.sidebar.side(session).filter = None;
                    self.sidebar_refilter(session);
                }
                KeyCode::Char('c') if ctrl => {
                    self.sidebar.side(session).filter = None;
                    self.sidebar_refilter(session);
                }
                KeyCode::Backspace => {
                    if let Some(f) = self.sidebar.side(session).filter.as_mut() {
                        f.text.pop();
                    }
                    self.sidebar_refilter(session);
                }
                KeyCode::Char(c) if none => {
                    if let Some(f) = self.sidebar.side(session).filter.as_mut() {
                        f.text.push(c);
                    }
                    self.sidebar_refilter(session);
                }
                _ => {}
            }
            return;
        }

        let ux2 = settings::is_on("ux2");
        let up_down = match k.code {
            KeyCode::Char('j') | KeyCode::Down if none => Some(1),
            KeyCode::Char('n') if ctrl => Some(1),
            KeyCode::Char('k') | KeyCode::Up if none => Some(-1),
            KeyCode::Char('p') if ctrl => Some(-1),
            _ => None,
        };
        if ux2 && let Some(by) = up_down {
            self.sidebar_step_ux2(session, stops, on_notice, at, by);
            return;
        }
        // On a folded Other repos, Enter, l or the right arrow unfolds it.
        if on_fold
            && (matches!(k.code, KeyCode::Right | KeyCode::Char('l')) && none
                || k.code == KeyCode::Enter)
        {
            self.sidebar_fold(session, false);
            return;
        }
        match k.code {
            // On the notifications, j and k step a kind at a time, the
            // Notifications window following onto the kind's first.
            KeyCode::Char('j') | KeyCode::Down if none => self.sidebar_move(session, at + 1),
            KeyCode::Char('n') if ctrl => self.sidebar_move(session, at + 1),
            KeyCode::Char('k') | KeyCode::Up if none => self.sidebar_move(session, at - 1),
            KeyCode::Char('p') if ctrl => self.sidebar_move(session, at - 1),
            KeyCode::Char('g') | KeyCode::Home if none => self.sidebar_move(session, 0),
            KeyCode::Char('G') | KeyCode::End if none => {
                self.sidebar_move(session, order.len() as isize - 1)
            }
            KeyCode::Char('d' | 'u') if ctrl => {
                self.sidebar_half_page(client, session, k, raw, &order, on_notice.is_some())
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l')
                if none || k.code == KeyCode::Enter =>
            {
                if on_notice.is_some() && settings::is_on("notice_list") {
                    // Into the Notifications window, as Enter goes into a
                    // tab's pane.
                    self.notices_enter(session, &cursor);
                } else if on_notice.is_some() {
                    self.sidebar_open_notice(client, &cursor);
                } else if let Some(t) = chosen {
                    self.sidebar_go(session, t);
                }
            }
            KeyCode::Char('/') if none => {
                let text = side.filter.map(|f| f.text).unwrap_or_default();
                self.sidebar.side(session).filter = Some(Filter { text, typing: true });
                self.sidebar_refilter(session);
            }
            KeyCode::Esc if none && side.filter.is_some() => {
                // Esc clears a kept filter before it backs out.
                self.sidebar.side(session).filter = None;
                self.sidebar_refilter(session);
            }
            KeyCode::Esc | KeyCode::Left if none => self.sidebar_leave(session),
            KeyCode::Char('h' | 'q') if none => self.sidebar_leave(session),
            KeyCode::Char('c') if ctrl => self.sidebar_leave(session),
            // Tab and Shift-Tab go round its tabs and notifications, as on a
            // webpage, the last wrapping to the first, one notification at a
            // time with the Notifications window; with tab_focus off, Tab
            // opens mission control, as M-Space does.
            KeyCode::Tab if tab_focus => {
                let entries = self.notice_entries();
                let last = match on_notice {
                    Some(_) if by_item => {
                        entries.iter().position(|(o, _)| *o == cursor)
                            == Some(entries.len().saturating_sub(1))
                    }
                    Some(i) => i + 1 >= notices.len(),
                    None => at + 1 >= stops as isize && notices.is_empty(),
                };
                if last {
                    self.sidebar_move(session, 0);
                } else if by_item {
                    self.sidebar_notice_step(session, stops, &cursor, 1);
                } else {
                    self.sidebar_move(session, at + 1);
                }
            }
            KeyCode::Tab => {
                self.sidebar_leave(session);
                self.open_mission(client, crate::mission::Opening::Tabs);
            }
            KeyCode::BackTab if tab_focus => {
                let last = stops as isize + notices.len() as isize - 1;
                let entries = self.notice_entries();
                if by_item {
                    self.sidebar_notice_step(session, stops, &cursor, -1);
                } else if on_notice.is_some() || at != 0 {
                    self.sidebar_move(session, at - 1);
                } else {
                    // From the first tab round to the last notification,
                    // the last one by one with the Notifications window.
                    self.sidebar_move(session, last);
                    if settings::is_on("notice_list")
                        && let Some((owner, _)) = entries.last()
                    {
                        self.sidebar_notice_to(session, owner);
                    }
                }
            }
            KeyCode::Char(' ') if none => {
                if let Some(t) = chosen.filter(|_| on_notice.is_none()) {
                    self.sidebar_peek(client, t);
                }
            }
            // Its Claude answered from here, in mission control's box, as
            // the sidebar's `a` answered the chosen tab in a popup.
            KeyCode::Char('a') if none => {
                if let Some(t) = chosen.filter(|_| on_notice.is_none()) {
                    self.open_answer(client, t, None);
                }
            }
            KeyCode::Char('n') if none => {
                if notices.is_empty() {
                    self.say(Some(client), "No notifications");
                } else {
                    self.sidebar_move(session, stops as isize);
                }
            }
            // d and L: the chosen tab gone to, its diff or lazygit in its
            // pane's place, as the tmux version's sidebar and mission
            // control's menu on a tab do.
            KeyCode::Char(c @ ('d' | 'L')) if none => {
                if let Some(t) = chosen.filter(|_| on_notice.is_none()) {
                    self.sidebar_go(session, t);
                    self.tool_in_front(client, c == 'L');
                }
            }
            // t: a new tab running Claude, as M-t opens one, from the chosen
            // tab, so its directory comes first.
            KeyCode::Char('t') if none => {
                if let Some(t) = chosen {
                    self.sidebar_go(session, t);
                    self.act(client, Action::NewClaudeTab);
                }
            }
            // i: its whole title in a box, as M-T shows it.
            KeyCode::Char('i') if none && settings::is_on("tab_title") => {
                if let Some(t) = chosen.filter(|_| on_notice.is_none()) {
                    self.show_tab_title(client, t);
                }
            }
            KeyCode::Char('?') if none => {
                let rows = KEYS
                    .iter()
                    .map(|(k, what)| (k.to_string(), what.to_string()))
                    .collect();
                self.open(
                    client,
                    Box::new(crate::ui::keylist::KeyList::new(rows)),
                    None,
                );
            }
            // A number puts the cursor on that tab, in the sidebar, as j and
            // k do; one the sidebar doesn't list (filtered out) is gone to.
            KeyCode::Char(c @ '1'..='9') if none => {
                let n = c.to_digit(10).unwrap_or(0) as usize;
                let tabs = self.sidebar_tabs(session);
                let target = tabs.iter().find(|t| t.index == n).map(|t| t.id);
                // One folded under Other repos unfolds it, the cursor
                // onto it in the sidebar.
                let folded_away = target.is_some_and(|t| {
                    !order.contains(&t) && self.sidebar_in_other_repos(session, t)
                });
                let order = if folded_away {
                    self.sidebar.side(session).repo_open = true;
                    self.sidebar_order(session)
                } else {
                    order.clone()
                };
                match target.and_then(|id| order.iter().position(|t| *t == id)) {
                    Some(i) => self.sidebar_move(session, i as isize),
                    None => {
                        if let Some(t) = target {
                            self.sidebar_go(session, t);
                        }
                    }
                }
            }
            // Any other Alt or Ctrl key runs in the chosen tab, as if pressed
            // there.
            _ if k.mods.alt || k.mods.ctrl => {
                if let Some(t) = chosen {
                    self.sidebar_go(session, t);
                    self.press_key(client, key, raw);
                }
            }
            _ => {}
        }
    }

    /// C-d and C-u: the list half a screen on, and the cursor onto the tab
    /// there, as in Vim, stopping at the first and last tab rather than going
    /// on into the notifications; from a notification, C-u goes back to the
    /// last tab. With sidebar_half_page off, the key runs in the chosen tab.
    fn sidebar_half_page(
        &mut self,
        client: ClientId,
        session: &str,
        key: Key,
        raw: &[u8],
        order: &[TabId],
        on_notice: bool,
    ) {
        let down = key.code == KeyCode::Char('d');
        let cursor = self.sidebar.side(session).cursor.clone();
        if !settings::is_on("sidebar_half_page") {
            let at = cursor
                .as_ref()
                .and_then(Owner::tab)
                .or(order.first().copied());
            if let Some(t) = at {
                self.sidebar_go(session, t);
                self.press_key(client, key, raw);
            }
            return;
        }
        if order.is_empty() {
            return;
        }
        if on_notice {
            if !down {
                self.sidebar_move(session, order.len() as isize - 1);
            }
            return;
        }
        let Some(scroll) = self.sidebar.drawn.get(&client).map(|d| d.scroll.clone()) else {
            return;
        };
        let by = (scroll.room / 2).max(1) as isize * if down { 1 } else { -1 };
        let most = scroll.length.saturating_sub(scroll.room) as isize;
        self.sidebar.side(session).top = Some((scroll.top as isize + by).clamp(0, most) as usize);
        // The tab on the line half a screen on, else the nearest one past
        // it, never the tab the cursor is on, which a tab taller than half
        // the list would otherwise hold it on press after press.
        let here = cursor.as_ref().and_then(Owner::tab);
        let line = scroll.cursor_at as isize + by;
        let found = if down {
            scroll
                .owners
                .iter()
                .skip(line.max(0) as usize)
                .flatten()
                .find(|t| Some(**t) != here)
                .copied()
        } else {
            scroll
                .owners
                .iter()
                .take((line + 1).max(0) as usize)
                .rev()
                .flatten()
                .find(|t| Some(**t) != here)
                .copied()
        };
        let id = found.unwrap_or(if down {
            order[order.len() - 1]
        } else {
            order[0]
        });
        let to = order.iter().position(|t| *t == id).unwrap_or(0);
        self.sidebar_move(session, to as isize);
    }

    /// A click on the ▲ or ▼ more row: the list half a screen up or down,
    /// the cursor with it, as C-u and C-d, whether or not a filter is being
    /// typed, the focus coming into the sidebar as the wheel brings it;
    /// nothing with sidebar_half_page off.
    fn sidebar_scroll_click(&mut self, client: ClientId, session: &str, down: bool) {
        if !settings::is_on("sidebar_half_page") {
            return;
        }
        if !self.sidebar.focused(session) {
            self.sidebar_enter(session, None);
        }
        let order = self.sidebar_order(session);
        let on_notice = self
            .sidebar
            .side(session)
            .cursor
            .as_ref()
            .is_some_and(|c| matches!(c, Owner::Notice { .. }));
        let key = Key::new(KeyCode::Char(if down { 'd' } else { 'u' }), Mods::CTRL);
        self.sidebar_half_page(client, session, key, &[], &order, on_notice);
    }

    /// Presses a key as if the sidebar weren't there: in the tab's pane, or
    /// whatever it is bound to.
    fn press_key(&mut self, client: ClientId, key: Key, raw: &[u8]) {
        let parsed = crate::mux::input::Parsed {
            input: crate::mux::input::Input::Key(key),
            raw: raw.to_vec(),
        };
        self.dispatch(client, parsed);
    }

    // ── The repository filter ───────────────────────────────────────────────

    /// Filters a session's sidebar to one repository (a main clone's
    /// directory), every other one's tabs folded under Other repos, or
    /// clears the filter with `None`. While the sidebar has the focus, the
    /// tabs are held where the filter puts them, and a cursor left on what
    /// folded away goes back to the tab in front.
    pub(crate) fn sidebar_set_repo(&mut self, session: &str, repo: Option<std::path::PathBuf>) {
        let side = self.sidebar.side(session);
        side.repo = repo;
        side.repo_open = false;
        if side.focused {
            let frozen = model::frozen_from(&self.sidebar_marked(session), &Rules::now());
            self.sidebar.side(session).frozen = Some(frozen);
        }
        self.sidebar_cursor_back(session);
        self.dirty_session(session);
    }

    /// M-p's "Filter the sidebar to a repository": a picker of the
    /// repositories the session's tabs are in, each with how many tabs,
    /// "Every repository" first to clear it, the one filtered to now
    /// marked; picking one filters to it and says so.
    pub(crate) fn sidebar_pick_repo(&mut self, client: ClientId) {
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        let now = self
            .sidebar_repo(&session)
            .map(std::path::Path::to_path_buf);
        let mut repos: Vec<(std::path::PathBuf, usize)> = Vec::new();
        for t in self.sidebar_tabs(&session) {
            let Some(dir) = t.repo_dir else { continue };
            match repos.iter_mut().find(|(d, _)| *d == dir) {
                Some((_, n)) => *n += 1,
                None => repos.push((dir, 1)),
            }
        }
        let name = |d: &std::path::Path| {
            d.file_name()
                .unwrap_or(d.as_os_str())
                .to_string_lossy()
                .into_owned()
        };
        repos.sort_by_key(|(d, _)| name(d).to_lowercase());
        let mut items = vec![PickItem {
            text: "Every repository".into(),
            tags: if now.is_none() {
                "now".into()
            } else {
                String::new()
            },
            ..PickItem::default()
        }];
        for (dir, n) in &repos {
            items.push(PickItem {
                text: format!(
                    "{}  {n} {}",
                    name(dir),
                    if *n == 1 { "tab" } else { "tabs" }
                ),
                tags: if now.as_ref() == Some(dir) {
                    "now".into()
                } else {
                    String::new()
                },
                value: dir.to_string_lossy().into_owned(),
                ..PickItem::default()
            });
        }
        let picker = Picker::new("Filter the sidebar to a repository", items)
            .named("sidebar-repo")
            .prompt("repository> ");
        self.pick(client, picker, move |server, client, picked| {
            let Some(picked) = picked else {
                return;
            };
            if picked.value.is_empty() {
                server.sidebar_set_repo(&session, None);
                server.say(Some(client), "Sidebar: every repository");
                return;
            }
            let dir = std::path::PathBuf::from(&picked.value);
            let said = format!("Sidebar: {} only", name(&dir));
            server.sidebar_set_repo(&session, Some(dir));
            server.say(Some(client), &said);
        });
    }

    /// Folds a session's Other repos to one line, or unfolds it, the
    /// cursor onto its first tab as it unfolds and onto its line as it
    /// folds over the cursor.
    fn sidebar_fold(&mut self, session: &str, fold: bool) {
        self.sidebar.side(session).repo_open = !fold;
        // Unfolded under the cursor, its first tab comes to the front as
        // the cursor reaches it, as j and k bring a tab.
        if !fold && self.sidebar.side(session).cursor == Some(Owner::OtherRepos) {
            let order = self.sidebar_order(session);
            let first = order
                .iter()
                .position(|id| self.sidebar_in_other_repos(session, *id));
            if let Some(at) = first {
                self.sidebar_move(session, at as isize);
            }
        }
        if fold {
            let on_folded = self
                .sidebar
                .side(session)
                .cursor
                .as_ref()
                .and_then(Owner::tab)
                .is_some_and(|t| !self.sidebar_order(session).contains(&t));
            if on_folded {
                self.sidebar.side(session).cursor = Some(Owner::OtherRepos);
            }
        }
        self.dirty_session(session);
    }

    /// The cursor back to the tab in front when it is on a tab or a line
    /// the sidebar no longer lists.
    fn sidebar_cursor_back(&mut self, session: &str) {
        let order = self.sidebar_order(session);
        let fold = self.sidebar_fold_stop(session);
        let filtered = self.sidebar_repo(session).is_some();
        let gone = match self.sidebar.side(session).cursor.as_ref() {
            Some(Owner::Tab(t)) => filtered && !order.contains(t),
            Some(Owner::OtherRepos) => !fold,
            _ => false,
        };
        if !gone {
            return;
        }
        // A tab folded away leaves the cursor on Other repos' line.
        let on_tab = matches!(self.sidebar.side(session).cursor, Some(Owner::Tab(_)));
        if fold && on_tab {
            self.sidebar.side(session).cursor = Some(Owner::OtherRepos);
        } else if let Some(here) = self.session(session).map(|s| s.current) {
            self.sidebar.side(session).cursor = Some(Owner::Tab(here));
        }
    }

    // ── The mouse ───────────────────────────────────────────────────────────

    /// A mouse event, before the panes see it: on the sidebar, a click goes
    /// to a tab (a right click peeks at it) or, on the ▲ or ▼ more row,
    /// scrolls half a screen, the wheel moves the cursor and
    /// the mouse lights the row it is over (hover_highlight); a press
    /// anywhere else takes the focus out of the sidebar. Drags and releases
    /// are the panes'. Gives whether the sidebar took it.
    pub fn sidebar_mouse(&mut self, client: ClientId, m: Mouse) -> bool {
        if matches!(m.kind, MouseKind::Drag | MouseKind::Release) {
            return false;
        }
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return false;
        };
        let drawn = self.sidebar.drawn.get(&client).cloned();
        let inside = drawn
            .as_ref()
            .is_some_and(|d| m.col <= d.area.right() && m.row < d.area.bottom());
        let Some(d) = drawn.filter(|_| inside) else {
            if self.sidebar.hover.remove(&client).is_some()
                && let Some(c) = self.clients.get_mut(&client)
            {
                c.dirty = true;
            }
            if m.kind == MouseKind::Press && !m.button.is_wheel() {
                self.sidebar_leave(&session);
            }
            return false;
        };
        let owner = (m.col < d.area.right())
            .then(|| d.owners.get((m.row - d.area.y) as usize).cloned().flatten())
            .flatten();
        match (m.kind, m.button) {
            (MouseKind::Move, _) => {
                let lit = owner.filter(|_| settings::is_on("hover_highlight"));
                let changed = self.sidebar.hover.get(&client) != lit.as_ref();
                match lit {
                    Some(o) => {
                        self.sidebar.hover.insert(client, o);
                    }
                    None => {
                        self.sidebar.hover.remove(&client);
                    }
                }
                if changed && let Some(c) = self.clients.get_mut(&client) {
                    c.dirty = true;
                }
            }
            (MouseKind::Press, MouseButton::WheelUp | MouseButton::WheelDown) => {
                // The wheel browses, as j and k do, the focus coming into
                // the sidebar.
                if !self.sidebar.focused(&session) {
                    self.sidebar_enter(&session, None);
                }
                let by = if m.button == MouseButton::WheelUp {
                    Key::new(KeyCode::Up, Mods::NONE)
                } else {
                    Key::new(KeyCode::Down, Mods::NONE)
                };
                self.sidebar_key_in(client, &session, by, &[]);
            }
            (MouseKind::Press, MouseButton::Left) => match owner {
                Some(Owner::Tab(t)) => self.sidebar_go(&session, t),
                // A notification clicked: the Notifications window, on it.
                Some(notice @ Owner::Notice { .. }) if settings::is_on("notice_list") => {
                    self.sidebar_enter(&session, Some(notice.clone()));
                    self.show_notices(&session, &notice);
                }
                Some(notice @ Owner::Notice { .. }) => self.sidebar_open_notice(client, &notice),
                // The ▲ and ▼ more rows scroll half a screen, as C-u and C-d.
                Some(Owner::ScrollUp) => self.sidebar_scroll_click(client, &session, false),
                Some(Owner::ScrollDown) => self.sidebar_scroll_click(client, &session, true),
                // The filter's chip clears it; Other repos folds or unfolds.
                Some(Owner::RepoChip) => {
                    self.sidebar_set_repo(&session, None);
                    self.say(Some(client), "Sidebar: every repository");
                }
                // Folded, a click unfolds it as Enter on its line does, the
                // focus coming into the sidebar and its first tab to the
                // front; unfolded, a click on its heading folds it.
                Some(Owner::OtherRepos) => {
                    let open = self
                        .sidebar
                        .sides
                        .get(&session)
                        .is_some_and(|s| s.repo_open);
                    if !open {
                        self.sidebar_enter(&session, Some(Owner::OtherRepos));
                    }
                    self.sidebar_fold(&session, open);
                }
                None => {}
            },
            (MouseKind::Press, MouseButton::Right) => {
                if let Some(Owner::Tab(t)) = owner {
                    self.sidebar_peek(client, t);
                }
            }
            _ => {}
        }
        true
    }

    // ── What it opens ───────────────────────────────────────────────────────

    /// A look at a tab without going there; Enter in it goes there.
    fn sidebar_peek(&mut self, client: ClientId, tab: TabId) {
        self.open_peek(client, tab, None, false);
    }

    /// A look at a tab, from the sidebar or mission control (`mission`,
    /// whose Enter goes there as its menus do): its last turn, or its screen
    /// with `SCROLLBACK` lines above it, on `mode` when given. Its `r` asks
    /// for a new look at the screen, which is read again here.
    pub(crate) fn open_peek(
        &mut self,
        client: ClientId,
        tab: TabId,
        mode: Option<super::peek::Mode>,
        mission: bool,
    ) {
        let Some(session) = self.session_of_tab(tab).map(|s| s.name.clone()) else {
            return;
        };
        let Some(model) = self
            .sidebar_tabs(&session)
            .into_iter()
            .find(|t| t.id == tab)
        else {
            return;
        };
        let pane = model.claude_pane.or(model.active);
        let screen = pane
            .and_then(|p| self.panes.get(&p))
            .map(|slot| slot.pane.text(super::peek::SCROLLBACK))
            .unwrap_or_default();
        let turn = model.claude.as_ref().and(model.turn.clone());
        let mut peek = Peek::new(
            format!("{} {}", model.index, model.name),
            turn,
            model.state.busy(),
            screen,
        );
        if let Some(mode) = mode {
            peek = peek.mode(mode);
        }
        self.open(
            client,
            Box::new(peek),
            Some(Box::new(
                move |server: &mut Server, client, outcome| match outcome {
                    Outcome::Yes if mission => server.mission_go(client, tab),
                    Outcome::Yes => server.sidebar_go(&session, tab),
                    Outcome::No => {
                        server.open_peek(client, tab, Some(super::peek::Mode::Screen), mission)
                    }
                    _ => {}
                },
            )),
        );
    }

    /// A kind of notification, chosen with the Notifications window off
    /// (daemon.js's `openNotice`): the next meeting joins its call (with
    /// none, the Calendar view opens), the newest note written for you
    /// opens in Notes, a calendar change opens the Calendar view, a
    /// suggestion Settings, autopilot mission control's tabs,
    /// where its line is, and the others mission control on their view.
    fn sidebar_open_notice(&mut self, client: ClientId, owner: &Owner) {
        let Owner::Notice { source, id } = owner else {
            return;
        };
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        let item = self
            .notices
            .get(source)
            .and_then(|items| items.iter().find(|i| &i.id == id).or(items.first()))
            .cloned();
        self.sidebar_leave(&session);
        let view = |slug: &str| crate::mission::Opening::View(slug.to_string());
        match source.as_str() {
            "new-notes" => {
                if let Some(open) = item
                    .as_ref()
                    .and_then(|i| crate::mission::choices::notice_opens(source, i))
                {
                    self.mission_do(client, open, "Opening it");
                }
            }
            "meeting" => match item.map(|i| i.target) {
                Some(Target::Url(url)) => self.open_link(client, url),
                _ => self.open_mission(client, view("calendar")),
            },
            "calendar" => self.open_mission(client, view("calendar")),
            "habits" => self.open_mission(client, crate::mission::Opening::Settings),
            "autopilot" => self.open_mission(client, crate::autopilot::opening()),
            // The landed tabs closed unseen are only to be read: Enter
            // dismisses them.
            "landed" => self.autopilot_dismiss_closed(None),
            // The newest landing made without asking: its tab, while it is
            // open, which dismisses it.
            "autolanded" => {
                let Some(last) = self
                    .notices
                    .get("autolanded")
                    .and_then(|i| i.last())
                    .cloned()
                else {
                    return;
                };
                self.autopilot_dismiss_autolanded(Some(&last.id));
                match self.autopilot_landed_tab(&last) {
                    Some(tab) => self.mission_go(client, tab),
                    None => self.say(
                        Some(client),
                        &format!("Landed {}; its tab has closed", last.id),
                    ),
                }
            }
            other => self.open_mission(client, view(other)),
        }
    }

    /// The view mission control shows a kind of notification in, while that
    /// view is on (goToNotices), for "Go to the notifications" with no
    /// sidebar to go to.
    fn notice_view(source: &str) -> Option<crate::mission::Opening> {
        let on = settings::is_on;
        let view = |slug: &str| Some(crate::mission::Opening::View(slug.to_string()));
        match source {
            "slack" if on("view_slack") => view("slack"),
            "inbox" if on("linear") && on("view_inbox") => view("inbox"),
            "prs" if crate::sources::enabled(crate::sources::PRS) => view("prs"),
            "meeting" | "calendar" if on("view_calendar") => view("calendar"),
            "habits" => Some(crate::mission::Opening::Settings),
            "autopilot" | "autolanded" => Some(crate::autopilot::opening()),
            _ => None,
        }
    }

    /// The cursor onto the first notification, in the sidebar of the
    /// client's session, the Notifications window coming to the front on it
    /// (goToNotices); with notice_list off and no sidebar there, it says
    /// why.
    pub(crate) fn sidebar_to_notices(&mut self, client: ClientId) {
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        let Some(first) = self.sidebar_notices().into_iter().next() else {
            self.say(Some(client), "No notifications");
            return;
        };
        // With the Notifications window, it comes to the front on the
        // first, browsed from the sidebar, or from its own keys with no
        // sidebar to browse from.
        if settings::is_on("notice_list") {
            if self.sidebar_shown(&session) {
                self.sidebar.side(&session).filter = None;
                self.sidebar_enter(&session, Some(first.owner.clone()));
                self.show_notices(&session, &first.owner);
            } else {
                self.notices_enter(&session, &first.owner);
            }
            return;
        }
        if !self.sidebar_shown(&session) {
            // The first one's view, when that view is on; else why there is
            // nowhere to show them.
            let source = match &first.owner {
                Owner::Notice { source, .. } => source.as_str(),
                _ => "",
            };
            if let Some(view) = Self::notice_view(source) {
                self.open_mission(client, view);
                return;
            }
            let why = if settings::is_on("sidebar") {
                "The notifications are in the sidebar, which this window is too narrow for; Settings' Notifications window shows them in a tab of their own"
            } else {
                "The notifications are in the sidebar, which is off; Settings' Notifications window shows them in a tab of their own"
            };
            self.say(Some(client), why);
            return;
        }
        self.sidebar.side(&session).filter = None;
        self.sidebar_enter(&session, Some(first.owner));
    }

    // ── M-a ─────────────────────────────────────────────────────────────────

    /// What M-a walks, in its order: the tabs under Needs you, then the
    /// notifications while there are any (needs_notices setting).
    fn sidebar_needs_stops(&self, all: &[TabModel]) -> Vec<Stop> {
        let r = Rules::now();
        let mut stops: Vec<Stop> = model::needs_you(all, &r)
            .iter()
            .map(|t| Stop {
                place: Place::Tab(t.id),
                why: if t.pinned {
                    Why::Pinned
                } else if model::reminder_due(t, &r) {
                    Why::Reminder
                } else if t.state == TabState::Waiting {
                    Why::Waiting
                } else {
                    Why::Done
                },
            })
            .collect();
        if settings::is_on("needs_notices") && !self.sidebar_notices().is_empty() {
            stops.push(Stop {
                place: Place::Notices,
                why: Why::Notices,
            });
        }
        stops
    }

    /// The tab M-a would go to next from `here`, its number and name, from
    /// the same walk without taking it (UX 2.0's header says it): none when
    /// it would go to the notifications, or nowhere else.
    pub fn sidebar_needs_next(&self, here: TabId) -> Option<(usize, String)> {
        let all = self.sidebar_walked_tabs();
        let stops = self.sidebar_needs_stops(&all);
        let Next::Go(i) = self.sidebar.walk.clone().next(&stops, here) else {
            return None;
        };
        let Place::Tab(t) = stops[i].place else {
            return None;
        };
        all.iter()
            .find(|m| m.id == t && t != here)
            .map(|m| (m.index, m.name.clone()))
    }

    /// M-a: the next of what needs you, in the sidebar's order, across every
    /// session: the tabs under Needs you, then the notifications while there
    /// are any (needs_notices setting). Pressed from the sidebar, the cursor
    /// stays in a sidebar, on the tab it went to.
    pub fn sidebar_next_needs_you(&mut self, client: ClientId) {
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        let Some(here) = self.session(&session).map(|s| s.current) else {
            return;
        };
        let all = self.sidebar_walked_tabs();
        let stops = self.sidebar_needs_stops(&all);
        let side = self.sidebar.sides.get(&session).filter(|s| s.focused);
        let from_sidebar = side.is_some();
        let on_notices = side
            .and_then(|s| s.cursor.as_ref())
            .is_some_and(|c| matches!(c, Owner::Notice { .. }));
        match self.sidebar.walk.next(&stops, here) {
            Next::Nothing => self.say(Some(client), "Nothing needs you."),
            Next::OnlyHere => self.say(Some(client), "This is the only tab that needs you."),
            Next::Go(i) => match stops[i].place {
                Place::Notices => self.sidebar_to_notices(client),
                Place::Tab(t) => {
                    let Some(tab) = all.iter().find(|m| m.id == t) else {
                        return;
                    };
                    // The tab's pane first, so switching sessions lands on
                    // it rather than on whatever was last focused there.
                    if let Some(pane) = tab.claude_pane.or(tab.active) {
                        self.focus_pane(pane);
                    }
                    if tab.session != session {
                        self.switch_client(client, &tab.session);
                    }
                    // Back from the notifications in this same tab's
                    // sidebar, the tab's pane takes the focus instead.
                    let back_here = on_notices && t == here;
                    if from_sidebar && !back_here {
                        self.sidebar_enter(&tab.session, Some(Owner::Tab(t)));
                    }
                }
            },
        }
    }

    /// M-A: the next tab under Idle, in the sidebar's order, wrapping at the
    /// end (agent-state.sh's `idle-next` on the list the daemon wrote,
    /// `writeIdle`); pressed from a sidebar, its cursor stays in the new
    /// tab's sidebar, as with M-a.
    pub fn sidebar_next_idle(&mut self, client: ClientId) {
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        let Some(here) = self.session(&session).map(|s| s.current) else {
            return;
        };
        let all = self.sidebar_walked_tabs();
        let idle: Vec<TabId> = model::idle_tabs(&all, &Rules::now())
            .iter()
            .map(|t| t.id)
            .collect();
        let from_sidebar = self.sidebar.focused(&session);
        match super::walk::next_idle(&idle, here) {
            Next::Nothing => self.say(Some(client), "No idle tabs."),
            Next::OnlyHere => self.say(Some(client), "This is the only idle tab."),
            Next::Go(i) => {
                let Some(tab) = all.iter().find(|m| m.id == idle[i]) else {
                    return;
                };
                if let Some(pane) = tab.claude_pane.or(tab.active) {
                    self.focus_pane(pane);
                }
                if tab.session != session {
                    self.switch_client(client, &tab.session);
                }
                if from_sidebar {
                    self.sidebar_enter(&tab.session, Some(Owner::Tab(tab.id)));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_labels_pr_is_split_from_its_name() {
        assert_eq!(split_pr("#12 Fix it"), (Some(12), "Fix it".into()));
        assert_eq!(split_pr("#x y"), (None, "#x y".into()));
        assert_eq!(split_pr("plain"), (None, "plain".into()));
    }
}
