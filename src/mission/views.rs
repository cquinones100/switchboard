//! Mission control's views after its tabs (lib/sidebar/views.js's
//! `builtInViews`, and the daemon's `viewsFor`): each a list of items in
//! the one shape every source gives (`crate::items`), from what the server
//! already keeps, so building them reads no network and starts no process.
//! In the daemon's order, each only while its setting is on: the
//! Notifications (the sidebar's, whole), the Work map, the Calendar, Slack,
//! Closed, the PR board, CI, Landing (while something lands), Autopilot
//! (with UX 2.0, while it is on, src/autopilot/view.rs), the ticket
//! board, the Inbox, Sessions, Resources, Usage, Keys (`keymap`), Settings,
//! then each plugin's. With UX 2.0 the bar shows them by feature in groups
//! (`groups`): Work, Code, Day and System.
//!
//! Two are put together here. The Work map is every piece of work in the
//! repositories the tabs are in, one row each: each worktree, and a main
//! checkout a tab is in, with its ticket (from the branch), the tab it is
//! open in or none, its PR and where that stands, and its directory. The
//! worktrees are read with git in the background (`worktrees`), and the
//! worktrees no tab is in anywhere else come from a source of their own
//! (`Revived`, what `wt-revive --json` prints), so work in flight isn't
//! forgotten. Settings is every option of `assets/settings.json` under its
//! category, then your own files' settings (`usage::own`).
//!
//! A view as a client sees it (`shown`) is narrowed by its `/` filter, which
//! matches an item's title, detail and `search`, the boards excepted, and the
//! Work map is scoped to open tabs or to work with no tab for M-j's and
//! M-o's pickers, its merged work hidden under its header until Enter there
//! shows it.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::facts::{Merge, PrFacts, PrState};
use crate::items::{Item, State, Target, ViewData};
use crate::sidebar::model::{MergeState, TabModel, TabState};
use crate::{paths, settings};

/// Which view.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ViewId {
    Notifications,
    WorkMap,
    Calendar,
    Slack,
    Closed,
    Prs,
    Ci,
    Landing,
    /// Autopilot's own view, with UX 2.0 on (src/autopilot/view.rs).
    Autopilot,
    Tickets,
    Inbox,
    Sessions,
    Resources,
    Usage,
    Keys,
    Settings,
    Plugin(String),
}

/// One view: its items and what it says of them.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub id: ViewId,
    pub title: String,
    pub data: ViewData,
    /// Whether its source has answered at least once.
    pub loaded: bool,
}

impl View {
    /// A board's columns, in their order (the PR and ticket boards).
    pub fn columns(&self) -> Option<&'static [&'static str]> {
        match self.id {
            ViewId::Prs => Some(crate::sources::prboard::columns(crate::settings::is_on(
                "ux2",
            ))),
            ViewId::Tickets => Some(crate::sources::tickets::columns(crate::settings::is_on(
                "ux2",
            ))),
            _ => None,
        }
    }

    /// Whether it is drawn as a table (Resources).
    pub fn table(&self) -> bool {
        !self.data.columns.is_empty()
    }

    /// Its name as M-p's "Mission control: <view>" names it: lowercased,
    /// dashes for spaces.
    pub fn slug(&self) -> String {
        self.title.to_lowercase().replace(' ', "-")
    }
}

/// With UX 2.0, the bar's groups of views by feature, in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Work,
    Code,
    Day,
    System,
}

impl Group {
    pub fn name(self) -> &'static str {
        match self {
            Group::Work => "Work",
            Group::Code => "Code",
            Group::Day => "Day",
            Group::System => "System",
        }
    }
}

/// A view's group and its place in it, after the tabs, which lead Work.
/// Closed and Sessions stand where the canvas's Reopen will, and each
/// plugin's view comes after Keys.
fn place(id: &ViewId) -> (Group, u8) {
    match id {
        ViewId::Landing => (Group::Work, 1),
        ViewId::Autopilot => (Group::Work, 2),
        ViewId::WorkMap => (Group::Work, 3),
        ViewId::Closed => (Group::Work, 4),
        ViewId::Sessions => (Group::Work, 5),
        ViewId::Prs => (Group::Code, 0),
        ViewId::Tickets => (Group::Code, 1),
        ViewId::Ci => (Group::Code, 2),
        ViewId::Calendar => (Group::Day, 0),
        ViewId::Slack => (Group::Day, 1),
        ViewId::Inbox => (Group::Day, 2),
        ViewId::Notifications => (Group::Day, 3),
        ViewId::Usage => (Group::System, 0),
        ViewId::Resources => (Group::System, 1),
        ViewId::Settings => (Group::System, 2),
        ViewId::Keys => (Group::System, 3),
        ViewId::Plugin(_) => (Group::System, 4),
    }
}

/// The bar's groups that have a view, each with its views in their order,
/// as places in mission control (0 the tabs, `i` the `i`th view after).
pub fn groups(views: &[View]) -> Vec<(Group, Vec<usize>)> {
    let mut placed: Vec<(Group, u8, usize)> = std::iter::once((Group::Work, 0, 0))
        .chain(views.iter().enumerate().map(|(i, v)| {
            let (g, at) = place(&v.id);
            (g, at, i + 1)
        }))
        .collect();
    placed.sort_by_key(|(g, at, _)| (*g as u8, *at));
    let mut out: Vec<(Group, Vec<usize>)> = Vec::new();
    for (g, _, i) in placed {
        match out.last_mut() {
            Some((last, held)) if *last == g => held.push(i),
            _ => out.push((g, vec![i])),
        }
    }
    out
}

/// A view's name in the grouped bar, where the Work map is the Map.
pub fn bar_title(view: &View) -> &str {
    match view.id {
        ViewId::WorkMap => "Map",
        _ => &view.title,
    }
}

/// Which of the Work map's rows M-j and M-o show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// M-j: the open tabs.
    Tabs,
    /// M-o: the work no tab has open.
    Untabbed,
}

/// Whether an item is a heading over the items after it (a Settings
/// category, the Work map's merged work) rather than one to act on.
pub fn is_header(item: &Item) -> bool {
    item.data.get("header").and_then(Value::as_bool) == Some(true)
}

/// Whether an item is a view's summary (the Autopilot view's heading).
pub fn is_summary(item: &Item) -> bool {
    item.data.get("summary").and_then(Value::as_bool) == Some(true)
}

pub(crate) fn header(id: &str, title: &str, count: usize) -> Item {
    Item::new(format!("header:{id}"), title).data(json!({ "header": true, "count": count }))
}

/// Whether a view lists its rows under headings to pass over (Settings'
/// categories, the Keys view's groups, Landing's repositories, CI's
/// failing, running and passing): the cursor
/// never rests on one, and a filter leaves out those with nothing under
/// them.
pub fn grouped(id: &ViewId) -> bool {
    matches!(
        id,
        ViewId::Settings | ViewId::Keys | ViewId::Landing | ViewId::Autopilot | ViewId::Ci
    )
}

/// The CI view's sections, in order: its items' `ci.state` (the default
/// branch's failing CI, which has none, is failing), the heading's id and
/// its title.
pub const CI_SECTIONS: [(&str, &str); 3] = [
    ("failing", "Failing"),
    ("running", "Running"),
    ("passing", "Passing"),
];

/// Where a CI item stands: `failing`, `running` or `passing`.
pub fn ci_state(item: &Item) -> &str {
    item.data
        .pointer("/ci/state")
        .and_then(Value::as_str)
        .unwrap_or("failing")
}

/// A failing CI item's failed checks: a PR's each, or the default
/// branch's first (src/sources/mainci.rs), which lists only that one.
pub fn ci_checks(item: &Item) -> Vec<&Value> {
    if ci_state(item) != "failing" {
        return Vec::new();
    }
    match item.data.pointer("/ci/checks").and_then(Value::as_array) {
        Some(checks) => checks.iter().collect(),
        None => item
            .data
            .pointer("/ci/check")
            .filter(|c| c.is_object())
            .into_iter()
            .collect(),
    }
}

/// What tells a failed check from the PR's others: its name and its page
/// together, since two workflows' checks may share a name and two
/// statuses a page.
pub fn ci_check_key(check: &Value) -> String {
    format!(
        "{}\t{}",
        check["name"].as_str().unwrap_or(""),
        check["url"].as_str().unwrap_or("")
    )
}

/// The CI view's items under a heading for each section that has any,
/// with its count, each section in the order its items came.
pub fn ci_sections(items: &[Item]) -> Vec<Item> {
    let mut out = Vec::new();
    for (state, title) in CI_SECTIONS {
        let these: Vec<&Item> = items.iter().filter(|i| ci_state(i) == state).collect();
        if these.is_empty() {
            continue;
        }
        out.push(header(state, title, these.len()));
        out.extend(these.into_iter().cloned());
    }
    out
}

/// What a `/` filter matches an item by.
pub fn search_text(item: &Item) -> String {
    let search = item
        .data
        .get("search")
        .and_then(Value::as_str)
        .unwrap_or("");
    format!("{} {} {search}", item.title, item.detail).to_lowercase()
}

/// A view as one client sees it: its `/` filter's text, the Work map's
/// scope, and whether its merged work is shown.
pub fn shown(view: &View, filter: &str, scope: Option<Scope>, show_merged: bool) -> View {
    let text = filter.trim().to_lowercase();
    let work_map = view.id == ViewId::WorkMap;
    let scope = scope.filter(|_| work_map);
    if !work_map && (text.is_empty() || view.columns().is_some()) {
        return view.clone();
    }
    let open_merged = show_merged || !text.is_empty();
    let kept: Vec<Item> = view
        .data
        .items
        .iter()
        .filter(|i| !merged(i) || open_merged)
        .filter(|i| {
            let scoped = match scope {
                None => true,
                Some(Scope::Tabs) => matches!(i.target, Target::Tab(_)),
                Some(Scope::Untabbed) => !matches!(i.target, Target::Tab(_)),
            };
            // A heading, which is no tab, stays with the work no tab has
            // (the merged work's, under M-o), as views.js kept it.
            scoped && (text.is_empty() || is_header(i) || search_text(i).contains(&text))
        })
        .cloned()
        .map(|mut i| {
            if i.id == MERGED_HEADER {
                let mark = if open_merged { "▾" } else { "▸" };
                i.title = format!("{mark} {}", i.title);
            }
            i
        })
        .collect();
    // A filtered Settings or Keys view leaves out the headings with no
    // match, but never the Autopilot view's summary, which heads it
    // whatever matches.
    let items = if grouped(&view.id) {
        kept.iter()
            .enumerate()
            .filter(|(at, i)| {
                !is_header(i) || is_summary(i) || kept.get(at + 1).is_some_and(|n| !is_header(n))
            })
            .map(|(_, i)| i.clone())
            .collect()
    } else {
        kept
    };
    let title = match scope {
        Some(Scope::Tabs) => format!("{} · open tabs", view.title),
        Some(Scope::Untabbed) => format!("{} · no tab", view.title),
        None => view.title.clone(),
    };
    View {
        title,
        data: ViewData {
            items,
            ..view.data.clone()
        },
        ..view.clone()
    }
}

fn merged(item: &Item) -> bool {
    item.data.get("merged").and_then(Value::as_bool) == Some(true)
}

/// Whether a source's view has answered: its time, or why it has none.
pub fn loaded(data: &ViewData) -> bool {
    data.at.is_some() || data.setup.is_some() || data.error.is_some()
}

// ── Settings ────────────────────────────────────────────────────────────────

/// The Settings view's rows: each category a heading, then its options, the
/// name, the value and what it does (an option at "off" drawn as idle), then
/// your own files' settings under a heading each.
pub fn settings_items(own: &[Item]) -> Vec<Item> {
    let all = settings::current_settings();
    let mut categories: Vec<&str> = Vec::new();
    for o in &all {
        if !categories.contains(&o.category.as_str()) {
            categories.push(&o.category);
        }
    }
    let mut items = Vec::new();
    for category in categories {
        let members: Vec<&settings::Setting> =
            all.iter().filter(|o| o.category == category).collect();
        items.push(header(category, category, members.len()));
        for o in members {
            let words = o.value_words();
            let off = o.value == Value::Bool(false) || o.value == "off";
            items.push(
                Item::new(format!("setting:{}", o.key), &o.label)
                    .detail(&o.description)
                    .state(if off { State::Idle } else { State::Active })
                    .data(json!({
                        "setting": o.key,
                        "value": words,
                        "search": format!("{category} {} {words}", o.key),
                    })),
            );
        }
    }
    let mut group: Option<&str> = None;
    for (at, item) in own.iter().enumerate() {
        let g = item.group.as_deref().unwrap_or("Your files");
        if group != Some(g) {
            let count = own[at..]
                .iter()
                .take_while(|i| i.group.as_deref().unwrap_or("Your files") == g)
                .count();
            items.push(header(g, g, count));
            group = Some(g);
        }
        items.push(item.clone());
    }
    items
}

// ── The Work map ────────────────────────────────────────────────────────────

/// A worktree as `git worktree list --porcelain` gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    /// The repository's main checkout.
    pub main: bool,
}

/// A worktree no tab has open, from wherever that is known (bin/wt-revive's
/// `--json` rows): its repository and branch, when it was last used and how
/// long ago that was, and the last prompt Claude was given there.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Revived {
    pub tree: PathBuf,
    #[serde(default)]
    pub repo: PathBuf,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub when: i64,
    #[serde(default)]
    pub age: String,
    #[serde(default)]
    pub prompt: String,
}

/// `git worktree list --porcelain`'s entries, the main checkout marked.
pub fn parse_worktrees(repo: &Path, porcelain: &str) -> Vec<Worktree> {
    porcelain
        .split("\n\n")
        .filter_map(|entry| {
            let path = entry
                .lines()
                .find_map(|l| l.strip_prefix("worktree "))
                .map(PathBuf::from)?;
            let branch = entry
                .lines()
                .find_map(|l| l.strip_prefix("branch refs/heads/"))
                .map(str::to_string);
            Some(Worktree {
                main: path == repo,
                path,
                branch,
            })
        })
        .collect()
}

static TICKET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^([a-z][a-z0-9]*-\d+)").expect("a regex"));

/// The Merged header's id, which Enter opens and closes.
pub const MERGED_HEADER: &str = "header:merged";

/// Where a PR stands, in a few words: `#12 merged`, `#12 ✗2 failing`; none
/// with the github setting off.
fn pr_words(tab: &TabModel) -> String {
    if !settings::is_on("github") {
        return String::new();
    }
    let Some(pr) = tab.pr else {
        return "no PR".into();
    };
    let where_ = match tab.merge {
        Some(MergeState::Merged) => "merged".to_string(),
        Some(MergeState::Queued) => match tab.position {
            Some(p) if p > 0 => format!("in merge queue at {p}"),
            _ => "in merge queue".into(),
        },
        None => match tab.checks {
            Some(c) if c.fail > 0 => format!("✗{} failing", c.fail),
            Some(c) if c.pending > 0 => "checks running".into(),
            _ => "open".into(),
        },
    };
    format!("#{pr} {where_}")
}

/// A worktree no tab is in, as far as its PR goes: a tab of no number
/// standing in for it, with what GitHub said of its branch's PR (views.js's
/// `mergeOf`), for `pr_words` and `state_of`.
fn stand_in(pr: Option<&PrFacts>) -> TabModel {
    let mut t = TabModel::new(crate::mux::TabId(0), 0, "");
    if let Some(p) = pr {
        t.pr = Some(p.number);
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
    }
    t
}

/// Whether a row needs you, is going on or waits: its tab's state, or for
/// a worktree no tab is in, only its PR's failing checks.
fn state_of(tab: Option<&TabModel>, facts: &TabModel) -> State {
    let Some(tab) = tab else {
        return if facts.checks.is_some_and(|c| c.fail > 0) {
            State::Attention
        } else {
            State::Idle
        };
    };
    let needs = matches!(tab.state, TabState::Waiting | TabState::Done) || tab.pinned;
    if needs || tab.checks.is_some_and(|c| c.fail > 0) {
        State::Attention
    } else if tab.state != TabState::Idle {
        State::Active
    } else {
        State::Idle
    }
}

/// What a row is called: its tab's name, or its branch in words, without
/// the ticket the row already leads with.
fn named(tab: Option<&TabModel>, w: &Worktree, ticket: Option<&str>) -> String {
    let name = match tab {
        Some(t) => t.name.clone(),
        None => {
            let base = w.branch.clone().unwrap_or_else(|| {
                w.path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            let rest = TICKET.replace(&base, "");
            rest.trim_start_matches(['-', '_'])
                .split(['-', '_'])
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        }
    };
    match ticket {
        Some(t) => {
            let lead = Regex::new(&format!("(?i)^{}:?\\s*", regex::escape(t)));
            match lead {
                Ok(re) => re.replace(&name, "").to_string(),
                Err(_) => name,
            }
        }
        None => name,
    }
}

/// Needing you first, then work going on, open PRs, merged ones, and work
/// no tab is in last, most recently used first.
fn rank(item: &Item) -> u8 {
    let tabbed = matches!(item.target, Target::Tab(_));
    match item.state {
        State::Attention => 0,
        State::Active => 1,
        _ if merged_pr(item) => 3,
        _ if tabbed => 2,
        _ => 4,
    }
}

fn merged_pr(item: &Item) -> bool {
    item.data.get("merge").and_then(Value::as_str) == Some("merged")
}

/// The Work map's rows, from the open tabs, the worktrees of their
/// repositories, and the worktrees no tab is in that are known elsewhere,
/// whose PRs `pr_of` gives by their branch. Every open tab is a row too, one
/// in no repository included, so M-j, which lists the open tabs' rows,
/// reaches them all.
pub fn work_map(
    tabs: &[TabModel],
    worktrees: &[Worktree],
    revived: &[Revived],
    pr_of: &dyn Fn(&Path) -> Option<PrFacts>,
) -> Vec<Item> {
    let mut all: Vec<Worktree> = worktrees.to_vec();
    for r in revived {
        if !all.iter().any(|w| w.path == r.tree) {
            all.push(Worktree {
                path: r.tree.clone(),
                branch: Some(r.branch.clone()).filter(|b| !b.is_empty()),
                main: false,
            });
        }
    }
    // The worktree a tab is in or under, the deepest.
    let owner_of = |t: &TabModel| -> Option<usize> {
        let path = t.path.as_ref()?;
        all.iter()
            .enumerate()
            .filter(|(_, w)| path == &w.path || path.starts_with(&w.path))
            .max_by_key(|(_, w)| w.path.as_os_str().len())
            .map(|(i, _)| i)
    };
    let linear = settings::is_on("linear");
    let mut rows: Vec<(Item, i64)> = Vec::new();
    let mut taken: Vec<crate::mux::TabId> = Vec::new();
    for (i, w) in all.iter().enumerate() {
        let tab = tabs.iter().find(|t| owner_of(t) == Some(i));
        if w.main && tab.is_none() {
            continue;
        }
        if let Some(t) = tab {
            taken.push(t.id);
        }
        let ticket = linear
            .then(|| {
                TICKET
                    .captures(w.branch.as_deref().unwrap_or(""))
                    .map(|m| m[1].to_uppercase())
            })
            .flatten();
        let last = tab
            .is_none()
            .then(|| revived.iter().find(|r| r.tree == w.path))
            .flatten();
        let title = named(tab, w, ticket.as_deref());
        let where_ = match tab {
            Some(t) => format!("tab {}", t.index),
            None => "no tab".into(),
        };
        // What the PR says, the tab's or the branch's for a worktree no
        // tab is in.
        let untabbed = stand_in(tab.is_none().then(|| pr_of(&w.path)).flatten().as_ref());
        let facts = tab.unwrap_or(&untabbed);
        let pr = pr_words(facts);
        let used = last.map(|l| format!("used {}", l.age)).unwrap_or_default();
        let place = paths::tilde(&w.path);
        let prompt = last.map(|l| l.prompt.clone()).unwrap_or_default();
        let detail: Vec<String> = [
            where_.clone(),
            pr.clone(),
            used.clone(),
            place.clone(),
            prompt.clone(),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
        let more: Vec<String> = [place, prompt]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        let merge = facts.merge.map(|m| match m {
            MergeState::Merged => "merged",
            MergeState::Queued => "queued",
        });
        let lead = ticket
            .as_ref()
            .map(|t| format!("{t}  "))
            .unwrap_or_default();
        let mut item = Item::new(w.path.to_string_lossy(), format!("{lead}{title}"))
            .detail(detail.join(" · "))
            .state(state_of(tab, facts))
            .data(json!({
                "merge": merge,
                "merged": tab.is_none() && merge == Some("merged"),
                "branch": w.branch,
            }));
        item.cells = vec![ticket.unwrap_or_default(), title, where_, pr, used];
        item.more = vec![more.join(" · ")]
            .into_iter()
            .filter(|m| !m.is_empty())
            .collect();
        item.target = match tab {
            Some(t) => Target::Tab(t.id),
            None => Target::Dir(w.path.clone()),
        };
        rows.push((item, last.map_or(0, |l| l.when)));
    }
    // The open tabs no worktree row took: in no repository, or a second
    // tab in the same worktree.
    for tab in tabs.iter().filter(|t| !taken.contains(&t.id)) {
        let where_ = format!("tab {}", tab.index);
        let pr = if tab.pr.is_some() {
            pr_words(tab)
        } else {
            String::new()
        };
        let place = tab.path.as_ref().map(paths::tilde).unwrap_or_default();
        let detail: Vec<String> = [where_.clone(), pr.clone(), place.clone()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        let mut item = Item::new(format!("tab:{}", tab.id), &tab.name)
            .detail(detail.join(" · "))
            .state(state_of(Some(tab), tab))
            .target(Target::Tab(tab.id))
            .data(json!({ "merge": tab.merge.map(|m| match m {
                MergeState::Merged => "merged",
                MergeState::Queued => "queued",
            }) }));
        item.cells = vec![String::new(), tab.name.clone(), where_, pr, String::new()];
        item.more = vec![place].into_iter().filter(|m| !m.is_empty()).collect();
        rows.push((item, 0));
    }
    rows.sort_by(|(a, wa), (b, wb)| {
        rank(a).cmp(&rank(b)).then_with(|| {
            if rank(a) == 4 {
                wb.cmp(wa)
            } else {
                std::cmp::Ordering::Equal
            }
        })
    });
    let (merged_rows, rest): (Vec<_>, Vec<_>) = rows.into_iter().partition(|(i, _)| merged(i));
    let mut out: Vec<Item> = rest.into_iter().map(|(i, _)| i).collect();
    if !merged_rows.is_empty() {
        let mut h = header("merged", "Merged, can be removed", merged_rows.len());
        h.id = MERGED_HEADER.into();
        out.push(h);
        out.extend(merged_rows.into_iter().map(|(i, _)| i));
    }
    out
}

/// The view's empty words when nothing at all is there yet.
pub fn work_map_empty() -> String {
    "No work found: open a tab in a git repository, and its worktrees show here.".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::TabId;
    use crate::sidebar::claude::SessionRef;

    #[test]
    fn the_ci_view_heads_its_failing_running_and_passing_prs_in_order_with_their_counts() {
        let pr = |n: u32, state: Option<&str>| {
            let mut ci = json!({ "number": n });
            if let Some(state) = state {
                ci["state"] = json!(state);
            }
            Item::new(n.to_string(), format!("#{n}")).data(json!({ "ci": ci }))
        };
        let items = vec![
            pr(1, Some("passing")),
            pr(2, Some("failing")),
            pr(3, Some("running")),
            pr(4, None),
            pr(5, Some("passing")),
        ];
        let shown: Vec<(String, Option<u64>)> = ci_sections(&items)
            .iter()
            .map(|i| (i.title.clone(), i.data["count"].as_u64()))
            .collect();
        let want = [
            ("Failing", Some(2)),
            ("#2", None),
            ("#4", None),
            ("Running", Some(1)),
            ("#3", None),
            ("Passing", Some(2)),
            ("#1", None),
            ("#5", None),
        ];
        let want: Vec<(String, Option<u64>)> =
            want.iter().map(|(t, c)| (t.to_string(), *c)).collect();
        assert_eq!(
            shown, want,
            "the default branch's, with no state, is failing"
        );
        assert!(
            ci_sections(&[pr(1, Some("passing"))])[0].title == "Passing",
            "no empty heads"
        );
    }

    #[test]
    fn the_bar_groups_the_views_by_feature_leaving_out_a_group_with_none() {
        let v = |id: ViewId, title: &str| View {
            id,
            title: title.into(),
            data: ViewData::default(),
            loaded: true,
        };
        let views = vec![
            v(ViewId::Notifications, "Notifications"),
            v(ViewId::WorkMap, "Work map"),
            v(ViewId::Calendar, "Calendar"),
            v(ViewId::Closed, "Closed"),
            v(ViewId::Landing, "Landing"),
            v(ViewId::Settings, "Settings"),
            v(ViewId::Plugin("p".into()), "Plugin"),
        ];
        assert_eq!(
            groups(&views),
            vec![
                (Group::Work, vec![0, 5, 2, 4]),
                (Group::Day, vec![3, 1]),
                (Group::System, vec![6, 7]),
            ]
        );
        assert_eq!(bar_title(&views[1]), "Map");
    }

    fn tab(n: u32, name: &str, path: &str) -> TabModel {
        let mut t = TabModel::new(TabId(n), n as usize, name);
        t.path = Some(PathBuf::from(path));
        t.claude = Some(SessionRef::default());
        t
    }

    #[test]
    fn git_s_worktrees_are_read_with_the_main_checkout_marked() {
        let out = "worktree /r\nHEAD abc\nbranch refs/heads/main\n\nworktree /r/.claude/worktrees/ab-1-fix\nHEAD def\nbranch refs/heads/AB-1-fix-it\n\nworktree /r/.claude/worktrees/x\nHEAD 123\ndetached\n";
        let w = parse_worktrees(Path::new("/r"), out);
        assert_eq!(w.len(), 3);
        assert!(w[0].main);
        assert_eq!(w[1].branch.as_deref(), Some("AB-1-fix-it"));
        assert_eq!(w[2].branch, None);
    }

    #[test]
    fn the_work_map_lists_each_worktree_with_its_tab_and_the_work_no_tab_has() {
        let trees = vec![
            Worktree {
                path: "/r".into(),
                branch: Some("main".into()),
                main: true,
            },
            Worktree {
                path: "/r/.claude/worktrees/ab-1".into(),
                branch: Some("AB-1-fix-the-login".into()),
                main: false,
            },
            Worktree {
                path: "/r/.claude/worktrees/ab-2".into(),
                branch: Some("ab-2-old-work".into()),
                main: false,
            },
        ];
        let mut waiting = tab(3, "AB-1: Fix the login", "/r/.claude/worktrees/ab-1/src");
        waiting.state = TabState::Waiting;
        let loose = tab(4, "Slack relay", "/tmp/elsewhere");
        let revived = vec![Revived {
            tree: "/other/wt".into(),
            branch: "try-it".into(),
            when: 5,
            age: "2d".into(),
            prompt: "try it out".into(),
            ..Revived::default()
        }];
        let rows = work_map(&[waiting, loose], &trees, &revived, &|_| None);
        let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "AB-1  Fix the login",
                "Slack relay",
                "try it",
                "AB-2  old work"
            ],
            "needing you first, then a tab, then work with no tab, the last used first"
        );
        assert_eq!(rows[0].target, Target::Tab(TabId(3)));
        assert_eq!(rows[0].state, State::Attention);
        assert_eq!(
            rows[3].target,
            Target::Dir("/r/.claude/worktrees/ab-2".into())
        );
        assert_eq!(rows[2].cells[4], "used 2d");
        assert!(rows[2].more[0].contains("try it out"));
        let picker = View {
            id: ViewId::WorkMap,
            title: "Work map".into(),
            data: ViewData {
                items: rows,
                ..ViewData::default()
            },
            loaded: true,
        };
        let open = shown(&picker, "", Some(Scope::Tabs), false);
        assert_eq!(open.title, "Work map · open tabs");
        assert_eq!(open.data.items.len(), 2);
        let untabbed = shown(&picker, "old", Some(Scope::Untabbed), false);
        assert_eq!(untabbed.data.items.len(), 1);
    }

    #[test]
    fn a_worktree_no_tab_is_in_whose_branch_merged_waits_to_be_removed() {
        // lib/sidebar's "the Work map shows merged work with no tab under
        // Merged, can be removed", its PR asked for by its branch.
        let trees = vec![
            Worktree {
                path: "/r".into(),
                branch: Some("main".into()),
                main: true,
            },
            Worktree {
                path: "/r/.claude/worktrees/ab-5".into(),
                branch: Some("AB-5-done".into()),
                main: false,
            },
            Worktree {
                path: "/r/.claude/worktrees/ab-6".into(),
                branch: Some("AB-6-going".into()),
                main: false,
            },
        ];
        let pr = |number, state| PrFacts {
            number,
            repo: "acme/r".into(),
            url: String::new(),
            state,
            merge: None,
            checks: None,
            review_decision: None,
            additions: 0,
            deletions: 0,
        };
        let pr_of = |p: &Path| match p.to_str() {
            Some("/r/.claude/worktrees/ab-5") => Some(pr(5, PrState::Merged)),
            Some("/r/.claude/worktrees/ab-6") => Some(pr(6, PrState::Open)),
            _ => None,
        };
        let rows = work_map(&[], &trees, &[], &pr_of);
        let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(
            titles,
            ["AB-6  going", "Merged, can be removed", "AB-5  done"]
        );
        assert_eq!(rows[2].data["merged"], true);
        let picker = View {
            id: ViewId::WorkMap,
            title: "Work map".into(),
            data: ViewData {
                items: rows,
                ..ViewData::default()
            },
            loaded: true,
        };
        // M-o's scope, work with no tab, keeps the header, which shows the
        // merged work once Enter opens it.
        let untabbed = shown(&picker, "", Some(Scope::Untabbed), false);
        let titles: Vec<&str> = untabbed
            .data
            .items
            .iter()
            .map(|r| r.title.as_str())
            .collect();
        assert_eq!(titles, ["AB-6  going", "▸ Merged, can be removed"]);
        let opened = shown(&picker, "", Some(Scope::Untabbed), true);
        assert_eq!(opened.data.items.len(), 3);
        let open = shown(&picker, "", Some(Scope::Tabs), true);
        assert!(open.data.items.is_empty(), "no header among the open tabs");
    }

    #[test]
    fn settings_are_listed_by_category_and_a_filter_keeps_only_categories_with_a_match() {
        let items = settings_items(&[]);
        assert!(is_header(&items[0]));
        assert_eq!(items[0].title, "Integrations");
        assert_eq!(items[1].title, "GitHub");
        assert!(["on", "off"].contains(&items[1].data["value"].as_str().unwrap()));
        let view = View {
            id: ViewId::Settings,
            title: "Settings".into(),
            data: ViewData {
                items,
                ..ViewData::default()
            },
            loaded: true,
        };
        let narrowed = shown(&view, "push notif", None, false);
        let titles: Vec<&str> = narrowed
            .data
            .items
            .iter()
            .map(|i| i.title.as_str())
            .collect();
        assert_eq!(titles, ["Notifications", "Push notifications"]);
    }

    #[test]
    fn a_filtered_autopilot_view_keeps_its_summary_over_a_heading_with_no_match() {
        let summary = crate::autopilot::Summary {
            note: "Work".into(),
            busy: 1,
            open: 1,
            slots: 2,
            cap: 5,
            next: "Later".into(),
            paused: false,
            away: false,
            ready_only: false,
            skipped: 0,
            spend_limit: 0.0,
            spent: 0.0,
            lands_alone: false,
            target: String::new(),
            drained: false,
            failing: String::new(),
            hands_free: false,
        };
        let row = crate::autopilot::view::Row {
            tab: crate::mux::TabId(4),
            index: 3,
            item: "Port the board".into(),
            state: crate::sidebar::model::TabState::Working,
            was_in: true,
            loop_skill: None,
            phase: None,
            landing: None,
            left: false,
            autoland: false,
            closed: None,
            in_note: true,
        };
        let view = View {
            id: ViewId::Autopilot,
            title: "Autopilot".into(),
            data: ViewData {
                items: crate::autopilot::view::items(
                    &summary,
                    "",
                    &[row],
                    &["Later".into()],
                    &[],
                    &std::collections::HashMap::new(),
                ),
                ..ViewData::default()
            },
            loaded: true,
        };
        let narrowed = shown(&view, "board", None, false);
        let titles: Vec<&str> = narrowed
            .data
            .items
            .iter()
            .map(|i| i.title.as_str())
            .collect();
        assert_eq!(
            titles,
            [
                "Work",
                "Its tabs, the least far along first",
                "Port the board"
            ]
        );
    }
}
