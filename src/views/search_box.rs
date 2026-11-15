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

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Span;
use tokio::sync::mpsc::UnboundedSender;

use super::ViewsEvent;
use super::search::{self, Found, Source};
use crate::mux::PaneId;
use crate::mux::input::{Key, KeyCode, Mods, Mouse, MouseButton, MouseKind};
use crate::mux::server::{ClientId, Event};
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use crate::ui::kit::{self, Col, Tone};
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

/// What has Tab's focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Field,
    All,
    Chip(Source),
    Close,
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
    /// The sources Settings has on, a chip each.
    sources: Vec<Source>,
    /// The ones lit.
    lit: Vec<Source>,
    /// Whether the chips lit are a typed prefix's, until a chip is
    /// pressed.
    prefixed: bool,
    query: String,
    asked: Option<Asked>,
    rows: Vec<Found>,
    waiting: Vec<Source>,
    /// The sources that answered the search asked last.
    answered: Vec<Source>,
    cursor: usize,
    top: usize,
    room: usize,
    focus: Part,
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
        let mut b = SearchBox {
            client,
            tx,
            pane,
            caps,
            sources,
            lit,
            prefixed: false,
            query: String::new(),
            asked: None,
            rows: Vec::new(),
            waiting: Vec::new(),
            answered: Vec::new(),
            cursor: 0,
            top: 0,
            room: 10,
            focus: Part::Field,
            hover: None,
            hover_row: None,
            chip_at: Vec::new(),
            shown: Vec::new(),
            drawn: None,
            dragging: None,
            last_click: None,
        };
        b.type_in(typed);
        b
    }

    /// The sources lit, for the tests.
    pub fn lit(&self) -> &[Source] {
        &self.lit
    }

    /// What is typed, for the tests.
    pub fn typed(&self) -> &str {
        &self.query
    }

    /// Text typed into the field: a source's prefix at its start leaves
    /// it and lights the sources it names alone.
    fn type_in(&mut self, text: &str) {
        // The space after a prefix taken out isn't kept either.
        let text = if self.query.is_empty() {
            text.trim_start()
        } else {
            text
        };
        self.query.push_str(text);
        let (named, rest) = search::scope(&self.query);
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
            self.query = rest.to_string();
            self.prefixed = true;
        }
        // Prefixes add up only while nothing but the start of another
        // source's word follows them.
        if !self.query.is_empty() && !search::starts_a_source(&self.query) {
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
        let query = self.query.trim().to_string();
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
            Part::Field => return Step::Same,
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
        let at = stops.iter().position(|p| *p == self.focus).unwrap_or(0) as isize;
        self.focus = stops[(at + by).rem_euclid(stops.len() as isize) as usize];
        Step::Redraw
    }

    /// Moves through the results, the focus back in the field, so Enter
    /// takes the row moved to rather than pressing a chip.
    fn move_to(&mut self, to: usize) -> Step {
        let refocused = self.focus != Part::Field;
        self.focus = Part::Field;
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
            Hit::Line(i, _) if i >= HEAD => match self.shown.get(i - HEAD) {
                Some(Line::Row(r)) if !self.rows[*r].link.is_empty() => Some(*r),
                _ => None,
            },
            _ => None,
        }
    }
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
        // Three fifths of the screen, as the pickers take, but never too
        // narrow for every chip with its count.
        let outer_w = (area.width as usize * 60 / 100)
            .max(CHIPS_WIDTH + 2)
            .min(area.width as usize);
        let outer_h = (area.height as usize * 60 / 100)
            .max(10)
            .min(area.height as usize);
        let inner = outer_w.saturating_sub(2);
        self.room = outer_h.saturating_sub(2 + HEAD).max(1);
        let theme = look.theme;
        let found = self.rows.iter().filter(|r| !r.link.is_empty()).count();
        let right = match &self.asked {
            Some(_) if !self.waiting.is_empty() => format!("{found} so far"),
            Some(_) => format!("{found} found"),
            None => String::new(),
        };
        let mut lines = vec![
            BoxLine::spans(kit::input_prompted(
                &self.prompt(),
                &self.query,
                "",
                inner,
                vec![Span::styled(right, Tone::Dim.style(&theme))],
                &theme,
                self.caps,
            ))
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
            lines.push(self.result_line(*line, inner, look));
        }
        // As tall as its share of the screen whatever it holds.
        while lines.len() < HEAD + self.room {
            lines.push(BoxLine::blank());
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
        match key.code {
            KeyCode::Esc => return Step::Close(Outcome::Dismissed),
            KeyCode::Char('c' | 'g') if ctrl => return Step::Close(Outcome::Dismissed),
            KeyCode::Tab => return self.step_focus(1, look),
            KeyCode::BackTab => return self.step_focus(-1, look),
            KeyCode::Down => return self.move_to(self.cursor + 1),
            KeyCode::Char('n' | 'j') if ctrl => return self.move_to(self.cursor + 1),
            KeyCode::Up => return self.move_to(self.cursor.saturating_sub(1)),
            KeyCode::Char('p' | 'k') if ctrl => return self.move_to(self.cursor.saturating_sub(1)),
            KeyCode::PageDown => return self.move_to(self.cursor + self.room),
            KeyCode::PageUp => return self.move_to(self.cursor.saturating_sub(self.room)),
            KeyCode::Char('o') if ctrl => return self.take(true),
            _ => {}
        }
        if self.focus != Part::Field && matches!(key.code, KeyCode::Enter | KeyCode::Char(' ')) {
            return self.press(self.focus);
        }
        match key.code {
            KeyCode::Enter => self.enter(false),
            // Anything taken out of the field ends a run of prefixes, so a
            // prefix typed after it starts its own.
            KeyCode::Backspace => {
                self.focus = Part::Field;
                self.prefixed = false;
                self.query.pop();
                Step::Redraw
            }
            KeyCode::Char('u') if ctrl => {
                self.focus = Part::Field;
                self.prefixed = false;
                self.query.clear();
                Step::Redraw
            }
            KeyCode::Char('w') if ctrl => {
                self.focus = Part::Field;
                self.prefixed = false;
                let kept = self.query.trim_end().rfind(' ').map_or(0, |at| at + 1);
                self.query.truncate(kept);
                Step::Redraw
            }
            KeyCode::Char(c) if plain => {
                self.focus = Part::Field;
                self.type_in(&c.to_string());
                Step::Redraw
            }
            _ => Step::Same,
        }
    }

    fn paste(&mut self, text: &str, _look: &Look) -> Step {
        self.focus = Part::Field;
        self.type_in(text.lines().next().unwrap_or(""));
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
            self.focus = Part::Field;
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
        match draw::gesture(&mouse) {
            Gesture::Click if look.mouse => {
                if hit == Hit::Outside {
                    return Step::Close(Outcome::Dismissed);
                }
                if let Some(part) = self.part_at(hit) {
                    if part == Part::Field {
                        self.focus = Part::Field;
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
                self.focus = Part::Field;
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

    fn key(code: KeyCode) -> Key {
        Key::new(code, Mods::NONE)
    }

    fn found(source: Source, link: &str) -> Found {
        Found {
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
            b.key(key(KeyCode::Char(c)), &Look::default());
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
    fn enter_asks_the_lit_sources_and_a_chip_turned_on_after_asks_it_alone() {
        let (mut b, mut rx) = open(&[Source::Slack], "");
        let look = Look::default();
        type_text(&mut b, "zebra");
        b.key(key(KeyCode::Enter), &look);
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
            b.key(key(KeyCode::Tab), &look);
        }
        assert_eq!(b.focus, Part::Chip(Source::Slack));
        b.key(key(KeyCode::Char(' ')), &look);
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
        b.key(key(KeyCode::Enter), &look);
        let id = b.asked.as_ref().unwrap().id;
        b.answered(
            id,
            Source::Notes,
            vec![found(Source::Notes, "n1"), found(Source::Notes, "n2")],
        );
        b.key(key(KeyCode::Down), &look);
        match b.key(key(KeyCode::Enter), &look) {
            Step::Close(Outcome::Picked(p)) => {
                assert_eq!((p.value.as_str(), p.query.as_str()), ("n2", "zebra"))
            }
            other => panic!("{other:?}"),
        }
        // A prefix lighting other sources, the same words: Enter asks them.
        b.key(Key::new(KeyCode::Char('u'), Mods::CTRL), &look);
        type_text(&mut b, "tab: zebra");
        assert_eq!(b.key(key(KeyCode::Enter), &look), Step::Redraw);
        assert!(
            b.rows.is_empty(),
            "the Notes rows aren't taken for the Tabs search"
        );
        // Typed again, Enter asks again.
        type_text(&mut b, "s");
        assert_eq!(b.key(key(KeyCode::Enter), &look), Step::Redraw);
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
        b.key(key(KeyCode::Enter), &look);
        let id = b.asked.as_ref().unwrap().id;
        b.answered(id, Source::Tabs, vec![found(Source::Tabs, "t1")]);
        b.key(Key::new(KeyCode::Char('u'), Mods::CTRL), &look);
        type_text(&mut b, "note:");
        assert_eq!(b.key(key(KeyCode::Enter), &look), Step::Same);
    }

    #[test]
    fn moving_through_the_results_takes_the_focus_off_a_chip() {
        let (mut b, _rx) = open(&[], "");
        let look = Look::default();
        type_text(&mut b, "zebra");
        b.key(key(KeyCode::Enter), &look);
        let id = b.asked.as_ref().unwrap().id;
        b.answered(
            id,
            Source::Tabs,
            vec![found(Source::Tabs, "t1"), found(Source::Tabs, "t2")],
        );
        b.key(key(KeyCode::Tab), &look);
        b.key(key(KeyCode::Down), &look);
        assert_eq!(b.focus, Part::Field);
        match b.key(key(KeyCode::Enter), &look) {
            Step::Close(Outcome::Picked(p)) => assert_eq!(p.value, "t2"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_chip_pressed_after_a_prefix_asks_every_source_the_search_did_not() {
        let (mut b, mut rx) = open(&[], "tab: deploy");
        b.key(key(KeyCode::Enter), &Look::default());
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
        b.key(key(KeyCode::Enter), &look);
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
        b.key(key(KeyCode::Enter), &look);
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
        b.key(key(KeyCode::Tab), &look);
        let drawn = b.drawn.clone().unwrap();
        let (row_y, _) = drawn.rows[HEAD + 2];
        assert_eq!(
            b.mouse(press(drawn.inner.x + 4, row_y), &look),
            Step::Redraw
        );
        assert_eq!(b.focus, Part::Field);
        // A second click on it takes it.
        assert!(matches!(
            b.mouse(press(drawn.inner.x + 4, row_y), &look),
            Step::Close(Outcome::Picked(_))
        ));
        b.mouse(press(drawn.inner.x + 4, row_y), &look);
        assert!(matches!(
            b.key(key(KeyCode::Enter), &look),
            Step::Close(Outcome::Picked(_))
        ));
    }

    #[test]
    fn every_chip_shows_on_an_80_column_terminal() {
        let (mut b, _rx) = open(&[], "zebra");
        let look = Look::default();
        b.key(key(KeyCode::Enter), &look);
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
}
