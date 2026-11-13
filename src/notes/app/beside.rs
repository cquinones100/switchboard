//! UX 2.0 board 8: an item's plan in a panel beside the note, which Enter
//! (or M-e in Notes' own tab) on an unticked item with a plan opens in
//! place of opening the plan note over the note. The list is put away while
//! it shows, the note narrows, and the plan takes the rest: its header
//! "Plan <name>" with its folder at the right, its sections as text, its
//! open choice as the kit's cards side by side (the recommended one's edge
//! in the accent, its mock inside, the option's words under it, a keycap to
//! choose it), then its steps. Ctrl-W and the M-arrows move between the
//! note and the plan as between Notes' other windows, a letter in the plan
//! (or a click on a card) chooses that option, `e` opens the plan note in
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
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::{App, Focus};
use crate::mux::input::{Key, KeyCode};
use crate::notes::plan::{self, Plan};
use crate::notes::render::{self, Kind};
use crate::ui::Theme;
use crate::ui::draw::{put_line, wrap};
use crate::ui::kit::{self, Col, Surface, Tone};
use crate::util;

/// The app's width at which the panel shows everything, the board's.
pub const WIDE: u16 = 125;
/// The app's width under which the note and the plan don't fit side by
/// side.
pub const NARROW: u16 = 105;
/// The plan panel's least width.
const PLAN_MIN: usize = 34;
/// The note's least width beside the plan.
const NOTE_MIN: usize = 24;
/// The header's name at most.
const NAME_CAP: usize = 40;

/// The plan shown beside the note.
pub struct Beside {
    /// The plan note's file.
    pub path: PathBuf,
    pub plan: Plan,
    /// The item it is the plan of, its first line.
    pub item: String,
    /// The first of the plan's rows shown.
    pub scroll: usize,
    /// The plan has the keys, else the note.
    pub focused: bool,
    /// The card Tab's focus is on in the plan, if any.
    pub stop: Option<Stop>,
    /// The card or button under the mouse.
    pub hover: Option<Stop>,
    /// On a narrow screen, the note shown in the plan's place.
    pub show_note: bool,
    /// Tab moved to another card: the next drawing scrolls it into view.
    pub follow: bool,
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
/// column after the note.
pub fn split(width: u16, show_note: bool) -> (Option<Cols>, Option<Cols>) {
    if width < NARROW {
        let all = Some((0, width));
        return if show_note { (all, None) } else { (None, all) };
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
) -> (Vec<Vec<Span<'static>>>, Vec<Hit>) {
    let mut out: Vec<Vec<Span<'static>>> = Vec::new();
    let mut hits = Vec::new();
    let inner = cols.saturating_sub(2);

    // The header: "Plan <name>", its folder at the right, easy mode's ×.
    let name = crate::ui::draw::cut(&beside.plan.name, NAME_CAP);
    let head = vec![
        Span::raw(" "),
        Span::styled("Plan ", Tone::Dim.style(theme)),
        Span::styled(name, Tone::Accent.bold(theme)),
    ];
    let mut right = Vec::new();
    if room == Room::All && !folder.is_empty() {
        right.push(Span::styled(folder.to_string(), Tone::Dim.style(theme)));
    }
    if easy {
        let lit = beside.hover == Some(Stop::Close) || beside.stop == Some(Stop::Close);
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
    let text = |out: &mut Vec<Vec<Span<'static>>>, lines: &[String]| {
        for line in lines {
            if line.trim().is_empty() {
                out.push(Vec::new());
                continue;
            }
            for row in wrapped(line, inner) {
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
                text(&mut out, &words);
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
                text(&mut out, &[line]);
            }
            out.push(Vec::new());
        } else if let Some(choice) = &section.choice {
            carded = true;
            out.push(Vec::new());
            let n = choice.options.len().max(1);
            let side_by_side = room > Room::Stacked && n > 1;
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
                                crate::ui::draw::cut(m, room),
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
                    let lit = beside.stop == Some(Stop::Card(o.letter))
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
                        || beside.stop == Some(Stop::Card(o.letter))
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
        text(&mut out, &section.lines);
        out.push(Vec::new());
    }
    (out, hits)
}

impl App {
    /// The plan of the unticked item on the cursor's line, opened beside
    /// the note; whether the line is such an item. Its plan note missing,
    /// says so.
    pub(super) fn open_beside(&mut self) -> bool {
        let lines = self.editor.doc.text.lines();
        let kinds = render::kinds(&lines);
        let row = self.editor.doc.row;
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
            scroll: 0,
            // Where the plan hides the note, the plan has the keys.
            focused: self.cols < NARROW,
            stop: None,
            hover: None,
            show_note: false,
            follow: false,
        });
        true
    }

    /// Whether the panel shows the plan of the item on the cursor's line.
    pub(super) fn beside_is_of_cursor(&mut self) -> bool {
        self.beside.is_some() && self.beside_row() == Some(self.editor.doc.row)
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
        if let Some(b) = self.beside.as_mut() {
            b.item = text;
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
        let still_open = crate::cmd::worktree_from_note::open_choice(&chosen);
        if let Some(b) = self.beside.as_mut() {
            b.plan = plan;
            b.stop = None;
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
        let Some(b) = self.beside.as_mut() else {
            return;
        };
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
            KeyCode::Tab => {
                step(b, 1);
                b.follow = true;
            }
            KeyCode::BackTab => {
                step(b, -1);
                b.follow = true;
            }
            KeyCode::Char('j') | KeyCode::Down if !ctrl => b.scroll += 1,
            KeyCode::Char('k') | KeyCode::Up if !ctrl => b.scroll = b.scroll.saturating_sub(1),
            KeyCode::Char('d') if ctrl => b.scroll += 10,
            KeyCode::Char('u') if ctrl => b.scroll = b.scroll.saturating_sub(10),
            KeyCode::Char('g') if !ctrl => b.scroll = 0,
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
    /// beside it.
    pub(super) fn draw_plan(&mut self, buf: &mut Buffer, area: Rect, rule: bool) {
        let theme = self.look.theme;
        if rule && area.x > 0 {
            let edge = Style::new().fg(kit::border(&theme));
            for y in area.y..area.bottom() {
                buf[(area.x - 1, y)].set_symbol("│").set_style(edge);
            }
        }
        self.geo.plan = area;
        self.geo.plan_hits.clear();
        let Some(b) = &self.beside else {
            return;
        };
        let folder = self.plan_folder(&b.path);
        let (rows, hits) = lines(
            b,
            &folder,
            area.width as usize,
            room(self.cols),
            self.opts.easy,
            self.opts.caps,
            &theme,
        );
        // The header stays; the rest scrolls, never past its end.
        let body = area.height.saturating_sub(1) as usize;
        let last = rows.len().saturating_sub(1).saturating_sub(body);
        let mut scroll = b.scroll;
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
        let scroll = scroll.min(last);
        if let Some(b) = self.beside.as_mut() {
            b.scroll = scroll;
            b.follow = false;
        }
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
    }

    /// The footer while the panel shows, in place of the item keys.
    pub(super) fn plan_footer(&self) -> Vec<Span<'static>> {
        let mut line = vec![Span::raw(" ")];
        let letters: String = self
            .beside
            .as_ref()
            .and_then(|b| b.plan.choice())
            .map(|c| {
                c.options
                    .iter()
                    .map(|o| o.letter.to_ascii_lowercase().to_string())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_default();
        let mut keys: Vec<(&str, &str)> = vec![("C-w", "note or plan")];
        if !letters.is_empty() {
            keys.push((&letters, "choose"));
        }
        keys.extend([
            ("Enter", "start the item"),
            ("e", "edit the plan"),
            ("Esc", "close"),
        ]);
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
        if let Some(b) = self.beside.as_mut() {
            b.focused = true;
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
}
