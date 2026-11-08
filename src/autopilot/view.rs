//! Autopilot's own view in mission control, with UX 2.0 on (the
//! view_autopilot setting): a heading saying which note it works, its tabs
//! working and open against its slots and cap, whether its tabs land
//! without asking, and whether it is paused and why; then a row per tab it
//! opened, its item's first line and a cell per stage, as the Landing
//! view's rows are drawn (src/mission/render.rs); then the items it will
//! open next, in the order it opens them, and the ones skipped.
//!
//! Each tab's stages come from what the server already holds, so building
//! the view reads no file and starts no process: its `@autopilot*` options,
//! its Claude's state, its loop and `@agent_phase`, the Landing view's
//! entry for its worktree (a worktree in the landing queue has passed its
//! tests, review and leak check, as queue-and-clean takes the lock after
//! them), and `@autopilot_left` once it landed. A stage nothing says
//! anything of yet is a dim "waiting" cell: the tests and the review while
//! the tab works and no phase names them, the leak check until the lock is
//! asked for, and closing while the tab is open. The next items are the
//! queue the mode last read for its line (`Pilot::next`), off the server's
//! task, never read here.

use serde_json::{Value, json};

use super::line::Summary;
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
        } else if let Some(rest) = phase.strip_prefix("review blocked") {
            out[3] = failed(rest, "blocked");
        } else if phase.contains("testing") {
            out[2] = ("running", "testing…".into());
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

/// The dim note under a tab's item: its tab, and where it is in words.
fn note_of(r: &Row) -> String {
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

/// The view's items: the heading, a row per tab, the next items and the
/// skipped ones, each section under a heading the cursor passes over.
pub fn items(
    s: &Summary,
    said: &str,
    rows: &[Row],
    next: &[String],
    skipped: &[String],
) -> Vec<Item> {
    let mut facts = Vec::new();
    // Where its tabs land, when not the repository's default branch.
    if !s.target.is_empty() {
        facts.push(format!("→ {}", s.target));
    }
    facts.extend([
        format!("{} of {} working", s.busy, s.slots),
        format!("{} of {} open", s.open, s.cap),
        if s.lands_alone {
            "lands without asking".to_string()
        } else {
            "asks before landing".to_string()
        },
    ]);
    if s.ready_only {
        facts.push("planned items only".into());
    }
    if s.spend_limit > 0.0 {
        facts.push(format!("stops at ${} today", s.spend_limit));
    }
    facts.extend(why_paused(s, said));
    let pilot = json!({ "paused": s.paused, "lands": s.lands_alone });
    let mut out = vec![
        Item::new(HEADING, s.note.clone())
            .detail(facts.join(" · "))
            .data(json!({ "header": true, "count": rows.len(), "summary": true })),
    ];
    for r in rows {
        let cells: Vec<Value> = stages(r)
            .into_iter()
            .map(|(look, words)| json!([look, words]))
            .collect();
        out.push(
            Item::new(format!("pilot:{}", r.tab), r.item.clone())
                .detail(note_of(r))
                .state(if r.state == TabState::Waiting {
                    State::Attention
                } else if r.state.busy() {
                    State::Active
                } else {
                    State::Idle
                })
                .target(Target::Tab(r.tab))
                .data(json!({
                    "pilot": { "tab": r.tab.to_string(), "item": r.item, "stages": cells },
                    "mode": pilot,
                })),
        );
    }
    if !next.is_empty() {
        out.push(
            Item::new("header:autopilot:next", "Next, in the order it opens them")
                .data(json!({ "header": true, "count": next.len() })),
        );
        for (at, text) in next.iter().enumerate() {
            out.push(
                Item::new(format!("next:{text}"), format!("{}. {text}", at + 1))
                    .data(json!({ "next": text, "mode": pilot })),
            );
        }
    }
    if !skipped.is_empty() {
        out.push(
            Item::new("header:autopilot:skipped", "Skipped")
                .data(json!({ "header": true, "count": skipped.len() })),
        );
        for text in skipped {
            out.push(
                Item::new(format!("skipped:{text}"), text.clone())
                    .detail("skipped by you from its menu")
                    .data(json!({ "skipped": text, "mode": pilot })),
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
            lands_alone: true,
            target: "team/next".into(),
        };
        let items = items(
            &s,
            "",
            &[row(TabState::Waiting)],
            &["Next".into()],
            &["Old".into()],
        );
        assert_eq!(items[0].title, "Work");
        assert_eq!(
            items[0].detail,
            "→ team/next · 2 of 2 working · 3 of 5 open · lands without asking · paused by you"
        );
        assert_eq!(items[0].id, HEADING);
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "header:autopilot",
                "pilot:@4",
                "header:autopilot:next",
                "next:Next",
                "header:autopilot:skipped",
                "skipped:Old"
            ]
        );
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
            lands_alone: false,
            target: String::new(),
        };
        let items = items(&s, "Nothing left in the note", &[], &[], &[]);
        assert_eq!(
            items[0].detail,
            "1 of 2 working · 1 of 4 open · asks before landing · planned items only · stops at $20 today · paused while you're away · Nothing left in the note"
        );
    }
}
