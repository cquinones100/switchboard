//! Mission control's views on the phone (boards 185 to 187): which of
//! mission control's views the phone's panel lists, each one's items under
//! the groups mission control files them under, what needs you in it, and
//! which options of its menus the phone offers and how. The views are
//! mission control's own (`Server::mission_views`) and their options its
//! own (`Server::item_choices`, `view_acts::of`), never a copy of their
//! rules: this only sorts what they give into what the phone shows, as
//! plain data tested without a server. `phone::mission` puts it on the
//! server. New with the phone's panel; no tmux-version file is behind it.
//!
//! An option is offered only when it needs nothing of the computer's
//! screen (`kind_of`): going to a tab is opening its channel, opening or
//! copying a link happens on the phone, and what runs on the computer
//! (`Kind::Run`) is a short list the server runs with no client
//! (`Server::mission_run`, `whole_run`), asked first on the phone where
//! mission control asks, or where it starts work in a tab. Reading in a
//! pane beside, the diff, peeking, answering, renaming, reminding, picking
//! a tab to send to and every other box of further choices are left out.

use serde_json::{Value, json};

use crate::autopilot::Act;
use crate::items::{Item, State};
use crate::mission::choices::{self, Do};
use crate::mission::view_acts::{self, Whole};
use crate::mission::views::{self, View, ViewId};
use crate::mux::TabId;

/// The views the phone lists, in its order, each while mission control
/// would list it for a client with no view kept in front.
pub const SHOWN: [ViewId; 5] = [
    ViewId::Landing,
    ViewId::Autopilot,
    ViewId::Prs,
    ViewId::Ci,
    ViewId::Notifications,
];

/// How many of the Notifications view's rows show before a day folds: a
/// phone's screenful.
pub const SCREENFUL: usize = 30;

/// What a tap on an option does on the phone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The tab's channel opened, by its Claude pane (`%<n>`).
    Go(String),
    /// The link opened on the phone.
    Open(String),
    /// The text copied on the phone.
    Copy(String),
    /// Run on the computer, asked first with the question when there is one.
    Run(Option<String>),
}

/// What the phone does with an option of an item's menu, `None` leaving
/// it out: `pane_of` gives a tab's Claude pane, the phone having a
/// channel only for a tab running Claude.
pub fn kind_of(act: &Do, label: &str, pane_of: &dyn Fn(TabId) -> Option<String>) -> Option<Kind> {
    match act {
        Do::GoTo(tab) => pane_of(*tab).map(Kind::Go),
        Do::Open(url) => Some(Kind::Open(url.clone())),
        Do::Copy(text) => Some(Kind::Copy(text.clone())),
        Do::Land { .. }
        | Do::PrMove { .. }
        | Do::Dismiss { .. }
        | Do::DismissAll(_)
        | Do::SlackDismiss(_)
        | Do::Autopilot(Act::Skip(_) | Act::Unskip(_)) => Some(Kind::Run(None)),
        // Asked in mission control; one whose question can't be put has
        // nothing to run.
        Do::PrLeave { .. } | Do::LeaveQueue(_) | Do::Autopilot(Act::Priority(..)) => {
            choices::question(act).map(|q| Kind::Run(Some(q)))
        }
        // Work started in a tab, which the phone asks about first.
        Do::TypeInto { .. } | Do::StartIn { .. } => Some(Kind::Run(Some(starts(label)))),
        _ => None,
    }
}

/// What the phone does with an option of a view's own menu, as `kind_of`.
pub fn whole_kind(act: &Whole, label: &str) -> Option<Kind> {
    match act {
        Whole::Do(act) => kind_of(act, label, &|_| None),
        Whole::Refresh(_) => Some(Kind::Run(None)),
        Whole::RerunMain(_) => view_acts::question(act).map(|q| Kind::Run(Some(q))),
        _ => None,
    }
}

/// The question an option that starts work in a tab asks on the phone:
/// its label, asked.
fn starts(label: &str) -> String {
    format!("{}?", label.trim_end_matches('…'))
}

/// An option as the phone gets it: its label (what the server finds it
/// by), the label it shows (… after one asked first, as board 185 draws
/// it), and what a tap does.
pub fn option_json(label: &str, kind: &Kind) -> Value {
    let asked = matches!(kind, Kind::Run(Some(_)));
    let shown = if asked && !label.ends_with('…') {
        format!("{label}…")
    } else {
        label.to_string()
    };
    let mut v = json!({ "label": label, "shown": shown });
    match kind {
        Kind::Go(pane) => v["go"] = json!(pane),
        Kind::Open(url) => v["open"] = json!(url),
        Kind::Copy(text) => v["copy"] = json!(text),
        Kind::Run(Some(q)) => v["ask"] = json!(q),
        Kind::Run(None) => {}
    }
    v
}

/// A group of a view's items, as mission control files them: under a
/// heading, under a board's column, or under the group an item names.
#[derive(Debug, PartialEq)]
pub struct Group<'a> {
    pub title: String,
    pub items: Vec<&'a Item>,
}

/// A view's items in mission control's groups and order: a board's
/// columns in its order (the PR board), else each heading and the items
/// after it, or each run of items naming the same group; a view's summary
/// (Autopilot's heading) left out, as the phone's list says it.
pub fn groups(view: &View) -> Vec<Group<'_>> {
    let items = view.data.items.iter().filter(|i| !views::is_summary(i));
    if let Some(columns) = view.columns() {
        let rows: Vec<&Item> = items.filter(|i| !views::is_header(i)).collect();
        let mut out: Vec<Group> = columns
            .iter()
            .map(|c| Group {
                title: c.to_string(),
                items: rows
                    .iter()
                    .copied()
                    .filter(|i| i.group.as_deref() == Some(c))
                    .collect(),
            })
            .filter(|g| !g.items.is_empty())
            .collect();
        let rest: Vec<&Item> = rows
            .into_iter()
            .filter(|i| !columns.iter().any(|c| i.group.as_deref() == Some(c)))
            .collect();
        if !rest.is_empty() {
            out.push(Group {
                title: String::new(),
                items: rest,
            });
        }
        return out;
    }
    let mut out: Vec<Group> = Vec::new();
    for item in items {
        if views::is_header(item) {
            out.push(Group {
                title: item.title.clone(),
                items: Vec::new(),
            });
            continue;
        }
        let named = item.group.as_deref();
        let starts_one = match (out.last(), named) {
            (None, _) => true,
            (Some(g), Some(name)) => g.title != name && !g.items.is_empty(),
            (Some(_), None) => false,
        };
        if starts_one {
            out.push(Group {
                title: named.unwrap_or_default().to_string(),
                items: Vec::new(),
            });
        }
        if let Some(g) = out.last_mut() {
            if g.items.is_empty() && g.title.is_empty() {
                g.title = named.unwrap_or_default().to_string();
            }
            g.items.push(item);
        }
    }
    out.retain(|g| !g.items.is_empty());
    out
}

/// An item's mark and tone as mission control colors it: CI's runs by
/// where they stand, Landing's in its own tone and what landed passed,
/// else by its state.
pub fn mark(view: &ViewId, item: &Item) -> (&'static str, &'static str) {
    if *view == ViewId::Ci {
        return match views::ci_state(item) {
            "running" => ("⠹", "working"),
            "passing" => ("✓", "passed"),
            _ => ("✕", "failed"),
        };
    }
    match item.state {
        State::Attention => ("●", "needs"),
        State::Active if *view == ViewId::Landing => ("▲", "landing"),
        State::Idle if *view == ViewId::Landing => ("✓", "passed"),
        State::Active => ("●", "working"),
        State::Idle => ("○", "idle"),
    }
}

/// The line under a view's name in the panel's list: each group's count
/// and name, the first three, or its items' count when it has no groups,
/// or its words for having none.
pub fn line(view: &View) -> String {
    let all = groups(view);
    let named: Vec<String> = all
        .iter()
        .filter(|g| !g.title.is_empty())
        .take(3)
        .map(|g| format!("{} {}", g.items.len(), g.title.to_lowercase()))
        .collect();
    if !named.is_empty() {
        return named.join(" · ");
    }
    let n: usize = all.iter().map(|g| g.items.len()).sum();
    if n > 0 {
        return format!("{n} item{}", if n == 1 { "" } else { "s" });
    }
    let words = view
        .data
        .setup
        .as_deref()
        .or(view.data.empty.as_deref())
        .unwrap_or("Nothing here.");
    words.lines().next().unwrap_or_default().to_string()
}

/// An item as the phone draws it, with its options.
pub fn item_json(view: &ViewId, item: &Item, options: Vec<Value>) -> Value {
    let (mark, tone) = mark(view, item);
    json!({
        "id": item.id,
        "title": item.title,
        "detail": item.detail,
        "time": item.time,
        "mark": mark,
        "tone": tone,
        "options": options,
    })
}

/// A view as the phone's stream carries it: its key and title, its line,
/// what needs you counted (`Server::needing_you`) with the first one's
/// mark and tone (its first item's while nothing needs you), its groups of items each with its options
/// (`item_options`), its own options (`acts`), and its words for having
/// none, needing setting up, or its last look failing.
pub fn view_json(
    view: &View,
    item_options: &dyn Fn(&Item) -> Vec<Value>,
    acts: Vec<Value>,
) -> Value {
    let needs = crate::mux::server::Server::needing_you(view);
    let first = needs
        .first()
        .copied()
        .or_else(|| groups(view).first().and_then(|g| g.items.first().copied()))
        .map(|i| mark(&view.id, i));
    let groups: Vec<Value> = groups(view)
        .into_iter()
        .map(|g| {
            let items: Vec<Value> = g
                .items
                .iter()
                .map(|item| item_json(&view.id, item, item_options(item)))
                .collect();
            json!({ "title": g.title, "items": items })
        })
        .collect();
    json!({
        "key": view.key(),
        "title": views::bar_title(view),
        "line": line(view),
        "needs": needs.len(),
        "mark": first.map(|(m, _)| m),
        "tone": first.map(|(_, t)| t),
        "groups": groups,
        "acts": acts,
        "empty": view.data.empty,
        "setup": view.data.setup,
        "error": view.data.error,
        "loaded": view.loaded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::ViewData;
    use crate::mux::TabId;
    use std::path::PathBuf;

    fn view(id: ViewId, items: Vec<Item>) -> View {
        View {
            id,
            title: "View".into(),
            data: ViewData {
                items,
                ..ViewData::default()
            },
            loaded: true,
        }
    }

    #[test]
    fn the_screen_only_options_are_left_out() {
        let tab = TabId(4);
        let pane = |t: TabId| (t == tab).then(|| "%9".to_string());
        let none = |_| None;
        let left_out = [
            Do::ReadHere("https://x".into()),
            Do::Diff(tab),
            Do::Peek(tab),
            Do::Palette(tab),
            Do::Answer(tab),
            Do::Rename(tab),
            Do::TabStatus(tab),
            Do::ChooseTicket(tab),
            Do::Remind(tab),
            Do::ReadOutput(PathBuf::from("/x")),
            Do::OpenNote("a.md".into()),
        ];
        for act in left_out {
            assert_eq!(kind_of(&act, "x", &pane), None, "{act:?}");
        }
        assert_eq!(
            kind_of(&Do::GoTo(tab), "Go to tab 4", &pane),
            Some(Kind::Go("%9".into()))
        );
        assert_eq!(
            kind_of(&Do::GoTo(tab), "Go to tab 4", &none),
            None,
            "no channel for a tab with no Claude"
        );
        assert_eq!(
            kind_of(&Do::Copy("u".into()), "Copy its link", &none),
            Some(Kind::Copy("u".into()))
        );
    }

    #[test]
    fn what_asks_in_mission_control_or_starts_work_asks_on_the_phone() {
        let none = |_| None;
        let leave = Do::PrLeave {
            common: PathBuf::from("/r"),
            number: 93,
        };
        assert_eq!(
            kind_of(&leave, "Take it out of the merge queue", &none),
            Some(Kind::Run(Some("Take #93 out of the merge queue?".into())))
        );
        let ci = Do::TypeInto {
            tab: TabId(11),
            pane: crate::mux::PaneId(3),
            text: "/ci-loop".into(),
        };
        let kind = kind_of(&ci, "Start a CI loop on it in tab 11", &none).unwrap();
        assert_eq!(
            kind,
            Kind::Run(Some("Start a CI loop on it in tab 11?".into()))
        );
        let v = option_json("Start a CI loop on it in tab 11", &kind);
        assert_eq!(v["shown"], "Start a CI loop on it in tab 11…");
        assert_eq!(v["label"], "Start a CI loop on it in tab 11");
        let moved = Do::PrMove {
            common: PathBuf::from("/r"),
            number: 93,
            way: "down",
        };
        assert_eq!(
            kind_of(&moved, "Move it down", &none),
            Some(Kind::Run(None))
        );
        assert_eq!(
            whole_kind(&Whole::RerunMain(vec![("o/r".into(), "1".into())]), "x"),
            Some(Kind::Run(Some(
                "Run the failed jobs of 1 default branch run again on GitHub?".into()
            )))
        );
        assert_eq!(
            whole_kind(&Whole::Refresh("ci"), "Read CI again now"),
            Some(Kind::Run(None))
        );
    }

    #[test]
    fn items_go_under_their_headings_and_columns() {
        let ci = view(
            ViewId::Ci,
            views::ci_sections(&[
                Item::new("a", "#92").data(json!({"ci": {"state": "passing"}})),
                Item::new("b", "#90")
                    .state(State::Attention)
                    .data(json!({"ci": {"state": "failing"}})),
            ]),
        );
        let g = groups(&ci);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].title, "Failing");
        assert_eq!(g[0].items[0].id, "b");
        assert_eq!(g[1].title, "Passing");
        assert_eq!(line(&ci), "1 failing · 1 passing");
        assert_eq!(mark(&ViewId::Ci, g[0].items[0]), ("✕", "failed"));
        let columns = crate::sources::prboard::COLUMNS;
        let prs = view(
            ViewId::Prs,
            vec![
                Item::new("late", "#2").group(columns[columns.len() - 1]),
                Item::new("early", "#1").group(columns[0]),
            ],
        );
        let g = groups(&prs);
        assert_eq!(g[0].items[0].id, "early", "in the board's column order");
        assert_eq!(g[1].items[0].id, "late");
        let empty = View {
            data: ViewData {
                empty: Some("Nothing is landing.".into()),
                ..ViewData::default()
            },
            ..view(ViewId::Landing, Vec::new())
        };
        assert_eq!(line(&empty), "Nothing is landing.");
    }
}
