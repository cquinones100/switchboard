//! A menu (tmux/menu.sh, and mission control's `chooseModal`): a few
//! options, the chosen one on the highlight across its whole row with a
//! yellow bold `›` and its label bright and bold, the others in the text
//! colour, and the keys dim at the bottom. j and k (the arrows and the wheel
//! too) move, Tab and Shift-Tab go round, through easy mode's × after the
//! options; Enter or Space takes the chosen one, a number the option with
//! that number, an option's own key that option; Esc or q goes back. A click
//! takes an option, a click outside closes it, and the mouse moving onto an
//! option moves the cursor there, as mission control's menus do.
//!
//! Text to read above the options (`body`) is wrapped to the box and cut
//! short with … when the options need the room. The box is at most 72
//! columns or two thirds of the screen, whichever is less, but never
//! narrower than its title: an option longer than that is cut with ….

use ratatui::buffer::Buffer;

use super::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use super::{Look, Outcome, Overlay, Step};
use crate::mux::input::{Key, KeyCode, Mods, Mouse};

/// One option: its label, and a key that takes it at once, shown dim at the
/// end of its row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuOption {
    pub label: String,
    pub key: Option<char>,
}

impl MenuOption {
    pub fn new(label: impl Into<String>) -> MenuOption {
        MenuOption {
            label: label.into(),
            key: None,
        }
    }

    pub fn key(mut self, key: char) -> MenuOption {
        self.key = Some(key);
        self
    }
}

pub struct Menu {
    title: String,
    body: String,
    options: Vec<MenuOption>,
    /// The chosen option; one past the last is the close button (Tab).
    cursor: usize,
    close_lit: bool,
    drawn: Option<Drawn>,
    /// Which of the box's lines is the first option, and the first option
    /// shown on it.
    first_line: usize,
    first_shown: usize,
}

impl Menu {
    pub fn new(title: impl Into<String>, options: Vec<MenuOption>) -> Menu {
        Menu {
            title: title.into(),
            body: String::new(),
            options,
            cursor: 0,
            close_lit: false,
            drawn: None,
            first_line: 0,
            first_shown: 0,
        }
    }

    /// A menu of labels alone.
    pub fn of(title: impl Into<String>, labels: &[&str]) -> Menu {
        Menu::new(title, labels.iter().map(|l| MenuOption::new(*l)).collect())
    }

    /// Text shown above the options.
    pub fn body(mut self, text: impl Into<String>) -> Menu {
        self.body = text.into();
        self
    }

    /// The option chosen when it opens.
    pub fn at(mut self, cursor: usize) -> Menu {
        self.cursor = cursor.min(self.options.len().saturating_sub(1));
        self
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    fn take(&self) -> Step {
        if self.cursor < self.options.len() {
            Step::Close(Outcome::Chose(self.cursor))
        } else {
            Step::Close(Outcome::Dismissed)
        }
    }

    fn stops(&self, look: &Look) -> usize {
        self.options.len() + usize::from(look.close)
    }
}

/// The keys a menu takes, on its bottom row.
pub(crate) const KEYS: &str = "jk or tab move  ↵ or click choose  esc back";

/// The widest a menu's inside is on a screen `cols` wide: its box 72
/// columns or two thirds of the screen, whichever is less.
fn widest(cols: usize) -> usize {
    (cols * 2 / 3).min(72).saturating_sub(2)
}

impl Overlay for Menu {
    fn kind(&self) -> &'static str {
        "menu"
    }

    fn stands_out(&self) -> Option<ratatui::layout::Rect> {
        self.drawn.as_ref().map(|d| d.outer)
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let area = buf.area;
        let close_w = if look.close {
            draw::width(draw::CLOSE)
        } else {
            0
        };
        let inner = widest(area.width as usize)
            .max(draw::width(&self.title) + 6 + close_w)
            .max(draw::width(KEYS) + 3)
            .min((area.width as usize).saturating_sub(2));
        let room = (area.height as usize).saturating_sub(6).max(1);
        let n = self.options.len();
        let fits = n.min(room);
        // The options come first, every one shown, or those around the
        // chosen one; the text takes what room is left.
        let first = self
            .cursor
            .min(n.saturating_sub(1))
            .saturating_sub(fits / 2)
            .min(n - fits);
        let said = if self.body.trim().is_empty() {
            Vec::new()
        } else {
            draw::wrap(self.body.trim(), inner.saturating_sub(2))
        };
        let body_room = room.saturating_sub(fits + 1);
        let shown: Vec<String> = if body_room < 1 {
            Vec::new()
        } else if said.len() > body_room {
            let mut s = said[..body_room - 1].to_vec();
            s.push("…".into());
            s
        } else {
            said
        };
        let mut lines: Vec<BoxLine> = shown
            .iter()
            .map(|l| BoxLine::text(format!(" {l}"), look.text()))
            .collect();
        if !lines.is_empty() {
            lines.push(BoxLine::blank());
        }
        self.first_line = lines.len();
        self.first_shown = first;
        // A row is `   label` and, two columns past the longest label, its
        // key, inside the box's margin of a column each side.
        let keyed = self.options.iter().any(|o| o.key.is_some());
        let label_room = inner.saturating_sub(5 + if keyed { 3 } else { 0 });
        let labels: Vec<String> = self
            .options
            .iter()
            .map(|o| draw::cut(&o.label, label_room))
            .collect();
        let key_col = labels.iter().map(|l| draw::width(l)).max().unwrap_or(0) + 5;
        for (i, option) in self.options.iter().enumerate().skip(first).take(fits) {
            let chosen = i == self.cursor;
            let spans = super::kit::option(&labels[i], option.key, key_col, chosen, &look.theme);
            lines.push(
                BoxLine::spans(spans)
                    .fill(chosen.then_some(look.theme.highlight))
                    .clickable(),
            );
        }
        let mut spec = BoxSpec::new(&self.title, &lines);
        spec.keys = Some(KEYS);
        if !shown.is_empty() {
            spec.width = inner as u16;
        }
        spec.close_lit = self.cursor == n || self.close_lit;
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn key(&mut self, key: Key, look: &Look) -> Step {
        let n = self.options.len();
        if n == 0 {
            return Step::Close(Outcome::Dismissed);
        }
        // An option's own key, but never one the menu moves or closes with.
        if key.mods == Mods::NONE
            && let KeyCode::Char(c) = key.code
            && !"jkgGq ".contains(c)
            && !c.is_ascii_digit()
            && let Some(i) = self.options.iter().position(|o| o.key == Some(c))
        {
            return Step::Close(Outcome::Chose(i));
        }
        let plain = key.mods == Mods::NONE;
        let ctrl = key.mods == Mods::CTRL;
        let moved = |to: usize, cursor: &mut usize| {
            if to == *cursor {
                Step::Same
            } else {
                *cursor = to;
                Step::Redraw
            }
        };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down if plain => {
                moved((self.cursor + 1).min(n - 1), &mut self.cursor)
            }
            KeyCode::Char('n') if ctrl => moved((self.cursor + 1).min(n - 1), &mut self.cursor),
            KeyCode::Char('k') | KeyCode::Up if plain => {
                moved(self.cursor.min(n).saturating_sub(1), &mut self.cursor)
            }
            KeyCode::Char('p') if ctrl => {
                moved(self.cursor.min(n).saturating_sub(1), &mut self.cursor)
            }
            KeyCode::Char('g') | KeyCode::Home if plain => moved(0, &mut self.cursor),
            KeyCode::Char('G') | KeyCode::End => moved(n - 1, &mut self.cursor),
            KeyCode::Tab if plain => {
                let stops = self.stops(look);
                moved((self.cursor + 1) % stops, &mut self.cursor)
            }
            KeyCode::BackTab => {
                let stops = self.stops(look);
                moved((self.cursor + stops - 1) % stops, &mut self.cursor)
            }
            KeyCode::Enter | KeyCode::Char(' ') if plain => self.take(),
            KeyCode::Char(c @ '1'..='9') if plain => {
                let i = c as usize - '1' as usize;
                if i < n {
                    Step::Close(Outcome::Chose(i))
                } else {
                    Step::Same
                }
            }
            KeyCode::Esc | KeyCode::Char('q') if plain => Step::Close(Outcome::Dismissed),
            KeyCode::Char('c') if ctrl => Step::Close(Outcome::Dismissed),
            _ => Step::Same,
        }
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        let Some(drawn) = &self.drawn else {
            return Step::Same;
        };
        let hit = drawn.hit(mouse.col, mouse.row);
        let option = match hit {
            Hit::Line(i, _) if i >= self.first_line => {
                Some(self.first_shown + i - self.first_line).filter(|o| *o < self.options.len())
            }
            _ => None,
        };
        let n = self.options.len();
        match draw::gesture(&mouse) {
            Gesture::Click if look.mouse => match (hit, option) {
                (_, Some(o)) => Step::Close(Outcome::Chose(o)),
                (Hit::Close | Hit::Outside, _) => Step::Close(Outcome::Dismissed),
                _ => Step::Same,
            },
            // The wheel too only with menu_mouse on, as the daemon left
            // the mouse alone over a box with it off.
            Gesture::WheelDown if n > 0 && look.mouse => {
                self.cursor = (self.cursor + 1).min(n - 1);
                Step::Redraw
            }
            Gesture::WheelUp if look.mouse => {
                self.cursor = self.cursor.min(n).saturating_sub(1);
                Step::Redraw
            }
            Gesture::Move if look.hover => {
                let over_close = hit == Hit::Close;
                let mut changed = over_close != self.close_lit;
                self.close_lit = over_close;
                if let Some(o) = option
                    && o != self.cursor
                {
                    self.cursor = o;
                    changed = true;
                }
                if changed { Step::Redraw } else { Step::Same }
            }
            _ => Step::Same,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::input::{MouseButton, MouseKind};
    use crate::ui::draw::tests::screen;
    use ratatui::layout::Rect;

    fn key(code: KeyCode) -> Key {
        Key::new(code, Mods::NONE)
    }

    fn menu() -> Menu {
        Menu::new(
            "What now?",
            vec![
                MenuOption::new("Open it"),
                MenuOption::new("Copy its link").key('c'),
                MenuOption::new("Leave it"),
            ],
        )
    }

    #[test]
    fn keys_move_and_choose() {
        let look = Look::default();
        let mut m = menu();
        assert_eq!(m.key(key(KeyCode::Char('j')), &look), Step::Redraw);
        assert_eq!(m.key(key(KeyCode::Down), &look), Step::Redraw);
        assert_eq!(
            m.key(key(KeyCode::Down), &look),
            Step::Same,
            "stops at the end"
        );
        assert_eq!(
            m.key(key(KeyCode::Enter), &look),
            Step::Close(Outcome::Chose(2))
        );
        let mut m = menu();
        assert_eq!(
            m.key(key(KeyCode::Char('2')), &look),
            Step::Close(Outcome::Chose(1))
        );
        let mut m = menu();
        assert_eq!(
            m.key(key(KeyCode::Char('c')), &look),
            Step::Close(Outcome::Chose(1))
        );
        let mut m = menu();
        assert_eq!(
            m.key(key(KeyCode::Esc), &look),
            Step::Close(Outcome::Dismissed)
        );
        let mut m = menu();
        m.key(key(KeyCode::BackTab), &look);
        assert_eq!(m.cursor(), 2, "Shift-Tab goes round to the last");
        m.key(key(KeyCode::Tab), &look);
        assert_eq!(m.cursor(), 0);
    }

    #[test]
    fn an_options_own_key_never_takes_one_the_menu_moves_with_nor_the_wheel_without_clicks() {
        let look = Look::default();
        let mut m = Menu::new(
            "Event",
            vec![
                MenuOption::new("Open it").key('o'),
                MenuOption::new("Join its call").key('j'),
                MenuOption::new("Read AB-1 here").key('1'),
            ],
        );
        assert_eq!(m.key(key(KeyCode::Char('j')), &look), Step::Redraw);
        assert_eq!(m.cursor(), 1, "j moves rather than joining");
        assert_eq!(
            m.key(key(KeyCode::Char('1')), &look),
            Step::Close(Outcome::Chose(0)),
            "a number is the option's place"
        );
        let off = Look {
            mouse: false,
            ..Look::default()
        };
        let mut m = menu();
        let wheel = Mouse {
            kind: MouseKind::Press,
            button: MouseButton::WheelDown,
            col: 0,
            row: 0,
            mods: Mods::NONE,
        };
        m.draw(&mut Buffer::empty(Rect::new(0, 0, 80, 20)), &off);
        assert_eq!(m.mouse(wheel, &off), Step::Same);
        assert_eq!(m.mouse(wheel, &look), Step::Redraw);
    }

    #[test]
    fn a_wide_screens_menu_is_at_most_72_columns_its_long_option_cut() {
        let look = Look::default();
        let long = "Skip the next item: ".to_string() + &"a very long item ".repeat(12);
        let mut m = Menu::new(
            "Working Work",
            vec![
                MenuOption::new("Pause").key('p'),
                MenuOption::new(long).key('s'),
            ],
        );
        let mut buf = Buffer::empty(Rect::new(0, 0, 200, 30));
        m.draw(&mut buf, &look);
        let d = m.drawn.clone().unwrap();
        assert_eq!(d.outer.width, 72);
        let s = screen(&buf);
        let row = s.iter().find(|l| l.contains("Skip the next")).unwrap();
        assert!(row.contains("…  s  │"), "{row}");
        // Two thirds of a narrower screen, never narrower than its title.
        let mut buf = Buffer::empty(Rect::new(0, 0, 90, 30));
        m.draw(&mut buf, &look);
        assert_eq!(m.drawn.clone().unwrap().outer.width, 60);
        let mut titled = Menu::new("T".repeat(70), vec![MenuOption::new("x".repeat(200))]);
        titled.draw(&mut buf, &look);
        assert_eq!(titled.drawn.clone().unwrap().outer.width, 78);
    }

    #[test]
    fn easy_modes_close_button_is_a_tab_stop() {
        let look = Look {
            close: true,
            ..Look::default()
        };
        let mut m = menu().at(2);
        m.key(key(KeyCode::Tab), &look);
        assert_eq!(m.cursor(), 3);
        assert_eq!(
            m.key(key(KeyCode::Enter), &look),
            Step::Close(Outcome::Dismissed)
        );
    }

    #[test]
    fn it_draws_the_chosen_option_on_the_highlight_and_takes_clicks() {
        let look = Look::default();
        let mut m = menu().body("A tab with a PR.");
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 20));
        m.draw(&mut buf, &look);
        let s = screen(&buf);
        assert!(s.iter().any(|l| l.contains("What now?")));
        assert!(s.iter().any(|l| l.contains(" A tab with a PR.")));
        let row = s.iter().position(|l| l.contains("› Open it")).unwrap() as u16;
        let d = m.drawn.clone().unwrap();
        assert_eq!(buf[(d.inner.right() - 1, row)].bg, look.theme.highlight);
        assert!(s[row as usize + 1].contains("Copy its link"));
        let at = |kind, button, col, row| Mouse {
            kind,
            button,
            col,
            row,
            mods: Mods::NONE,
        };
        assert_eq!(
            m.mouse(
                at(MouseKind::Move, MouseButton::None, d.inner.x + 4, row + 2),
                &look
            ),
            Step::Redraw
        );
        assert_eq!(m.cursor(), 2, "the mouse moves the cursor");
        assert_eq!(
            m.mouse(
                at(MouseKind::Press, MouseButton::Left, d.inner.x + 4, row + 1),
                &look
            ),
            Step::Close(Outcome::Chose(1))
        );
        assert_eq!(
            m.mouse(at(MouseKind::Press, MouseButton::Left, 0, 0), &look),
            Step::Close(Outcome::Dismissed)
        );
    }
}
