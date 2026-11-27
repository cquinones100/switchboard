//! Drawing the boxes: text measured in the columns a terminal gives it, and
//! the box every menu, question and message is drawn in, mission control's
//! look (lib/sidebar/render.js's `modalBox`): a rounded edge in colour81
//! (colour220 for a question), the title bold and bright in its top edge,
//! the lines inside, then a blank line and the keys, dim, on a row of their
//! own. With easy mode's close button (`Look::close`), a ` × ` sits in the
//! top edge a cell short of the corner, as `closeAt` placed it.
//!
//! A box sized by its lines wraps a line of text wider than 72 columns
//! (`WRAP`) at its spaces, each row indented as the line's first, and is
//! no wider than that; a row of a list or a menu (one a click means
//! something on, or one on a highlight) is cut with … instead, as is any
//! line of a box that fills a share of the screen.
//!
//! A box remembers where it drew each line (`Drawn`), as `lastModal` did, so
//! a click is read against what is on the screen rather than worked out
//! again.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::Look;
use crate::input::{Mouse, MouseButton, MouseKind};

/// The edge of every box: tmux.conf's `popup-border-style`.
pub const EDGE: Color = Color::Indexed(81);
/// A question's edge, and the `›` beside what Enter takes.
pub const YELLOW: Color = Color::Indexed(220);
/// Easy mode's close button (render.js's `CLOSE`), in a box's top edge.
pub const CLOSE: &str = " × ";

/// The widest a box's line of text runs before it wraps.
pub const WRAP: usize = 72;

/// The columns a terminal draws `text` in.
pub fn width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// `text` cut to `cols` columns, an ellipsis in place of what didn't fit
/// (render.js's `cut`).
pub fn cut(text: &str, cols: usize) -> String {
    if width(text) <= cols {
        return text.to_string();
    }
    if cols == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for g in text.graphemes(true) {
        let w = width(g);
        if used + w > cols - 1 {
            break;
        }
        out.push_str(g);
        used += w;
    }
    out.push('…');
    out
}

/// `text` cut to `cols` columns and padded with spaces to fill them.
pub fn fit(text: &str, cols: usize) -> String {
    let mut out = cut(text, cols);
    let w = width(&out);
    out.extend(std::iter::repeat_n(' ', cols.saturating_sub(w)));
    out
}

/// Text broken into lines at most `cols` wide, at spaces where it can be
/// and inside a word where it can't, as `fold -s` did for say.sh. Lines it
/// already has are kept.
pub fn wrap(text: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(1);
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_inclusive(' ') {
            if width(&line) + width(word.trim_end()) > cols && !line.is_empty() {
                out.push(line.trim_end().to_string());
                line.clear();
            }
            let mut word = word;
            // A word longer than a line goes over as many as it takes.
            while width(word.trim_end()) > cols {
                let mut head = String::new();
                let mut rest = word;
                for g in word.graphemes(true) {
                    if width(&head) + width(g) > cols {
                        break;
                    }
                    head.push_str(g);
                    rest = &rest[g.len()..];
                }
                out.push(head);
                word = rest;
            }
            line.push_str(word);
        }
        out.push(line.trim_end().to_string());
    }
    out
}

/// Writes styled spans from column `x` of row `y`, at most `cols` columns
/// and never past the buffer's edge; gives the column after them.
pub fn put_line(buf: &mut Buffer, x: u16, y: u16, cols: u16, line: &Line<'_>) -> u16 {
    let area = buf.area;
    if y < area.top() || y >= area.bottom() || x >= area.right() || x < area.left() {
        return x;
    }
    buf.set_line(x, y, line, cols).0
}

/// Where a box goes on the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// In the middle, as mission control's boxes and say.sh's.
    Center,
    /// In the top-right corner, as tip.sh's (`-x R -y 0`).
    TopRight,
    /// A share of the screen in its middle, as a `display-popup -w 60% -h
    /// 60%` was: the palette, a picker. The lines fill it.
    Share { cols_pct: u16, rows_pct: u16 },
    /// Its top-left corner at a cell, moved in as far as it must to fit:
    /// the restart's steps beside the sidebar's build row.
    At { x: u16, y: u16 },
    /// Beside a click at a cell, for a menu or question the click opened:
    /// its top-left a cell right of and a row below it, flipped to the left
    /// or above where it would run off the screen, then moved in to fit as
    /// `At` is, so its options are a short move from the mouse.
    Near { x: u16, y: u16 },
}

/// One line inside a box: its spans, a background for the whole line (the
/// chosen option's highlight), and whether a click on it means something.
#[derive(Clone, Debug, Default)]
pub struct BoxLine {
    pub spans: Vec<Span<'static>>,
    pub fill: Option<Color>,
    pub clickable: bool,
}

impl BoxLine {
    pub fn blank() -> BoxLine {
        BoxLine::default()
    }

    pub fn text(text: impl Into<String>, style: Style) -> BoxLine {
        BoxLine {
            spans: vec![Span::styled(text.into(), style)],
            ..BoxLine::default()
        }
    }

    pub fn spans(spans: Vec<Span<'static>>) -> BoxLine {
        BoxLine {
            spans,
            ..BoxLine::default()
        }
    }

    pub fn fill(mut self, color: Option<Color>) -> BoxLine {
        self.fill = color;
        self
    }

    pub fn clickable(mut self) -> BoxLine {
        self.clickable = true;
        self
    }

    fn width(&self) -> usize {
        self.spans.iter().map(|s| width(&s.content)).sum()
    }

    /// A line of text, which wraps, rather than a row of a list or a menu,
    /// which is cut.
    fn is_text(&self) -> bool {
        !self.clickable && self.fill.is_none()
    }

    /// The spaces it starts with.
    fn indent(&self) -> usize {
        let mut n = 0;
        for span in &self.spans {
            let lead = span.content.len() - span.content.trim_start_matches(' ').len();
            n += lead;
            if lead < span.content.len() {
                break;
            }
        }
        n
    }

    /// The columns it asks of a box: a line of text `WRAP` at most after
    /// its indent; and two to spare, as modalBox kept.
    fn wants(&self) -> usize {
        let w = self.width();
        if self.is_text() {
            w.min(self.indent() + WRAP) + 2
        } else {
            w + 2
        }
    }

    /// The line in a box `inner` columns wide: one row when it fits there
    /// or isn't text, else rows broken at spaces where it can be and inside
    /// a word where it can't, two columns to spare, each row indented as
    /// its first.
    fn rows(&self, inner: usize) -> Vec<BoxLine> {
        if !self.is_text() || self.width() <= inner {
            return vec![self.clone()];
        }
        let indent = self.indent();
        let room = inner.saturating_sub(indent + 2).max(1);
        // Its graphemes after the indent, each with its style.
        let mut cells: Vec<(String, Style)> = Vec::new();
        let mut skip = indent;
        for span in &self.spans {
            for g in span.content.graphemes(true) {
                if skip > 0 && g == " " {
                    skip -= 1;
                    continue;
                }
                skip = 0;
                cells.push((g.to_string(), span.style));
            }
        }
        // Words, each with the spaces after it.
        let mut words: Vec<&[(String, Style)]> = Vec::new();
        let mut start = 0;
        for i in 0..cells.len() {
            let ends = cells[i].0 == " " && cells.get(i + 1).is_none_or(|c| c.0 != " ");
            if ends || i + 1 == cells.len() {
                words.push(&cells[start..=i]);
                start = i + 1;
            }
        }
        let wide = |cs: &[(String, Style)]| cs.iter().map(|c| width(&c.0)).sum::<usize>();
        let bare = |cs: &[(String, Style)]| {
            let end = cs.iter().rposition(|c| c.0 != " ").map_or(0, |i| i + 1);
            wide(&cs[..end])
        };
        let mut rows: Vec<Vec<(String, Style)>> = Vec::new();
        let mut row: Vec<(String, Style)> = Vec::new();
        for word in words {
            if !row.is_empty() && wide(&row) + bare(word) > room {
                rows.push(std::mem::take(&mut row));
            }
            let mut word = word;
            // A word longer than a row goes over as many as it takes.
            while bare(word) > room {
                let mut used = 0;
                let n = word
                    .iter()
                    .take_while(|c| {
                        used += width(&c.0);
                        used <= room
                    })
                    .count()
                    .max(1);
                rows.push(word[..n].to_vec());
                word = &word[n..];
            }
            row.extend_from_slice(word);
        }
        rows.push(row);
        rows.into_iter()
            .map(|cells| {
                let end = cells.iter().rposition(|c| c.0 != " ").map_or(0, |i| i + 1);
                let mut spans = vec![Span::raw(" ".repeat(indent))];
                for (g, style) in &cells[..end] {
                    match spans.last_mut() {
                        Some(last) if last.style == *style => last.content.to_mut().push_str(g),
                        _ => spans.push(Span::styled(g.clone(), *style)),
                    }
                }
                BoxLine::spans(spans)
            })
            .collect()
    }
}

/// The rows a box sized by its lines draws them in on a screen `cols`
/// wide, its text wrapped (`BoxLine::rows`), for a box that scrolls
/// them itself.
pub fn wrapped(spec: &BoxSpec<'_>, cols: u16, look: &Look) -> Vec<BoxLine> {
    let inner = inner_width(spec, cols as usize, look);
    lay_out(spec.lines, inner)
        .into_iter()
        .map(|(_, l)| l)
        .collect()
}

/// The width inside a box sized by its lines, on a screen `cols` wide.
fn inner_width(spec: &BoxSpec<'_>, cols: usize, look: &Look) -> usize {
    let close_w = if spec.closes && look.close {
        width(CLOSE)
    } else {
        0
    };
    let widest = spec
        .lines
        .iter()
        .map(BoxLine::wants)
        .chain(spec.keys.map(|k| width(k) + 3))
        .max()
        .unwrap_or(0);
    (spec.width as usize)
        .max(if spec.title.is_empty() {
            0
        } else {
            width(spec.title) + 6 + close_w
        })
        .max(widest)
        .min(cols.saturating_sub(2))
}

/// A box's lines as it draws them `inner` columns wide (`BoxLine::rows`),
/// each with the index of the line it came from.
fn lay_out(lines: &[BoxLine], inner: usize) -> Vec<(usize, BoxLine)> {
    lines
        .iter()
        .enumerate()
        .flat_map(|(i, l)| l.rows(inner).into_iter().map(move |r| (i, r)))
        .collect()
}

/// What a box shows.
pub struct BoxSpec<'a> {
    pub title: &'a str,
    pub lines: &'a [BoxLine],
    /// The keys it takes, dim on a row of their own at the bottom; none for
    /// a message, which any key closes.
    pub keys: Option<&'a str>,
    pub edge: Color,
    /// The narrowest its inside is, as modalBox's `width` (36).
    pub width: u16,
    pub place: Place,
    /// Whether it has easy mode's close button (when the look allows one).
    pub closes: bool,
    /// The close button has Tab's focus (or the mouse over it): drawn in
    /// reverse video.
    pub close_lit: bool,
}

impl<'a> BoxSpec<'a> {
    pub fn new(title: &'a str, lines: &'a [BoxLine]) -> BoxSpec<'a> {
        BoxSpec {
            title,
            lines,
            keys: None,
            edge: EDGE,
            width: 36,
            place: Place::Center,
            closes: true,
            close_lit: false,
        }
    }
}

/// Where a box was drawn, for clicks on it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Drawn {
    /// The whole box, edges included.
    pub outer: Rect,
    /// Inside the edges.
    pub inner: Rect,
    /// The screen row of each line shown, by its index in the spec's lines.
    pub rows: Vec<(u16, usize)>,
    /// The screen rows of the lines a click means something on.
    pub clickable: Vec<u16>,
    /// The close button's cells, when it has one.
    pub close: Option<Rect>,
    /// Drawn as a pane beside a narrow lone pane (`draw_pane`): the
    /// header's columns after its title, up to the close button, where the
    /// tab it is about is named.
    pub header_end: Option<(u16, u16)>,
}

/// What a mouse event landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// Outside the box.
    Outside,
    /// Its close button.
    Close,
    /// One of its lines (an index into the spec's lines), at a column
    /// counted from the inside's left edge.
    Line(usize, u16),
    /// Somewhere else inside it.
    Inside,
}

impl Drawn {
    /// What a cell of the screen is in this box.
    pub fn hit(&self, col: u16, row: u16) -> Hit {
        let within = |r: Rect| col >= r.x && col < r.right() && row >= r.y && row < r.bottom();
        if self.close.is_some_and(within) {
            return Hit::Close;
        }
        if !within(self.outer) {
            return Hit::Outside;
        }
        match self.rows.iter().find(|(y, _)| *y == row) {
            Some((_, i)) if col >= self.inner.x && col < self.inner.right() => {
                Hit::Line(*i, col - self.inner.x)
            }
            _ => Hit::Inside,
        }
    }
}

/// A box's lines `first..first + room` show `total` rows from `top`: while
/// they run past it (and Settings' `scroll_bars` is on), its right edge
/// beside them is their scroll bar, the edge's `│` the track and the
/// thumb a heavier `┃` on it, so the box keeps its width.
pub fn edge_bar(
    buf: &mut Buffer,
    drawn: &Drawn,
    (first, room): (usize, usize),
    (total, top): (usize, usize),
    look: &Look,
) {
    if !look.scroll_bars {
        return;
    }
    let Some((at, len)) = crate::kit::thumb(total, top, room, room) else {
        return;
    };
    let x = drawn.outer.right().saturating_sub(1);
    for (y, i) in &drawn.rows {
        if (first + at..first + at + len).contains(i)
            && let Some(cell) = buf.cell_mut((x, *y))
        {
            cell.set_symbol("┃");
        }
    }
}

/// The row of a box's edge bar (`edge_bar`, its lines `first..first +
/// room`) at a screen cell, counted from the bar's top.
pub fn edge_bar_row(
    drawn: &Drawn,
    (first, room): (usize, usize),
    col: u16,
    row: u16,
) -> Option<usize> {
    if col + 1 != drawn.outer.right() {
        return None;
    }
    drawn
        .rows
        .iter()
        .find(|(y, _)| *y == row)
        .and_then(|(_, i)| i.checked_sub(first))
        .filter(|i| *i < room)
}

/// The row of a box's edge bar a drag under way is at, whatever column
/// the mouse is in: screen row `row` counted from the bar's top, past its
/// bottom too, so a drag on beyond it still reaches the end
/// (`kit::scroll_drag` holds it to the track).
pub fn edge_bar_drag_row(drawn: &Drawn, first: usize, row: u16) -> Option<usize> {
    let (top, _) = drawn.rows.iter().find(|(_, i)| *i == first)?;
    Some(usize::from(row.saturating_sub(*top)))
}

/// What a mouse event is, to a box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    Click,
    WheelUp,
    WheelDown,
    /// The mouse moved with no button held (hover).
    Move,
    /// A release, a drag or another button: nothing to a box.
    Other,
}

pub fn gesture(m: &Mouse) -> Gesture {
    match (m.kind, m.button) {
        (MouseKind::Press, MouseButton::Left) => Gesture::Click,
        (MouseKind::Press, MouseButton::WheelUp) => Gesture::WheelUp,
        (MouseKind::Press, MouseButton::WheelDown) => Gesture::WheelDown,
        (MouseKind::Move, _) => Gesture::Move,
        _ => Gesture::Other,
    }
}

/// The narrowest the empty columns beside a lone pane are for a `Share`
/// box about the tab to be drawn there as a pane, as a `Share` box is at
/// its narrowest.
const PANE_LEAST: u16 = 40;

/// Where a `Share` box about the tab goes as a pane (`Look::middle`, the
/// empty columns beside a narrow lone pane, when they are wide enough), so
/// a box that lays out its own lines (the picker) does so at the size
/// `draw_box` gives it.
pub fn pane_beside(screen: Rect, look: &Look) -> Option<Rect> {
    look.middle
        .map(|m| m.intersection(screen))
        .filter(|m| m.width >= PANE_LEAST && m.height >= 3)
}

/// Draws a box over what is in the buffer and gives where it went. The
/// lines are cut to fit; a box taller than the screen shows as many of its
/// lines as fit, as modalBox showed `rows - 6`.
pub fn draw_box(buf: &mut Buffer, spec: &BoxSpec<'_>, look: &Look) -> Drawn {
    let screen = buf.area;
    let (cols, rows) = (screen.width, screen.height);
    if cols < 4 || rows < 3 {
        return Drawn::default();
    }
    let close = spec.closes && look.close;
    let close_w = if close { width(CLOSE) } else { 0 };
    let keys_line = spec.keys.map(|k| format!(" {k}"));
    let keys_rows = if keys_line.is_some() { 2 } else { 0 };
    // A row clear above and below, as modalBox kept.
    let room = (rows as usize).saturating_sub(4 + keys_rows).max(1);
    let (inner_w, shown) = match spec.place {
        Place::Share { cols_pct, rows_pct } => {
            let w = (cols as usize * cols_pct as usize / 100)
                .max(40)
                .min(cols as usize)
                - 2;
            let h = (rows as usize * rows_pct as usize / 100)
                .max(10)
                .min(rows as usize)
                .saturating_sub(2 + keys_rows)
                .max(1);
            (w, h)
        }
        _ => {
            let w = inner_width(spec, cols as usize, look);
            (w, lay_out(spec.lines, w).len().min(room).max(1))
        }
    };
    // A box filling a share of the screen lays its own lines out to it.
    let wraps = !matches!(spec.place, Place::Share { .. });
    let outer_w = inner_w as u16 + 2;
    // A box about the tab, beside a narrow lone pane, is a pane there when
    // it fits (`Look::middle`, set only for such a box).
    let pane = match spec.place {
        Place::Share { .. } => pane_beside(screen, look),
        Place::Center => pane_beside(screen, look).filter(|m| outer_w <= m.width),
        _ => None,
    };
    if let Some(m) = pane {
        return draw_pane(buf, spec, look, m, (close, wraps), keys_line);
    }
    let outer_h = (shown + 2 + keys_rows) as u16;
    let outer_h = outer_h.min(rows);
    let (left, top) = match spec.place {
        Place::TopRight => (cols.saturating_sub(outer_w), 0),
        Place::At { x, y } => (
            x.min(cols.saturating_sub(outer_w)),
            y.min(rows.saturating_sub(outer_h)),
        ),
        Place::Near { x, y } => {
            let right = x.saturating_add(1);
            let below = y.saturating_add(1);
            let left = if right.saturating_add(outer_w) > cols {
                x.saturating_sub(outer_w)
            } else {
                right
            };
            let top = if below.saturating_add(outer_h) > rows {
                y.saturating_sub(outer_h)
            } else {
                below
            };
            (
                left.min(cols.saturating_sub(outer_w)),
                top.min(rows.saturating_sub(outer_h)),
            )
        }
        _ => (
            cols.saturating_sub(outer_w) / 2,
            rows.saturating_sub(outer_h) / 2,
        ),
    };
    let outer = Rect::new(left, top, outer_w, outer_h).intersection(screen);
    let inner = Rect::new(
        outer.x + 1,
        outer.y + 1,
        outer.width.saturating_sub(2),
        outer.height.saturating_sub(2),
    );
    let edge = Style::new().fg(spec.edge);
    for y in outer.top()..outer.bottom() {
        for x in outer.left()..outer.right() {
            buf[(x, y)].reset();
        }
    }
    let (l, r, t, b) = (
        outer.left(),
        outer.right() - 1,
        outer.top(),
        outer.bottom() - 1,
    );
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
    let iw = inner.width as usize;
    if !spec.title.is_empty() && iw > 4 + close_w {
        let heading = format!(" {} ", cut(spec.title, iw - 4 - close_w));
        let style = Style::new()
            .fg(look.theme.bright)
            .add_modifier(Modifier::BOLD);
        put_line(
            buf,
            l + 2,
            t,
            inner.width.saturating_sub(1),
            &Line::from(Span::styled(heading, style)),
        );
    }
    let close_at = (close && outer.width as usize > close_w + 2).then(|| {
        let x = r - close_w as u16;
        let mut style = Style::new()
            .fg(look.theme.bright)
            .add_modifier(Modifier::BOLD);
        if spec.close_lit {
            style = style.add_modifier(Modifier::REVERSED);
        }
        put_line(
            buf,
            x,
            t,
            close_w as u16,
            &Line::from(Span::styled(CLOSE, style)),
        );
        Rect::new(x, t, close_w as u16, 1)
    });
    let (rows, clickable) = draw_inside(buf, spec, look, (inner, wraps), keys_line);
    Drawn {
        outer,
        inner,
        rows,
        clickable,
        close: close_at,
        header_end: None,
    }
}

/// A box about the tab drawn as a pane in the empty columns `m` beside a
/// narrow lone pane (UX 2.0 board 111): as tall as they are, on the base
/// surface as the pane beside it, a header line naming it (the tab named
/// after it by `Stack::draw`, at `Drawn::header_end`) with the close
/// button at its right end, the rule at the pane's edge its only border,
/// and no shadow; its lines and keys laid out as a box's.
fn draw_pane(
    buf: &mut Buffer,
    spec: &BoxSpec<'_>,
    look: &Look,
    m: Rect,
    (close, wraps): (bool, bool),
    keys_line: Option<String>,
) -> Drawn {
    for y in m.top()..m.bottom() {
        for x in m.left()..m.right() {
            buf[(x, y)].reset();
        }
    }
    let header = Rect::new(m.x, m.y, m.width, 1);
    buf.set_style(
        header,
        Style::new().bg(crate::kit::Surface::Panel.color(&look.theme)),
    );
    let close_w = if close { width(CLOSE) as u16 } else { 0 };
    // The close button a column in from the edge, as the board has it.
    let close_at = close.then(|| Rect::new(m.right() - 1 - close_w, m.y, close_w, 1));
    if let Some(at) = close_at {
        let mut style = Style::new().fg(look.theme.dim);
        if spec.close_lit {
            style = style.add_modifier(Modifier::REVERSED);
        }
        put_line(
            buf,
            at.x,
            at.y,
            at.width,
            &Line::from(Span::styled(CLOSE, style)),
        );
    }
    let room = m.width.saturating_sub(3 + close_w) as usize;
    let title = cut(spec.title, room);
    let style = Style::new()
        .fg(look.theme.bright)
        .add_modifier(Modifier::BOLD);
    let header_end = put_line(
        buf,
        m.x + 1,
        m.y,
        room as u16,
        &Line::from(Span::styled(title, style)),
    );
    // A blank row under the header, a column clear each side.
    let inner = Rect::new(
        m.x + 1,
        m.y + 2,
        m.width.saturating_sub(2),
        m.height.saturating_sub(2),
    );
    let (rows, clickable) = draw_inside(buf, spec, look, (inner, wraps), keys_line);
    Drawn {
        outer: m,
        inner,
        rows,
        clickable,
        close: close_at,
        header_end: Some((header_end, m.x + m.width - 1 - close_w)),
    }
}

/// A box's lines from the top of `inner` (its text wrapped to it, unless
/// it fills a share of the screen), and its keys on its last row with a
/// blank one above; gives the screen row of each line shown and the rows a
/// click means something on.
fn draw_inside(
    buf: &mut Buffer,
    spec: &BoxSpec<'_>,
    look: &Look,
    (inner, wraps): (Rect, bool),
    keys_line: Option<String>,
) -> (Vec<(u16, usize)>, Vec<u16>) {
    let keys_rows = if keys_line.is_some() { 2 } else { 0 };
    let mut drawn_rows = Vec::new();
    let mut clickable = Vec::new();
    let line_rows = inner.height.saturating_sub(keys_rows as u16);
    let lines = if wraps {
        lay_out(spec.lines, inner.width as usize)
    } else {
        spec.lines.iter().cloned().enumerate().collect()
    };
    for (row, (i, line)) in lines.iter().take(line_rows as usize).enumerate() {
        let (i, y) = (*i, inner.y + row as u16);
        if let Some(fill) = line.fill {
            buf.set_style(Rect::new(inner.x, y, inner.width, 1), Style::new().bg(fill));
        }
        put_line(
            buf,
            inner.x,
            y,
            inner.width,
            &Line::from(line.spans.clone()),
        );
        drawn_rows.push((y, i));
        if line.clickable {
            clickable.push(y);
        }
    }
    if let Some(keys) = keys_line
        && inner.height >= 2
    {
        put_line(
            buf,
            inner.x,
            inner.bottom() - 1,
            inner.width,
            &Line::from(Span::styled(keys, Style::new().fg(look.theme.dim))),
        );
    }
    (drawn_rows, clickable)
}

/// What the tests read a buffer with, here and in the programs drawn with
/// this crate.
pub mod tests {
    use ratatui::buffer::Buffer;

    /// A row of a buffer as text.
    pub fn row(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect::<String>()
    }

    /// Every row of a buffer as text, trailing blanks trimmed.
    pub fn screen(buf: &Buffer) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| row(buf, y).trim_end().to_string())
            .collect()
    }
}

#[cfg(test)]
mod cases {
    use super::tests::{row, screen};
    use super::*;

    #[test]
    fn both_widths_count_what_a_terminal_draws() {
        for (text, cols) in [("plain text", 10), ("漢字", 4), ("⏱️ 5m", 5), ("⚠️", 2)] {
            assert_eq!(width(text), cols, "ui::draw::width of {text:?}");
            assert_eq!(
                crate::text::width(text),
                cols,
                "sidebar::text::width of {text:?}"
            );
        }
        // Where they part: a ZWJ sequence is one grapheme here, as ratatui
        // draws it, and two emoji in the sidebar, which counts and writes
        // its cells by the code point (`sidebar::render::put`).
        let coder = "👩\u{200d}💻";
        assert_eq!(width(coder), 2);
        assert_eq!(crate::text::width(coder), 4);
    }

    #[test]
    fn text_is_cut_and_wrapped_by_columns() {
        assert_eq!(cut("hello world", 5), "hell…");
        assert_eq!(cut("hi", 5), "hi");
        assert_eq!(fit("hi", 4), "hi  ");
        assert_eq!(width("日本"), 4);
        assert_eq!(cut("日本語", 5), "日本…");
        assert_eq!(
            wrap("the quick brown fox jumps", 10),
            vec!["the quick", "brown fox", "jumps"]
        );
        assert_eq!(wrap("a\nb", 10), vec!["a", "b"]);
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn a_box_is_drawn_in_the_middle_with_its_title_lines_and_keys() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 12));
        let lines = [BoxLine::text(" Close pane?", Style::new())];
        let mut spec = BoxSpec::new("Are you sure?", &lines);
        spec.keys = Some("y yes  n no");
        let drawn = draw_box(&mut buf, &spec, &Look::default());
        let s = screen(&buf);
        assert_eq!(
            drawn.outer.height, 5,
            "edges, one line, a blank and the keys"
        );
        assert!(s[drawn.outer.y as usize].contains("╭─ Are you sure? ─"));
        assert!(s[drawn.outer.y as usize + 1].contains("│ Close pane?"));
        assert!(s[drawn.outer.y as usize + 3].contains(" y yes  n no"));
        assert!(s[drawn.outer.bottom() as usize - 1].contains("╰──"));
        assert_eq!(drawn.rows, vec![(drawn.inner.y, 0)]);
        assert_eq!(drawn.hit(drawn.inner.x + 2, drawn.inner.y), Hit::Line(0, 2));
        assert_eq!(drawn.hit(0, 0), Hit::Outside);
        assert_eq!(drawn.hit(drawn.outer.x, drawn.outer.y), Hit::Inside);
        assert_eq!(drawn.close, None, "no close button out of easy mode");
    }

    #[test]
    fn easy_mode_puts_a_close_button_in_the_top_edge() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 50, 10));
        let lines = [BoxLine::text("hello", Style::new())];
        let spec = BoxSpec::new("Title", &lines);
        let look = Look {
            close: true,
            ..Look::default()
        };
        let drawn = draw_box(&mut buf, &spec, &look);
        let close = drawn.close.expect("a close button");
        let top = row(&buf, drawn.outer.y);
        assert!(top.contains(" × ╮"), "{top:?}");
        assert_eq!(close.right(), drawn.outer.right() - 1);
        assert_eq!(drawn.hit(close.x + 1, close.y), Hit::Close);
    }

    #[test]
    fn a_tip_goes_in_the_top_right_corner() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 20));
        let lines = [BoxLine::text(" Pinned.", Style::new())];
        let mut spec = BoxSpec::new("tip", &lines);
        spec.place = Place::TopRight;
        spec.width = 10;
        let drawn = draw_box(&mut buf, &spec, &Look::default());
        assert_eq!(drawn.outer.y, 0);
        assert_eq!(drawn.outer.right(), 80);
    }

    #[test]
    fn a_box_near_a_click_opens_beside_it_and_flips_at_the_edges() {
        let lines = [BoxLine::text(" Open", Style::new())];
        let near = |x, y| {
            let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
            let mut spec = BoxSpec::new("Menu", &lines);
            spec.place = Place::Near { x, y };
            spec.width = 20;
            draw_box(&mut buf, &spec, &Look::default()).outer
        };
        let beside = near(10, 5);
        assert_eq!((beside.x, beside.y), (11, 6));
        // Near the right edge it opens to the click's left, near the
        // bottom above it.
        let corner = near(75, 22);
        assert_eq!((corner.right(), corner.bottom()), (75, 22));
        // Where neither side has room it is moved in to fit.
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 6));
        let mut spec = BoxSpec::new("Menu", &lines);
        spec.place = Place::Near { x: 15, y: 3 };
        spec.width = 20;
        let squeezed = draw_box(&mut buf, &spec, &Look::default()).outer;
        assert!(squeezed.right() <= 30 && squeezed.bottom() <= 6);
    }

    #[test]
    fn a_box_about_the_tab_is_a_pane_beside_a_narrow_lone_pane_when_it_fits() {
        let lines = [BoxLine::text(" Open", Style::new())];
        let spare = Rect::new(100, 0, 140, 40);
        let drawn = |place, middle| {
            let mut buf = Buffer::empty(Rect::new(0, 0, 240, 40));
            let mut spec = BoxSpec::new("Box", &lines);
            spec.place = place;
            spec.keys = Some("esc close");
            let look = Look {
                middle,
                ..Look::default()
            };
            (draw_box(&mut buf, &spec, &look), buf)
        };
        // The spare columns whole, its title on their first row, its first
        // line under a blank one and its keys on the last row.
        let (pane, buf) = drawn(Place::Center, Some(spare));
        assert_eq!(pane.outer, spare);
        assert_eq!(buf[(101, 0)].symbol(), "B");
        assert_eq!(pane.rows.first(), Some(&(2, 0)));
        assert_eq!(buf[(102, 39)].symbol(), "e");
        let (to, until) = pane.header_end.expect("a pane names its tab");
        assert!(to > 101 && until < 240 && to < until, "{to}..{until}");
        // No box edge: the rule at the pane's edge is its border.
        assert_eq!(buf[(100, 0)].symbol(), " ");
        let share = Place::Share {
            cols_pct: 60,
            rows_pct: 60,
        };
        assert_eq!(drawn(share, Some(spare)).0.outer, spare);
        let screen = Rect::new(0, 0, 240, 40);
        let beside = Look {
            middle: Some(spare),
            ..Look::default()
        };
        assert_eq!(pane_beside(screen, &beside), Some(spare));
        assert_eq!(pane_beside(screen, &Look::default()), None);
        // Too few columns: the middle of the screen, as before.
        let narrow = Rect::new(210, 0, 30, 40);
        assert_eq!(
            drawn(Place::Center, Some(narrow)).0,
            drawn(Place::Center, None).0
        );
        assert_eq!(drawn(share, Some(narrow)).0, drawn(share, None).0);
        // A box beside a click stays beside it.
        let near = Place::Near { x: 10, y: 5 };
        assert_eq!(drawn(near, Some(spare)).0, drawn(near, None).0);
    }

    #[test]
    fn a_box_on_a_small_screen_fits_inside_it() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        let lines: Vec<BoxLine> = (0..10)
            .map(|i| BoxLine::text(format!("line {i} is long enough to cut"), Style::new()))
            .collect();
        let mut spec = BoxSpec::new("A long title here", &lines);
        spec.keys = Some("keys");
        let drawn = draw_box(&mut buf, &spec, &Look::default());
        assert!(drawn.outer.width <= 20 && drawn.outer.height <= 5);
        assert_eq!(drawn.rows.len(), 1);
    }
}
