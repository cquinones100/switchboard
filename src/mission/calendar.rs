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
            b.x = GUTTER + b.lane * w;
        }
        start = end;
    }
    // An event that starts on the row the one above it in the same place
    // ends takes the other shade of that one.
    for i in 0..placed.len() {
        let above = (0..i).rev().find(|&a| {
            placed[a].x == placed[i].x
                && placed[a].width == placed[i].width
                && placed[a].bottom == placed[i].top
        });
        placed[i].shade = above.map_or(0, |a| 1 - placed[a].shade);
    }

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
