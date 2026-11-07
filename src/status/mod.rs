//! The status line's parts beyond the tabs: what tmux.conf's `status-left`
//! held after the session's name (the ⌘ Commands button and the health
//! light), the session badge and the focused pane's lines in the current
//! tab's color (tmux/update-colors.sh), the second status line under the
//! tabs (tmux/tab-status.sh), and dark and light (tmux/theme.sh). The tabs
//! themselves, the session's name and the Claude counts are drawn by
//! `mux::compose::draw_status`, which calls in here for the rest.
//!
//! - `theme`: the palettes for dark and light, and which is in use.
//! - `health`: health.sh's checks, read on the server's tick.
//! - `second`: the second status line, from what the server holds.
//! - `serve`: the server's side: the tick, drawing the bottom rows, a click
//!   on the button, the terminal's answer about its background, M-p's
//!   "Toggle light and dark theme".
//!
//! How many rows the status line takes is the one thing the layout needs
//! from here (`lines`): the server sets it on its tick from the
//! `second_status` setting, and lays every tab out again when it changes.

pub mod health;
pub mod second;
mod serve;
pub mod theme;

use std::sync::atomic::{AtomicU16, Ordering};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

pub use theme::Palette;

use crate::mux::compose::{TabSpans, put};

/// The rows the status line takes, as the server last laid the tabs out.
static LINES: AtomicU16 = AtomicU16::new(1);

/// The rows the status line takes at the bottom of the screen: 2 with the
/// second status line on, else 1.
pub fn lines() -> u16 {
    LINES.load(Ordering::Relaxed)
}

/// What the server keeps for the status line.
#[derive(Debug, Default)]
pub struct Status {
    /// What the health light says is wrong, as last checked.
    pub health: Vec<String>,
}

/// What `status-left` shows after the Claude counts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Left {
    /// The current tab's color, which the session's badge wears
    /// (update-colors.sh); `None` before it is known.
    pub color: Option<Color>,
    /// The ⌘ Commands button (`palette_button` setting).
    pub button: bool,
    /// What the health light names, empty while all is well.
    pub health: Vec<String>,
}

/// What a click on the status line can land on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// The ⌘ Commands button.
    Button,
    /// A tab, by its number.
    Tab(usize),
}

/// Where the status line's clickable parts went: the button's columns and
/// each tab's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hits {
    pub button: Option<(u16, u16)>,
    pub tabs: TabSpans,
}

impl Hits {
    /// What is at a column of the status line, as `MouseDown1Status` read
    /// `mouse_status_range`: the button, else the tab under it.
    pub fn at(&self, col: u16) -> Option<Hit> {
        if self.button.is_some_and(|(a, b)| col >= a && col < b) {
            return Some(Hit::Button);
        }
        self.tabs
            .iter()
            .find(|(a, b, _)| col >= *a && col < *b)
            .map(|(_, _, i)| Hit::Tab(*i))
    }
}

/// The session badge's style: bold in the bright text on the current tab's
/// color, as update-colors.sh rewrote `status-left`.
pub fn session_style(left: &Left, p: &Palette) -> Style {
    Style::new()
        .bg(left.color.unwrap_or(theme::SESSION_BG))
        .fg(p.bright)
        .add_modifier(Modifier::BOLD)
}

/// Draws what follows the counts from column `x`: a space on the status
/// line's own background, the ⌘ Commands button in `@button_style` and a
/// space after it, then the health light in red when something is wrong.
/// Gives the column after it and where the button went.
pub fn draw_left(
    buf: &mut Buffer,
    x: u16,
    area: Rect,
    left: &Left,
    p: &Palette,
) -> (u16, Option<(u16, u16)>) {
    let base = Style::new().bg(p.status_bg).fg(p.status_fg);
    let mut x = put(buf, x, area, " ", base);
    let mut button = None;
    if left.button {
        let start = x;
        x = put(
            buf,
            x,
            area,
            " ⌘ Commands ",
            Style::new().bg(p.button_bg).fg(p.button_fg),
        );
        button = Some((start, x));
        x = put(buf, x, area, " ", base);
    }
    if !left.health.is_empty() {
        x = put(
            buf,
            x,
            area,
            &format!(" ● {} ", left.health.join(", ")),
            base.fg(theme::RED).add_modifier(Modifier::BOLD),
        );
    }
    (x, button)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(buf: &Buffer) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, 0)].symbol().to_string())
            .collect()
    }

    #[test]
    fn the_button_and_the_health_light_follow_the_counts() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 1));
        let left = Left {
            color: None,
            button: true,
            health: vec!["relay down".into(), "Slack not connected".into()],
        };
        let (x, button) = draw_left(&mut buf, 3, Rect::new(0, 0, 60, 1), &left, &Palette::DARK);
        let line = row(&buf);
        assert!(
            line.starts_with("     ⌘ Commands   ● relay down, Slack not connected "),
            "{line:?}"
        );
        assert_eq!(button, Some((4, 16)));
        assert_eq!(buf[(5, 0)].bg, Palette::DARK.button_bg);
        let dot = line.chars().position(|c| c == '●').unwrap() as u16;
        assert_eq!(buf[(dot, 0)].fg, theme::RED);
        assert!(buf[(dot, 0)].modifier.contains(Modifier::BOLD));
        assert_eq!(
            x,
            dot + 1 + " relay down, Slack not connected ".len() as u16
        );
    }

    #[test]
    fn with_no_button_and_all_well_only_the_space_is_left() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        let (x, button) = draw_left(
            &mut buf,
            0,
            Rect::new(0, 0, 20, 1),
            &Left::default(),
            &Palette::LIGHT,
        );
        assert_eq!((x, button), (1, None));
        assert_eq!(buf[(0, 0)].bg, Palette::LIGHT.status_bg);
    }

    #[test]
    fn a_click_finds_the_button_before_the_tabs() {
        let hits = Hits {
            button: Some((10, 22)),
            tabs: vec![(23, 30, 1), (30, 36, 2)],
        };
        assert_eq!(hits.at(10), Some(Hit::Button));
        assert_eq!(hits.at(21), Some(Hit::Button));
        assert_eq!(hits.at(22), None);
        assert_eq!(hits.at(31), Some(Hit::Tab(2)));
        assert_eq!(hits.at(80), None);
    }

    #[test]
    fn the_session_badge_wears_the_tab_s_color() {
        let green = Color::Rgb(0x23, 0xef, 0x59);
        let left = Left {
            color: Some(green),
            ..Left::default()
        };
        assert_eq!(session_style(&left, &Palette::DARK).bg, Some(green));
        assert_eq!(
            session_style(&Left::default(), &Palette::LIGHT).bg,
            Some(theme::SESSION_BG)
        );
        assert_eq!(
            session_style(&Left::default(), &Palette::LIGHT).fg,
            Some(Palette::LIGHT.bright)
        );
    }
}
