//! The Format menu, Ctrl-P in a note: everything a note can do, each with
//! its key, under its heading (init.lua's `format_items` and `menu`). A
//! key, Enter or a click on a line picks it; Escape or Q closes it. Opened
//! from Visual mode, it acts on the selection. A paragraph or character
//! style leaves you typing, as `a` would, unless it already has: you pick a
//! style to write in it. The toolbar's buttons (easy.rs) and the palette
//! (palette.rs) run the same items.
//!
//! It is drawn as every box is (src/ui/draw.rs), and is an overlay of the
//! ui module's, so the app stacks it with the palette and the questions.

use ratatui::buffer::Buffer;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use super::format::{Character, Paragraph};
use crate::mux::input::{Key, KeyCode, Mouse};
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use crate::ui::{Look, Outcome, Overlay, Step};

/// What an item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Paragraph(Paragraph),
    Monospaced,
    Character(Character),
    Link,
    Tick,
    SortChecked,
    UncheckAll,
    DeleteChecked,
    /// Start work on the item in a new tab (`gw`), from Switchboard.
    WorkOnItem,
    PasteImage,
    Table,
    AddRow,
    AddColumn,
    DeleteRow,
    DeleteColumn,
}

impl Act {
    /// A style picked to write in starts typing after it.
    pub fn typing(self) -> bool {
        matches!(
            self,
            Act::Paragraph(_) | Act::Monospaced | Act::Character(_)
        )
    }

    /// A style, which a note's leader keys (Space, then the item's key)
    /// put on the word, the line or the selection, as the menu does.
    pub fn leader(self) -> bool {
        matches!(
            self,
            Act::Paragraph(_) | Act::Monospaced | Act::Character(_) | Act::Link | Act::Table
        )
    }
}

/// The item a note's leader key takes: Space, then the menu's key of one
/// of its styles.
pub fn leader(key: char) -> Option<Item> {
    leader_items()
        .into_iter()
        .find(|i| i.key.chars().eq(std::iter::once(key)))
}

/// The items the leader keys take, in the menu's order, for the leader and
/// mission control's Keys view.
pub fn leader_items() -> Vec<Item> {
    items()
        .into_iter()
        .map(|(_, i)| i)
        .filter(|i| i.act.leader())
        .collect()
}

/// An item: its key, its label, what it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Item {
    pub key: &'static str,
    pub label: &'static str,
    pub act: Act,
}

const fn item(key: &'static str, label: &'static str, act: Act) -> Item {
    Item { key, label, act }
}

/// The menu's headings and their items, in its order.
pub const SECTIONS: &[(&str, &[Item])] = &[
    (
        "Paragraph",
        &[
            item("1", "Title", Act::Paragraph(Paragraph::Title)),
            item("2", "Heading", Act::Paragraph(Paragraph::Heading)),
            item("3", "Subheading", Act::Paragraph(Paragraph::Subheading)),
            item("0", "Body", Act::Paragraph(Paragraph::Body)),
            item("m", "Monospaced", Act::Monospaced),
            item("8", "Bulleted List", Act::Paragraph(Paragraph::Bullet)),
            item("-", "Dashed List", Act::Paragraph(Paragraph::Dash)),
            item("7", "Numbered List", Act::Paragraph(Paragraph::Number)),
            item("q", "Block Quote", Act::Paragraph(Paragraph::Quote)),
        ],
    ),
    (
        "Character",
        &[
            item("b", "Bold", Act::Character(Character::Bold)),
            item("i", "Italic", Act::Character(Character::Italic)),
            item("U", "Underline", Act::Character(Character::Underline)),
            item("x", "Strikethrough", Act::Character(Character::Strike)),
            item("h", "Highlight", Act::Character(Character::Highlight)),
            item("`", "Monospaced text", Act::Character(Character::Code)),
            item("k", "Link…", Act::Link),
        ],
    ),
    (
        "Checklist",
        &[
            item("l", "Checklist", Act::Paragraph(Paragraph::Check)),
            item("c", "Tick or untick", Act::Tick),
            item("s", "Move checked to bottom", Act::SortChecked),
            item("u", "Uncheck all", Act::UncheckAll),
            item("D", "Delete checked items", Act::DeleteChecked),
            item("w", "Start work on the item in a new tab", Act::WorkOnItem),
        ],
    ),
    ("Image", &[item("v", "Paste image", Act::PasteImage)]),
    (
        "Table",
        &[
            item("t", "Table", Act::Table),
            item("r", "Add row", Act::AddRow),
            item("C", "Add column", Act::AddColumn),
            item("R", "Delete row", Act::DeleteRow),
            item("X", "Delete column", Act::DeleteColumn),
        ],
    ),
];

/// Every item, in the menu's order, each with its heading.
pub fn items() -> Vec<(&'static str, Item)> {
    SECTIONS
        .iter()
        .flat_map(|(heading, items)| items.iter().map(move |i| (*heading, *i)))
        .collect()
}

/// The item a toolbar button names, by its label.
pub fn by_label(label: &str) -> Option<Item> {
    items()
        .into_iter()
        .map(|(_, i)| i)
        .find(|i| i.label == label)
}

/// The menu: picking gives `Outcome::Chose` with the item's place in
/// `items()`.
#[derive(Default)]
pub struct FormatMenu {
    cursor: usize,
    hover: Option<usize>,
    close_lit: bool,
    drawn: Option<Drawn>,
    /// Each line's item, by its index in the box's lines.
    rows: Vec<Option<usize>>,
    /// The first line shown, when the screen is too short for them all.
    top: usize,
}

impl Overlay for FormatMenu {
    fn kind(&self) -> &'static str {
        "format-menu"
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let mut lines = Vec::new();
        self.rows.clear();
        let mut n = 0;
        for (heading, items) in SECTIONS {
            lines.push(BoxLine::text(
                format!(" {heading}"),
                look.bold_bright().add_modifier(Modifier::UNDERLINED),
            ));
            self.rows.push(None);
            for item in *items {
                let chosen = n == self.cursor;
                let fill = if chosen {
                    Some(look.theme.highlight)
                } else if self.hover == Some(n) {
                    Some(look.theme.panel)
                } else {
                    None
                };
                let mut spans = if chosen {
                    vec![Span::raw(" "), look.pointer(), Span::raw(" ")]
                } else {
                    vec![Span::raw("   ")]
                };
                spans.push(Span::styled(format!("{:<3} ", item.key), look.dim()));
                spans.push(Span::styled(
                    item.label.to_string(),
                    if chosen {
                        look.bold_bright()
                    } else {
                        look.text()
                    },
                ));
                lines.push(BoxLine::spans(spans).fill(fill).clickable());
                self.rows.push(Some(n));
                n += 1;
            }
        }
        // Too many for the screen: the lines around the cursor's, its
        // heading with it.
        let room = (buf.area.height as usize).saturating_sub(6).max(1);
        if lines.len() > room {
            let at = self
                .rows
                .iter()
                .position(|r| *r == Some(self.cursor))
                .unwrap_or(0);
            let heading = at.saturating_sub(usize::from(at > 0 && self.rows[at - 1].is_none()));
            if heading < self.top {
                self.top = heading;
            } else if at >= self.top + room {
                self.top = at + 1 - room;
            }
            self.top = self.top.min(lines.len() - room);
            lines = lines.split_off(self.top);
            lines.truncate(room);
            self.rows = self.rows.split_off(self.top);
            self.rows.truncate(room);
        } else {
            self.top = 0;
        }
        let mut spec = BoxSpec::new("Format", &lines);
        spec.keys = Some("a key or ↵ picks  ↑↓ move  esc or Q closes");
        spec.width = 30;
        spec.close_lit = self.close_lit;
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn key(&mut self, key: Key, _look: &Look) -> Step {
        let total = items().len();
        let plain = !key.mods.ctrl && !key.mods.alt;
        match key.code {
            KeyCode::Esc => Step::Close(Outcome::Dismissed),
            KeyCode::Enter => Step::Close(Outcome::Chose(self.cursor)),
            KeyCode::Down => {
                self.cursor = (self.cursor + 1) % total;
                Step::Redraw
            }
            KeyCode::Up => {
                self.cursor = (self.cursor + total - 1) % total;
                Step::Redraw
            }
            KeyCode::Char('n') if key.mods.ctrl => {
                self.cursor = (self.cursor + 1) % total;
                Step::Redraw
            }
            KeyCode::Char('p') if key.mods.ctrl => {
                self.cursor = (self.cursor + total - 1) % total;
                Step::Redraw
            }
            KeyCode::Char('Q') if plain => Step::Close(Outcome::Dismissed),
            KeyCode::Char(c) if plain => {
                let typed = c.to_string();
                match items().iter().position(|(_, i)| i.key == typed) {
                    Some(at) => Step::Close(Outcome::Chose(at)),
                    None if c == 'j' => {
                        self.cursor = (self.cursor + 1) % total;
                        Step::Redraw
                    }
                    None if c == 'k' => {
                        self.cursor = (self.cursor + total - 1) % total;
                        Step::Redraw
                    }
                    None => Step::Same,
                }
            }
            _ => Step::Same,
        }
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        let Some(drawn) = &self.drawn else {
            return Step::Same;
        };
        let hit = drawn.hit(mouse.col, mouse.row);
        let row = match hit {
            Hit::Line(i, _) => self.rows.get(i).copied().flatten(),
            _ => None,
        };
        match draw::gesture(&mouse) {
            Gesture::Click if look.mouse => match (hit, row) {
                (Hit::Outside | Hit::Close, _) => Step::Close(Outcome::Dismissed),
                (_, Some(at)) => Step::Close(Outcome::Chose(at)),
                _ => Step::Same,
            },
            Gesture::WheelDown => {
                self.cursor = (self.cursor + 1).min(items().len() - 1);
                Step::Redraw
            }
            Gesture::WheelUp => {
                self.cursor = self.cursor.saturating_sub(1);
                Step::Redraw
            }
            Gesture::Move if look.hover => {
                let close = hit == Hit::Close;
                if row == self.hover && close == self.close_lit {
                    return Step::Same;
                }
                self.hover = row;
                self.close_lit = close;
                Step::Redraw
            }
            _ => Step::Same,
        }
    }
}

/// A box asking for a line of text (a folder's name, a search, a link's
/// address), as `vim.ui.input` asked: Enter gives what is typed, Esc
/// nothing.
pub struct Ask {
    title: String,
    prompt: String,
    text: String,
    drawn: Option<Drawn>,
    close_lit: bool,
}

impl Ask {
    pub fn new(title: &str, prompt: &str, text: &str) -> Ask {
        Ask {
            title: title.into(),
            prompt: prompt.into(),
            text: text.into(),
            drawn: None,
            close_lit: false,
        }
    }
}

impl Overlay for Ask {
    fn kind(&self) -> &'static str {
        "ask"
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let lines = vec![
            BoxLine::blank(),
            BoxLine::spans(vec![
                Span::styled(format!(" {} ", self.prompt), Style::new().fg(draw::EDGE)),
                Span::styled(self.text.clone(), look.bright()),
                Span::styled("▏", look.dim()),
            ]),
        ];
        let mut spec = BoxSpec::new(&self.title, &lines);
        spec.keys = Some("↵ ok  esc cancel");
        spec.width = 50;
        spec.close_lit = self.close_lit;
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn key(&mut self, key: Key, _look: &Look) -> Step {
        match key.code {
            KeyCode::Esc => Step::Close(Outcome::Dismissed),
            KeyCode::Enter => Step::Close(Outcome::Picked(crate::ui::Picked {
                index: 0,
                value: self.text.clone(),
                query: self.text.clone(),
            })),
            KeyCode::Backspace => {
                self.text.pop();
                Step::Redraw
            }
            KeyCode::Char('u') if key.mods.ctrl => {
                self.text.clear();
                Step::Redraw
            }
            KeyCode::Char('w') if key.mods.ctrl => {
                let kept = self.text.trim_end().rfind(' ').map_or(0, |at| at + 1);
                self.text.truncate(kept);
                Step::Redraw
            }
            KeyCode::Char(c) if !key.mods.ctrl && !key.mods.alt => {
                self.text.push(c);
                Step::Redraw
            }
            _ => Step::Same,
        }
    }

    fn paste(&mut self, text: &str, _look: &Look) -> Step {
        self.text.push_str(text.lines().next().unwrap_or(""));
        Step::Redraw
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        let Some(drawn) = &self.drawn else {
            return Step::Same;
        };
        let hit = drawn.hit(mouse.col, mouse.row);
        match draw::gesture(&mouse) {
            Gesture::Click if look.mouse && matches!(hit, Hit::Outside | Hit::Close) => {
                Step::Close(Outcome::Dismissed)
            }
            Gesture::Move if look.hover => {
                let lit = hit == Hit::Close;
                if lit == self.close_lit {
                    Step::Same
                } else {
                    self.close_lit = lit;
                    Step::Redraw
                }
            }
            _ => Step::Same,
        }
    }

    fn cursor(&self) -> Option<(u16, u16)> {
        let d = self.drawn.as_ref()?;
        let x = d.inner.x + 2 + draw::width(&self.prompt) as u16 + draw::width(&self.text) as u16;
        Some((x.min(d.inner.right().saturating_sub(1)), d.inner.y + 1))
    }
}

/// A box while a squash runs: a spinner, the notes being merged, the
/// seconds so far, and Esc to cancel, since Claude takes a while
/// (squash.lua's `progress`).
pub struct Progress {
    pub titles: Vec<String>,
    pub started: std::time::Instant,
    pub frame: usize,
    drawn: Option<Drawn>,
}

impl Progress {
    pub fn new(titles: Vec<String>) -> Progress {
        Progress {
            titles,
            started: std::time::Instant::now(),
            frame: 0,
            drawn: None,
        }
    }
}

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

impl Overlay for Progress {
    fn kind(&self) -> &'static str {
        "squash-progress"
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        self.frame = (self.started.elapsed().as_millis() / 100) as usize;
        let mut lines = vec![
            BoxLine::blank(),
            BoxLine::text(
                format!(
                    "  {} Squashing {} notes with Claude ({})",
                    FRAMES[self.frame % FRAMES.len()],
                    self.titles.len(),
                    super::squash::MODEL
                ),
                look.bold_bright(),
            ),
            BoxLine::blank(),
        ];
        for t in &self.titles {
            lines.push(BoxLine::text(format!("    · {t}"), look.text()));
        }
        lines.push(BoxLine::blank());
        lines.push(BoxLine::text(
            format!(
                "  {}s so far · it usually takes up to half a minute · Esc cancels",
                self.started.elapsed().as_secs()
            ),
            look.dim(),
        ));
        let spec = BoxSpec::new("Squash", &lines);
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn key(&mut self, key: Key, _look: &Look) -> Step {
        if key.code == KeyCode::Esc {
            Step::Close(Outcome::Dismissed)
        } else {
            Step::Same
        }
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        let close = self
            .drawn
            .as_ref()
            .is_some_and(|d| d.hit(mouse.col, mouse.row) == Hit::Close);
        if close && draw::gesture(&mouse) == Gesture::Click && look.mouse {
            return Step::Close(Outcome::Dismissed);
        }
        Step::Same
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::easy::BUTTONS;

    #[test]
    fn every_toolbar_button_names_an_item_of_the_menu() {
        for b in BUTTONS {
            if let Some(label) = b.item {
                assert!(by_label(label).is_some(), "{label}");
            }
        }
        let keys: Vec<&str> = items().iter().map(|(_, i)| i.key).collect();
        let mut unique = keys.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(keys.len(), unique.len(), "each key picks one item");
    }
}
