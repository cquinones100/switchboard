//! The Slack view's reply box (UX 2.0 board 108), opened from a message's
//! menu with Settings' `slack_reply` on: where the reply goes in its title
//! and first line, the message it answers quoted, a box to type in, and
//! Send and Cancel as buttons. What is typed is a multi-line `ui::field`,
//! as every box's typing is (the cursor keys, wrapping, undo): Enter on the
//! text is Send, and S-Enter and M-Enter a new line; Send is a Tab stop
//! too, pressed with Enter or Space, or a click; Esc cancels. Nothing is
//! sent from here alone:
//! Send closes it with what was typed (`Outcome::Picked`, its query the
//! text, as typed), and the server asks before sending (board 109), coming
//! back here with the text kept on a no, and on a send that failed, saying
//! why in the box. New with UX 2.0, with no
//! tmux-version file behind it.

use ratatui::buffer::Buffer;
use ratatui::text::Span;

use crate::day::slack_reply::Reply;
use crate::mux::input::{Key, KeyCode, Mods, Mouse};
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use crate::ui::field::{Edit, Field};
use crate::ui::kit::{self, Press, Surface, Tone};
use crate::ui::{Look, Outcome, Overlay, Picked, Step};

/// The box's inside, while the screen leaves room around it.
const WIDTH: usize = 80;
/// The rows the text takes.
const TEXT_ROWS: usize = 4;
/// The lines of the message it answers shown at most.
const QUOTE_ROWS: usize = 2;

/// The keys on the box's last row.
const KEYS: &str = "↵ send  S-↵ new line  esc cancel";

/// What Tab's focus or the mouse is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Text,
    Send,
    Cancel,
    Close,
}

/// A part on a line: the line's index, its first and last column, the part.
type Placed = (usize, u16, u16, Part);

pub struct ReplyBox {
    reply: Reply,
    title: String,
    field: Field,
    /// The box's lines that hold the field's rows.
    field_lines: std::ops::Range<usize>,
    /// Why the last Send didn't send, when it didn't.
    failed: Option<String>,
    focus: Part,
    hover: Option<Part>,
    drawn: Option<Drawn>,
    parts: Vec<Placed>,
    /// Where it opens: the middle, or beside the click that opened it.
    place: draw::Place,
}

impl ReplyBox {
    /// The box for `reply`, with `text` typed to start with (what a no to
    /// Send's question brings back).
    pub fn new(reply: Reply, text: &str) -> ReplyBox {
        ReplyBox {
            title: reply.title(),
            reply,
            field: Field::new().multi_line().keep_tabs().with_text(text),
            field_lines: 0..0,
            failed: None,
            focus: Part::Text,
            hover: None,
            drawn: None,
            parts: Vec::new(),
            place: draw::Place::Center,
        }
    }

    /// The box again after a Send that failed, saying why.
    pub fn failed(mut self, why: Option<String>) -> ReplyBox {
        self.failed = why;
        self
    }

    fn stops(look: &Look) -> Vec<Part> {
        let mut out = vec![Part::Text, Part::Send, Part::Cancel];
        if look.close {
            out.push(Part::Close);
        }
        out
    }

    fn step_focus(&mut self, by: isize, look: &Look) {
        let stops = Self::stops(look);
        let at = stops.iter().position(|p| *p == self.focus).unwrap_or(0) as isize;
        self.focus = stops[(at + by).rem_euclid(stops.len() as isize) as usize];
    }

    fn press(&mut self, part: Part) -> Step {
        match part {
            Part::Text => {
                self.focus = Part::Text;
                Step::Redraw
            }
            // Nothing typed, nothing to send: the box stays.
            Part::Send if self.field.text().trim().is_empty() => {
                self.focus = Part::Text;
                Step::Redraw
            }
            Part::Send => Step::Close(Outcome::Picked(Picked {
                index: 0,
                value: String::new(),
                query: self.field.text().to_string(),
            })),
            Part::Cancel | Part::Close => Step::Close(Outcome::Dismissed),
        }
    }

    fn lines(&mut self, inner: usize, look: &Look) -> (Vec<BoxLine>, Vec<Placed>) {
        let theme = &look.theme;
        let mut lines: Vec<BoxLine> = Vec::new();
        let mut parts = Vec::new();
        // Where it goes, the conversation bold.
        let to = self.reply.to_line();
        let place = &self.reply.place;
        let mut head = vec![Span::raw(" ")];
        match to.split_once(place.as_str()) {
            Some((before, after)) => {
                head.push(Span::styled(before.to_string(), Tone::Dim.style(theme)));
                head.push(Span::styled(place.clone(), Tone::Text.bold(theme)));
                head.push(Span::styled(after.to_string(), Tone::Dim.style(theme)));
            }
            None => head.push(Span::styled(to, Tone::Dim.style(theme))),
        }
        lines.push(BoxLine::spans(kit::fit(head, inner, Default::default())));
        lines.push(BoxLine::blank());
        // The message it answers, quoted under a faint bar.
        let who = &self.reply.starter;
        let room = inner.saturating_sub(4);
        let said = self
            .reply
            .text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let quoted = draw::wrap(&format!("{who}  {said}"), room);
        let shown = quoted.len().min(QUOTE_ROWS);
        for (i, line) in quoted.iter().take(shown).enumerate() {
            let mut line = line.clone();
            if i + 1 == shown && quoted.len() > shown {
                line = draw::cut(&format!("{line} …"), room);
            }
            let mut spans = vec![Span::styled(" ▎ ", Tone::Faint.style(theme))];
            match line.strip_prefix(who.as_str()).filter(|_| i == 0) {
                Some(rest) => {
                    spans.push(Span::styled(who.clone(), Tone::Text.bold(theme)));
                    spans.push(Span::styled(rest.to_string(), Tone::Dim.style(theme)));
                }
                None => spans.push(Span::styled(line, Tone::Dim.style(theme))),
            }
            lines.push(BoxLine::spans(spans));
        }
        lines.push(BoxLine::blank());
        // What you type, on the panel, its last rows shown with the cursor.
        let area = inner.saturating_sub(4);
        let panel = Surface::Panel.color(theme);
        let typing = self.focus == Part::Text;
        let on_panel = |tone: Tone| tone.style(theme).bg(panel);
        let room = area.saturating_sub(2);
        let rows = self.field.view(
            room,
            TEXT_ROWS,
            on_panel(Tone::Text),
            on_panel(Tone::Accent),
            typing,
        );
        let first = lines.len();
        for i in 0..TEXT_ROWS {
            let mut spans = vec![Span::raw(" "), Span::styled(" ", on_panel(Tone::Text))];
            if let Some(row) = rows.get(i) {
                spans.extend(row.iter().cloned());
            }
            if self.field.is_empty() && i == 0 {
                spans.push(Span::styled("Your reply", on_panel(Tone::Faint)));
            }
            let used = kit::spans_width(&spans) - 1;
            spans.push(Span::styled(
                " ".repeat(area.saturating_sub(used)),
                on_panel(Tone::Text),
            ));
            parts.push((lines.len(), 1, area as u16, Part::Text));
            lines.push(BoxLine::spans(spans).clickable());
        }
        self.field_lines = first..lines.len();
        lines.push(BoxLine::blank());
        if let Some(why) = &self.failed {
            for line in draw::wrap(why, inner.saturating_sub(2)) {
                lines.push(BoxLine::text(format!(" {line}"), Tone::Failed.style(theme)));
            }
        }
        // As whom, then Cancel and Send at the right.
        let press = |part: Part| {
            if self.focus == part {
                Press::Lit
            } else if self.hover == Some(part) {
                Press::Hover
            } else {
                Press::Rest
            }
        };
        let cancel = kit::button(" Cancel ", None, press(Part::Cancel), false, theme);
        let send = kit::button(" Send ", None, press(Part::Send), false, theme);
        let (cw, sw) = (kit::spans_width(&cancel), kit::spans_width(&send));
        let note = " as you, from your Slack sign-in";
        let gap = inner.saturating_sub(draw::width(note) + cw + sw + 3);
        let mut row = vec![
            Span::styled(note, Tone::Faint.style(theme)),
            Span::raw(" ".repeat(gap)),
        ];
        let cancel_at = kit::spans_width(&row);
        row.extend(cancel);
        row.push(Span::raw(" "));
        let send_at = kit::spans_width(&row);
        row.extend(send);
        parts.push((
            lines.len(),
            cancel_at as u16,
            (cancel_at + cw) as u16,
            Part::Cancel,
        ));
        parts.push((
            lines.len(),
            send_at as u16,
            (send_at + sw) as u16,
            Part::Send,
        ));
        lines.push(BoxLine::spans(row).clickable());
        (lines, parts)
    }
}

impl Overlay for ReplyBox {
    fn drawn(&self) -> Option<&Drawn> {
        self.drawn.as_ref()
    }

    fn kind(&self) -> &'static str {
        "slack-reply"
    }

    fn stands_out(&self) -> Option<ratatui::layout::Rect> {
        self.drawn.as_ref().map(|d| d.outer)
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let inner = (buf.area.width as usize).saturating_sub(6).min(WIDTH);
        let (lines, parts) = self.lines(inner, look);
        self.parts = parts;
        let mut spec = BoxSpec::new(&self.title, &lines);
        spec.keys = Some(KEYS);
        spec.width = inner as u16;
        spec.place = self.place;
        spec.close_lit = self.focus == Part::Close || self.hover == Some(Part::Close);
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn open_near(&mut self, (x, y): (u16, u16)) {
        self.place = draw::Place::Near { x, y };
    }

    fn key(&mut self, key: Key, look: &Look) -> Step {
        let k = key.normalized();
        match k.code {
            KeyCode::Esc => return Step::Close(Outcome::Dismissed),
            KeyCode::Char('c') if k.mods == Mods::CTRL => return Step::Close(Outcome::Dismissed),
            KeyCode::Tab => {
                self.step_focus(1, look);
                return Step::Redraw;
            }
            KeyCode::BackTab => {
                self.step_focus(-1, look);
                return Step::Redraw;
            }
            KeyCode::Enter if self.focus != Part::Text => return self.press(self.focus),
            KeyCode::Char(' ') if self.focus != Part::Text => return self.press(self.focus),
            _ => {}
        }
        // Typing on a button types in the text.
        self.focus = Part::Text;
        match self.field.key(k) {
            // Enter on the text is Send; S-Enter and M-Enter are the field's
            // new lines.
            Edit::Not if k.code == KeyCode::Enter => self.press(Part::Send),
            Edit::Changed | Edit::Moved => Step::Redraw,
            Edit::Same | Edit::Up | Edit::Down | Edit::Not => Step::Same,
        }
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        let Some(drawn) = &self.drawn else {
            return Step::Same;
        };
        let hit = drawn.hit(mouse.col, mouse.row);
        let part = match hit {
            Hit::Close => Some(Part::Close),
            Hit::Line(line, x) => self
                .parts
                .iter()
                .find(|(l, x0, x1, _)| *l == line && x >= *x0 && x < *x1)
                .map(|p| p.3),
            _ => None,
        };
        match draw::gesture(&mouse) {
            // A click outside keeps what was typed rather than losing it.
            Gesture::Click if look.mouse => match (hit, part) {
                (Hit::Line(line, x), Some(Part::Text)) if self.field_lines.contains(&line) => {
                    self.focus = Part::Text;
                    self.field.click(
                        line - self.field_lines.start,
                        (x as usize).saturating_sub(2),
                    );
                    Step::Redraw
                }
                (_, part) => part.map_or(Step::Same, |p| self.press(p)),
            },
            Gesture::Move if look.hover => {
                if part == self.hover {
                    Step::Same
                } else {
                    self.hover = part;
                    Step::Redraw
                }
            }
            _ => Step::Same,
        }
    }

    fn paste(&mut self, text: &str, _look: &Look) -> Step {
        self.focus = Part::Text;
        self.field.paste(text);
        Step::Redraw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply() -> Reply {
        Reply {
            channel: "C1".into(),
            thread: Some("1700000000.000100".into()),
            place: "#web-app".into(),
            dm: false,
            starter: "Ana".into(),
            text: "can you look at the RSVP menu?".into(),
            replies: Some(3),
            shown: None,
        }
    }

    fn key(code: KeyCode) -> Key {
        Key::new(code, Mods::NONE)
    }

    fn text_of(b: &mut ReplyBox) -> String {
        let (lines, _) = b.lines(WIDTH, &Look::default());
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn it_names_where_it_goes_and_quotes_the_message() {
        let mut b = ReplyBox::new(reply(), "");
        assert_eq!(b.title, "Reply in Ana's thread · #web-app");
        let text = text_of(&mut b);
        assert!(
            text.contains("To #web-app, in the thread Ana started · 3 replies"),
            "{text}"
        );
        assert!(
            text.contains("Ana  can you look at the RSVP menu?"),
            "{text}"
        );
        assert!(text.contains("Your reply"), "{text}");
        assert!(text.contains("Send"), "{text}");
        assert!(text.contains("Cancel"), "{text}");
    }

    #[test]
    fn a_send_that_failed_comes_back_with_the_text_and_why() {
        let mut b = ReplyBox::new(reply(), "On it")
            .failed(Some("Couldn't send it: Slack isn't connected".into()));
        let text = text_of(&mut b);
        assert!(text.contains("On it"), "{text}");
        assert!(
            text.contains("Couldn't send it: Slack isn't connected"),
            "{text}"
        );
    }

    #[test]
    fn enter_on_the_text_sends_it_as_typed_and_nothing_when_empty() {
        let look = Look::default();
        let mut b = ReplyBox::new(reply(), "");
        // Nothing typed, or only blanks: the box stays, with nothing added.
        assert_eq!(b.key(key(KeyCode::Enter), &look), Step::Redraw);
        typed(&mut b, &chars("  "));
        assert_eq!(b.key(key(KeyCode::Enter), &look), Step::Redraw);
        assert_eq!(b.field.text(), "  ");
        let mut b = ReplyBox::new(reply(), "");
        typed(&mut b, &chars("On it"));
        typed(&mut b, &[Key::new(KeyCode::Enter, Mods::SHIFT)]);
        typed(&mut b, &chars("before 3:00 "));
        match b.key(key(KeyCode::Enter), &look) {
            Step::Close(Outcome::Picked(p)) => assert_eq!(p.query, "On it\nbefore 3:00 "),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_keys_line_says_enter_sends_and_s_enter_is_a_new_line() {
        assert_eq!(KEYS, "↵ send  S-↵ new line  esc cancel");
    }

    #[test]
    fn tab_reaches_send_and_cancel_and_esc_cancels() {
        let look = Look::default();
        let mut b = ReplyBox::new(reply(), "");
        // Send with nothing typed stays.
        b.key(key(KeyCode::Tab), &look);
        assert_eq!(b.key(key(KeyCode::Enter), &look), Step::Redraw);
        typed(&mut b, &chars("On it"));
        b.key(key(KeyCode::Tab), &look);
        match b.key(key(KeyCode::Enter), &look) {
            Step::Close(Outcome::Picked(p)) => assert_eq!(p.query, "On it"),
            other => panic!("{other:?}"),
        }
        let mut b = ReplyBox::new(reply(), "kept");
        b.key(key(KeyCode::Tab), &look);
        b.key(key(KeyCode::Tab), &look);
        assert_eq!(
            b.key(key(KeyCode::Enter), &look),
            Step::Close(Outcome::Dismissed)
        );
        assert_eq!(
            ReplyBox::new(reply(), "x").key(key(KeyCode::Esc), &look),
            Step::Close(Outcome::Dismissed)
        );
    }

    fn typed(b: &mut ReplyBox, keys: &[Key]) {
        let look = Look::default();
        for k in keys {
            b.key(*k, &look);
        }
    }

    fn chars(text: &str) -> Vec<Key> {
        text.chars().map(|c| key(KeyCode::Char(c))).collect()
    }

    fn query(b: &mut ReplyBox) -> String {
        let look = Look::default();
        b.key(key(KeyCode::Tab), &look);
        match b.key(key(KeyCode::Enter), &look) {
            Step::Close(Outcome::Picked(p)) => p.query,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn s_enter_and_m_enter_are_new_lines_and_the_keys_after_them_land() {
        for new_line in [
            Key::new(KeyCode::Enter, Mods::SHIFT),
            // S-Enter from a terminal without kitty's keyboard protocol.
            Key::new(KeyCode::Char('\r'), Mods::ALT),
            Key::new(KeyCode::Enter, Mods::ALT),
        ] {
            let mut b = ReplyBox::new(reply(), "");
            typed(&mut b, &chars("On it"));
            typed(&mut b, &[new_line]);
            typed(&mut b, &chars("- the menu"));
            typed(&mut b, &[new_line]);
            typed(&mut b, &chars("- the refresh"));
            let text = text_of(&mut b);
            assert!(text.contains("- the menu"), "{text}");
            assert!(text.contains("- the refresh"), "{text}");
            assert_eq!(query(&mut b), "On it\n- the menu\n- the refresh");
        }
    }

    #[test]
    fn the_cursor_moves_and_undo_takes_back_a_word_as_in_every_box() {
        let mut b = ReplyBox::new(reply(), "");
        typed(&mut b, &chars("On it today"));
        typed(&mut b, &[Key::new(KeyCode::Left, Mods::ALT)]);
        typed(&mut b, &chars("later "));
        assert_eq!(b.field.text(), "On it later today");
        typed(&mut b, &[Key::new(KeyCode::Char('z'), Mods::CTRL)]);
        assert_eq!(b.field.text(), "On it today");
        typed(&mut b, &[key(KeyCode::Home)]);
        typed(&mut b, &chars("Yes, "));
        assert_eq!(query(&mut b), "Yes, On it today");
    }

    #[test]
    fn a_paste_keeps_its_lines_and_tabs_as_sent() {
        let look = Look::default();
        let mut b = ReplyBox::new(reply(), "");
        b.paste("a\tb\r\nc\x1b", &look);
        assert_eq!(query(&mut b), "a\tb\nc");
    }

    #[test]
    fn a_long_reply_wraps_and_keeps_the_cursor_in_sight() {
        let mut b = ReplyBox::new(reply(), "");
        let words = "word ".repeat(100);
        typed(&mut b, &chars(words.trim_end()));
        typed(&mut b, &chars(" END"));
        let text = text_of(&mut b);
        assert!(
            text.contains("END▏"),
            "the last row shown, with the cursor: {text}"
        );
        let rows = text.lines().filter(|l| l.contains("word")).count();
        assert_eq!(rows, TEXT_ROWS, "{text}");
        assert!(
            text.lines().all(|l| draw::width(l) <= WIDTH),
            "nothing past the box: {text}"
        );
    }
}
