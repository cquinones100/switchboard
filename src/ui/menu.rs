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
//! `/` searches it (`Filter`, which the boxes with a list of their own
//! share): a line under the text, `›` and what is typed, and only the
//! options whose label holds every word typed, best first by
//! `palette::rank` and in their own order among equals. While it is open
//! every printable key types into it, the arrows (and Ctrl-N, Ctrl-P) move,
//! Enter takes the chosen match, Backspace on the last character and Esc
//! show every option again, and a second Esc goes back. A menu of more than
//! five options says `/ search` among its keys.
//!
//! Text to read above the options (`body`) is wrapped to the box and cut
//! short with … when the options need the room. The box is at most 72
//! columns or two thirds of the screen, whichever is less, but never
//! narrower than its title: an option longer than that is cut with ….

use ratatui::buffer::Buffer;

use super::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use super::{Look, Outcome, Overlay, Step};
use crate::mux::input::{Key, KeyCode, Mods, Mouse};

/// The search a box with a list opens with `/`: what is typed, and which
/// of the list's labels it matches.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    open: bool,
    typed: String,
}

/// What a key did to a `Filter`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filtered {
    /// It opened, typed or took a character off: the list shown changed.
    Changed,
    /// It closed (Esc, or Backspace on its last character): every option
    /// shows again.
    Closed,
    /// It isn't the filter's: the box handles it.
    Not,
}

impl Filter {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn typed(&self) -> &str {
        &self.typed
    }

    /// A key: `/` opens it; while it is open, printable keys type, Ctrl-U
    /// clears, Backspace takes a character off and Esc closes it.
    pub fn key(&mut self, key: Key) -> Filtered {
        let k = key.normalized();
        if !self.open {
            if k.mods == Mods::NONE && k.code == KeyCode::Char('/') {
                self.open = true;
                return Filtered::Changed;
            }
            return Filtered::Not;
        }
        match k.code {
            KeyCode::Esc => {
                *self = Filter::default();
                Filtered::Closed
            }
            KeyCode::Backspace => {
                self.typed.pop();
                if self.typed.is_empty() {
                    *self = Filter::default();
                    Filtered::Closed
                } else {
                    Filtered::Changed
                }
            }
            KeyCode::Char('u') if k.mods == Mods::CTRL => {
                self.typed.clear();
                Filtered::Changed
            }
            KeyCode::Char(c) if !k.mods.ctrl && !k.mods.alt => {
                self.typed.push(c);
                Filtered::Changed
            }
            _ => Filtered::Not,
        }
    }

    /// Text pasted while it is open types into it.
    pub fn paste(&mut self, text: &str) -> bool {
        if !self.open {
            return false;
        }
        // Words pasted on lines of their own stay words apart.
        self.typed.extend(
            text.chars()
                .map(|c| if c.is_whitespace() { ' ' } else { c })
                .filter(|c| !c.is_control()),
        );
        true
    }

    /// Which of `labels` show, by their index: every one while nothing is
    /// typed, else those holding every word typed, best first, in their own
    /// order among equals.
    pub fn shown<S: AsRef<str>>(&self, labels: &[S]) -> Vec<usize> {
        let typed = crate::ui::palette::normalize(&self.typed);
        if typed.is_empty() {
            return (0..labels.len()).collect();
        }
        let terms: Vec<&str> = typed.split(' ').collect();
        let matching: Vec<usize> = (0..labels.len())
            .filter(|i| {
                let label = labels[*i].as_ref().to_lowercase();
                terms.iter().all(|t| label.contains(t))
            })
            .collect();
        let rows: Vec<(String, String)> = matching
            .iter()
            .map(|i| (i.to_string(), labels[*i].as_ref().to_string()))
            .collect();
        crate::ui::palette::rank(&typed, &rows, &[])
            .into_iter()
            .map(|r| matching[r])
            .collect()
    }

    /// Its line, `cols` wide (a box's inside less its margins): `›`, what is typed and the cursor, and
    /// how many match of how many there are at its right.
    pub fn line(&self, cols: usize, matched: usize, of: usize, look: &Look) -> BoxLine {
        BoxLine::spans(super::kit::input_beside(
            &self.typed,
            "type to search",
            cols,
            vec![ratatui::text::Span::styled(
                format!("{matched}/{of}"),
                super::kit::Tone::Dim.style(&look.theme),
            )],
            &look.theme,
            false,
        ))
    }
}

/// What a box shows in place of its options when none matches the search.
pub const NO_MATCH: &str = " No option matches";

/// The keys a list's bottom row shows while its search is open.
pub const SEARCH_KEYS: &str = "↑↓ move  ↵ take  esc all";

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
    /// The search `/` opens, and the options it shows, by their index.
    filter: Filter,
    shown: Vec<usize>,
    /// The chosen one of those shown; one past the last is the close button
    /// (Tab).
    cursor: usize,
    close_lit: bool,
    drawn: Option<Drawn>,
    /// Where it opens: the middle, or beside the click that opened it.
    place: draw::Place,
    /// Which of the box's lines is the first option, and the first option
    /// shown on it.
    first_line: usize,
    first_shown: usize,
    /// How wide its inside was drawn with the search closed, which it
    /// keeps while searching.
    closed_width: Option<usize>,
}

impl Menu {
    pub fn new(title: impl Into<String>, options: Vec<MenuOption>) -> Menu {
        Menu {
            title: title.into(),
            body: String::new(),
            shown: (0..options.len()).collect(),
            options,
            filter: Filter::default(),
            cursor: 0,
            close_lit: false,
            drawn: None,
            place: draw::Place::Center,
            first_line: 0,
            first_shown: 0,
            closed_width: None,
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

    /// The chosen option, by its index among them all (one past the last
    /// is the close button).
    pub fn cursor(&self) -> usize {
        self.shown
            .get(self.cursor)
            .copied()
            .unwrap_or(self.options.len())
    }

    fn take(&self) -> Step {
        match self.shown.get(self.cursor) {
            Some(i) => Step::Close(Outcome::Chose(*i)),
            None if self.cursor == self.shown.len() && !self.filter.is_open() => {
                Step::Close(Outcome::Dismissed)
            }
            None => Step::Same,
        }
    }

    fn stops(&self, look: &Look) -> usize {
        self.shown.len() + usize::from(look.close)
    }

    fn refilter(&mut self) {
        let labels: Vec<&str> = self.options.iter().map(|o| o.label.as_str()).collect();
        self.shown = self.filter.shown(&labels);
        self.cursor = 0;
    }

    fn keys(&self) -> &'static str {
        if self.filter.is_open() {
            SEARCH_KEYS
        } else {
            self.closed_keys()
        }
    }

    /// Its keys with the search closed, which its width follows either
    /// way, so the search opening never wraps its text again.
    fn closed_keys(&self) -> &'static str {
        if self.options.len() > 5 {
            KEYS_SEARCH
        } else {
            KEYS
        }
    }
}

/// The keys a menu takes, on its bottom row.
pub(crate) const KEYS: &str = "jk or tab move  ↵ or click choose  esc back";
/// A long menu's, which says it searches.
const KEYS_SEARCH: &str = "jk or tab move  ↵ or click choose  / search  esc back";

/// The widest a menu's inside is on a screen `cols` wide: its box 72
/// columns or two thirds of the screen, whichever is less.
fn widest(cols: usize) -> usize {
    (cols * 2 / 3).min(72).saturating_sub(2)
}

/// The widest an option's label (one with no key of its own) is drawn on
/// a screen `cols` wide, before it is cut.
pub fn label_room(cols: usize) -> usize {
    widest(cols).saturating_sub(5)
}

impl Overlay for Menu {
    fn drawn(&self) -> Option<&crate::ui::draw::Drawn> {
        self.drawn.as_ref()
    }

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
            .max(draw::width(self.closed_keys()) + 3)
            .min((area.width as usize).saturating_sub(2));
        let searching = self.filter.is_open();
        let search_w = self.closed_width.unwrap_or(inner);
        let room = (area.height as usize).saturating_sub(6).max(1);
        let n = self.shown.len();
        // The options come first, every one shown, or those around the
        // chosen one; the text takes what room is left with the search
        // closed. The search's line comes out of the options' room, never
        // the text's, so a request whose answers a menu offers (which it
        // offers only with the request whole above them, among a tab's many
        // choices) stays whole; only a menu of one or two options under a
        // long text gives the text's last rows up, so a match still shows.
        let closed_room = room.saturating_sub(self.options.len().min(room) + 1);
        let body_room = if searching {
            closed_room.min(room.saturating_sub(4))
        } else {
            closed_room
        };
        let said = if self.body.trim().is_empty() {
            Vec::new()
        } else {
            draw::wrap(self.body.trim(), inner.saturating_sub(2))
        };
        let shown: Vec<String> = if body_room < 1 {
            Vec::new()
        } else if said.len() > body_room {
            let mut s = said[..body_room - 1].to_vec();
            s.push("…".into());
            s
        } else {
            said
        };
        let fits = if searching {
            let taken = shown.len() + usize::from(!shown.is_empty()) + 2;
            n.min(room.saturating_sub(taken).max(1))
        } else {
            n.min(room)
        };
        let first = self
            .cursor
            .min(n.saturating_sub(1))
            .saturating_sub(fits / 2)
            .min(n.saturating_sub(fits));
        let mut lines: Vec<BoxLine> = shown
            .iter()
            .map(|l| BoxLine::text(format!(" {l}"), look.text()))
            .collect();
        if !lines.is_empty() {
            lines.push(BoxLine::blank());
        }
        if searching {
            lines.push(
                self.filter
                    .line(search_w.saturating_sub(2), n, self.options.len(), look),
            );
            lines.push(BoxLine::blank());
            if n == 0 {
                lines.push(BoxLine::text(NO_MATCH, look.dim()));
            }
        }
        self.first_line = lines.len();
        self.first_shown = first;
        // A row is `   label` and, two columns past the longest label, its
        // key, inside the box's margin of a column each side.
        let keyed = self.options.iter().any(|o| o.key.is_some());
        let label_room = if keyed {
            inner.saturating_sub(8)
        } else {
            inner.saturating_sub(5)
        };
        let labels: Vec<String> = self
            .options
            .iter()
            .map(|o| draw::cut(&o.label, label_room))
            .collect();
        let key_col = labels.iter().map(|l| draw::width(l)).max().unwrap_or(0) + 5;
        for (at, i) in self.shown.iter().enumerate().skip(first).take(fits) {
            let chosen = at == self.cursor;
            let spans = super::kit::option(
                &labels[*i],
                self.options[*i].key,
                key_col,
                chosen,
                &look.theme,
            );
            lines.push(
                BoxLine::spans(spans)
                    .fill(chosen.then_some(look.theme.highlight))
                    .clickable(),
            );
        }
        let mut spec = BoxSpec::new(&self.title, &lines);
        spec.keys = Some(self.keys());
        if searching {
            spec.width = search_w as u16;
        } else if !shown.is_empty() {
            spec.width = inner as u16;
        }
        spec.close_lit = self.cursor == n || self.close_lit;
        spec.place = self.place;
        let drawn = draw::draw_box(buf, &spec, look);
        if !searching {
            self.closed_width = Some(drawn.inner.width as usize);
        }
        // Its window follows the cursor, so its scroll bar only shows where
        // it is.
        draw::edge_bar(buf, &drawn, (self.first_line, fits), (n, first), look);
        self.drawn = Some(drawn);
    }

    fn open_near(&mut self, (x, y): (u16, u16)) {
        self.place = draw::Place::Near { x, y };
    }

    fn key(&mut self, key: Key, look: &Look) -> Step {
        if self.options.is_empty() {
            return Step::Close(Outcome::Dismissed);
        }
        match self.filter.key(key) {
            Filtered::Changed | Filtered::Closed => {
                self.refilter();
                return Step::Redraw;
            }
            Filtered::Not => {}
        }
        let n = self.shown.len();
        if self.filter.is_open() {
            let ctrl = key.mods == Mods::CTRL;
            let to = match key.code {
                KeyCode::Down | KeyCode::Tab => self.cursor + 1,
                KeyCode::Char('n') if ctrl => self.cursor + 1,
                KeyCode::Up | KeyCode::BackTab => self.cursor.saturating_sub(1),
                KeyCode::Char('p') if ctrl => self.cursor.saturating_sub(1),
                KeyCode::Enter => return self.take(),
                KeyCode::Char('c') if ctrl => return Step::Close(Outcome::Dismissed),
                _ => return Step::Same,
            }
            .min(n.saturating_sub(1));
            if to == self.cursor {
                return Step::Same;
            }
            self.cursor = to;
            return Step::Redraw;
        }
        // An option's own key, but never one the menu moves or closes with.
        if key.mods == Mods::NONE
            && let KeyCode::Char(c) = key.code
            && !"jkgGq /".contains(c)
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

    fn paste(&mut self, text: &str, _look: &Look) -> Step {
        if self.filter.paste(text) {
            self.refilter();
            Step::Redraw
        } else {
            Step::Same
        }
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        let Some(drawn) = &self.drawn else {
            return Step::Same;
        };
        let hit = drawn.hit(mouse.col, mouse.row);
        let option = match hit {
            Hit::Line(i, _) if i >= self.first_line => {
                Some(self.first_shown + i - self.first_line).filter(|o| *o < self.shown.len())
            }
            _ => None,
        };
        let n = self.shown.len();
        match draw::gesture(&mouse) {
            Gesture::Click if look.mouse => match (hit, option) {
                (_, Some(o)) => Step::Close(Outcome::Chose(self.shown[o])),
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

    fn typed(m: &mut Menu, text: &str, look: &Look) {
        for c in text.chars() {
            m.key(key(KeyCode::Char(c)), look);
        }
    }

    fn long() -> Menu {
        Menu::new(
            "Tab",
            vec![
                MenuOption::new("Open its PR").key('o'),
                MenuOption::new("Copy its branch").key('c'),
                MenuOption::new("Land it").key('l'),
                MenuOption::new("Rename it"),
                MenuOption::new("Close it"),
                MenuOption::new("Plan the landing"),
            ],
        )
    }

    #[test]
    fn slash_searches_the_options_and_enter_takes_the_chosen_match() {
        let look = Look::default();
        let mut m = long();
        assert_eq!(m.key(key(KeyCode::Char('/')), &look), Step::Redraw);
        typed(&mut m, "lan", &look);
        assert_eq!(m.shown, vec![2, 5], "only those matching, best first");
        assert_eq!(m.cursor(), 2, "the cursor on the first match");
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
        m.draw(&mut buf, &look);
        let s = screen(&buf);
        assert!(s.iter().any(|l| l.contains("› lan▏")), "{s:#?}");
        assert!(s.iter().any(|l| l.contains("2/6")));
        assert!(!s.iter().any(|l| l.contains("Rename it")));
        assert!(s.iter().any(|l| l.contains("esc all")));
        m.key(key(KeyCode::Down), &look);
        assert_eq!(
            m.key(key(KeyCode::Enter), &look),
            Step::Close(Outcome::Chose(5))
        );
    }

    #[test]
    fn while_searching_numbers_and_an_options_key_type() {
        let look = Look::default();
        let mut m = long();
        m.key(key(KeyCode::Char('/')), &look);
        assert_eq!(m.key(key(KeyCode::Char('2')), &look), Step::Redraw);
        assert_eq!(m.key(key(KeyCode::Char('c')), &look), Step::Redraw);
        assert_eq!(m.filter.typed(), "2c");
        let mut m = long();
        m.key(key(KeyCode::Char('/')), &look);
        typed(&mut m, "co", &look);
        assert_eq!(m.shown, vec![1], "c typed rather than taking Copy");
    }

    #[test]
    fn backspace_to_empty_and_esc_show_every_option_and_a_second_esc_goes_back() {
        let look = Look::default();
        let mut m = long();
        m.key(key(KeyCode::Char('/')), &look);
        typed(&mut m, "x", &look);
        m.key(key(KeyCode::Backspace), &look);
        assert!(!m.filter.is_open());
        assert_eq!(m.shown.len(), 6);
        m.key(key(KeyCode::Char('/')), &look);
        typed(&mut m, "land", &look);
        assert_eq!(m.key(key(KeyCode::Esc), &look), Step::Redraw);
        assert_eq!(m.shown.len(), 6, "every option again");
        assert_eq!(
            m.key(key(KeyCode::Esc), &look),
            Step::Close(Outcome::Dismissed)
        );
    }

    #[test]
    fn with_nothing_matching_it_says_so_and_enter_does_nothing() {
        let look = Look::default();
        let mut m = long();
        m.key(key(KeyCode::Char('/')), &look);
        typed(&mut m, "zzz", &look);
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
        m.draw(&mut buf, &look);
        assert!(screen(&buf).iter().any(|l| l.contains("No option matches")));
        assert_eq!(m.key(key(KeyCode::Enter), &look), Step::Same);
    }

    #[test]
    fn searching_keeps_the_whole_text_above_the_options() {
        let look = Look::default();
        let options = (0..15)
            .map(|i| MenuOption::new(format!("Option {i}")))
            .collect();
        let mut m = Menu::new("Tab", options).body("Bash\ngit push");
        m.key(key(KeyCode::Char('/')), &look);
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
        m.draw(&mut buf, &look);
        let s = screen(&buf);
        assert!(s.iter().any(|l| l.contains(" Bash")), "{s:#?}");
        assert!(s.iter().any(|l| l.contains(" git push")), "{s:#?}");
        assert!(s.iter().any(|l| l.contains("› Option 0")), "{s:#?}");
        let closed = Menu::new(
            "Tab",
            (0..15).map(|i| MenuOption::new(format!("{i}"))).collect(),
        )
        .body("Bash");
        let mut open = Menu::new(
            "Tab",
            (0..15).map(|i| MenuOption::new(format!("{i}"))).collect(),
        )
        .body("Bash");
        open.key(key(KeyCode::Char('/')), &look);
        let mut closed = closed;
        closed.draw(&mut buf, &look);
        open.draw(&mut buf, &look);
        assert_eq!(
            closed.drawn.unwrap().inner.width,
            open.drawn.unwrap().inner.width,
            "as wide searching, so the text wraps the same"
        );
        let mut plain = Menu::of("Actions", &["Open", "Close"]);
        let mut buf = Buffer::empty(Rect::new(0, 0, 120, 24));
        plain.draw(&mut buf, &look);
        let closed = plain.drawn.clone().unwrap().inner.width;
        plain.key(key(KeyCode::Char('/')), &look);
        plain.draw(&mut buf, &look);
        assert_eq!(plain.drawn.unwrap().inner.width, closed, "with no text too");
    }

    #[test]
    fn a_long_text_leaves_a_searched_match_in_view() {
        let look = Look::default();
        let body = (0..30)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut m = Menu::of("Title", &["Copy it", "Close"]).body(body);
        m.key(key(KeyCode::Char('/')), &look);
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
        m.draw(&mut buf, &look);
        let s = screen(&buf);
        assert!(s.iter().any(|l| l.contains("› Copy it")), "{s:#?}");
    }

    #[test]
    fn a_paste_on_two_lines_searches_two_words() {
        let look = Look::default();
        let mut m = long();
        m.key(key(KeyCode::Char('/')), &look);
        m.paste("plan\nlanding", &look);
        assert_eq!(m.shown, vec![5]);
    }

    #[test]
    fn a_long_menu_says_it_searches() {
        let look = Look::default();
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
        long().draw(&mut buf, &look);
        assert!(screen(&buf).iter().any(|l| l.contains("/ search")));
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
        menu().draw(&mut buf, &look);
        assert!(!screen(&buf).iter().any(|l| l.contains("/ search")));
    }

    #[test]
    fn a_click_while_searching_takes_the_match_under_it() {
        let look = Look::default();
        let mut m = long();
        m.key(key(KeyCode::Char('/')), &look);
        typed(&mut m, "plan", &look);
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
        m.draw(&mut buf, &look);
        let s = screen(&buf);
        let row = s
            .iter()
            .position(|l| l.contains("Plan the landing"))
            .unwrap() as u16;
        let d = m.drawn.clone().unwrap();
        let click = Mouse {
            kind: MouseKind::Press,
            button: MouseButton::Left,
            col: d.inner.x + 4,
            row,
            mods: Mods::NONE,
        };
        assert_eq!(m.mouse(click, &look), Step::Close(Outcome::Chose(5)));
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
