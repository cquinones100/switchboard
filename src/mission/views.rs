//! Mission control's views after its tabs (lib/sidebar/views.js's
//! `builtInViews`, and the daemon's `viewsFor`): each a list of items in
//! the one shape every source gives (`crate::items`), from what the server
//! already keeps, so building them reads no network and starts no process.
//! In the daemon's order, each only while its setting is on: the
//! Notifications (the sidebar's, whole), the Work map, the Calendar,
//! Reminders (while `reminders` is on), Slack,
//! Closed, the PR board, CI, Landing (while something lands), Autopilot
//! (with UX 2.0, while it is on, src/autopilot/view.rs), the ticket
//! board, the Inbox, Sessions, Resources, Usage, Keys (`keymap`), Settings,
//! then each plugin's. With UX 2.0 the bar shows them by feature in groups
//! (`groups`): Work, Code, Day and System.
//!
//! Two are put together here. The Work map is every piece of work in the
//! repositories the tabs are in, one row each under its repository's
//! heading: each worktree, and a main checkout a tab is in, with its ticket
//! (from the branch), the tab it is open in or none, its PR and where that
//! stands (the board's words, `standing` and `checks`), and its directory. The
//! worktrees are read with git in the background (`worktrees`), and the
//! worktrees no tab is in anywhere else come from a source of their own
//! (`Revived`, what `wt-revive --json` prints), so work in flight isn't
//! forgotten. Settings is every option of `assets/settings.json` under its
//! category, then your own files' settings (`usage::own`).
//!
//! A view as a client sees it (`shown`) is narrowed by its `/` filter, which
//! matches an item's title, detail and `search`, the boards excepted, and the
//! Work map is scoped to open tabs or to work with no tab (M-j's and M-o's
//! pickers, `o` and `n`), each repository folded by Enter on its heading
//! or `z`, and its merged work hidden under a line of its own until Enter
//! there shows it (`Folds`).

use std::collections::{HashMap, HashSet};
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
    /// Every tab's reminder on a month (src/mission/reminders.rs).
    Reminders,
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

    /// What it is called whatever its title says (a scope or a day added to
    /// it), for keeping a choice of views: "work-map", "slack", a plugin's
    /// "plugin:<name>".
    pub fn key(&self) -> String {
        let name = match &self.id {
            ViewId::Plugin(name) => return format!("plugin:{name}"),
            ViewId::Notifications => "notifications",
            ViewId::WorkMap => "work-map",
            ViewId::Calendar => "calendar",
            ViewId::Reminders => "reminders",
            ViewId::Slack => "slack",
            ViewId::Closed => "closed",
            ViewId::Prs => "prs",
            ViewId::Ci => "ci",
            ViewId::Landing => "landing",
            ViewId::Autopilot => "autopilot",
            ViewId::Tickets => "tickets",
            ViewId::Inbox => "inbox",
            ViewId::Sessions => "sessions",
            ViewId::Resources => "resources",
            ViewId::Usage => "usage",
            ViewId::Keys => "keys",
            ViewId::Settings => "settings",
        };
        name.to_string()
    }

    /// Its name as M-p's "Mission control: <view>" names it: lowercased,
    /// dashes for spaces.
    pub fn slug(&self) -> String {
        self.title.to_lowercase().replace(' ', "-")
    }
}

/// What M-p says of each view: its "Mission control: <view>" command
/// (`extras::Command::Mission`) lists every view here while it shows, so a
/// view added to `ViewId` is one pick away once it is in `BUILT_IN`.
impl ViewId {
    /// Every view but the plugins', in mission control's order.
    pub const BUILT_IN: [ViewId; 17] = [
        ViewId::Notifications,
        ViewId::WorkMap,
        ViewId::Calendar,
        ViewId::Reminders,
        ViewId::Slack,
        ViewId::Closed,
        ViewId::Prs,
        ViewId::Ci,
        ViewId::Landing,
        ViewId::Autopilot,
        ViewId::Tickets,
        ViewId::Inbox,
        ViewId::Sessions,
        ViewId::Resources,
        ViewId::Usage,
        ViewId::Keys,
        ViewId::Settings,
    ];

    /// Its title in mission control; a plugin's view by the plugin's name,
    /// since the title it shows can come from what the plugin answers.
    pub fn title(&self) -> &str {
        match self {
            ViewId::Notifications => "Notifications",
            ViewId::WorkMap => "Work map",
            ViewId::Calendar => "Calendar",
            ViewId::Reminders => "Reminders",
            ViewId::Slack => "Slack",
            ViewId::Closed => "Closed",
            ViewId::Prs => "PRs",
            ViewId::Ci => "CI",
            ViewId::Landing => "Landing",
            ViewId::Autopilot => "Autopilot",
            ViewId::Tickets => "Tickets",
            ViewId::Inbox => "Inbox",
            ViewId::Sessions => "Sessions",
            ViewId::Resources => "Resources",
            ViewId::Usage => "Usage",
            ViewId::Keys => "Keys",
            ViewId::Settings => "Settings",
            ViewId::Plugin(name) => name,
        }
    }

    /// Its title lowercased with dashes for spaces, as `View::slug` and
    /// sidebar.sh's `view:<name>` name it.
    pub fn slug(&self) -> String {
        self.title().to_lowercase().replace(' ', "-")
    }

    /// Whether mission control shows it with the settings as they are: its
    /// own setting on (`usage::habits::VIEWS`), and the service it needs.
    /// Landing shows only while something lands, but opening it keeps it.
    pub fn shows(&self) -> bool {
        self.shows_with(settings::is_on)
    }

    /// `shows`, with `on` saying whether a setting is on.
    pub fn shows_with(&self, on: impl Fn(&str) -> bool) -> bool {
        match self {
            ViewId::Notifications => on("sidebar_notifications"),
            ViewId::Reminders => on("reminders"),
            ViewId::Autopilot => on("ux2") && on("view_autopilot") && on("autopilot"),
            ViewId::Settings | ViewId::Plugin(_) => true,
            built_in => crate::usage::habits::VIEWS
                .iter()
                .find(|v| v.title == built_in.title())
                .is_some_and(|v| on(v.key) && v.needs.is_none_or(&on)),
        }
    }

    /// M-p's words for it.
    pub fn about(&self) -> String {
        let what = match self {
            ViewId::Notifications => "the sidebar's notifications, each one whole",
            ViewId::WorkMap => "every piece of work in your repositories",
            ViewId::Calendar => "today's meetings",
            ViewId::Reminders => "every tab's reminder, by when it comes due",
            ViewId::Slack => "what is new for you on Slack",
            ViewId::Closed => "the tabs closed last, to reopen",
            ViewId::Prs => "your pull requests by where they stand",
            ViewId::Ci => "your pull requests failing, running and passing",
            ViewId::Landing => {
                "what is landing in each repository, its landing queue or merge queue, and which tabs have them"
            }
            ViewId::Autopilot => "the note autopilot works, a card for each item",
            ViewId::Tickets => "your Linear issues by state",
            ViewId::Inbox => "your unread Linear mentions, comments and assignments",
            ViewId::Sessions => "each Claude session's context and age",
            ViewId::Resources => "the CPU and memory each tab uses",
            ViewId::Usage => "today's Claude spend and the rate limits",
            ViewId::Keys => "every key, by what it is about",
            ViewId::Settings => "turn the workspace's options on and off",
            ViewId::Plugin(_) => "a sidebar plugin's view",
        };
        format!("Mission control: {}, {what}", self.title())
    }

    /// More on it than `about`, for the palette's preview.
    pub fn detail(&self) -> &'static str {
        match self {
            ViewId::Notifications => {
                "Newest first, Enter opening one's actions; with Settings' \"Notifications' history\" (notification_history) on, every one of the last two weeks by day, with what it led to. Settings' \"Notifications\" (sidebar_notifications)."
            }
            ViewId::WorkMap => {
                "A row for each worktree and tab in your tabs' repositories, with its ticket, its tab and where its PR stands, what needs you first. Enter goes to it or reopens it. Settings' \"Work map\" (view_work_map)."
            }
            ViewId::Calendar => {
                "Today laid out as Google Calendar's Day view, read from its iCal feed every five minutes, else from what /today last saved. Enter joins a call or answers an invitation. Settings' \"Calendar\" (view_calendar)."
            }
            ViewId::Reminders => {
                "Each tab M-p's \"Remind me about this tab later\" set, with the time left, on a month with UX 2.0. Enter goes to the tab. Settings' \"Reminders\" (reminders)."
            }
            ViewId::Slack => {
                "From the last day: direct messages and mentions first, then replies in your threads and the channels in slack-updates.json. Enter opens one in Slack or a tab for its thread. Settings' \"Slack\" (view_slack)."
            }
            ViewId::Closed => {
                "Newest first, one per Claude conversation. Enter reopens it where it was, with its name and its Claude resumed, or a shell when it had none. The newest 200 outlive a restart. Settings' \"Closed tabs\" (view_closed)."
            }
            ViewId::Prs => {
                "Columns Draft, In review, Approved, In merge queue and Merged in the last 7 days, from the repositories your tabs are in, with checks and reviews. Needs GitHub; Settings' \"PR board\" (pr_board)."
            }
            ViewId::Ci => {
                "Failing, running and passing, each failed check with the log line that says why; with UX 2.0 the chosen check's failing step beside, l its whole log, o its tab, M-r the browser. Enter starts a CI loop on it. Needs GitHub; Settings' \"CI\" (view_ci)."
            }
            ViewId::Landing => {
                "There only while something lands. A landing queue's row moves up, down or out from its menu; merge queues are asked of GitHub and hold everyone's PRs. Settings' \"Landing view\" (view_landing)."
            }
            ViewId::Autopilot => {
                "With UX 2.0, the note's tabs working and open against its slots and cap, today's spend, and a card for each item in the column where it stands. Settings' \"Autopilot\" (autopilot) and \"Autopilot view\" (view_autopilot)."
            }
            ViewId::Tickets => {
                "Columns Todo, In progress, In review and Done in the last 7 days, an urgent one marked as needing you. Enter starts work or shapes it in a new tab. Needs Linear; Settings' \"Ticket board\" (ticket_board)."
            }
            ViewId::Inbox => {
                "Newest first, each in the sidebar's notifications too; status changes are left out. Enter offers what the Tickets board does, and marks it read in Linear. Needs Linear; Settings' \"Inbox\" (view_inbox)."
            }
            ViewId::Sessions => {
                "The fullest context first, with its model, how long it has run and since it last did anything; one at 80% or more is marked as needing you. Settings' \"Sessions\" (view_sessions)."
            }
            ViewId::Resources => {
                "Every process under a tab's panes counted, the heaviest tab first, read every three seconds only while the view is in front. Enter goes to the tab. Settings' \"Resources\" (view_resources)."
            }
            ViewId::Usage => {
                "Spend at API prices as Claude Code estimates it, interactive sessions only, and when each limit resets; below, today's requests to services outside the computer. Settings' \"Usage\" (usage)."
            }
            ViewId::Keys => {
                "The same as M-/: the workspace's keys by their first tag, then the sidebar's, mission control's, the diff's and Notes', then those only through M-p, with / to filter. Settings' \"Keys\" (view_keys)."
            }
            ViewId::Settings => {
                "The same as M-p's \"Settings\": every option under its category, then your own files' settings."
            }
            ViewId::Plugin(_) => {
                "The items its source last answered, as sidebar.json and sidebar-plugins/ set it up; r looks again."
            }
        }
    }

    /// Its tags in M-p: `#github` and `#linear` take it out while that
    /// service is off.
    pub fn tags(&self) -> &'static [&'static str] {
        match self {
            ViewId::Notifications => &["mission", "notifications"],
            ViewId::WorkMap => &["mission", "worktree"],
            ViewId::Calendar => &["mission", "calendar"],
            ViewId::Reminders => &["mission", "remind", "tabs"],
            ViewId::Slack => &["mission", "slack"],
            ViewId::Closed => &["mission", "tabs"],
            ViewId::Prs => &["mission", "github", "pr"],
            ViewId::Ci => &["mission", "github", "ci"],
            ViewId::Landing => &["mission", "queue", "landing"],
            ViewId::Autopilot => &["mission", "autopilot"],
            ViewId::Tickets => &["mission", "linear", "ticket"],
            ViewId::Inbox => &["mission", "linear", "inbox"],
            ViewId::Sessions => &["mission", "claude", "context"],
            ViewId::Resources => &["mission", "cpu", "memory", "processes"],
            ViewId::Usage => &["mission", "claude", "usage", "cost"],
            ViewId::Keys => &["mission", "help", "keys"],
            ViewId::Settings => &["mission", "settings"],
            ViewId::Plugin(_) => &["mission", "plugin"],
        }
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
        ViewId::Reminders => (Group::Day, 1),
        ViewId::Slack => (Group::Day, 2),
        ViewId::Inbox => (Group::Day, 3),
        ViewId::Notifications => (Group::Day, 4),
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

/// Whether the cursor passes over an item: a heading, a row of the Usage
/// view that is neither a tab nor a service, or, while today's board is
/// drawn (`hide_week_only`, `Mission::week_only_hidden`), a service with no
/// requests today, which it leaves out.
pub fn passed_over(item: &Item, hide_week_only: bool) -> bool {
    use crate::usage::charts::{is_still, is_week_only};
    is_header(item) || is_still(item) || (hide_week_only && is_week_only(item))
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
        ViewId::Settings
            | ViewId::Keys
            | ViewId::Landing
            | ViewId::Autopilot
            | ViewId::Ci
            | ViewId::Notifications
            | ViewId::Usage
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

/// A view as one client sees it: its `/` filter's text, and for the Work
/// map its scope and which repositories are folded or show their merged
/// work.
pub fn shown(view: &View, filter: &str, scope: Option<Scope>, folds: &Folds) -> View {
    let text = filter.trim().to_lowercase();
    let work_map = view.id == ViewId::WorkMap;
    let scope = scope.filter(|_| work_map);
    if !work_map && (text.is_empty() || view.columns().is_some()) {
        return view.clone();
    }
    let items = if work_map {
        work_map_shown(&view.data.items, &text, scope, folds)
    } else {
        let kept: Vec<Item> = view
            .data
            .items
            .iter()
            .filter(|i| is_header(i) || is_summary(i) || search_text(i).contains(&text))
            .cloned()
            .collect();
        // A filtered Settings or Keys view leaves out the headings with no
        // match, but never the Autopilot view's summary, which heads it
        // whatever matches.
        if grouped(&view.id) {
            kept.iter()
                .enumerate()
                .filter(|(at, i)| {
                    !is_header(i)
                        || is_summary(i)
                        || kept.get(at + 1).is_some_and(|n| !is_header(n))
                })
                .map(|(_, i)| i.clone())
                .collect()
        } else {
            kept
        }
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

/// The Work map's items as shown: each repository's rows that its scope
/// and filter keep under its heading (marked `▾`, or `▸` folded, its rows
/// left out), then its fold line saying how many merged worktrees it keeps
/// and those, once Enter there opens it; a typed filter opens every fold. A
/// heading or fold line with nothing kept under it is left out.
fn work_map_shown(items: &[Item], text: &str, scope: Option<Scope>, folds: &Folds) -> Vec<Item> {
    let keeps = |i: &Item| {
        let scoped = match scope {
            None => true,
            Some(Scope::Tabs) => matches!(i.target, Target::Tab(_)),
            Some(Scope::Untabbed) => !matches!(i.target, Target::Tab(_)),
        };
        scoped && (text.is_empty() || search_text(i).contains(text))
    };
    let filtering = !text.is_empty();
    let mut out = Vec::new();
    let mut at = 0;
    while at < items.len() {
        let heading = &items[at];
        let end = items[at + 1..]
            .iter()
            .position(is_repo_heading)
            .map_or(items.len(), |n| at + 1 + n);
        let members = &items[at + 1..end];
        at = end;
        let key = repo_key(heading).unwrap_or("").to_string();
        let rows: Vec<&Item> = members
            .iter()
            .filter(|i| !is_header(i) && !merged(i) && keeps(i))
            .collect();
        let merged_rows: Vec<&Item> = members
            .iter()
            .filter(|i| !is_header(i) && merged(i) && keeps(i))
            .collect();
        if rows.is_empty() && merged_rows.is_empty() {
            continue;
        }
        let folded = !filtering && folds.folded.contains(&key);
        let mut h = heading.clone();
        h.title = format!("{} {}", if folded { "▸" } else { "▾" }, h.title);
        out.push(h);
        if folded {
            continue;
        }
        out.extend(rows.into_iter().cloned());
        if let Some(line) = members.iter().find(|i| is_fold_line(i))
            && !merged_rows.is_empty()
        {
            let open = filtering || folds.merged_open.contains(&key);
            let n = merged_rows.len();
            let mut line = line.clone();
            line.title = format!(
                "{} {n} merged worktree{}{}",
                if open { "▾" } else { "▸" },
                if n == 1 { "" } else { "s" },
                if open { "" } else { ", folded" },
            );
            line.data["count"] = json!(n);
            line.data["open"] = json!(open);
            out.push(line);
            if open {
                out.extend(merged_rows.into_iter().cloned());
            }
        }
    }
    out
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
/// Mission control's Settings view: every option under a heading for its
/// category (named as `settings::category_name` gives it), each heading
/// counting its options and how many of them differ from their defaults,
/// each option with its value and default in words and whether it
/// changed; then your own files' rows under a heading for each.
pub fn settings_items(own: &[Item], ux2: bool) -> Vec<Item> {
    settings_items_of(&settings::current_settings(), own, ux2)
}

pub(crate) fn settings_items_of(all: &[settings::Setting], own: &[Item], ux2: bool) -> Vec<Item> {
    let mut categories: Vec<&str> = Vec::new();
    for o in all {
        if !categories.contains(&o.category.as_str()) {
            categories.push(&o.category);
        }
    }
    let mut items = Vec::new();
    for category in categories {
        let members: Vec<&settings::Setting> =
            all.iter().filter(|o| o.category == category).collect();
        let name = settings::category_name(category, ux2);
        let mut heading = header(category, name, members.len());
        heading.data["changed"] = json!(members.iter().filter(|o| o.changed()).count());
        items.push(heading);
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
                        "default": o.default_words(),
                        "changed": o.changed(),
                        "choice": o.choices.is_some(),
                        "next": settings::Setting { value: o.next_value(), ..(*o).clone() }.value_words(),
                        "search": format!("{category} {name} {} {words}", o.key),
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
    /// Its repository, by the main checkout's path.
    pub repo: PathBuf,
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
                repo: repo.to_path_buf(),
            })
        })
        .collect()
}

/// What the Work map knows besides the tabs and the worktrees: how each
/// repository lands, in words, by its main checkout (read in the
/// background, `MissionEvent::Worktrees`), the branches counted for the
/// rows chosen (`Counts`, by worktree), and the time, for "last used".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MapFacts {
    pub landings: HashMap<PathBuf, String>,
    pub counts: HashMap<PathBuf, Counts>,
    pub now_secs: i64,
}

/// A worktree's branch counted, for the pane under the map: at which
/// commits (`tabs::git::head_commit`), against which default branch, how
/// many commits ahead of it (`None` when git couldn't count), how many its
/// upstream doesn't have (`None` with no upstream), and what is watched for
/// a push (the upstream's ref as git named it, else the repository's
/// config, which a push setting one writes) and where it was.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub head: String,
    pub default: String,
    pub ahead: Option<u32>,
    pub to_push: Option<u32>,
    pub upstream: String,
    pub upstream_at: String,
}

/// Whether the chosen row's branch needs counting: not yet, not at the
/// commits it is at now (`tabs::git::head_commit`), or its upstream moved
/// (`upstream_at` reads the commit a ref is at now).
pub fn needs_count(
    counts: &HashMap<PathBuf, Counts>,
    tree: &Path,
    head: &str,
    upstream_at: &dyn Fn(&str) -> String,
) -> bool {
    counts
        .get(tree)
        .is_none_or(|c| c.head != head || upstream_at(&c.upstream) != c.upstream_at)
}

/// Which repositories one client's Work map folds, and which show their
/// merged work, by the repository's key (an item's `data.repo`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Folds {
    pub folded: HashSet<String>,
    pub merged_open: HashSet<String>,
}

impl Folds {
    /// Folds a repository, or opens it again.
    pub fn fold(&mut self, repo: &str) {
        if !self.folded.remove(repo) {
            self.folded.insert(repo.to_string());
        }
    }

    /// Shows a repository's merged work, or folds it again.
    pub fn merged(&mut self, repo: &str) {
        if !self.merged_open.remove(repo) {
            self.merged_open.insert(repo.to_string());
        }
    }
}

/// The key of the open tabs in no repository, whose heading comes last.
const OTHER: &str = "other";

/// Whether an item is a repository's heading in the Work map.
pub fn is_repo_heading(item: &Item) -> bool {
    item.id.starts_with("repo:") && is_header(item)
}

/// Whether an item is the line a repository's merged work is folded under.
pub fn is_fold_line(item: &Item) -> bool {
    item.id.starts_with("merged:") && is_header(item)
}

/// The repository a Work map item is under, by its key.
pub fn repo_key(item: &Item) -> Option<&str> {
    item.data.get("repo").and_then(Value::as_str)
}

static TICKET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^([a-z][a-z0-9]*-\d+)").expect("a regex"));

/// Where a PR stands, in a few words: `#12 merged`, `#12 ✗2 failing`; none
/// with the github setting off. Without UX 2.0, the row's PR cell.
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

/// Where a row's work stands, in the board's words, with the name of its
/// tone (`kit::Tone`, as render's `tone_named` reads it): its place in the
/// landing queue, failed tests, the PR's review or place in the merge
/// queue, commits to push, else "no PR". The PR's words only with the
/// github setting on.
pub fn standing(facts: &TabModel, counts: Option<&Counts>) -> (String, &'static str) {
    if facts
        .phase
        .as_deref()
        .is_some_and(|p| p.starts_with("tests failed"))
    {
        return ("✕ tests failed".into(), "failed");
    }
    if let Some(l) = facts.landing {
        return if l.holding {
            ("Landing".into(), "landing")
        } else {
            (
                format!(
                    "{} in the landing queue",
                    crate::sidebar::text::ordinal(l.line)
                ),
                "landing",
            )
        };
    }
    // Landed straight on a branch, with no PR (`@tab_landed`).
    if let Some((hash, _)) = &facts.landed_on {
        return (format!("Landed {hash}"), "done");
    }
    let github = settings::is_on("github");
    if github && facts.pr.is_some() {
        match facts.merge {
            Some(MergeState::Merged) => return ("Merged".into(), "done"),
            Some(MergeState::Queued) => {
                return match facts.position {
                    Some(p) if p > 0 => (
                        format!("{} in the merge queue", crate::sidebar::text::ordinal(p)),
                        "landing",
                    ),
                    _ => ("In the merge queue".into(), "landing"),
                };
            }
            None => {}
        }
        if facts.pr_state == Some(PrState::Closed) {
            return ("Closed".into(), "dim");
        }
        return match facts.review.as_deref() {
            Some("APPROVED") => ("Approved".into(), "done"),
            Some("CHANGES_REQUESTED") => ("Changes asked".into(), "needs"),
            _ => ("In review".into(), "text"),
        };
    }
    if let Some(n) = counts.and_then(|c| c.to_push).filter(|n| *n > 0) {
        let s = if n == 1 { "" } else { "s" };
        return (format!("{n} commit{s} to push"), "text");
    }
    if github {
        ("no PR".into(), "faint")
    } else {
        (String::new(), "faint")
    }
}

/// The PR's checks, in the board's words, with its tone's name: failing,
/// how many of them have finished while some run, or passed; nothing for a
/// merged PR, one with no checks, or with the github setting off.
pub fn checks(facts: &TabModel) -> (String, &'static str) {
    let merged = facts.merge == Some(MergeState::Merged);
    let Some(c) = facts
        .checks
        .filter(|_| facts.pr.is_some() && !merged && settings::is_on("github"))
    else {
        return (String::new(), "faint");
    };
    let total = c.pass + c.fail + c.pending;
    if c.fail > 0 {
        (format!("✕ {} failing", c.fail), "failed")
    } else if c.pending > 0 {
        (format!("◌ {} of {total}", c.pass), "needs")
    } else if c.pass > 0 {
        ("✓ passed".into(), "done")
    } else {
        (String::new(), "faint")
    }
}

/// A worktree no tab is in, as far as its PR goes: a tab of no number
/// standing in for it, with what GitHub said of its branch's PR (views.js's
/// `mergeOf`), for `pr_words` and `state_of`.
fn stand_in(pr: Option<&PrFacts>) -> TabModel {
    let mut t = TabModel::new(crate::mux::TabId(0), 0, "");
    if let Some(p) = pr {
        t.pr = Some(p.number);
        t.pr_state = Some(p.state);
        t.review = p.review_decision.clone();
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
            let base = w.branch.clone().unwrap_or_else(|| dir_name(&w.path));
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

fn dir_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
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

/// "Last used": now for a tab at work, else how long since its last turn
/// (ms) or the worktree's last use (seconds), shortly.
fn last_used(tab: Option<&TabModel>, last: Option<&Revived>, now_secs: i64) -> String {
    if let Some(t) = tab {
        if t.state.busy() {
            return "now".into();
        }
        let at = t.turn.as_ref().and_then(|turn| turn.at).map(|ms| ms / 1000);
        return crate::sidebar::text::ago(at, now_secs);
    }
    last.map(|l| crate::sidebar::text::ago(Some(l.when), now_secs))
        .unwrap_or_default()
}

/// One row of the Work map before it is grouped: the item, when it was
/// last used (for the work no tab is in), its repository and whether it is
/// the main checkout.
struct Placed {
    item: Item,
    when: i64,
    repo: Option<PathBuf>,
    main: bool,
}

/// The Work map's rows, from the open tabs, the worktrees of their
/// repositories, and the worktrees no tab is in that are known elsewhere,
/// whose PRs `pr_of` gives by their branch, grouped by repository: each
/// repository's heading (where it is, how it lands, its worktrees and how
/// many are open), its main checkout, its other rows needing you first, and
/// its merged work with no tab behind a fold line of its own. Every open tab
/// is a row too, one in no repository under "Other tabs" last, so M-j,
/// which lists the open tabs' rows, reaches them all.
pub fn work_map(
    tabs: &[TabModel],
    worktrees: &[Worktree],
    revived: &[Revived],
    pr_of: &dyn Fn(&Path) -> Option<PrFacts>,
    facts: &MapFacts,
) -> Vec<Item> {
    let mut all: Vec<Worktree> = worktrees.to_vec();
    for r in revived {
        if !all.iter().any(|w| w.path == r.tree) {
            all.push(Worktree {
                path: r.tree.clone(),
                branch: Some(r.branch.clone()).filter(|b| !b.is_empty()),
                main: false,
                repo: r.repo.clone(),
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
    let mut rows: Vec<Placed> = Vec::new();
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
        let facts_of = tab.unwrap_or(&untabbed);
        let pr = pr_words(facts_of);
        let used = last.map(|l| format!("used {}", l.age)).unwrap_or_default();
        let place = paths::tilde(&w.path);
        let prompt = match tab {
            Some(t) => t
                .turn
                .as_ref()
                .map(|t| t.prompt.clone())
                .unwrap_or_default(),
            None => last.map(|l| l.prompt.clone()).unwrap_or_default(),
        };
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
        let more: Vec<String> = [place.clone(), prompt.clone()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        let merge = facts_of.merge.map(|m| match m {
            MergeState::Merged => "merged",
            MergeState::Queued => "queued",
        });
        let lead = ticket
            .as_ref()
            .map(|t| format!("{t}  "))
            .unwrap_or_default();
        let counts = facts.counts.get(&w.path);
        let (standing, standing_tone) = standing(facts_of, counts);
        let (checks, checks_tone) = checks(facts_of);
        let repo = (!w.repo.as_os_str().is_empty()).then(|| w.repo.clone());
        let mut item = Item::new(w.path.to_string_lossy(), format!("{lead}{title}"))
            .detail(detail.join(" · "))
            .state(state_of(tab, facts_of))
            .data(json!({
                "merge": merge,
                "merged": tab.is_none() && merge == Some("merged"),
                "branch": w.branch,
                "tree": w.path,
                "search": w.branch.clone().unwrap_or_default(),
                "repo": repo.as_ref().map_or(OTHER.into(), |r| r.to_string_lossy().into_owned()),
                "worktree": w.branch.clone().unwrap_or_else(|| dir_name(&w.path)),
                "ticket": ticket.clone().unwrap_or_default(),
                "pr": facts_of.pr.filter(|_| settings::is_on("github")).map(|n| format!("#{n}")),
                "standing": standing,
                "standingTone": standing_tone,
                "checks": checks,
                "checksTone": checks_tone,
                "used": last_used(tab, last, facts.now_secs),
                "tab": tab.map(|t| json!({ "index": t.index, "name": t.name })),
                "place": place,
                "prompt": prompt,
                "counts": counts.map(|c| json!({
                    "default": c.default,
                    "ahead": c.ahead,
                    "toPush": c.to_push,
                })),
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
        rows.push(Placed {
            item,
            when: last.map_or(0, |l| l.when),
            repo,
            main: w.main,
        });
    }
    // The open tabs no worktree row took: in no repository, or a second
    // tab in the same worktree.
    for tab in tabs.iter().filter(|t| !taken.contains(&t.id)) {
        // A second tab in a worktree takes the worktree's facts.
        let owner = owner_of(tab).map(|i| &all[i]);
        let repo = owner
            .map(|w| w.repo.clone())
            .filter(|r| !r.as_os_str().is_empty());
        let counts = owner.and_then(|w| facts.counts.get(&w.path));
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
        let (standing, standing_tone) =
            if owner.is_some() || tab.pr.is_some() || tab.landing.is_some() {
                standing(tab, counts)
            } else {
                (String::new(), "faint")
            };
        let (checks, checks_tone) = checks(tab);
        let mut item = Item::new(format!("tab:{}", tab.id), &tab.name)
            .detail(detail.join(" · "))
            .state(state_of(Some(tab), tab))
            .target(Target::Tab(tab.id))
            .data(json!({
                "merge": tab.merge.map(|m| match m {
                    MergeState::Merged => "merged",
                    MergeState::Queued => "queued",
                }),
                "repo": repo.as_ref().map_or(OTHER.into(), |r| r.to_string_lossy().into_owned()),
                "worktree": owner
                    .map(|w| w.branch.clone().unwrap_or_else(|| dir_name(&w.path)))
                    .or_else(|| tab.path.as_deref().map(dir_name))
                    .unwrap_or_default(),
                "branch": owner.and_then(|w| w.branch.clone()),
                "tree": owner.map(|w| w.path.clone()),
                "search": owner.and_then(|w| w.branch.clone()).unwrap_or_default(),
                "counts": counts.map(|c| json!({
                    "default": c.default,
                    "ahead": c.ahead,
                    "toPush": c.to_push,
                })),
                "ticket": "",
                "pr": tab.pr.filter(|_| settings::is_on("github")).map(|n| format!("#{n}")),
                "standing": standing,
                "standingTone": standing_tone,
                "checks": checks,
                "checksTone": checks_tone,
                "used": last_used(Some(tab), None, facts.now_secs),
                "tab": { "index": tab.index, "name": tab.name },
                "place": place,
                "prompt": tab.turn.as_ref().map(|t| t.prompt.clone()).unwrap_or_default(),
            }));
        item.cells = vec![String::new(), tab.name.clone(), where_, pr, String::new()];
        item.more = vec![place].into_iter().filter(|m| !m.is_empty()).collect();
        rows.push(Placed {
            item,
            when: 0,
            repo,
            main: false,
        });
    }
    rows.sort_by(|a, b| {
        let (ra, rb) = (rank(&a.item), rank(&b.item));
        b.main.cmp(&a.main).then(ra.cmp(&rb)).then_with(|| {
            if ra == 4 {
                b.when.cmp(&a.when)
            } else {
                std::cmp::Ordering::Equal
            }
        })
    });
    // The repositories, the one with the most pressing row first, then by
    // name; the tabs in none last.
    let mut repos: Vec<(u8, String, Option<PathBuf>)> = Vec::new();
    for r in &rows {
        let name = r.repo.as_deref().map(dir_name).unwrap_or_default();
        match repos.iter_mut().find(|(_, _, p)| *p == r.repo) {
            Some(found) => found.0 = found.0.min(rank(&r.item)),
            None => repos.push((rank(&r.item), name, r.repo.clone())),
        }
    }
    repos.sort_by(|a, b| {
        a.2.is_none()
            .cmp(&b.2.is_none())
            .then(a.0.cmp(&b.0))
            .then_with(|| a.1.cmp(&b.1))
    });
    let mut out: Vec<Item> = Vec::new();
    for (_, name, repo) in repos {
        let key = repo
            .as_ref()
            .map_or(OTHER.to_string(), |r| r.to_string_lossy().into_owned());
        let members: Vec<&Placed> = rows.iter().filter(|r| r.repo == repo).collect();
        let (merged_rows, rest): (Vec<&Placed>, Vec<&Placed>) =
            members.iter().partition(|r| merged(&r.item));
        // A repository's worktrees open in a tab, a second tab in one not
        // counted again; the tabs in none, each.
        let open = members
            .iter()
            .filter(|r| matches!(r.item.target, Target::Tab(_)))
            .filter(|r| repo.is_none() || !r.item.id.starts_with("tab:"))
            .count();
        let trees = members
            .iter()
            .filter(|r| !r.item.id.starts_with("tab:"))
            .count();
        let heading = match &repo {
            Some(r) => {
                let s = |n: usize| if n == 1 { "" } else { "s" };
                let place = paths::tilde(r);
                let lands = facts.landings.get(r).cloned().unwrap_or_default();
                let mut h = header(&key, &name, members.len()).data(json!({
                    "header": true,
                    "count": members.len(),
                    "repo": key,
                    "place": place,
                    "lands": lands,
                    "counts": format!("{trees} worktree{} · {open} open", s(trees)),
                    "search": format!("{name} {place}"),
                }));
                h.id = format!("repo:{key}");
                h
            }
            None => {
                let mut h = header(&key, "Other tabs", members.len()).data(json!({
                    "header": true,
                    "count": members.len(),
                    "repo": key,
                    "counts": format!("{open} open"),
                }));
                h.id = format!("repo:{key}");
                h
            }
        };
        out.push(heading);
        out.extend(rest.into_iter().map(|r| r.item.clone()));
        if !merged_rows.is_empty() {
            let mut fold = header(&key, "merged worktrees", merged_rows.len());
            fold.id = format!("merged:{key}");
            fold.data["repo"] = json!(key);
            out.push(fold);
            out.extend(merged_rows.into_iter().map(|r| r.item.clone()));
        }
    }
    out
}

/// The view's empty words when nothing at all is there yet.
pub fn work_map_empty() -> String {
    "No work found: open a tab in a git repository, and its worktrees show here.".into()
}

/// What a view's label says of it when several views show at once
/// (`grid`): Slack's messages not yet seen ("4 new"), the Inbox's
/// notifications, today's spend ("$8.41 today"), the Calendar's events not
/// yet over ("3 left"), the tabs' count, else its items'. `None` is the
/// tabs, `tabs` of them.
pub fn count_words(view: Option<&View>, tabs: usize) -> String {
    let Some(view) = view else {
        return format!("{tabs} tab{}", if tabs == 1 { "" } else { "s" });
    };
    let items = &view.data.items;
    match view.id {
        ViewId::Slack => {
            let new = items
                .iter()
                .filter(|i| i.data.get("old").and_then(Value::as_bool) == Some(false))
                .count();
            format!("{new} new")
        }
        ViewId::Inbox => format!("{} unread", items.len()),
        ViewId::Usage => {
            let spent = crate::usage::charts::model_of(items).map_or(0.0, |m| m.today.total);
            format!("${spent:.2} today")
        }
        ViewId::Calendar => {
            let left = items
                .iter()
                .filter(|i| {
                    i.state != crate::items::State::Idle && i.group.as_deref() != Some("All day")
                })
                .count();
            format!("{left} left")
        }
        _ => items.iter().filter(|i| !is_header(i)).count().to_string(),
    }
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
    fn a_view_s_label_counts_what_it_is_about() {
        let view = |id: ViewId, items: Vec<Item>| View {
            id,
            title: String::new(),
            data: ViewData {
                items,
                ..ViewData::default()
            },
            loaded: true,
        };
        let message = |n: u32, old: bool| Item::new(n.to_string(), "m").data(json!({ "old": old }));
        let slack = view(
            ViewId::Slack,
            vec![
                message(1, false),
                message(2, false),
                message(3, true),
                message(4, false),
                message(5, false),
            ],
        );
        assert_eq!(count_words(Some(&slack), 0), "4 new");
        let model = crate::usage::charts::Model {
            today: crate::usage::charts::SpanSpend {
                total: 8.414,
                ..Default::default()
            },
            ..Default::default()
        };
        let usage = view(
            ViewId::Usage,
            vec![Item::new("spent", "Spent today").data(json!({ "model": model }))],
        );
        assert_eq!(count_words(Some(&usage), 0), "$8.41 today");
        let event = |n: u32, state: crate::items::State| Item::new(n.to_string(), "e").state(state);
        let calendar = view(
            ViewId::Calendar,
            vec![
                event(1, crate::items::State::Idle),
                event(2, crate::items::State::Active),
                event(3, crate::items::State::Attention),
                event(4, crate::items::State::Active),
                event(5, crate::items::State::Active).group("All day"),
            ],
        );
        assert_eq!(count_words(Some(&calendar), 0), "3 left");
        assert_eq!(count_words(None, 5), "5 tabs");
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

    fn tree(path: &str, branch: &str, main: bool) -> Worktree {
        Worktree {
            path: path.into(),
            branch: Some(branch.into()),
            main,
            repo: "/r".into(),
        }
    }

    fn titles(items: &[Item]) -> Vec<&str> {
        items.iter().map(|r| r.title.as_str()).collect()
    }

    fn map_view(items: Vec<Item>) -> View {
        View {
            id: ViewId::WorkMap,
            title: "Work map".into(),
            data: ViewData {
                items,
                ..ViewData::default()
            },
            loaded: true,
        }
    }

    #[test]
    fn the_work_map_lists_each_repository_s_worktrees_under_its_heading() {
        let trees = vec![
            tree("/r", "main", true),
            tree("/r/.claude/worktrees/ab-1", "AB-1-fix-the-login", false),
            tree("/r/.claude/worktrees/ab-2", "ab-2-old-work", false),
        ];
        let mut waiting = tab(3, "AB-1: Fix the login", "/r/.claude/worktrees/ab-1/src");
        waiting.state = TabState::Waiting;
        let main = tab(5, "Main work", "/r");
        let loose = tab(4, "Slack relay", "/tmp/elsewhere");
        let revived = vec![Revived {
            tree: "/other/wt".into(),
            repo: "/other".into(),
            branch: "try-it".into(),
            when: 5,
            age: "2d".into(),
            prompt: "try it out".into(),
        }];
        let facts = MapFacts {
            landings: HashMap::from([("/r".into(), "lands straight on main".into())]),
            ..MapFacts::default()
        };
        let rows = work_map(&[waiting, loose, main], &trees, &revived, &|_| None, &facts);
        assert_eq!(
            titles(&rows),
            [
                "r",
                "Main work",
                "AB-1  Fix the login",
                "AB-2  old work",
                "other",
                "try it",
                "Other tabs",
                "Slack relay",
            ],
            "the main checkout first, then needing you; the tabs in no repository last"
        );
        assert!(is_repo_heading(&rows[0]));
        assert_eq!(rows[0].data["counts"], "3 worktrees · 2 open");
        assert_eq!(rows[0].data["lands"], "lands straight on main");
        assert_eq!(rows[0].data["place"], "/r");
        assert_eq!(rows[2].target, Target::Tab(TabId(3)));
        assert_eq!(rows[2].state, State::Attention);
        assert_eq!(rows[2].data["ticket"], "AB-1");
        assert_eq!(rows[2].data["worktree"], "AB-1-fix-the-login");
        assert_eq!(
            rows[3].target,
            Target::Dir("/r/.claude/worktrees/ab-2".into())
        );
        assert_eq!(rows[5].cells[4], "used 2d");
        assert_eq!(rows[5].data["prompt"], "try it out");
        assert_eq!(rows[5].data["repo"], "/other");
        let picker = map_view(rows);
        let open = shown(&picker, "", Some(Scope::Tabs), &Folds::default());
        assert_eq!(open.title, "Work map · open tabs");
        assert_eq!(
            titles(&open.data.items),
            [
                "▾ r",
                "Main work",
                "AB-1  Fix the login",
                "▾ Other tabs",
                "Slack relay"
            ],
            "a repository with no open tab is left out"
        );
        let untabbed = shown(&picker, "old", Some(Scope::Untabbed), &Folds::default());
        assert_eq!(titles(&untabbed.data.items), ["▾ r", "AB-2  old work"]);
        let mut folds = Folds::default();
        folds.fold("/r");
        let folded = shown(&picker, "", None, &folds);
        assert_eq!(&titles(&folded.data.items)[..2], ["▸ r", "▾ other"]);
    }

    #[test]
    fn a_repository_s_merged_work_with_no_tab_is_folded_under_its_own_line() {
        let trees = vec![
            tree("/r", "main", true),
            tree("/r/.claude/worktrees/ab-5", "AB-5-done", false),
            tree("/r/.claude/worktrees/ab-6", "AB-6-going", false),
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
        let rows = work_map(&[], &trees, &[], &pr_of, &MapFacts::default());
        assert_eq!(
            titles(&rows),
            ["r", "AB-6  going", "merged worktrees", "AB-5  done"]
        );
        assert!(is_fold_line(&rows[2]));
        assert_eq!(rows[3].data["merged"], true);
        let view = map_view(rows);
        let none = Folds::default();
        let shut = shown(&view, "", Some(Scope::Untabbed), &none);
        assert_eq!(
            titles(&shut.data.items),
            ["▾ r", "AB-6  going", "▸ 1 merged worktree, folded"]
        );
        let mut folds = Folds::default();
        folds.merged("/r");
        let opened = shown(&view, "", None, &folds);
        assert_eq!(
            titles(&opened.data.items),
            ["▾ r", "AB-6  going", "▾ 1 merged worktree", "AB-5  done"]
        );
        let found = shown(&view, "done", None, &none);
        assert_eq!(
            titles(&found.data.items),
            ["▾ r", "▾ 1 merged worktree", "AB-5  done"],
            "a filter opens the fold"
        );
        let open = shown(&view, "", Some(Scope::Tabs), &folds);
        assert!(open.data.items.is_empty(), "no heading among the open tabs");
    }

    #[test]
    fn where_a_row_stands_is_said_in_the_board_s_words() {
        let with_pr = |f: &dyn Fn(&mut TabModel)| {
            let mut t = TabModel::new(TabId(1), 1, "t");
            t.pr = Some(12);
            f(&mut t);
            standing(&t, None).0
        };
        assert_eq!(with_pr(&|_| {}), "In review");
        assert_eq!(with_pr(&|t| t.review = Some("APPROVED".into())), "Approved");
        assert_eq!(
            with_pr(&|t| t.review = Some("CHANGES_REQUESTED".into())),
            "Changes asked"
        );
        assert_eq!(
            with_pr(&|t| {
                t.merge = Some(MergeState::Queued);
                t.position = Some(2);
            }),
            "2nd in the merge queue"
        );
        assert_eq!(
            with_pr(&|t| {
                t.landing = Some(crate::facts::Landing {
                    holding: false,
                    line: 3,
                    method: crate::facts::Method::Direct,
                })
            }),
            "3rd in the landing queue"
        );
        assert_eq!(
            with_pr(&|t| t.phase = Some("tests failed: 2".into())),
            "✕ tests failed"
        );
        let bare = TabModel::new(TabId(2), 2, "t");
        let counted = Counts {
            head: "a b".into(),
            default: "main".into(),
            ahead: Some(3),
            to_push: Some(2),
            ..Counts::default()
        };
        assert_eq!(standing(&bare, Some(&counted)).0, "2 commits to push");
        assert_eq!(standing(&bare, None).0, "no PR");
        let mut landed = bare.clone();
        landed.landed_on = Some(("abc1234".into(), "main".into()));
        landed.merge = Some(MergeState::Merged);
        assert_eq!(standing(&landed, None), ("Landed abc1234".into(), "done"));
        let mut checked = bare.clone();
        checked.pr = Some(4);
        checked.checks = Some(crate::facts::Checks {
            pass: 3,
            fail: 0,
            pending: 2,
        });
        assert_eq!(checks(&checked), ("◌ 3 of 5".into(), "needs"));
        checked.checks = Some(crate::facts::Checks {
            pass: 3,
            fail: 1,
            pending: 0,
        });
        assert_eq!(checks(&checked), ("✕ 1 failing".into(), "failed"));
    }

    #[test]
    fn a_row_is_counted_once_for_each_commit_it_is_at() {
        let mut counts = HashMap::new();
        let tree = Path::new("/r/wt");
        let pushed_at = |at: &'static str| {
            move |r: &str| {
                if r == "refs/remotes/origin/topic" {
                    at.to_string()
                } else {
                    String::new()
                }
            }
        };
        assert!(needs_count(&counts, tree, "a b", &pushed_at("p1")));
        counts.insert(
            tree.to_path_buf(),
            Counts {
                head: "a b".into(),
                default: "main".into(),
                ahead: Some(1),
                to_push: Some(1),
                upstream: "refs/remotes/origin/topic".into(),
                upstream_at: "p1".into(),
            },
        );
        assert!(
            !needs_count(&counts, tree, "a b", &pushed_at("p1")),
            "moving back counts none"
        );
        assert!(
            needs_count(&counts, tree, "c b", &pushed_at("p1")),
            "a new commit counts again"
        );
        assert!(
            needs_count(&counts, tree, "a b", &pushed_at("p2")),
            "a push to its upstream counts again"
        );
    }

    #[test]
    fn settings_are_listed_by_category_and_a_filter_keeps_only_categories_with_a_match() {
        let items = settings_items(&[], false);
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
        let narrowed = shown(&view, "push notif", None, &Folds::default());
        let titles: Vec<&str> = narrowed
            .data
            .items
            .iter()
            .map(|i| i.title.as_str())
            .collect();
        assert_eq!(titles, ["Notifications", "Push notifications"]);
    }

    #[test]
    fn a_changed_option_is_marked_with_its_default_and_counted_under_its_heading() {
        let mut all = settings::schema();
        let github = all.iter_mut().find(|o| o.key == "github").unwrap();
        github.value = serde_json::Value::Bool(false);
        let items = settings_items_of(&all, &[], false);
        assert_eq!(items[0].data["changed"], 1);
        assert_eq!(items[1].data["changed"], true);
        assert_eq!(items[1].data["default"], "on");
        assert_eq!(items[1].data["value"], "off");
        assert_eq!(items[2].data["changed"], false);

        let named = |ux2: bool| {
            settings_items_of(&all, &[], ux2)
                .into_iter()
                .filter(is_header)
                .map(|h| h.title)
                .collect::<Vec<_>>()
        };
        assert!(named(true).contains(&"Tabs and header".to_string()));
        assert!(!named(true).contains(&"Tabs and status bar".to_string()));
        assert!(named(false).contains(&"Tabs and status bar".to_string()));
        // Either name finds the category's options.
        let tabs = settings_items_of(&all, &[], true)
            .into_iter()
            .find(|i| search_text(i).contains("tabs and status bar") && !is_header(i));
        assert!(tabs.is_some_and(|i| search_text(&i).contains("tabs and header")));
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
            started: None,
            ended: None,
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
                    &[],
                    &std::collections::HashMap::new(),
                ),
                ..ViewData::default()
            },
            loaded: true,
        };
        let narrowed = shown(&view, "board", None, &Folds::default());
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

    #[test]
    fn every_view_says_what_it_is_and_shows_under_its_own_settings() {
        for id in ViewId::BUILT_IN {
            assert!(
                id.about()
                    .starts_with(&format!("Mission control: {}, ", id.title())),
                "{}",
                id.about()
            );
            assert!(!id.detail().is_empty(), "{}", id.title());
            assert!(
                id.shows_with(|_| true),
                "{} shows with everything on",
                id.title()
            );
        }
        let off = |off: &'static [&'static str]| move |k: &str| !off.contains(&k);
        let shown = |on: &dyn Fn(&str) -> bool| -> Vec<&str> {
            ViewId::BUILT_IN
                .iter()
                .filter(|v| v.shows_with(on))
                .map(|v| v.title())
                .collect::<Vec<_>>()
        };
        assert!(!shown(&off(&["usage"])).contains(&"Usage"));
        let linear_off = shown(&off(&["linear"]));
        assert!(!linear_off.contains(&"Tickets") && !linear_off.contains(&"Inbox"));
        assert!(linear_off.contains(&"PRs"));
        assert!(!shown(&off(&["reminders"])).contains(&"Reminders"));
        assert!(!shown(&off(&["ux2"])).contains(&"Autopilot"));
        assert!(!shown(&off(&["sidebar_notifications"])).contains(&"Notifications"));
        assert!(ViewId::Plugin("deploys".into()).shows_with(|_| false));
        assert_eq!(ViewId::Plugin("deploys".into()).title(), "deploys");
        assert!(ViewId::Prs.tags().contains(&"github"));
        assert!(ViewId::Inbox.tags().contains(&"linear"));
        assert_eq!(ViewId::WorkMap.slug(), "work-map");
    }

    #[test]
    fn every_view_with_a_setting_is_one_of_the_built_in_views() {
        for v in crate::usage::habits::VIEWS {
            assert!(
                ViewId::BUILT_IN.iter().any(|id| id.title() == v.title),
                "{} is in mission control but not in ViewId::BUILT_IN",
                v.title
            );
        }
    }
}
