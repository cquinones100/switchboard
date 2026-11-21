//! The server's side of src/status: the tick that draws the headers again
//! for their ages, each terminal asked for its
//! background as it attaches, and M-p's "Toggle light and dark theme"
//! (settings.sh's `toggle-theme`).

use std::sync::atomic::Ordering;

use serde_json::Value;
use tokio::sync::mpsc;

use super::theme;
use crate::mux::proto::ServerMsg;
use crate::mux::server::{ClientId, Server};
use crate::{settings, util};

/// The minute the headers were last drawn again for their ages.
static HEADER_MINUTE: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

impl Server {
    /// On the server's tick: each minute every client drawn again for the
    /// headers' ages.
    pub fn status_tick(&mut self) {
        // The header's ages ("opened 1h12m") move by the minute, with
        // nothing else to draw it again while the sidebar is hidden.
        let minute = util::now_secs() / 60;
        if settings::is_on("tab_age") && HEADER_MINUTE.swap(minute, Ordering::Relaxed) != minute {
            self.dirty_all();
        }
    }

    /// Every client draws again.
    fn dirty_all(&mut self) {
        for c in self.clients.values_mut() {
            c.dirty = true;
        }
    }

    /// Asks a terminal that has just attached for its colours: its
    /// background's for the `system` theme (`theme::QUERY`), and the rest
    /// for the panes' programs (src/mux/terminal.rs); and, where pictures
    /// reach it as sixel (src/mux/graphics.rs, by what it says it is,
    /// `term`), its cell's size in pixels (src/mux/sixel).
    pub fn ask_background(tx: &mpsc::UnboundedSender<ServerMsg>, term: &str) {
        let _ = tx.send(ServerMsg::Output(crate::mux::terminal::queries()));
        // And "what are you" (DA1), whose answer says whether it draws
        // sixel (src/mux/graphics.rs, `Server::terminal_answered`).
        let _ = tx.send(ServerMsg::Output(b"\x1b[c".to_vec()));
        if crate::mux::graphics::for_panes(term) == crate::mux::graphics::Protocol::Sixel {
            let _ = tx.send(ServerMsg::Output(crate::mux::sixel::CELL_QUERY.to_vec()));
        }
    }

    /// A terminal's answer about its background, among its keys: kept, and
    /// every client drawn again when it changes what `system` shows. Gives
    /// whether the bytes were that answer, which no pane is given. A
    /// terminal is asked only as it attaches, before `terminal_colors` sets
    /// its colors, so the answer is always its own.
    pub fn status_reply(&mut self, raw: &[u8]) -> bool {
        let Some(light) = theme::read_reply(raw) else {
            return false;
        };
        if theme::set_system(light)
            && settings::cached("theme") == Some(Value::String("system".into()))
        {
            self.terminal_colors_sync();
            self.dirty_all();
        }
        true
    }

    /// Sets each attached terminal's own colors to the dark theme's, or puts
    /// its own back, where that differs from what it has
    /// (`theme::sets_terminal_colors`): as a terminal attaches, and when the
    /// setting or the theme changes. The panes are told the colors it shows
    /// (`Server::pane_colors`), so a program asking for them is answered
    /// with those, without asking the terminal again, whose answers the
    /// `system` theme reads.
    pub fn terminal_colors_sync(&mut self) {
        let set = theme::sets_terminal_colors();
        let colors = self.pane_colors();
        for slot in self.panes.values_mut() {
            slot.pane.set_colors(colors);
        }
        for c in self.clients.values_mut() {
            if c.terminal_colors == set {
                continue;
            }
            c.terminal_colors = set;
            let bytes = if set {
                theme::SET_COLORS
            } else {
                theme::PUT_COLORS_BACK
            };
            let _ = c.tx.send(ServerMsg::Output(bytes.to_vec()));
        }
    }

    /// M-p's "Toggle light and dark theme": the theme setting set to the
    /// other of what is drawn now.
    pub fn toggle_theme(&mut self, client: ClientId) {
        match settings::write_setting("theme", Value::from(theme::toggled())) {
            Ok(()) => {
                settings::reload_settings();
                self.terminal_colors_sync();
                self.dirty_all();
            }
            Err(e) => self.say(Some(client), &format!("Couldn't change the theme: {e}")),
        }
    }
}
