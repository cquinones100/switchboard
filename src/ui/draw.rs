//! Drawing the boxes: text measured in the columns a terminal gives it, and
//! the box every menu, question and message is drawn in, mission control's
//! look (lib/sidebar/render.js's `modalBox`): a rounded edge in colour81
//! (colour220 for a question), the title bold and bright in its top edge,
//! the lines inside, then a blank line and the keys, dim, on a row of their
//! own. With easy mode's close button (`Look::close`), a ` × ` sits in the
//! top edge a cell short of the corner, as `closeAt` placed it.
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

use super::Look;
use crate::mux::input::{Mouse, MouseButton, MouseKind};

/// The edge of every box: tmux.conf's `popup-border-style`.
pub const EDGE: Color = Color::Indexed(81);
/// A question's edge, and the `›` beside what Enter takes.
pub const YELLOW: Color = Color::Indexed(220);
/// Easy mode's close button (render.js's `CLOSE`), in a box's top edge.
pub const CLOSE: &str = " × ";

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
    /// The close button's cells, when it has one.
    pub close: Option<Rect>,
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
    let Some((at, len)) = super::kit::thumb(total, top, room, room) else {
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
    // A row clear above and below, as modalBox kept, so the status line
    // stays in sight.
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
            let widest = spec
                .lines
                .iter()
                .map(|l| l.width() + 2)
                .chain(keys_line.as_deref().map(|k| width(k) + 2))
                .max()
                .unwrap_or(0);
            let w = (spec.width as usize)
                .max(if spec.title.is_empty() {
                    0
                } else {
                    width(spec.title) + 6 + close_w
                })
                .max(widest)
                .min(cols as usize - 2);
            (w, spec.lines.len().min(room).max(1))
        }
    };
    let outer_w = inner_w as u16 + 2;
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
    let mut drawn_rows = Vec::new();
    let line_rows = inner.height.saturating_sub(keys_rows as u16);
    for (i, line) in spec.lines.iter().take(line_rows as usize).enumerate() {
        let y = inner.y + i as u16;
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
    Drawn {
        outer,
        inner,
        rows: drawn_rows,
        close: close_at,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

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
