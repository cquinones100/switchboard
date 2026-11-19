//! UX 2.0 board 8: an item's plan in a panel beside the note, which Enter
//! (or M-e in Notes' own tab) on an unticked item with a plan opens in
//! place of opening the plan note over the note. The list is put away while
//! it shows, the note narrows, and the plan takes the rest: its header
//! "Plan <name>" with its folder at the right, its sections as text, its
//! open choice as the kit's cards side by side (the recommended one's edge
//! in the accent, its mock inside, the option's words under it, a keycap to
//! choose it), then its steps. Ctrl-W and the M-arrows move between the
//! note and the plan as between Notes' other windows, the terminal's cursor
//! going with the keys (in the plan on a line of it, which `j`/`k` move and
//! which lights the card it is on, so the side with the keys shows), a
//! letter in the plan (or a click on a card) chooses that option, `e` opens the plan note in
//! the note's place, Esc closes the panel, and Enter starts the item, as
//! `gw` does. New with UX 2.0; the tmux version opened a plan as any note.
//!
//! The note and the plan are laid out with `kit::columns`, weighted as
//! board 8 draws them (45 and 79, the plan never under 34); the board's
//! Notes pane is 125 columns on its 160-column screen, so what board 45
//! drops below 160 and 140 columns of the screen goes here below 125 and
//! 105 of the app's: the folder in the header, then the mocks inside the
//! cards, then the cards side by side, stacked; under 105 the plan takes
//! the width, and "Show the note beside its plan" shows the note in its
//! place.

use std::path::{Path, PathBuf};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use super::{App, Focus};
use crate::plan::{self, Plan};
use crate::render::{self, Kind};
use crate::util;
use switchboard_ui::Theme;
use switchboard_ui::draw::{put_line, wrap};
use switchboard_ui::input::{Key, KeyCode};
use switchboard_ui::kit::{self, Col, Surface, Tone};

/// The app's width at which the panel shows everything, the board's.
pub const WIDE: u16 = 125;
/// The app's width under which the note and the plan don't fit side by
/// side.
pub const NARROW: u16 = 105;
/// The plan panel's least width.
const PLAN_MIN: usize = 34;
/// The note's least width beside the plan.
const NOTE_MIN: usize = 24;
/// A card's least width side by side: its frame, "recommended", and room
/// for its letter and the start of its words.
const CARD_LEAST: usize = 24;
/// The header's name at most.
const NAME_CAP: usize = 40;

/// The plan shown beside the note.
pub struct Beside {
    /// The plan note's file.
    pub path: PathBuf,
    pub plan: Plan,
    /// The item it is the plan of, its first line.
    pub item: String,
    /// The title the item's `Plan:` link names it by, as written.
    pub link: String,
    /// The first of the plan's rows shown.
    pub scroll: usize,
    /// The plan's row the cursor is on while it has the keys, counted as
    /// `scroll` is, kept while the note has them.
    pub line: usize,
    /// The plan has the keys, else the note.
    pub focused: bool,
    /// The card Tab's focus is on in the plan, if any.
    pub stop: Option<Stop>,
    /// The card or button under the mouse.
    pub hover: Option<Stop>,
    /// On a narrow screen, the note shown in the plan's place.
    pub show_note: bool,
    /// Tab moved to another card, or the cursor moved: the next drawing
    /// scrolls it into view.
    pub follow: bool,
    /// The width and room `line` and `scroll` were counted at, as last
    /// drawn: drawn at another, the plan rewraps, and both move to the rows
    /// of what they were on.
    pub laid: Option<(u16, Room)>,
    /// What the cursor and the first row shown were on as the plan last
    /// rewrapped, and the rows that put them on: while the reader hasn't
    /// moved them since, the next rewrap starts from these, so a drag
    /// through many widths never drifts off the word it started on.
    pub held: Option<Held>,
    /// The plan was given the keys as the note was hidden (Notes narrowed
    /// under the plan), so this drawing keeps the view where it was.
    pub took_keys: bool,
}

/// What the cursor and the first row shown were on as the plan last
/// rewrapped, the rows that put them on, and the card lit then.
pub type Held = (Option<Spot>, Option<Spot>, usize, usize, Option<Stop>);

/// A text row of the panel: the piece of text it shows (a line of the
/// plan, a heading, a blank, counted from the top past the cards) and the
/// letters of that line before the row, spaces left out, the same at any
/// width, unlike its row number.
pub type Piece = (usize, usize);

/// What a row of the panel shows, which the cursor stays on as the plan
/// rewraps: a row of a piece of text, or a row of an option's card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spot {
    Text(Piece),
    Card(char, usize),
}

/// The panel laid out: its rows, where each card is, and, for each row,
/// the piece of text it shows (none on a card's).
pub struct Laid {
    pub rows: Vec<Vec<Span<'static>>>,
    pub hits: Vec<Hit>,
    pub pieces: Vec<Option<Piece>>,
}

impl Laid {
    /// What row `row` shows, a row of cards side by side named by the card
    /// `lit` when it is one of them.
    pub fn spot(&self, row: usize, lit: Option<Stop>) -> Option<Spot> {
        let on: Vec<&Hit> = self
            .hits
            .iter()
            .filter(|h| matches!(h.4, Stop::Card(_)) && h.0 > 0 && (h.0..h.0 + h.3).contains(&row))
            .collect();
        let card = on.iter().find(|h| Some(h.4) == lit).or(on.first());
        if let Some(&&(top, _, _, _, Stop::Card(l))) = card {
            return Some(Spot::Card(l, row - top));
        }
        self.pieces.get(row).copied().flatten().map(Spot::Text)
    }

    /// The row showing `spot`: the same row of its card, or the row of its
    /// piece of text holding the letter it started at; none once it isn't
    /// in the plan.
    pub fn row_of(&self, spot: Spot) -> Option<usize> {
        match spot {
            Spot::Card(l, sub) => self
                .hits
                .iter()
                .find(|h| h.0 > 0 && h.4 == Stop::Card(l))
                .map(|h| h.0 + sub.min(h.3.saturating_sub(1))),
            Spot::Text((n, sub)) => self
                .pieces
                .iter()
                .enumerate()
                .filter_map(|(r, p)| p.filter(|p| p.0 == n && p.1 <= sub).map(|p| (p.1, r)))
                .max()
                .map(|(_, r)| r),
        }
    }
}

/// What a click in the panel presses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// An option's card, by its letter.
    Card(char),
    /// Easy mode's × in the header.
    Close,
}

/// How much of the panel shows, by the app's width.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Room {
    /// Under `NARROW`: the cards stacked.
    Stacked,
    /// The cards side by side without their mocks.
    NoMocks,
    /// Everything but the folder.
    NoFolder,
    All,
}

/// A column's x and width.
pub type Cols = (u16, u16);

/// A card's place in the panel's rows: its first row, x, width and height,
/// and what a click there presses.
pub type Hit = (usize, usize, usize, usize, Stop);

/// Where the note and the plan go across an app `width` wide: each one's x
/// and width, `None` for the one hidden; the rule between them is the
/// column after the note. `own` is the plan's own width (a drag of its
/// edge, the pane resize keys), else it takes board 8's share.
pub fn split(width: u16, show_note: bool, own: Option<u16>) -> (Option<Cols>, Option<Cols>) {
    if width < NARROW {
        let all = Some((0, width));
        return if show_note { (all, None) } else { (None, all) };
    }
    if let Some(own) = own {
        let plan = plan_clamp(own, width);
        let note = width - plan - 1;
        return (Some((0, note)), Some((note + 1, plan)));
    }
    let cols = kit::columns(
        &[
            Col::Stretch {
                weight: 45,
                min: NOTE_MIN,
                max: None,
            },
            Col::Fixed {
                min: 1,
                natural: 1,
                cap: 1,
            },
            Col::Stretch {
                weight: 79,
                min: PLAN_MIN,
                max: None,
            },
        ],
        width as usize,
        0,
        &[],
    );
    let at = |c: Option<(usize, usize)>| c.map(|(x, w)| (x as u16, w as u16));
    (at(cols[0]), at(cols[2]))
}

/// A width asked for the plan, held to what an app `width` wide lets it
/// be: its least, and the note's least left beside it.
fn plan_clamp(asked: u16, width: u16) -> u16 {
    let most = width
        .saturating_sub(NOTE_MIN as u16 + 1)
        .max(PLAN_MIN as u16);
    asked.clamp(PLAN_MIN as u16, most)
}

/// How much of the panel shows at an app `width` wide.
pub fn room(width: u16) -> Room {
    if width < NARROW + 7 {
        Room::Stacked
    } else if width < NARROW + 14 {
        Room::NoMocks
    } else if width < WIDE {
        Room::NoFolder
    } else {
        Room::All
    }
}

/// A plan's line as shown: its Markdown marks for bold and code taken off.
fn plain(line: &str) -> String {
    line.replace("**", "").replace('`', "")
}

/// Text wrapped `cols` wide (at most `kit::WRAP_MAX`), a list item's later
/// rows indented under its words.
fn wrapped(line: &str, cols: usize) -> Vec<String> {
    let cols = cols.clamp(8, kit::WRAP_MAX);
    let line = plain(line);
    let lead = line.len() - line.trim_start().len();
    let marker = line[lead..]
        .find(' ')
        .filter(|&i| {
            let m = &line[lead..lead + i];
            m == "-" || m == "*" || m.trim_end_matches('.').chars().all(|c| c.is_ascii_digit())
        })
        .map_or(0, |i| i + 1);
    let indent = (lead + marker).min(cols / 2);
    let mut out = Vec::new();
    for (i, row) in wrap(line.trim_end(), cols).into_iter().enumerate() {
        if i == 0 {
            out.push(row);
        } else {
            for more in wrap(row.trim_start(), cols - indent) {
                out.push(format!("{}{more}", " ".repeat(indent)));
            }
        }
    }
    out
}

/// The panel's rows, `cols` wide: the header, the sections, the choice's
/// cards and the steps, and where each card is (its first row, x, width
/// and height, and its letter).
pub fn lines(
    beside: &Beside,
    folder: &str,
    cols: usize,
    room: Room,
    easy: bool,
    caps: bool,
    theme: &Theme,
) -> Laid {
    let mut out: Vec<Vec<Span<'static>>> = Vec::new();
    let mut hits = Vec::new();
    // The rows that go on with a line of text wrapped, each with the
    // letters of its line before it (all but spaces, the same at any width,
    // a word split across rows included); every other row starts a piece
    // of its own.
    let mut more: Vec<(usize, usize)> = Vec::new();
    let inner = cols.saturating_sub(2);
    // What Tab lit shows only while the plan has the keys, and comes back
    // with them.
    let stop = beside.stop.filter(|_| beside.focused);

    // The header: "Plan <name>", its folder at the right, easy mode's ×.
    let name = switchboard_ui::draw::cut(&beside.plan.name, NAME_CAP);
    let head = vec![
        Span::raw(" "),
        Span::styled("Plan ", Tone::Dim.style(theme)),
        Span::styled(name, Tone::Accent.bold(theme)),
    ];
    let mut right = Vec::new();
    // The folder only where the name, it and easy mode's × all fit in the
    // plan's own width, so the × is never cut off.
    let head_w = kit::spans_width(&head);
    let folder_fits = head_w + 1 + switchboard_ui::draw::width(folder) + 3 <= cols;
    if room == Room::All && !folder.is_empty() && folder_fits {
        right.push(Span::styled(folder.to_string(), Tone::Dim.style(theme)));
    }
    if easy {
        let lit = beside.hover == Some(Stop::Close) || stop == Some(Stop::Close);
        right.push(Span::raw(" "));
        right.push(Span::styled(
            "×",
            if lit {
                Tone::Accent.bold(theme)
            } else {
                Tone::Dim.style(theme)
            },
        ));
        hits.push((0, cols.saturating_sub(2), 1, 1, Stop::Close));
    }
    let right_w = kit::spans_width(&right);
    let mut head = kit::fit(head, cols.saturating_sub(right_w + 1), Style::new());
    head.extend(right);
    head.push(Span::raw(" "));
    let panel = Surface::Panel.color(theme);
    out.push(
        kit::fit(head, cols, Style::new())
            .into_iter()
            .map(|s| Span::styled(s.content, s.style.bg(panel)))
            .collect(),
    );
    out.push(Vec::new());

    // Only the first open decision is drawn as cards, the one a letter or
    // a click chooses in; one after it waits its turn, drawn as text.
    let mut carded = false;
    let text =
        |out: &mut Vec<Vec<Span<'static>>>, more: &mut Vec<(usize, usize)>, lines: &[String]| {
            for line in lines {
                if line.trim().is_empty() {
                    out.push(Vec::new());
                    continue;
                }
                let mut before = 0;
                for (i, row) in wrapped(line, inner).into_iter().enumerate() {
                    if i > 0 {
                        more.push((out.len(), before));
                    }
                    before += row.chars().filter(|c| !c.is_whitespace()).count();
                    out.push(vec![
                        Span::raw(" "),
                        Span::styled(row, Tone::Text.style(theme)),
                    ]);
                }
            }
        };
    for section in &beside.plan.sections {
        // The words before the first section, but for the Item: line that
        // repeats the item, as text with no label.
        if section.heading.is_empty() {
            let words: Vec<String> = section
                .lines
                .iter()
                .filter(|l| !l.starts_with("Item:"))
                .cloned()
                .collect();
            if words.iter().any(|l| !l.trim().is_empty()) {
                text(&mut out, &mut more, &words);
                out.push(Vec::new());
            }
            continue;
        }
        let tone = if section.choice.is_some() {
            Tone::Needs
        } else {
            Tone::Dim
        };
        out.push(kit::label(&section.heading, tone, None, cols, theme));
        if let Some(choice) = section.choice.as_ref().filter(|_| carded) {
            for o in &choice.options {
                let line = format!(
                    "- {}{}: {}",
                    o.letter,
                    if o.recommended { " (recommended)" } else { "" },
                    o.words
                );
                text(&mut out, &mut more, &[line]);
            }
            out.push(Vec::new());
        } else if let Some(choice) = &section.choice {
            carded = true;
            out.push(Vec::new());
            let n = choice.options.len().max(1);
            // Side by side only where every card gets its least width, the
            // plan's own width narrower than the app's share.
            let fits = n * CARD_LEAST + (n - 1) * 2 <= inner;
            let side_by_side = room > Room::Stacked && n > 1 && fits;
            let mocks = room > Room::NoMocks;
            let places: Vec<(usize, usize)> = if side_by_side {
                let col = Col::Stretch {
                    weight: 1,
                    min: 12,
                    max: None,
                };
                kit::columns(&vec![col; n], inner, 2, &[])
                    .into_iter()
                    .map(|c| c.map_or((0, 0), |(x, w)| (x + 1, w)))
                    .collect()
            } else {
                vec![(1, inner); n]
            };
            let insides: Vec<Vec<Vec<Span<'static>>>> = choice
                .options
                .iter()
                .zip(&places)
                .map(|(o, (_, w))| {
                    let room = w.saturating_sub(4);
                    let mut inside: Vec<Vec<Span<'static>>> = Vec::new();
                    if mocks && !o.mock.is_empty() {
                        for m in &o.mock {
                            inside.push(vec![Span::styled(
                                switchboard_ui::draw::cut(m, room),
                                Tone::Text.style(theme),
                            )]);
                        }
                        inside.push(Vec::new());
                    }
                    for row in wrap(&plain(&o.words), room.max(4)) {
                        inside.push(vec![Span::styled(row, Tone::Dim.style(theme))]);
                    }
                    inside.push(Vec::new());
                    let key = o.letter.to_ascii_lowercase().to_string();
                    let lit = stop == Some(Stop::Card(o.letter))
                        || beside.hover == Some(Stop::Card(o.letter));
                    inside.push(kit::keycap(
                        &key,
                        &format!("Choose {}", o.letter),
                        lit,
                        caps,
                        theme,
                    ));
                    inside
                })
                .collect();
            let cards: Vec<Vec<Vec<Span<'static>>>> = choice
                .options
                .iter()
                .zip(&places)
                .zip(&insides)
                .map(|((o, (_, w)), inside)| {
                    let h = if side_by_side {
                        insides.iter().map(Vec::len).max().unwrap_or(0) + 2
                    } else {
                        inside.len() + 2
                    };
                    let chosen = o.recommended
                        || stop == Some(Stop::Card(o.letter))
                        || beside.hover == Some(Stop::Card(o.letter));
                    let title = vec![Span::styled(
                        format!("{} · {}", o.letter, o.words),
                        Tone::Text.bold(theme),
                    )];
                    let right = if o.recommended {
                        vec![Span::styled("recommended", Tone::Accent.style(theme))]
                    } else {
                        Vec::new()
                    };
                    kit::card(
                        *w,
                        h,
                        title,
                        right,
                        inside,
                        kit::card_edge(chosen, None, theme),
                    )
                })
                .collect();
            if side_by_side {
                let top = out.len();
                let h = cards.iter().map(Vec::len).max().unwrap_or(0);
                for r in 0..h {
                    let mut line = Vec::new();
                    for (card, at) in cards.iter().zip(&places) {
                        if let Some(row) = card.get(r) {
                            kit::place(&mut line, row.clone(), *at);
                        }
                    }
                    out.push(line);
                }
                for (o, (x, w)) in choice.options.iter().zip(&places) {
                    hits.push((top, *x, *w, h, Stop::Card(o.letter)));
                }
            } else {
                for (o, card) in choice.options.iter().zip(cards) {
                    hits.push((out.len(), 1, inner, card.len(), Stop::Card(o.letter)));
                    for row in card {
                        let mut line = vec![Span::raw(" ")];
                        line.extend(row);
                        out.push(line);
                    }
                }
            }
            if !section.lines.is_empty() {
                out.push(Vec::new());
            }
        }
        text(&mut out, &mut more, &section.lines);
        out.push(Vec::new());
    }
    let card_row = |r: usize| {
        hits.iter()
            .any(|h: &Hit| matches!(h.4, Stop::Card(_)) && h.0 > 0 && (h.0..h.0 + h.3).contains(&r))
    };
    let mut pieces = Vec::with_capacity(out.len());
    let mut n = 0;
    for r in 0..out.len() {
        if card_row(r) {
            pieces.push(None);
            continue;
        }
        let sub = match more.iter().find(|m| m.0 == r) {
            Some(&(_, words)) => words,
            None => {
                n += 1;
                0
            }
        };
        pieces.push(Some((n, sub)));
    }
    Laid {
        rows: out,
        hits,
        pieces,
    }
}

/// The cursor moved in the plan: the card it is on lit, as Tab lights one,
/// so Enter chooses it; the one lit kept while the cursor is still on it,
/// as on a row of cards side by side.
fn follow_card(b: &mut Beside, hits: &[Hit]) {
    let row = b.line + 1;
    let on = |h: &&Hit| matches!(h.4, Stop::Card(_)) && h.0 > 0 && (h.0..h.0 + h.3).contains(&row);
    if hits.iter().filter(on).any(|h| Some(h.4) == b.stop) {
        return;
    }
    b.stop = hits.iter().find(on).map(|h| h.4);
}

impl App {
    /// The plan's rows below its header, and where its cards are, at the
    /// width it was last drawn (else the width it will be).
    pub(super) fn plan_layout(&self) -> (usize, Vec<Hit>) {
        let Some(b) = &self.beside else {
            return (0, Vec::new());
        };
        let width = match self.geo.plan.width {
            0 => self
                .plan_split(self.cols, b.show_note)
                .1
                .map_or(0, |(_, w)| w),
            w => w,
        };
        let laid = lines(
            b,
            &self.plan_folder(&b.path),
            width as usize,
            room(self.cols),
            self.opts.easy,
            self.opts.caps,
            &self.look.theme,
        );
        (laid.rows.len().saturating_sub(1), laid.hits)
    }

    /// The plan of the unticked item on the cursor's line, opened beside
    /// the note; whether the line is such an item. Its plan note missing,
    /// says so.
    pub(super) fn open_beside(&mut self) -> bool {
        self.open_beside_at(self.editor.doc.row)
    }

    /// The plan of the unticked item on `row`, opened beside the note, as
    /// `open_beside` opens the cursor's.
    pub(super) fn open_beside_at(&mut self, row: usize) -> bool {
        let lines = self.editor.doc.text.lines();
        let kinds = render::kinds(&lines);
        if self.editor.folds.closed_at(&kinds, row).is_some() {
            return false;
        }
        let Some(title) = render::plan_of(&lines, &kinds, row) else {
            return false;
        };
        let Some(path) = self.store.find_by_title(&title).map(|n| n.path) else {
            self.say(format!("notes: no note {title}"));
            return true;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            self.say(format!("notes: can't read {title}"));
            return true;
        };
        let item = render::item_text(&lines[row], kinds[row]).to_string();
        self.leave_modes();
        self.beside = Some(Beside {
            path,
            plan: Plan::parse(&text),
            item,
            link: title,
            scroll: 0,
            line: 0,
            // Where the plan hides the note, the plan has the keys.
            focused: self.cols < NARROW,
            stop: None,
            hover: None,
            show_note: false,
            follow: false,
            laid: None,
            held: None,
            took_keys: false,
        });
        true
    }

    /// Whether the panel shows the plan of the item on the cursor's line.
    pub(super) fn beside_is_of_cursor(&mut self) -> bool {
        self.beside_is_of(self.editor.doc.row)
    }

    /// Whether the panel shows the plan of the item the cursor is in (its
    /// first line or a line under it): the item's `Plan:` link names the
    /// panel's plan by the title it was opened by, so drawing reads no other
    /// note and an item being retitled keeps it.
    pub(super) fn beside_previews_cursor(&self) -> bool {
        let Some(b) = self.beside.as_ref() else {
            return false;
        };
        let lines = self.editor.doc.text.lines();
        let kinds = render::kinds(&lines);
        // The cursor on a row in a closed fold is on the fold's heading, as
        // the note draws it.
        let row = match self.editor.folds.closed_at(&kinds, self.editor.doc.row) {
            Some((heading, _)) => heading,
            None => self.editor.doc.row,
        };
        render::item_under(&lines, &kinds, row)
            .and_then(|r| render::plan_of(&lines, &kinds, r))
            .is_some_and(|title| title.trim().to_lowercase() == b.link.trim().to_lowercase())
    }

    /// Whether the panel shows the plan of the item on `row`.
    pub(super) fn beside_is_of(&mut self, row: usize) -> bool {
        self.beside.is_some() && self.beside_row() == Some(row)
    }

    /// M-e in Notes' own tab: the plan of the item under the cursor beside
    /// the note, or, while one shows, the panel closed.
    pub(super) fn toggle_beside(&mut self) {
        if self.beside.is_some() {
            return self.close_beside();
        }
        if self.focus == Focus::List && self.list_shown {
            self.focus = Focus::Note;
        }
        if !self.open_beside() {
            self.say("No plan for this line");
        }
    }

    pub(super) fn close_beside(&mut self) {
        self.beside = None;
        self.geo.plan = Rect::default();
        self.geo.plan_edge = None;
        self.geo.plan_hits.clear();
        if self.focus == Focus::List && !self.list_shown {
            self.focus = Focus::Note;
        }
    }

    /// The row of the panel's item in the note: the unticked item that still
    /// links this plan, by its first line, else, its first line edited
    /// since, the one that links it, the panel then following its new first
    /// line; none once no item links it.
    pub(super) fn beside_row(&mut self) -> Option<usize> {
        let b = self.beside.as_ref()?;
        let lines = self.editor.doc.text.lines();
        let kinds = render::kinds(&lines);
        // Only an item that still links this plan, found as the panel found
        // it (by the linked note's title), is its item.
        let notes = self.store.notes(None);
        let links = |r: &usize| {
            matches!(kinds[*r], Kind::Check { done: false, .. })
                && render::plan_of(&lines, &kinds, *r).is_some_and(|t| {
                    let wanted = t.trim().to_lowercase();
                    notes
                        .iter()
                        .find(|n| n.title.to_lowercase() == wanted)
                        .is_some_and(|n| n.path == b.path)
                })
        };
        let by_text = (0..lines.len())
            .filter(links)
            .find(|&r| render::item_text(&lines[r], kinds[r]) == b.item);
        let row = by_text.or_else(|| (0..lines.len()).find(links))?;
        let text = render::item_text(&lines[row], kinds[row]).to_string();
        let link = render::plan_of(&lines, &kinds, row);
        if let Some(b) = self.beside.as_mut() {
            b.item = text;
            // Its link rewritten to name the same plan another way, the
            // panel follows it, as it does the item's first line.
            if let Some(link) = link {
                b.link = link;
            }
        }
        Some(row)
    }

    /// The panel put away once no item of the note links its plan (the
    /// item or its Plan: line deleted).
    pub(super) fn check_beside(&mut self) {
        if self.beside.is_some() && self.beside_row().is_none() {
            self.close_beside();
            self.say("notes: the item no longer links that plan");
        }
    }

    /// The plan read again from its file, as the notes folder changes, so
    /// the panel shows what a choice would act on.
    pub(super) fn reread_beside(&mut self) {
        let Some(b) = self.beside.as_mut() else {
            return;
        };
        if let Ok(text) = std::fs::read_to_string(&b.path) {
            let plan = Plan::parse(&text);
            if plan != b.plan {
                b.plan = plan;
                b.stop = None;
                b.held = None;
            }
        }
    }

    /// The plan's folder within the notes, as its header shows it.
    fn plan_folder(&self, path: &Path) -> String {
        let rel = self.store.relative(path);
        Path::new(&rel)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Option `letter` of the plan chosen: written into the plan note (the
    /// option ticked, the headings renamed), and the item's "(needs a
    /// choice: …)" turned to "(chose <letter>)" in the note, saved.
    pub(super) fn choose(&mut self, letter: char) {
        let Some(b) = &self.beside else {
            return;
        };
        let path = b.path.clone();
        // Neither the note nor its plan is this app's to change while
        // another app has it open (notes_per_tab).
        if let Some(line) = self.held_line() {
            self.say(line);
            return;
        }
        if self.held_elsewhere(&path) {
            return;
        }
        // The item found before anything is written, so a choice never
        // leaves the plan settled and the item still needing it.
        let Some(at) = self.beside_row() else {
            self.say("notes: the item isn't in the note any more");
            return;
        };
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        // The plan changed since it was drawn: shown again, chosen in again.
        let now = Plan::parse(&text);
        if self.beside.as_ref().map(|b| &b.plan) != Some(&now) {
            if let Some(b) = self.beside.as_mut() {
                b.plan = now;
                b.stop = None;
                b.held = None;
            }
            self.say("notes: the plan changed; look again and choose");
            return;
        }
        let Some(chosen) = plan::choose(&text, letter) else {
            self.say(format!("notes: the plan has no open choice {letter}"));
            return;
        };
        if let Err(e) = util::write_atomic(&path, &chosen) {
            self.say(format!("notes: couldn't write the plan: {e}"));
            return;
        }
        let plan = Plan::parse(&chosen);
        // Another decision still open keeps the item needing a choice, as
        // autopilot reads one, options or none.
        let still_open = plan::open_choice(&chosen);
        if let Some(b) = self.beside.as_mut() {
            b.plan = plan;
            b.stop = None;
            b.held = None;
        }
        if still_open {
            self.say(format!(
                "Chose {letter}; the plan has another choice to make"
            ));
            return;
        }
        // The item's line, or one indented under it, says it needs a choice.
        let lines = self.editor.doc.text.lines();
        let kinds = render::kinds(&lines);
        let indent = |l: &str| l.len() - l.trim_start().len();
        let mut changed = None;
        // A sub-item's block, with its own choices, is passed by, as plan_of
        // passes it looking for the item's plan.
        let mut sub: Option<usize> = None;
        for r in at..lines.len() {
            // Blank lines inside the item's block, as plan_of reads it.
            if r > at && lines[r].trim().is_empty() {
                continue;
            }
            if r > at && indent(&lines[r]) <= indent(&lines[at]) {
                break;
            }
            if sub.is_some_and(|i| indent(&lines[r]) > i) {
                continue;
            }
            sub = None;
            if r > at && matches!(kinds[r], Kind::Check { .. }) {
                sub = Some(indent(&lines[r]));
                continue;
            }
            if let Some(new) = plan::item_chose(&lines[r], letter) {
                changed = Some((r, new));
                break;
            }
        }
        if let Some((r, new)) = changed {
            // The item's first line rewritten is the panel's item now.
            if r == at
                && let Some(b) = self.beside.as_mut()
            {
                b.item = render::item_text(&new, kinds[at]).to_string();
            }
            let before = lines.clone();
            // Part of the choice, which writes the plan too, not an edit of
            // the note's to undo on its own: undone, the item would need a
            // choice its plan no longer offers.
            let was = self.editor.doc.text.clone();
            self.editor.tidy(|doc| {
                doc.text.set_lines(r, r + 1, &[new]);
            });
            if self.editor.doc.text != was {
                self.editor
                    .folds
                    .follow(&before, &self.editor.doc.text.lines());
                self.editor.clamp();
                self.changed();
                self.save();
            }
        }
        self.say(format!("Chose {letter}"));
    }

    /// A key while the plan has the focus.
    pub(super) fn plan_key(&mut self, key: Key) {
        let ctrl = key.mods.ctrl && !key.mods.alt;
        if ctrl && key.code == KeyCode::Char('w') {
            self.list_pending = Some('w');
            return;
        }
        // The app's own keys for the list and the finder, as in the note.
        if ctrl && key.code == KeyCode::Char('b') {
            return self.toggle_list();
        }
        if ctrl && key.code == KeyCode::Char('f') {
            return self.finder();
        }
        let (count, hits) = self.plan_layout();
        let last = count.saturating_sub(1);
        let half = (self.geo.plan.height.saturating_sub(1) as usize / 2).max(1);
        let Some(b) = self.beside.as_mut() else {
            return;
        };
        if b.stop != Some(Stop::Close) {
            follow_card(b, &hits);
        }
        let letters: Vec<char> = b
            .plan
            .choice()
            .map(|c| c.options.iter().map(|o| o.letter).collect())
            .unwrap_or_default();
        let mut stops: Vec<Stop> = letters.iter().map(|l| Stop::Card(*l)).collect();
        // Easy mode's × is drawn, so it is a stop too.
        if self.opts.easy {
            stops.push(Stop::Close);
        }
        // Tab from nothing to the first card, past the last to nothing.
        let step = |b: &mut Beside, by: isize| {
            if stops.is_empty() {
                return;
            }
            let n = stops.len() as isize;
            let at = b
                .stop
                .and_then(|s| stops.iter().position(|x| *x == s))
                .map_or(if by > 0 { -1 } else { n }, |i| i as isize);
            let next = at + by;
            b.stop = (0..n).contains(&next).then(|| stops[next as usize]);
        };
        match key.code {
            // An option's letter chooses it, over the panel's own keys.
            KeyCode::Char(c) if !ctrl && letters.contains(&c.to_ascii_uppercase()) => {
                self.choose(c.to_ascii_uppercase());
            }
            KeyCode::Tab | KeyCode::BackTab => {
                step(b, if key.code == KeyCode::Tab { 1 } else { -1 });
                // The cursor goes to the card's top, or, past the cards, to
                // the line above them, so no card lights under it again.
                let cards = || {
                    hits.iter()
                        .filter(|h| matches!(h.4, Stop::Card(_)) && h.0 > 0)
                };
                if let Some((top, ..)) = cards().find(|h| Some(h.4) == b.stop) {
                    b.line = top - 1;
                } else if b.stop.is_none()
                    && let Some(top) = cards().map(|h| h.0).min()
                {
                    b.line = top.saturating_sub(2);
                }
                b.follow = true;
            }
            KeyCode::Char('j' | 'k' | 'g' | 'G') | KeyCode::Down | KeyCode::Up if !ctrl => {
                b.line = match key.code {
                    KeyCode::Char('j') | KeyCode::Down => b.line + 1,
                    KeyCode::Char('k') | KeyCode::Up => b.line.saturating_sub(1),
                    KeyCode::Char('g') => 0,
                    _ => last,
                }
                .min(last);
                b.follow = true;
                follow_card(b, &hits);
            }
            KeyCode::Char('d' | 'u') if ctrl => {
                b.line = if key.code == KeyCode::Char('d') {
                    (b.line + half).min(last)
                } else {
                    b.line.saturating_sub(half)
                };
                b.follow = true;
                follow_card(b, &hits);
            }
            KeyCode::Esc => self.close_beside(),
            KeyCode::Char('e') if !ctrl => {
                let path = b.path.clone();
                self.close_beside();
                self.focus = Focus::Note;
                self.open_note(&path, false, false);
            }
            KeyCode::Enter | KeyCode::Char(' ') => match (b.stop, key.code) {
                (Some(Stop::Card(l)), _) => self.choose(l),
                (Some(Stop::Close), _) => self.close_beside(),
                (None, KeyCode::Enter) => self.start_from_plan(),
                _ => {}
            },
            _ => {}
        }
    }

    /// Enter in the plan: the item started, as `gw` on its line does, the
    /// cursor put on it first.
    fn start_from_plan(&mut self) {
        match self.beside_row() {
            Some(r) => {
                self.editor.doc.row = r;
                self.editor.doc.col = 0;
                self.editor.clamp();
                self.work_on_item();
            }
            None => self.say("notes: the item isn't in the note any more"),
        }
    }

    /// The panel drawn in `area`, a rule at its left when the note is
    /// beside it; where the cursor goes while the plan has the keys.
    pub(super) fn draw_plan(
        &mut self,
        buf: &mut Buffer,
        area: Rect,
        rule: bool,
    ) -> Option<(u16, u16)> {
        let theme = self.look.theme;
        // The rule a drag moves (notes_resize), lit under the mouse and
        // while it is held, as the list's edge is.
        self.geo.plan_edge = (self.opts.resize && rule && area.x > 0)
            .then(|| Rect::new(area.x - 1, area.y, 1, area.height));
        if rule && area.x > 0 {
            let edge = self
                .plan_edge_lit()
                .unwrap_or_else(|| Style::new().fg(kit::border(&theme)));
            for y in area.y..area.bottom() {
                buf[(area.x - 1, y)].set_symbol("│").set_style(edge);
            }
        }
        self.geo.plan = area;
        self.geo.plan_hits.clear();
        let b = self.beside.as_ref()?;
        let folder = self.plan_folder(&b.path);
        let (fits, easy, caps) = (room(self.cols), self.opts.easy, self.opts.caps);
        let draw = |b: &Beside, (width, fits): (u16, Room)| {
            lines(b, &folder, width as usize, fits, easy, caps, &theme)
        };
        let now = (area.width, fits);
        let laid = draw(b, now);
        // Rewrapped (its edge moved, Notes resized): the cursor and the
        // first row shown go to the rows of what they were on, so no card
        // coming up to the cursor's row number is lit by it, and, with the
        // keys, the view follows the cursor rather than the cursor the
        // view; without them it stays where it was scrolled.
        let moved = b.laid.filter(|was| *was != now).map(|was| {
            let spots = match b.held {
                Some((line, scroll, at, top, lit))
                    if (at, top, lit) == (b.line, b.scroll, b.stop) =>
                {
                    (line, scroll)
                }
                _ => {
                    let before = draw(b, was);
                    (
                        before.spot(b.line + 1, b.stop),
                        before.spot(b.scroll + 1, None),
                    )
                }
            };
            let to = |s: Option<Spot>| s.and_then(|s| laid.row_of(s)).map(|r| r.saturating_sub(1));
            (spots, to(spots.0), to(spots.1))
        });
        let b = self.beside.as_mut()?;
        b.laid = Some(now);
        let rewrapped = moved.map(|(spots, line, scroll)| {
            b.line = line.unwrap_or(b.line);
            b.scroll = scroll.unwrap_or(b.scroll);
            spots
        });
        let b = self.beside.as_ref()?;
        let Laid { mut rows, hits, .. } = laid;
        // The header stays; the rest scrolls, never past its end.
        let body = area.height.saturating_sub(1) as usize;
        let last = rows.len().saturating_sub(1).saturating_sub(body);
        let mut scroll = b.scroll;
        let mut line = b.line.min(rows.len().saturating_sub(2));
        // The card Tab just moved to scrolled into view, its top first.
        if b.follow
            && let Some((top, _, _, h, _)) = hits.iter().find(|hit| Some(hit.4) == b.stop)
        {
            let (first, end) = (top.saturating_sub(1), top.saturating_sub(1) + h);
            if first < scroll {
                scroll = first;
            } else if end > scroll + body {
                scroll = end.saturating_sub(body).min(first);
            }
        }
        // The cursor kept in sight as it moves, or as the plan with the
        // keys rewraps; the wheel scrolling past it takes it along, as
        // Vim's does.
        if b.follow || (rewrapped.is_some() && b.focused && !b.took_keys) {
            if line < scroll {
                scroll = line;
            } else if body > 0 && line >= scroll + body {
                scroll = line + 1 - body;
            }
        }
        let scroll = scroll.min(last);
        let focused = b.focused;
        if focused && body > 0 {
            line = line.clamp(scroll, scroll + body - 1);
        }
        let stop = b.stop;
        let b = self.beside.as_mut()?;
        b.scroll = scroll;
        b.line = line;
        b.follow = false;
        b.took_keys = false;
        if let Some((at, top)) = rewrapped {
            b.held = Some((at, top, line, scroll, b.stop));
        }
        // The card lit follows where the cursor ended up (the wheel, or the
        // keys coming back from the note), so Enter acts on what is lit.
        if focused && b.stop != Some(Stop::Close) {
            follow_card(b, &hits);
        }
        if b.stop != stop {
            rows = draw(b, now).rows;
        }
        // On a card, the cursor sits on its edge, the one lit when they
        // stand side by side.
        let x = hits
            .iter()
            .find(|h| h.0 > 0 && Some(h.4) == b.stop && (h.0..h.0 + h.3).contains(&(line + 1)))
            .map_or(1, |h| h.1);
        let cursor = (focused && body > 0 && line >= scroll && line < scroll + body).then(|| {
            (
                area.x + (x as u16).min(area.width.saturating_sub(1)),
                area.y + 1 + (line - scroll) as u16,
            )
        });
        if let Some(head) = rows.first() {
            put_line(buf, area.x, area.y, area.width, &Line::from(head.clone()));
        }
        for (i, row) in rows.iter().skip(1 + scroll).take(body).enumerate() {
            let y = area.y + 1 + i as u16;
            put_line(buf, area.x, y, area.width, &Line::from(row.clone()));
        }
        for (top, x, w, h, stop) in hits {
            let (y, h) = if top == 0 {
                (area.y, 1)
            } else {
                let first = top as isize - 1 - scroll as isize;
                let last = first + h as isize;
                let first = first.max(0);
                let last = last.min(body as isize);
                if last <= first {
                    continue;
                }
                (area.y + 1 + first as u16, (last - first) as u16)
            };
            self.geo
                .plan_hits
                .push((Rect::new(area.x + x as u16, y, w as u16, h), stop));
        }
        cursor
    }

    /// The footer while the panel shows, in place of the item keys.
    pub(super) fn plan_footer(&self) -> Vec<Span<'static>> {
        // No letter keys (board 8): a choice's letters and `e` still work,
        // the cards take a click and Tab, and the plan opens from the list.
        let mut line = vec![Span::raw(" ")];
        let Some(b) = self.beside.as_ref().filter(|b| b.focused) else {
            // In the note, the keys are the note's: the plan's are a C-w away.
            line.extend(kit::hints(&[("C-w l", "plan")], &self.look.theme));
            return line;
        };
        let enter = match b.stop {
            Some(Stop::Card(l)) => format!("choose {l}"),
            Some(Stop::Close) => "close".to_string(),
            None => "start the item".to_string(),
        };
        let keys = [
            ("Enter", enter.as_str()),
            ("Esc", "close"),
            ("C-w h", "note"),
        ];
        line.extend(kit::hints(&keys, &self.look.theme));
        line
    }

    /// A click in the panel: a card chooses its option, the × closes it,
    /// anywhere else gives the plan the keys. Whether it was in the panel.
    pub(super) fn plan_click(&mut self, x: u16, y: u16) -> bool {
        let inside = |r: Rect| x >= r.x && x < r.right() && y >= r.y && y < r.bottom();
        if self.beside.is_none() || !inside(self.geo.plan) {
            return false;
        }
        let hit = self
            .geo
            .plan_hits
            .iter()
            .find(|(r, _)| inside(*r))
            .map(|(_, s)| *s);
        self.leave_modes();
        let top = self.geo.plan.y;
        if let Some(b) = self.beside.as_mut() {
            b.focused = true;
            // The cursor on the row clicked, the header's click leaving it.
            if y > top {
                b.line = b.scroll + (y - top - 1) as usize;
                b.stop = hit.filter(|s| matches!(s, Stop::Card(_)));
            }
        }
        match hit {
            Some(Stop::Card(l)) => self.choose(l),
            Some(Stop::Close) => self.close_beside(),
            None => {}
        }
        true
    }

    /// The mouse moved: the card under it lit. Whether that changed.
    pub(super) fn plan_hover(&mut self, x: u16, y: u16) {
        let inside = |r: Rect| x >= r.x && x < r.right() && y >= r.y && y < r.bottom();
        let hit = self
            .geo
            .plan_hits
            .iter()
            .find(|(r, _)| inside(*r))
            .map(|(_, s)| *s);
        if let Some(b) = self.beside.as_mut() {
            b.hover = hit;
        }
    }

    // ── The plan's width ──────────────────────────────────────────────────

    /// Where the note and the plan go across an app `cols` wide, the plan
    /// at its own width while Settings' notes_resize is on.
    pub(super) fn plan_split(&self, cols: u16, show_note: bool) -> (Option<Cols>, Option<Cols>) {
        split(
            cols,
            show_note,
            self.plan_width.filter(|_| self.opts.resize),
        )
    }

    /// The plan's width as it shows beside its note, 0 with none there.
    pub(super) fn plan_cols(&self) -> u16 {
        self.beside
            .as_ref()
            .and_then(|b| self.plan_split(self.cols, b.show_note).1)
            .map_or(0, |(_, w)| w)
    }

    /// Board 8's share of the width for the plan, its width with none of
    /// its own.
    fn plan_default(&self) -> u16 {
        split(self.cols, false, None).1.map_or(0, |(_, w)| w)
    }

    /// Whether screen cell (`x`, `y`) is on the plan's edge, while it
    /// takes a drag.
    pub(super) fn on_plan_edge(&self, x: u16, y: u16) -> bool {
        self.opts.resize
            && self
                .geo
                .plan_edge
                .is_some_and(|r| x == r.x && y >= r.y && y < r.bottom())
    }

    /// How the plan's edge is drawn while it is dragged, or under the mouse
    /// with Settings' hover_highlight on; none to draw it as the rule it is.
    fn plan_edge_lit(&self) -> Option<Style> {
        let under = self.opts.hover && self.mouse_at.is_some_and(|(x, y)| self.on_plan_edge(x, y));
        ((self.edge_drag && self.edge_plan) || under).then(|| Style::new().fg(Color::Indexed(81)))
    }

    /// The plan given a width of its own, held to what fits; board 8's
    /// share is no width of its own, so reaching it the plan follows the
    /// screen again.
    fn set_plan_width(&mut self, width: u16) {
        let width = plan_clamp(width, self.cols);
        self.plan_width = (width != self.plan_default()).then_some(width);
    }

    /// The plan's edge dragged to column `x`: the plan takes the columns
    /// right of it.
    pub(super) fn drag_plan_edge(&mut self, x: u16) {
        self.set_plan_width(self.cols.saturating_sub(x.saturating_add(1)));
    }

    /// A pane resize key (`by` columns, right positive) while the plan
    /// beside its note has the keys: its edge moved that way, stopping on
    /// board 8's share on the way past. Whether it moved; a plan without
    /// the keys, without the note beside it, or at its limit that way hands
    /// the key back.
    pub(super) fn plan_resize_key(&mut self, by: i32) -> bool {
        let Some(b) = self.beside.as_ref() else {
            return false;
        };
        let beside_note = self.plan_split(self.cols, b.show_note).0.is_some();
        if !b.focused || !beside_note {
            return false;
        }
        // The edge is at the plan's left: moving it left widens the plan.
        let now = i32::from(self.plan_cols());
        let asked = (now - by).clamp(0, i32::from(u16::MAX));
        let share = i32::from(self.plan_default());
        let passes = share != now && now.min(asked) <= share && share <= now.max(asked);
        let to = if passes {
            share
        } else {
            i32::from(plan_clamp(asked as u16, self.cols))
        };
        if (to - now).signum() != (-by).signum() {
            return false;
        }
        self.set_plan_width(to as u16);
        true
    }

    /// "Use the default width" with a plan beside the note, and a double
    /// click on its edge: the plan back to board 8's share, nothing kept.
    pub(super) fn default_plan_width(&mut self) {
        self.plan_width = None;
        if self.keys_plan {
            self.keys_moved = None;
        }
        self.keep_plan_width();
    }

    /// The plan's width written to the state folder, as the list's is.
    pub(super) fn keep_plan_width(&mut self) {
        super::keep_in(&self.store.state.join(super::PLAN_WIDTH), self.plan_width);
    }

    /// The plan's width as the state folder keeps it.
    pub(super) fn kept_plan_width(&self) -> Option<u16> {
        util::read_text(self.store.state.join(super::PLAN_WIDTH))
            .and_then(|text| text.trim().parse().ok())
    }
}
