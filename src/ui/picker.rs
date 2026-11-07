//! A filterable list, what fzf was in the popups (the palette, the pickers),
//! drawn as tmux/fzf-style.sh made fzf look: a prompt and what is typed at
//! the top, the count matched under it, the rows below, the chosen row on
//! the highlight with a yellow bold `›` and its text bright and bold, the
//! letters matched in colour81, and the keys dim at the bottom.
//!
//! Matching is nucleo-matcher's: fuzzy by default, or each word typed as it
//! is written (`exact`), as palette.sh ran fzf with `--exact`, so "shape"
//! doesn't match the letters scattered through "Close the pane". Rows that
//! match are shown best first, or in the order a list gives them
//! (`order`), as palette.sh ranked its rows itself on every keystroke.
//!
//! Each row has its text, tags (matched too, and dimmed) and a key (shown
//! in a column of its own at the end): the columns are fitted to the box,
//! a text or tags too long for theirs cut with an ellipsis, so every row's
//! tags and key line up.
//!
//! A list that takes what is typed as well as its rows (`free`, as fzf's
//! `--print-query` let pick-dir.zsh take a path typed in) gives back the
//! text typed when Enter finds no row matching it, or at once on Alt-Enter.
//!
//! A list may show the chosen row whole below the rows (`previews`, as
//! fzf's `--preview` window at the bottom showed wtn-open.zsh's item), when
//! a row is cut to one line of something longer.
//! A list may do a second thing with a row (`also`), as fzf's `--expect
//! ctrl-o` let link-view.sh open a link it would otherwise read: Ctrl-O
//! takes the row for that, and the keys row names both.
//!
//! Up and Down (C-p and C-n, C-k and C-j, Tab and Shift-Tab, the wheel)
//! move; Enter picks; Esc or C-c closes; Backspace, C-w and C-u edit what is
//! typed. A click moves to a row and a second click on it picks it, as fzf
//! does; a click outside closes the list; the row under the mouse is lit.

use std::time::{Duration, Instant};

use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::buffer::Buffer;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use unicode_segmentation::UnicodeSegmentation;

use super::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit, Place};
use super::{Look, Outcome, Overlay, Step};
use crate::mux::input::{Key, KeyCode, Mods, Mouse};

/// Two clicks on a row this close together pick it.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// The widest the tags and the key columns get.
const TAGS_MAX: usize = 24;
const KEY_MAX: usize = 10;
/// The narrowest the text gets before the tags give way (palette.sh's 30).
const TEXT_MIN: usize = 30;

/// One row of a list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PickItem {
    pub text: String,
    /// Words about it (`#panes #split`), dimmed and matched as any other.
    pub tags: String,
    /// A key that does the same, in a column of its own.
    pub key: String,
    /// What picking it gives back.
    pub value: String,
}

impl PickItem {
    pub fn new(text: impl Into<String>, value: impl Into<String>) -> PickItem {
        PickItem {
            text: text.into(),
            value: value.into(),
            ..PickItem::default()
        }
    }

    /// What is matched: the text, the tags and the key, a space between.
    pub fn haystack(&self) -> String {
        format!("{} {} {}", self.text, self.tags, self.key)
    }
}

/// An item picked: where it is in the list given, its value, and what was
/// typed before it was. From a `free` list, what was typed alone has the
/// index `TYPED` and the text typed as its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picked {
    pub index: usize,
    pub value: String,
    pub query: String,
}

/// Orders the rows that matched what is typed: given the query and the
/// matched items' indices (in the list's order), the order to show them in.
pub type Order = Box<dyn Fn(&str, &[usize]) -> Vec<usize> + Send>;

/// The index a `free` list gives what was typed, picked in place of a row.
pub const TYPED: usize = usize::MAX;

pub struct Picker {
    kind: &'static str,
    title: String,
    prompt: String,
    items: Vec<PickItem>,
    query: String,
    exact: bool,
    free: bool,
    order: Option<Order>,
    matcher: Matcher,
    /// What Enter does and what Ctrl-O does, for a list that does two
    /// things with a row, named on the keys row.
    also: Option<(String, String)>,
    /// The rows shown, in order: the item and the grapheme indices matched
    /// in its haystack.
    shown: Vec<(usize, Vec<u32>)>,
    cursor: usize,
    top: usize,
    hover: Option<usize>,
    last_click: Option<(Instant, usize)>,
    drawn: Option<Drawn>,
    /// How many rows the last draw had room for.
    room: usize,
    /// Each item's text whole, by its place in `items`, shown under the
    /// rows for the chosen one; none when empty.
    previews: Vec<String>,
    /// Lines said under the count, dimmed, as fzf's `--header`.
    header: Vec<String>,
}

impl Picker {
    pub fn new(title: impl Into<String>, items: Vec<PickItem>) -> Picker {
        let mut picker = Picker {
            kind: "picker",
            title: title.into(),
            prompt: "> ".into(),
            items,
            query: String::new(),
            exact: false,
            free: false,
            order: None,
            matcher: Matcher::new(Config::DEFAULT),
            also: None,
            shown: Vec::new(),
            cursor: 0,
            top: 0,
            hover: None,
            last_click: None,
            drawn: None,
            room: 10,
            previews: Vec::new(),
            header: Vec::new(),
        };
        picker.refilter();
        picker
    }

    /// The kind of overlay it is to the server ("palette" for M-p's).
    pub fn named(mut self, kind: &'static str) -> Picker {
        self.kind = kind;
        self
    }

    /// What the prompt says before what is typed.
    pub fn prompt(mut self, prompt: impl Into<String>) -> Picker {
        self.prompt = prompt.into();
        self
    }

    /// Each word typed matched as it is written, not as letters scattered
    /// through a row.
    pub fn exact(mut self) -> Picker {
        self.exact = true;
        self.refilter();
        self
    }

    /// What is typed taken too: Enter with no row matching it, or Alt-Enter
    /// whatever matches, gives it back (`TYPED`).
    pub fn free(mut self) -> Picker {
        self.free = true;
        self
    }

    /// A second thing done with a row, on Ctrl-O (`Outcome::PickedAlso`):
    /// `enter` says what Enter does and `other` what Ctrl-O does.
    pub fn also(mut self, enter: impl Into<String>, other: impl Into<String>) -> Picker {
        self.also = Some((enter.into(), other.into()));
        self
    }

    /// The rows matched shown in this order rather than best match first.
    pub fn order(mut self, order: Order) -> Picker {
        self.order = Some(order);
        self.refilter();
        self
    }

    /// The rows given anew while it is up, as a list filled in as its
    /// sources answer: what is typed is kept, and the row chosen stays
    /// chosen while it is there.
    pub fn set_items(&mut self, items: Vec<PickItem>) {
        let chosen = self.chosen().map(|i| i.value.clone());
        self.items = items;
        self.refilter();
        if let Some(value) = chosen
            && let Some(at) = self
                .shown
                .iter()
                .position(|(i, _)| self.items[*i].value == value)
        {
            self.cursor = at;
            if self.cursor >= self.top + self.room.max(1) {
                self.top = self.cursor + 1 - self.room.max(1);
            }
        }
    }

    /// Its title given anew while it is up.
    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
    }

    /// Each item's text whole, by its place in the items given, shown in
    /// the lower part of the box for the row chosen.
    pub fn previews(mut self, previews: Vec<String>) -> Picker {
        self.previews = previews;
        self
    }

    /// A line said under the count, dimmed, as fzf's `--header`.
    pub fn header(mut self, line: impl Into<String>) -> Picker {
        self.header.push(line.into());
        self
    }

    /// The box's line the first row is drawn on: after the prompt, the
    /// count and the header.
    fn first_row(&self) -> usize {
        2 + self.header.len()
    }

    /// Typed, as if by hand.
    pub fn query(mut self, query: &str) -> Picker {
        self.query = query.to_string();
        self.refilter();
        self
    }

    /// What is typed now.
    pub fn typed(&self) -> &str {
        &self.query
    }

    /// The rows shown, in order, by their text.
    pub fn shown(&self) -> Vec<&str> {
        self.shown
            .iter()
            .map(|(i, _)| self.items[*i].text.as_str())
            .collect()
    }

    /// The row chosen, by its text.
    pub fn chosen(&self) -> Option<&PickItem> {
        self.shown.get(self.cursor).map(|(i, _)| &self.items[*i])
    }

    fn refilter(&mut self) {
        let kind = if self.exact {
            AtomKind::Substring
        } else {
            AtomKind::Fuzzy
        };
        let pattern = Pattern::new(&self.query, CaseMatching::Smart, Normalization::Smart, kind);
        let mut buf = Vec::new();
        let mut found: Vec<(usize, u32, Vec<u32>)> = Vec::new();
        for (i, item) in self.items.iter().enumerate() {
            let hay = item.haystack();
            let mut indices = Vec::new();
            if let Some(score) = pattern.indices(
                Utf32Str::new(&hay, &mut buf),
                &mut self.matcher,
                &mut indices,
            ) {
                indices.sort_unstable();
                indices.dedup();
                found.push((i, score, indices));
            }
        }
        let order: Vec<usize> = match &self.order {
            Some(order) => order(
                &self.query,
                &found.iter().map(|(i, _, _)| *i).collect::<Vec<_>>(),
            ),
            None => {
                let mut by_score: Vec<_> = found.iter().map(|(i, s, _)| (*i, *s)).collect();
                by_score.sort_by_key(|(i, s)| (std::cmp::Reverse(*s), *i));
                by_score.into_iter().map(|(i, _)| i).collect()
            }
        };
        self.shown = order
            .into_iter()
            .filter_map(|i| {
                found
                    .iter()
                    .find(|(j, _, _)| *j == i)
                    .map(|(_, _, idx)| (i, idx.clone()))
            })
            .collect();
        self.cursor = 0;
        self.top = 0;
        self.hover = None;
    }

    fn pick(&self) -> Step {
        match self.shown.get(self.cursor) {
            Some((i, _)) => Step::Close(Outcome::Picked(Picked {
                index: *i,
                value: self.items[*i].value.clone(),
                query: self.query.clone(),
            })),
            None if self.free => self.take_typed(),
            None => Step::Close(Outcome::Dismissed),
        }
    }

    /// Ctrl-O: the row, or what is typed in a free list with no row
    /// matching it, taken for the second thing.
    fn pick_also(&self) -> Step {
        match self.pick() {
            Step::Close(Outcome::Picked(p)) => Step::Close(Outcome::PickedAlso(p)),
            other => other,
        }
    }

    /// What is typed, picked as it is; nothing typed closes the list.
    fn take_typed(&self) -> Step {
        if self.query.trim().is_empty() {
            return Step::Close(Outcome::Dismissed);
        }
        Step::Close(Outcome::Picked(Picked {
            index: TYPED,
            value: self.query.clone(),
            query: self.query.clone(),
        }))
    }

    fn move_to(&mut self, to: usize) -> Step {
        let to = to.min(self.shown.len().saturating_sub(1));
        if to == self.cursor {
            return Step::Same;
        }
        self.cursor = to;
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.cursor >= self.top + self.room.max(1) {
            self.top = self.cursor + 1 - self.room.max(1);
        }
        Step::Redraw
    }

    fn edit(&mut self, change: impl FnOnce(&mut String)) -> Step {
        let before = self.query.clone();
        change(&mut self.query);
        if self.query == before {
            return Step::Same;
        }
        self.refilter();
        Step::Redraw
    }

    /// The columns a row's parts get in a box `inner` wide: the text, the
    /// tags and the key, with a space between each. The tags give way first
    /// when the box is narrow.
    fn columns(&self, inner: usize) -> (usize, usize, usize) {
        let key_w = self
            .items
            .iter()
            .map(|i| draw::width(&i.key))
            .max()
            .unwrap_or(0)
            .min(KEY_MAX);
        let mut tags_w = self
            .items
            .iter()
            .map(|i| draw::width(&i.tags))
            .max()
            .unwrap_or(0)
            .min(TAGS_MAX);
        // The pointer's three columns, a space before the tags and the key,
        // and one at the right edge.
        let fixed = |tags_w: usize| {
            3 + usize::from(tags_w > 0) + tags_w + usize::from(key_w > 0) + key_w + 1
        };
        while tags_w > 0 && inner.saturating_sub(fixed(tags_w)) < TEXT_MIN {
            tags_w -= 1;
        }
        // Tags too narrow to read a word of are left out.
        if tags_w < 6 {
            tags_w = 0;
        }
        (inner.saturating_sub(fixed(tags_w)), tags_w, key_w)
    }

    fn row_line(&self, row: usize, widths: (usize, usize, usize), look: &Look) -> BoxLine {
        let (i, matched) = &self.shown[row];
        let item = &self.items[*i];
        let chosen = row == self.cursor;
        let (text_w, tags_w, key_w) = widths;
        let fill = if chosen {
            Some(look.theme.highlight)
        } else if self.hover == Some(row) {
            Some(look.theme.panel)
        } else {
            None
        };
        let base = if chosen {
            look.bold_bright()
        } else {
            look.text()
        };
        let hit = Style::new().fg(draw::EDGE);
        let hit = if chosen {
            hit.add_modifier(Modifier::BOLD)
        } else {
            hit
        };
        let mut spans: Vec<Span<'static>> = if chosen {
            vec![Span::raw(" "), look.pointer(), Span::raw(" ")]
        } else {
            vec![Span::raw("   ")]
        };
        // Each part cut to its column, its matched letters lit; the
        // haystack's graphemes run text, space, tags, space, key.
        let tags_at = item.text.graphemes(true).count() as u32 + 1;
        let key_at = tags_at + item.tags.graphemes(true).count() as u32 + 1;
        spans.extend(lit(&item.text, text_w, base, hit, matched, 0));
        if tags_w > 0 {
            spans.push(Span::raw(" "));
            spans.extend(lit(&item.tags, tags_w, look.faint(), hit, matched, tags_at));
        }
        if key_w > 0 {
            spans.push(Span::raw(" "));
            spans.extend(lit(&item.key, key_w, look.dim(), hit, matched, key_at));
        }
        BoxLine::spans(spans).fill(fill).clickable()
    }
}

/// A part of a row cut and padded to its column, the graphemes matched (by
/// their place in the haystack, the part starting at `at`) in `hit`'s style
/// and the rest in `style`.
fn lit(
    text: &str,
    cols: usize,
    style: Style,
    hit: Style,
    matched: &[u32],
    at: u32,
) -> Vec<Span<'static>> {
    let whole = text.graphemes(true).count();
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_lit = false;
    for (i, g) in draw::fit(text, cols).graphemes(true).enumerate() {
        let on = i < whole && matched.binary_search(&(at + i as u32)).is_ok();
        if on != run_lit && !run.is_empty() {
            spans.push(Span::styled(
                std::mem::take(&mut run),
                if run_lit { hit } else { style },
            ));
        }
        run_lit = on;
        run.push_str(g);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, if run_lit { hit } else { style }));
    }
    spans
}

impl Overlay for Picker {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        // The box's size first, for the room its rows have: the share of the
        // screen palette.sh's popup took, less the prompt, the count, and the
        // blank line and keys at the bottom.
        let area = buf.area;
        let outer_w = (area.width as usize * 60 / 100)
            .max(40)
            .min(area.width as usize);
        let outer_h = (area.height as usize * 60 / 100)
            .max(10)
            .min(area.height as usize);
        let inner = outer_w.saturating_sub(2);
        let rows = outer_h.saturating_sub(2 + self.first_row() + 2).max(1);
        // With previews, the lower half shows the chosen one, under a rule.
        let preview_rows = if self.previews.is_empty() {
            0
        } else {
            rows / 2
        };
        self.room = rows.saturating_sub(preview_rows).max(1);
        if self.cursor >= self.top + self.room {
            self.top = self.cursor + 1 - self.room;
        }
        let widths = self.columns(inner);
        let mut lines = vec![
            BoxLine::spans(vec![
                Span::styled(format!(" {}", self.prompt), Style::new().fg(draw::EDGE)),
                Span::styled(self.query.clone(), look.bright()),
                Span::styled("▏", look.dim()),
            ]),
            BoxLine::text(
                format!("   {}/{}", self.shown.len(), self.items.len()),
                look.dim(),
            ),
        ];
        for line in &self.header {
            lines.push(BoxLine::text(format!("   {line}"), look.dim()));
        }
        for row in self.top..(self.top + self.room).min(self.shown.len()) {
            lines.push(self.row_line(row, widths, look));
        }
        if preview_rows > 1 {
            while lines.len() < self.first_row() + self.room {
                lines.push(BoxLine::blank());
            }
            lines.push(BoxLine::text("─".repeat(inner), look.faint()));
            let whole = self
                .shown
                .get(self.cursor)
                .and_then(|(i, _)| self.previews.get(*i))
                .map(String::as_str)
                .unwrap_or("");
            let wrapped: Vec<String> = whole
                .lines()
                .flat_map(|l| {
                    let indent = l.len() - l.trim_start().len();
                    let pad = " ".repeat(indent + 1);
                    draw::wrap(l.trim_start(), inner.saturating_sub(indent + 2))
                        .into_iter()
                        .map(move |w| format!("{pad}{w}"))
                })
                .take(preview_rows - 1)
                .collect();
            for line in wrapped {
                lines.push(BoxLine::text(line, look.text()));
            }
        }
        let keys = match &self.also {
            Some((enter, other)) => {
                format!("↑↓ move  ↵ {enter}  ctrl-o {other}  esc close")
            }
            None if self.free => {
                "type to filter  ↑↓ move  ↵ choose  alt-↵ take what's typed  esc close".into()
            }
            None => "type to filter  ↑↓ move  ↵ choose  esc close".into(),
        };
        let mut spec = BoxSpec::new(&self.title, &lines);
        spec.keys = Some(&keys);
        spec.place = Place::Share {
            cols_pct: 60,
            rows_pct: 60,
        };
        // fzf's lists kept Esc alone, with no close button of their own.
        spec.closes = false;
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn key(&mut self, key: Key, _look: &Look) -> Step {
        let ctrl = key.mods == Mods::CTRL;
        let plain = key.mods == Mods::NONE || key.mods == Mods::SHIFT;
        match key.code {
            KeyCode::Enter if self.free && key.mods == Mods::ALT => self.take_typed(),
            KeyCode::Enter => self.pick(),
            KeyCode::Char('o') if ctrl && self.also.is_some() => self.pick_also(),
            KeyCode::Esc => Step::Close(Outcome::Dismissed),
            KeyCode::Char('c' | 'g') if ctrl => Step::Close(Outcome::Dismissed),
            KeyCode::Down | KeyCode::Tab => self.move_to(self.cursor + 1),
            KeyCode::Char('n' | 'j') if ctrl => self.move_to(self.cursor + 1),
            KeyCode::Up | KeyCode::BackTab => self.move_to(self.cursor.saturating_sub(1)),
            KeyCode::Char('p' | 'k') if ctrl => self.move_to(self.cursor.saturating_sub(1)),
            KeyCode::PageDown => self.move_to(self.cursor + self.room),
            KeyCode::PageUp => self.move_to(self.cursor.saturating_sub(self.room)),
            KeyCode::Backspace => self.edit(|q| {
                q.pop();
            }),
            KeyCode::Char('u') if ctrl => self.edit(String::clear),
            KeyCode::Char('w') if ctrl => self.edit(|q| {
                let kept = q.trim_end().rfind(' ').map_or(0, |at| at + 1);
                q.truncate(kept);
            }),
            KeyCode::Char(c) if plain => self.edit(|q| q.push(c)),
            _ => Step::Same,
        }
    }

    fn paste(&mut self, text: &str, _look: &Look) -> Step {
        let line = text.lines().next().unwrap_or("").to_string();
        self.edit(|q| q.push_str(&line))
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        let Some(drawn) = &self.drawn else {
            return Step::Same;
        };
        let hit = drawn.hit(mouse.col, mouse.row);
        let first = self.first_row();
        let row = match hit {
            Hit::Line(i, _) if i >= first && i < first + self.room => {
                Some(self.top + i - first).filter(|r| *r < self.shown.len())
            }

            _ => None,
        };
        match draw::gesture(&mouse) {
            Gesture::Click if look.mouse => {
                if hit == Hit::Outside {
                    return Step::Close(Outcome::Dismissed);
                }
                let Some(row) = row else {
                    return Step::Same;
                };
                let now = Instant::now();
                let again = self
                    .last_click
                    .is_some_and(|(at, r)| r == row && now.duration_since(at) < DOUBLE_CLICK);
                self.last_click = Some((now, row));
                self.cursor = row;
                if again { self.pick() } else { Step::Redraw }
            }
            Gesture::WheelDown => self.move_to(self.cursor + 1),
            Gesture::WheelUp => self.move_to(self.cursor.saturating_sub(1)),
            Gesture::Move if look.hover => {
                if row == self.hover {
                    Step::Same
                } else {
                    self.hover = row;
                    Step::Redraw
                }
            }
            _ => Step::Same,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::input::{MouseButton, MouseKind};
    use crate::ui::draw::tests::screen;
    use ratatui::layout::Rect;

    fn items() -> Vec<PickItem> {
        [
            "Split pane to the left",
            "Close the pane",
            "New tab",
            "Zoom the pane in or out",
        ]
        .iter()
        .enumerate()
        .map(|(i, t)| PickItem::new(*t, i.to_string()))
        .collect()
    }

    fn typed(p: &mut Picker, text: &str) {
        for c in text.chars() {
            p.key(Key::new(KeyCode::Char(c), Mods::NONE), &Look::default());
        }
    }

    #[test]
    fn typing_filters_fuzzily_and_enter_picks() {
        let mut p = Picker::new("Pick", items());
        assert_eq!(p.shown().len(), 4);
        typed(&mut p, "nwtb");
        assert_eq!(p.shown(), vec!["New tab"]);
        assert_eq!(
            p.key(Key::new(KeyCode::Enter, Mods::NONE), &Look::default()),
            Step::Close(Outcome::Picked(Picked {
                index: 2,
                value: "2".into(),
                query: "nwtb".into(),
            }))
        );
    }

    #[test]
    fn a_free_list_takes_what_is_typed_when_no_row_matches_or_on_alt_enter() {
        let enter = |p: &mut Picker, mods| p.key(Key::new(KeyCode::Enter, mods), &Look::default());
        let mut p = Picker::new("Pick", items()).exact().free();
        typed(&mut p, "~/elsewhere");
        assert!(p.shown().is_empty());
        assert_eq!(
            enter(&mut p, Mods::NONE),
            Step::Close(Outcome::Picked(Picked {
                index: TYPED,
                value: "~/elsewhere".into(),
                query: "~/elsewhere".into(),
            }))
        );
        let mut p = Picker::new("Pick", items()).exact().free();
        typed(&mut p, "pane");
        assert!(matches!(
            enter(&mut p, Mods::ALT),
            Step::Close(Outcome::Picked(Picked { index: TYPED, .. }))
        ));
        let mut empty = Picker::new("Pick", Vec::new()).free();
        assert_eq!(
            enter(&mut empty, Mods::NONE),
            Step::Close(Outcome::Dismissed),
            "nothing typed is nothing taken"
        );
        let mut strict = Picker::new("Pick", items());
        typed(&mut strict, "zzzz");
        assert_eq!(
            enter(&mut strict, Mods::NONE),
            Step::Close(Outcome::Dismissed)
        );
    }

    #[test]
    fn a_list_that_does_two_things_takes_the_second_on_ctrl_o_and_names_both() {
        let ctrl_o = Key::new(KeyCode::Char('o'), Mods::CTRL);
        let mut plain = Picker::new("Pick", items());
        assert_eq!(plain.key(ctrl_o, &Look::default()), Step::Same);
        let mut p = Picker::new("Links", items()).also("read it", "open in browser");
        typed(&mut p, "new");
        assert_eq!(
            p.key(ctrl_o, &Look::default()),
            Step::Close(Outcome::PickedAlso(Picked {
                index: 2,
                value: "2".into(),
                query: "new".into(),
            }))
        );
        let mut buf = Buffer::empty(Rect::new(0, 0, 160, 30));
        p.draw(&mut buf, &Look::default());
        let shown = screen(&buf);
        assert!(
            shown
                .iter()
                .any(|l| l.contains("↵ read it  ctrl-o open in browser")),
            "{shown:?}"
        );
    }

    #[test]
    fn exact_matches_words_as_written() {
        let mut p = Picker::new("Pick", items()).exact();
        typed(&mut p, "pane");
        assert_eq!(p.shown().len(), 3);
        p.key(Key::new(KeyCode::Char('u'), Mods::CTRL), &Look::default());
        typed(&mut p, "cpn");
        assert!(
            p.shown().is_empty(),
            "no scattered letters: {:?}",
            p.shown()
        );
        assert_eq!(
            p.key(Key::new(KeyCode::Enter, Mods::NONE), &Look::default()),
            Step::Close(Outcome::Dismissed)
        );
    }

    #[test]
    fn it_moves_and_draws_the_matched_letters() {
        let look = Look::default();
        let mut p = Picker::new("Actions", items()).prompt("action> ");
        p.key(Key::new(KeyCode::Down, Mods::NONE), &look);
        assert_eq!(p.chosen().unwrap().text, "Close the pane");
        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 30));
        p.draw(&mut buf, &look);
        let s = screen(&buf);
        assert!(s.iter().any(|l| l.contains("action> ▏")), "{s:#?}");
        assert!(s.iter().any(|l| l.contains("4/4")));
        assert!(s.iter().any(|l| l.contains(" › Close the pane")));
        typed(&mut p, "zoom");
        p.draw(&mut buf, &look);
        let s = screen(&buf);
        let y = s.iter().position(|l| l.contains("› Zoom")).unwrap() as u16;
        let x = s[y as usize].find("Zoom").unwrap() as u16;
        // The screen is ASCII up to there, so its bytes are its columns.
        let x = s[y as usize][..x as usize].chars().count() as u16;
        assert_eq!(buf[(x, y)].fg, draw::EDGE, "matched letters in colour81");
    }

    #[test]
    fn a_click_chooses_and_a_second_picks() {
        let look = Look::default();
        let mut p = Picker::new("Pick", items());
        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 30));
        p.draw(&mut buf, &look);
        let d = p.drawn.clone().unwrap();
        let row = d.rows[3].0;
        let click = Mouse {
            kind: MouseKind::Press,
            button: MouseButton::Left,
            col: d.inner.x + 5,
            row,
            mods: Mods::NONE,
        };
        assert_eq!(p.mouse(click, &look), Step::Redraw);
        assert_eq!(p.chosen().unwrap().text, "Close the pane");
        assert!(matches!(
            p.mouse(click, &look),
            Step::Close(Outcome::Picked(_))
        ));
        let outside = Mouse {
            col: 0,
            row: 0,
            ..click
        };
        assert_eq!(p.mouse(outside, &look), Step::Close(Outcome::Dismissed));
    }

    #[test]
    fn a_header_goes_under_the_count_and_clicks_still_find_their_rows() {
        let look = Look::default();
        let mut p = Picker::new("Pick", items()).header("Or type one of your own.");
        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 30));
        p.draw(&mut buf, &look);
        let s = screen(&buf);
        let count = s.iter().position(|l| l.contains("4/4")).unwrap();
        assert!(s[count + 1].contains("Or type one of your own."), "{s:#?}");
        let d = p.drawn.clone().unwrap();
        let click = Mouse {
            kind: MouseKind::Press,
            button: MouseButton::Left,
            col: d.inner.x + 5,
            row: d.rows[4].0,
            mods: Mods::NONE,
        };
        p.mouse(click, &look);
        assert_eq!(p.chosen().unwrap().text, "Close the pane");
    }

    #[test]
    fn the_chosen_row_shows_whole_below_the_rows() {
        let look = Look::default();
        let wholes = items()
            .iter()
            .map(|i| format!("{} in full\n  and what is under it", i.text))
            .collect();
        let mut p = Picker::new("Items", items()).previews(wholes);
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 30));
        p.draw(&mut buf, &look);
        let s = screen(&buf);
        assert!(
            s.iter()
                .any(|l| l.contains("Split pane to the left in full"))
        );
        assert!(s.iter().any(|l| l.contains("   and what is under it")));
        p.key(Key::new(KeyCode::Down, Mods::NONE), &look);
        p.draw(&mut buf, &look);
        let s = screen(&buf);
        assert!(s.iter().any(|l| l.contains("Close the pane in full")));
        assert!(
            !s.iter()
                .any(|l| l.contains("Split pane to the left in full"))
        );
    }

    #[test]
    fn a_free_list_gives_what_is_typed_and_takes_rows_while_up() {
        let look = Look::default();
        let mut p = Picker::new("Search", Vec::new()).free();
        typed(&mut p, "deploy");
        assert_eq!(
            p.key(Key::new(KeyCode::Enter, Mods::NONE), &look),
            Step::Close(Outcome::Picked(Picked {
                index: usize::MAX,
                value: "deploy".into(),
                query: "deploy".into(),
            }))
        );
        let mut p = Picker::new("Found", items());
        p.key(Key::new(KeyCode::Down, Mods::NONE), &look);
        assert_eq!(p.chosen().unwrap().text, "Close the pane");
        let mut more = vec![PickItem::new("Arrived first", "9")];
        more.extend(items());
        p.set_items(more);
        assert_eq!(p.shown().len(), 5);
        assert_eq!(
            p.chosen().unwrap().text,
            "Close the pane",
            "the row chosen stays chosen"
        );
    }
}
