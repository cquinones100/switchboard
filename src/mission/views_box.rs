//! The "Views at once" box (board 32 of the UX 2.0 canvas, new with the
//! port): "Several views at once…" in a view's `m` menu opens it to pick the views shown together
//! (`grid`). At the left, every view under its group, Work and Code in one
//! column and Day and System in the next, each ticked with its place when
//! picked; at the right, the layouts as keycaps and the one in use drawn
//! small, each picked view in its cell, the first's, which takes the
//! focus, in the accent. Space picks or unpicks the chosen view (up to
//! four), `J` and `K` move it a place later or earlier, 1 2 3 choose the
//! layout, Enter shows them and Esc goes back to one view. Each row and
//! keycap takes a click and lights under the mouse, and a click on a cell
//! of the drawing gives its view the first place.
//!
//! What Enter picked is handed back through `Picked`, which the server
//! reads as the box closes with `Outcome::Yes`.

use std::sync::{Arc, Mutex};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;

use super::grid::{self, Choice, Layout, MOST};
use super::views::Group;
use crate::mux::input::{Key, KeyCode, Mods, Mouse};
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit, width};
use crate::ui::kit::{self, Tone};
use crate::ui::{Look, Outcome, Overlay, Step, Theme};

/// The box's inside.
const INSIDE: u16 = 92;
/// A column of views.
const COL_W: usize = 22;
/// The layout's side.
const RIGHT_W: usize = 42;
/// Where the layout's side starts inside the box.
const RIGHT_X: usize = 1 + COL_W + 2 + COL_W + 3;
/// The layout drawn small.
const DRAW_W: usize = 40;
const DRAW_H: usize = 9;
const SUBTITLE: &str = "Pick up to four. The first is where the focus starts.";
const TOO_MANY: &str = "Up to four at once.";
pub const KEYS: &str =
    "↑↓ move  space pick  J K its place  1 2 3 layout  ↵ show them  esc one view";

/// A view the box lists: its group, its slug and its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub group: Group,
    pub slug: String,
    pub title: String,
}

/// What Enter picked, for the server to read once the box has closed.
pub type Picked = Arc<Mutex<Option<Choice>>>;

/// A line of the layout's side: its spans, and the targets on it, each
/// with its columns there.
type Part = (Vec<Span<'static>>, Vec<(u16, u16, Target)>);

/// What a click or the cursor is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Entry(usize),
    Layout(Layout),
    /// A cell of the drawing, by its place.
    Cell(usize),
    /// Easy mode's ×, a Tab stop after the layouts.
    Close,
}

pub struct ViewsBox {
    entries: Vec<Entry>,
    picked: Vec<String>,
    layout: Layout,
    /// The entries, then the layouts' keycaps.
    cursor: usize,
    hover: Option<Target>,
    too_many: bool,
    caps: bool,
    out: Picked,
    drawn: Option<Drawn>,
    /// Where each target was drawn: its line and its columns inside the box.
    targets: Vec<(usize, u16, u16, Target)>,
}

impl ViewsBox {
    /// The box over `entries`, starting from `choice` (the views it names
    /// that are there), the cursor on the first picked.
    pub fn new(entries: Vec<Entry>, choice: &Choice, caps: bool) -> (ViewsBox, Picked) {
        let picked: Vec<String> = choice
            .views
            .iter()
            .filter(|s| entries.iter().any(|e| e.slug == **s))
            .take(MOST)
            .cloned()
            .collect();
        let cursor = picked
            .first()
            .and_then(|s| entries.iter().position(|e| e.slug == *s))
            .unwrap_or(0);
        let out: Picked = Arc::default();
        (
            ViewsBox {
                entries,
                picked,
                layout: choice.layout,
                cursor,
                hover: None,
                too_many: false,
                caps,
                out: out.clone(),
                drawn: None,
                targets: Vec::new(),
            },
            out,
        )
    }

    /// The views picked, by slug, in their places.
    pub fn picked(&self) -> &[String] {
        &self.picked
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// The Tab stops: the views, the layouts, and with easy mode the ×.
    fn stops(&self, close: bool) -> usize {
        self.entries.len() + Layout::ALL.len() + usize::from(close)
    }

    fn at_cursor(&self) -> Target {
        match self.cursor.checked_sub(self.entries.len()) {
            None => Target::Entry(self.cursor),
            Some(l) if l < Layout::ALL.len() => Target::Layout(Layout::ALL[l]),
            Some(_) => Target::Close,
        }
    }

    /// Space on a view: picked, or unpicked; a fifth refused.
    fn toggle(&mut self, at: usize) {
        let Some(slug) = self.entries.get(at).map(|e| e.slug.clone()) else {
            return;
        };
        if let Some(p) = self.picked.iter().position(|s| *s == slug) {
            self.picked.remove(p);
        } else if self.picked.len() >= MOST {
            self.too_many = true;
        } else {
            self.picked.push(slug);
        }
    }

    /// `J` and `K`: the chosen view a place later or earlier.
    fn move_place(&mut self, by: isize) {
        let Target::Entry(at) = self.at_cursor() else {
            return;
        };
        let slug = &self.entries[at].slug;
        let Some(p) = self.picked.iter().position(|s| s == slug) else {
            return;
        };
        let to = p as isize + by;
        if to >= 0 && (to as usize) < self.picked.len() {
            self.picked.swap(p, to as usize);
        }
    }

    fn act(&mut self, target: Target) {
        match target {
            Target::Entry(at) => self.toggle(at),
            Target::Layout(l) => self.layout = l,
            Target::Cell(p) => {
                if p < self.picked.len() {
                    let slug = self.picked.remove(p);
                    self.picked.insert(0, slug);
                }
            }
            Target::Close => {}
        }
    }

    fn done(&self) -> Step {
        if let Ok(mut out) = self.out.lock() {
            *out = Some(Choice {
                views: self.picked.clone(),
                layout: self.layout,
            });
        }
        Step::Close(Outcome::Yes)
    }

    /// A view's row in its column: its tick, its name and its place.
    fn entry_row(&self, at: usize, theme: &Theme) -> Vec<Span<'static>> {
        let e = &self.entries[at];
        let place = self.picked.iter().position(|s| *s == e.slug);
        let (mark, tone) = match place {
            Some(_) => ("☑", Tone::Accent),
            None => ("☐", Tone::Dim),
        };
        let number = place.map(|p| (p + 1).to_string()).unwrap_or_default();
        let gap = COL_W.saturating_sub(4 + width(&e.title) + width(&number) + 1);
        let spans = vec![
            Span::styled(mark, tone.style(theme)),
            Span::raw(" "),
            Span::styled(
                e.title.clone(),
                if place.is_some() {
                    Tone::Text.bold(theme)
                } else {
                    Tone::Text.style(theme)
                },
            ),
            Span::raw(" ".repeat(gap.max(1))),
            Span::styled(number, Tone::Accent.bold(theme)),
        ];
        let lit = self.cursor == at || self.hover == Some(Target::Entry(at));
        kit::row(spans, COL_W, lit, theme)
    }

    /// A column of views: each group's label and its views.
    fn column(&self, groups: &[Group], theme: &Theme) -> Vec<(Vec<Span<'static>>, Option<Target>)> {
        let mut out = Vec::new();
        for g in groups {
            let held: Vec<usize> = (0..self.entries.len())
                .filter(|&i| self.entries[i].group == *g)
                .collect();
            if held.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push((Vec::new(), None));
            }
            out.push((kit::label(g.name(), Tone::Dim, None, COL_W, theme), None));
            for i in held {
                out.push((self.entry_row(i, theme), Some(Target::Entry(i))));
            }
        }
        out
    }

    /// The layout in use drawn small: each picked view's cell a box with
    /// its place and name, the first's in the accent.
    fn drawing(&self, theme: &Theme) -> Vec<Part> {
        let n = self.picked.len();
        let blank = (" ".to_string(), Style::new());
        let mut grid = vec![vec![blank.clone(); DRAW_W]; DRAW_H];
        let mut owner: Vec<Vec<Option<usize>>> = vec![vec![None; DRAW_W]; DRAW_H];
        let columns = (n > 0)
            .then(|| grid::split(self.layout, n, DRAW_W, DRAW_H, (8, 2)))
            .flatten();
        if let Some(columns) = columns {
            for (p, x, y, w, h) in grid::areas(&columns) {
                // The first's edge in the accent, another's bright under the
                // mouse, which a click makes the first.
                let style = if p == 0 {
                    Tone::Accent.style(theme)
                } else if self.hover == Some(Target::Cell(p)) {
                    Tone::Text.bold(theme)
                } else {
                    Tone::Dim.style(theme)
                };
                if w < 2 || h < 2 {
                    continue;
                }
                for r in y..y + h {
                    for c in x..x + w {
                        let ch = match (r == y, r == y + h - 1, c == x, c == x + w - 1) {
                            (true, _, true, _) => "╭",
                            (true, _, _, true) => "╮",
                            (_, true, true, _) => "╰",
                            (_, true, _, true) => "╯",
                            (true, _, _, _) | (_, true, _, _) => "─",
                            (_, _, true, _) | (_, _, _, true) => "│",
                            _ => " ",
                        };
                        grid[r][c] = (ch.to_string(), style);
                        owner[r][c] = Some(p);
                    }
                }
                let name = self
                    .entries
                    .iter()
                    .find(|e| e.slug == self.picked[p])
                    .map_or("", |e| e.title.as_str());
                let text = draw::cut(&format!("{} {name}", p + 1), w.saturating_sub(4));
                let text_style = if p == 0 {
                    Tone::Accent.bold(theme)
                } else {
                    Tone::Text.style(theme)
                };
                let mid = y + h / 2;
                for (k, ch) in text.chars().enumerate() {
                    if x + 2 + k < x + w - 1 {
                        grid[mid][x + 2 + k] = (ch.to_string(), text_style);
                    }
                }
            }
        }
        grid.into_iter()
            .zip(owner)
            .map(|(row, owners)| {
                let mut spans: Vec<Span<'static>> = Vec::new();
                for (text, style) in row {
                    match spans.last_mut() {
                        Some(last) if last.style == style => last.content.to_mut().push_str(&text),
                        _ => spans.push(Span::styled(text, style)),
                    }
                }
                let mut hits: Vec<(u16, u16, Target)> = Vec::new();
                for (c, o) in owners.iter().enumerate() {
                    let Some(p) = o else { continue };
                    match hits.last_mut() {
                        Some((_, to, Target::Cell(q))) if q == p && *to as usize + 1 == c => {
                            *to = c as u16
                        }
                        _ => hits.push((c as u16, c as u16, Target::Cell(*p))),
                    }
                }
                (spans, hits)
            })
            .collect()
    }

    /// The layout's side: its label, the keycaps, the drawing and the words
    /// under it.
    fn right(&self, theme: &Theme) -> Vec<Part> {
        let mut out = vec![
            (
                kit::label("Layout", Tone::Dim, None, RIGHT_W, theme),
                Vec::new(),
            ),
            (Vec::new(), Vec::new()),
        ];
        let mut caps = Vec::new();
        let mut caps_hits = Vec::new();
        for (i, l) in Layout::ALL.into_iter().enumerate() {
            let lit = l == self.layout
                || self.cursor == self.entries.len() + i
                || self.hover == Some(Target::Layout(l));
            let from = caps.iter().map(|s: &Span| width(&s.content)).sum::<usize>();
            caps.extend(kit::keycap(
                &l.key().to_string(),
                l.name(),
                lit,
                self.caps,
                theme,
            ));
            let to = caps.iter().map(|s: &Span| width(&s.content)).sum::<usize>();
            caps_hits.push((from as u16, to.saturating_sub(1) as u16, Target::Layout(l)));
        }
        out.push((caps, caps_hits));
        out.push((Vec::new(), Vec::new()));
        out.extend(self.drawing(theme));
        out.push((Vec::new(), Vec::new()));
        out.push((
            vec![Span::styled(
                "the sidebar stays; Tab moves the focus between them",
                Tone::Faint.style(theme),
            )],
            Vec::new(),
        ));
        out.push((
            vec![Span::styled("Kept for next time.", Tone::Dim.style(theme))],
            Vec::new(),
        ));
        out
    }

    /// The box's lines, `room` of them shown: past that the views' columns
    /// scroll to keep the chosen view in sight, the layout's side staying.
    fn lines(&mut self, theme: &Theme, room: usize) -> Vec<BoxLine> {
        self.targets.clear();
        let inside = INSIDE as usize;
        let top = if self.too_many {
            Span::styled(TOO_MANY, Tone::Needs.bold(theme))
        } else {
            Span::styled(SUBTITLE, Tone::Dim.style(theme))
        };
        let mut out = vec![BoxLine::spans(vec![Span::raw(" "), top]), BoxLine::blank()];
        let left = self.column(&[Group::Work, Group::Code], theme);
        let middle = self.column(&[Group::Day, Group::System], theme);
        let right = self.right(theme);
        // The title's two lines and the rule under the columns.
        let body = room.saturating_sub(3).max(1);
        let chosen_row = [&left, &middle].iter().find_map(|column| {
            column
                .iter()
                .position(|(_, t)| *t == Some(Target::Entry(self.cursor)))
        });
        let first = chosen_row.map_or(0, |r| (r + 1).saturating_sub(body));
        let views = left.len().max(middle.len()).saturating_sub(first);
        let rows = views.max(right.len()).min(body.max(right.len()));
        for r in 0..rows {
            let line = out.len();
            let mut spans = vec![Span::raw(" ")];
            for (x, column) in [(1, &left), (1 + COL_W + 2, &middle)] {
                let (cell, target) = column.get(r + first).cloned().unwrap_or_default();
                kit::place(&mut spans, cell, (x, COL_W));
                if let Some(t) = target {
                    self.targets
                        .push((line, x as u16, (x + COL_W - 1) as u16, t));
                }
            }
            spans.push(Span::raw(" "));
            spans.push(kit::rule_v(theme));
            spans.push(Span::raw(" "));
            let (cell, hits) = right.get(r).cloned().unwrap_or_default();
            spans.extend(kit::fit(cell, RIGHT_W, Style::new()));
            for (from, to, t) in hits {
                self.targets.push((
                    line,
                    (RIGHT_X + from as usize) as u16,
                    (RIGHT_X + to as usize) as u16,
                    t,
                ));
            }
            out.push(BoxLine::spans(kit::fit(spans, inside, Style::new())).clickable());
        }
        out.push(BoxLine::spans(vec![kit::rule(inside, theme)]));
        out
    }

    fn target_at(&self, line: usize, col: u16) -> Option<Target> {
        self.targets
            .iter()
            .find(|(l, from, to, _)| *l == line && (*from..=*to).contains(&col))
            .map(|(.., t)| *t)
    }
}

impl Overlay for ViewsBox {
    fn drawn(&self) -> Option<&crate::ui::draw::Drawn> {
        self.drawn.as_ref()
    }

    fn kind(&self) -> &'static str {
        "views-at-once"
    }

    fn stands_out(&self) -> Option<Rect> {
        self.drawn.as_ref().map(|d| d.outer)
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        // draw_box shows the lines a screen's rows less six hold.
        let room = (buf.area.height as usize).saturating_sub(6);
        let lines = self.lines(&look.theme, room);
        let mut spec = BoxSpec::new("Views at once", &lines);
        spec.width = INSIDE;
        spec.keys = Some(KEYS);
        spec.close_lit = self.at_cursor() == Target::Close || self.hover == Some(Target::Close);
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn key(&mut self, key: Key, look: &Look) -> Step {
        self.too_many = false;
        let alone = key.mods == Mods::NONE || key.mods == Mods::SHIFT;
        if !alone {
            return Step::Same;
        }
        let entries = self.entries.len().max(1);
        match key.code {
            KeyCode::Esc => return Step::Close(Outcome::Dismissed),
            KeyCode::Enter | KeyCode::Char(' ') if self.at_cursor() == Target::Close => {
                return Step::Close(Outcome::Dismissed);
            }
            KeyCode::Enter => return self.done(),
            KeyCode::Up | KeyCode::Char('k') => {
                self.cursor = (self.cursor.min(entries - 1) + entries - 1) % entries
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.cursor = (self.cursor.min(entries - 1) + 1) % entries
            }
            KeyCode::Tab if key.mods == Mods::NONE => {
                self.cursor = (self.cursor + 1) % self.stops(look.close)
            }
            KeyCode::Tab | KeyCode::BackTab => {
                let stops = self.stops(look.close);
                self.cursor = (self.cursor + stops - 1) % stops
            }
            KeyCode::Char(' ') => self.act(self.at_cursor()),
            KeyCode::Char('J') => self.move_place(1),
            KeyCode::Char('K') => self.move_place(-1),
            KeyCode::Char(c) => match Layout::of_key(c) {
                Some(l) => self.layout = l,
                None => return Step::Same,
            },
            _ => return Step::Same,
        }
        Step::Redraw
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        let Some(hit) = self.drawn.as_ref().map(|d| d.hit(mouse.col, mouse.row)) else {
            return Step::Same;
        };
        let target = match hit {
            Hit::Line(line, col) => self.target_at(line, col),
            Hit::Close => Some(Target::Close),
            _ => None,
        };
        match draw::gesture(&mouse) {
            Gesture::Move if look.hover && look.mouse => {
                if target == self.hover {
                    return Step::Same;
                }
                self.hover = target;
                Step::Redraw
            }
            Gesture::Click if look.mouse => match hit {
                Hit::Close => Step::Close(Outcome::Dismissed),
                Hit::Outside => Step::Close(Outcome::Dismissed),
                _ => {
                    let Some(t) = target else {
                        return Step::Same;
                    };
                    self.too_many = false;
                    if let Target::Entry(i) = t {
                        self.cursor = i;
                    }
                    self.act(t);
                    Step::Redraw
                }
            },
            _ => Step::Same,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::draw::tests::screen;

    fn entries() -> Vec<Entry> {
        let e = |group, slug: &str, title: &str| Entry {
            group,
            slug: slug.into(),
            title: title.into(),
        };
        vec![
            e(Group::Work, "tabs", "Tabs"),
            e(Group::Work, "landing", "Landing"),
            e(Group::Code, "prs", "PRs"),
            e(Group::Code, "ci", "CI"),
            e(Group::Day, "calendar", "Calendar"),
            e(Group::Day, "slack", "Slack"),
            e(Group::Day, "inbox", "Inbox"),
            e(Group::System, "usage", "Usage"),
            e(Group::System, "resources", "Resources"),
        ]
    }

    fn drawn(b: &mut ViewsBox) -> Vec<String> {
        let mut buf = Buffer::empty(Rect::new(0, 0, 110, 30));
        b.draw(&mut buf, &Look::default());
        screen(&buf)
    }

    fn press(b: &mut ViewsBox, c: char) -> Step {
        b.key(Key::new(KeyCode::Char(c), Mods::NONE), &Look::default())
    }

    #[test]
    fn the_box_lists_the_groups_in_two_columns_beside_the_layout() {
        let (mut b, _) = ViewsBox::new(entries(), &Choice::default(), false);
        let screen = drawn(&mut b).join("\n");
        assert!(screen.contains("Views at once"), "{screen}");
        assert!(screen.contains(SUBTITLE), "{screen}");
        let row_of = |text: &str| {
            screen
                .lines()
                .find(|l| l.contains(text))
                .unwrap_or_else(|| panic!("no {text} in\n{screen}"))
                .to_string()
        };
        // Work and Code in the first column, Day and System beside them.
        assert!(row_of("WORK").contains("DAY"), "{screen}");
        assert!(row_of("Tabs").contains("☐ Tabs"), "{screen}");
        // Picked views ticked with their places, board 11's four.
        assert!(row_of("☑ Slack").trim_end().contains('1'), "{screen}");
        assert!(row_of("☑ Calendar").contains('4'), "{screen}");
        // The layouts, and the picked views' cells drawn small.
        assert!(
            screen.contains("2 × 2") && screen.contains("1 + 2"),
            "{screen}"
        );
        assert!(
            screen.contains("1 Slack") && screen.contains("4 Calendar"),
            "{screen}"
        );
        assert!(screen.contains("Kept for next time."), "{screen}");
        // A short screen scrolls the views to the chosen one.
        b.cursor = entries().len() - 1;
        let mut buf = Buffer::empty(Rect::new(0, 0, 110, 16));
        b.draw(&mut buf, &Look::default());
        let short = crate::ui::draw::tests::screen(&buf).join("\n");
        assert!(short.contains("Resources"), "{short}");
        // With nothing picked it draws no cells.
        let none = Choice {
            views: Vec::new(),
            layout: Layout::Grid,
        };
        let (mut b, _) = ViewsBox::new(entries(), &none, false);
        assert!(drawn(&mut b).join("\n").contains("☐ Slack"));
    }

    #[test]
    fn with_easy_mode_tab_reaches_the_close_button_and_enter_closes() {
        let (mut b, _) = ViewsBox::new(entries(), &Choice::default(), false);
        let look = Look {
            close: true,
            ..Look::default()
        };
        b.cursor = entries().len() + Layout::ALL.len() - 1;
        b.key(Key::new(KeyCode::Tab, Mods::NONE), &look);
        assert_eq!(b.at_cursor(), Target::Close);
        assert_eq!(
            b.key(Key::new(KeyCode::Enter, Mods::NONE), &look),
            Step::Close(Outcome::Dismissed)
        );
    }

    #[test]
    fn space_picks_up_to_four_and_j_and_k_move_a_view_s_place() {
        let (mut b, out) = ViewsBox::new(entries(), &Choice::default(), false);
        // The cursor starts on Slack, the first picked.
        b.cursor = 0;
        press(&mut b, ' ');
        assert!(b.too_many);
        assert!(drawn(&mut b).join("\n").contains(TOO_MANY));
        assert_eq!(b.picked().len(), 4);
        // Unpick Slack, pick Tabs: it goes last.
        b.cursor = 5;
        press(&mut b, ' ');
        b.cursor = 0;
        press(&mut b, ' ');
        assert_eq!(b.picked(), ["inbox", "usage", "calendar", "tabs"]);
        press(&mut b, 'K');
        press(&mut b, 'K');
        assert_eq!(b.picked(), ["inbox", "tabs", "usage", "calendar"]);
        press(&mut b, 'J');
        assert_eq!(b.picked(), ["inbox", "usage", "tabs", "calendar"]);
        // 2 draws the layout as one beside two stacked.
        press(&mut b, '2');
        assert_eq!(b.layout(), Layout::OneTwo);
        let screen = drawn(&mut b);
        let first = screen.iter().position(|l| l.contains("1 Inbox")).unwrap();
        let second = screen.iter().position(|l| l.contains("2 Usage")).unwrap();
        assert!(
            second < first,
            "the first spans the height, the rest stacked:\n{}",
            screen.join("\n")
        );
        assert_eq!(
            b.key(Key::new(KeyCode::Enter, Mods::NONE), &Look::default()),
            Step::Close(Outcome::Yes)
        );
        assert_eq!(
            out.lock().unwrap().clone(),
            Some(Choice {
                views: ["inbox", "usage", "tabs", "calendar"]
                    .map(String::from)
                    .to_vec(),
                layout: Layout::OneTwo,
            })
        );
    }
}
