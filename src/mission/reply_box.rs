//! The Slack view's reply box (UX 2.0 board 108), opened from a message's
//! menu with Settings' `slack_reply` on: where the reply goes in its title
//! and first line, the message it answers quoted, a box to type in, and
//! Send and Cancel as buttons, drawn by `mission::text_box`. What is typed is a multi-line `ui::field`,
//! as every box's typing is (the cursor keys, wrapping, undo): Enter on the
//! text is Send, and S-Enter and M-Enter a new line; Send is a Tab stop
//! too, pressed with Enter or Space, or a click; Esc cancels. Nothing is
//! sent from here alone:
//! Send closes it with what was typed (`Outcome::Picked`, its query the
//! text, as typed), and the server makes a draft of it in Slack (board
//! 109) or, with Settings' `slack_send_as` at once, asks before sending,
//! coming back here with the text kept on a no, and on a draft or a send
//! that failed, saying why in the box for a send. Its last row says which
//! of the two Send does. New, with no
//! tmux-version file behind it.

use ratatui::buffer::Buffer;
use ratatui::text::Span;

use super::text_box::TextBox;
use crate::day::slack_reply::Reply;
use crate::mux::input::{Key, Mouse};
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn};
use crate::ui::field::Field;
use crate::ui::kit::{self, Tone};
use crate::ui::{Look, Overlay, Step};

/// The box's inside, while the screen leaves room around it.
const WIDTH: usize = 80;
/// The lines of the message it answers shown at most.
const QUOTE_ROWS: usize = 2;

/// The keys on the box's last row.
const KEYS: &str = "↵ send  S-↵ new line  esc cancel";

pub struct ReplyBox {
    reply: Reply,
    title: String,
    text: TextBox,
    /// The rows the text may grow to, from the screen's height.
    most: usize,
    /// Why the last Send didn't send, when it didn't.
    failed: Option<String>,
    /// Send makes a draft in Slack, rather than sending.
    draft: bool,
    drawn: Option<Drawn>,
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
            text: TextBox::new(
                Field::new().multi_line().keep_tabs().with_text(text),
                "Your reply",
                "Send",
            ),
            most: super::text_box::LEAST_ROWS,
            failed: None,
            draft: false,
            drawn: None,
            place: draw::Place::Center,
        }
    }

    /// The box again after a Send that failed, saying why.
    pub fn failed(mut self, why: Option<String>) -> ReplyBox {
        self.failed = why;
        self
    }

    /// The box whose Send makes a draft in Slack (`draft`), or sends.
    pub fn as_draft(mut self, draft: bool) -> ReplyBox {
        self.draft = draft;
        self
    }

    fn lines(&mut self, inner: usize, look: &Look) -> Vec<BoxLine> {
        let theme = &look.theme;
        let mut lines: Vec<BoxLine> = Vec::new();
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
        self.text.text_lines(&mut lines, inner, self.most, look);
        if let Some(why) = &self.failed {
            for line in draw::wrap(why, inner.saturating_sub(2)) {
                lines.push(BoxLine::text(format!(" {line}"), Tone::Failed.style(theme)));
            }
        }
        // As whom, then Cancel and Send at the right.
        let note = if self.draft {
            " as a draft in Slack, for you to send there"
        } else {
            " as you, from your Slack sign-in"
        };
        self.text.buttons_line(&mut lines, inner, note, look);
        lines
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
        // Its own lines around the text: the head and the quote, the blank
        // lines, the buttons, and the box's border, title and keys.
        let around = 4 + QUOTE_ROWS + 6 + usize::from(self.failed.is_some());
        self.most = TextBox::most_rows(buf.area.height as usize, around);
        let lines = self.lines(inner, look);
        let mut spec = BoxSpec::new(&self.title, &lines);
        spec.keys = Some(KEYS);
        spec.width = inner as u16;
        spec.place = self.place;
        spec.close_lit = self.text.close_lit();
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn open_near(&mut self, (x, y): (u16, u16)) {
        self.place = draw::Place::Near { x, y };
    }

    fn key(&mut self, key: Key, look: &Look) -> Step {
        self.text.key(key, look)
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        self.text.mouse(mouse, self.drawn.as_ref(), look)
    }

    fn paste(&mut self, text: &str, _look: &Look) -> Step {
        self.text.paste(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::input::{KeyCode, Mods};
    use crate::ui::Outcome;

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
            link: None,
        }
    }

    fn text_of(b: &mut ReplyBox) -> String {
        let lines = b.lines(WIDTH, &Look::default());
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
    fn its_last_row_says_whether_send_makes_a_draft() {
        let text = text_of(&mut ReplyBox::new(reply(), "").as_draft(true));
        assert!(
            text.contains("as a draft in Slack, for you to send there"),
            "{text}"
        );
        let text = text_of(&mut ReplyBox::new(reply(), ""));
        assert!(text.contains("as you, from your Slack sign-in"), "{text}");
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
        assert_eq!(b.key(Key::plain(KeyCode::Enter), &look), Step::Redraw);
        typed(&mut b, &chars("  "));
        assert_eq!(b.key(Key::plain(KeyCode::Enter), &look), Step::Redraw);
        assert_eq!(b.text.field.text(), "  ");
        let mut b = ReplyBox::new(reply(), "");
        typed(&mut b, &chars("On it"));
        typed(&mut b, &[Key::new(KeyCode::Enter, Mods::SHIFT)]);
        typed(&mut b, &chars("before 3:00 "));
        match b.key(Key::plain(KeyCode::Enter), &look) {
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
        b.key(Key::plain(KeyCode::Tab), &look);
        assert_eq!(b.key(Key::plain(KeyCode::Enter), &look), Step::Redraw);
        typed(&mut b, &chars("On it"));
        b.key(Key::plain(KeyCode::Tab), &look);
        match b.key(Key::plain(KeyCode::Enter), &look) {
            Step::Close(Outcome::Picked(p)) => assert_eq!(p.query, "On it"),
            other => panic!("{other:?}"),
        }
        let mut b = ReplyBox::new(reply(), "kept");
        b.key(Key::plain(KeyCode::Tab), &look);
        b.key(Key::plain(KeyCode::Tab), &look);
        assert_eq!(
            b.key(Key::plain(KeyCode::Enter), &look),
            Step::Close(Outcome::Dismissed)
        );
        assert_eq!(
            ReplyBox::new(reply(), "x").key(Key::plain(KeyCode::Esc), &look),
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
        text.chars().map(|c| Key::plain(KeyCode::Char(c))).collect()
    }

    fn query(b: &mut ReplyBox) -> String {
        let look = Look::default();
        b.key(Key::plain(KeyCode::Tab), &look);
        match b.key(Key::plain(KeyCode::Enter), &look) {
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
        assert_eq!(b.text.field.text(), "On it later today");
        typed(&mut b, &[Key::new(KeyCode::Char('z'), Mods::CTRL)]);
        assert_eq!(b.text.field.text(), "On it today");
        typed(&mut b, &[Key::plain(KeyCode::Home)]);
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
        assert_eq!(rows, super::super::text_box::LEAST_ROWS, "{text}");
        assert!(
            text.lines().all(|l| draw::width(l) <= WIDTH),
            "nothing past the box: {text}"
        );
    }
}
