//! The Reminders view (UX 2.0 board 28): every tab's reminder, which the
//! sidebar lists one by one under Reminders, on a month. Its items are the
//! tabs with a reminder set (`items`), in time order, each its tab's
//! target, so Enter opens the tab's menu as anywhere else.
//!
//! With UX 2.0 on, it is drawn as the board draws it (`month`): the month's
//! name, the weekdays across, six weeks each a band headed by a dashed rule
//! with each day's number on it (today's red, the chosen day's lit, days of
//! the months around faint), each reminder an event in its day in its tab's
//! state's tone with the tab's number in its tab color; beside it, the
//! chosen day's reminder with its keys as keycaps (g go there, e change, x
//! clear), then this month's reminders and the later ones, a reminder come
//! due showing a "due" chip in place of its time; under the month, the
//! twelve months ahead as chips counting their reminders. The keys move a
//! chosen day (`Mission::reminders_day`), the month following it. Narrower
//! than `BESIDE_AT`, the pane goes under the month and its lists behind
//! `i`; without UX 2.0 the view is a plain list. Drawing reads nothing but
//! the items and the tabs the snapshot holds.

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use serde_json::{Value, json};

use super::views::View;
use super::{HitBox, Mission, Spot};
use crate::items::{Item, State, Target};
use crate::mux::input::{Key, KeyCode, Mods};
use crate::sidebar::model::{self, Rules, TabModel};
use crate::sidebar::render::Line;
use crate::sidebar::text::{self, cut, width};
use crate::ui::kit::{self, Col, Mark, Tone};

use super::remind;

/// Narrower than this, the pane goes under the month.
pub const BESIDE_AT: usize = 140;
/// Narrower than this, the months ahead drop from the end, then the
/// reminders' names in their days.
const ROOMY_AT: usize = 160;
/// The pane's share of 160 columns (36 of the board's 125), and its least.
const PANE_SHARE: usize = 46;
const PANE_LEAST: usize = 34;
/// The least a day's column is.
const DAY_LEAST: usize = 6;
/// What the view says with no reminder set.
pub const EMPTY: &str =
    "No reminders. Set one from a tab's menu or M-p's Remind me about this tab.";

/// The view's items: one per tab with a reminder, by its time, its data
/// the time (seconds), the note, the tab's number and name and whether it
/// came due.
pub fn items(tabs: &[TabModel], r: &Rules) -> Vec<Item> {
    let mut set: Vec<&TabModel> = tabs.iter().filter(|t| t.remind_at.is_some()).collect();
    set.sort_by_key(|t| (t.remind_at, t.index));
    set.into_iter()
        .map(|t| {
            let at = t.remind_at.unwrap_or(0);
            let due = model::reminder_due(t, r);
            let note = t.remind_note.clone().unwrap_or_default();
            let mut item = Item::new(
                format!("remind:{}", t.id.0),
                format!("{} {}", t.index, t.name),
            )
            .data(json!({
                "at": at,
                "note": note,
                "index": t.index,
                "name": t.name,
                "due": due,
            }));
            item.detail = if due {
                format!("came due · {note}")
            } else {
                format!("{} · {note}", text::until(at, r.now_ms))
            }
            .trim_end_matches(" · ")
            .to_string();
            item.time = Some(at * 1000);
            item.target = Target::Tab(t.id);
            item.state = if due { State::Attention } else { State::Idle };
            item
        })
        .collect()
}

fn at_of(item: &Item) -> i64 {
    item.data.get("at").and_then(Value::as_i64).unwrap_or(0)
}

fn due(item: &Item) -> bool {
    item.data.get("due").and_then(Value::as_bool) == Some(true)
}

fn index_of(item: &Item) -> usize {
    item.data.get("index").and_then(Value::as_u64).unwrap_or(0) as usize
}

fn name_of(item: &Item) -> &str {
    item.data.get("name").and_then(Value::as_str).unwrap_or("")
}

fn note_of(item: &Item) -> &str {
    item.data.get("note").and_then(Value::as_str).unwrap_or("")
}

fn zoned(at: i64, tz: &TimeZone) -> jiff::Zoned {
    Timestamp::from_second(at)
        .unwrap_or(Timestamp::UNIX_EPOCH)
        .to_zoned(tz.clone())
}

/// The day a reminder is on.
pub fn day_of(item: &Item, tz: &TimeZone) -> Date {
    zoned(at_of(item), tz).date()
}

/// The bar's right with the view in front: "6 reminders · 1 came due".
pub fn bar_words(view: &View) -> String {
    let n = view.data.items.len();
    let came = view.data.items.iter().filter(|i| due(i)).count();
    let mut words = format!("{n} reminder{}", if n == 1 { "" } else { "s" });
    if came > 0 {
        words.push_str(&format!(" · {came} came due"));
    }
    words
}

/// Today, as the snapshot has it.
pub fn today(m: &Mission) -> Date {
    zoned(m.snap.now_secs, &m.snap.tz).date()
}

/// The chosen day: where the keys have moved it, else today.
pub fn chosen_day(m: &Mission) -> Date {
    m.reminders_day.unwrap_or_else(|| today(m))
}

/// The chosen day's reminder, by its place in the view: the cursor's when
/// it is on that day, else the day's first.
pub fn chosen(m: &Mission, view: &View) -> Option<usize> {
    let day = chosen_day(m);
    let tz = &m.snap.tz;
    let on_day = |i: &usize| day_of(&view.data.items[*i], tz) == day;
    usize::try_from(m.cursor)
        .ok()
        .filter(|c| *c < view.data.items.len())
        .filter(on_day)
        .or_else(|| (0..view.data.items.len()).find(on_day))
}

/// The day `by` days from `day`, or the same day a month or more on
/// (`months`), its day of the month kept within the month.
pub fn step(day: Date, days: i64, months: i32) -> Date {
    let moved = if months != 0 {
        let first = day.first_of_month();
        let target = first
            .checked_add(jiff::Span::new().months(months))
            .unwrap_or(first);
        let last = target.last_of_month().day();
        Date::new(target.year(), target.month(), day.day().min(last)).unwrap_or(target)
    } else {
        day
    };
    moved
        .checked_add(jiff::Span::new().days(days))
        .unwrap_or(moved)
}

/// Whether the view is drawn as a month: UX 2.0 on.
pub fn is_month(m: &Mission) -> bool {
    m.snap.rules.ux2
}

/// A row of the month: its spans and where clicks on it land.
type GridRow = (Vec<Span<'static>>, Vec<(usize, usize, Spot)>);

/// A line of the pane: its spans and where a click on it lands, by
/// columns from the pane's left.
#[derive(Default)]
struct PaneLine {
    spans: Vec<Span<'static>>,
    hits: Vec<(usize, usize, Spot)>,
}

impl PaneLine {
    fn of(spans: Vec<Span<'static>>) -> PaneLine {
        PaneLine {
            spans,
            hits: Vec::new(),
        }
    }
}

/// The month, its pane and the months ahead, from row `lines.len()`, and
/// where clicks land.
pub fn month(
    view: &View,
    m: &Mission,
    cols: usize,
    rows: usize,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
) {
    let theme = &m.snap.theme;
    let tz = &m.snap.tz;
    let top = lines.len();
    let body = rows.saturating_sub(1 + top);
    let beside = cols >= BESIDE_AT;
    let pane_w = if beside {
        kit::beside(cols, PANE_SHARE, PANE_LEAST, 7 * (DAY_LEAST + 1))
    } else {
        0
    };
    let month_w = if pane_w > 0 { cols - pane_w - 3 } else { cols };
    let day = chosen_day(m);
    let today = today(m);
    let chosen = chosen(m, view);
    let items = &view.data.items;

    // The month's rows: its name, the weekdays, six weeks, the months ahead.
    let ahead_rows = 4;
    let fixed = 2;
    // Under the month, the chosen day's reminder and its keys keep their
    // rows: the months ahead give way first, then the weeks but the chosen
    // day's.
    let (band, ahead, weeks) = if beside {
        let band = body.saturating_sub(fixed + ahead_rows) / 6;
        if band >= 2 {
            (band, true, 6)
        } else {
            ((body.saturating_sub(fixed) / 6).max(2), false, 6)
        }
    } else {
        let side = pane_lines(view, m, cols.saturating_sub(2), false).len() + 2;
        let room = body.saturating_sub(fixed + side);
        if room >= 6 * 2 + ahead_rows {
            (((room - ahead_rows) / 6).clamp(2, 4), true, 6)
        } else if room >= 6 * 2 {
            ((room / 6).clamp(2, 4), false, 6)
        } else {
            (room.clamp(2, 4), false, 1)
        }
    };
    let mut grid: Vec<GridRow> = Vec::new();

    // The month's name, and its keys at the right.
    let name = format!(
        "{} {}",
        remind::MONTHS[(day.month() - 1) as usize],
        day.year()
    );
    let mut head = kit::label(&name, Tone::Text, None, month_w, theme);
    head.truncate(2);
    let keys = kit::hints(&[("H L", "month"), ("t", "today")], theme);
    let gap = month_w.saturating_sub(kit::spans_width(&head) + kit::spans_width(&keys) + 1);
    if gap > 1 {
        head.push(Span::raw(" ".repeat(gap)));
        head.extend(keys);
    }
    grid.push((kit::fit(head, month_w, Style::new()), Vec::new()));

    // The weekdays across, a column each.
    let day_cols: Vec<(usize, usize)> = kit::columns(
        &[Col::Stretch {
            weight: 1,
            min: DAY_LEAST,
            max: None,
        }; 7],
        month_w.saturating_sub(1),
        1,
        &[],
    )
    .into_iter()
    .map(|c| c.map(|(x, w)| (x + 1, w)).unwrap_or((0, 0)))
    .collect();
    let mut weekdays = Vec::new();
    for (i, wd) in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
        .iter()
        .enumerate()
    {
        kit::place(
            &mut weekdays,
            vec![Span::styled(*wd, Tone::Dim.bold(theme))],
            day_cols[i],
        );
    }
    grid.push((kit::fit(weekdays, month_w, Style::new()), Vec::new()));

    // Six weeks from the Monday on or before the first.
    let first = day.first_of_month();
    let start = first
        .checked_sub(jiff::Span::new().days(first.weekday().to_monday_zero_offset() as i64))
        .unwrap_or(first);
    let narrow_names = cols < ROOMY_AT;
    let border = Style::new().fg(kit::border(theme));
    let red = Style::new().fg(Color::Indexed(196));
    // One week only: the chosen day's.
    let from = if weeks == 1 {
        (day.since(start).map_or(0, |d| d.get_days()) / 7) as usize
    } else {
        0
    };
    for week in from..from + weeks {
        let days: Vec<Date> = (0..7)
            .map(|d| step(start, (week * 7 + d) as i64, 0))
            .collect();
        let row_y = grid.len();
        // The rule with each day's number on it.
        let mut rule = Vec::new();
        let mut rule_hits = Vec::new();
        for (d, date) in days.iter().enumerate() {
            let (x, w) = day_cols[d];
            if w == 0 {
                continue;
            }
            let number = date.day().to_string();
            let is_today = *date == today;
            let piece = if is_today { red } else { border };
            let number_style = if *date == day {
                Style::new()
                    .bg(kit::Surface::Chosen.color(theme))
                    .fg(theme.bright)
                    .add_modifier(Modifier::BOLD)
            } else if is_today {
                red.add_modifier(Modifier::BOLD)
            } else if date.month() != day.month() {
                Tone::Faint.style(theme)
            } else {
                Tone::Dim.style(theme)
            };
            let spans = vec![
                Span::styled("┄", piece),
                Span::styled(number.clone(), number_style),
                Span::styled(
                    "┄".repeat(w.saturating_sub(1 + number.len())),
                    if is_today { red } else { border },
                ),
            ];
            kit::place(&mut rule, spans, (x, w));
            rule_hits.push((x, x + w - 1, Spot::Day(*date)));
        }
        grid.push((kit::fit(rule, month_w, Style::new()), rule_hits));
        // Each day's reminders, a row each.
        let per_day: Vec<Vec<usize>> = days
            .iter()
            .map(|date| {
                (0..items.len())
                    .filter(|i| day_of(&items[*i], tz) == *date)
                    .collect()
            })
            .collect();
        for r in 0..band.saturating_sub(1) {
            let mut line = Vec::new();
            let mut row_hits = Vec::new();
            for (d, date) in days.iter().enumerate() {
                let (x, w) = day_cols[d];
                if w == 0 {
                    continue;
                }
                let these = &per_day[d];
                let room = band - 1;
                let more = these.len() > room && r == room - 1;
                if more {
                    let n = these.len() - (room - 1);
                    kit::place(
                        &mut line,
                        vec![Span::styled(format!(" +{n}"), Tone::Dim.style(theme))],
                        (x, w),
                    );
                    row_hits.push((x, x + w - 1, Spot::Day(*date)));
                    continue;
                }
                let Some(&i) = these.get(r) else {
                    row_hits.push((x, x + w - 1, Spot::Day(*date)));
                    continue;
                };
                // Chosen, or lit under the mouse.
                let lit = chosen == Some(i) || m.hover == Some(Spot::Index(i as isize));
                let event = event_line(&items[i], m, w, narrow_names, lit);
                kit::place(&mut line, event, (x, w));
                row_hits.push((x, x + w - 1, Spot::Index(i as isize)));
            }
            grid.push((kit::fit(line, month_w, Style::new()), row_hits));
        }
        let _ = row_y;
    }

    // The months ahead: twelve from this one, the one shown lit.
    if ahead {
        grid.push((vec![kit::rule(month_w, theme)], Vec::new()));
        grid.push((
            kit::label("The months ahead", Tone::Dim, None, month_w, theme),
            Vec::new(),
        ));
        let this = today.first_of_month();
        let months: Vec<Date> = (0..12).map(|i| step(this, 0, i)).collect();
        let mut specs: Vec<Col> = months
            .iter()
            .map(|_| Col::Stretch {
                weight: 1,
                min: 8,
                max: None,
            })
            .collect();
        specs.push(Col::Right { natural: 1, cap: 1 });
        let drop: Vec<usize> = if cols < ROOMY_AT {
            (0..12).rev().chain([12]).collect()
        } else {
            vec![12]
        };
        let placed = kit::columns(&specs, month_w.saturating_sub(1), 1, &drop);
        let mut years = Vec::new();
        let mut chips = Vec::new();
        let mut chip_hits = Vec::new();
        for (i, month) in months.iter().enumerate() {
            let Some((x, w)) = placed[i] else {
                continue;
            };
            let x = x + 1;
            if i == 0 || month.month() == 1 {
                kit::place(
                    &mut years,
                    vec![Span::styled(
                        month.year().to_string(),
                        Tone::Faint.style(theme),
                    )],
                    (x, w),
                );
            }
            let n = items
                .iter()
                .filter(|it| day_of(it, tz).first_of_month() == *month)
                .count();
            let words = if n > 0 {
                format!("{} {n}", remind::short_month(month.month()))
            } else {
                remind::short_month(month.month()).to_string()
            };
            let shown = month.year() == day.year() && month.month() == day.month();
            let tone = if shown {
                Tone::Accent
            } else if n > 0 {
                Tone::Working
            } else {
                Tone::Faint
            };
            let mut chip = kit::chip(&words, tone, m.snap.caps, theme);
            if shown {
                chip = chip
                    .into_iter()
                    .map(|s| {
                        let style = s.style.add_modifier(Modifier::BOLD);
                        Span::styled(s.content, style)
                    })
                    .collect();
            }
            kit::place(&mut chips, chip, (x, w));
            chip_hits.push((x, x + w - 1, Spot::Day(*month)));
        }
        if let Some((x, _)) = placed[12] {
            let x = x + 1;
            kit::place(
                &mut chips,
                vec![Span::styled("›", Tone::Accent.bold(theme))],
                (x, 1),
            );
            chip_hits.push((x, x, Spot::Day(step(this, 0, 12))));
        }
        grid.push((kit::fit(years, month_w, Style::new()), Vec::new()));
        grid.push((kit::fit(chips, month_w, Style::new()), chip_hits));
    }

    // The pane: beside the month, else under it.
    let pane = pane_lines(
        view,
        m,
        if beside {
            pane_w
        } else {
            cols.saturating_sub(2)
        },
        beside || m.reminders_lists,
    );
    let side_hint = !beside && !m.reminders_lists;
    let mut out: Vec<GridRow> = Vec::new();
    if pane_w > 0 {
        let height = body.max(grid.len());
        for r in 0..height {
            let (mut spans, mut row_hits) = grid
                .get(r)
                .cloned()
                .unwrap_or_else(|| (vec![Span::raw(" ".repeat(month_w))], Vec::new()));
            spans = kit::fit(spans, month_w, Style::new());
            spans.push(Span::raw(" "));
            spans.push(kit::rule_v(theme));
            spans.push(Span::raw(" "));
            let x0 = month_w + 3;
            if let Some(p) = pane.get(r) {
                spans.extend(kit::fit(p.spans.clone(), pane_w, Style::new()));
                row_hits.extend(p.hits.iter().map(|(a, b, s)| (a + x0, b + x0, *s)));
            }
            out.push((spans, row_hits));
        }
    } else {
        out.extend(grid);
        out.push((vec![kit::rule(cols, theme)], Vec::new()));
        for p in pane {
            let mut spans = vec![Span::raw(" ")];
            spans.extend(p.spans);
            out.push((
                spans,
                p.hits.iter().map(|(a, b, s)| (a + 1, b + 1, *s)).collect(),
            ));
        }
        if side_hint {
            out.push((
                {
                    let mut s = vec![Span::raw(" ")];
                    s.extend(kit::hints(&[("i", "this month and later")], theme));
                    s
                },
                Vec::new(),
            ));
        }
    }
    for (r, (spans, row_hits)) in out.into_iter().take(body).enumerate() {
        let y = (top + r) as u16;
        for (x0, x1, spot) in row_hits {
            hits.push(HitBox {
                y,
                x0: x0 as u16,
                x1: x1 as u16,
                spot,
            });
        }
        lines.push(Line {
            spans: kit::fit(spans, cols, Style::new()),
            fill: None,
        });
    }
}

/// The tab an item is the reminder of, among those the snapshot holds.
fn tab_of<'a>(item: &Item, m: &'a Mission) -> Option<&'a TabModel> {
    let Target::Tab(id) = item.target else {
        return None;
    };
    m.snap
        .all_tabs
        .iter()
        .chain(m.snap.tabs.iter())
        .find(|t| t.id == id)
}

/// The tone a reminder is drawn in: due, the needs-you tone; else its
/// tab's state's.
fn tone_of(item: &Item, m: &Mission) -> Tone {
    if due(item) {
        return Tone::Needs;
    }
    match tab_of(item, m) {
        Some(t) => {
            Mark::of_badge(model::badge_state(t, &m.snap.rules))
                .look()
                .1
        }
        None => Tone::Dim,
    }
}

/// The tab's number in its tab color, bold.
fn number_style(item: &Item, m: &Mission) -> Style {
    let color = tab_of(item, m)
        .and_then(|t| t.color)
        .map(Color::from)
        .unwrap_or(m.snap.theme.bright);
    Style::new().fg(color).add_modifier(Modifier::BOLD)
}

/// A reminder in its day: an event a row tall, the tab's number in its
/// tab color, then its name (left out when the screen is narrow).
fn event_line(
    item: &Item,
    m: &Mission,
    w: usize,
    narrow: bool,
    chosen: bool,
) -> Vec<Span<'static>> {
    let theme = &m.snap.theme;
    let number = index_of(item).to_string();
    let title = if narrow {
        number.clone()
    } else {
        format!("{number} {}", name_of(item))
    };
    let mut line = kit::event(&title, "", tone_of(item, m), w, 1, false, chosen, theme)
        .into_iter()
        .next()
        .unwrap_or_default();
    // The number takes its tab's color, on the event's own background.
    if let Some(words) = line.get(1).cloned()
        && words.content.starts_with(&number)
    {
        let bg = words.style.bg;
        let rest = words.content[number.len()..].to_string();
        let mut style = number_style(item, m);
        style.bg = bg;
        line.splice(
            1..2,
            [
                Span::styled(number.clone(), style),
                Span::styled(rest, words.style),
            ],
        );
    }
    line
}

/// What the pane shows: the chosen day's reminder with its keycaps, then,
/// when `lists`, this month's reminders and the later ones.
fn pane_lines(view: &View, m: &Mission, w: usize, lists: bool) -> Vec<PaneLine> {
    let theme = &m.snap.theme;
    let tz = &m.snap.tz;
    let caps = m.snap.caps;
    let items = &view.data.items;
    let day = chosen_day(m);
    let now = zoned(m.snap.now_secs, tz);
    let mut out: Vec<PaneLine> = Vec::new();
    let title = format!(
        "{:?}, {} {}",
        day.weekday(),
        remind::MONTHS[(day.month() - 1) as usize],
        day.day()
    );
    let lit = |key: char| match (m.hover, m.focus) {
        (Some(Spot::Key(k)), _) | (_, Some(super::Focus::Key(k))) => {
            k == Key::new(KeyCode::Char(key), Mods::NONE)
        }
        _ => false,
    };
    let keycaps = |pairs: &[(char, &str)]| -> PaneLine {
        let mut line = PaneLine::default();
        for (key, what) in pairs {
            let x = kit::spans_width(&line.spans);
            let cap = kit::keycap(&key.to_string(), what, lit(*key), caps, theme);
            let cw = kit::spans_width(&cap);
            line.spans.extend(cap);
            line.hits.push((
                x,
                (x + cw).saturating_sub(2),
                Spot::Key(Key::new(KeyCode::Char(*key), Mods::NONE)),
            ));
        }
        line
    };
    match chosen(m, view) {
        Some(i) => {
            let item = &items[i];
            let at = at_of(item);
            let facts = if due(item) {
                let mut f = kit::chip("due", Tone::Needs, caps, theme);
                f.push(Span::styled(
                    format!("came due {}", remind::said_from(at, &now)),
                    Tone::Dim.style(theme),
                ));
                f
            } else {
                vec![Span::styled(
                    format!(
                        "{} · {}",
                        zoned(at, tz).strftime("%-I:%M %p"),
                        remind::left(at, m.snap.now_secs)
                    ),
                    Tone::Dim.style(theme),
                )]
            };
            out.extend(
                kit::pane(&title, facts, w, theme)
                    .into_iter()
                    .map(PaneLine::of),
            );
            let mut tab = Vec::new();
            match tab_of(item, m) {
                Some(t) => {
                    let badge = model::badge_state(t, &m.snap.rules);
                    tab.push(kit::mark(Mark::of_badge(badge), m.snap.frame, theme));
                    tab.push(Span::raw(" "));
                    tab.push(Span::styled(
                        index_of(item).to_string(),
                        number_style(item, m),
                    ));
                    tab.push(Span::styled(
                        format!(" {}", name_of(item)),
                        Tone::Text.bold(theme),
                    ));
                    let state = model::state_only(t, &m.snap.rules);
                    if !state.is_empty() {
                        tab.push(Span::styled(format!("  {state}"), Tone::Dim.style(theme)));
                    }
                }
                None => {
                    tab.push(Span::styled(
                        index_of(item).to_string(),
                        number_style(item, m),
                    ));
                    tab.push(Span::styled(
                        format!(" {}", name_of(item)),
                        Tone::Text.bold(theme),
                    ));
                }
            }
            out.push(PaneLine::of(kit::fit(tab, w, Style::new())));
            let note = note_of(item);
            if !note.is_empty() {
                out.extend(
                    text::wrap(note, w)
                        .into_iter()
                        .map(|l| PaneLine::of(vec![Span::styled(l, Tone::Text.style(theme))])),
                );
            }
            out.push(PaneLine::default());
            out.push(keycaps(&[
                ('g', "Go there"),
                ('e', "Change"),
                ('x', "Clear"),
            ]));
        }
        None => {
            let facts = vec![Span::styled(
                if items.is_empty() {
                    "no reminders".to_string()
                } else {
                    "nothing this day".to_string()
                },
                Tone::Dim.style(theme),
            )];
            out.extend(
                kit::pane(&title, facts, w, theme)
                    .into_iter()
                    .map(PaneLine::of),
            );
            if items.is_empty() {
                out.extend(
                    text::wrap(EMPTY, w)
                        .into_iter()
                        .map(|l| PaneLine::of(vec![Span::styled(l, Tone::Dim.style(theme))])),
                );
            }
            out.push(PaneLine::default());
            out.push(keycaps(&[('e', "Remind me about a tab this day")]));
        }
    }
    if !lists || items.is_empty() {
        return out;
    }
    let month = day.first_of_month();
    let this: Vec<usize> = (0..items.len())
        .filter(|i| day_of(&items[*i], tz).first_of_month() == month)
        .collect();
    let later: Vec<usize> = (0..items.len())
        .filter(|i| day_of(&items[*i], tz).first_of_month() > month)
        .collect();
    let chosen = chosen(m, view);
    for (label, these) in [("This month", this), ("Later", later)] {
        if these.is_empty() {
            continue;
        }
        out.push(PaneLine::default());
        out.push(PaneLine::of(kit::label(
            label,
            Tone::Dim,
            Some(these.len()),
            w,
            theme,
        )));
        for i in these {
            out.push(list_row(&items[i], m, w, chosen == Some(i), i));
        }
    }
    out
}

/// A row of This month or Later: the day, the tab's number and name, the
/// time at the right (or a "due" chip in its place).
fn list_row(item: &Item, m: &Mission, w: usize, chosen: bool, i: usize) -> PaneLine {
    let theme = &m.snap.theme;
    let tz = &m.snap.tz;
    let z = zoned(at_of(item), tz);
    let day = format!(
        "{} {} {}",
        remind::short_day(z.weekday()),
        remind::short_month(z.month()),
        z.day()
    );
    let time = z.strftime("%-I:%M %p").to_string();
    let right: Vec<Span<'static>> = if due(item) {
        kit::chip("due", Tone::Needs, m.snap.caps, theme)
    } else {
        vec![Span::styled(time, Tone::Dim.style(theme))]
    };
    let number = index_of(item).to_string();
    let cols = kit::columns(
        &[
            Col::Fixed {
                min: 0,
                natural: width(&day),
                cap: 12,
            },
            Col::Fixed {
                min: 0,
                natural: width(&number),
                cap: 4,
            },
            Col::Stretch {
                weight: 1,
                min: 4,
                max: None,
            },
            Col::Right { natural: 9, cap: 9 },
        ],
        w.saturating_sub(1),
        1,
        &[2],
    );
    let mut line = Vec::new();
    let name_style = if chosen {
        Style::new().fg(theme.bright).add_modifier(Modifier::BOLD)
    } else {
        Tone::Text.style(theme)
    };
    if let Some(c) = cols[0] {
        kit::place(
            &mut line,
            vec![Span::styled(day, Tone::Dim.style(theme))],
            (c.0 + 1, c.1),
        );
    }
    if let Some(c) = cols[1] {
        kit::place(
            &mut line,
            vec![Span::styled(number, number_style(item, m))],
            (c.0 + 1, c.1),
        );
    }
    if let Some(c) = cols[2] {
        kit::place(
            &mut line,
            vec![Span::styled(cut(name_of(item), c.1), name_style)],
            (c.0 + 1, c.1),
        );
    }
    if let Some(c) = cols[3] {
        let rw = kit::spans_width(&right);
        let x = (c.0 + 1 + c.1).saturating_sub(rw);
        kit::place(&mut line, right, (x, rw));
    }
    let mut line = kit::fit(line, w, Style::new());
    let lit = m.hover == Some(Spot::Index(i as isize));
    if chosen || lit {
        let bg = if chosen {
            kit::Surface::Chosen.color(theme)
        } else {
            kit::Surface::Panel.color(theme)
        };
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
    PaneLine {
        spans: line,
        hits: vec![(0, w.saturating_sub(1), Spot::Index(i as isize))],
    }
}

/// The view's keys, for the footer.
pub fn hints() -> &'static [(&'static str, &'static str)] {
    &[
        ("hjkl", "day, week"),
        ("H L", "month"),
        ("t", "today"),
        ("↵", "actions"),
        ("g", "go there"),
        ("e", "change"),
        ("x", "clear"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::TabId;

    fn tab(n: u32, name: &str, at: Option<i64>) -> TabModel {
        let mut t = TabModel::new(TabId(n), n as usize, name);
        t.remind_at = at;
        t.remind_note = Some(format!("about {name}"));
        t
    }

    #[test]
    fn a_reminder_per_tab_in_time_order_with_whether_it_came_due() {
        let mut r = crate::sidebar::model::tests::rules();
        r.now_ms = 1_000_000 * 1000;
        let tabs = vec![
            tab(1, "later", Some(1_000_000 + 7200)),
            tab(2, "none", None),
            tab(3, "due", Some(1_000_000 - 60)),
        ];
        let items = items(&tabs, &r);
        let names: Vec<&str> = items.iter().map(name_of).collect();
        assert_eq!(names, ["due", "later"]);
        assert!(due(&items[0]));
        assert!(!due(&items[1]));
        assert_eq!(items[0].target, Target::Tab(TabId(3)));
        assert_eq!(note_of(&items[1]), "about later");
    }

    #[test]
    fn a_month_on_keeps_the_day_within_the_month() {
        let d = Date::new(2026, 1, 31).unwrap();
        assert_eq!(step(d, 0, 1), Date::new(2026, 2, 28).unwrap());
        assert_eq!(step(d, 7, 0), Date::new(2026, 2, 7).unwrap());
        assert_eq!(step(d, 0, -2), Date::new(2025, 11, 30).unwrap());
    }
}
