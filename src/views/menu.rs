//! A board's menu in the pager (`m`, a right click, or a click on the
//! footer's "m view actions"; UX 2.0 board 154): reading the view again and
//! what its board offers (`board::Board::actions`), the PR view's opening
//! it on GitHub, seeing its failing checks and having the tab's Claude
//! resolve its conflicts. New with the PR view taking in M-m's box, with
//! no tmux-version file behind it.
//!
//! It is the workspace's own menu (`ui::Menu`, crates/ui/src/menu.rs), so
//! it looks, searches, takes clicks and lights under the mouse as every
//! menu does. The pager draws with escapes of its own rather than through
//! ratatui's cells, so the menu is drawn into a buffer the pane's size and
//! its box's cells written over the pager's frame (`ansi`); the pager's
//! crossterm keys and clicks are handed to it as the workspace's
//! (`key_of`, `mouse_of`).

use crossterm::event::{KeyCode as CKey, KeyEvent, KeyModifiers, MouseButton as CButton};
use crossterm::event::{MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

use super::board::Do;
use crate::mux::input::{Key, KeyCode, Mods, Mouse, MouseButton, MouseKind};
use crate::ui::{Look, Menu, MenuOption, Outcome, Overlay, Step};

/// The option that reads the view again, as r does.
pub const AGAIN: &str = "Read it again";

/// What an option does: read the view again, or one of the board's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    Again,
    Board(Do),
}

/// The menu while it is up, and what each of its options does.
pub struct Open {
    menu: Menu,
    choices: Vec<Choice>,
    look: Look,
}

/// What came of a key or click while it is up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Came {
    /// Still up, drawn again or not.
    Up(bool),
    /// Closed, with an option chosen or none.
    Closed(Option<Choice>),
}

impl Open {
    /// The menu titled `title`, reading again first when the view follows
    /// what it shows, then `actions`; the first action chosen as it opens,
    /// beside `near` (a click's cell) or in the middle.
    pub fn new(
        title: &str,
        follows: bool,
        actions: &[(String, Do)],
        near: Option<(u16, u16)>,
    ) -> Open {
        let mut choices = Vec::new();
        let mut options = Vec::new();
        if follows {
            choices.push(Choice::Again);
            options.push(MenuOption::new(AGAIN));
        }
        for (label, does) in actions {
            choices.push(Choice::Board(does.clone()));
            options.push(MenuOption::new(label.clone()));
        }
        // The last of a board's options are what it offers now (board 154's
        // /resolve-conflicts and failing checks), after opening it on GitHub.
        let first = choices
            .iter()
            .position(|c| matches!(c, Choice::Board(Do::Press(_))))
            .unwrap_or(0);
        let mut menu = Menu::new(title, options).at(first);
        if let Some(at) = near {
            menu.open_near(at);
        }
        Open {
            menu,
            choices,
            look: Look::read(),
        }
    }

    fn came(&self, step: Step) -> Came {
        match step {
            Step::Close(Outcome::Chose(i)) => Came::Closed(self.choices.get(i).cloned()),
            Step::Close(_) | Step::Pass => Came::Closed(None),
            Step::Redraw => Came::Up(true),
            _ => Came::Up(false),
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Came {
        match key_of(key) {
            Some(key) => {
                let step = self.menu.key(key, &self.look);
                self.came(step)
            }
            None => Came::Up(false),
        }
    }

    pub fn mouse(&mut self, mouse: MouseEvent) -> Came {
        match mouse_of(mouse) {
            Some(mouse) => {
                let step = self.menu.mouse(mouse, &self.look);
                self.came(step)
            }
            None => Came::Up(false),
        }
    }

    /// The bytes that draw it over a pane `cols` by `rows`.
    pub fn draw(&mut self, cols: u16, rows: u16) -> String {
        let mut buf = Buffer::empty(Rect::new(0, 0, cols, rows));
        self.menu.draw(&mut buf, &self.look);
        let area = self.menu.stands_out().unwrap_or(buf.area);
        ansi(&buf, area.intersection(buf.area))
    }
}

/// A crossterm key as the workspace's, for the menu.
pub fn key_of(key: KeyEvent) -> Option<Key> {
    let m = key.modifiers;
    let mods = Mods {
        shift: m.contains(KeyModifiers::SHIFT),
        alt: m.contains(KeyModifiers::ALT),
        ctrl: m.contains(KeyModifiers::CONTROL),
    };
    let code = match key.code {
        CKey::Char(c) => KeyCode::Char(c),
        CKey::Enter => KeyCode::Enter,
        CKey::Tab => KeyCode::Tab,
        CKey::BackTab => KeyCode::BackTab,
        CKey::Backspace => KeyCode::Backspace,
        CKey::Esc => KeyCode::Esc,
        CKey::Left => KeyCode::Left,
        CKey::Right => KeyCode::Right,
        CKey::Up => KeyCode::Up,
        CKey::Down => KeyCode::Down,
        CKey::Home => KeyCode::Home,
        CKey::End => KeyCode::End,
        CKey::PageUp => KeyCode::PageUp,
        CKey::PageDown => KeyCode::PageDown,
        CKey::Insert => KeyCode::Insert,
        CKey::Delete => KeyCode::Delete,
        CKey::F(n) => KeyCode::F(n),
        _ => return None,
    };
    Some(Key { code, mods })
}

/// A crossterm mouse event as the workspace's, for the menu.
pub fn mouse_of(mouse: MouseEvent) -> Option<Mouse> {
    let button = |b: CButton| match b {
        CButton::Left => MouseButton::Left,
        CButton::Middle => MouseButton::Middle,
        CButton::Right => MouseButton::Right,
    };
    let (kind, button) = match mouse.kind {
        MouseEventKind::Down(b) => (MouseKind::Press, button(b)),
        MouseEventKind::Up(b) => (MouseKind::Release, button(b)),
        MouseEventKind::Drag(b) => (MouseKind::Drag, button(b)),
        MouseEventKind::Moved => (MouseKind::Move, MouseButton::None),
        MouseEventKind::ScrollDown => (MouseKind::Press, MouseButton::WheelDown),
        MouseEventKind::ScrollUp => (MouseKind::Press, MouseButton::WheelUp),
        MouseEventKind::ScrollLeft => (MouseKind::Press, MouseButton::WheelLeft),
        MouseEventKind::ScrollRight => (MouseKind::Press, MouseButton::WheelRight),
    };
    let m = mouse.modifiers;
    Some(Mouse {
        kind,
        button,
        col: mouse.column,
        row: mouse.row,
        mods: Mods {
            shift: m.contains(KeyModifiers::SHIFT),
            alt: m.contains(KeyModifiers::ALT),
            ctrl: m.contains(KeyModifiers::CONTROL),
        },
    })
}

/// SGR's parameters for a color, `base` 30 for the text and 40 for the
/// background.
fn color(c: Color, base: u8) -> String {
    let named = |n: u8| format!("{}", base + n);
    let bright = |n: u8| format!("{}", base + 60 + n);
    match c {
        Color::Reset => format!("{}", base + 9),
        Color::Black => named(0),
        Color::Red => named(1),
        Color::Green => named(2),
        Color::Yellow => named(3),
        Color::Blue => named(4),
        Color::Magenta => named(5),
        Color::Cyan => named(6),
        Color::Gray => named(7),
        Color::DarkGray => bright(0),
        Color::LightRed => bright(1),
        Color::LightGreen => bright(2),
        Color::LightYellow => bright(3),
        Color::LightBlue => bright(4),
        Color::LightMagenta => bright(5),
        Color::LightCyan => bright(6),
        Color::White => bright(7),
        Color::Indexed(i) => format!("{};5;{i}", base + 8),
        Color::Rgb(r, g, b) => format!("{};2;{r};{g};{b}", base + 8),
    }
}

/// The cells of `area` in `buf` as the escapes that draw them where they
/// are, a row at a time.
pub fn ansi(buf: &Buffer, area: Rect) -> String {
    let mut out = String::new();
    for y in area.top()..area.bottom() {
        out.push_str(&format!("\x1b[{};{}H", y + 1, area.left() + 1));
        let mut pen: Option<String> = None;
        let mut covered = 0;
        for x in area.left()..area.right() {
            // The columns after a wide character are drawn by it.
            if covered > 0 {
                covered -= 1;
                continue;
            }
            let Some(cell) = buf.cell((x, y)) else {
                continue;
            };
            let mut sgr = format!("0;{};{}", color(cell.fg, 30), color(cell.bg, 40));
            for (flag, code) in [
                (Modifier::BOLD, "1"),
                (Modifier::DIM, "2"),
                (Modifier::ITALIC, "3"),
                (Modifier::UNDERLINED, "4"),
                (Modifier::REVERSED, "7"),
                (Modifier::CROSSED_OUT, "9"),
            ] {
                if cell.modifier.contains(flag) {
                    sgr.push(';');
                    sgr.push_str(code);
                }
            }
            if pen.as_deref() != Some(sgr.as_str()) {
                out.push_str(&format!("\x1b[{sgr}m"));
                pen = Some(sgr);
            }
            out.push_str(cell.symbol());
            covered = crate::ui::draw::width(cell.symbol()).saturating_sub(1);
        }
        out.push_str("\x1b[0m");
    }
    out
}
