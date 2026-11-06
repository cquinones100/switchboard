//! The box an option of mission control's menus shows while it works
//! (render.js's `busyModal`): the option's label with a spinner turning
//! beside it, until what it does has done, since one that takes a moment (a
//! browser coming to the front, Linear answering) would otherwise look as
//! if nothing happened. It takes no keys and no clicks, so nothing acts
//! unseen beneath it, and the server closes it when the work answers.
//!
//! The spinner turns by the clock from when it opened, so it needs nothing
//! from the server but to be drawn again.

use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::style::Style;
use ratatui::text::Span;

use crate::mux::input::{Key, Mouse};
use crate::sidebar::text::{SPINNER, SPINNER_MS};
use crate::ui::draw::{self, BoxLine, BoxSpec};
use crate::ui::{Look, Overlay, Step};

pub struct Busy {
    text: String,
    since: Instant,
}

impl Busy {
    pub fn new(text: impl Into<String>) -> Busy {
        Busy {
            text: text.into(),
            since: Instant::now(),
        }
    }

    fn frame(&self) -> &'static str {
        let turns = self.since.elapsed().as_millis() / u128::from(SPINNER_MS);
        SPINNER[turns as usize % SPINNER.len()]
    }
}

impl Overlay for Busy {
    fn kind(&self) -> &'static str {
        "busy"
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let lines = [BoxLine::spans(vec![
            Span::styled(format!(" {} ", self.frame()), Style::new().fg(draw::YELLOW)),
            Span::styled(self.text.clone(), look.bright()),
        ])];
        let mut spec = BoxSpec::new("One moment", &lines);
        spec.keys = Some("it closes itself when done");
        spec.closes = false;
        draw::draw_box(buf, &spec, look);
    }

    fn key(&mut self, _key: Key, _look: &Look) -> Step {
        Step::Same
    }

    fn mouse(&mut self, _mouse: Mouse, _look: &Look) -> Step {
        Step::Same
    }
}
