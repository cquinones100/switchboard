//! The file list's width, yours to set as a pane's is, split from app.rs:
//! a drag of the list's right edge (the column between it and the diff,
//! lit under the mouse) and the pane resize keys while the list has the
//! keys set it, and the diff's menu (`m`, a right click) has "Use the file
//! list's default width", which goes back to `LIST_WIDTH`. New with the
//! diff_list_resize setting; nothing in the tmux version did this. A first
//! version was built beside the sidebar's resizing and taken out before it
//! landed (the branch `keep/resize-notes-and-diff`); this is its diff part,
//! with the cursor kept on what it is on through every width change.
//!
//! The width is kept in the diff's state folder (`diff-list-width`, the
//! columns as text), one for every diff, written when a drag is let go and
//! after a key, never as the mouse moves, and read as the diff opens; no
//! file is the default. A list given a width opens at it whatever
//! diff_list_wide says, and `E` goes between it and the longest path.
//! With diff_list_resize off the list is as it was: `LIST_WIDTH`, its edge
//! and the keys doing nothing, the width kept left in its file.
//!
//! The keys are the ones bound to moving a pane's divider left or right,
//! which the workspace names as it starts the diff (`--resize-key`), since
//! `keys.json` may bind others; alone, or typed in a pane, the defaults.
//! The diff marks its pane with them (`@resize_keys`), so the workspace's
//! server passes those keys on to it rather than moving the divider; the
//! diff hands one back (`press resize-left-5`) when the list hasn't the
//! keys, so the pane's divider moves as it did.
//!
//! Every change of the list's width or the screen's (a drag, a key, `E`,
//! the menu, the pane resized) wraps the diff's lines anew beside it, and
//! `relaid` keeps the cursor and a selection made with `v` on what they
//! were on (a row of the diff, a fold, a note, a card or an "also in"
//! line, and which of its wrapped lines), rather than on the same screen
//! line, which a new wrapping fills with another. The view's first line is
//! kept the same way, and then follows the cursor as it does after any
//! key, wherever the drawing keeps the cursor in sight, so a cursor the
//! new wrapping pushes past the view's bottom scrolls it as far. The
//! mouse's selection is cells of the screen, which a new width moves, so
//! it is let go.
//!
//! Started by the workspace (in a pane of it), the diff takes only the keys
//! it was named, none when keys.json binds none; alone, the workspace's
//! default keys.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use switchboard_ui::input::{self, Key, Mods};
use switchboard_ui::kit::Tone;

use super::{Act, App, Cell, Focus, LIST_WIDTH, Line, ListRow, Mode, Telling};
use crate::host::host;
use crate::{paths, util};

/// The narrowest the list is made; the widest is half the screen.
const LEAST_WIDTH: u16 = 24;

/// A key that moves a pane's divider, and the columns it moves it by: to
/// the right when positive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResizeKey {
    key: Key,
    by: i32,
}

impl ResizeKey {
    /// A key in the workspace's notation (`C-M-S-h`) with its columns, as
    /// `--resize-key` names one; `None` when either isn't one, or the key
    /// is one the diff can't take as a resize (Esc, Enter, Tab and the
    /// like, which keep their own meaning in it), so the workspace keeps
    /// such a key's binding.
    pub fn parse(notation: &str, columns: &str) -> Option<ResizeKey> {
        let key = Key::parse(notation).filter(|k| takes(k.code))?;
        let most = i32::from(u16::MAX);
        let by = columns
            .parse::<i32>()
            .ok()
            .filter(|by| *by != 0 && (-most..=most).contains(by))?;
        Some(ResizeKey { key, by })
    }
}

/// Whether a key of this kind can be a resize key in the diff: the ones
/// `ui_key` reads back, but the function keys, some of which a terminal
/// sends as other replies with a modifier (S-F3), which stay the
/// workspace's.
fn takes(code: input::KeyCode) -> bool {
    use input::KeyCode as K;
    matches!(
        code,
        K::Char(_)
            | K::Left
            | K::Right
            | K::Up
            | K::Down
            | K::Home
            | K::End
            | K::PageUp
            | K::PageDown
    )
}

/// The keys the diff takes, each in the workspace's notation with the
/// columns it moves the list by (`C-M-S-h=-5`), a space between, as it
/// marks its pane with them (`@resize_keys`, `keys:` before them), so
/// the workspace passes on only a key still bound to those columns.
pub(super) fn notations(keys: &[ResizeKey]) -> String {
    keys.iter()
        .map(|k| format!("{}={}", k.key.notation(), k.by))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The workspace's own keys for it, five columns a press.
pub(super) fn default_keys() -> Vec<ResizeKey> {
    [("C-M-S-h", "-5"), ("C-M-S-l", "5")]
        .iter()
        .filter_map(|(key, by)| ResizeKey::parse(key, by))
        .collect()
}

/// Where the list's width is kept.
pub(super) fn kept_file() -> PathBuf {
    paths::state_dir().join("diff-list-width")
}

/// A width for a screen `cols` wide: from `LEAST_WIDTH` to half of it.
fn clamped(width: i32, cols: u16) -> u16 {
    let most = (cols / 2).max(LEAST_WIDTH);
    width.clamp(i32::from(LEAST_WIDTH), i32::from(most)) as u16
}

/// A key as the workspace names one.
fn ui_key(key: &KeyEvent) -> Option<Key> {
    let code = match key.code {
        KeyCode::Char(c) => input::KeyCode::Char(c),
        KeyCode::Left => input::KeyCode::Left,
        KeyCode::Right => input::KeyCode::Right,
        KeyCode::Up => input::KeyCode::Up,
        KeyCode::Down => input::KeyCode::Down,
        KeyCode::Home => input::KeyCode::Home,
        KeyCode::End => input::KeyCode::End,
        KeyCode::PageUp => input::KeyCode::PageUp,
        KeyCode::PageDown => input::KeyCode::PageDown,
        KeyCode::F(n) => input::KeyCode::F(n),
        _ => return None,
    };
    let mods = Mods {
        shift: key.modifiers.contains(KeyModifiers::SHIFT),
        alt: key.modifiers.contains(KeyModifiers::ALT),
        ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
    };
    Some(Key::new(code, mods))
}

/// What a screen line of the diff is part of, the same for each of its
/// wrapped lines: a row of the diff, a fold, or a note, a note's card or
/// an "also in" line after the row or fold before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spot {
    Row(usize),
    Fold(usize),
    Note(Option<After>),
    Card(Option<After>),
    Also(Option<After>),
}

/// The row or the fold a note, a card or an "also in" line comes after, so
/// a card after a fold isn't taken for one after the row before the fold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum After {
    Row(usize),
    Fold(usize),
}

/// Each screen line's `Spot`, in order.
fn spots_of(lines: &[Line]) -> Vec<Spot> {
    let mut last = None;
    lines
        .iter()
        .map(|l| match l {
            Line::Pair { row, .. } => {
                last = Some(After::Row(*row));
                Spot::Row(*row)
            }
            Line::Fold { first, .. } => {
                last = Some(After::Fold(*first));
                Spot::Fold(*first)
            }
            Line::Note { .. } => Spot::Note(last),
            Line::Card(_) => Spot::Card(last),
            Line::Also(..) => Spot::Also(last),
        })
        .collect()
}

/// Whether a screen line is blank on one side: the gap facing the other
/// side's wrapped piece.
fn blank_on(line: &Line, new_side: bool) -> bool {
    matches!(line, Line::Pair { old, new, .. }
        if matches!(if new_side { new } else { old }, Cell::Gap))
}

/// A note, card or "also in" line whose row or fold has opened or closed
/// since: the same kind of line after the first row past an opened fold's
/// start, or after the fold now closed over its row.
fn after_moved(spot: Spot, spots: &[Spot], lines: &[Line]) -> Spot {
    let (kind, after): (fn(Option<After>) -> Spot, After) = match spot {
        Spot::Note(Some(a)) => (Spot::Note, a),
        Spot::Card(Some(a)) => (Spot::Card, a),
        Spot::Also(Some(a)) => (Spot::Also, a),
        _ => return spot,
    };
    let moved = match after {
        After::Fold(start) => spots.iter().find_map(|s| {
            let candidate = match *s {
                Spot::Note(Some(After::Row(r)))
                | Spot::Card(Some(After::Row(r)))
                | Spot::Also(Some(After::Row(r)))
                    if r >= start =>
                {
                    kind(Some(After::Row(r)))
                }
                _ => return None,
            };
            (candidate == *s).then_some(candidate)
        }),
        After::Row(row) => lines.iter().find_map(|l| match l {
            Line::Fold { first, count } if (*first..first + count).contains(&row) => {
                Some(kind(Some(After::Fold(*first))))
            }
            _ => None,
        }),
    };
    moved.unwrap_or(spot)
}

/// Where the list is, kept as its lines wrap anew (`list_rewrapped`).
#[derive(Clone, Debug)]
pub(super) struct ListPlace {
    cursor: Option<usize>,
    top: Option<(ListRow, usize)>,
    /// How many rows from the list's top to its end, for a top under its
    /// last file (the filter's hints).
    from_end: usize,
    /// Whether the cursor's file was in sight, at the height then.
    in_sight: bool,
}

/// A screen line of a file by what is on it: its spot and which of the
/// spot's lines, or, in the one scroll, how far past the file's last line
/// (the rule and the next file's bar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    /// The spot, which of its lines, and whether that line held text on
    /// the old side and on the new.
    On(Spot, usize, [bool; 2]),
    Past(usize),
}

/// Where the cursor, a selection by `v` and the view's first line were, as
/// the lines wrap anew.
#[derive(Clone, Copy, Debug)]
pub(super) struct Anchors {
    cur: Place,
    visual: Option<Place>,
    top: Place,
}

impl App {
    /// Where screen line `i` of the list's file `k` is.
    fn place_in(&mut self, k: usize, i: usize) -> Place {
        let lines = self.lines_of(k);
        let spots = spots_of(lines);
        let Some(spot) = spots.get(i).copied() else {
            return Place::Past(i.saturating_sub(spots.len()));
        };
        let first = spots.iter().position(|s| *s == spot).unwrap_or(i);
        let text = [!blank_on(&lines[i], false), !blank_on(&lines[i], true)];
        Place::On(spot, i - first, text)
    }

    /// The sides, old and new, the cursor and a selection by `v` are kept
    /// on text on: the side with the keys, and from the list both, since
    /// Tab goes back to one and M-Right to the other.
    fn kept_sides(&self) -> [bool; 2] {
        match self.focus {
            Focus::Old => [true, false],
            Focus::New => [false, true],
            Focus::List => [true, true],
        }
    }

    /// The screen line of the list's file `k` at `place`: the spot's `nth`
    /// line, or its last where it now has fewer, and, for the cursor or a
    /// selection by `v` (`cursor`), never a blank piece of the side with the keys where that
    /// side's line now takes fewer rows. `None` when the file no longer
    /// shows the spot.
    /// `cursor` for the cursor and a selection by `v`, kept on text on the
    /// sides `kept_sides` names.
    fn line_in(&mut self, k: usize, place: Place, cursor: bool) -> Option<usize> {
        let sides = self.kept_sides();
        let lines = self.lines_of(k);
        let (spot, nth, text) = match place {
            Place::Past(by) => return Some(lines.len() + by),
            Place::On(spot, nth, text) => (spot, nth, text),
        };
        let spots = spots_of(lines);
        // A fold opened since is its first row now, and a row a fold has
        // closed over since is that fold.
        let spot = match spot {
            Spot::Fold(row) if !spots.contains(&spot) => Spot::Row(row),
            Spot::Row(row) if !spots.contains(&spot) => lines
                .iter()
                .find_map(|l| match l {
                    Line::Fold { first, count } if (*first..first + count).contains(&row) => {
                        Some(Spot::Fold(*first))
                    }
                    _ => None,
                })
                .unwrap_or(spot),
            spot if !spots.contains(&spot) => after_moved(spot, &spots, lines),
            spot => spot,
        };
        let first = spots.iter().position(|s| *s == spot)?;
        let rows = spots[first..].iter().take_while(|s| **s == spot).count();
        let mut at = first + nth.min(rows.saturating_sub(1));
        // A line that held text on a side kept lands on text there, its
        // last piece where that side now takes fewer rows; a side it was
        // blank on (an added line's old side) doesn't move it.
        let lands_blank = |at: usize| {
            [false, true].into_iter().any(|new| {
                let side = usize::from(new);
                sides[side] && text[side] && blank_on(&lines[at], new)
            })
        };
        while cursor && at > first && lands_blank(at) {
            at -= 1;
        }
        Some(at)
    }

    /// Where the cursor, a selection by `v` and the view's first line are,
    /// before the lines wrap anew; none before the change is read.
    pub(super) fn anchors(&mut self) -> Option<Anchors> {
        if self.change.is_none() || self.order.is_empty() {
            return None;
        }
        let (at, view) = (self.at, self.view);
        Some(Anchors {
            cur: self.place_in(at, self.cur),
            visual: self.visual.map(|v| self.place_in(at, v)),
            top: self.place_in(view, self.top),
        })
    }

    /// The cursor, a selection by `v` and the view's first line back on
    /// what they were on, the lines wrapped anew, and the cursor in sight.
    /// The mouse's selection, cells of the screen, is let go.
    pub(super) fn relaid(&mut self, anchors: Option<Anchors>) {
        // The mouse's selection, and a drag still selecting, from cells
        // the new width has moved.
        self.selection = None;
        self.pressed = None;
        self.pressed_cite = None;
        // Nor a click held on a chapter or an "also in" line, which its
        // release would otherwise follow.
        self.open_on_up = None;
        self.also_on_up = None;
        // Every file laid out again at the new widths when it is next read,
        // the scroll bar's counts among them.
        self.lines.clear();
        let Some(a) = anchors else {
            return;
        };
        self.anchor_back(a);
        let at = self.at;
        if let Some(v) = a.visual {
            self.visual = self.line_in(at, v, true).or(self.visual);
        }
        let n = self.lines_of(at).len();
        self.cur = self.cur.min(n.saturating_sub(1));
        self.keep_cursor();
    }

    /// The cursor and the view's first line back on what they were on, at
    /// the widths the lines are laid out at now: after a width change, and
    /// as a mode comes back to the place it kept.
    pub(super) fn anchor_back(&mut self, a: Anchors) {
        let (at, view) = (self.at, self.view);
        if let Some(cur) = self.line_in(at, a.cur, true) {
            self.cur = cur;
        }
        if let Some(top) = self.line_in(view, a.top, false) {
            self.top = top;
        }
    }

    /// The list `width` wide, the diff's lines wrapped anew beside it with
    /// the cursor kept on what it was on.
    pub(super) fn relay_out(&mut self, width: u16) {
        // Nothing wraps anew at the width it already has.
        if width == self.list_width {
            return;
        }
        let anchors = self.anchors();
        let row = self.list_row();
        self.list_width = width;
        self.list_rewrapped(row);
        // The edge moved from under a mouse that isn't dragging it: lit
        // again by the next move over it.
        if self.edge_drag.is_none() {
            self.edge_hover = false;
        }
        self.relaid(anchors);
    }

    /// Where the list is before its lines wrap anew: the row the cursor's
    /// file is on, and the first file at or under the list's top with how
    /// far under it.
    pub(super) fn list_row(&self) -> ListPlace {
        let rows = self.list_rows();
        let file = |r: &ListRow| matches!(r, ListRow::File(_));
        ListPlace {
            cursor: rows.iter().position(|r| *r == ListRow::File(self.at)),
            top: rows
                .iter()
                .enumerate()
                .skip(self.list_top)
                .find(|(_, r)| file(r))
                .map(|(i, r)| (r.clone(), i - self.list_top)),
            from_end: rows.len().saturating_sub(self.list_top),
            in_sight: self.file_in_sight(&rows),
        }
    }

    /// Whether the cursor's file is in the list's sight.
    fn file_in_sight(&self, rows: &[ListRow]) -> bool {
        let h = self.list_height();
        rows.iter()
            .position(|r| *r == ListRow::File(self.at))
            .is_some_and(|r| r >= self.list_top && r < self.list_top + h)
    }

    /// The list kept on the rows it showed as its lines wrapped anew (a
    /// commit's subject over it): at its top while it was there, else the
    /// same first file as far under its top, and the cursor's file not
    /// taken for moved, so the list neither jumps back to it nor scrolls.
    pub(super) fn list_rewrapped(&mut self, before: ListPlace) {
        let rows = self.list_rows();
        self.list_top_rewrapped(&before, &rows);
        let after = rows.iter().position(|r| *r == ListRow::File(self.at));
        // The cursor's file in sight stays in sight: the drawing brings it
        // back as it does after a move. One the wheel scrolled away from
        // stays where the list was, its row not taken for a move.
        if before.in_sight && !self.file_in_sight(&rows) {
            self.list_followed = None;
        } else if before.cursor.is_some() && self.list_followed == before.cursor {
            self.list_followed = after;
        }
    }

    /// The list's top on the rows it showed: at its top while it was there,
    /// else on the same first file, else as far from its end.
    fn list_top_rewrapped(&mut self, before: &ListPlace, rows: &[ListRow]) {
        if self.list_top == 0 {
            return;
        }
        if let Some((row, under)) = &before.top
            && let Some(i) = rows.iter().position(|r| r == row)
        {
            // No file shown above the one that was first: the rows over it
            // may have grown fewer (a chapter's why cut to its first
            // sentence).
            let floor = rows[..i]
                .iter()
                .rposition(|r| matches!(r, ListRow::File(_)))
                .map_or(0, |p| p + 1);
            self.list_top = i.saturating_sub(*under).max(floor);
        } else if before.top.is_none() {
            self.list_top = rows.len().saturating_sub(before.from_end);
        }
    }

    /// The width you gave the list, while diff_list_resize is on.
    pub(super) fn kept_width(&self) -> Option<u16> {
        self.set_width.filter(|_| self.opts.resize)
    }

    /// The list's width before `E` widens it: yours, on this screen, else
    /// `LIST_WIDTH`.
    pub(super) fn narrow_width(&self) -> u16 {
        self.kept_width()
            .map_or(LIST_WIDTH, |w| clamped(i32::from(w), self.cols))
    }

    /// The screen's size, from a resize or from the buffer a drawing was
    /// handed, whichever sees it first: the lines wrapped anew at a new
    /// width with the cursor kept on what it was on, followed at the new
    /// height, and the list's place kept, the cursor's file in sight
    /// staying so. A list at the width you gave it takes it as this screen
    /// lets; one `E` widened fits its longest path on this screen.
    pub(super) fn screen_now(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(20), rows.max(5));
        if cols == self.cols && rows == self.rows {
            return;
        }
        let row = self.list_row();
        if cols == self.cols {
            // A height alone wraps nothing anew.
            self.rows = rows;
            self.list_rewrapped(row);
            return;
        }
        let anchors = self.anchors();
        self.rows = rows;
        self.cols = cols;
        // The edge moves with the screen: lit again by the next move.
        self.edge_hover = false;
        if self.opts.resize && !self.list_widened {
            self.list_width = self.narrow_width();
        } else if self.opts.resize {
            // One E widened fits its longest path on this screen, within
            // half of it, so the next key moves it by its own columns.
            self.list_width = self.fit_width();
        }
        self.list_rewrapped(row);
        self.relaid(anchors);
    }

    /// The list's columns on the screen: the column after them is its edge.
    fn edge(&self) -> u16 {
        self.list_width.min(self.cols.saturating_sub(10))
    }

    /// The list's width kept in `file`, read now: the width it opens at.
    pub fn keep_width_in(&mut self, file: PathBuf) {
        self.set_width = util::read_text(&file)
            .and_then(|text| text.trim().parse::<u16>().ok())
            .filter(|w| *w > 0);
        self.width_file = Some(file);
        self.list_width = self.narrow_width();
    }

    /// The width as it is now, kept: its file gone for the default.
    /// Said when it can't be, so a width that won't last isn't taken for
    /// one that will.
    fn save_width(&mut self) {
        let Some(file) = &self.width_file else {
            return;
        };
        let saved = match self.set_width {
            Some(w) => util::write_atomic(file, format!("{w}\n")),
            None => match std::fs::remove_file(file) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                done => done,
            },
        };
        if let Err(e) = saved {
            let file = crate::paths::tilde(file);
            self.say(format!(
                "Couldn't keep the file list's width in {file}: {e}"
            ));
        }
    }

    /// The list given a width of its own, as near `to` as the screen lets.
    fn set_list_width(&mut self, to: i32) {
        let width = clamped(to, self.cols);
        self.set_width = Some(width);
        self.width_touched = true;
        self.list_widened = false;
        self.relay_out(width);
    }

    /// "Use the file list's default width": the width kept forgotten.
    pub(super) fn default_width(&mut self) {
        self.default_width_here();
        self.save_width();
    }

    /// The default width taken here alone, its file left as it is: a
    /// commit's diff that took it away already.
    pub(super) fn default_width_here(&mut self) {
        self.set_width = None;
        self.width_touched = true;
        // The width a diff opens at with none kept: its longest path's
        // with diff_list_wide on.
        let to = if self.opts.list_wide {
            self.fit_width()
        } else {
            LIST_WIDTH
        };
        self.list_widened = self.opts.list_wide;
        self.relay_out(to);
    }

    /// Whether the menu offers the default width: while this diff has a
    /// width of its own, or the file holds one another diff kept since this
    /// one opened.
    pub(super) fn offers_default_width(&self) -> bool {
        self.opts.resize
            && (self.set_width.is_some() || self.width_file.as_ref().is_some_and(|f| f.exists()))
    }

    /// Whether the list (or the Story view's outline, in its place) and
    /// what is beside it are on the screen, with no box over them: not the
    /// story of a change read again still being told, as the screen is
    /// drawn.
    fn plain(&self) -> bool {
        !(self.mode == Mode::Story && !self.mapped() && self.telling == Telling::Waiting)
            && self.loading.is_none()
            && self.failed.is_none()
            && self.nothing.is_none()
            && self.ask_box.is_none()
            && self.answer.is_none()
            && self.menu.is_none()
            && self.settings.is_none()
            && !(self.cover.is_some() && self.telling == Telling::Waiting)
    }

    /// Whether a cell is on the list's edge, which a drag moves.
    pub(super) fn on_edge(&self, x: u16, y: u16) -> bool {
        self.opts.resize && self.plain() && x == self.edge() && y < self.diff_area().bottom()
    }

    /// A left press on the list's edge: the drag after it sets the width.
    pub(super) fn press_edge(&mut self, x: u16) -> Act {
        self.edge_drag = Some((x, false));
        self.selection = None;
        Act::Redraw
    }

    /// The edge dragged to column `x`: the list as wide as the columns
    /// before it. Nothing is set until the mouse leaves the column the
    /// press landed on, and the edge goes only the way the mouse does:
    /// pulled against the screen's limit it stays, rather than jumping
    /// back to the limit from a list already past it.
    pub(super) fn drag_edge(&mut self, x: u16) -> Act {
        let Some((from, set)) = self.edge_drag else {
            return Act::Same;
        };
        // The list gone from under the drag (a box came up over it):
        // the drag ends, keeping what it had set.
        if !self.plain() {
            return self.release_edge(None);
        }
        let now = i32::from(self.edge());
        let to = i32::from(clamped(i32::from(x), self.cols));
        let the_mouse_s_way = (to - now).signum() == (i32::from(x) - now).signum();
        if (!set && x == from) || to == now || !the_mouse_s_way {
            return Act::Same;
        }
        self.edge_drag = Some((from, true));
        self.set_list_width(i32::from(x));
        Act::Redraw
    }

    /// The edge let go: the width kept, when the drag moved it, whatever
    /// this diff had read before, since another open diff may have kept
    /// one since.
    pub(super) fn release_edge(&mut self, at: Option<(u16, u16)>) -> Act {
        if let Some((_, true)) = self.edge_drag.take() {
            self.save_width();
        }
        // Lit while the mouse is let go over it, else by the next move.
        self.edge_hover = self.opts.hover && at.is_some_and(|(x, y)| self.on_edge(x, y));
        Act::Redraw
    }

    /// The columns a key moves a pane's divider by, when it is bound to.
    /// The key as bound, Shift and all: a Ctrl chord a terminal sent
    /// without its Shift is no resize, here as in the workspace.
    pub(super) fn resize_step(&self, key: &KeyEvent) -> Option<i32> {
        // With diff_list_resize off the pane isn't marked, so any key that
        // reaches the diff is its own.
        if !self.opts.resize {
            return None;
        }
        let pressed = ui_key(key)?.normalized();
        self.resize_keys
            .iter()
            .find(|k| pressed == k.key.normalized())
            .map(|k| k.by)
    }

    /// A pane resize key: the list's width while the list has the keys,
    /// kept at once; else the workspace's own, handed back to it.
    pub(super) fn resize_key(&mut self, by: i32) -> Act {
        // Not while the path filter is typed.
        let lists = self.focus == Focus::List && self.plain() && !self.filtering;
        if !(self.opts.resize && lists) {
            self.hand_back(by);
            return Act::Same;
        }
        // A key half typed (y, g, a count) ends, as any other key ends it.
        self.pending = None;
        self.count = None;
        // Only the way the key asks: where the screen's limits would take
        // the edge the other way (a list already past half a narrow pane),
        // it stays.
        let now = i32::from(self.edge());
        let to = i32::from(clamped(now + by, self.cols));
        if (to - now).signum() != by.signum() {
            return Act::Same;
        }
        self.set_list_width(to);
        self.save_width();
        Act::Redraw
    }

    /// The key's own action asked of the workspace (`press
    /// resize-left-5`), off the screen's thread; nothing outside it.
    fn hand_back(&self, by: i32) {
        if !self.opts.server {
            return;
        }
        let side = if by < 0 { "left" } else { "right" };
        let action = format!("resize-{side}-{}", by.unsigned_abs());
        std::thread::spawn(move || {
            let _ = host().command(&["press", &action]);
        });
    }

    /// The list's edge in the accent while the mouse is over it or drags
    /// it, as every edge that drags lights.
    pub(super) fn draw_edge(&self, buf: &mut Buffer) {
        let lit = self.opts.resize && (self.edge_drag.is_some() || self.edge_hover);
        let x = self.edge();
        if !lit || x >= buf.area.width {
            return;
        }
        let accent = Tone::Accent.style(&self.theme());
        for y in 0..self.diff_area().height.min(buf.area.height) {
            buf[(x, y)].set_symbol("│").set_style(accent);
        }
    }
}
