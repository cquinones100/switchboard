//! A line to type in a box over mission control (render.js's `askModal`):
//! a tab's new name, a reminder's time or what it is for, one of your own
//! settings. Its hint dim above, then what is typed after a `›`; Enter takes
//! it (an empty line too, where that means something: unsetting a setting,
//! a reminder for nothing in particular), Esc goes back. With easy mode's
//! ×, Tab moves to it and Enter presses it; typing takes the focus back.
//!
//! What is typed is a `ui::field`, with its cursor keys, wrapping at the
//! box's width and a click putting the cursor; one whose use takes more
//! than a line (`multi_line`) takes S-Enter and M-Enter as a new line.
//!
//! A multi-line ask can be a text box (`text_box`), as Shape a new ticket
//! is: its question dim, then `mission::text_box`'s field on the panel,
//! five rows at least, and a button that sends beside Cancel, both reached
//! by Tab and a click, as the Slack reply box draws them; a line under the
//! text can say why it came back (`failed`).
//!
//! It closes with what was typed as its outcome's value
//! (`Outcome::Picked`), trimmed.

use ratatui::buffer::Buffer;
use ratatui::text::Span;

use super::text_box::TextBox;
use crate::mux::input::{Key, KeyCode, Mods, Mouse};
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use crate::ui::field::{Edit, Field};
use crate::ui::kit::Tone;
use crate::ui::{Look, Outcome, Overlay, Picked, Step};

pub struct Ask {
    title: String,
    hint: String,
    field: Field,
    /// The index of the field's first row among the box's lines.
    field_line: usize,
    /// Whether Enter on an empty line takes it.
    empty: bool,
    close_focus: bool,
    /// Its text box, in place of the bare line, once `text_box` makes it one.
    boxed: Option<TextBox>,
    /// Why it came back, under the text box.
    failed: Option<String>,
    drawn: Option<Drawn>,
}

impl Ask {
    pub fn new(title: impl Into<String>, hint: impl Into<String>) -> Ask {
        Ask {
            title: title.into(),
            hint: hint.into(),
            field: Field::new(),
            field_line: 0,
            empty: false,
            close_focus: false,
            boxed: None,
            failed: None,
            drawn: None,
        }
    }

    /// What it starts with, to change rather than type again.
    pub fn text(mut self, text: impl Into<String>) -> Ask {
        let text = text.into();
        if let Some(boxed) = self.boxed.as_mut() {
            boxed.field = Field::new().multi_line().with_text(text);
            return self;
        }
        // Text of several lines (a shell command kept in a setting) keeps
        // them, and takes S-Enter for another.
        let field = if text.contains(['\n', '\r']) || self.field.is_multi_line() {
            Field::new().multi_line()
        } else {
            Field::new()
        };
        self.field = field.with_text(text);
        self
    }

    /// It takes more than a line: S-Enter and M-Enter add one.
    pub fn multi_line(mut self) -> Ask {
        self.field = Field::new().multi_line().with_text(self.field.text());
        self
    }

    /// Enter on an empty line takes it, rather than doing nothing.
    pub fn empty_ok(mut self) -> Ask {
        self.empty = true;
        self
    }

    /// Drawn as a text box, with `send` the button that takes it beside
    /// Cancel: a multi-line ask's form.
    pub fn text_box(mut self, send: &str) -> Ask {
        let field = std::mem::replace(&mut self.field, Field::new());
        self.boxed = Some(TextBox::new(field.multi_line(), "", send));
        self
    }

    /// The text box saying why it came back, under the text.
    pub fn failed(mut self, why: impl Into<String>) -> Ask {
        self.failed = Some(why.into());
        self
    }

    /// What is typed now.
    pub fn typed(&self) -> &str {
        match &self.boxed {
            Some(b) => b.field.text(),
            None => self.field.text(),
        }
    }

    /// The text box's form drawn.
    fn draw_boxed(&mut self, buf: &mut Buffer, look: &Look) {
        let Some(boxed) = self.boxed.as_mut() else {
            return;
        };
        let inner = (buf.area.width as usize).saturating_sub(6).min(80);
        let mut lines: Vec<BoxLine> = draw::wrap(&self.hint, inner.saturating_sub(2))
            .into_iter()
            .map(|w| BoxLine::text(format!(" {w}"), look.dim()))
            .collect();
        lines.push(BoxLine::blank());
        let fails: Vec<String> = self
            .failed
            .as_deref()
            .map(|why| draw::wrap(why, inner.saturating_sub(2)))
            .unwrap_or_default();
        // The box's border, title and keys, the blank lines and the buttons.
        let around = lines.len() + fails.len() + 6;
        let most = TextBox::most_rows(buf.area.height as usize, around);
        boxed.text_lines(&mut lines, inner, most, look);
        for line in fails {
            lines.push(BoxLine::text(
                format!(" {line}"),
                Tone::Failed.style(&look.theme),
            ));
        }
        boxed.buttons_line(&mut lines, inner, "", look);
        let mut spec = BoxSpec::new(&self.title, &lines);
        spec.keys = Some("↵ done  S-↵ new line  esc cancel");
        spec.width = inner as u16;
        spec.close_lit = boxed.close_lit();
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    /// What the text box's step means for the ask: what was typed, trimmed.
    fn taken(step: Step) -> Step {
        match step {
            Step::Close(Outcome::Picked(p)) => {
                let text = p.query.trim().to_string();
                Step::Close(Outcome::Picked(Picked {
                    index: 0,
                    value: text.clone(),
                    query: text,
                }))
            }
            other => other,
        }
    }
}

impl Overlay for Ask {
    fn drawn(&self) -> Option<&crate::ui::draw::Drawn> {
        self.drawn.as_ref()
    }

    fn kind(&self) -> &'static str {
        "ask"
    }

    /// What is typed, so a box Esc goes back to from the next opens with it.
    fn kept(&self) -> Option<switchboard_ui::overlay::Kept> {
        Some(switchboard_ui::overlay::Kept {
            typed: self.typed().to_string(),
            chosen: None,
        })
    }

    fn restore(&mut self, kept: &switchboard_ui::overlay::Kept) {
        if kept.typed.is_empty() {
            return;
        }
        match self.boxed.as_mut() {
            Some(boxed) => boxed.field = Field::new().multi_line().with_text(&kept.typed),
            None => {
                let multi = self.field.is_multi_line() || kept.typed.contains('\n');
                let field = if multi {
                    Field::new().multi_line()
                } else {
                    Field::new()
                };
                self.field = field.with_text(&kept.typed);
            }
        }
    }

    fn stands_out(&self) -> Option<ratatui::layout::Rect> {
        self.drawn.as_ref().map(|d| d.outer)
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        if self.boxed.is_some() {
            return self.draw_boxed(buf, look);
        }
        let inner = (buf.area.width as usize).saturating_sub(4).min(90);
        let mut lines: Vec<BoxLine> = draw::wrap(&self.hint, inner.saturating_sub(2))
            .into_iter()
            .map(|w| BoxLine::text(format!(" {w}"), look.dim()))
            .collect();
        lines.push(BoxLine::blank());
        self.field_line = lines.len();
        // Up to half the screen, and never more than the box has rows for
        // under the hint.
        let left = (buf.area.height as usize).saturating_sub(6 + lines.len());
        let most = (buf.area.height as usize / 2)
            .saturating_sub(4)
            .min(left)
            .max(1);
        let rows = self.field.view(
            inner.saturating_sub(4),
            most,
            look.bright(),
            look.dim(),
            true,
        );
        for (i, row) in rows.into_iter().enumerate() {
            let lead = if i == 0 { " › " } else { "   " };
            let mut spans = vec![Span::styled(lead, look.bright())];
            spans.extend(row);
            lines.push(BoxLine::spans(spans));
        }
        let mut spec = BoxSpec::new(&self.title, &lines);
        spec.keys = Some(if self.field.is_multi_line() {
            "type  ⇧↵ new line  ↵ done  esc back"
        } else {
            "type  ↵ done  esc back"
        });
        spec.width = inner as u16;
        spec.close_lit = self.close_focus;
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn key(&mut self, key: Key, look: &Look) -> Step {
        if let Some(boxed) = self.boxed.as_mut() {
            return Self::taken(boxed.key(key, look));
        }
        let k = key.normalized();
        if matches!(k.code, KeyCode::Tab | KeyCode::BackTab) && look.close {
            self.close_focus = !self.close_focus;
            return Step::Redraw;
        }
        if self.close_focus && matches!(k.code, KeyCode::Enter | KeyCode::Char(' ')) {
            return Step::Close(Outcome::Dismissed);
        }
        self.close_focus = false;
        match self.field.key(k) {
            Edit::Changed | Edit::Moved => return Step::Redraw,
            Edit::Same | Edit::Up | Edit::Down => return Step::Same,
            Edit::Not => {}
        }
        match k.code {
            KeyCode::Esc => Step::Close(Outcome::Dismissed),
            KeyCode::Char('c') if k.mods == Mods::CTRL => Step::Close(Outcome::Dismissed),
            KeyCode::Enter => {
                let text = self.field.text().trim().to_string();
                if text.is_empty() && !self.empty {
                    return Step::Same;
                }
                Step::Close(Outcome::Picked(Picked {
                    index: 0,
                    value: text.clone(),
                    query: text,
                }))
            }
            _ => Step::Same,
        }
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        if let Some(boxed) = self.boxed.as_mut() {
            return Self::taken(boxed.mouse(mouse, self.drawn.as_ref(), look));
        }
        let Some(drawn) = &self.drawn else {
            return Step::Same;
        };
        let hit = drawn.hit(mouse.col, mouse.row);
        match draw::gesture(&mouse) {
            Gesture::Click if look.mouse && matches!(hit, Hit::Close | Hit::Outside) => {
                Step::Close(Outcome::Dismissed)
            }
            Gesture::Click if look.mouse => match hit {
                Hit::Line(i, col) if i >= self.field_line => {
                    self.close_focus = false;
                    self.field
                        .click(i - self.field_line, (col as usize).saturating_sub(3));
                    Step::Redraw
                }
                _ => Step::Same,
            },
            Gesture::Move if look.hover => {
                let close = hit == Hit::Close;
                if close == self.close_focus {
                    Step::Same
                } else {
                    self.close_focus = close;
                    Step::Redraw
                }
            }
            _ => Step::Same,
        }
    }

    fn paste(&mut self, text: &str, _look: &Look) -> Step {
        if let Some(boxed) = self.boxed.as_mut() {
            return boxed.paste(text);
        }
        self.field.paste(text);
        Step::Redraw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_takes_what_is_typed_and_an_empty_line_only_when_that_means_something() {
        let look = Look::default();
        let mut a = Ask::new("Rename", "A name.").text("old");
        for _ in 0..3 {
            a.key(Key::plain(KeyCode::Backspace), &look);
        }
        assert_eq!(a.key(Key::plain(KeyCode::Enter), &look), Step::Same);
        for c in "new one ".chars() {
            a.key(Key::plain(KeyCode::Char(c)), &look);
        }
        assert!(matches!(
            a.key(Key::plain(KeyCode::Enter), &look),
            Step::Close(Outcome::Picked(p)) if p.value == "new one"
        ));
        let mut a = Ask::new("What for?", "Optional.").empty_ok();
        assert!(matches!(
            a.key(Key::plain(KeyCode::Enter), &look),
            Step::Close(Outcome::Picked(p)) if p.value.is_empty()
        ));
        let mut a = Ask::new("x", "y");
        assert_eq!(
            a.key(Key::plain(KeyCode::Esc), &look),
            Step::Close(Outcome::Dismissed)
        );
    }

    fn screen(a: &mut Ask, cols: u16, rows: u16) -> Vec<String> {
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, cols, rows));
        a.draw(&mut buf, &Look::default());
        crate::ui::draw::tests::screen(&buf)
    }

    #[test]
    fn s_enter_adds_a_line_in_a_multi_line_ask_and_enter_takes_both() {
        let look = Look::default();
        let mut a = Ask::new("Shape a new ticket", "What about?").multi_line();
        a.paste("first", &look);
        assert_eq!(
            a.key(Key::new(KeyCode::Enter, Mods::SHIFT), &look),
            Step::Redraw
        );
        a.paste("second", &look);
        let s = screen(&mut a, 60, 20);
        assert!(s.iter().any(|l| l.contains("› first")), "{s:#?}");
        assert!(s.iter().any(|l| l.contains("   second▏")), "{s:#?}");
        assert!(matches!(
            a.key(Key::plain(KeyCode::Enter), &look),
            Step::Close(Outcome::Picked(p)) if p.value == "first\nsecond"
        ));
        let mut one = Ask::new("Rename", "A name.").text("a");
        assert_eq!(
            one.key(Key::new(KeyCode::Enter, Mods::SHIFT), &look),
            Step::Same
        );
    }

    #[test]
    fn text_of_several_lines_keeps_them() {
        let look = Look::default();
        let mut a = Ask::new("Test command", "A command.").text("cat <<-EOF\n\tx\n\tEOF");
        assert!(matches!(
            a.key(Key::plain(KeyCode::Enter), &look),
            Step::Close(Outcome::Picked(p)) if p.value == "cat <<-EOF\n\tx\n\tEOF"
        ));
    }

    fn shape_box() -> Ask {
        Ask::new("Shape a new GitHub issue", "What should it be about?")
            .multi_line()
            .text_box("Shape it")
    }

    #[test]
    fn the_text_box_draws_five_empty_rows_and_its_buttons() {
        let mut a = shape_box();
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 70, 30));
        a.draw(&mut buf, &Look::default());
        let panel = crate::ui::kit::Surface::Panel.color(&Look::default().theme);
        // The rows drawn on the panel, at the text's first column.
        let rows = (0..30)
            .filter(|y| buf[(10, *y)].bg == panel && buf[(30, *y)].bg == panel)
            .count();
        assert_eq!(rows, super::super::text_box::LEAST_ROWS);
        let s = crate::ui::draw::tests::screen(&buf);
        assert!(
            s.iter().any(|l| l.contains("Shape a new GitHub issue")),
            "{s:#?}"
        );
        assert!(
            s.iter().any(|l| l.contains("What should it be about?")),
            "{s:#?}"
        );
        assert!(
            s.iter()
                .any(|l| l.contains("Cancel") && l.contains("Shape it")),
            "{s:#?}"
        );
    }

    #[test]
    fn text_wraps_inside_the_text_box() {
        let mut a = shape_box();
        a.paste(&"word ".repeat(40), &Look::default());
        let s = screen(&mut a, 60, 30);
        let rows: Vec<&String> = s.iter().filter(|l| l.contains("word")).collect();
        assert!(rows.len() > 1, "{s:#?}");
        // Inside the box's border, which is at the edges of a 60-wide box.
        assert!(
            rows.iter().all(|l| draw::width(l.trim_end()) <= 58),
            "{s:#?}"
        );
    }

    #[test]
    fn tab_reaches_shape_it_and_cancel_and_enter_takes_the_lines() {
        let look = Look::default();
        let mut a = shape_box();
        // Shape it with nothing typed stays.
        a.key(Key::plain(KeyCode::Tab), &look);
        assert_eq!(a.key(Key::plain(KeyCode::Enter), &look), Step::Redraw);
        a.paste("first", &look);
        a.key(Key::new(KeyCode::Enter, Mods::SHIFT), &look);
        a.paste("second  ", &look);
        assert_eq!(a.typed(), "first\nsecond  ");
        a.key(Key::plain(KeyCode::Tab), &look);
        assert!(matches!(
            a.key(Key::plain(KeyCode::Enter), &look),
            Step::Close(Outcome::Picked(p)) if p.value == "first\nsecond"
        ));
        let mut a = shape_box();
        a.paste("kept", &look);
        a.key(Key::plain(KeyCode::Tab), &look);
        a.key(Key::plain(KeyCode::Tab), &look);
        assert_eq!(
            a.key(Key::plain(KeyCode::Char(' ')), &look),
            Step::Close(Outcome::Dismissed)
        );
        let mut a = shape_box();
        a.paste("one", &look);
        assert!(matches!(
            a.key(Key::plain(KeyCode::Enter), &look),
            Step::Close(Outcome::Picked(p)) if p.value == "one"
        ));
    }

    #[test]
    fn a_click_on_shape_it_takes_it_and_on_cancel_closes() {
        use crate::mux::input::{MouseButton, MouseKind};
        let look = Look::default();
        let find = |a: &mut Ask, word: &str| -> (u16, u16) {
            let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 70, 30));
            a.draw(&mut buf, &look);
            let s = crate::ui::draw::tests::screen(&buf);
            let y = s.iter().position(|l| l.contains(word)).unwrap();
            let x = s[y].find(word).unwrap();
            (draw::width(&s[y][..x]) as u16 + 1, y as u16)
        };
        let click = |x: u16, y: u16| Mouse {
            kind: MouseKind::Press,
            button: MouseButton::Left,
            col: x,
            row: y,
            mods: Mods::NONE,
        };
        let mut a = shape_box().text("an idea");
        let (x, y) = find(&mut a, "Shape it");
        assert!(matches!(
            a.mouse(click(x, y), &look),
            Step::Close(Outcome::Picked(p)) if p.value == "an idea"
        ));
        let mut a = shape_box().text("an idea");
        let (x, y) = find(&mut a, "Cancel");
        assert_eq!(a.mouse(click(x, y), &look), Step::Close(Outcome::Dismissed));
    }

    #[test]
    fn it_comes_back_with_the_text_and_why() {
        let mut a = shape_box().text("kept").failed("Not in a repository: ~/x");
        let s = screen(&mut a, 70, 30);
        assert!(s.iter().any(|l| l.contains("kept")), "{s:#?}");
        assert!(
            s.iter().any(|l| l.contains("Not in a repository: ~/x")),
            "{s:#?}"
        );
    }

    #[test]
    fn what_is_typed_is_kept_for_the_box_esc_goes_back_to() {
        let look = Look::default();
        let mut a = shape_box();
        a.paste("an idea", &look);
        let kept = a.kept().unwrap();
        let mut again = shape_box();
        again.restore(&kept);
        assert_eq!(again.typed(), "an idea");
    }

    #[test]
    fn a_long_name_wraps_in_the_box() {
        let mut a = Ask::new("Rename", "A name.").text("alpha bravo charlie delta echo foxtrot");
        let s = screen(&mut a, 24, 20);
        assert!(s.iter().any(|l| l.contains("› alpha bravo")), "{s:#?}");
        assert!(s.iter().any(|l| l.contains("foxtrot▏")), "{s:#?}");
    }
}
