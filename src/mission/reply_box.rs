//! The Slack view's reply box (UX 2.0 board 108), opened from a message's
//! menu with Settings' `slack_reply` on: where the reply goes in its title
//! and first line, the message it answers quoted, a box to type in, and
//! Send and Cancel as buttons. Enter is a new line; Send is Tab then Enter
//! (or Space), or a click; Esc cancels. Nothing is sent from here alone:
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
use crate::ui::kit::{self, Press, Surface, Tone};
use crate::ui::{Look, Outcome, Overlay, Picked, Step};

/// The box's inside, while the screen leaves room around it.
const WIDTH: usize = 80;
/// The rows the text takes.
const TEXT_ROWS: usize = 4;
/// The lines of the message it answers shown at most.
const QUOTE_ROWS: usize = 2;

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
    text: String,
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
            text: text.to_string(),
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
            Part::Send if self.text.trim().is_empty() => {
                self.focus = Part::Text;
                Step::Redraw
            }
            Part::Send => Step::Close(Outcome::Picked(Picked {
                index: 0,
                value: String::new(),
                query: self.text.clone(),
            })),
            Part::Cancel | Part::Close => Step::Close(Outcome::Dismissed),
        }
    }

    fn lines(&self, inner: usize, look: &Look) -> (Vec<BoxLine>, Vec<Placed>) {
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
        let mut rows = draw::wrap(&self.text, area.saturating_sub(2));
        let skip = rows.len().saturating_sub(TEXT_ROWS);
        rows.drain(..skip);
        let last = rows.len() - 1;
        for i in 0..TEXT_ROWS {
            let mut spans = vec![Span::raw(" "), Span::styled(" ", on_panel(Tone::Text))];
            let mut used = 1;
            if let Some(r) = rows.get(i) {
                spans.push(Span::styled(r.clone(), on_panel(Tone::Text)));
                used += draw::width(r);
            }
            // The cursor after the text.
            if typing && i == last {
                spans.push(Span::styled("▏", on_panel(Tone::Accent)));
                used += 1;
            }
            if self.text.is_empty() && i == 0 {
                spans.push(Span::styled("Your reply", on_panel(Tone::Faint)));
                used += draw::width("Your reply");
            }
            spans.push(Span::styled(
                " ".repeat(area.saturating_sub(used)),
                on_panel(Tone::Text),
            ));
            parts.push((lines.len(), 1, area as u16, Part::Text));
            lines.push(BoxLine::spans(spans).clickable());
        }
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
        spec.keys = Some("↵ new line  tab Send, Cancel  esc cancel");
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
        match k.code {
            KeyCode::Enter => {
                self.text.push('\n');
                Step::Redraw
            }
            KeyCode::Backspace => {
                self.text.pop();
                Step::Redraw
            }
            KeyCode::Char('u') if k.mods == Mods::CTRL => {
                // The line being typed, as a shell's C-u.
                let keep = self.text.rfind('\n').map_or(0, |i| i + 1);
                self.text.truncate(keep);
                Step::Redraw
            }
            KeyCode::Char(c) if !k.mods.ctrl && !k.mods.alt => {
                self.text.push(c);
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
            Gesture::Click if look.mouse => part.map_or(Step::Same, |p| self.press(p)),
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
        self.text
            .push_str(&text.replace("\r\n", "\n").replace('\r', "\n"));
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

    fn text_of(b: &ReplyBox) -> String {
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
        let b = ReplyBox::new(reply(), "");
        assert_eq!(b.title, "Reply in Ana's thread · #web-app");
        let text = text_of(&b);
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
        let b = ReplyBox::new(reply(), "On it")
            .failed(Some("Couldn't send it: Slack isn't connected".into()));
        let text = text_of(&b);
        assert!(text.contains("On it"), "{text}");
        assert!(
            text.contains("Couldn't send it: Slack isn't connected"),
            "{text}"
        );
    }

    #[test]
    fn enter_is_a_new_line_and_send_is_tab_then_enter_with_the_text_as_typed() {
        let look = Look::default();
        let mut b = ReplyBox::new(reply(), "");
        // Send with nothing typed stays.
        b.key(key(KeyCode::Tab), &look);
        assert_eq!(b.key(key(KeyCode::Enter), &look), Step::Redraw);
        for c in "On it".chars() {
            b.key(key(KeyCode::Char(c)), &look);
        }
        assert_eq!(b.key(key(KeyCode::Enter), &look), Step::Redraw);
        for c in "before 3:00 ".chars() {
            b.key(key(KeyCode::Char(c)), &look);
        }
        assert!(text_of(&b).contains("before 3:00"));
        b.key(key(KeyCode::Tab), &look);
        match b.key(key(KeyCode::Enter), &look) {
            Step::Close(Outcome::Picked(p)) => assert_eq!(p.query, "On it\nbefore 3:00 "),
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
}
