//! Dark and light: the shades the status line, the lines between panes, the
//! panes without the focus and the boxes are drawn in, as tmux/theme.sh set
//! them for each mode (`status-style`, `window-status-format`,
//! `@button_style`, `pane-border-style`, `window-style`), and which mode is
//! in use.
//!
//! The `theme` setting picks one. Its `system` choice followed macOS's
//! appearance through `defaults read -g AppleInterfaceStyle`; here it follows
//! the terminal's own background, which the server asks each client's
//! terminal for as it attaches (OSC 11) and reads from the reply among the
//! keys it sends. A terminal that never answers leaves it dark, as tmux.conf's
//! colours were. The answer is kept for the whole server, as tmux kept one
//! theme for every client: the latest terminal to answer says which.
//!
//! theme.sh also switched Ghostty's own theme and wrote the mode to
//! `.theme-mode` for the scripts to read; the server is the only reader now,
//! and the terminal's theme is the terminal's business.

use std::sync::atomic::{AtomicU8, Ordering};

use ratatui::style::Color;
use serde_json::Value;

use crate::settings;

/// The shades one mode draws in, by the 256-colour palette's numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// The status line's background and text (`status-style`, theme.sh's
    /// `BG_DARK` and `FG`).
    pub status_bg: Color,
    pub status_fg: Color,
    /// A tab's number and name before its color is known (`FG_DIM`).
    pub tab_fg: Color,
    /// The current tab's background (`BG_MED`).
    pub current_bg: Color,
    /// The session's name on its badge (`FG_BRIGHT`), and the second status
    /// line's label.
    pub bright: Color,
    /// Fainter words on the second status line (`FG_DIM`).
    pub dim: Color,
    /// The lines between panes (`pane-border-style`, `BG_MED`).
    pub border: Color,
    /// The background of a pane without the focus (`window-style`, `BG_DIM`).
    pub inactive_bg: Color,
    /// The ⌘ Commands button (`@button_style`).
    pub button_bg: Color,
    pub button_fg: Color,
}

impl Palette {
    pub const DARK: Palette = Palette {
        status_bg: Color::Indexed(235),
        status_fg: Color::Indexed(248),
        tab_fg: Color::Indexed(244),
        current_bg: Color::Indexed(238),
        bright: Color::Indexed(255),
        dim: Color::Indexed(244),
        border: Color::Indexed(238),
        inactive_bg: Color::Indexed(236),
        button_bg: Color::Indexed(238),
        button_fg: Color::Indexed(255),
    };
    pub const LIGHT: Palette = Palette {
        status_bg: Color::Indexed(253),
        status_fg: Color::Indexed(234),
        tab_fg: Color::Indexed(238),
        current_bg: Color::Indexed(248),
        bright: Color::Indexed(232),
        dim: Color::Indexed(238),
        border: Color::Indexed(248),
        inactive_bg: Color::Indexed(254),
        button_bg: Color::Indexed(250),
        button_fg: Color::Indexed(232),
    };
}

/// The session badge's background before the current tab's color is known
/// (tmux.conf's `colour24`, which update-colors.sh replaced on the first
/// focus).
pub const SESSION_BG: Color = Color::Indexed(24);
/// The accent: the focused pane's lines and the current tab before its
/// color is known (`colour81`).
pub const ACCENT: Color = Color::Indexed(81);
/// The health light's red (`colour196`).
pub const RED: Color = Color::Indexed(196);
/// The second status line's "waiting on you" and "finished" (`colour220`,
/// `colour41`), the badges' own yellow and green.
pub const YELLOW: Color = Color::Indexed(220);
pub const GREEN: Color = Color::Indexed(41);

/// What a terminal said its background is: not known, dark or light.
static SYSTEM: AtomicU8 = AtomicU8::new(UNKNOWN);
const UNKNOWN: u8 = 0;
const DARK: u8 = 1;
const LIGHT: u8 = 2;

/// What the server writes to a terminal as it attaches to ask for its
/// background colour (OSC 11). A terminal that knows it answers with
/// `ESC ] 11 ; rgb:RRRR/GGGG/BBBB` and the same terminator.
pub const QUERY: &[u8] = b"\x1b]11;?\x1b\\";

/// Whether a value of the `theme` setting, as it stands, is light: `light`,
/// or `system` on a terminal that said it is light.
pub fn light_for(value: &Value) -> bool {
    match value.as_str() {
        Some("light") => true,
        Some("system") => SYSTEM.load(Ordering::Relaxed) == LIGHT,
        _ => false,
    }
}

/// Whether the workspace is drawn light now, from the settings as last read.
pub fn light() -> bool {
    settings::cached("theme").is_some_and(|v| light_for(&v))
}

/// The palette to draw in now.
pub fn palette() -> &'static Palette {
    if light() {
        &Palette::LIGHT
    } else {
        &Palette::DARK
    }
}

/// Keeps what a terminal said its background is; gives whether that
/// changed what `system` means.
pub fn set_system(light: bool) -> bool {
    let new = if light { LIGHT } else { DARK };
    SYSTEM.swap(new, Ordering::Relaxed) != new
}

/// The mode M-p's "Toggle light and dark theme" switches to: the other one
/// from what is drawn now, `system` resolved to what it shows, as
/// settings.sh's `toggle-theme` read the mode theme.sh last applied.
pub fn toggled() -> &'static str {
    if light() { "dark" } else { "light" }
}

/// Reads a terminal's answer to `QUERY`: whether its background is light,
/// or `None` for anything else. Each channel is one to four hex digits,
/// scaled by its own width as xterm's `rgb:` form is; light is a perceived
/// brightness over half.
pub fn read_reply(raw: &[u8]) -> Option<bool> {
    let text = std::str::from_utf8(raw).ok()?;
    let body = text.strip_prefix("\x1b]11;")?;
    let body = body
        .strip_suffix('\x07')
        .or_else(|| body.strip_suffix("\x1b\\"))?;
    let channels = body
        .strip_prefix("rgb:")
        .or_else(|| body.strip_prefix("rgba:"))?;
    let mut parts = channels.split('/').map(|hex| {
        if hex.is_empty() || hex.len() > 4 {
            return None;
        }
        let value = u32::from_str_radix(hex, 16).ok()?;
        let max = (1u32 << (4 * hex.len() as u32)) - 1;
        Some(f64::from(value) / f64::from(max))
    });
    let (r, g, b) = (parts.next()??, parts.next()??, parts.next()??);
    Some(0.299 * r + 0.587 * g + 0.114 * b > 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terminal_s_background_reads_as_light_or_dark() {
        assert_eq!(read_reply(b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\"), Some(true));
        assert_eq!(read_reply(b"\x1b]11;rgb:1d1d/1f1f/2121\x07"), Some(false));
        // Two digits a channel, and kitty's rgba.
        assert_eq!(read_reply(b"\x1b]11;rgb:fa/f8/f0\x07"), Some(true));
        assert_eq!(
            read_reply(b"\x1b]11;rgba:0000/0000/0000/ffff\x07"),
            Some(false)
        );
        // Not an answer to the query.
        assert_eq!(read_reply(b"\x1b]10;rgb:ffff/ffff/ffff\x07"), None);
        assert_eq!(read_reply(b"\x1b]11;?\x07"), None);
        assert_eq!(read_reply(b"\x1b]11;rgb:zz/00/00\x07"), None);
        assert_eq!(read_reply(b"\x1b]11;rgb:00/00\x07"), None);
    }

    #[test]
    fn system_follows_the_terminal_and_the_others_are_what_they_say() {
        assert!(light_for(&Value::from("light")));
        assert!(!light_for(&Value::from("dark")));
        set_system(true);
        assert!(light_for(&Value::from("system")));
        assert!(!set_system(true), "the same answer changes nothing");
        assert!(set_system(false));
        assert!(!light_for(&Value::from("system")));
    }

    #[test]
    fn the_light_palette_is_theme_sh_s() {
        assert_eq!(Palette::LIGHT.status_bg, Color::Indexed(253));
        assert_eq!(Palette::LIGHT.button_bg, Color::Indexed(250));
        assert_eq!(Palette::DARK.inactive_bg, Color::Indexed(236));
        assert_eq!(Palette::DARK.button_fg, Color::Indexed(255));
    }
}
