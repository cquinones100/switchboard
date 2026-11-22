//! UX 2.0's Slack view, board 21 of the canvas ("Slack: what is new for
//! you, by section as Slack's sidebar lists them", each section's rows now
//! oldest to newest as a conversation reads, where the board drew them
//! newest first):
//! a column of sections as Slack's own sidebar lists them (Mentions,
//! Threads, your reminders that are due while your own Slack sign-in
//! lists them, each direct message by its people, each channel by its name),
//! each with its count, bold in the accent while it holds something new;
//! beside it the chosen section one row per thread (`in_order`), so a
//! thread with several new replies is one row and nothing shows twice
//! beside the thread pane, the rows by when each last moved, oldest to newest as a conversation reads, new and seen
//! together: who started it, with their Large avatar (`kit::avatar`),
//! "started <day>" when that was before the look, and when it last moved;
//! its first message on one line; and for a thread its people as Small
//! avatars, its replies and an "N new" chip; and beside the list, the
//! chosen row's whole thread, each message with its sender's Large
//! avatar, replies since you last looked marked with a bar and a "new" chip, its pictures and videos' first frames, then
//! "From your work": the tickets the thread names, with their state and the
//! tab on them, and today's events it names, with how soon. New with UX
//! 2.0, with no tmux-version file behind it.
//!
//! It draws the view's own items (`day::slack::slack_view`), so the cursor,
//! the filter and the menu work on them as on the plain list; the thread
//! comes from `slack-threads.json` (`day::slack_thread`) by way of the
//! snapshot, and its pictures from those the server has read and decoded,
//! so drawing reads nothing. The cursor stays the view's own, on a
//! row: the section in front is the chosen row's, as the Keys
//! board's group is its chosen key's, so the filter and Enter work as on
//! the plain list (`in_order` puts each section's messages together).
//! Laid out as board 45 says (`kit::columns`): the sections at a fixed
//! `SECTIONS_W`, the list and the pane stretching by `LIST_WEIGHT` and
//! `PANE_WEIGHT`, the pane never under `PANE_LEAST`; under `SECTIONS_AT`
//! columns the sections fold into a line above the messages; below
//! `NAMES_AT` the names beside the list's pictures go, then where each
//! message was, then From your work's states; under `BESIDE_AT` the pane
//! goes behind `i`, drawn in the list's place until `i` or Esc.

use std::collections::HashMap;

use jiff::tz::TimeZone;
use ratatui::style::Style;
use ratatui::text::Span;
use serde_json::Value;

use super::diagrams::Ready;
use super::render::{Image, Shape};
use super::views::{View, ViewId};
use super::{HitBox, Spot};
use crate::day::slack_emoji::{self, Custom, Piece};
use crate::day::slack_thread::{Said, SlackFile, Thread};
use crate::items::Item;
use crate::photos::{Service, Who};
use crate::sidebar::model::TabModel;
use crate::sidebar::render::Line;
use crate::sidebar::text::{cut, width, wrap};
use crate::ui::Theme;
use crate::ui::kit::{self, Col, Tone};
use crate::views::ticket::Ticket;

/// Under this many columns the thread pane goes behind `i`.
pub const BESIDE_AT: usize = 120;
/// Under this many, the names beside the list's pictures, where each
/// message was and From your work's states go.
pub const NAMES_AT: usize = 120;
/// Under this many the sections' column folds into a line above the
/// messages.
pub const SECTIONS_AT: usize = 100;
/// The sections' column, as the board's.
pub const SECTIONS_W: usize = 23;
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
    /// The section under the mouse, by its newest row, lit.
    pub hover_section: Option<usize>,
    /// The sections' column has the focus (`h`, `C-w h`).
    pub sections_focused: bool,
    /// Settings' `slack_emoji`: a workspace's own emoji are drawn.
    pub emoji_on: bool,
    /// What the workspace's own emoji the board shows are, those loaded,
    /// by name.
    pub emoji: &'a HashMap<String, Custom>,
}

/// A piece of a line: text, someone's avatar (`kit::avatar`, `rows` of
/// it showing), or a picture placed with its top-left corner here.
enum Run {
    Text(Span<'static>),
    Avatar {
        who: Option<Who>,
        name: String,
        size: kit::AvatarSize,
        rows: usize,
    },
    Picture {
        id: String,
        cols: usize,
        rows: usize,
    },
    /// A workspace's own emoji (`kit::emoji`), its picture placed over it
    /// once loaded.
    Emoji {
        name: String,
    },
}

fn texts(spans: Vec<Span<'static>>) -> Vec<Run> {
    spans.into_iter().map(Run::Text).collect()
}

fn get<'a>(item: &'a Item, key: &str) -> &'a str {
    item.data.get(key).and_then(Value::as_str).unwrap_or("")
}

/// What a section holds, in the order the column lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Mentions,
    Threads,
    /// Your reminders that are due (`day::slack_user`).
    Reminders,
    Direct,
    Channel,
}

/// A section of the column: what it holds, its name, a direct message's
/// person (for their avatar), its rows' places in the view's items,
/// oldest first, how many messages they stand for, and whether any of them
/// is new.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub kind: Kind,
    pub title: String,
    pub user: String,
    pub items: Vec<usize>,
    pub messages: usize,
    pub new: bool,
}

impl Section {
    /// Its newest row, at the bottom, where moving to it opens.
    pub fn newest(&self) -> usize {
        self.items.last().copied().unwrap_or(0)
    }
}

/// The section a message is in: Mentions and Threads whole, a direct
/// message or a channel's by its conversation.
fn section_key(item: &Item) -> (Kind, String) {
    let conversation = || {
        item.data
            .pointer("/message/channel")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| get(item, "where"))
            .to_string()
    };
    match get(item, "kind") {
        "mention" => (Kind::Mentions, String::new()),
        "thread" => (Kind::Threads, String::new()),
        "reminder" => (Kind::Reminders, String::new()),
        "dm" => (Kind::Direct, conversation()),
        _ => (Kind::Channel, conversation()),
    }
}

fn when(item: &Item) -> f64 {
    get(item, "ts").parse::<f64>().unwrap_or(0.0)
}

/// The board's rows in the order it draws them: by section (Mentions,
/// Threads, then the direct messages and the channels, each by its newest
/// message), each section's threads by their newest message, oldest to
/// newest as a conversation reads, new and seen together. A thread is one row however many of its messages the look
/// brought, so nothing shows twice beside the thread pane: its newest
/// message, kept by its section and thread so the cursor stays on it as
/// replies come and old ones go, with every message it stands for in
/// `members` (`members`), a thread of one message too.
/// With Settings' `slack_threads` off (`by_thread` false) no thread is
/// read and no pane shows one, so each message stays a row of its own.
pub fn in_order(items: &mut Vec<Item>, by_thread: bool) {
    let mut newest: HashMap<(Kind, String), f64> = HashMap::new();
    for i in items.iter() {
        let at = newest.entry(section_key(i)).or_insert(f64::MIN);
        *at = at.max(when(i));
    }
    items.sort_by(|a, b| {
        let (ka, kb) = (section_key(a), section_key(b));
        ka.0.cmp(&kb.0)
            .then(newest[&kb].total_cmp(&newest[&ka]))
            .then(ka.1.cmp(&kb.1))
            .then(when(b).total_cmp(&when(a)))
    });
    if by_thread {
        by_threads(items);
    }
    oldest_first(items);
}

/// Each section's rows, which the sort left newest first, oldest first,
/// the sections keeping their order.
fn oldest_first(items: &mut [Item]) {
    let mut start = 0;
    while start < items.len() {
        let key = section_key(&items[start]);
        let len = items[start..]
            .iter()
            .take_while(|i| section_key(i) == key)
            .count();
        items[start..start + len].reverse();
        start += len;
    }
}

/// Each thread's messages, newest first, made one row at its newest.
fn by_threads(items: &mut Vec<Item>) {
    let mut rows: Vec<Vec<Item>> = Vec::new();
    let mut place: HashMap<((Kind, String), String), usize> = HashMap::new();
    for item in items.drain(..) {
        let thread = get(&item, "thread").to_string();
        if thread.is_empty() {
            rows.push(vec![item]);
            continue;
        }
        match place.entry((section_key(&item), thread)) {
            std::collections::hash_map::Entry::Occupied(at) => rows[*at.get()].push(item),
            std::collections::hash_map::Entry::Vacant(at) => {
                at.insert(rows.len());
                rows.push(vec![item]);
            }
        }
    }
    items.extend(rows.into_iter().map(thread_row));
}

fn is_old(item: &Item) -> bool {
    item.data.get("old").and_then(Value::as_bool) == Some(true)
}

/// One row for a thread's messages, newest first.
fn thread_row(mut messages: Vec<Item>) -> Item {
    let thread = get(&messages[0], "thread").to_string();
    if thread.is_empty() {
        return messages.remove(0);
    }
    let fresh = messages.iter().filter(|m| !is_old(m)).count();
    // Its newest message whole (its words, sender, title and link, which
    // its menu quotes and hands to a tab), new while any of them is.
    let mut row = messages[0].clone();
    if fresh > 0
        && is_old(&row)
        && let Some(new) = messages.iter().find(|m| !is_old(m))
    {
        row.state = new.state;
        row.title = row.title.trim_end_matches(" · seen").to_string();
    }
    let (kind, conversation) = section_key(&row);
    row.id = format!("slack-thread:{kind:?}:{conversation}:{thread}");
    if let Some(data) = row.data.as_object_mut() {
        data.insert("old".into(), Value::Bool(fresh == 0));
        data.insert("fresh".into(), fresh.into());
        data.insert(
            "members".into(),
            serde_json::to_value(&messages).unwrap_or_default(),
        );
    }
    row
}

/// The messages a row stands for: a thread's, or the message itself.
pub fn members(item: &Item) -> Vec<Item> {
    item.data
        .get("members")
        .and_then(|m| serde_json::from_value(m.clone()).ok())
        .unwrap_or_else(|| vec![item.clone()])
}

/// How many messages a row stands for, and how many of them are new.
fn counts(item: &Item) -> (usize, usize) {
    match item.data.get("members").and_then(Value::as_array) {
        Some(all) => (
            all.len(),
            item.data.get("fresh").and_then(Value::as_u64).unwrap_or(0) as usize,
        ),
        None => (1, usize::from(!is_old(item))),
    }
}

/// The column's sections, from items `in_order` put in order.
pub fn sections(items: &[Item]) -> Vec<Section> {
    let mut out: Vec<(String, Section)> = Vec::new();
    for (n, item) in items.iter().enumerate() {
        let (kind, key) = section_key(item);
        let new = item.data.get("old").and_then(Value::as_bool) != Some(true);
        if let Some((k, s)) = out.last_mut()
            && s.kind == kind
            && *k == key
        {
            s.items.push(n);
            s.messages += counts(item).0;
            s.new |= new;
            continue;
        }
        let title = match kind {
            Kind::Mentions => "Mentions".to_string(),
            Kind::Threads => "Threads".to_string(),
            Kind::Reminders => "Reminders".to_string(),
            Kind::Direct => match (get(item, "where"), get(item, "from")) {
                ("", "") => "Someone".to_string(),
                ("", from) => from.to_string(),
                (place, _) => place.to_string(),
            },
            Kind::Channel => match get(item, "where").trim_start_matches('#') {
                "" => "Slack".to_string(),
                name => name.to_string(),
            },
        };
        let user = if kind == Kind::Direct {
            get(item, "user").to_string()
        } else {
            String::new()
        };
        out.push((
            key,
            Section {
                kind,
                title,
                user,
                items: vec![n],
                messages: counts(item).0,
                new,
            },
        ));
    }
    out.into_iter().map(|(_, s)| s).collect()
}

/// The place of the section holding message `cursor`.
fn section_at(sections: &[Section], cursor: isize) -> Option<usize> {
    let c = usize::try_from(cursor).ok()?;
    sections.iter().position(|s| s.items.contains(&c))
}

/// `j` and `k` in the sections' column, or `h` and `l` on the line
/// they fold into: the newest row of the section `by` from the chosen
/// one's, staying at the ends, as Slack opens a conversation at its end.
pub fn section_step(items: &[Item], cursor: isize, by: isize) -> isize {
    let all = sections(items);
    let Some(at) = section_at(&all, cursor).or((!all.is_empty()).then_some(0)) else {
        return cursor;
    };
    let to = at.saturating_add_signed(by).min(all.len() - 1);
    all[to].newest() as isize
}

/// Whether row `cursor` is its section's newest, at the bottom, where a
/// new message's row keeps the cursor following it.
pub fn on_newest(items: &[Item], cursor: isize) -> bool {
    cursor >= 0 && section_step(items, cursor, 0) == cursor
}

/// The section holding row `cursor`, by what it holds and its
/// conversation.
pub fn section_key_at(items: &[Item], cursor: isize) -> Option<(Kind, String)> {
    usize::try_from(cursor)
        .ok()
        .and_then(|c| items.get(c))
        .map(section_key)
}

/// The newest row of the section `key` names, while it is listed.
pub fn newest_in(items: &[Item], key: &(Kind, String)) -> Option<isize> {
    items
        .iter()
        .rposition(|i| section_key(i) == *key)
        .map(|i| i as isize)
}

/// `j` and `k` in the messages: the message `by` from the chosen one
/// within its section, staying at its ends.
pub fn row_step(items: &[Item], cursor: isize, by: isize) -> isize {
    let all = sections(items);
    let Some(section) = section_at(&all, cursor).map(|s| &all[s]) else {
        return section_step(items, cursor, 0);
    };
    let at = section
        .items
        .iter()
        .position(|i| *i as isize == cursor)
        .unwrap_or(0);
    let to = at.saturating_add_signed(by).min(section.items.len() - 1);
    section.items[to] as isize
}

/// The messages the board shows with message `cursor` chosen: its
/// section's, or the first section's with none chosen.
pub fn shown(items: &[Item], cursor: isize) -> Vec<usize> {
    let mut all = sections(items);
    match section_at(&all, cursor).or((!all.is_empty()).then_some(0)) {
        Some(at) => all.swap_remove(at).items,
        None => Vec::new(),
    }
}

/// Whether the pane is drawn beside the list on a screen this wide.
pub fn beside(cols: usize) -> bool {
    cols >= BESIDE_AT
}

/// Whether the sections are a column of their own on a screen this wide,
/// rather than a line above the messages.
pub fn column(cols: usize) -> bool {
    cols >= SECTIONS_AT
}

/// The people the board draws, for their photos: the senders of the
/// listed messages, who started each row's thread and who is in it, and
/// the chosen thread's senders.
pub fn people(view: &View, thread: Option<&Thread>) -> Vec<Who> {
    let mut out: Vec<Who> = Vec::new();
    let users = view
        .data
        .items
        .iter()
        .flat_map(|i| {
            // A row draws its thread's starter and its people too.
            let mut users = vec![get(i, "user").to_string(), root_of(i).0.user];
            users.extend(people_of(i).into_iter().map(|(user, _)| user));
            users
        })
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
    let mut rows = look.rows.saturating_sub(1);
    let mut top_row = look.top_row + 1;
    // What your own Slack sign-in would add, said once, dim, with none.
    if let Some(note) = view.data.head.get("note").and_then(Value::as_str) {
        lines.push(Line {
            spans: kit::fit(
                vec![Span::styled(format!(" {note}"), Tone::Dim.style(theme))],
                cols,
                Style::new(),
            ),
            fill: None,
        });
        rows = rows.saturating_sub(1);
        top_row += 1;
    }

    let secs = sections(&view.data.items);
    let front = look
        .cursor
        .and_then(|c| section_at(&secs, c as isize))
        .or((!secs.is_empty()).then_some(0));
    let in_column = column(cols) && !secs.is_empty();
    // Narrower, the sections fold into a line above the messages, while
    // the thread isn't in their place.
    if !in_column && !secs.is_empty() && !(look.threads_on && look.thread_open && !beside(cols)) {
        let (spans, at) = section_line(&secs, front, look, cols);
        for (x0, x1, newest) in at {
            hits.push(HitBox {
                y: top_row as u16,
                x0: x0 as u16,
                x1: x1.saturating_sub(1) as u16,
                spot: Spot::Choose(newest as isize),
            });
        }
        lines.push(Line {
            spans: kit::fit(spans, cols, Style::new()),
            fill: None,
        });
        rows = rows.saturating_sub(1);
        top_row += 1;
    }
    // The sections' column and the rule after it; the rest beside them.
    let left = if in_column { SECTIONS_W + 1 } else { 0 };
    let rest = cols.saturating_sub(left);
    let shift = |at: Option<(usize, usize)>| at.map(|(x, w)| (x + left, w));

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
            rest,
            1,
            &[],
        );
        (shift(at[0]), shift(at[2]))
    } else if look.threads_on && look.thread_open {
        (None, Some((1, cols.saturating_sub(2))))
    } else {
        (Some((left, rest)), None)
    };
    let in_column = in_column && list_at.is_some();
    if list_at.is_none() && pane_at.is_some() {
        hits.push(HitBox::in_place(top_row));
    }

    let chosen = look.cursor.and_then(|c| view.data.items.get(c));
    let section = front.map(|s| &secs[s]);
    let (list, owners, list_scroll) = match list_at {
        Some((_, w)) => list_lines(view, section, look, w, rows),
        None => (Vec::new(), Vec::new(), 0),
    };
    let (column_lines, column_owners, column_scroll) = if in_column {
        section_lines(&secs, front, look, SECTIONS_W, rows)
    } else {
        (Vec::new(), Vec::new(), 0)
    };
    let pane = match (pane_at, chosen) {
        (Some((_, w)), Some(item)) => {
            // Since you last looked at the look, or at this thread when
            // the board left the look's other sections unseen.
            let seen = view
                .data
                .head
                .get("seen")
                .and_then(Value::as_i64)
                .unwrap_or(0)
                .max(
                    view.data
                        .head
                        .pointer(&format!(
                            "/read_threads/{}",
                            get(item, "thread").replace('~', "~0").replace('/', "~1")
                        ))
                        .and_then(Value::as_i64)
                        .unwrap_or(0),
                );
            let seen_alone: Vec<String> = view
                .data
                .head
                .get("seen_alone")
                .and_then(Value::as_array)
                .map(|all| {
                    all.iter()
                        .filter_map(|k| k.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            thread_lines(item, look, w, seen, &seen_alone)
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
    let mut column_rows = window(column_lines, column_scroll, rows).into_iter();
    let mut column_owners = column_owners.into_iter().skip(column_scroll);
    let mut pane = window(pane, pane_top, rows).into_iter();
    for r in 0..rows {
        let y = top_row + r;
        let mut line: Vec<Span<'static>> = Vec::new();
        if in_column {
            let runs = column_rows.next().unwrap_or_default();
            put(
                &mut line,
                runs,
                (0, SECTIONS_W),
                y,
                look,
                &mut images,
                "slack-sections",
            );
            if let Some(Some(first)) = column_owners.next() {
                hits.push(HitBox {
                    y: y as u16,
                    x0: 0,
                    x1: (SECTIONS_W - 1) as u16,
                    spot: Spot::Choose(first as isize),
                });
            }
            kit::place(&mut line, vec![kit::rule_v(theme)], (SECTIONS_W, 1));
        }
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
            // An avatar running past the last row shows in the rows left.
            if let Run::Avatar { rows: high, .. } = run
                && i >= top
                && i + *high > end
            {
                *high = end - i;
            }
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
        Run::Avatar { size, .. } => size.cols(),
        Run::Picture { cols, .. } => *cols,
        Run::Emoji { .. } => kit::EMOJI_COLS,
    }
}

/// A Large avatar on a message's first line, its name and words
/// `kit::SAID_INDENT` columns in.
fn large_avatar(who: Option<Who>, name: &str) -> Run {
    Run::Avatar {
        who,
        name: name.to_string(),
        size: kit::AvatarSize::Large,
        rows: kit::AvatarSize::Large.rows(),
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
            Run::Avatar {
                who,
                name: person,
                size,
                rows,
            } => {
                if x + size.cols() > w {
                    continue;
                }
                let mut image = Image::avatar(
                    format!("{name}-avatar-{y}-{}", x0 + x),
                    (x0 + x, y),
                    who,
                    &person,
                    size,
                );
                image.height = rows as u16;
                images.push(image);
                spans.extend(kit::avatar(&person, size).remove(0));
                x += size.cols();
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
            Run::Emoji { name: emoji } => {
                if x + kit::EMOJI_COLS > w {
                    continue;
                }
                if look.graphics && matches!(look.emoji.get(&emoji), Some(Custom::Picture(_))) {
                    images.push(Image {
                        name: format!("{name}-emoji-{y}-{}", x0 + x),
                        x: (x0 + x) as u16,
                        y: y as u16,
                        width: kit::EMOJI_COLS as u16,
                        height: 1,
                        shape: Shape::Emoji {
                            name: emoji.clone(),
                        },
                    });
                }
                spans.push(kit::emoji(&emoji, look.theme));
                x += kit::EMOJI_COLS;
            }
        }
    }
    kit::place(line, spans, (x0, w));
}

/// The chosen section's rows: its name and facts, then each row (a thread,
/// or a message with no replies), oldest to newest as a conversation
/// reads, a blank line after each; which item each line belongs to, and
/// the first line shown so the chosen message is on the screen, at the
/// bottom when the section is longer than the pane.
fn list_lines(
    view: &View,
    section: Option<&Section>,
    look: &Look,
    w: usize,
    rows: usize,
) -> (Vec<Vec<Run>>, Vec<Option<usize>>, usize) {
    let theme = look.theme;
    let Some(section) = section else {
        return (
            vec![texts(vec![Span::styled(
                " Nothing new on Slack.",
                Tone::Dim.style(theme),
            )])],
            vec![None],
            0,
        );
    };
    let mut out: Vec<Vec<Run>> = Vec::new();
    let mut owners = Vec::new();
    let listed: Vec<&Item> = section.items.iter().map(|i| &view.data.items[*i]).collect();
    let fresh: usize = listed.iter().map(|r| counts(r).1).sum();
    // Threads with new replies: rows standing for a new reply.
    // Each thread once, as with threads off its messages are rows apart.
    let threads = listed
        .iter()
        .filter(|r| members(r).iter().any(|m| !is_old(m) && is_reply(m)))
        .map(|r| get(r, "thread"))
        .collect::<std::collections::HashSet<_>>()
        .len();
    let mut facts = Vec::new();
    match threads {
        0 => {}
        1 => facts.push("1 thread with new replies".to_string()),
        n => facts.push(format!("{n} threads with new replies")),
    }
    if fresh > 0 {
        facts.push(format!("{fresh} new"));
    }
    facts.push("oldest first".into());
    let title = match section.kind {
        Kind::Channel => format!("#{}", section.title),
        _ => section.title.clone(),
    };
    for line in kit::pane(
        &title,
        vec![Span::styled(facts.join(" · "), Tone::Dim.style(theme))],
        w.saturating_sub(1),
        theme,
    ) {
        let mut runs = vec![Run::Text(Span::raw(" "))];
        runs.extend(texts(line));
        out.push(runs);
        owners.push(None);
    }
    // What is older than the rows, above them, as Slack keeps it there.
    let before = match section.kind {
        Kind::Reminders => " A reminder stays until it is complete in Slack.",
        _ => " Older messages stay in Slack.",
    };
    out.push(texts(vec![Span::styled(before, Tone::Faint.style(theme))]));
    owners.push(None);
    out.push(Vec::new());
    owners.push(None);
    // With none of its rows chosen, the section shows at its newest.
    let mut chosen_at = None;
    for &i in &section.items {
        let item = &view.data.items[i];
        let start = out.len();
        let lit = look.cursor != Some(i) && look.hover == Some(i);
        let lines = message_lines(item, look, w, look.cursor == Some(i), lit);
        owners.extend(std::iter::repeat_n(Some(i), lines.len()));
        out.extend(lines);
        if look.cursor == Some(i) {
            chosen_at = Some((start, out.len()));
        }
        out.push(Vec::new());
        owners.push(None);
    }
    // The chosen message whole on the screen, at its bottom while the
    // rows below it are out of sight; a message taller than the screen
    // shows from its top.
    let (start, end) = chosen_at.unwrap_or((out.len(), out.len()));
    let scroll = if end > rows {
        (end - rows).min(start)
    } else {
        0
    };
    (out, owners, scroll)
}

/// A section's mark and name, as the column and the folded line show it.
fn section_name(section: &Section) -> (&'static str, Tone, String) {
    match section.kind {
        Kind::Mentions => ("@", Tone::Working, section.title.clone()),
        Kind::Threads => ("◇", Tone::Working, section.title.clone()),
        Kind::Reminders => ("◷", Tone::Working, section.title.clone()),
        Kind::Direct => ("", Tone::Dim, section.title.clone()),
        Kind::Channel => ("#", Tone::Dim, section.title.clone()),
    }
}

/// The sections' column, `w` wide: Mentions and Threads, then DIRECT
/// MESSAGES (each with its person's Small avatar) and CHANNELS, each with
/// its count at the right edge, bold in the accent while it holds
/// something new; the chosen one on the chosen row (resting while the
/// messages have the focus), the one under the mouse lit. Which section
/// each line belongs to, by its newest row, and the first line shown
/// so the chosen one is on the screen.
fn section_lines(
    secs: &[Section],
    front: Option<usize>,
    look: &Look,
    w: usize,
    rows: usize,
) -> (Vec<Vec<Run>>, Vec<Option<usize>>, usize) {
    let theme = look.theme;
    let mut out: Vec<Vec<Run>> = Vec::new();
    let mut owners = Vec::new();
    let mut chosen_at = 0;
    let mut last: Option<Kind> = None;
    for (n, s) in secs.iter().enumerate() {
        let heading = match s.kind {
            Kind::Direct => Some("Direct messages"),
            Kind::Channel => Some("Channels"),
            _ => None,
        };
        if last != Some(s.kind)
            && let Some(heading) = heading
        {
            if !out.is_empty() {
                out.push(Vec::new());
                owners.push(None);
            }
            out.push(texts(kit::label(heading, Tone::Dim, None, w, theme)));
            owners.push(None);
        }
        last = Some(s.kind);
        let chosen = front == Some(n);
        let newest = s.newest();
        let (mark, tone, name) = section_name(s);
        let strong = s.new || chosen;
        let name_style = if strong {
            Tone::Text.bold(theme)
        } else {
            Tone::Dim.style(theme)
        };
        let count = s.messages.to_string();
        let count_style = if s.new {
            Tone::Accent.bold(theme)
        } else {
            Tone::Dim.style(theme)
        };
        // The bar, then the mark or avatar, the name stretching and the
        // count at the right edge.
        let lead = if s.kind == Kind::Direct {
            kit::AvatarSize::Small.cols()
        } else {
            width(mark)
        };
        let at = kit::columns(
            &[
                Col::Fixed {
                    min: lead,
                    natural: lead,
                    cap: lead,
                },
                Col::Stretch {
                    weight: 1,
                    min: 0,
                    max: None,
                },
                Col::Right {
                    natural: width(&count),
                    cap: width(&count),
                },
            ],
            w.saturating_sub(1),
            1,
            &[],
        );
        let mut runs = vec![bar(chosen && look.sections_focused, theme)];
        let mut x = 0;
        let pad_to = |runs: &mut Vec<Run>, to: usize, x: &mut usize| {
            if to > *x {
                runs.push(Run::Text(Span::raw(" ".repeat(to - *x))));
                *x = to;
            }
        };
        if let Some((mx, _)) = at[0] {
            pad_to(&mut runs, mx, &mut x);
            if s.kind == Kind::Direct {
                runs.push(Run::Avatar {
                    who: (!s.user.is_empty()).then(|| Who::new(Service::Slack, &s.user)),
                    name: s.title.clone(),
                    size: kit::AvatarSize::Small,
                    rows: 1,
                });
            } else {
                runs.push(Run::Text(Span::styled(mark, tone.style(theme))));
            }
            x += lead;
        }
        if let Some((nx, nw)) = at[1] {
            pad_to(&mut runs, nx, &mut x);
            let name = cut(&name, nw);
            x += width(&name);
            runs.push(Run::Text(Span::styled(name, name_style)));
        }
        if let Some((cx, _)) = at[2] {
            pad_to(&mut runs, cx, &mut x);
            runs.push(Run::Text(Span::styled(count, count_style)));
        }
        let lit = !chosen && look.hover_section == Some(newest);
        let runs = if chosen {
            let bg = if look.sections_focused {
                kit::Surface::Chosen.color(theme)
            } else {
                kit::Surface::Raised.color(theme)
            };
            on_surface(runs, w, bg)
        } else if lit {
            on_surface(runs, w, theme.panel)
        } else {
            runs
        };
        if chosen {
            chosen_at = out.len();
        }
        out.push(runs);
        owners.push(Some(newest));
    }
    let scroll = (chosen_at + 1).saturating_sub(rows);
    (out, owners, scroll)
}

/// The sections folded into one line, `cols` wide, on a narrow screen:
/// each its mark, name and count, the chosen one lit, as many as fit and
/// then how many more; where each is, for a click.
fn section_line(
    secs: &[Section],
    front: Option<usize>,
    look: &Look,
    cols: usize,
) -> (Vec<Span<'static>>, Vec<(usize, usize, usize)>) {
    let theme = look.theme;
    // A long name is cut, so even the chosen section fits on the line.
    let cap = cols.saturating_sub(16).clamp(4, 30);
    let named = |s: &Section| {
        let (mark, tone, name) = section_name(s);
        (mark, tone, cut(&name, cap))
    };
    let mut spans = vec![Span::raw(" ")];
    let mut x = 1;
    let mut at = Vec::new();
    // The chosen one always shows: the line starts far enough along.
    let from = front.unwrap_or(0);
    let mut start = 0;
    let piece = |s: &Section| {
        let (mark, _, name) = named(s);
        let mark = if mark.is_empty() {
            String::new()
        } else {
            format!("{mark} ")
        };
        width(&format!(" {mark}{name} {} ", s.messages)) + 1
    };
    while start < from && secs[start..=from].iter().map(piece).sum::<usize>() + 8 > cols {
        start += 1;
    }
    if start > 0 {
        let more = format!("+{start} ");
        x += width(&more);
        spans.push(Span::styled(more, Tone::Dim.style(theme)));
    }
    for (n, s) in secs.iter().enumerate().skip(start) {
        let left = secs.len() - n;
        let tail = if left > 1 { 6 } else { 0 };
        if x + piece(s) + tail > cols {
            spans.push(Span::styled(format!("… +{left}"), Tone::Dim.style(theme)));
            break;
        }
        let chosen = front == Some(n);
        // The chosen one lit, the one under the mouse on the panel's.
        let bg = if chosen {
            Some(kit::Surface::Chosen.color(theme))
        } else {
            (look.hover_section == Some(s.newest())).then_some(theme.panel)
        };
        let on = |style: Style| match bg {
            Some(bg) => style.bg(bg),
            None => style,
        };
        let (mark, tone, name) = named(s);
        let x0 = x;
        let mut piece_spans = vec![Span::styled(" ", on(Style::new()))];
        if !mark.is_empty() {
            piece_spans.push(Span::styled(format!("{mark} "), on(tone.style(theme))));
        }
        let name_style = if s.new || chosen {
            Tone::Text.bold(theme)
        } else {
            Tone::Dim.style(theme)
        };
        piece_spans.push(Span::styled(name, on(name_style)));
        let count_style = if s.new {
            Tone::Accent.bold(theme)
        } else {
            Tone::Dim.style(theme)
        };
        piece_spans.push(Span::styled(format!(" {}", s.messages), on(count_style)));
        piece_spans.push(Span::styled(" ", on(Style::new())));
        x += piece_spans.iter().map(|s| width(&s.content)).sum::<usize>();
        at.push((x0, x, s.newest()));
        spans.extend(piece_spans);
        spans.push(Span::raw(" "));
        x += 1;
    }
    (spans, at)
}

/// `runs` on a surface edge to edge, `w` wide.
fn on_surface(runs: Vec<Run>, w: usize, bg: ratatui::style::Color) -> Vec<Run> {
    let mut used = 0;
    let mut runs: Vec<Run> = runs
        .into_iter()
        .map(|r| match r {
            Run::Text(s) => {
                used += width(&s.content);
                let style = s.style.bg(bg);
                Run::Text(Span::styled(s.content, style))
            }
            Run::Avatar { size, .. } => {
                used += size.cols();
                r
            }
            Run::Picture { cols, .. } => {
                used += cols;
                r
            }
            Run::Emoji { .. } => {
                used += kit::EMOJI_COLS;
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
}

/// How long ago a message's ts was: `4m`, `2h`.
fn ago(ts: &str, now_ms: i64) -> String {
    let secs = ts.parse::<f64>().unwrap_or(0.0) as i64;
    crate::sidebar::text::ago(Some(secs), now_ms.div_euclid(1000))
}

/// The first of the characters a workspace's emoji drawn as a picture
/// stands as while its message is wrapped, each two columns wide as the
/// picture is (`kit::EMOJI_COLS`), so the lines break where it is drawn:
/// the end of plane 3, where no character is assigned.
const EMOJI_MARK: u32 = 0x3f000;
/// How many emoji a message draws as pictures, the rest left as codes.
const EMOJI_MARKS: usize = 0xff0;

/// A message's text wrapped to `w`, `@someone` in the accent, at most
/// `most` lines, each with `indent` before it. While Settings'
/// `slack_emoji` is on, a workspace's own emoji (`slack_emoji::pieces`)
/// is drawn by `kit::emoji` where the terminal draws images, until it is
/// known the workspace has none by its name; else, and for none, it is its
/// code, dimmed; and one that is an alias of a standard emoji is that emoji.
fn body(
    text: &str,
    w: usize,
    most: usize,
    indent: usize,
    tone: Tone,
    look: &Look,
) -> Vec<Vec<Run>> {
    let theme = look.theme;
    let room = w.saturating_sub(indent + 1).min(kit::WRAP_MAX);
    let mut drawn: Vec<&str> = Vec::new();
    let marked = if look.emoji_on {
        let mut marked = String::with_capacity(text.len());
        for piece in slack_emoji::pieces(text) {
            match piece {
                Piece::Text(t) => marked.push_str(t),
                Piece::Custom(name) => match look.emoji.get(name) {
                    Some(Custom::Standard(c)) => marked.push_str(c),
                    Some(Custom::Picture(_)) | None
                        if look.graphics && drawn.len() < EMOJI_MARKS =>
                    {
                        marked.extend(char::from_u32(EMOJI_MARK + drawn.len() as u32));
                        drawn.push(name);
                    }
                    _ => marked.push_str(&format!(":{name}:")),
                },
            }
        }
        marked
    } else {
        text.to_string()
    };
    let mut lines = wrap(&marked, room);
    let cut_short = lines.len() > most;
    lines.truncate(most);
    if cut_short && let Some(last) = lines.last_mut() {
        *last = format!("{}…", cut(last, room.saturating_sub(1)));
    }
    // Words as they are, a code left in them dimmed.
    let words = |runs: &mut Vec<Run>, text: &str, style: Style| {
        if !look.emoji_on {
            runs.push(Run::Text(Span::styled(text.to_string(), style)));
            return;
        }
        for piece in slack_emoji::pieces(text) {
            runs.push(Run::Text(match piece {
                Piece::Text(t) => Span::styled(t.to_string(), style),
                Piece::Custom(name) => Span::styled(format!(":{name}:"), Tone::Dim.style(theme)),
            }));
        }
    };
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
                let mut said = String::new();
                for c in word.chars() {
                    let mark = (c as u32).checked_sub(EMOJI_MARK).map(|i| i as usize);
                    match mark.and_then(|i| drawn.get(i)) {
                        Some(name) => {
                            words(&mut runs, &std::mem::take(&mut said), style);
                            runs.push(Run::Emoji {
                                name: name.to_string(),
                            });
                        }
                        None => said.push(c),
                    }
                }
                if !said.is_empty() || word.is_empty() {
                    words(&mut runs, &said, style);
                }
            }
            runs
        })
        .collect()
}

/// When a thread started before the look: its day ("Tue"), or how long
/// ago when that was today.
fn started(ts: &str, look: &Look) -> String {
    let secs = ts.parse::<f64>().unwrap_or(0.0) as i64;
    let day = |s: i64| {
        jiff::Timestamp::from_second(s)
            .ok()
            .map(|t| t.to_zoned(look.tz.clone()).date())
    };
    match (day(secs), day(look.now_ms.div_euclid(1000))) {
        (Some(then), Some(now)) if then != now => then.strftime("%a").to_string(),
        _ => format!("{} ago", ago(ts, look.now_ms)),
    }
}

/// Who started a row's thread and what they wrote first: the thread's first
/// message once it is read, which may be older than the look, else the
/// oldest message the row stands for; and whether it started before the
/// look brought anything of it.
fn root_of(item: &Item) -> (Said, bool) {
    let all = members(item);
    let listed = said_of(&all[all.len() - 1]);
    match item
        .data
        .get("root")
        .and_then(|r| serde_json::from_value::<Said>(r.clone()).ok())
    {
        Some(root) => {
            let before = !all.iter().any(|m| get(m, "ts") == root.ts);
            (root, before)
        }
        None => (listed, false),
    }
}

/// The words a row draws: its messages', and its thread's first
/// message's.
pub fn words_of(item: &Item) -> Vec<String> {
    let mut out: Vec<String> = members(item).iter().map(|m| m.lines.join("\n")).collect();
    out.push(root_of(item).0.text);
    out
}

/// A message of the view as a thread's message.
fn said_of(item: &Item) -> Said {
    Said {
        user: get(item, "user").to_string(),
        from: get(item, "from").to_string(),
        text: item.lines.join("\n"),
        ts: get(item, "ts").to_string(),
        files: Vec::new(),
        refs: Vec::new(),
    }
}

/// A message that answers another in its thread.
fn is_reply(item: &Item) -> bool {
    item.data
        .pointer("/message/thread")
        .and_then(Value::as_str)
        .is_some_and(|t| !t.is_empty() && t != get(item, "ts"))
}

/// Who is in a row's thread, `(user, name)` each once: everyone in it
/// as it was read, then whoever wrote the messages the row stands for that
/// the read didn't have.
fn people_of(item: &Item) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = item
        .data
        .get("people")
        .and_then(Value::as_array)
        .map(|all| {
            all.iter()
                .map(|p| {
                    let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                    (s("user"), s("from"))
                })
                .collect()
        })
        .unwrap_or_default();
    for m in members(item).iter().rev() {
        let who = (get(m, "user").to_string(), get(m, "from").to_string());
        if !out.iter().any(|(user, _)| *user == who.0) {
            out.push(who);
        }
    }
    out
}

/// A row of the list, as board 21 draws it: who started its thread (their
/// Large avatar and name), "started <day>" when that was before the look,
/// and when it last moved at the right, in the accent while something in
/// it is new; its first message on one line; and for a thread, who is in
/// it (Small avatars), its replies, how many are new, and "↩ replied" when
/// you wrote after it; then its pictures. The chosen one on the chosen
/// surface with the bar.
fn message_lines(item: &Item, look: &Look, w: usize, chosen: bool, lit: bool) -> Vec<Vec<Run>> {
    let theme = look.theme;
    let old = item.data.get("old").and_then(Value::as_bool) == Some(true);
    let tone = if old { Tone::Dim } else { Tone::Text };
    let (listed, fresh) = counts(item);
    // With threads off no row stands for a thread: each is its message.
    let (root, before) = if look.threads_on {
        root_of(item)
    } else {
        (said_of(item), false)
    };
    let from = match root.from.as_str() {
        "" => "Someone",
        f => f,
    };
    // Slack's count when the thread was read, with the replies a look
    // saved since; until it is read, the replies the row stands for.
    let read_to = item.data.get("read_to").and_then(Value::as_f64);
    let saved_after = |from: f64| {
        members(item)
            .iter()
            .filter(|m| is_reply(m) && when(m) > from)
            .count()
    };
    let replies = match item.data.get("replies").and_then(Value::as_u64) {
        Some(n) => n as usize + read_to.map_or(0, saved_after),
        None => saved_after(f64::MIN),
    };
    let is_thread = replies > 0 || listed > 1;
    let place = match get(item, "kind") {
        "mention" | "thread" if look.cols >= NAMES_AT => get(item, "where"),
        _ => "",
    };
    let mut facts = Vec::new();
    if !place.is_empty() {
        facts.push(place.to_string());
    }
    if before {
        facts.push(format!("started {}", started(&root.ts, look)));
    }
    let answered = item.data.get("answered").and_then(Value::as_bool) == Some(true);
    // A message with no replies says you answered it among its facts.
    if answered && !is_thread {
        facts.push("↩ replied".into());
    }
    if get(item, "kind") == "reminder" {
        facts.push("due".into());
    }
    // Marking it read in Slack failed, after the one more try.
    if item.data.get("mark_failed").is_some() {
        facts.push("not marked read".into());
    }
    let facts = facts.join(" · ");
    let wide = look.cols >= NAMES_AT;
    let when = ago(get(item, "ts"), look.now_ms);
    // The bar's column, the avatar, who started it, where and when it
    // started, when it last moved.
    let inner = w.saturating_sub(1);
    let at = kit::columns(
        &[
            Col::Fixed {
                min: kit::AvatarSize::Large.cols(),
                natural: kit::AvatarSize::Large.cols(),
                cap: kit::AvatarSize::Large.cols(),
            },
            Col::Fixed {
                min: 4,
                natural: width(from),
                cap: FROM_CAP,
            },
            Col::Fixed {
                min: 0,
                natural: width(&facts),
                cap: WHERE_CAP,
            },
            Col::Stretch {
                weight: 1,
                min: 0,
                max: None,
            },
            Col::Right {
                natural: width(&when),
                cap: 12,
            },
        ],
        inner,
        1,
        &[2],
    );
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
        first.push(large_avatar(
            (!root.user.is_empty()).then(|| Who::new(Service::Slack, &root.user)),
            from,
        ));
        x += kit::AvatarSize::Large.cols();
    }
    if let Some((fx, fw)) = at[1] {
        pad_to(&mut first, fx, &mut x);
        let name = cut(from, fw);
        x += width(&name);
        first.push(Run::Text(Span::styled(name, tone.bold(theme))));
    }
    if let Some((px, pw)) = at[2] {
        pad_to(&mut first, px, &mut x);
        let words = cut(&facts, pw);
        x += width(&words);
        first.push(Run::Text(Span::styled(words, Tone::Dim.style(theme))));
    }
    if let Some((rx, rw)) = at[4] {
        pad_to(&mut first, rx, &mut x);
        let tone = if fresh > 0 { Tone::Accent } else { Tone::Dim };
        first.push(Run::Text(Span::styled(cut(&when, rw), tone.style(theme))));
    }
    let mut out = vec![first];
    // Its first message on one line, beside the avatar's second row, which
    // stays blank when there are no words.
    let indent = at[1].map_or(kit::SAID_INDENT, |(fx, _)| fx);
    let mut words = body(&root.text, inner, 1, indent, tone, look);
    if words.is_empty() {
        words.push(Vec::new());
    }
    for line in words {
        let mut runs = vec![bar(chosen, theme)];
        runs.extend(line);
        out.push(runs);
    }
    if is_thread {
        let mut runs = vec![bar(chosen, theme), Run::Text(Span::raw(" ".repeat(indent)))];
        let mut x = indent;
        let reply_words = if replies == 1 {
            "1 reply".to_string()
        } else {
            format!("{replies} replies")
        };
        // As many of its people as leave room for its counts.
        let room = inner.saturating_sub(indent + width(&reply_words) + 12);
        for (user, name) in people_of(item) {
            if x + kit::AvatarSize::Small.cols() > indent + room {
                break;
            }
            runs.push(Run::Avatar {
                who: (!user.is_empty()).then(|| Who::new(Service::Slack, &user)),
                name: if name.is_empty() {
                    "Someone".into()
                } else {
                    name
                },
                size: kit::AvatarSize::Small,
                rows: kit::AvatarSize::Small.rows(),
            });
            x += kit::AvatarSize::Small.cols();
        }
        if x > indent {
            runs.push(Run::Text(Span::raw(" ")));
        }
        runs.push(Run::Text(Span::styled(reply_words, Tone::Dim.style(theme))));
        if fresh > 0 {
            runs.push(Run::Text(Span::raw("  ")));
            runs.extend(texts(kit::chip(
                &format!("{fresh} new"),
                Tone::Accent,
                look.caps,
                theme,
            )));
        }
        if answered {
            runs.push(Run::Text(Span::styled(
                "  ↩ replied",
                Tone::Done.style(theme),
            )));
        }
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
        .map(|runs| on_surface(runs, w, bg))
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
        "reminder" => "a reminder of yours, due",
        _ => "from a channel you follow",
    }
}

/// The chosen message's thread, `w` wide: where it was and its facts, each
/// message with its sender's photo, how long ago and its text, replies
/// since you last looked marked; its pictures and videos; then From your
/// work.
fn thread_lines(
    item: &Item,
    look: &Look,
    w: usize,
    seen: i64,
    seen_alone: &[String],
) -> Vec<Vec<Run>> {
    let theme = look.theme;
    let dim = Tone::Dim.style(theme);
    let place = match get(item, "where") {
        "" => "Slack",
        p => p,
    };
    let kind = get(item, "kind");
    let fresh_after = |m: &Said| (m.ts.parse::<f64>().unwrap_or(0.0) * 1000.0) as i64 > seen;
    // The thread as read, with every message the row stands for after it
    // (all of them until it is read, newer replies a look saved since),
    // oldest first, so none of them is out of sight.
    let at = |m: &Said| m.ts.parse::<f64>().unwrap_or(0.0);
    let mut listed: Vec<Said> = look.thread.map(|t| t.messages.clone()).unwrap_or_default();
    let read_to = listed.iter().map(at).fold(f64::MIN, f64::max);
    let mut saved_since = 0;
    for m in members(item).iter().rev() {
        let said = said_of(m);
        if at(&said) > read_to {
            listed.push(said);
            saved_since += 1;
        }
    }
    listed.sort_by(|a, b| at(a).total_cmp(&at(b)));
    let messages: &[Said] = &listed;
    let first_is_root = look.thread.is_some() || !members(item).last().is_some_and(is_reply);
    // A message seen on its own (acted on from its menu) isn't new,
    // whatever its time, in any section: the look's, then this row's.
    let channel = item
        .data
        .pointer("/message/channel")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut seen_alone: Vec<String> = seen_alone
        .iter()
        .filter_map(|k| k.strip_prefix(&format!("{channel}:")).map(str::to_string))
        .collect();
    seen_alone.extend(
        members(item)
            .iter()
            .filter(|m| is_old(m))
            .map(|m| get(m, "ts").to_string()),
    );
    let fresh_after = |m: &Said| fresh_after(m) && !seen_alone.contains(&m.ts);
    let replies = messages.len() - usize::from(first_is_root);
    // Slack's count when it knows more than are kept, with the replies a
    // look saved since, as the row counts them.
    let replies = look
        .thread
        .map_or(replies, |t| replies.max(t.replies as usize + saved_since));
    let mut facts = Vec::new();
    if replies > 0 {
        let fresh = messages
            .iter()
            .skip(usize::from(first_is_root))
            .filter(|m| fresh_after(m))
            .count();
        let mut words = format!(
            "thread · {replies} repl{}",
            if replies == 1 { "y" } else { "ies" }
        );
        if fresh > 0 {
            words.push_str(&format!(", {fresh} new"));
        }
        facts.push(words);
    } else {
        // A message with none says what brought it.
        if look.thread.is_some() {
            facts.push("no replies".into());
        }
        facts.push(brought_by(kind).to_string());
    }
    let mut out: Vec<Vec<Run>> = Vec::new();
    out.extend(
        kit::pane(place, vec![Span::styled(facts.join(" · "), dim)], w, theme)
            .into_iter()
            .map(texts),
    );
    for (n, m) in messages.iter().enumerate() {
        let fresh = (n > 0 || !first_is_root) && fresh_after(m);
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
            large_avatar(
                (!m.user.is_empty()).then(|| Who::new(Service::Slack, &m.user)),
                from,
            ),
            Run::Text(Span::raw(" ")),
            Run::Text(Span::styled(cut(from, FROM_CAP), Tone::Text.bold(theme))),
            Run::Text(Span::styled(format!("  {}", ago(&m.ts, look.now_ms)), dim)),
        ];
        if fresh {
            head.push(Run::Text(Span::raw("  ")));
            head.extend(texts(kit::chip("new", Tone::Accent, look.caps, theme)));
        }
        out.push(head);
        let mut words = body(&m.text, w, usize::MAX, kit::SAID_INDENT, Tone::Text, look);
        if words.is_empty() {
            words.push(Vec::new());
        }
        for line in words {
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
                (true, _) => format!("  ▶ {} · a click or Enter's menu plays it", f.name),
                (false, true) => format!("  {} · a click or Enter's menu shows it", f.name),
                (false, false) if look.graphics && !look.files_kept.contains(&f.id) => {
                    format!("  ▣ {} · loading", f.name)
                }
                (false, false) => format!("  ▣ {} · a click or Enter's menu shows it", f.name),
            };
            out.push(vec![mark_again(&mark), Run::Text(Span::styled(words, dim))]);
        }
    }
    // Marking it read in Slack failed: why, whole.
    if let Some(why) = item.data.get("mark_failed").and_then(Value::as_str) {
        out.push(Vec::new());
        for line in wrap(
            &format!("Couldn't mark it read in Slack: {why}"),
            w.saturating_sub(2),
        ) {
            out.push(texts(vec![Span::styled(
                format!(" {line}"),
                Tone::Needs.style(theme),
            )]));
        }
    }
    // Why it couldn't be read, under what was kept when it was read before;
    // a reminder has no thread to read.
    if look.threads_on && kind != "reminder" && (look.thread.is_none() || look.failed.is_some()) {
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
/// the Tickets board and the tab on it, or "no tab yet", then each GitHub
/// issue it links or names (`owner/repo#12`) with the tab on it and the
/// title its tab shows; then today's events it names, with how soon they
/// start.
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
    let issues = crate::views::ticket::issues_in(&text);
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
    if ids.is_empty() && issues.is_empty() && events.is_empty() {
        return Vec::new();
    }
    let states = look.cols >= NAMES_AT;
    let mut out = vec![
        Vec::new(),
        vec![Run::Text(kit::head_rule(w, theme))],
        texts(kit::label("From your work", Tone::Dim, None, w, theme)),
    ];
    // Each ticket as its row shows it: what it is called, its title, its
    // state, and the tab on it.
    let mut rows: Vec<(String, String, String, Option<&TabModel>)> = Vec::new();
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
        let linear = Ticket::Linear(id.clone());
        let tab = look.tabs.iter().find(|t| {
            t.ticket.as_ref() == Some(&linear)
                || crate::views::ticket::id_in_name(&t.ticket_name).as_deref() == Some(id.as_str())
        });
        rows.push((id, title, state, tab));
    }
    for issue in issues {
        let tab = look.tabs.iter().find(|t| t.ticket.as_ref() == Some(&issue));
        let short = issue.short();
        // The title its tab's label gives, `#12: <title>`.
        let title = tab
            .and_then(|t| t.name.strip_prefix(&format!("{short}: ")))
            .unwrap_or("")
            .to_string();
        rows.push((short, title, String::new(), tab));
    }
    for (id, title, state, tab) in rows {
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
        draw_of(&view(), cols, thread, open, work)
    }

    fn draw_of(
        view: &View,
        cols: usize,
        thread: Option<&Thread>,
        open: bool,
        work: &[View],
    ) -> (Vec<String>, Vec<Image>) {
        draw_with_tabs(view, cols, thread, open, work, &[])
    }

    fn draw_with_tabs(
        view: &View,
        cols: usize,
        thread: Option<&Thread>,
        open: bool,
        work: &[View],
        tabs: &[TabModel],
    ) -> (Vec<String>, Vec<Image>) {
        let tz = TimeZone::UTC;
        let pictures = HashMap::new();
        let look = Look {
            cols,
            rows: 40,
            top_row: 1,
            // As the board opens: the first section at its newest row.
            cursor: Some(section_step(&view.data.items, -1, 0) as usize),
            theme: &Theme::DARK,
            caps: false,
            now_ms: NOW,
            tz: &tz,
            thread,
            threads_on: true,
            pictures: &pictures,
            graphics: false,
            work,
            tabs,
            side_top: 0,
            thread_open: open,
            failed: None,
            files_failed: &Default::default(),
            files_kept: &Default::default(),
            hover: None,
            hover_section: None,
            sections_focused: false,
            emoji_on: true,
            emoji: &Default::default(),
        };
        let (lines, _, images) = board(view, &look);
        for l in &lines {
            assert!(
                width(&l.text()) <= cols,
                "a line fits {cols} columns: {:?}",
                l.text()
            );
        }
        (texts(&lines), images)
    }

    /// A message's words as `body` lays them out on a board drawing images
    /// (`graphics`) or not, with what is known of its emoji: each line's
    /// text, an emoji as `{name}`, and the dimmed words.
    fn emoji_body(
        text: &str,
        emoji: &HashMap<String, Custom>,
        graphics: bool,
        on: bool,
    ) -> (Vec<String>, Vec<String>) {
        let tz = TimeZone::UTC;
        let pictures = HashMap::new();
        let look = Look {
            cols: 100,
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
            graphics,
            work: &[],
            tabs: &[],
            side_top: 0,
            thread_open: false,
            failed: None,
            files_failed: &Default::default(),
            files_kept: &Default::default(),
            hover: None,
            hover_section: None,
            sections_focused: false,
            emoji_on: on,
            emoji,
        };
        let dim = Tone::Dim.style(&Theme::DARK);
        let mut dimmed = Vec::new();
        let lines = body(text, 40, usize::MAX, 0, Tone::Text, &look)
            .into_iter()
            .map(|runs| {
                let mut line = String::new();
                for run in runs {
                    match run {
                        Run::Emoji { name } => line.push_str(&format!("{{{name}}}")),
                        Run::Text(s) => {
                            if s.style == dim {
                                dimmed.push(s.content.to_string());
                            }
                            line.push_str(&s.content);
                        }
                        _ => {}
                    }
                }
                line
            })
            .collect();
        (lines, dimmed)
    }

    #[test]
    fn a_workspaces_emoji_is_two_columns_while_its_picture_is_had_or_awaited() {
        let none = HashMap::new();
        let (lines, dimmed) = emoji_body("ship :shipit: now", &none, true, true);
        assert_eq!(lines, vec!["ship {shipit} now"]);
        assert!(dimmed.is_empty(), "{dimmed:?}");
        let had = HashMap::from([(
            "shipit".to_string(),
            Custom::Picture(std::sync::Arc::new(image::RgbaImage::new(4, 4))),
        )]);
        assert_eq!(
            emoji_body("ship :shipit: now", &had, true, true).0,
            vec!["ship {shipit} now"]
        );
        // Wrapped as two columns each: thirteen and their spaces fill 39.
        let many = vec![":shipit:"; 14].join(" ");
        let (lines, _) = emoji_body(&many, &none, true, true);
        assert_eq!(lines[0].matches("{shipit}").count(), 13, "{lines:?}");
    }

    #[test]
    fn a_workspaces_emoji_had_is_placed_as_a_picture_over_its_two_columns() {
        let had = HashMap::from([(
            "shipit".to_string(),
            Custom::Picture(std::sync::Arc::new(image::RgbaImage::new(4, 4))),
        )]);
        let (tz, pictures) = (TimeZone::UTC, HashMap::new());
        let mut look = Look {
            cols: 100,
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
            graphics: true,
            work: &[],
            tabs: &[],
            side_top: 0,
            thread_open: false,
            failed: None,
            files_failed: &Default::default(),
            files_kept: &Default::default(),
            hover: None,
            hover_section: None,
            sections_focused: false,
            emoji_on: true,
            emoji: &had,
        };
        for graphics in [true, false] {
            look.graphics = graphics;
            let runs = vec![
                Run::Text(Span::raw("ok ")),
                Run::Emoji {
                    name: "shipit".into(),
                },
                Run::Text(Span::raw(" go")),
            ];
            let (mut line, mut images) = (Vec::new(), Vec::new());
            put(&mut line, runs, (10, 40), 3, &look, &mut images, "t");
            let text: String = line.iter().map(|s| s.content.to_string()).collect();
            assert_eq!(text.trim(), "ok :… go", "the same width either way");
            let placed: Vec<_> = images
                .iter()
                .map(|i| (i.x, i.y, i.width, i.height, i.shape.clone()))
                .collect();
            let want = if graphics {
                vec![(
                    13,
                    3,
                    2,
                    1,
                    Shape::Emoji {
                        name: "shipit".into(),
                    },
                )]
            } else {
                Vec::new()
            };
            assert_eq!(placed, want);
        }
    }

    #[test]
    fn a_workspaces_emoji_is_its_dimmed_code_without_images_or_by_a_name_it_lacks() {
        let none = HashMap::new();
        let (lines, dimmed) = emoji_body("ship :shipit: now", &none, false, true);
        assert_eq!(lines, vec!["ship :shipit: now"]);
        assert_eq!(dimmed, vec![":shipit:"]);
        let unknown = HashMap::from([("shipit".to_string(), Custom::Unknown)]);
        let (lines, dimmed) = emoji_body("ship :shipit: now", &unknown, true, true);
        assert_eq!(lines, vec!["ship :shipit: now"]);
        assert_eq!(dimmed, vec![":shipit:"]);
        let alias = HashMap::from([("yes".to_string(), Custom::Standard("👍".into()))]);
        assert_eq!(emoji_body("ok :yes:", &alias, true, true).0, vec!["ok 👍"]);
        let (lines, dimmed) = emoji_body("ship :shipit: now", &none, true, false);
        assert_eq!(lines, vec!["ship :shipit: now"], "the setting off");
        assert!(dimmed.is_empty());
    }

    /// A reply in the thread `C1:<root>` of #dev.
    fn reply(from: &str, text: &str, ago_s: i64, root: &str, old: bool) -> Item {
        let mut item = message(
            "channel",
            from,
            text,
            ago_s,
            json!({ "thread": format!("C1:{root}"), "old": old }),
        );
        item.data["message"]["thread"] = json!(root);
        item
    }

    fn channel(items: Vec<Item>) -> View {
        channel_by(items, true)
    }

    fn channel_by(items: Vec<Item>, by_thread: bool) -> View {
        let mut items = items;
        in_order(&mut items, by_thread);
        View {
            id: ViewId::Slack,
            title: "Slack".into(),
            data: ViewData {
                items,
                at: Some(NOW - 60_000),
                head: json!({ "seen": NOW - 3_600_000 }),
                ..ViewData::default()
            },
            loaded: true,
        }
    }

    #[test]
    fn a_threads_new_replies_are_one_row_with_its_people_newest_first() {
        let root = ts(900);
        let view = channel(vec![
            message(
                "channel",
                "Ana",
                "standalone note",
                600,
                json!({ "replies": 0 }),
            ),
            reply("Ben", "first reply", 500, &root, false),
            reply("Cy", "second reply", 120, &root, false),
            reply("Ben", "third reply", 60, &root, false),
        ]);
        assert_eq!(view.data.items.len(), 2, "the thread is one row");
        let mut alone = vec![
            reply("Ben", "first reply", 500, &root, false),
            reply("Cy", "second reply", 120, &root, false),
        ];
        in_order(&mut alone, false);
        assert_eq!(alone.len(), 2, "with no threads read, a row a message");
        let (apart, _) = draw_of(&channel_by(alone, false), 200, None, false, &[]);
        assert!(
            apart
                .iter()
                .any(|l| l.contains("1 thread with new replies")),
            "each thread counted once: {}",
            apart.join("\n")
        );
        // A row is its newest message whole, new while any message is.
        let mut seen_newest = vec![
            reply("Ada", "older, unread", 300, &root, false),
            reply("Ben", "newer, seen", 100, &root, true),
        ];
        seen_newest[1].title = "Ben · #dev · seen".into();
        let (lines, _) = draw_of(&channel(seen_newest.clone()), 200, None, false, &[]);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("thread · 2 replies, 1 new")),
            "a reply seen on its own isn't new beside: {}",
            lines.join("\n")
        );
        let rows = channel(seen_newest).data.items;
        assert_eq!(rows[0].title, "Ben · #dev");
        assert_eq!(get(&rows[0], "from"), "Ben");
        assert!(!is_old(&rows[0]));
        // An unread first message with its replies seen is no thread with
        // new replies.
        let mut first = message("channel", "Ana", "the question", 900, json!({}));
        first.data["thread"] = json!(format!("C1:{root}"));
        let (lines, _) = draw_of(
            &channel(vec![first, reply("Ben", "seen reply", 100, &root, true)]),
            200,
            None,
            false,
            &[],
        );
        assert!(
            !lines.iter().any(|l| l.contains("thread with new")),
            "{}",
            lines.join("\n")
        );
        assert_eq!(
            get(&view.data.items[0], "from"),
            "Ana",
            "older than the thread's latest reply, above it"
        );
        let row = &view.data.items[1];
        assert_eq!(get(row, "ts"), ts(60), "by its latest reply, last");
        assert_eq!(
            row.id,
            format!("slack-thread:Channel:C1:C1:{root}"),
            "kept by its section and thread"
        );
        assert_eq!(members(row).len(), 3);
        assert_eq!(sections(&view.data.items)[0].messages, 4);
        let (lines, images) = draw_of(&view, 200, None, false, &[]);
        let all = lines.join("\n");
        assert!(
            all.contains("1 thread with new replies · 4 new · oldest first"),
            "{all}"
        );
        assert_eq!(
            lines
                .iter()
                .filter(|l| {
                    l.split('│')
                        .nth(1)
                        .is_some_and(|r| r.contains("second") || r.contains("third"))
                })
                .count(),
            0,
            "a row shows its thread's first message, not each reply: {all}"
        );
        let counts = lines.iter().find(|l| l.contains("3 replies")).unwrap();
        assert!(
            counts.contains("3 replies") && counts.contains("3 new"),
            "{counts}"
        );
        let small = |who: &str| {
            images.iter().any(|i| {
                matches!(&i.shape, Shape::Avatar { label, .. } if label == who) && i.height == 1
            })
        };
        assert!(small("Ben") && small("Cy"), "its people, small");
        // A thread read before the last reply came takes it in.
        let older = Thread {
            messages: vec![
                Said {
                    user: "UAna".into(),
                    from: "Ana".into(),
                    text: "the question".into(),
                    ts: root.clone(),
                    ..Said::default()
                },
                Said {
                    user: "UBen".into(),
                    from: "Ben".into(),
                    text: "first reply".into(),
                    ts: ts(500),
                    ..Said::default()
                },
            ],
            replies: 1,
            read: NOW,
        };
        let (beside, _) = draw_of(&view, 200, Some(&older), false, &[]);
        let pane = |l: &String| l.split('│').nth(2).unwrap_or("").to_string();
        assert!(
            beside.iter().any(|l| pane(l).contains("third reply")),
            "{}",
            beside.join("\n")
        );
        assert!(
            beside
                .iter()
                .any(|l| pane(l).contains("thread · 3 replies, 3 new")),
            "its header counts the replies saved since: {}",
            beside.join("\n")
        );
        for words in ["first reply", "second reply", "third reply"] {
            assert!(
                lines
                    .iter()
                    .any(|l| l.split('│').nth(2).is_some_and(|p| p.contains(words))),
                "until the thread is read, its messages beside: {all}"
            );
        }
        // The standalone message: who, when, its words, and nothing under.
        let at = lines
            .iter()
            .position(|l| l.contains("standalone note"))
            .unwrap();
        assert!(lines[at - 1].contains("Ana") && lines[at - 1].contains("10m"));
        assert!(
            lines[at + 1].split('│').nth(1).unwrap().trim().is_empty(),
            "two lines: {all}"
        );
    }

    #[test]
    fn a_rows_reply_count_takes_in_replies_saved_since_its_thread_was_read() {
        let root = ts(900);
        let read: f64 = ts(500).parse().unwrap();
        let mut all = vec![
            reply("Ben", "first reply", 500, &root, false),
            reply("Cy", "second reply", 120, &root, false),
            reply("Ben", "third reply", 60, &root, false),
        ];
        for m in &mut all {
            m.data["replies"] = json!(1);
            m.data["read_to"] = json!(read);
        }
        let all_again = all;
        let (lines, _) = draw_of(
            &channel(vec![reply("Ben", "a lone reply", 60, &root, false)]),
            200,
            None,
            false,
            &[],
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("1 thread with new replies")),
            "a lone new reply is a thread with new replies: {}",
            lines.join("\n")
        );
        let (lines, _) = draw_of(&channel(all_again), 200, None, false, &[]);
        assert!(
            lines.iter().any(|l| l.contains("3 replies")),
            "{}",
            lines.join("\n")
        );
    }

    #[test]
    fn the_photos_wanted_are_every_rows_starter_and_people() {
        let root = ts(900);
        let mut first = reply("Ben", "a reply", 60, &root, false);
        first.data["root"] = json!({ "user": "ULee", "from": "Lee", "text": "q", "ts": root });
        first.data["people"] =
            json!([{ "user": "ULee", "from": "Lee" }, { "user": "UMaya", "from": "Maya" }]);
        let wanted = people(&channel(vec![first]), None);
        for user in ["UBen", "ULee", "UMaya"] {
            assert!(wanted.contains(&Who::new(Service::Slack, user)), "{user}");
        }
    }

    #[test]
    fn a_reply_seen_on_its_own_in_another_section_isnt_new_beside() {
        let root = ts(900);
        let said = |from: &str, at: String| Said {
            user: format!("U{from}"),
            from: from.into(),
            text: format!("{from} says"),
            ts: at,
            ..Said::default()
        };
        let thread = Thread {
            messages: vec![
                said("Ana", root.clone()),
                said("Ada", ts(120)),
                said("Ben", ts(60)),
            ],
            replies: 2,
            read: NOW,
        };
        let mut view = channel(vec![reply("Ben", "Ben says", 60, &root, false)]);
        view.data.head["seen_alone"] = json!([format!("C1:{}", ts(120))]);
        let (lines, _) = draw_of(&view, 200, Some(&thread), false, &[]);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("thread · 2 replies, 1 new")),
            "{}",
            lines.join("\n")
        );
    }

    #[test]
    fn a_direct_message_you_answered_says_so() {
        let mut dm = message(
            "dm",
            "Bo",
            "lunch?",
            300,
            json!({ "answered": true, "replies": 0 }),
        );
        dm.data["where"] = json!("Bo");
        let (lines, _) = draw_of(&channel(vec![dm]), 200, None, false, &[]);
        let all = lines.join("\n");
        assert!(
            lines
                .iter()
                .any(|l| l.contains("Bo") && l.contains("↩ replied")),
            "{all}"
        );
    }

    #[test]
    fn a_thread_started_before_the_look_is_headed_by_its_first_message() {
        let root = ts(3 * 86_400);
        let mut first = reply("Ben", "a new reply", 60, &root, false);
        first.data["root"] = json!({
            "user": "ULee", "from": "Lee", "text": "Retry banner copy?", "ts": root,
        });
        first.data["replies"] = json!(7);
        let view = channel(vec![first, reply("Cy", "an old reply", 300, &root, true)]);
        let thread = Thread {
            messages: vec![
                Said {
                    user: "ULee".into(),
                    from: "Lee".into(),
                    text: "Retry banner copy?".into(),
                    ts: root.clone(),
                    ..Said::default()
                },
                Said {
                    user: "UCy".into(),
                    from: "Cy".into(),
                    text: "an old reply".into(),
                    ts: ts(7200),
                    ..Said::default()
                },
                Said {
                    user: "UBen".into(),
                    from: "Ben".into(),
                    text: "a new reply".into(),
                    ts: ts(60),
                    ..Said::default()
                },
            ],
            replies: 7,
            read: NOW,
        };
        let (lines, _) = draw_of(&view, 200, Some(&thread), false, &[]);
        let all = lines.join("\n");
        let head = lines
            .iter()
            .find(|l| l.split('│').nth(1).is_some_and(|r| r.contains("Lee")))
            .unwrap();
        let list = head.split('│').nth(1).unwrap();
        assert!(list.contains("started Tue"), "{head}");
        assert!(all.contains("Retry banner copy?"), "{all}");
        assert!(all.contains("7 replies") && all.contains("1 new"), "{all}");
        // Beside it, the thread whole, its new reply marked.
        assert!(all.contains("thread · 7 replies, 1 new"), "{all}");
        let ben = lines
            .iter()
            .find(|l| l.split('│').nth(2).is_some_and(|p| p.contains("Ben")))
            .unwrap();
        assert!(ben.contains("▎") && ben.contains("new"), "{ben}");
        let cy = lines
            .iter()
            .find(|l| l.split('│').nth(2).is_some_and(|p| p.contains("Cy")))
            .unwrap();
        assert!(!cy.split('│').nth(2).unwrap().contains("new"), "{cy}");
    }

    #[test]
    fn the_sections_column_lists_the_sections_and_the_chosen_ones_messages_beside() {
        let (lines, images) = draw(200, None, false, &[]);
        let all = lines.join("\n");
        let at = |s: &str| all.find(s).unwrap_or_else(|| panic!("{s} in\n{all}"));
        assert!(
            at("@ Mentions") < at("DIRECT MESSAGES") && at("DIRECT MESSAGES") < at("CHANNELS"),
            "{all}"
        );
        assert!(
            !all.contains("Threads"),
            "a section with nothing is left out"
        );
        let mentions = lines.iter().find(|l| l.contains("@ Mentions")).unwrap();
        let column = mentions.split('│').next().unwrap();
        assert_eq!(width(column), SECTIONS_W, "{mentions}");
        assert!(
            column.trim_end().ends_with('1'),
            "its count at the right: {mentions}"
        );
        assert!(
            all.contains(" 1 new · oldest first") && !all.contains("thread with new"),
            "{all}"
        );
        assert!(all.contains("Older messages stay in Slack."));
        let ada = lines
            .iter()
            .find(|l| l.contains("Ada") && l.contains("#dev"))
            .unwrap();
        let list = ada.split('│').nth(1).unwrap();
        assert!(
            list.trim_end().ends_with("2m") && list.ends_with(" "),
            "when it last moved at the list's right edge: {ada}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("3 replies") && l.contains("1 new")),
            "its replies and how many are new under its words: {all}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("lunch?")),
            "another section's messages aren't listed"
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
            hover_section: None,
            sections_focused: false,
            emoji_on: true,
            emoji: &Default::default(),
        };
        let said = texts(&board(&view(), &look).0).join("\n");
        assert!(
            said.contains("Couldn't read the thread: Slack isn't connected"),
            "{said}"
        );
    }

    #[test]
    fn a_thread_naming_a_github_issue_lists_it_with_the_tab_on_it() {
        let mut thread = thread();
        thread.messages[1].text =
            "see https://github.com/acme/widgets/issues/12 and acme/widgets#7".into();
        let mut tab = TabModel::new(crate::mux::TabId(3), 4, "#12: Make the widget blue");
        tab.ticket = Ticket::parse("acme/widgets#12");
        let (lines, _) = draw_with_tabs(&view(), 200, Some(&thread), false, &[], &[tab]);
        let on = lines.iter().find(|l| l.contains("● #12")).unwrap();
        assert!(
            on.contains("Make the widget blue") && on.contains("tab 4"),
            "{on}"
        );
        let other = lines.iter().find(|l| l.contains("● #7")).unwrap();
        assert!(other.contains("no tab yet"), "{other}");
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
        assert!(all.contains("thread · 2 replies, 1 new"), "{all}");
        let pane = |who: &str| {
            lines
                .iter()
                .filter_map(|l| l.split('│').nth(2))
                .find(|p| p.contains(who))
                .unwrap()
        };
        let dee = pane("Dee");
        assert!(!dee.contains("new"), "seen before: {dee}");
        let ed = pane("Ed ");
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
    fn a_message_has_a_large_avatar_its_name_and_words_five_columns_in() {
        let thread = thread();
        let (lines, images) = draw(200, Some(&thread), false, &[]);
        let row = |s: &str| lines.iter().position(|l| l.contains(s)).unwrap();
        let col = |l: &str, s: &str| width(&l[..l.find(s).unwrap()]);
        let upto = |l: &str, n: usize| l.chars().take(n).collect::<String>();
        // In the list: the avatar after the bar, the sender 5 past it and
        // the words under the sender, beside the avatar's second row.
        let ada = row("Ada  ");
        let face = images
            .iter()
            .find(|i| matches!(&i.shape, Shape::Avatar { label, .. } if label == "Ada"))
            .unwrap();
        assert_eq!((face.width, face.height), (4, 2));
        // The board starts on the screen's row 1, so line n is row n + 1.
        assert_eq!(face.y as usize, ada + 1, "on the sender's line");
        let x = face.x as usize;
        assert_eq!(col(&lines[ada], "Ada"), x + 5, "{}", lines[ada]);
        assert!(
            upto(&lines[ada], x + 2).ends_with("A "),
            "the initial without the photo"
        );
        let words = &lines[ada + 1];
        assert_eq!(col(words, "can"), x + 5, "{words}");
        assert!(
            upto(words, x + 5)
                .chars()
                .skip(x)
                .all(|c| c == ' ' || c == '▌'),
            "the avatar's second row blank: {words}"
        );
        // In the thread beside, each message the same way.
        let ed = images
            .iter()
            .find(|i| matches!(&i.shape, Shape::Avatar { label, .. } if label == "Ed"))
            .unwrap();
        assert_eq!((ed.width, ed.height), (4, 2));
        let said = &lines[ed.y as usize]; // the line under Ed's name
        assert_eq!(col(said, "a newer one"), ed.x as usize + 5, "{said}");
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

    /// A message in a conversation of its own, as a DM's or a channel's.
    fn in_place(kind: &str, from: &str, channel: &str, place: &str, ago_s: i64, old: bool) -> Item {
        message(
            kind,
            from,
            "x",
            ago_s,
            json!({ "where": place, "old": old, "message": { "channel": channel, "ts": ts(ago_s) } }),
        )
    }

    fn mixed() -> Vec<Item> {
        let mut items = vec![
            in_place("channel", "Cy", "C2", "#ops", 900, true),
            in_place("dm", "Bo", "D1", "Bo", 60, false),
            in_place("mention", "Ada", "C1", "#dev", 600, true),
            in_place("thread", "Lin", "C3", "#docs", 30, false),
            in_place("channel", "Dee", "C4", "#web", 20, false),
            in_place("mention", "Ed", "C1", "#dev", 120, false),
            in_place("dm", "Bo", "D1", "Bo", 700, true),
            in_place("channel", "Fay", "C2", "#ops", 10, true),
        ];
        in_order(&mut items, true);
        items
    }

    #[test]
    fn each_message_is_in_its_section_oldest_first_with_counts_and_the_new_flag() {
        let items = mixed();
        let from: Vec<&str> = items.iter().map(|i| get(i, "from")).collect();
        assert_eq!(
            from,
            ["Ada", "Ed", "Lin", "Bo", "Bo", "Cy", "Fay", "Dee"],
            "Mentions, Threads, the DMs, then the channels by their newest, \
             each oldest to newest"
        );
        assert_eq!(
            items[3].data["message"]["ts"],
            json!(ts(700)),
            "Bo's older message above"
        );
        let secs = sections(&items);
        let shape: Vec<(Kind, &str, usize, bool)> = secs
            .iter()
            .map(|s| (s.kind, s.title.as_str(), s.items.len(), s.new))
            .collect();
        assert_eq!(
            shape,
            [
                (Kind::Mentions, "Mentions", 2, true),
                (Kind::Threads, "Threads", 1, true),
                (Kind::Direct, "Bo", 2, true),
                (Kind::Channel, "ops", 2, false),
                (Kind::Channel, "web", 1, true),
            ]
        );
        assert_eq!(secs[2].user, "UBo", "a DM's person, for their avatar");
    }

    #[test]
    fn j_and_k_stay_in_the_section_and_the_sections_step_to_their_newest() {
        let items = mixed();
        // Ada, Ed | Lin | Bo, Bo | Cy, Fay | Dee, each oldest first
        assert_eq!(row_step(&items, 0, 1), 1, "j to the newer");
        assert_eq!(row_step(&items, 1, 1), 1, "the section's newest stays");
        assert_eq!(row_step(&items, 4, -1), 3, "k to the older");
        assert_eq!(section_step(&items, 1, 1), 2);
        assert_eq!(section_step(&items, 2, 1), 4, "at its newest, the bottom");
        assert_eq!(section_step(&items, 4, 1), 6);
        assert_eq!(section_step(&items, 7, 1), 7, "the last section stays");
        assert_eq!(section_step(&items, 3, -1), 2);
        assert_eq!(section_step(&items, 6, -1), 4);
        assert_eq!(
            section_step(&items, -1, 0),
            1,
            "none chosen: the first's newest"
        );
        assert!(on_newest(&items, 1) && on_newest(&items, 4));
        assert!(!on_newest(&items, 0) && !on_newest(&items, 3));
    }

    #[test]
    fn a_section_reads_oldest_to_newest_opening_at_its_newest() {
        let mut items = vec![
            in_place("channel", "Ann", "C1", "#dev", 300, false),
            in_place("channel", "Ben", "C1", "#dev", 200, false),
            in_place("channel", "Cy", "C1", "#dev", 100, false),
        ];
        in_order(&mut items, true);
        let view = channel(items.clone());
        let (lines, _) = draw_of(&view, 200, None, false, &[]);
        let list: Vec<&str> = lines.iter().filter_map(|l| l.split('│').nth(1)).collect();
        let at = |who: &str| list.iter().position(|l| l.contains(who)).unwrap();
        assert!(at("Ann") < at("Ben") && at("Ben") < at("Cy"), "{list:#?}");
        assert!(
            list[at("Cy")].contains('▌'),
            "opens on the newest: {list:#?}"
        );
        let older = list
            .iter()
            .position(|l| l.contains("Older messages stay in Slack."))
            .unwrap();
        assert!(older < at("Ann"), "what is older, above: {list:#?}");
        // A section longer than the pane shows its newest rows.
        let tz = TimeZone::UTC;
        let pictures = HashMap::new();
        let look = Look {
            cols: 200,
            rows: 8,
            top_row: 1,
            cursor: Some(2),
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
            failed: None,
            files_failed: &Default::default(),
            files_kept: &Default::default(),
            hover: None,
            hover_section: None,
            sections_focused: false,
        };
        let short = texts(&board(&view, &look).0).join("\n");
        assert!(short.contains("Cy") && !short.contains("Ann"), "{short}");

        // A new message lands at the bottom: the cursor on the newest row
        // follows it, one on an older row stays.
        let mut more = items.clone();
        more.push(in_place("channel", "Dee", "C1", "#dev", 10, false));
        in_order(&mut more, true);
        assert_eq!(get(&more[3], "from"), "Dee", "at the bottom");
        let follow = |cursor: usize| {
            let id = &items[cursor].id;
            let at = more.iter().position(|i| &i.id == id).unwrap() as isize;
            if on_newest(&items, cursor as isize) {
                section_step(&more, at, 0)
            } else {
                at
            }
        };
        assert_eq!(follow(2), 3, "from the last row, to the new one");
        assert_eq!(follow(1), 1, "from an older row, staying");

        // The newest row dismissed: its section's new newest, never the
        // next section's first.
        let mut both = items.clone();
        both.push(in_place("channel", "Eve", "C2", "#ops", 900, false));
        in_order(&mut both, true);
        let key = section_key_at(&both, 2).unwrap();
        assert_eq!(key, (Kind::Channel, "C1".to_string()));
        both.remove(2);
        assert_eq!(newest_in(&both, &key), Some(1));
        assert_eq!(
            get(&both[2], "from"),
            "Eve",
            "the old index, another section"
        );
    }

    #[test]
    fn the_sections_order_by_their_newest_message() {
        let mut items = vec![
            in_place("channel", "Ann", "C1", "#dev", 50, false),
            in_place("channel", "Ben", "C2", "#ops", 100, false),
            in_place("channel", "Cy", "C1", "#dev", 900, false),
            in_place("channel", "Dee", "C2", "#ops", 200, false),
        ];
        in_order(&mut items, true);
        let titles: Vec<String> = sections(&items).into_iter().map(|s| s.title).collect();
        assert_eq!(titles, ["dev", "ops"], "#dev's newest is newer");
        let from: Vec<&str> = items.iter().map(|i| get(i, "from")).collect();
        assert_eq!(from, ["Cy", "Ann", "Dee", "Ben"]);
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
            hover_section: None,
            sections_focused: false,
            emoji_on: true,
            emoji: &Default::default(),
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
        let rules = |l: &str| -> Vec<usize> {
            l.char_indices()
                .filter(|(_, c)| *c == '│')
                .map(|(b, _)| width(&l[..b]))
                .collect()
        };
        // From 120 the sections, the messages and the thread, the list and
        // the pane sharing what the sections leave equally.
        for cols in [240, 160, 120] {
            let (lines, _) = draw(cols, Some(&thread), false, &[]);
            let at = rules(&lines[2]);
            assert_eq!(at.len(), 2, "{cols}: {}", lines[2]);
            assert_eq!(at[0], SECTIONS_W, "{cols}");
            let half = SECTIONS_W + 1 + (cols - SECTIONS_W - 1) / 2;
            assert!(at[1].abs_diff(half) <= 1, "{cols}: the rule at {}", at[1]);
            assert!(lines[1].starts_with(" @ Mentions"), "{cols}: {}", lines[1]);
            assert!(
                lines.iter().any(|l| l.contains("Ada  #dev")),
                "where, at {cols}"
            );
        }
        // Narrower the thread is behind i and where goes.
        let (lines, _) = draw(110, Some(&thread), false, &[]);
        assert_eq!(rules(&lines[2]), [SECTIONS_W], "no pane: {}", lines[2]);
        assert!(!lines.iter().any(|l| l.contains("Ada  #dev")), "no where");
        assert!(
            lines.iter().any(|l| l.contains("Ada") && l.contains("2m")),
            "how long ago kept"
        );
        // Under 100 the sections fold into a line above the messages.
        let (lines, _) = draw(90, Some(&thread), false, &[]);
        assert!(lines.iter().all(|l| !l.contains('│')), "no column");
        assert!(
            lines[1].contains("@ Mentions 1") && lines[1].contains("# dev 1"),
            "{}",
            lines[1]
        );
        let (lines, _) = draw(90, Some(&thread), true, &[]);
        let all = lines.join("\n");
        assert!(
            all.contains("thread · 2 replies") && !all.contains("@ Mentions"),
            "i shows the thread in the list's place: {all}"
        );
    }

    #[test]
    fn a_thread_read_on_its_own_marks_no_reply_before_then_new() {
        let thread = thread();
        let mut view = view();
        let key = format!("C1:{}", ts(120));
        view.data.head = json!({ "seen": NOW - 200_000, "read_threads": { key: NOW } });
        let tz = TimeZone::UTC;
        let pictures = HashMap::new();
        let look = Look {
            cols: 200,
            rows: 40,
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
            thread_open: false,
            failed: None,
            files_failed: &Default::default(),
            files_kept: &Default::default(),
            hover: None,
            hover_section: None,
            sections_focused: false,
            emoji_on: true,
            emoji: &Default::default(),
        };
        let lines = texts(&board(&view, &look).0);
        let ed = lines.iter().find(|l| l.contains("Ed ")).unwrap();
        assert!(!ed.contains("▎") && !ed.contains("new"), "{ed}");
    }

    #[test]
    fn a_long_channel_name_is_cut_so_the_chosen_section_stays_on_the_folded_line() {
        let long = format!("#{}", "x".repeat(75));
        let mut items = vec![
            message("mention", "Ada", "hi", 60, json!({})),
            message("channel", "Cy", "yo", 30, json!({ "where": long })),
        ];
        in_order(&mut items, true);
        let secs = sections(&items);
        let tz = TimeZone::UTC;
        let pictures = HashMap::new();
        let look = Look {
            cols: 80,
            rows: 40,
            top_row: 1,
            cursor: Some(1),
            theme: &Theme::DARK,
            caps: false,
            now_ms: NOW,
            tz: &tz,
            thread: None,
            threads_on: false,
            pictures: &pictures,
            graphics: false,
            work: &[],
            tabs: &[],
            side_top: 0,
            thread_open: false,
            failed: None,
            files_failed: &Default::default(),
            files_kept: &Default::default(),
            hover: None,
            hover_section: None,
            sections_focused: false,
            emoji_on: true,
            emoji: &Default::default(),
        };
        let (spans, at) = section_line(&secs, Some(1), &look, 80);
        assert!(
            at.iter().any(|(_, _, first)| *first == 1),
            "the chosen one has its place"
        );
        let line: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(width(&line) <= 80 && line.contains("# xxx"), "{line}");
    }

    #[test]
    fn your_reminders_are_a_section_of_their_own_and_the_sign_in_line_is_under_the_bar() {
        let t = ts(600);
        let mut reminder = Item::new("reminder:Rm1", "◷ Reminder · due").data(json!({
            "kind": "reminder", "old": false, "reminder": "Rm1", "from": "Reminder",
            "where": "Reminders", "ts": t,
        }));
        reminder.lines = vec!["water the plants".into()];
        let mut failed = message("dm", "Bo", "lunch?", 300, json!({}));
        failed.data["mark_failed"] = json!("no network");
        let mut view = channel(vec![
            message("mention", "Ada", "a mention", 120, json!({})),
            reminder,
            failed,
        ]);
        let kinds: Vec<Kind> = sections(&view.data.items).iter().map(|s| s.kind).collect();
        assert_eq!(kinds, [Kind::Mentions, Kind::Reminders, Kind::Direct]);
        view.data.head["note"] = json!("Slack reminders need your Slack sign-in");
        let draw_at = |cursor: usize| {
            let tz = TimeZone::UTC;
            let pictures = HashMap::new();
            let look = Look {
                cols: 200,
                rows: 40,
                top_row: 1,
                cursor: Some(cursor),
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
                failed: None,
                files_failed: &Default::default(),
                files_kept: &Default::default(),
                hover: None,
                hover_section: None,
                sections_focused: false,
                emoji_on: true,
                emoji: &Default::default(),
            };
            texts(&board(&view, &look).0)
        };
        let lines = draw_at(1);
        let all = lines.join("\n");
        assert_eq!(
            lines[1].trim(),
            "Slack reminders need your Slack sign-in",
            "under the bar: {all}"
        );
        assert!(all.contains("◷ Reminders"), "{all}");
        let row = lines
            .iter()
            .position(|l| l.contains("Reminder") && l.contains("due") && l.contains("10m"))
            .unwrap_or_else(|| panic!("its row: {all}"));
        assert!(lines[row + 1].contains("water the plants"), "{all}");
        assert!(all.contains("a reminder of yours, due"), "{all}");
        assert!(
            !all.contains("Reading the thread"),
            "a reminder has no thread: {all}"
        );
        assert!(
            all.contains("A reminder stays until it is complete in Slack."),
            "{all}"
        );
        // A message that couldn't be marked read says so on its row, and
        // why beside it.
        let lines = draw_at(2);
        let all = lines.join("\n");
        assert!(all.contains("not marked read"), "{all}");
        assert!(
            all.contains("Couldn't mark it read in Slack: no network"),
            "{all}"
        );
    }
}
