//! Autopilot's own view in mission control, with UX 2.0 on (the
//! view_autopilot setting): a heading saying which note it works, its tabs
//! working and open against its slots and cap, whether its tabs land
//! without asking, today's spend against its limit, and whether it is
//! paused and why; then every item it works in the column of the board
//! where it stands (`COLUMNS`: the items it will open next, in the order
//! it opens them, an issue's card naming its priority (`say_priorities`),
//! then those blocked, each held by an `After:` line on an open item its
//! card names, or an issue by an open issue blocking it, a column only
//! while any is (board 113), then
//! its tabs working, waiting on you, landing and
//! landed, today's landings whose tabs are closed with them, then the ones
//! skipped, a column only while any is), the least far along first, so
//! landed ones come last of the tabs. Each tab keeps a cell per stage,
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
//! asked for ("not run" outside Switchboard's clone, where none runs,
//! src/dev.rs), and closing while the tab is open. The next items are the
//! queue the mode last read for its line (`Pilot::next`), off the server's
//! task, never read here; the plan each item links was read with them, and
//! goes on its card for the side panel to show under its stages. Each row
//! carries when its tab opened and when it landed (`times_of_tab`,
//! `times_of_closed`), for the card's timer, which stops at the landing.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};

use super::eta;
use super::line::Summary;
use super::mode::Landed;
use super::pilot::{Held, Stands};
use super::serve::{LANDED_AT, LEFT, OPENED};
use crate::cmd::worktree_from_note::{PlanNote, WAITS_ON_SHOWN};
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
pub const COLUMNS: [&str; 7] = [
    "Next up",
    "Blocked",
    "Working",
    "Waiting on you",
    "Landing",
    "Landed",
    "Skipped",
];

/// Each column's place in `COLUMNS`.
pub const NEXT: usize = 0;
pub const BLOCKED: usize = 1;
pub const WORKING: usize = 2;
pub const WAITING: usize = 3;
pub const LANDING: usize = 4;
pub const LANDED: usize = 5;
pub const SKIPPED: usize = 6;

/// The columns the board draws: every one of `COLUMNS` but Blocked and
/// Skipped while no item stands in them, so an empty one takes none of
/// the width and comes back with the first item blocked or skipped.
pub fn board_columns(items: &[Item]) -> Vec<&'static str> {
    COLUMNS
        .into_iter()
        .enumerate()
        .filter(|(at, column)| {
            ![BLOCKED, SKIPPED].contains(at)
                || items.iter().any(|i| i.group.as_deref() == Some(*column))
        })
        .map(|(_, column)| column)
        .collect()
}

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
    /// When it opened, in seconds (`@autopilot`, or a closed landing's id).
    pub started: Option<i64>,
    /// When it landed, in seconds (`@autopilot_landed_at`, else
    /// `@autopilot_left`, or a closed landing's `at`): none while it works.
    pub ended: Option<i64>,
    /// Each step its Claude's loop said it was at, with when it began
    /// (`PaneAgent::phases`), for each stage's time; none for a closed tab.
    pub phases: Vec<(String, i64)>,
    /// Now, in seconds, for the time of a stage still running.
    pub now: i64,
}

/// An option's time in seconds, none when unset or zero.
fn secs(v: Option<&String>) -> Option<i64> {
    v.and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|s| *s != 0.0)
        .map(|s| s as i64)
}

/// An open tab's `started` and `ended`, from its options.
pub fn times_of_tab(opts: &BTreeMap<String, String>) -> (Option<i64>, Option<i64>) {
    let ended = secs(opts.get(LANDED_AT)).or_else(|| secs(opts.get(LEFT)));
    (secs(opts.get(OPENED)), ended)
}

/// A closed landing's `started` and `ended`: its id, `<tab>@<opened>`, and
/// when it landed.
pub fn times_of_closed(l: &Landed) -> (Option<i64>, Option<i64>) {
    let opened = l.id.split_once('@').map(|(_, at)| at.to_string());
    let ended = (l.at != 0).then(|| l.at.div_euclid(1000));
    (secs(opened.as_ref()), ended)
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

/// A tab's eight stages, as the module's comment says, each done or
/// running one with how long it took or has taken so far (`timed`), with
/// the tab_age setting on.
pub fn stages(r: &Row) -> [Cell; 8] {
    let cells = cells(r);
    if crate::settings::is_on("tab_age") {
        timed(cells, r)
    } else {
        cells
    }
}

/// How long the phases matching `which` took, together: each from when
/// it began to when the next began, the last to when the tab landed, else
/// now, none past the landing; none when no phase matches.
fn phase_time(r: &Row, which: impl Fn(&str) -> bool) -> Option<i64> {
    let end = r.ended.unwrap_or(r.now);
    let mut total = None;
    for (at, (phase, began)) in r.phases.iter().enumerate() {
        if phase.is_empty() || !which(phase) {
            continue;
        }
        // A step after the landing (cleaning up) doesn't stretch one before it.
        let until = r
            .phases
            .get(at + 1)
            .map_or(end, |(_, next)| (*next).min(end));
        *total.get_or_insert(0) += (until - began).max(0);
    }
    total
}

/// The stages' cells with their times: the work from the tab's opening to
/// its first phase (now, while there is none), the tests (`testing`,
/// `testing in CI`, `testing again`, `checking after the rebase`), the
/// review (`reviewing`, `reviewing again`), the wait for the landing lock
/// and the push, each as its phases said; a failed or waiting cell keeps
/// its words alone.
fn timed(mut cells: [Cell; 8], r: &Row) -> [Cell; 8] {
    let work = r.started.and_then(|started| {
        let until = r
            .phases
            .first()
            .map(|(_, at)| *at)
            .or(r.ended)
            .unwrap_or(r.now);
        // Done with no phase to end it at, the work's end isn't known.
        (cells[1].0 == "running" || !r.phases.is_empty()).then_some(until - started)
    });
    let times = [
        (1, work),
        (
            2,
            phase_time(r, |p| p.contains("testing") || p.starts_with("checking")),
        ),
        (3, phase_time(r, |p| p.starts_with("reviewing"))),
        (5, phase_time(r, |p| p == "waiting for the landing lock")),
        (6, phase_time(r, |p| p.starts_with("pushing"))),
    ];
    for (at, time) in times {
        let (look, words) = &mut cells[at];
        if let Some(time) = time
            && matches!(*look, "done" | "running" | "queued")
        {
            words.push(' ');
            words.push_str(&crate::sidebar::text::span(time));
        }
    }
    cells
}

/// A tab's eight stages' cells, without their times.
fn cells(r: &Row) -> [Cell; 8] {
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
        out[4] = if l["record"]["leak"]["skipped"] == true {
            ("waiting", "not run".into())
        } else {
            done("none")
        };
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
    } else if phase.starts_with("asking") {
        // Asking before the push: the tests and the review it ran are
        // behind it; one the repository skips stays waiting.
        let ran = |which: fn(&str) -> bool| r.phases.iter().any(|(p, _)| which(p));
        if ran(|p| p.contains("testing")) {
            out[2] = done("passed");
        }
        if ran(|p| p.starts_with("reviewing")) {
            out[3] = done("clean");
        }
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

/// The column a tab stands in (an index into `COLUMNS`, never Next up or
/// Blocked):
/// landed once back in the main clone; waiting on you while its Claude
/// waits on you, a landing asking before its push included; landing once
/// its worktree is in the landing queue or its Claude runs queue-and-clean;
/// waiting on you too when its working stage asks you (idle, or its turn
/// ended); else working (opening its worktree too).
pub fn column(r: &Row, stages: &[Cell; 8]) -> usize {
    if r.left {
        LANDED
    } else if r.state == TabState::Waiting {
        WAITING
    } else if r.landing.as_ref().is_some_and(Value::is_object)
        || r.loop_skill.as_deref() == Some("queue")
    {
        LANDING
    } else if stages[1].0 == "asks" {
        WAITING
    } else {
        WORKING
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
        "wt is making its worktree".to_string()
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
        if let Some(file) = &plan.file {
            data["planFile"] = json!(file);
        }
        if let Some(text) = &plan.text {
            data["plan"] = json!(text);
        }
    }
    data
}

/// When the note's ready items should all be done, for the heading
/// (`eta::words`): today's landings give the typical time, those the mode
/// kept (`kept`, of any note, its tabs open or closed) and its open tabs
/// landed since, each once; of its rows, those still open and not landed are at work, but for those the board
/// puts under Waiting on you, which free their slot and are counted with
/// what waits on you, as are the items held after them; `left` is what
/// of the note it will still take, and holds on you. Those waiting on you,
/// and those landed and not yet closed, still count toward `cap`, the most tabs open, so the ready items have
/// the slots it leaves, and no time while it leaves none. No estimate while it is paused
/// or holds off (while you are away, or past the spend limit), since it
/// opens nothing then.
pub fn eta(
    rows: &[Row],
    kept: &[Landed],
    slots: u32,
    cap: u32,
    paused: bool,
    left: &eta::Left,
    now: &jiff::Zoned,
) -> String {
    let secs = now.timestamp().as_second();
    // Each landing by its tab and opening, as the mode keeps its id.
    let mut by_id: HashMap<(u32, i64), i64> = HashMap::new();
    for l in kept {
        let tab = l.id.split_once('@').and_then(|(t, _)| t.parse().ok());
        if let (Some(tab), (Some(started), Some(ended))) = (tab, times_of_closed(l)) {
            by_id.insert((tab, started), ended);
        }
    }
    for r in rows.iter().filter(|r| r.closed.is_none()) {
        if let (Some(started), Some(ended)) = (r.started, r.ended) {
            by_id.entry((r.tab.0, started)).or_insert(ended);
        }
    }
    let mut landed: Vec<(i64, i64)> = by_id
        .into_iter()
        .map(|((_, started), ended)| (ended, ended - started))
        .filter(|(ended, _)| crate::autopilot::mode::same_day(ended * 1000, secs * 1000))
        .collect();
    landed.sort_unstable();
    let took: Vec<i64> = landed.into_iter().map(|(_, took)| took).collect();
    // Its tabs open and not yet back in a main clone, each counting toward
    // the most open: those under Waiting on you, those landed and not yet
    // closed, and the rest at work.
    let open: Vec<&Row> = rows
        .iter()
        .filter(|r| r.closed.is_none() && !r.left)
        .collect();
    let (asking, others): (Vec<&Row>, Vec<&Row>) =
        open.iter().partition(|r| column(r, &stages(r)) == WAITING);
    let at_work: Vec<&Row> = others.into_iter().filter(|r| r.ended.is_none()).collect();
    let items: Vec<String> = at_work.iter().map(|r| r.item.clone()).collect();
    let (ready, on_you) = left.counts(&items);
    let on_you = on_you + asking.len();
    let starts: Vec<i64> = at_work.iter().map(|r| r.started.unwrap_or(secs)).collect();
    let free = slots.min(cap.saturating_sub((open.len() - at_work.len()) as u32));
    let end = (!paused && (free > 0 || ready == 0))
        .then(|| eta::estimate(secs, free, &starts, ready, eta::typical(&took)))
        .flatten();
    eta::words(end, starts.len() + ready, on_you, now)
}

/// The view's items: the heading, the next items under a heading the
/// cursor passes over, those held after them (one held by an `After:` line
/// on an open item in the Blocked column, its data's `after` saying where
/// each item it waits on stands, `blockers`; one held by a design board's
/// review dim in Next up), a row per tab under one too, in its column's order
/// and, within a column, the least far along first, then the skipped ones
/// under a heading, the board's last column. Every item on the board has
/// its column as its `group`, and the plan its item links (`with_plan`);
/// the headings, in none, lead the plain list a narrow screen shows.
#[allow(clippy::too_many_arguments)]
pub fn items(
    s: &Summary,
    eta: &str,
    said: &str,
    rows: &[Row],
    next: &[String],
    held: &[Held],
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
    ]);
    // When the note's ready items should be done (src/autopilot/eta.rs).
    if !eta.is_empty() {
        facts.push(eta.to_string());
    }
    facts.push(if s.hands_free {
        "hands-free".to_string()
    } else if s.lands_alone {
        "lands without asking".to_string()
    } else {
        "asks before landing".to_string()
    });
    if s.ready_only {
        facts.push("planned items only".into());
    }
    if s.spend_limit > 0.0 {
        facts.push(format!("${:.2} of ${} today", s.spent, s.spend_limit));
    }
    if s.weekly_limit > 0 {
        facts.push(match s.weekly {
            Some((used, _)) => format!("{}% of {}% this week", used.round() as i64, s.weekly_limit),
            None => format!("stops at {}% of the week", s.weekly_limit),
        });
    }
    if s.drained {
        facts.push("nothing more to start".into());
    }
    facts.extend(why_paused(s, said));
    let spend = (s.spend_limit > 0.0).then(|| json!([s.spent, s.spend_limit]));
    let mut out = vec![
        Item::new(HEADING, s.note.clone())
            .detail(facts.join(" · "))
            .data(json!({ "header": true, "count": rows.len(), "summary": true, "spend": spend })),
    ];
    if !next.is_empty() || !held.is_empty() {
        out.push(
            Item::new("header:autopilot:next", "Next, in the order it opens them")
                .data(json!({ "header": true, "count": next.len() + held.len() })),
        );
        for (at, text) in next.iter().enumerate() {
            out.push(
                Item::new(format!("next:{text}"), format!("{}. {text}", at + 1))
                    .group(COLUMNS[NEXT])
                    .data(with_plan(
                        json!({ "next": text, "place": at + 1 }),
                        plans,
                        text,
                    )),
            );
        }
        // After the free ones: those held by the open items each waits
        // on, which the queue takes once they are ticked, in the Blocked
        // column; one held by a board's review stays in Next up, dim.
        for Held {
            text,
            waits_on,
            after,
        } in held
        {
            let more = match after.len() {
                0 | 1 => String::new(),
                n => format!(" and {} more", n - 1),
            };
            let waits = format!(
                "waits on {}{more}",
                crate::ui::draw::cut(waits_on, WAITS_ON_SHOWN)
            );
            let mut data = json!({ "next": text, "waitsOn": waits_on });
            if !after.is_empty() {
                data["after"] = blockers(after, rows, plans);
            }
            out.push(
                Item::new(format!("held:{text}"), text.clone())
                    .detail(waits)
                    .group(COLUMNS[if after.is_empty() { NEXT } else { BLOCKED }])
                    .data(with_plan(data, plans, text)),
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
                "started": r.started,
                "ended": r.ended,
            },
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
                    .group(COLUMNS[SKIPPED])
                    .data(with_plan(json!({ "skipped": text }), plans, text)),
            );
        }
    }
    out
}

/// Where each open item a blocked item waits on stands, for its card, the
/// pane beside the board and its menu: its first line (`item`); the tab
/// working it, when one of the mode's is open on it in this note, never a
/// tab left from a note worked before whose item reads the same (`tab`, its number
/// `index`, and `column`, where that tab stands); else its place in line
/// (`place`, in the whole queue) when the queue will take it, or `column`
/// Blocked alone when it is blocked itself; and its plan note's file
/// (`plan`), for the menu to open while no tab works it.
fn blockers(after: &[(String, Stands)], rows: &[Row], plans: &HashMap<String, PlanNote>) -> Value {
    after
        .iter()
        .map(|(item, stands)| {
            let mut on = json!({ "item": item });
            let tab = rows
                .iter()
                .find(|r| r.in_note && r.closed.is_none() && !r.left && r.item == *item);
            if let Some(r) = tab {
                on["tab"] = json!(r.tab.to_string());
                on["index"] = json!(r.index);
                on["column"] = json!(COLUMNS[column(r, &stages(r))]);
            } else {
                match stands {
                    Stands::Queued(place) => on["place"] = json!(place),
                    Stands::Blocked => on["column"] = json!(COLUMNS[BLOCKED]),
                    Stands::Open => {}
                }
            }
            if let Some(file) = plans.get(item).and_then(|p| p.file.as_deref()) {
                on["plan"] = json!(file);
            }
            on
        })
        .collect()
}

/// Each issue in Next up or Blocked carries its priority (`priorities`, by
/// its first line) as its data's `priority`, for its card and its menu's
/// "Put next", "Move up" and "Move down".
pub fn say_priorities(items: &mut [Item], priorities: &HashMap<String, u8>) {
    for item in items {
        let text = item.data["next"].as_str().unwrap_or_default();
        if let Some(p) = priorities.get(text) {
            item.data["priority"] = json!(p);
        }
    }
}

/// Each item in Next up whose last start failed (`failed`, by its first
/// line, why) says so, "start failed: <why>": as its detail for the plain
/// list, and as its data's `startFailed` for its card and the pane beside
/// the board.
pub fn say_failed_starts(items: &mut [Item], failed: &HashMap<String, String>) {
    for item in items.iter_mut() {
        let text = item
            .id
            .strip_prefix("next:")
            .or_else(|| item.id.strip_prefix("held:"));
        if let Some(why) = text.and_then(|t| failed.get(t)) {
            item.detail = format!("start failed: {why}");
            item.data["startFailed"] = json!(why);
        }
    }
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
            started: None,
            ended: None,
            phases: Vec::new(),
            now: 0,
        }
    }

    #[test]
    fn an_item_whose_start_failed_says_so_on_its_card() {
        let mut list = vec![
            Item::new("next:One", "1. One"),
            Item::new("next:Two", "2. Two"),
        ];
        let failed = HashMap::from([(
            "Two".to_string(),
            "couldn't fetch origin's main".to_string(),
        )]);
        say_failed_starts(&mut list, &failed);
        assert_eq!(list[0].detail, "");
        assert_eq!(list[1].detail, "start failed: couldn't fetch origin's main");
        assert_eq!(list[1].data["startFailed"], "couldn't fetch origin's main");
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
        // Outside Switchboard's clone no leak-check runs.
        r.landing = Some(json!({
            "holding": true,
            "phase": "pushing",
            "record": { "leak": { "skipped": true } },
        }));
        assert_eq!(stages(&r)[4], ("waiting", "not run".to_string()));
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
    fn asking_to_land_marks_only_the_checks_its_landing_ran() {
        let mut r = row(TabState::Waiting);
        r.loop_skill = Some("queue".into());
        r.phase = Some("asking to land".into());
        r.phases = vec![("testing".into(), 1), ("asking to land".into(), 2)];
        let s = cells(&r);
        assert_eq!(s[2], done("passed"));
        assert_eq!(s[3].0, "waiting", "no review ran: {s:?}");
        r.phases = vec![("asking to land".into(), 2)];
        assert_eq!(cells(&r)[2].0, "waiting");
    }

    #[test]
    fn each_stage_says_how_long_it_took_and_a_running_one_how_long_so_far() {
        let mut r = row(TabState::Working);
        r.started = Some(1_000);
        r.now = 1_000 + 12 * 60;
        let s = timed(cells(&r), &r);
        assert_eq!(s[1], ("running", "working… 12m".to_string()));
        // Its landing: worked 20m, tested 3m, reviewed 1h12m, waited 4m
        // for the lock in line, then tested again 2m, and pushing now.
        let at = |m: i64| 1_000 + m * 60;
        r.loop_skill = Some("queue".into());
        r.phases = vec![
            ("testing".into(), at(20)),
            ("reviewing".into(), at(23)),
            // The question before the push is no part of the review.
            ("asking to land".into(), at(95)),
            ("waiting for the landing lock".into(), at(95)),
            ("testing again".into(), at(99)),
            ("pushing".into(), at(101)),
        ];
        r.phase = Some("pushing".into());
        r.landing = Some(json!({ "holding": true, "phase": "pushing" }));
        r.now = at(102);
        let s = timed(cells(&r), &r);
        assert_eq!(s[1], done("done 20m"));
        assert_eq!(s[2], done("passed 5m"));
        assert_eq!(s[3], done("clean 1h12m"));
        assert_eq!(s[4], done("none"), "the leak check has no phase");
        assert_eq!(s[5], done("held 4m"));
        assert_eq!(s[6], ("running", "pushing… 1m".to_string()));
        // Landed: the push ends where the tab landed, not now, nor where a
        // step after the landing began.
        r.left = true;
        r.ended = Some(at(103));
        r.now = at(500);
        assert_eq!(timed(cells(&r), &r)[6], done("landed 2m"));
        r.phases.push(("cleaning up".into(), at(110)));
        assert_eq!(timed(cells(&r), &r)[6], done("landed 2m"));
        r.phases.pop();
        // A failed cell keeps its words alone.
        r.left = false;
        r.ended = None;
        r.phase = Some("tests failed 2 of 401".into());
        r.phases.push(("tests failed 2 of 401".into(), at(104)));
        assert_eq!(timed(cells(&r), &r)[2], ("failed", "✕ 2 of 401".into()));
    }

    #[test]
    fn a_card_carries_its_items_plan_the_name_alone_when_its_note_is_missing() {
        let plans = HashMap::from([
            (
                "Give notes stable IDs".to_string(),
                PlanNote {
                    name: "Plan - Stable IDs".into(),
                    text: Some("## Today\n\nFound by name.".into()),
                    file: None,
                },
            ),
            (
                "Next".to_string(),
                PlanNote {
                    name: "Plan - Next".into(),
                    text: None,
                    file: None,
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
            weekly_limit: 0,
            weekly: None,
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
            "",
            &[row(TabState::Working), other],
            &["Next".into()],
            &[],
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
            weekly_limit: 0,
            weekly: None,
            lands_alone: true,
            hands_free: false,
            target: "team/next".into(),
            drained: false,
            failing: "main failing at ebde129".into(),
        };
        let items = items(
            &s,
            "",
            "",
            &[row(TabState::Waiting)],
            &["Next".into()],
            &[],
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
        assert_eq!(
            board_columns(&items),
            [
                "Next up",
                "Working",
                "Waiting on you",
                "Landing",
                "Landed",
                "Skipped"
            ],
            "Blocked left out while nothing is blocked"
        );
        let none_skipped = super::items(
            &s,
            "",
            "",
            &[row(TabState::Waiting)],
            &["Next".into()],
            &[],
            &[],
            &HashMap::new(),
        );
        assert_eq!(
            board_columns(&none_skipped),
            ["Next up", "Working", "Waiting on you", "Landing", "Landed"]
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
            weekly_limit: 0,
            weekly: None,
            lands_alone: false,
            target: String::new(),
            drained: false,
            failing: String::new(),
            hands_free: false,
        };
        let items = items(
            &s,
            "",
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
            &[Held {
                text: "Builds on it".into(),
                waits_on: "item 6".into(),
                // One a tab works, one ninth in line, past the eight the
                // view is given, one the queue won't take.
                after: vec![
                    ("item 6".into(), Stands::Queued(3)),
                    ("Far back".into(), Stands::Queued(9)),
                    ("Elsewhere".into(), Stands::Open),
                ],
            }],
            &[],
            &HashMap::new(),
        );
        let held = items.iter().find(|i| i.title == "Builds on it").unwrap();
        assert_eq!(held.detail, "waits on item 6 and 2 more");
        assert_eq!(held.data["waitsOn"], "item 6");
        assert_eq!(
            held.data["after"],
            json!([
                { "item": "item 6", "tab": "@6", "index": 6, "column": "Working" },
                { "item": "Far back", "place": 9 },
                { "item": "Elsewhere" },
            ]),
            "each item it waits on, with the tab working it or its place in line"
        );
        assert!(board_columns(&items).contains(&"Blocked"));
        // One that waits on an item blocked itself says so.
        let chain = super::items(
            &s,
            "",
            "",
            &[],
            &[],
            &[
                Held {
                    text: "Middle".into(),
                    waits_on: "item 6".into(),
                    after: vec![("item 6".into(), Stands::Open)],
                },
                Held {
                    text: "Last".into(),
                    waits_on: "Middle".into(),
                    after: vec![("Middle".into(), Stands::Blocked)],
                },
            ],
            &[],
            &HashMap::new(),
        );
        let last = chain.iter().find(|i| i.title == "Last").unwrap();
        assert_eq!(
            last.data["after"],
            json!([{ "item": "Middle", "column": "Blocked" }])
        );
        // A tab left from a note worked before, its item reading the same,
        // isn't the one it waits on.
        let elsewhere = Row {
            in_note: false,
            ..tab(8, TabState::Working)
        };
        let other = super::items(
            &s,
            "",
            "",
            &[elsewhere],
            &[],
            &[Held {
                text: "Builds on it".into(),
                waits_on: "item 8".into(),
                after: vec![("item 8".into(), Stands::Open)],
            }],
            &[],
            &HashMap::new(),
        );
        let held = other.iter().find(|i| i.title == "Builds on it").unwrap();
        assert_eq!(held.data["after"], json!([{ "item": "item 8" }]));
        let review = super::items(
            &s,
            "",
            "",
            &[],
            &[],
            &[Held {
                text: "New list".into(),
                waits_on: "your review of board 69".into(),
                after: Vec::new(),
            }],
            &[],
            &HashMap::new(),
        );
        let review = review.iter().find(|i| i.title == "New list").unwrap();
        assert_eq!(review.detail, "waits on your review of board 69");
        assert_eq!(
            review.group.as_deref(),
            Some("Next up"),
            "held by a board's review, not by an item"
        );
        assert_eq!(review.data.get("after"), None);
        let placed: Vec<(&str, &str)> = items
            .iter()
            .filter_map(|i| Some((i.group.as_deref()?, i.title.as_str())))
            .collect();
        assert_eq!(
            placed,
            [
                ("Next up", "1. Later"),
                ("Blocked", "Builds on it"),
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
            weekly_limit: 0,
            weekly: None,
            lands_alone: false,
            hands_free: false,
            target: String::new(),
            drained: false,
            failing: String::new(),
        };
        let items = items(
            &summary,
            "",
            "",
            &[open, closed],
            &[],
            &[],
            &[],
            &HashMap::new(),
        );
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
            weekly_limit: 0,
            weekly: None,
            lands_alone: false,
            hands_free: false,
            target: String::new(),
            drained: true,
            failing: String::new(),
        };
        let items = items(
            &s,
            "",
            "Nothing left in the note",
            &[],
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

    #[test]
    fn a_row_is_timed_from_its_opening_to_its_landing() {
        let opts = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        assert_eq!(
            times_of_tab(&opts(&[(OPENED, "1700000000")])),
            (Some(1_700_000_000), None)
        );
        assert_eq!(
            times_of_tab(&opts(&[
                (OPENED, "1700000000"),
                (LEFT, "1700009000"),
                (LANDED_AT, "1700007000"),
            ])),
            (Some(1_700_000_000), Some(1_700_007_000))
        );
        assert_eq!(
            times_of_tab(&opts(&[(OPENED, "1700000000"), (LEFT, "1700009000")])),
            (Some(1_700_000_000), Some(1_700_009_000))
        );
        let closed = Landed {
            id: "9@1700000000".into(),
            item: "Copy the reply".into(),
            note: "n-1".into(),
            hash: "abc1234".into(),
            at: 1_700_004_320_500,
        };
        assert_eq!(
            times_of_closed(&closed),
            (Some(1_700_000_000), Some(1_700_004_320))
        );
    }

    #[test]
    fn the_heading_says_when_the_ready_items_should_be_done_and_how_many_are_left() {
        let now = jiff::civil::date(2026, 11, 11)
            .at(13, 0, 0, 0)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .unwrap();
        let secs = now.timestamp().as_second();
        let at_work = Row {
            started: Some(secs - 15 * 60),
            ..row(TabState::Working)
        };
        let landed = |n: i64| Row {
            left: true,
            started: Some(secs - 70 * 60 + n * 60),
            ended: Some(secs - 40 * 60 + n * 60),
            ..row(TabState::Idle)
        };
        let rows = [landed(0), landed(1), landed(2), at_work];
        let left = eta::Left {
            ready: vec!["A".into(), "B".into(), "C".into()],
            after: Vec::new(),
            on_you: 2,
        };
        let words = eta(&rows, &[], 2, 5, false, &left, &now);
        assert_eq!(words, "done by about 2:00 PM · 4 left · 2 wait on you");
        let s = Summary {
            note: "Work".into(),
            busy: 1,
            open: 1,
            slots: 2,
            cap: 4,
            next: String::new(),
            paused: false,
            away: false,
            ready_only: false,
            skipped: 0,
            spend_limit: 0.0,
            spent: 0.0,
            weekly_limit: 0,
            weekly: None,
            lands_alone: false,
            target: String::new(),
            drained: false,
            failing: String::new(),
            hands_free: false,
        };
        let list = items(&s, &words, "", &rows, &[], &[], &[], &HashMap::new());
        assert!(
            list[0]
                .detail
                .contains("1 of 4 open · done by about 2:00 PM · 4 left · 2 wait on you · asks"),
            "{}",
            list[0].detail
        );
        // Landings the mode kept count, a closed tab of another note's too,
        // each once with its open tab: three of ten minutes and the three
        // of 30 give 20 minutes, so the three ready end at 20, 25 and 40.
        let kept: Vec<Landed> = (0..3)
            .map(|n| Landed {
                id: format!("9@{}", secs - 200 * 60 + n * 60),
                item: format!("Elsewhere {n}"),
                note: "other".into(),
                hash: "abc1234".into(),
                at: (secs - 190 * 60 + n * 60) * 1000,
            })
            .chain(std::iter::once(Landed {
                id: format!("4@{}", secs - 70 * 60),
                item: "Give notes stable IDs".into(),
                note: "work".into(),
                hash: "def5678".into(),
                at: (secs - 40 * 60) * 1000,
            }))
            .collect();
        assert_eq!(
            eta(&rows, &kept, 2, 5, false, &left, &now),
            "done by about 1:40 PM · 4 left · 2 wait on you"
        );
        // Ten-minute landings two days ago don't count: still half an hour.
        let old = |n: i64| Row {
            started: Some(secs - 2 * 86_400 + n * 60),
            ended: Some(secs - 2 * 86_400 + 10 * 60 + n * 60),
            ..landed(n)
        };
        let stale = [old(0), old(1), old(2), rows[3].clone()];
        assert_eq!(eta(&stale, &[], 2, 5, false, &left, &now), words);
        // A tab waiting on you frees its slot and waits on you: the three
        // ready take 30, 30 and 60 minutes in two slots.
        let asking = Row {
            started: Some(secs - 15 * 60),
            ..row(TabState::Waiting)
        };
        let with_asking = [landed(0), landed(1), landed(2), asking];
        assert_eq!(
            eta(&with_asking, &[], 2, 5, false, &left, &now),
            "done by about 2:00 PM · 3 left · 3 wait on you"
        );
        // One that finished its turn without landing waits on you too.
        let finished = Row {
            started: Some(secs - 15 * 60),
            ..row(TabState::Done)
        };
        let with_finished = [landed(0), landed(1), landed(2), finished];
        assert_eq!(
            eta(&with_finished, &[], 2, 5, false, &left, &now),
            "done by about 2:00 PM · 3 left · 3 wait on you"
        );
        // One landed and asking whether to apply the change waits on you,
        // and holds its place among the most open.
        let applying = Row {
            started: Some(secs - 50 * 60),
            ended: Some(secs - 5 * 60),
            ..row(TabState::Waiting)
        };
        let with_applying = [landed(0), landed(1), landed(2), applying];
        assert_eq!(
            eta(&with_applying, &[], 2, 1, false, &left, &now),
            "3 wait on you"
        );
        // With the tabs waiting on you filling the most open, nothing starts.
        assert_eq!(
            eta(&with_asking, &[], 2, 1, false, &left, &now),
            "3 wait on you"
        );
        let nothing = eta(&rows[..3], &[], 2, 5, false, &eta::Left::default(), &now);
        assert_eq!(nothing, "", "nothing working or ready");
        assert_eq!(
            eta(&rows, &[], 2, 5, true, &eta::Left::default(), &now),
            "",
            "paused, it opens nothing"
        );
    }
}
