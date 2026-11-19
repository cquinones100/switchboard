//! The story's map (boards 121 to 125, 134 and 135), a view of the story
//! told in chapters beside the Story view (reading.rs) and the files.
//!
//! The chapters in a strip across the top, each with how many of its pages
//! are read; under it a box a page, the goal at the top, an arrow from each
//! to the pages it builds on, as the model gave them, the pages read marked.
//! The boxes are laid out by the Mermaid layout the Markdown views draw
//! flowcharts with (`switchboard_ui::mermaid`), which gives the drawing as
//! text: each box is found in it by its first line, so the cursor can move
//! box to box and a click can choose one. The layout is worked out as the
//! screen is drawn, from the story alone, reading and starting nothing.
//!
//! `j` and `k` move down a level to what a page builds on and up to what
//! needs it, `h` and `l` across one, `]` and `[` show the next chapter and
//! the one before, Tab gives the strip the keys, and Enter (or a second
//! click) reads the chosen page in the Story view's column.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as Spans, Span};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::reading::{Hit, Mode};
use super::select::Pane;
use super::{Act, App, Focus};
use crate::story::{self, Kind, Story};
use switchboard_ui::draw::{cut, put_line, width, wrap};
use switchboard_ui::kit::{self, Surface, Tone};
use switchboard_ui::{Theme, mermaid};

/// The map's own state: the chapter shown, the page chosen on it, and
/// where the chapters' strip has the keys (a chapter, or past the last the
/// chip of the files no page tells; `None` while the map has them).
#[derive(Clone, Debug, Default)]
pub(super) struct Mapper {
    pub(super) chapter: usize,
    pub(super) at: usize,
    pub(super) strip: Option<usize>,
}

/// `n` of something, by its name: "1 file", "3 files".
fn counted(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// How a box of a map is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct BoxLook {
    chosen: bool,
    lit: bool,
    read: bool,
}

/// A map laid out: the drawing's lines, a character a cell, and where each
/// box is in it, by the number it was given.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Laid {
    lines: Vec<Vec<char>>,
    boxes: Vec<(usize, Rect)>,
    width: usize,
}

impl Laid {
    /// Lays out a Mermaid flowchart, and finds each box by its label: the
    /// number it goes by and its lines, the first of which no other box
    /// starts with.
    pub(super) fn lay(source: &str, labels: &[(usize, Vec<String>)]) -> Option<Laid> {
        let drawn = mermaid::draw(source, usize::MAX)?;
        let lines: Vec<Vec<char>> = drawn.lines().map(|l| l.chars().collect()).collect();
        let mut boxes: Vec<(usize, Rect)> = Vec::new();
        for (y, line) in lines.iter().enumerate() {
            let edges: Vec<usize> = (0..line.len()).filter(|&x| line[x] == '│').collect();
            for pair in edges.windows(2) {
                let inside: String = line[pair[0] + 1..pair[1]].iter().collect();
                let inside = inside.trim();
                let found = labels.iter().find(|(id, label)| {
                    label.first().is_some_and(|first| first.trim() == inside)
                        && !boxes.iter().any(|(seen, _)| seen == id)
                });
                if let (Some((id, label)), true) = (found, y > 0 && !inside.is_empty()) {
                    boxes.push((
                        *id,
                        Rect::new(
                            pair[0] as u16,
                            y as u16 - 1,
                            (pair[1] - pair[0] + 1) as u16,
                            label.len() as u16 + 2,
                        ),
                    ));
                }
            }
        }
        let width = lines.iter().map(Vec::len).max().unwrap_or(0);
        Some(Laid {
            lines,
            boxes,
            width,
        })
    }

    fn rect(&self, id: usize) -> Option<Rect> {
        self.boxes.iter().find(|(i, _)| *i == id).map(|(_, r)| *r)
    }

    /// The part of the drawing shown in `w` columns and `h` rows: its first
    /// column and row, kept round box `around` when the drawing is larger,
    /// and the columns left of it when it is narrower, so it sits in the
    /// middle.
    fn window(&self, w: usize, h: usize, around: Option<usize>) -> (usize, usize, usize) {
        let b = around.and_then(|id| self.rect(id));
        let start = |len: usize, room: usize, at: Option<(usize, usize)>| {
            if len <= room {
                return 0;
            }
            at.map_or(0, |(from, size)| {
                (from + size / 2).saturating_sub(room / 2).min(len - room)
            })
        };
        let left = start(self.width, w, b.map(|r| (r.x as usize, r.width as usize)));
        let top = start(
            self.lines.len(),
            h,
            b.map(|r| (r.y as usize, r.height as usize)),
        );
        (left, top, w.saturating_sub(self.width) / 2)
    }

    /// The rows the drawing takes in `h`.
    fn height(&self, h: usize) -> usize {
        self.lines.len().min(h)
    }

    /// The drawing's rows as shown in `w` by `h`, each box in its look: its
    /// edge the accent when chosen and bright under the mouse, its first
    /// line bold when chosen and dim when read, its other lines dim; an
    /// arrow dim, a dotted one in the needs tone.
    fn rows(
        &self,
        w: usize,
        h: usize,
        around: Option<usize>,
        look: &dyn Fn(usize) -> BoxLook,
        theme: &Theme,
    ) -> Vec<Vec<Span<'static>>> {
        let (left, top, pad) = self.window(w, h, around);
        let looks: Vec<(Rect, BoxLook)> =
            self.boxes.iter().map(|(id, r)| (*r, look(*id))).collect();
        let mut out = Vec::new();
        for (y, line) in self.lines.iter().enumerate().skip(top).take(h) {
            let mut spans: Vec<Span<'static>> = vec![Span::raw(" ".repeat(pad))];
            let mut run = String::new();
            let mut run_style = Style::new();
            for (x, &c) in line.iter().enumerate().skip(left).take(w) {
                let inside = looks.iter().find(|(r, _)| {
                    x >= r.x as usize
                        && x < (r.x + r.width) as usize
                        && y >= r.y as usize
                        && y < (r.y + r.height) as usize
                });
                let style = match inside {
                    Some((r, look)) => {
                        let edge = x == r.x as usize
                            || x + 1 == (r.x + r.width) as usize
                            || y == r.y as usize
                            || y + 1 == (r.y + r.height) as usize;
                        if edge && look.chosen {
                            Tone::Accent.style(theme)
                        } else if edge && look.lit {
                            Style::new().fg(theme.bright)
                        } else if edge {
                            Style::new().fg(theme.faint)
                        } else if c == '✓' {
                            Tone::Done.style(theme)
                        } else if y != r.y as usize + 1 {
                            Tone::Dim.style(theme)
                        } else if look.chosen {
                            Style::new().fg(theme.bright).add_modifier(Modifier::BOLD)
                        } else if look.read {
                            Tone::Dim.style(theme)
                        } else {
                            Tone::Text.style(theme)
                        }
                    }
                    None if matches!(c, '┄' | '┆') => Tone::Needs.style(theme),
                    None => Tone::Dim.style(theme),
                };
                if style != run_style && !run.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut run), run_style));
                }
                run_style = style;
                run.push(c);
            }
            if !run.is_empty() {
                spans.push(Span::styled(run, run_style));
            }
            out.push(spans);
        }
        out
    }

    /// Where each box is on the screen, the drawing shown in `w` by `h`
    /// from `(x0, y0)`: the part of it in sight.
    fn hits(
        &self,
        (x0, y0): (u16, u16),
        w: usize,
        h: usize,
        around: Option<usize>,
    ) -> Vec<(usize, Rect)> {
        let (left, top, pad) = self.window(w, h, around);
        let h = self.height(h);
        self.boxes
            .iter()
            .filter_map(|(id, r)| {
                let (x, y) = (r.x as usize, r.y as usize);
                let (right, bottom) = (x + r.width as usize, y + r.height as usize);
                let (x, right) = (x.max(left), right.min(left + w));
                let (y, bottom) = (y.max(top), bottom.min(top + h));
                (x < right && y < bottom).then(|| {
                    (
                        *id,
                        Rect::new(
                            x0 + (pad + x - left) as u16,
                            y0 + (y - top) as u16,
                            (right - x) as u16,
                            (bottom - y) as u16,
                        ),
                    )
                })
            })
            .collect()
    }
}

impl App {
    /// Whether the map itself is on the screen.
    pub(super) fn on_map(&self) -> bool {
        self.mapped() && self.mode == Mode::Map
    }

    /// The chapter whose map is shown, kept among those the story has.
    fn map_chapter(&self) -> usize {
        let n = self.story.as_ref().map_or(0, |s| s.chapters.len());
        self.mapper.chapter.min(n.saturating_sub(1))
    }

    /// `g m` and Map in the switch and the menu: the story's map, on the
    /// page the Story view is reading.
    pub(super) fn open_map_view(&mut self) {
        if !self.mapped() {
            return self.no_map();
        }
        if self.mode == Mode::Files {
            self.files_place = Some(self.place());
        }
        let section = self.section();
        if let Some(story) = self.told_map()
            && let Some(c) = story::chapter_of(story, section)
        {
            self.mapper.chapter = c;
            self.mapper.at = section;
        } else {
            self.mapper.at = self.goal_of(self.map_chapter());
        }
        self.mode = Mode::Map;
        self.keep_mode();
        self.filtering = false;
        self.path_filter.clear();
        self.list_visual = None;
        self.visual = None;
        self.focus = Focus::List;
        if self.alone.take().is_some() {
            self.whole_at = None;
            self.lines.clear();
            self.sort();
            self.show_at(0);
        }
    }

    /// A page of the map read: the Story view's column at it, with the
    /// keys.
    pub(super) fn read_page(&mut self, n: usize) {
        self.mapper.strip = None;
        self.open_story_view();
        self.go_to_section(n);
        self.focus = Focus::New;
        self.read_if_reading();
    }

    /// The files page `n` has a hunk in, a hunk repeated there among
    /// them, and its hunks.
    fn page_counts(&self, story: &Story, n: usize) -> (usize, usize) {
        let files = story.pages.get(n - 1).map_or(0, |c| c.touches.len());
        let own = story
            .hunks
            .values()
            .flatten()
            .filter(|h| h.page == n)
            .count();
        let again = story
            .repeats
            .values()
            .filter(|shown| shown.contains(&n))
            .count();
        (files, own + again)
    }

    fn goal_of(&self, chapter: usize) -> usize {
        self.story
            .as_ref()
            .and_then(|s| s.chapters.get(chapter))
            .and_then(|t| t.goal.or(t.pages.first().copied()))
            .unwrap_or(1)
    }

    /// The wheel over the map: the chapter read from the goal down, the
    /// next page or the one before, chosen on the map.
    pub(super) fn map_step(&mut self, step: i32) {
        let Some(story) = self.told_map() else {
            return;
        };
        let order = story::goal_down(story, self.map_chapter());
        let here = self.mapper.at;
        let to = match order.iter().position(|&n| n == here) {
            Some(i) => i as i32 + step,
            None => 0,
        };
        let Some(&n) = usize::try_from(to).ok().and_then(|i| order.get(i)) else {
            return;
        };
        self.mapper.at = n;
    }

    /// ] and [ on the map, and ]c and [c from an open page's list: the
    /// next chapter's map, or the one before.
    pub(super) fn map_chapter_step(&mut self, step: i32) {
        let n = self.story.as_ref().map_or(0, |s| s.chapters.len()) as i32;
        if !self.mapped() || n < 2 {
            return;
        }
        self.show_chapter((self.map_chapter() as i32 + step).rem_euclid(n) as usize);
    }

    fn show_chapter(&mut self, chapter: usize) {
        if !self.mapped() {
            return;
        }
        if self.mode != Mode::Map {
            self.open_map_view();
        }
        self.mapper.chapter = chapter;
        self.mapper.at = self.goal_of(chapter);
        // The strip, with the keys on a chapter, follows the one shown.
        let chapters = self.story.as_ref().map_or(0, |s| s.chapters.len());
        if self.mapper.strip.is_some_and(|s| s < chapters) {
            self.mapper.strip = Some(chapter);
        }
    }

    /// A key while the chapters' strip has the keys, as mission control's
    /// sections are one stop of Tab: h and l (and the arrows) move between
    /// the chapters, showing each, and on to the chip of the files no page
    /// tells; Enter on the chip opens Files mode on them; Tab, Enter on a
    /// chapter, j, the down arrow and Esc give the keys to the map.
    fn strip_key(&mut self, key: KeyEvent) -> Option<Act> {
        let at = self.mapper.strip?;
        let story = self.told_map()?;
        let chapters = story.chapters.len();
        let last = if self.files_only(story).1 > 0 {
            chapters
        } else {
            chapters.saturating_sub(1)
        };
        let to = |app: &mut App, to: usize| {
            app.mapper.strip = Some(to);
            if to < chapters && to != app.map_chapter() {
                app.show_chapter(to);
            }
        };
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => to(self, at.saturating_sub(1)),
            KeyCode::Char('l') | KeyCode::Right => to(self, (at + 1).min(last)),
            KeyCode::Enter | KeyCode::Char('o' | ' ') if at >= chapters => {
                self.open_files_only();
            }
            KeyCode::Enter
            | KeyCode::Char('o' | ' ' | 'j')
            | KeyCode::Down
            | KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Esc => self.mapper.strip = None,
            // What the diff takes anywhere goes on to the map's keys.
            KeyCode::Char('q' | 'y' | 'g' | ']' | '[' | 'm' | 'R' | 'c' | 'E') | KeyCode::F(10) => {
                return None;
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => return None,
            _ => return Some(Act::Same),
        }
        Some(Act::Redraw)
    }

    /// A key on the map: the cursor moves box to box along the arrows (j
    /// and k down and up a level, h and l across one), Enter opens the
    /// chosen page, ] and [ show the next chapter and the one before, Tab
    /// gives the keys to the chapters' strip. `None` for
    /// a key the diff takes anywhere.
    pub(super) fn map_key(&mut self, key: KeyEvent) -> Option<Act> {
        // A c or p typed after ] or [ from the habit of ]c and ]p adds
        // nothing (c would open the commits).
        let after_bracket = std::mem::take(&mut self.reader.after_bracket);
        let plain = !key.modifiers.contains(KeyModifiers::CONTROL);
        if after_bracket && plain && matches!(key.code, KeyCode::Char('c' | 'p')) {
            return Some(Act::Same);
        }
        if let Some(act) = self.strip_key(key) {
            return Some(act);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let story = self.told_map()?;
        let chapter = self.map_chapter();
        let levels = story::levels(story, chapter);
        let at = self.mapper.at;
        let row = levels.iter().position(|l| l.contains(&at));
        // Across a level by where the boxes are drawn, which the layout
        // orders by what each connects to.
        let laid = self.lay_map(story, chapter);
        let across = |step: i32| -> Option<usize> {
            let mut level: Vec<usize> = levels[row?].clone();
            if let Some(laid) = &laid {
                level.sort_by_key(|&n| laid.rect(n).map_or(u16::MAX, |r| r.x));
            }
            let i = level.iter().position(|&n| n == at)? as i32 + step;
            usize::try_from(i).ok().and_then(|i| level.get(i)).copied()
        };
        let to = match key.code {
            // To the nearest level linked that way, the first there.
            KeyCode::Char('j') | KeyCode::Down if !ctrl => {
                let level_of = |n: usize| levels.iter().position(|l| l.contains(&n));
                story::builds_on(story, at)
                    .into_iter()
                    .map(|(n, _)| n)
                    .filter(|&n| level_of(n).is_some())
                    .min_by_key(|&n| level_of(n))
            }
            KeyCode::Char('k') | KeyCode::Up if !ctrl => {
                let level_of = |n: usize| levels.iter().position(|l| l.contains(&n));
                story::needed_by(story, at)
                    .into_iter()
                    .map(|(n, _)| n)
                    .filter(|&n| level_of(n).is_some())
                    .min_by_key(|&n| std::cmp::Reverse(level_of(n)))
            }
            KeyCode::Char('h') | KeyCode::Left if !ctrl => across(-1),
            KeyCode::Char('l') | KeyCode::Right if !ctrl => across(1),
            KeyCode::Enter | KeyCode::Char('o' | ' ') if !ctrl => {
                self.read_page(at);
                return Some(Act::Redraw);
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.mapper.strip = Some(self.map_chapter());
                return Some(Act::Redraw);
            }
            // The map has no file or change to jump to: ] and [ alone
            // show the next chapter and the one before.
            KeyCode::Char(']') if !ctrl => {
                self.reader.after_bracket = true;
                self.map_chapter_step(1);
                return Some(Act::Redraw);
            }
            KeyCode::Char('[') if !ctrl => {
                self.reader.after_bracket = true;
                self.map_chapter_step(-1);
                return Some(Act::Redraw);
            }
            KeyCode::Esc => return Some(Act::Quit),
            // What the diff takes anywhere: closing, the first key of a
            // pair, the menu, reading the change again, the commits.
            KeyCode::Char('q' | 'y' | 'g' | 'm' | 'R' | 'c' | 'E') if !ctrl => {
                return None;
            }
            KeyCode::Char('w') if ctrl => return None,
            KeyCode::F(10) => return None,
            _ => return Some(Act::Same),
        };
        if let Some(to) = to {
            self.mapper.at = to;
        }
        Some(Act::Redraw)
    }

    /// A click on something of the map: a page is chosen, and opened
    /// when it already was; a chapter's map is shown; a thing's hunk is gone
    /// to; and the way back goes back.
    pub(super) fn map_click(&mut self, hit: Hit) -> Act {
        match hit {
            // The chosen page opens as the mouse is let go, so a drag
            // from it selects its text instead.
            // A click on the map gives it the keys back from the strip.
            Hit::Page(n) if n == self.mapper.at => {
                self.mapper.strip = None;
                self.open_on_up = Some(hit);
            }
            Hit::Page(n) => {
                self.mapper.strip = None;
                self.mapper.at = n;
            }
            Hit::Chapter(t) => self.show_chapter(t),
            Hit::FilesOnly => self.open_files_only(),
            Hit::Item(_) | Hit::Line(_) | Hit::Run(_) => {}
        }
        Act::Redraw
    }

    /// A chapter's map laid out: each page a box with its number and
    /// title, a tick before them once read, over how many files and hunks
    /// it has, the next one to read from the goal down saying so.
    fn lay_map(&self, story: &Story, chapter: usize) -> Option<Laid> {
        let order = story::goal_down(story, chapter);
        let next = order.iter().copied().find(|n| !self.read.contains(n));
        let label = |n: usize| -> Vec<String> {
            let title = story.pages.get(n - 1).map_or("", |c| c.title.as_str());
            let tick = if self.read.contains(&n) { "✓ " } else { "" };
            let (_, k, _) = story::page_number(story, n);
            let (files, hunks) = self.page_counts(story, n);
            vec![
                story::label_text(&cut(&format!("{tick}{k} {title}"), 44)),
                format!(
                    "{} · {}{}",
                    counted(files, "file"),
                    counted(hunks, "hunk"),
                    if next == Some(n) { " · next" } else { "" }
                ),
            ]
        };
        let labels: Vec<(usize, Vec<String>)> = order.iter().map(|&n| (n, label(n))).collect();
        Laid::lay(&story::mermaid(story, chapter, &label), &labels)
    }

    /// The map's screen, as boards 134 and 135 draw it, across the diff's
    /// whole width: the chapters' strip, with what the change is compared
    /// with and the files no page tells at its right; the chosen chapter's
    /// pages as the map, the goal on chapter 1's top page; and under it
    /// the legend and the chosen page, what needs it and its why.
    pub(super) fn draw_map(&mut self, buf: &mut Buffer) {
        let theme = self.theme();
        let area = self.diff_area();
        let Some(story) = self.told_map().cloned() else {
            return;
        };
        let chapter = self.map_chapter();
        let Some(told) = story.chapters.get(chapter).cloned() else {
            return;
        };
        let order = story::goal_down(&story, chapter);
        if !order.contains(&self.mapper.at) {
            self.mapper.at = self.goal_of(chapter);
        }
        let at = self.mapper.at;
        let (x0, pane) = (area.x + 1, area.width.saturating_sub(2) as usize);
        // Each row of the map is text a drag selects.
        for y in 0..area.height {
            self.spot(
                Pane::Head,
                Rect::new(x0, y, pane as u16, 1),
                x0,
                i64::from(y),
                false,
            );
        }
        self.draw_strip(buf, &story, chapter, (x0, pane), &theme);
        put_line(
            buf,
            area.x,
            1,
            area.width,
            &Spans::from(kit::rule(area.width as usize, &theme)),
        );
        // Under the map: the chosen page, and the map's legend over it.
        let detail = self.map_detail(&story, chapter, at, pane, &theme);
        // The blank row, the legend and the blank row after it, then the lines.
        let below = (detail.len() + 3).min((area.height as usize).saturating_sub(9));
        let top = 2u16;
        let room = (area.height as usize).saturating_sub(top as usize + 1 + below);
        let Some(laid) = self.lay_map(&story, chapter) else {
            return;
        };
        let look = |n: usize| BoxLook {
            chosen: n == at,
            lit: self.lit == Some(Hit::Page(n)),
            read: self.read.contains(&n),
        };
        let drawn = laid.rows(pane, room, Some(at), &look, &theme);
        let hits = laid.hits((x0, top + 1), pane, room, Some(at));
        // Over the map: the goal named over chapter 1's top page, or that
        // none of a chapter's pages builds on another.
        let linked = order
            .iter()
            .any(|&n| !story::builds_on(&story, n).is_empty());
        let goal = told
            .goal
            .filter(|_| told.kind == Kind::Main)
            .and_then(|g| hits.iter().find(|(n, _)| *n == g));
        match goal {
            Some((_, r)) if linked && r.y == top + 1 => {
                let text = "THE GOAL";
                let gx = r.x + r.width.saturating_sub(width(text) as u16) / 2;
                put_line(
                    buf,
                    gx,
                    top,
                    area.right().saturating_sub(gx),
                    &Spans::from(Span::styled(text, Tone::Accent.bold(&theme))),
                );
            }
            _ if !linked && order.len() > 1 => {
                let text = format!(" {} PAGES, NONE BUILDS ON ANOTHER", order.len());
                put_line(
                    buf,
                    x0,
                    top,
                    pane as u16,
                    &Spans::from(Span::styled(text, Tone::Dim.bold(&theme))),
                );
            }
            _ => {}
        }
        for (i, spans) in drawn.iter().enumerate() {
            put_line(
                buf,
                x0,
                top + 1 + i as u16,
                pane as u16,
                &Spans::from(spans.clone()),
            );
        }
        for (n, r) in hits {
            self.hits.push((Hit::Page(n), r));
        }
        let mut y = top + 1 + laid.height(room) as u16 + 1;
        let legend = vec![
            Span::styled(" ──▼", Tone::Dim.style(&theme)),
            Span::styled(" builds on    ", Tone::Dim.style(&theme)),
            Span::styled("┄┄▼", Tone::Needs.style(&theme)),
            Span::styled(
                " a link the code can't confirm    ",
                Tone::Dim.style(&theme),
            ),
            Span::styled("✓", Tone::Done.style(&theme)),
            Span::styled(" read", Tone::Dim.style(&theme)),
        ];
        put_line(buf, x0, y, pane as u16, &Spans::from(legend));
        y += 2;
        for spans in detail {
            if y >= area.bottom() {
                break;
            }
            put_line(buf, x0, y, pane as u16, &Spans::from(spans));
            y += 1;
        }
    }

    /// The chapters' strip, as board 135 draws it: each chapter's number,
    /// name and how many of its pages are read, the one shown lit (in the
    /// accent while the strip has the keys); at its right what the change
    /// is compared with and its files, and an accent chip for the files no
    /// page tells, which opens Files mode on them.
    fn draw_strip(
        &mut self,
        buf: &mut Buffer,
        story: &Story,
        chapter: usize,
        (x0, pane): (u16, usize),
        theme: &Theme,
    ) {
        let against = self
            .change
            .as_ref()
            .map(|c| format!("{} · {}", c.against(), counted(self.totals.files, "file")))
            .unwrap_or_default();
        let (_, only) = self.files_only(story);
        let strip_at = self.mapper.strip;
        let chip = (only > 0).then(|| {
            let chosen = strip_at == Some(story.chapters.len());
            let lit = chosen || self.lit == Some(Hit::FilesOnly);
            let mut chip = kit::chip(
                &format!("{} · {only} ›", super::FILES_ONLY),
                Tone::Accent,
                false,
                theme,
            );
            if lit {
                for span in &mut chip {
                    span.style = span.style.add_modifier(Modifier::BOLD);
                }
            }
            chip
        });
        let chip_w = chip.as_ref().map_or(0, |c| kit::spans_width(c));
        let right = x0 + pane as u16;
        let chip_x = right.saturating_sub(chip_w as u16);
        let against_x = chip_x.saturating_sub(width(&against) as u16 + 2);
        // The chapters share what is left, each name cut to its share.
        let room = (against_x.saturating_sub(x0) as usize).max(pane / 2);
        let count = story.chapters.len().max(1);
        let entries: Vec<(usize, Vec<Span<'static>>)> = story
            .chapters
            .iter()
            .enumerate()
            .map(|(t, told)| {
                let (read, of) = self.read_of(t);
                let shown = t == chapter;
                let keys = shown && strip_at.is_some_and(|s| s < story.chapters.len());
                let lit = self.lit == Some(Hit::Chapter(t));
                let tally = format!("  {read} of {of} read");
                let share = (room / count).saturating_sub(width(&tally) + 6).max(6);
                let fill = if keys {
                    Tone::Accent.tint(theme)
                } else {
                    Surface::Chosen.color(theme)
                };
                let spans = vec![
                    Span::styled(format!(" {} ", t + 1), Tone::Accent.bold(theme)),
                    Span::styled(
                        cut(&told.title, share),
                        if shown || lit {
                            Tone::Text.bold(theme)
                        } else {
                            Tone::Dim.style(theme)
                        },
                    ),
                    Span::styled(
                        tally,
                        if shown && read > 0 {
                            Tone::Done.style(theme)
                        } else {
                            Tone::Faint.style(theme)
                        },
                    ),
                    Span::raw(" "),
                ];
                let spans = if shown {
                    spans
                        .into_iter()
                        .map(|s| {
                            let style = s.style.bg(fill);
                            Span::styled(s.content, style)
                        })
                        .collect()
                } else {
                    spans
                };
                (t, spans)
            })
            .collect();
        let widths: Vec<usize> = entries
            .iter()
            .map(|(_, s)| kit::spans_width(s) + 1)
            .collect();
        let total: usize = widths.iter().sum();
        // What the change is compared with gives way first; then, when the
        // chapters still don't fit, those in sight are the shown one and its
        // neighbours, a ‹ and a › at the ends reaching the rest by a click.
        let keep_against = total <= against_x.saturating_sub(x0) as usize;
        let avail = if keep_against {
            against_x.saturating_sub(x0) as usize
        } else {
            chip_x.saturating_sub(x0 + 1) as usize
        };
        let (mut from, mut to) = (0, entries.len());
        if total > avail {
            let room = avail.saturating_sub(4);
            let (mut lo, mut hi) = (chapter, chapter + 1);
            let mut used = widths.get(chapter).copied().unwrap_or(0);
            loop {
                let right = widths.get(hi).copied().filter(|w| used + w <= room);
                if let Some(w) = right {
                    used += w;
                    hi += 1;
                }
                let left = lo
                    .checked_sub(1)
                    .and_then(|l| widths.get(l))
                    .copied()
                    .filter(|w| used + w <= room);
                if let Some(w) = left {
                    used += w;
                    lo -= 1;
                }
                if right.is_none() && left.is_none() {
                    break;
                }
            }
            (from, to) = (lo, hi);
        }
        let mut x = x0;
        let arrow = |buf: &mut Buffer, x: u16, text: &str| {
            put_line(
                buf,
                x,
                0,
                2,
                &Spans::from(Span::styled(text.to_string(), Tone::Accent.bold(theme))),
            );
        };
        if from > 0 {
            arrow(buf, x, "‹ ");
            self.hits
                .push((Hit::Chapter(from - 1), Rect::new(x, 0, 1, 1)));
            x += 2;
        }
        for (t, spans) in entries.into_iter().take(to).skip(from) {
            let w = kit::spans_width(&spans) as u16;
            put_line(buf, x, 0, w, &Spans::from(spans));
            self.hits.push((Hit::Chapter(t), Rect::new(x, 0, w, 1)));
            x += w + 1;
        }
        if to < story.chapters.len() {
            arrow(buf, x, " ›");
            self.hits
                .push((Hit::Chapter(to), Rect::new(x + 1, 0, 1, 1)));
            x += 2;
        }
        if keep_against && against_x > x && !against.is_empty() {
            put_line(
                buf,
                against_x,
                0,
                chip_x.saturating_sub(against_x),
                &Spans::from(Span::styled(against, Tone::Dim.style(theme))),
            );
        }
        if let Some(chip) = chip
            && chip_x >= x
        {
            put_line(buf, chip_x, 0, chip_w as u16, &Spans::from(chip));
            self.hits.push((
                Hit::FilesOnly,
                Rect::new(chip_x, 0, (chip_w as u16).saturating_sub(1), 1),
            ));
        }
    }

    /// The lines under the map about the chosen page, as board 135 draws
    /// them: "Page n of m" and its title, with what needs it, its files
    /// and its hunks at the right; what it builds on in another chapter;
    /// its why; and why a chapter beside the goal's is apart.
    fn map_detail(
        &self,
        story: &Story,
        chapter: usize,
        at: usize,
        cols: usize,
        theme: &Theme,
    ) -> Vec<Vec<Span<'static>>> {
        let Some(page) = story.pages.get(at - 1) else {
            return Vec::new();
        };
        let told = &story.chapters[chapter];
        let main = story.chapters.first().map_or("", |t| t.title.as_str());
        let here = Some(chapter);
        let needers = story::needed_by(story, at);
        let needs = if !needers.is_empty() {
            format!("needed by {}", story::pages_named(story, &needers, here))
        } else if told.goal == Some(at) && told.kind == Kind::Main {
            "the goal".to_string()
        } else if told.kind != Kind::Main {
            format!("nothing in {main} needs it")
        } else {
            "nothing needs it".to_string()
        };
        let (files, hunks) = self.page_counts(story, at);
        let right = format!(
            "{needs} · {} · {}",
            counted(files, "file"),
            counted(hunks, "hunk")
        );
        let (_, k, m) = story::page_number(story, at);
        let room = cols.saturating_sub(width(&right) + 2);
        let mut first = kit::fit(
            vec![
                Span::raw(" "),
                Span::styled(format!("Page {k} of {m}"), Tone::Accent.bold(theme)),
                Span::styled(format!("  {}", page.title), Tone::Text.bold(theme)),
            ],
            room,
            Style::new(),
        );
        first.push(Span::raw(" "));
        first.push(Span::styled(right, Tone::Dim.style(theme)));
        let mut out = vec![first];
        let text = |out: &mut Vec<Vec<Span<'static>>>, words: &str, tone: Tone, most: usize| {
            let mut lines = wrap(words, cols.saturating_sub(2).max(10));
            lines.retain(|l| !l.is_empty());
            for line in lines.into_iter().take(most) {
                out.push(vec![Span::raw(" "), Span::styled(line, tone.style(theme))]);
            }
        };
        // What it builds on in another chapter, which no arrow here shows:
        // first, so a short pane cuts the why's end before it.
        for need in &page.needs {
            let other = story::chapter_of(story, need.page).filter(|t| *t != chapter);
            if let (Some(t), Some(on)) = (other, story.pages.get(need.page - 1)) {
                text(
                    &mut out,
                    &format!(
                        "Builds on {} {}, in {}{}.",
                        story::page_name(story, need.page, here),
                        on.title,
                        story.chapters[t].title,
                        if need.guess { " (a guess)" } else { "" }
                    ),
                    Tone::Dim,
                    2,
                );
            }
        }
        if !page.circles.is_empty() {
            let left: Vec<(usize, bool)> = page.circles.iter().map(|n| (*n, false)).collect();
            text(
                &mut out,
                &format!(
                    "Haiku also said it builds on {}, left off the map: it would close a circle.",
                    story::pages_named(story, &left, here)
                ),
                Tone::Faint,
                2,
            );
        }
        text(&mut out, &page.why, Tone::Dim, 3);
        // A dashed link: why the code can't confirm it.
        let guesses: Vec<(usize, bool)> = story::builds_on(story, at)
            .into_iter()
            .filter(|(_, guess)| *guess)
            .collect();
        if !guesses.is_empty() {
            text(
                &mut out,
                &format!(
                    "Dashed to {}: Haiku linked them, but the code can't confirm it.",
                    story::pages_named(story, &guesses, here)
                ),
                Tone::Dim,
                1,
            );
        }
        if told.kind != Kind::Main {
            out.push(Vec::new());
            out.push(kit::label(
                "Why this chapter is apart",
                Tone::Dim,
                None,
                cols,
                theme,
            ));
            let outside: Vec<(usize, &str)> = told
                .pages
                .iter()
                .flat_map(|&n| story.pages[n - 1].needs.iter())
                .filter(|need| !told.pages.contains(&need.page))
                .filter_map(|need| {
                    Some((need.page, story.pages.get(need.page - 1)?.title.as_str()))
                })
                .collect();
            // A page of another chapter that builds on one of these,
            // which the checked links may show whatever Haiku said.
            let needed: Option<(usize, &str)> = story
                .pages
                .iter()
                .enumerate()
                .filter(|(i, _)| !told.pages.contains(&(i + 1)))
                .find(|(_, c)| c.needs.iter().any(|need| told.pages.contains(&need.page)))
                .map(|(i, c)| (i + 1, c.title.as_str()));
            let name = |n: usize| story::page_name(story, n, here);
            let why = match (told.kind, outside.first()) {
                _ if needed.is_some() => {
                    let (n, title) = needed.unwrap_or_default();
                    format!(
                        "{} was told apart from {main}, but {} {title} builds on it.",
                        told.title,
                        name(n)
                    )
                }
                (Kind::Side, _) => format!(
                    "{} is what {main} can be read without: a rename, formatting, a drive-by fix.",
                    told.title
                ),
                (_, Some((n, title))) => format!(
                    "{} is a chapter of its own: it builds on {} {title}, but nothing in {main} needs it.",
                    told.title,
                    name(*n)
                ),
                (_, None) => format!(
                    "{} is a chapter of its own: nothing in {main} needs it.",
                    told.title
                ),
            };
            text(&mut out, &why, Tone::Dim, 3);
        }
        out
    }
}
