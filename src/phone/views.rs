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
use crate::usage::charts::{self, Limit};
use crate::usage::sessions::{self, LIMIT_FULL};

/// The views the phone lists, in its order, each while mission control
/// would list it for a client with no view kept in front.
pub const SHOWN: [ViewId; 6] = [
    ViewId::Landing,
    ViewId::Autopilot,
    ViewId::Prs,
    ViewId::Ci,
    ViewId::Notifications,
    ViewId::Usage,
];

/// How many of today's tabs by spend the Usage view lists.
pub const TOP_TABS: usize = 5;

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

/// Whether a limit needs you: as full as mission control marks it
/// (`LIMIT_FULL`), or filling at a pace that runs past it before it resets.
fn limit_needs(limit: &Limit) -> bool {
    limit.used >= LIMIT_FULL || limit.pace.is_some_and(|p| p > 100.0)
}

/// A limit's tone as the computer's board colors its meter, its pace
/// running over counting as full.
fn limit_tone(limit: &Limit) -> &'static str {
    if limit.used >= 100.0 {
        "failed"
    } else if limit_needs(limit) {
        "needs"
    } else {
        "passed"
    }
}

/// The Usage view as the phone's stream carries it, from the board's own
/// model (`charts::Model`, carried by the view's first row) at `now`
/// (seconds): its line, the fullest limit and today's spend; what needs
/// you, each limit full or filling past it; each limit's percent, reset
/// and pace; today's and the week's spend; and today's top tabs by spend,
/// each with the Claude pane `pane_of` gives an open tab, for its channel.
/// It has no groups and no options: nothing on it acts.
pub fn usage_json(view: &View, now: i64, pane_of: &dyn Fn(TabId) -> Option<String>) -> Value {
    let model = view
        .loaded
        .then(|| charts::model_of(&view.data.items))
        .flatten();
    let needs = model
        .as_ref()
        .map_or(0, |m| m.limits.iter().filter(|l| limit_needs(l)).count());
    let line = match &model {
        None if !view.loaded => "Loading…".to_string(),
        None => line(view),
        Some(m) => {
            let spent = format!("${:.2} today", m.today.total);
            match m.limits.iter().max_by(|a, b| a.used.total_cmp(&b.used)) {
                Some(l) => format!("{} {}% · {spent}", l.name, l.used.round() as i64),
                None => spent,
            }
        }
    };
    let usage = model.as_ref().map(|m| {
        let limits: Vec<Value> = m
            .limits
            .iter()
            .map(|l| {
                json!({
                    "key": l.key,
                    "name": l.name,
                    "used": l.used.round() as i64,
                    "resets": sessions::resets(l.resets_at, now),
                    "pace": l.pace.map(|p| p.round() as i64),
                    "over": l.pace.is_some_and(|p| p > 100.0),
                    "tone": limit_tone(l),
                })
            })
            .collect();
        let tabs: Vec<Value> = m
            .today
            .tabs
            .iter()
            .filter(|t| t.spent > 0.0)
            .take(TOP_TABS)
            .map(|t| {
                json!({
                    "index": (t.index > 0).then_some(t.index),
                    "name": t.name,
                    "spent": t.spent,
                    "pane": t.tab.and_then(|id| pane_of(TabId(id))),
                })
            })
            .collect();
        json!({
            "limits": limits,
            "today": m.today.total,
            "week": m.week.total,
            "tabs": tabs,
        })
    });
    json!({
        "key": view.key(),
        "title": views::bar_title(view),
        "line": line,
        "needs": needs,
        "mark": if needs > 0 { "●" } else { "○" },
        "tone": if needs > 0 { "needs" } else { "idle" },
        "groups": [],
        "acts": [],
        "usage": usage,
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
    fn usage_view(limits: Vec<Limit>) -> View {
        use crate::usage::charts::{Model, SpanSpend, TabSpend};
        let tab = |tab: Option<u32>, index: usize, name: &str, spent: f64| TabSpend {
            key: format!("k{index}"),
            tab,
            index,
            name: name.into(),
            spent,
            ..TabSpend::default()
        };
        let model = Model {
            today: SpanSpend {
                total: 14.2,
                tabs: vec![
                    tab(Some(7), 3, "Checkout", 6.0),
                    tab(None, 9, "Closed one", 3.0),
                    tab(None, 0, "sessions outside a tab", 2.0),
                    tab(Some(8), 4, "Four", 1.5),
                    tab(Some(9), 5, "Five", 1.0),
                    tab(Some(10), 6, "Six", 0.7),
                ],
                ..SpanSpend::default()
            },
            week: SpanSpend {
                total: 80.5,
                ..SpanSpend::default()
            },
            limits,
            ..Model::default()
        };
        let items = charts::items(vec![Item::new("spent", "$14.20 today")], &model);
        View {
            loaded: true,
            ..view(ViewId::Usage, items)
        }
    }

    fn limit(key: &str, used: f64, pace: Option<f64>) -> Limit {
        Limit {
            key: key.into(),
            name: sessions::limit_name(key),
            used,
            resets_at: 1_000 + 7_800,
            pace,
            detail: None,
        }
    }

    #[test]
    fn the_usage_view_carries_the_limits_spends_and_top_tabs() {
        let pane = |t: TabId| (t == TabId(7)).then(|| "%21".to_string());
        let v = usage_view(vec![
            limit("five_hour", 62.4, Some(80.0)),
            limit("seven_day", 30.0, None),
        ]);
        let u = usage_json(&v, 1_000, &pane);
        assert_eq!(u["key"], "usage");
        assert_eq!(u["line"], "5-hour 62% · $14.20 today");
        assert_eq!(u["needs"], 0, "no count while no limit is near full");
        assert_eq!(u["groups"], json!([]));
        assert_eq!(u["acts"], json!([]), "nothing on it acts");
        let d = &u["usage"];
        assert_eq!(d["today"], 14.2);
        assert_eq!(d["week"], 80.5);
        assert_eq!(d["limits"][0]["name"], "5-hour");
        assert_eq!(d["limits"][0]["used"], 62);
        assert_eq!(d["limits"][0]["resets"], "resets in 2h 10m");
        assert_eq!(d["limits"][0]["pace"], 80);
        assert_eq!(d["limits"][0]["over"], false);
        assert_eq!(d["limits"][0]["tone"], "passed");
        let tabs = d["tabs"].as_array().unwrap();
        assert_eq!(tabs.len(), TOP_TABS);
        assert_eq!(tabs[0]["index"], 3);
        assert_eq!(tabs[0]["spent"], 6.0);
        assert_eq!(tabs[0]["pane"], "%21", "an open tab's channel");
        assert_eq!(tabs[1]["index"], 9);
        assert_eq!(tabs[1]["pane"], Value::Null, "a closed tab has none");
        assert_eq!(tabs[2]["index"], Value::Null, "outside a tab: no number");
    }

    #[test]
    fn a_full_limit_or_one_paced_past_it_needs_you() {
        let none = |_| None;
        let v = usage_view(vec![
            limit("five_hour", 92.0, None),
            limit("seven_day", 40.0, Some(130.0)),
            limit("spend_limit", 10.0, None),
        ]);
        let u = usage_json(&v, 1_000, &none);
        assert_eq!(u["needs"], 2);
        assert_eq!(u["tone"], "needs");
        assert_eq!(u["line"], "5-hour 92% · $14.20 today", "the fullest");
        let limits = &u["usage"]["limits"];
        assert_eq!(limits[0]["tone"], "needs");
        assert_eq!(limits[1]["over"], true);
        assert_eq!(limits[1]["tone"], "needs");
        assert_eq!(limits[2]["tone"], "passed");
        let loading = View {
            loaded: false,
            ..view(ViewId::Usage, Vec::new())
        };
        let u = usage_json(&loading, 1_000, &none);
        assert_eq!(u["line"], "Loading…");
        assert_eq!(u["usage"], Value::Null);
    }
}
