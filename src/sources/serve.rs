//! The sources on the server: on every tick, the lookups `schedule` says are
//! due are started off the server's task, each answering with
//! `Event::Sources`; the answers are kept in `Sources`, each view's
//! `ViewData` built from them, and every tab's facts (`Server.facts`) worked
//! out again from what is known, so the sidebar and mission control read
//! only what is already here. The daemon's lookups (lib/sidebar/daemon.js's
//! `merges`, `board`, `checks`, `mergeQueues`, `tickets` and `linearInbox`)
//! keep their names, and each is skipped with its settings off.
//!
//! Beyond its turn, a lookup is asked again when something appends to the
//! `github` file in the state directory (the PR lookups, at most every 15
//! seconds), when a tab leads to a PR not asked about yet (so a new PR's
//! state shows at once, not on the next round), and when mission control
//! goes to its view (`came_to_view`) or asks for it (`refresh_sources`).
//! The landing queue is read on every tick, on a blocking thread, since it
//! is files alone.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::landing::{self, Place};
use super::repo::{self, PrKey};
use super::schedule::GITHUB_LOOKUPS;
use super::{
    CI, INBOX, PRS, QUEUES, TICKETS, TabInfo, VIEWS, ciboard, enabled, inbox, prboard, prs, queues,
    repos_of, setup, tickets,
};
use crate::facts::{Facts, PrFacts, TabFacts};
use crate::integrations::github::Repo;
use crate::items::Item;
use crate::mux::TabId;
use crate::mux::server::{Event, Reply, Server};
use crate::{paths, settings, util};

/// How long a directory's repository is kept before its config is read again.
const REPO_KEPT: Duration = Duration::from_secs(60);

/// A lookup's answer, from its task.
pub enum Answer {
    Merges(anyhow::Result<Vec<(PrKey, PrFacts)>>),
    Board {
        scope: String,
        at: i64,
        result: anyhow::Result<(Vec<Value>, Option<Vec<Value>>)>,
    },
    Checks(anyhow::Result<Vec<Item>>),
    Queues {
        repos: Vec<String>,
        result: anyhow::Result<(Value, HashSet<String>)>,
    },
    Tickets(anyhow::Result<Vec<Value>>),
    Inbox(anyhow::Result<Vec<Value>>),
    Landing(HashMap<TabId, Place>),
}

/// The lookup that fills a view.
fn lookup_of(view: &str) -> Option<&'static str> {
    Some(match view {
        PRS => "board",
        CI => "checks",
        QUEUES => "mergeQueues",
        TICKETS => "tickets",
        INBOX => "linearInbox",
        _ => return None,
    })
}

/// What the views that name tabs were last built for, so they are built
/// again only when a tab's place, name, PR or repository changes.
type TabsSeen = Vec<(TabId, usize, String, Option<u64>, Option<Repo>)>;

/// What `Sources` keeps for the server beyond the views: each directory's
/// GitHub repository, read again after `REPO_KEPT` (prs.js read it once per
/// directory), the ticket board's issues for building it again as the tabs
/// change, and whether a read of the landing queue is under way.
#[derive(Default)]
pub(super) struct Kept {
    repos: HashMap<PathBuf, (Option<Repo>, Instant)>,
    ticket_issues: Option<Vec<Value>>,
    tabs_seen: TabsSeen,
    landing_busy: bool,
}

impl Server {
    /// The sources' part of the server's tick.
    pub fn sources_tick(&mut self) {
        let now = Instant::now();
        let tabs = self.source_tabs();
        if self.github_file_appended() && self.sources.schedule.github_now(now) {
            let _ = std::fs::write(github_file(), "");
            for name in GITHUB_LOOKUPS {
                if self.sources.schedule.ask(name, now) {
                    self.start_lookup_named(name, &tabs);
                }
            }
        }
        let away = self.away();
        let fast = settings::is_on("poll_fast");
        for name in self.sources.schedule.due(now, fast, away) {
            self.start_lookup_named(name, &tabs);
        }
        // A tab's PR not asked about yet is asked about now.
        if !away
            && settings::is_on("github")
            && tabs
                .iter()
                .filter_map(|t| t.key.as_ref())
                .any(|k| !self.sources.asked.contains(k))
            && self.sources.schedule.ask("merges", now)
        {
            self.start_lookup_named("merges", &tabs);
        }
        self.start_landing(&tabs);
        self.sources_tabs_changed(&tabs);
        self.update_facts(&tabs);
        if !enabled(INBOX) && self.notices.contains_key("inbox") {
            self.set_notices("inbox", Vec::new());
        }
    }

    /// A lookup answered.
    pub fn source_answer(&mut self, answer: Answer) {
        let tabs = self.source_tabs();
        // The views today's todo note is written from (src/day), told what
        // they read once each answers.
        let for_todo = match &answer {
            Answer::Board { .. } => Some(PRS),
            Answer::Checks(_) => Some(CI),
            Answer::Tickets(_) => Some(TICKETS),
            Answer::Inbox(_) => Some(INBOX),
            _ => None,
        };
        match answer {
            Answer::Merges(result) => {
                self.sources.schedule.done("merges");
                match result {
                    Ok(found) => self.sources.known.extend(found),
                    Err(e) => eprintln!("merge states: {e}"),
                }
            }
            Answer::Board { scope, at, result } => {
                self.sources.schedule.done("board");
                let view = self.sources.views.entry(PRS).or_default();
                match result {
                    Ok((open, merged)) => {
                        view.items = self.sources.board.take(&scope, at, open, merged);
                        setup::answered(view);
                    }
                    Err(e) => setup::failed(view, &e, "your pull requests"),
                }
            }
            Answer::Checks(result) => {
                self.sources.schedule.done("checks");
                let view = self.sources.views.entry(CI).or_default();
                match result {
                    Ok(items) => {
                        view.items = items;
                        setup::answered(view);
                    }
                    Err(e) => setup::failed(view, &e, "your pull requests' checks"),
                }
            }
            Answer::Queues { repos, result } => {
                self.sources.schedule.done("mergeQueues");
                let view = self.sources.views.entry(QUEUES).or_default();
                match result {
                    Ok((data, failed)) => {
                        self.sources.queues.take(&repos, &data, &failed);
                        setup::answered(view);
                        self.sources.queues.view_into(view, &tabs);
                    }
                    Err(e) => setup::failed(view, &e, "the merge queues"),
                }
            }
            Answer::Tickets(result) => {
                self.sources.schedule.done("tickets");
                let view = self.sources.views.entry(TICKETS).or_default();
                match result {
                    Ok(issues) => {
                        view.items = tickets::board_from(&issues, &tabs);
                        self.sources.kept.ticket_issues = Some(issues);
                        setup::answered(view);
                    }
                    Err(e) => setup::failed(view, &e, "your tickets"),
                }
            }
            Answer::Inbox(result) => {
                self.sources.schedule.done("linearInbox");
                let view = self.sources.views.entry(INBOX).or_default();
                match result {
                    Ok(nodes) => {
                        view.items = inbox::inbox_from(&nodes, util::now_ms());
                        setup::answered(view);
                    }
                    Err(e) => setup::failed(view, &e, "your Linear inbox"),
                }
                self.inbox_notices();
            }
            Answer::Landing(places) => {
                self.sources.kept.landing_busy = false;
                self.sources.landing = places;
            }
        }
        if let Some(name) = for_todo {
            self.hand_to_todo(name);
        }
        self.update_facts(&tabs);
        self.redraw_everywhere();
    }

    /// The Inbox's items as notifications, those dismissed there left out,
    /// while the view is on.
    pub(crate) fn inbox_notices(&mut self) {
        let items = match self.sources.views.get(INBOX) {
            Some(view) if enabled(INBOX) => inbox::notices(&paths::state_dir(), &view.items),
            _ => Vec::new(),
        };
        self.set_notices("inbox", items);
    }

    /// Dismisses an Inbox item from the notifications, here alone: it stays
    /// in the Inbox view and unread in Linear.
    pub fn inbox_dismiss(&mut self, item: &Item) {
        inbox::dismiss(&paths::state_dir(), item);
        self.inbox_notices();
    }

    /// Gives today's todo note what a view last read: each item as a line,
    /// and whether the look failed, as the daemon handed todo.js its views.
    fn hand_to_todo(&mut self, name: &str) {
        let Some(view) = self.sources.views.get(name) else {
            return;
        };
        let lines = view
            .items
            .iter()
            .map(|i| {
                if i.detail.is_empty() {
                    i.title.clone()
                } else {
                    format!("{} — {}", i.title, i.detail)
                }
            })
            .collect();
        let failed = view.error.is_some() || view.setup.is_some();
        self.day.answered(name, lines, failed);
    }

    /// Asks for a view's lookup now, or every lookup with `None`, unless it
    /// is still waiting on its last answer.
    pub fn refresh_sources(&mut self, view: Option<&str>) {
        let tabs = self.source_tabs();
        let names: Vec<&'static str> = match view {
            Some(view) => lookup_of(view).into_iter().collect(),
            None => VIEWS
                .iter()
                .filter_map(|v| lookup_of(v))
                .chain(["merges"])
                .collect(),
        };
        for name in names {
            if self.sources.schedule.ask(name, Instant::now()) {
                self.start_lookup_named(name, &tabs);
            }
        }
    }

    /// Mission control went to a view: the Inbox and CI views ask again at
    /// most a minute apart, the Merge queues view at most 15 seconds apart,
    /// as the daemon's `cameTo` did.
    pub fn came_to_view(&mut self, view: &str) {
        let every = match view {
            INBOX | CI => Duration::from_secs(60),
            QUEUES => Duration::from_secs(15),
            _ => return,
        };
        let Some(view) = super::VIEWS.into_iter().find(|v| *v == view) else {
            return;
        };
        if self
            .sources
            .came
            .get(view)
            .is_some_and(|at| at.elapsed() < every)
        {
            return;
        }
        self.sources.came.insert(view, Instant::now());
        self.refresh_sources(Some(view));
    }

    /// `switchboard sources`: every view's data as JSON, or with `refresh
    /// [<view>]`, asks for it now.
    pub fn sources_command(&mut self, rest: &[String]) -> Reply {
        match rest.first().map(String::as_str) {
            None => {
                let views: serde_json::Map<String, Value> = VIEWS
                    .iter()
                    .map(|v| {
                        let data = self
                            .sources
                            .view(v)
                            .map(|d| serde_json::to_value(d).unwrap_or(Value::Null))
                            .unwrap_or(Value::Null);
                        (v.to_string(), data)
                    })
                    .collect();
                Reply::ok(format!("{}\n", json!({ "views": views })))
            }
            Some("refresh") => {
                let view = rest.get(1).map(String::as_str);
                if view.is_some_and(|v| lookup_of(v).is_none()) {
                    return Reply::fail(format!(
                        "no such view: {} (one of {})",
                        view.unwrap_or(""),
                        VIEWS.join(", ")
                    ));
                }
                self.refresh_sources(view);
                Reply::ok("")
            }
            Some(other) => Reply::fail(format!(
                "usage: switchboard sources [refresh [{}]] (not {other})",
                VIEWS.join("|")
            )),
        }
    }

    // ── The tabs as the lookups see them ────────────────────────────────────

    /// Every tab, in its session's order, with what the lookups need of it.
    fn source_tabs(&mut self) -> Vec<TabInfo> {
        let mut tabs = Vec::new();
        for session in &self.sessions {
            for (i, id) in session.tabs.iter().enumerate() {
                let Some(tab) = self.tabs.get(id) else {
                    continue;
                };
                let path = self.tab_path(*id).map(Path::to_path_buf);
                let mut dirs: Vec<PathBuf> = path.iter().cloned().collect();
                for pane in tab.layout.panes() {
                    let Some(slot) = self.panes.get(&pane) else {
                        continue;
                    };
                    let marked = |name: &str| {
                        slot.opts
                            .get(name)
                            .is_some_and(|v| !v.is_empty() && v != "0")
                    };
                    if marked("@sidebar") || marked("@notes_pane") {
                        continue;
                    }
                    if let Some(cwd) = slot.pane.cwd()
                        && !dirs.contains(&cwd)
                    {
                        dirs.push(cwd);
                    }
                }
                tabs.push(TabInfo {
                    id: *id,
                    index: i + 1,
                    name: self.tab_label(*id),
                    path,
                    pr: self.tab_pr(*id),
                    repo: None,
                    key: None,
                    dirs,
                });
            }
        }
        let kept = &mut self.sources.kept;
        let mut origin = |dir: &Path| {
            match kept.repos.get(dir) {
                Some((repo, at)) if at.elapsed() < REPO_KEPT => return repo.clone(),
                _ => {}
            }
            let repo = repo::origin_of(dir);
            kept.repos
                .insert(dir.to_path_buf(), (repo.clone(), Instant::now()));
            repo
        };
        for tab in &mut tabs {
            tab.repo = tab.path.as_deref().and_then(&mut origin);
            tab.key = repo::key_of(
                &tab.name,
                tab.path.as_deref(),
                tab.pr,
                tab.repo.as_ref(),
                &mut origin,
            );
        }
        let live: HashSet<&Path> = tabs.iter().filter_map(|t| t.path.as_deref()).collect();
        if kept.repos.len() > 4 * live.len() + 16 {
            kept.repos.retain(|dir, _| live.contains(dir.as_path()));
        }
        tabs
    }

    // Starts a lookup the schedule has marked asked, or lets it go again at
    // once when its settings are off or it has nothing to ask about.
    fn start_lookup_named(&mut self, name: &'static str, tabs: &[TabInfo]) {
        if !self.start_lookup_inner(name, tabs) {
            self.sources.schedule.done(name);
        }
    }

    fn start_lookup_inner(&mut self, name: &'static str, tabs: &[TabInfo]) -> bool {
        if tokio::runtime::Handle::try_current().is_err() {
            return false;
        }
        let tx = self.tx.clone();
        let send = move |answer: Answer| {
            let _ = tx.send(Event::Sources(answer));
        };
        let mut repos = repos_of(tabs);
        match name {
            "merges" => {
                if !settings::is_on("github") {
                    return false;
                }
                let keys: Vec<PrKey> = tabs.iter().filter_map(|t| t.key.clone()).collect();
                if keys.is_empty() {
                    return false;
                }
                self.sources.asked.extend(keys.iter().cloned());
                tokio::spawn(async move { send(Answer::Merges(prs::fetch(keys).await)) });
            }
            "board" => {
                if !enabled(PRS) {
                    return false;
                }
                if repos.is_empty() {
                    setup::no_repos(self.sources.views.entry(PRS).or_default());
                    return false;
                }
                let scope = prboard::scope_of(&repos);
                let at = util::now_ms();
                let with_merged = self.sources.board.wants_merged(&scope, at);
                tokio::spawn(async move {
                    let result = prboard::fetch(scope.clone(), with_merged, at).await;
                    send(Answer::Board { scope, at, result });
                });
            }
            "checks" => {
                if !enabled(CI) {
                    return false;
                }
                if repos.is_empty() {
                    setup::no_repos(self.sources.views.entry(CI).or_default());
                    return false;
                }
                tokio::spawn(async move { send(Answer::Checks(ciboard::fetch(repos).await)) });
            }
            "mergeQueues" => {
                if !enabled(QUEUES) {
                    return false;
                }
                repos.sort();
                if repos.is_empty() {
                    self.sources.queues = queues::Queues::default();
                    let view = self.sources.views.entry(QUEUES).or_default();
                    setup::no_repos(view);
                    view.empty = None;
                    return false;
                }
                tokio::spawn(async move {
                    let result = queues::fetch(&repos).await;
                    send(Answer::Queues { repos, result });
                });
            }
            "tickets" => {
                if !enabled(TICKETS) {
                    return false;
                }
                let now = util::now_ms();
                tokio::spawn(async move { send(Answer::Tickets(tickets::fetch(now).await)) });
            }
            "linearInbox" => {
                if !enabled(INBOX) {
                    return false;
                }
                tokio::spawn(async move { send(Answer::Inbox(inbox::fetch().await)) });
            }
            _ => return false,
        }
        true
    }

    // Reads the landing queue on a blocking thread, unless a read is still
    // under way; with the landing_queue setting off, forgets it.
    fn start_landing(&mut self, tabs: &[TabInfo]) {
        if !settings::is_on("landing_queue") {
            self.sources.landing.clear();
            return;
        }
        if self.sources.kept.landing_busy || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        self.sources.kept.landing_busy = true;
        let dirs: Vec<(TabId, Vec<PathBuf>)> =
            tabs.iter().map(|t| (t.id, t.dirs.clone())).collect();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let running = landing::running_sessions(&crate::agent::sessions::sessions_dir());
            let places = landing::landing_of(&dirs, &running);
            let _ = tx.send(Event::Sources(Answer::Landing(places)));
        });
    }

    // Builds the views that name tabs again when the tabs changed.
    fn sources_tabs_changed(&mut self, tabs: &[TabInfo]) {
        let seen: TabsSeen = tabs
            .iter()
            .map(|t| (t.id, t.index, t.name.clone(), t.pr, t.repo.clone()))
            .collect();
        if seen == self.sources.kept.tabs_seen {
            return;
        }
        self.sources.kept.tabs_seen = seen;
        if let Some(view) = self.sources.views.get_mut(QUEUES)
            && view.setup.is_none()
        {
            self.sources.queues.view_into(view, tabs);
        }
        if let (Some(view), Some(issues)) = (
            self.sources.views.get_mut(TICKETS),
            &self.sources.kept.ticket_issues,
        ) {
            view.items = tickets::board_from(issues, tabs);
        }
        self.redraw_everywhere();
    }

    // Every tab's facts from what is known, the PR's only with the github
    // setting on and the landing queue's only with landing_queue on.
    fn update_facts(&mut self, tabs: &[TabInfo]) {
        let github = settings::is_on("github");
        let mut facts = Facts::new();
        for tab in tabs {
            let pr = tab
                .key
                .as_ref()
                .filter(|_| github)
                .and_then(|k| self.sources.known.get(k))
                .cloned();
            let landing = self.sources.landing.get(&tab.id).map(|p| p.landing);
            if pr.is_some() || landing.is_some() {
                facts.insert(tab.id, TabFacts { pr, landing });
            }
        }
        if facts != self.facts {
            self.facts = facts;
            self.redraw_everywhere();
        }
        let keys: HashSet<&PrKey> = tabs.iter().filter_map(|t| t.key.as_ref()).collect();
        if self.sources.known.len() > 4 * keys.len() + 64 {
            self.sources.known.retain(|k, _| keys.contains(k));
        }
    }

    // Whether something has appended to the `github` file since it was last
    // emptied.
    fn github_file_appended(&self) -> bool {
        std::fs::metadata(github_file()).is_ok_and(|m| m.len() > 0)
    }

    fn redraw_everywhere(&mut self) {
        for client in self.clients.values_mut() {
            client.dirty = true;
        }
    }
}

/// The file that asks the PR lookups again: `github` in the state directory.
pub fn github_file() -> PathBuf {
    paths::state_dir().join("github")
}
