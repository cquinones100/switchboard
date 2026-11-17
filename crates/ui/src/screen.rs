//! A frame sent to a terminal as what changed since the last one, for an
//! app that draws its whole screen itself (the diff, Notes): the multiplexer's own
//! renderer (`switchboard`'s src/mux/compose.rs, `Screen`) without what
//! only it draws, links and pictures, so an app's screen reaches a terminal
//! by the same bytes a pane's does.

use std::io::Write;

use crossterm::{cursor, queue};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::buffer::Buffer;

/// What the terminal was last sent, to send only what changes.
#[derive(Default)]
pub struct Screen {
    prev: Option<Buffer>,
    /// The cursor's shape as the terminal was last told it.
    style: Option<cursor::SetCursorStyle>,
}

impl Screen {
    /// The next frame draws everything again (after a resize, or anything
    /// that may have left the terminal showing something else).
    pub fn invalidate(&mut self) {
        self.prev = None;
        self.style = None;
    }

    /// The bytes that turn the last frame sent into `buf`, the cursor at
    /// `cursor` (hidden when `None`), in the terminal's own shape.
    pub fn render(&mut self, buf: &Buffer, cursor: Option<(u16, u16)>) -> Vec<u8> {
        self.render_shaped(buf, cursor, cursor::SetCursorStyle::DefaultUserShape)
    }

    /// The same with the cursor in `shape`, for an app whose modes each
    /// have one (Notes: a bar while typing).
    pub fn render_shaped(
        &mut self,
        buf: &Buffer,
        cursor: Option<(u16, u16)>,
        shape: cursor::SetCursorStyle,
    ) -> Vec<u8> {
        let mut out = Vec::with_capacity(4096);
        // Synchronized output: the terminal shows the frame once it is whole.
        out.extend_from_slice(b"\x1b[?2026h\x1b[?25l");
        let prev = match self.prev.take() {
            Some(p) if p.area == buf.area => p,
            _ => {
                out.extend_from_slice(b"\x1b[0m\x1b[H\x1b[2J");
                Buffer::empty(buf.area)
            }
        };
        {
            let mut backend = CrosstermBackend::new(&mut out);
            // Writing into a Vec can't fail.
            let _ = backend.draw(prev.diff(buf).into_iter());
        }
        if let Some((x, y)) = cursor {
            if self.style != Some(shape) {
                let _ = queue!(out, shape);
                self.style = Some(shape);
            }
            let _ = queue!(out, cursor::MoveTo(x, y), cursor::Show);
        }
        let _ = out.write_all(b"\x1b[?2026l");
        self.prev = Some(buf.clone());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn a_frame_is_sent_whole_once_and_then_only_what_changed() {
        let mut screen = Screen::default();
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 2));
        buf[(0, 0)].set_symbol("a");
        let first = String::from_utf8_lossy(&screen.render(&buf, None)).into_owned();
        assert!(first.contains("\x1b[2J") && first.contains('a'));
        buf[(1, 0)].set_symbol("b");
        let second = String::from_utf8_lossy(&screen.render(&buf, Some((1, 0)))).into_owned();
        assert!(!second.contains("\x1b[2J") && second.contains('b') && !second.contains('a'));
        assert!(second.contains("\x1b[?25h"));
        screen.invalidate();
        let again = String::from_utf8_lossy(&screen.render(&buf, None)).into_owned();
        assert!(again.contains("\x1b[2J") && again.contains('a'));
    }
}
