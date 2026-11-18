//! Selecting the diff's text and copying it, which nothing did in
//! diffview.nvim's place but the terminal's own selection, lost once the
//! diff took the mouse for its clicks, wheel and scroll bars. A drag
//! selects by characters within the pane it starts in (the list, a side of
//! the diff, the rows across both sides, the heading over the diff, an
//! answer), a double click a word and a triple click a line; the release
//! copies it, as a terminal with copy-on-select does, and `y` copies it
//! again while it shows.
//!
//! The selection is of the cells drawn, so every pane is selected one way:
//! as the screen is drawn each row of text notes where it is (`Spot`), and
//! the drawn cells are read back from the buffer once the frame is done. A
//! side of the diff's spots leave out its line numbers and the other side,
//! so a copy has neither. Rows are numbered by their place in what scrolls
//! (the list's row, the answer's line, the diff's rows counted from where
//! it was first drawn), so the selection stays on its text as the wheel
//! scrolls, and the text of each row is kept as it is seen, so what is
//! copied holds the rows scrolled out of sight too.
//!
//! With the keys, `v` and `V` select lines (in a side of the diff, the list
//! and an answer) and `y` copies their text, from the file's lines rather
//! than the screen's, so a line wrapped is copied whole and a tab as a tab.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use super::{Act, App, Focus, ListRow};
use switchboard_ui::draw::width;

/// How soon a press on the same cell counts as the same click again.
const DOUBLE_CLICK: Duration = Duration::from_millis(500);

/// The parts of the screen a selection stays within.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pane {
    List,
    /// The page's heading and the bar over the diff, which don't scroll.
    Head,
    Old,
    New,
    /// A row of the diff across both sides: a fold, a note, the rule and
    /// the bar between two files of the one scroll.
    Wide,
    Answer,
}

/// A row of text as it was last drawn: its pane, the cells of its text,
/// the column its pane starts at there (a side's line numbers included),
/// its row in what scrolls, whether it goes on from the row before (a line
/// wrapped), how many of its cells hold text (a wrapped line's piece ends
/// short of the width, and what follows it is no part of the line), and
/// its text, read from those cells once the frame is drawn.
pub(super) struct Spot {
    pane: Pane,
    rect: Rect,
    left: u16,
    row: i64,
    joins: bool,
    used: u16,
    text: String,
}

impl Spot {
    pub(super) fn new(pane: Pane, rect: Rect, left: u16, row: i64, joins: bool) -> Spot {
        Spot {
            pane,
            rect,
            left,
            row,
            joins,
            used: rect.width,
            text: String::new(),
        }
    }

    /// The spot with only its first `used` cells its text.
    pub(super) fn fitted(mut self, used: u16) -> Spot {
        self.used = used.min(self.rect.width);
        self
    }

    /// How far column `x` is from the spot's pane on its row.
    fn away(&self, x: u16) -> u16 {
        if x < self.left {
            self.left - x
        } else {
            (x + 1).saturating_sub(self.rect.right())
        }
    }
}

/// A row of the selection as it was last seen.
struct Seen {
    x: u16,
    text: String,
    joins: bool,
}

/// What the mouse selected: its pane, the two ends as a row and a column,
/// and each of its rows as last seen.
pub(super) struct Selection {
    pane: Pane,
    from: (i64, u16),
    to: (i64, u16),
    /// Whole rows, as a triple click selects.
    lines: bool,
    /// Where the mouse is while its button is held, so the end follows the
    /// text scrolling under it.
    held: Option<(u16, u16)>,
    /// Until the button is let go, which copies it.
    fresh: bool,
    seen: BTreeMap<i64, Seen>,
}

impl Selection {
    fn ends(&self) -> ((i64, u16), (i64, u16)) {
        (self.from.min(self.to), self.from.max(self.to))
    }

    /// The columns of a row selected, the last included.
    fn columns(&self, row: i64) -> (u16, u16) {
        let (a, b) = self.ends();
        if self.lines {
            return (0, u16::MAX);
        }
        (
            if row == a.0 { a.1 } else { 0 },
            if row == b.0 { b.1 } else { u16::MAX },
        )
    }

    /// Keeps the text of its pane's rows that are on the screen, those
    /// outside it too: one turn of the wheel takes several rows off the
    /// screen at once, which the selection then runs over unseen.
    fn see(&mut self, spots: &[Spot]) {
        for s in spots {
            if s.pane == self.pane {
                self.seen.insert(
                    s.row,
                    Seen {
                        x: s.rect.x,
                        text: s.text.clone(),
                        joins: s.joins,
                    },
                );
            }
        }
    }

    /// The text selected: each row's, a new line between them but where a
    /// row goes on from the one before, with nothing after a line's last
    /// character.
    fn text(&self) -> String {
        let (a, b) = self.ends();
        let mut lines: Vec<String> = Vec::new();
        for (&row, seen) in self.seen.range(a.0..=b.0) {
            let (lo, hi) = self.columns(row);
            let piece = columns_of(&seen.text, seen.x, lo, hi);
            match lines.last_mut() {
                Some(last) if seen.joins => last.push_str(&piece),
                _ => lines.push(piece),
            }
        }
        let lines: Vec<&str> = lines.iter().map(|l| l.trim_end()).collect();
        lines.join("\n").trim_end().to_string()
    }
}

/// The characters of a row's text, drawn from column `x`, with the column
/// each starts at.
fn placed(text: &str, x: u16) -> impl Iterator<Item = (u16, char)> + '_ {
    let mut col = x;
    text.chars().map(move |c| {
        let at = col;
        col += width(c.encode_utf8(&mut [0; 4])) as u16;
        (at, c)
    })
}

/// What of a row's text is in columns `lo` to `hi`, both included.
fn columns_of(text: &str, x: u16, lo: u16, hi: u16) -> String {
    placed(text, x)
        .filter(|(col, _)| (lo..=hi).contains(col))
        .map(|(_, c)| c)
        .collect()
}

/// The cells of a row as text, a wide character's second cell left out.
fn text_of(buf: &Buffer, rect: Rect) -> String {
    let rect = rect.intersection(buf.area);
    let mut out = String::new();
    let mut skip = 0;
    for x in rect.x..rect.right() {
        if skip > 0 {
            skip -= 1;
            continue;
        }
        let symbol = buf[(x, rect.y)].symbol();
        out.push_str(symbol);
        skip = width(symbol).saturating_sub(1);
    }
    out
}

/// The pane a press at a place is in: the one whose row of text, or the
/// line numbers before it, the place is on; none on a row with no text
/// there (the list's empty rows, its filter's line, the gap facing a line
/// only the other side has).
fn pane_at(spots: &[Spot], x: u16, y: u16) -> Option<Pane> {
    spots
        .iter()
        .find(|s| s.rect.y == y && s.away(x) == 0)
        .map(|s| s.pane)
}

/// The row and column of a pane nearest a place on the screen: on the
/// place's own row its column kept within the text, above the pane's rows
/// the start of its first, below them the end of its last.
fn point(spots: &[Spot], pane: Pane, x: u16, y: u16) -> Option<(i64, u16)> {
    let s = spots
        .iter()
        .filter(|s| s.pane == pane)
        .min_by_key(|s| (s.rect.y.abs_diff(y), s.away(x)))?;
    let last = s.rect.right().saturating_sub(1);
    let col = match s.rect.y.cmp(&y) {
        std::cmp::Ordering::Equal => x.clamp(s.rect.x, last),
        std::cmp::Ordering::Less => last,
        std::cmp::Ordering::Greater => s.rect.x,
    };
    Some((s.row, col))
}

/// What a copy says: how many lines it took.
fn copied(text: &str) -> String {
    match text.lines().count() {
        1 => "Copied 1 line".to_string(),
        n => format!("Copied {n} lines"),
    }
}

/// The kinds of character a double click tells apart.
fn kind(c: char) -> u8 {
    if c.is_alphanumeric() || c == '_' {
        0
    } else if c.is_whitespace() {
        1
    } else {
        2
    }
}

impl App {
    // ── What the screen notes as it is drawn ────────────────────────────────

    /// The diff's rows are about to be drawn from `now` (a file of the list
    /// and its line), under row `body_y`: counts how far that is from where
    /// they were last drawn, so a row keeps its number as the diff scrolls.
    /// A selection in the diff ends where that can't be counted (another
    /// file shown).
    pub(super) fn diff_moved(&mut self, now: (usize, usize), body_y: u16) {
        let moved = match self.diff_drawn {
            Some(was) => self.rows_between(was, now),
            None => Some(0),
        };
        match moved {
            Some(by) => self.diff_scrolled += by,
            None => {
                self.diff_scrolled = 0;
                let in_diff = self
                    .selection
                    .as_ref()
                    .is_some_and(|s| matches!(s.pane, Pane::Old | Pane::New | Pane::Wide));
                if in_diff {
                    self.selection = None;
                }
            }
        }
        self.diff_drawn = Some(now);
        self.diff_base = self.diff_scrolled - i64::from(body_y);
    }

    /// The rows from one place of the diff's scroll to another, back or
    /// ahead; none between two files that don't follow in one scroll, or
    /// past a file not read yet.
    fn rows_between(&mut self, from: (usize, usize), to: (usize, usize)) -> Option<i64> {
        if from > to {
            return self.rows_between(to, from).map(|rows| -rows);
        }
        if from.0 == to.0 {
            return Some((to.1 - from.1) as i64);
        }
        if !self.one_scroll() {
            return None;
        }
        let mut rows = 0;
        let mut t = from.1;
        for k in from.0..to.0 {
            if !self.is_read(k) {
                return None;
            }
            rows += self.stream_len(k).saturating_sub(t);
            t = 0;
        }
        Some((rows + to.1) as i64)
    }

    /// Notes a row of text drawn, for the mouse to select.
    pub(super) fn spot(&mut self, pane: Pane, rect: Rect, left: u16, row: i64, joins: bool) {
        if rect.width > 0 {
            self.spots.push(Spot::new(pane, rect, left, row, joins));
        }
    }

    /// Once the frame's panes are drawn: reads each row's text back from
    /// the cells, keeps the selection's, and lights it. Under a box only
    /// the box's own rows are there to select.
    pub(super) fn read_spots(&mut self, buf: &mut Buffer) {
        if self.ask_box.is_some() {
            self.spots.clear();
        } else if self.answer.is_some() {
            self.spots.retain(|s| s.pane == Pane::Answer);
        }
        // A row with no cells (a note on a side too narrow to show) has no
        // text to select, nor a column to put the selection's end on.
        self.spots.retain(|s| s.rect.width > 0);
        for s in &mut self.spots {
            let cells = Rect {
                width: s.used,
                ..s.rect
            };
            s.text = text_of(buf, cells);
        }
        let lit = Color::Indexed(if self.opts.light { 252 } else { 239 });
        let Some(sel) = &mut self.selection else {
            return;
        };
        if let Some((x, y)) = sel.held
            && let Some(to) = point(&self.spots, sel.pane, x, y)
        {
            sel.to = to;
        }
        sel.see(&self.spots);
        let (a, b) = sel.ends();
        for s in &self.spots {
            if s.pane != sel.pane || !(a.0..=b.0).contains(&s.row) {
                continue;
            }
            let (lo, hi) = sel.columns(s.row);
            let rect = s.rect.intersection(buf.area);
            for x in rect.x.max(lo)..rect.right().min(hi.saturating_add(1)) {
                buf[(x, rect.y)].set_bg(lit);
            }
        }
    }

    // ── The mouse ───────────────────────────────────────────────────────────

    /// How many presses in a row this one makes on its cell: 1, 2 or 3.
    pub(super) fn count_click(&mut self, x: u16, y: u16) -> u8 {
        let n = match self.clicks {
            Some((at, cx, cy, n)) if (cx, cy) == (x, y) && at.elapsed() < DOUBLE_CLICK && n < 3 => {
                n + 1
            }
            _ => 1,
        };
        self.clicks = Some((Instant::now(), x, y, n));
        n
    }

    /// A double click's word, or a triple click's line, selected; false
    /// where the click is on no text.
    pub(super) fn select_unit(&mut self, x: u16, y: u16, lines: bool) -> bool {
        let Some(s) = self
            .spots
            .iter()
            .find(|s| s.rect.y == y && x >= s.rect.x && x < s.rect.right())
        else {
            return false;
        };
        let (mut first, mut last) = (x, x);
        // A line wrapped is the rows from its first piece to its last.
        let (mut top, mut bottom) = (s.row, s.row);
        if lines {
            let joins = |row: i64| {
                self.spots
                    .iter()
                    .any(|o| o.pane == s.pane && o.row == row && o.joins)
            };
            while joins(top) {
                top -= 1;
            }
            while joins(bottom + 1) {
                bottom += 1;
            }
        } else {
            let chars: Vec<(u16, char)> = placed(&s.text, s.rect.x).collect();
            let Some(at) = chars.iter().rposition(|(col, _)| *col <= x) else {
                return false;
            };
            let of = kind(chars[at].1);
            let start = chars[..at]
                .iter()
                .rposition(|(_, c)| kind(*c) != of)
                .map_or(0, |i| i + 1);
            let end = chars[at..]
                .iter()
                .position(|(_, c)| kind(*c) != of)
                .map_or(chars.len(), |i| at + i);
            (first, last) = (chars[start].0, chars[end - 1].0);
        }
        let mut sel = Selection {
            pane: s.pane,
            from: (top, first),
            to: (bottom, last),
            lines,
            held: None,
            fresh: true,
            seen: BTreeMap::new(),
        };
        sel.see(&self.spots);
        self.selection = Some(sel);
        true
    }

    /// The mouse dragged with its button held since a press: a selection
    /// starts once it has left the press's cell, in the pane pressed in,
    /// and its end follows the mouse.
    pub(super) fn drag_select(&mut self, x: u16, y: u16) -> Act {
        let Some((px, py)) = self.pressed else {
            return Act::Same;
        };
        match &mut self.selection {
            Some(sel) if sel.fresh => {
                sel.held = Some((x, y));
                if let Some(to) = point(&self.spots, sel.pane, x, y) {
                    sel.to = to;
                }
                sel.see(&self.spots);
            }
            _ => {
                if (x, y) == (px, py) {
                    return Act::Same;
                }
                let Some(pane) = pane_at(&self.spots, px, py) else {
                    return Act::Same;
                };
                let (Some(from), Some(to)) = (
                    point(&self.spots, pane, px, py),
                    point(&self.spots, pane, x, y),
                ) else {
                    return Act::Same;
                };
                let mut sel = Selection {
                    pane,
                    from,
                    to,
                    lines: false,
                    held: Some((x, y)),
                    fresh: true,
                    seen: BTreeMap::new(),
                };
                sel.see(&self.spots);
                self.selection = Some(sel);
            }
        }
        Act::Redraw
    }

    /// The button let go: what was selected since it was pressed is copied,
    /// and with nothing selected a press on an answer's path:line goes
    /// there.
    pub(super) fn release_select(&mut self) -> Act {
        self.pressed = None;
        let cite = self.pressed_cite.take();
        let Some(sel) = &mut self.selection else {
            let Some(cite) = cite else {
                return Act::Same;
            };
            self.cited(cite);
            return Act::Redraw;
        };
        if !sel.fresh {
            return Act::Same;
        }
        sel.fresh = false;
        sel.held = None;
        self.copy_selection();
        Act::Redraw
    }

    /// Copies what the mouse selected, and says how much.
    pub(super) fn copy_selection(&mut self) {
        let Some(text) = self.selection.as_ref().map(Selection::text) else {
            return;
        };
        self.copy_text(&text);
    }

    fn copy_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.clip(text);
        self.say(copied(text));
    }

    // ── The keys ────────────────────────────────────────────────────────────

    /// `y` on the lines `v` or `V` selected: their text copied, the list's
    /// rows as they read (a file's path, a page's title) and a side's
    /// lines whole. The selection stays for the key after it, since `yp`
    /// and `yl` copy its place instead.
    pub(super) fn copy_visual(&mut self) {
        let list = self.focus == Focus::List;
        let Some(from) = (if list { self.list_visual } else { self.visual }) else {
            return;
        };
        let text = if list {
            let rows = self.list_rows();
            let place = |k: usize| rows.iter().position(|r| *r == ListRow::File(k));
            let (Some(a), Some(b)) = (place(from), place(self.at)) else {
                return;
            };
            let lines: Vec<String> = rows[a.min(b)..=a.max(b)]
                .iter()
                .filter_map(|r| self.list_text(r))
                .collect();
            lines.join("\n")
        } else {
            let new_side = self.focus == Focus::New;
            let (Some(line), Some(entry)) = (self.line_under(new_side), self.current().cloned())
            else {
                return;
            };
            let other = self.line_at(new_side, from).unwrap_or(line);
            // The file's own lines, read as the side was (and kept since):
            // the screen's have a tab as four spaces.
            let Some((left, right)) = self.change.as_ref().map(|c| c.sides(&entry)) else {
                return;
            };
            let read = if new_side {
                self.text(&right, &entry.path)
            } else {
                self.text(&left, entry.old())
            };
            let Some(read) = read else {
                return;
            };
            let side: Vec<&str> = read
                .split('\n')
                .map(|l| l.strip_suffix('\r').unwrap_or(l))
                .collect();
            let (first, last) = (line.min(other), line.max(other).min(side.len()));
            side.get(first.saturating_sub(1)..last)
                .unwrap_or_default()
                .join("\n")
        };
        self.yanked = true;
        self.copy_text(&text);
    }

    /// A row of the list as `y` copies it; none for a row with no words of
    /// the change's own.
    fn list_text(&self, row: &ListRow) -> Option<String> {
        match row {
            ListRow::File(k) => self
                .order
                .get(*k)
                .and_then(|&i| self.entry(i))
                .map(|e| e.path.clone()),
            ListRow::Heading(text, _) => Some(text.trim().to_string()),
            ListRow::Page(n) => self
                .story
                .as_ref()
                .and_then(|s| s.pages.get(n - 1))
                .map(|c| format!("{}. {}", self.page_shown(*n), c.title)),
            ListRow::Chapter(c) => self
                .story
                .as_ref()
                .and_then(|s| s.chapters.get(*c))
                .map(|t| format!("{}. {}", c + 1, t.title)),
            ListRow::Label(text, ..) => Some(text.clone()),
            ListRow::Header(section) => Some(section.heading().to_string()),
            ListRow::Loading | ListRow::Blank | ListRow::Totals | ListRow::Hint(_) => None,
        }
    }

    /// `y` on the lines `v` selected in an answer: their text, as laid out,
    /// less the margin they share.
    pub(super) fn copy_answer_lines(&mut self) {
        let Some((from, cursor)) = self
            .answer
            .as_ref()
            .and_then(|a| Some((a.visual?, a.cursor)))
        else {
            return;
        };
        let lines = self.answer_lines(self.answer_width());
        let last = lines.len().saturating_sub(1);
        let (a, b) = (from.min(cursor).min(last), from.max(cursor).min(last));
        let text: Vec<&str> = lines
            .get(a..=b)
            .unwrap_or_default()
            .iter()
            .map(|l| l.0.trim_end())
            .collect();
        let margin = text
            .iter()
            .filter(|l| !l.is_empty())
            .map(|l| l.len() - l.trim_start_matches(' ').len())
            .min()
            .unwrap_or(0);
        let text: Vec<&str> = text.iter().map(|l| l.get(margin..).unwrap_or("")).collect();
        self.yanked = true;
        self.copy_text(text.join("\n").trim_end());
    }
}
