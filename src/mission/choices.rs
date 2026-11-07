//! What the menu Enter opens in mission control offers (lib/sidebar/
//! daemon.js's `tabChoices`, `itemChoices`, `boardChoices` and
//! `noticeChoices`): on a tab, going there first, so Enter twice goes, then
//! answering it, peeking at it, renaming it, pinning it, a reminder, its
//! place in the landing queue, closing it; on an item, what its source can
//! do with it, its usual action first.
//!
//! Each option says what it does as data (`Do`), which `serve` carries out,
//! so the menus are plain functions of what is shown and the tests can read
//! them without a server.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::views::{self, ViewId};
use crate::day::calendar::{self, CalEvent, Rsvp};
use crate::items::{Item, Target};
use crate::mux::{PaneId, TabId};
use crate::sidebar::model::{MergeState, TabModel, TabState};
use crate::sources::Waiter;
use crate::usage::habits;
use crate::worktree;

/// What an option does.
#[derive(Clone, Debug, PartialEq)]
pub enum Do {
    /// Mission control closes and the tab comes to the front.
    GoTo(TabId),
    /// Its prompt answered, or a message sent to it.
    Answer(TabId),
    /// A look at it without going there.
    Peek(TabId),
    /// The tab, and its diff in its pane's place (M-d).
    Diff(TabId),
    /// The tab, and lazygit in its pane's place.
    Lazygit(TabId),
    /// Its name typed in.
    Rename(TabId),
    Pin(TabId),
    /// A reminder set (remind.sh's times), or cleared.
    Remind(TabId),
    /// The tab, and M-p's palette in it.
    Palette(TabId),
    /// Its place in the landing queue moved (`land-lock move`).
    Land {
        root: PathBuf,
        way: &'static str,
    },
    /// The tab closed, asked first.
    CloseTab(TabId),
    /// Every merged tab closed and its worktree removed, asked first.
    CloseMerged(Vec<TabId>),
    Open(String),
    /// A link read in a pane beside the one the client is in, mission
    /// control closing first (daemon.js's `readHere`).
    ReadHere(String),
    Copy(String),
    /// A new tab in a directory, starting work on a ticket or a PR as its
    /// own command.
    StartIn {
        dir: PathBuf,
        work: Work,
    },
    /// Text typed into a tab's Claude, then the tab gone to (/ci-loop).
    TypeInto {
        tab: TabId,
        pane: PaneId,
        text: String,
    },
    /// A ticket moved to its team's first completed state, asked first.
    CloseTicket {
        ticket: String,
        title: String,
    },
    /// An Inbox notification marked read in Linear.
    MarkRead(String),
    /// An invitation answered.
    Rsvp(Box<CalEvent>, Rsvp),
    /// A tab of its own for a Slack thread, Claude told to read it.
    SlackThread {
        url: String,
        who: String,
        said: String,
    },
    /// Slack messages dismissed until their threads get more.
    SlackDismiss(Vec<Item>),
    /// A closed tab opened again.
    Reopen(String),
    RemoveClosed(String),
    /// A tab in a directory, resuming its Claude there.
    OpenHere(PathBuf),
    /// A merged worktree removed, asked first.
    RemoveWorktree(PathBuf),
    /// A suggestion's menu.
    Habit {
        id: String,
        outcome: habits::Outcome,
    },
    /// A notification put away, by its source.
    Dismiss {
        source: String,
        item: Box<Item>,
    },
    /// What working a note continuously offers (src/autopilot).
    Autopilot(crate::autopilot::Act),
    /// A CI job's kept log, read in a pane beside the one you're in.
    ReadLog {
        job: String,
        name: String,
    },
    /// A note written for you opened in Notes (a path in the notes
    /// directory), its notification let go.
    OpenNote(String),
    /// Mission control on a view, by its name lowercased with dashes.
    OpenView(String),
}

/// What a tab opened from a menu works on: what `wt`, `wtpr` and `shape`
/// did, typed into a new shell by the tmux version, run here as the tab's
/// own command through the binary (`command`), as the workspace's keys
/// start work (src/cmd/work.rs), so nothing needs the shell functions in
/// your rc and nothing races the shell's prompt. Only a ticket's ID or a
/// PR's address is ever in one, each a word of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Work {
    /// A worktree for the ticket, and Claude in it (`wt`).
    Ticket(String),
    /// Claude shaping the ticket with you (`shape`).
    Shape(String),
    /// A worktree on the PR's branch, and Claude in it (`wtpr`), with a
    /// first prompt of its own when there is one (WTPR_PROMPT).
    Pr { url: String, prompt: Option<String> },
}

impl Work {
    /// The new tab's command: an interactive shell of yours that runs the
    /// work and stays a shell once it ends.
    pub fn command(&self) -> Vec<String> {
        match self {
            Work::Ticket(ticket) => {
                worktree::tab_running("worktree-from-ticket", "wt", std::slice::from_ref(ticket))
            }
            Work::Shape(ticket) => {
                let script = format!("{} \"$@\"", worktree::self_command("shape"));
                worktree::tab_command(&script, "shape", std::slice::from_ref(ticket))
            }
            Work::Pr { url, prompt } => {
                let run =
                    worktree::tab_running("worktree-from-pr", "wtpr", std::slice::from_ref(url));
                match prompt {
                    Some(prompt) => ["env".to_string(), format!("WTPR_PROMPT={prompt}")]
                        .into_iter()
                        .chain(run)
                        .collect(),
                    None => run,
                }
            }
        }
    }
}

/// One option: the key that takes it at once, its label, and what it does.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub key: char,
    pub label: String,
    pub act: Do,
}

fn choice(key: char, label: impl Into<String>, act: Do) -> Choice {
    Choice {
        key,
        label: label.into(),
        act,
    }
}

/// A tab's menu. `waiter` is its place in the landing queue when it waits
/// there; `all` every tab, for closing the merged ones together.
pub fn tab_choices(
    tab: &TabModel,
    all: &[TabModel],
    waiter: Option<&Waiter>,
    reminders: bool,
) -> Vec<Choice> {
    let mut out = vec![choice(
        'g',
        format!("Go to tab {}", tab.index),
        Do::GoTo(tab.id),
    )];
    if tab.claude_pane.is_some() {
        out.push(choice(
            'a',
            "Answer it here, or send it a message",
            Do::Answer(tab.id),
        ));
    }
    out.push(choice(
        'v',
        if tab.claude.is_some() {
            "Peek at its last turn"
        } else {
            "Peek at its screen"
        },
        Do::Peek(tab.id),
    ));
    out.push(choice('d', "Its diff (M-d)", Do::Diff(tab.id)));
    out.push(choice('L', "lazygit", Do::Lazygit(tab.id)));
    out.push(choice('p', "Actions (M-p) in the tab", Do::Palette(tab.id)));
    out.push(choice('r', "Rename it", Do::Rename(tab.id)));
    out.push(choice(
        'f',
        if tab.pinned {
            "Unpin it"
        } else {
            "Pin it, as needing you (M-f)"
        },
        Do::Pin(tab.id),
    ));
    if reminders {
        out.push(choice(
            'e',
            if tab.remind_at.is_some() {
                "Change or clear its reminder"
            } else {
                "Remind me about it later"
            },
            Do::Remind(tab.id),
        ));
    }
    // A tab waiting in the landing queue moves in it, never past the
    // landing one, which isn't in the queue.
    if let Some(w) = waiter.filter(|_| tab.landing.is_some_and(|l| !l.holding)) {
        let land = |way| Do::Land {
            root: w.root.clone(),
            way,
        };
        if !w.first {
            out.push(choice('n', "Land it next", land("first")));
            out.push(choice('u', "Move it up in the landing queue", land("up")));
        }
        if !w.last {
            out.push(choice(
                'b',
                "Move it down in the landing queue",
                land("down"),
            ));
        }
    }
    out.push(choice('x', "Close the tab", Do::CloseTab(tab.id)));
    let merged: Vec<TabId> = all
        .iter()
        .filter(|t| {
            t.merge == Some(MergeState::Merged)
                && !matches!(t.state, TabState::Waiting | TabState::Done)
        })
        .map(|t| t.id)
        .collect();
    if tab.merge == Some(MergeState::Merged) && !merged.is_empty() {
        out.push(choice(
            'X',
            format!("Close every Merged tab and its worktree ({})", merged.len()),
            Do::CloseMerged(merged),
        ));
    }
    out
}

/// What a menu needs to know beyond the item: the tabs, the GitHub
/// repository a directory is in, your home (where work starts when no tab
/// is in the repository), and the suggestions' own menus.
pub struct Ctx<'a> {
    pub tabs: &'a [TabModel],
    pub repo_of: &'a dyn Fn(&Path) -> Option<String>,
    pub home: PathBuf,
    pub habit: &'a dyn Fn(&str) -> Vec<habits::Choice>,
}

static PR_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https://github\.com/[\w.-]+/[\w.-]+/pull/\d+$").expect("a regex")
});
static TICKET_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][A-Z0-9]*-\d+$").expect("a regex"));

/// Whether a tab's name has a ticket (or a pattern), and not one whose
/// number only starts the same (AB-3 isn't AB-30).
fn names(tab: &TabModel, pattern: &str) -> bool {
    Regex::new(&format!("(?i)(^|[^A-Za-z0-9]){pattern}([^0-9]|$)"))
        .is_ok_and(|re| re.is_match(&tab.name))
}

fn text<'a>(v: &'a Value, pointer: &str) -> &'a str {
    v.pointer(pointer).and_then(Value::as_str).unwrap_or("")
}

/// The tab a PR is open in, and the directory of a tab in its repository
/// (else your home), where work on it starts.
fn pr_places(ctx: &Ctx<'_>, number: u64, repo: &str) -> (Option<TabModel>, PathBuf) {
    let in_repo = |t: &&TabModel| {
        t.path
            .as_deref()
            .and_then(|p| (ctx.repo_of)(p))
            .is_some_and(|r| r == repo)
    };
    let tab = ctx
        .tabs
        .iter()
        .filter(in_repo)
        .find(|t| t.pr == Some(number))
        .cloned();
    let dir = ctx
        .tabs
        .iter()
        .find(in_repo)
        .and_then(|t| t.path.clone())
        .unwrap_or_else(|| ctx.home.clone());
    (tab, dir)
}

/// A PR's or ticket's menu, on a board or in the Inbox: the tab where it is
/// open, work started on it in a new tab, closing a ticket, then opening it
/// and copying it.
pub fn board_choices(item: &Item, ctx: &Ctx<'_>) -> Vec<Choice> {
    let mut out = Vec::new();
    let url = text(&item.data, "/url").to_string();
    let url = if url.is_empty() {
        match &item.target {
            Target::Url(u) => u.clone(),
            _ => String::new(),
        }
    } else {
        url
    };
    let number = item.data.pointer("/pr/number").and_then(Value::as_u64);
    let repo = text(&item.data, "/pr/repo");
    if let Some(n) = number
        && PR_URL.is_match(&url)
    {
        let (tab, dir) = pr_places(ctx, n, repo);
        if let Some(t) = tab {
            out.push(choice(
                't',
                format!("Go to tab {}, where it is open", t.index),
                Do::GoTo(t.id),
            ));
        }
        out.push(choice(
            'w',
            "Work on it in a new tab (wtpr)",
            Do::StartIn {
                dir,
                work: Work::Pr {
                    url: url.clone(),
                    prompt: None,
                },
            },
        ));
    }
    let ticket = text(&item.data, "/ticket").to_string();
    if TICKET_ID.is_match(&ticket) {
        if let Some(t) = ctx.tabs.iter().find(|t| names(t, &regex::escape(&ticket))) {
            out.push(choice(
                't',
                format!("Go to tab {}, where it is open", t.index),
                Do::GoTo(t.id),
            ));
        }
        let team = ticket.split('-').next().unwrap_or("");
        let dir = ctx
            .tabs
            .iter()
            .filter(|t| t.path.is_some())
            .find(|t| names(t, &format!("{}-\\d+", regex::escape(team))))
            .and_then(|t| t.path.clone())
            .unwrap_or_else(|| ctx.home.clone());
        out.push(choice(
            'w',
            "Start work on it in a new tab (wt)",
            Do::StartIn {
                dir: dir.clone(),
                work: Work::Ticket(ticket.clone()),
            },
        ));
        out.push(choice(
            's',
            "Shape it in a new tab (shape)",
            Do::StartIn {
                dir,
                work: Work::Shape(ticket.clone()),
            },
        ));
        out.push(choice(
            'D',
            "Close it in Linear (its team's Done)",
            Do::CloseTicket {
                ticket: ticket.clone(),
                title: item.title.clone(),
            },
        ));
    }
    if !url.is_empty() {
        out.push(choice(
            'v',
            "Read it here, in a pane beside",
            Do::ReadHere(url.clone()),
        ));
        out.push(choice('o', "Open it in the browser", Do::Open(url.clone())));
    }
    if !ticket.is_empty() {
        out.push(choice(
            'y',
            format!("Copy its ID ({ticket})"),
            Do::Copy(ticket),
        ));
    } else if !url.is_empty() {
        out.push(choice('y', "Copy its link", Do::Copy(url)));
    }
    out
}

/// What an item's target offers when its source has nothing of its own.
fn target_choices(item: &Item, ctx: &Ctx<'_>) -> Vec<Choice> {
    match &item.target {
        Target::Tab(t) => {
            let index = ctx.tabs.iter().find(|x| x.id == *t).map_or(0, |x| x.index);
            vec![choice('g', format!("Go to tab {index}"), Do::GoTo(*t))]
        }
        Target::Url(u) => vec![
            choice('o', "Open it", Do::Open(u.clone())),
            choice('y', "Copy its link", Do::Copy(u.clone())),
        ],
        Target::Dir(d) => vec![
            choice(
                'o',
                "Open a tab there, resuming its Claude",
                Do::OpenHere(d.clone()),
            ),
            choice(
                'y',
                "Copy its directory",
                Do::Copy(d.to_string_lossy().into_owned()),
            ),
        ],
        Target::None => Vec::new(),
    }
}

/// An item's menu in a view after the tabs.
pub fn item_choices(view: &ViewId, item: &Item, shown: &[Item], ctx: &Ctx<'_>) -> Vec<Choice> {
    let mut out = Vec::new();
    match view {
        ViewId::Prs | ViewId::Tickets => return board_choices(item, ctx),
        ViewId::Inbox => {
            out = board_choices(item, ctx);
            let id = text(&item.data, "/notification");
            let id = if id.is_empty() { item.id.as_str() } else { id };
            out.push(choice('r', "Mark it read", Do::MarkRead(id.to_string())));
        }
        ViewId::Ci => {
            let ci = &item.data["ci"];
            let url = text(ci, "/url").to_string();
            let number = ci.get("number").and_then(Value::as_u64).unwrap_or(0);
            let (tab, dir) = pr_places(ctx, number, text(ci, "/repo"));
            // A tab already on it whose Claude is idle takes the CI loop
            // where it is; else a new tab on the PR's worktree starts with it.
            match &tab {
                Some(t)
                    if t.claude_pane.is_some()
                        && matches!(t.state, TabState::Idle | TabState::Done) =>
                {
                    out.push(choice(
                        'c',
                        format!("Start a CI loop on it in tab {}", t.index),
                        Do::TypeInto {
                            tab: t.id,
                            pane: t.claude_pane.expect("checked"),
                            text: "/ci-loop".into(),
                        },
                    ));
                }
                _ if PR_URL.is_match(&url) => out.push(choice(
                    'c',
                    "Start a CI loop on it in a new tab",
                    Do::StartIn {
                        dir,
                        work: Work::Pr {
                            url: url.clone(),
                            prompt: Some("/ci-loop".into()),
                        },
                    },
                )),
                _ => {}
            }
            if let Some(t) = tab {
                out.push(choice(
                    'g',
                    format!("Go to tab {}, where it is open", t.index),
                    Do::GoTo(t.id),
                ));
            }
            // The failing job's log, kept when the view read why it failed.
            let job = text(ci, "/job");
            if !job.is_empty() && ci.get("logged").and_then(Value::as_bool) == Some(true) {
                let name = text(ci, "/jobName");
                out.push(choice(
                    'l',
                    format!("Read the failing log ({name}) in a pane beside"),
                    Do::ReadLog {
                        job: job.to_string(),
                        name: name.to_string(),
                    },
                ));
            }
            let check = text(ci, "/check/url");
            if !check.is_empty() {
                out.push(choice(
                    'o',
                    "Open the failing check in the browser",
                    Do::Open(check.to_string()),
                ));
            }
            if !url.is_empty() {
                out.push(choice(
                    'v',
                    "Read the PR here, in a pane beside",
                    Do::ReadHere(url.clone()),
                ));
                out.push(choice(
                    'p',
                    "Open the PR in the browser",
                    Do::Open(url.clone()),
                ));
                out.push(choice('y', "Copy its link", Do::Copy(url)));
            }
        }
        ViewId::Queues => {
            let q = &item.data["queue"];
            let url = text(q, "/url").to_string();
            if let Some(t) = ctx
                .tabs
                .iter()
                .find(|t| Some(t.id.to_string().as_str()) == q.get("tab").and_then(Value::as_str))
            {
                out.push(choice(
                    'g',
                    format!("Go to tab {}, where it is open", t.index),
                    Do::GoTo(t.id),
                ));
            }
            if PR_URL.is_match(&url) {
                out.push(choice(
                    'v',
                    "Read the PR here, in a pane beside",
                    Do::ReadHere(url.clone()),
                ));
            }
            if !url.is_empty() {
                out.push(choice(
                    'p',
                    "Open the PR in the browser",
                    Do::Open(url.clone()),
                ));
            }
            let queue = text(q, "/queueUrl");
            if !queue.is_empty() {
                out.push(choice(
                    'q',
                    "Open the merge queue in the browser",
                    Do::Open(queue.to_string()),
                ));
            }
            if !url.is_empty() {
                out.push(choice('y', "Copy its link", Do::Copy(url)));
            }
        }
        ViewId::Slack => {
            if let Target::Url(url) = &item.target {
                out.push(choice('o', "Open it in Slack", Do::Open(url.clone())));
                let who = item
                    .title
                    .split_once(' ')
                    .map_or(item.title.as_str(), |(_, rest)| rest)
                    .trim_end_matches(" · seen")
                    .to_string();
                let said = if item.lines.is_empty() {
                    item.detail.clone()
                } else {
                    item.lines.join("\n")
                };
                out.push(choice(
                    't',
                    "Open a tab for this thread with Claude",
                    Do::SlackThread {
                        url: url.clone(),
                        who,
                        said,
                    },
                ));
            }
            if item.data.get("message").is_some() {
                out.push(choice(
                    'x',
                    "Dismiss it, until its thread gets a reply",
                    Do::SlackDismiss(vec![item.clone()]),
                ));
                out.push(choice(
                    'X',
                    "Dismiss every message shown",
                    Do::SlackDismiss(
                        shown
                            .iter()
                            .filter(|i| i.data.get("message").is_some())
                            .cloned()
                            .collect(),
                    ),
                ));
            }
        }
        ViewId::Calendar => {
            let event: Option<CalEvent> = serde_json::from_value(item.data.clone()).ok();
            if let Some(e) = event {
                if let Some(join) = &e.join {
                    out.push(choice('j', "Join its call", Do::Open(join.clone())));
                }
                if let Some(link) = &e.link {
                    out.push(choice(
                        'o',
                        "Open it in Google Calendar",
                        Do::Open(link.clone()),
                    ));
                }
                for r in calendar::rsvp_choices(&e) {
                    let (key, label) = r.choice();
                    out.push(choice(
                        key.chars().next().unwrap_or('r'),
                        label,
                        Do::Rsvp(Box::new(e.clone()), r),
                    ));
                }
                for (i, r) in e.refs.iter().enumerate() {
                    if let Some(url) = &r.url {
                        out.push(choice(
                            char::from_digit(i as u32 + 1, 10).unwrap_or('#'),
                            format!("Read {} here", r.id),
                            Do::ReadHere(url.clone()),
                        ));
                    }
                }
            }
        }
        ViewId::Closed => {
            let key = text(&item.data, "/closed/key");
            let key = if key.is_empty() {
                item.id.as_str()
            } else {
                key
            };
            let shell = item.data.get("session").is_none_or(Value::is_null);
            out.push(choice(
                'r',
                if shell {
                    "Reopen it, with a shell"
                } else {
                    "Reopen it, resuming its Claude"
                },
                Do::Reopen(key.to_string()),
            ));
            out.push(choice(
                'x',
                "Remove it from this list",
                Do::RemoveClosed(key.to_string()),
            ));
            if let Target::Dir(d) = &item.target {
                out.push(choice(
                    'y',
                    "Copy its directory",
                    Do::Copy(d.to_string_lossy().into_owned()),
                ));
            }
        }
        ViewId::WorkMap => {
            out = target_choices(item, ctx);
            let merged = item.data.get("merged").and_then(Value::as_bool) == Some(true);
            if let (true, Target::Dir(d)) = (merged, &item.target) {
                out.push(choice(
                    'x',
                    "Remove this merged worktree",
                    Do::RemoveWorktree(d.clone()),
                ));
            }
        }
        ViewId::Notifications => return notice_choices(item, ctx),
        _ => out = target_choices(item, ctx),
    }
    out
}

/// A notification's source and its own id, from the id the Notifications
/// view gives it (`<source>\t<id>`).
pub fn notice_of(item: &Item) -> (String, String) {
    match item.id.split_once('\t') {
        Some((source, id)) => (source.to_string(), id.to_string()),
        None => (String::new(), item.id.clone()),
    }
}

/// Where Enter on a notification takes it when it leads somewhere of its own
/// (daemon.js's `openNotice`): a new note opens in Notes, a calendar change
/// opens the Calendar view. `source` is the source that gave it.
pub fn notice_opens(source: &str, item: &Item) -> Option<Do> {
    match source {
        "new-notes" => item
            .data
            .get("path")
            .and_then(Value::as_str)
            .map(|p| Do::OpenNote(p.to_string())),
        "calendar" => Some(Do::OpenView("calendar".into())),
        _ => None,
    }
}

/// A notification's menu: a suggestion's own choices; else where it leads
/// and putting it away.
pub fn notice_choices(item: &Item, ctx: &Ctx<'_>) -> Vec<Choice> {
    let (source, id) = notice_of(item);
    if source == "habits" {
        return (ctx.habit)(&id)
            .into_iter()
            .map(|c| {
                choice(
                    c.key,
                    c.label,
                    Do::Habit {
                        id: id.clone(),
                        outcome: c.outcome,
                    },
                )
            })
            .collect();
    }
    if source == "autopilot" {
        return crate::autopilot::line::notice_choices()
            .into_iter()
            .map(|(key, label, act)| choice(key, label, Do::Autopilot(act)))
            .collect();
    }
    let mut own = item.clone();
    own.id = id;
    let mut out = if let Some(open) = notice_opens(&source, item) {
        vec![choice(
            'o',
            match &open {
                Do::OpenNote(_) => "Open it in Notes",
                _ => "Open the Calendar view",
            },
            open,
        )]
    } else if source == "inbox" {
        let mut b = board_choices(&own, ctx);
        let n = text(&own.data, "/notification");
        b.push(choice(
            'r',
            "Mark it read",
            Do::MarkRead(if n.is_empty() {
                own.id.clone()
            } else {
                n.to_string()
            }),
        ));
        b
    } else {
        target_choices(&own, ctx)
    };
    if matches!(
        source.as_str(),
        "meeting" | "slack" | "calendar" | "new-notes"
    ) {
        out.push(choice(
            'x',
            "Dismiss it",
            Do::Dismiss {
                source,
                item: Box::new(own),
            },
        ));
    }
    out
}

/// Whether Enter on an item does something of its own rather than open a
/// menu: a heading, a setting, a plugin's item.
pub fn is_plain(view: &ViewId, item: &Item) -> bool {
    views::is_header(item)
        || item.data.get("setting").is_some()
        || item.data.get("own").is_some()
        || matches!(view, ViewId::Plugin(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::Landing;
    use crate::sidebar::claude::SessionRef;
    use serde_json::json;

    fn tab(n: u32, name: &str) -> TabModel {
        let mut t = TabModel::new(TabId(n), n as usize, name);
        t.claude = Some(SessionRef::default());
        t.claude_pane = Some(PaneId(n * 10));
        t.path = Some(format!("/r{n}").into());
        t
    }

    fn keys(choices: &[Choice]) -> String {
        choices.iter().map(|c| c.key).collect()
    }

    fn ctx<'a>(tabs: &'a [TabModel], repo: &'a dyn Fn(&Path) -> Option<String>) -> Ctx<'a> {
        Ctx {
            tabs,
            repo_of: repo,
            home: "/home/me".into(),
            habit: &|_| Vec::new(),
        }
    }

    #[test]
    fn a_tabs_menu_goes_there_first_then_answers_peeks_renames_pins_and_reminds() {
        let mut t = tab(3, "fix");
        let choices = tab_choices(&t, std::slice::from_ref(&t), None, true);
        assert_eq!(keys(&choices), "gavdLprfex");
        assert_eq!(choices[0].label, "Go to tab 3");
        assert_eq!(choices[0].act, Do::GoTo(TabId(3)));
        t.claude_pane = None;
        t.claude = None;
        t.pinned = true;
        let choices = tab_choices(&t, std::slice::from_ref(&t), None, false);
        assert_eq!(
            keys(&choices),
            "gvdLprfx",
            "no answer without a Claude, no reminder when off"
        );
        assert_eq!(choices[1].label, "Peek at its screen");
        assert_eq!(choices[2].act, Do::Diff(TabId(3)));
        assert_eq!(choices[3].act, Do::Lazygit(TabId(3)));
        assert_eq!(choices[6].label, "Unpin it");
    }

    #[test]
    fn a_tab_waiting_to_land_moves_in_the_queue_and_merged_tabs_close_together() {
        let mut waiting = tab(4, "land me");
        waiting.landing = Some(Landing {
            holding: false,
            line: 2,
        });
        let w = Waiter {
            root: "/r4".into(),
            first: false,
            last: true,
        };
        let choices = tab_choices(&waiting, std::slice::from_ref(&waiting), Some(&w), true);
        assert!(keys(&choices).contains("nu") && !keys(&choices).contains('b'));
        let mut merged = tab(5, "done with");
        merged.merge = Some(MergeState::Merged);
        let mut also = tab(6, "also merged");
        also.merge = Some(MergeState::Merged);
        let all = vec![merged.clone(), also];
        let choices = tab_choices(&merged, &all, None, true);
        let last = choices.last().unwrap();
        assert_eq!(last.label, "Close every Merged tab and its worktree (2)");
        assert_eq!(last.act, Do::CloseMerged(vec![TabId(5), TabId(6)]));
    }

    #[test]
    fn a_pr_on_the_board_offers_its_tab_new_work_and_its_link() {
        let mut open = tab(2, "#12 the fix");
        open.pr = Some(12);
        let tabs = vec![open];
        let repo = |_: &Path| Some("acme/app".to_string());
        let item = Item::new("u", "#12 the fix")
            .target(Target::Url("https://github.com/acme/app/pull/12".into()))
            .data(json!({ "pr": { "number": 12, "repo": "acme/app" }, "url": "https://github.com/acme/app/pull/12" }));
        let choices = item_choices(&ViewId::Prs, &item, &[], &ctx(&tabs, &repo));
        assert_eq!(keys(&choices), "twvoy");
        assert_eq!(
            choices[2].act,
            Do::ReadHere("https://github.com/acme/app/pull/12".into())
        );
        assert_eq!(choices[0].act, Do::GoTo(TabId(2)));
        assert_eq!(
            choices[1].act,
            Do::StartIn {
                dir: "/r2".into(),
                work: Work::Pr {
                    url: "https://github.com/acme/app/pull/12".into(),
                    prompt: None,
                },
            }
        );
    }

    #[test]
    fn work_starts_as_the_tabs_own_command_through_the_binary() {
        let runs = |command: &[String], what: &str| command.iter().any(|w| w.contains(what));
        let wt = Work::Ticket("AB-3".into()).command();
        assert!(runs(&wt, "worktree-from-ticket init"), "{wt:?}");
        assert!(runs(&wt, "wt \"$@\""), "{wt:?}");
        assert_eq!(wt.last().map(String::as_str), Some("AB-3"));

        let url = "https://github.com/acme/app/pull/12";
        let pr = Work::Pr {
            url: url.into(),
            prompt: None,
        }
        .command();
        assert!(runs(&pr, "worktree-from-pr init") && runs(&pr, "wtpr \"$@\""));
        assert_eq!(pr.last().map(String::as_str), Some(url));
        let ci = Work::Pr {
            url: url.into(),
            prompt: Some("/ci-loop".into()),
        }
        .command();
        assert_eq!(&ci[..2], ["env", "WTPR_PROMPT=/ci-loop"]);
        assert_eq!(&ci[2..], &pr[..]);

        let shape = Work::Shape("AB-3".into()).command();
        assert!(runs(&shape, " shape \"$@\""), "{shape:?}");
        assert_eq!(shape.last().map(String::as_str), Some("AB-3"));
    }

    #[test]
    fn a_failing_pr_reads_its_kept_log_beside_when_there_is_one() {
        let mut open = tab(2, "#12 the fix");
        open.pr = Some(12);
        let tabs = vec![open];
        let repo = |_: &Path| Some("acme/app".to_string());
        let url = "https://github.com/acme/app/pull/12";
        let ci = |logged: bool| {
            Item::new(url, "#12 the fix").data(json!({ "ci": {
                "number": 12, "repo": "acme/app", "url": url, "job": "77",
                "jobName": "test", "logged": logged,
                "check": { "name": "test", "url": "https://github.com/acme/app/actions/runs/1/job/77" },
            } }))
        };
        let choices = item_choices(&ViewId::Ci, &ci(true), &[], &ctx(&tabs, &repo));
        assert_eq!(keys(&choices), "cglopy");
        assert_eq!(
            choices[2].label,
            "Read the failing log (test) in a pane beside"
        );
        assert_eq!(
            choices[2].act,
            Do::ReadLog {
                job: "77".into(),
                name: "test".into()
            }
        );
        let choices = item_choices(&ViewId::Ci, &ci(false), &[], &ctx(&tabs, &repo));
        assert_eq!(keys(&choices), "cgopy", "no log kept, nothing to read");
    }

    #[test]
    fn a_ticket_offers_work_shaping_and_closing_and_the_inbox_marks_read() {
        let tabs = vec![tab(7, "AB-30: other"), tab(8, "AB-3: this one")];
        let repo = |_: &Path| None;
        let item = Item::new("n1", "AB-3 Do it")
            .target(Target::Url("https://linear.app/x/issue/AB-3".into()))
            .data(json!({ "ticket": "AB-3", "notification": "n1", "url": "https://linear.app/x/issue/AB-3" }));
        let choices = item_choices(&ViewId::Inbox, &item, &[], &ctx(&tabs, &repo));
        assert_eq!(keys(&choices), "twsDvoyr");
        assert_eq!(choices[0].act, Do::GoTo(TabId(8)), "AB-3, not AB-30");
        assert_eq!(choices.last().unwrap().act, Do::MarkRead("n1".into()));
    }

    #[test]
    fn a_suggestion_takes_its_own_choices_and_a_notification_can_be_dismissed() {
        let tabs: Vec<TabModel> = Vec::new();
        let repo = |_: &Path| None;
        let habit = |id: &str| {
            vec![habits::Choice {
                key: 'y',
                label: format!("Got {id}"),
                outcome: habits::Outcome::GotIt,
            }]
        };
        let c = Ctx {
            tabs: &tabs,
            repo_of: &repo,
            home: "/h".into(),
            habit: &habit,
        };
        let suggestion = Item::new("habits\tkey:M-d", "Press M-d");
        let choices = notice_choices(&suggestion, &c);
        assert_eq!(choices[0].label, "Got key:M-d");
        let meeting = Item::new("meeting\tm1", "Standup in 5m")
            .target(Target::Url("https://meet.example/x".into()));
        let choices = notice_choices(&meeting, &c);
        assert_eq!(keys(&choices), "oyx");
        assert!(matches!(&choices[2].act, Do::Dismiss { source, .. } if source == "meeting"));
    }

    #[test]
    fn a_new_note_opens_in_notes_and_a_calendar_change_opens_the_calendar() {
        let tabs: Vec<TabModel> = Vec::new();
        let repo = |_: &Path| None;
        let c = ctx(&tabs, &repo);
        let note = Item::new("new-notes\tnote:Meetings/Standup.md", "Standup")
            .data(json!({ "kind": "note", "path": "Meetings/Standup.md" }));
        let choices = notice_choices(&note, &c);
        assert_eq!(keys(&choices), "ox");
        assert_eq!(choices[0].label, "Open it in Notes");
        assert_eq!(choices[0].act, Do::OpenNote("Meetings/Standup.md".into()));
        let change = Item::new("calendar\tcalendar:k1", "Standup moved to 3pm")
            .data(json!({ "kind": "calendar-change", "key": "k1" }));
        let choices = notice_choices(&change, &c);
        assert_eq!(keys(&choices), "ox");
        assert_eq!(choices[0].act, Do::OpenView("calendar".into()));
        assert_eq!(notice_opens("slack", &change), None);
    }
}
