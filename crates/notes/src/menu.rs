//! The Format menu, Ctrl-P in a note: everything a note can do, each with
//! its key, under its heading (init.lua's `format_items` and `menu`). It
//! is `ui::menu`'s menu, its items' keys coming first: a key, Enter or a
//! click on a line picks it; Escape or Q closes it. `/` searches it as it
//! searches every menu: the items whose label holds what is typed, best
//! first, without their headings. Opened
//! from Visual mode, it acts on the selection. A paragraph or character
//! style leaves you typing, as `a` would, unless it already has: you pick a
//! style to write in it. The toolbar's buttons (easy.rs) and the palette
//! (palette.rs) run the same items.
//!
//! Being an overlay of the ui module's, the app stacks it with the palette
//! and the questions.

use ratatui::buffer::Buffer;
use ratatui::style::Style;
use ratatui::text::Span;

use super::format::{Character, Paragraph};
use switchboard_ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use switchboard_ui::input::{Key, KeyCode, Mouse};
use switchboard_ui::{Look, Outcome, Overlay, Step};
use switchboard_ui::{Menu, MenuOption};

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
    /// The note's mermaid blocks shown as their code, or drawn again.
    DiagramCode,
    /// Attach the note to a tab, from Switchboard, which asks which.
    AttachNote,
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

/// The Diagram item's label, and its label while the note shows its
/// diagrams' code.
pub const SHOW_DIAGRAM_CODE: &str = "Show diagram code";
pub const SHOW_DIAGRAMS: &str = "Show diagrams";

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
    ("Diagram", &[item("", SHOW_DIAGRAM_CODE, Act::DiagramCode)]),
    (
        "Note",
        &[item("", "Attach this note to a tab", Act::AttachNote)],
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

/// The menu, as every menu is (crates/ui/src/menu.rs), its items' keys
/// coming before the keys that move it: picking gives `Outcome::Chose`
/// with the item's place in `items()`. The Diagram item reads "Show
/// diagrams" while the note shows its diagrams' code (`diagram_code`).
pub fn format_menu(diagram_code: bool) -> Menu {
    let sections = SECTIONS
        .iter()
        .map(|(heading, items)| {
            let options = items
                .iter()
                .map(|i| {
                    let option = MenuOption::new(label(i, diagram_code));
                    match i.key.chars().next() {
                        Some(c) => option.key(c),
                        None => option,
                    }
                })
                .collect();
            (heading.to_string(), options)
        })
        .collect();
    Menu::sections("Format", sections)
        .keys_first()
        .named("format-menu")
}

/// An item's label as the menu shows it: the Diagram item's "Show
/// diagrams" while the note shows its diagrams' code.
fn label(item: &Item, diagram_code: bool) -> &'static str {
    if item.act == Act::DiagramCode && diagram_code {
        SHOW_DIAGRAMS
    } else {
        item.label
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
    fn drawn(&self) -> Option<&switchboard_ui::draw::Drawn> {
        self.drawn.as_ref()
    }

    fn kind(&self) -> &'static str {
        "ask"
    }

    fn stands_out(&self) -> Option<ratatui::layout::Rect> {
        self.drawn.as_ref().map(|d| d.outer)
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
            KeyCode::Enter => Step::Close(Outcome::Picked(switchboard_ui::Picked {
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
    fn drawn(&self) -> Option<&switchboard_ui::draw::Drawn> {
        self.drawn.as_ref()
    }

    fn kind(&self) -> &'static str {
        "squash-progress"
    }

    fn stands_out(&self) -> Option<ratatui::layout::Rect> {
        self.drawn.as_ref().map(|d| d.outer)
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
    use crate::easy::BUTTONS;

    #[test]
    fn every_toolbar_button_names_an_item_of_the_menu() {
        for b in BUTTONS {
            if let Some(label) = b.item {
                assert!(by_label(label).is_some(), "{label}");
            }
        }
        // An item with no key is picked with the arrows or a search alone.
        let keys: Vec<&str> = items()
            .iter()
            .map(|(_, i)| i.key)
            .filter(|k| !k.is_empty())
            .collect();
        let mut unique = keys.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(keys.len(), unique.len(), "each key picks one item");
    }

    #[test]
    fn its_items_keys_come_first_and_headings_show() {
        use switchboard_ui::input::Mods;
        let look = Look::default();
        let mut m = format_menu(false);
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 80, 60));
        m.draw(&mut buf, &look);
        let s = switchboard_ui::draw::tests::screen(&buf);
        assert!(s.iter().any(|l| l.contains("PARAGRAPH")), "{s:#?}");
        let link = items().iter().position(|(_, i)| i.act == Act::Link);
        assert_eq!(
            m.key(Key::new(KeyCode::Char('k'), Mods::NONE), &look),
            Step::Close(Outcome::Chose(link.unwrap())),
            "k is Link…, not up"
        );
        let mut m = format_menu(false);
        let quote = items()
            .iter()
            .position(|(_, i)| i.act == Act::Paragraph(Paragraph::Quote));
        assert_eq!(
            m.key(Key::new(KeyCode::Char('q'), Mods::NONE), &look),
            Step::Close(Outcome::Chose(quote.unwrap()))
        );
        assert_eq!(
            m.key(Key::new(KeyCode::Char('Q'), Mods::NONE), &look),
            Step::Close(Outcome::Dismissed)
        );
    }

    #[test]
    fn slash_searches_the_items_and_their_keys_type() {
        use switchboard_ui::input::Mods;
        let look = Look::default();
        let mut m = format_menu(false);
        for c in "/check".chars() {
            assert_eq!(
                m.key(Key::new(KeyCode::Char(c), Mods::NONE), &look),
                Step::Redraw
            );
        }
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 60, 30));
        m.draw(&mut buf, &look);
        let s = switchboard_ui::draw::tests::screen(&buf);
        let shown: Vec<&str> = [
            "Checklist",
            "Move checked to bottom",
            "Delete checked items",
            "Uncheck all",
        ]
        .into_iter()
        .filter(|l| s.iter().any(|row| row.contains(l)))
        .collect();
        assert_eq!(shown.len(), 4, "{s:#?}");
        assert!(s.iter().any(|l| l.contains("› check")), "{s:#?}");
        assert!(!s.iter().any(|l| l.contains("PARAGRAPH")), "no headings");
        let mut short = Buffer::empty(ratatui::layout::Rect::new(0, 0, 60, 11));
        for _ in 0..3 {
            m.key(Key::new(KeyCode::Down, Mods::NONE), &look);
        }
        m.draw(&mut short, &look);
        let s = switchboard_ui::draw::tests::screen(&short);
        assert!(
            s.iter().any(|l| l.contains("› check▏")),
            "the search stays: {s:#?}"
        );
        assert!(s.iter().any(|l| l.contains("› Uncheck all")), "{s:#?}");
        for _ in 0..3 {
            m.key(Key::new(KeyCode::Up, Mods::NONE), &look);
        }
        m.key(Key::new(KeyCode::Tab, Mods::NONE), &look);
        let sort = items().iter().position(|(_, i)| i.act == Act::SortChecked);
        assert_eq!(
            m.key(Key::new(KeyCode::Enter, Mods::NONE), &look),
            Step::Close(Outcome::Chose(sort.unwrap()))
        );
    }
}
