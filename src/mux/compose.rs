//! Drawing a client's screen. Each frame is built whole in a cell buffer (the
//! tab's panes, the lines between them, the status line, anything over them),
//! then only the cells that changed since the client's last frame are sent,
//! inside a synchronized update so the terminal never shows half a frame.
//!
//! The look is tmux.conf's: lines between panes in colour238, the focused
//! pane's in the current tab's color (update-colors.sh's
//! `pane-active-border-style`), panes without the focus on a colour236
//! background (its `window-style`) and the focused one on the terminal's
//! own, and the status line at the bottom with the session's name and the
//! tabs; theme.sh's light shades in place of the dark ones with the theme
//! light (src/status/theme.rs).

use std::io::Write;

use crossterm::{cursor, queue};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use unicode_width::UnicodeWidthStr;

use super::layout::Placement;
use crate::agent::State;
use crate::status::{Hits, Left, Palette};

/// tmux.conf's colours, by the 256-colour palette's numbers: the dark
/// theme's (src/status/theme.rs has both).
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
    /// The cursor's shape, as the focused pane's program asked (a bar in
    /// Neovim's Insert mode).
    pub cursor_style: Option<cursor::SetCursorStyle>,
}

impl Frame {
    pub fn new(cols: u16, rows: u16) -> Frame {
        Frame {
            buf: Buffer::empty(Rect::new(0, 0, cols, rows)),
            cursor: None,
            cursor_style: None,
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
    style: Option<cursor::SetCursorStyle>,
}

impl Screen {
    /// The next frame draws everything again (after an attach, a resize, or
    /// anything that may have left the terminal showing something else).
    pub fn invalidate(&mut self) {
        self.prev = None;
        self.style = None;
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
            let style = frame
                .cursor_style
                .unwrap_or(cursor::SetCursorStyle::DefaultUserShape);
            if self.style != Some(style) {
                let _ = queue!(out, style);
                self.style = Some(style);
            }
            let _ = queue!(out, cursor::MoveTo(x, y), cursor::Show);
        }
        let _ = out.write_all(b"\x1b[?2026l");
        self.prev = Some(frame.buf.clone());
        out
    }
}

/// Lines between panes, with their crossings joined, in `line`, the ones
/// beside the focused pane in `focus` (the current tab's color).
pub fn draw_borders(
    buf: &mut Buffer,
    placement: &Placement,
    focused: Option<Rect>,
    focus: Color,
    line: Color,
) {
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
        let color = if beside_focus { focus } else { line };
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
/// the terminal's default background get `bg` (colour236, or colour254
/// light).
pub fn tint_inactive(buf: &mut Buffer, area: Rect, bg: Color) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            if cell.bg == Color::Reset {
                cell.bg = bg;
            }
        }
    }
}

/// A tab as the status line shows it.
pub struct TabStatus {
    pub index: usize,
    pub name: String,
    pub current: bool,
    /// Its Claude's state, most urgent of its panes' (src/agent).
    pub agent: Option<State>,
    /// Its project's accent color (src/tabs), which its number and name are
    /// drawn in; `None` keeps tmux.conf's own colours.
    pub color: Option<Color>,
}

/// The badge a Claude state wears (tmux.conf's `@agent_marker`): a black !
/// on yellow when it is waiting on you, a black ✓ on green when it has
/// responded and you have not looked, a white ⋯ on muted blue while it
/// works, a colour that reads as busy rather than as needing you. Badges
/// with their own background, because tab names are drawn in each project's
/// accent colour, and a coloured dot vanishes on a tab of the same colour.
fn badge(state: State) -> (&'static str, Style) {
    let (symbol, bg, fg) = match state {
        State::Waiting => ("!", 220, 232),
        State::Done => ("✓", 41, 232),
        State::Working => ("⋯", 67, 255),
    };
    (
        symbol,
        Style::new()
            .bg(Color::Indexed(bg))
            .fg(Color::Indexed(fg))
            .add_modifier(Modifier::BOLD),
    )
}

/// Where each tab's label went on the status line, for clicks: (start
/// column, end column, tab index).
pub type TabSpans = Vec<(u16, u16, usize)>;

/// The status line in `area` (one row): the session's name on its badge in
/// the current tab's color, how many of its tabs wait on you, have finished
/// unseen and are working (tmux.conf's `@agent_counts`, a zero hiding its
/// badge), the ⌘ Commands button and the health light (src/status), then
/// the tabs, each with its badge after its number. Gives where the button
/// and each tab went, for clicks.
pub fn draw_status(
    buf: &mut Buffer,
    area: Rect,
    session: &str,
    left: &Left,
    tabs: &[TabStatus],
    p: &Palette,
) -> Hits {
    let base = Style::new().bg(p.status_bg).fg(p.status_fg);
    buf.set_style(area, base);
    let mut x = area.x;
    let session_label = format!("  {session} ");
    x = put(
        buf,
        x,
        area,
        &session_label,
        crate::status::session_style(left, p),
    );
    for state in [State::Waiting, State::Done, State::Working] {
        let n = tabs.iter().filter(|t| t.agent == Some(state)).count();
        if n > 0 {
            let (symbol, style) = badge(state);
            x = put(buf, x, area, &format!("  {symbol} {n}  "), style);
        }
    }
    let (after, button) = crate::status::draw_left(buf, x, area, left, p);
    x = after;
    let mut spans = Vec::new();
    // Each tab in its project's color, as window-names.sh's
    // window-status-format and update-colors.sh's current one drew it: the
    // current tab bold on colour238, the rest on the status line's own.
    for tab in tabs {
        let style = if tab.current {
            Style::new()
                .bg(p.current_bg)
                .fg(tab.color.unwrap_or(colors::ACCENT))
                .add_modifier(Modifier::BOLD)
        } else {
            base.fg(tab.color.unwrap_or(p.tab_fg))
        };
        let start = x;
        x = put(buf, x, area, &format!(" {}:", tab.index), style);
        if let Some(state) = tab.agent {
            let (symbol, marker) = badge(state);
            x = put(buf, x, area, &format!("  {symbol}  "), marker);
            x = put(buf, x, area, " ", style);
        }
        x = put(buf, x, area, &format!("{} ", tab.name), style);
        spans.push((start, x, tab.index));
        if x >= area.right() {
            break;
        }
    }
    Hits {
        button,
        tabs: spans,
    }
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
            &Left::default(),
            &[
                TabStatus {
                    index: 1,
                    name: "api".into(),
                    current: true,
                    agent: None,
                    color: None,
                },
                TabStatus {
                    index: 2,
                    name: "web".into(),
                    current: false,
                    agent: None,
                    color: None,
                },
            ],
            &Palette::DARK,
        );
        assert!(
            row(&frame.buf, 0).starts_with("  main   1:api  2:web "),
            "{:?}",
            row(&frame.buf, 0)
        );
        assert_eq!(spans.tabs.len(), 2);
        assert_eq!(spans.tabs[1].2, 2);
        assert_eq!(spans.button, None);
    }

    #[test]
    fn each_tab_is_drawn_in_its_project_s_color() {
        let mut frame = Frame::new(40, 1);
        let area = frame.area();
        let tab = |index: usize, name: &str, color: Option<Color>| TabStatus {
            index,
            name: name.into(),
            current: index == 1,
            agent: None,
            color,
        };
        let green = Color::Rgb(0x23, 0xef, 0x59);
        let red = Color::Rgb(0xf4, 0x28, 0x28);
        draw_status(
            &mut frame.buf,
            area,
            "main",
            &Left::default(),
            &[
                tab(1, "api", Some(green)),
                tab(2, "web", Some(red)),
                tab(3, "x", None),
            ],
            &Palette::DARK,
        );
        let line = row(&frame.buf, 0);
        let at = |text: &str| line.find(text).unwrap() as u16;
        let api = &frame.buf[(at("api"), 0)];
        assert_eq!((api.fg, api.bg), (green, colors::CURRENT_BG));
        assert!(api.modifier.contains(Modifier::BOLD));
        let web = &frame.buf[(at("web"), 0)];
        assert_eq!((web.fg, web.bg), (red, colors::STATUS_BG));
        assert!(!web.modifier.contains(Modifier::BOLD));
        assert_eq!(frame.buf[(at("2:"), 0)].fg, red);
        // Unknown yet: tmux.conf's own colour.
        assert_eq!(frame.buf[(at("x "), 0)].fg, colors::TAB_FG);
    }

    #[test]
    fn claudes_states_show_as_badges_and_counts() {
        let mut frame = Frame::new(80, 1);
        let area = frame.area();
        let tab = |index: usize, name: &str, agent: Option<State>| TabStatus {
            index,
            name: name.into(),
            current: index == 1,
            agent,
            color: None,
        };
        let spans = draw_status(
            &mut frame.buf,
            area,
            "main",
            &Left::default(),
            &[
                tab(1, "api", Some(State::Waiting)),
                tab(2, "web", Some(State::Done)),
                tab(3, "db", Some(State::Waiting)),
                tab(4, "ops", None),
            ],
            &Palette::DARK,
        );
        let line = row(&frame.buf, 0);
        assert!(
            line.starts_with("  main   ! 2    ✓ 1    1:  !   api  2:  ✓   web  3:  !   db  4:ops "),
            "{line:?}"
        );
        // The badge has colours of its own, and the tab's click span covers it.
        let bang = line.find('!').unwrap() as u16;
        assert_eq!(frame.buf[(bang, 0)].bg, Color::Indexed(220));
        let check = line.chars().position(|c| c == '✓').unwrap() as u16;
        assert_eq!(frame.buf[(check, 0)].bg, Color::Indexed(41));
        assert_eq!(
            spans.tabs[0].1 - spans.tabs[0].0,
            " 1:  !   api ".chars().count() as u16
        );
    }

    #[test]
    fn the_session_wears_the_tab_s_color_and_the_button_comes_before_the_tabs() {
        let mut frame = Frame::new(60, 1);
        let area = frame.area();
        let green = Color::Rgb(0x23, 0xef, 0x59);
        let left = Left {
            color: Some(green),
            button: true,
            health: Vec::new(),
        };
        let hits = draw_status(
            &mut frame.buf,
            area,
            "main",
            &left,
            &[TabStatus {
                index: 1,
                name: "api".into(),
                current: true,
                agent: None,
                color: Some(green),
            }],
            &Palette::LIGHT,
        );
        let line = row(&frame.buf, 0);
        assert!(line.starts_with("  main   ⌘ Commands   1:api "), "{line:?}");
        assert_eq!(frame.buf[(2, 0)].bg, green);
        assert_eq!(frame.buf[(2, 0)].fg, Palette::LIGHT.bright);
        assert_eq!(hits.button, Some((8, 20)));
        assert_eq!(hits.tabs, vec![(21, 28, 1)]);
        assert_eq!(frame.buf[(21, 0)].bg, Palette::LIGHT.current_bg);
        assert_eq!(frame.buf[(30, 0)].bg, Palette::LIGHT.status_bg);
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
        draw_borders(
            &mut frame.buf,
            &placement,
            None,
            colors::ACCENT,
            colors::BORDER,
        );
        assert_eq!(frame.buf[(2, 2)].symbol(), "┼");
        assert_eq!(frame.buf[(2, 0)].symbol(), "│");
        assert_eq!(frame.buf[(0, 2)].symbol(), "─");
    }
}
