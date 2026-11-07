//! The server's side of the overlays: opening one on a client, handing it
//! the client's input, telling its opener how it ended, closing the ones
//! whose time is up, and asking the client's terminal for every mouse
//! movement while one is up and hover_highlight is on (the server's own
//! 1003, which the client turns off again as it leaves). On top of that,
//! what Switchboard's scripts did with popups: `say` (say.sh), `tip`
//! (tip.sh), asking before M-w closes a busy pane (confirm.sh), M-p's
//! palette (palette.sh) and the list of keys, and the `say` and `tip`
//! commands.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use super::draw;
use super::palette::{self, Entry};
use super::picker::Order;
use super::{
    Confirm, Layer, Look, Menu, Message, Outcome, Overlay, PickItem, Picked, Picker, Step, Then,
    Tip, Ui, UiEvent, keylist, message, tip,
};
use crate::mux::PaneId;
use crate::mux::input::{Input, Parsed};
use crate::mux::proto::ServerMsg;
use crate::mux::server::{ClientId, Event, Reply, Server};
use crate::{paths, settings, util};

/// How long the status line shows a message with message_boxes off: tmux's
/// `display-time`, whose default tmux.conf kept.
const DISPLAY_TIME: Duration = Duration::from_millis(750);
/// How long after a pick from the palette its key's tip comes, so it shows
/// over what the command opened rather than before it (tip.sh's `-d 1`).
const KEY_TIP_DELAY: Duration = Duration::from_secs(1);

/// Whether an on/off option is on, read from the file now: these are read
/// on rare events (a box opening, a pane closing), so a change made by hand
/// applies at once.
fn setting_on(key: &str) -> bool {
    settings::setting(key) != Some(serde_json::Value::Bool(false))
}

impl Ui {
    /// The tips shown, read from their file the first time.
    fn seen(&mut self) -> &mut BTreeSet<String> {
        self.tips_seen.get_or_insert_with(tip::read_seen)
    }
}

impl Server {
    // ── Overlays ─────────────────────────────────────────────────────────────

    /// Puts an overlay over a client's screen, above any already up; `then`
    /// is told how it ends.
    pub fn open(&mut self, client: ClientId, overlay: Box<dyn Overlay>, then: Option<Then>) {
        let Some(c) = self.clients.get_mut(&client) else {
            return;
        };
        c.overlays.look = Look::read();
        c.overlays.layers.push(Layer { overlay, then });
        self.overlays_changed(client);
    }

    /// Asks a question; `then` is told whether it was answered yes.
    pub fn ask(
        &mut self,
        client: ClientId,
        question: Confirm,
        then: impl FnOnce(&mut Server, ClientId, bool) + Send + 'static,
    ) {
        let then: Then = Box::new(move |s, c, o| then(s, c, o == Outcome::Yes));
        self.open(client, Box::new(question), Some(then));
    }

    /// Shows a menu; `then` is told the option chosen, by its place, or
    /// `None` when it was closed without one.
    pub fn choose(
        &mut self,
        client: ClientId,
        menu: Menu,
        then: impl FnOnce(&mut Server, ClientId, Option<usize>) + Send + 'static,
    ) {
        let then: Then = Box::new(move |s, c, o| {
            let chose = match o {
                Outcome::Chose(i) => Some(i),
                _ => None,
            };
            then(s, c, chose)
        });
        self.open(client, Box::new(menu), Some(then));
    }

    /// Shows a list to pick from; `then` is told the item picked, or `None`.
    pub fn pick(
        &mut self,
        client: ClientId,
        picker: Picker,
        then: impl FnOnce(&mut Server, ClientId, Option<Picked>) + Send + 'static,
    ) {
        let then: Then = Box::new(move |s, c, o| {
            let picked = match o {
                Outcome::Picked(p) => Some(p),
                _ => None,
            };
            then(s, c, picked)
        });
        self.open(client, Box::new(picker), Some(then));
    }

    /// As `pick`, for a list whose rows can be marked with Tab and taken
    /// together (`Picker::multi`): `then` is told the rows marked, or the
    /// one chosen when none is, or nothing.
    pub fn pick_many(
        &mut self,
        client: ClientId,
        picker: Picker,
        then: impl FnOnce(&mut Server, ClientId, Vec<Picked>) + Send + 'static,
    ) {
        let then: Then = Box::new(move |s, c, o| {
            let picked = match o {
                Outcome::Picked(p) => vec![p],
                Outcome::PickedMany(many) => many,
                _ => Vec::new(),
            };
            then(s, c, picked)
        });
        self.open(client, Box::new(picker), Some(then));
    }

    /// As `pick`, for a list that does a second thing with a row
    /// (`Picker::also`): `then` is told the item, and whether it was taken
    /// with Ctrl-O for that.
    pub fn pick_also(
        &mut self,
        client: ClientId,
        picker: Picker,
        then: impl FnOnce(&mut Server, ClientId, Option<(Picked, bool)>) + Send + 'static,
    ) {
        let then: Then = Box::new(move |s, c, o| {
            let picked = match o {
                Outcome::Picked(p) => Some((p, false)),
                Outcome::PickedAlso(p) => Some((p, true)),
                _ => None,
            };
            then(s, c, picked)
        });
        self.open(client, Box::new(picker), Some(then));
    }

    /// Closes every overlay of a kind on a client, each opener told it was
    /// dismissed.
    pub fn close_overlays(&mut self, client: ClientId, kind: &str) {
        let Some(c) = self.clients.get_mut(&client) else {
            return;
        };
        let (gone, kept): (Vec<Layer>, Vec<Layer>) = std::mem::take(&mut c.overlays.layers)
            .into_iter()
            .partition(|l| l.overlay.kind() == kind);
        c.overlays.layers = kept;
        for layer in gone {
            self.finished(client, layer, Outcome::Dismissed);
        }
        self.overlays_changed(client);
    }

    /// Hands a client's input to its overlays: the top one takes keys, the
    /// mouse and pastes, and one that closes on a key can pass it on, to the
    /// one under it or, past the last, to the tab. Gives back the input
    /// nothing took, for the tab.
    pub fn overlay_input(&mut self, id: ClientId, parsed: Parsed) -> Option<Parsed> {
        if !matches!(
            parsed.input,
            Input::Key(_) | Input::Mouse(_) | Input::Paste(_)
        ) {
            return Some(parsed);
        }
        loop {
            let c = self.clients.get_mut(&id)?;
            let look = c.overlays.look;
            let Some(top) = c.overlays.layers.last_mut() else {
                return Some(parsed);
            };
            let step = match &parsed.input {
                Input::Key(k) => top.overlay.key(*k, &look),
                Input::Mouse(m) => top.overlay.mouse(*m, &look),
                Input::Paste(text) => top.overlay.paste(text, &look),
                _ => Step::Same,
            };
            match step {
                Step::Same => return None,
                Step::Redraw => {
                    c.dirty = true;
                    return None;
                }
                Step::Close(outcome) => {
                    self.finish_top(id, outcome);
                    return None;
                }
                Step::Pass => self.finish_top(id, Outcome::Dismissed),
            }
        }
    }

    fn finish_top(&mut self, id: ClientId, outcome: Outcome) {
        if let Some(layer) = self
            .clients
            .get_mut(&id)
            .and_then(|c| c.overlays.layers.pop())
        {
            self.finished(id, layer, outcome);
        }
        self.overlays_changed(id);
    }

    /// An overlay taken off the stack: its opener is told how it ended.
    fn finished(&mut self, id: ClientId, layer: Layer, outcome: Outcome) {
        if let Some(then) = layer.then {
            then(self, id, outcome);
        }
    }

    /// After overlays open or close: the client is drawn again, its terminal
    /// reports every mouse movement while one is up (for hover), and the
    /// soonest that closes by itself is timed.
    fn overlays_changed(&mut self, id: ClientId) {
        let Some(c) = self.clients.get_mut(&id) else {
            return;
        };
        c.dirty = true;
        let deadline = c.overlays.deadline();
        self.sync_motion(id);
        if let Some(at) = deadline {
            let after = at.saturating_duration_since(Instant::now()) + Duration::from_millis(10);
            self.send_later(after, Event::Ui(UiEvent::Due(id)));
        }
    }

    /// Asks a client's terminal for every mouse movement while something
    /// lights what is under the mouse (an overlay with hover_highlight, or
    /// the sidebar's rows), and stops asking when nothing does. Turning
    /// 1003 off leaves a terminal reporting no mouse at all, so the presses
    /// and drags the client asked for (1002) are asked for again with it.
    pub fn sync_motion(&mut self, id: ClientId) {
        let sidebar = self.sidebar_wants_motion(id);
        let Some(c) = self.clients.get_mut(&id) else {
            return;
        };
        let motion = (!c.overlays.layers.is_empty() && c.overlays.look.hover) || sidebar;
        if motion != c.overlays.motion {
            c.overlays.motion = motion;
            let mode: &[u8] = if motion {
                b"\x1b[?1003h"
            } else {
                b"\x1b[?1003l\x1b[?1002h"
            };
            let _ = c.tx.send(ServerMsg::Output(mode.to_vec()));
        }
    }

    /// What the overlays' timers say.
    pub fn ui_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::Due(id) => self.overlays_due(id),
            // Only on the client the pick was made on: one that has gone
            // (the pick was Detach) gets none, and nobody else does.
            UiEvent::Tip { name, key, client } => {
                if self.clients.contains_key(&client) {
                    self.show_tip(&name, key.as_deref(), Some(client));
                }
            }
        }
    }

    /// Closes a client's overlays whose time is up, and the status line's
    /// message once it has been shown long enough.
    fn overlays_due(&mut self, id: ClientId) {
        let now = Instant::now();
        let Some(c) = self.clients.get_mut(&id) else {
            return;
        };
        if c.overlays.note.as_ref().is_some_and(|(_, at)| *at <= now) {
            c.overlays.note = None;
            c.dirty = true;
        }
        let (due, kept): (Vec<Layer>, Vec<Layer>) = std::mem::take(&mut c.overlays.layers)
            .into_iter()
            .partition(|l| l.overlay.deadline().is_some_and(|d| d <= now));
        c.overlays.layers = kept;
        if due.is_empty() {
            return;
        }
        for layer in due {
            self.finished(id, layer, Outcome::Dismissed);
        }
        self.overlays_changed(id);
    }

    /// The client acting now in a session: its most recently active one.
    pub fn latest_client_of(&self, session: &str) -> Option<ClientId> {
        self.clients
            .values()
            .filter(|c| c.session == session)
            .max_by_key(|c| c.activity)
            .map(|c| c.id)
    }

    /// The client a command from a pane speaks to: the one used last on the
    /// pane's session, else the one used last anywhere.
    fn client_for(&self, caller: Option<PaneId>) -> Option<ClientId> {
        caller
            .and_then(|p| self.panes.get(&p))
            .and_then(|slot| self.session_of_tab(slot.tab))
            .and_then(|s| self.latest_client_of(&s.name))
            .or_else(|| self.latest_client())
    }

    // ── Messages ─────────────────────────────────────────────────────────────

    /// Says something in a box in the middle of a client's screen (the one
    /// used last when `None`), which any key closes and which closes itself
    /// after five seconds; with message_boxes off, on the status line for a
    /// moment, as tmux's message was.
    pub fn say(&mut self, client: Option<ClientId>, text: &str) {
        self.say_for(client, text, message::SECONDS);
    }

    /// `say`, up for this many seconds.
    pub fn say_for(&mut self, client: Option<ClientId>, text: &str, seconds: u64) {
        if text.trim().is_empty() {
            return;
        }
        let Some(id) = client
            .filter(|c| self.clients.contains_key(c))
            .or_else(|| self.latest_client())
        else {
            return;
        };
        if !setting_on("message_boxes") {
            if let Some(c) = self.clients.get_mut(&id) {
                let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
                c.overlays.note = Some((line, Instant::now() + DISPLAY_TIME));
            }
            self.overlays_changed(id);
            return;
        }
        // One message at a time: a new one takes the place of one still up.
        self.close_overlays(id, "message");
        self.open(id, Box::new(Message::new(text, seconds)), None);
    }

    // ── Tips ─────────────────────────────────────────────────────────────────

    /// Shows a tip on a client (the one used last when `None`), unless it
    /// has been shown, the tips setting is off, or a tip is up there already,
    /// in which case it is left for another time. Gives whether it showed.
    pub fn tip(&mut self, name: &str, client: Option<ClientId>) -> bool {
        self.show_tip(name, None, client)
    }

    fn show_tip(&mut self, name: &str, key: Option<&str>, client: Option<ClientId>) -> bool {
        if self.tip_was_seen(name) {
            return false;
        }
        let Some(tip) = Tip::new(name, key) else {
            return false;
        };
        if !setting_on("tips") {
            return false;
        }
        let Some(id) = client
            .filter(|c| self.clients.contains_key(c))
            .or_else(|| self.latest_client())
        else {
            return false;
        };
        if self.clients[&id].overlays.kinds().contains(&"tip") {
            return false;
        }
        self.tip_seen(name);
        self.open(id, Box::new(tip), None);
        true
    }

    /// Whether a tip has been shown.
    pub fn tip_was_seen(&mut self, name: &str) -> bool {
        self.ui.seen().contains(name)
    }

    /// Marks a tip shown without showing it, as doing what it teaches does.
    pub fn tip_seen(&mut self, name: &str) {
        if self.ui.seen().insert(name.to_string()) {
            util::append_quietly(tip::seen_file(), format!("{name}\n"));
        }
    }

    /// Every tip shows again, once more ("Show the tips again" in M-p).
    pub fn reset_tips(&mut self) {
        let _ = std::fs::remove_file(tip::seen_file());
        self.ui.tips_seen = Some(BTreeSet::new());
    }

    /// The palette's "next time, press" a moment after a pick that has a
    /// key of its own, once for each key.
    fn key_tip_later(&mut self, client: ClientId, key: &str) {
        let (name, shown) = tip::key_tip(key);
        if self.tip_was_seen(&name) {
            return;
        }
        self.send_later(
            KEY_TIP_DELAY,
            Event::Ui(UiEvent::Tip {
                name,
                key: Some(shown),
                client,
            }),
        );
    }

    /// A tab switch: with five tabs or more, the tip on finding one (the
    /// `session-window-changed` hook's), once.
    pub fn tab_switched(&mut self, session: &str) {
        if self.session(session).is_none_or(|s| s.tabs.len() < 5) || self.tip_was_seen("jump") {
            return;
        }
        if let Some(client) = self.latest_client_of(session) {
            self.tip("jump", Some(client));
        }
    }

    // ── What the keys open ──────────────────────────────────────────────────

    /// The list of keys.
    pub fn list_keys(&mut self, client: ClientId) {
        let rows = self.bindings.list();
        self.open(client, Box::new(keylist::KeyList::new(rows)), None);
    }

    /// M-p: every command, to type in and pick from; the pick runs as its
    /// key would, then is remembered, and its key's tip comes a moment later.
    pub fn open_palette(&mut self, client: ClientId) {
        let all = settings::read_settings();
        let on = |key: &str| {
            all.iter()
                .find(|s| s.key == key)
                .is_none_or(|s| s.value != serde_json::Value::Bool(false))
        };
        let learn = on("palette_learn");
        // The diff's commands only where they can act (src/diff).
        let mut entries = palette::entries(&self.bindings, on("github"), on("linear"));
        let listed = self.diff_palette(client);
        entries.retain(|e| listed(&e.action));
        let entries = palette::recent_first(entries, &palette::read_recent());
        let picks = if learn {
            palette::read_queries()
        } else {
            Vec::new()
        };
        let items: Vec<PickItem> = entries.iter().map(Entry::item).collect();
        let rows: Vec<(String, String)> = items
            .iter()
            .map(|i| (i.value.clone(), i.haystack()))
            .collect();
        let order: Order = Box::new(move |query, matched| {
            let sub: Vec<(String, String)> = matched.iter().map(|&i| rows[i].clone()).collect();
            palette::rank(query, &sub, &picks)
                .into_iter()
                .map(|j| matched[j])
                .collect()
        });
        let picker = Picker::new("Actions", items)
            .named("palette")
            .prompt("action> ")
            .exact()
            .order(order);
        self.pick(client, picker, move |server, client, picked| {
            let Some(picked) = picked else {
                return;
            };
            let Some(entry) = entries.get(picked.index) else {
                return;
            };
            palette::remember(&picked.value, &picked.query, learn);
            server.count_pick(entry);
            server.act(client, entry.action.clone());
            if let Some(key) = entry.keys.first() {
                server.key_tip_later(client, key);
            }
        });
    }

    /// M-w: closes the pane, asking first when its program isn't a shell at
    /// its prompt (confirm.sh's question), unless the confirm_close setting
    /// is off. The pane is the one M-w was pressed in, whatever has the
    /// focus by the time it is answered.
    pub fn close_pane(&mut self, client: ClientId, pane: PaneId) {
        // Notes beside a pane goes back to its own tab instead (notes-open.sh
        // away, src/notes/serve.rs).
        if self.notes_close(pane) {
            return;
        }
        let running = self.running(pane);
        if running.is_none() || !setting_on("confirm_close") {
            self.kill_pane(pane);
            return;
        }
        let list: Vec<String> = running.into_iter().filter(|r| !r.is_empty()).collect();
        self.ask(
            client,
            Confirm::new("Close pane?").list(list),
            move |server, _, yes| {
                if yes {
                    server.kill_pane(pane);
                }
            },
        );
    }

    /// What a pane runs in front, its command line, or `None` when it is a
    /// shell at its prompt or its program has ended. A shell running a
    /// command puts it in a process group of its own in front, so the
    /// group in front being the pane's own process, and a shell, is a
    /// prompt.
    fn running(&self, pane: PaneId) -> Option<String> {
        let slot = self.panes.get(&pane)?;
        if slot.pane.exited().is_some() {
            return None;
        }
        let front = slot.pane.foreground_pgid();
        let command = slot.pane.foreground_command().unwrap_or_default();
        if front.is_some() && front == slot.pane.pid() && is_shell(&command) {
            return None;
        }
        let line = front
            .and_then(crate::mux::procinfo::args_of)
            .map(|args| args.join(" "))
            .filter(|l| !l.trim().is_empty())
            .unwrap_or(command);
        Some(draw::cut(&line, 60))
    }

    // ── Commands ─────────────────────────────────────────────────────────────

    /// `say [-t seconds] [--] text...`: a message on the client used last on
    /// the caller's session.
    pub fn cmd_say(&mut self, args: &[String], caller: Option<PaneId>) -> Reply {
        let mut seconds = message::SECONDS;
        let mut rest = args;
        while let Some(first) = rest.first() {
            match first.as_str() {
                "-t" => {
                    let Some(n) = rest.get(1).and_then(|n| n.parse::<u64>().ok()) else {
                        return Reply::fail("say: -t needs a number of seconds");
                    };
                    seconds = n;
                    rest = &rest[2..];
                }
                "--" => {
                    rest = &rest[1..];
                    break;
                }
                _ => break,
            }
        }
        let text = rest.join(" ");
        if text.trim().is_empty() {
            return Reply::fail("Usage: switchboard say [-t seconds] <text>");
        }
        let client = self.client_for(caller);
        self.say_for(client, &text, seconds);
        Reply::ok("")
    }

    /// `tip <name>` shows a tip (once), `tip seen <name>` marks it shown,
    /// `tip reset` shows every tip again.
    pub fn cmd_tip(&mut self, args: &[String], caller: Option<PaneId>) -> Reply {
        match args {
            [what] if what == "reset" => {
                self.reset_tips();
                Reply::ok("")
            }
            [what, name] if what == "seen" => {
                self.tip_seen(name);
                Reply::ok("")
            }
            [name] => {
                if tip::text_of(name, None).is_none() {
                    return Reply::fail(format!("tip: no tip named {name}"));
                }
                let client = self.client_for(caller);
                self.tip(name, client);
                Reply::ok("")
            }
            _ => Reply::fail(format!(
                "Usage: switchboard tip <name> | seen <name> | reset  (tips kept in {})",
                paths::tilde(tip::seen_file())
            )),
        }
    }
}

/// Whether a program is a shell, by its name (a login shell's has a `-`
/// before it).
fn is_shell(command: &str) -> bool {
    const SHELLS: &[&str] = &[
        "sh", "bash", "zsh", "fish", "dash", "ksh", "mksh", "oksh", "yash", "ash", "tcsh", "csh",
        "nu", "elvish", "xonsh", "pwsh",
    ];
    let name = command.trim_start_matches('-');
    let name = name.rsplit('/').next().unwrap_or(name);
    SHELLS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shells_are_known_by_name() {
        assert!(is_shell("zsh"));
        assert!(is_shell("-zsh"));
        assert!(is_shell("/bin/sh"));
        assert!(!is_shell("sleep"));
        assert!(!is_shell("claude"));
        assert!(!is_shell(""));
    }
}
