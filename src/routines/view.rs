//! Mission control's Routines view (UX 2.0 board 29). Its items are the
//! routines (`items`), the repeating ones first, then those to run once,
//! then those that ran once, each carrying in its data what the board
//! draws, so drawing reads nothing but the snapshot.
//!
//! With UX 2.0 it is drawn as the board draws it (`board`): on the left,
//! Recurring, Once and Ran once, each routine a two-line row (its mark,
//! its name, its schedule dim and when it runs next at the right; under
//! it a chip for where it runs and one for its last run in its outcome's
//! tone), then This week, a row a routine and a column a day with each
//! day's runs as their marks (◌ ran late on waking, – skipped), today's
//! day red; on the right, the chosen routine as the kit's pane: its
//! schedule and directory, its keys as keycaps, its prompt on a panel,
//! where it runs, today's runs (earlier ones folded with their spend) and
//! its next runs. Narrower than `BESIDE_AT`, the pane is behind `i`, in
//! the list's place. Without UX 2.0 the view is a plain list.

use std::collections::HashMap;

use jiff::Zoned;
use jiff::civil::{Date, Weekday};
use jiff::tz::TimeZone;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use serde_json::{Value, json};

use super::routine::{Routine, Run, TabPolicy, WhenBusy, WhenMissed};
use super::{LATE_BY, Live};
use crate::items::{Item, State, Target};
use crate::mission::views::View;
use crate::mission::{Focus, HitBox, Mission, Spot};
use crate::mux::TabId;
use crate::mux::input::{Key, KeyCode, Mods};
use crate::sidebar::model::{self, TabModel};
use crate::sidebar::render::Line;
use crate::sidebar::text::{cut, width, wrap};
use crate::ui::kit::{self, Col, Mark, Surface, Tone};

/// Narrower than this, the pane is behind `i`.
pub const BESIDE_AT: usize = 140;
/// Narrower than this, each run's tab goes, then the rows' schedule.
const ROOMY_AT: usize = 160;
/// The pane's share of 160 columns, and its least.
const PANE_SHARE: usize = 62;
const PANE_LEAST: usize = 34;
/// What the view says with no routine.
pub const EMPTY: &str =
    "No routines yet. n makes one: a prompt of yours run on a schedule in a Claude tab.";

/// What the server holds that the items are made from.
pub struct Facts<'a> {
    pub now: Zoned,
    /// Each routine's id, its tab while open, and that tab's number.
    pub tabs: Vec<(String, Option<TabId>, String)>,
    pub live: &'a HashMap<String, Live>,
    pub runs: &'a HashMap<String, Vec<Run>>,
}

/// Where a routine stands in the list.
fn section(r: &Routine, tz: &TimeZone) -> u8 {
    if !r.schedule.once() {
        0
    } else if r.next(tz).is_some() {
        1
    } else {
        2
    }
}

fn run_json(run: &Run) -> Value {
    json!({
        "at": run.at(),
        "mark": run.outcome.mark(),
        "outcome": format!("{:?}", run.outcome),
        "words": run.words,
        "tab": run.tab,
        "secs": run.secs,
        "cost": run.cost,
    })
}

/// The view's items, one per routine.
pub fn items(routines: &[Routine], f: &Facts<'_>) -> Vec<Item> {
    let tz = f.now.time_zone().clone();
    let now = f.now.timestamp().as_second();
    let today = f.now.date();
    let monday = today
        .checked_sub(jiff::Span::new().days(today.weekday().to_monday_zero_offset() as i64))
        .unwrap_or(today);
    let mut sorted: Vec<&Routine> = routines.iter().collect();
    sorted.sort_by_key(|r| {
        (
            section(r, &tz),
            r.next(&tz).unwrap_or(i64::MAX),
            r.id.clone(),
        )
    });
    sorted
        .into_iter()
        .map(|r| {
            let (tab, number) = f
                .tabs
                .iter()
                .find(|(id, _, _)| *id == r.id)
                .map(|(_, t, n)| (*t, n.clone()))
                .unwrap_or((None, String::new()));
            let next = if r.paused { None } else { r.next(&tz) };
            let upcoming = if r.paused {
                Vec::new()
            } else {
                r.schedule.next(r.handled.max(now), 3, &tz)
            };
            let runs: Vec<&Run> = f.runs.get(&r.id).map(|v| v.iter().collect()).unwrap_or_default();
            let last = runs.last().map(|run| run_json(run));
            let mut week = vec![Vec::<String>::new(); 7];
            for run in &runs {
                let day = jiff::Timestamp::from_second(run.at())
                    .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
                    .to_zoned(tz.clone())
                    .date();
                if let Ok(d) = day.since(monday)
                    && (0..7).contains(&d.get_days())
                {
                    week[d.get_days() as usize].push(run.outcome.mark().to_string());
                }
            }
            let live = f.live.get(&r.id).map(|l| {
                if l.started > 0 {
                    "running"
                } else {
                    "waiting"
                }
            });
            let describe = r.schedule.describe(&f.now);
            let next_words = next.map(|n| when_words(n, &f.now)).unwrap_or_default();
            let mut item = Item::new(format!("routine:{}", r.id), r.name.clone());
            let mut detail = vec![describe.clone()];
            if r.paused {
                detail.push("paused".into());
            } else if let Some(n) = next {
                detail.push(format!("next {}", when_words(n, &f.now)));
            } else {
                detail.push("ran once".into());
            }
            if !number.is_empty() {
                detail.push(format!("tab {number}"));
            }
            item.detail = detail.join(" · ");
            item.time = next.map(|n| n * 1000);
            item.target = tab.map(Target::Tab).unwrap_or(Target::None);
            item.state = match (live, last.as_ref().and_then(|l| l["mark"].as_str())) {
                (Some(_), _) => State::Active,
                (None, Some("✕")) => State::Attention,
                _ => State::Idle,
            };
            item.data = json!({
                "routine": r.id,
                "search": format!("{} {}", r.prompt, r.dir),
                "name": r.name,
                "describe": describe,
                "when": r.when,
                "section": section(r, &tz),
                "next": next,
                "nextWords": next_words,
                "upcoming": upcoming,
                "paused": r.paused,
                "dir": crate::paths::tilde(std::path::Path::new(&r.dir)),
                "prompt": r.prompt,
                "model": r.model,
                "policy": match r.tab_policy { TabPolicy::SameTab => "same", TabPolicy::NewTab => "new" },
                "busy": match r.when_busy { WhenBusy::Wait => "wait", WhenBusy::Skip => "skip" },
                "missed": match r.when_missed { WhenMissed::OnWaking => "waking", WhenMissed::Skip => "skip" },
                "tab": number,
                "session": !r.resumable().is_empty(),
                "live": live,
                "last": last,
                "week": week,
                "runs": runs.iter().rev().map(|run| run_json(run)).collect::<Vec<_>>(),
            });
            item
        })
        .collect()
}

/// When a run is: its clock time today, else its weekday and time this
/// week, else its date.
pub fn when_words(at: i64, now: &Zoned) -> String {
    let z = jiff::Timestamp::from_second(at)
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
        .to_zoned(now.time_zone().clone());
    let clock = z.strftime("%-I:%M %p").to_string();
    let days = z.date().since(now.date()).map_or(0, |d| d.get_days());
    match days {
        0 => clock,
        1 => format!("tomorrow {clock}"),
        2..=6 => format!("{} {clock}", crate::mission::remind::short_day(z.weekday())),
        _ => format!(
            "{} {} {clock}",
            crate::mission::remind::short_month(z.month()),
            z.day()
        ),
    }
}

fn s<'a>(item: &'a Item, key: &str) -> &'a str {
    item.data.get(key).and_then(Value::as_str).unwrap_or("")
}

fn b(item: &Item, key: &str) -> bool {
    item.data.get(key).and_then(Value::as_bool) == Some(true)
}

fn section_of(item: &Item) -> u64 {
    item.data
        .get("section")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

/// The routine's id an item is.
pub fn id_of(item: &Item) -> Option<&str> {
    item.data.get("routine").and_then(Value::as_str)
}

/// The bar's right with the view in front: "5 routines · next at 2:30 PM".
pub fn bar_words(view: &View, now: &Zoned) -> String {
    let n = view.data.items.len();
    let mut words = format!("{n} routine{}", if n == 1 { "" } else { "s" });
    let soonest = view
        .data
        .items
        .iter()
        .filter_map(|i| i.data.get("next").and_then(Value::as_i64))
        .min();
    if let Some(at) = soonest {
        words.push_str(&format!(" · next at {}", when_words(at, now)));
    }
    words
}

/// The tone a run's outcome is drawn in.
fn tone_of(outcome: &str) -> Tone {
    match outcome {
        "Ran" => Tone::Done,
        "Late" => Tone::Needs,
        "Missed" | "Busy" => Tone::Dim,
        _ => Tone::Failed,
    }
}

/// Whether the board is drawn: UX 2.0 on.
pub fn is_board(m: &Mission) -> bool {
    m.snap.rules.ux2
}

/// The routine chosen: the cursor's.
pub fn chosen(m: &Mission, view: &View) -> Option<usize> {
    usize::try_from(m.cursor)
        .ok()
        .filter(|c| *c < view.data.items.len())
        .or_else(|| (!view.data.items.is_empty()).then_some(0))
}

/// A line drawn, and where clicks on it land, by columns from its left.
#[derive(Default, Clone)]
struct Drawn {
    spans: Vec<Span<'static>>,
    hits: Vec<(usize, usize, Spot)>,
}

impl Drawn {
    fn of(spans: Vec<Span<'static>>) -> Drawn {
        Drawn {
            spans,
            hits: Vec::new(),
        }
    }
}

/// The board, from row `lines.len()`, and where clicks land. The list
/// keeps the chosen routine's rows in sight; the pane beside it scrolls
/// with J and K and the wheel over it (`Spot::Side`, `Mission::side_top`).
pub fn board(
    view: &View,
    m: &Mission,
    cols: usize,
    rows: usize,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
) {
    let theme = &m.snap.theme;
    let top = lines.len();
    let body = rows.saturating_sub(1 + top);
    let beside = cols >= BESIDE_AT;
    let alone = !beside && m.routines_pane;
    let pane_w = if beside {
        kit::beside(cols, PANE_SHARE * 160 / 124, PANE_LEAST, 60).min(cols / 2)
    } else if alone {
        cols.saturating_sub(2)
    } else {
        0
    };
    let list_w = if beside { cols - pane_w - 3 } else { cols };
    let pane_x = if beside { list_w + 3 } else { 1 };
    let chosen = chosen(m, view);
    // The pane, from where it was scrolled to.
    let side = chosen
        .filter(|_| pane_w > 0)
        .map(|i| {
            pane(
                &view.data.items[i],
                m,
                pane_w,
                body,
                beside && cols >= ROOMY_AT,
            )
        })
        .unwrap_or_default();
    let last = side.len().saturating_sub(body);
    let side: Vec<Drawn> = side
        .into_iter()
        .skip(m.side_top().min(last))
        .take(body)
        .collect();
    // The list, from where the chosen routine's two rows show.
    let mut list = Vec::new();
    if !alone {
        let (rows_of, chosen_line) = self::list(view, m, list_w, chosen, cols);
        // The last routine chosen shows This week under it, so the week is
        // reached by moving down to it.
        let last_chosen = chosen.is_some_and(|i| i + 1 == view.data.items.len());
        let end = if last_chosen {
            rows_of.len()
        } else {
            chosen_line + 2
        };
        // The last one chosen scrolls to the week's end, however long the
        // week is; any other keeps its own rows in sight.
        let first = if last_chosen {
            end.saturating_sub(body)
        } else {
            end.saturating_sub(body).min(chosen_line)
        };
        list = rows_of.into_iter().skip(first).take(body).collect();
        if !beside && chosen.is_some() && list.len() < body {
            let mut hint = vec![Span::raw(" ")];
            hint.extend(kit::hints(&[("i", "the chosen routine")], theme));
            list.push(Drawn::of(hint));
        }
    }
    for r in 0..body {
        let y = (top + r) as u16;
        let mut spans = Vec::new();
        let mut at = |x0: usize, x1: usize, spot: Spot| {
            hits.push(HitBox {
                y,
                x0: x0 as u16,
                x1: x1 as u16,
                spot,
            })
        };
        if !alone {
            let d = list.get(r).cloned().unwrap_or_default();
            spans = kit::fit(d.spans, list_w, Style::new());
            for (x0, x1, spot) in d.hits {
                at(x0, x1, spot);
            }
        }
        if beside {
            spans.push(Span::raw(" "));
            spans.push(kit::rule_v(theme));
            spans.push(Span::raw(" "));
        } else if alone {
            spans.push(Span::raw(" "));
        }
        if let Some(p) = side.get(r) {
            spans.extend(kit::fit(p.spans.clone(), pane_w, Style::new()));
            for (a, b, spot) in &p.hits {
                at(a + pane_x, b + pane_x, *spot);
            }
        }
        if !alone && !beside && list.get(r).is_none() {
            continue;
        }
        lines.push(Line {
            spans: kit::fit(spans, cols, Style::new()),
            fill: None,
        });
    }
    // After the pane's own clicks, which a click on their row finds first.
    if pane_w > 0 && !side.is_empty() {
        for r in 0..body {
            hits.push(HitBox {
                y: (top + r) as u16,
                x0: pane_x as u16,
                x1: cols.saturating_sub(1) as u16,
                spot: Spot::Side(last),
            });
        }
    }
}

/// The list: each section's routines, then This week; and the line the
/// chosen routine's rows start on.
fn list(
    view: &View,
    m: &Mission,
    w: usize,
    chosen: Option<usize>,
    cols: usize,
) -> (Vec<Drawn>, usize) {
    let theme = &m.snap.theme;
    let items = &view.data.items;
    let mut out: Vec<Drawn> = Vec::new();
    if items.is_empty() {
        out.push(Drawn::default());
        out.extend(wrap(EMPTY, w.saturating_sub(2)).into_iter().map(|l| {
            Drawn::of(vec![
                Span::raw(" "),
                Span::styled(l, Tone::Dim.style(theme)),
            ])
        }));
        return (out, 0);
    }
    let mut chosen_line = 0;
    for (sec, label) in [(0, "Recurring"), (1, "Once"), (2, "Ran once")] {
        let these: Vec<usize> = (0..items.len())
            .filter(|i| section_of(&items[*i]) == sec)
            .collect();
        if these.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(Drawn::default());
        }
        let tone = if sec == 2 { Tone::Faint } else { Tone::Dim };
        out.push(Drawn::of(kit::label(
            label,
            tone,
            Some(these.len()),
            w,
            theme,
        )));
        for i in these {
            if chosen == Some(i) {
                chosen_line = out.len();
            }
            out.extend(row(&items[i], m, w, chosen == Some(i), i, cols));
        }
    }
    out.push(Drawn::default());
    out.extend(week(view, m, w, chosen));
    (out, chosen_line)
}

/// A routine's two lines.
fn row(item: &Item, m: &Mission, w: usize, chosen: bool, i: usize, cols: usize) -> Vec<Drawn> {
    let theme = &m.snap.theme;
    let caps = m.snap.caps;
    let lit = m.hover == Some(Spot::Index(i as isize));
    let mark = match (
        item.data.get("live").and_then(Value::as_str),
        b(item, "paused"),
    ) {
        (Some("running"), _) => kit::mark(Mark::Working, m.snap.frame, theme),
        (Some(_), _) => kit::mark(Mark::Pending, m.snap.frame, theme),
        (None, true) => Span::styled("‖", Tone::Dim.bold(theme)),
        (None, false) if section_of(item) == 2 => Span::styled("✓", Tone::Faint.bold(theme)),
        (None, false) if section_of(item) == 1 => kit::mark(Mark::Later, m.snap.frame, theme),
        (None, false) => Span::styled("↻", Tone::Accent.bold(theme)),
    };
    let name = s(item, "name").to_string();
    let schedule = s(item, "describe").to_string();
    let next = if b(item, "paused") {
        "paused".to_string()
    } else {
        match s(item, "nextWords") {
            "" => "ran once".to_string(),
            n => n.to_string(),
        }
    };
    let specs = [
        Col::Fixed {
            min: 1,
            natural: 1,
            cap: 1,
        },
        Col::Fixed {
            min: 4,
            natural: width(&name),
            cap: 22,
        },
        Col::Fixed {
            min: 0,
            natural: width(&schedule),
            cap: 24,
        },
        Col::Stretch {
            weight: 1,
            min: 0,
            max: None,
        },
        Col::Right {
            natural: width(&next),
            cap: 18,
        },
    ];
    // Below 160 columns the rows' schedule goes, the pane saying it.
    let drop: &[usize] = &[2];
    let mut specs = specs.to_vec();
    if cols < ROOMY_AT {
        specs[2] = Col::Fixed {
            min: 0,
            natural: 0,
            cap: 0,
        };
    }
    let placed = kit::columns(&specs, w.saturating_sub(2), 2, drop);
    let mut first = Vec::new();
    let name_style = if chosen {
        Style::new().fg(theme.bright).add_modifier(Modifier::BOLD)
    } else {
        Tone::Text.bold(theme)
    };
    if let Some(c) = placed[0] {
        kit::place(&mut first, vec![mark], c);
    }
    if let Some(c) = placed[1] {
        kit::place(
            &mut first,
            vec![Span::styled(cut(&name, c.1), name_style)],
            c,
        );
    }
    if let Some(c) = placed[2].filter(|c| c.1 > 0) {
        kit::place(
            &mut first,
            vec![Span::styled(cut(&schedule, c.1), Tone::Dim.style(theme))],
            c,
        );
    }
    if let Some(c) = placed[4] {
        let tone = if b(item, "paused") {
            Tone::Faint
        } else {
            Tone::Dim
        };
        let rw = width(&next).min(c.1);
        kit::place(
            &mut first,
            vec![Span::styled(cut(&next, rw), tone.style(theme))],
            (c.0 + c.1 - rw, rw),
        );
    }
    // Where it runs, and its last run.
    let tab = s(item, "tab");
    let where_ = match (s(item, "policy"), tab.is_empty(), b(item, "session")) {
        ("new", _, _) => "a new tab each run".to_string(),
        (_, false, _) => format!("tab {tab} · open"),
        (_, true, true) => "tab closed · resumes its session".to_string(),
        (_, true, false) => "opens its tab on its first run".to_string(),
    };
    let mut second = vec![Span::raw("  ")];
    second.extend(kit::chip(&where_, Tone::Dim, caps, theme));
    if let Some(last) = item.data.get("last").filter(|l| !l.is_null()) {
        let outcome = last["outcome"].as_str().unwrap_or("");
        let at = last["at"].as_i64().unwrap_or(0);
        let now = jiff::Timestamp::from_second(m.snap.now_secs)
            .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
            .to_zoned(m.snap.tz.clone());
        let words = format!(
            "{} {}",
            last["mark"].as_str().unwrap_or(""),
            when_words(at, &now)
        );
        second.extend(kit::chip(&words, tone_of(outcome), caps, theme));
    }
    let wrap_row = |spans: Vec<Span<'static>>| {
        let mut line = kit::row(spans, w, chosen, theme);
        if lit && !chosen {
            let bg = Surface::Panel.color(theme);
            line = line
                .into_iter()
                .map(|s| {
                    let style = if s.style.bg.is_none() {
                        s.style.bg(bg)
                    } else {
                        s.style
                    };
                    Span::styled(s.content, style)
                })
                .collect();
        }
        Drawn {
            spans: line,
            hits: vec![(0, w.saturating_sub(1), Spot::Index(i as isize))],
        }
    };
    vec![wrap_row(first), wrap_row(second)]
}

/// This week: a row a routine and a column a day, today's red.
fn week(view: &View, m: &Mission, w: usize, chosen: Option<usize>) -> Vec<Drawn> {
    let theme = &m.snap.theme;
    let items = &view.data.items;
    let today = jiff::Timestamp::from_second(m.snap.now_secs)
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
        .to_zoned(m.snap.tz.clone())
        .date();
    let monday: Date = today
        .checked_sub(jiff::Span::new().days(today.weekday().to_monday_zero_offset() as i64))
        .unwrap_or(today);
    let mut out = vec![Drawn::of(kit::label(
        "This week",
        Tone::Dim,
        None,
        w,
        theme,
    ))];
    let names_w = items.iter().map(|i| width(s(i, "name"))).max().unwrap_or(4);
    let mut specs = vec![Col::Fixed {
        min: 4,
        natural: names_w,
        cap: 20,
    }];
    specs.extend(
        [Col::Stretch {
            weight: 1,
            min: 3,
            max: None,
        }; 7],
    );
    let placed = kit::columns(&specs, w.saturating_sub(2), 1, &[]);
    let red = Style::new()
        .fg(Color::Indexed(196))
        .add_modifier(Modifier::BOLD);
    let mut head = Vec::new();
    for d in 0..7 {
        let Some(c) = placed[d + 1] else { continue };
        let date = monday
            .checked_add(jiff::Span::new().days(d as i64))
            .unwrap_or(monday);
        let words = format!("{} {}", day_word(date.weekday()), date.day());
        let style = if date == today {
            red
        } else {
            Tone::Dim.style(theme)
        };
        kit::place(
            &mut head,
            vec![Span::styled(cut(&words, c.1), style)],
            (c.0 + 2, c.1),
        );
    }
    out.push(Drawn::of(head));
    out.push(Drawn::of(vec![
        Span::raw("  "),
        kit::head_rule(w.saturating_sub(2), theme),
    ]));
    let mut late_or_skipped = (false, false);
    for (i, item) in items.iter().enumerate() {
        let mut line = Vec::new();
        if let Some(c) = placed[0] {
            let style = if chosen == Some(i) {
                Style::new().fg(theme.bright).add_modifier(Modifier::BOLD)
            } else {
                Tone::Text.style(theme)
            };
            kit::place(
                &mut line,
                vec![Span::styled(cut(s(item, "name"), c.1), style)],
                (c.0 + 2, c.1),
            );
        }
        let days: Vec<Vec<String>> = item
            .data
            .get("week")
            .and_then(Value::as_array)
            .map(|days| {
                days.iter()
                    .map(|d| {
                        d.as_array()
                            .map(|marks| {
                                marks
                                    .iter()
                                    .filter_map(|x| x.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .unwrap_or_default();
        for d in 0..7 {
            let Some(c) = placed[d + 1] else { continue };
            let marks = days.get(d).cloned().unwrap_or_default();
            let mut spans = Vec::new();
            for mark in ["✓", "◌", "–", "✕"] {
                let n = marks.iter().filter(|x| *x == mark).count();
                if n == 0 {
                    continue;
                }
                late_or_skipped.0 |= mark == "◌";
                late_or_skipped.1 |= mark == "–";
                let tone = match mark {
                    "✓" => Tone::Done,
                    "◌" => Tone::Needs,
                    "–" => Tone::Dim,
                    _ => Tone::Failed,
                };
                spans.push(Span::styled(mark, tone.bold(theme)));
                // The counts go first on a narrow day.
                if n > 1 && c.1 >= 6 {
                    spans.push(Span::styled(n.to_string(), tone.style(theme)));
                }
                spans.push(Span::raw(" "));
            }
            if spans.is_empty() {
                spans.push(Span::styled("·", Tone::Faint.style(theme)));
            }
            kit::place(&mut line, spans, (c.0 + 2, c.1));
        }
        out.push(Drawn {
            spans: line,
            hits: vec![(0, w.saturating_sub(1), Spot::Index(i as isize))],
        });
    }
    let mut keys = Vec::new();
    if late_or_skipped.0 {
        keys.push("◌ missed while asleep, ran on waking");
    }
    if late_or_skipped.1 {
        keys.push("– missed or busy, so skipped");
    }
    if !keys.is_empty() {
        out.push(Drawn::of(vec![
            Span::raw("  "),
            Span::styled(
                cut(&keys.join(" · "), w.saturating_sub(2)),
                Tone::Faint.style(theme),
            ),
        ]));
    }
    out
}

fn day_word(d: Weekday) -> &'static str {
    crate::mission::remind::short_day(d)
}

/// A duration in a few characters: 42s, 3m, 1h 5m.
fn length(secs: i64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s => format!("{}h {}m", s / 3600, s % 3600 / 60),
    }
}

/// The chosen routine, as the kit's pane.
fn pane(item: &Item, m: &Mission, w: usize, body: usize, roomy: bool) -> Vec<Drawn> {
    let theme = &m.snap.theme;
    let caps = m.snap.caps;
    let now = jiff::Timestamp::from_second(m.snap.now_secs)
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
        .to_zoned(m.snap.tz.clone());
    let mut out: Vec<Drawn> = Vec::new();
    let mut facts = vec![Span::styled(
        s(item, "describe").to_string(),
        Tone::Dim.style(theme),
    )];
    facts.push(Span::styled(
        format!(" · {}", s(item, "dir")),
        Tone::Faint.style(theme),
    ));
    out.extend(
        kit::pane(s(item, "name"), facts, w, theme)
            .into_iter()
            .map(Drawn::of),
    );
    // Its keys, each a click.
    let lit = |key: char| match (m.hover, m.focus) {
        (Some(Spot::Key(k)), _) | (_, Some(Focus::Key(k))) => {
            k == Key::new(KeyCode::Char(key), Mods::NONE)
        }
        _ => false,
    };
    let mut caps_line = Drawn::default();
    let pause = if b(item, "paused") { "Resume" } else { "Pause" };
    let mut pairs = vec![
        ('r', "Run now"),
        ('e', "Edit"),
        ('p', pause),
        ('x', "Delete"),
    ];
    if !s(item, "tab").is_empty() {
        pairs.insert(1, ('o', "Its tab"));
    }
    for (key, what) in pairs {
        let x = kit::spans_width(&caps_line.spans);
        let cap = kit::keycap(&key.to_string(), what, lit(key), caps, theme);
        let cw = kit::spans_width(&cap);
        if x + cw > w {
            break;
        }
        caps_line.spans.extend(cap);
        caps_line.hits.push((
            x,
            (x + cw).saturating_sub(2),
            Spot::Key(Key::new(KeyCode::Char(key), Mods::NONE)),
        ));
    }
    out.push(caps_line);
    out.push(Drawn::default());

    // The prompt, on a panel.
    let right = "added to Claude's system prompt";
    let mut head = kit::label("Prompt", Tone::Dim, None, w, theme);
    head.truncate(2);
    let gap = w.saturating_sub(kit::spans_width(&head) + width(right) + 1);
    if gap > 1 {
        head.push(Span::raw(" ".repeat(gap)));
        head.push(Span::styled(right, Tone::Faint.style(theme)));
    }
    out.push(Drawn::of(head));
    let panel = Surface::Panel.color(theme);
    let text_w = w.saturating_sub(4).min(kit::WRAP_MAX);
    let mut prompt_lines: Vec<String> = s(item, "prompt")
        .lines()
        .flat_map(|l| {
            if l.is_empty() {
                vec![String::new()]
            } else {
                wrap(l, text_w)
            }
        })
        .collect();
    // The panel takes what rows are left over, up to its words.
    let most = (body / 4).max(3);
    if prompt_lines.len() > most {
        prompt_lines.truncate(most);
        if let Some(last) = prompt_lines.last_mut() {
            *last = format!("{}…", cut(last, text_w.saturating_sub(1)));
        }
    }
    for l in prompt_lines {
        out.push(Drawn::of(kit::fit(
            vec![
                Span::styled("  ", Style::new().bg(panel)),
                Span::styled(l, Tone::Text.style(theme).bg(panel)),
            ],
            w,
            Style::new().bg(panel),
        )));
    }
    let model = s(item, "model");
    if !model.is_empty() {
        out.push(Drawn::of(vec![Span::styled(
            format!(" model {model}"),
            Tone::Faint.style(theme),
        )]));
    }
    out.push(Drawn::default());

    // Where it runs.
    out.push(Drawn::of(kit::label(
        "Where it runs",
        Tone::Dim,
        None,
        w,
        theme,
    )));
    let mut tab_line = vec![Span::raw(" ")];
    let tab = match item.target {
        Target::Tab(id) => m
            .snap
            .all_tabs
            .iter()
            .chain(m.snap.tabs.iter())
            .find(|t| t.id == id),
        _ => None,
    };
    match tab {
        Some(t) => tab_line.extend(tab_words(t, m)),
        None if s(item, "policy") == "new" => {
            tab_line.push(Span::styled("a new tab each run", Tone::Text.style(theme)))
        }
        None => tab_line.push(Span::styled("no tab open", Tone::Dim.style(theme))),
    }
    out.push(Drawn::of(tab_line));
    let mut rules = Vec::new();
    rules.push(match (s(item, "policy"), b(item, "session")) {
        ("new", _) => "Each run opens a tab of its own in its directory.".to_string(),
        (_, true) => {
            "The same tab each run; once it is closed, a new one resumes its session.".to_string()
        }
        _ => "The same tab each run, opened on its first.".to_string(),
    });
    rules.push(match s(item, "busy") {
        "skip" => "Busy then, the run is skipped.".to_string(),
        _ => "Busy then, it waits for the turn to end, up to 30 minutes.".to_string(),
    });
    rules.push(match s(item, "missed") {
        "skip" => "Missed while the computer slept, it is skipped.".to_string(),
        _ => format!(
            "Missed while the computer slept, it runs on waking, up to {} late.",
            if LATE_BY == 3600 {
                "an hour"
            } else {
                "a while"
            }
        ),
    });
    for rule in rules {
        out.extend(wrap(&rule, w.saturating_sub(2)).into_iter().map(|l| {
            Drawn::of(vec![
                Span::raw(" "),
                Span::styled(l, Tone::Faint.style(theme)),
            ])
        }));
    }
    out.push(Drawn::default());

    // Today's runs, the earlier ones folded with their spend.
    let runs: Vec<&Value> = item
        .data
        .get("runs")
        .and_then(Value::as_array)
        .map(|r| r.iter().collect())
        .unwrap_or_default();
    let day_of = |at: i64| {
        jiff::Timestamp::from_second(at)
            .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
            .to_zoned(m.snap.tz.clone())
            .date()
    };
    let today: Vec<&Value> = runs
        .iter()
        .copied()
        .filter(|r| day_of(r["at"].as_i64().unwrap_or(0)) == now.date())
        .collect();
    let monday = now
        .date()
        .checked_sub(jiff::Span::new().days(now.date().weekday().to_monday_zero_offset() as i64))
        .unwrap_or(now.date());
    let earlier: Vec<&Value> = runs
        .iter()
        .copied()
        .filter(|r| {
            let day = day_of(r["at"].as_i64().unwrap_or(0));
            day < now.date() && day >= monday
        })
        .collect();
    out.push(Drawn::of(kit::label(
        "Runs today",
        Tone::Dim,
        Some(today.len()),
        w,
        theme,
    )));
    if today.is_empty() {
        out.push(Drawn::of(vec![Span::styled(
            " none yet",
            Tone::Faint.style(theme),
        )]));
    }
    for r in &today {
        out.push(Drawn::of(run_line(r, m, w, roomy)));
    }
    if !earlier.is_empty() {
        let spent: f64 = earlier
            .iter()
            .map(|r| r["cost"].as_f64().unwrap_or(0.0))
            .sum();
        out.push(Drawn::of(vec![Span::styled(
            format!(" ▸ {} earlier this week · ${spent:.2}", earlier.len()),
            Tone::Dim.style(theme),
        )]));
    }
    out.push(Drawn::default());

    // Its next runs.
    let upcoming: Vec<i64> = item
        .data
        .get("upcoming")
        .and_then(Value::as_array)
        .map(|u| u.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();
    out.push(Drawn::of(kit::label("Next", Tone::Dim, None, w, theme)));
    let mut chips = vec![Span::raw(" ")];
    if upcoming.is_empty() {
        let words = if b(item, "paused") {
            "paused"
        } else {
            "no more runs"
        };
        chips.push(Span::styled(words, Tone::Faint.style(theme)));
    }
    for at in upcoming {
        let chip = kit::chip(&when_words(at, &now), Tone::Working, caps, theme);
        if kit::spans_width(&chips) + kit::spans_width(&chip) > w {
            break;
        }
        chips.extend(chip);
    }
    out.push(Drawn::of(chips));
    out
}

/// A tab as the pane names it: its state's mark, its number in its tab
/// color, its name and its state in words.
fn tab_words(t: &TabModel, m: &Mission) -> Vec<Span<'static>> {
    let theme = &m.snap.theme;
    let badge = model::badge_state(t, &m.snap.rules);
    let color = t.color.map(Color::from).unwrap_or(theme.bright);
    let mut out = vec![
        kit::mark(Mark::of_badge(badge), m.snap.frame, theme),
        Span::raw(" "),
        Span::styled(
            t.index.to_string(),
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {}", t.name), Tone::Text.bold(theme)),
    ];
    let state = model::state_only(t, &m.snap.rules);
    if !state.is_empty() {
        out.push(Span::styled(format!("  {state}"), Tone::Dim.style(theme)));
    }
    out
}

/// A run's line: its time right-aligned, its mark, the reply's first line
/// stretching, and its tab, length and cost at the right; the tab goes on
/// a narrow pane.
fn run_line(r: &Value, m: &Mission, w: usize, wide: bool) -> Vec<Span<'static>> {
    let theme = &m.snap.theme;
    let at = r["at"].as_i64().unwrap_or(0);
    let time = jiff::Timestamp::from_second(at)
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
        .to_zoned(m.snap.tz.clone())
        .strftime("%-I:%M %p")
        .to_string();
    let outcome = r["outcome"].as_str().unwrap_or("");
    let tone = tone_of(outcome);
    let words = r["words"].as_str().unwrap_or("").to_string();
    let tab = r["tab"].as_str().unwrap_or("");
    let ran = matches!(outcome, "Ran" | "Late" | "Failed");
    let mut right = Vec::new();
    if wide && !tab.is_empty() {
        right.push(format!("tab {tab}"));
    }
    if ran {
        right.push(length(r["secs"].as_i64().unwrap_or(0)));
        right.push(format!("${:.2}", r["cost"].as_f64().unwrap_or(0.0)));
    }
    let right = right.join("  ");
    let placed = kit::columns(
        &[
            Col::Fixed {
                min: 8,
                natural: 8,
                cap: 8,
            },
            Col::Fixed {
                min: 1,
                natural: 1,
                cap: 1,
            },
            Col::Stretch {
                weight: 1,
                min: 4,
                max: None,
            },
            Col::Right {
                natural: width(&right),
                cap: 24,
            },
        ],
        w.saturating_sub(1),
        1,
        &[3],
    );
    let mut line = Vec::new();
    if let Some(c) = placed[0] {
        let shown = format!("{time:>8}");
        kit::place(
            &mut line,
            vec![Span::styled(shown, Tone::Dim.style(theme))],
            c,
        );
    }
    if let Some(c) = placed[1] {
        kit::place(
            &mut line,
            vec![Span::styled(
                r["mark"].as_str().unwrap_or("").to_string(),
                tone.bold(theme),
            )],
            c,
        );
    }
    if let Some(c) = placed[2] {
        let style = if outcome == "Ran" || outcome == "Late" {
            Tone::Text.style(theme)
        } else {
            tone.style(theme)
        };
        kit::place(&mut line, vec![Span::styled(cut(&words, c.1), style)], c);
    }
    if let Some(c) = placed[3] {
        let rw = width(&right).min(c.1);
        kit::place(
            &mut line,
            vec![Span::styled(cut(&right, rw), Tone::Dim.style(theme))],
            (c.0 + c.1 - rw, rw),
        );
    }
    line
}

/// The view's own keys for the footer, after moving, Enter and `m`:
/// n, and on a narrow screen i for the chosen routine; the rest are the
/// pane's keycaps.
pub fn hints(narrow: bool) -> &'static [(&'static str, &'static str)] {
    if narrow {
        &[("n", "new"), ("i", "the routine")]
    } else {
        &[("n", "new")]
    }
}

/// A routine for the tests elsewhere.
#[cfg(test)]
pub(crate) fn tests_routine() -> Routine {
    tests::routine("r1", "Triage", super::schedule::Schedule::Once { at: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routines::routine::Outcome;
    use crate::routines::schedule::{Days, Schedule};

    pub(crate) fn routine(id: &str, name: &str, schedule: Schedule) -> Routine {
        Routine {
            id: id.into(),
            name: name.into(),
            dir: "/work/api".into(),
            when: String::new(),
            schedule,
            tab_policy: TabPolicy::SameTab,
            when_busy: WhenBusy::Wait,
            when_missed: WhenMissed::OnWaking,
            prompt: "Sort the new issues.".into(),
            model: String::new(),
            paused: false,
            session: String::new(),
            session_dir: String::new(),
            handled: 0,
        }
    }

    #[test]
    fn recurring_first_then_once_then_those_that_ran_once_with_their_week() {
        let now: Zoned = "2026-11-04T14:30:00[America/New_York]".parse().unwrap();
        let secs = now.timestamp().as_second();
        let mut daily = routine(
            "r1",
            "Triage",
            Schedule::Days {
                days: Days::WEEKDAYS,
                at: 8 * 60 + 45,
            },
        );
        daily.handled = secs - 3600;
        let soon = routine("r2", "Ship notes", Schedule::Once { at: secs + 600 });
        let mut gone = routine("r3", "Old", Schedule::Once { at: secs - 600 });
        gone.handled = secs - 600;
        let runs = HashMap::from([(
            "r1".to_string(),
            vec![
                Run {
                    due: secs - 86_400,
                    ran: secs - 86_400,
                    tab: "9".into(),
                    secs: 40,
                    cost: 0.1,
                    outcome: Outcome::Ran,
                    words: "Sorted four".into(),
                },
                Run {
                    due: secs - 3600,
                    ran: 0,
                    tab: String::new(),
                    secs: 0,
                    cost: 0.0,
                    outcome: Outcome::Missed,
                    words: "asleep".into(),
                },
            ],
        )]);
        let live = HashMap::new();
        let f = Facts {
            now: now.clone(),
            tabs: vec![("r1".into(), Some(TabId(9)), "9".into())],
            live: &live,
            runs: &runs,
        };
        let items = items(&[gone, soon, daily], &f);
        let names: Vec<&str> = items.iter().map(|i| s(i, "name")).collect();
        assert_eq!(names, ["Triage", "Ship notes", "Old"]);
        assert_eq!(items[0].target, Target::Tab(TabId(9)));
        assert_eq!(s(&items[0], "nextWords"), "tomorrow 8:45 AM");
        assert_eq!(items[0].data["week"][1], json!(["✓"]));
        assert_eq!(items[0].data["week"][2], json!(["–"]));
        assert_eq!(items[0].data["last"]["mark"], "–");
        assert_eq!(s(&items[1], "nextWords"), "2:40 PM");
        assert_eq!(section_of(&items[2]), 2);
        assert!(items[2].detail.contains("ran once"), "{}", items[2].detail);
    }
}
