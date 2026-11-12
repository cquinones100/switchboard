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
//! from 8:00 to 7:00 (or the first event to the last), three rows an hour,
//! dashed rules, each event a block with a bar in its color, past ones
//! struck through, one moved since it was first seen today with a "moved"
//! chip, the next one saying how soon it starts, now a red line with its
//! time; and, where the screen is wide enough for both, the chosen event
//! beside the day: its time,
//! where to join and your reply, what Enter's menu on it offers (join,
//! reply going, not going or maybe),
//! who is going with their photos (`Run::Avatar`, drawn as images over the
//! text by mission control), its agenda, what of your work it concerns (the
//! tickets and PRs it names, with their state from those boards, and the
//! note items that name them or it, `notes_naming`), then the next working
//! day's first events. The view shows another day than today when `h` and
//! `l` have moved to it (`Look::date`), with no now line then.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use std::path::Path;

use super::render::{Image, Shape};
use super::views::{View, ViewId};
use super::{HitBox, Spot};
use crate::day::calendar::{self, CalEvent, NextDay, Ref};
use crate::photos::Who;
use crate::sidebar::render::Line;
use crate::sidebar::text::{self, width};
use crate::ui::Theme;
use crate::ui::kit::{self, Tone};

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
    /// The terminal draws the kit's half circles.
    pub caps: bool,
    /// The day shown; `None` is today.
    pub date: Option<jiff::civil::Date>,
    /// The next working day after it, from the feed.
    pub next: Option<&'a NextDay>,
    /// The other views, for the state of the tickets and PRs an event
    /// names (the Tickets and PRs boards' last answers).
    pub work: &'a [View],
    /// The note items that name the chosen event or what it names.
    pub notes: &'a [NoteLine],
    /// The first line of the pane beside the day shown, as `J`, `K` and
    /// the wheel over it have scrolled it (`Mission::side_top`).
    pub side_top: usize,
}

/// A note's unticked item that names an event or a ticket or PR it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteLine {
    /// The note's title (its file's name).
    pub note: String,
    /// The item's first line, without its box.
    pub item: String,
    /// Its plan note, from its `Plan: [[…]]` line.
    pub plan: Option<String>,
    /// Its plan line says it needs a choice.
    pub needs_choice: bool,
}

/// The most note items a pane lists.
const NOTE_ITEMS: usize = 5;

/// The unticked items in the notes under `dir` (`.trash` and hidden
/// folders left out) that name one of `refs` or the event's `title`, each
/// with the plan its indented lines name and whether that needs a choice.
/// Reads files, so it runs off the server's task; nothing is sent anywhere.
pub fn notes_naming(dir: &Path, title: &str, refs: &[String]) -> Vec<NoteLine> {
    let title = title.trim().to_lowercase();
    let refs: Vec<String> = refs.iter().map(|r| r.to_lowercase()).collect();
    let names = |line: &str| {
        let line = line.to_lowercase();
        (title.chars().count() >= 4 && line.contains(&title))
            || refs.iter().any(|r| {
                line.match_indices(r.as_str()).any(|(at, _)| {
                    let after = line[at + r.len()..].chars().next();
                    !after.is_some_and(|c| c.is_alphanumeric())
                })
            })
    };
    let mut files = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(d) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            if hidden {
                continue;
            }
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|x| x == "md") {
                files.push(path);
            }
        }
    }
    files.sort();
    let mut out = Vec::new();
    for path in files {
        let small = std::fs::metadata(&path).is_ok_and(|m| m.len() <= 1 << 20);
        let Some(text) = small.then(|| std::fs::read_to_string(&path).ok()).flatten() else {
            continue;
        };
        let text = crate::notes::id::strip(&text);
        let note = path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let Some(item) = line.trim_start().strip_prefix("- [ ] ") else {
                continue;
            };
            if !names(item) {
                continue;
            }
            let indent = line.len() - line.trim_start().len();
            let under = lines[i + 1..]
                .iter()
                .take_while(|l| l.trim().is_empty() || l.len() - l.trim_start().len() > indent);
            let mut plan = None;
            let mut needs_choice = false;
            // A sub-item and what is under it are passed over: a plan
            // there is the sub-item's (as worktree_from_note::own_plan).
            let mut sub: Option<usize> = None;
            for l in under {
                let at = l.len() - l.trim_start().len();
                if l.trim().is_empty() || sub.is_some_and(|s| at > s) {
                    continue;
                }
                sub = l.trim_start().starts_with("- [").then_some(at);
                if sub.is_some() || plan.is_some() {
                    continue;
                }
                if let Some(rest) = l.trim().strip_prefix("Plan:") {
                    plan = rest
                        .split("[[")
                        .nth(1)
                        .and_then(|p| p.split("]]").next())
                        .map(str::to_string);
                    needs_choice = rest.contains("needs a choice");
                }
            }
            out.push(NoteLine {
                note: note.clone(),
                item: item.trim().to_string(),
                plan,
                needs_choice,
            });
            if out.len() >= NOTE_ITEMS {
                return out;
            }
        }
    }
    out
}

/// The day's lines, where a click on each event (or keycap) lands, and the
/// avatars to draw over the text.
pub fn day(view: &View, look: &Look) -> (Vec<Line>, Vec<HitBox>, Vec<Image>) {
    let (cols, tz, theme) = (look.cols, look.tz, look.theme);
    let mut lines: Vec<Line> = Vec::new();
    let mut hits = Vec::new();
    let mut images = Vec::new();
    let now = Timestamp::from_millisecond(look.now_ms).unwrap_or(Timestamp::UNIX_EPOCH);
    let today = now.to_zoned(tz.clone()).date();
    let shown = look.date.unwrap_or(today);
    let midnight = tz
        .to_ambiguous_timestamp(shown.to_datetime(jiff::civil::Time::midnight()))
        .compatible()
        .map(|t| t.as_millisecond())
        .unwrap_or(look.now_ms);
    let minutes =
        |at: Option<Timestamp>| at.map(|t| (t.as_millisecond() - midnight) as f64 / 60_000.0);
    // Another day has no now: NaN leaves the hours to the events, draws no
    // line and says no event is next.
    let now_min = if shown == today {
        (look.now_ms - midnight) as f64 / 60_000.0
    } else {
        f64::NAN
    };
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
    let date = shown.strftime("%A, %B %-d").to_string();
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
        timeline(
            &events,
            look,
            midnight,
            now_min,
            &mut lines,
            &mut hits,
            &mut images,
        );
        return (lines, hits, images);
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
    (lines, hits, images)
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
/// Three rows to an hour at least, twenty minutes a row; a taller screen
/// gives the hours more, up to `MOST_PER_HOUR`.
const PER_HOUR: i64 = 3;
const MOST_PER_HOUR: i64 = 6;
/// The day's grid and the pane beside it, as board 45 lays a view out
/// (`kit::columns`): both stretch, by these weights, the grid never under
/// `DAY_LEAST` columns and the pane never under `PANE_LEAST`. On a screen
/// too narrow for both, the parts give way in `DROPS`' order: the pane
/// beside first, its event then behind Enter's menu and the keys `o`
/// `y` `n` `m`.
const DAY_WEIGHT: usize = 12;
const PANE_WEIGHT: usize = 13;
const DAY_LEAST: usize = 40;
const PANE_LEAST: usize = 34;
const DROPS: &[usize] = &[1];
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

/// The day with UX 2.0 on, as the board draws it: the hours down the left
/// from 8:00 (or the first event, or now) to 7:00 (or the last), three rows
/// an hour, each hour's label dim and right-aligned with a dashed
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
    images: &mut Vec<Image>,
) {
    let (cols, tz, theme) = (look.cols, look.tz, look.theme);
    // Minutes on the shown day's wall clock, as the hours are labelled,
    // so a day daylight saving time starts or ends on keeps 10 AM at 10:00.
    let day = Timestamp::from_millisecond(midnight)
        .unwrap_or(Timestamp::UNIX_EPOCH)
        .to_zoned(tz.clone())
        .date();
    let clock = |t: Timestamp| {
        let z = t.to_zoned(tz.clone());
        let days = z.date().since(day).map_or(0, |d| d.get_days()) as f64;
        days * 1440.0 + z.hour() as f64 * 60.0 + z.minute() as f64 + z.second() as f64 / 60.0
    };
    let minutes = |at: Option<Timestamp>| at.map(clock);
    let now_min = if now_min.is_nan() {
        now_min
    } else {
        clock(Timestamp::from_millisecond(look.now_ms).unwrap_or(Timestamp::UNIX_EPOCH))
    };
    let stretch = |weight, min| kit::Col::Stretch {
        weight,
        min,
        max: None,
    };
    // Two columns between the grid and the pane: the rule and a space.
    let parts = kit::columns(
        &[
            stretch(DAY_WEIGHT, DAY_LEAST),
            stretch(PANE_WEIGHT, PANE_LEAST),
        ],
        cols,
        2,
        DROPS,
    );
    let beside = parts[1].is_some();
    let day_w = match parts[0] {
        Some((_, w)) if beside => w,
        _ => cols,
    };
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
        .fold((8.0f64 * 60.0).min(now_min), f64::min)
        / 60.0)
        .floor()
        .max(0.0) as i64;
    let to = ((timed
        .iter()
        .map(|t| t.3)
        .fold((19.0f64 * 60.0).max(now_min + 1.0), f64::max)
        / 60.0)
        .ceil() as i64)
        .min(24);
    // The hours share the rows the screen has, three to six an hour.
    let per_hour = (rows as i64 / (to - from).max(1)).clamp(PER_HOUR, MOST_PER_HOUR);
    // A day longer than the screen starts an hour before now, then moves
    // as little as it takes to show the chosen event, its start at least.
    let fits = (rows as i64 / per_hour).max(1);
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
    let count = rows.min(((to - first) * per_hour).max(0) as usize);
    let step = 60.0 / per_hour as f64;
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
            let hour = r as i64 % per_hour == 0;
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
                let label = short_clock((first + r as i64 / per_hour) as f64 * 60.0);
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
                let chip = e
                    .moved_from
                    .as_ref()
                    .map(|_| kit::chip("moved", Tone::Needs, look.caps, theme))
                    .unwrap_or_default();
                let chip_w: usize = chip.iter().map(|s| width(&s.content)).sum();
                let title_room = if room > chip_w + 6 {
                    room - chip_w - 1
                } else {
                    room
                };
                let title = text::cut(&e.title, title_room);
                row.put(b.x + 2, &title, title_style, end);
                let mut after = b.x + 2 + width(&title);
                if !chip.is_empty() && after + 1 + chip_w <= end {
                    after += 1;
                    for span in &chip {
                        row.put(after, &span.content, span.style, end);
                        after += width(&span.content);
                    }
                }
                if height == 1 && !range.is_empty() {
                    let at = after + 2;
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

    let mut pane_last = 0;
    if beside && count > 0 {
        let x0 = day_w + 2;
        for row in &mut grid {
            row.put(day_w, "│", faint, cols);
        }
        let chosen = look
            .cursor
            .and_then(|c| events.iter().find(|(i, _)| *i == c))
            .map(|(_, e)| e);
        let pane_w = cols.saturating_sub(x0 + 1);
        let mut runs = event_detail(chosen, look, pane_w);
        runs.extend(next_day_lines(look, pane_w));
        // Taller than the day, it scrolls: J, K and the wheel over it.
        let last = runs.len().saturating_sub(count);
        let top = look.side_top.min(last);
        pane_last = last;
        for (r, runs) in runs.into_iter().skip(top).take(count).enumerate() {
            let y = (top_row + r) as u16;
            let mut x = x0;
            for run in runs {
                match run {
                    Run::Text(span) => {
                        let words = text::cut(&span.content, cols.saturating_sub(x + 1));
                        grid[r].put(x, &words, span.style, cols);
                        x += width(&words);
                    }
                    Run::Avatar { who, name } => {
                        if x + 2 >= cols {
                            continue;
                        }
                        let color = crate::views::board::color_of(&name);
                        let initial: String = name
                            .chars()
                            .find(|c| c.is_alphanumeric())
                            .map(|c| c.to_uppercase().collect())
                            .unwrap_or_else(|| "?".into());
                        let style = Style::new()
                            .bg(Color::Indexed(color))
                            .fg(Color::Indexed(255))
                            .add_modifier(Modifier::BOLD);
                        grid[r].put(x, &format!("{initial} "), style, cols);
                        images.push(Image {
                            name: format!("calendar-avatar-{r}-{x}"),
                            x: x as u16,
                            y,
                            width: 2,
                            height: 1,
                            shape: Shape::Avatar {
                                who,
                                label: name,
                                color: Color::Indexed(color),
                            },
                        });
                        x += 2;
                    }
                }
            }
        }
    }
    if beside && count > 0 {
        // After the keycaps, which a click on their row finds first.
        for r in 0..count {
            hits.push(HitBox {
                y: (top_row + r) as u16,
                x0: (day_w + 1) as u16,
                x1: cols.saturating_sub(1) as u16,
                spot: Spot::Side(pane_last),
            });
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

/// The people whose photos the pane beside the day draws for an event:
/// the first going attendees it shows, by their address.
pub fn people(e: &CalEvent) -> Vec<Who> {
    e.attendees
        .iter()
        .filter(|a| a.reply == "going" && !a.address.is_empty())
        .take(GOING_SHOWN)
        .map(|a| Who::address(&a.address))
        .collect()
}

/// How many of those going the pane shows, the rest as "+N".
const GOING_SHOWN: usize = 4;

/// A piece of a line of the pane beside the day: text, or someone's
/// avatar (two columns, their photo drawn over the initial).
enum Run {
    Text(Span<'static>),
    Avatar { who: Option<Who>, name: String },
}

fn texts_of(spans: Vec<Span<'static>>) -> Vec<Run> {
    spans.into_iter().map(Run::Text).collect()
}

/// `2:00` from an instant as a day keeps it.
fn clock_of(at: &str, tz: &TimeZone) -> Option<String> {
    let t = calendar::parse_instant(at, tz)?.to_zoned(tz.clone());
    Some(short_clock(
        (t.hour() as i64 * 60 + t.minute() as i64) as f64,
    ))
}

/// The chosen event, beside the day, `w` wide: its title, then its time
/// (and where it moved from, and who moved it), where to join and your
/// reply; its keys as keycaps; who is going with their photos, four and
/// then how many more; the start of its agenda; and what of your work it
/// concerns.
fn event_detail(chosen: Option<&CalEvent>, look: &Look, w: usize) -> Vec<Vec<Run>> {
    let (tz, theme) = (look.tz, look.theme);
    let dim = Tone::Dim.style(theme);
    let Some(e) = chosen else {
        return vec![
            Vec::new(),
            vec![Run::Text(Span::styled("j/k to choose an event", dim))],
        ];
    };
    let mut when = if e.all_day {
        "all day".to_string()
    } else {
        match (e.starts(tz), e.ends(tz)) {
            (Some(s), Some(t)) => calendar::range(s, t, tz),
            _ => String::new(),
        }
    };
    if let Some(old) = e.moved_from.as_deref().and_then(|o| clock_of(o, tz)) {
        when.push_str(&format!(" · moved from {old}"));
        if let Some(by) = &e.organizer {
            when.push_str(&format!(
                " by {}",
                by.split_whitespace().next().unwrap_or(by)
            ));
        }
    }
    if let Some(j) = &e.join {
        when.push_str(&format!(" · {}", place(j)));
    }
    let facts = vec![Span::styled(when, dim)];
    let mut out: Vec<Vec<Run>> = vec![Vec::new()];
    out.extend(
        kit::pane(&e.title, facts, w, theme)
            .into_iter()
            .map(texts_of),
    );
    // You, your reply as a chip, and what Enter's menu does, with no
    // keycaps (board 54).
    let tone = if e.going() { Tone::Accent } else { Tone::Dim };
    let mut you = vec![Run::Text(Span::styled("You ", Tone::Text.style(theme)))];
    you.extend(
        kit::chip(reply(e), tone, look.caps, theme)
            .into_iter()
            .map(Run::Text),
    );
    you.push(Run::Text(Span::styled(
        format!("  {}", enter_words(e)),
        Tone::Dim.style(theme),
    )));
    out.push(you);
    let going: Vec<&calendar::Attendee> =
        e.attendees.iter().filter(|a| a.reply == "going").collect();
    if !going.is_empty() {
        out.push(Vec::new());
        out.push(texts_of(kit::label(
            "Going",
            Tone::Dim,
            Some(going.len()),
            w,
            theme,
        )));
        let mut row = vec![Run::Text(Span::raw(" "))];
        for a in going.iter().take(GOING_SHOWN) {
            let first = a.name.split_whitespace().next().unwrap_or(&a.name);
            row.push(Run::Avatar {
                who: (!a.address.is_empty()).then(|| Who::address(&a.address)),
                name: a.name.clone(),
            });
            row.push(Run::Text(Span::styled(
                format!("{first}  "),
                Tone::Text.style(theme),
            )));
        }
        if going.len() > GOING_SHOWN {
            row.push(Run::Text(Span::styled(
                format!("+{}", going.len() - GOING_SHOWN),
                dim,
            )));
        }
        out.push(row);
    }
    if !e.description.is_empty() {
        out.push(Vec::new());
        out.push(texts_of(kit::label("Agenda", Tone::Dim, None, w, theme)));
        out.extend(
            e.description
                .lines()
                .filter(|l| !l.trim().is_empty())
                .take(3)
                .map(|l| {
                    vec![Run::Text(Span::styled(
                        format!(" {}", l.trim()),
                        Tone::Text.style(theme),
                    ))]
                }),
        );
    }
    if !e.refs.is_empty() || !look.notes.is_empty() {
        out.push(Vec::new());
        out.push(texts_of(kit::label(
            "From your work",
            Tone::Dim,
            None,
            w,
            theme,
        )));
        for r in &e.refs {
            out.push(texts_of(work_line(r, look.work, theme)));
        }
        for n in look.notes {
            let mut line = vec![
                Span::styled(" ☐ ", Tone::Accent.style(theme)),
                Span::styled(n.item.clone(), Tone::Text.style(theme)),
            ];
            if let Some(plan) = &n.plan {
                let plan = plan.strip_prefix("Plan - ").unwrap_or(plan);
                line.push(Span::styled(format!("  Plan: {plan}"), dim));
            } else {
                line.push(Span::styled(
                    format!("  {}", n.note),
                    Tone::Faint.style(theme),
                ));
            }
            if n.needs_choice {
                line.push(Span::raw(" "));
                line.extend(kit::chip("needs a choice", Tone::Needs, look.caps, theme));
            }
            out.push(texts_of(line));
        }
    }
    out
}

/// A ticket or PR an event names, with its title and state from the
/// Tickets or PRs board's last answer: `● AB-142 Fix the RSVP menu  In
/// progress`, `✓` once it is done or merged; its id alone while no board
/// knows it.
fn work_line(r: &Ref, work: &[View], theme: &Theme) -> Vec<Span<'static>> {
    let text = |item: &crate::items::Item, p: &str| {
        item.data
            .pointer(p)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let items = |id: ViewId| {
        work.iter()
            .filter(move |v| v.id == id)
            .flat_map(|v| v.data.items.iter())
    };
    let known = items(ViewId::Tickets)
        .find(|i| i.id.eq_ignore_ascii_case(&r.id) && i.data.get("ticket").is_some())
        .map(|i| {
            (
                text(i, "/title"),
                text(i, "/state"),
                i.state == crate::items::State::Idle,
            )
        })
        .or_else(|| {
            let (repo, number) = r.id.rsplit_once('#')?;
            let number: u64 = number.parse().ok()?;
            // A link names its owner too, which a fork of the same name
            // doesn't share; a bare reference only its repository's name.
            let owned = r.url.as_deref().and_then(|u| {
                let path = u.split("github.com/").nth(1)?;
                let mut parts = path.split('/');
                Some(format!("{}/{}", parts.next()?, parts.next()?))
            });
            items(ViewId::Prs)
                .find(|i| {
                    let theirs = text(i, "/pr/repo");
                    let same_repo = match &owned {
                        Some(full) => theirs.eq_ignore_ascii_case(full),
                        None => theirs.rsplit('/').next().is_some_and(|name| {
                            name.eq_ignore_ascii_case(repo.rsplit('/').next().unwrap_or(repo))
                        }),
                    };
                    i.data
                        .pointer("/pr/number")
                        .and_then(serde_json::Value::as_u64)
                        == Some(number)
                        && same_repo
                })
                .map(|i| {
                    let title = i
                        .title
                        .strip_prefix(&format!("#{number} "))
                        .unwrap_or(&i.title)
                        .to_string();
                    let state = i.group.clone().unwrap_or_default();
                    let done = i.state == crate::items::State::Idle;
                    (title, state, done)
                })
        });
    match known {
        Some((title, state, done)) => {
            let (mark, tone) = if done {
                ("✓", Tone::Done)
            } else {
                ("●", Tone::Working)
            };
            vec![
                Span::styled(format!(" {mark} "), tone.style(theme)),
                Span::styled(format!("{} ", r.id), Tone::Text.bold(theme)),
                Span::styled(title, Tone::Text.style(theme)),
                Span::styled(format!("  {state}"), Tone::Dim.style(theme)),
            ]
        }
        None => vec![
            Span::styled(" ● ", Tone::Dim.style(theme)),
            Span::styled(r.id.clone(), Tone::Text.style(theme)),
        ],
    }
}

/// What Enter's menu on the event does, said after your reply: joining
/// or replying when it offers either, else its other actions.
fn enter_words(e: &CalEvent) -> &'static str {
    if e.join.is_some() || !calendar::rsvp_choices(e).is_empty() {
        "Enter: join or reply"
    } else {
        "Enter: its actions"
    }
}

/// Under the pane, the next working day: its weekday and date, and that a
/// weekend with nothing on it was passed over; then its first three timed
/// events, dim: the time, the title, then how long, where to join and your
/// reply when you aren't going, faint.
fn next_day_lines(look: &Look, w: usize) -> Vec<Vec<Run>> {
    let Some(next) = look.next else {
        return Vec::new();
    };
    let (tz, theme) = (look.tz, look.theme);
    let Ok(date) = next.date.parse::<jiff::civil::Date>() else {
        return Vec::new();
    };
    let mut head = vec![
        Span::styled(
            format!(" Next: {}", date.strftime("%A")),
            Tone::Text.bold(theme),
        ),
        Span::styled(
            format!("  {}", date.strftime("%B %-d")),
            Tone::Dim.style(theme),
        ),
    ];
    if next.skipped {
        head.push(Span::styled(
            "  nothing this weekend",
            Tone::Faint.style(theme),
        ));
    }
    let mut out = vec![
        Vec::new(),
        vec![Run::Text(kit::head_rule(w, theme))],
        texts_of(head),
    ];
    if next.events.is_empty() {
        out.push(vec![Run::Text(Span::styled(
            " nothing on it",
            Tone::Faint.style(theme),
        ))]);
    }
    for e in next.events.iter().take(3) {
        let at = e
            .start
            .as_deref()
            .and_then(|s| clock_of(s, tz))
            .unwrap_or_default();
        let mut more = Vec::new();
        if let (Some(s), Some(t)) = (e.starts(tz), e.ends(tz)) {
            more.push(countdown(
                (t.as_millisecond() - s.as_millisecond()) as f64 / 60_000.0,
            ));
        }
        if let Some(j) = &e.join {
            more.push(place(j));
        }
        if !e.going() {
            more.push(reply(e).to_string());
        }
        out.push(texts_of(vec![
            Span::styled(format!("{at:>6}  "), Tone::Dim.style(theme)),
            Span::styled(e.title.clone(), Tone::Dim.style(theme)),
            Span::styled(format!("  {}", more.join(" · ")), Tone::Faint.style(theme)),
        ]));
    }
    out
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
            caps: false,
            date: None,
            next: None,
            work: &[],
            notes: &[],
            side_top: 0,
        }
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
        let (lines, hits, _) = day(&v, &look(Some(2), 25, now(13, 0), &tz));
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
        let (lines, _, _) = day(&v, &look(None, 13, now(7, 0), &tz));
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
                { "name": "Ana Ruiz", "reply": "going", "address": "ana@example.com" },
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
        let (lines, hits, avatars) = day(&v, &look);
        let text = texts(&lines);
        // Three rows an hour from eight, each hour's label right-aligned
        // with a dashed rule across.
        assert!(text[2].starts_with("   8:00 ┄┄┄"), "{text:?}");
        assert!(
            text[5].starts_with("   9:00 ▌ Standup  9 – 9:15 AM"),
            "{text:?}"
        );
        assert!(text[8].starts_with("  10:00 ┄┄┄"), "{text:?}");
        // The standup is over: dimmed and struck through.
        let standup = lines[5]
            .spans
            .iter()
            .find(|s| s.content.contains("Standup"))
            .unwrap();
        assert!(standup.style.add_modifier.contains(Modifier::CROSSED_OUT));
        // Now, 1:15, a red line with its time.
        assert!(text[18].starts_with("   1:15 ━━━"), "{text:?}");
        assert_eq!(
            lines[18]
                .spans
                .iter()
                .find(|s| s.content.contains('━'))
                .unwrap()
                .style
                .fg,
            Some(Color::Indexed(NOW))
        );
        // The pairing, an hour: three rows, its title bold, then its time.
        assert!(text[19].starts_with("        ▌ Pairing"), "{text:?}");
        assert!(text[20].contains("1:30 – 2:30 PM · in 15m"), "{text:?}");
        let bar = lines[19].spans.iter().find(|s| s.content == "▌").unwrap();
        assert_eq!(bar.style.fg, Some(Color::Indexed(GOING_BAR)));
        assert!(text[23].contains("▌ Design review"), "{text:?}");
        // The chosen review beside the day: its title, then its time,
        // where to join and your reply, under them a rule.
        let edge = text[2].chars().position(|c| c == '│').unwrap();
        let right: Vec<String> = text
            .iter()
            .map(|l| l.chars().skip(edge).collect())
            .collect();
        assert_eq!(right[3].trim(), "│ Design review");
        assert_eq!(right[4].trim(), "│ 3 – 4 PM · Google Meet");
        assert!(right[5].contains("────"), "{right:?}");
        // You, your reply as a chip, and what Enter does, with no keycaps.
        assert!(right[6].contains("You"), "{right:?}");
        assert!(right[6].contains("not replied"), "{right:?}");
        assert!(
            right[6].trim_end().ends_with("Enter: join or reply"),
            "{right:?}"
        );
        assert!(!hits.iter().any(|h| matches!(h.spot, Spot::Key(_))));
        // Who is going: four by first name with their photos, and how
        // many more.
        assert!(right[8].contains("GOING"), "{right:?}");
        assert_eq!(right[9].trim(), "│  A Ana  B Ben  P Priya  S Sam  +2");
        assert_eq!(avatars.len(), 4);
        assert_eq!(
            avatars[0].shape,
            Shape::Avatar {
                who: Some(Who::address("ana@example.com")),
                label: "Ana Ruiz".into(),
                color: Color::Indexed(crate::views::board::color_of("Ana Ruiz")),
            }
        );
        assert_eq!((avatars[0].y, avatars[0].width), (10, 2));
        assert_eq!(
            people(&serde_json::from_value(v.data.items[2].data.clone()).unwrap()).len(),
            1
        );
        assert_eq!(right[11].trim(), "│  AGENDA");
        assert_eq!(right[12].trim(), "│  Walk through options A and B.");
        assert_eq!(right[16].trim(), "│  FROM YOUR WORK");
        assert_eq!(right[17].trim(), "│  ● AB-12");
        assert!(hits.iter().any(|h| h.y == 24 && h.spot == Spot::Index(2)));
        for l in &lines {
            assert_eq!(width(&l.text()), 120, "{:?}", l.text());
        }
    }

    fn ux2_look<'a>(cursor: Option<usize>, now_ms: i64, tz: &'a TimeZone) -> Look<'a> {
        Look {
            cols: 120,
            ux2: true,
            ..look(cursor, 40, now_ms, tz)
        }
    }

    #[test]
    fn with_ux2_a_day_with_no_events_runs_from_8_00_to_7_00() {
        let tz = tz();
        let (lines, _, _) = day(&view(Vec::new()), &ux2_look(None, now(12, 0), &tz));
        let text = texts(&lines);
        let hours: Vec<&str> = text
            .iter()
            .filter_map(|l| l.get(..7).map(str::trim).filter(|h| h.ends_with(":00")))
            .collect();
        assert_eq!(
            hours,
            [
                "8:00", "9:00", "10:00", "11:00", "12:00", "1:00", "2:00", "3:00", "4:00", "5:00",
                "6:00"
            ]
        );
        assert_eq!(lines.len(), 2 + 33, "the last hour, 6 to 7, whole");
    }

    #[test]
    fn a_moved_event_carries_a_chip_and_its_pane_says_from_when_and_by_whom() {
        let tz = tz();
        let moved = Item::new("Sync", "Sync").data(json!({
            "title": "Sync", "start": at(15, 0), "end": at(16, 0), "you": "going",
            "movedFrom": at(14, 0), "organizer": "Ana Ruiz",
        }));
        let v = view(vec![moved]);
        let (lines, _, _) = day(&v, &ux2_look(Some(0), now(9, 0), &tz));
        let text = texts(&lines);
        let block = text.iter().find(|l| l.contains("▌ Sync")).unwrap();
        assert!(block.contains("Sync  moved"), "{block}");
        let chip = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content.contains("moved") && !s.content.contains("from"))
            .unwrap();
        assert_eq!(chip.style.fg, Some(Tone::Needs.fg(&Theme::DARK)));
        assert!(
            text.iter()
                .any(|l| l.contains("3 – 4 PM · moved from 2:00 by Ana")),
            "{text:?}"
        );
    }

    #[test]
    fn the_pane_ends_with_the_next_working_day_past_an_empty_weekend() {
        let tz = tz();
        let next = NextDay {
            date: "2026-11-09".into(),
            events: vec![
                serde_json::from_value(json!({
                    "title": "Planning", "start": "2026-11-09T09:00:00Z",
                    "end": "2026-11-09T09:30:00Z", "you": "not answered",
                    "join": "https://meet.google.com/x",
                }))
                .unwrap(),
            ],
            skipped: true,
        };
        let v = view(vec![event("Standup", (9, 0), (9, 15), "going")]);
        let look = Look {
            next: Some(&next),
            ..ux2_look(Some(0), now(8, 0), &tz)
        };
        let text = texts(&day(&v, &look).0);
        let pane: Vec<String> = text
            .iter()
            .map(|l| {
                let edge = l.chars().position(|c| c == '│').unwrap_or(0);
                l.chars()
                    .skip(edge + 1)
                    .collect::<String>()
                    .trim()
                    .to_string()
            })
            .filter(|l| !l.is_empty())
            .collect();
        let at = pane
            .iter()
            .position(|l| l.starts_with("Next: Monday"))
            .unwrap();
        assert!(pane[at - 1].starts_with("┄┄┄"), "{pane:?}");
        assert_eq!(pane[at], "Next: Monday  November 9  nothing this weekend");
        assert_eq!(
            pane[at + 1],
            "9:00  Planning  30m · Google Meet · not replied"
        );
    }

    #[test]
    fn another_day_has_no_now_and_nothing_struck_through_ahead() {
        let tz = tz();
        let v = view(vec![Item::new("Later", "Later").data(json!({
            "title": "Later", "start": "2026-11-03T10:00:00Z",
            "end": "2026-11-03T11:00:00Z", "you": "going",
        }))]);
        let look = Look {
            date: Some(jiff::civil::date(2026, 11, 3)),
            ..ux2_look(Some(0), now(13, 0), &tz)
        };
        let (lines, _, _) = day(&v, &look);
        let text = texts(&lines);
        assert!(text[0].starts_with(" Tuesday, November 3"), "{text:?}");
        assert!(!text.iter().any(|l| l.contains('━')), "no now line");
        let later = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content.contains("Later"))
            .unwrap();
        assert!(!later.style.add_modifier.contains(Modifier::CROSSED_OUT));
        assert!(
            text.iter().any(|l| l.starts_with("  10:00 ▌ Later")),
            "{text:?}"
        );
    }

    #[test]
    fn what_of_your_work_an_event_concerns_comes_from_the_boards_and_your_notes() {
        let tz = tz();
        let tickets = View {
            id: ViewId::Tickets,
            title: "Tickets".into(),
            data: ViewData {
                items: vec![
                    Item::new("AB-142", "AB-142 Fix the RSVP menu").state(crate::items::State::Active).data(json!({
                        "ticket": "AB-142", "title": "Fix the RSVP menu", "state": "In progress",
                    })),
                ],
                ..ViewData::default()
            },
            loaded: true,
        };
        let notes = vec![NoteLine {
            note: "Work".into(),
            item: "FEAT: RSVP for AB-142".into(),
            plan: Some("Plan - RSVP".into()),
            needs_choice: true,
        }];
        let v = view(vec![Item::new("Sync", "Sync").data(json!({
            "title": "Sync", "start": at(15, 0), "end": at(16, 0), "you": "going",
            "refs": [{ "id": "AB-142" }, { "id": "AB-9" },
                { "id": "app#7", "url": "https://github.com/team/app/pull/7" }],
        }))]);
        let prs = View {
            id: ViewId::Prs,
            title: "PRs".into(),
            data: ViewData {
                items: vec![
                    Item::new("p", "#7 A fork's fix")
                        .state(crate::items::State::Active)
                        .group("Review")
                        .data(json!({ "pr": { "number": 7, "repo": "fork/app" } })),
                ],
                ..ViewData::default()
            },
            loaded: true,
        };
        let work = [tickets, prs];
        let look = Look {
            work: &work,
            notes: &notes,
            ..ux2_look(Some(0), now(9, 0), &tz)
        };
        let text = texts(&day(&v, &look).0).join("\n");
        assert!(
            text.contains("● AB-142 Fix the RSVP menu  In progress"),
            "{text}"
        );
        assert!(text.contains("● AB-9"), "one no board knows, by its id");
        assert!(
            text.contains("● app#7") && !text.contains("A fork's fix"),
            "a fork's PR of the same name and number isn't the one linked: {text}"
        );
        assert!(
            text.contains("☐ FEAT: RSVP for AB-142  Plan: RSVP  needs a choice"),
            "{text}"
        );
    }

    #[test]
    fn notes_naming_finds_unticked_items_naming_the_event_or_its_tickets() {
        let dir = std::env::temp_dir().join(format!("sb-cal-notes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".trash")).unwrap();
        std::fs::write(
            dir.join("Work.md"),
            "# Work\n\n- [ ] Review https://github.com/team/app/pull/7 before it\n- [ ] FEAT: fix AB-142\n  Plan: [[Plan - RSVP]] (needs a choice: A, B)\n- [ ] about AB-1420 instead\n- [x] AB-142 done already\n- [ ] Prepare the design review deck\n",
        )
        .unwrap();
        std::fs::write(dir.join(".trash/Old.md"), "- [ ] AB-142 gone\n").unwrap();
        std::fs::write(
            dir.join("Nested.md"),
            "- [ ] Prepare AB-77\n  - [ ] a sub-item\n    Plan: [[Plan - Child]] (needs a choice: A, B)\n",
        )
        .unwrap();
        let nested = notes_naming(&dir, "Sync", &["AB-77".into()]);
        assert_eq!(nested.len(), 1);
        assert_eq!(nested[0].plan, None, "a sub-item's plan isn't its parent's");
        assert!(!nested[0].needs_choice);
        let found = notes_naming(
            &dir,
            "Design review",
            &["AB-142".into(), "https://github.com/team/app/pull/7".into()],
        );
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            found,
            [
                NoteLine {
                    note: "Work".into(),
                    item: "Review https://github.com/team/app/pull/7 before it".into(),
                    plan: None,
                    needs_choice: false,
                },
                NoteLine {
                    note: "Work".into(),
                    item: "FEAT: fix AB-142".into(),
                    plan: Some("Plan - RSVP".into()),
                    needs_choice: true,
                },
                NoteLine {
                    note: "Work".into(),
                    item: "Prepare the design review deck".into(),
                    plan: None,
                    needs_choice: false,
                },
            ]
        );
    }

    #[test]
    fn the_board_lays_out_by_the_screens_width_and_height() {
        let tz = tz();
        let v = view(vec![
            event("Standup", (9, 0), (9, 30), "going"),
            event("Review", (15, 0), (16, 0), "going"),
        ]);
        let mut days = Vec::new();
        for cols in [120, 160, 240] {
            let look = Look {
                cols,
                ..ux2_look(Some(1), now(8, 0), &tz)
            };
            let (lines, _, _) = day(&v, &look);
            for l in &lines {
                assert_eq!(width(&l.text()), cols, "{cols}: {:?}", l.text());
            }
            let text = texts(&lines);
            let edge = text[2].chars().position(|c| c == '│').unwrap();
            let pane = cols - edge - 1;
            assert!(pane >= PANE_LEAST, "{cols}: a pane of {pane}");
            assert!(
                text.iter().any(|l| l.ends_with("Review")),
                "{cols}: the pane"
            );
            days.push(edge);
        }
        assert!(
            days[0] < days[1] && days[1] < days[2],
            "the grid stretches: {days:?}"
        );
        // A narrow screen drops the pane first.
        let narrow = Look {
            cols: DAY_LEAST + 2 + PANE_LEAST - 1,
            ..ux2_look(Some(1), now(8, 0), &tz)
        };
        assert!(!texts(&day(&v, &narrow).0).iter().any(|l| l.contains('│')));
        // A tall screen gives the hours more rows.
        let tall = Look {
            rows: 80,
            ..ux2_look(None, now(8, 0), &tz)
        };
        let text = texts(&day(&v, &tall).0);
        let at = |h: &str| {
            text.iter()
                .position(|l| l.trim_start().starts_with(h))
                .unwrap()
        };
        assert_eq!(at("10:00") - at("9:00"), 6, "{text:?}");
    }

    #[test]
    fn a_pane_taller_than_the_day_scrolls_with_j_k_and_the_wheel() {
        let tz = tz();
        let v = view(vec![Item::new("Sync", "Sync").data(json!({
            "title": "Sync", "start": at(9, 0), "end": at(10, 0), "you": "going",
            "description": "One\nTwo\nThree",
            "refs": [{ "id": "AB-1" }, { "id": "AB-2" }, { "id": "AB-3" }],
        }))]);
        let small = |side_top| Look {
            rows: 12,
            side_top,
            ..ux2_look(Some(0), now(8, 0), &tz)
        };
        let (lines, hits, _) = day(&v, &small(0));
        let last = hits
            .iter()
            .find_map(|h| match h.spot {
                Spot::Side(last) => Some(last),
                _ => None,
            })
            .expect("the pane is a scroll stop");
        assert!(last > 0, "taller than the day");
        assert!(texts(&lines).iter().any(|l| l.contains("Sync")));
        let (scrolled, _, _) = day(&v, &small(last));
        let text = texts(&scrolled).join("\n");
        assert!(text.contains("AB-3"), "{text}");
        assert!(!text.contains("│ Sync"), "the title scrolled away: {text}");
    }

    #[test]
    fn a_day_daylight_saving_time_ends_on_keeps_events_at_their_hours() {
        let tz = TimeZone::get("America/New_York").unwrap();
        // November 1, 2026: clocks go back at 2 AM, so 10 AM is 15:00Z.
        let v = view(vec![Item::new("Sync", "Sync").data(json!({
            "title": "Sync", "start": "2026-11-01T15:00:00Z", "end": "2026-11-01T16:00:00Z",
            "you": "going",
        }))]);
        let now_ms = "2026-11-01T17:00:00Z"
            .parse::<Timestamp>()
            .unwrap()
            .as_millisecond();
        let (lines, _, _) = day(&v, &ux2_look(None, now_ms, &tz));
        let text = texts(&lines);
        assert!(
            text.iter().any(|l| l.starts_with("  10:00 ▌ Sync")),
            "{text:?}"
        );
        assert!(
            text.iter().any(|l| l.starts_with("  12:00 ━━━")),
            "now at noon: {text:?}"
        );
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
