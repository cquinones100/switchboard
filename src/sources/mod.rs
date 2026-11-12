//! What the sidebar and mission control know from GitHub and Linear, looked
//! up in the background, as the sidebar daemon's lookups did
//! (lib/sidebar/daemon.js's `polled` and `guarded`, and the files each one
//! came from):
//!
//! - `schedule`: when each lookup is due: every 15 seconds with the
//!   `poll_fast` setting on, else at its slow interval, never twice at once,
//!   paused while you're away and all asked again on your return, and asked
//!   again soon when something appends to the `github` file in the state
//!   directory.
//! - `repo`: the GitHub repository a tab's directory belongs to, and in a
//!   fork's clone the project its `upstream` remote names, read from git's
//!   config file rather than by running git.
//! - `prs`: every tab's pull request, in one GraphQL query, into
//!   `Server.facts` (lib/sidebar/prs.js).
//! - `landing`: what lands in each repository a tab is in, and each tab's
//!   place in it: the files land-lock keeps (lib/sidebar/landing.js), and
//!   the merge queues; mission control's Landing view.
//! - `prboard`, `ciboard`, `queues`, `tickets`, `inbox`: mission control's
//!   PRs and CI views, the merge queues the Landing view shows, and the
//!   Tickets and Inbox views, as `ViewData`.
//! - `mainci`: the default branch's CI, for each repository whose landings
//!   test in CI while autopilot works on it or a tab waits to land on it: a
//!   failing tip as an item of the CI view and a notification.
//! - `setup`: what a view says when GitHub or Linear isn't set up
//!   (lib/sidebar/setup-help.js).
//! - `serve`: the server's side: starting the lookups on its tick, taking
//!   their answers, and what mission control reads (`Sources::view`).
//!
//! Every lookup runs in a task of its own and answers with
//! `Event::Sources`; the server keeps only what the last answer said, so a
//! draw never waits on the network, and a failed look keeps what was known
//! with its error above it.

pub mod ciboard;
pub mod inbox;
pub mod landing;
pub mod mainci;
pub mod prboard;
pub mod prnotices;
pub mod prs;
pub mod queues;
pub mod repo;
pub mod schedule;
mod serve;
pub mod setup;
pub mod tickets;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use crate::facts::PrFacts;
use crate::integrations::github::Repo;
use crate::items::{Item, ViewData};
use crate::mux::{PaneId, TabId};
use crate::settings;

pub use landing::Waiter;
pub use repo::PrKey;
pub use serve::Answer;

/// The views this part gives mission control, by the name `Sources::view`
/// takes.
pub const PRS: &str = "prs";
pub const CI: &str = "ci";
/// What is landing: each repository's landing queue and merge queue (the
/// Merge queues view before).
pub const LANDING: &str = "landing";
pub const TICKETS: &str = "tickets";
pub const INBOX: &str = "inbox";
pub const VIEWS: [&str; 5] = [PRS, CI, LANDING, TICKETS, INBOX];

/// What a lookup knows of a tab, taken from the server when it is asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabInfo {
    pub id: TabId,
    /// Its number (`@tab_number`), as the views say "tab 3".
    pub index: usize,
    /// Its label, as the tab bar shows it.
    pub name: String,
    /// The directory it is named from.
    pub path: Option<PathBuf>,
    /// Its branch's PR, from the tab names' lookup.
    pub pr: Option<u64>,
    /// The repository that PR is in, where the lookup found it; `None` when
    /// only its number is known (a `wtpr` directory's), its origin's then.
    pub pr_in: Option<Repo>,
    /// The GitHub repository its directory belongs to: its origin's.
    pub repo: Option<Repo>,
    /// In a fork's clone, the project its `upstream` remote names, where
    /// its PRs may be.
    pub upstream: Option<Repo>,
    /// What its PR is asked for by, when it leads to one: the first GitHub
    /// has an answer for is its PR (origin's before upstream's).
    pub keys: Vec<PrKey>,
    /// Where it works, for the landing queue: its directory, then its panes'
    /// (Notes, which M-e may have put beside it and focused, left out).
    pub dirs: Vec<PathBuf>,
    /// Its panes and the Claude sessions running in them, which place it in
    /// the landing queue by the pane or session that took the lock.
    pub panes: Vec<PaneId>,
    pub sessions: Vec<String>,
}

impl TabInfo {
    /// The repository its PR is in: where the lookup found it, else its
    /// origin.
    pub fn pr_repo(&self) -> Option<&Repo> {
        self.pr_in.as_ref().or(self.repo.as_ref())
    }
}

impl Default for TabInfo {
    fn default() -> TabInfo {
        TabInfo {
            id: TabId(0),
            index: 0,
            name: String::new(),
            path: None,
            pr: None,
            pr_in: None,
            repo: None,
            upstream: None,
            keys: Vec::new(),
            dirs: Vec::new(),
            panes: Vec::new(),
            sessions: Vec::new(),
        }
    }
}

/// What the sources keep on the server.
#[derive(Default)]
pub struct Sources {
    schedule: schedule::Schedule,
    /// What GitHub last said of each PR the tabs lead to.
    known: HashMap<PrKey, PrFacts>,
    /// The keys asked about at least once, so a tab's new PR is asked about
    /// at once rather than on the next round.
    asked: HashSet<PrKey>,
    /// The worktrees no tab is in, by their path, with what their PR is
    /// asked for by (their branch, in origin and a fork's upstream), so the
    /// Work map knows which merged (the daemon's `untabbed` stand-ins for
    /// tabs).
    untabbed: HashMap<PathBuf, Vec<PrKey>>,
    /// Each tab's place in the landing queue, and where its worktree is
    /// when it waits, for moving it; each repository the tabs are in, with
    /// what lands there straight on its default branch.
    landing: landing::Read,
    board: prboard::Board,
    queues: queues::Queues,
    /// The watched default branches failing at their last look
    /// (`mainci`), and your PRs' failing checks, which the CI view lists
    /// after them.
    main_ci: Vec<Item>,
    checks: Vec<Item>,
    /// Your PRs' checks past the CI search's first page, as last read.
    ci_kept: ciboard::Kept,
    /// The failing tips whose notification you dismissed, by their item's id.
    main_ci_dismissed: HashSet<String>,
    /// What the PR board's last answer said of each PR, and your PRs' news
    /// found from it, kept until dismissed (`prnotices`).
    pr_known: prnotices::Known,
    pr_notices: Vec<Item>,
    views: BTreeMap<&'static str, ViewData>,
    /// When a view was last asked for by going to it, for asking at most so
    /// often (the daemon's `cameTo`).
    came: HashMap<&'static str, std::time::Instant>,
    kept: serve::Kept,
}

impl Sources {
    /// A directory's origin as `owner/name`, as the lookups last read it,
    /// with no read of its own, so the drawing path may ask
    /// (`source_tabs` keeps it fresh).
    pub(crate) fn origin_kept(&self, dir: &std::path::Path) -> Option<String> {
        self.kept.origin_kept(dir).map(ToString::to_string)
    }

    /// A view's data, or `None` while its settings keep it out of mission
    /// control (the service off, or the view): `PRS`, `CI`, `TICKETS` or
    /// `INBOX`; `LANDING`'s is only how its merge queues were last asked,
    /// the view itself put together on the server (`Server::landing_view`).
    /// Before its first answer it is an empty `ViewData` with no `at`, for a
    /// view to say it is still looking.
    pub fn view(&self, name: &str) -> Option<&ViewData> {
        if !enabled(name) {
            return None;
        }
        static EMPTY: ViewData = ViewData {
            items: Vec::new(),
            setup: None,
            error: None,
            empty: None,
            columns: Vec::new(),
            footer: Vec::new(),
            head: serde_json::Value::Null,
            at: None,
        };
        Some(self.views.get(name).unwrap_or(&EMPTY))
    }

    /// The landing queue's place for a waiting tab's worktree, for its menu
    /// (Move up, Move down, Move to the front through `land-lock move`).
    pub fn landing_waiter(&self, tab: TabId) -> Option<&Waiter> {
        self.landing.places.get(&tab)?.waiter.as_ref()
    }

    /// What GitHub last said of the PR of a worktree no tab is in, by its
    /// branch; none with the github setting off.
    pub fn untabbed_pr(&self, path: &std::path::Path) -> Option<&PrFacts> {
        if !settings::is_on("github") {
            return None;
        }
        self.untabbed
            .get(path)?
            .iter()
            .find_map(|key| self.known.get(key))
    }

    /// What the Autopilot line says while a watched default branch fails;
    /// empty while none does.
    pub fn main_failing(&self) -> String {
        mainci::failing_line(&self.main_ci)
    }

    /// The newest commit naming each of the Inbox's tickets, as the last
    /// look found them.
    pub fn inbox_links(&self) -> &std::collections::HashMap<String, inbox::Commit> {
        &self.kept.inbox_links
    }

    /// Takes a notification off the Inbox at once, once its menu has marked
    /// it read in Linear (`inbox::mark_read`).
    pub fn drop_inbox_item(&mut self, notification: &str) {
        if let Some(view) = self.views.get_mut(INBOX) {
            view.items.retain(|i| i.id != notification);
        }
    }
}

/// Whether a view is on: its service's setting and its own. The Landing
/// view needs no service, since its landing queues are files; its merge
/// queues are asked of GitHub only with the github setting on too.
pub fn enabled(name: &str) -> bool {
    let (service, own) = match name {
        PRS => ("github", "pr_board"),
        CI => ("github", "view_ci"),
        LANDING => return settings::is_on("view_landing"),
        TICKETS => ("linear", "ticket_board"),
        INBOX => ("linear", "view_inbox"),
        _ => return false,
    };
    settings::is_on(service) && settings::is_on(own)
}

/// Whether a tab is in the Build failing group: its open PR has a failing
/// check and is in no merge queue, with the `build_failing` setting on, as
/// model.js's `buildFailing` had it.
pub fn build_failing(pr: Option<&PrFacts>) -> bool {
    pr.is_some_and(|pr| {
        pr.state == crate::facts::PrState::Open
            && pr.merge.is_none()
            && pr.checks.is_some_and(|c| c.fail > 0)
    }) && settings::is_on("build_failing")
}

/// The repositories your PRs from the tabs may be in, each once, in the
/// order first met: each tab's origin, and in a fork's clone the project its
/// upstream names. What the PR board and the CI view search.
pub(crate) fn pr_repos_of(tabs: &[TabInfo]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for repo in tabs.iter().flat_map(|t| t.repo.iter().chain(&t.upstream)) {
        let name = repo.to_string();
        if !seen.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
            seen.push(name);
        }
    }
    seen
}

/// The repositories the tabs are in (their origins), each once, in the
/// order first met: whose merge queues the Landing view shows.
pub(crate) fn repos_of(tabs: &[TabInfo]) -> Vec<String> {
    let mut seen = Vec::new();
    for tab in tabs {
        if let Some(repo) = &tab.repo {
            let name = repo.to_string();
            if !seen.contains(&name) {
                seen.push(name);
            }
        }
    }
    seen
}

/// An item's time from GitHub's or Linear's ISO timestamp.
pub(crate) fn ms_of(iso: Option<&str>) -> Option<i64> {
    iso?.parse::<jiff::Timestamp>()
        .ok()
        .map(|t| t.as_millisecond())
}

/// A JSON value's text at a path, or "".
pub(crate) fn text<'a>(v: &'a serde_json::Value, pointer: &str) -> &'a str {
    v.pointer(pointer)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
}

/// Items with their columns' order kept: the board views give their items
/// column by column, each with its column as `group`.
pub(crate) fn in_columns(columns: &[&str], items: Vec<Item>) -> Vec<Item> {
    let mut out = Vec::with_capacity(items.len());
    for column in columns {
        out.extend(
            items
                .iter()
                .filter(|i| i.group.as_deref() == Some(column))
                .cloned(),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::github::parse_name_with_owner;

    #[test]
    fn the_pr_views_search_each_origin_and_a_fork_s_upstream_once() {
        let tab = |origin: &str, upstream: Option<&str>| TabInfo {
            repo: parse_name_with_owner(origin),
            upstream: upstream.and_then(parse_name_with_owner),
            ..TabInfo::default()
        };
        let tabs = vec![
            tab("me/widgets", Some("acme/widgets")),
            tab("acme/api", None),
            tab("Me/Widgets", Some("acme/widgets")),
        ];
        assert_eq!(
            pr_repos_of(&tabs),
            ["me/widgets", "acme/widgets", "acme/api"]
        );
        // The merge queues stay each tab's origin's.
        assert!(!repos_of(&tabs).contains(&"acme/widgets".to_string()));
    }
}
