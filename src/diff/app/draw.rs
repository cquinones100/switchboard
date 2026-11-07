//! Drawing the diff's screen, split from app.rs for its length: the list
//! with its chapters' headings, the two sides with their bars and notes,
//! the cover while the story is told, easy mode's buttons, and the boxes
//! for a question and its answer, into a cell buffer, as
//! nvim/lua/plugins/diffview.lua's highlights, winbars and floats drew them.

use std::collections::HashSet;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use super::{
    ACCENT, ADDED, App, BUTTONS, Button, COVER_BLANK, Cell, Focus, Line, ListRow, REMOVED,
    SHORT_WHY, Telling, blend,
};
use crate::diff::git::Entry;
use crate::diff::sides::{self, Kind};
use crate::diff::story::{self, Look as StoryLook};
use crate::ui::draw::{BoxLine, BoxSpec, Place, draw_box, width, wrap};
use crate::util;

impl App {
    /// Draws the whole screen into `buf`; gives where the cursor goes.
    pub fn draw(&mut self, buf: &mut Buffer) -> Option<(u16, u16)> {
        // A commit of the history draws itself in this one's place.
        if let Some(h) = &mut self.history {
            let cursor = h.app.draw(buf);
            self.out.append(&mut h.app.out);
            return cursor;
        }
        let area = buf.area;
        self.cols = area.width.max(20);
        self.rows = area.height.max(5);
        self.buttons.clear();
        self.ask_rect = None;
        self.answer_rect = None;
        if let Some(text) = self.failed.clone() {
            self.centred(
                buf,
                &[
                    (text, Style::new().fg(Color::Indexed(203))),
                    (String::new(), Style::new()),
                    ("Press any key to close.".into(), self.faint()),
                ],
            );
            return None;
        }
        if let Some(text) = self.loading.clone() {
            let line = format!("{} {text}", self.spinner());
            self.centred(buf, &[(line, Style::new().add_modifier(Modifier::BOLD))]);
            return None;
        }
        if self.cover.is_some() && self.telling == Telling::Waiting {
            self.draw_cover(buf);
            return None;
        }
        self.draw_list(buf);
        let mut cursor = self.draw_diff(buf);
        if self.opts.easy {
            self.draw_buttons(buf);
        }
        if self.ask_box.is_some() {
            cursor = self.draw_ask_box(buf);
        } else if self.answer.is_some() {
            self.draw_answer(buf);
            cursor = None;
        }
        if let Some((text, _)) = self.message.clone() {
            let lines: Vec<BoxLine> = text
                .lines()
                .map(|l| BoxLine::text(l.to_string(), Style::new().fg(Color::Indexed(252))))
                .collect();
            let mut spec = BoxSpec::new("", &lines);
            spec.width = 10;
            spec.place = Place::Center;
            draw_box(buf, &spec, &crate::ui::Look::default());
            cursor = None;
        }
        if self.focus == Focus::List {
            cursor = None;
        }
        cursor
    }

    fn faint(&self) -> Style {
        Style::new().fg(Color::Indexed(if self.opts.light { 246 } else { 243 }))
    }

    fn comment(&self) -> Style {
        Style::new()
            .fg(Color::Indexed(if self.opts.light { 244 } else { 245 }))
            .add_modifier(Modifier::ITALIC)
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
    /// then what is read and each chapter as Haiku names it, or Haiku's
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
            (block, "Esc shows the diff now · t shows the chapters")
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

    fn draw_list(&mut self, buf: &mut Buffer) {
        let w = self.list_width.min(self.cols.saturating_sub(10));
        let h = self.diff_area().height as usize;
        let rows = self.list_rows();
        // The cursor's file in sight, with its heading where there's room,
        // once it has moved (or the story has moved its row): the wheel
        // scrolls the list past it.
        let row = rows.iter().position(|r| *r == ListRow::File(self.at));
        if row != self.list_followed
            && let Some(at) = row
        {
            self.list_followed = row;
            let mut first = at;
            while first > 0 && matches!(rows[first - 1], ListRow::Heading(..)) {
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
        for (y, row) in rows.iter().skip(self.list_top).take(h).enumerate() {
            let y = y as u16;
            match row {
                ListRow::Header(section) => {
                    let n = self
                        .order
                        .iter()
                        .filter(|&&i| entries.get(i).is_some_and(|e| e.section == *section))
                        .count();
                    let text = match (&self.title, self.page, &self.story) {
                        (Some((title, _)), _, _) => title.clone(),
                        (None, Some(p), Some(s)) => {
                            format!(" Page {p} of {} ({n})", s.chapters.len())
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
                        StoryLook::Title => Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                        StoryLook::Comment => self.comment(),
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
                    let cw = width(&counts) as u16;
                    let (dir, name) = match e.path.rsplit_once('/') {
                        Some((d, n)) => (format!("{d}/"), n.to_string()),
                        None => (String::new(), e.path.clone()),
                    };
                    let room = w.saturating_sub(cw + 1);
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
        if self.order.is_empty() {
            Self::put(buf, 1, 2, w.saturating_sub(1), "No changes", self.faint());
        }
    }

    /// The bar over a side: the file's path from the repository's root,
    /// its folders faint, its chapter before it; the old side's only for a
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
        let number = self.page.or_else(|| {
            self.story
                .as_ref()
                .and_then(|s| s.chapter.get(&entry.path).copied())
        });
        let mut used = 1;
        if let (Some(story), Some(n), true) =
            (&self.story, number, new_side || self.side_widths().1 == 0)
            && let Some(chapter) = story.chapters.get(n - 1)
        {
            let text = format!("{n}/{} {} · ", story.chapters.len(), chapter.title);
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
        // A list too narrow for a whole why heads its chapter with the
        // first sentence: the whole why goes here, cut to fit.
        if self.list_width < SHORT_WHY
            && let (Some(story), Some(n)) = (&self.story, number)
            && let Some(chapter) = story.chapters.get(n - 1)
            && !chapter.why.is_empty()
            && story::first_sentence(&chapter.why) != chapter.why
        {
            let room = (cols as usize).saturating_sub(used + 3);
            if room > 10 {
                out.push((
                    format!("   {}", crate::ui::draw::cut(&chapter.why, room)),
                    self.comment(),
                ));
            }
        }
        out
    }

    fn draw_diff(&mut self, buf: &mut Buffer) -> Option<(u16, u16)> {
        let area = self.diff_area();
        let Some(entry) = self.current().cloned() else {
            if let Some(change) = &self.change {
                let text = if change.working() {
                    "No changes in the working tree.".to_string()
                } else if self.title.is_some() {
                    "This commit changes no files.".to_string()
                } else {
                    format!("No changes between {} and this branch.", change.base)
                };
                Self::put(buf, area.x + 1, 1, area.width, &text, self.faint());
            }
            return None;
        };
        let (ow, nw) = self.side_widths();
        let old_x = area.x;
        let new_x = if ow > 0 { area.x + ow + 1 } else { area.x };
        for (new_side, x, w) in [(false, old_x, ow), (true, new_x, nw)] {
            if w == 0 {
                continue;
            }
            let mut cx = x;
            for (text, style) in self.bar(&entry, new_side, w) {
                cx = Self::put(buf, cx, area.y, (x + w).saturating_sub(cx), &text, style);
            }
        }
        let binary = self.files.get(&entry.key()).is_some_and(|s| s.binary);
        if binary {
            Self::put(
                buf,
                area.x + 1,
                area.y + 2,
                area.width,
                "A binary file: no text to compare.",
                self.faint(),
            );
            return None;
        }
        let lines = self.screen_lines().to_vec();
        let h = self.body_height();
        self.cur = self.cur.min(lines.len().saturating_sub(1));
        self.keep_cursor();
        self.top = self.top.min(lines.len().saturating_sub(1));
        let palette = self.palette();
        let dimmed = self.dimmed(&entry);
        let s = self.files.get(&entry.key())?;
        let mut cursor = None;
        for (k, line) in lines.iter().skip(self.top).take(h).enumerate() {
            let y = area.y + 1 + k as u16;
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
            if let Some(from) = self.visual
                && self.focus != Focus::List
                && (from.min(self.cur)..=from.max(self.cur)).contains(&(self.top + k))
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
            let at_cursor = self.top + k == self.cur;
            if at_cursor && self.focus != Focus::List {
                let (x, w) = if self.focus == Focus::Old {
                    (old_x, ow)
                } else {
                    (new_x, nw)
                };
                cursor = Some((x + (self.col as u16).min(w.saturating_sub(1)), y));
            }
        }
        cursor
    }

    /// The lines of the hunks of another chapter than the page's, which a
    /// page dims, since it can't leave part of a file out.
    fn dimmed(&self, entry: &Entry) -> HashSet<(bool, usize)> {
        let mut out = HashSet::new();
        let (Some(page), Some(story)) = (self.page, &self.story) else {
            return out;
        };
        let run = self.run_of(entry);
        for hunk in story
            .hunks
            .get(&entry.path)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter(|h| run.is_none_or(|r| h.run == r))
        {
            if hunk.chapter == page {
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

    /// The old side's line and word shades, then the new side's.
    fn palette(&self) -> (Color, Color, Color, Color) {
        let normal = if self.opts.light { 0xffffff } else { 0x1d1f21 };
        (
            blend(REMOVED, normal, 0.14),
            blend(REMOVED, normal, 0.55),
            blend(ADDED, normal, 0.14),
            blend(ADDED, normal, 0.55),
        )
    }

    fn button_style(&self, lit: bool) -> Style {
        if lit {
            Style::new()
                .bg(ACCENT)
                .fg(Color::Indexed(232))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new()
                .bg(Color::Indexed(if self.opts.light { 252 } else { 238 }))
                .fg(Color::Indexed(if self.opts.light { 235 } else { 252 }))
        }
    }

    /// Easy mode's row of buttons for the list's keys, then the keys named.
    fn draw_buttons(&mut self, buf: &mut Buffer) {
        let y = self.rows.saturating_sub(1);
        let mut x = 1;
        let shown: Vec<&(&str, &str)> = BUTTONS
            .iter()
            .filter(|(_, k)| self.opts.ask || *k != "ga")
            .collect();
        for (i, (label, _)) in shown.iter().enumerate() {
            let text = format!(" {label} ");
            let w = width(&text) as u16;
            if x + w >= self.cols {
                break;
            }
            let lit = self.hover == Some(Button::Bar(i));
            Self::put(buf, x, y, w, &text, self.button_style(lit));
            self.buttons.push((Button::Bar(i), Rect::new(x, y, w, 1)));
            x += w + 1;
        }
        if self.opts.key_help {
            let mut help = "↑↓ choose a file · Enter open it · ]c [c chapters · ]p [p pages · E widen · yl copy a link".to_string();
            if self.opts.ask {
                help.push_str(" · ga ask Claude");
            }
            help.push_str(" · q close");
            Self::put(
                buf,
                x + 1,
                y,
                self.cols.saturating_sub(x + 1),
                &help,
                self.faint(),
            );
        }
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
            let text = crate::ui::draw::cut(text, room as usize);
            let x = l + (area.width.saturating_sub(width(&text) as u16)) / 2;
            Self::put(buf, x, y, room, &text, style);
        }
        Rect::new(l + 1, t + 1, area.width - 2, area.height - 2)
    }

    /// A row of buttons along a box's top line inside.
    fn box_buttons(&mut self, buf: &mut Buffer, inner: Rect, buttons: &[(Button, &str)]) {
        let mut x = inner.x + 1;
        for (button, label) in buttons {
            let text = format!(" {label} ");
            let w = width(&text) as u16;
            let lit = self.hover == Some(*button);
            Self::put(buf, x, inner.y, w, &text, self.button_style(lit));
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
            crate::ui::draw::cut(&question, w.saturating_sub(6) as usize)
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
        for (k, (_, spans)) in lines.iter().skip(top).take(body).enumerate() {
            let y = inner.y + 1 + k as u16;
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
        }
    }
}
