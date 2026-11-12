//! UX 2.0's Slack view, board 21 of the UX 2.0 canvas ("Slack: the chosen
//! message beside its thread"): the messages grouped as Mentions, Direct,
//! Your threads and Channels, each with its sender's photo, where and how
//! long ago, its reply count or "↩ replied", two lines of its text and its
//! pictures; and beside the list, the chosen message's whole thread, each
//! message with its sender's photo, replies since you last looked marked
//! with a bar and a "new" chip, its pictures and videos' first frames, then
//! "From your work": the tickets the thread names, with their state and the
//! tab on them, and today's events it names, with how soon. New with UX
//! 2.0, with no tmux-version file behind it.
//!
//! It draws the view's own items (`day::slack::slack_view`), so the cursor,
//! the filter and the menu work on them as on the plain list; the thread
//! comes from `slack-threads.json` (`day::slack_thread`) by way of the
//! snapshot, and its pictures from those the server has read and decoded,
//! so drawing reads nothing. Laid out as board 45 says (`kit::columns`): the
//! list and the pane stretch by `LIST_WEIGHT` and `PANE_WEIGHT`, the pane
//! never under `PANE_LEAST`; below `NAMES_AT` columns the names beside the
//! list's pictures go, then where each message was, then From your work's
//! states; under `BESIDE_AT` the pane goes behind `i`, drawn in the list's
//! place until `i` or Esc.

use std::collections::HashMap;

use jiff::tz::TimeZone;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use serde_json::Value;

use super::diagrams::Ready;
use super::render::{Image, Shape};
use super::views::{View, ViewId};
use super::{HitBox, Spot};
use crate::day::slack_thread::{Said, SlackFile, Thread};
use crate::items::Item;
use crate::photos::{Service, Who};
use crate::sidebar::model::TabModel;
use crate::sidebar::render::Line;
use crate::sidebar::text::{cut, width, wrap};
use crate::ui::Theme;
use crate::ui::kit::{self, Col, Tone};

/// Under this many columns the thread pane goes behind `i`.
pub const BESIDE_AT: usize = 140;
/// Under this many, the names beside the list's pictures, where each
/// message was and From your work's states go.
pub const NAMES_AT: usize = 160;
const LIST_WEIGHT: usize = 62;
const PANE_WEIGHT: usize = 62;
const LIST_LEAST: usize = 34;
const PANE_LEAST: usize = 34;
/// A message's sender at most, and where and when.
const FROM_CAP: usize = 16;
const WHERE_CAP: usize = 24;
/// A picture in the list, in cells, and how many a message shows.
const PIC_COLS: usize = 14;
const PIC_ROWS: usize = 4;
const PICS: usize = 3;
/// A picture in the pane: its share of the pane's width, as the board's
/// 34 of 61 columns, and its rows.
const PANE_PIC_SHARE: (usize, usize) = (34, 61);
const PANE_PIC_ROWS: usize = 8;

/// The groups the list is in, by their kind, in order.
const GROUPS: [(&str, &str); 4] = [
    ("mention", "Mentions"),
    ("dm", "Direct"),
    ("thread", "Your threads"),
    ("channel", "Channels"),
];

/// What drawing the board needs beyond the view.
pub struct Look<'a> {
    pub cols: usize,
    pub rows: usize,
    /// The screen row the first line goes on.
    pub top_row: usize,
    pub cursor: Option<usize>,
    pub theme: &'a Theme,
    pub caps: bool,
    pub now_ms: i64,
    pub tz: &'a TimeZone,
    /// The chosen message's thread, once read.
    pub thread: Option<&'a Thread>,
    /// Settings' `slack_threads`: threads are read (else the pane isn't
    /// drawn).
    pub threads_on: bool,
    /// The pictures read and decoded, by file ID.
    pub pictures: &'a HashMap<String, Ready>,
    /// The terminal draws the server's images.
    pub graphics: bool,
    /// The other views, for the tickets' states and today's events.
    pub work: &'a [View],
    pub tabs: &'a [TabModel],
    /// The pane's first line, as `J`, `K` and the wheel have scrolled it.
    pub side_top: usize,
    /// `i` has the thread in the list's place on a narrow screen.
    pub thread_open: bool,
    /// Why the chosen thread couldn't be read (a connector not connected
    /// says to run /mcp), drawn in place of "Reading the thread…".
    pub failed: Option<&'a str>,
    /// The thread's files that couldn't be read, by ID.
    pub files_failed: &'a HashMap<String, String>,
    /// The thread's files kept on this computer, by ID.
    pub files_kept: &'a std::collections::HashSet<String>,
    /// The message under the mouse, lit.
    pub hover: Option<usize>,
}

/// A piece of a line: text, someone's avatar, or a picture placed with its
/// top-left corner here.
enum Run {
    Text(Span<'static>),
    Avatar {
        who: Option<Who>,
        name: String,
    },
    Picture {
        id: String,
        cols: usize,
        rows: usize,
    },
}

fn texts(spans: Vec<Span<'static>>) -> Vec<Run> {
    spans.into_iter().map(Run::Text).collect()
}

fn get<'a>(item: &'a Item, key: &str) -> &'a str {
    item.data.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The messages in the order the board draws them: by group, each group
/// as the view orders it (the new before the seen, newest first).
pub fn in_groups(items: &mut [Item]) {
    let group = |i: &Item| {
        GROUPS
            .iter()
            .position(|(k, _)| *k == get(i, "kind"))
            .unwrap_or(GROUPS.len())
    };
    items.sort_by_key(group);
}

/// Whether the pane is drawn beside the list on a screen this wide.
pub fn beside(cols: usize) -> bool {
    cols >= BESIDE_AT
}

/// The people the board draws, for their photos: the senders of the
/// listed messages and of the chosen thread's.
pub fn people(view: &View, thread: Option<&Thread>) -> Vec<Who> {
    let mut out: Vec<Who> = Vec::new();
    let users = view
        .data
        .items
        .iter()
        .map(|i| get(i, "user").to_string())
        .chain(
            thread
                .into_iter()
                .flat_map(|t| t.messages.iter().map(|m| m.user.clone())),
        );
    for u in users {
        if u.is_empty() {
            continue;
        }
        let who = Who::new(Service::Slack, &u);
        if !out.contains(&who) {
            out.push(who);
        }
    }
    out
}

/// The board: its bar, then the list and the thread beside it.
pub fn board(view: &View, look: &Look) -> (Vec<Line>, Vec<HitBox>, Vec<Image>) {
    let (cols, theme) = (look.cols, look.theme);
    let mut lines = Vec::new();
    let mut hits = Vec::new();
    let mut images = Vec::new();

    // The bar: what is wrong at the left, when it last looked at the right.
    let looked = view
        .data
        .at
        .filter(|at| *at > 1)
        .map(|at| {
            format!(
                "looked {} ago ",
                crate::sidebar::text::ago(Some(at.div_euclid(1000)), look.now_ms.div_euclid(1000))
            )
        })
        .unwrap_or_default();
    let mut bar = Vec::new();
    if let Some(e) = &view.data.error {
        bar.push(Span::styled(format!(" {e}"), Tone::Needs.style(theme)));
    }
    let at = kit::columns(
        &[
            Col::Stretch {
                weight: 1,
                min: 0,
                max: None,
            },
            Col::Right {
                natural: width(&looked),
                cap: width(&looked),
            },
        ],
        cols,
        1,
        &[0],
    );
    let mut bar_line = Vec::new();
    if let Some(place) = at[0] {
        kit::place(&mut bar_line, bar, place);
    }
    if let Some(place) = at[1] {
        kit::place(
            &mut bar_line,
            vec![Span::styled(looked, Tone::Faint.style(theme))],
            place,
        );
    }
    lines.push(Line {
        spans: kit::fit(bar_line, cols, Style::new()),
        fill: None,
    });
    let rows = look.rows.saturating_sub(1);
    let top_row = look.top_row + 1;

    let side = look.threads_on && beside(cols);
    let (list_at, pane_at) = if side {
        let at = kit::columns(
            &[
                Col::Stretch {
                    weight: LIST_WEIGHT,
                    min: LIST_LEAST,
                    max: None,
                },
                Col::Fixed {
                    min: 1,
                    natural: 1,
                    cap: 1,
                },
                Col::Stretch {
                    weight: PANE_WEIGHT,
                    min: PANE_LEAST,
                    max: None,
                },
            ],
            cols,
            1,
            &[],
        );
        (at[0], at[2])
    } else if look.threads_on && look.thread_open {
        (None, Some((1, cols.saturating_sub(2))))
    } else {
        (Some((0, cols)), None)
    };

    let chosen = look.cursor.and_then(|c| view.data.items.get(c));
    let (list, owners, list_scroll) = match list_at {
        Some((_, w)) => list_lines(view, look, w, rows),
        None => (Vec::new(), Vec::new(), 0),
    };
    let pane = match (pane_at, chosen) {
        (Some((_, w)), Some(item)) => {
            let seen = view
                .data
                .head
                .get("seen")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            thread_lines(item, look, w, seen)
        }
        (Some(_), None) => vec![
            Vec::new(),
            texts(vec![Span::styled(
                " j/k to choose a message",
                Tone::Dim.style(theme),
            )]),
        ],
        _ => Vec::new(),
    };
    let pane_last = pane.len().saturating_sub(rows);
    let pane_top = look.side_top.min(pane_last);

    let mut list = window(list, list_scroll, rows).into_iter();
    let mut owners = owners.into_iter().skip(list_scroll);
    let mut pane = window(pane, pane_top, rows).into_iter();
    for r in 0..rows {
        let y = top_row + r;
        let mut line: Vec<Span<'static>> = Vec::new();
        if let Some(at) = list_at {
            let runs = list.next().unwrap_or_default();
            let before = images.len();
            put(&mut line, runs, at, y, look, &mut images, "slack-list");
            // Its pictures before the row, which a click finds first.
            picture_hits(&images[before..], look, &mut hits);
            if let Some(Some(i)) = owners.next() {
                hits.push(HitBox {
                    y: y as u16,
                    x0: at.0 as u16,
                    x1: (at.0 + at.1).saturating_sub(1) as u16,
                    spot: Spot::Index(i as isize),
                });
            }
        }
        if let (Some(list_at), Some(_)) = (list_at, pane_at) {
            kit::place(
                &mut line,
                vec![kit::rule_v(theme)],
                (list_at.0 + list_at.1 + 1, 1),
            );
        }
        if let Some(at) = pane_at {
            let runs = pane.next().unwrap_or_default();
            let before = images.len();
            put(&mut line, runs, at, y, look, &mut images, "slack-pane");
            picture_hits(&images[before..], look, &mut hits);
            // J, K and the wheel scroll it, beside the list or in its place.
            hits.push(HitBox {
                y: y as u16,
                x0: at.0 as u16,
                x1: cols.saturating_sub(1) as u16,
                spot: Spot::Side(pane_last),
            });
        }
        lines.push(Line {
            spans: kit::fit(line, cols, Style::new()),
            fill: None,
        });
    }
    (lines, hits, images)
}

/// The `rows` lines from `top` that show, a picture that starts above them
/// or runs past them drawn in its rows that show (scaled to fit them), so
/// scrolling never leaves its rows blank.
fn window(lines: Vec<Vec<Run>>, top: usize, rows: usize) -> Vec<Vec<Run>> {
    let end = top + rows;
    let mut lines = lines;
    for i in 0..lines.len().min(end) {
        let mut x = 0;
        let mut moved = Vec::new();
        for run in &mut lines[i] {
            let at = x;
            x += run_width(run);
            let Run::Picture {
                id,
                cols,
                rows: high,
            } = run
            else {
                continue;
            };
            let last = i + *high;
            if i < top && last > top {
                moved.push((at, id.clone(), *cols, (last - top).min(rows)));
                *run = Run::Text(Span::raw(" ".repeat(*cols)));
            } else if i >= top && last > end {
                *high = end - i;
            }
        }
        // Scrolled partly away: drawn in what shows of it, in its column on
        // the first row shown, over the blank its rows leave there.
        for (at, id, cols, high) in moved {
            if let Some(row) = lines.get_mut(top) {
                place_picture(
                    row,
                    at,
                    Run::Picture {
                        id,
                        cols,
                        rows: high,
                    },
                );
            }
        }
    }
    lines.into_iter().skip(top).take(rows).collect()
}

/// `picture` put on `row` at column `at`: in place of the blank run that
/// starts there, or after the row's end; left out where text is.
fn place_picture(row: &mut Vec<Run>, at: usize, picture: Run) {
    let cols = run_width(&picture);
    let mut x = 0;
    for i in 0..row.len() {
        let w = run_width(&row[i]);
        let blank = matches!(&row[i], Run::Text(s) if s.content.trim().is_empty());
        if x == at && blank && w >= cols {
            row[i] = picture;
            if w > cols {
                row.insert(i + 1, Run::Text(Span::raw(" ".repeat(w - cols))));
            }
            return;
        }
        x += w;
    }
    if x <= at {
        row.push(Run::Text(Span::raw(" ".repeat(at - x))));
        row.push(picture);
    }
}

fn run_width(run: &Run) -> usize {
    match run {
        Run::Text(s) => width(&s.content),
        Run::Avatar { .. } => 2,
        Run::Picture { cols, .. } => *cols,
    }
}

/// A picture is a click that shows it, as `p` does, by its place among
/// the thread's files.
fn picture_hits(images: &[Image], look: &Look, hits: &mut Vec<HitBox>) {
    for p in images {
        let Shape::Picture { key } = &p.shape else {
            continue;
        };
        let Some(n) = look
            .thread
            .and_then(|t| t.files().position(|f| f.id == *key))
        else {
            continue;
        };
        for r in 0..p.height {
            hits.push(HitBox {
                y: p.y + r,
                x0: p.x,
                x1: p.x + p.width.saturating_sub(1),
                spot: Spot::Picture(n),
            });
        }
    }
}

/// `runs` put on `line` at a column, its avatars and pictures placed as
/// images where they fall.
fn put(
    line: &mut Vec<Span<'static>>,
    runs: Vec<Run>,
    (x0, w): (usize, usize),
    y: usize,
    look: &Look,
    images: &mut Vec<Image>,
    name: &str,
) {
    let mut spans = Vec::new();
    let mut x = 0;
    for run in runs {
        match run {
            Run::Text(span) => {
                x += width(&span.content);
                spans.push(span);
            }
            Run::Avatar { who, name: person } => {
                if x + 2 > w {
                    continue;
                }
                let color = crate::views::board::color_of(&person);
                let initial: String = person
                    .chars()
                    .find(|c| c.is_alphanumeric())
                    .map(|c| c.to_uppercase().collect())
                    .unwrap_or_else(|| "?".into());
                spans.push(Span::styled(
                    format!("{initial} "),
                    Style::new()
                        .bg(Color::Indexed(color))
                        .fg(Color::Indexed(255))
                        .add_modifier(Modifier::BOLD),
                ));
                images.push(Image {
                    name: format!("{name}-avatar-{y}-{}", x0 + x),
                    x: (x0 + x) as u16,
                    y: y as u16,
                    width: 2,
                    height: 1,
                    shape: Shape::Avatar {
                        who,
                        label: person,
                        color: Color::Indexed(color),
                    },
                });
                x += 2;
            }
            Run::Picture { id, cols, rows } => {
                let cols = cols.min(w.saturating_sub(x));
                if cols >= 4 && look.pictures.contains_key(&id) {
                    images.push(Image {
                        name: format!("{name}-picture-{y}-{}", x0 + x),
                        x: (x0 + x) as u16,
                        y: y as u16,
                        width: cols as u16,
                        height: rows as u16,
                        shape: Shape::Picture { key: id },
                    });
                }
                spans.push(Span::raw(" ".repeat(cols)));
                x += cols;
            }
        }
    }
    kit::place(line, spans, (x0, w));
}

/// The list's lines, which item each belongs to, and the first line shown
/// so the chosen message is on the screen.
fn list_lines(
    view: &View,
    look: &Look,
    w: usize,
    rows: usize,
) -> (Vec<Vec<Run>>, Vec<Option<usize>>, usize) {
    let theme = look.theme;
    let mut out: Vec<Vec<Run>> = Vec::new();
    let mut owners = Vec::new();
    let mut chosen_at = (0, 0);
    for (kind, name) in GROUPS {
        let members: Vec<(usize, &Item)> = view
            .data
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| get(i, "kind") == kind)
            .collect();
        if members.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(Vec::new());
            owners.push(None);
        }
        out.push(texts(kit::label(
            name,
            Tone::Dim,
            Some(members.len()),
            w,
            theme,
        )));
        owners.push(None);
        for (i, item) in members {
            let start = out.len();
            let lit = look.cursor != Some(i) && look.hover == Some(i);
            let lines = message_lines(item, look, w, look.cursor == Some(i), lit);
            owners.extend(std::iter::repeat_n(Some(i), lines.len()));
            out.extend(lines);
            if look.cursor == Some(i) {
                chosen_at = (start, out.len());
            }
        }
    }
    if out.is_empty() {
        out.push(texts(vec![Span::styled(
            " Nothing new on Slack.",
            Tone::Dim.style(theme),
        )]));
        owners.push(None);
    }
    // The chosen message whole on the screen, its group's label too when
    // it fits.
    let (start, end) = chosen_at;
    // A message taller than the screen shows from its top.
    let scroll = if end > rows {
        (end - rows).min(start)
    } else {
        0
    };
    (out, owners, scroll)
}

/// How long ago a message's ts was: `4m`, `2h`.
fn ago(ts: &str, now_ms: i64) -> String {
    let secs = ts.parse::<f64>().unwrap_or(0.0) as i64;
    crate::sidebar::text::ago(Some(secs), now_ms.div_euclid(1000))
}

/// A message's text wrapped to `w`, `@someone` in the accent, at most
/// `most` lines, each with `indent` before it.
fn body(
    text: &str,
    w: usize,
    most: usize,
    indent: usize,
    tone: Tone,
    theme: &Theme,
) -> Vec<Vec<Run>> {
    let room = w.saturating_sub(indent + 1).min(kit::WRAP_MAX);
    let mut lines = wrap(text, room);
    let cut_short = lines.len() > most;
    lines.truncate(most);
    if cut_short && let Some(last) = lines.last_mut() {
        *last = format!("{}…", cut(last, room.saturating_sub(1)));
    }
    lines
        .into_iter()
        .map(|l| {
            let mut runs = vec![Run::Text(Span::raw(" ".repeat(indent)))];
            for (n, word) in l.split(' ').enumerate() {
                if n > 0 {
                    runs.push(Run::Text(Span::styled(" ", tone.style(theme))));
                }
                let style = if word.starts_with('@') && word.len() > 1 {
                    Tone::Accent.style(theme)
                } else {
                    tone.style(theme)
                };
                runs.push(Run::Text(Span::styled(word.to_string(), style)));
            }
            runs
        })
        .collect()
}

/// A message of the list: its sender's photo and name, where and how long
/// ago, its reply count or "↩ replied" at the right; two lines of its
/// text; its pictures. The chosen one on the chosen surface with the bar.
fn message_lines(item: &Item, look: &Look, w: usize, chosen: bool, lit: bool) -> Vec<Vec<Run>> {
    let theme = look.theme;
    let old = item.data.get("old").and_then(Value::as_bool) == Some(true);
    let tone = if old { Tone::Dim } else { Tone::Text };
    let from = match get(item, "from") {
        "" => "Someone",
        f => f,
    };
    let place = get(item, "where");
    let when = ago(get(item, "ts"), look.now_ms);
    let right = if item.data.get("answered").and_then(Value::as_bool) == Some(true) {
        "↩ replied".to_string()
    } else {
        match item.data.get("replies").and_then(Value::as_u64) {
            Some(0) | None => String::new(),
            Some(1) => "1 reply".into(),
            Some(n) => format!("{n} replies"),
        }
    };
    let wide = look.cols >= NAMES_AT;
    let where_when = if wide && !place.is_empty() {
        format!("{place} · {when}")
    } else {
        when
    };
    // The bar's column, the avatar, the sender, where and when, the count.
    let inner = w.saturating_sub(1);
    let at = kit::columns(
        &[
            Col::Fixed {
                min: 2,
                natural: 2,
                cap: 2,
            },
            Col::Fixed {
                min: 4,
                natural: width(from),
                cap: FROM_CAP,
            },
            Col::Fixed {
                min: 0,
                natural: width(&where_when),
                cap: WHERE_CAP,
            },
            Col::Stretch {
                weight: 1,
                min: 0,
                max: None,
            },
            Col::Right {
                natural: width(&right),
                cap: 12,
            },
        ],
        inner,
        1,
        &[2, 4],
    );
    let user = get(item, "user");
    let mut first: Vec<Run> = vec![bar(chosen, theme)];
    let mut x = 0;
    let pad_to = |runs: &mut Vec<Run>, to: usize, x: &mut usize| {
        if to > *x {
            runs.push(Run::Text(Span::raw(" ".repeat(to - *x))));
            *x = to;
        }
    };
    if let Some((ax, _)) = at[0] {
        pad_to(&mut first, ax, &mut x);
        first.push(Run::Avatar {
            who: (!user.is_empty()).then(|| Who::new(Service::Slack, user)),
            name: from.to_string(),
        });
        x += 2;
    }
    if let Some((fx, fw)) = at[1] {
        pad_to(&mut first, fx, &mut x);
        let name = cut(from, fw);
        x += width(&name);
        first.push(Run::Text(Span::styled(name, tone.bold(theme))));
    }
    if let Some((px, pw)) = at[2] {
        pad_to(&mut first, px, &mut x);
        let words = cut(&where_when, pw);
        x += width(&words);
        first.push(Run::Text(Span::styled(words, Tone::Dim.style(theme))));
    }
    if let Some((rx, rw)) = at[4] {
        pad_to(&mut first, rx, &mut x);
        let words = cut(&right, rw);
        let style = if right.starts_with('↩') {
            Tone::Done.style(theme)
        } else {
            Tone::Dim.style(theme)
        };
        first.push(Run::Text(Span::styled(words, style)));
    }
    let mut out = vec![first];
    let text = item.lines.join("\n");
    for line in body(&text, inner, 2, 3, tone, theme) {
        let mut runs = vec![bar(chosen, theme)];
        runs.extend(line);
        out.push(runs);
    }
    // Its pictures, those read, three at most, their names beside them on
    // a wide screen.
    let own: Vec<&SlackFile> = look
        .thread
        .filter(|_| chosen)
        .and_then(|t| t.messages.iter().find(|m| m.ts == get(item, "ts")))
        .map(|m| m.files.iter().filter(|f| f.drawn()).collect())
        .unwrap_or_default();
    let shown: Vec<&&SlackFile> = own
        .iter()
        .filter(|f| look.graphics && look.pictures.contains_key(&f.id))
        .take(PICS)
        .collect();
    if !shown.is_empty() && inner >= 3 + PIC_COLS {
        let fit = ((inner - 3) / (PIC_COLS + 1)).clamp(1, PICS);
        let shown = &shown[..shown.len().min(fit)];
        for r in 0..PIC_ROWS {
            let mut runs = vec![bar(chosen, theme), Run::Text(Span::raw("  "))];
            for f in shown {
                if r == 0 {
                    runs.push(Run::Picture {
                        id: f.id.clone(),
                        cols: PIC_COLS,
                        rows: PIC_ROWS,
                    });
                } else {
                    runs.push(Run::Text(Span::raw(" ".repeat(PIC_COLS))));
                }
                runs.push(Run::Text(Span::raw(" ")));
            }
            if wide && let Some(f) = shown.get(r) {
                runs.push(Run::Text(Span::styled(
                    f.name.clone(),
                    Tone::Dim.style(theme),
                )));
            }
            out.push(runs);
        }
    } else if !own.is_empty() {
        let names: Vec<&str> = own.iter().map(|f| f.name.as_str()).collect();
        out.push(vec![
            bar(chosen, theme),
            Run::Text(Span::styled(
                format!("   ▣ {}", names.join(", ")),
                Tone::Dim.style(theme),
            )),
        ]);
    }
    if !chosen && !lit {
        return out;
    }
    // The chosen one on the chosen surface, edge to edge; the one under the
    // mouse on the panel's.
    let bg = if chosen {
        kit::Surface::Chosen.color(theme)
    } else {
        theme.panel
    };
    out.into_iter()
        .map(|runs| {
            let mut used = 0;
            let mut runs: Vec<Run> = runs
                .into_iter()
                .map(|r| match r {
                    Run::Text(s) => {
                        used += width(&s.content);
                        let style = s.style.bg(bg);
                        Run::Text(Span::styled(s.content, style))
                    }
                    Run::Avatar { .. } => {
                        used += 2;
                        r
                    }
                    Run::Picture { cols, .. } => {
                        used += cols;
                        r
                    }
                })
                .collect();
            if used < w {
                runs.push(Run::Text(Span::styled(
                    " ".repeat(w - used),
                    Style::new().bg(bg),
                )));
            }
            runs
        })
        .collect()
}

/// The list's left column: the accent bar on the chosen message's lines.
fn bar(chosen: bool, theme: &Theme) -> Run {
    Run::Text(Span::styled(
        if chosen { "▌" } else { " " },
        Tone::Accent.style(theme),
    ))
}

/// What brought a message, as the pane's facts say it.
fn brought_by(kind: &str) -> &'static str {
    match kind {
        "mention" => "opened from your mention",
        "dm" => "a direct message",
        "thread" => "a reply in your thread",
        _ => "from a channel you follow",
    }
}

/// The chosen message's thread, `w` wide: where it was and its facts, each
/// message with its sender's photo, how long ago and its text, replies
/// since you last looked marked; its pictures and videos; then From your
/// work.
fn thread_lines(item: &Item, look: &Look, w: usize, seen: i64) -> Vec<Vec<Run>> {
    let theme = look.theme;
    let dim = Tone::Dim.style(theme);
    let place = match get(item, "where") {
        "" => "Slack",
        p => p,
    };
    let kind = get(item, "kind");
    let mut facts = Vec::new();
    match look.thread {
        Some(t) if t.replies > 0 => facts.push(format!(
            "thread · {} repl{}",
            t.replies,
            if t.replies == 1 { "y" } else { "ies" }
        )),
        Some(_) => facts.push("no replies".into()),
        None => {}
    }
    facts.push(brought_by(kind).into());
    let mut out: Vec<Vec<Run>> = vec![Vec::new()];
    out.extend(
        kit::pane(place, vec![Span::styled(facts.join(" · "), dim)], w, theme)
            .into_iter()
            .map(texts),
    );
    let alone;
    let messages: &[Said] = match look.thread {
        Some(t) => &t.messages,
        None => {
            alone = [Said {
                user: get(item, "user").to_string(),
                from: get(item, "from").to_string(),
                text: item.lines.join("\n"),
                ts: get(item, "ts").to_string(),
                files: Vec::new(),
                refs: Vec::new(),
            }];
            &alone
        }
    };
    for (n, m) in messages.iter().enumerate() {
        let fresh = n > 0 && (m.ts.parse::<f64>().unwrap_or(0.0) * 1000.0) as i64 > seen;
        let mark = if fresh {
            Run::Text(Span::styled("▎", Tone::Accent.style(theme)))
        } else {
            Run::Text(Span::raw(" "))
        };
        out.push(Vec::new());
        let from = if m.from.is_empty() {
            "Someone"
        } else {
            &m.from
        };
        let mut head = vec![
            mark_again(&mark),
            Run::Avatar {
                who: (!m.user.is_empty()).then(|| Who::new(Service::Slack, &m.user)),
                name: from.to_string(),
            },
            Run::Text(Span::raw(" ")),
            Run::Text(Span::styled(cut(from, FROM_CAP), Tone::Text.bold(theme))),
            Run::Text(Span::styled(format!("  {}", ago(&m.ts, look.now_ms)), dim)),
        ];
        if fresh {
            head.push(Run::Text(Span::raw("  ")));
            head.extend(texts(kit::chip("new", Tone::Accent, look.caps, theme)));
        }
        out.push(head);
        for line in body(&m.text, w, usize::MAX, 3, Tone::Text, theme) {
            let mut runs = vec![mark_again(&mark)];
            runs.extend(line);
            out.push(runs);
        }
        for f in m.files.iter().filter(|f| f.drawn()) {
            let shown = look.graphics && look.pictures.contains_key(&f.id);
            if shown {
                let cols = (w * PANE_PIC_SHARE.0 / PANE_PIC_SHARE.1).max(PIC_COLS);
                for r in 0..PANE_PIC_ROWS {
                    let mut runs = vec![mark_again(&mark), Run::Text(Span::raw("  "))];
                    if r == 0 {
                        runs.push(Run::Picture {
                            id: f.id.clone(),
                            cols,
                            rows: PANE_PIC_ROWS,
                        });
                    }
                    out.push(runs);
                }
            }
            let sign = if f.is_video() { "▶" } else { "▣" };
            let words = match (f.is_video(), shown) {
                _ if look.files_failed.contains_key(&f.id) => {
                    let why = look.files_failed.get(&f.id).map_or("", String::as_str);
                    format!("  {sign} {} · couldn't read it: {why}", f.name)
                }
                (true, _) => format!("  ▶ {} · p plays it", f.name),
                (false, true) => format!("  {} · p shows it", f.name),
                (false, false) if look.graphics && !look.files_kept.contains(&f.id) => {
                    format!("  ▣ {} · loading", f.name)
                }
                (false, false) => format!("  ▣ {} · p shows it", f.name),
            };
            out.push(vec![mark_again(&mark), Run::Text(Span::styled(words, dim))]);
        }
    }
    // Why it couldn't be read, under what was kept when it was read before.
    if look.threads_on && (look.thread.is_none() || look.failed.is_some()) {
        out.push(Vec::new());
        let what = if look.thread.is_some() {
            "Couldn't read it again"
        } else {
            "Couldn't read the thread"
        };
        match look.failed {
            Some(why) => {
                for line in wrap(&format!("{what}: {why}"), w.saturating_sub(2)) {
                    out.push(texts(vec![Span::styled(
                        format!(" {line}"),
                        Tone::Needs.style(theme),
                    )]));
                }
            }
            None => out.push(texts(vec![Span::styled(" Reading the thread…", dim)])),
        }
    }
    out.extend(from_your_work(messages, look, w));
    out
}

/// A run's copy, for the mark down a message's lines.
fn mark_again(run: &Run) -> Run {
    match run {
        Run::Text(s) => Run::Text(s.clone()),
        _ => Run::Text(Span::raw(" ")),
    }
}

/// Under the thread: each ticket it names with its title and state from
/// the Tickets board and the tab on it, or "no tab yet"; then today's
/// events it names, with how soon they start.
fn from_your_work(messages: &[Said], look: &Look, w: usize) -> Vec<Vec<Run>> {
    let theme = look.theme;
    let text: String = messages
        .iter()
        .map(|m| m.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let mut ids = crate::views::ticket::ids_in(&text);
    for id in messages.iter().flat_map(|m| m.refs.iter()) {
        if !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    let lower = text.to_lowercase();
    let now = jiff::Timestamp::from_millisecond(look.now_ms).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    // Each as when it is: "today" all day, "now" while it runs, else how
    // soon it starts; one already over is left out.
    let events: Vec<(String, String)> = look
        .work
        .iter()
        .filter(|v| v.id == ViewId::Calendar)
        .flat_map(|v| v.data.items.iter())
        .filter_map(|i| {
            serde_json::from_value::<crate::day::calendar::CalEvent>(i.data.clone()).ok()
        })
        .filter(|e| e.title.chars().count() >= 4 && lower.contains(&e.title.to_lowercase()))
        .filter_map(|e| {
            if e.all_day {
                return Some((e.title.clone(), "today".to_string()));
            }
            let at = e.starts(look.tz)?;
            if at <= now {
                let ended = e.ends(look.tz).is_some_and(|end| end <= now);
                return (!ended).then(|| (e.title.clone(), "now".to_string()));
            }
            let minutes = (at.as_millisecond() - look.now_ms) / 60_000;
            let soon = if minutes < 60 {
                format!("in {}m", minutes.max(1))
            } else {
                format!("in {}h {}m", minutes / 60, minutes % 60)
            };
            Some((e.title.clone(), soon))
        })
        .collect();
    if ids.is_empty() && events.is_empty() {
        return Vec::new();
    }
    let states = look.cols >= NAMES_AT;
    let mut out = vec![
        Vec::new(),
        vec![Run::Text(kit::head_rule(w, theme))],
        texts(kit::label("From your work", Tone::Dim, None, w, theme)),
    ];
    for id in ids {
        let ticket = look
            .work
            .iter()
            .filter(|v| v.id == ViewId::Tickets)
            .flat_map(|v| v.data.items.iter())
            .find(|i| i.id.eq_ignore_ascii_case(&id) && i.data.get("ticket").is_some());
        let title = ticket
            .and_then(|i| i.data.get("title").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        let state = ticket
            .and_then(|i| i.data.get("state").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        let tab = look
            .tabs
            .iter()
            .find(|t| crate::views::ticket::id_in_name(&t.name).as_deref() == Some(id.as_str()));
        let after = match tab {
            Some(t) => format!("tab {}", t.index),
            None => "no tab yet".into(),
        };
        let tail = if states && !state.is_empty() {
            format!("  {state} · {after}")
        } else {
            format!("  {after}")
        };
        let at = kit::columns(
            &[
                Col::Fixed {
                    min: 4,
                    natural: width(&id) + 3,
                    cap: 16,
                },
                Col::Stretch {
                    weight: 1,
                    min: 0,
                    max: None,
                },
                Col::Fixed {
                    min: 0,
                    natural: width(&tail),
                    cap: 30,
                },
            ],
            w,
            0,
            &[1],
        );
        let mut line = Vec::new();
        if let Some(p) = at[0] {
            kit::place(
                &mut line,
                vec![
                    Span::styled(" ● ", Tone::Working.style(theme)),
                    Span::styled(id.clone(), Tone::Text.bold(theme)),
                ],
                p,
            );
        }
        if let Some(p) = at[1] {
            kit::place(
                &mut line,
                vec![Span::styled(format!(" {title}"), Tone::Text.style(theme))],
                p,
            );
        }
        if let Some(p) = at[2] {
            kit::place(
                &mut line,
                vec![Span::styled(tail, Tone::Dim.style(theme))],
                p,
            );
        }
        out.push(texts(line));
    }
    for (title, soon) in events {
        out.push(texts(vec![
            Span::styled(" ◷ ", Tone::Accent.style(theme)),
            Span::styled(title, Tone::Text.style(theme)),
            Span::styled(format!("  {soon}"), Tone::Dim.style(theme)),
        ]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::day::slack_thread::Said;
    use crate::items::ViewData;
    use serde_json::json;

    const NOW: i64 = 1_800_000_000_000;

    fn ts(ago_s: i64) -> String {
        format!("{}.000100", NOW / 1000 - ago_s)
    }

    fn message(kind: &str, from: &str, text: &str, ago_s: i64, extra: Value) -> Item {
        let t = ts(ago_s);
        let mut data = json!({
            "kind": kind, "old": false, "user": format!("U{from}"), "from": from,
            "where": "#dev", "ts": t, "thread": format!("C1:{t}"),
            "message": { "channel": "C1", "ts": t },
        });
        if let (Some(d), Some(e)) = (data.as_object_mut(), extra.as_object()) {
            d.extend(e.clone());
        }
        let mut item = Item::new(format!("C1:{t}"), from).data(data);
        item.lines = text.split('\n').map(str::to_string).collect();
        item
    }

    fn view() -> View {
        View {
            id: ViewId::Slack,
            title: "Slack".into(),
            data: ViewData {
                items: vec![
                    message(
                        "mention",
                        "Ada",
                        "can @me look at ABC-142 before Planning",
                        120,
                        json!({ "replies": 3 }),
                    ),
                    message("dm", "Bo", "lunch?", 300, json!({ "answered": true })),
                    message("channel", "Cy", "deploy done", 600, json!({})),
                ],
                at: Some(NOW - 120_000),
                head: json!({ "seen": NOW - 200_000 }),
                ..ViewData::default()
            },
            loaded: true,
        }
    }

    fn thread() -> Thread {
        Thread {
            messages: vec![
                Said {
                    user: "UAda".into(),
                    from: "Ada".into(),
                    text: "can @me look at ABC-142 before Planning".into(),
                    ts: ts(400),
                    files: Vec::new(),
                    refs: Vec::new(),
                },
                Said {
                    user: "UDee".into(),
                    from: "Dee".into(),
                    text: "an older reply".into(),
                    ts: ts(300),
                    files: Vec::new(),
                    refs: Vec::new(),
                },
                Said {
                    user: "UEd".into(),
                    from: "Ed".into(),
                    text: "a newer one".into(),
                    ts: ts(60),
                    files: Vec::new(),
                    refs: Vec::new(),
                },
            ],
            replies: 2,
            read: NOW,
        }
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.text().trim_end().to_string())
            .collect()
    }

    fn draw(
        cols: usize,
        thread: Option<&Thread>,
        open: bool,
        work: &[View],
    ) -> (Vec<String>, Vec<Image>) {
        let tz = TimeZone::UTC;
        let pictures = HashMap::new();
        let look = Look {
            cols,
            rows: 40,
            top_row: 1,
            cursor: Some(0),
            theme: &Theme::DARK,
            caps: false,
            now_ms: NOW,
            tz: &tz,
            thread,
            threads_on: true,
            pictures: &pictures,
            graphics: false,
            work,
            tabs: &[],
            side_top: 0,
            thread_open: open,
            failed: None,
            files_failed: &Default::default(),
            files_kept: &Default::default(),
            hover: None,
        };
        let (lines, _, images) = board(&view(), &look);
        for l in &lines {
            assert!(
                width(&l.text()) <= cols,
                "a line fits {cols} columns: {:?}",
                l.text()
            );
        }
        (texts(&lines), images)
    }

    #[test]
    fn the_list_is_grouped_with_counts_and_replies_or_replied_at_the_right() {
        let (lines, images) = draw(200, None, false, &[]);
        let all = lines.join("\n");
        let at = |s: &str| all.find(s).unwrap_or_else(|| panic!("{s} in\n{all}"));
        assert!(at("MENTIONS") < at("DIRECT") && at("DIRECT") < at("CHANNELS"));
        assert!(
            !all.contains("YOUR THREADS"),
            "a group with nothing is left out"
        );
        let ada = lines
            .iter()
            .find(|l| l.contains("Ada") && l.contains("#dev"))
            .unwrap();
        assert!(ada.contains("3 replies"));
        let left = ada.split('│').next().unwrap();
        assert!(
            left.trim_end().ends_with("3 replies") && left.ends_with(" "),
            "at the list's right edge: {ada}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("Bo") && l.contains("↩ replied"))
        );
        assert!(
            lines[0].ends_with("looked 2m ago"),
            "the bar's right: {}",
            lines[0]
        );
        assert!(
            images
                .iter()
                .any(|i| matches!(&i.shape, Shape::Avatar { label, .. } if label == "Ada"))
        );
        assert!(
            all.contains("Reading the thread…"),
            "until its thread is read"
        );
        let tz = TimeZone::UTC;
        let (pictures, failed_files, kept_files) =
            (HashMap::new(), HashMap::new(), Default::default());
        let look = Look {
            cols: 200,
            rows: 40,
            top_row: 1,
            cursor: Some(0),
            theme: &Theme::DARK,
            caps: false,
            now_ms: NOW,
            tz: &tz,
            thread: None,
            threads_on: true,
            pictures: &pictures,
            graphics: false,
            work: &[],
            tabs: &[],
            side_top: 0,
            thread_open: false,
            failed: Some("Slack isn't connected in Claude: run /mcp"),
            files_failed: &failed_files,
            files_kept: &kept_files,
            hover: None,
        };
        let said = texts(&board(&view(), &look).0).join("\n");
        assert!(
            said.contains("Couldn't read the thread: Slack isn't connected"),
            "{said}"
        );
    }

    #[test]
    fn the_thread_beside_marks_replies_since_you_looked_and_names_your_work() {
        let tickets = View {
            id: ViewId::Tickets,
            title: "Tickets".into(),
            data: ViewData {
                items: vec![Item::new("ABC-142", "Fix the RSVP menu").data(
                    json!({ "ticket": true, "title": "Fix the RSVP menu", "state": "In Progress" }),
                )],
                ..ViewData::default()
            },
            loaded: true,
        };
        let thread = thread();
        let (lines, _) = draw(200, Some(&thread), false, &[tickets]);
        let all = lines.join("\n");
        assert!(
            all.contains("thread · 2 replies · opened from your mention"),
            "{all}"
        );
        let dee = lines.iter().find(|l| l.contains("Dee")).unwrap();
        assert!(!dee.contains("new"), "seen before: {dee}");
        let ed = lines.iter().find(|l| l.contains("Ed ")).unwrap();
        assert!(ed.contains("▎") && ed.contains("new"), "{ed}");
        assert!(all.contains("FROM YOUR WORK"));
        let web = lines.iter().find(|l| l.contains("● ABC-142")).unwrap();
        assert!(
            web.contains("Fix the RSVP menu")
                && web.contains("In Progress")
                && web.contains("no tab yet"),
            "{web}"
        );
    }

    #[test]
    fn a_picture_scrolled_partly_away_is_drawn_in_its_rows_that_show() {
        let pic = |rows| Run::Picture {
            id: "F1".into(),
            cols: 14,
            rows,
        };
        let mut lines: Vec<Vec<Run>> = vec![vec![Run::Text(Span::raw("   ")), pic(8)]];
        for _ in 1..12 {
            lines.push(vec![Run::Text(Span::raw("   "))]);
        }
        let placed = |lines: &[Vec<Run>]| -> Vec<(usize, usize)> {
            lines
                .iter()
                .enumerate()
                .flat_map(|(i, l)| {
                    l.iter().filter_map(move |r| match r {
                        Run::Picture { rows, .. } => Some((i, *rows)),
                        _ => None,
                    })
                })
                .collect()
        };
        let shown = window(lines, 3, 4);
        assert_eq!(
            placed(&shown),
            [(0, 4)],
            "its five rows below the top, cut to the four shown"
        );
        assert_eq!(
            run_width(&shown[0][0]) + run_width(&shown[0][1]),
            3,
            "in its column"
        );
    }

    #[test]
    fn j_and_k_walk_the_messages_in_the_order_they_are_drawn() {
        let mut items = vec![
            message("dm", "Bo", "new", 60, json!({})),
            message("mention", "Ada", "x", 120, json!({})),
            message("dm", "Cy", "older", 600, json!({})),
        ];
        in_groups(&mut items);
        let from: Vec<&str> = items.iter().map(|i| get(i, "from")).collect();
        assert_eq!(from, ["Ada", "Bo", "Cy"]);
    }

    #[test]
    fn the_thread_in_the_lists_place_scrolls_with_j_k_and_the_wheel() {
        let tz = TimeZone::UTC;
        let pictures = HashMap::new();
        let thread = thread();
        let look = Look {
            cols: 120,
            rows: 6,
            top_row: 1,
            cursor: Some(0),
            theme: &Theme::DARK,
            caps: false,
            now_ms: NOW,
            tz: &tz,
            thread: Some(&thread),
            threads_on: true,
            pictures: &pictures,
            graphics: false,
            work: &[],
            tabs: &[],
            side_top: 0,
            thread_open: true,
            failed: None,
            files_failed: &Default::default(),
            files_kept: &Default::default(),
            hover: None,
        };
        let (_, hits, _) = board(&view(), &look);
        assert!(
            hits.iter()
                .any(|h| matches!(h.spot, Spot::Side(last) if last > 0))
        );
    }

    #[test]
    fn the_board_lays_out_by_the_screens_width() {
        let thread = thread();
        // At 240 and 160 the list and the pane share the width equally.
        for cols in [240, 160] {
            let (lines, _) = draw(cols, Some(&thread), false, &[]);
            let rule = lines[2]
                .find('│')
                .map(|b| lines[2][..b].chars().count())
                .unwrap();
            assert!(rule.abs_diff(cols / 2) <= 1, "{cols}: the rule at {rule}");
            assert!(
                lines
                    .iter()
                    .any(|l| l.contains("Ada") && l.contains("#dev")),
                "where, at {cols}"
            );
        }
        // At 120 where goes and the thread is behind i.
        let (lines, _) = draw(120, Some(&thread), false, &[]);
        assert!(!lines.iter().any(|l| l.contains('│')), "no pane");
        assert!(
            !lines
                .iter()
                .any(|l| l.contains("Ada") && l.contains("#dev")),
            "no where"
        );
        assert!(
            lines.iter().any(|l| l.contains("Ada") && l.contains("2m")),
            "how long ago kept"
        );
        let (lines, _) = draw(120, Some(&thread), true, &[]);
        let all = lines.join("\n");
        assert!(
            all.contains("thread · 2 replies") && !all.contains("MENTIONS"),
            "i shows the thread in the list's place"
        );
    }
}
