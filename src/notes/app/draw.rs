//! Drawing the notes app, split from app.rs for its length: the list on the
//! left with its cursor's row lit, the note on the right with its toolbar,
//! its line numbers relative to the cursor's line, its lines wrapped at
//! words and indented under themselves (Neovim's `linebreak` and
//! `breakindent`), its folds, and its pictures under their lines; at the
//! bottom the key help line in easy mode and the line Vim's messages and
//! prompts take; and the boxes over it all. Each cell of a note's line
//! knows the byte it came from, so the cursor and a click find their place.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::{App, Focus, Shown, list_width};
use crate::notes::easy;
use crate::notes::editor::Mode;
use crate::notes::images::Picture;
use crate::notes::render::{self, Cell, Hl};
use crate::notes::sidebar::Look as RowLook;
use crate::ui::draw::{put_line, width};

/// Lines kept above and below the cursor (the config's `scrolloff`).
const SCROLLOFF: usize = 8;

/// A line's cells broken into the pieces a window `cols` wide shows, at a
/// blank where one fits (`linebreak`), each after the first indented as the
/// line is (`breakindent`).
fn wrap(cells: Vec<Cell>, cols: usize) -> Vec<(Vec<Cell>, u16)> {
    let cols = cols.max(4);
    let indent = cells
        .iter()
        .take_while(|c| c.text.trim().is_empty() && c.col.is_some())
        .map(|c| c.width)
        .sum::<usize>()
        .min(cols / 2);
    let mut out: Vec<(Vec<Cell>, u16)> = Vec::new();
    let mut cur: Vec<Cell> = Vec::new();
    let mut used = 0;
    for cell in cells {
        let room = if out.is_empty() { cols } else { cols - indent };
        if used + cell.width > room && !cur.is_empty() {
            let at = cur.iter().rposition(|c| c.text == " ").filter(|&k| k > 0);
            let rest = match at {
                Some(k) => cur.split_off(k + 1),
                None => Vec::new(),
            };
            let pad = if out.is_empty() { 0 } else { indent as u16 };
            out.push((std::mem::replace(&mut cur, rest), pad));
            used = cur.iter().map(|c| c.width).sum();
        }
        used += cell.width;
        cur.push(cell);
    }
    let pad = if out.is_empty() { 0 } else { indent as u16 };
    out.push((cur, pad));
    out
}

impl App {
    /// Draws the whole screen into `buf`; gives where the cursor goes.
    pub fn draw(&mut self, buf: &mut Buffer) -> Option<(u16, u16)> {
        let area = buf.area;
        self.cols = area.width.max(20);
        self.rows = area.height.max(5);
        if let Some(pid) = self.locked_by {
            self.draw_locked(buf, pid);
            return None;
        }
        let bottom = 1 + u16::from(self.opts.easy && self.opts.key_help);
        let body_h = area.height.saturating_sub(bottom);
        let list_w = if self.list_shown {
            list_width(area.width).min(area.width.saturating_sub(10))
        } else {
            0
        };
        self.geo.list = Rect::new(0, 0, list_w, body_h);
        let note_x = if list_w > 0 { list_w + 1 } else { 0 };
        self.geo.note = Rect::new(note_x, 0, area.width.saturating_sub(note_x), body_h);
        let mut cursor = None;
        if list_w > 0 {
            self.draw_list(buf);
            let edge = Style::new().fg(Color::Indexed(if self.opts.light { 250 } else { 238 }));
            for y in 0..body_h {
                buf[(list_w, y)].set_symbol("│").set_style(edge);
            }
        }
        let note_cursor = self.draw_note(buf);
        if self.focus == Focus::Note {
            cursor = note_cursor;
        }
        cursor = self.draw_bottom(buf, body_h).or(cursor);
        if self.focus == Focus::List && self.list_shown && self.prompt.is_none() {
            cursor = None;
        }
        for layer in &mut self.layers {
            layer.overlay.draw(buf, &self.look);
        }
        if let Some(layer) = self.layers.last() {
            cursor = layer.overlay.cursor();
        }
        cursor
    }

    /// Another app has the notes open: this one says so, and q closes it,
    /// rather than start and write over that one's notes.
    fn draw_locked(&self, buf: &mut Buffer, pid: i64) {
        let lines = [
            String::new(),
            format!("  Notes is open already, in another window (process {pid})."),
            String::new(),
            "  Two of them would write over each other's notes as you type, so this".into(),
            "  one hasn't opened them. Use that one, or quit it and open Notes again.".into(),
            String::new(),
            "  Press q to close this.".into(),
        ];
        for (i, l) in lines.iter().enumerate() {
            if (i as u16) < buf.area.height {
                buf.set_stringn(0, i as u16, l, buf.area.width as usize, Style::new());
            }
        }
    }

    fn list_style(&self, look: RowLook) -> Style {
        let light = self.opts.light;
        let c = |dark: u8, lite: u8| Color::Indexed(if light { lite } else { dark });
        match look {
            RowLook::Header => Style::new().fg(c(255, 232)).add_modifier(Modifier::BOLD),
            RowLook::Section => Style::new().fg(c(81, 31)),
            RowLook::Current => Style::new().bg(c(239, 252)),
            RowLook::Open => Style::new().bg(c(24, 153)).fg(c(255, 232)),
            RowLook::Title => Style::new().add_modifier(Modifier::BOLD),
            RowLook::Detail | RowLook::Faint => Style::new().fg(c(244, 243)),
            RowLook::Tag => Hl::Tag.style(light),
            RowLook::Rule => Style::new().fg(c(238, 250)),
            RowLook::Plain => Style::new(),
        }
    }

    fn draw_list(&mut self, buf: &mut Buffer) {
        let r = self.geo.list;
        let h = r.height as usize;
        // The cursor in sight once it has moved; the wheel scrolls past it.
        if self.list_followed != Some(self.list_at) {
            self.list_followed = Some(self.list_at);
            if self.list_at < self.list_top {
                self.list_top = self.list_at;
            } else if h > 0 && self.list_at >= self.list_top + h {
                self.list_top = self.list_at + 1 - h;
            }
        }
        self.list_top = self.list_top.min(self.list_rows.len().saturating_sub(1));
        for (i, row) in self
            .list_rows
            .iter()
            .enumerate()
            .skip(self.list_top)
            .take(h)
        {
            let y = r.y + (i - self.list_top) as u16;
            let mut style = self.list_style(row.look);
            let lit = i == self.list_at;
            if matches!(row.look, RowLook::Current | RowLook::Open) || lit {
                let bg = if lit {
                    if self.focus == Focus::List {
                        self.look.theme.highlight
                    } else {
                        self.look.theme.panel
                    }
                } else {
                    style.bg.unwrap_or(Color::Reset)
                };
                if !(matches!(row.look, RowLook::Open) && !lit) {
                    style = style.bg(bg);
                }
                buf.set_style(
                    Rect::new(r.x, y, r.width, 1),
                    Style::new().bg(style.bg.unwrap_or(bg)),
                );
            }
            buf.set_stringn(r.x, y, &row.text, r.width as usize, style);
        }
    }

    /// The rows a buffer row takes on the screen, and its pieces.
    fn pieces(&mut self, row: usize, ctx: &Ctx) -> Vec<Shown> {
        let line = &ctx.lines[row];
        if let Some(Some(hidden)) = ctx.folded.get(&row) {
            let text = line.trim_start_matches('#').trim().to_string();
            return vec![Shown::Fold {
                row,
                text: format!(
                    "▸ {text}  ({hidden} line{})",
                    if *hidden == 1 { "" } else { "s" }
                ),
            }];
        }
        let raw = row == ctx.cursor_row && ctx.raw;
        let extra = ctx
            .selected
            .get(&row)
            .map(|&(s, e)| vec![(s, e, Hl::Visual)])
            .unwrap_or_default();
        let cells = render::cells(line, &ctx.looks[row], raw, &extra, self.opts.light);
        let bg = ctx.looks[row].row_bg;
        let mut out: Vec<Shown> = wrap(cells, ctx.text_w as usize)
            .into_iter()
            .enumerate()
            .map(|(i, (cells, indent))| Shown::Text {
                row,
                cells,
                first: i == 0,
                indent,
                bg,
            })
            .collect();
        if line.contains("![")
            && let Some(note) = self.current_path().map(|p| p.to_path_buf())
        {
            let pics = self.images.pictures(
                self.store.root(),
                &note,
                line,
                ctx.kinds[row],
                ctx.text_w,
                ctx.text_h,
            );
            for pic in pics {
                for r in 0..pic.rows {
                    out.push(Shown::Picture {
                        row,
                        pic: pic.clone(),
                        r,
                    });
                }
            }
        }
        out
    }

    fn draw_note(&mut self, buf: &mut Buffer) -> Option<(u16, u16)> {
        let note = self.geo.note;
        self.geo.toolbar.clear();
        self.geo.shown.clear();
        if self.open.is_none() || note.width < 4 || note.height < 2 {
            self.geo.text = Rect::default();
            return None;
        }
        let mut top_y = note.y;
        if self.opts.toolbar {
            self.geo.toolbar = easy::toolbar(note.x, note.y, note.width);
            for (i, r) in &self.geo.toolbar {
                let lit = self.hover_button == Some(*i);
                let style = if lit {
                    Style::new().bg(Color::Indexed(81)).fg(Color::Indexed(16))
                } else {
                    Style::new()
                        .bg(Color::Indexed(if self.opts.light { 252 } else { 237 }))
                        .fg(Color::Indexed(if self.opts.light { 235 } else { 252 }))
                };
                buf.set_stringn(
                    r.x,
                    r.y,
                    format!(" {} ", easy::BUTTONS[*i].label),
                    r.width as usize,
                    style,
                );
            }
            top_y += 1;
        }
        let lines = self.editor.doc.text.lines();
        let kinds = render::kinds(&lines);
        let titles = &self.titles;
        let exists = |t: &str| titles.contains(&t.trim().to_lowercase());
        let drawing = render::Drawing {
            strike_checked: self.opts.strike_checked,
            note_exists: &exists,
        };
        let looks = render::looks(&lines, &kinds, &drawing);
        let number_w = if self.opts.numbers {
            (lines.len().to_string().len().max(3) + 1) as u16
        } else {
            0
        };
        let text = Rect::new(
            note.x + number_w,
            top_y,
            note.width.saturating_sub(number_w + 1),
            note.bottom().saturating_sub(top_y),
        );
        self.geo.text = text;
        self.editor.height = text.height as usize;
        let visible = self.editor.folds.visible(&kinds);
        let folded: std::collections::HashMap<usize, Option<usize>> =
            visible.iter().copied().collect();
        // The cursor on a row in a closed fold is on the fold's heading.
        let mut cursor_row = self.editor.doc.row;
        if let Some((s, _)) = self.editor.folds.closed_at(&kinds, cursor_row) {
            cursor_row = s;
        }
        let mut selected = std::collections::HashMap::new();
        if let Some(sel) = self.editor.selection() {
            for (r, line) in lines
                .iter()
                .enumerate()
                .take(sel.end.0 + 1)
                .skip(sel.start.0)
            {
                let len = line.len();
                let s = if sel.linewise || r > sel.start.0 {
                    0
                } else {
                    sel.start.1
                };
                let e = if sel.linewise || r < sel.end.0 {
                    len
                } else {
                    sel.end.1
                };
                selected.insert(r, (s, e.max(s)));
            }
        }
        let mut ctx = Ctx {
            raw: self.focus == Focus::Note
                && matches!(self.editor.mode, Mode::Insert | Mode::Visual { .. }),
            cursor_row,
            text_w: text.width,
            text_h: text.height,
            lines,
            kinds,
            looks,
            folded,
            selected,
        };
        // Scrolled so the cursor's line shows with lines around it.
        let rows: Vec<usize> = visible.iter().map(|v| v.0).collect();
        let mut ci = rows.iter().rposition(|&r| r <= cursor_row).unwrap_or(0);
        let mut ti = rows.iter().position(|&r| r >= self.editor.top).unwrap_or(0);
        let so = SCROLLOFF.min((text.height as usize).saturating_sub(1) / 2);
        let mut heights = std::collections::HashMap::new();
        // The wheel moved the view: the cursor comes into it, the
        // scrolloff's distance from its edges (but at the note's own first
        // and last lines), as Vim moves it, rather than the view going back
        // to the cursor.
        if std::mem::take(&mut self.scrolled) && !rows.is_empty() {
            let low = if ti == 0 {
                0
            } else {
                (ti + so).min(rows.len() - 1)
            };
            let (mut total, mut last) = (0, ti);
            for (i, &r) in rows.iter().enumerate().skip(ti) {
                total += *heights
                    .entry(r)
                    .or_insert_with(|| self.pieces(r, &ctx).len());
                if total > text.height as usize {
                    break;
                }
                last = i;
            }
            let high = if last + 1 >= rows.len() {
                last
            } else {
                last.saturating_sub(so)
            };
            let want = ci.clamp(low, high.max(low));
            if want != ci {
                ci = want;
                self.editor.doc.row = rows[want];
                self.editor.clamp();
                ctx.cursor_row = rows[want];
            }
        }
        if ci < ti + so {
            ti = ci.saturating_sub(so);
        } else {
            loop {
                let end = (ci + so).min(rows.len() - 1);
                let mut total = 0;
                for &r in &rows[ti..=end] {
                    let n = match heights.get(&r) {
                        Some(n) => *n,
                        None => {
                            let n = self.pieces(r, &ctx).len();
                            heights.insert(r, n);
                            n
                        }
                    };
                    total += n;
                }
                if total <= text.height as usize || ti >= ci {
                    break;
                }
                ti += 1;
            }
        }
        self.editor.top = rows.get(ti).copied().unwrap_or(0);
        // The rows on the screen, and the pictures near it, which the
        // terminal is given (and those gone far from it taken back).
        let mut shown: Vec<Shown> = Vec::new();
        let mut i = ti;
        while i < rows.len() && shown.len() < text.height as usize {
            shown.extend(self.pieces(rows[i], &ctx));
            i += 1;
        }
        shown.truncate(text.height as usize);
        let mut near: Vec<Picture> = Vec::new();
        let span = text.height as usize;
        for &r in rows[ti.saturating_sub(span)..(i + span).min(rows.len())].iter() {
            if ctx.lines[r].contains("![")
                && let Some(note) = self.current_path().map(|p| p.to_path_buf())
            {
                for pic in self.images.pictures(
                    self.store.root(),
                    &note,
                    &ctx.lines[r],
                    ctx.kinds[r],
                    text.width,
                    text.height,
                ) {
                    if !near.iter().any(|p| p.id == pic.id) {
                        near.push(pic);
                    }
                }
            }
        }
        let commands = self.images.commands(&near);
        self.out.extend(commands);
        // The lines drawn.
        let mut cursor = None;
        let want = self.editor.doc.col;
        let line_len = ctx.lines[cursor_row].len();
        for (k, s) in shown.iter().enumerate() {
            let y = text.y + k as u16;
            if number_w > 0 {
                let first = matches!(s, Shown::Text { first: true, .. } | Shown::Fold { .. });
                if first {
                    let r = s.row();
                    let n = if r == cursor_row {
                        format!("{:<w$}", r + 1, w = number_w as usize - 1)
                    } else {
                        format!("{:>w$}", r.abs_diff(cursor_row), w = number_w as usize - 1)
                    };
                    let style = if r == cursor_row {
                        Style::new().fg(Color::Indexed(if self.opts.light { 236 } else { 250 }))
                    } else {
                        Style::new().fg(Color::Indexed(if self.opts.light { 248 } else { 240 }))
                    };
                    buf.set_stringn(note.x, y, n, number_w as usize - 1, style);
                }
            }
            match s {
                Shown::Fold { text: t, .. } => {
                    buf.set_stringn(
                        text.x,
                        y,
                        t,
                        text.width as usize,
                        Hl::Fold.style(self.opts.light),
                    );
                    if s.row() == cursor_row {
                        cursor = Some((text.x, y));
                    }
                }
                Shown::Picture { pic, r, .. } => {
                    let x0 = text.x + pic.indent;
                    let style = Style::new().fg(pic.color());
                    for c in 0..pic.cols {
                        let x = x0 + c;
                        if x < text.right() {
                            buf[(x, y)]
                                .set_symbol(&Picture::cell(*r, c))
                                .set_style(style);
                        }
                    }
                }
                Shown::Text {
                    row,
                    cells,
                    indent,
                    bg,
                    ..
                } => {
                    if let Some(bg) = bg {
                        buf.set_style(
                            Rect::new(text.x, y, text.width, 1),
                            bg.style(self.opts.light),
                        );
                    }
                    let mut x = text.x + indent;
                    for cell in cells {
                        if x + cell.width as u16 > text.right() {
                            break;
                        }
                        if *row == cursor_row
                            && cursor.is_none()
                            && cell.col.is_some_and(|c| want < c + cell.len.max(1))
                        {
                            cursor = Some((x, y));
                        }
                        let style = match bg {
                            Some(bg) => bg.style(self.opts.light).patch(cell.style),
                            None => cell.style,
                        };
                        buf.set_stringn(x, y, &cell.text, cell.width, style);
                        x += cell.width as u16;
                    }
                    // Past the end of the line, where Insert mode types.
                    let last_piece = shown
                        .get(k + 1)
                        .is_none_or(|n| n.row() != *row || !matches!(n, Shown::Text { .. }));
                    if *row == cursor_row
                        && cursor.is_none()
                        && last_piece
                        && want >= line_len.min(want)
                    {
                        cursor = Some((x.min(text.right().saturating_sub(1)), y));
                    }
                }
            }
        }
        self.geo.shown = shown;
        cursor
    }

    /// The row and byte a cell of the note's text stands for: `None` for
    /// the byte where there is none (a heading's ▾, a picture's row).
    pub(super) fn at_screen(&self, x: u16, y: u16) -> Option<(usize, Option<usize>)> {
        let t = self.geo.text;
        if y < t.y {
            return None;
        }
        let k = (y - t.y) as usize;
        let s = self.geo.shown.get(k).or_else(|| self.geo.shown.last())?;
        match s {
            Shown::Text {
                row, cells, indent, ..
            } => {
                let mut at = t.x + indent;
                for c in cells {
                    if x < at + c.width as u16 {
                        return Some((*row, c.col));
                    }
                    at += c.width as u16;
                }
                let end = cells
                    .iter()
                    .rev()
                    .find_map(|c| c.col.map(|b| b + c.len))
                    .unwrap_or(0);
                Some((*row, Some(end)))
            }
            Shown::Fold { row, .. } | Shown::Picture { row, .. } => Some((*row, None)),
        }
    }

    /// The key help line in easy mode, and the line for Vim's prompts and
    /// messages, or the mode; gives where the cursor goes while a prompt
    /// takes the keys.
    fn draw_bottom(&mut self, buf: &mut Buffer, y0: u16) -> Option<(u16, u16)> {
        let w = buf.area.width;
        let faint = Style::new().fg(Color::Indexed(if self.opts.light { 243 } else { 245 }));
        let mut y = y0;
        if self.opts.easy && self.opts.key_help {
            let mode = match self.editor.mode {
                Mode::Insert => easy::HelpMode::Insert,
                Mode::Visual { .. } => easy::HelpMode::Visual,
                Mode::Normal => easy::HelpMode::Normal,
            };
            buf.set_stringn(
                0,
                y,
                format!(" {}", easy::key_help(mode)),
                w as usize,
                faint,
            );
            y += 1;
        }
        if y >= buf.area.height {
            return None;
        }
        if let Some((kind, text)) = &self.prompt {
            let line = format!("{kind}{text}");
            buf.set_stringn(0, y, &line, w as usize, Style::new());
            return Some(((width(&line) as u16).min(w.saturating_sub(1)), y));
        }
        if let Some((text, _)) = &self.message {
            put_line(
                buf,
                0,
                y,
                w,
                &Line::from(Span::styled(text.clone(), Style::new())),
            );
            return None;
        }
        let mode = match self.editor.mode {
            Mode::Insert if self.focus == Focus::Note => "-- INSERT --",
            Mode::Visual { select: true, .. } => "-- SELECT --",
            Mode::Visual { line: true, .. } => "-- VISUAL LINE --",
            Mode::Visual { .. } => "-- VISUAL --",
            _ => "",
        };
        buf.set_stringn(
            0,
            y,
            mode,
            w as usize,
            Style::new().add_modifier(Modifier::BOLD),
        );
        None
    }
}

/// What drawing a note's lines works from.
struct Ctx {
    raw: bool,
    cursor_row: usize,
    text_w: u16,
    text_h: u16,
    lines: Vec<String>,
    kinds: Vec<render::Kind>,
    looks: Vec<render::Look>,
    folded: std::collections::HashMap<usize, Option<usize>>,
    selected: std::collections::HashMap<usize, (usize, usize)>,
}
