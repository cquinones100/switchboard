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

use super::landing::{self, Read};
use super::repo::{self, PrKey};
use super::schedule::GITHUB_LOOKUPS;
use super::{
    CI, INBOX, LANDING, PRS, TICKETS, TabInfo, VIEWS, ciboard, enabled, inbox, mainci, pr_repos_of,
    prboard, prnotices, prs, queues, repos_of, setup, tickets,
};
use crate::facts::{Facts, Landing, Merge, Method, PrFacts, PrState, TabFacts};
use crate::integrations::github::Repo;
use crate::items::{Item, ViewData};
use crate::mux::TabId;
use crate::mux::server::{Event, Reply, Server};
use crate::{paths, settings, util};

/// How long a directory's repository is kept before its config is read again.
const REPO_KEPT: Duration = Duration::from_secs(60);

/// How long the Inbox's commits stand while its tickets stay the same.
const LINKS_EVERY: Duration = Duration::from_secs(10 * 60);

/// A lookup's answer, from its task.
pub enum Answer {
    Merges(anyhow::Result<Vec<(PrKey, PrFacts)>>),
    Board {
        scope: String,
        at: i64,
        result: anyhow::Result<(Vec<Value>, Option<Vec<Value>>)>,
    },
    Checks(anyhow::Result<Vec<Item>>),
    MainCi(anyhow::Result<Vec<mainci::Looked>>),
    Queues {
        repos: Vec<String>,
        result: anyhow::Result<(Value, HashSet<String>)>,
    },
    Tickets(anyhow::Result<Vec<Value>>),
    Inbox(anyhow::Result<Vec<Value>>),
    /// The newest commit naming each of the Inbox's tickets (`inbox::links`).
    InboxLinks(HashMap<String, inbox::Commit>),
    Landing(Read),
}

/// The lookup that fills a view.
fn lookup_of(view: &str) -> Option<&'static str> {
    Some(match view {
        PRS => "board",
        CI => "checks",
        LANDING => "mergeQueues",
        TICKETS => "tickets",
        INBOX => "linearInbox",
        _ => return None,
    })
}

/// What the views that name tabs were last built for, so they are built
/// again only when a tab's place, name, PR or repository changes.
type TabsSeen = Vec<(TabId, usize, String, Option<u64>, Option<Repo>)>;

/// What `Sources` keeps for the server beyond the views: each directory's
/// GitHub repositories (origin, and a fork's upstream), read again after
/// `REPO_KEPT` (prs.js read origin once per directory), the ticket board's
/// issues for building it again as the tabs change, and whether a read of
/// the landing queue is under way.
#[derive(Default)]
pub(super) struct Kept {
    repos: HashMap<PathBuf, (repo::Remotes, Instant)>,
    ticket_issues: Option<Vec<Value>>,
    tabs_seen: TabsSeen,
    landing_busy: bool,
    /// The Inbox's commits, the tickets they were looked for, when, and
    /// whether a look is under way.
    pub(super) inbox_links: HashMap<String, inbox::Commit>,
    inbox_links_for: Vec<String>,
    inbox_links_at: Option<Instant>,
    inbox_links_busy: bool,
}

impl Kept {
    /// A directory's origin as last read, with no read of its own.
    pub(super) fn origin_kept(&self, dir: &Path) -> Option<&super::Repo> {
        self.repos.get(dir)?.0.origin.as_ref()
    }

    // A directory's repositories, as last read unless that was `REPO_KEPT`
    // ago.
    fn remotes(&mut self, dir: &Path) -> repo::Remotes {
        match self.repos.get(dir) {
            Some((remotes, at)) if at.elapsed() < REPO_KEPT => return remotes.clone(),
            _ => {}
        }
        let remotes = repo::remotes_of(dir);
        self.repos
            .insert(dir.to_path_buf(), (remotes.clone(), Instant::now()));
        remotes
    }
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
        // A tab's PR not asked about yet is asked about now, and so is a
        // worktree's no tab is in.
        if !away
            && settings::is_on("github")
            && tabs
                .iter()
                .flat_map(|t| &t.keys)
                .chain(self.sources.untabbed.values().flatten())
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
                        let board = view.items.clone();
                        let news = self.sources.pr_known.take(&board, &scope, util::now_ms());
                        if !news.is_empty() {
                            self.sources.pr_notices =
                                prnotices::kept_with(&self.sources.pr_notices, news);
                        }
                        self.pr_notices();
                    }
                    Err(e) => setup::failed(view, &e, "your pull requests"),
                }
            }
            Answer::Checks(result) => {
                self.sources.schedule.done("checks");
                let view = self.sources.views.entry(CI).or_default();
                match result {
                    Ok(items) => {
                        self.sources.checks = items;
                        setup::answered(view);
                    }
                    Err(e) => setup::failed(view, &e, "your pull requests' checks"),
                }
                self.ci_view_items();
            }
            Answer::MainCi(result) => {
                self.sources.schedule.done("mainCi");
                match result {
                    Ok(looked) => {
                        let items = mainci::merged(&self.sources.main_ci, looked);
                        self.take_main_ci(items);
                    }
                    Err(e) => eprintln!("the default branch's CI: {e}"),
                }
            }
            Answer::Queues { repos, result } => {
                self.sources.schedule.done("mergeQueues");
                // How the merge queues were last asked, for the Landing view.
                let view = self.sources.views.entry(LANDING).or_default();
                match result {
                    Ok((data, failed)) => {
                        self.sources.queues.take(&repos, &data, &failed);
                        setup::answered(view);
                    }
                    Err(e) => setup::failed(view, &e, "the merge queues"),
                }
            }
            Answer::Tickets(result) => {
                self.sources.schedule.done("tickets");
                let view = self.sources.views.entry(TICKETS).or_default();
                match result {
                    Ok(issues) => {
                        view.items =
                            tickets::board_from(&issues, &tabs, crate::settings::is_on("ux2"));
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
                        // Those dismissed leave the view as they leave the
                        // notifications.
                        let items = inbox::inbox_from(&nodes, util::now_ms());
                        view.items = inbox::notices(&paths::state_dir(), &items);
                        setup::answered(view);
                    }
                    Err(e) => setup::failed(view, &e, "your Linear inbox"),
                }
                self.inbox_notices();
                self.inbox_links_ask(&tabs);
            }
            Answer::InboxLinks(found) => {
                self.sources.kept.inbox_links_busy = false;
                self.sources.kept.inbox_links = found;
                self.mission_changed();
            }
            Answer::Landing(read) => {
                self.sources.kept.landing_busy = false;
                self.sources.landing = read;
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

    /// The default branches failing at the last look: the CI view's first
    /// items, and their notifications.
    fn take_main_ci(&mut self, items: Vec<Item>) {
        let failing: HashSet<&String> = items.iter().map(|i| &i.id).collect();
        self.sources
            .main_ci_dismissed
            .retain(|id| failing.contains(id));
        self.sources.main_ci = items;
        self.ci_view_items();
        self.main_ci_notices();
    }

    // The CI view's items: the failing default branches, then your PRs'.
    fn ci_view_items(&mut self) {
        let items: Vec<Item> = self
            .sources
            .main_ci
            .iter()
            .chain(&self.sources.checks)
            .cloned()
            .collect();
        let view = self.sources.views.entry(CI).or_default();
        if view.items != items {
            view.items = items;
            self.redraw_everywhere();
        }
    }

    /// The failing default branches as notifications (the `ci` kind), those
    /// dismissed left out, each said once per failing commit.
    pub(crate) fn main_ci_notices(&mut self) {
        let items: Vec<Item> = self
            .sources
            .main_ci
            .iter()
            .filter(|i| !self.sources.main_ci_dismissed.contains(&i.id))
            .cloned()
            .collect();
        self.set_notices("ci", items);
    }

    /// Dismisses a failing default branch's notification until it fails at
    /// another commit; it stays in the CI view while it fails.
    pub fn main_ci_dismiss(&mut self, item: &Item) {
        self.sources.main_ci_dismissed.insert(item.id.clone());
        self.main_ci_notices();
    }

    /// Your PRs' news as notifications (the `prs` kind), while
    /// `pr_notices` and `github` are on.
    pub(crate) fn pr_notices(&mut self) {
        let items = if crate::settings::is_on("pr_notices") && crate::settings::is_on("github") {
            self.sources.pr_notices.clone()
        } else {
            Vec::new()
        };
        self.set_notices("prs", items);
    }

    /// Dismisses one of your PRs' news.
    pub fn pr_notice_dismiss(&mut self, item: &Item) {
        self.sources.pr_notices.retain(|i| i.id != item.id);
        self.pr_notices();
    }

    /// Dismisses an Inbox item, here alone: it leaves the notifications and
    /// the Inbox view, and stays unread in Linear.
    pub fn inbox_dismiss(&mut self, item: &Item) {
        inbox::dismiss(&paths::state_dir(), item);
        self.sources.drop_inbox_item(inbox::notification_of(item));
        self.inbox_notices();
    }

    /// Looks, off the task, for the newest commit naming each of the
    /// Inbox's tickets in the tabs' repositories, when the tickets have
    /// changed since the last look or `LINKS_EVERY` has passed; local git
    /// alone, and never in a contributor's PR checked out for review.
    fn inbox_links_ask(&mut self, tabs: &[TabInfo]) {
        let mut tickets: Vec<String> = self
            .sources
            .views
            .get(INBOX)
            .map(|v| {
                v.items
                    .iter()
                    .filter_map(|i| i.data["ticket"].as_str())
                    .filter(|t| !t.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        tickets.sort();
        tickets.dedup();
        let kept = &mut self.sources.kept;
        let fresh = kept.inbox_links_for == tickets
            && kept
                .inbox_links_at
                .is_some_and(|at| at.elapsed() < LINKS_EVERY);
        if kept.inbox_links_busy || fresh || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        kept.inbox_links_busy = true;
        kept.inbox_links_at = Some(Instant::now());
        kept.inbox_links_for = tickets.clone();
        let mut clones: Vec<PathBuf> = tabs
            .iter()
            .filter_map(|t| t.path.clone())
            .filter(|p| !crate::trust::in_review_dir(p))
            .collect();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            for c in &mut clones {
                *c = crate::tabs::git::main_clone(c);
            }
            clones.sort();
            clones.dedup();
            let found = inbox::links(&clones, &tickets);
            let _ = tx.send(Event::Sources(Answer::InboxLinks(found)));
        });
    }

    /// Gives today's todo note what a view last read, as the daemon handed
    /// todo.js its views: each item as a line (`todo_line`), the PR board's
    /// open ones under their column ("In review: ..."), the merged left out,
    /// and the ticket board's not done or closed; and whether the look failed.
    fn hand_to_todo(&mut self, name: &str) {
        let Some(view) = self.sources.views.get(name) else {
            return;
        };
        let lines = view
            .items
            .iter()
            .filter_map(|i| {
                let column = i.group.as_deref().unwrap_or("");
                match name {
                    PRS if column == "Merged" => None,
                    PRS if !column.is_empty() => Some(format!("{column}: {}", todo_line(i))),
                    TICKETS if column == "Done" || column == "Closed" => None,
                    // The todo hears of failing PRs only, not those running
                    // or passing the CI view lists too.
                    CI if i
                        .data
                        .pointer("/ci/state")
                        .and_then(Value::as_str)
                        .is_some_and(|state| state != "failing") =>
                    {
                        None
                    }
                    _ => Some(todo_line(i)),
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
        // The CI view lists the default branches' CI too (mainci.rs).
        let names: Vec<&'static str> = match view {
            Some(CI) => vec!["checks", "mainCi"],
            Some(view) => lookup_of(view).into_iter().collect(),
            None => VIEWS
                .iter()
                .filter_map(|v| lookup_of(v))
                .chain(["merges", "mainCi"])
                .collect(),
        };
        for name in names {
            if self.sources.schedule.ask(name, Instant::now()) {
                self.start_lookup_named(name, &tabs);
            }
        }
    }

    /// Mission control went to a view: the Inbox and CI views ask again at
    /// most a minute apart, the Landing view its merge queues at most 15
    /// seconds apart, as the daemon's `cameTo` did.
    pub fn came_to_view(&mut self, view: &str) {
        let every = match view {
            INBOX | CI => Duration::from_secs(60),
            LANDING => Duration::from_secs(15),
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
                        let data = if *v == LANDING {
                            self.landing_view(true)
                        } else {
                            self.sources.view(v).cloned()
                        };
                        let data = data
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
    pub(crate) fn source_tabs(&mut self) -> Vec<TabInfo> {
        let mut tabs = Vec::new();
        for session in &self.sessions {
            for (i, id) in session.tabs.iter().enumerate() {
                let Some(tab) = self.tabs.get(id) else {
                    continue;
                };
                let path = self.tab_path(*id).map(Path::to_path_buf);
                // Where it works (landing.js's `dirsOf`): its Claude
                // session's directory first, then its own, then its panes'.
                let panes = tab.layout.panes();
                let claude_dir = self
                    .agents
                    .leader(&panes)
                    .map(|(p, _, _)| p)
                    .into_iter()
                    .chain(panes.iter().copied())
                    .find_map(|p| {
                        let cwd = self.sidebar.panes.get(&p)?.session.as_ref()?.cwd.as_ref()?;
                        Some(PathBuf::from(cwd))
                    });
                let mut dirs: Vec<PathBuf> = claude_dir.into_iter().collect();
                if let Some(p) = path.as_ref().filter(|p| !dirs.contains(p)) {
                    dirs.push(p.clone());
                }
                // Who it is, for the landing queue's entries: every pane,
                // a Claude parked behind the diff or lazygit included.
                let with_parked = self.tab_panes(*id);
                let sessions: Vec<String> = with_parked
                    .iter()
                    .filter_map(|p| self.sidebar.panes.get(p)?.session.as_ref())
                    .map(|s| s.session_id.clone())
                    .filter(|id| !id.is_empty())
                    .collect();
                for &pane in &panes {
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
                    if let Some(cwd) = self.pane_dir(pane)
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
                    pr_in: self.tab_pr_repo(*id).cloned(),
                    repo: None,
                    upstream: None,
                    keys: Vec::new(),
                    dirs,
                    panes: with_parked,
                    sessions,
                });
            }
        }
        let kept = &mut self.sources.kept;
        let mut remotes = |dir: &Path| kept.remotes(dir);
        for tab in &mut tabs {
            let here = tab.path.as_deref().map(&mut remotes).unwrap_or_default();
            tab.repo = here.origin;
            tab.upstream = here.upstream;
            tab.keys = repo::keys_of(
                &tab.name,
                tab.path.as_deref(),
                tab.pr,
                tab.pr_repo(),
                &mut remotes,
            );
        }
        let live: HashSet<&Path> = tabs.iter().filter_map(|t| t.path.as_deref()).collect();
        if kept.repos.len() > 4 * live.len() + 16 {
            kept.repos.retain(|dir, _| live.contains(dir.as_path()));
        }
        tabs
    }

    /// The worktrees no tab is in, from what mission control's Work map
    /// has listed (each repository's worktrees and those `wt-revive`
    /// knows), with what their PR is asked for by: their branch, as the
    /// daemon asked about them beside the tabs. Kept for the Work map.
    pub fn untabbed_prs(&mut self) {
        let tabs: HashSet<PathBuf> = self
            .sessions
            .iter()
            .flat_map(|s| s.tabs.clone())
            .filter_map(|t| self.tab_path(t).map(Path::to_path_buf))
            .collect();
        let mut trees: Vec<(PathBuf, String)> = self
            .mission
            .worktrees
            .values()
            .flatten()
            .filter(|w| !w.main)
            .filter_map(|w| Some((w.path.clone(), w.branch.clone()?)))
            .collect();
        for r in &self.mission.revived {
            if !r.branch.is_empty() && !trees.iter().any(|(p, _)| *p == r.tree) {
                trees.push((r.tree.clone(), r.branch.clone()));
            }
        }
        let kept = &mut self.sources.kept;
        let mut found = HashMap::new();
        for (path, branch) in trees.into_iter().filter(|(p, _)| !tabs.contains(p)) {
            let keys = repo::branch_keys(&kept.remotes(&path), &branch);
            if !keys.is_empty() {
                found.insert(path, keys);
            }
        }
        self.sources.untabbed = found;
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
                // The tabs' PRs, and those of the worktrees no tab is in.
                let mut keys: Vec<PrKey> = Vec::new();
                for key in tabs
                    .iter()
                    .flat_map(|t| &t.keys)
                    .chain(self.sources.untabbed.values().flatten())
                {
                    if !keys.contains(key) {
                        keys.push(key.clone());
                    }
                }
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
                let repos = pr_repos_of(tabs);
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
                let repos = pr_repos_of(tabs);
                if repos.is_empty() {
                    setup::no_repos(self.sources.views.entry(CI).or_default());
                    return false;
                }
                let kept: Vec<String> = self
                    .sources
                    .main_ci
                    .iter()
                    .filter_map(|i| i.data.pointer("/ci/job").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect();
                tokio::spawn(
                    async move { send(Answer::Checks(ciboard::fetch(repos, kept).await)) },
                );
            }
            "mainCi" => {
                let watched = if settings::is_on("main_ci_watch") && settings::is_on("github") {
                    self.main_ci_watched(tabs)
                } else {
                    Vec::new()
                };
                if watched.is_empty() {
                    if !self.sources.main_ci.is_empty() {
                        self.take_main_ci(Vec::new());
                    }
                    return false;
                }
                tokio::spawn(async move { send(Answer::MainCi(mainci::fetch(watched).await)) });
            }
            "mergeQueues" => {
                if !enabled(LANDING) || !settings::is_on("github") {
                    return false;
                }
                repos.sort();
                // No repository to ask about is nothing wrong: nothing lands
                // through a merge queue.
                if repos.is_empty() {
                    self.sources.queues = queues::Queues::default();
                    setup::answered(self.sources.views.entry(LANDING).or_default());
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

    // The default branches to watch (mainci.rs): the repositories the tabs
    // are in, as the landing queues' last read found them, that autopilot
    // works on (a tab it opened is in one), where a tab holds or waits for
    // the default branch's landing lock, or where a landing let the lock go
    // to wait for the default branch to pass (queue-and-clean's phase
    // "waiting for <branch> to pass"), and whose landings test in CI.
    fn main_ci_watched(&self, tabs: &[TabInfo]) -> Vec<mainci::Watched> {
        // A tab's phase is its Claude's (`@agent_phase`, which `land-lock
        // phase` sets on its pane).
        let waiting = |tab: &crate::mux::server::Tab| {
            tab.layout.panes().iter().any(|p| {
                self.agents
                    .get(*p)
                    .and_then(|a| a.phase.as_deref())
                    .is_some_and(|p| p.starts_with("waiting for ") && p.ends_with(" to pass"))
            })
        };
        let busy_tabs: Vec<String> = tabs
            .iter()
            .filter(|t| {
                self.tabs.get(&t.id).is_some_and(|tab| {
                    tab.opts.contains_key(crate::autopilot::OPENED) || waiting(tab)
                })
            })
            .filter_map(|t| t.repo.as_ref().map(ToString::to_string))
            .collect();
        let busy: Vec<(String, String)> = self
            .sources
            .landing
            .repos
            .iter()
            .filter(|r| r.target.is_none())
            .filter_map(|r| {
                let repo = r.repo.clone()?;
                let queued = r.holder.is_some() || !r.waiting.is_empty();
                let worked_on = busy_tabs.iter().any(|a| a.eq_ignore_ascii_case(&repo));
                (queued || worked_on).then_some((repo, r.branch.clone()?))
            })
            .collect();
        if busy.is_empty() {
            return Vec::new();
        }
        let cache = crate::repos::cache_file();
        mainci::watched(&busy, &crate::repos::read_safely(), &|repo| {
            crate::repos::cached_in(&cache, repo).map(|c| c.landing)
        })
    }

    // Reads the landing queues on a blocking thread, unless a read is still
    // under way; with neither the landing_queue setting nor the Landing
    // view on, forgets them.
    fn start_landing(&mut self, tabs: &[TabInfo]) {
        if !settings::is_on("landing_queue") && !enabled(LANDING) {
            self.sources.landing = Read::default();
            return;
        }
        if self.sources.kept.landing_busy || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        self.sources.kept.landing_busy = true;
        let places: Vec<landing::TabPlace> = tabs
            .iter()
            .map(|t| landing::TabPlace {
                id: t.id,
                dirs: t.dirs.clone(),
                panes: t.panes.clone(),
                sessions: t.sessions.clone(),
            })
            .collect();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let running = landing::running_sessions(&crate::agent::sessions::sessions_dir());
            let read = landing::landing_of(&places, &running);
            let _ = tx.send(Event::Sources(Answer::Landing(read)));
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
        if let (Some(view), Some(issues)) = (
            self.sources.views.get_mut(TICKETS),
            &self.sources.kept.ticket_issues,
        ) {
            view.items = tickets::board_from(issues, tabs, crate::settings::is_on("ux2"));
        }
        self.redraw_everywhere();
    }

    // Every tab's facts from what is known, the PR's only with the github
    // setting on and the landing queue's only with landing_queue on: its
    // place in the landing queue, else its PR's in the merge queue, from
    // the merge queues last asked about, else from the PR's own lookup.
    fn update_facts(&mut self, tabs: &[TabInfo]) {
        let github = settings::is_on("github");
        let landing_queue = settings::is_on("landing_queue");
        let mut facts = Facts::new();
        for tab in tabs {
            let pr = tab
                .keys
                .iter()
                .filter(|_| github)
                .find_map(|k| self.sources.known.get(k))
                .cloned();
            let landing = self
                .sources
                .landing
                .places
                .get(&tab.id)
                .map(|p| p.landing)
                .or_else(|| pr.as_ref().and_then(|p| merging(p, &self.sources.queues)))
                .filter(|_| landing_queue);
            if pr.is_some() || landing.is_some() {
                facts.insert(tab.id, TabFacts { pr, landing });
            }
        }
        if facts != self.facts {
            self.facts = facts;
            self.redraw_everywhere();
        }
        let keys: HashSet<&PrKey> = tabs
            .iter()
            .flat_map(|t| &t.keys)
            .chain(self.sources.untabbed.values().flatten())
            .collect();
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

/// A PR's place in its repository's merge queue, as landing there: from the
/// merge queue last asked about, else the PR's own lookup; none once GitHub
/// has said it merged, whatever the queue said last.
fn merging(pr: &PrFacts, queues: &queues::Queues) -> Option<Landing> {
    if pr.state == PrState::Merged || pr.merge == Some(Merge::Merged) {
        return None;
    }
    let listed = queues
        .by_repo
        .iter()
        .find(|(repo, _)| repo.eq_ignore_ascii_case(&pr.repo))
        .and_then(|(_, q)| {
            q.entries.iter().find(|e| {
                e.pointer("/pullRequest/number").and_then(Value::as_u64) == Some(pr.number)
            })
        })
        .and_then(|e| e.get("position").and_then(Value::as_u64))
        .map(|p| p as u32);
    let line = listed.or(match pr.merge {
        Some(Merge::Queued { position }) => Some(position),
        _ => None,
    })?;
    (line > 0).then_some(Landing {
        holding: line == 1,
        line,
        method: Method::Pr,
    })
}

impl Server {
    /// Mission control's Landing view, with the view_landing setting on:
    /// there only while something lands in a repository a tab is in, while
    /// `keep` (a client has it in front, so it doesn't vanish under you once
    /// the last landing ends), or while the merge queues can't be asked of
    /// GitHub, saying why. Built from what the server holds: the landing
    /// queues last read, the merge queues last asked about, and the tabs.
    pub fn landing_view(&mut self, keep: bool) -> Option<ViewData> {
        if !enabled(LANDING) {
            return None;
        }
        let github = settings::is_on("github");
        let status = self.sources.views.get(LANDING).cloned().unwrap_or_default();
        let infos = self.source_tabs();
        let models = self.sidebar_all_tabs();
        let tabs: Vec<landing::ViewTab> = models
            .iter()
            .map(|t| landing::ViewTab {
                id: t.id,
                index: t.index,
                name: t.name.clone(),
                phase: t.phase.clone(),
                waiting: t.state == crate::sidebar::model::TabState::Waiting,
            })
            .collect();
        let by_repo = &self.sources.queues.by_repo;
        let queued = |repo: &str| -> Vec<Item> {
            if !github {
                return Vec::new();
            }
            let one: std::collections::BTreeMap<String, queues::Queue> = by_repo
                .iter()
                .filter(|(r, _)| r.eq_ignore_ascii_case(repo))
                .map(|(r, q)| (r.clone(), q.clone()))
                .collect();
            queues::queue_items(&one, &infos)
        };
        let items = landing::view_items(&self.sources.landing.repos, &queued, &tabs);
        let unanswered = &self.sources.queues.unanswered;
        let error = github
            .then(|| {
                status.error.clone().or_else(|| {
                    (!unanswered.is_empty()).then(|| {
                        format!(
                            "GitHub couldn't say what is in the merge queue of {}.",
                            unanswered.join(", ")
                        )
                    })
                })
            })
            .flatten();
        let setup = status.setup.clone().filter(|_| github);
        if items.is_empty() && !keep && error.is_none() && setup.is_none() {
            return None;
        }
        Some(ViewData {
            items,
            setup,
            error,
            empty: Some("Nothing is landing now.".into()),
            at: Some(status.at.unwrap_or(1)),
            ..ViewData::default()
        })
    }
}

/// The file that asks the PR lookups again: `github` in the state directory.
pub fn github_file() -> PathBuf {
    paths::state_dir().join("github")
}

/// An item as a line of today's todo note's prompt (daemon.js's `line`): its
/// title and detail, then its link, so the note can link to it.
fn todo_line(item: &Item) -> String {
    let mut line = [item.title.as_str(), item.detail.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let data = |pointer: &str| {
        item.data
            .pointer(pointer)
            .and_then(Value::as_str)
            .filter(|u| !u.is_empty())
    };
    let url = match &item.target {
        crate::items::Target::Url(u) => Some(u.as_str()),
        _ => data("/url").or_else(|| data("/ci/url")),
    };
    if let Some(url) = url {
        line.push(' ');
        line.push_str(url);
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::Target;

    #[test]
    fn a_directory_s_origin_is_what_was_last_read_of_it_with_no_read_of_its_own() {
        let dir = crate::mux::socket::testing::TempDir::new();
        let clone = dir.path().join("web");
        std::fs::create_dir_all(clone.join(".git")).unwrap();
        std::fs::write(clone.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(
            clone.join(".git/config"),
            "[remote \"origin\"]\n\turl = git@github.com:acme/web.git\n",
        )
        .unwrap();
        let mut kept = Kept::default();
        assert!(kept.origin_kept(&clone).is_none(), "not read yet");
        kept.remotes(&clone);
        assert_eq!(
            kept.origin_kept(&clone).map(ToString::to_string).as_deref(),
            Some("acme/web")
        );
        let outside = dir.path().join("plain");
        std::fs::create_dir_all(&outside).unwrap();
        kept.remotes(&outside);
        assert!(kept.origin_kept(&outside).is_none(), "outside a repository");
    }

    #[test]
    fn a_todo_line_is_the_title_the_detail_and_the_link() {
        let pr = Item::new("1", "#12 Fix the login")
            .detail("acme/app · checks passing")
            .target(Target::Url("https://github.com/acme/app/pull/12".into()));
        assert_eq!(
            todo_line(&pr),
            "#12 Fix the login · acme/app · checks passing https://github.com/acme/app/pull/12"
        );
        let ci = Item::new("2", "#3 Tidy").data(json!({ "ci": { "url": "https://x/3" } }));
        assert_eq!(todo_line(&ci), "#3 Tidy https://x/3");
    }
}
