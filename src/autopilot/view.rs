//! Autopilot's own view in mission control, with UX 2.0 on (the
//! view_autopilot setting): a heading saying which note it works, its tabs
//! working and open against its slots and cap, whether its tabs land
//! without asking, today's spend against its limit, and whether it is
//! paused and why; then every item it works in the column of the board
//! where it stands (`COLUMNS`: the items it will open next, in the order
//! it opens them, then its tabs working, waiting on you, landing and
//! landed, today's landings whose tabs are closed with them, then the ones
//! skipped), the least far along first, so landed ones come last of the
//! tabs. Each tab keeps a cell per stage,
//! which the board's meter counts and its side panel lists
//! (src/mission/render.rs).
//!
//! Each tab's stages come from what the server already holds, so building
//! the view reads no file and starts no process: its `@autopilot*` options,
//! its Claude's state, its loop and `@agent_phase`, the Landing view's
//! entry for its worktree (a worktree in the landing queue has passed its
//! tests, review and leak check, as queue-and-clean takes the lock after
//! them), and `@autopilot_left` once it landed; a landing whose tab is
//! closed from what the mode's file kept of it (`Mode::landed`), read on
//! every tick. A stage nothing says
//! anything of yet is a dim "waiting" cell: the tests and the review while
//! the tab works and no phase names them, the leak check until the lock is
//! asked for, and closing while the tab is open. The next items are the
//! queue the mode last read for its line (`Pilot::next`), off the server's
//! task, never read here; the plan each item links was read with them, and
//! goes on its card for the side panel to show under its stages.

use std::collections::HashMap;

use serde_json::{Value, json};

use super::line::Summary;
use super::mode::Landed;
use crate::cmd::worktree_from_note::PlanNote;
use crate::items::{Item, State, Target};
use crate::mux::TabId;
use crate::sidebar::model::TabState;

/// The heading's id: Enter on it opens the menu the Tabs view's line opens
/// without UX 2.0 (`line::choices`).
pub const HEADING: &str = "header:autopilot";

/// The stages' names, as the view's heading shows them.
pub const STAGES: [&str; 8] = [
    "opened",
    "working",
    "tests",
    "review",
    "leak check",
    "lock",
    "landed",
    "closed",
];

/// The board's columns, left to right: where an item stands.
pub const COLUMNS: [&str; 6] = [
    "Next up",
    "Working",
    "Waiting on you",
    "Landing",
    "Landed",
    "Skipped",
];

/// One of the mode's tabs as the view needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub tab: TabId,
    /// Its number in its session.
    pub index: usize,
    /// Its item's first line (`@autopilot_item`).
    pub item: String,
    pub state: TabState,
    /// Seen in its worktree (`@autopilot_in`).
    pub was_in: bool,
    /// The loop skill its Claude runs, and the step it says it is at.
    pub loop_skill: Option<String>,
    pub phase: Option<String>,
    /// What the Landing view holds of its worktree (`data.landing`).
    pub landing: Option<Value>,
    /// Landed and back in the main clone (`@autopilot_left`).
    pub left: bool,
    /// Landed without asking (`@autopilot_autoland`).
    pub autoland: bool,
    /// For a landing whose tab is closed, what the mode kept of it.
    pub closed: Option<Landed>,
    /// Its item is in the note the mode works now, whose plans were read:
    /// not so for a tab still open from a note worked before.
    pub in_note: bool,
}

/// A stage cell: its look (`done`, `running`, `waiting`, `queued`,
/// `failed` or `asks`) and its words.
pub type Cell = (&'static str, String);

fn done(s: &str) -> Cell {
    ("done", format!("✓ {s}"))
}

fn waiting() -> Cell {
    ("waiting", "waiting".into())
}

/// A tab's eight stages, as the module's comment says.
pub fn stages(r: &Row) -> [Cell; 8] {
    let mut out: [Cell; 8] = std::array::from_fn(|_| waiting());
    let phase = r.phase.as_deref().unwrap_or("");
    out[0] = if r.was_in || r.left {
        done(&format!("tab {}", r.index))
    } else {
        ("running", "opening…".into())
    };
    if r.left {
        for (at, s) in ["done", "passed", "clean", "none", "released"]
            .iter()
            .enumerate()
        {
            out[at + 1] = done(s);
        }
        out[6] = done(if r.autoland { "on its own" } else { "landed" });
        if r.closed.is_some() {
            out[7] = done("closed");
        }
        return out;
    }
    let landing = r.landing.as_ref().filter(|l| l.is_object());
    if r.was_in {
        out[1] = if landing.is_some() || r.loop_skill.as_deref() == Some("queue") {
            done("done")
        } else if r.state == TabState::Waiting {
            ("asks", "asks you".into())
        } else if r.state.busy() {
            ("running", "working…".into())
        } else {
            ("asks", "idle".into())
        };
    }
    if let Some(l) = landing {
        let holding = l.get("holding").and_then(Value::as_bool) == Some(true);
        out[2] = done("passed");
        out[3] = done("clean");
        out[4] = done("none");
        if !holding {
            let place = l.get("place").and_then(Value::as_u64).unwrap_or(1);
            out[5] = ("queued", format!("{place} in line"));
            return out;
        }
        out[5] = done("held");
        if let Some(rest) = phase.strip_prefix("tests failed") {
            out[2] = failed(rest, "failed");
        } else if phase.starts_with("check failed") {
            out[2] = failed("", "check failed");
        } else if let Some(rest) = phase.strip_prefix("review blocked") {
            out[3] = failed(rest, "blocked");
        } else if phase.contains("testing") {
            out[2] = ("running", "testing…".into());
        } else if phase.starts_with("checking") {
            out[2] = ("running", "checking…".into());
        } else if phase.starts_with("reviewing") {
            out[3] = ("running", "reviewing…".into());
        } else if phase.starts_with("pushing") {
            out[6] = ("running", "pushing…".into());
        } else if phase.starts_with("applying") || phase.starts_with("cleaning") {
            out[6] = done("pushed");
        }
        return out;
    }
    // Not in a landing queue yet: only what its loop and phase say.
    if r.loop_skill.as_deref() == Some("review") || phase.starts_with("reviewing") {
        out[3] = ("running", "reviewing…".into());
    } else if phase.contains("testing") {
        out[2] = ("running", "testing…".into());
    } else if let Some(rest) = phase.strip_prefix("tests failed") {
        out[2] = failed(rest, "failed");
    }
    out
}

fn failed(rest: &str, or: &str) -> Cell {
    let rest = rest.trim_start_matches([':', ' ']).trim();
    (
        "failed",
        format!("✕ {}", if rest.is_empty() { or } else { rest }),
    )
}

/// The column a tab stands in (an index into `COLUMNS`, never Next up):
/// landed once back in the main clone; waiting on you while its Claude
/// waits on you, a landing asking before its push included; landing once
/// its worktree is in the landing queue or its Claude runs queue-and-clean;
/// waiting on you too when its working stage asks you (idle, or its turn
/// ended); else working (opening its worktree too).
pub fn column(r: &Row, stages: &[Cell; 8]) -> usize {
    if r.left {
        4
    } else if r.state == TabState::Waiting {
        2
    } else if r.landing.as_ref().is_some_and(Value::is_object)
        || r.loop_skill.as_deref() == Some("queue")
    {
        3
    } else if stages[1].0 == "asks" {
        2
    } else {
        1
    }
}

/// How many of a tab's stages are done.
pub fn progress(stages: &[Cell; 8]) -> usize {
    stages.iter().filter(|c| c.0 == "done").count()
}

/// The dim note under a tab's item: its tab, and where it is in words.
fn note_of(r: &Row) -> String {
    if let Some(l) = &r.closed {
        return if l.hash.is_empty() {
            "landed, its tab closed".into()
        } else {
            format!("landed {}, its tab closed", l.hash)
        };
    }
    let what = if r.left {
        "landed, back in the main clone".to_string()
    } else if let Some(phase) = r.phase.as_deref().filter(|p| !p.is_empty()) {
        phase.to_string()
    } else if !r.was_in {
        "wtn is making its worktree".to_string()
    } else {
        match r.state {
            TabState::Waiting => "waiting on you".into(),
            TabState::Working | TabState::Background => match r.loop_skill.as_deref() {
                Some(skill) => format!("in its {skill} loop"),
                None => "working in its worktree".into(),
            },
            TabState::Done => "finished its turn".into(),
            TabState::Idle => "idle".into(),
        }
    };
    format!("tab {} · {what}", r.index)
}

/// Why it is paused, or nothing while it opens tabs: paused by you, or
/// while you're away; then what it last said (the spend limit passed, the
/// note lost or with nothing left).
fn why_paused(s: &Summary, said: &str) -> Vec<String> {
    let mut out = Vec::new();
    if s.paused {
        out.push("paused by you".into());
    } else if s.away {
        out.push("paused while you're away".into());
    }
    if !said.is_empty() {
        out.push(said.to_string());
    }
    out
}

/// A card's data with its item's plan: `planName`, the plan note's name,
/// and `plan`, its Markdown; the name alone when no note has it, neither
/// for an item that links no plan. A tab of a note worked before, whose
/// plans weren't read, gets `planElsewhere` instead (`elsewhere`).
fn elsewhere(mut data: Value) -> Value {
    data["planElsewhere"] = json!(true);
    data
}

fn with_plan(mut data: Value, plans: &HashMap<String, PlanNote>, item: &str) -> Value {
    if let Some(plan) = plans.get(item) {
        data["planName"] = json!(plan.name);
        if let Some(text) = &plan.text {
            data["plan"] = json!(text);
        }
    }
    data
}

/// The view's items: the heading, the next items under a heading the
/// cursor passes over, a row per tab under one too, in its column's order
/// and, within a column, the least far along first, then the skipped ones
/// under a heading, the board's last column. Every item on the board has
/// its column as its `group`, and the plan its item links (`with_plan`);
/// the headings, in none, lead the plain list a narrow screen shows.
pub fn items(
    s: &Summary,
    said: &str,
    rows: &[Row],
    next: &[String],
    skipped: &[String],
    plans: &HashMap<String, PlanNote>,
) -> Vec<Item> {
    let mut facts = Vec::new();
    // A default branch its tabs land on failing its CI (src/sources/mainci.rs).
    if !s.failing.is_empty() {
        facts.push(s.failing.clone());
    }
    // Where its tabs land, when not the repository's default branch.
    if !s.target.is_empty() {
        facts.push(format!("→ {}", s.target));
    }
    facts.extend([
        format!("{} of {} working", s.busy, s.slots),
        format!("{} of {} open", s.open, s.cap),
        if s.hands_free {
            "hands-free".to_string()
        } else if s.lands_alone {
            "lands without asking".to_string()
        } else {
            "asks before landing".to_string()
        },
    ]);
    if s.ready_only {
        facts.push("planned items only".into());
    }
    if s.spend_limit > 0.0 {
        facts.push(format!("${:.2} of ${} today", s.spent, s.spend_limit));
    }
    if s.drained {
        facts.push("nothing more to start".into());
    }
    facts.extend(why_paused(s, said));
    let pilot = json!({
        "paused": s.paused,
        "lands": s.lands_alone,
        "handsFree": s.hands_free,
        "slots": s.slots,
        "cap": s.cap,
        "spendLimit": s.spend_limit,
    });
    let spend = (s.spend_limit > 0.0).then(|| json!([s.spent, s.spend_limit]));
    let mut out = vec![
        Item::new(HEADING, s.note.clone())
            .detail(facts.join(" · "))
            .data(json!({ "header": true, "count": rows.len(), "summary": true, "spend": spend })),
    ];
    if !next.is_empty() {
        out.push(
            Item::new("header:autopilot:next", "Next, in the order it opens them")
                .data(json!({ "header": true, "count": next.len() })),
        );
        for (at, text) in next.iter().enumerate() {
            out.push(
                Item::new(format!("next:{text}"), format!("{}. {text}", at + 1))
                    .group(COLUMNS[0])
                    .data(with_plan(
                        json!({ "next": text, "place": at + 1, "mode": pilot }),
                        plans,
                        text,
                    )),
            );
        }
    }
    let mut placed: Vec<(usize, usize, &Row, [Cell; 8])> = rows
        .iter()
        .map(|r| {
            let cells = stages(r);
            (column(r, &cells), progress(&cells), r, cells)
        })
        .collect();
    placed.sort_by_key(|(column, progress, _, _)| (*column, *progress));
    if !placed.is_empty() {
        out.push(
            Item::new(
                "header:autopilot:tabs",
                "Its tabs, the least far along first",
            )
            .data(json!({ "header": true, "count": placed.len() })),
        );
    }
    for (column, progress, r, cells) in placed {
        let failed = cells.iter().any(|c| c.0 == "failed");
        let cells: Vec<Value> = cells
            .into_iter()
            .map(|(look, words)| json!([look, words]))
            .collect();
        let id = match &r.closed {
            Some(l) => format!("pilot:landed:{}", l.id),
            None => format!("pilot:{}", r.tab),
        };
        let item = Item::new(id, r.item.clone())
            .detail(note_of(r))
            .group(COLUMNS[column])
            .state(if r.state == TabState::Waiting {
                State::Attention
            } else if r.state.busy() {
                State::Active
            } else {
                State::Idle
            });
        // A closed tab's landing has no tab to go to.
        let item = match r.closed {
            Some(_) => item,
            None => item.target(Target::Tab(r.tab)),
        };
        let data = |data: Value| {
            if r.in_note {
                with_plan(data, plans, &r.item)
            } else {
                elsewhere(data)
            }
        };
        out.push(item.data(data(json!({
            "pilot": {
                "tab": r.tab.to_string(),
                "index": r.index,
                "item": r.item,
                "stages": cells,
                "done": progress,
                "failed": failed,
                "closed": r.closed.is_some(),
                "left": r.left,
            },
            "mode": pilot,
        }))));
    }
    if !skipped.is_empty() {
        out.push(
            Item::new("header:autopilot:skipped", "Skipped")
                .data(json!({ "header": true, "count": skipped.len() })),
        );
        for text in skipped {
            out.push(
                Item::new(format!("skipped:{text}"), text.clone())
                    .detail("skipped by you from autopilot's menu")
                    .group(COLUMNS[5])
                    .data(with_plan(
                        json!({ "skipped": text, "mode": pilot }),
                        plans,
                        text,
                    )),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(state: TabState) -> Row {
        Row {
            tab: TabId(4),
            index: 3,
            item: "Give notes stable IDs".into(),
            state,
            was_in: true,
            loop_skill: None,
            phase: None,
            landing: None,
            left: false,
            autoland: false,
            closed: None,
            in_note: true,
        }
    }

    #[test]
    fn a_tab_working_is_opened_and_working_with_the_rest_waiting() {
        let s = stages(&row(TabState::Working));
        assert_eq!(s[0], done("tab 3"));
        assert_eq!(s[1].0, "running");
        assert!(s[2..].iter().all(|c| c.0 == "waiting"), "{s:?}");
    }

    #[test]
    fn a_tab_holding_the_lock_has_passed_its_checks_and_one_landed_has_done_all_but_closing() {
        let mut r = row(TabState::Working);
        r.loop_skill = Some("queue".into());
        r.phase = Some("pushing".into());
        r.landing = Some(json!({ "holding": true, "phase": "pushing" }));
        let s = stages(&r);
        assert_eq!(s[1], done("done"));
        assert_eq!(s[4], done("none"));
        assert_eq!(s[5], done("held"));
        assert_eq!(s[6], ("running", "pushing…".to_string()));
        r.landing = Some(json!({ "holding": false, "place": 2 }));
        assert_eq!(stages(&r)[5], ("queued", "2 in line".to_string()));
        r.left = true;
        r.autoland = true;
        let s = stages(&r);
        assert_eq!(s[6], done("on its own"));
        assert_eq!(s[7].0, "waiting");
    }

    #[test]
    fn a_card_carries_its_items_plan_the_name_alone_when_its_note_is_missing() {
        let plans = HashMap::from([
            (
                "Give notes stable IDs".to_string(),
                PlanNote {
                    name: "Plan - Stable IDs".into(),
                    text: Some("## Today\n\nFound by name.".into()),
                },
            ),
            (
                "Next".to_string(),
                PlanNote {
                    name: "Plan - Next".into(),
                    text: None,
                },
            ),
        ]);
        let s = Summary {
            note: "Work".into(),
            busy: 1,
            open: 1,
            slots: 2,
            cap: 5,
            next: String::new(),
            paused: false,
            away: false,
            ready_only: false,
            skipped: 1,
            spend_limit: 0.0,
            spent: 0.0,
            lands_alone: false,
            hands_free: false,
            target: String::new(),
            drained: false,
            failing: String::new(),
        };
        let mut other = row(TabState::Working);
        other.tab = TabId(5);
        other.in_note = false;
        let items = items(
            &s,
            "",
            &[row(TabState::Working), other],
            &["Next".into()],
            &["Old".into()],
            &plans,
        );
        let of = |id: &str| &items.iter().find(|i| i.id == id).unwrap().data;
        // A tab left from a note worked before has none of this note's
        // plans, though its item's first line is the same.
        let left = of("pilot:@5");
        assert!(left.get("planName").is_none(), "{left}");
        assert_eq!(left["planElsewhere"], true);
        let tab = of("pilot:@4");
        assert_eq!(tab["planName"], "Plan - Stable IDs");
        assert_eq!(tab["plan"], "## Today\n\nFound by name.");
        let next = of("next:Next");
        assert_eq!(next["planName"], "Plan - Next");
        assert!(next.get("plan").is_none(), "{next}");
        let skipped = of("skipped:Old");
        assert!(
            skipped.get("planName").is_none() && skipped.get("plan").is_none(),
            "{skipped}"
        );
        assert!(of(HEADING).get("planName").is_none());
    }

    #[test]
    fn the_heading_says_slots_cap_landing_and_why_it_is_paused() {
        let s = Summary {
            note: "Work".into(),
            busy: 2,
            open: 3,
            slots: 2,
            cap: 5,
            next: String::new(),
            paused: true,
            away: false,
            ready_only: false,
            skipped: 1,
            spend_limit: 0.0,
            spent: 0.0,
            lands_alone: true,
            hands_free: false,
            target: "team/next".into(),
            drained: false,
            failing: "main failing at ebde129".into(),
        };
        let items = items(
            &s,
            "",
            &[row(TabState::Waiting)],
            &["Next".into()],
            &["Old".into()],
            &HashMap::new(),
        );
        assert_eq!(items[0].title, "Work");
        assert_eq!(
            items[0].detail,
            "main failing at ebde129 · → team/next · 2 of 2 working · 3 of 5 open · lands without asking · paused by you"
        );
        assert_eq!(items[0].id, HEADING);
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "header:autopilot",
                "header:autopilot:next",
                "next:Next",
                "header:autopilot:tabs",
                "pilot:@4",
                "header:autopilot:skipped",
                "skipped:Old"
            ]
        );
        let skipped = items.last().unwrap();
        assert_eq!(
            skipped.group.as_deref(),
            Some("Skipped"),
            "the board's last column"
        );
        assert_eq!(
            items[5].group, None,
            "its heading leads only the plain list"
        );
    }

    #[test]
    fn each_tab_stands_in_its_column_the_least_far_along_first_and_landed_last() {
        let tab = |n: u32, state: TabState| Row {
            tab: TabId(n),
            index: n as usize,
            item: format!("item {n}"),
            ..row(state)
        };
        let landed = Row {
            left: true,
            ..tab(1, TabState::Idle)
        };
        let mut holding = tab(2, TabState::Working);
        holding.loop_skill = Some("queue".into());
        holding.landing = Some(json!({ "holding": true, "phase": "pushing" }));
        let mut queued = tab(3, TabState::Working);
        queued.landing = Some(json!({ "holding": false, "place": 2 }));
        let asks = tab(4, TabState::Waiting);
        let mut asks_to_push = tab(7, TabState::Waiting);
        asks_to_push.loop_skill = Some("queue".into());
        let working = tab(5, TabState::Working);
        let opening = Row {
            was_in: false,
            ..tab(6, TabState::Working)
        };
        let s = Summary {
            note: "Work".into(),
            busy: 0,
            open: 0,
            slots: 2,
            cap: 5,
            next: String::new(),
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
        let items = items(
            &s,
            "",
            &[
                landed,
                holding,
                queued,
                asks,
                working,
                opening,
                asks_to_push,
            ],
            &["Later".into()],
            &[],
            &HashMap::new(),
        );
        let placed: Vec<(&str, &str)> = items
            .iter()
            .filter_map(|i| Some((i.group.as_deref()?, i.title.as_str())))
            .collect();
        assert_eq!(
            placed,
            [
                ("Next up", "1. Later"),
                ("Working", "item 6"),
                ("Working", "item 5"),
                ("Waiting on you", "item 4"),
                ("Waiting on you", "item 7"),
                ("Landing", "item 3"),
                ("Landing", "item 2"),
                ("Landed", "item 1"),
            ]
        );
        let landed = items.iter().find(|i| i.title == "item 1").unwrap();
        assert_eq!(landed.data["pilot"]["done"], 7);
        assert_eq!(items[0].data["spend"], Value::Null, "no limit, no spend");
    }

    #[test]
    fn a_landing_stands_under_landed_its_tab_open_or_closed() {
        let open = Row {
            left: true,
            ..row(TabState::Idle)
        };
        let closed = Row {
            tab: TabId(9),
            index: 0,
            item: "Copy the reply".into(),
            closed: Some(Landed {
                id: "9@1700000000".into(),
                item: "Copy the reply".into(),
                note: "n-1".into(),
                hash: "abc1234".into(),
                at: 1,
            }),
            ..open.clone()
        };
        let s = stages(&closed);
        assert_eq!(s[7], done("closed"));
        assert!(s.iter().all(|c| c.0 == "done"), "{s:?}");
        assert_eq!(
            stages(&open)[7].0,
            "waiting",
            "closing while its tab is open"
        );
        let summary = Summary {
            note: "Work".into(),
            busy: 0,
            open: 1,
            slots: 2,
            cap: 5,
            next: String::new(),
            paused: false,
            away: false,
            ready_only: false,
            skipped: 0,
            spend_limit: 0.0,
            spent: 0.0,
            lands_alone: false,
            hands_free: false,
            target: String::new(),
            drained: false,
            failing: String::new(),
        };
        let items = items(&summary, "", &[open, closed], &[], &[], &HashMap::new());
        let landed: Vec<&Item> = items
            .iter()
            .filter(|i| i.group.as_deref() == Some("Landed"))
            .collect();
        assert_eq!(landed.len(), 2, "{items:#?}");
        let gone = landed
            .iter()
            .find(|i| i.id == "pilot:landed:9@1700000000")
            .unwrap();
        assert_eq!(gone.detail, "landed abc1234, its tab closed");
        assert_eq!(gone.target, Target::None, "no tab to go to");
        assert_eq!(gone.data["pilot"]["closed"], true);
        let here = landed.iter().find(|i| i.id == "pilot:@4").unwrap();
        assert_eq!(here.target, Target::Tab(TabId(4)));
    }

    #[test]
    fn the_heading_says_everything_the_tabs_views_line_said() {
        let s = Summary {
            note: "Work".into(),
            busy: 1,
            open: 1,
            slots: 2,
            cap: 4,
            next: String::new(),
            paused: false,
            away: true,
            ready_only: true,
            skipped: 0,
            spend_limit: 20.0,
            spent: 4.2,
            lands_alone: false,
            hands_free: false,
            target: String::new(),
            drained: true,
            failing: String::new(),
        };
        let items = items(
            &s,
            "Nothing left in the note",
            &[],
            &[],
            &[],
            &HashMap::new(),
        );
        assert_eq!(
            items[0].detail,
            "1 of 2 working · 1 of 4 open · asks before landing · planned items only · $4.20 of $20 today · nothing more to start · paused while you're away · Nothing left in the note"
        );
    }
}
