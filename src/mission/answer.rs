//! Answering a tab from mission control (lib/sidebar/answer.js), as the
//! Slack relay answers one from a thread: the question or permission prompt
//! its Claude shows, read off its screen by `agent::screen`, and your answer
//! pressed there. A tab showing no prompt takes what you type as a message,
//! pasted and sent with Enter.
//!
//! `AnswerBox` is the box over mission control (render.js's `answerModal`):
//! the question, its options numbered, and a line to type in. A number
//! answers with that option, `s` submits a question of several answers, `t`
//! types your own answer where the question takes one; Tab and Shift-Tab
//! move a highlight round the options (and easy mode's ×), Enter taking it.
//! `/` searches the options as it searches every menu (crates/ui/src/menu.rs's
//! `Filter`), each keeping its number, since that is what is pressed.
//! What is typed is a multi-line `ui::field`: S-Enter and M-Enter start a
//! new line, pasted with the rest, and Enter sends it.
//! It closes with what to press, which the server presses in the pane.

use ratatui::buffer::Buffer;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::agent::screen::Prompt;
use crate::mux::input::{Key, KeyCode, Mods, Mouse};
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use crate::ui::field::{Edit, Field};
use crate::ui::menu::{Filter, Filtered, NO_MATCH, SEARCH_KEYS, wrap_step};
use crate::ui::{Look, Outcome, Overlay, Picked, Step};

// ── The box ─────────────────────────────────────────────────────────────────

/// What the box gives back as its outcome's value: an option's number,
/// `submit`, or the text typed.
pub struct AnswerBox {
    title: String,
    found: Option<Prompt>,
    field: Field,
    /// The box's lines that hold the field's rows.
    field_lines: std::ops::Range<usize>,
    typing: bool,
    note: Option<String>,
    /// The search `/` opens over the options.
    filter: Filter,
    /// The line the search was last drawn on, while it is open.
    search_line: Option<usize>,
    /// Where Tab has put the highlight: one of the options shown, or past
    /// the last the close button.
    focus: Option<usize>,
    close_focus: bool,
    /// The option under the mouse.
    lit: Option<u32>,
    drawn: Option<Drawn>,
    /// Which of the box's lines holds which option.
    option_rows: Vec<(usize, u32)>,
}

impl AnswerBox {
    pub fn new(title: String, found: Option<Prompt>, note: Option<String>) -> AnswerBox {
        AnswerBox {
            title,
            found,
            field: Field::new().multi_line(),
            field_lines: 0..0,
            typing: false,
            note,
            filter: Filter::default(),
            search_line: None,
            focus: None,
            close_focus: false,
            lit: None,
            drawn: None,
            option_rows: Vec::new(),
        }
    }

    /// The prompt it shows.
    pub fn found(&self) -> Option<&Prompt> {
        self.found.as_ref()
    }

    fn give(reply: impl Into<String>) -> Step {
        Step::Close(Outcome::Picked(Picked {
            index: 0,
            value: reply.into(),
            query: String::new(),
        }))
    }

    /// The options shown, by their place: every one while the search is
    /// closed.
    fn shown(&self) -> Vec<usize> {
        let labels: Vec<&str> = self
            .found
            .iter()
            .flat_map(|f| f.options.iter().map(|o| o.label.as_str()))
            .collect();
        self.filter.shown(&labels)
    }

    fn typing_line(&self) -> bool {
        self.typing || self.found.is_none()
    }
}

impl Overlay for AnswerBox {
    fn drawn(&self) -> Option<&crate::ui::draw::Drawn> {
        self.drawn.as_ref()
    }

    fn kind(&self) -> &'static str {
        "answer"
    }

    fn stands_out(&self) -> Option<ratatui::layout::Rect> {
        self.drawn.as_ref().map(|d| d.outer)
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let inner = (buf.area.width as usize).saturating_sub(4).min(90);
        let mut lines: Vec<BoxLine> = Vec::new();
        self.option_rows.clear();
        let typing = self.typing_line();
        if let Some(found) = &self.found {
            let mut asked: Vec<String> = found
                .question
                .split('\n')
                .flat_map(|line| draw::wrap(line, inner.saturating_sub(2)))
                .collect();
            // While searching, a question too long for the screen gives up
            // its last rows, so the search and a match still fit in the box
            // (four rows of its own and two of keys).
            // While typing, it gives them up so the options, a blank and a
            // row of the field (and the note) still fit under it.
            let room = (buf.area.height as usize).saturating_sub(6);
            let most = if typing {
                let below = if self.note.is_some() { 2 } else { 0 };
                room.saturating_sub(found.options.len() + 3 + below).max(1)
            } else {
                room.saturating_sub(4).max(1)
            };
            if (self.filter.is_open() || typing) && asked.len() > most {
                asked.truncate(most - 1);
                asked.push("…".into());
            }
            for w in asked {
                lines.push(BoxLine::text(format!(" {w}"), look.bright()));
            }
            lines.push(BoxLine::blank());
            let shown = self.shown();
            self.search_line = self.filter.is_open().then_some(lines.len());
            if self.filter.is_open() {
                lines.push(self.filter.line(
                    inner.saturating_sub(2),
                    shown.len(),
                    found.options.len(),
                    look,
                ));
                lines.push(BoxLine::blank());
                if shown.is_empty() {
                    lines.push(BoxLine::text(NO_MATCH, look.dim()));
                }
            }
            // While searching, as many matches as the screen has rows for
            // under the question and the search, the chosen one among them.
            let (first, fits) = if self.filter.is_open() {
                let room = room.saturating_sub(lines.len()).max(1);
                let at = self.focus.unwrap_or(0);
                (at.saturating_sub(room - 1), room)
            } else {
                (0, shown.len())
            };
            for (at, o) in shown
                .iter()
                .map(|i| &found.options[*i])
                .enumerate()
                .skip(first)
                .take(fits)
            {
                let chosen = self.focus == Some(at) && !typing;
                let lit = !chosen && !typing && self.lit == Some(o.n);
                let reverse = |s: Style| {
                    if lit {
                        s.add_modifier(Modifier::REVERSED)
                    } else {
                        s
                    }
                };
                let boxed = match o.checked {
                    None => "",
                    Some(true) => "☑ ",
                    Some(false) => "☐ ",
                };
                let label = if chosen {
                    look.bold_bright()
                } else {
                    look.text()
                };
                self.option_rows.push((lines.len(), o.n));
                // While a custom answer is typed, the options take no click.
                lines.push(BoxLine {
                    clickable: !typing,
                    ..BoxLine::spans(vec![
                        Span::raw(" "),
                        Span::styled(
                            o.n.to_string(),
                            reverse(Style::new().fg(draw::YELLOW).add_modifier(Modifier::BOLD)),
                        ),
                        Span::raw("  "),
                        Span::styled(
                            format!("{boxed}{}", draw::cut(&o.label, inner.saturating_sub(8))),
                            reverse(label),
                        ),
                    ])
                    .fill(chosen.then_some(look.theme.highlight))
                });
            }
        } else {
            lines.push(BoxLine::text(
                " No question showing: what you type is sent to it as a message.",
                look.dim(),
            ));
        }
        if typing {
            lines.push(BoxLine::blank());
            let first = lines.len();
            // Up to half the screen, and never more than the box has rows
            // for under the question and the options (and the note after).
            let below = if self.note.is_some() { 2 } else { 0 };
            let left = (buf.area.height as usize).saturating_sub(6 + lines.len() + below);
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
            self.field_lines = first..lines.len();
        }
        if let Some(note) = &self.note {
            lines.push(BoxLine::blank());
            lines.push(BoxLine::text(format!(" {note}"), look.dim()));
        }
        let keys = match &self.found {
            Some(_) if self.filter.is_open() => SEARCH_KEYS.to_string(),
            Some(found) if !typing => format!(
                "1-{} or tab ↵ choose{}{}  esc back",
                found.options.len(),
                if found.multi { "  s submit" } else { "" },
                if found.free.is_some() {
                    "  t type your own"
                } else {
                    ""
                }
            ),
            _ => "type  ⇧↵ new line  ↵ send  esc back".to_string(),
        };
        let mut spec = BoxSpec::new(&self.title, &lines);
        spec.keys = Some(&keys);
        spec.width = inner as u16;
        let options = self.shown().len();
        spec.close_lit = self.close_focus || (!typing && self.focus == Some(options));
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn key(&mut self, key: Key, look: &Look) -> Step {
        let k = key.normalized();
        let plain = k.mods.is_empty();
        if !self.typing_line() {
            match self.filter.key(k) {
                Filtered::Changed | Filtered::Closed => {
                    self.focus = self.filter.is_open().then_some(0);
                    return Step::Redraw;
                }
                Filtered::Moved => return Step::Redraw,
                Filtered::Not => {}
            }
        }
        if self.filter.is_open() {
            let shown = self.shown();
            let last = shown.len().saturating_sub(1);
            let at = self.focus.unwrap_or(0);
            let to = match k.code {
                KeyCode::Enter => {
                    return match (shown.get(at), &self.found) {
                        (Some(i), Some(found)) => AnswerBox::give(found.options[*i].n.to_string()),
                        _ => Step::Same,
                    };
                }
                KeyCode::Down => wrap_step(at, shown.len(), true),
                KeyCode::Char('n') if k.mods == Mods::CTRL => wrap_step(at, shown.len(), true),
                KeyCode::Up => wrap_step(at, shown.len(), false),
                KeyCode::Char('p') if k.mods == Mods::CTRL => wrap_step(at, shown.len(), false),
                KeyCode::Tab => at + 1,
                KeyCode::BackTab => at.saturating_sub(1),
                KeyCode::Char('c') if k.mods == Mods::CTRL => {
                    return Step::Close(Outcome::Dismissed);
                }
                _ => return Step::Same,
            }
            .min(last);
            self.focus = Some(to);
            return Step::Redraw;
        }
        if k.code == KeyCode::Esc || (k.code == KeyCode::Char('c') && k.mods == Mods::CTRL) {
            return Step::Close(Outcome::Dismissed);
        }
        if self.typing_line() {
            // Tab puts focus on easy mode's close button, Enter pressing it;
            // typing takes it back to the line.
            if let Some(by) = crate::ui::form::tab_by(&k).filter(|_| look.close) {
                // Round the line (false) and the × (true).
                self.close_focus = crate::ui::form::step(&[false, true], self.close_focus, by);
                return Step::Redraw;
            }
            if self.close_focus && matches!(k.code, KeyCode::Enter | KeyCode::Char(' ')) {
                return Step::Close(Outcome::Dismissed);
            }
            self.close_focus = false;
            return match self.field.key(k) {
                Edit::Changed | Edit::Moved => Step::Redraw,
                Edit::Not if k.code == KeyCode::Enter => {
                    AnswerBox::give(self.field.text().to_string())
                }
                _ => Step::Same,
            };
        }
        let Some(found) = &self.found else {
            return Step::Same;
        };
        let options = found.options.len();
        let stops = options + usize::from(look.close);
        let ctrl = k.mods == Mods::CTRL;
        let arrow_down = k.code == KeyCode::Down
            || (ctrl && k.code == KeyCode::Char('n'))
            || (plain && k.code == KeyCode::Char('j'));
        let arrow_up = k.code == KeyCode::Up
            || (ctrl && k.code == KeyCode::Char('p'))
            || (plain && k.code == KeyCode::Char('k'));
        if (arrow_down || arrow_up) && options > 0 {
            self.focus = Some(match self.focus {
                None if arrow_down => 0,
                None => options - 1,
                Some(at) => wrap_step(at, options, arrow_down),
            });
            return Step::Redraw;
        }
        match k.code {
            KeyCode::Tab | KeyCode::BackTab if stops > 0 => {
                let by = crate::ui::form::tab_by(&k).unwrap_or(1);
                // From nowhere, Tab goes to the first and Shift-Tab the last.
                let from = self.focus.unwrap_or(if by > 0 { stops - 1 } else { 0 });
                self.focus = Some(if from >= stops {
                    0
                } else {
                    crate::ui::form::step_index(from, stops, by)
                });
                Step::Redraw
            }
            KeyCode::Enter | KeyCode::Char(' ') if plain || k.code == KeyCode::Enter => {
                match self.focus {
                    Some(at) if at == options && look.close => Step::Close(Outcome::Dismissed),
                    Some(at) if at < options => AnswerBox::give(found.options[at].n.to_string()),
                    _ => Step::Same,
                }
            }
            KeyCode::Char(c @ '1'..='9') if plain => AnswerBox::give(c.to_string()),
            KeyCode::Char('s') if plain && found.multi => AnswerBox::give("submit"),
            KeyCode::Char('t') if plain && found.free.is_some() => {
                self.typing = true;
                Step::Redraw
            }
            _ => Step::Same,
        }
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        let Some(drawn) = &self.drawn else {
            return Step::Same;
        };
        let hit = drawn.hit(mouse.col, mouse.row);
        let option = match hit {
            Hit::Line(i, _) if !self.typing_line() => self
                .option_rows
                .iter()
                .find(|(line, _)| *line == i)
                .map(|(_, n)| *n),
            _ => None,
        };
        match draw::gesture(&mouse) {
            Gesture::Click if look.mouse => match (hit, option) {
                (_, Some(n)) => AnswerBox::give(n.to_string()),
                (Hit::Line(i, col), _) if Some(i) == self.search_line => {
                    self.filter.click(col as usize);
                    Step::Redraw
                }
                (Hit::Line(i, col), _) if self.typing_line() && self.field_lines.contains(&i) => {
                    self.close_focus = false;
                    self.field
                        .click(i - self.field_lines.start, (col as usize).saturating_sub(3));
                    Step::Redraw
                }
                (Hit::Close | Hit::Outside, _) => Step::Close(Outcome::Dismissed),
                _ => Step::Same,
            },
            Gesture::Move if look.hover => {
                let close = hit == Hit::Close;
                if option == self.lit && close == self.close_focus {
                    Step::Same
                } else {
                    self.lit = option;
                    self.close_focus = close;
                    Step::Redraw
                }
            }
            _ => Step::Same,
        }
    }

    fn paste(&mut self, text: &str, _look: &Look) -> Step {
        if self.filter.paste(text) {
            self.focus = Some(0);
            return Step::Redraw;
        }
        if !self.typing_line() {
            return Step::Same;
        }
        self.field.paste(text);
        Step::Redraw
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::screen::options_in;

    const PERMISSION: &str = "\
╭───────────────────────────────────────────╮
│ Bash command                              │
│                                           │
│   git push origin main                    │
│                                           │
│ Do you want to proceed?                   │
│ ❯ 1. Yes                                  │
│   2. Yes, and don't ask again for git push│
│   3. No, and tell Claude what to do       │
╰───────────────────────────────────────────╯";

    #[test]
    fn the_arrows_choose_an_answer_and_wrap_round() {
        let look = Look::default();
        let mut b = AnswerBox::new("Answer 2 fix".into(), options_in(PERMISSION), None);
        let options = b.found.as_ref().unwrap().options.len();
        b.key(Key::new(KeyCode::Up, Mods::NONE), &look);
        assert_eq!(b.focus, Some(options - 1), "Up first chooses the last");
        b.key(Key::new(KeyCode::Down, Mods::NONE), &look);
        assert_eq!(b.focus, Some(0), "Down on the last chooses the first");
        b.key(Key::new(KeyCode::Char('p'), Mods::CTRL), &look);
        assert_eq!(b.focus, Some(options - 1));
        b.key(Key::new(KeyCode::Char('j'), Mods::NONE), &look);
        assert_eq!(b.focus, Some(0), "j on the last chooses the first");
        b.key(Key::new(KeyCode::Char('k'), Mods::NONE), &look);
        assert_eq!(
            b.focus,
            Some(options - 1),
            "k on the first chooses the last"
        );
        assert!(matches!(
            b.key(Key::new(KeyCode::Enter, Mods::NONE), &look),
            Step::Close(Outcome::Picked(p)) if p.value == options.to_string()
        ));
    }

    #[test]
    fn the_box_answers_with_a_number_and_types_a_message_when_no_prompt_shows() {
        let look = Look::default();
        let mut b = AnswerBox::new("Answer 2 fix".into(), options_in(PERMISSION), None);
        let key = |c| Key::new(KeyCode::Char(c), Mods::NONE);
        assert!(matches!(
            b.key(key('2'), &look),
            Step::Close(Outcome::Picked(p)) if p.value == "2"
        ));
        let mut b = AnswerBox::new("Answer 2 fix".into(), options_in(PERMISSION), None);
        b.key(Key::new(KeyCode::Tab, Mods::NONE), &look);
        b.key(Key::new(KeyCode::Tab, Mods::NONE), &look);
        assert!(matches!(
            b.key(Key::new(KeyCode::Enter, Mods::NONE), &look),
            Step::Close(Outcome::Picked(p)) if p.value == "2"
        ));
        let mut b = AnswerBox::new("Answer 2 fix".into(), None, None);
        for c in "hi there".chars() {
            b.key(key(c), &look);
        }
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 20));
        b.draw(&mut buf, &look);
        let s = crate::ui::draw::tests::screen(&buf);
        assert!(s.iter().any(|l| l.contains("No question showing")));
        assert!(s.iter().any(|l| l.contains("› hi there")));
        assert!(matches!(
            b.key(Key::new(KeyCode::Enter, Mods::NONE), &look),
            Step::Close(Outcome::Picked(p)) if p.value == "hi there"
        ));
    }

    #[test]
    fn slash_searches_the_options_each_keeping_its_number() {
        let look = Look::default();
        let mut b = AnswerBox::new("Answer 2 fix".into(), options_in(PERMISSION), None);
        for c in "/tell".chars() {
            assert_eq!(
                b.key(Key::new(KeyCode::Char(c), Mods::NONE), &look),
                Step::Redraw
            );
        }
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 24));
        b.draw(&mut buf, &look);
        let s = crate::ui::draw::tests::screen(&buf);
        assert!(s.iter().any(|l| l.contains("› tell▏")), "{s:#?}");
        assert!(s.iter().any(|l| l.contains("3  No, and tell Claude")));
        assert!(!s.iter().any(|l| l.contains("don't ask again")));
        assert!(matches!(
            b.key(Key::new(KeyCode::Enter, Mods::NONE), &look),
            Step::Close(Outcome::Picked(p)) if p.value == "3"
        ));
        let mut b = AnswerBox::new("Answer 2 fix".into(), options_in(PERMISSION), None);
        b.key(Key::new(KeyCode::Char('/'), Mods::NONE), &look);
        b.key(Key::new(KeyCode::Down, Mods::NONE), &look);
        b.key(Key::new(KeyCode::Down, Mods::NONE), &look);
        let mut short = Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 11));
        b.draw(&mut short, &look);
        let s = crate::ui::draw::tests::screen(&short);
        assert!(
            s.iter().any(|l| l.contains("3  No, and tell Claude")),
            "the chosen match in view on a short screen: {s:#?}"
        );
        let long = Prompt {
            question: (0..20)
                .map(|i| format!("line {i}"))
                .collect::<Vec<_>>()
                .join("\n"),
            ..options_in(PERMISSION).unwrap()
        };
        let mut b = AnswerBox::new("Answer 2 fix".into(), Some(long), None);
        b.key(Key::new(KeyCode::Char('/'), Mods::NONE), &look);
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 24));
        b.draw(&mut buf, &look);
        let s = crate::ui::draw::tests::screen(&buf);
        assert!(
            s.iter().any(|l| l.contains("1  Yes")),
            "a long question leaves a match: {s:#?}"
        );
        let mut b = AnswerBox::new("Answer 2 fix".into(), options_in(PERMISSION), None);
        for c in "/yes".chars() {
            b.key(Key::new(KeyCode::Char(c), Mods::NONE), &look);
        }
        b.key(Key::new(KeyCode::Char('n'), Mods::CTRL), &look);
        assert!(
            matches!(
                b.key(Key::new(KeyCode::Enter, Mods::NONE), &look),
                Step::Close(Outcome::Picked(p)) if p.value == "2"
            ),
            "Ctrl-N moves to the second match"
        );
        let mut b = AnswerBox::new("Answer 2 fix".into(), options_in(PERMISSION), None);
        b.key(Key::new(KeyCode::Char('/'), Mods::NONE), &look);
        assert_eq!(
            b.key(Key::new(KeyCode::Esc, Mods::NONE), &look),
            Step::Redraw
        );
        assert_eq!(
            b.key(Key::new(KeyCode::Esc, Mods::NONE), &look),
            Step::Close(Outcome::Dismissed)
        );
    }
}
