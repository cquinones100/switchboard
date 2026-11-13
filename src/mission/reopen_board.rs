//! With UX 2.0, the Reopen view as board 18 draws it and board 45 lays it
//! out, new with the port (with UX 2.0 off the view is `render::list`'s,
//! one line a row): the rows under their day's label, each two lines, its
//! kind's chip (`tab`, `session`), its name bold, its directory dim, what
//! became of its work as a chip and when at the right edge, then its last
//! prompt; and under the list the chosen row's pane: its name and facts,
//! what reopening brings back, and its last screen as a picture
//! (`Shape::Screen`, `ui::images::screen`) with when it was opened and
//! closed and its turns beside it. Under `PANE_AT` columns the pane shows
//! over the list while `i` holds it there.
//!
//! Drawing reads only the snapshot: the rows' `reopen` data
//! (src/usage/reopen.rs) and the chosen row's last screen as the server
//! read it (`Snapshot::reopen_shot`).

use std::sync::Arc;

use ratatui::style::Style;
use ratatui::text::Span;
use serde_json::Value;

use super::render::{Image, Shape};
use super::views::{self, View, ViewId};
use super::{HitBox, Mission, Spot};
use crate::items::Item;
use crate::sidebar::render::Line;
use crate::sidebar::text::{self, width};
use crate::ui::kit::{self, Col, Tone};
use crate::usage::reopen;

/// Under this many columns the chosen row's pane is behind `i`.
pub const PANE_AT: usize = 140;
/// Under these many columns the parts that matter least go, in this
/// order: the facts beside the picture, the directory, and the outcome
/// chip's words past its mark.
const FACTS_AT: usize = 160;
const DIR_AT: usize = 150;
const WORDS_AT: usize = 140;
/// The name and the directory are cut past these.
const NAME_CAP: usize = 32;
const DIR_CAP: usize = 48;
/// The picture and the facts beside it share the pane by these weights,
/// the facts' column fixed at its width.
const PICTURE_WEIGHT: usize = 58;
const FACTS_W: usize = 38;
/// The picture's rows, and the least the list keeps above the pane.
const PICTURE_ROWS: usize = 8;
const LIST_LEAST: usize = 6;

/// Whether the board draws the view: UX 2.0's, read, with rows.
pub fn shown(m: &Mission, view: &View) -> bool {
    m.snap.rules.ux2
        && view.id == ViewId::Reopen
        && view.loaded
        && view.data.setup.is_none()
        && !view.data.items.is_empty()
}

fn s<'a>(r: &'a Value, key: &str) -> &'a str {
    r[key].as_str().unwrap_or("")
}

/// A kind's chip's tone.
fn kind_tone(kind: &str) -> Tone {
    if kind == reopen::SESSION {
        Tone::Landing
    } else {
        Tone::Accent
    }
}

/// What became of the work: its mark and tone.
fn outcome_look(outcome: &str) -> (&'static str, Tone) {
    if outcome.starts_with("landed") {
        ("✓", Tone::Done)
    } else if outcome.starts_with("untrusted") {
        ("●", Tone::Needs)
    } else {
        ("✕", Tone::Failed)
    }
}

/// How long ago, at the right edge.
fn ago(ms: i64, now_secs: i64) -> String {
    let a = crate::usage::sessions::ago(ms.div_euclid(1000), now_secs);
    if a.is_empty() {
        String::new()
    } else {
        format!("{a} ago")
    }
}

/// A time as the pane says it: its hour today, else its day and hour.
fn clock(ms: i64, m: &Mission) -> String {
    let Ok(at) = jiff::Timestamp::from_millisecond(ms) else {
        return String::new();
    };
    let at = at.to_zoned(m.snap.tz.clone());
    let now = jiff::Timestamp::from_second(m.snap.now_secs)
        .unwrap_or_default()
        .to_zoned(m.snap.tz.clone());
    if at.date() == now.date() {
        at.strftime("%-H:%M").to_string()
    } else {
        at.strftime("%b %-d %-H:%M").to_string()
    }
}

/// The directory cut from its start, so its end shows.
fn dir_cut(dir: &str, w: usize) -> String {
    if width(dir) <= w || w == 0 {
        return dir.to_string();
    }
    let chars: Vec<char> = dir.chars().collect();
    let mut kept = String::new();
    for c in chars.iter().rev() {
        if width(&kept) + 2 > w {
            break;
        }
        kept.insert(0, *c);
    }
    format!("…{kept}")
}

/// A row's two lines: its chips, name, directory and when; then its prompt.
fn row_lines(
    item: &Item,
    r: &Value,
    cols: usize,
    chosen: bool,
    m: &Mission,
) -> [Vec<Span<'static>>; 2] {
    let theme = &m.snap.theme;
    let caps = m.snap.caps;
    let kind = s(r, "kind");
    let outcome = s(r, "outcome");
    let (mark, tone) = outcome_look(outcome);
    let outcome_text = if outcome.is_empty() {
        String::new()
    } else if cols >= WORDS_AT {
        format!("{mark} {outcome}")
    } else {
        mark.to_string()
    };
    let kind_chip = kit::chip(kind, kind_tone(kind), caps, theme);
    let outcome_chip = if outcome_text.is_empty() {
        Vec::new()
    } else {
        kit::chip(&outcome_text, tone, caps, theme)
    };
    let when = ago(item.time.unwrap_or(0), m.snap.now_secs);
    let dir = s(r, "dir");
    let at = kit::columns(
        &[
            Col::Fixed {
                min: 0,
                natural: width("session") + 3,
                cap: 12,
            },
            Col::Fixed {
                min: 8,
                natural: width(&item.title),
                cap: NAME_CAP,
            },
            Col::Stretch {
                weight: 1,
                min: 0,
                max: Some(DIR_CAP),
            },
            Col::Fixed {
                min: 0,
                natural: kit::spans_width(&outcome_chip),
                cap: 40,
            },
            Col::Right {
                natural: width(&when),
                cap: 10,
            },
        ],
        cols.saturating_sub(2),
        1,
        &[2],
    );
    let dir_shown = cols >= DIR_AT;
    let name_style = if chosen {
        Tone::Text.bold(theme).fg(theme.bright)
    } else {
        Tone::Text.bold(theme)
    };
    let mut first = vec![Span::raw(" ")];
    let mut put = |spans: Vec<Span<'static>>, slot: Option<(usize, usize)>| {
        if let Some((x, w)) = slot {
            kit::place(&mut first, spans, (x + 1, w));
        }
    };
    put(kind_chip, at[0]);
    put(
        vec![Span::styled(text::cut(&item.title, NAME_CAP), name_style)],
        at[1],
    );
    if dir_shown && let Some((_, w)) = at[2] {
        put(
            vec![Span::styled(dir_cut(dir, w), Tone::Dim.style(theme))],
            at[2],
        );
    }
    put(outcome_chip, at[3]);
    put(vec![Span::styled(when, Tone::Faint.style(theme))], at[4]);
    let prompt = s(r, "prompt");
    let second = if prompt.is_empty() {
        vec![Span::styled("   no prompt", Tone::Faint.style(theme))]
    } else {
        vec![Span::styled(
            format!("   > {}", prompt.replace('\n', " ")),
            Tone::Dim.style(theme),
        )]
    };
    [first, second]
}

/// The chosen row's pane's lines, and where in them its picture goes (its
/// first line, x and width), `cols` wide.
type PaneOut = (Vec<Vec<Span<'static>>>, Option<(usize, usize, usize)>);

fn pane_lines(item: &Item, r: &Value, cols: usize, m: &Mission) -> PaneOut {
    let theme = &m.snap.theme;
    let inner = cols.saturating_sub(4);
    let kind = s(r, "kind");
    let mut facts: Vec<String> = Vec::new();
    let closed = r["closed"].as_i64().unwrap_or(0);
    let when = ago(closed, m.snap.now_secs);
    facts.push(if kind == reopen::SESSION {
        format!("last used {when}")
    } else {
        format!("closed {when}")
    });
    if let Some(n) = r["panes"].as_u64().filter(|n| *n > 0) {
        facts.push(format!("{n} pane{}", if n == 1 { "" } else { "s" }));
    }
    if let Some(id) = r["session"].as_str() {
        let short: String = id.chars().take(4).collect();
        facts.push(format!("Claude session {short}…"));
    }
    let mut out = kit::pane(
        &item.title,
        vec![Span::styled(facts.join(" · "), Tone::Dim.style(theme))],
        inner,
        theme,
    );
    let words = format!("Reopening brings back {}.", reopen::brings_back(r));
    for w in text::wrap(&words, inner.min(kit::WRAP_MAX)) {
        out.push(vec![Span::styled(w, Tone::Text.style(theme))]);
    }
    let mut times = Vec::new();
    if let Some(opened) = r["opened"].as_i64().filter(|o| *o > 0) {
        times.push(format!("Opened {}", clock(opened, m)));
    }
    if closed > 0 {
        let word = if kind == reopen::SESSION {
            "last used"
        } else {
            "closed"
        };
        let word = if times.is_empty() {
            let mut w = word.to_string();
            w[..1].make_ascii_uppercase();
            w
        } else {
            word.to_string()
        };
        times.push(format!("{word} {}", clock(closed, m)));
    }
    if let Some(n) = r["turns"].as_u64() {
        times.push(format!("{n} turn{}", if n == 1 { "" } else { "s" }));
    }
    let times = times.join(" · ");
    let has_screen = m.snap.reopen_shot.is_some() && r["screen"].is_string();
    if !has_screen {
        if !times.is_empty() {
            out.push(vec![Span::styled(times, Tone::Dim.style(theme))]);
        }
        return (out, None);
    }
    out.push(kit::label("Last screen", Tone::Dim, None, inner, theme));
    let at = kit::columns(
        &[
            Col::Stretch {
                weight: PICTURE_WEIGHT,
                min: 20,
                max: None,
            },
            Col::Fixed {
                min: 0,
                natural: FACTS_W,
                cap: FACTS_W,
            },
        ],
        inner,
        2,
        &[1],
    );
    let picture_at = out.len();
    let facts_beside = cols >= FACTS_AT && at[1].is_some();
    let facts_lines: Vec<String> = if facts_beside {
        text::wrap(&times, FACTS_W)
    } else {
        Vec::new()
    };
    let (px, pw) = at[0].unwrap_or((0, inner));
    for row in 0..PICTURE_ROWS {
        let mut line = vec![Span::raw(" ".repeat(px + pw))];
        if let (Some((fx, fw)), Some(f)) = (at[1], facts_lines.get(row)) {
            kit::place(
                &mut line,
                vec![Span::styled(f.clone(), Tone::Dim.style(theme))],
                (fx, fw),
            );
        }
        out.push(line);
    }
    if !facts_beside && !times.is_empty() {
        out.push(vec![Span::styled(times, Tone::Dim.style(theme))]);
    }
    (out, Some((picture_at, px, pw)))
}

/// The board, under the view's bar.
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
    let items = &view.data.items;
    let cursor = (m.cursor.max(0) as usize).min(items.len().saturating_sub(1));
    let chosen = items.get(cursor).filter(|i| !views::is_header(i));
    let body = rows.saturating_sub(1 + lines.len());
    let wide = cols >= PANE_AT;
    let pane = chosen
        .and_then(|i| Some((i, reopen::of(i)?)))
        .map(|(i, r)| pane_lines(i, r, cols, m));
    // Under PANE_AT, `i` shows the pane in the list's place, while a row
    // is chosen (a view beside it with the focus chooses none here).
    if !wide
        && m.reopen_pane
        && let Some((pane, picture)) = pane
    {
        put_pane(pane, picture, cols, body, m, lines, hits, images);
        return;
    }
    let pane = pane.filter(|(p, _)| wide && body >= p.len() + 1 + LIST_LEAST);
    let pane_h = pane.as_ref().map_or(0, |(p, _)| p.len() + 1);
    let room = body.saturating_sub(pane_h).max(1);
    // Each row is two lines, a heading one; the window on them keeps the
    // chosen row in sight.
    let heights: Vec<usize> = items
        .iter()
        .map(|i| if views::is_header(i) { 1 } else { 2 })
        .collect();
    let mut first = 0;
    while first < cursor && heights[first..=cursor].iter().sum::<usize>() > room {
        first += 1;
    }
    let full = cols;
    let mut used = 0;
    for (index, item) in items.iter().enumerate().skip(first) {
        let h = heights[index];
        if used + h > room {
            break;
        }
        used += h;
        let is_chosen = index == cursor && m.cursor >= 0;
        let hovered = !is_chosen && m.hover == Some(Spot::Index(index as isize));
        let spot = Spot::Index(index as isize);
        if views::is_header(item) {
            let count = item.data["count"].as_u64().map(|n| n as usize);
            hits.push(HitBox {
                y: lines.len() as u16,
                x0: 0,
                x1: full.saturating_sub(1) as u16,
                spot,
            });
            lines.push(Line {
                spans: kit::label(&item.title, Tone::Dim, count, full, theme),
                fill: None,
            });
            continue;
        }
        let Some(r) = reopen::of(item) else {
            continue;
        };
        for spans in row_lines(item, r, full, is_chosen, m) {
            hits.push(HitBox {
                y: lines.len() as u16,
                x0: 0,
                x1: full.saturating_sub(1) as u16,
                spot,
            });
            lines.push(Line {
                spans: kit::row(spans, full, is_chosen, theme),
                fill: hovered.then_some(theme.panel),
            });
        }
    }
    if let Some((pane, picture)) = pane {
        let top = lines.len();
        while lines.len() < top + room.saturating_sub(used) {
            lines.push(Line {
                spans: vec![Span::raw(" ".repeat(full))],
                fill: None,
            });
        }
        lines.push(Line {
            spans: vec![kit::rule(full, theme)],
            fill: None,
        });
        put_pane(pane, picture, cols, pane_h - 1, m, lines, hits, images);
    }
}

/// The pane's lines put under what is there, indented two columns, its
/// picture placed as an image over its blank lines.
#[allow(clippy::too_many_arguments)]
fn put_pane(
    pane: Vec<Vec<Span<'static>>>,
    picture: Option<(usize, usize, usize)>,
    cols: usize,
    room: usize,
    m: &Mission,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
    images: &mut Vec<Image>,
) {
    let top = lines.len();
    let last = pane.len().saturating_sub(room);
    for l in pane.into_iter().take(room) {
        hits.push(HitBox {
            y: lines.len() as u16,
            x0: 0,
            x1: cols.saturating_sub(1) as u16,
            spot: Spot::Side(last),
        });
        let mut spans = vec![Span::raw("  ")];
        spans.extend(l);
        lines.push(Line {
            spans: kit::fit(spans, cols, Style::new()),
            fill: None,
        });
    }
    if let (Some((at, x, w)), Some(shot)) = (picture, m.snap.reopen_shot.as_ref())
        && at + PICTURE_ROWS <= room
    {
        images.push(Image {
            name: "mission/reopen/screen".into(),
            x: (x + 2) as u16,
            y: (top + at) as u16,
            width: w as u16,
            height: PICTURE_ROWS as u16,
            shape: Shape::Screen {
                shot: Arc::clone(shot),
            },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_directory_keeps_its_end() {
        assert_eq!(dir_cut("~/code/app", 20), "~/code/app");
        let cut = dir_cut("~/code/some/very/long/path/to/worktree", 16);
        assert!(cut.starts_with('…') && cut.ends_with("worktree"));
        assert!(width(&cut) <= 16);
    }

    #[test]
    fn each_outcome_has_its_mark_and_tone() {
        assert_eq!(outcome_look("landed a3f1"), ("✓", Tone::Done));
        assert_eq!(outcome_look("tests failed"), ("✕", Tone::Failed));
        assert_eq!(outcome_look("changes asked"), ("✕", Tone::Failed));
        assert_eq!(
            outcome_look("untrusted · resumes sealed"),
            ("●", Tone::Needs)
        );
    }
}
