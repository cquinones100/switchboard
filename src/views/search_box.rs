//! M-F's box (UX 2.0 board 78): the query, a chip per source turned on in
//! Settings, and the results under them, each source's under its label as
//! it answers. New with the source chips; search.sh asked the query in one
//! fzf and listed the results in a second.
//!
//! A chip is a click and a Tab stop that turns its source on or off for
//! this search, All turning every one back on; the chips the person set
//! are kept for the next M-F (`search::write_off`). A source's prefix typed
//! at the start of the query (`note:`, `slack:`, `search::scope`) leaves
//! the query and lights those sources alone, so the chips and the prefixes
//! always agree, without being kept.
//!
//! Enter asks the lit sources for what is typed (`ViewsEvent::Ask`, which
//! the server answers with `ViewsEvent::Found`, given to `answered`), and,
//! once the results are of what is typed and its sources, takes the chosen
//! row as the second list took it: read beside the pane, or Ctrl-O for the
//! browser or a tab's match. A chip turned on after a search asks that
//! source alone; one turned off drops its rows and any answer still on its
//! way, so a source off is sent nothing. The box shows no keys: board 78
//! keeps its room for the results.
//!
//! With Settings' `search_preview` on (board 91) the box is wider and the
//! chosen result is previewed beside the results, or under them in a box
//! narrower than `SIDE_MIN`: its place, what it is, its line number when
//! it has one, and the lines around the match its source gave
//! (`search::Context`), the match's line lit and the words searched for in
//! the accent. Each line is wrapped at the preview's width under its own
//! indentation rather than cut, and the match's whole line is shown first,
//! with as many rows before and after it as fit, so a long message is read
//! whole (each source gives its lines unwrapped). It follows the cursor
//! from what the rows hold, reading nothing. With the results focused, J
//! and K scroll it a row at a time, as the wheel over it does; beside the results, the pane resize keys and a drag of the rule
//! between the two move that rule.

use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;
use tokio::sync::mpsc::UnboundedSender;
use unicode_segmentation::UnicodeSegmentation;

use super::ViewsEvent;
use super::search::{self, Found, Source};
use crate::mux::PaneId;
use crate::mux::input::{Key, KeyCode, Mods, Mouse, MouseButton, MouseKind};
use crate::mux::server::{ClientId, Event};
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use crate::ui::field::{Edit, Field};
use crate::ui::kit::{self, Col, Tone};
use crate::ui::menu::wrap_step;
use crate::ui::{Look, Outcome, Overlay, Picked, Step};

/// Two clicks on a row this close together take it.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// Each search's number, so an answer to an earlier one is let go.
static SEARCHES: AtomicU64 = AtomicU64::new(0);

/// The lines above the results: the field, a blank, the chips, a rule.
const HEAD: usize = 4;
const FIELD: usize = 0;
const CHIPS: usize = 2;
/// The inside's least width: every chip with a two-digit count, and room
/// to spare.
const CHIPS_WIDTH: usize = 72;
/// The inside's least width for the preview beside the results; under it,
/// the preview goes under them.
const SIDE_MIN: usize = 100;
/// The results' and the preview's least widths beside each other.
const RESULTS_MIN: usize = 30;
const PREVIEW_MIN: usize = 40;
/// The preview's lines above the context: its place, what it is, a rule.
const PREVIEW_HEAD: usize = 3;
/// The most lines the preview takes under the results: its head and the
/// context's eleven.
const PREVIEW_UNDER: usize = PREVIEW_HEAD + 11;

/// Where the preview is, as drawn last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Preview {
    Off,
    /// Beside the results, which are this wide; the rule between is the
    /// next column.
    Beside(usize),
    /// Under the results, from this line of the box, this many lines.
    Under(usize, usize),
}

/// What has the focus: Tab's stops, and the results, which Up and Down
/// reach below the chips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Field,
    All,
    Chip(Source),
    Close,
    Results,
}

fn is_chip(part: Part) -> bool {
    matches!(part, Part::All | Part::Chip(_))
}

/// One line of the results: a source's label, a row, or a source still
/// looking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Line {
    Label(Source),
    Row(usize),
    Looking(Source),
    /// Every source answered, with nothing.
    Nothing,
    Gap,
}

/// The search asked last: its number, its query and the sources its
/// results are of (the chips pressed since included).
#[derive(Clone, Debug)]
struct Asked {
    id: u64,
    query: String,
    sources: Vec<Source>,
}

pub struct SearchBox {
    client: ClientId,
    tx: UnboundedSender<Event>,
    /// The pane the search started from, whose tab the Tabs source leaves
    /// out.
    pane: Option<PaneId>,
    caps: bool,
    /// Settings' `search_arrows`: Up and Down step through the field, the
    /// chips and the results, Left and Right along the chips; off, Up and
    /// Down move through the results alone.
    arrows: bool,
    /// The sources Settings has on, a chip each.
    sources: Vec<Source>,
    /// The ones lit.
    lit: Vec<Source>,
    /// Whether the chips lit are a typed prefix's, until a chip is
    /// pressed.
    prefixed: bool,
    /// What is typed (a `ui::field`), a source's prefix taken out.
    query: Field,
    /// The columns the field was last drawn in, and where on its line
    /// they start, for a click on it.
    field_at: (usize, usize),
    asked: Option<Asked>,
    rows: Vec<Found>,
    waiting: Vec<Source>,
    /// The sources that answered the search asked last.
    answered: Vec<Source>,
    cursor: usize,
    top: usize,
    room: usize,
    focus: Part,
    /// The chip Down from the field lands on: the one focused last.
    last_chip: Part,
    hover: Option<Part>,
    hover_row: Option<usize>,
    /// Each chip's columns on its line, from the inside's left edge.
    chip_at: Vec<(Part, u16, u16)>,
    /// The result line each shown line is, by its index in the box.
    shown: Vec<Line>,
    drawn: Option<Drawn>,
    /// The scroll bar's thumb being dragged, from where (`kit::scroll_grab`).
    dragging: Option<kit::Grab>,
    /// The last click on a row, for a second one.
    last_click: Option<(Instant, usize)>,
    /// Settings' `search_preview`.
    preview: bool,
    /// Where the preview was drawn last.
    previewed: Preview,
    /// The result the preview's scroll is of (its search's number and its
    /// link), and its first context line shown; another result, or the
    /// same link found by another search, starts at its match again.
    peek: Option<(u64, String, usize)>,
    /// The results' width beside the preview, once a drag or a resize key
    /// moved the rule between them.
    split: Option<usize>,
    /// The rule between them being dragged.
    splitting: bool,
    /// The keys bound to moving a pane's divider left or right, and by how
    /// many columns (right positive).
    resize_keys: Vec<(Key, isize)>,
}

impl SearchBox {
    /// The box for `client`, with the sources Settings has on, those
    /// `off` (the set kept from the last M-F) unlit, and `typed` in the
    /// field (a prefix the palette's per-source commands type).
    pub fn new(
        client: ClientId,
        tx: UnboundedSender<Event>,
        pane: Option<PaneId>,
        sources: Vec<Source>,
        off: &[Source],
        typed: &str,
        caps: bool,
    ) -> SearchBox {
        let mut lit: Vec<Source> = sources
            .iter()
            .copied()
            .filter(|s| !off.contains(s))
            .collect();
        if lit.is_empty() {
            lit = sources.clone();
        }
        let first_chip = sources.first().map_or(Part::All, |s| Part::Chip(*s));
        let mut b = SearchBox {
            client,
            tx,
            pane,
            caps,
            sources,
            lit,
            prefixed: false,
            query: Field::new(),
            field_at: (0, 0),
            asked: None,
            rows: Vec::new(),
            waiting: Vec::new(),
            answered: Vec::new(),
            cursor: 0,
            top: 0,
            room: 10,
            focus: Part::Field,
            last_chip: first_chip,
            hover: None,
            hover_row: None,
            chip_at: Vec::new(),
            shown: Vec::new(),
            drawn: None,
            dragging: None,
            last_click: None,
            arrows: true,
            preview: false,
            previewed: Preview::Off,
            peek: None,
            split: None,
            splitting: false,
            resize_keys: Vec::new(),
        };
        b.type_in(typed);
        b
    }

    /// With Settings' `search_arrows` as `on`.
    pub fn with_arrows(mut self, on: bool) -> SearchBox {
        self.arrows = on;
        self
    }

    /// With Settings' `search_preview` as `on`, and the pane resize keys
    /// (each with its columns, right positive) moving the rule between the
    /// results and the preview.
    pub fn with_preview(mut self, on: bool, resize_keys: Vec<(Key, isize)>) -> SearchBox {
        self.preview = on;
        self.resize_keys = resize_keys;
        self
    }

    /// The sources lit, for the tests.
    pub fn lit(&self) -> &[Source] {
        &self.lit
    }

    /// What is typed, for the tests.
    pub fn typed(&self) -> &str {
        self.query.text()
    }

    /// A character typed into the field, at its cursor.
    fn type_key(&mut self, key: Key) {
        // The space after a prefix taken out isn't kept either.
        if !(self.query.is_empty() && key.code == KeyCode::Char(' ')) {
            self.query.key(key);
        }
        self.scope_typed();
    }

    /// Text pasted into the field, at its cursor.
    fn type_in(&mut self, text: &str) {
        let text = if self.query.is_empty() {
            text.trim_start()
        } else {
            text
        };
        self.query.paste(text);
        self.scope_typed();
    }

    /// A source's prefix at the field's start leaves it and lights the
    /// sources it names alone.
    fn scope_typed(&mut self) {
        let (named, rest) = search::scope(self.query.text());
        if named != Source::ALL {
            // A prefix typed after another adds its sources to the other's.
            let lit: Vec<Source> = self
                .sources
                .iter()
                .copied()
                .filter(|s| named.contains(s) || (self.prefixed && self.lit.contains(s)))
                .collect();
            // A prefix naming only sources Settings has off leaves the
            // chips as they are.
            if !lit.is_empty() {
                self.lit = lit;
            }
            // What follows the prefixes, from where it starts in the text
            // (scope gives it trimmed), the cursor moved back by as much.
            let text = self.query.text();
            let cut = (rest.as_ptr() as usize)
                .saturating_sub(text.as_ptr() as usize)
                .min(text.len());
            let cursor = self.query.cursor().saturating_sub(cut);
            let rest = text[cut..].to_string();
            self.query.take_in(&rest, cursor);
            self.prefixed = true;
        }
        // Prefixes add up only while nothing but the start of another
        // source's word follows them.
        if !self.query.is_empty() && !search::starts_a_source(self.query.text()) {
            self.prefixed = false;
        }
    }

    fn send(&self, sources: Vec<Source>) {
        let Some(asked) = &self.asked else {
            return;
        };
        let _ = self.tx.send(Event::Views(ViewsEvent::Ask {
            client: self.client,
            search: asked.id,
            query: asked.query.clone(),
            sources,
            pane: self.pane,
        }));
    }

    /// The chips the person set, kept for the next M-F.
    fn keep(&self) {
        let off = self
            .sources
            .iter()
            .copied()
            .filter(|s| !self.lit.contains(s))
            .collect();
        let _ = self.tx.send(Event::Views(ViewsEvent::Chips { off }));
    }

    /// Enter in the field: asks the lit sources for what is typed, unless
    /// the results are of it already, when it takes the chosen row.
    fn enter(&mut self, browser: bool) -> Step {
        let query = self.query.text().trim().to_string();
        // A prefix typed since lights other sources: those are asked, and
        // the rows of the ones lit before are never taken.
        let (same_query, same_sources) = self.asked.as_ref().map_or((false, false), |a| {
            (a.query == query, a.sources == self.lit)
        });
        if query.is_empty() {
            return if same_sources {
                self.take(browser)
            } else {
                Step::Same
            };
        }
        if same_query && same_sources {
            return self.take(browser);
        }
        self.ask(query)
    }

    fn ask(&mut self, query: String) -> Step {
        if self.lit.is_empty() {
            return Step::Same;
        }
        self.prefixed = false;
        let id = SEARCHES.fetch_add(1, Ordering::Relaxed) + 1;
        self.asked = Some(Asked {
            id,
            query,
            sources: self.lit.clone(),
        });
        self.rows.clear();
        self.answered.clear();
        self.waiting = self.lit.clone();
        self.cursor = 0;
        self.top = 0;
        self.send(self.lit.clone());
        Step::Redraw
    }

    /// The chosen row taken (Enter, Ctrl-O, a second click); a row that
    /// only says something, or one of results whose sources a prefix
    /// changed since, takes nothing.
    fn take(&self, browser: bool) -> Step {
        let current = self.asked.as_ref().is_some_and(|a| a.sources == self.lit);
        let Some(row) = self.rows.get(self.cursor).filter(|_| current) else {
            return Step::Same;
        };
        if row.link.is_empty() {
            return Step::Same;
        }
        let picked = Picked {
            index: self.cursor,
            value: row.link.clone(),
            query: self
                .asked
                .as_ref()
                .map(|a| a.query.clone())
                .unwrap_or_default(),
        };
        Step::Close(if browser {
            Outcome::PickedAlso(picked)
        } else {
            Outcome::Picked(picked)
        })
    }

    /// A chip pressed: All lights every source; a source's turns it on or
    /// off, the last one lit staying on. Kept for the next M-F.
    fn press(&mut self, part: Part) -> Step {
        let before = self.lit.clone();
        match part {
            Part::All => self.lit = self.sources.clone(),
            Part::Chip(s) if self.lit.contains(&s) => {
                if self.lit.len() > 1 {
                    self.lit.retain(|l| *l != s);
                }
            }
            Part::Chip(s) => {
                self.lit.push(s);
                self.lit.sort();
            }
            Part::Close => return Step::Close(Outcome::Dismissed),
            Part::Field | Part::Results => return Step::Same,
        }
        self.prefixed = false;
        self.keep();
        if self.lit == before && self.asked.as_ref().is_none_or(|a| a.sources == self.lit) {
            return Step::Redraw;
        }
        // Off: its rows and any answer on its way let go.
        let chosen = self.rows.get(self.cursor).map(|r| r.link.clone());
        self.rows.retain(|r| self.lit.contains(&r.source));
        self.waiting.retain(|s| self.lit.contains(s));
        self.answered.retain(|s| self.lit.contains(s));
        self.cursor = chosen
            .and_then(|link| self.rows.iter().position(|r| r.link == link))
            .unwrap_or(0);
        // On, after a search: asked alone, every source lit that the search
        // didn't ask (one a prefix lit since included).
        let asked_before = self
            .asked
            .as_ref()
            .map(|a| a.sources.clone())
            .unwrap_or_default();
        let new: Vec<Source> = self
            .lit
            .iter()
            .copied()
            .filter(|s| !asked_before.contains(s))
            .collect();
        if let Some(asked) = &mut self.asked {
            asked.sources = self.lit.clone();
        }
        if self.asked.is_some() && !new.is_empty() {
            self.waiting.extend(new.iter().copied());
            self.waiting.sort();
            self.send(new);
        }
        Step::Redraw
    }

    /// A source's answer to search `id`: taken while that search is the
    /// one shown and its source is lit; whether anything changed.
    pub fn answered(&mut self, id: u64, source: Source, rows: Vec<Found>) -> bool {
        if self.asked.as_ref().is_none_or(|a| a.id != id) || !self.waiting.contains(&source) {
            return false;
        }
        let chosen = self.rows.get(self.cursor).map(|r| r.link.clone());
        search::merge(&mut self.rows, source, rows);
        self.waiting.retain(|s| *s != source);
        self.answered.push(source);
        // The cursor stays on its row as a slower source's arrive above it.
        if let Some(at) = chosen.and_then(|link| self.rows.iter().position(|r| r.link == link)) {
            self.cursor = at;
        }
        true
    }

    fn stops(&self, look: &Look) -> Vec<Part> {
        let mut stops = vec![Part::Field, Part::All];
        stops.extend(self.sources.iter().map(|s| Part::Chip(*s)));
        if look.close {
            stops.push(Part::Close);
        }
        stops
    }

    fn step_focus(&mut self, by: isize, look: &Look) -> Step {
        let stops = self.stops(look);
        self.focus_on(crate::ui::form::step(&stops, self.focus, by));
        Step::Redraw
    }

    fn focus_on(&mut self, part: Part) {
        if is_chip(part) {
            self.last_chip = part;
        }
        self.focus = part;
    }

    /// Left and Right along the chips, wrapping at either end.
    fn step_chip(&mut self, by: isize) -> Step {
        let mut chips = vec![Part::All];
        chips.extend(self.sources.iter().map(|s| Part::Chip(*s)));
        self.focus_on(crate::ui::form::step(&chips, self.focus, by));
        Step::Redraw
    }

    /// Down: from the field (or the close button) to the chip focused
    /// last, from the chips to the first result, then through the results.
    fn down(&mut self) -> Step {
        match self.focus {
            Part::Field | Part::Close => {
                self.focus_on(self.last_chip);
                Step::Redraw
            }
            Part::All | Part::Chip(_) if self.rows.is_empty() => Step::Same,
            Part::All | Part::Chip(_) => self.move_to(0),
            Part::Results => self.step(true),
        }
    }

    /// Up: through the results, from the first to the chips, from the
    /// chips back to the field.
    fn up(&mut self) -> Step {
        match self.focus {
            Part::Field | Part::Close => Step::Same,
            Part::All | Part::Chip(_) => {
                self.focus = Part::Field;
                Step::Redraw
            }
            Part::Results if self.cursor == 0 => {
                self.focus_on(self.last_chip);
                Step::Redraw
            }
            Part::Results => self.move_to(self.cursor - 1),
        }
    }

    /// One result down or up, the last's next the first and the first's
    /// previous the last (`menu::wrap_step`).
    fn step(&mut self, down: bool) -> Step {
        self.move_to(wrap_step(self.cursor, self.rows.len(), down))
    }

    /// Moves through the results, the focus on them, so Enter takes the
    /// row moved to rather than pressing a chip.
    fn move_to(&mut self, to: usize) -> Step {
        let refocused = self.focus != Part::Results;
        self.focus = Part::Results;
        let to = to.min(self.rows.len().saturating_sub(1));
        if to == self.cursor {
            return if refocused { Step::Redraw } else { Step::Same };
        }
        self.cursor = to;
        Step::Redraw
    }

    /// How many rows each lit source has.
    fn count(&self, source: Source) -> usize {
        self.rows.iter().filter(|r| r.source == source).count()
    }

    /// What the field's prompt says is searched: every source, or those
    /// lit by name.
    fn prompt(&self) -> String {
        if self.lit == self.sources {
            return "Search: ".to_string();
        }
        let names: Vec<&str> = self.lit.iter().map(|s| s.scoped_name()).collect();
        format!("Search {}: ", names.join(", "))
    }

    /// The results' lines: each lit source that found something or is
    /// looking, its label with its count, its rows, or that it is still
    /// looking; that nothing was found once every one answered.
    fn result_lines(&self) -> Vec<Line> {
        let mut out = Vec::new();
        if self.asked.is_none() {
            return out;
        }
        for source in &self.lit {
            let looking = self.waiting.contains(source);
            if !looking && self.count(*source) == 0 {
                continue;
            }
            if !out.is_empty() {
                out.push(Line::Gap);
            }
            out.push(Line::Label(*source));
            if looking {
                out.push(Line::Looking(*source));
            }
            for (i, r) in self.rows.iter().enumerate() {
                if r.source == *source {
                    out.push(Line::Row(i));
                }
            }
        }
        if out.is_empty() && self.waiting.is_empty() {
            out.push(Line::Nothing);
        }
        out
    }

    fn chip_text(&self, source: Source) -> String {
        let name = source.scoped_name();
        let mut name = name[..1].to_uppercase() + &name[1..];
        if self.asked.is_some() && self.lit.contains(&source) {
            if self.waiting.contains(&source) {
                name.push_str(" ⋯");
            } else if self.answered.contains(&source) {
                name.push_str(&format!(" {}", self.count(source)));
            }
        }
        name
    }

    fn lit_part(&self, part: Part) -> bool {
        self.focus == part || self.hover == Some(part)
    }

    fn chips_line(&mut self, look: &Look) -> Vec<Span<'static>> {
        let theme = &look.theme;
        let mut spans = vec![Span::raw(" ")];
        let mut x = 1u16;
        self.chip_at.clear();
        let all_on = self.lit == self.sources;
        let mut chips = vec![(Part::All, "All".to_string(), all_on)];
        for s in &self.sources {
            chips.push((Part::Chip(*s), self.chip_text(*s), self.lit.contains(s)));
        }
        for (part, text, on) in chips {
            let chip = kit::switch_chip(&text, on, self.lit_part(part), self.caps, theme);
            let w = kit::spans_width(&chip) as u16;
            // The column after the chip isn't part of it.
            self.chip_at.push((part, x, x + w - 1));
            x += w;
            spans.extend(chip);
        }
        spans
    }

    fn result_line(&self, line: Line, inner: usize, look: &Look) -> BoxLine {
        // `inner`: the results' width, the whole inside's or, beside the
        // preview, theirs.
        let theme = &look.theme;
        match line {
            Line::Gap => BoxLine::blank(),
            Line::Label(s) => {
                let count = self.answered.contains(&s).then(|| self.count(s));
                BoxLine::spans(kit::label(s.scoped_name(), Tone::Dim, count, inner, theme))
            }
            Line::Nothing => BoxLine::spans(vec![Span::styled(
                " Nothing found.",
                Tone::Faint.style(theme),
            )]),
            Line::Looking(_) => BoxLine::spans(vec![Span::styled(
                "   ◌ searching…",
                Tone::Faint.style(theme),
            )]),
            Line::Row(i) => {
                let row = &self.rows[i];
                let chosen = i == self.cursor;
                let text_style = if row.link.is_empty() {
                    Tone::Faint.style(theme)
                } else if chosen {
                    Tone::Text.bold(theme)
                } else {
                    Tone::Text.style(theme)
                };
                // The text stretching, what it is about at the right edge,
                // dropped first when the box is narrow; two columns in, past
                // the chosen row's bar.
                let at = kit::columns(
                    &[
                        Col::Stretch {
                            weight: 1,
                            min: 12,
                            max: None,
                        },
                        Col::Right {
                            natural: draw::width(&row.detail),
                            cap: (inner / 3).max(8),
                        },
                    ],
                    inner.saturating_sub(4),
                    2,
                    &[1],
                );
                let mut spans = Vec::new();
                if let Some((x, w)) = at[0] {
                    kit::place(
                        &mut spans,
                        vec![Span::styled(row.text.clone(), text_style)],
                        (x + 2, w),
                    );
                }
                if let Some((x, w)) = at[1].filter(|_| !row.detail.is_empty()) {
                    kit::place(
                        &mut spans,
                        vec![Span::styled(row.detail.clone(), Tone::Dim.style(theme))],
                        (x + 2, w),
                    );
                }
                let hovered = self.hover_row == Some(i);
                let line = kit::row(spans, inner, chosen, theme);
                let line = BoxLine::spans(line);
                let line = if hovered && !chosen {
                    line.fill(Some(look.theme.panel))
                } else {
                    line
                };
                if row.link.is_empty() {
                    line
                } else {
                    line.clickable()
                }
            }
        }
    }

    fn part_at(&self, hit: Hit) -> Option<Part> {
        match hit {
            Hit::Close => Some(Part::Close),
            Hit::Line(FIELD, _) => Some(Part::Field),
            Hit::Line(CHIPS, col) => self
                .chip_at
                .iter()
                .find(|(_, from, to)| (*from..*to).contains(&col))
                .map(|(p, _, _)| *p),
            _ => None,
        }
    }

    fn row_at(&self, hit: Hit) -> Option<usize> {
        match hit {
            Hit::Line(_, col) if matches!(self.previewed, Preview::Beside(w) if col as usize >= w) => {
                None
            }
            Hit::Line(i, _) if i >= HEAD => match self.shown.get(i - HEAD) {
                Some(Line::Row(r)) if !self.rows[*r].link.is_empty() => Some(*r),
                _ => None,
            },
            _ => None,
        }
    }

    /// Whether `hit` is on the preview.
    fn on_preview(&self, hit: Hit) -> bool {
        match (self.previewed, hit) {
            (Preview::Beside(w), Hit::Line(i, col)) => i >= HEAD && col as usize > w,
            (Preview::Under(from, n), Hit::Line(i, _)) => (from..from + n).contains(&i),
            _ => false,
        }
    }

    /// The words searched for, which the preview marks.
    /// The tabs and notes are searched for the words as typed, the services
    /// by their own syntax (`search::terms`).
    fn words(&self) -> Vec<String> {
        let Some(a) = &self.asked else {
            return Vec::new();
        };
        let query = search::scope(&a.query).1;
        match self.rows.get(self.cursor).map(|r| r.source) {
            Some(Source::Tabs | Source::Notes) => query
                .to_lowercase()
                .split_whitespace()
                .map(str::to_string)
                .collect(),
            _ => search::terms(query),
        }
    }

    /// The chosen row's context wrapped to `cols` columns.
    fn wrapped(&self, cols: usize) -> Wrapped {
        let Some(row) = self.rows.get(self.cursor) else {
            return Wrapped::default();
        };
        Wrapped::of(&row.context, cols)
    }

    /// The first of the wrapped context's rows the preview shows in
    /// `room` rows `cols` wide: where its scroll left them, else the
    /// match's whole line with as many rows before and after it as fit,
    /// its start in sight when it is taller than the preview.
    fn peek_top(&self, room: usize, cols: usize) -> usize {
        let Some(row) = self.rows.get(self.cursor) else {
            return 0;
        };
        let wrapped = self.wrapped(cols);
        let most = wrapped.rows.len().saturating_sub(room);
        if let Some((id, link, top)) = &self.peek
            && self.peek_of() == Some((*id, link.as_str()))
        {
            return (*top).min(most);
        }
        let at = row.context.at;
        let first = wrapped.rows.iter().position(|r| r.line == at).unwrap_or(0);
        let tall = wrapped.rows.iter().filter(|r| r.line == at).count();
        first
            .saturating_sub(room.saturating_sub(tall) / 2)
            .min(most)
            .min(first)
    }

    /// The preview's context: how many columns wide and rows tall, as last
    /// drawn.
    fn peek_size(&self) -> Option<(usize, usize)> {
        let inner = self.drawn.as_ref().map_or(0, |d| d.inner.width as usize);
        let (cols, rows) = match self.previewed {
            Preview::Off => return None,
            Preview::Beside(w) => (inner.saturating_sub(w + 1), self.room),
            Preview::Under(_, n) => (inner, n),
        };
        Some((cols, rows.saturating_sub(PREVIEW_HEAD).max(1)))
    }

    /// The chosen result as the preview's scroll is kept for it.
    fn peek_of(&self) -> Option<(u64, &str)> {
        Some((
            self.asked.as_ref()?.id,
            self.rows.get(self.cursor)?.link.as_str(),
        ))
    }

    /// Scrolls the preview by `by` rows.
    fn scroll_peek(&mut self, by: isize) -> Step {
        let Some((cols, room)) = self.peek_size() else {
            return Step::Same;
        };
        let top = self.peek_top(room, cols);
        let most = self.wrapped(cols).rows.len().saturating_sub(room);
        let to = top.saturating_add_signed(by).min(most);
        if to == top {
            return Step::Same;
        }
        self.peek = self.peek_of().map(|(id, link)| (id, link.to_string(), to));
        Step::Redraw
    }

    /// Moves the rule between the results and the preview to give the
    /// results `to` columns, within both's least widths.
    fn split_to(&mut self, to: isize) -> Step {
        let Preview::Beside(now) = self.previewed else {
            return Step::Same;
        };
        let inner = self.drawn.as_ref().map_or(0, |d| d.inner.width as usize);
        let to = (to.max(0) as usize).clamp(
            RESULTS_MIN,
            inner.saturating_sub(PREVIEW_MIN + 1).max(RESULTS_MIN),
        );
        if to == now {
            return Step::Same;
        }
        self.split = Some(to);
        Step::Redraw
    }

    /// The preview of the chosen row, `cols` wide and `rows` tall: its
    /// place and line number, what it is, a rule, and the lines around the
    /// match, numbered when they have numbers, the match's line lit.
    fn preview_lines(&self, cols: usize, rows: usize, look: &Look) -> Vec<Vec<Span<'static>>> {
        let theme = &look.theme;
        let mut out: Vec<Vec<Span<'static>>> = Vec::new();
        let row = self
            .rows
            .get(self.cursor)
            .filter(|_| self.asked.is_some() && !self.rows.is_empty());
        if let Some(row) = row {
            let ctx = &row.context;
            let place = if ctx.place.is_empty() {
                &row.text
            } else {
                &ctx.place
            };
            let line_no = if ctx.line > 0 {
                format!("line {}", ctx.line)
            } else {
                String::new()
            };
            let at = kit::columns(
                &[
                    Col::Stretch {
                        weight: 1,
                        min: 8,
                        max: None,
                    },
                    Col::Right {
                        natural: draw::width(&line_no),
                        cap: 12,
                    },
                ],
                cols.saturating_sub(2),
                2,
                &[1],
            );
            let mut head = Vec::new();
            if let Some((x, w)) = at[0] {
                kit::place(
                    &mut head,
                    vec![Span::styled(place.clone(), Tone::Text.bold(theme))],
                    (x + 1, w),
                );
            }
            if let Some((x, w)) = at[1].filter(|_| !line_no.is_empty()) {
                kit::place(
                    &mut head,
                    vec![Span::styled(line_no, Tone::Dim.style(theme))],
                    (x + 1, w),
                );
            }
            out.push(kit::fit(head, cols, Style::new()));
            out.push(kit::fit(
                vec![
                    Span::raw(" "),
                    Span::styled(ctx.about.clone(), Tone::Dim.style(theme)),
                ],
                cols,
                Style::new(),
            ));
            out.push(vec![kit::rule(cols, theme)]);
            let room = rows.saturating_sub(PREVIEW_HEAD);
            let top = self.peek_top(room, cols);
            let first = ctx.line.saturating_sub(ctx.at);
            let wrapped = self.wrapped(cols);
            let digits = wrapped.digits;
            let words = self.words();
            // Each line's words lit once, whole, so a word a wrap splits
            // stays lit on both rows.
            let marked: Vec<Vec<Span<'static>>> = wrapped
                .texts
                .iter()
                .enumerate()
                .map(|(i, text)| {
                    let tone = if i == ctx.at { Tone::Text } else { Tone::Dim };
                    kit::found(text, &words, tone.style(theme), theme)
                })
                .collect();
            for row in wrapped.rows.iter().skip(top).take(room) {
                let lit = row.line == ctx.at;
                let mut spans = Vec::new();
                if digits > 0 {
                    let tone = if lit { Tone::Accent } else { Tone::Faint };
                    let number = if row.first {
                        (first + row.line).to_string()
                    } else {
                        String::new()
                    };
                    spans.push(Span::styled(
                        format!("{number:>digits$}  "),
                        tone.style(theme),
                    ));
                }
                // Every row under its line's indentation, so a nested item
                // reads as one.
                spans.push(Span::raw(" ".repeat(row.indent)));
                spans.extend(slice_spans(&marked[row.line], row.part.clone()));
                out.push(kit::row(spans, cols, lit, theme));
            }
        }
        while out.len() < rows {
            out.push(vec![Span::raw(" ".repeat(cols))]);
        }
        out.truncate(rows);
        out
    }
}

/// The chosen result's context lines wrapped to the preview's width.
#[derive(Default)]
struct Wrapped {
    /// Each line as drawn: tabs as four spaces, control characters left
    /// out.
    texts: Vec<String>,
    rows: Vec<PeekRow>,
    /// The line numbers' width, 0 when the context has none.
    digits: usize,
}

/// One of the wrapped context's rows: its line, the part of the line's
/// text it holds after its indentation, the indentation it is drawn under
/// (the line's own, at most half the row, so no indent leaves a line
/// without room), and whether it starts the line.
#[derive(Debug, PartialEq, Eq)]
struct PeekRow {
    line: usize,
    part: Range<usize>,
    indent: usize,
    first: bool,
}

impl Wrapped {
    fn of(ctx: &search::Context, cols: usize) -> Wrapped {
        let first = ctx.line.saturating_sub(ctx.at);
        let digits = if ctx.line > 0 {
            (first + ctx.lines.len()).to_string().len()
        } else {
            0
        };
        // The chosen bar's column and the line numbers' with their gap.
        let text_cols = cols
            .saturating_sub(1 + if digits > 0 { digits + 2 } else { 0 })
            .max(1);
        let texts: Vec<String> = ctx
            .lines
            .iter()
            .map(|t| {
                t.replace('\t', "    ")
                    .chars()
                    .filter(|c| !c.is_control())
                    .collect()
            })
            .collect();
        let mut rows = Vec::new();
        for (line, text) in texts.iter().enumerate() {
            let lead = text.len() - text.trim_start_matches(' ').len();
            // Never so deep that a wrapped row has less than half the room.
            let indent = lead.min(text_cols / 2);
            let mut from = lead;
            loop {
                let part = wrap_at(text, from, text_cols - indent);
                let first = from == lead;
                if part.is_empty() && !first {
                    break;
                }
                from = part.end;
                rows.push(PeekRow {
                    line,
                    part,
                    indent,
                    first,
                });
                if from >= text.len() {
                    break;
                }
            }
        }
        Wrapped {
            texts,
            rows,
            digits,
        }
    }
}

/// The next row of `text` from byte `from`, at most `cols` wide: up to
/// the last space that fits, or inside a word where none does, the spaces
/// at the break left out of both rows.
fn wrap_at(text: &str, from: usize, cols: usize) -> Range<usize> {
    let from = text.len() - text[from..].trim_start_matches(' ').len();
    let rest = &text[from..];
    if draw::width(rest) <= cols {
        return from..text.len();
    }
    let mut used = 0;
    let mut end = from;
    let mut space = None;
    for (i, g) in rest.grapheme_indices(true) {
        if g == " " {
            space = Some(from + i);
        }
        let w = draw::width(g);
        if used + w > cols {
            break;
        }
        used += w;
        end = from + i + g.len();
    }
    let cut = match space {
        Some(at) => at,
        // A grapheme wider than the row goes on one of its own.
        None if end == from => from + rest.graphemes(true).next().map_or(0, str::len),
        None => end,
    };
    from..from + text[from..cut].trim_end_matches(' ').len()
}

/// The part of styled `spans` that falls in bytes `range` of their text.
fn slice_spans(spans: &[Span<'static>], range: Range<usize>) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut at = 0;
    for span in spans {
        let (start, end) = (at, at + span.content.len());
        at = end;
        let (from, to) = (range.start.max(start), range.end.min(end));
        if from < to {
            out.push(Span::styled(
                span.content[from - start..to - start].to_string(),
                span.style,
            ));
        }
    }
    out
}

impl Overlay for SearchBox {
    fn drawn(&self) -> Option<&Drawn> {
        self.drawn.as_ref()
    }

    fn kind(&self) -> &'static str {
        "search"
    }

    fn stands_out(&self) -> Option<Rect> {
        self.drawn.as_ref().map(|d| d.outer)
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let area = buf.area;
        // Three fifths of the screen, as the pickers take (four fifths with
        // the preview), but never too narrow for every chip with its count.
        let share = if self.preview { 80 } else { 60 };
        let outer_w = (area.width as usize * share / 100)
            .max(CHIPS_WIDTH + 2)
            .min(area.width as usize);
        let outer_h = (area.height as usize * 60 / 100)
            .max(10)
            .min(area.height as usize);
        let inner = outer_w.saturating_sub(2);
        let body = outer_h.saturating_sub(2 + HEAD).max(1);
        self.previewed = match self.preview {
            false => Preview::Off,
            true if inner >= SIDE_MIN => {
                let natural = inner * 45 / 100;
                let most = inner.saturating_sub(PREVIEW_MIN + 1).max(RESULTS_MIN);
                Preview::Beside(self.split.unwrap_or(natural).clamp(RESULTS_MIN, most))
            }
            // Under the results, with a rule above it: half the lines, at
            // least its head and one line of context, the results keeping
            // three.
            true if body > PREVIEW_HEAD + 1 + 3 => {
                let n = PREVIEW_UNDER.min((body - 1) / 2).max(PREVIEW_HEAD + 1);
                Preview::Under(HEAD + body - n, n)
            }
            true => Preview::Off,
        };
        self.room = match self.previewed {
            Preview::Under(_, n) => body - n - 1,
            _ => body,
        };
        let results_w = match self.previewed {
            Preview::Beside(w) => w,
            _ => inner,
        };
        let theme = look.theme;
        let found = self.rows.iter().filter(|r| !r.link.is_empty()).count();
        let right = match &self.asked {
            Some(_) if !self.waiting.is_empty() => format!("{found} so far"),
            Some(_) => format!("{found} found"),
            None => String::new(),
        };
        let mut lines = vec![
            BoxLine::spans({
                let prompt = self.prompt();
                self.field_at = (
                    kit::input_field_room(&prompt, inner, draw::width(&right)),
                    3 + draw::width(&prompt),
                );
                kit::input_field(
                    &prompt,
                    &self.query,
                    "",
                    inner,
                    vec![Span::styled(right, Tone::Dim.style(&theme))],
                    &theme,
                    self.caps,
                    true,
                )
            })
            .clickable(),
            BoxLine::blank(),
            BoxLine::spans(self.chips_line(look)).clickable(),
            BoxLine::spans(vec![kit::rule(inner, &theme)]),
        ];
        let all = self.result_lines();
        // The chosen row in sight, with its label when there is room.
        if let Some(at) = all.iter().position(|l| *l == Line::Row(self.cursor)) {
            let from = at.saturating_sub(1);
            if from < self.top {
                self.top = from;
            } else if at >= self.top + self.room {
                self.top = at + 1 - self.room;
            }
        }
        self.top = self.top.min(all.len().saturating_sub(self.room));
        self.shown = all.iter().copied().skip(self.top).take(self.room).collect();
        for line in &self.shown {
            lines.push(self.result_line(*line, results_w, look));
        }
        // As tall as its share of the screen whatever it holds.
        while lines.len() < HEAD + self.room {
            lines.push(BoxLine::blank());
        }
        match self.previewed {
            Preview::Off => {}
            Preview::Beside(w) => {
                let peek = self.preview_lines(inner - w - 1, self.room, look);
                for (line, peek) in lines[HEAD..].iter_mut().zip(peek) {
                    // The results' line cut to its column, a hovered one's
                    // fill kept to it.
                    let mut spans = kit::fit(std::mem::take(&mut line.spans), w, Style::new());
                    if let Some(bg) = line.fill.take() {
                        spans = spans
                            .into_iter()
                            .map(|s| Span::styled(s.content, s.style.bg(bg)))
                            .collect();
                    }
                    spans.push(kit::rule_v(&theme));
                    spans.extend(peek);
                    line.spans = spans;
                }
            }
            Preview::Under(_, n) => {
                lines.push(BoxLine::spans(vec![kit::rule(inner, &theme)]));
                lines.extend(
                    self.preview_lines(inner, n, look)
                        .into_iter()
                        .map(BoxLine::spans),
                );
            }
        }
        let mut spec = BoxSpec::new("Search everything", &lines);
        spec.width = inner as u16;
        spec.close_lit = self.lit_part(Part::Close);
        let drawn = draw::draw_box(buf, &spec, look);
        draw::edge_bar(buf, &drawn, (HEAD, self.room), (all.len(), self.top), look);
        self.drawn = Some(drawn);
    }

    fn key(&mut self, key: Key, look: &Look) -> Step {
        let ctrl = key.mods == Mods::CTRL;
        let plain = key.mods == Mods::NONE || key.mods == Mods::SHIFT;
        if let Preview::Beside(w) = self.previewed
            && let Some((_, by)) = self
                .resize_keys
                .iter()
                .find(|(k, _)| *k == key || *k == key.normalized())
        {
            return self.split_to(w as isize + by);
        }
        match key.code {
            KeyCode::Esc => return Step::Close(Outcome::Dismissed),
            KeyCode::Char('c' | 'g') if ctrl => return Step::Close(Outcome::Dismissed),
            KeyCode::Tab => return self.step_focus(1, look),
            KeyCode::BackTab => return self.step_focus(-1, look),
            _ if let Some(by) =
                crate::ui::kit::nav::typing_rows(key).filter(|_| !self.arrows || ctrl) =>
            {
                return self.step(by > 0);
            }
            KeyCode::Down => return self.down(),
            KeyCode::Up => return self.up(),
            // C-k is the field's, to the line's end.
            KeyCode::Char('j') if ctrl => return self.step(true),
            KeyCode::PageDown => return self.move_to(self.cursor + self.room),
            KeyCode::PageUp => return self.move_to(self.cursor.saturating_sub(self.room)),
            KeyCode::Char('o') if ctrl => return self.take(true),
            KeyCode::Char(c @ ('J' | 'K'))
                if plain && self.focus == Part::Results && self.previewed != Preview::Off =>
            {
                return self.scroll_peek(if c == 'J' { 1 } else { -1 });
            }
            KeyCode::Left if self.arrows && is_chip(self.focus) => return self.step_chip(-1),
            KeyCode::Right if self.arrows && is_chip(self.focus) => return self.step_chip(1),
            _ => {}
        }
        let on_button = is_chip(self.focus) || self.focus == Part::Close;
        if on_button && matches!(key.code, KeyCode::Enter | KeyCode::Char(' ')) {
            return self.press(self.focus);
        }
        match key.code {
            KeyCode::Enter if key.mods == Mods::NONE => self.enter(false),
            KeyCode::Char(_) if plain => {
                self.focus = Part::Field;
                self.type_key(key);
                Step::Redraw
            }
            // Anything taken out of the field ends a run of prefixes, so a
            // prefix typed after it starts its own.
            _ => match self.query.key(key) {
                Edit::Changed => {
                    self.focus = Part::Field;
                    self.prefixed = false;
                    // An edit that leaves a prefix at the start takes it, as typing it does.
                    self.scope_typed();
                    Step::Redraw
                }
                Edit::Moved => {
                    self.focus = Part::Field;
                    Step::Redraw
                }
                _ => Step::Same,
            },
        }
    }

    fn paste(&mut self, text: &str, _look: &Look) -> Step {
        self.focus = Part::Field;
        self.type_in(text);
        Step::Redraw
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        let Some(drawn) = &self.drawn else {
            return Step::Same;
        };
        // The results' scroll bar: a press pages toward it, a drag moves
        // the results, the cursor kept on a row shown.
        let lines = self.result_lines();
        let (total, room) = (lines.len(), self.room);
        let bar = match self.dragging {
            Some(_) => draw::edge_bar_drag_row(drawn, HEAD, mouse.row),
            None => draw::edge_bar_row(drawn, (HEAD, room), mouse.col, mouse.row),
        }
        .filter(|_| look.mouse && look.scroll_bars && total > room);
        let to = match (mouse.kind, mouse.button, bar) {
            (MouseKind::Press, MouseButton::Left, Some(row)) => {
                self.dragging = Some(kit::scroll_grab(total, self.top, room, room, row));
                Some(kit::scroll_press(total, self.top, room, room, row).unwrap_or(self.top))
            }
            (MouseKind::Drag, _, Some(row)) if self.dragging.is_some() => {
                let grab = self.dragging.unwrap_or_default();
                Some(kit::scroll_drag(total, room, room, row, grab))
            }
            (MouseKind::Drag, ..) if self.dragging.is_some() => return Step::Same,
            (MouseKind::Release, ..) if self.dragging.is_some() => {
                self.dragging = None;
                return Step::Same;
            }
            _ => None,
        };
        if let Some(to) = to {
            self.top = to;
            self.focus = Part::Results;
            // The row nearest the one chosen among those now shown, past
            // the first line, whose label the drawing keeps above it.
            let shown = lines.iter().enumerate().skip(to).take(room);
            let near = shown
                .filter_map(|(at, l)| match l {
                    Line::Row(r) if at > to || to == 0 => Some(*r),
                    _ => None,
                })
                .min_by_key(|r| r.abs_diff(self.cursor));
            if let Some(r) = near {
                self.cursor = r;
            }
            return Step::Redraw;
        }
        let Some(drawn) = &self.drawn else {
            return Step::Same;
        };
        let hit = drawn.hit(mouse.col, mouse.row);
        // The rule between the results and the preview: a press on it
        // starts a drag that moves it.
        if let Preview::Beside(w) = self.previewed
            && look.mouse
        {
            let inner_x = drawn.inner.x;
            match (mouse.kind, mouse.button, hit) {
                (MouseKind::Press, MouseButton::Left, Hit::Line(i, col))
                    if i >= HEAD && col as usize == w =>
                {
                    self.splitting = true;
                    return Step::Same;
                }
                (MouseKind::Drag, ..) if self.splitting => {
                    return self.split_to(mouse.col as isize - inner_x as isize);
                }
                (MouseKind::Release, ..) if self.splitting => {
                    self.splitting = false;
                    return Step::Same;
                }
                _ => {}
            }
        }
        if self.on_preview(hit) {
            match draw::gesture(&mouse) {
                Gesture::WheelDown => return self.scroll_peek(1),
                Gesture::WheelUp => return self.scroll_peek(-1),
                _ => {}
            }
        }
        match draw::gesture(&mouse) {
            Gesture::Click if look.mouse => {
                if hit == Hit::Outside {
                    return Step::Close(Outcome::Dismissed);
                }
                if let Some(part) = self.part_at(hit) {
                    if part == Part::Field {
                        self.focus = Part::Field;
                        if let Hit::Line(_, col) = hit {
                            let (room, start) = self.field_at;
                            self.query
                                .click_one_row(room, (col as usize).saturating_sub(start));
                        }
                        return Step::Redraw;
                    }
                    return self.press(part);
                }
                // A click chooses a row, and a second on it soon after
                // takes it, as the picker's rows do.
                let Some(row) = self.row_at(hit) else {
                    return Step::Same;
                };
                let now = Instant::now();
                let second = self
                    .last_click
                    .is_some_and(|(at, r)| r == row && now.duration_since(at) < DOUBLE_CLICK);
                if second && row == self.cursor {
                    self.last_click = None;
                    return self.take(false);
                }
                self.last_click = Some((now, row));
                self.cursor = row;
                self.focus = Part::Results;
                Step::Redraw
            }
            Gesture::WheelDown => self.move_to(self.cursor + 1),
            Gesture::WheelUp => self.move_to(self.cursor.saturating_sub(1)),
            Gesture::Move if look.hover => {
                let part = self.part_at(hit).filter(|p| *p != Part::Field);
                let row = self.row_at(hit);
                if (part, row) == (self.hover, self.hover_row) {
                    return Step::Same;
                }
                self.hover = part;
                self.hover_row = row;
                Step::Redraw
            }
            _ => Step::Same,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(source: Source, link: &str) -> Found {
        Found {
            context: search::Context::default(),
            source,
            text: link.to_string(),
            detail: String::new(),
            link: link.to_string(),
        }
    }

    fn open(
        off: &[Source],
        typed: &str,
    ) -> (SearchBox, tokio::sync::mpsc::UnboundedReceiver<Event>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let b = SearchBox::new(
            ClientId(1),
            tx,
            None,
            Source::ALL.to_vec(),
            off,
            typed,
            false,
        );
        (b, rx)
    }

    /// The asks and chip sets sent, in order.
    fn sent(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(e) = rx.try_recv() {
            match e {
                Event::Views(ViewsEvent::Ask { query, sources, .. }) => {
                    out.push(format!("ask {query} {sources:?}"))
                }
                Event::Views(ViewsEvent::Chips { off }) => out.push(format!("off {off:?}")),
                _ => {}
            }
        }
        out
    }

    fn type_text(b: &mut SearchBox, text: &str) {
        for c in text.chars() {
            b.key(Key::plain(KeyCode::Char(c)), &Look::default());
        }
    }

    #[test]
    fn a_typed_prefix_leaves_the_query_and_lights_its_sources_alone() {
        let (b, _) = open(&[], "note: ");
        assert_eq!(b.lit(), [Source::Notes]);
        assert_eq!(b.typed(), "");
        let (mut b, mut rx) = open(&[Source::Slack], "");
        assert_eq!(b.lit().len(), 4, "the kept set");
        type_text(&mut b, "slack:gh: deploy");
        assert_eq!(b.lit(), [Source::GitHub, Source::Slack]);
        assert_eq!(b.typed(), "deploy");
        assert_eq!(sent(&mut rx), Vec::<String>::new(), "a prefix isn't kept");
        let (mut b, _) = open(&[], "");
        type_text(&mut b, "fix: it");
        assert_eq!(b.typed(), "fix: it", "a word naming no source stays");
        assert_eq!(b.lit(), Source::ALL);
    }

    #[test]
    fn a_prefix_pasted_before_the_text_keeps_the_cursor_where_it_was() {
        let (mut b, _) = open(&[], "");
        let look = Look::default();
        type_text(&mut b, "beta ");
        b.key(Key::plain(KeyCode::Home), &look);
        b.paste("note: alpha", &look);
        type_text(&mut b, "X");
        assert_eq!(b.typed(), "alphaXbeta ");
        assert_eq!(b.lit(), [Source::Notes]);
    }

    #[test]
    fn undo_never_brings_a_prefix_back_as_text() {
        let (mut b, _) = open(&[], "");
        type_text(&mut b, "note: alpha");
        let undo = Key::new(KeyCode::Char('z'), Mods::CTRL);
        b.key(undo, &Look::default());
        b.key(undo, &Look::default());
        assert!(!b.typed().contains("note:"), "{}", b.typed());
        assert_eq!(b.lit(), [Source::Notes]);
    }

    #[test]
    fn enter_asks_the_lit_sources_and_a_chip_turned_on_after_asks_it_alone() {
        let (mut b, mut rx) = open(&[Source::Slack], "");
        let look = Look::default();
        type_text(&mut b, "zebra");
        b.key(Key::plain(KeyCode::Enter), &look);
        assert_eq!(
            sent(&mut rx),
            ["ask zebra [Tabs, Notes, Linear, GitHub]"],
            "Slack, off, is sent nothing"
        );
        let id = b.asked.as_ref().unwrap().id;
        assert!(b.answered(id, Source::Linear, vec![found(Source::Linear, "l1")]));
        assert!(!b.answered(id, Source::Slack, vec![found(Source::Slack, "s1")]));
        assert!(
            !b.answered(id + 99, Source::Notes, vec![]),
            "an earlier search"
        );
        // Tab to Slack's chip (field, All, Tabs, Notes, Linear, GitHub,
        // Slack) and Space turns it on.
        for _ in 0..6 {
            b.key(Key::plain(KeyCode::Tab), &look);
        }
        assert_eq!(b.focus, Part::Chip(Source::Slack));
        b.key(Key::plain(KeyCode::Char(' ')), &look);
        assert_eq!(sent(&mut rx), ["off []", "ask zebra [Slack]"]);
        // Linear's chip, clicked off, drops its rows and is kept.
        b.press(Part::Chip(Source::Linear));
        assert!(b.rows.is_empty());
        assert_eq!(sent(&mut rx), ["off [Linear]"]);
        assert!(!b.answered(id, Source::Linear, vec![found(Source::Linear, "l2")]));
        // All lights every source again.
        b.press(Part::All);
        assert_eq!(b.lit(), Source::ALL);
        assert_eq!(sent(&mut rx), ["off []", "ask zebra [Linear]"]);
    }

    #[test]
    fn enter_takes_the_chosen_row_once_the_results_are_of_what_is_typed() {
        let (mut b, _rx) = open(&[], "");
        let look = Look::default();
        type_text(&mut b, "zebra");
        b.key(Key::plain(KeyCode::Enter), &look);
        let id = b.asked.as_ref().unwrap().id;
        b.answered(
            id,
            Source::Notes,
            vec![found(Source::Notes, "n1"), found(Source::Notes, "n2")],
        );
        // Down to the chips, the first result, the second.
        for _ in 0..3 {
            b.key(Key::plain(KeyCode::Down), &look);
        }
        match b.key(Key::plain(KeyCode::Enter), &look) {
            Step::Close(Outcome::Picked(p)) => {
                assert_eq!((p.value.as_str(), p.query.as_str()), ("n2", "zebra"))
            }
            other => panic!("{other:?}"),
        }
        // A prefix lighting other sources, the same words: Enter asks them.
        b.key(Key::new(KeyCode::Char('u'), Mods::CTRL), &look);
        type_text(&mut b, "tab: zebra");
        assert_eq!(b.key(Key::plain(KeyCode::Enter), &look), Step::Redraw);
        assert!(
            b.rows.is_empty(),
            "the Notes rows aren't taken for the Tabs search"
        );
        // Typed again, Enter asks again.
        type_text(&mut b, "s");
        assert_eq!(b.key(Key::plain(KeyCode::Enter), &look), Step::Redraw);
        assert!(b.rows.is_empty());
    }

    #[test]
    fn a_new_query_s_prefix_replaces_the_last_one_s_sources() {
        let (mut b, _rx) = open(&[], "");
        let look = Look::default();
        type_text(&mut b, "slack: deploy");
        assert_eq!(b.lit(), [Source::Slack]);
        b.key(Key::new(KeyCode::Char('u'), Mods::CTRL), &look);
        type_text(&mut b, "note: x");
        assert_eq!(b.lit(), [Source::Notes], "not Slack too");
        for clear in [
            Key::new(KeyCode::Char('w'), Mods::CTRL),
            Key::new(KeyCode::Backspace, Mods::NONE),
        ] {
            let (mut b, _rx) = open(&[], "slack: n");
            b.key(clear, &look);
            type_text(&mut b, "note: x");
            assert_eq!(b.lit(), [Source::Notes], "{clear:?}");
        }
        // Typed after a search, with nothing asked of Notes alone yet, an
        // empty field takes nothing.
        let (mut b, _rx) = open(&[], "");
        type_text(&mut b, "zebra");
        b.key(Key::plain(KeyCode::Enter), &look);
        let id = b.asked.as_ref().unwrap().id;
        b.answered(id, Source::Tabs, vec![found(Source::Tabs, "t1")]);
        b.key(Key::new(KeyCode::Char('u'), Mods::CTRL), &look);
        type_text(&mut b, "note:");
        assert_eq!(b.key(Key::plain(KeyCode::Enter), &look), Step::Same);
    }

    #[test]
    fn moving_through_the_results_takes_the_focus_off_a_chip() {
        let (mut b, _rx) = open(&[], "");
        let look = Look::default();
        type_text(&mut b, "zebra");
        b.key(Key::plain(KeyCode::Enter), &look);
        let id = b.asked.as_ref().unwrap().id;
        b.answered(
            id,
            Source::Tabs,
            vec![found(Source::Tabs, "t1"), found(Source::Tabs, "t2")],
        );
        b.key(Key::plain(KeyCode::Tab), &look);
        b.key(Key::plain(KeyCode::Down), &look);
        b.key(Key::plain(KeyCode::Down), &look);
        assert_eq!(b.focus, Part::Results);
        match b.key(Key::plain(KeyCode::Enter), &look) {
            Step::Close(Outcome::Picked(p)) => assert_eq!(p.value, "t2"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn down_from_the_field_focuses_the_tabs_chip_and_up_goes_back() {
        let (mut b, _rx) = open(&[], "");
        let look = Look::default();
        type_text(&mut b, "zeb");
        b.key(Key::plain(KeyCode::Down), &look);
        assert_eq!(b.focus, Part::Chip(Source::Tabs));
        b.key(Key::plain(KeyCode::Up), &look);
        assert_eq!(b.focus, Part::Field);
        type_text(&mut b, "ra");
        assert_eq!(b.typed(), "zebra");
    }

    #[test]
    fn left_and_right_wrap_along_the_chips() {
        let (mut b, _rx) = open(&[], "");
        let look = Look::default();
        b.key(Key::plain(KeyCode::Down), &look);
        b.key(Key::plain(KeyCode::Left), &look);
        b.key(Key::plain(KeyCode::Left), &look);
        assert_eq!(b.focus, Part::Chip(Source::Slack));
        b.key(Key::plain(KeyCode::Right), &look);
        assert_eq!(b.focus, Part::All);
        // Down from the field again comes back to the chip left last.
        b.key(Key::plain(KeyCode::Right), &look);
        b.key(Key::plain(KeyCode::Up), &look);
        b.key(Key::plain(KeyCode::Down), &look);
        assert_eq!(b.focus, Part::Chip(Source::Tabs));
    }

    #[test]
    fn enter_on_a_chip_reached_by_the_arrows_turns_its_source_off() {
        let (mut b, _rx) = open(&[], "zebra");
        let look = Look::default();
        b.key(Key::plain(KeyCode::Enter), &look);
        let id = b.asked.as_ref().unwrap().id;
        b.answered(id, Source::Tabs, vec![found(Source::Tabs, "t1")]);
        b.answered(id, Source::Notes, vec![found(Source::Notes, "n1")]);
        b.key(Key::plain(KeyCode::Down), &look);
        b.key(Key::plain(KeyCode::Enter), &look);
        assert!(!b.lit().contains(&Source::Tabs));
        assert!(b.rows.iter().all(|r| r.source != Source::Tabs));
    }

    #[test]
    fn down_from_the_chips_reaches_the_first_result_and_up_returns() {
        let (mut b, _rx) = open(&[], "zebra");
        let look = Look::default();
        b.key(Key::plain(KeyCode::Enter), &look);
        let id = b.asked.as_ref().unwrap().id;
        b.answered(
            id,
            Source::Tabs,
            vec![found(Source::Tabs, "t1"), found(Source::Tabs, "t2")],
        );
        b.key(Key::plain(KeyCode::Down), &look);
        b.key(Key::plain(KeyCode::Right), &look);
        b.key(Key::plain(KeyCode::Down), &look);
        assert_eq!((b.focus, b.cursor), (Part::Results, 0));
        b.key(Key::plain(KeyCode::Up), &look);
        assert_eq!(b.focus, Part::Chip(Source::Notes));
    }

    #[test]
    fn down_on_the_last_result_chooses_the_first_and_ctrl_p_on_the_first_the_last() {
        let (mut b, _rx) = open(&[], "zebra");
        let look = Look::default();
        b.key(Key::plain(KeyCode::Enter), &look);
        let id = b.asked.as_ref().unwrap().id;
        b.answered(
            id,
            Source::Tabs,
            vec![found(Source::Tabs, "t1"), found(Source::Tabs, "t2")],
        );
        b.key(Key::plain(KeyCode::Down), &look);
        b.key(Key::plain(KeyCode::Down), &look);
        b.key(Key::plain(KeyCode::Down), &look);
        assert_eq!((b.focus, b.cursor), (Part::Results, 1));
        b.key(Key::plain(KeyCode::Down), &look);
        assert_eq!(b.cursor, 0, "wrapped to the first result");
        b.key(Key::new(KeyCode::Char('p'), Mods::CTRL), &look);
        assert_eq!(b.cursor, 1, "wrapped to the last result");
        assert_eq!(b.key(Key::plain(KeyCode::PageDown), &look), Step::Same);
    }

    #[test]
    fn with_the_arrows_setting_off_down_from_the_field_moves_through_the_results() {
        let (b, _rx) = open(&[], "zebra");
        let mut b = b.with_arrows(false);
        let look = Look::default();
        b.key(Key::plain(KeyCode::Enter), &look);
        let id = b.asked.as_ref().unwrap().id;
        b.answered(
            id,
            Source::Tabs,
            vec![found(Source::Tabs, "t1"), found(Source::Tabs, "t2")],
        );
        b.key(Key::plain(KeyCode::Down), &look);
        assert_eq!((b.focus, b.cursor), (Part::Results, 1));
        b.key(Key::plain(KeyCode::Tab), &look);
        b.key(Key::plain(KeyCode::Right), &look);
        assert_eq!(b.focus, Part::All);
    }

    #[test]
    fn a_letter_typed_on_a_chip_goes_into_the_field() {
        let (mut b, _rx) = open(&[], "zeb");
        let look = Look::default();
        b.key(Key::plain(KeyCode::Down), &look);
        b.key(Key::plain(KeyCode::Char('r')), &look);
        assert_eq!((b.focus, b.typed()), (Part::Field, "zebr"));
    }

    #[test]
    fn a_chip_pressed_after_a_prefix_asks_every_source_the_search_did_not() {
        let (mut b, mut rx) = open(&[], "tab: deploy");
        b.key(Key::plain(KeyCode::Enter), &Look::default());
        b.key(Key::new(KeyCode::Char('u'), Mods::CTRL), &Look::default());
        type_text(&mut b, "note: deploy");
        sent(&mut rx);
        b.press(Part::All);
        assert_eq!(
            sent(&mut rx),
            ["off []", "ask deploy [Notes, Linear, GitHub, Slack]"]
        );
    }

    #[test]
    fn ctrl_o_takes_nothing_once_a_prefix_changed_the_sources() {
        let (mut b, _rx) = open(&[], "zebra");
        let look = Look::default();
        b.key(Key::plain(KeyCode::Enter), &look);
        let id = b.asked.as_ref().unwrap().id;
        b.answered(id, Source::Tabs, vec![found(Source::Tabs, "t1")]);
        b.key(Key::new(KeyCode::Char('u'), Mods::CTRL), &look);
        type_text(&mut b, "note: zebra");
        assert_eq!(
            b.key(Key::new(KeyCode::Char('o'), Mods::CTRL), &look),
            Step::Same
        );
    }

    #[test]
    fn the_results_scroll_bar_scrolls_and_a_row_clicked_takes_the_focus() {
        let (mut b, _rx) = open(&[], "zebra");
        let look = Look::default();
        b.key(Key::plain(KeyCode::Enter), &look);
        let id = b.asked.as_ref().unwrap().id;
        let many = (0..40)
            .map(|i| found(Source::Notes, &format!("n{i}")))
            .collect();
        b.answered(id, Source::Notes, many);
        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 30));
        b.draw(&mut buf, &look);
        let drawn = b.drawn.clone().unwrap();
        let (last, _) = *drawn.rows.last().unwrap();
        let press = |col, row| Mouse {
            kind: MouseKind::Press,
            button: MouseButton::Left,
            col,
            row,
            mods: Mods::NONE,
        };
        assert_eq!(
            b.mouse(press(drawn.outer.right() - 1, last), &look),
            Step::Redraw
        );
        assert!(b.top > 0, "a press low on the bar pages down");
        assert!(b.cursor > 0, "the cursor follows onto a row shown");
        b.mouse(
            Mouse {
                kind: MouseKind::Release,
                ..press(0, 0)
            },
            &look,
        );
        // Tab onto a chip, then a click on another row: Enter takes it.
        b.draw(&mut buf, &look);
        b.key(Key::plain(KeyCode::Tab), &look);
        let drawn = b.drawn.clone().unwrap();
        let (row_y, _) = drawn.rows[HEAD + 2];
        assert_eq!(
            b.mouse(press(drawn.inner.x + 4, row_y), &look),
            Step::Redraw
        );
        assert_eq!(b.focus, Part::Results);
        // A second click on it takes it.
        assert!(matches!(
            b.mouse(press(drawn.inner.x + 4, row_y), &look),
            Step::Close(Outcome::Picked(_))
        ));
        b.mouse(press(drawn.inner.x + 4, row_y), &look);
        assert!(matches!(
            b.key(Key::plain(KeyCode::Enter), &look),
            Step::Close(Outcome::Picked(_))
        ));
    }

    #[test]
    fn every_chip_shows_on_an_80_column_terminal() {
        let (mut b, _rx) = open(&[], "zebra");
        let look = Look::default();
        b.key(Key::plain(KeyCode::Enter), &look);
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
        b.draw(&mut buf, &look);
        let drawn = b.drawn.clone().unwrap();
        let (last, _, end) = *b.chip_at.last().unwrap();
        assert_eq!(last, Part::Chip(Source::Slack));
        assert!(
            end < drawn.inner.width,
            "{:?} in {}",
            b.chip_at,
            drawn.inner.width
        );
    }

    #[test]
    fn the_last_chip_lit_stays_on() {
        let (mut b, _rx) = open(&[], "note:");
        b.press(Part::Chip(Source::Notes));
        assert_eq!(b.lit(), [Source::Notes]);
    }

    /// A row whose context is `lines`, the match at `at`, numbered from
    /// `first`.
    fn with_context(link: &str, lines: &[&str], at: usize, first: usize) -> Found {
        Found {
            context: search::Context {
                place: format!("Place of {link}"),
                about: "Note in Notes".to_string(),
                line: first + at,
                lines: lines.iter().map(|l| l.to_string()).collect(),
                at,
            },
            ..found(Source::Notes, link)
        }
    }

    /// The screen's rows as text.
    fn screen(buf: &Buffer) -> Vec<String> {
        let a = buf.area;
        (0..a.height)
            .map(|y| {
                (0..a.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect()
            })
            .collect()
    }

    /// A box with the preview on, `zebra` searched and two notes found.
    fn previewing() -> (SearchBox, tokio::sync::mpsc::UnboundedReceiver<Event>) {
        let (b, rx) = open(&[], "zebra");
        let mut b = b.with_preview(
            true,
            vec![(
                Key::new(
                    KeyCode::Char('h'),
                    Mods {
                        shift: false,
                        alt: true,
                        ctrl: true,
                    },
                ),
                -5,
            )],
        );
        b.key(Key::plain(KeyCode::Enter), &Look::default());
        let id = b.asked.as_ref().unwrap().id;
        b.answered(
            id,
            Source::Notes,
            vec![
                with_context(
                    "n1",
                    &["## Steps", "", "the zebra line", "    - after it"],
                    2,
                    9,
                ),
                with_context("n2", &["one zebra"], 0, 1),
            ],
        );
        (b, rx)
    }

    #[test]
    fn the_preview_shows_the_chosen_row_s_context_with_the_match_lit_and_follows_down() {
        let (mut b, _rx) = previewing();
        let look = Look::default();
        let mut buf = Buffer::empty(Rect::new(0, 0, 160, 45));
        b.draw(&mut buf, &look);
        let Preview::Beside(w) = b.previewed else {
            panic!("{:?}", b.previewed)
        };
        let rows = screen(&buf);
        let text = rows.join("\n");
        assert!(text.contains("Place of n1"), "{text}");
        assert!(text.contains("line 11"), "{text}");
        let lit = rows
            .iter()
            .position(|r| r.contains("11  the zebra line"))
            .expect(&text);
        let x = rows[lit].find("zebra line").unwrap();
        let col = rows[lit][..x].chars().count() as u16;
        let cell = &buf[(col, lit as u16)];
        assert_eq!(
            cell.fg,
            Tone::Accent.fg(&look.theme),
            "the word searched for in the accent"
        );
        assert_eq!(
            cell.bg,
            kit::Surface::Chosen.color(&look.theme),
            "its line lit"
        );
        assert!(rows[lit].contains("│"), "beside the results, {w} wide");
        assert!(
            text.contains("12      - after it"),
            "indentation kept: {text}"
        );
        // Down to the chips, the first result, the second: the preview
        // follows.
        for _ in 0..3 {
            b.key(Key::plain(KeyCode::Down), &look);
        }
        b.draw(&mut buf, &look);
        let text = screen(&buf).join("\n");
        assert!(
            text.contains("Place of n2") && !text.contains("Place of n1"),
            "{text}"
        );
        // A resize key moves the rule between them.
        b.key(
            Key::new(
                KeyCode::Char('h'),
                Mods {
                    shift: false,
                    alt: true,
                    ctrl: true,
                },
            ),
            &look,
        );
        b.draw(&mut buf, &look);
        assert_eq!(b.previewed, Preview::Beside(w - 5));
    }

    #[test]
    fn with_the_setting_off_no_preview_draws() {
        let (mut b, _rx) = open(&[], "zebra");
        let look = Look::default();
        b.key(Key::plain(KeyCode::Enter), &look);
        let id = b.asked.as_ref().unwrap().id;
        b.answered(
            id,
            Source::Notes,
            vec![with_context("n1", &["the zebra line"], 0, 3)],
        );
        let mut buf = Buffer::empty(Rect::new(0, 0, 160, 45));
        b.draw(&mut buf, &look);
        assert_eq!(b.previewed, Preview::Off);
        assert!(!screen(&buf).join("\n").contains("Place of n1"));
    }

    #[test]
    fn an_80_column_terminal_s_preview_shows_the_match_s_line() {
        let (mut b, _rx) = previewing();
        let look = Look::default();
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
        b.draw(&mut buf, &look);
        assert!(
            matches!(b.previewed, Preview::Under(..)),
            "{:?}",
            b.previewed
        );
        let text = screen(&buf).join("\n");
        assert!(text.contains("11  the zebra line"), "{text}");
    }

    #[test]
    fn a_narrow_box_puts_the_preview_under_the_results() {
        let (mut b, _rx) = previewing();
        let look = Look::default();
        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 45));
        b.draw(&mut buf, &look);
        let Preview::Under(from, _) = b.previewed else {
            panic!("{:?}", b.previewed)
        };
        let rows = screen(&buf);
        let place = rows.iter().position(|r| r.contains("Place of n1")).unwrap();
        let first = rows.iter().position(|r| r.contains("n2")).unwrap();
        assert!(place > first, "under the results");
        assert_eq!(
            place as u16,
            b.drawn.as_ref().unwrap().inner.y + from as u16
        );
    }

    /// The preview's first context row, at the size it was last drawn.
    fn top(b: &SearchBox) -> usize {
        let (cols, room) = b.peek_size().unwrap();
        b.peek_top(room, cols)
    }

    /// A box with one result found, its context `lines`, the match at `at`.
    fn previewing_lines(lines: &[&str], at: usize) -> SearchBox {
        let (b, _rx) = open(&[], "zebra");
        let mut b = b.with_preview(true, Vec::new());
        b.key(Key::plain(KeyCode::Enter), &Look::default());
        let id = b.asked.as_ref().unwrap().id;
        // Unnumbered, as a message's lines are, so the text has every
        // column but the chosen bar's.
        let mut row = with_context("n1", lines, at, 0);
        row.context.line = 0;
        b.answered(id, Source::Notes, vec![row]);
        b
    }

    /// A 299-character line of nine-letter words, `zebra` among them.
    fn long_line(first: &str) -> String {
        let mut words: Vec<&str> = vec!["abcdefghi"; 30];
        words[0] = first;
        words[25] = "zebrawalk";
        words.join(" ")
    }

    /// The preview's rows as text.
    fn text_of(rows: &[Vec<Span<'static>>]) -> Vec<String> {
        rows.iter()
            .map(|r| r.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn a_long_match_line_wraps_whole_with_its_match_lit() {
        let long = long_line("beginning");
        let b = previewing_lines(&["before it", &long, "after it"], 1);
        let look = Look::default();
        let rows = b.preview_lines(100, PREVIEW_HEAD + 20, &look);
        let text = text_of(&rows);
        let context = &text[PREVIEW_HEAD..];
        assert!(!text.join("\n").contains('…'), "{text:#?}");
        let of_long: Vec<&String> = context
            .iter()
            .filter(|r| r.contains("abcdefghi") || r.contains("beginning"))
            .collect();
        assert_eq!(of_long.len(), 3, "{text:#?}");
        let joined: String = of_long
            .iter()
            .map(|r| r.trim_start_matches('▌').trim())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(joined, long, "every word kept, in order");
        let at = context
            .iter()
            .position(|r| r.contains("zebrawalk"))
            .unwrap();
        let lit = rows[PREVIEW_HEAD + at]
            .iter()
            .find(|s| s.content.contains("zebra"))
            .unwrap();
        assert_eq!(lit.style.fg, Some(Tone::Accent.fg(&look.theme)));
        assert!(context.iter().any(|r| r.contains("before it")));
        assert!(context.iter().any(|r| r.contains("after it")));
    }

    #[test]
    fn a_line_indented_past_the_preview_s_width_still_shows_its_words() {
        let deep = format!("{}zebra at the end", " ".repeat(60));
        let b = previewing_lines(&[&deep], 0);
        let rows = text_of(&b.preview_lines(40, PREVIEW_HEAD + 5, &Look::default()));
        assert!(
            rows[PREVIEW_HEAD..]
                .iter()
                .any(|r| r.contains("zebra at the end")),
            "{rows:#?}"
        );
    }

    #[test]
    fn a_wrapped_row_keeps_its_line_s_indentation() {
        let long = format!("    - {}", long_line("item"));
        let b = previewing_lines(&[&long], 0);
        let rows = text_of(&b.preview_lines(100, PREVIEW_HEAD + 10, &Look::default()));
        let context: Vec<&String> = rows[PREVIEW_HEAD..]
            .iter()
            .filter(|r| r.trim() != "" && r.trim() != "▌")
            .collect();
        assert!(context.len() > 1, "{rows:#?}");
        assert!(context[0].starts_with("▌    - item"), "{rows:#?}");
        for r in &context[1..] {
            assert!(r.starts_with("▌    abcdefghi"), "{rows:#?}");
        }
    }

    #[test]
    fn a_match_line_taller_than_the_preview_shows_its_start() {
        let long = long_line("beginning");
        let b = previewing_lines(&["one", "two", "three", &long, "after"], 3);
        let rows = text_of(&b.preview_lines(40, PREVIEW_HEAD + 3, &Look::default()));
        assert!(rows[PREVIEW_HEAD].contains("beginning"), "{rows:#?}");
    }

    #[test]
    fn j_scrolls_the_wrapped_context_a_row_at_a_time() {
        let long = long_line("beginning");
        let mut b = previewing_lines(&[&long, "after"], 0);
        let look = Look::default();
        let mut buf = Buffer::empty(Rect::new(0, 0, 140, 16));
        b.draw(&mut buf, &look);
        b.key(Key::plain(KeyCode::Down), &look);
        b.key(Key::plain(KeyCode::Down), &look);
        assert_eq!(b.focus, Part::Results);
        assert_eq!(top(&b), 0);
        assert_eq!(b.key(Key::plain(KeyCode::Char('J')), &look), Step::Redraw);
        assert_eq!(top(&b), 1);
        b.draw(&mut buf, &look);
        let (cols, room) = b.peek_size().unwrap();
        let rows = text_of(&b.preview_lines(cols, room + PREVIEW_HEAD, &look));
        assert!(
            !rows.iter().any(|r| r.contains("beginning")),
            "its first row scrolled off: {rows:#?}"
        );
    }

    #[test]
    fn j_and_k_scroll_the_preview_with_the_results_focused() {
        let (mut b, _rx) = previewing();
        let look = Look::default();
        // A small screen, so the context runs past the preview.
        let mut buf = Buffer::empty(Rect::new(0, 0, 140, 16));
        b.draw(&mut buf, &look);
        b.key(Key::plain(KeyCode::Down), &look);
        b.key(Key::plain(KeyCode::Down), &look);
        assert_eq!(b.focus, Part::Results);
        let before = top(&b);
        assert_eq!(b.key(Key::plain(KeyCode::Char('J')), &look), Step::Redraw);
        assert_eq!(top(&b), before + 1);
        b.key(Key::plain(KeyCode::Char('K')), &look);
        assert_eq!(top(&b), before);
        assert_eq!(b.typed(), "zebra", "nothing typed");
        // Scrolled, then a new search: its first result starts at its match.
        b.key(Key::plain(KeyCode::Char('J')), &look);
        assert_eq!(top(&b), before + 1);
        b.ask("zebra".to_string());
        let id = b.asked.as_ref().unwrap().id;
        b.answered(
            id,
            Source::Notes,
            vec![with_context(
                "n1",
                &["## Steps", "", "the zebra line", "after it"],
                2,
                9,
            )],
        );
        assert_eq!(top(&b), before);
    }
}
