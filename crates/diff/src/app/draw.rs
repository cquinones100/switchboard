//! Drawing the diff's screen, split from app.rs for its length: the list
//! with its pages' headings, the two sides with their bars and notes,
//! the cover while the story is told, the row of buttons, and the boxes
//! for a question and its answer, into a cell buffer, as
//! nvim/lua/plugins/diffview.lua's highlights, winbars and floats drew them.

use std::collections::HashSet;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as Spans, Span};

use super::select::{Pane, Spot};
use super::{
    ACCENT, ADDED, App, Button, COVER_BLANK, Cell, Focus, Line, ListRow, REMOVED, SHORT_WHY,
    Telling, UNCOMMITTED, blend, own,
};
use crate::git::{self, Entry, Section};
use crate::sides::{self, Kind};
use crate::story::{self, Look as StoryLook};
use crate::util;
use switchboard_ui::draw::{BoxLine, BoxSpec, Place, cut, draw_box, put_line, width, wrap};
use switchboard_ui::kit::{self, Surface, Tone};

/// The most lines of a page's why UX 2.0 puts over the diff.
const WHY_LINES: usize = 3;

impl App {
    /// Draws the whole screen into `buf`; gives where the cursor goes.
    /// Nothing is read while it draws (`drawing`): a file of the scroll
    /// not read yet ends what is drawn of it.
    pub fn draw(&mut self, buf: &mut Buffer) -> Option<(u16, u16)> {
        self.drawing = true;
        let area = buf.area;
        let cursor = if self.has_header() && area.height > 1 {
            // Alone, the diff is drawn under its header: into a buffer of
            // its own a row shorter, put a row down.
            let mut body = Buffer::empty(Rect::new(0, 0, area.width, area.height - 1));
            let cursor = self.draw_now(&mut body);
            for y in 0..body.area.height {
                for x in 0..area.width {
                    buf[(x, y + 1)] = body[(x, y)].clone();
                }
            }
            self.draw_header(buf);
            cursor.map(|(x, y)| (x, y + 1))
        } else {
            self.draw_now(buf)
        };
        self.drawing = false;
        cursor
    }

    /// The message in its box over the screen; whether there is one.
    fn draw_message(&self, buf: &mut Buffer) -> bool {
        let Some((text, _)) = &self.message else {
            return false;
        };
        let lines: Vec<BoxLine> = text
            .lines()
            .map(|l| BoxLine::text(l.to_string(), Style::new().fg(Color::Indexed(252))))
            .collect();
        let mut spec = BoxSpec::new("", &lines);
        spec.width = 10;
        spec.place = Place::Center;
        draw_box(buf, &spec, &switchboard_ui::Look::default());
        true
    }

    fn draw_now(&mut self, buf: &mut Buffer) -> Option<(u16, u16)> {
        // The settings view, alone, in the place of the diff and of a
        // commit of the history.
        if self.settings.is_some() {
            self.screen_now(buf.area.width, buf.area.height);
            self.buttons.clear();
            self.spots.clear();
            self.draw_settings(buf);
            self.draw_message(buf);
            return None;
        }
        // A commit of the history draws itself in this one's place.
        if let Some(h) = &mut self.history {
            let cursor = h.app.draw(buf);
            self.out.append(&mut h.app.out);
            // What the branch's diff has to say (an update's result) shows
            // over the commit.
            if self.draw_message(buf) {
                return None;
            }
            return cursor;
        }
        let area = buf.area;
        // A size seen here before its resize event is taken the same way.
        self.screen_now(area.width, area.height);
        self.buttons.clear();
        self.spots.clear();
        self.hits.clear();
        self.ask_rect = None;
        self.answer_rect = None;
        self.answer_shown.clear();
        self.diff_bar = None;
        self.body_top = None;
        if let Some(text) = self.failed.clone() {
            self.centred(
                buf,
                &[
                    (text, Style::new().fg(Color::Indexed(203))),
                    (String::new(), Style::new()),
                    ("Press any key to close.".into(), self.faint()),
                ],
            );
            if self.opts.alone {
                self.draw_buttons(buf);
            }
            self.draw_message(buf);
            return None;
        }
        if let Some(text) = self.nothing.clone() {
            let mut lines: Vec<(String, Style)> = text
                .lines()
                .map(|l| (l.to_string(), Style::new()))
                .collect();
            lines.push((String::new(), Style::new()));
            lines.push(("Press any key to close.".into(), self.faint()));
            self.centred(buf, &lines);
            // Alone, the row stays: a setting may be why nothing shows.
            if self.opts.alone {
                self.draw_buttons(buf);
            }
            self.draw_message(buf);
            return None;
        }
        if let Some(text) = self.loading.clone() {
            let line = format!("{} {text}", self.spinner());
            self.centred(buf, &[(line, Style::new().add_modifier(Modifier::BOLD))]);
            if self.opts.alone {
                self.draw_buttons(buf);
            }
            self.draw_message(buf);
            return None;
        }
        if self.cover.is_some() && self.telling == Telling::Waiting {
            self.draw_cover(buf);
            // Alone, the row stays under the cover: its Settings button is
            // how a click turns the story off.
            if self.opts.alone {
                self.draw_buttons(buf);
            }
            self.draw_message(buf);
            return None;
        }
        let mut cursor = None;
        if self.on_map() {
            // The story's map, one pane with the keys.
            self.draw_map(buf);
        } else if self.mode == super::Mode::Map && self.telling == Telling::Waiting {
            // The map of the change read again, still being told.
            let line = format!("{} Telling the story as a map", self.spinner());
            self.centred(buf, &[(line, Style::new().add_modifier(Modifier::BOLD))]);
        } else {
            self.draw_list(buf);
            cursor = self.draw_diff(buf);
            self.things_hits();
            // The list greyed while the file (or a page's own map) has the
            // keys. The file is never greyed: with the keys in the list it
            // shows the list's chosen file, which is what is being read.
            if self.focus != Focus::List || self.thing.is_some() {
                let file = self.diff_area();
                let list = Rect::new(0, 0, file.x.saturating_sub(1), file.height);
                kit::unfocused(buf, list, &self.theme());
            }
            self.draw_edge(buf);
        }
        if self.has_buttons() {
            self.draw_buttons(buf);
        }
        if self.has_footer() {
            let y = self.rows - 1;
            put_line(
                buf,
                1,
                y,
                self.cols.saturating_sub(1),
                &Spans::from(kit::hints(&self.footer_keys(), &self.theme())),
            );
        }
        if self.ask_box.is_some() {
            cursor = self.draw_ask_box(buf);
        } else if self.answer.is_some() {
            self.draw_answer(buf);
            cursor = None;
        }
        self.read_spots(buf);
        if self.draw_message(buf) {
            cursor = None;
        }
        if self.menu.is_some() {
            self.draw_menu(buf);
            cursor = None;
        }
        if self.focus == Focus::List || self.thing.is_some() {
            cursor = None;
        }
        cursor
    }

    /// The footer's keys, as board 12 lists them but with no letter key,
    /// as every footer: those of the list or of the diff, whichever has
    /// the focus, then M-c back to Claude in the workspace.
    pub(super) fn footer_keys(&self) -> Vec<(&'static str, &'static str)> {
        let mut keys = if self.filtering {
            vec![("↑↓", "move"), ("↵", "keep"), ("esc", "clear")]
        } else if self.on_map() && self.strip.is_some() {
            vec![("←→", "chapter"), ("↵", "open"), ("esc", "the map")]
        } else if self.on_map() {
            let mut keys = vec![("↑↓←→", "move"), ("↵", "open")];
            if self.story.as_ref().is_some_and(|s| s.chapters.len() > 1) {
                keys.push(("] [", "chapter"));
            }
            keys.push(("esc", "close"));
            keys
        } else if self.thing.is_some() {
            vec![("↑↓", "move"), ("↵", "its hunk"), ("esc", "list")]
        } else if self.focus == Focus::List && self.in_page() {
            vec![("↑↓", "move"), ("↵", "read"), ("esc", "the map")]
        } else if self.focus == Focus::List {
            vec![("↑↓", "move"), ("↵", "read"), ("esc", "close")]
        } else {
            vec![("↑↓", "move"), ("←→", "sides"), ("esc", "list")]
        };
        if self.opts.server {
            keys.push(("M-c", "Claude"));
        }
        keys
    }

    /// The terminal's background and text, when it said both and its
    /// background agrees with the theme drawn in.
    fn ends(&self) -> Option<(u32, u32)> {
        let rgb = |c: switchboard_ui::terminal::Rgb| {
            (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b)
        };
        let (bg, fg) = (self.term.bg?, self.term.fg?);
        (self.term.light() == Some(self.opts.light)).then(|| (rgb(bg), rgb(fg)))
    }

    /// Faint text: halfway from the background to the text, as base16's
    /// comments are, else a grey of the palette's.
    fn faint(&self) -> Style {
        Style::new().fg(match self.ends() {
            Some((bg, fg)) => blend(fg, bg, 0.5),
            None => Color::Indexed(if self.opts.light { 246 } else { 243 }),
        })
    }

    fn comment(&self) -> Style {
        let fg = match self.ends() {
            Some((bg, fg)) => blend(fg, bg, 0.6),
            None => Color::Indexed(if self.opts.light { 244 } else { 245 }),
        };
        Style::new().fg(fg).add_modifier(Modifier::ITALIC)
    }

    fn put(buf: &mut Buffer, x: u16, y: u16, cols: u16, text: &str, style: Style) -> u16 {
        if cols == 0 || y >= buf.area.bottom() || x >= buf.area.right() {
            return x;
        }
        let cols = cols.min(buf.area.right() - x);
        buf.set_stringn(x, y, text, cols as usize, style).0
    }

    /// Lines in the middle of the screen, each centred.
    fn centred(&self, buf: &mut Buffer, lines: &[(String, Style)]) {
        let area = buf.area;
        let mut wrapped: Vec<(String, Style)> = Vec::new();
        for (text, style) in lines {
            for line in wrap(text, area.width.saturating_sub(4) as usize) {
                wrapped.push((line, *style));
            }
        }
        let y0 = area.height.saturating_sub(wrapped.len() as u16) / 2;
        for (i, (text, style)) in wrapped.iter().enumerate() {
            let x = area.width.saturating_sub(width(text) as u16) / 2;
            Self::put(buf, x, y0 + i as u16, area.width, text, *style);
        }
    }

    /// The cover while the story is told: blank for its first moments,
    /// then what is read and each page as Haiku names it, or Haiku's
    /// answer so far, centred as one block so its ticks line up.
    fn draw_cover(&self, buf: &mut Buffer) {
        let Some(up) = self.cover else {
            return;
        };
        if up.elapsed() < COVER_BLANK {
            return;
        }
        let area = buf.area;
        let spin = self.spinner();
        let (block, keys): (Vec<String>, &str) = if !self.opts.progress {
            (
                vec![format!("{spin} Telling the change as a story")],
                "Esc shows the diff now",
            )
        } else if self.raw {
            let mut block = vec![format!("{spin} Haiku's answer so far")];
            let room = area.width.saturating_sub(8).max(20) as usize;
            let mut wrapped: Vec<String> = Vec::new();
            for line in self
                .progress
                .as_ref()
                .map_or("", |p| p.text.as_str())
                .split('\n')
            {
                let chars: Vec<char> = line.chars().collect();
                if chars.is_empty() {
                    wrapped.push(String::new());
                }
                for piece in chars.chunks(room) {
                    wrapped.push(piece.iter().collect());
                }
            }
            let keep = (area.height as usize).saturating_sub(6).max(1);
            let from = wrapped.len().saturating_sub(keep);
            block.extend(wrapped[from..].iter().cloned());
            (block, "Esc shows the diff now · t shows the pages")
        } else {
            (
                story::progress(
                    self.progress.as_ref(),
                    Some(&self.reading),
                    spin,
                    util::now_secs(),
                ),
                "Esc shows the diff now · t shows Haiku's answer as it comes",
            )
        };
        let wide = block.iter().map(|l| width(l)).max().unwrap_or(0) as u16;
        let x = area.width.saturating_sub(wide) / 2;
        let y0 = area.height.saturating_sub(block.len() as u16 + 2) / 2;
        for (i, line) in block.iter().enumerate() {
            let style = if i == 0 {
                Style::new().add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };
            Self::put(
                buf,
                x,
                y0 + i as u16,
                area.width.saturating_sub(x),
                line,
                style,
            );
        }
        let kx = area.width.saturating_sub(width(keys) as u16) / 2;
        Self::put(
            buf,
            kx,
            y0 + block.len() as u16 + 1,
            area.width,
            keys,
            self.faint(),
        );
    }

    /// The path filter's line, pinned at the list's top: what is typed and
    /// how many files it leaves of how many.
    fn draw_filter_line(&self, buf: &mut Buffer, w: u16, theme: &switchboard_ui::Theme) {
        let (left, of) = self.filtered_count();
        let count = format!("{left} of {of} files");
        if self.opts.ux2 {
            let spans = kit::input_beside(
                &self.path_filter,
                "type to filter paths",
                w as usize,
                vec![Span::styled(count, Tone::Dim.style(theme))],
                theme,
                false,
            );
            put_line(buf, 0, 0, w, &Spans::from(spans));
        } else {
            let cursor = if self.filtering { "▏" } else { "" };
            let text = format!(" /{}{cursor}", self.path_filter);
            let room = (w as usize).saturating_sub(width(&count) + 1);
            Self::put(buf, 0, 0, room as u16, &text, Style::new());
            let x = w.saturating_sub(width(&count) as u16);
            Self::put(buf, x, 0, w - x, &count, self.faint());
        }
    }

    fn draw_list(&mut self, buf: &mut Buffer) {
        let w = self.list_width.min(self.cols.saturating_sub(10));
        let h = self.list_height();
        let pinned = self.list_pinned();
        let rows = self.list_rows();
        // How many rows the list has and where its last file is, kept for
        // the pointer's shape over it.
        self.list_drawn = (
            rows.len(),
            rows.iter()
                .rposition(|r| matches!(r, ListRow::File(_)))
                .map(|i| i + 1),
        );
        // The cursor's file in sight, with its heading where there's room,
        // once it has moved (or the story has moved its row): the wheel
        // scrolls the list past it.
        let row = rows.iter().position(|r| *r == ListRow::File(self.at));
        if row != self.list_followed
            && let Some(at) = row
        {
            self.list_followed = row;
            let mut first = at;
            while first > 0
                && matches!(
                    rows[first - 1],
                    ListRow::Heading(..)
                        | ListRow::Page(_)
                        | ListRow::Chapter(_)
                        | ListRow::Label(..)
                        | ListRow::Blank
                        | ListRow::Totals
                )
            {
                first -= 1;
            }
            if self.list_top > at || (self.list_top > first && at - first < h) {
                self.list_top = if at - first < h { first } else { at };
            }
            if at >= self.list_top + h {
                self.list_top = at + 1 - h;
            }
        }
        self.list_top = self.list_top.min(rows.len().saturating_sub(1));
        let entries = self.entries().to_vec();
        let theme = self.theme();
        if self.opts.ux2 {
            // The list on the panel, a rule between it and the diff.
            let panel = Surface::Panel.color(&theme);
            for y in 0..self.diff_area().height {
                for x in 0..w {
                    buf[(x, y)].set_bg(panel);
                }
                put_line(buf, w, y, 1, &Spans::from(kit::rule_v(&theme)));
            }
        }
        // The rows a column narrower while the scroll bar is beside them.
        let bar = self.opts.scroll_bars && w > 1 && rows.len() > h;
        let full = w;
        let w = w - u16::from(bar);
        if bar {
            kit::draw_scrollbar(
                buf,
                (full - 1, pinned, h as u16),
                (rows.len(), self.list_top, h),
                &theme,
            );
        }
        if pinned > 0 {
            self.draw_filter_line(buf, w, &theme);
        }
        let mut lit: Vec<u16> = Vec::new();
        // The rows of the files the keys selected (v), lit below.
        let selected = self
            .list_visual
            .filter(|_| self.focus == Focus::List)
            .and_then(|from| {
                let place = |k: usize| rows.iter().position(|r| *r == ListRow::File(k));
                let (a, b) = (place(from)?, place(self.at)?);
                Some(a.min(b)..=a.max(b))
            });
        // The way back heads only the list's first section; a later one
        // (staged after not staged) keeps its own label.
        let first_header = rows.iter().position(|r| matches!(r, ListRow::Header(_)));
        for (i, row) in rows.iter().enumerate().skip(self.list_top).take(h) {
            let y = (i - self.list_top) as u16 + pinned;
            self.spot(Pane::List, Rect::new(0, y, w, 1), 0, i as i64, false);
            if selected.as_ref().is_some_and(|s| s.contains(&i)) {
                lit.push(y);
            }
            match row {
                ListRow::Header(section) if self.opts.ux2 => {
                    // What is listed apart under "Not committed yet" counted
                    // there, not here.
                    let apart = self.not_committed_apart();
                    let n = self
                        .order
                        .iter()
                        .filter(|&&i| {
                            entries.get(i).is_some_and(|e| {
                                e.section == *section && !apart.contains(e.path.as_str())
                            })
                        })
                        .count();
                    let spans = match (&self.title, self.alone, &self.story) {
                        (Some((title, _)), _, _) => vec![
                            Span::raw(" "),
                            Span::styled(title.clone(), Tone::Text.bold(&theme)),
                        ],
                        // A page opened from the map: the way back to
                        // its chapter's map.
                        (None, Some(_), Some(_)) if self.in_page() && Some(i) == first_header => {
                            self.hits
                                .push((super::map::Hit::Back, Rect::new(0, y, w, 1)));
                            let lit = self.lit == Some(super::map::Hit::Back);
                            vec![
                                Span::styled(" ‹ ", Tone::Accent.bold(&theme)),
                                Span::styled(
                                    self.shown_story_title(),
                                    if lit {
                                        Tone::Text.bold(&theme)
                                    } else {
                                        Tone::Dim.bold(&theme)
                                    },
                                ),
                            ]
                        }
                        // The page of what isn't committed yet is named, the
                        // others counted among the pages told.
                        (None, Some(p), Some(_)) if self.uncommitted_page() == Some(p) => {
                            kit::label(
                                story::NOT_COMMITTED,
                                Tone::Needs,
                                Some(n),
                                w as usize,
                                &theme,
                            )
                        }
                        (None, Some(p), Some(_)) if self.generated_page() == Some(p) => kit::label(
                            super::GENERATED_LAST,
                            Tone::Dim,
                            Some(n),
                            w as usize,
                            &theme,
                        ),
                        (None, Some(p), Some(_)) => kit::label(
                            &self.page_heading(p),
                            Tone::Dim,
                            Some(n),
                            w as usize,
                            &theme,
                        ),
                        (None, None, Some(_)) if *section == Section::Committed => kit::label(
                            "Story",
                            Tone::Dim,
                            Some(self.told_pages()),
                            w as usize,
                            &theme,
                        ),
                        _ => kit::label(section.heading(), Tone::Dim, Some(n), w as usize, &theme),
                    };
                    put_line(buf, 0, y, w, &Spans::from(spans));
                }
                ListRow::Page(n) => {
                    let Some(page) = self.story.as_ref().and_then(|s| s.pages.get(n - 1)) else {
                        continue;
                    };
                    let here = self
                        .current()
                        .and_then(|e| self.home(&e.path))
                        .is_some_and(|h| h == *n);
                    let spans = vec![
                        Span::raw(" "),
                        Span::styled(self.page_shown(*n).to_string(), Tone::Accent.bold(&theme)),
                        Span::styled(
                            format!(" {}", page.title),
                            if here {
                                Tone::Text.bold(&theme)
                            } else {
                                Tone::Dim.style(&theme)
                            },
                        ),
                    ];
                    put_line(
                        buf,
                        0,
                        y,
                        w,
                        &Spans::from(kit::fit(spans, w as usize, Style::new())),
                    );
                }
                ListRow::Chapter(c) => {
                    let Some(told) = self.story.as_ref().and_then(|s| s.chapters.get(*c)) else {
                        continue;
                    };
                    let here = self
                        .current()
                        .and_then(|e| self.home(&e.path))
                        .is_some_and(|h| {
                            self.story
                                .as_ref()
                                .is_some_and(|s| story::chapter_of(s, h) == Some(*c))
                        });
                    let (read, of) = self.read_of(*c);
                    let tally = format!("{read} of {of} ");
                    let room = (w as usize).saturating_sub(width(&tally) + 1);
                    let mut spans = kit::fit(
                        vec![
                            Span::raw(" "),
                            Span::styled((c + 1).to_string(), Tone::Accent.bold(&theme)),
                            Span::styled(
                                format!(" {}", told.title),
                                if here {
                                    Tone::Text.bold(&theme)
                                } else {
                                    Tone::Dim.style(&theme)
                                },
                            ),
                        ],
                        room,
                        Style::new(),
                    );
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(
                        tally,
                        if here && read > 0 {
                            Tone::Done.style(&theme)
                        } else {
                            Tone::Faint.style(&theme)
                        },
                    ));
                    put_line(buf, 0, y, w, &Spans::from(spans));
                }
                ListRow::Label(text, tone, count) => {
                    let spans = kit::label(text, *tone, *count, w as usize, &theme);
                    put_line(buf, 0, y, w, &Spans::from(spans));
                }
                ListRow::Blank => {}
                ListRow::Totals => {
                    let spans = self.totals_row(w as usize);
                    put_line(buf, 0, y, w, &Spans::from(spans));
                }
                ListRow::Hint(text) => {
                    let spans = vec![
                        Span::raw(" "),
                        Span::styled(text.clone(), Tone::Faint.style(&theme)),
                    ];
                    put_line(
                        buf,
                        0,
                        y,
                        w,
                        &Spans::from(kit::fit(spans, w as usize, Style::new())),
                    );
                }
                ListRow::File(k) if self.opts.ux2 => {
                    let Some(e) = self.order.get(*k).and_then(|&i| entries.get(i)) else {
                        continue;
                    };
                    let chosen = *k == self.at;
                    let spans = self.file_row(e, chosen, w as usize);
                    put_line(
                        buf,
                        0,
                        y,
                        w,
                        &Spans::from(kit::row(spans, w as usize, chosen, &theme)),
                    );
                }
                ListRow::Header(section) => {
                    let n = self
                        .order
                        .iter()
                        .filter(|&&i| entries.get(i).is_some_and(|e| e.section == *section))
                        .count();
                    let text = match (&self.title, self.alone, &self.story) {
                        (Some((title, _)), _, _) => title.clone(),
                        // A page opened from the map: the way back to it.
                        (None, Some(_), Some(_)) if self.in_page() && Some(i) == first_header => {
                            self.hits
                                .push((super::map::Hit::Back, Rect::new(0, y, w, 1)));
                            format!(" ‹ {}", self.shown_story_title())
                        }
                        (None, Some(p), Some(_)) => {
                            format!(" {} ({n})", self.page_heading(p))
                        }
                        _ => format!(" {} ({n})", section.heading()),
                    };
                    Self::put(buf, 0, y, w, &text, self.faint());
                }
                ListRow::Loading => {
                    Self::put(
                        buf,
                        0,
                        y,
                        w,
                        &format!(" {} telling the story", self.spinner()),
                        self.comment(),
                    );
                }
                ListRow::Heading(text, look) => {
                    let style = match look {
                        // The colorscheme's Title, the terminal's blue.
                        StoryLook::Title => Style::new()
                            .fg(Color::Indexed(4))
                            .add_modifier(Modifier::BOLD),
                        StoryLook::Comment => self.comment(),
                        StoryLook::Watch => Tone::Needs.style(&self.theme()),
                        StoryLook::Normal => Style::new(),
                    };
                    Self::put(buf, 1, y, w.saturating_sub(1), text, style);
                }
                ListRow::File(k) => {
                    let Some(e) = self.order.get(*k).and_then(|&i| entries.get(i)) else {
                        continue;
                    };
                    let chosen = *k == self.at;
                    let bg = if chosen {
                        Some(Color::Indexed(if self.focus == Focus::List {
                            238
                        } else {
                            236
                        }))
                    } else {
                        None
                    };
                    if let Some(bg) = bg {
                        for x in 0..w {
                            buf[(x, y)].set_bg(bg);
                        }
                    }
                    let with = |s: Style| match bg {
                        Some(bg) => s.bg(bg),
                        None => s,
                    };
                    let status = match e.status {
                        'A' => Color::Indexed(114),
                        'D' => Color::Indexed(167),
                        'R' | 'C' => Color::Indexed(75),
                        _ => Color::Indexed(179),
                    };
                    let counts = if e.binary {
                        " bin".to_string()
                    } else {
                        format!(" +{} −{}", e.additions, e.deletions)
                    };
                    // A file the open page shows a hunk of from another.
                    // Left out where it would leave the name no room.
                    let counts = match self.also_of(&e.path, self.run_of(e)) {
                        Some(n) => {
                            let also = format!(" also {}{counts}", self.page_named(n, &e.path));
                            if width(&also) + 12 <= w as usize {
                                also
                            } else {
                                counts
                            }
                        }
                        None => counts,
                    };
                    let cw = width(&counts) as u16;
                    let (dir, name) = match e.path.rsplit_once('/') {
                        Some((d, n)) => (format!("{d}/"), n.to_string()),
                        None => (String::new(), e.path.clone()),
                    };
                    // A file with changes not yet committed: a ● before its
                    // counts, as diffview.lua marked its row.
                    let marked = self.changed_since(e);
                    if marked {
                        Self::put(
                            buf,
                            w.saturating_sub(cw + 2),
                            y,
                            2,
                            " ●",
                            with(Style::new().fg(UNCOMMITTED)),
                        );
                    }
                    let room = w.saturating_sub(cw + 1 + if marked { 2 } else { 0 });
                    let mut x = Self::put(
                        buf,
                        1,
                        y,
                        room,
                        &e.status.to_string(),
                        with(Style::new().fg(status)),
                    );
                    x = Self::put(
                        buf,
                        x + 1,
                        y,
                        room.saturating_sub(x),
                        &name,
                        with(if chosen {
                            Style::new().add_modifier(Modifier::BOLD)
                        } else {
                            Style::new()
                        }),
                    );
                    if x + 1 < room {
                        Self::put(buf, x + 1, y, room - x - 1, &dir, with(self.faint()));
                    }
                    Self::put(
                        buf,
                        w.saturating_sub(cw),
                        y,
                        cw,
                        &counts,
                        with(self.faint()),
                    );
                }
            }
        }
        for y in lit {
            for x in 0..w {
                buf[(x, y)].modifier.insert(Modifier::REVERSED);
            }
        }
        if self.order.is_empty() {
            Self::put(buf, 1, 2, w.saturating_sub(1), "No changes", self.faint());
        }
    }

    /// The totals at the head of the list, as board 105 draws them: the
    /// change's files, its lines added and deleted, and five cells shared
    /// out by them, cut to the list's width; with the path filter set, as
    /// board 106 does, the files left of how many and their lines alone.
    pub(super) fn totals_row(&self, cols: usize) -> Vec<Span<'static>> {
        let theme = self.theme();
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let (t, files) = if self.path_filter.trim().is_empty() {
            let t = self.totals;
            (
                t,
                vec![Span::styled(
                    format!("{} file{}", t.files, plural(t.files)),
                    Tone::Text.bold(&theme),
                )],
            )
        } else {
            let (left, of) = (self.left.files, self.listed_files);
            (
                self.left,
                vec![
                    Span::styled(left.to_string(), Tone::Text.bold(&theme)),
                    Span::styled(
                        format!(" of {of} file{}", plural(of)),
                        Tone::Dim.style(&theme),
                    ),
                ],
            )
        };
        // Two spaces between the parts where they fit, one where they
        // don't; the words then cut before the bar, which keeps its room
        // so its deleted share is never the part cut.
        let words = |gap: &'static str| {
            let mut spans = vec![Span::raw(" ")];
            spans.extend(files.clone());
            spans.extend([
                Span::raw(gap),
                Span::styled(format!("+{}", t.added), Tone::Done.bold(&theme)),
                Span::raw(" "),
                Span::styled(format!("−{}", t.deleted), Tone::Failed.bold(&theme)),
                Span::raw(gap),
            ]);
            spans
        };
        let meter = kit::diff_meter(t.added, t.deleted, 5, &theme);
        let room = cols.saturating_sub(kit::spans_width(&meter));
        let wide = words("  ");
        let mut spans = if kit::spans_width(&wide) <= room {
            wide
        } else {
            let mut narrow = words(" ");
            narrow.pop();
            if kit::spans_width(&narrow) >= room {
                narrow = kit::fit(narrow, room.saturating_sub(1), Style::new());
            }
            narrow.push(Span::raw(" "));
            narrow
        };
        spans.extend(meter);
        kit::fit(spans, cols, Style::new())
    }

    /// A file's row in UX 2.0's list, after the chosen row's bar: a ● in
    /// the needs tone for one with changes not yet committed, its path
    /// (the folders dim, the name bold when chosen), its counts at the
    /// right.
    fn file_row(&self, e: &Entry, chosen: bool, cols: usize) -> Vec<Span<'static>> {
        let theme = self.theme();
        let counts = if e.binary {
            "bin".to_string()
        } else {
            format!("+{} −{}", e.additions, e.deletions)
        };
        let mark = if self.changed_since(e) { "●" } else { " " };
        // In an open page, a file shown from another one says which.
        let also = self
            .also_of(&e.path, self.run_of(e))
            .map(|n| format!("also {} ", self.page_named(n, &e.path)));
        // Left out where it would leave the name no room.
        let counts = match also {
            Some(also) if width(&also) + width(&counts) + 13 <= cols => format!("{also}{counts}"),
            _ => counts,
        };
        let (dir, name) = match e.path.rsplit_once('/') {
            Some((d, n)) => (format!("{d}/"), n.to_string()),
            None => (String::new(), e.path.clone()),
        };
        // The bar, the mark and a space before the path, a space and the
        // counts after it, a column short of the edge.
        let room = cols.saturating_sub(1 + 2 + 1 + width(&counts) + 1);
        let name_style = if chosen {
            Tone::Text.bold(&theme)
        } else {
            Tone::Text.style(&theme)
        };
        let mut out = vec![
            Span::styled(mark, Tone::Needs.style(&theme)),
            Span::raw(" "),
        ];
        let mut used = 0;
        if width(&dir) + width(&name) <= room {
            used += width(&dir) + width(&name);
            out.push(Span::styled(dir, Tone::Dim.style(&theme)));
            out.push(Span::styled(name, name_style));
        } else {
            // The name first, then as much of its folders as fits from
            // the end, nearest the name, so two files of the same name in
            // folders that start alike are told apart.
            let name = cut(&name, room);
            used += width(&name);
            out.push(Span::styled(name, name_style));
            if room > used + 1 {
                let dir = cut_left(&dir, room - used - 1);
                used += 1 + width(&dir);
                out.push(Span::raw(" "));
                out.push(Span::styled(dir, Tone::Dim.style(&theme)));
            }
        }
        out.push(Span::raw(" ".repeat(room.saturating_sub(used) + 1)));
        out.push(Span::styled(counts, Tone::Dim.style(&theme)));
        out
    }

    /// UX 2.0's heading over the diff: the shown file's page, its
    /// number and title (or "Not committed yet" in the needs tone) with
    /// what the diff compares with at the right, its why under them in at
    /// most `WHY_LINES` lines, its watch line (what a reviewer should check)
    /// on one, and a rule; nothing without a story or UX
    /// 2.0.
    pub(super) fn page_head(&self, cols: usize) -> Vec<Vec<Span<'static>>> {
        let mut out = self.page_title(cols);
        out.extend(self.things_rows(cols));
        out
    }

    /// The page's lines over the diff with UX 2.0: its number and title
    /// with what the change is compared with at the right (in a page
    /// opened from the map, how much of its chapter is read and the way
    /// back), its why, its watch line, and a rule.
    pub(super) fn page_title(&self, cols: usize) -> Vec<Vec<Span<'static>>> {
        if !self.opts.ux2 {
            return Vec::new();
        }
        let (Some(story), Some(n)) = (&self.story, self.current().and_then(|e| self.home(&e.path)))
        else {
            return Vec::new();
        };
        let Some(page) = story.pages.get(n - 1) else {
            return Vec::new();
        };
        let theme = self.theme();
        let mut title = vec![Span::raw(" ")];
        if self.uncommitted_page() == Some(n) {
            title.push(Span::styled(page.title.clone(), Tone::Needs.bold(&theme)));
        } else if self.generated_page() == Some(n) {
            title.push(Span::styled(super::GENERATED_LAST, Tone::Dim.bold(&theme)));
        } else {
            title.push(Span::styled(
                self.page_heading(n),
                Tone::Accent.bold(&theme),
            ));
            title.push(Span::styled(
                format!("  {}", page.title),
                Tone::Text.bold(&theme),
            ));
        }
        let right = if self.in_page() {
            let (read, of) = self.read_of_shown();
            format!("✓ {read} of {of} read · g m the map ")
        } else {
            self.change
                .as_ref()
                .map(|c| format!("{} ", c.against()))
                .unwrap_or_default()
        };
        let room = cols.saturating_sub(width(&right) + 1);
        let mut first = kit::fit(title, room, Style::new());
        first.push(Span::raw(" "));
        first.push(Span::styled(right, Tone::Dim.style(&theme)));
        let mut out = vec![first];
        let mut why = wrap(&page.why, cols.saturating_sub(2).max(10));
        why.retain(|l| !l.is_empty());
        if why.len() > WHY_LINES {
            let rest = why[WHY_LINES - 1..].join(" ");
            why.truncate(WHY_LINES - 1);
            why.push(cut(&rest, cols.saturating_sub(2)));
        }
        for line in why {
            out.push(vec![
                Span::raw(" "),
                Span::styled(line, Tone::Dim.style(&theme)),
            ]);
        }
        if !page.watch.is_empty() {
            let mark = kit::mark(kit::Mark::Watch, 0, &theme);
            let room = cols.saturating_sub(width(&mark.content) + 2);
            out.push(vec![
                Span::raw(" "),
                mark,
                Span::raw(" "),
                Span::styled(cut(&page.watch, room), Tone::Dim.style(&theme)),
            ]);
        }
        out.push(vec![kit::rule(cols, &theme)]);
        out
    }

    /// UX 2.0's one bar over both sides of a file: its path (the folders
    /// dim, the name bold, where a renamed one came from after it), its
    /// counts, and at the right the two sides compared.
    pub(super) fn bar_ux2(&self, entry: &Entry, cols: usize) -> Vec<Span<'static>> {
        let theme = self.theme();
        let (dir, name) = match entry.path.rsplit_once('/') {
            Some((d, n)) => (format!("{d}/"), n.to_string()),
            None => (String::new(), entry.path.clone()),
        };
        // The counts and what happened to the file (which the counts alone
        // don't say for a binary or empty one) always shown, the path cut
        // to the room left, then the sides compared where they fit.
        let counts = if entry.binary {
            "   bin".to_string()
        } else {
            format!("   +{} −{}", entry.additions, entry.deletions)
        };
        let mut tail = vec![Span::styled(counts, Tone::Dim.style(&theme))];
        let status = match entry.status {
            'A' => Some(("added", Tone::Done)),
            'D' => Some(("deleted", Tone::Failed)),
            'R' => Some(("renamed", Tone::Working)),
            'C' => Some(("copied", Tone::Working)),
            'T' => Some(("type changed", Tone::Working)),
            _ => None,
        };
        if let Some((word, tone)) = status {
            tail.push(Span::styled(format!("  {word}"), tone.style(&theme)));
        }
        let tail_w: usize = tail.iter().map(|s| width(&s.content)).sum();
        let base = self.change.as_ref().map_or("", |c| c.base.as_str());
        // A commit of the history compares its parent with it.
        let short = |side: fn(&git::Change) -> &str| -> String {
            self.change
                .as_ref()
                .map(|c| side(c).chars().take(7).collect())
                .unwrap_or_default()
        };
        let parent = short(|c| c.left.as_str());
        let (from, to) = match entry.section {
            Section::Staged => ("HEAD", "index".to_string()),
            Section::Unstaged => ("index", "working tree".to_string()),
            Section::Committed if self.changed_since(entry) => (base, "working tree".into()),
            Section::Committed => match &self.title {
                Some(_) => (parent.as_str(), short(|c| c.right.as_str())),
                None => (base, "HEAD".into()),
            },
        };
        let right = format!("{from}  ⟷  {to} ");
        let right_w = width(&right) + 1;
        let name_w = width(&name);
        let show_right = cols >= 1 + name_w + tail_w + right_w;
        let room = if show_right { cols - right_w } else { cols };
        let path_room = room.saturating_sub(1 + tail_w);
        let renamed = entry
            .old_path
            .as_ref()
            .map(|old| format!(" ← {old}"))
            .unwrap_or_default();
        let mut out = vec![Span::raw(" ")];
        if width(&dir) + name_w + width(&renamed) <= path_room {
            out.push(Span::styled(dir, Tone::Dim.style(&theme)));
            out.push(Span::styled(name, Tone::Text.bold(&theme)));
            out.push(Span::styled(renamed, Tone::Dim.style(&theme)));
        } else if name_w < path_room {
            let dir = cut_left(&dir, path_room - name_w);
            out.push(Span::styled(dir, Tone::Dim.style(&theme)));
            out.push(Span::styled(name, Tone::Text.bold(&theme)));
        } else {
            out.push(Span::styled(cut(&name, path_room), Tone::Text.bold(&theme)));
        }
        out.extend(tail);
        let mut out = kit::fit(out, room, Style::new());
        if show_right {
            out.push(Span::raw(" "));
            out.push(Span::styled(right, Tone::Dim.style(&theme)));
        }
        out
    }

    /// The bar over a side: the file's path from the repository's root,
    /// its folders faint, its page before it; the old side's only for a
    /// file renamed, which names where it was.
    fn bar(&self, entry: &Entry, new_side: bool, cols: u16) -> Vec<(String, Style)> {
        let mut out: Vec<(String, Style)> = vec![(" ".into(), Style::new())];
        let path = if new_side {
            entry.path.clone()
        } else {
            entry.old().to_string()
        };
        if !new_side && self.side_widths().1 > 0 && entry.old_path.is_none() {
            return out;
        }
        let number = self.alone.or_else(|| {
            self.story
                .as_ref()
                .and_then(|s| s.page.get(&entry.path).copied())
        });
        let mut used = 1;
        if let (Some(story), Some(n), true) =
            (&self.story, number, new_side || self.side_widths().1 == 0)
            && let Some(page) = story.pages.get(n - 1)
        {
            let (k, m) = match self.told_map() {
                Some(s) => {
                    let (_, k, m) = story::page_number(s, n);
                    (k, m)
                }
                None => (n, story.pages.len()),
            };
            let text = format!("{k}/{m} {} · ", page.title);
            used += width(&text);
            out.push((text, self.comment()));
        }
        let (dir, name) = match path.rsplit_once('/') {
            Some((d, n)) => (format!("{d}/"), n.to_string()),
            None => (String::new(), path.clone()),
        };
        used += width(&dir) + width(&name);
        out.push((dir, self.faint()));
        out.push((name, Style::new().add_modifier(Modifier::BOLD)));
        // A file with changes not yet committed says so over the side that
        // shows them.
        if (new_side || self.side_widths().1 == 0) && self.changed_since(entry) {
            let text = "   ● uncommitted";
            used += width(text);
            out.push((text.into(), Style::new().fg(UNCOMMITTED)));
        }
        // A list too narrow for a whole why heads its page with the
        // first sentence: the whole why goes here, cut to fit.
        if self.list_width < SHORT_WHY
            && let (Some(story), Some(n)) = (&self.story, number)
            && let Some(page) = story.pages.get(n - 1)
            && !page.why.is_empty()
            && story::first_sentence(&page.why) != page.why
        {
            let room = (cols as usize).saturating_sub(used + 3);
            if room > 10 {
                out.push((
                    format!("   {}", switchboard_ui::draw::cut(&page.why, room)),
                    self.comment(),
                ));
            }
        }
        out
    }

    /// The diff, and its scroll bar in the column kept for it.
    fn draw_diff(&mut self, buf: &mut Buffer) -> Option<(u16, u16)> {
        let cursor = self.draw_diff_body(buf);
        self.body_top = Some(self.diff_area().y + self.head_height() as u16);
        if self.opts.scroll_bars && self.current().is_some() {
            let area = self.diff_area();
            let h = self.body_height();
            let (total, top) = self.diff_extent();
            let y = area.y + self.head_height() as u16;
            let x = area.right().saturating_sub(1);
            if kit::draw_scrollbar(buf, (x, y, h as u16), (total, top, h), &self.theme()) {
                self.diff_bar = Some((x, y, h));
            }
        }
        cursor
    }

    fn draw_diff_body(&mut self, buf: &mut Buffer) -> Option<(u16, u16)> {
        let area = self.diff_area();
        let Some(entry) = self.current().cloned() else {
            if let Some(change) = &self.change {
                let text = if self.alone.is_some()
                    && self.order.is_empty()
                    && self.path_filter.trim().is_empty()
                {
                    "This page's files aren't in the diff as it is now: they changed back since its commits.".to_string()
                } else if !self.path_filter.trim().is_empty() {
                    format!(
                        "No file's path matches “{}”. Backspace or esc widens it.",
                        self.path_filter.trim()
                    )
                } else if change.working() {
                    "No changes in the working tree.".to_string()
                } else if change.to_work_tree() {
                    format!("No changes between {} and the working tree.", change.base)
                } else if self.title.is_some() {
                    "This commit changes no files.".to_string()
                } else {
                    format!("No changes between {} and this branch.", change.base)
                };
                Self::put(buf, area.x + 1, 1, area.width, &text, self.faint());
            }
            return None;
        };
        let head = self.page_head(area.width as usize);
        for (i, spans) in head.iter().enumerate() {
            put_line(
                buf,
                area.x,
                area.y + i as u16,
                area.width,
                &Spans::from(spans.clone()),
            );
        }
        let bar_y = area.y + head.len() as u16;
        let body_y = bar_y + 1;
        // The heading and the bar, which stay as the diff scrolls.
        for y in area.y..body_y {
            let row = Rect::new(area.x, y, area.width, 1);
            self.spot(Pane::Head, row, area.x, i64::from(y), false);
        }
        let h = self.body_height();
        if self.one_scroll() {
            return self.draw_scroll(buf, bar_y, body_y, h);
        }
        let (ow, nw) = self.side_widths();
        let old_x = area.x;
        let new_x = if ow > 0 { area.x + ow + 1 } else { area.x };
        if self.opts.ux2 {
            put_line(
                buf,
                area.x,
                bar_y,
                area.width,
                &Spans::from(self.bar_ux2(&entry, area.width as usize)),
            );
        } else {
            for (new_side, x, w) in [(false, old_x, ow), (true, new_x, nw)] {
                if w == 0 {
                    continue;
                }
                let mut cx = x;
                for (text, style) in self.bar(&entry, new_side, w) {
                    cx = Self::put(buf, cx, bar_y, (x + w).saturating_sub(cx), &text, style);
                }
            }
        }
        let binary = self.files.get(&entry.key()).is_some_and(|s| s.binary);
        if binary {
            Self::put(
                buf,
                area.x + 1,
                body_y + 1,
                area.width,
                "A binary file: no text to compare.",
                self.faint(),
            );
            return None;
        }
        let n = self.screen_lines().len();
        self.cur = self.cur.min(n.saturating_sub(1));
        self.keep_cursor();
        self.top = self.top.min(n.saturating_sub(1));
        let (at, top) = (self.at, self.top);
        self.diff_moved((at, top), body_y);
        self.draw_rows(buf, at, top, h, body_y)
    }

    /// The files read as one scroll (UX 2.0): from the view's first line
    /// on, each file's lines, then a rule and the next file's bar, the bar
    /// of the file at the top kept over them all.
    fn draw_scroll(
        &mut self,
        buf: &mut Buffer,
        bar_y: u16,
        body_y: u16,
        h: usize,
    ) -> Option<(u16, u16)> {
        let area = self.diff_area();
        let n = self.screen_lines().len();
        self.cur = self.cur.min(n.saturating_sub(1));
        self.keep_cursor();
        let len = self.stream_len(self.view);
        self.top = self.top.min(len.saturating_sub(1));
        let theme = self.theme();
        let bar_of = |app: &Self, k: usize| {
            app.order
                .get(k)
                .and_then(|&i| app.entry(i))
                .map(|e| app.bar_ux2(e, area.width as usize))
                .unwrap_or_default()
        };
        put_line(
            buf,
            area.x,
            bar_y,
            area.width,
            &Spans::from(bar_of(self, self.view)),
        );
        let (mut k, mut t) = (self.view, self.top);
        self.diff_moved((k, t), body_y);
        let across = Rect::new(area.x, 0, self.body_width(), 1);
        let mut y = body_y;
        let bottom = body_y + h as u16;
        let mut cursor = None;
        while y < bottom && self.is_read(k) {
            let n = self.lines_of(k).len();
            if t < n {
                let take = (n - t).min((bottom - y) as usize);
                if let Some(c) = self.draw_rows(buf, k, t, take, y) {
                    cursor = Some(c);
                }
                y += take as u16;
                t += take;
            } else if k + 1 >= self.order.len() {
                break;
            } else if t == n {
                put_line(
                    buf,
                    area.x,
                    y,
                    area.width,
                    &Spans::from(kit::rule(area.width as usize, &theme)),
                );
                y += 1;
                t += 1;
            } else {
                put_line(
                    buf,
                    area.x,
                    y,
                    area.width,
                    &Spans::from(bar_of(self, k + 1)),
                );
                let row = Rect { y, ..across };
                self.spot(
                    Pane::Wide,
                    row,
                    area.x,
                    self.diff_base + i64::from(y),
                    false,
                );
                y += 1;
                k += 1;
                t = 0;
            }
        }
        cursor
    }

    /// Lines `from` to `from + count` of the list's file `k`, drawn from
    /// row `y0`; where the cursor goes when it is among them.
    fn draw_rows(
        &mut self,
        buf: &mut Buffer,
        k: usize,
        from: usize,
        count: usize,
        y0: u16,
    ) -> Option<(u16, u16)> {
        let area = self.diff_area();
        let entry = self.order.get(k).and_then(|&i| self.entry(i)).cloned()?;
        let lines = self.lines_of(k).to_vec();
        let shown = k == self.at;
        let (ow, nw) = self.widths_of(&entry);
        let old_x = area.x;
        let new_x = if ow > 0 { area.x + ow + 1 } else { area.x };
        let palette = self.palette();
        let dimmed = self.dimmed(&entry);
        let across = Rect::new(area.x, 0, self.body_width(), 1);
        let base = self.diff_base;
        let mut spots: Vec<Spot> = Vec::new();
        let s = self.files.get(&entry.key())?;
        let mut cursor = None;
        for (k, line) in lines.iter().skip(from).take(count).enumerate() {
            let y = y0 + k as u16;
            let place = base + i64::from(y);
            match line {
                Line::Fold { .. } | Line::Card(_) | Line::Also(..) => {
                    let whole = Rect { y, ..across };
                    spots.push(Spot::new(Pane::Wide, whole, area.x, place, false));
                }
                Line::Note { new_side, .. } => {
                    let (x, w) = if *new_side { (new_x, nw) } else { (old_x, ow) };
                    spots.push(Spot::new(
                        Pane::Wide,
                        Rect::new(x, y, w, 1),
                        x,
                        place,
                        false,
                    ));
                }
                Line::Pair { .. } => {}
            }
            match line {
                Line::Fold { count, .. } => {
                    let text = format!(" ⋯ {count} unchanged lines");
                    for x in area.x..area.right() {
                        buf[(x, y)].set_bg(Color::Indexed(if self.opts.light { 254 } else { 235 }));
                    }
                    Self::put(
                        buf,
                        area.x,
                        y,
                        area.width,
                        &text,
                        self.faint()
                            .bg(Color::Indexed(if self.opts.light { 254 } else { 235 })),
                    );
                }
                Line::Card(spans) | Line::Also(_, _, spans) => {
                    put_line(buf, area.x, y, area.width, &Spans::from(spans.clone()));
                }
                Line::Note { new_side, text } => {
                    let (x, w) = if *new_side { (new_x, nw) } else { (old_x, ow) };
                    Self::put(buf, x, y, w, text, self.comment());
                }
                Line::Pair { row, old, new } => {
                    let r = &s.rows[*row];
                    for (new_side, cell, x, w) in [(false, old, old_x, ow), (true, new, new_x, nw)]
                    {
                        if w == 0 {
                            continue;
                        }
                        let Cell::Text { line, range } = cell else {
                            continue;
                        };
                        let (text, hl, words) = if new_side {
                            (
                                &s.new[*line],
                                s.new_hl.as_ref().and_then(|h| h.get(*line)),
                                &r.new_words,
                            )
                        } else {
                            (
                                &s.old[*line],
                                s.old_hl.as_ref().and_then(|h| h.get(*line)),
                                &r.old_words,
                            )
                        };
                        let tinted = match (new_side, r.kind) {
                            (false, Kind::Changed | Kind::Removed) => Some((palette.0, palette.1)),
                            (true, Kind::Changed | Kind::Added) => Some((palette.2, palette.3)),
                            _ => None,
                        };
                        let dim = dimmed.contains(&(new_side, *line));
                        if let Some((line_bg, _)) = tinted {
                            for cx in x..x + w {
                                buf[(cx, y)].set_bg(line_bg);
                            }
                        }
                        // The line's number on its first screen line, faint,
                        // at the right of the gutter.
                        let gutter = s.gutter(w as usize) as u16;
                        if w > gutter {
                            // The piece's own cells: a line wrapped at a
                            // space ends short of the side's width.
                            let used: usize =
                                text[range.clone()].chars().map(sides::char_width).sum();
                            spots.push(
                                Spot::new(
                                    if new_side { Pane::New } else { Pane::Old },
                                    Rect::new(x + gutter, y, w - gutter, 1),
                                    x,
                                    place,
                                    range.start > 0,
                                )
                                .fitted(used as u16),
                            );
                        }
                        if gutter > 0 && range.start == 0 {
                            let number = (*line + 1).to_string();
                            let nx = x + gutter - 1 - width(&number) as u16;
                            let mut style = self.faint();
                            if let Some((line_bg, _)) = tinted {
                                style = style.bg(line_bg);
                            }
                            Self::put(buf, nx, y, gutter, &number, style);
                        }
                        let (x, w) = (x + gutter, w - gutter);
                        let mut cx = x;
                        for (i, c) in text[range.clone()].char_indices() {
                            let at = range.start + i;
                            let cw = sides::char_width(c) as u16;
                            if cw == 0 || cx + cw > x + w {
                                continue;
                            }
                            let mut style = Style::new();
                            if let Some(hl) =
                                hl.and_then(|runs| runs.iter().find(|h| h.range.contains(&at)))
                            {
                                style = style.fg(hl.color);
                            }
                            if let Some((line_bg, word_bg)) = tinted {
                                let in_word = words.iter().any(|w| w.contains(&at));
                                style = style.bg(if in_word { word_bg } else { line_bg });
                            }
                            if dim {
                                style = style.fg(Color::Indexed(240));
                            }
                            buf[(cx, y)].set_char(c).set_style(style);
                            cx += cw;
                        }
                    }
                }
            }
            // The lines selected, on the side with the focus, reversed as
            // Visual mode's highlight.
            if let Some(start) = self.visual
                && shown
                && self.focus != Focus::List
                && (start.min(self.cur)..=start.max(self.cur)).contains(&(from + k))
            {
                let (x, w) = if self.focus == Focus::Old {
                    (old_x, ow)
                } else {
                    (new_x, nw)
                };
                for cx in x..x + w {
                    buf[(cx, y)].modifier.insert(Modifier::REVERSED);
                }
            }
            let at_cursor = shown && from + k == self.cur;
            if at_cursor && self.focus != Focus::List {
                let (x, w) = if self.focus == Focus::Old {
                    (old_x, ow)
                } else {
                    (new_x, nw)
                };
                let gutter = s.gutter(w as usize) as u16;
                let (x, w) = (x + gutter, w - gutter);
                cursor = Some((x + (self.col as u16).min(w.saturating_sub(1)), y));
            }
        }
        self.spots.append(&mut spots);
        cursor
    }

    /// The columns the diff's rows take, less the scroll bar's.
    fn body_width(&self) -> u16 {
        let (ow, nw) = self.widths_of_alone(None);
        ow + 1 + nw
    }

    /// The lines of the hunks of another page than the page's, which a
    /// page dims, since it can't leave part of a file out; none in a file
    /// changed since its commits, whose story's hunks are at the committed
    /// lines.
    fn dimmed(&self, entry: &Entry) -> HashSet<(bool, usize)> {
        let mut out = HashSet::new();
        let (Some(page), Some(story)) = (self.alone, &self.story) else {
            return out;
        };
        if self.changed_since(entry) {
            return out;
        }
        let run = self.run_of(entry);
        for hunk in story
            .hunks
            .get(&entry.path)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter(|h| run.is_none_or(|r| h.run == r))
        {
            // The page's own hunks, and those it shows again from another
            // page, at full strength.
            let again = story
                .repeats
                .get(&hunk.id)
                .is_some_and(|shown| shown.contains(&page));
            if hunk.page == page || again {
                continue;
            }
            for (new_side, first, span) in [
                (false, hunk.old_start, hunk.old_count),
                (true, hunk.new_start, hunk.new_count),
            ] {
                for line in first.max(1)..first.max(1) + span {
                    out.insert((new_side, line - 1));
                }
            }
        }
        out
    }

    /// The old side's line and word shades, then the new side's, mixed over
    /// the terminal's own background as diffview.lua mixed them over
    /// Ghostty's, else over a white or Tomorrow Night's.
    pub(super) fn palette(&self) -> (Color, Color, Color, Color) {
        let normal = match self.ends() {
            Some((bg, _)) => bg,
            None if self.opts.light => 0xffffff,
            None => 0x1d1f21,
        };
        (
            blend(REMOVED, normal, 0.14),
            blend(REMOVED, normal, 0.55),
            blend(ADDED, normal, 0.14),
            blend(ADDED, normal, 0.55),
        )
    }

    /// The row of buttons at the bottom (easy mode's, and the diff alone's),
    /// drawn as the kit draws Switchboard's own buttons (board 118): each
    /// presses its keys in the list, the one under the mouse on the chosen
    /// surface, and alone a Settings button before Close, lit while the
    /// settings view is open. After them, the keys named.
    pub(super) fn draw_buttons(&mut self, buf: &mut Buffer) {
        let theme = self.theme();
        let y = self.rows.saturating_sub(1);
        // Alone, Settings and Close are always there: on a screen too
        // narrow for the row, the buttons before them give way.
        let bar = self.bar_buttons();
        let kept = if self.opts.alone { 2.min(bar.len()) } else { 0 };
        let tail: u16 = bar[bar.len() - kept..]
            .iter()
            .map(|(_, label)| width(label) as u16 + 3)
            .sum();
        // The Map / Files switch leads the row (board 125) only where every
        // button still fits after it; else m and a right click switch.
        let buttons: u16 = bar.iter().map(|(_, label)| width(label) as u16 + 3).sum();
        let switch = {
            let (spans, _) = kit::choice_switch(&["Map", "Files"], None, None, &[], false, &theme);
            kit::spans_width(&spans) as u16 + 1
        };
        let mut x = if self.settings.is_none() && 1 + switch + buttons < self.cols {
            self.draw_switch(buf, 1, y)
        } else {
            1
        };
        let mut full = false;
        for (i, (button, label)) in bar.iter().copied().enumerate() {
            let w = width(label) as u16 + 2;
            let last = i + kept >= bar.len();
            full |= !last && x + w + tail >= self.cols;
            if !last && full {
                continue;
            }
            if last && x + w > self.cols {
                break;
            }
            let lit = button == Button::Settings && self.settings.is_some();
            let spans = own::button(label, self.hover == Some(button), lit, self.round, &theme);
            put_line(buf, x, y, w, &Spans::from(spans));
            self.buttons.push((button, Rect::new(x, y, w, 1)));
            x += w + 1;
        }
        let help = if self.settings.is_some() {
            "Esc back to the diff".to_string()
        } else if self.opts.easy && self.opts.key_help {
            let mut help = "↑↓ choose a file · Enter open it · ]c [c chapters · ]p [p pages · E widen · yl copy a link · R refresh".to_string();
            if self.opts.ask {
                help.push_str(" · ga ask Claude");
            }
            if self.opts.alone {
                help.push_str(" · M-m settings");
            }
            if self.mapped() {
                help.insert_str(0, "g m map · g f files · ");
            }
            help.push_str(" · q close");
            help
        } else if self.opts.alone {
            "M-m settings".to_string()
        } else {
            return;
        };
        Self::put(
            buf,
            x + 1,
            y,
            self.cols.saturating_sub(x + 1),
            &help,
            self.faint(),
        );
    }

    /// A box with a rounded edge in the accent colour, its title in the top
    /// edge and its footer in the bottom, cleared inside; gives its inside.
    fn frame(buf: &mut Buffer, area: Rect, title: &str, footer: &str) -> Rect {
        let area = area.intersection(buf.area);
        if area.width < 4 || area.height < 3 {
            return Rect::default();
        }
        let edge = Style::new().fg(ACCENT);
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                buf[(x, y)].reset();
            }
        }
        let (l, r, t, b) = (area.left(), area.right() - 1, area.top(), area.bottom() - 1);
        for x in l..=r {
            buf[(x, t)].set_symbol("─").set_style(edge);
            buf[(x, b)].set_symbol("─").set_style(edge);
        }
        for y in t..=b {
            buf[(l, y)].set_symbol("│").set_style(edge);
            buf[(r, y)].set_symbol("│").set_style(edge);
        }
        buf[(l, t)].set_symbol("╭");
        buf[(r, t)].set_symbol("╮");
        buf[(l, b)].set_symbol("╰");
        buf[(r, b)].set_symbol("╯");
        let room = area.width.saturating_sub(4);
        for (y, text, style) in [
            (t, title, edge.add_modifier(Modifier::BOLD)),
            (b, footer, Style::new().fg(Color::Indexed(244))),
        ] {
            if text.is_empty() {
                continue;
            }
            let text = switchboard_ui::draw::cut(text, room as usize);
            let x = l + (area.width.saturating_sub(width(&text) as u16)) / 2;
            Self::put(buf, x, y, room, &text, style);
        }
        Rect::new(l + 1, t + 1, area.width - 2, area.height - 2)
    }

    /// A row of buttons along a box's top line inside.
    fn box_buttons(&mut self, buf: &mut Buffer, inner: Rect, buttons: &[(Button, &str)]) {
        let mut x = inner.x + 1;
        let theme = self.theme();
        for (button, label) in buttons {
            let w = width(label) as u16 + 2;
            let hover = self.hover == Some(*button);
            let spans = own::button(label, hover, false, self.round, &theme);
            put_line(buf, x, inner.y, w, &Spans::from(spans));
            self.buttons.push((*button, Rect::new(x, inner.y, w, 1)));
            x += w + 1;
        }
    }

    /// The box a question is typed in, in the middle of the diff.
    fn draw_ask_box(&mut self, buf: &mut Buffer) -> Option<(u16, u16)> {
        let w = 80.min(self.cols.saturating_sub(8)) + 2;
        let h = 7;
        let x = self.cols.saturating_sub(w) / 2;
        let y = self.rows.saturating_sub(h) / 2;
        let area = Rect::new(x, y, w, h);
        let inner = Self::frame(
            buf,
            area,
            " Ask about this diff ",
            " Enter ask · Shift-Enter new line · Esc cancel ",
        );
        self.ask_rect = Some(area);
        self.box_buttons(
            buf,
            inner,
            &[(Button::AskSend, "Ask"), (Button::AskCancel, "Cancel")],
        );
        let (text, pos) = self
            .ask_box
            .as_ref()
            .map(|b| (b.text.clone(), b.pos))
            .unwrap_or_default();
        let room = (inner.width.saturating_sub(2) as usize).max(1);
        // The text wrapped at the box's width, and the typing cursor's row
        // and column in it, a character after a full row starting the next.
        let mut lines = vec![String::new()];
        let (mut row, mut col) = (0, 0);
        let mut at = None;
        for (i, ch) in text.char_indices() {
            if i == pos {
                at = Some(if col == room {
                    (row + 1, 0)
                } else {
                    (row, col)
                });
            }
            if ch == '\n' || col == room {
                lines.push(String::new());
                row += 1;
                col = 0;
                if ch == '\n' {
                    continue;
                }
            }
            lines[row].push(ch);
            col += 1;
        }
        let (row, col) = at.unwrap_or(if col == room {
            (row + 1, 0)
        } else {
            (row, col)
        });
        let shown = (inner.height.saturating_sub(1)) as usize;
        let from = (row + 1).saturating_sub(shown);
        for (i, line) in lines.iter().skip(from).take(shown).enumerate() {
            Self::put(
                buf,
                inner.x + 1,
                inner.y + 1 + i as u16,
                inner.width.saturating_sub(2),
                line,
                Style::new(),
            );
        }
        let before: String = lines
            .get(row)
            .map(|l| l.chars().take(col).collect())
            .unwrap_or_default();
        let x = width(&before) as u16;
        Some((
            inner.x + 1 + x.min(inner.width.saturating_sub(3)),
            inner.y + 1 + (row - from) as u16,
        ))
    }

    /// The answer's window, on the right half, titled with the question.
    fn draw_answer(&mut self, buf: &mut Buffer) {
        let w = self.answer_width();
        let h = self.rows.saturating_sub(2).max(5);
        let x = self.cols.saturating_sub(w + 1);
        let area = Rect::new(x, 1, w, h.min(self.rows.saturating_sub(1)));
        let question = self
            .answer
            .as_ref()
            .map(|a| a.question.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        let title = format!(
            " {} ",
            switchboard_ui::draw::cut(&question, w.saturating_sub(6) as usize)
        );
        let inner = Self::frame(
            buf,
            area,
            &title,
            " q close · ga ask again · gf open path:line · yy copy ",
        );
        self.answer_rect = Some(area);
        self.box_buttons(
            buf,
            inner,
            &[
                (Button::AnswerClose, "Close"),
                (Button::AnswerAgain, "Ask again"),
                (Button::AnswerCopy, "Copy"),
            ],
        );
        let lines = self.answer_lines(w);
        let body = inner.height.saturating_sub(1) as usize;
        let Some(a) = &mut self.answer else {
            return;
        };
        if a.cursor < a.top {
            a.top = a.cursor;
        } else if a.cursor >= a.top + body {
            a.top = a.cursor + 1 - body;
        }
        a.top = a.top.min(lines.len().saturating_sub(1));
        let (top, cursor) = (a.top, a.cursor);
        // The rows shown, kept for the pointer's shape over them.
        self.answer_shown = lines
            .iter()
            .skip(top)
            .take(body)
            .map(|(plain, _)| plain.clone())
            .collect();
        // Its scroll bar in the column the text leaves at its right.
        self.answer_bar = None;
        if self.opts.scroll_bars {
            let (x, y) = (inner.right().saturating_sub(1), inner.y + 1);
            let theme = self.theme();
            if kit::draw_scrollbar(buf, (x, y, body as u16), (lines.len(), top, body), &theme) {
                self.answer_bar = Some((x, y, body));
            }
        }
        let selected = self
            .answer
            .as_ref()
            .and_then(|a| a.visual)
            .map(|from| from.min(cursor)..=from.max(cursor));
        let text = Rect::new(inner.x + 1, 0, inner.width.saturating_sub(2), 1);
        for (k, (_, spans)) in lines.iter().skip(top).take(body).enumerate() {
            let y = inner.y + 1 + k as u16;
            let row = Rect { y, ..text };
            self.spot(Pane::Answer, row, inner.x, (top + k) as i64, false);
            let mut x = inner.x + 1;
            for (text, style) in spans {
                x = Self::put(
                    buf,
                    x,
                    y,
                    (inner.right().saturating_sub(1)).saturating_sub(x),
                    text,
                    *style,
                );
            }
            if top + k == cursor {
                for cx in inner.x..inner.right() {
                    buf[(cx, y)].set_bg(Color::Indexed(if self.opts.light { 254 } else { 236 }));
                }
            }
            if selected.as_ref().is_some_and(|s| s.contains(&(top + k))) {
                for cx in inner.x..inner.right() {
                    buf[(cx, y)].modifier.insert(Modifier::REVERSED);
                }
            }
        }
    }
}

/// Text cut to `cols` columns from its start, an ellipsis in place of what
/// went, so its end stays.
fn cut_left(text: &str, cols: usize) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    if width(text) <= cols {
        return text.to_string();
    }
    if cols == 0 {
        return String::new();
    }
    let mut kept: Vec<&str> = Vec::new();
    let mut used = 0;
    for g in text.graphemes(true).rev() {
        let w = width(g);
        if used + w > cols - 1 {
            break;
        }
        kept.push(g);
        used += w;
    }
    kept.reverse();
    format!("…{}", kept.concat())
}

#[cfg(test)]
mod tests {
    use super::cut_left;

    #[test]
    fn a_path_cut_from_its_start_keeps_the_folders_nearest_its_name() {
        assert_eq!(cut_left("src/components/alpha/", 8), "…/alpha/");
        assert_eq!(cut_left("src/", 8), "src/");
    }
}
