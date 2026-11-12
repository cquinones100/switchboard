//! With UX 2.0, the Inbox view as a board (board 22 of the canvas, new with
//! the port; the tmux version, and this view with UX 2.0 off, list the
//! notifications as `render::list` does): the notifications grouped by day
//! with their counts, each two rows beside the Large avatar of who acted
//! (`kit::avatar`, their Linear photo drawn over their initial), its kind's
//! mark, the ticket, what happened and when, then the ticket's title under
//! the ticket; and beside the list the chosen one in its ticket: the ticket's state, a priority chip,
//! its project and the tab working on it, the comments up to the
//! notification's own, which is marked new (or, for an assignment or a
//! mention in the description, the description's start), then "From your
//! work": the tab working on it and the newest commit on a default branch
//! naming it. Under `PANE_AT` columns the ticket shows in the list's place
//! while `i` holds it there.
//!
//! Drawing reads only the snapshot: the chosen ticket as the server last
//! read it off its task (`Snapshot::inbox_ticket`), the commits the
//! sources found with local git (`Snapshot::inbox_links`), and the tabs.

use std::collections::HashMap;

use ratatui::style::Style;
use ratatui::text::Span;
use serde_json::Value;

use super::render::Image;
use super::views::{View, ViewId};
use super::{HitBox, Mission, Spot};
use crate::items::Item;
use crate::mux::input::{Key, KeyCode, Mods};
use crate::photos::{Service, Who};
use crate::sidebar::model;
use crate::sidebar::render::Line;
use crate::sidebar::text::{self, width};
use crate::sources::inbox::Commit;
use crate::ui::kit::{self, Col, Mark, Tone};

/// Under this many columns the ticket isn't beside the list: `i` shows it
/// in the list's place.
pub const PANE_AT: usize = 140;
/// Under this many columns the parts that matter least go, in this order:
/// the files a commit touches, the ticket's project, and what happened
/// past who did it.
const ROOMY_AT: usize = 160;
/// The list and the ticket share the screen by these weights (board 22's
/// 62 and 62), the ticket never under `PANE_LEAST` columns.
const LIST_WEIGHT: usize = 62;
const PANE_WEIGHT: usize = 62;
const LIST_LEAST: usize = 40;
const PANE_LEAST: usize = 34;
/// What happened is cut past this many columns.
const WHAT_CAP: usize = 32;
/// The comments the ticket shows, up to the notification's own.
const COMMENTS_SHOWN: usize = 3;
/// The lines of the description an assignment or a mention in it shows.
const DESCRIPTION_LINES: usize = 8;
const FOOT: &str = "Read, archived and snoozed ones stay in Linear.";

/// Whether the board draws the view: UX 2.0's, read, with notifications.
pub fn shown(m: &Mission, view: &View) -> bool {
    m.snap.rules.ux2
        && view.id == ViewId::Inbox
        && view.loaded
        && view.data.setup.is_none()
        && !view.data.items.is_empty()
}

/// Whether the ticket is beside the list at this width.
pub fn beside(cols: usize) -> bool {
    cols >= PANE_AT
}

fn data<'a>(item: &'a Item, key: &str) -> &'a str {
    item.data.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The ticket a notification is about.
pub fn ticket_of(item: &Item) -> &str {
    data(item, "ticket")
}

/// Who acted, for their photo: their Linear user.
pub fn actor_of(item: &Item) -> Option<Who> {
    Some(data(item, "actor_id"))
        .filter(|id| !id.is_empty())
        .map(|id| Who::new(Service::Linear, id))
}

/// The people whose photos the ticket beside shows: those who wrote the
/// comments it shows.
pub fn people(ticket: &Value, item: &Item) -> Vec<Who> {
    comments_shown(ticket, data(item, "comment"))
        .unwrap_or_default()
        .iter()
        .filter_map(|c| c.pointer("/user/id").and_then(Value::as_str))
        .filter(|id| !id.is_empty())
        .map(|id| Who::new(Service::Linear, id))
        .collect()
}

/// What happened, as the row says it: "assigned to you by Ana", "Ben
/// commented", "Priya mentioned you"; `short` keeps only who did it and
/// the verb.
pub fn what(item: &Item, short: bool) -> String {
    let actor = match data(item, "actor") {
        "" => "Someone",
        a => a,
    };
    match (data(item, "kind"), short) {
        ("needs", false) => format!("assigned to you by {actor}"),
        ("needs", true) => format!("{actor} assigned"),
        ("comment", _) => format!("{actor} commented"),
        (_, false) => format!("{actor} mentioned you"),
        (_, true) => format!("{actor} mentioned"),
    }
}

fn mark_of(item: &Item) -> Mark {
    match data(item, "kind") {
        "needs" => Mark::Needs,
        "comment" => Mark::Comment,
        _ => Mark::Mention,
    }
}

/// A day's label: Today, Yesterday, the weekday within the week, else the
/// date.
fn day_label(day: jiff::civil::Date, today: jiff::civil::Date) -> String {
    let back = today.since(day).map_or(0, |s| s.get_days());
    match back {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => day.strftime("%A").to_string(),
        _ => day.strftime("%b %-d").to_string(),
    }
}

/// When it happened: how long ago today, else its time on its day.
fn when(ms: i64, now_ms: i64, tz: &jiff::tz::TimeZone) -> String {
    let Ok(at) = jiff::Timestamp::from_millisecond(ms) else {
        return String::new();
    };
    let now = jiff::Timestamp::from_millisecond(now_ms).unwrap_or(at);
    let (at, now) = (at.to_zoned(tz.clone()), now.to_zoned(tz.clone()));
    if at.date() == now.date() {
        match (now_ms - ms).max(0) / 60_000 {
            0 => "now".into(),
            m @ 1..60 => format!("{m}m ago"),
            m => format!("{}h ago", m / 60),
        }
    } else {
        at.strftime("%-H:%M").to_string()
    }
}

/// A Large avatar placed in a line: its column and whose it is.
struct Face {
    x: usize,
    who: Option<Who>,
    name: String,
}

/// A line of the list or the ticket: its spans, the avatar in it, and what
/// a click on it does.
#[derive(Default)]
struct Drawn {
    spans: Vec<Span<'static>>,
    face: Option<Face>,
    spot: Option<Spot>,
}

impl Drawn {
    fn text(spans: Vec<Span<'static>>) -> Drawn {
        Drawn {
            spans,
            ..Drawn::default()
        }
    }
}

/// The first row of someone's Large avatar, under the photo the avatar
/// atom draws.
fn initial(name: &str) -> Vec<Span<'static>> {
    kit::avatar(name, kit::AvatarSize::Large).remove(0)
}

/// The list's columns in a notification's first row, as board 45 lays a
/// row out: the avatar, the mark, the ticket and what happened fixed, when
/// at the right edge; what happened goes first when even that won't fit.
fn row_columns(w: usize, ticket: usize, what: usize, when: usize) -> Vec<Option<(usize, usize)>> {
    kit::columns(
        &[
            Col::Fixed {
                min: kit::AvatarSize::Large.cols(),
                natural: kit::AvatarSize::Large.cols(),
                cap: kit::AvatarSize::Large.cols(),
            },
            Col::Fixed {
                min: 1,
                natural: 1,
                cap: 1,
            },
            Col::Fixed {
                min: 0,
                natural: ticket,
                cap: ticket,
            },
            Col::Fixed {
                min: 0,
                natural: what,
                cap: WHAT_CAP,
            },
            Col::Right {
                natural: when,
                cap: when,
            },
        ],
        w,
        1,
        &[3],
    )
}

/// The list's lines: each day's label and count, each notification's two
/// rows, the foot; and the line the chosen one starts on.
fn list_lines(view: &View, w: usize, m: &Mission, roomy: bool) -> (Vec<Drawn>, usize) {
    let snap = &m.snap;
    let theme = &snap.theme;
    let now_ms = snap.now_secs * 1000;
    let tz = &snap.tz;
    let today = jiff::Timestamp::from_millisecond(now_ms)
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
        .to_zoned(tz.clone())
        .date();
    let day_of = |item: &Item| {
        item.time
            .and_then(|t| jiff::Timestamp::from_millisecond(t).ok())
            .map(|t| t.to_zoned(tz.clone()).date())
    };
    let mut counts: HashMap<Option<jiff::civil::Date>, usize> = HashMap::new();
    for item in &view.data.items {
        *counts.entry(day_of(item)).or_default() += 1;
    }
    let short = !roomy;
    let cursor = usize::try_from(m.cursor).ok();
    let mut out: Vec<Drawn> = Vec::new();
    let mut chosen_line = 0;
    let mut last_day = None;
    for (at, item) in view.data.items.iter().enumerate() {
        let day = day_of(item);
        if out.is_empty() || last_day != Some(day) {
            if !out.is_empty() {
                out.push(Drawn::default());
            }
            let words = day.map_or("Earlier".to_string(), |d| day_label(d, today));
            out.push(Drawn::text(kit::label(
                &words,
                Tone::Dim,
                counts.get(&day).copied(),
                w,
                theme,
            )));
            last_day = Some(day);
        }
        let chosen = cursor == Some(at);
        if chosen {
            chosen_line = out.len();
        }
        let spot = Spot::Index(at as isize);
        let hover = m.hover == Some(spot);
        let ticket = ticket_of(item).to_string();
        let what = what(item, short);
        let when = item.time.map(|t| when(t, now_ms, tz)).unwrap_or_default();
        // After kit::row's bar.
        let inner = w.saturating_sub(1);
        let at_cols = row_columns(inner, width(&ticket), width(&what), width(&when));
        let actor = match data(item, "actor") {
            "" => "Someone".to_string(),
            a => a.to_string(),
        };
        let mut first = Vec::new();
        let mut face = None;
        if let Some(col) = at_cols[0] {
            face = Some(Face {
                x: 1 + col.0,
                who: actor_of(item),
                name: actor.clone(),
            });
            kit::place(&mut first, initial(&actor), col);
        }
        if let Some(col) = at_cols[1] {
            kit::place(&mut first, vec![kit::mark(mark_of(item), 0, theme)], col);
        }
        if let Some(col) = at_cols[2] {
            kit::place(
                &mut first,
                vec![Span::styled(ticket.clone(), Tone::Accent.bold(theme))],
                col,
            );
        }
        if let Some(col) = at_cols[3] {
            kit::place(
                &mut first,
                vec![Span::styled(what, Tone::Text.style(theme))],
                col,
            );
        }
        if let Some(col) = at_cols[4] {
            kit::place(
                &mut first,
                vec![Span::styled(when, Tone::Dim.style(theme))],
                col,
            );
        }
        let title = item
            .title
            .strip_prefix(&format!("{ticket} "))
            .unwrap_or(&item.title)
            .to_string();
        let title_x = at_cols[2].map_or(4, |c| c.0);
        let mut second = Vec::new();
        kit::place(
            &mut second,
            vec![Span::styled(title, Tone::Dim.style(theme))],
            (title_x, inner.saturating_sub(title_x)),
        );
        for (spans, face) in [(first, face), (second, None)] {
            let line = kit::row(spans, w, chosen, theme);
            out.push(Drawn {
                spans: if hover {
                    super::render::lit(line, theme)
                } else {
                    line
                },
                face,
                spot: Some(spot),
            });
        }
    }
    out.push(Drawn::default());
    out.push(Drawn::text(vec![
        Span::raw(" "),
        Span::styled(FOOT, Tone::Faint.style(theme)),
    ]));
    (out, chosen_line)
}

/// The comments the ticket shows for a notification: the last
/// `COMMENTS_SHOWN` up to and including its own (`comment`, its ID), oldest
/// first; none for a notification with no comment, and `None` when its
/// comment isn't among those read (a copy older than it, or past the
/// first 100), rather than comments it doesn't follow.
fn comments_shown<'a>(ticket: &'a Value, comment: &str) -> Option<Vec<&'a Value>> {
    if comment.is_empty() {
        return Some(Vec::new());
    }
    let mut all: Vec<&Value> = ticket
        .pointer("/comments/nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    all.sort_by_key(|c| c.get("createdAt").and_then(Value::as_str).unwrap_or(""));
    let end = all
        .iter()
        .position(|c| c.get("id").and_then(Value::as_str) == Some(comment))
        .map(|i| i + 1)?;
    Some(all[end.saturating_sub(COMMENTS_SHOWN)..end].to_vec())
}

fn ms_of(at: &str) -> Option<i64> {
    at.parse::<jiff::Timestamp>()
        .ok()
        .map(|t| t.as_millisecond())
}

/// The chosen notification in its ticket, `w` wide: the ticket's head
/// (`kit::pane`: its title, then its state, a priority chip, its project
/// and the tab working on it), the comments before the notification's with
/// its own marked new, or the description's start, then From your work.
fn ticket_lines(item: &Item, w: usize, m: &Mission, roomy: bool) -> Vec<Drawn> {
    let snap = &m.snap;
    let theme = &snap.theme;
    let ticket_id = ticket_of(item);
    let ticket = snap.inbox_ticket.as_ref().filter(|t| {
        t.get("identifier").and_then(Value::as_str) == Some(ticket_id) || t.get("error").is_some()
    });
    let title = ticket
        .and_then(|t| t.get("title"))
        .and_then(Value::as_str)
        .map(|t| format!("{ticket_id} {t}"))
        .unwrap_or_else(|| item.title.clone());
    let tabs = if snap.all_tabs.is_empty() {
        &snap.tabs
    } else {
        &snap.all_tabs
    };
    let tab = super::choices::tab_naming(tabs, ticket_id);
    let mut facts: Vec<Span<'static>> = Vec::new();
    if let Some(t) = ticket.filter(|t| t.get("error").is_none()) {
        if let Some(state) = t.pointer("/state/name").and_then(Value::as_str) {
            facts.push(Span::styled(state.to_string(), Tone::Text.style(theme)));
            facts.push(Span::raw("  "));
        }
        if let Some(p) = t
            .get("priorityLabel")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty() && *p != "No priority")
        {
            let tone = if matches!(p, "Urgent" | "High") {
                Tone::Needs
            } else {
                Tone::Dim
            };
            facts.extend(kit::chip(p, tone, snap.caps, theme));
            facts.push(Span::raw(" "));
        }
        if roomy && let Some(project) = t.pointer("/project/name").and_then(Value::as_str) {
            facts.push(Span::styled(project.to_string(), Tone::Dim.style(theme)));
            facts.push(Span::raw("  "));
        }
    }
    if let Some(t) = tab {
        facts.push(Span::styled(
            format!("▲ tab {} works on it", t.index),
            Tone::Landing.style(theme),
        ));
    }
    let mut out: Vec<Drawn> = kit::pane(&title, facts, w, theme)
        .into_iter()
        .map(Drawn::text)
        .collect();
    let dim = Tone::Dim.style(theme);
    match ticket {
        None => out.push(Drawn::text(vec![Span::styled(
            format!("Reading {ticket_id}…"),
            dim,
        )])),
        Some(t) if t.get("error").is_some() => {
            for l in text::wrap(t["error"].as_str().unwrap_or(""), w) {
                out.push(Drawn::text(vec![Span::styled(
                    l,
                    Tone::Failed.style(theme),
                )]));
            }
        }
        Some(t) => {
            let own = data(item, "comment");
            let Some(comments) = comments_shown(t, own) else {
                out.push(Drawn::text(vec![Span::styled(
                    "Its comment isn't in the copy read yet: M-l shows it in Linear.",
                    dim,
                )]));
                return from_your_work(out, item, w, m, roomy, tab);
            };
            let body_w = w.saturating_sub(kit::SAID_INDENT).min(kit::WRAP_MAX);
            let place = crate::views::ticket::PLACE;
            if comments.is_empty() {
                let about = super::render::markdown_lines(
                    t.get("description").and_then(Value::as_str).unwrap_or(""),
                    place,
                    w.saturating_sub(2).min(kit::WRAP_MAX),
                    theme,
                    None,
                );
                if about.lines.is_empty() {
                    out.push(Drawn::text(vec![Span::styled(
                        "No description.",
                        Tone::Faint.style(theme),
                    )]));
                }
                for line in about.lines.into_iter().take(DESCRIPTION_LINES) {
                    let mut spans = vec![Span::raw("  ")];
                    spans.extend(line);
                    out.push(Drawn::text(spans));
                }
            }
            let now_ms = snap.now_secs * 1000;
            for (i, c) in comments.iter().enumerate() {
                if i > 0 {
                    out.push(Drawn::default());
                }
                let name = c
                    .pointer("/user/name")
                    .or_else(|| c.pointer("/botActor/name"))
                    .and_then(Value::as_str)
                    .unwrap_or("Someone")
                    .to_string();
                let who = c
                    .pointer("/user/id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .map(|id| Who::new(Service::Linear, id));
                let is_new = c.get("id").and_then(Value::as_str) == Some(own);
                let ago = text::ago(
                    c.get("createdAt")
                        .and_then(Value::as_str)
                        .and_then(ms_of)
                        .map(|ms| ms / 1000),
                    now_ms / 1000,
                );
                let mut head = initial(&name);
                head.extend([
                    Span::raw(" "),
                    Span::styled(name.clone(), Tone::Text.bold(theme)),
                ]);
                if !ago.is_empty() {
                    head.push(Span::styled(format!("  {ago} ago"), dim));
                }
                if is_new {
                    head.push(Span::raw("  "));
                    head.extend(kit::chip("new", Tone::Accent, snap.caps, theme));
                }
                out.push(Drawn {
                    spans: head,
                    face: Some(Face { x: 0, who, name }),
                    spot: None,
                });
                let about = super::render::markdown_lines(
                    c.get("body").and_then(Value::as_str).unwrap_or(""),
                    place,
                    body_w,
                    theme,
                    None,
                );
                // Its words beside the avatar's second row, which stays
                // blank when there are none; a new one's bar just before them.
                let mut lines = about.lines;
                if lines.is_empty() {
                    lines.push(Vec::new());
                }
                for line in lines {
                    let gap = " ".repeat(kit::SAID_INDENT - 1);
                    let bar = if is_new {
                        Span::styled(format!("{gap}▌"), Tone::Accent.style(theme))
                    } else {
                        Span::raw(format!("{gap} "))
                    };
                    let mut spans = vec![bar];
                    spans.extend(line);
                    out.push(Drawn::text(spans));
                }
            }
        }
    }
    from_your_work(out, item, w, m, roomy, tab)
}

/// The ticket's lines ended with From your work: the tab working on it,
/// the commit naming it, and what Enter does.
fn from_your_work(
    mut out: Vec<Drawn>,
    item: &Item,
    w: usize,
    m: &Mission,
    roomy: bool,
    tab: Option<&model::TabModel>,
) -> Vec<Drawn> {
    let snap = &m.snap;
    let theme = &snap.theme;
    let dim = Tone::Dim.style(theme);
    let ticket_id = ticket_of(item);
    out.push(Drawn::default());
    out.push(Drawn::text(vec![kit::head_rule(w, theme)]));
    out.push(Drawn::text(kit::label(
        "From your work",
        Tone::Dim,
        None,
        w,
        theme,
    )));
    match tab {
        Some(t) => {
            let badge = model::badge_state(t, &snap.rules);
            let spans = vec![
                Span::raw(" "),
                kit::mark(Mark::of_badge(badge), snap.frame, theme),
                Span::styled(format!(" tab {} ", t.index), Tone::Text.bold(theme)),
                Span::styled(t.name.clone(), Tone::Text.style(theme)),
                Span::styled(format!("  {}", model::state_words(t, &snap.rules)), dim),
            ];
            let spot = Spot::Key(Key::new(KeyCode::Enter, Mods::NONE));
            out.push(Drawn {
                spans: if m.hover == Some(spot) {
                    super::render::lit(spans, theme)
                } else {
                    spans
                },
                face: None,
                spot: Some(spot),
            });
        }
        None => out.push(Drawn::text(vec![Span::styled(
            " No tab works on it.",
            Tone::Faint.style(theme),
        )])),
    }
    match snap.inbox_links.get(ticket_id) {
        Some(c) => out.push(Drawn::text(commit_line(c, w, m, roomy))),
        None => out.push(Drawn::text(vec![Span::styled(
            " No commit on a default branch names it.",
            Tone::Faint.style(theme),
        )])),
    }
    out.push(Drawn::default());
    for l in text::wrap(
        "Enter marks it read and offers what the Tickets board does: go to its tab, start work, open in Linear, close the ticket.",
        w.saturating_sub(1),
    ) {
        out.push(Drawn::text(vec![Span::raw(" "), Span::styled(l, dim)]));
    }
    out
}

/// The commit naming the ticket: `✓ 3f2a91c landed 2h ago  Its subject
/// touches land_lock.rs`, its hash fixed, its subject stretching, and the
/// file it touches only on a roomy screen.
fn commit_line(c: &Commit, w: usize, m: &Mission, roomy: bool) -> Vec<Span<'static>> {
    let theme = &m.snap.theme;
    let ago = text::ago(Some(c.at), m.snap.now_secs);
    let landed = if ago.is_empty() {
        " landed".to_string()
    } else {
        format!(" landed {ago} ago")
    };
    let touches = match (roomy, c.files.as_slice()) {
        (true, [one]) => format!("touches {}", file_name(one)),
        (true, [first, rest @ ..]) => {
            format!("touches {} +{}", file_name(first), rest.len())
        }
        _ => String::new(),
    };
    let head = 3 + width(&c.hash) + width(&landed);
    let at = kit::columns(
        &[
            Col::Fixed {
                min: head,
                natural: head,
                cap: head,
            },
            Col::Stretch {
                weight: 1,
                min: 8,
                max: None,
            },
            Col::Fixed {
                min: 0,
                natural: width(&touches),
                cap: 40,
            },
        ],
        w,
        2,
        &[2],
    );
    let mut line = Vec::new();
    if let Some(col) = at[0] {
        kit::place(
            &mut line,
            vec![
                Span::raw(" "),
                kit::mark(Mark::Passed, 0, theme),
                Span::raw(" "),
                Span::styled(c.hash.clone(), Tone::Accent.style(theme)),
                Span::styled(landed, Tone::Dim.style(theme)),
            ],
            col,
        );
    }
    if let Some(col) = at[1] {
        kit::place(
            &mut line,
            vec![Span::styled(c.subject.clone(), Tone::Text.style(theme))],
            col,
        );
    }
    if let (Some(col), false) = (at[2], touches.is_empty()) {
        kit::place(
            &mut line,
            vec![Span::styled(touches, Tone::Dim.style(theme))],
            col,
        );
    }
    line
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Draws the board from the line after the bar: the list and, on a wide
/// screen, the chosen notification's ticket beside it; on a narrow one,
/// the ticket in the list's place while `i` holds it.
pub fn board(
    view: &View,
    cols: usize,
    rows: usize,
    m: &Mission,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
    images: &mut Vec<Image>,
) {
    let theme = &m.snap.theme;
    // A look that failed keeps the notifications it had, saying why above.
    if let Some(e) = &view.data.error {
        for w in text::wrap(e, cols.saturating_sub(4)) {
            lines.push(Line {
                spans: vec![Span::styled(format!("  {w}"), Tone::Failed.style(theme))],
                fill: None,
            });
        }
    }
    let top = lines.len();
    let body = rows.saturating_sub(1 + top);
    let wide = beside(cols);
    let roomy = cols >= ROOMY_AT;
    let stretch = |weight, min| Col::Stretch {
        weight,
        min,
        max: None,
    };
    let (list_w, pane) = if wide {
        let at = kit::columns(
            &[
                stretch(LIST_WEIGHT, LIST_LEAST),
                stretch(PANE_WEIGHT, PANE_LEAST),
            ],
            cols,
            2,
            &[],
        );
        let list_w = at[0].map_or(cols, |c| c.1);
        (list_w, at[1])
    } else {
        (cols, None)
    };
    let chosen = m.chosen_item();
    // Under PANE_AT the ticket shows in the list's place while `i` holds it.
    let ticket_alone = !wide && m.inbox_pane && chosen.is_some();
    let (pane_x, pane_w) = match (pane, ticket_alone) {
        (Some((x, w)), _) => (x, w.saturating_sub(1)),
        (None, true) => (1, cols.saturating_sub(2)),
        (None, false) => (0, 0),
    };
    let ticket = chosen
        .filter(|_| pane_w > 0)
        .map(|item| ticket_lines(item, pane_w, m, roomy))
        .unwrap_or_default();
    let last = ticket.len().saturating_sub(body);
    let side_top = m.side_top().min(last);
    let ticket: Vec<Drawn> = ticket.into_iter().skip(side_top).take(body).collect();
    let (list, chosen_line) = if ticket_alone {
        (Vec::new(), 0)
    } else {
        list_lines(view, list_w, m, roomy)
    };
    // The chosen notification's two rows show, its day's label too when
    // it is the first.
    let first = (chosen_line + 2).saturating_sub(body);
    let list: Vec<Drawn> = list.into_iter().skip(first).take(body).collect();
    for i in 0..body {
        let y = top + i;
        let mut spans: Vec<Span<'static>> = Vec::new();
        if let Some(d) = list.get(i) {
            spans = kit::fit(d.spans.clone(), list_w, Style::new());
            if let Some(f) = &d.face {
                images.push(avatar_image(f, y, body - i, "inbox-list"));
            }
            if let Some(spot) = d.spot {
                hits.push(HitBox {
                    y: y as u16,
                    x0: 0,
                    x1: list_w.saturating_sub(1) as u16,
                    spot,
                });
            }
        } else if !ticket_alone {
            spans.push(Span::raw(" ".repeat(list_w)));
        }
        if wide {
            spans.push(kit::rule_v(theme));
            spans.push(Span::raw(" "));
        } else if ticket_alone {
            spans.push(Span::raw(" "));
        }
        if let Some(d) = ticket.get(i) {
            spans.extend(kit::fit(d.spans.clone(), pane_w, Style::new()));
            if let Some(f) = &d.face {
                let placed = Face {
                    x: pane_x + f.x,
                    who: f.who.clone(),
                    name: f.name.clone(),
                };
                images.push(avatar_image(&placed, y, body - i, "inbox-ticket"));
            }
            if let Some(spot) = d.spot {
                hits.push(HitBox {
                    y: y as u16,
                    x0: pane_x as u16,
                    x1: (pane_x + pane_w).saturating_sub(1) as u16,
                    spot,
                });
            }
        }
        lines.push(Line { spans, fill: None });
    }
    // After the pane's own clicks, which a click on their row finds first.
    if pane_w > 0 && !ticket.is_empty() {
        for i in 0..body {
            hits.push(HitBox {
                y: (top + i) as u16,
                x0: pane_x as u16,
                x1: cols.saturating_sub(1) as u16,
                spot: Spot::Side(last),
            });
        }
    }
}

/// A Large avatar at row `y`, cut to the `left` rows that show.
fn avatar_image(f: &Face, y: usize, left: usize, kind: &str) -> Image {
    let mut image = Image::avatar(
        format!("{kind}-avatar-{y}"),
        (f.x, y),
        f.who.clone(),
        &f.name,
        kit::AvatarSize::Large,
    );
    image.height = image.height.min(left as u16);
    image
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::ViewData;
    use crate::mission::render::Shape;
    use crate::mux::TabId;
    use crate::sidebar::model::TabModel;
    use crate::sidebar::model::tests::{NOW_MS, rules};
    use serde_json::json;

    const HOUR: i64 = 3_600_000;

    fn note(id: &str, ticket: &str, kind: &str, actor: &str, at: i64, comment: &str) -> Item {
        let mut item = Item::new(id, format!("{ticket} Hold the landing lock")).data(json!({
            "ticket": ticket,
            "notification": id,
            "url": format!("https://linear.app/x/issue/{ticket}"),
            "actor": actor,
            "actor_id": format!("u-{actor}"),
            "kind": kind,
            "comment": comment,
        }));
        item.time = Some(at);
        item
    }

    fn mission(cols: usize) -> Mission {
        let items = vec![
            note("n1", "ABC-138", "comment", "Ben", NOW_MS - HOUR, "c-3"),
            note("n2", "ABC-140", "needs", "Ana", NOW_MS - 2 * HOUR, ""),
            note("n3", "ABC-141", "mention", "Priya", NOW_MS - 30 * HOUR, ""),
        ];
        let mut m = Mission::new();
        m.view = 1;
        m.size = (cols as u16, 40);
        m.snap.rules = rules();
        m.snap.rules.ux2 = true;
        m.snap.now_secs = NOW_MS / 1000;
        m.snap.views = vec![View {
            id: ViewId::Inbox,
            title: "Inbox".into(),
            data: ViewData {
                items,
                at: Some(1),
                ..ViewData::default()
            },
            loaded: true,
        }];
        let mut tab = TabModel::new(TabId(4), 4, "ABC-138 landing lock");
        tab.path = Some("/work".into());
        m.snap.tabs = vec![tab];
        let comment = |id: &str, who: &str, body: &str, at: i64| {
            json!({ "id": id, "body": body, "createdAt": jiff::Timestamp::from_millisecond(at).unwrap().to_string(),
                    "user": { "id": format!("u-{who}"), "name": who } })
        };
        m.snap.inbox_ticket = Some(json!({
            "identifier": "ABC-138", "title": "Hold the landing lock",
            "state": { "name": "In Progress" }, "priorityLabel": "High",
            "project": { "name": "Landing" },
            "comments": { "nodes": [
                comment("c-0", "Ana", "zeroth thought", NOW_MS - 6 * HOUR),
                comment("c-1", "Ana", "first thought", NOW_MS - 5 * HOUR),
                comment("c-2", "Kai", "second thought", NOW_MS - 4 * HOUR),
                comment("c-3", "Ben", "the new one", NOW_MS - HOUR),
                comment("c-4", "Ana", "after it", NOW_MS - HOUR / 2),
            ] }
        }));
        m.snap.inbox_links.insert(
            "ABC-138".into(),
            Commit {
                hash: "3f2a91c".into(),
                subject: "Hold the lock through the rebase".into(),
                at: NOW_MS / 1000 - 2 * 3600,
                files: vec!["src/cmd/land_lock.rs".into()],
            },
        );
        m
    }

    fn draw(m: &Mission, cols: usize) -> (Vec<String>, Vec<HitBox>, Vec<Image>) {
        let view = m.snap.views[0].clone();
        let (mut lines, mut hits, mut images) = (Vec::new(), Vec::new(), Vec::new());
        board(&view, cols, 40, m, &mut lines, &mut hits, &mut images);
        let texts = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        (texts, hits, images)
    }

    fn line_with<'a>(texts: &'a [String], words: &str) -> &'a str {
        texts
            .iter()
            .find(|l| l.contains(words))
            .unwrap_or_else(|| panic!("no line says {words:?} in {texts:#?}"))
    }

    #[test]
    fn notifications_by_day_with_counts_and_the_actors_avatars() {
        let m = mission(160);
        let (texts, _, images) = draw(&m, 160);
        assert!(line_with(&texts, "TODAY").contains('2'), "two today");
        assert!(line_with(&texts, "YESTERDAY").contains('1'));
        let row = line_with(&texts, "ABC-140");
        assert!(row.contains("assigned to you by Ana"));
        assert!(row.contains("2h ago"));
        assert!(line_with(&texts, "◇ ABC-138").contains("Ben commented"));
        assert!(line_with(&texts, "ABC-141").contains("Priya mentioned you"));
        let faces: Vec<String> = images
            .iter()
            .filter_map(|i| match &i.shape {
                Shape::Avatar { who: Some(w), .. } if i.x < 80 => Some(w.id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(faces, vec!["u-Ben", "u-Ana", "u-Priya"]);
        // Each a Large avatar on its notification's two rows, the mark 5 in
        // and the title under the ticket.
        let ben = images
            .iter()
            .find(|i| matches!(&i.shape, Shape::Avatar { label, .. } if label == "Ben"))
            .unwrap();
        assert_eq!((ben.width, ben.height), (4, 2));
        let first = &texts[ben.y as usize];
        let col = |l: &str, s: &str| width(&l[..l.find(s).unwrap()]);
        assert_eq!(col(first, "◇"), ben.x as usize + 5, "{first}");
        let title = &texts[ben.y as usize + 1];
        let starts = width(title) - width(title.trim_start_matches([' ', '▌']));
        assert_eq!(starts, col(first, "ABC-138"), "{title}");
        line_with(&texts, FOOT);
    }

    #[test]
    fn the_ticket_beside_with_its_comments_the_new_one_marked_and_your_work() {
        let m = mission(160);
        let (texts, hits, images) = draw(&m, 160);
        let facts = line_with(&texts, "In Progress");
        assert!(facts.contains("High"));
        assert!(facts.contains("Landing"));
        assert!(facts.contains("▲ tab 4 works on it"));
        line_with(&texts, "second thought");
        assert!(line_with(&texts, "the new one").contains('▌'));
        assert!(line_with(&texts, "Ben  1h ago").contains("new"));
        assert!(
            !texts
                .iter()
                .any(|l| l.contains("zeroth thought") || l.contains("after it")),
            "the last three up to its own"
        );
        line_with(&texts, "FROM YOUR WORK");
        let commit = line_with(&texts, "3f2a91c");
        assert!(commit.contains("landed 2h ago"));
        assert!(commit.contains("touches land_lock.rs"));
        let tab_row = texts
            .iter()
            .position(|l| l.contains(" tab 4 ABC-138"))
            .unwrap();
        assert!(hits.iter().any(|h| h.y as usize == tab_row
            && h.spot == Spot::Key(Key::new(KeyCode::Enter, Mods::NONE))));
        let writers: Vec<String> = images
            .iter()
            .filter_map(|i| match &i.shape {
                Shape::Avatar { who: Some(w), .. } if i.x >= 80 => Some(w.id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(writers, vec!["u-Ana", "u-Kai", "u-Ben"]);
        // A comment's Large avatar at its writer's name, the words 5 past it.
        let kai = images
            .iter()
            .find(|i| {
                i.x >= 80 && matches!(&i.shape, Shape::Avatar { label, .. } if label == "Kai")
            })
            .unwrap();
        assert_eq!((kai.width, kai.height), (4, 2));
        let col = |l: &str, s: &str| width(&l[..l.find(s).unwrap()]);
        let head = &texts[kai.y as usize];
        assert_eq!(col(head, "Kai"), kai.x as usize + 5, "{head}");
        let words = &texts[kai.y as usize + 1];
        let at = col(words, "second thought");
        assert_eq!(at, kai.x as usize + 5, "{words}");
    }

    #[test]
    fn a_look_that_failed_says_so_above_the_notifications_it_kept() {
        let mut m = mission(160);
        m.snap.views[0].data.error = Some("Couldn't read your Linear inbox: timed out".into());
        let (texts, _, _) = draw(&m, 160);
        assert!(texts[0].contains("Couldn't read your Linear inbox"));
        line_with(&texts, "Ben commented");
    }

    #[test]
    fn a_comment_not_in_the_copy_shows_no_other_comments_in_its_place() {
        let mut m = mission(160);
        m.snap.views[0].data.items[0].data["comment"] = json!("c-9");
        let (texts, _, _) = draw(&m, 160);
        line_with(&texts, "Its comment isn't in the copy read yet");
        assert!(
            !texts
                .iter()
                .any(|l| l.contains("after it") || l.contains("second thought"))
        );
        line_with(&texts, "FROM YOUR WORK");
    }

    #[test]
    fn the_list_and_the_ticket_share_any_width_and_give_way_in_order() {
        for cols in [160, 240] {
            let m = mission(cols);
            let (texts, _, _) = draw(&m, cols);
            let row: Vec<char> = line_with(&texts, "ABC-140").chars().collect();
            let rule = row.iter().position(|c| *c == '│').unwrap();
            assert!(
                rule.abs_diff(cols / 2) <= 2,
                "{cols}: the list and the ticket share it, the rule at {rule}"
            );
            let list: String = row[..rule].iter().collect();
            let when_end = list.trim_end().chars().count();
            assert!(list.trim_end().ends_with("2h ago"));
            assert!(
                rule - when_end <= 2,
                "{cols}: when at the list's right edge"
            );
            assert!(
                line_with(&texts, "TODAY")
                    .chars()
                    .take(rule)
                    .collect::<String>()
                    .trim_end()
                    .ends_with('2')
            );
        }
        let mut m = mission(120);
        let (texts, _, _) = draw(&m, 120);
        assert!(
            !texts
                .iter()
                .any(|l| l.contains('│') || l.contains("FROM YOUR WORK"))
        );
        assert!(line_with(&texts, "ABC-140").contains("Ana assigned"));
        assert!(!line_with(&texts, "ABC-140").contains("to you by"));
        m.inbox_pane = true;
        let (texts, _, _) = draw(&m, 120);
        assert!(
            !texts.iter().any(|l| l.contains("TODAY")),
            "the ticket in the list's place"
        );
        assert!(!line_with(&texts, "3f2a91c").contains("touches"));
        assert!(!line_with(&texts, "In Progress").contains("Landing"));
    }
}
