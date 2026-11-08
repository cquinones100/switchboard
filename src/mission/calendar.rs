//! The Calendar view drawn as Google Calendar's Day view lays out a day
//! (lib/sidebar/calendar.js's `calendarLines`): the date and when it was
//! read, all-day events as chips in a row of their own, then an hour column
//! down the left, each event a colored block from its start to its end with
//! its title and times, events at the same time side by side, a red line
//! where it is now, and events already over faded. One you haven't said yes
//! to is drawn in outline, on the panel's color with a colored edge, as
//! Google Calendar draws it, and one that starts just as the one above it
//! in its lane ends takes the deeper shade, so two meetings back to back
//! read as two. The rows fit the day from 8 AM (or the first event) to 6 PM
//! (or the last) to the screen, up to eight to an hour.
//!
//! The view's items are its events in the order the cursor walks them, all
//! day first, then by start (`day::calendar::calendar_view`), each item's
//! `data` the event itself; the chosen one is drawn in a light blue, and a
//! click on a block chooses it. Drawing reads nothing but the items.
//!
//! With UX 2.0 on (`timeline`), the day is drawn as the board draws it:
//! three rows an hour, dashed rules, each event a block with a bar in its
//! color, past ones struck through, the next one saying how soon it starts,
//! and on a wide screen the chosen event beside the day, with its menu's
//! join and answers as buttons, who is going and its agenda.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use super::views::View;
use super::{HitBox, Spot};
use crate::day::calendar::{self, CalEvent};
use crate::sidebar::render::Line;
use crate::sidebar::text::{self, width};
use crate::ui::Theme;

/// The hours' column, the time right-aligned in all but its last two.
const GUTTER: usize = 9;
/// An event's blue, and the deeper one it takes when it starts just as the
/// one above it in its lane ends; the same for events already over.
const SOLID: [u8; 2] = [32, 25];
const PAST: [u8; 2] = [24, 23];
const CHOSEN: u8 = 75;
const NOW: u8 = 196;
/// What says the day is out of date.
const STALE: u8 = 214;

/// One cell of a row being drawn.
#[derive(Clone)]
struct Cell {
    symbol: String,
    style: Style,
}

/// A row of the screen, a cell to a column.
struct Row(Vec<Cell>);

impl Row {
    fn new(cols: usize, symbol: impl Fn(usize) -> &'static str, style: Style) -> Row {
        Row((0..cols)
            .map(|x| Cell {
                symbol: symbol(x).to_string(),
                style,
            })
            .collect())
    }

    /// Text from column `x`, up to `limit`: a wide character takes its cell
    /// and an empty one after it.
    fn put(&mut self, x: usize, text: &str, style: Style, limit: usize) {
        let limit = limit.min(self.0.len());
        let mut at = x;
        for c in text.chars() {
            let w = text::char_width(c);
            if at + w > limit {
                return;
            }
            self.0[at] = Cell {
                symbol: c.to_string(),
                style,
            };
            if w == 2 {
                self.0[at + 1] = Cell {
                    symbol: String::new(),
                    style,
                };
            }
            at += w;
        }
    }

    fn line(self) -> Line {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut text = String::new();
        let mut style = None;
        for cell in self.0 {
            if style != Some(cell.style) {
                if let Some(s) = style {
                    spans.push(Span::styled(std::mem::take(&mut text), s));
                }
                style = Some(cell.style);
            }
            text.push_str(&cell.symbol);
        }
        if let Some(s) = style {
            spans.push(Span::styled(text, s));
        }
        Line { spans, fill: None }
    }
}

fn fg(c: u8) -> Style {
    Style::new().fg(Color::Indexed(c))
}

/// Whether the view is a day to draw: today's, read, with nothing in its
/// place (how to set it up, or why there is no day, which are said whole).
pub fn is_day(view: &View) -> bool {
    view.loaded
        && view.data.setup.is_none()
        && (view.data.error.is_none() || !view.data.items.is_empty())
}

/// An event block on the grid: its rows, its lane's columns, its shade.
struct Block {
    item: usize,
    event: CalEvent,
    top: usize,
    bottom: usize,
    x: usize,
    width: usize,
    lane: usize,
    shade: usize,
}

/// What drawing the day needs beyond the view: the screen's columns, the
/// rows it has, the chosen item, the time zone and the time now.
pub struct Look<'a> {
    pub cols: usize,
    pub rows: usize,
    pub cursor: Option<usize>,
    pub tz: &'a TimeZone,
    pub now_ms: i64,
    pub theme: &'a Theme,
    /// The screen row the first line goes on, for where clicks land.
    pub top_row: usize,
    /// UX 2.0's look: the day as a timeline, the chosen event beside it.
    pub ux2: bool,
}

/// The day's lines, and where a click on each event lands.
pub fn day(view: &View, look: &Look) -> (Vec<Line>, Vec<HitBox>) {
    let (cols, tz, theme) = (look.cols, look.tz, look.theme);
    let mut lines: Vec<Line> = Vec::new();
    let mut hits = Vec::new();
    let now = Timestamp::from_millisecond(look.now_ms).unwrap_or(Timestamp::UNIX_EPOCH);
    let today = now.to_zoned(tz.clone());
    let midnight = tz
        .to_ambiguous_timestamp(today.date().to_datetime(jiff::civil::Time::midnight()))
        .compatible()
        .map(|t| t.as_millisecond())
        .unwrap_or(look.now_ms);
    let minutes =
        |at: Option<Timestamp>| at.map(|t| (t.as_millisecond() - midnight) as f64 / 60_000.0);
    let now_min = (look.now_ms - midnight) as f64 / 60_000.0;
    let events: Vec<(usize, CalEvent)> = view
        .data
        .items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            serde_json::from_value::<CalEvent>(item.data.clone())
                .ok()
                .map(|e| (i, e))
        })
        .collect();

    // The date, when it was read, and why it may be out of date.
    let mut head = Row::new(cols, |_| " ", Style::new());
    let date = today.strftime("%A, %B %-d").to_string();
    head.put(
        1,
        &date,
        Style::new().fg(theme.bright).add_modifier(Modifier::BOLD),
        cols,
    );
    let mut x = 1 + width(&date);
    if let Some(at) = view.data.at.filter(|at| *at > 1) {
        let said = format!(
            "  updated {} ago",
            text::ago(Some(at.div_euclid(1000)), look.now_ms.div_euclid(1000))
        );
        head.put(x, &said, Style::new().fg(theme.faint), cols);
        x += width(&said);
    }
    if let Some(error) = &view.data.error {
        head.put(x, &format!("  {error}"), fg(STALE), cols);
    }
    lines.push(head.line());

    // All-day events as chips in a row of their own, above the hours.
    let area = cols.saturating_sub(GUTTER);
    let chip = |chosen: bool, e: &CalEvent| {
        if chosen {
            Style::new()
                .bg(Color::Indexed(CHOSEN))
                .fg(Color::Indexed(232))
                .add_modifier(Modifier::BOLD)
        } else if e.going() {
            Style::new()
                .bg(Color::Indexed(SOLID[0]))
                .fg(Color::Indexed(255))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().bg(theme.panel).fg(Color::Indexed(SOLID[0]))
        }
    };
    let all_day: Vec<&(usize, CalEvent)> = events.iter().filter(|(_, e)| e.all_day).collect();
    if !all_day.is_empty() {
        let mut row = Row::new(cols, |_| " ", Style::new());
        row.put(
            0,
            &format!("{:>w$}", "all day", w = GUTTER - 2),
            Style::new().fg(theme.dim),
            GUTTER,
        );
        let mut x = GUTTER;
        for (i, e) in all_day {
            let room = 30.min(cols.saturating_sub(x + 3)).max(4);
            let label = format!(" {} ", text::cut(&e.title, room));
            row.put(x, &label, chip(look.cursor == Some(*i), e), cols);
            let w = width(&label);
            if x < cols {
                hits.push(HitBox {
                    y: (look.top_row + lines.len()) as u16,
                    x0: x as u16,
                    x1: (x + w).min(cols).saturating_sub(1) as u16,
                    spot: Spot::Index(*i as isize),
                });
            }
            x += w + 1;
        }
        lines.push(row.line());
    }
    lines.push(Row::new(cols, |_| " ", Style::new()).line());
    if look.ux2 {
        timeline(&events, look, midnight, now_min, &mut lines, &mut hits);
        return (lines, hits);
    }

    // As many rows to an hour as the screen has room for, up to eight, so
    // the day fills it as Google Calendar's does; an hour a row at least.
    let timed: Vec<&(usize, CalEvent)> = events.iter().filter(|(_, e)| !e.all_day).collect();
    let starts: Vec<f64> = timed
        .iter()
        .filter_map(|(_, e)| minutes(e.starts(tz)))
        .collect();
    let ends: Vec<f64> = timed
        .iter()
        .filter_map(|(_, e)| minutes(e.ends(tz)))
        .collect();
    let rows = 4.max(look.rows.saturating_sub(lines.len()));
    let from = (starts
        .iter()
        .copied()
        .fold((8.0f64 * 60.0).min(now_min), f64::min)
        / 60.0)
        .floor()
        .max(0.0) as i64;
    let to = ((ends
        .iter()
        .copied()
        .fold((18.0 * 60.0f64).max(now_min + 1.0), f64::max)
        / 60.0)
        .ceil() as i64)
        .min(24);
    let hours = (to - from).max(1);
    let per_hour = (rows as i64 / hours).clamp(1, 8);
    let step = 60.0 / per_hour as f64;
    // A day longer than the screen even by the hour starts an hour before now.
    let first = if (hours as f64 * 60.0 / step) <= rows as f64 {
        from
    } else {
        from.max(((now_min / 60.0).floor() as i64 - 1).min(to - rows as i64))
    };
    let count = (rows as f64)
        .min(((to - first) as f64 * 60.0 / step).ceil())
        .max(0.0) as usize;
    let slot = |m: f64| (m - first as f64 * 60.0) / step;

    // Events that overlap share the width, each in the first lane free, as
    // Google Calendar places them.
    let mut placed: Vec<Block> = timed
        .iter()
        .filter_map(|(i, e)| {
            let start = minutes(e.starts(tz))?;
            let end = minutes(e.ends(tz)).unwrap_or(start + 30.0);
            let top = slot(start).round().max(0.0) as usize;
            let bottom = (slot(end).round().max(0.0) as usize).max(top + 1);
            (slot(end).round() > 0.0 && top < count).then(|| Block {
                item: *i,
                event: e.clone(),
                top,
                bottom,
                x: GUTTER,
                width: area,
                lane: 0,
                shade: 0,
            })
        })
        .collect();
    lanes(&mut placed, GUTTER, area);
    // An event that starts on the row the one above it in the same place
    // ends takes the other shade of that one.
    shades(&mut placed);

    let now_row = slot(now_min).floor();
    let top_row = look.top_row + lines.len();
    for b in &placed {
        let w = b.width.saturating_sub(1);
        if w == 0 {
            continue;
        }
        for r in b.top..b.bottom.min(count) {
            hits.push(HitBox {
                y: (top_row + r) as u16,
                x0: b.x as u16,
                x1: (b.x + w - 1) as u16,
                spot: Spot::Index(b.item as isize),
            });
        }
    }
    for r in 0..count {
        let at = first as f64 * 60.0 + r as f64 * step;
        let hour = at.rem_euclid(60.0) == 0.0;
        let mut row = Row::new(
            cols,
            |x| if x >= GUTTER && hour { "┈" } else { " " },
            Style::new().fg(theme.faint),
        );
        if hour {
            let label = calendar::clock(
                Timestamp::from_millisecond(midnight + (at * 60_000.0) as i64)
                    .unwrap_or(Timestamp::UNIX_EPOCH),
                tz,
            );
            row.put(
                0,
                &format!("{label:>w$}", w = GUTTER - 2),
                Style::new().fg(theme.dim),
                GUTTER,
            );
        }
        if r as f64 == now_row {
            row.put(GUTTER - 1, "●", fg(NOW), GUTTER);
            for x in GUTTER..cols {
                row.0[x] = Cell {
                    symbol: "─".into(),
                    style: fg(NOW),
                };
            }
        }
        for b in placed
            .iter()
            .filter(|b| b.top <= r && r < b.bottom.min(count))
        {
            let e = &b.event;
            let over = e
                .ends(tz)
                .is_some_and(|t| t.as_millisecond() <= look.now_ms);
            let chosen = look.cursor == Some(b.item);
            let style = if chosen {
                Style::new()
                    .bg(Color::Indexed(CHOSEN))
                    .fg(Color::Indexed(232))
            } else if e.going() {
                let shades = if over { PAST } else { SOLID };
                Style::new()
                    .bg(Color::Indexed(shades[b.shade]))
                    .fg(Color::Indexed(if over { 250 } else { 255 }))
            } else {
                Style::new().bg(theme.panel).fg(if over {
                    theme.dim
                } else {
                    Color::Indexed(SOLID[b.shade])
                })
            };
            let w = b.width.saturating_sub(1);
            for x in b.x..(b.x + w).min(cols) {
                row.0[x] = Cell {
                    symbol: " ".into(),
                    style,
                };
            }
            // An outlined event's colored edge.
            if !e.going() && !chosen && b.x < cols {
                row.0[b.x] = Cell {
                    symbol: "▎".into(),
                    style: style.fg(Color::Indexed(SOLID[b.shade])),
                };
            }
            let line = r - b.top;
            let single = b.bottom.min(count) - b.top == 1;
            let range = match (e.starts(tz), e.ends(tz)) {
                (Some(s), Some(t)) => calendar::range(s, t, tz),
                _ => String::new(),
            };
            let words = if single {
                format!("{} · {range}", e.title)
            } else if line == 0 {
                e.title.clone()
            } else if line == 1 {
                range
            } else {
                String::new()
            };
            if !words.is_empty() {
                let style = if line == 0 {
                    style.add_modifier(Modifier::BOLD)
                } else {
                    style
                };
                row.put(
                    b.x + 1,
                    &text::cut(&words, w.saturating_sub(2)),
                    style,
                    b.x + w,
                );
            }
        }
        lines.push(row.line());
    }
    (lines, hits)
}

/// Events that overlap share the width from `x0`, each in the first lane
/// free, as Google Calendar places them.
fn lanes(placed: &mut [Block], x0: usize, area: usize) {
    placed.sort_by(|a, b| a.top.cmp(&b.top).then(b.bottom.cmp(&a.bottom)));
    let mut start = 0;
    while start < placed.len() {
        // A cluster: the blocks that overlap one another, one after another.
        let mut end = start + 1;
        let mut bottom = placed[start].bottom;
        while end < placed.len() && placed[end].top < bottom {
            bottom = bottom.max(placed[end].bottom);
            end += 1;
        }
        let mut lanes: Vec<usize> = Vec::new();
        for b in &mut placed[start..end] {
            b.lane = match lanes.iter().position(|e| *e <= b.top) {
                Some(l) => l,
                None => {
                    lanes.push(0);
                    lanes.len() - 1
                }
            };
            lanes[b.lane] = b.bottom;
        }
        let w = area / lanes.len().max(1);
        for b in &mut placed[start..end] {
            b.width = w;
            b.x = x0 + b.lane * w;
        }
        start = end;
    }
}

/// An event that starts on the row the one above it in the same place ends
/// takes the other shade of that one.
fn shades(placed: &mut [Block]) {
    for i in 0..placed.len() {
        let above = (0..i).rev().find(|&a| {
            placed[a].x == placed[i].x
                && placed[a].width == placed[i].width
                && placed[a].bottom == placed[i].top
        });
        placed[i].shade = above.map_or(0, |a| 1 - placed[a].shade);
    }
}

/// UX 2.0's timeline: the hours' labels right-aligned in these columns.
const LABEL: usize = 7;
/// Three rows to an hour, twenty minutes a row.
const PER_HOUR: i64 = 3;
/// A going event's block and bar, and the accent of one not answered or
/// chosen.
const GOING_BG: u8 = 17;
const GOING_BAR: u8 = 111;
const ACCENT: u8 = 81;

/// `9:00`, `1:15`: the board's clock, with no AM or PM.
fn short_clock(minutes: f64) -> String {
    let m = minutes.round() as i64;
    let h = (m / 60).rem_euclid(24);
    let h12 = if h % 12 == 0 { 12 } else { h % 12 };
    format!("{h12}:{:02}", m.rem_euclid(60))
}

/// Where a call is held, from its link: `Google Meet`, `Zoom`, `Teams`, else
/// the link's host.
fn place(url: &str) -> String {
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split(['/', '?'])
        .next()
        .unwrap_or("");
    if host.contains("meet.google") {
        "Google Meet".into()
    } else if host.contains("zoom.") {
        "Zoom".into()
    } else if host.contains("teams.") {
        "Teams".into()
    } else {
        host.trim_start_matches("www.").to_string()
    }
}

/// What you said to an event, as the detail says it.
fn reply(e: &CalEvent) -> &'static str {
    match e.you.as_str() {
        "going" => "going",
        "yours" => "yours",
        "maybe" => "maybe",
        "not going" | "declined" => "not going",
        _ => "not replied",
    }
}

/// The day with UX 2.0 on, as the board draws it: the hours down the left,
/// three rows an hour, each hour's label dim and right-aligned with a dashed
/// rule across; each event a block of its own shade the height of its
/// length with a bar in its color, its title bold and its time dim; one
/// already over dimmed and struck through; now a red line with its time.
/// On a screen wide enough, the chosen event is shown beside the day.
fn timeline(
    events: &[(usize, CalEvent)],
    look: &Look,
    midnight: i64,
    now_min: f64,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
) {
    let (cols, tz, theme) = (look.cols, look.tz, look.theme);
    let minutes =
        |at: Option<Timestamp>| at.map(|t| (t.as_millisecond() - midnight) as f64 / 60_000.0);
    let beside = cols >= 100;
    let day_w = if beside { cols * 12 / 25 } else { cols };
    let rule = LABEL + 1;
    let area = day_w.saturating_sub(rule + 1).max(1);
    let timed: Vec<(usize, &CalEvent, f64, f64)> = events
        .iter()
        .filter(|(_, e)| !e.all_day)
        .filter_map(|(i, e)| {
            let start = minutes(e.starts(tz))?;
            Some((*i, e, start, minutes(e.ends(tz)).unwrap_or(start + 30.0)))
        })
        .collect();
    let rows = look.rows.saturating_sub(lines.len()).max(PER_HOUR as usize);
    let from = (timed
        .iter()
        .map(|t| t.2)
        .fold((9.0f64 * 60.0).min(now_min), f64::min)
        / 60.0)
        .floor()
        .max(0.0) as i64;
    let to = ((timed
        .iter()
        .map(|t| t.3)
        .fold((18.0f64 * 60.0).max(now_min + 1.0), f64::max)
        / 60.0)
        .ceil() as i64)
        .min(24);
    // A day longer than the screen starts an hour before now, then moves
    // as little as it takes to show the chosen event, its start at least.
    let fits = (rows as i64 / PER_HOUR).max(1);
    let first = if to - from <= fits {
        from
    } else {
        let around_now = from.max(((now_min / 60.0).floor() as i64 - 1).min(to - fits));
        match timed.iter().find(|t| look.cursor == Some(t.0)) {
            Some(&(_, _, start, end)) => {
                let start_hour = (start / 60.0).floor() as i64;
                let end_hour = ((end / 60.0).ceil() as i64).max(start_hour + 1);
                let first = if start_hour < around_now {
                    start_hour
                } else if end_hour > around_now + fits {
                    (end_hour - fits).min(start_hour)
                } else {
                    around_now
                };
                first.clamp(from, (to - fits).max(from))
            }
            None => around_now,
        }
    };
    let count = rows.min(((to - first) * PER_HOUR).max(0) as usize);
    let step = 60.0 / PER_HOUR as f64;
    let slot = |m: f64| (m - first as f64 * 60.0) / step;

    let mut placed: Vec<Block> = timed
        .iter()
        .filter_map(|&(i, e, start, end)| {
            let top = slot(start).round().max(0.0) as usize;
            let bottom = (slot(end).round().max(0.0) as usize).max(top + 1);
            (slot(end).round() > 0.0 && top < count).then(|| Block {
                item: i,
                event: e.clone(),
                top,
                bottom,
                x: rule,
                width: area,
                lane: 0,
                shade: 0,
            })
        })
        .collect();
    lanes(&mut placed, rule, area);

    let faint = Style::new().fg(theme.faint);
    let mut grid: Vec<Row> = (0..count)
        .map(|r| {
            let hour = r as i64 % PER_HOUR == 0;
            let mut row = Row::new(
                cols,
                |x| {
                    if hour && x >= rule && x < rule + area {
                        "┄"
                    } else {
                        " "
                    }
                },
                faint,
            );
            if hour {
                let label = short_clock((first + r as i64 / PER_HOUR) as f64 * 60.0);
                row.put(
                    0,
                    &format!("{label:>w$}", w = LABEL),
                    Style::new().fg(theme.dim),
                    LABEL,
                );
            }
            row
        })
        .collect();

    // Now, a red line with its time.
    let now_row = slot(now_min).round();
    if now_row >= 0.0 && (now_row as usize) < count {
        let row = &mut grid[now_row as usize];
        let red = fg(NOW);
        for x in 0..LABEL {
            row.0[x].symbol = " ".into();
        }
        row.put(
            0,
            &format!("{:>w$}", short_clock(now_min), w = LABEL),
            red.add_modifier(Modifier::BOLD),
            LABEL,
        );
        for x in rule..(rule + area).min(cols) {
            row.0[x] = Cell {
                symbol: "━".into(),
                style: red,
            };
        }
    }

    // The next event to start, which says how soon.
    let next = timed
        .iter()
        .filter(|t| t.2 > now_min)
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|t| (t.0, t.2 - now_min));
    let top_row = look.top_row + lines.len();
    for b in &placed {
        let e = &b.event;
        let over = e
            .ends(tz)
            .is_some_and(|t| t.as_millisecond() <= look.now_ms);
        let chosen = look.cursor == Some(b.item);
        let (bg, bar) = if chosen {
            (theme.highlight, Color::Indexed(ACCENT))
        } else if over {
            (theme.panel, theme.faint)
        } else if e.going() {
            (Color::Indexed(GOING_BG), Color::Indexed(GOING_BAR))
        } else {
            (theme.panel, Color::Indexed(ACCENT))
        };
        let base = Style::new().bg(bg);
        let w = if b.x + b.width < rule + area {
            b.width.saturating_sub(1)
        } else {
            b.width
        };
        let end = (b.x + w).min(cols);
        let range = match (e.starts(tz), e.ends(tz)) {
            (Some(s), Some(t)) => calendar::range(s, t, tz),
            _ => String::new(),
        };
        let mut detail = range.clone();
        if let Some(j) = &e.join {
            detail.push_str(&format!(" · {}", place(j)));
        }
        if let Some((_, soon)) = next.filter(|n| n.0 == b.item) {
            detail.push_str(&format!(" · in {}", countdown(soon)));
        }
        let height = b.bottom.min(count) - b.top;
        let title_style = if over {
            base.fg(theme.dim).add_modifier(Modifier::CROSSED_OUT)
        } else {
            base.fg(theme.bright).add_modifier(Modifier::BOLD)
        };
        for (r, row) in grid
            .iter_mut()
            .enumerate()
            .take(b.bottom.min(count))
            .skip(b.top)
        {
            for x in b.x..end {
                row.0[x] = Cell {
                    symbol: " ".into(),
                    style: base,
                };
            }
            if b.x < cols {
                row.0[b.x] = Cell {
                    symbol: "▌".into(),
                    style: base.fg(bar),
                };
            }
            let room = end.saturating_sub(b.x + 3);
            if r == b.top {
                let title = text::cut(&e.title, room);
                row.put(b.x + 2, &title, title_style, end);
                if height == 1 && !range.is_empty() {
                    let at = b.x + 2 + width(&title) + 2;
                    let words = text::cut(&range, end.saturating_sub(at + 1));
                    let style = if over {
                        title_style.remove_modifier(Modifier::BOLD)
                    } else {
                        base.fg(theme.dim)
                    };
                    row.put(at, &words, style, end);
                }
            } else if r == b.top + 1 {
                row.put(b.x + 2, &text::cut(&detail, room), base.fg(theme.dim), end);
            }
            if w > 0 {
                hits.push(HitBox {
                    y: (top_row + r) as u16,
                    x0: b.x as u16,
                    x1: (end - 1) as u16,
                    spot: Spot::Index(b.item as isize),
                });
            }
        }
    }

    if beside && count > 0 {
        let x0 = day_w + 2;
        for row in &mut grid {
            row.put(day_w, "│", faint, cols);
        }
        let chosen = look
            .cursor
            .and_then(|c| events.iter().find(|(i, _)| *i == c))
            .map(|(_, e)| e);
        for (r, runs) in event_detail(chosen, look).into_iter().enumerate() {
            if r < count {
                let mut x = x0;
                for (words, style) in runs {
                    let words = text::cut(&words, cols.saturating_sub(x + 1));
                    grid[r].put(x, &words, style, cols);
                    x += width(&words);
                }
            }
        }
    }
    lines.extend(grid.into_iter().map(Row::line));
}

/// How long until something starts, from minutes: "15m", "1h", "2h 5m".
fn countdown(minutes: f64) -> String {
    let m = minutes.ceil().max(1.0) as i64;
    match (m / 60, m % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

/// The chosen event, beside the day, a line a list of runs: its title, its
/// time and where to join, the answers its menu offers as buttons, your
/// reply, who is going, the start of its description, where to join it,
/// and the tickets and PRs it names.
fn event_detail(chosen: Option<&CalEvent>, look: &Look) -> Vec<Vec<(String, Style)>> {
    let (tz, theme) = (look.tz, look.theme);
    let dim = Style::new().fg(theme.dim);
    let heading = dim.add_modifier(Modifier::BOLD);
    let one = |words: String, style: Style| vec![(words, style)];
    let gap = || vec![(String::new(), dim)];
    let Some(e) = chosen else {
        return vec![gap(), one("j/k to choose an event".into(), dim)];
    };
    let mut when = if e.all_day {
        "all day".to_string()
    } else {
        match (e.starts(tz), e.ends(tz)) {
            (Some(s), Some(t)) => calendar::range(s, t, tz),
            _ => String::new(),
        }
    };
    if let Some(j) = &e.join {
        when.push_str(&format!(" · {}", place(j)));
    }
    let mut out = vec![
        gap(),
        one(
            e.title.clone(),
            Style::new().fg(theme.bright).add_modifier(Modifier::BOLD),
        ),
        one(when, dim),
    ];
    let buttons = buttons(e, theme);
    if !buttons.is_empty() {
        out.extend([gap(), buttons]);
    }
    out.extend([
        gap(),
        one("YOUR REPLY".into(), heading),
        one(
            reply(e).into(),
            if e.going() { fg(GOING_BAR) } else { fg(STALE) },
        ),
    ]);
    let going: Vec<&str> = e
        .attendees
        .iter()
        .filter(|a| a.reply == "going")
        .map(|a| a.name.split_whitespace().next().unwrap_or(&a.name))
        .collect();
    if !going.is_empty() {
        const SHOWN: usize = 4;
        let chip = Style::new().bg(theme.panel).fg(theme.text);
        let mut names = Vec::new();
        for name in going.iter().take(SHOWN) {
            names.push((format!(" {name} "), chip));
            names.push((" ".into(), Style::new()));
        }
        if going.len() > SHOWN {
            names.push((format!("+{}", going.len() - SHOWN), dim));
        }
        out.extend([gap(), one("GOING".into(), heading), names]);
    }
    if !e.description.is_empty() {
        out.extend([gap(), one("AGENDA".into(), heading)]);
        out.extend(
            e.description
                .lines()
                .filter(|l| !l.trim().is_empty())
                .take(2)
                .map(|l| one(l.trim().to_string(), Style::new().fg(theme.text))),
        );
    }
    if let Some(j) = &e.join {
        out.extend([
            gap(),
            one("JOIN".into(), heading),
            one(j.clone(), Style::new().fg(theme.text)),
        ]);
    }
    if !e.refs.is_empty() {
        out.extend([gap(), one("FROM YOUR WORK".into(), heading)]);
        out.extend(
            e.refs
                .iter()
                .map(|r| one(format!("● {}", r.id), Style::new().fg(theme.text))),
        );
    }
    out
}

/// Joining and answering as buttons, a run of color each labelled with the
/// key that takes it in the event's menu, only those the menu offers, and
/// then that they are in Enter's menu, since the view has no keys of its
/// own for them.
fn buttons(e: &CalEvent, theme: &Theme) -> Vec<(String, Style)> {
    let mut out = Vec::new();
    if e.join.is_some() {
        out.push((
            " j Join ".to_string(),
            Style::new()
                .bg(Color::Indexed(ACCENT))
                .fg(Color::Indexed(16))
                .add_modifier(Modifier::BOLD),
        ));
    }
    let offered = calendar::rsvp_choices(e);
    for (answer, label) in [
        (calendar::Rsvp::Yes, "Going"),
        (calendar::Rsvp::No, "Not going"),
        (calendar::Rsvp::Maybe, "Maybe"),
    ] {
        if offered.contains(&answer) {
            let (key, _) = answer.choice();
            if !out.is_empty() {
                out.push((" ".into(), Style::new()));
            }
            out.push((
                format!(" {key} {label} "),
                Style::new().bg(theme.panel).fg(theme.text),
            ));
        }
    }
    if !out.is_empty() {
        out.push(("  in Enter menu".into(), Style::new().fg(theme.dim)));
    }
    out
}

/// `h` and `l` in the Calendar view (daemon.js's `besideStep`): the event
/// beside the chosen one, among those sharing its time (they overlap it),
/// in the order they sit side by side (by start, the longer first, as the
/// lanes are laid out); for an all-day event, along the all-day ones. The
/// cursor stays where it is past either end. `j` and `k` still walk every
/// event.
pub fn beside(items: &[crate::items::Item], cursor: usize, by: isize, tz: &TimeZone) -> usize {
    let event =
        |i: usize| -> Option<CalEvent> { serde_json::from_value(items.get(i)?.data.clone()).ok() };
    let Some(chosen) = event(cursor) else {
        return cursor;
    };
    let span = |e: &CalEvent| -> Option<(i64, i64)> {
        Some((e.starts(tz)?.as_millisecond(), e.ends(tz)?.as_millisecond()))
    };
    let mine = span(&chosen);
    let mut near: Vec<(usize, Option<(i64, i64)>)> = (0..items.len())
        .filter_map(|i| {
            let e = event(i)?;
            let s = span(&e);
            let with = if chosen.all_day {
                e.all_day
            } else {
                !e.all_day
                    && matches!((s, mine), (Some((a, b)), Some((from, to))) if a < to && b > from)
            };
            with.then_some((i, s))
        })
        .collect();
    if !chosen.all_day {
        near.sort_by_key(|(i, s)| {
            let (a, b) = s.unwrap_or((0, 0));
            (a, std::cmp::Reverse(b), *i)
        });
    }
    let Some(here) = near.iter().position(|(i, _)| *i == cursor) else {
        return cursor;
    };
    let to = here as isize + by;
    if to < 0 {
        return cursor;
    }
    near.get(to as usize).map_or(cursor, |(i, _)| *i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{Item, ViewData};
    use crate::mission::views::ViewId;
    use serde_json::json;

    fn tz() -> TimeZone {
        TimeZone::UTC
    }

    fn at(h: i64, m: i64) -> String {
        format!("2026-11-02T{h:02}:{m:02}:00Z")
    }

    fn event(title: &str, from: (i64, i64), to: (i64, i64), you: &str) -> Item {
        Item::new(title, title).data(json!({
            "title": title, "start": at(from.0, from.1), "end": at(to.0, to.1), "you": you,
        }))
    }

    fn view(items: Vec<Item>) -> View {
        View {
            id: ViewId::Calendar,
            title: "Calendar".into(),
            data: ViewData {
                items,
                at: Some(1),
                ..ViewData::default()
            },
            loaded: true,
        }
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.text().trim_end().to_string())
            .collect()
    }

    fn now(h: i64, m: i64) -> i64 {
        at(h, m).parse::<Timestamp>().unwrap().as_millisecond()
    }

    fn look(cursor: Option<usize>, rows: usize, now_ms: i64, tz: &TimeZone) -> Look<'_> {
        Look {
            cols: 60,
            rows,
            cursor,
            tz,
            now_ms,
            theme: &Theme::DARK,
            top_row: 1,
            ux2: false,
        }
    }

    #[test]
    fn h_and_l_move_between_events_side_by_side_and_along_the_all_day_ones() {
        // lib/sidebar's "h and l in the Calendar view move between events
        // that share their time".
        let tz = tz();
        let all_day = |t: &str| Item::new(t, t).data(json!({ "title": t, "allDay": true }));
        let items = vec![
            all_day("Offsite"),
            all_day("Holiday"),
            event("Standup", (9, 0), (9, 30), "going"),
            event("Review", (9, 0), (10, 0), "going"),
            event("Coffee", (9, 15), (9, 45), "going"),
            event("Lunch", (12, 0), (13, 0), "going"),
        ];
        // At nine: Review (the longer) first, then Standup, then Coffee.
        assert_eq!(beside(&items, 3, 1, &tz), 2);
        assert_eq!(beside(&items, 2, 1, &tz), 4);
        assert_eq!(beside(&items, 4, 1, &tz), 4, "the last stays");
        assert_eq!(beside(&items, 3, -1, &tz), 3, "the first stays");
        assert_eq!(beside(&items, 5, 1, &tz), 5, "nothing beside lunch");
        assert_eq!(beside(&items, 0, 1, &tz), 1, "along the all-day ones");
        assert_eq!(beside(&items, 1, 1, &tz), 1);
    }

    #[test]
    fn the_day_runs_down_the_hours_with_events_as_blocks_and_a_line_at_now() {
        let tz = tz();
        let v = view(vec![
            Item::new("a", "Offsite")
                .data(json!({ "title": "Offsite", "allDay": true, "you": "going" })),
            event("Standup", (9, 0), (9, 30), "going"),
            event("Design review", (10, 0), (12, 0), "not answered"),
        ]);
        // Eleven hours, 8 to 6, in 22 rows: two rows to an hour.
        let (lines, hits) = day(&v, &look(Some(2), 25, now(13, 0), &tz));
        let text = texts(&lines);
        assert!(text[0].starts_with(" Monday, November 2"), "{text:?}");
        assert!(text[1].starts_with("all day   Offsite"), "{text:?}");
        assert_eq!(text[2], "");
        assert!(text[3].starts_with("   8 AM  ┈┈┈"), "{text:?}");
        // The standup, half an hour: one row with its times.
        assert!(text[5].contains("Standup · 9 – 9:30 AM"), "{text:?}");
        // The review, two hours: its title, then its times.
        assert!(text[7].contains("Design review"), "{text:?}");
        assert!(text[8].contains("10 AM – 12 PM"), "{text:?}");
        // Now, at 1 PM: the red line across the hours.
        let now_line = &lines[13];
        assert!(
            now_line.text().starts_with("   1 PM ●──"),
            "{:?}",
            now_line.text()
        );
        let dot = now_line.spans.iter().find(|s| s.content.contains('●'));
        assert_eq!(dot.unwrap().style.fg, Some(Color::Indexed(NOW)));
        // The chosen review is drawn in the light blue; a click on it chooses it.
        assert!(
            lines[7]
                .spans
                .iter()
                .any(|s| s.style.bg == Some(Color::Indexed(CHOSEN)))
        );
        assert!(hits.iter().any(|h| h.y == 8 && h.spot == Spot::Index(2)));
        assert!(hits.iter().any(|h| h.y == 2 && h.spot == Spot::Index(0)));
        assert_eq!(lines.len(), 23, "ten hours of two rows, under three lines");
        for l in &lines {
            assert_eq!(width(&l.text()), 60, "{:?}", l.text());
        }
    }

    #[test]
    fn events_at_the_same_time_sit_side_by_side_and_one_unanswered_is_outlined() {
        let tz = tz();
        let v = view(vec![
            event("One", (9, 0), (10, 0), "going"),
            event("Two", (9, 0), (10, 0), "maybe"),
            event("Three", (10, 0), (11, 0), "going"),
            event("Four", (11, 0), (12, 0), "going"),
        ]);
        let (lines, _) = day(&v, &look(None, 13, now(7, 0), &tz));
        let text = texts(&lines);
        let row = text.iter().find(|l| l.contains("One")).unwrap();
        let one = row.find("One").unwrap();
        let two = row.find("▎Two").expect("outlined, with its edge");
        assert!(two > one, "{row}");
        // Back to back in the same place: the other shade.
        let shade = |title: &str| {
            let line = lines.iter().find(|l| l.text().contains(title)).unwrap();
            line.spans
                .iter()
                .find(|s| s.content.contains(title))
                .and_then(|s| s.style.bg)
        };
        assert_eq!(shade("Three"), Some(Color::Indexed(SOLID[0])));
        assert_eq!(shade("Four"), Some(Color::Indexed(SOLID[1])));
        // From 7, an hour before the first event's day.
        assert!(text[2].starts_with("   7 AM"), "{text:?}");
    }

    #[test]
    fn with_ux2_a_day_longer_than_the_screen_moves_to_show_the_chosen_event() {
        let tz = tz();
        let v = view(vec![
            event("Standup", (9, 0), (9, 15), "going"),
            event("Retro", (17, 0), (18, 0), "going"),
        ]);
        let shown = |cursor| {
            let look = Look {
                ux2: true,
                ..look(cursor, 16, now(9, 0), &tz)
            };
            texts(&day(&v, &look).0).join("\n")
        };
        // From an hour before now, five hours fit: the five o'clock event
        // is below them until it is chosen, then in sight.
        assert!(!shown(None).contains("Retro"), "{}", shown(None));
        assert!(shown(Some(1)).contains("Retro"), "{}", shown(Some(1)));
        assert!(shown(Some(0)).contains("Standup"), "{}", shown(Some(0)));
    }

    #[test]
    fn with_ux2_the_day_is_a_timeline_and_the_chosen_event_sits_beside_it() {
        let tz = tz();
        let review = Item::new("Design review", "Design review").data(json!({
            "title": "Design review", "start": at(15, 0), "end": at(16, 0),
            "you": "not answered", "join": "https://meet.google.com/abc-defg-hij",
            "refs": [{ "id": "AB-12" }], "id": "g1",
            "attendees": [
                { "name": "Ana Ruiz", "reply": "going" },
                { "name": "Ben", "reply": "going" },
                { "name": "Priya", "reply": "going" },
                { "name": "Sam", "reply": "going" },
                { "name": "Lee", "reply": "going" },
                { "name": "Kim", "reply": "going" },
                { "name": "Max", "reply": "not answered" },
            ],
            "description": "Walk through options A and B.\n\nPick one.\nThird line",
        }));
        let v = view(vec![
            event("Standup", (9, 0), (9, 15), "going"),
            event("Pairing", (13, 30), (14, 30), "going"),
            review,
        ]);
        let look = Look {
            cols: 120,
            ux2: true,
            ..look(Some(2), 40, now(13, 15), &tz)
        };
        let (lines, hits) = day(&v, &look);
        let text = texts(&lines);
        // Three rows an hour from nine, each hour's label right-aligned
        // with a dashed rule across.
        assert!(
            text[2].starts_with("   9:00 ▌ Standup  9 – 9:15 AM"),
            "{text:?}"
        );
        assert!(text[5].starts_with("  10:00 ┄┄┄"), "{text:?}");
        assert!(text[8].starts_with("  11:00 ┄┄┄"), "{text:?}");
        // The standup is over: dimmed and struck through.
        let standup = lines[2]
            .spans
            .iter()
            .find(|s| s.content.contains("Standup"))
            .unwrap();
        assert!(standup.style.add_modifier.contains(Modifier::CROSSED_OUT));
        // Now, 1:15, a red line with its time.
        assert!(text[15].starts_with("   1:15 ━━━"), "{text:?}");
        assert_eq!(
            lines[15]
                .spans
                .iter()
                .find(|s| s.content.contains('━'))
                .unwrap()
                .style
                .fg,
            Some(Color::Indexed(NOW))
        );
        // The pairing, an hour: three rows, its title bold, then its time.
        assert!(text[16].starts_with("        ▌ Pairing"), "{text:?}");
        assert!(text[17].contains("1:30 – 2:30 PM · in 15m"), "{text:?}");
        let bar = lines[16].spans.iter().find(|s| s.content == "▌").unwrap();
        assert_eq!(bar.style.fg, Some(Color::Indexed(GOING_BAR)));
        // The chosen review, beside the day: title, time and where to join,
        // your reply, the link, what it names.
        assert!(text[20].contains("▌ Design review"), "{text:?}");
        assert!(text[21].contains("3 – 4 PM · Google Meet"), "{text:?}");
        let right: Vec<String> = text.iter().map(|l| l.chars().skip(57).collect()).collect();
        assert_eq!(right[3].trim(), "│ Design review");
        assert_eq!(right[4].trim(), "│ 3 – 4 PM · Google Meet");
        // The menu's join and answers as buttons, labelled with its keys.
        assert_eq!(
            right[6].trim(),
            "│  j Join   y Going   n Not going   m Maybe   in Enter menu"
        );
        let join = lines[6]
            .spans
            .iter()
            .find(|s| s.content.contains("j Join"))
            .unwrap();
        assert_eq!(join.style.bg, Some(Color::Indexed(ACCENT)));
        assert_eq!(right[9].trim(), "│ not replied");
        // Four of the six going, by first name, and how many more.
        assert_eq!(right[12].trim(), "│  Ana   Ben   Priya   Sam  +2");
        assert_eq!(right[14].trim(), "│ AGENDA");
        assert_eq!(right[15].trim(), "│ Walk through options A and B.");
        assert_eq!(right[16].trim(), "│ Pick one.");
        assert_eq!(right[19].trim(), "│ https://meet.google.com/abc-defg-hij");
        assert_eq!(right[22].trim(), "│ ● AB-12");
        assert!(hits.iter().any(|h| h.y == 21 && h.spot == Spot::Index(2)));
        for l in &lines {
            assert_eq!(width(&l.text()), 120, "{:?}", l.text());
        }
    }

    #[test]
    fn a_day_with_nothing_to_draw_is_left_to_the_list() {
        let mut v = view(Vec::new());
        assert!(is_day(&v), "today with no events still has its hours");
        v.data.error = Some("No calendar for today yet".into());
        assert!(!is_day(&v));
        v.data.error = None;
        v.data.setup = Some("set it up".into());
        assert!(!is_day(&v));
    }
}
