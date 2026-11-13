//! A tab's history on the screen (M-h): over the tab's area under its
//! header, the pane's screen at the moment shown and the timeline under it
//! (src/history/timeline.rs). An overlay, so it takes the keys while it is
//! up: h/l move a column through time, `[` and `]` to the turn before and
//! after, c to the next commit, w to the next wait, Esc back to now and,
//! at now, closes it, as the key that opened it does; any other key closes
//! it and goes on to the tab, so typing goes to Claude as before. A click
//! on the lanes shows that moment, and the wheel moves through time.
//!
//! The server keeps what it draws current on each draw
//! (`Server::history_before_draw`): its place, the timeline's model, and
//! the screen at the moment shown, a snapshot kept then or, at now, the
//! pane as it is. The pane itself is never resized for it, so Claude
//! doesn't draw itself again.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::timeline::{self, Model, Move, ROWS};
use crate::mux::compose::Frame;
use crate::mux::input::{Key, KeyCode, Mouse};
use crate::mux::{PaneId, TabId};
use crate::ui::draw::{self, Gesture};
use crate::ui::kit::{self, Tone};
use crate::ui::pointer::Pointer;
use crate::ui::{Look, Outcome, Overlay, Step};

pub struct HistoryView {
    pub tab: TabId,
    pub pane: PaneId,
    /// The tab's area under its header, as the server last found it.
    pub area: Option<Rect>,
    pub model: Model,
    /// The screen at the moment shown; `None` when none was kept from then.
    pub screen: Option<Vec<Vec<Span<'static>>>>,
    /// A header says which moment is shown, so the keys row needn't.
    pub header: bool,
    /// The terminal draws the kit's half circles.
    pub caps: bool,
    /// The moment shown, `None` being now.
    pub shown: Option<i64>,
    /// The keys bound to opening it, which close it too.
    toggles: Vec<Key>,
    /// Where the lanes went, for a click.
    plot: Rect,
    /// Where easy mode's × went, for a click.
    close: Option<Rect>,
}

impl HistoryView {
    pub fn new(tab: TabId, pane: PaneId, toggles: Vec<Key>) -> HistoryView {
        HistoryView {
            tab,
            pane,
            area: None,
            model: Model::default(),
            screen: None,
            header: false,
            caps: false,
            shown: None,
            toggles,
            plot: Rect::default(),
            close: None,
        }
    }

    fn go(&mut self, how: Move) -> Step {
        let cols = self.plot.width.max(1);
        let to = self.model.moved(self.shown, how, cols);
        if to == self.shown {
            return Step::Same;
        }
        self.shown = to;
        Step::Redraw
    }

    /// What the chip under the screen says at the moment shown: how long it
    /// waited on you, and what you chose.
    fn chip(&self) -> Option<String> {
        let at = self.shown?;
        if let Some(a) = self.model.answer_at(at) {
            return Some(format!(
                "waited {} for you · you chose {}",
                timeline::span_words(a.at - a.asked),
                a.chose
            ));
        }
        let (from, to) = self.model.wait_at(at)?;
        Some(if to >= self.model.now {
            format!("waiting on you for {}", timeline::span_words(to - from))
        } else {
            format!("waited {} for you", timeline::span_words(to - from))
        })
    }

    fn draw_screen(&self, frame: &mut Frame, area: Rect, look: &Look) {
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                frame.buf[(x, y)].reset();
            }
        }
        frame.links.clear_area(area);
        let chip = self.chip();
        let rows = usize::from(area.height.saturating_sub(u16::from(chip.is_some())));
        match &self.screen {
            Some(lines) => {
                // The end of the screen, where Claude asks and you type,
                // when the screen was taller than the room here.
                let from = lines.len().saturating_sub(rows);
                for (i, line) in lines[from..].iter().enumerate() {
                    draw::put_line(
                        &mut frame.buf,
                        area.x,
                        area.y + i as u16,
                        area.width,
                        &Line::from(line.clone()),
                    );
                }
            }
            None => {
                let say = "No screen was kept from then: a tab's screens are kept in memory from when the server started";
                draw::put_line(
                    &mut frame.buf,
                    area.x + 1,
                    area.y,
                    area.width.saturating_sub(1),
                    &Line::from(Span::styled(say, look.dim())),
                );
            }
        }
        if let Some(words) = chip {
            let spans = kit::chip(&words, Tone::Accent, self.caps, &look.theme);
            draw::put_line(
                &mut frame.buf,
                area.x + 1,
                area.bottom() - 1,
                area.width.saturating_sub(1),
                &Line::from(spans),
            );
        }
    }
}

impl Overlay for HistoryView {
    fn kind(&self) -> &'static str {
        "history"
    }

    fn draw(&mut self, _buf: &mut Buffer, _look: &Look) {}

    fn draw_frame(&mut self, frame: &mut Frame, look: &Look) {
        let Some(area) = self.area.map(|a| a.intersection(frame.buf.area)) else {
            return;
        };
        if area.height < ROWS || area.width == 0 {
            return;
        }
        let screen = Rect {
            height: area.height - ROWS,
            ..area
        };
        if screen.height > 0 {
            self.draw_screen(frame, screen, look);
        }
        let lanes = Rect::new(area.x, screen.bottom(), area.width, ROWS);
        self.plot = timeline::draw_timeline(
            frame,
            lanes,
            &self.model,
            self.shown,
            &look.theme,
            !self.header,
        );
        // Easy mode's ×, at the keys row's right end.
        self.close = look.close.then(|| {
            let at = Rect::new(lanes.right().saturating_sub(4), lanes.bottom() - 1, 3, 1);
            draw::put_line(
                &mut frame.buf,
                at.x,
                at.y,
                at.width,
                &Line::from(Span::styled(" × ", look.dim())),
            );
            at
        });
    }

    fn key(&mut self, key: Key, _look: &Look) -> Step {
        if self.toggles.contains(&key) {
            return Step::Close(Outcome::Dismissed);
        }
        let plain = !key.mods.alt && !key.mods.ctrl;
        match key.code {
            KeyCode::Esc if self.shown.is_some() => {
                self.shown = None;
                Step::Redraw
            }
            KeyCode::Esc => Step::Close(Outcome::Dismissed),
            KeyCode::Left if plain => self.go(Move::Back),
            KeyCode::Right if plain => self.go(Move::On),
            KeyCode::Char('h') if plain => self.go(Move::Back),
            KeyCode::Char('l') if plain => self.go(Move::On),
            KeyCode::Char('[') if plain => self.go(Move::PrevTurn),
            KeyCode::Char(']') if plain => self.go(Move::NextTurn),
            KeyCode::Char('c') if plain => self.go(Move::NextCommit),
            KeyCode::Char('w') if plain => self.go(Move::NextWait),
            _ => Step::Pass,
        }
    }

    /// A paste closes it and goes on to the tab, as typing does.
    fn paste(&mut self, _text: &str, _look: &Look) -> Step {
        Step::Pass
    }

    /// A hand over easy mode's × and the timeline a click moves to, the
    /// arrow everywhere else, since a click there only closes it or does
    /// nothing.
    fn pointer(&self, col: u16, row: u16, look: &Look) -> Option<Pointer> {
        let within = |r: Rect| col >= r.x && col < r.right() && row >= r.y && row < r.bottom();
        let pressed = self.close.is_some_and(within) || (self.plot.width > 0 && within(self.plot));
        Some(if pressed && look.mouse {
            Pointer::Hand
        } else {
            Pointer::Arrow
        })
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        match draw::gesture(&mouse) {
            Gesture::WheelUp => self.go(Move::Back),
            Gesture::WheelDown => self.go(Move::On),
            // Easy mode's ×, or the header over it (its History key among
            // what is there), closes it.
            Gesture::Click
                if look.mouse
                    && (self.close.is_some_and(|c| {
                        mouse.col >= c.x && mouse.col < c.right() && mouse.row == c.y
                    }) || self.area.is_some_and(|a| mouse.row < a.y)) =>
            {
                Step::Close(Outcome::Dismissed)
            }
            Gesture::Click
                if look.mouse
                    && self.plot.width > 0
                    && mouse.col >= self.plot.x
                    && mouse.col < self.plot.right()
                    && mouse.row >= self.plot.y
                    && mouse.row < self.plot.bottom() =>
            {
                let per = self.model.per_col(self.plot.width);
                let col = f64::from(mouse.col - self.plot.x) + 0.5;
                let at = self.model.start + (col * per) as i64;
                self.shown = (at < self.model.now - per as i64).then_some(at);
                Step::Redraw
            }
            _ => Step::Same,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::State;
    use crate::history::transcript::{Answer, Lanes};
    use crate::mux::input::Mods;

    fn view() -> HistoryView {
        let mut lanes = Lanes::default();
        lanes.turns = vec![1000, 2200];
        lanes.answers = vec![Answer {
            asked: 2000,
            at: 2360,
            chose: "1. Fix it".into(),
        }];
        let mut v = HistoryView::new(
            TabId(1),
            PaneId(2),
            vec![Key {
                code: KeyCode::Char('h'),
                mods: Mods {
                    alt: true,
                    ..Mods::NONE
                },
            }],
        );
        v.model = Model {
            start: 1000,
            now: 4000,
            states: vec![(1000, Some(State::Working)), (2000, Some(State::Waiting))],
            lanes,
            window: 200_000,
        };
        v.area = Some(Rect::new(0, 0, 112, 30));
        v.screen = Some(vec![
            vec![Span::raw("first")],
            vec![Span::raw("❯ 1. Fix it")],
        ]);
        v
    }

    fn key(c: char) -> Key {
        Key {
            code: KeyCode::Char(c),
            mods: Mods::NONE,
        }
    }

    fn text(frame: &Frame, y: u16) -> String {
        (0..frame.buf.area.width)
            .map(|x| frame.buf[(x, y)].symbol().to_string())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn draws_the_screen_then_and_what_you_chose() {
        let mut v = view();
        v.shown = Some(2100);
        let mut frame = Frame::new(112, 30);
        v.draw_frame(&mut frame, &Look::default());
        assert_eq!(text(&frame, 0), "first");
        assert_eq!(text(&frame, 1), "❯ 1. Fix it");
        let chip = text(&frame, 30 - ROWS - 1);
        assert!(
            chip.contains("waited 6m for you · you chose 1. Fix it"),
            "{chip}"
        );
        assert!(
            text(&frame, 30 - ROWS).starts_with("───"),
            "the timeline under it"
        );
    }

    #[test]
    fn keys_move_through_time_and_esc_comes_back_then_closes() {
        let mut v = view();
        let look = Look::default();
        let mut frame = Frame::new(112, 30);
        v.draw_frame(&mut frame, &look);
        assert_eq!(v.key(key('['), &look), Step::Redraw);
        assert_eq!(v.shown, Some(2200));
        assert_eq!(v.key(key('w'), &look), Step::Redraw);
        assert_eq!(v.shown, Some(2000));
        assert_eq!(v.key(key('h'), &look), Step::Redraw);
        assert_eq!(v.shown, Some(1970));
        let esc = Key {
            code: KeyCode::Esc,
            mods: Mods::NONE,
        };
        assert_eq!(v.key(esc, &look), Step::Redraw);
        assert_eq!(v.shown, None);
        assert_eq!(v.key(esc, &look), Step::Close(Outcome::Dismissed));
        assert_eq!(v.key(key('x'), &look), Step::Pass, "typing goes to the tab");
        assert_eq!(
            v.paste("a prompt", &look),
            Step::Pass,
            "and so does a paste"
        );
        let alt_left = Key {
            code: KeyCode::Left,
            mods: Mods {
                alt: true,
                ..Mods::NONE
            },
        };
        assert_eq!(v.key(alt_left, &look), Step::Pass, "M-Left moves to a pane");
        let toggle = v.toggles[0];
        assert_eq!(v.key(toggle, &look), Step::Close(Outcome::Dismissed));
    }

    #[test]
    fn says_so_when_no_screen_was_kept() {
        let mut v = view();
        v.screen = None;
        v.shown = Some(1500);
        let mut frame = Frame::new(112, 30);
        v.draw_frame(&mut frame, &Look::default());
        assert!(text(&frame, 0).contains("No screen was kept from then"));
    }
}

#[cfg(test)]
mod easy_tests {
    use super::*;
    use crate::mux::input::{Mods, MouseButton, MouseKind};

    fn click(col: u16, row: u16) -> Mouse {
        Mouse {
            kind: MouseKind::Press,
            button: MouseButton::Left,
            col,
            row,
            mods: Mods::NONE,
        }
    }

    #[test]
    fn easy_modes_cross_closes_it_and_clicks_follow_menu_mouse() {
        let mut v = HistoryView::new(TabId(1), PaneId(2), Vec::new());
        v.model = Model {
            start: 1000,
            now: 4000,
            window: 200_000,
            ..Model::default()
        };
        v.area = Some(Rect::new(0, 4, 112, 30));
        let look = Look {
            mouse: true,
            close: true,
            ..Look::default()
        };
        let mut frame = Frame::new(112, 34);
        v.draw_frame(&mut frame, &look);
        let cross = v.close.expect("drawn with easy_keys");
        assert_eq!(frame.buf[(cross.x + 1, cross.y)].symbol(), "×");
        let lanes_row = 34 - ROWS + 3;
        let off = Look {
            mouse: false,
            ..look
        };
        assert_eq!(v.mouse(click(60, lanes_row), &off), Step::Same);
        assert_eq!(v.mouse(click(60, lanes_row), &look), Step::Redraw);
        assert_eq!(
            v.mouse(click(30, 2), &look),
            Step::Close(Outcome::Dismissed),
            "the header"
        );
        assert_eq!(
            v.mouse(click(cross.x + 1, cross.y), &look),
            Step::Close(Outcome::Dismissed)
        );
    }
}
