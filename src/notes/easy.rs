//! Notes for people who don't know Vim's keys, ported from
//! notes/lua/notes/easy.lua: the toolbar over a note (a button for each of
//! the Format menu's styles, the notes_toolbar setting, on by default), and
//! easy mode (the easy_keys setting, off by default): Shift-arrows and the
//! mouse select, typing replaces a selection, Cmd-S saves, Cmd-Z undoes,
//! Cmd-Shift-Z redoes and Cmd-X cuts, with a line naming the keys at the
//! bottom (key_help_line). Vim's keys keep working beside them.
//!
//! Switchboard's Ghostty config sends the Cmd keys as Ctrl+Alt+Shift ones
//! (CSI u), which the workspace passes only to a pane Notes marks
//! `@notes_cmd_keys` while it runs in easy mode.

use ratatui::layout::Rect;

use crate::mux::input::{Key, KeyCode};
use crate::ui::draw::width;

/// A toolbar button: the Format menu item it runs, by its label, so the two
/// can't drift apart, or the whole menu (⋯).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Button {
    pub item: Option<&'static str>,
    pub label: &'static str,
}

pub const BUTTONS: [Button; 12] = [
    Button {
        item: Some("Bold"),
        label: "B",
    },
    Button {
        item: Some("Italic"),
        label: "I",
    },
    Button {
        item: Some("Strikethrough"),
        label: "S",
    },
    Button {
        item: Some("Monospaced text"),
        label: "</>",
    },
    Button {
        item: Some("Title"),
        label: "H1",
    },
    Button {
        item: Some("Heading"),
        label: "H2",
    },
    Button {
        item: Some("Bulleted List"),
        label: "•",
    },
    Button {
        item: Some("Checklist"),
        label: "☐",
    },
    Button {
        item: Some("Table"),
        label: "Table",
    },
    Button {
        item: Some("Link…"),
        label: "Link",
    },
    Button {
        item: Some("Paste image"),
        label: "Image",
    },
    Button {
        item: None,
        label: "⋯",
    },
];

/// Where each button shown sits on a toolbar `cols` wide starting at `x`:
/// (its index in `BUTTONS`, its cells). The buttons that don't fit drop off
/// the end, ⋯ (the whole Format menu, which has them all) always kept.
pub fn toolbar(x: u16, y: u16, cols: u16) -> Vec<(usize, Rect)> {
    let text_w = |b: &Button| width(b.label) as u16 + 2;
    let menu = BUTTONS.len() - 1;
    let room = cols.saturating_sub(1 + text_w(&BUTTONS[menu]) + 1);
    let mut shown = Vec::new();
    let mut used = 0;
    for (i, b) in BUTTONS[..menu].iter().enumerate() {
        let w = text_w(b) + 1;
        if used + w > room {
            break;
        }
        used += w;
        shown.push(i);
    }
    shown.push(menu);
    let mut at = x + 1;
    shown
        .into_iter()
        .map(|i| {
            let w = text_w(&BUTTONS[i]);
            let r = Rect::new(at, y, w, 1);
            at += w + 1;
            (i, r)
        })
        .collect()
}

/// The keys that work now, named in the help line at the bottom, by mode.
pub fn key_help(mode: HelpMode) -> &'static str {
    match mode {
        HelpMode::Normal => {
            "i type · Shift+arrows select · Ctrl-P format · Enter tick, fold or follow · Ctrl-F find · Ctrl-B list · Cmd-Z undo · Cmd-S save"
        }
        HelpMode::Insert => {
            "Esc stop typing · Shift+arrows select · Ctrl-P format · Tab indent · Enter next item · Cmd-Z undo"
        }
        HelpMode::Visual => {
            "Type to replace · Ctrl-P format the selection · Cmd-X cut · Ctrl-C copy · Esc cancel"
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelpMode {
    Normal,
    Insert,
    Visual,
}

/// What a Cmd key is, from the Ctrl+Alt+Shift key Ghostty sends for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    Save,
    Undo,
    Redo,
    Cut,
}

pub fn cmd_key(key: &Key) -> Option<Cmd> {
    if !(key.mods.ctrl && key.mods.alt) {
        return None;
    }
    match key.code {
        KeyCode::Char(c) => match c.to_ascii_lowercase() {
            's' => Some(Cmd::Save),
            'z' => Some(Cmd::Undo),
            'y' => Some(Cmd::Redo),
            'x' => Some(Cmd::Cut),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::input::Mods;

    #[test]
    fn the_toolbar_keeps_the_first_buttons_and_the_menu_in_a_narrow_window() {
        let wide = toolbar(0, 0, 200);
        assert_eq!(
            wide.len(),
            BUTTONS.len(),
            "one click region per button, given the room"
        );
        let narrow = toolbar(0, 0, 30);
        assert_eq!(narrow[0].0, 0, "a narrow window keeps the first buttons");
        assert_eq!(
            narrow.last().unwrap().0,
            BUTTONS.len() - 1,
            "and ⋯, for the rest"
        );
        assert!(narrow.len() < BUTTONS.len());
        assert!(
            narrow.iter().all(|(_, r)| r.right() <= 30),
            "nothing past the window's edge"
        );
    }

    #[test]
    fn the_cmd_keys_are_read_from_what_ghostty_sends() {
        let all = Mods {
            shift: true,
            alt: true,
            ctrl: true,
        };
        assert_eq!(cmd_key(&Key::new(KeyCode::Char('s'), all)), Some(Cmd::Save));
        assert_eq!(cmd_key(&Key::new(KeyCode::Char('y'), all)), Some(Cmd::Redo));
        assert_eq!(cmd_key(&Key::new(KeyCode::Char('s'), Mods::CTRL)), None);
    }
}
