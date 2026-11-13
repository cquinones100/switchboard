//! This part on the server, as the sidebar daemon's timers ran it: the keys
//! pressed and palette picks counted as they happen and written once a
//! minute (`keepHabits`), the suggestions worked out at most every
//! `SUGGEST_EVERY` and shown in the sidebar's notifications under "habits",
//! which pane runs which Claude read every `SURVEY_EVERY`, each tab kept as
//! last seen and logged as it closes, and the plugins run on their
//! intervals.
//!
//! Mission control asks for a view with `want_view` each time it draws it,
//! and reads it with `usage_view`: Resources and Usage are worked
//! out off the server's task, only while one was wanted within `WANT_FOR`,
//! so a view nobody looks at reads no processes and no files. Nothing here
//! starts a process or waits on the server's task, but the plugins', which
//! run in tasks of their own.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use serde_json::Value;

use super::charts;
use super::closed::{OpenTab, TabNow};
use super::habits::{self, Count, Outcome};
use super::report;
use super::resources::{self, TabPanes};
use super::sessions::{self, ClaudeTab};
use super::{UsageEvent, plugins};
use crate::items::ViewData;
use crate::mux::keys::Action;
use crate::mux::server::{ClientId, Event, Server};
use crate::mux::{PaneId, TabId};
use crate::ui::palette::Entry;
use crate::{agent, paths, settings, util};

/// A view mission control showed within this long is kept up to date.
pub const WANT_FOR: Duration = Duration::from_secs(10);
/// How often the counts are written.
const KEEP_EVERY: Duration = Duration::from_secs(60);
/// How often the suggestions are worked out again, since each reads a
/// month of files.
const SUGGEST_EVERY: Duration = Duration::from_secs(10 * 60);
/// How often which pane runs which Claude is read again.
const SURVEY_EVERY: Duration = Duration::from_secs(5);
/// How often the Usage view is read again while in front.
const FILES_EVERY: Duration = Duration::from_secs(4);
/// How often the past Claude sessions are read again while the Reopen view
/// is in front, and how long a look may take.
const SESSIONS_EVERY: Duration = Duration::from_secs(30);
const SESSIONS_TIMEOUT: Duration = Duration::from_secs(30);
/// How long after a tab reopens its `claude --resume` is typed: its shell
/// has drawn its prompt by then (persist.rs's wait).
const TYPE_AFTER: Duration = Duration::from_millis(500);

/// The views this part keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ViewKind {
    Resources,
    Usage,
    Reopen,
}

impl ViewKind {
    /// The view's title, as mission control names it and the habits count it.
    pub fn title(self) -> &'static str {
        match self {
            ViewKind::Resources => "Resources",
            ViewKind::Usage => "Usage",
            ViewKind::Reopen => "Reopen",
        }
    }

    /// The setting that turns it off.
    pub fn setting(self) -> &'static str {
        match self {
            ViewKind::Resources => "view_resources",
            ViewKind::Usage => "usage",
            ViewKind::Reopen => "view_reopen",
        }
    }
}

/// A plugin's view, as mission control and the sidebar list it.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginView {
    pub name: String,
    pub title: String,
    /// How many of its items the sidebar shows under the tabs.
    pub sidebar: usize,
    pub loaded: bool,
    pub data: ViewData,
}

/// The tab option `/review-pr` sets once its sealed try starts: the
/// worktree of the PR it reviews, which a closing keeps.
pub const SEALED_REVIEW: &str = "@sealed_review";
/// Set to 1 beside it when that try fetched the PR's crates (`--fetch`).
pub const SEALED_REVIEW_FETCH: &str = "@sealed_review_fetch";

/// What became of a tab's work, as the Reopen view's chip says it: landed
/// straight on its branch or through its merged PR, its PR's checks
/// failing, or changes asked for.
fn outcome_of(t: &crate::sidebar::model::TabModel) -> String {
    if let Some((hash, _)) = &t.landed_on {
        return format!("landed {hash}");
    }
    let merged = t.merge == Some(crate::sidebar::model::MergeState::Merged)
        || t.pr_state == Some(crate::facts::PrState::Merged);
    if merged {
        return t.pr.map_or("landed".into(), |n| format!("landed #{n}"));
    }
    // Its PR's checks failing, or a landing straight on a branch stopped
    // on its tests (its phase, `land-lock phase tests failed …`).
    let landing_failed = t
        .phase
        .as_deref()
        .is_some_and(|p| p.starts_with("tests failed"));
    if t.checks.is_some_and(|c| c.fail > 0) || landing_failed {
        return "tests failed".into();
    }
    if t.review.as_deref() == Some("CHANGES_REQUESTED") {
        return "changes asked".into();
    }
    String::new()
}

fn setting_value(key: &str) -> Option<Value> {
    settings::cached(key)
}

impl Server {
    // ── The tick ────────────────────────────────────────────────────────────

    /// Everything this part does on the server's tick.
    pub fn usage_tick(&mut self) {
        let now = Instant::now();
        self.usage_survey(now);
        // Every tab is seen again as the survey answers (every
        // `SURVEY_EVERY`), each with its sidebar model, and once more as it
        // closes, rather than on every tick.
        self.usage_habits(now);
        self.usage_past_sessions(now);
        for kind in [ViewKind::Usage, ViewKind::Reopen] {
            if self.usage_wanted(kind, now)
                && self
                    .usage
                    .asked
                    .get(&kind)
                    .is_none_or(|at| now.duration_since(*at) >= FILES_EVERY)
            {
                self.usage_refresh(kind);
            }
        }
        // Resources keeps itself going while wanted (`Refresh`); this starts
        // it again after a pause.
        if self.usage_wanted(ViewKind::Resources, now)
            && !self.usage_in_flight(ViewKind::Resources)
            && self.usage.asked.get(&ViewKind::Resources).is_none_or(|at| {
                now.duration_since(*at) >= Duration::from_millis(resources::RESOURCES_MS) * 2
            })
        {
            self.usage_refresh(ViewKind::Resources);
        }
        self.usage_run_plugins(now);
        let keys_due = self
            .usage
            .keys_asked
            .is_none_or(|at| now.duration_since(at) >= KEEP_EVERY);
        if keys_due
            && self
                .usage
                .keys_wanted
                .is_some_and(|at| now.duration_since(at) < WANT_FOR)
        {
            self.keys_refresh();
        }
    }

    /// What the work off the task answered.
    pub fn usage_event(&mut self, event: UsageEvent) {
        match event {
            UsageEvent::Claudes(found) => {
                self.usage.claudes = found;
                self.usage.surveying = false;
                self.usage_see_tabs();
            }
            UsageEvent::Reading {
                at,
                list,
                procs,
                system,
                system_cpu,
            } => {
                self.usage.reading = Some((at, list, system));
                self.usage.system = system;
                self.usage.system_cpu = system_cpu;
                self.usage.in_flight.insert(ViewKind::Resources, false);
                let tabs = self.usage_tab_panes();
                let usage = resources::tab_usage(&tabs, &procs);
                self.usage.history.add(at, &usage, system_cpu);
                let panes: Vec<u32> = self
                    .panes
                    .values()
                    .filter_map(|slot| slot.pane.pid())
                    .collect();
                let (own_cpu, own_memory) =
                    resources::own_usage(&procs, &panes, std::process::id());
                let top = system.map(|s| resources::Top {
                    cpu: system_cpu,
                    cores: s.cores,
                    memory_used: s.memory_used,
                    memory_total: s.memory_total,
                    own_cpu,
                    own_memory,
                });
                self.usage.procs = Some(procs);
                let data = resources::view(
                    &tabs,
                    self.usage.procs.as_deref(),
                    Some(util::now_ms()),
                    &self.usage.history,
                    top.as_ref(),
                );
                self.usage_set_view(ViewKind::Resources, data);
                if self.usage_wanted(ViewKind::Resources, Instant::now()) {
                    self.send_later(
                        Duration::from_millis(resources::RESOURCES_MS),
                        Event::Usage(UsageEvent::Refresh(ViewKind::Resources)),
                    );
                }
            }
            UsageEvent::Sessions(rows) => {
                self.usage.sessions_busy = false;
                if let Some(rows) = rows {
                    self.usage.sessions = rows;
                    self.usage_refresh(ViewKind::Reopen);
                }
            }
            UsageEvent::View(kind, data) => {
                self.usage.in_flight.insert(kind, false);
                self.usage_set_view(kind, data);
            }
            UsageEvent::Refresh(kind) => {
                if self.usage_wanted(kind, Instant::now()) && !self.usage_in_flight(kind) {
                    self.usage_refresh(kind);
                }
            }
            UsageEvent::Plugin { name, result } => {
                let now = Instant::now();
                if let Some(p) = self.usage.plugins.iter_mut().find(|p| p.name == name) {
                    if let Err(e) = &result {
                        eprintln!("plugin {name}: {e}");
                    }
                    p.answered(result, now, util::now_ms());
                    self.usage_redraw();
                }
            }
            UsageEvent::Suggested(list) => {
                let items = list.iter().map(habits::Suggestion::item).collect();
                self.usage.suggestions = list;
                self.set_notices("habits", items);
            }
            UsageEvent::Keys(counts) => {
                if self.usage.week_keys != counts {
                    self.usage.week_keys = counts;
                    self.usage_redraw();
                }
            }
            UsageEvent::Type { pane, text } => {
                if let Some(slot) = self.panes.get_mut(&pane) {
                    let _ = slot.pane.write(text.as_bytes());
                }
            }
        }
    }

    fn usage_redraw(&mut self) {
        for c in self.clients.values_mut() {
            c.dirty = true;
        }
    }

    // ── The views ───────────────────────────────────────────────────────────

    /// Mission control shows a view: it is kept up to date while shown, and
    /// worked out now when it hasn't been yet.
    pub fn want_view(&mut self, kind: ViewKind) {
        let now = Instant::now();
        let first = !self.usage_wanted(kind, now);
        self.usage.wanted.insert(kind, now);
        if first && !self.usage_in_flight(kind) {
            self.usage_refresh(kind);
        }
    }

    /// The Keys view is in front: the week's presses are read now when
    /// they haven't been within `KEEP_EVERY`, and kept up to date while it
    /// stays.
    pub fn want_key_counts(&mut self) {
        let now = Instant::now();
        self.usage.keys_wanted = Some(now);
        if self
            .usage
            .keys_asked
            .is_none_or(|at| now.duration_since(at) >= KEEP_EVERY)
        {
            self.keys_refresh();
        }
    }

    /// Reads the last seven days' presses off the task; with `habits` off,
    /// forgets them.
    fn keys_refresh(&mut self) {
        self.usage.keys_asked = Some(Instant::now());
        if !settings::is_on("habits") {
            self.usage.week_keys.clear();
            return;
        }
        let tx = self.tx.clone();
        let job = move || {
            let tally = habits::tally(&paths::state_dir(), 7, util::now_ms());
            let counts = tally.of_kind("key").into_iter().collect();
            let _ = tx.send(Event::Usage(UsageEvent::Keys(counts)));
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::spawn_blocking(job);
        }
    }

    /// The keys pressed in the last seven days by their notation, what
    /// hasn't been written yet included; empty with `habits` off.
    pub fn keys_week(&self) -> HashMap<String, u64> {
        if !settings::is_on("habits") {
            return HashMap::new();
        }
        let mut out = self.usage.week_keys.clone();
        for ((kind, name), n) in &self.usage.pending {
            if kind == "key" {
                *out.entry(name.clone()).or_insert(0) += n;
            }
        }
        out
    }

    /// A view as last worked out (empty before the first time).
    pub fn usage_view(&self, kind: ViewKind) -> ViewData {
        self.usage.views.get(&kind).cloned().unwrap_or_default()
    }

    fn usage_wanted(&self, kind: ViewKind, now: Instant) -> bool {
        // M-o's picker lists Reopen with the view turned off too.
        (kind == ViewKind::Reopen || settings::is_on(kind.setting()))
            && self
                .usage
                .wanted
                .get(&kind)
                .is_some_and(|at| now.duration_since(*at) < WANT_FOR)
    }

    fn usage_in_flight(&self, kind: ViewKind) -> bool {
        self.usage.in_flight.get(&kind).copied().unwrap_or(false)
    }

    fn usage_set_view(&mut self, kind: ViewKind, data: ViewData) {
        let changed = self.usage.views.get(&kind) != Some(&data);
        self.usage.views.insert(kind, data);
        if changed {
            self.usage_redraw();
        }
    }

    /// Works a view out again: off the task for what reads files or
    /// processes, here for Reopen, whose closings are kept in memory and
    /// whose past sessions were read in the background
    /// (`usage_past_sessions`).
    fn usage_refresh(&mut self, kind: ViewKind) {
        self.usage.asked.insert(kind, Instant::now());
        if kind == ViewKind::Reopen {
            let data = self.usage_reopen_view();
            self.usage_set_view(kind, data);
            return;
        }
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        self.usage.in_flight.insert(kind, true);
        let tx = self.tx.clone();
        let state = paths::state_dir();
        match kind {
            ViewKind::Resources => {
                // A reading from before the view was last put away would
                // average the CPU over the whole time between; the first
                // look takes two readings of its own.
                let before = self.usage.reading.clone().filter(|(at, _, _)| {
                    at.elapsed() < Duration::from_millis(resources::RESOURCES_MS) * 3
                });
                tokio::task::spawn_blocking(move || {
                    use crate::mux::procinfo;
                    let (then, before, system_before) = match before {
                        Some(b) => b,
                        None => {
                            let system = procinfo::system();
                            let first = procinfo::all_usage();
                            let at = Instant::now();
                            std::thread::sleep(resources::FIRST_GAP);
                            (at, first, system)
                        }
                    };
                    let system = procinfo::system();
                    let list = procinfo::all_usage();
                    let at = Instant::now();
                    let procs = resources::between(&before, &list, at.duration_since(then));
                    let system_cpu = system
                        .zip(system_before)
                        .map(|(now, was)| now.cpu_since(&was));
                    let _ = tx.send(Event::Usage(UsageEvent::Reading {
                        at,
                        list,
                        procs,
                        system,
                        system_cpu,
                    }));
                });
            }
            ViewKind::Usage => {
                let audit = settings::is_on("request_audit");
                let open = self.usage_tabs_seen();
                let tabs = self.usage_claude_tabs();
                tokio::task::spawn_blocking(move || {
                    let now = util::now_ms();
                    let local = util::local_at_ms(now);
                    let requests = audit.then(|| {
                        (
                            crate::requests::tally_on(&local),
                            crate::requests::tally_week(&local),
                        )
                    });
                    let mut base =
                        sessions::usage_items(&state, now, requests.as_ref().map(|r| &r.0));
                    sessions::github_left(
                        &mut base,
                        crate::integrations::github::tightest_limit()
                            .map(|l| (l.remaining, l.limit)),
                    );
                    let model = charts::model(&state, now, &open, requests);
                    let mut items = charts::items(base, &model);
                    items.extend(sessions::sessions_section(
                        &tabs,
                        &state,
                        &paths::claude_dir(),
                        now,
                    ));
                    let data = ViewData {
                        items,
                        at: Some(now),
                        ..ViewData::default()
                    };
                    let _ = tx.send(Event::Usage(UsageEvent::View(kind, data)));
                });
            }
            ViewKind::Reopen => {}
        }
    }

    /// Every open tab, with its number, name and color and the Claude
    /// session it runs (empty with none running), for the Usage board: a
    /// tab whose Claude has ended is still open for the sessions it ran.
    fn usage_tabs_seen(&self) -> Vec<charts::TabSeen> {
        self.sidebar_all_tabs()
            .into_iter()
            .map(|t| charts::TabSeen {
                session: t
                    .claude
                    .as_ref()
                    .map(|c| c.session_id.clone())
                    .unwrap_or_default(),
                tab: Some(t.id.0),
                server: std::process::id(),
                index: t.index,
                name: t.name.clone(),
                color: t.color.map(|c| c.hex()),
            })
            .collect()
    }

    /// Each tab's number in its session, its label and its panes' own
    /// processes, a pane parked behind the diff or lazygit among them, for
    /// Resources.
    fn usage_tab_panes(&self) -> Vec<TabPanes> {
        self.usage_tab_list()
            .into_iter()
            .map(|(tab, index)| TabPanes {
                tab,
                index,
                name: self.tab_label(tab),
                pids: self
                    .panes_with_parked(tab)
                    .iter()
                    .filter_map(|p| self.panes.get(p)?.pane.pid())
                    .collect(),
            })
            .collect()
    }

    /// Every tab with its number, the sessions in order.
    fn usage_tab_list(&self) -> Vec<(TabId, usize)> {
        // The Notifications window is no tab (src/sidebar/notices.rs).
        self.sessions
            .iter()
            .flat_map(|s| s.tabs.iter().copied())
            .filter(|t| !self.is_notices_tab(*t))
            .map(|t| (t, self.tab_number(t)))
            .collect()
    }

    /// The tabs running Claude, for Usage's Sessions, each quiet while the sidebar
    /// has it idle or finished (sessions.js read the tab's state), so one
    /// looping in the background or stopped on an API error isn't.
    fn usage_claude_tabs(&self) -> Vec<ClaudeTab> {
        use crate::sidebar::model::TabState;
        let states: HashMap<TabId, TabState> = self
            .sidebar_all_tabs()
            .into_iter()
            .map(|t| (t.id, t.state))
            .collect();
        self.usage_tab_list()
            .into_iter()
            .filter_map(|(tab, index)| {
                let panes = self.tabs.get(&tab)?.layout.panes();
                let claude = panes.iter().find_map(|p| self.usage.claudes.get(p))?;
                let quiet = match states.get(&tab) {
                    Some(state) => matches!(state, TabState::Idle | TabState::Done),
                    None => matches!(self.tab_agent(tab), None | Some(agent::State::Done)),
                };
                Some(ClaudeTab {
                    tab,
                    index,
                    name: self.tab_label(tab),
                    quiet,
                    claude: claude.clone(),
                })
            })
            .collect()
    }

    /// The Claude session running in a tab, as last read.
    pub fn tab_claude_session(&self, tab: TabId) -> Option<&sessions::Claude> {
        // A pane parked behind a tool (the diff in its place) included,
        // its Claude still running there.
        let panes = self.panes_with_parked(tab);
        panes.iter().find_map(|p| self.usage.claudes.get(p))
    }

    /// The Reopen view, from the log and the past sessions last read.
    fn usage_reopen_view(&mut self) -> ViewData {
        let tabs = self.usage_tab_list();
        let open: Vec<OpenTab> = tabs
            .iter()
            .map(|(tab, _)| OpenTab {
                path: self.tab_reopen_path(*tab),
                session: self.tab_claude_session(*tab).map(|c| c.session_id.clone()),
            })
            .collect();
        let running: std::collections::HashSet<String> =
            open.iter().filter_map(|t| t.session.clone()).collect();
        let state = paths::state_dir();
        let now = util::now_ms();
        let closings = self.usage.closed.listed(&state, &open);
        let items = super::reopen::items(
            &closings,
            &self.usage.sessions,
            &running,
            &super::closed::removed(&state),
            now,
            &|ms| util::ymd(&util::local_at_ms(ms)),
        );
        ViewData {
            items,
            empty: Some(
                "Nothing to reopen: a tab you close and a Claude session of the last two weeks with no tab are listed here."
                    .into(),
            ),
            at: Some(now),
            ..ViewData::default()
        }
    }

    /// The Reopen view shows, or M-o's picker lists it: its past sessions
    /// are read while it does, at once the first time.
    pub fn want_past_sessions(&mut self) {
        let now = Instant::now();
        let first = self
            .usage
            .sessions_wanted
            .is_none_or(|at| now.duration_since(at) >= WANT_FOR);
        self.usage.sessions_wanted = Some(now);
        if first {
            self.usage_past_sessions(now);
        }
    }

    /// Reads the Claude sessions of the last two weeks again (`wt-revive
    /// --sessions --json`, off the task, as the Work map reads its rows)
    /// while the Reopen view or M-o's picker was shown within `WANT_FOR`,
    /// at most every `SESSIONS_EVERY`; a look that fails keeps the rows the
    /// last one gave.
    fn usage_past_sessions(&mut self, now: Instant) {
        let wanted = self
            .usage
            .sessions_wanted
            .is_some_and(|at| now.duration_since(at) < WANT_FOR);
        if !wanted
            || self.usage.sessions_busy
            || self
                .usage
                .sessions_at
                .is_some_and(|at| now.duration_since(at) < SESSIONS_EVERY)
            || tokio::runtime::Handle::try_current().is_err()
        {
            return;
        }
        let Ok(exe) = util::exe() else {
            return;
        };
        self.usage.sessions_busy = true;
        self.usage.sessions_at = Some(now);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut cmd = tokio::process::Command::new(exe);
            cmd.args(["wt-revive", "--sessions", "--json"]);
            let rows: Option<Vec<crate::cmd::wt_revive::SessionRow>> =
                util::run_async(&mut cmd, Some(SESSIONS_TIMEOUT))
                    .await
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| {
                        String::from_utf8_lossy(&o.stdout)
                            .lines()
                            .filter_map(|l| serde_json::from_str(l).ok())
                            .collect()
                    });
            let _ = tx.send(Event::Usage(UsageEvent::Sessions(rows)));
        });
    }

    // ── Which pane runs which Claude ────────────────────────────────────────

    fn usage_survey(&mut self, now: Instant) {
        if self.usage.surveying
            || self
                .usage
                .surveyed
                .is_some_and(|at| now.duration_since(at) < SURVEY_EVERY)
            || tokio::runtime::Handle::try_current().is_err()
        {
            return;
        }
        self.usage.surveying = true;
        self.usage.surveyed = Some(now);
        let panes: Vec<(PaneId, u32)> = self
            .panes
            .iter()
            .filter_map(|(id, slot)| Some((*id, slot.pane.foreground_pgid()?)))
            .collect();
        let tx = self.tx.clone();
        let seen = settings::is_on("usage").then(|| self.usage_tabs_seen());
        let state = paths::state_dir();
        tokio::task::spawn_blocking(move || {
            if let Some(seen) = seen {
                charts::keep_tabs(&state, util::now_ms(), &seen);
            }
            let found = sessions::claudes(&panes, &agent::sessions::sessions_dir());
            let _ = tx.send(Event::Usage(UsageEvent::Claudes(found)));
        });
    }

    // ── Closed tabs ─────────────────────────────────────────────────────────

    /// A tab as it is now, what its sidebar model says of it (`model`)
    /// giving its last prompt and what became of its work.
    fn usage_tab_now(
        &self,
        tab: TabId,
        index: usize,
        model: Option<&crate::sidebar::model::TabModel>,
    ) -> TabNow {
        let opt = |key: &str| {
            self.tabs
                .get(&tab)
                .and_then(|t| t.opts.get(key))
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        TabNow {
            prompt: model
                .and_then(|m| m.turn.as_ref())
                .map(|t| crate::sidebar::text::plain(&t.prompt).replace('\n', " "))
                .unwrap_or_default(),
            outcome: model.map(outcome_of).unwrap_or_default(),
            panes: self.tabs.get(&tab).map_or(0, |t| t.layout.panes().len()),
            opened: self
                .tabs
                .get(&tab)
                .and_then(|t| crate::tabs::opened(&t.opts))
                .map(|secs| secs * 1000),
            sealed: opt(SEALED_REVIEW),
            sealed_fetch: opt(SEALED_REVIEW_FETCH).is_some_and(|v| v.as_os_str() == "1"),
            tab,
            index,
            name: self.tab_label(tab),
            tab_name: self.tab_own_name(tab).map(str::to_string),
            path: self.tab_reopen_path(tab),
            tab_dir: self
                .tabs
                .get(&tab)
                .and_then(|t| t.opts.get(crate::mux::server::TAB_DIR))
                .map(PathBuf::from),
            session: self.tab_claude_session(tab).map(|c| c.session_id.clone()),
        }
    }

    /// Where a closed tab opens again: the directory it is named from,
    /// except in a tab about another directory than its panes' (`@tab_dir`,
    /// the review tab's PR worktree), whose main clone it is, where its
    /// Claude ran, never someone else's code. Read from git's files alone,
    /// so it is known as the tab closes, its panes already gone.
    fn tab_reopen_path(&self, tab: TabId) -> Option<PathBuf> {
        let t = self.tabs.get(&tab)?;
        if let Some(dir) = t.opts.get(crate::mux::server::TAB_DIR) {
            return crate::cmd::work::main_clone(std::path::Path::new(dir));
        }
        self.tab_path(tab).map(PathBuf::from)
    }

    /// Keeps every tab as it is now, for when it closes.
    fn usage_see_tabs(&mut self) {
        let now = util::now_ms();
        let models: HashMap<TabId, crate::sidebar::model::TabModel> = self
            .sidebar_all_tabs()
            .into_iter()
            .map(|t| (t.id, t))
            .collect();
        let seen: Vec<TabNow> = self
            .usage_tab_list()
            .into_iter()
            .map(|(tab, index)| self.usage_tab_now(tab, index, models.get(&tab)))
            .collect();
        for t in seen {
            self.usage.closed.seen(t, now);
        }
    }

    /// A tab about to close is losing its panes (`kill_tab`, or its last
    /// pane ending): the screen of its Claude's pane (else its active one),
    /// read from the emulator in memory alone, is kept for the closing's
    /// last screen.
    /// Only while Settings' "Save what panes show" (`save_contents`) and
    /// the Reopen view are on, since a screen can hold secrets.
    pub fn keep_closing_screen(&mut self, tab: TabId, shot: Option<super::shots::Shot>) {
        let kept = settings::is_on("save_contents") && settings::is_on("view_reopen");
        if let Some(shot) = shot.filter(|_| kept) {
            self.usage.closing_shots.insert(tab, shot);
        }
    }

    /// The pane whose screen a closing tab keeps: its Claude's, else its
    /// active one.
    pub fn closing_screen_pane(&self, tab: TabId) -> Option<PaneId> {
        let t = self.tabs.get(&tab)?;
        let panes = t.layout.panes();
        panes
            .iter()
            .copied()
            .find(|p| self.sidebar.panes.get(p).is_some_and(|r| r.claude))
            .or(Some(t.active))
    }

    /// A tab is being taken out (`remove_tab`, before it goes): it is seen
    /// once more and logged, with its last screen, for the Reopen view.
    pub fn tab_closing(&mut self, tab: TabId) {
        let shot = self.usage.closing_shots.remove(&tab);
        // The Notifications window is no tab to reopen.
        if self.is_notices_tab(tab) {
            return;
        }
        let index = self.tab_number(tab);
        let now = util::now_ms();
        let model = self.sidebar_all_tabs().into_iter().find(|t| t.id == tab);
        let seen = self.usage_tab_now(tab, index, model.as_ref());
        self.usage.closed.seen(seen, now);
        self.usage
            .closed
            .closed(&paths::state_dir(), tab, now, shot.as_ref());
        if self.usage_wanted(ViewKind::Reopen, Instant::now()) {
            self.usage_refresh(ViewKind::Reopen);
        }
    }

    /// Reopens a closed tab, by its item's id, in the session `client`
    /// shows: in its directory (home when that is gone), with the name it
    /// was given, and its Claude resumed in its shell. Gives the tab, and
    /// what to say once mission control has closed (a sealed review's try
    /// that couldn't start).
    pub fn reopen_closed(
        &mut self,
        client: ClientId,
        key: &str,
    ) -> Result<(TabId, Option<String>), String> {
        let state = paths::state_dir();
        let Some(closing) = self.usage.closed.find(&state, key) else {
            return Err("That tab isn't in the closed list any more.".into());
        };
        let session = self
            .clients
            .get(&client)
            .map(|c| c.session.clone())
            .or_else(|| self.sessions.first().map(|s| s.name.clone()))
            .ok_or("No session to open it in.")?;
        let dir = closing
            .path
            .clone()
            .filter(|p| p.is_dir())
            .unwrap_or_else(paths::home);
        // A worktree of someone else's PR, kept by a build before the
        // review tab's record was its main clone's: no shell or Claude
        // there (src/trust.rs).
        if crate::trust::in_review_dir(&dir) {
            return Err(crate::trust::refusal(&dir));
        }
        let (tab, pane) = self
            .new_tab(
                &session,
                Some(dir),
                None,
                closing.tab_name.clone(),
                crate::mux::server::Opened::Front,
            )
            .map_err(|e| format!("Couldn't open it: {e}"))?;
        // A sealed review stays one, so closing it again keeps its try.
        if let Some(sealed) = &closing.sealed
            && let Some(t) = self.tabs.get_mut(&tab)
        {
            t.opts
                .insert(SEALED_REVIEW.into(), sealed.to_string_lossy().into_owned());
            if closing.sealed_fetch {
                t.opts.insert(SEALED_REVIEW_FETCH.into(), "1".into());
            }
        }
        // The review tab about its PR's worktree again, while that is there.
        if let Some(about) = closing.tab_dir.as_ref().filter(|d| d.is_dir())
            && let Some(t) = self.tabs.get_mut(&tab)
        {
            t.opts.insert(
                crate::mux::server::TAB_DIR.into(),
                about.to_string_lossy().into_owned(),
            );
            self.dirty_tab(tab);
            self.name_tab(tab);
        }
        if let Some(text) = super::closed::resume_line(closing.session.as_deref()) {
            self.send_later(TYPE_AFTER, Event::Usage(UsageEvent::Type { pane, text }));
        }
        let note = closing
            .sealed
            .as_ref()
            .and_then(|worktree| self.reopen_sealed(pane, worktree, closing.sealed_fetch));
        Ok((tab, note))
    }

    /// A sealed review reopened: the PR's code tried again in a sealed try
    /// (`switchboard try --sealed --detached`, with `--fetch` when the
    /// review's try fetched the PR's crates), typed into a shell split
    /// beside its Claude, so what it prints stays; on a computer with no
    /// sandbox, or with the worktree gone, only its Claude is resumed,
    /// which it gives back to say.
    fn reopen_sealed(
        &mut self,
        pane: PaneId,
        worktree: &std::path::Path,
        fetch: bool,
    ) -> Option<String> {
        if let Err(why) = crate::platform::sandbox_ready() {
            return Some(format!("Only the review's Claude was resumed: {why}"));
        }
        if !worktree.is_dir() {
            return Some(format!(
                "Only the review's Claude was resumed: {} is gone.",
                paths::tilde(worktree)
            ));
        }
        let Ok(beside) = self.split(pane, crate::mux::Direction::Right, None, None, None, true)
        else {
            return Some("Only the review's Claude was resumed: no room beside it.".into());
        };
        let quoted = format!("'{}'", worktree.to_string_lossy().replace('\'', "'\\''"));
        self.send_later(
            TYPE_AFTER,
            Event::Usage(UsageEvent::Type {
                pane: beside,
                text: format!(
                    "switchboard try --sealed{} --detached {quoted}\r",
                    if fetch { " --fetch" } else { "" }
                ),
            }),
        );
        None
    }

    /// Resumes a past Claude session in a new tab in the folder Claude finds
    /// it from (its project's), never in someone else's PR's worktree.
    pub fn resume_session(&mut self, client: ClientId, id: &str) -> Result<TabId, String> {
        let Some(row) = self.usage.sessions.iter().find(|s| s.id == id).cloned() else {
            return Err("That session isn't in the list any more.".into());
        };
        let Some(text) = super::closed::resume_line(Some(&row.id)) else {
            return Err("That session can't be resumed.".into());
        };
        // Where Claude finds the conversation: the folder its project is
        // named from, and nowhere else.
        let dir = Some(PathBuf::from(&row.resume))
            .filter(|p| !row.resume.is_empty() && p.is_dir())
            .ok_or("That session's folder is gone, so Claude can't find it to resume.")?;
        if crate::trust::in_review_dir(&dir) {
            return Err(crate::trust::refusal(&dir));
        }
        let session = self
            .clients
            .get(&client)
            .map(|c| c.session.clone())
            .or_else(|| self.sessions.first().map(|s| s.name.clone()))
            .ok_or("No session to open it in.")?;
        let (tab, pane) = self
            .new_tab(
                &session,
                Some(dir),
                None,
                None,
                crate::mux::server::Opened::Front,
            )
            .map_err(|e| format!("Couldn't open it: {e}"))?;
        self.send_later(TYPE_AFTER, Event::Usage(UsageEvent::Type { pane, text }));
        Ok(tab)
    }

    /// M-T: reopens the newest closed tab of the Reopen view (one in a
    /// review worktree passed over, as `reopen_closed` refuses it) and takes
    /// it out of the list, so pressing again reopens the one closed before.
    pub fn reopen_last(&mut self, client: ClientId) {
        let state = paths::state_dir();
        let newest = self
            .usage_reopen_view()
            .items
            .into_iter()
            .filter(|item| super::reopen::of(item).is_some() && !super::reopen::is_session(item))
            .find(|item| match &item.target {
                crate::items::Target::Dir(dir) => !crate::trust::in_review_dir(dir),
                _ => true,
            });
        let Some(item) = newest else {
            self.say(Some(client), "No closed tab to reopen.");
            return;
        };
        let closed_at = self.usage.closed.find(&state, &item.id).map(|c| c.at);
        match self.reopen_closed(client, &item.id) {
            Ok((_, note)) => {
                if let Some(at) = closed_at {
                    self.usage.closed.remove(&state, &item.id, at);
                }
                if self.usage_wanted(ViewKind::Reopen, Instant::now()) {
                    self.usage_refresh(ViewKind::Reopen);
                }
                if let Some(note) = note {
                    self.say(Some(client), &note);
                }
            }
            Err(e) => self.say(Some(client), &e),
        }
    }

    /// Removes a closed tab from the view, up to when it closed, or a past
    /// session, up to when it was last used.
    pub fn remove_closed(&mut self, key: &str) {
        let state = paths::state_dir();
        // Up to the newest of its closing and its session's last use, so a
        // conversation resumed elsewhere since it closed goes too.
        let closed = self.usage.closed.find(&state, key).map(|c| c.at);
        let used = self
            .usage
            .sessions
            .iter()
            .find(|s| s.id == key)
            .map(|s| s.when * 1000);
        let until = closed.max(used);
        if let Some(at) = until {
            self.usage.closed.remove(&state, key, at);
        }
        self.usage_refresh(ViewKind::Reopen);
    }

    // ── Habits ──────────────────────────────────────────────────────────────

    fn usage_count(&mut self, kind: &str, name: &str) {
        if !settings::is_on("habits") || name.is_empty() {
            return;
        }
        *self
            .usage
            .pending
            .entry((kind.to_string(), name.to_string()))
            .or_insert(0) += 1;
    }

    /// A bound key was pressed: its notation, `M-b d` after the prefix.
    pub fn count_key(&mut self, notation: &str) {
        self.usage_count("key", notation);
    }

    /// A command was picked in M-p: by its first key, which counts as
    /// pressed too, as tmux's palette pressed it; else `@palette <action>`.
    pub fn count_pick(&mut self, entry: &Entry) {
        match entry.keys.first() {
            Some(key) => {
                let key = key.clone();
                self.usage_count("palette", &key);
                self.usage_count("key", &key);
            }
            None => self.usage_count("palette", &format!("@palette {}", entry.action.name())),
        }
    }

    /// A mission control view came to the front (not a scoped picker).
    pub fn count_view(&mut self, title: &str) {
        self.usage_count("view", title);
    }

    /// Once a minute: what was counted written to today's file, old days
    /// removed; with the setting off, everything counted forgotten. The
    /// setting turned on or off also drops what wasn't written yet, so
    /// nothing done while it was off is ever kept. At most every
    /// `SUGGEST_EVERY`, after that, the suggestions are worked out again.
    /// The files are read and written off the server's task; the
    /// suggestions come back as `UsageEvent::Suggested`.
    fn usage_habits(&mut self, now: Instant) {
        let on = settings::is_on("habits");
        if self.usage.habits_were.is_some_and(|was| was != on) {
            self.usage.pending.clear();
            self.usage.suggested = None;
            self.usage.kept = None;
        }
        self.usage.habits_were = Some(on);
        let keep = self
            .usage
            .kept
            .is_none_or(|at| now.duration_since(at) >= KEEP_EVERY);
        let suggest = self
            .usage
            .suggested
            .is_none_or(|at| now.duration_since(at) >= SUGGEST_EVERY);
        if !keep && !suggest {
            return;
        }
        if keep {
            self.usage.kept = Some(now);
        }
        if suggest {
            self.usage.suggested = Some(now);
        }
        let counts: Vec<Count> = if keep {
            self.usage
                .pending
                .drain()
                .map(|((kind, name), n)| Count { kind, name, n })
                .collect()
        } else {
            Vec::new()
        };
        // The keys written stay in the week's counts until the next read of
        // the files has them.
        for c in counts.iter().filter(|c| c.kind == "key") {
            *self.usage.week_keys.entry(c.name.clone()).or_insert(0) += c.n;
        }
        let notes = self.habit_key_notes();
        let tx = self.tx.clone();
        let job = move || {
            let state = paths::state_dir();
            let at = util::now_ms();
            if keep && !on {
                habits::forget(&state);
            } else if keep {
                habits::prune(&state, at);
                habits::record(&state, &counts, at);
            }
            if suggest {
                let list = if on {
                    habits::suggestions(&state, &setting_value, &notes, at)
                } else {
                    Vec::new()
                };
                let _ = tx.send(Event::Usage(UsageEvent::Suggested(list)));
            }
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::spawn_blocking(job);
        } else {
            job();
        }
    }

    /// The root keys' descriptions, for the suggestions: each bound key
    /// with what it does (M-2 to M-9 left out, as tmux.conf described only
    /// M-1).
    pub fn habit_key_notes(&self) -> IndexMap<String, String> {
        let mut keys: Vec<(String, String)> = self
            .bindings
            .root
            .iter()
            .filter(|(_, a)| !matches!(a, Action::SelectTab(n) if *n > 1))
            .map(|(k, a)| {
                let words = match a {
                    Action::SelectTab(_) => a.palette_note().map_or_else(|| a.about(), |(w, _)| w),
                    _ => a.about(),
                };
                (k.notation(), words)
            })
            .collect();
        keys.sort();
        keys.into_iter().collect()
    }

    /// The commands with no key of their own, by `@palette <action>`.
    fn usage_palette_notes() -> IndexMap<String, String> {
        Action::palette_only()
            .into_iter()
            .map(|a| (format!("@palette {}", a.name()), a.about()))
            .collect()
    }

    /// The menu for a suggestion, by its id.
    pub fn habit_choices(&self, id: &str) -> Vec<habits::Choice> {
        self.usage
            .suggestions
            .iter()
            .find(|s| s.id == id)
            .map(habits::choices)
            .unwrap_or_default()
    }

    /// Does what a suggestion's menu picked; gives what to say, if anything.
    pub fn habit_outcome(&mut self, id: &str, outcome: Outcome) -> Result<Option<String>, String> {
        let state = paths::state_dir();
        let now = util::now_ms();
        let said = match outcome {
            Outcome::Apply => {
                let change = self
                    .usage
                    .suggestions
                    .iter()
                    .find(|s| s.id == id)
                    .and_then(|s| s.setting.clone())
                    .ok_or("That suggestion is gone.")?;
                settings::write_setting(&change.key, change.value.clone())
                    .map_err(|e| format!("Couldn't save it: {e}"))?;
                settings::reload_settings();
                habits::dismiss(&state, id, true, now);
                Some(
                    if change.value == Value::Bool(true) {
                        "Turned on; Settings changes it back"
                    } else {
                        "Turned off; Settings changes it back"
                    }
                    .to_string(),
                )
            }
            Outcome::GotIt | Outcome::Never => {
                habits::dismiss(&state, id, true, now);
                None
            }
            Outcome::NotNow => {
                habits::dismiss(&state, id, false, now);
                None
            }
        };
        self.usage.suggested = None;
        self.usage_habits(Instant::now());
        Ok(said)
    }

    /// M-p's "How you use Switchboard": the report's lines, read from a
    /// month of counts (a palette command, so it may read them now).
    pub fn habits_report(&self) -> Vec<report::Line> {
        if !settings::is_on("habits") {
            return report::off_lines();
        }
        let state = paths::state_dir();
        let now = util::now_ms();
        let month = habits::tally(&state, habits::UNUSED_DAYS, now);
        let week = habits::tally(&state, 7, now);
        report::report_lines(&report::Inputs {
            month: &month,
            week: &week,
            value: &setting_value,
            notes: &self.habit_key_notes(),
            palette_notes: &Self::usage_palette_notes(),
            suggestions: &self.usage.suggestions,
        })
    }

    // ── Plugins ─────────────────────────────────────────────────────────────

    /// Loads the plugins again when sidebar.json changed, and runs each
    /// one due.
    fn usage_run_plugins(&mut self, now: Instant) {
        let file = paths::config_dir().join("sidebar.json");
        let modified = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
        if self.usage.plugins_seen != Some(modified) {
            self.usage.plugins_seen = Some(modified);
            let config = util::read_json(&file).unwrap_or(Value::Null);
            let (mut loaded, problems) =
                plugins::load(&config, &paths::config_dir().join("sidebar-plugins"));
            for p in problems {
                eprintln!("{p}");
            }
            // A plugin kept keeps what it had.
            let mut had: HashMap<String, plugins::Plugin> = self
                .usage
                .plugins
                .drain(..)
                .map(|p| (p.name.clone(), p))
                .collect();
            for p in &mut loaded {
                if let Some(old) = had.remove(&p.name)
                    && old.source == p.source
                    && old.options == p.options
                {
                    *p = old;
                }
            }
            self.usage.plugins = loaded;
            self.usage_redraw();
        }
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        for p in self.usage.plugins.iter_mut().filter(|p| p.due(now)) {
            p.fetching = true;
            let (name, source, options) = (p.name.clone(), p.source.clone(), p.options.clone());
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let result = plugins::run(source, options).await;
                let _ = tx.send(Event::Usage(UsageEvent::Plugin { name, result }));
            });
        }
    }

    /// Every plugin's view, in sidebar.json's order.
    pub fn plugin_views(&self) -> Vec<PluginView> {
        self.usage
            .plugins
            .iter()
            .map(|p| PluginView {
                name: p.name.clone(),
                title: p.title.clone(),
                sidebar: p.sidebar,
                loaded: p.loaded,
                data: p.view(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{Checks, PrState};
    use crate::sidebar::model::{MergeState, TabModel};

    #[test]
    fn what_became_of_a_tab_s_work() {
        let mut t = TabModel::new(TabId(1), 1, "api");
        assert_eq!(outcome_of(&t), "");
        t.review = Some("CHANGES_REQUESTED".into());
        assert_eq!(outcome_of(&t), "changes asked");
        t.phase = Some("tests failed 2 of 401".into());
        assert_eq!(outcome_of(&t), "tests failed", "a landing's tests");
        t.phase = None;
        t.checks = Some(Checks {
            pass: 3,
            fail: 1,
            pending: 0,
        });
        assert_eq!(outcome_of(&t), "tests failed");
        t.pr = Some(12);
        t.pr_state = Some(PrState::Merged);
        t.merge = Some(MergeState::Merged);
        assert_eq!(outcome_of(&t), "landed #12", "a merged PR landed");
        t.landed_on = Some(("a3f1c2e".into(), "main".into()));
        assert_eq!(outcome_of(&t), "landed a3f1c2e");
    }
}
