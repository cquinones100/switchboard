//! Drawing a client's screen. Each frame is built whole in a cell buffer (the
//! tab's panes, the lines between them, the status line, anything over them),
//! then only the cells that changed since the client's last frame are sent,
//! inside a synchronized update so the terminal never shows half a frame.
//!
//! The look is tmux.conf's: lines between panes in colour238, the focused
//! pane's in colour81, panes without the focus on a colour236 background (its
//! `window-style`) and the focused one on the terminal's own, and the status
//! line at the bottom with the session's name and the tabs.

use std::io::Write;

use crossterm::{cursor, queue};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use unicode_width::UnicodeWidthStr;

use super::layout::Placement;

/// tmux.conf's colours, by the 256-colour palette's numbers.
pub mod colors {
    use ratatui::style::Color;
    pub const STATUS_BG: Color = Color::Indexed(235);
    pub const STATUS_FG: Color = Color::Indexed(248);
    pub const SESSION_BG: Color = Color::Indexed(24);
    pub const WHITE: Color = Color::Indexed(255);
    pub const TAB_FG: Color = Color::Indexed(244);
    pub const CURRENT_BG: Color = Color::Indexed(238);
    pub const ACCENT: Color = Color::Indexed(81);
    pub const BORDER: Color = Color::Indexed(238);
    pub const INACTIVE_BG: Color = Color::Indexed(236);
}

/// One client's frame: what to show, and where the cursor goes (hidden when
/// `None`).
pub struct Frame {
    pub buf: Buffer,
    pub cursor: Option<(u16, u16)>,
}

impl Frame {
    pub fn new(cols: u16, rows: u16) -> Frame {
        Frame {
            buf: Buffer::empty(Rect::new(0, 0, cols, rows)),
            cursor: None,
        }
    }

    pub fn area(&self) -> Rect {
        self.buf.area
    }
}

/// What a client was last sent, to send only what changes.
#[derive(Default)]
pub struct Screen {
    prev: Option<Buffer>,
}

impl Screen {
    /// The next frame draws everything again (after an attach, a resize, or
    /// anything that may have left the terminal showing something else).
    pub fn invalidate(&mut self) {
        self.prev = None;
    }

    /// The bytes that turn the last frame sent into this one.
    pub fn render(&mut self, frame: &Frame) -> Vec<u8> {
        let mut out = Vec::with_capacity(4096);
        // Synchronized output: the terminal shows the frame once it is whole.
        out.extend_from_slice(b"\x1b[?2026h\x1b[?25l");
        let full = self.prev.as_ref().is_none_or(|p| p.area != frame.buf.area);
        let prev = match self.prev.take() {
            Some(p) if !full => p,
            _ => {
                out.extend_from_slice(b"\x1b[0m\x1b[H\x1b[2J");
                Buffer::empty(frame.buf.area)
            }
        };
        let diff = prev.diff(&frame.buf);
        {
            let mut backend = CrosstermBackend::new(&mut out);
            // Writing into a Vec can't fail.
            let _ = backend.draw(diff.into_iter());
        }
        if let Some((x, y)) = frame.cursor {
            let _ = queue!(out, cursor::MoveTo(x, y), cursor::Show);
        }
        let _ = out.write_all(b"\x1b[?2026l");
        self.prev = Some(frame.buf.clone());
        out
    }
}

/// Lines between panes, with their crossings joined, the ones beside the
/// focused pane in the accent colour.
pub fn draw_borders(buf: &mut Buffer, placement: &Placement, focused: Option<Rect>) {
    let area = buf.area;
    let is_border = |x: u16, y: u16| placement.borders.iter().any(|b| on_border(b, x, y));
    let cells: Vec<(u16, u16)> = placement
        .borders
        .iter()
        .flat_map(|b| {
            (0..b.len).map(move |i| {
                if b.vertical {
                    (b.x, b.y + i)
                } else {
                    (b.x + i, b.y)
                }
            })
        })
        .collect();
    for (x, y) in cells {
        if x >= area.right() || y >= area.bottom() {
            continue;
        }
        let up = y > 0 && is_border(x, y - 1);
        let down = is_border(x, y + 1);
        let left = x > 0 && is_border(x - 1, y);
        let right = is_border(x + 1, y);
        let symbol = match (up, down, left, right) {
            (true, true, true, true) => "┼",
            (true, true, true, false) => "┤",
            (true, true, false, true) => "├",
            (false, true, true, true) => "┬",
            (true, false, true, true) => "┴",
            (true, true, false, false)
            | (true, false, false, false)
            | (false, true, false, false) => "│",
            _ => "─",
        };
        let beside_focus = focused.is_some_and(|r| touches(r, x, y));
        let color = if beside_focus {
            colors::ACCENT
        } else {
            colors::BORDER
        };
        buf[(x, y)]
            .set_symbol(symbol)
            .set_style(Style::new().fg(color).bg(Color::Reset));
    }
}

fn on_border(b: &super::layout::Border, x: u16, y: u16) -> bool {
    if b.vertical {
        x == b.x && y >= b.y && y < b.y + b.len
    } else {
        y == b.y && x >= b.x && x < b.x + b.len
    }
}

/// Whether a cell is on the edge just outside a rectangle (a border of it).
fn touches(r: Rect, x: u16, y: u16) -> bool {
    let (x, y) = (x as i32, y as i32);
    let (l, t, rr, b) = (r.x as i32, r.y as i32, r.right() as i32, r.bottom() as i32);
    let beside_x = (x == l - 1 || x == rr) && y >= t - 1 && y <= b;
    let beside_y = (y == t - 1 || y == b) && x >= l - 1 && x <= rr;
    beside_x || beside_y
}

/// Gives an unfocused pane tmux's `window-style` background: cells left on
/// the terminal's default background get colour236.
pub fn tint_inactive(buf: &mut Buffer, area: Rect) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            if cell.bg == Color::Reset {
                cell.bg = colors::INACTIVE_BG;
            }
        }
    }
}

/// A tab as the status line shows it.
pub struct TabStatus {
    pub index: usize,
    pub name: String,
    pub current: bool,
}

/// Where each tab's label went on the status line, for clicks: (start
/// column, end column, tab index).
pub type TabSpans = Vec<(u16, u16, usize)>;

/// The status line in `area` (one row): the session's name, then the tabs.
pub fn draw_status(buf: &mut Buffer, area: Rect, session: &str, tabs: &[TabStatus]) -> TabSpans {
    let base = Style::new().bg(colors::STATUS_BG).fg(colors::STATUS_FG);
    buf.set_style(area, base);
    let mut x = area.x;
    let session_label = format!("  {session} ");
    x = put(
        buf,
        x,
        area,
        &session_label,
        Style::new()
            .bg(colors::SESSION_BG)
            .fg(colors::WHITE)
            .add_modifier(Modifier::BOLD),
    );
    x = put(buf, x, area, " ", base);
    let mut spans = Vec::new();
    for tab in tabs {
        let label = format!(" {}:{} ", tab.index, tab.name);
        let style = if tab.current {
            Style::new()
                .bg(colors::CURRENT_BG)
                .fg(colors::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            base.fg(colors::TAB_FG)
        };
        let start = x;
        x = put(buf, x, area, &label, style);
        spans.push((start, x, tab.index));
        if x >= area.right() {
            break;
        }
    }
    spans
}

/// Writes text from column `x`, clipped to the area; gives the column after it.
pub fn put(buf: &mut Buffer, x: u16, area: Rect, text: &str, style: Style) -> u16 {
    if x >= area.right() {
        return x;
    }
    let room = (area.right() - x) as usize;
    let (end, _) = buf.set_stringn(x, area.y, text, room, style);
    end.min(area.right())
        .max(x + UnicodeWidthStr::width(text).min(room) as u16)
}

/// A box with a rounded edge in the accent colour and a title, cleared
/// inside: the frame every popup and menu is drawn in (tmux's
/// `popup-border-lines rounded`). Gives the area inside it.
pub fn draw_box(buf: &mut Buffer, area: Rect, title: &str) -> Rect {
    let area = area.intersection(buf.area);
    if area.width < 2 || area.height < 2 {
        return Rect::default();
    }
    let edge = Style::new().fg(colors::ACCENT);
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
    if !title.is_empty() && area.width > 4 {
        put(
            buf,
            l + 2,
            Rect::new(l + 2, t, area.width - 4, 1),
            &format!(" {title} "),
            edge.add_modifier(Modifier::BOLD),
        );
    }
    Rect::new(l + 1, t + 1, area.width - 2, area.height - 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect()
    }

    #[test]
    fn the_status_line_names_the_session_and_tabs() {
        let mut frame = Frame::new(40, 1);
        let area = frame.area();
        let spans = draw_status(
            &mut frame.buf,
            area,
            "main",
            &[
                TabStatus {
                    index: 1,
                    name: "api".into(),
                    current: true,
                },
                TabStatus {
                    index: 2,
                    name: "web".into(),
                    current: false,
                },
            ],
        );
        assert!(
            row(&frame.buf, 0).starts_with("  main   1:api  2:web "),
            "{:?}",
            row(&frame.buf, 0)
        );
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[1].2, 2);
    }

    #[test]
    fn a_second_frame_sends_only_what_changed() {
        let mut screen = Screen::default();
        let mut frame = Frame::new(10, 2);
        frame.buf.set_string(0, 0, "hello", Style::new());
        let first = screen.render(&frame);
        assert!(String::from_utf8_lossy(&first).contains("hello"));
        frame.buf.set_string(0, 1, "x", Style::new());
        let second = String::from_utf8_lossy(&screen.render(&frame)).into_owned();
        assert!(
            second.contains('x') && !second.contains("hello"),
            "{second:?}"
        );
    }

    #[test]
    fn crossing_lines_join() {
        use super::super::layout::Border;
        let mut frame = Frame::new(5, 5);
        let placement = Placement {
            panes: vec![],
            borders: vec![
                Border {
                    x: 2,
                    y: 0,
                    len: 5,
                    vertical: true,
                },
                Border {
                    x: 0,
                    y: 2,
                    len: 5,
                    vertical: false,
                },
            ],
        };
        draw_borders(&mut frame.buf, &placement, None);
        assert_eq!(frame.buf[(2, 2)].symbol(), "┼");
        assert_eq!(frame.buf[(2, 0)].symbol(), "│");
        assert_eq!(frame.buf[(0, 2)].symbol(), "─");
    }
}
