//! The notes app in the workspace, on the server: what tmux/notes-open.sh,
//! tmux/notes-away.sh, tmux/note-tab.sh, tmux/note-pick.sh and the Notes
//! parts of tmux/new-tab.zsh did, and the bindings tmux.conf gave them. There
//! is only ever one Notes, since two apps saving the same notes as you type
//! would write over each other.
//!
//! M-N goes to Notes wherever it is open, or opens it in a tab of its own,
//! named Notes. M-e moves that same pane beside the pane you are in, its own
//! tab closing behind it, or, pressed beside it in that tab, back to a tab
//! of its own; with none open, it starts one there. Leaving its own tab, it
//! leaves a pane in its place (`switchboard notes away`, `away`) saying which
//! tab has it, so the tab stays in the sidebar and says where Notes went;
//! going home, it takes that pane's place. Beside another tab's pane the
//! app's own list is put away, where the note wants the room, and shown again
//! at home (the notes_hide_sidebar setting). M-w on Notes beside a pane sends
//! it home too, rather than closing it. Left alone in a tab not its own when
//! that tab's last other pane closes, it goes home and the tab closes, only
//! in a tab M-e marked `@notes_beside`.
//!
//! The notes pane is found by its `@notes_pane` mark, which the server sets
//! on the panes it starts Notes in, so it is found wherever it has been
//! moved; one started by hand is found from the pid in the notes' lock
//! (`.notes.lock`) among the panes' programs, and marked then. A tab of its
//! own carries `@notes`.
//!
//! Each tab shows its own note, the first whose file is there
//! (`by_precedence`, with the note_by_precedence setting on): the one you
//! linked (`@tab_note_by_you`, set with `@tab_note` by the picker and a
//! note's tab), an autopilot tab's item's plan (`@tab_plan_note`, found
//! once off the server's task, `find_tab_plan`), the note autopilot is
//! working through; then the one its Claude works from (`@tab_note`,
//! which /note-watch and /queue-and-clean set once a note is settled), else
//! the one Notes last showed beside it (`@tab_note_last`, kept as Notes
//! leaves, but only once the tab has a note or another was opened while
//! beside it: a tab with none that Notes merely passed through, still
//! showing the last tab's note, would otherwise take that note as its own;
//! the app records a note opened there as soon as it is, dropping the
//! arrival marker `@tab_note_arrived`). Both are paths in the notes
//! directory. M-e opens it in the running app through the socket it listens
//! on (src/notes/socket.rs), or, for an app not yet running, by making it
//! the note the app opens with (`.notes.json`'s last). A tab with neither is
//! asked which note is its (`pick`), with the ask_tab_note setting on, until
//! it is told to keep what is open (`@tab_note_asked`).
//!
//! A note's own tab (`tab`): Notes' `t` and M-p's "Open the tab for the note
//! in Notes" go to the tab linked to the note, or ask which tab is its, a
//! new one among them.
//!
//! The app's palette asks for the workspace's commands by name (`switchboard
//! press <name> [arg]`, `cmd_press`), as the tmux version's pressed their
//! keys.
//!
//! Requests to the app go through one thread of their own, in order, so a
//! key never waits on the app answering.

use std::path::PathBuf;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, OnceLock};

use super::store::{self, Store};
use crate::mux::keys::Action;
use crate::mux::server::{ClientId, Event, Reply, Server};
use crate::mux::{Direction, PaneId, TabId};
use crate::{paths, settings, util};

pub mod away;
mod pick;
mod tab;

pub use pick::rank;
pub use tab::new_tab_context;

/// What Notes' keys and its palette's commands do in the server, an action
/// of the multiplexer's (`mux::keys::Action::Notes`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    /// M-N: to Notes, or Notes in a tab of its own.
    Open,
    /// M-e: Notes beside this pane, or back in its own tab.
    Beside,
    /// The tab for the note Notes shows (or, from `press`, the note named).
    NoteTab,
    /// Which note is this tab's, asked whatever it has.
    ChooseNote,
}

impl Command {
    pub const ALL: [Command; 4] = [
        Command::Open,
        Command::Beside,
        Command::NoteTab,
        Command::ChooseNote,
    ];

    /// Its name in keys.json, and what the app presses.
    pub fn name(self) -> &'static str {
        match self {
            Command::Open => "notes",
            Command::Beside => "notes-beside",
            Command::NoteTab => "note-tab",
            Command::ChooseNote => "choose-tab-note",
        }
    }

    pub fn parse(name: &str) -> Option<Command> {
        Command::ALL.into_iter().find(|c| c.name() == name)
    }

    /// What it does, as tmux.conf described its binding.
    pub fn about(self) -> &'static str {
        match self {
            Command::Open => "Go to Notes, wherever it is open, or open it in its own tab",
            Command::Beside => "Open Notes in a pane to the right, or put it back in its own tab",
            Command::NoteTab => {
                "Open the tab for the note in Notes: the one linked to it, or a new one"
            }
            Command::ChooseNote => "Choose this tab's note, the one M-e shows beside it",
        }
    }

    /// More on what it does than `about`, for the palette's preview of the chosen command.
    pub fn detail(self) -> &'static str {
        match self {
            Command::Open => {
                "There is only one Notes, since two would save the same notes over each other, so it is found wherever it has moved. A new one opens at the end of the tabs, named Notes."
            }
            Command::Beside => {
                "The one Notes moves there, showing this tab's own note, and its own tab says which tab has it. Its list is put away beside another tab (Settings' \"Hide the sidebar beside a tab\")."
            }
            Command::NoteTab => {
                "With no tab linked, it asks which open tab is the note's, or opens a new one in a directory you pick. Notes then moves beside that tab's Claude (Settings' \"Notes beside a note's tab\")."
            }
            Command::ChooseNote => {
                "The notes are ranked on the computer, those naming the tab's ticket or PR first, then those sharing words with its name. \"Keep whatever is open\" stops the question for this tab."
            }
        }
    }

    /// tmux.conf's tags.
    pub fn tags(self) -> &'static [&'static str] {
        match self {
            Command::Open => &["notes", "navigate"],
            Command::Beside => &["notes", "panes"],
            Command::NoteTab => &["notes", "tabs", "new"],
            Command::ChooseNote => &["notes", "tabs"],
        }
    }

    /// The key tmux.conf bound it to, or `None` for one only in the palette.
    pub fn key(self) -> Option<&'static str> {
        match self {
            Command::Open => Some("M-N"),
            Command::Beside => Some("M-e"),
            _ => None,
        }
    }
}

/// What Notes presses to start work on a checklist item (`gw`), with the
/// note and the item's line.
pub const WORK_ON_ITEM: &str = "work-on-item";

/// What Notes presses for M-Enter in "Add an item": a tab whose Claude adds
/// the item to the note and writes its plan, with `/plan-item`.
pub const PLAN_ITEM: &str = "plan-item";

/// The rest of what a key does, back on the server's task once what it read
/// off the task (the notes, for ranking them) is in.
pub struct NotesEvent(Box<dyn FnOnce(&mut Server) + Send>);

/// A pane option or tab option that is set: not empty and not "0".
fn is_on(value: Option<&String>) -> bool {
    value.is_some_and(|v| !v.is_empty() && v != "0")
}

/// A path that stays in the notes directory.
pub fn in_notes(rel: &str) -> bool {
    !rel.is_empty() && !rel.starts_with('/') && !rel.split('/').any(|p| p == "..")
}

/// The note Notes shows, as it records it: `.notes.json`'s last.
pub fn shown() -> Option<String> {
    let meta = util::read_json_object(store::notes_dir().join(store::META));
    meta.get("last")
        .and_then(|v| v.as_str())
        .filter(|rel| in_notes(rel))
        .map(str::to_string)
}

/// The program that runs Notes in a pane: this binary's `notes`, or
/// `$NOTES_OPEN_COMMAND`, a shell command, when it is set (notes-open.sh let
/// the tests put a stand-in there); with `NOTES_SIDEBAR` set when `sidebar`
/// says whether the app starts with its list.
fn notes_command(sidebar: Option<bool>) -> Vec<String> {
    let mut command = Vec::new();
    if let Some(on) = sidebar {
        command.push("env".to_string());
        command.push(format!("NOTES_SIDEBAR={}", if on { "on" } else { "off" }));
    }
    match std::env::var("NOTES_OPEN_COMMAND")
        .ok()
        .filter(|c| !c.is_empty())
    {
        Some(line) => command.extend(["sh".to_string(), "-c".into(), line]),
        None => command.extend([exe(), "notes".into()]),
    }
    command
}

/// This binary, for the programs Notes' panes run.
fn exe() -> String {
    util::exe()
        .ok()
        .filter(|p| p.exists())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "switchboard".into())
}

/// Sends a request to the running app, after those sent before it, on a
/// thread of its own: an app that isn't running, or doesn't answer, holds
/// up nothing.
/// Marks the note you linked to a tab yourself (the picker, Notes' `t`,
/// M-p's "Choose the tab's note"), set with `@tab_note`; what the workspace
/// links leaves it alone.
pub const BY_YOU: &str = "@tab_note_by_you";

/// The plan note of an autopilot tab's item, a path in the notes directory,
/// found once off the server's task (`find_tab_plan`); empty while it is
/// looked for, or when the item links none.
pub const PLAN_NOTE: &str = "@tab_plan_note";

/// Why a tab's note is the one M-e shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// The note you linked to the tab.
    Linked,
    /// The plan of the autopilot tab's item.
    Plan,
    /// The note autopilot is working through.
    Autopilot,
    /// The tab's note, else the one shown there last, as before precedence.
    Tab,
}

impl Why {
    /// What Notes says as it shows the note, so a note shown by precedence
    /// isn't taken for one you chose.
    fn said(self) -> Option<&'static str> {
        match self {
            Why::Linked => Some("The note linked to this tab"),
            Why::Plan => Some("This tab's plan"),
            Why::Autopilot => Some("Autopilot's note"),
            Why::Tab => None,
        }
    }
}

/// A tab's note, from its options (`opt`), the note autopilot is working
/// through (`working`, while it is on) and whether a note's file is
/// there: with `precedence` (Settings' note_by_precedence), the note you
/// linked, else an autopilot tab's plan, else autopilot's note, else the
/// tab's note or the one shown beside it last; without it, those last two
/// alone. The first whose file is there.
pub fn by_precedence(
    opt: impl Fn(&str) -> Option<String>,
    working: Option<&str>,
    precedence: bool,
    there: impl Fn(&str) -> bool,
) -> Option<(String, Why)> {
    let autopilot_tab = opt(crate::autopilot::OPENED).is_some_and(|v| v != "0");
    let mut order: Vec<(Option<String>, Why)> = Vec::new();
    if precedence {
        order.push((opt(BY_YOU), Why::Linked));
        if autopilot_tab {
            order.push((opt(PLAN_NOTE), Why::Plan));
        }
        order.push((working.map(str::to_string), Why::Autopilot));
    }
    order.push((opt("@tab_note"), Why::Tab));
    order.push((opt("@tab_note_last"), Why::Tab));
    order
        .into_iter()
        .filter_map(|(note, why)| Some((note?, why)))
        .find(|(rel, _)| in_notes(rel) && there(rel))
}

pub(crate) fn tell(request: String) {
    static QUEUE: OnceLock<Mutex<Sender<String>>> = OnceLock::new();
    let queue = QUEUE.get_or_init(|| {
        let (tx, rx) = channel::<String>();
        std::thread::spawn(move || {
            while let Ok(request) = rx.recv() {
                let _ = super::socket::send(&super::socket::path(), &request);
            }
        });
        Mutex::new(tx)
    });
    // It fails only once the thread has gone, which leaves no one to tell.
    let _ = queue
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .send(request);
}

impl Server {
    /// A Notes command, from its key or the palette, `pane` the one it was
    /// pressed in.
    pub fn notes_command(&mut self, client: ClientId, command: Command, pane: Option<PaneId>) {
        match command {
            Command::Open => self.open_notes(client),
            Command::Beside => {
                if let Some(here) = pane {
                    self.notes_beside(Some(client), here);
                }
            }
            Command::NoteTab => self.note_tab(client, None),
            Command::ChooseNote => {
                if let Some(tab) = pane.and_then(|p| self.panes.get(&p)).map(|s| s.tab) {
                    self.ask_tab_note(tab, Some(client));
                }
            }
        }
    }

    /// The rest of a key's work, back from off the task.
    pub fn notes_event(&mut self, event: NotesEvent) {
        (event.0)(self);
    }

    /// Runs `work` off the server's task and `then` with what it gives,
    /// back on it.
    pub(crate) fn notes_off_task<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
        then: impl FnOnce(&mut Server, T) + Send + 'static,
    ) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let found = work();
            let _ = tx.send(Event::Notes(NotesEvent(Box::new(move |server| {
                then(server, found)
            }))));
        });
    }

    // ── Options ──────────────────────────────────────────────────────────────

    pub(crate) fn tab_opt(&self, tab: TabId, name: &str) -> Option<String> {
        self.tabs
            .get(&tab)
            .and_then(|t| t.opts.get(name))
            .filter(|v| !v.is_empty())
            .cloned()
    }

    pub(crate) fn set_tab_opt(&mut self, tab: TabId, name: &str, value: Option<&str>) {
        if let Some(t) = self.tabs.get_mut(&tab) {
            match value {
                Some(v) => t.opts.insert(name.to_string(), v.to_string()),
                None => t.opts.remove(name),
            };
        }
        self.dirty_everywhere(tab);
    }

    fn set_pane_opt(&mut self, pane: PaneId, name: &str, value: Option<&str>) {
        let Some(slot) = self.panes.get_mut(&pane) else {
            return;
        };
        match value {
            Some(v) => slot.opts.insert(name.to_string(), v.to_string()),
            None => slot.opts.remove(name),
        };
        let tab = slot.tab;
        self.dirty_everywhere(tab);
    }

    fn pane_marked(&self, pane: PaneId, name: &str) -> bool {
        self.panes
            .get(&pane)
            .is_some_and(|s| is_on(s.opts.get(name)))
    }

    /// Draws again every client of a tab's session, whichever tab it shows:
    /// the sidebar lists them all.
    pub(crate) fn dirty_everywhere(&mut self, tab: TabId) {
        if let Some(name) = self.session_of_tab(tab).map(|s| s.name.clone()) {
            self.dirty_session(&name);
        }
    }

    fn pane_tab(&self, pane: PaneId) -> Option<TabId> {
        self.panes.get(&pane).map(|s| s.tab)
    }

    /// The panes in a tab's layout.
    fn pane_count(&self, tab: TabId) -> usize {
        self.tabs.get(&tab).map_or(0, |t| t.layout.panes().len())
    }

    // ── Finding Notes ────────────────────────────────────────────────────────

    /// The pane marked as Notes', if any (the lowest, should there be two).
    fn marked_notes_pane(&self) -> Option<PaneId> {
        self.panes
            .iter()
            .filter(|(_, s)| is_on(s.opts.get("@notes_pane")))
            .map(|(p, _)| *p)
            .min()
    }

    /// The notes pane: the one marked, else the pane whose program holds the
    /// notes' lock (an app started by hand), marked then.
    pub fn notes_pane(&mut self) -> Option<PaneId> {
        if let Some(p) = self.marked_notes_pane() {
            return Some(p);
        }
        let lock = store::notes_dir().join(store::LOCK);
        let pid = util::read_text(&lock)
            .and_then(|t| t.trim().parse::<u32>().ok())
            .filter(|pid| util::is_alive(i64::from(*pid)))?;
        let found = self
            .panes
            .iter()
            .find(|(_, s)| s.pane.pid() == Some(pid) || s.pane.foreground_pids().contains(&pid))
            .map(|(p, _)| *p)?;
        self.set_pane_opt(found, "@notes_pane", Some("1"));
        Some(found)
    }

    /// The pane standing in for Notes in its own tab while it is away.
    fn notes_placeholder(&self) -> Option<PaneId> {
        self.panes
            .iter()
            .filter(|(_, s)| is_on(s.opts.get("@notes_placeholder")))
            .map(|(p, _)| *p)
            .min()
    }

    // ── M-N ──────────────────────────────────────────────────────────────────

    /// M-N: to Notes, wherever it is open; else Notes in a tab of its own.
    fn open_notes(&mut self, client: ClientId) {
        if let Some(pane) = self.notes_pane() {
            self.go_to_pane(client, pane);
            return;
        }
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        match self.new_tab(
            &session,
            Some(paths::home()),
            Some(notes_command(None)),
            None,
            crate::mux::server::Opened::Front,
        ) {
            Ok((tab, pane)) => {
                // At the end, as notes-open.sh's new-window took the next
                // free index, so no tab is renumbered.
                self.tab_to_end(tab);
                self.set_pane_opt(pane, "@notes_pane", Some("1"));
                self.make_notes_tab(tab);
            }
            Err(e) => self.say(Some(client), &format!("Couldn't open Notes: {e}")),
        }
    }

    /// A note shown in Notes, and Notes gone to, as notes-open.sh's `note`
    /// did for a new note's notification: opened in the running app through
    /// its socket, or, with none running, made the note the app opens with
    /// (`.notes.json`'s last); then Notes is gone to in its own tab, or
    /// opened in one. Notes beside a tab's work goes home first, as M-e
    /// pressed again there takes it, so that tab keeps its work and its own
    /// note rather than being gone to and shown this one. `rel` is a path in
    /// the notes directory.
    pub(crate) fn open_note(&mut self, client: ClientId, rel: &str) {
        if !in_notes(rel) || !store::notes_dir().join(rel).is_file() {
            self.say(Some(client), &format!("There is no note {rel} any more."));
            return;
        }
        if let Some(notes) = self.notes_pane() {
            let beside = self.pane_tab(notes).filter(|tab| {
                self.tab_opt(*tab, "@notes").as_deref() != Some("1") && self.pane_count(*tab) > 1
            });
            if let Some(tab) = beside {
                // Before the app is told: what it shows now is that tab's.
                self.remember_note(tab);
                self.put_notes_away(notes, false);
            }
            tell(format!("open {rel}"));
        } else {
            Store::at(store::notes_dir()).set_last(Some(rel));
        }
        self.open_notes(client);
    }

    /// Shows a client a pane, in whichever session it is.
    pub(crate) fn go_to_pane(&mut self, client: ClientId, pane: PaneId) {
        let Some(session) = self
            .pane_tab(pane)
            .and_then(|t| self.session_of_tab(t))
            .map(|s| s.name.clone())
        else {
            return;
        };
        self.switch_client(client, &session);
        self.focus_pane(pane);
    }

    /// Shows a client a tab, in whichever session it is.
    pub(crate) fn go_to_tab(&mut self, client: ClientId, tab: TabId) {
        let Some(session) = self.session_of_tab(tab).map(|s| s.name.clone()) else {
            return;
        };
        self.switch_client(client, &session);
        self.select_tab(&session, tab);
    }

    /// Makes a tab Notes' own: marked, and named Notes.
    fn make_notes_tab(&mut self, tab: TabId) {
        self.set_tab_opt(tab, "@notes", Some("1"));
        self.set_tab_opt(tab, "@notes_beside", None);
        self.set_tab_opt(tab, "@tab_name", Some("Notes"));
        self.name_tab(tab);
    }

    // ── M-e ──────────────────────────────────────────────────────────────────

    /// M-e pressed in `here`: Notes brought beside it, or, beside it
    /// already, back to its own tab; with none open, started beside it.
    pub fn notes_beside(&mut self, client: Option<ClientId>, here: PaneId) {
        let Some(here_tab) = self.pane_tab(here) else {
            return;
        };
        let Some(notes) = self.notes_pane() else {
            self.ask_note_if_none(here_tab, client);
            self.open_with_note(here_tab);
            self.note_arrived(here_tab);
            let list = !settings::is_on("notes_hide_sidebar");
            match self.split(
                here,
                Direction::Right,
                Some(paths::home()),
                Some(notes_command(Some(list))),
                Some(40),
                false,
            ) {
                Ok(pane) => {
                    self.set_tab_opt(here_tab, "@notes_beside", Some("1"));
                    self.set_pane_opt(pane, "@notes_pane", Some("1"));
                }
                Err(e) => self.say(client, &format!("Couldn't open Notes: {e}")),
            }
            return;
        };
        let Some(old) = self.pane_tab(notes) else {
            return;
        };
        if old == here_tab
            && here == notes
            && self.tab_opt(here_tab, "@notes").as_deref() == Some("1")
            && settings::is_on("ux2")
        {
            // In Notes' own tab, with UX 2.0, M-e is the app's: the plan of
            // the item under its cursor beside the note, or put away.
            if let Some(slot) = self.panes.get_mut(&notes)
                && let Some(key) = crate::mux::input::Key::parse("M-e")
            {
                let bytes = crate::mux::input::encode_key(&key, &slot.pane.modes());
                let _ = slot.pane.write(&bytes);
            }
            return;
        }
        if old == here_tab {
            // Beside the pane already: home again. Alone in its own tab, it
            // is home.
            self.remember_note(here_tab);
            self.put_notes_away(notes, false);
            return;
        }
        self.remember_note(old);
        // Leaving its own tab (marked, or holding nothing else, as a Notes
        // tab started by hand does), it leaves a pane there saying where it
        // went; leaving another tab, nothing.
        let own = self.tab_opt(old, "@notes").as_deref() == Some("1");
        if (own || self.pane_count(old) == 1) && self.notes_placeholder().is_none() {
            let away = vec![exe(), "notes".into(), "away".into()];
            if let Ok(stand_in) = self.split(
                notes,
                Direction::Right,
                Some(paths::home()),
                Some(away),
                None,
                true,
            ) {
                self.set_pane_opt(stand_in, "@notes_placeholder", Some("1"));
                self.make_notes_tab(old);
            }
        }
        self.ask_note_if_none(here_tab, client);
        self.join_pane(notes, here, Direction::Right, Some(40), true);
        // Brought into its own tab, it takes the stand-in's place there.
        match self
            .notes_placeholder()
            .filter(|s| self.pane_tab(*s) == Some(here_tab))
        {
            Some(stand_in) => {
                self.kill_pane(stand_in);
                self.focus_pane(notes);
                self.notes_list(true);
            }
            None => {
                self.set_tab_opt(here_tab, "@notes_beside", Some("1"));
                self.notes_list(false);
            }
        }
        self.show_note(here_tab);
        self.note_arrived(here_tab);
    }

    /// Moves `pane` from its tab into `target`'s, on its `side`, taking
    /// `percent` of its room, as tmux's `join-pane`; the focus goes with it
    /// when `focus`. Its old tab goes when it was its last pane.
    fn join_pane(
        &mut self,
        pane: PaneId,
        target: PaneId,
        side: Direction,
        percent: Option<u16>,
        focus: bool,
    ) -> bool {
        let Some(to) = self.pane_tab(target) else {
            return false;
        };
        if self.pane_tab(pane) == Some(to) || self.take_out(pane).is_none() {
            return false;
        }
        let Some(tab) = self.tabs.get_mut(&to) else {
            return false;
        };
        tab.zoomed = false;
        tab.layout.split(target, pane, side, percent);
        if let Some(slot) = self.panes.get_mut(&pane) {
            slot.tab = to;
        }
        self.relayout(to);
        self.name_tab(to);
        self.dirty_everywhere(to);
        if focus {
            self.focus_pane(pane);
        }
        true
    }

    /// Puts `pane` where `stand_in` is, the stand-in ended: Notes coming home
    /// to the tab it left. Its old tab goes when it was its last pane.
    fn take_place_of(&mut self, pane: PaneId, stand_in: PaneId) -> bool {
        let Some(to) = self.pane_tab(stand_in) else {
            return false;
        };
        if self.pane_tab(pane) == Some(to) {
            self.kill_pane(stand_in);
            return true;
        }
        if self.take_out(pane).is_none() {
            return false;
        }
        let Some(tab) = self.tabs.get_mut(&to) else {
            return false;
        };
        tab.layout.replace(stand_in, pane);
        if tab.active == stand_in {
            tab.active = pane;
        }
        if tab.last_pane == Some(stand_in) {
            tab.last_pane = None;
        }
        if let Some(slot) = self.panes.get_mut(&pane) {
            slot.tab = to;
        }
        if let Some(mut slot) = self.panes.remove(&stand_in) {
            slot.pane.kill();
        }
        self.relayout(to);
        self.name_tab(to);
        self.dirty_everywhere(to);
        true
    }

    /// Notes back in its own tab, where the pane standing in for it is, else
    /// in a new one, the focus left where it is. Alone in its tab already it
    /// stays, unless `left` there (the orphan rule), when that tab becomes
    /// its own. Gives whether it went.
    fn put_notes_away(&mut self, pane: PaneId, left: bool) -> bool {
        let Some(tab) = self.pane_tab(pane) else {
            return false;
        };
        let alone = self.pane_count(tab) <= 1;
        if alone && !left {
            return false;
        }
        match self.notes_placeholder() {
            Some(stand_in) => {
                if !self.take_place_of(pane, stand_in) {
                    return false;
                }
            }
            None if alone => self.make_notes_tab(tab),
            None => match self.break_pane_to(pane, false) {
                Ok(home) => {
                    self.tab_to_end(home);
                    self.make_notes_tab(home);
                }
                Err(_) => return false,
            },
        }
        self.notes_list(true);
        true
    }

    /// M-w on Notes beside a pane: home, rather than closed; gives whether
    /// it was that. Alone in its own tab, it is closed as any pane is.
    pub fn notes_close(&mut self, pane: PaneId) -> bool {
        if !self.pane_marked(pane, "@notes_pane") {
            return false;
        }
        let Some(tab) = self.pane_tab(pane) else {
            return false;
        };
        self.remember_note(tab);
        self.put_notes_away(pane, false)
    }

    /// A pane of `tab` closed: Notes left alone in it, a tab M-e brought it
    /// beside and not its own, goes home and the tab closes, so it never
    /// holds a tab but Notes'.
    pub fn notes_orphan(&mut self, tab: TabId) {
        let Some(notes) = self.marked_notes_pane() else {
            return;
        };
        if self.pane_tab(notes) != Some(tab)
            || self.tab_opt(tab, "@notes").as_deref() == Some("1")
            || self.tab_opt(tab, "@notes_beside").as_deref() != Some("1")
            || self.pane_count(tab) != 1
        {
            return;
        }
        self.remember_note(tab);
        self.put_notes_away(notes, true);
    }

    // ── Each tab's note ──────────────────────────────────────────────────────

    /// A tab's note, as `note_and_why` picks it.
    pub(crate) fn note_for(&self, tab: TabId) -> Option<String> {
        self.note_and_why(tab).map(|(note, _)| note)
    }

    /// A tab's note and why it is that one (`by_precedence`), from the
    /// tab's options and the autopilot mode the server holds, while its
    /// file is there.
    fn note_and_why(&self, tab: TabId) -> Option<(String, Why)> {
        let dir = store::notes_dir();
        let mode = &self.autopilot.mode;
        let working = (settings::is_on("autopilot") && mode.on && !mode.note.is_empty())
            .then_some(mode.note.as_str());
        by_precedence(
            |name| self.tab_opt(tab, name),
            working,
            settings::is_on("note_by_precedence"),
            |rel| dir.join(rel).is_file(),
        )
    }

    /// An autopilot tab's plan note (`PLAN_NOTE`), looked for once, off the
    /// server's task: the plan the item it works on links in its note
    /// (`@tab_note`, else the note autopilot works through). Empty while it
    /// is looked for and when there is none, so it is looked for only once.
    pub(crate) fn find_tab_plan(&mut self, tab: TabId) {
        let Some(t) = self.tabs.get(&tab) else {
            return;
        };
        if t.opts.contains_key(PLAN_NOTE) || !settings::is_on("note_by_precedence") {
            return;
        }
        let Some(item) = self.tab_opt(tab, crate::autopilot::ITEM) else {
            return;
        };
        let note = self
            .tab_opt(tab, "@tab_note")
            .unwrap_or_else(|| self.autopilot.mode.note.clone());
        if !in_notes(&note) {
            return;
        }
        self.set_tab_opt(tab, PLAN_NOTE, Some(""));
        self.notes_off_task(
            move || {
                let dir = store::notes_dir();
                let text = std::fs::read_to_string(dir.join(&note)).ok()?;
                let file = crate::cmd::worktree_from_note::item_plan_file(&dir, &text, &item)?;
                let rel = file.strip_prefix(&dir).ok()?.to_string_lossy().into_owned();
                Some(rel)
            },
            move |server, found| {
                if let Some(rel) = found {
                    server.set_tab_opt(tab, PLAN_NOTE, Some(&rel));
                }
            },
        );
    }

    /// Notes leaving a tab: the note it shows becomes the tab's last, once
    /// the tab has a note or another was opened there than the one Notes
    /// came with.
    fn remember_note(&mut self, tab: TabId) {
        if self.tab_opt(tab, "@notes").as_deref() == Some("1") {
            return;
        }
        let arrived = self.tab_opt(tab, "@tab_note_arrived");
        self.set_tab_opt(tab, "@tab_note_arrived", None);
        let Some(note) = shown() else {
            return;
        };
        if self.note_for(tab).is_none() && Some(&note) == arrived.as_ref() {
            return;
        }
        self.set_tab_opt(tab, "@tab_note_last", Some(&note));
    }

    /// What Notes showed as it came beside a tab, for `remember_note` to tell
    /// whether another note was opened there.
    fn note_arrived(&mut self, tab: TabId) {
        if let Some(note) = self.note_for(tab).or_else(shown) {
            self.set_tab_opt(tab, "@tab_note_arrived", Some(&note));
        }
    }

    /// The tab's note shown in the running app, which says why it is that
    /// one when precedence picked it.
    fn show_note(&mut self, tab: TabId) {
        let Some((want, why)) = self.note_and_why(tab) else {
            return;
        };
        if shown().as_ref() != Some(&want) {
            tell(format!("open {want}"));
        }
        if let Some(said) = why.said() {
            tell(format!("say {said}"));
        }
    }

    /// The tab's note shown, if Notes is beside it (a note just chosen for
    /// it).
    pub(crate) fn notes_show(&mut self, tab: TabId) {
        if self
            .notes_pane()
            .is_some_and(|p| self.pane_tab(p) == Some(tab))
        {
            self.show_note(tab);
        }
    }

    /// The tab's note made the one the app opens with, for an app about to
    /// start.
    fn open_with_note(&mut self, tab: TabId) {
        if let Some(want) = self.note_for(tab) {
            Store::at(store::notes_dir()).set_last(Some(&want));
        }
    }

    /// The app's list shown, or put away; with notes_hide_sidebar off,
    /// always shown.
    fn notes_list(&mut self, show: bool) {
        let show = show || !settings::is_on("notes_hide_sidebar");
        tell(format!("sidebar {}", if show { "on" } else { "off" }));
    }

    /// A tab with no note yet, neither one chosen nor one shown beside it
    /// before, is asked which is its, until told to keep what is open, with
    /// the ask_tab_note setting on.
    fn ask_note_if_none(&mut self, tab: TabId, client: Option<ClientId>) {
        if !settings::is_on("ask_tab_note")
            || self.note_for(tab).is_some()
            || self.tab_opt(tab, "@tab_note_asked").is_some()
            || self.tab_opt(tab, "@notes").as_deref() == Some("1")
        {
            return;
        }
        self.ask_tab_note(tab, client);
    }

    // ── For the sidebar ──────────────────────────────────────────────────────

    /// Which tab of a session has Notes beside its work, for the Notes tab's
    /// line while it is away: its number and its label, PR left out.
    pub fn notes_owner(&self, session: &str) -> Option<(usize, String)> {
        let notes = self.marked_notes_pane()?;
        let tab = self.pane_tab(notes)?;
        if self.pane_count(tab) < 2 {
            return None;
        }
        self.session(session)?.tabs.contains(&tab).then_some(())?;
        let index = self.tab_number(tab);
        let label = self.tab_label(tab);
        let name = label
            .strip_prefix('#')
            .and_then(|rest| rest.split_once(' '))
            .filter(|(n, _)| n.parse::<u64>().is_ok())
            .map_or(label.clone(), |(_, name)| name.to_string());
        Some((index, name))
    }

    // ── press ────────────────────────────────────────────────────────────────

    /// `press <name> [arg]`: runs the action named as its key would, in the
    /// client used last on the caller's session (the app's palette asks
    /// this); `note-tab` takes the note as `arg`. `work-on-item <note>
    /// <line>`, Notes' `gw`, is no key's: it starts work on the item on
    /// that line of the note (src/cmd/work.rs). Nor is UX 2.0's
    /// `plan-item <note> <text>` (M-Enter in "Add an item": a tab whose
    /// Claude runs `/plan-item`).
    pub fn cmd_press(&mut self, args: &[String], caller: Option<PaneId>) -> Reply {
        let Some((name, rest)) = args.split_first() else {
            return Reply::fail("press: which action?");
        };
        let arg = rest.first().filter(|a| !a.is_empty()).cloned();
        if name == WORK_ON_ITEM {
            let (Some(note), Some(line)) = (
                arg.filter(|n| in_notes(n)),
                rest.get(1).and_then(|l| l.parse::<usize>().ok()),
            ) else {
                return Reply::fail("press: work-on-item takes a note and a line number");
            };
            let Some(client) = self.press_client(caller) else {
                return Reply::fail("press: no client is attached to press it in");
            };
            self.work_on_item(client, note, line);
            return Reply::ok("");
        }
        if name == PLAN_ITEM {
            let Some(note) = arg.filter(|n| in_notes(n)) else {
                return Reply::fail(format!("press: {name} takes a note"));
            };
            let text = rest.get(1).map(|t| t.trim()).unwrap_or("").to_string();
            if text.is_empty() {
                return Reply::fail("press: plan-item takes a note and the item");
            }
            let Some(client) = self.press_client(caller) else {
                return Reply::fail("press: no client is attached to press it in");
            };
            self.plan_new_item(client, note, text);
            return Reply::ok("");
        }
        let Some(action) = Action::parse(name) else {
            return Reply::fail(format!("press: no action named {name}"));
        };
        if arg.is_some() && action != Action::Notes(Command::NoteTab) {
            return Reply::fail(format!("press: {name} takes nothing more"));
        }
        let Some(client) = self.press_client(caller) else {
            return Reply::fail("press: no client is attached to press it in");
        };
        match action {
            Action::Notes(Command::NoteTab) => self.note_tab(client, arg),
            action => self.act(client, action),
        }
        Reply::ok("")
    }

    /// The client a press runs in: the one used last on the caller's
    /// session, else the one used last.
    pub(crate) fn press_client(&self, caller: Option<PaneId>) -> Option<ClientId> {
        caller
            .and_then(|p| self.pane_tab(p))
            .and_then(|t| self.session_of_tab(t))
            .and_then(|s| self.latest_client_of(&s.name))
            .or_else(|| self.latest_client())
    }

    /// The directory a pane is in, for a picker that offers it first.
    pub(crate) fn dir_of(&self, pane: PaneId) -> Option<PathBuf> {
        self.pane_dir(pane)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_names_round_trip_and_keys_are_tmux_confs() {
        for c in Command::ALL {
            assert_eq!(Command::parse(c.name()), Some(c));
        }
        assert_eq!(Command::Open.key(), Some("M-N"));
        assert_eq!(Command::Beside.key(), Some("M-e"));
        assert_eq!(Command::NoteTab.key(), None);
    }

    /// `by_precedence` over the options given, every note there but those
    /// in `gone`.
    fn pick(
        opts: &[(&str, &str)],
        working: Option<&str>,
        precedence: bool,
        gone: &[&str],
    ) -> Option<(String, Why)> {
        let opts: std::collections::HashMap<String, String> = opts
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        by_precedence(
            |name| opts.get(name).filter(|v| !v.is_empty()).cloned(),
            working,
            precedence,
            |rel| !gone.contains(&rel),
        )
    }

    fn picked(note: &str, why: Why) -> Option<(String, Why)> {
        Some((note.to_string(), why))
    }

    #[test]
    fn m_e_picks_the_tabs_note_by_precedence() {
        let autopilot_tab = [
            ("@autopilot", "1700000000"),
            ("@tab_note", "Work.md"),
            ("@tab_plan_note", "Plans/Plan - X.md"),
            ("@tab_note_last", "Last.md"),
        ];
        let mut linked = autopilot_tab.to_vec();
        linked.push((BY_YOU, "Mine.md"));
        assert_eq!(
            pick(&linked, Some("Work.md"), true, &[]),
            picked("Mine.md", Why::Linked),
            "a note you linked wins over the plan"
        );
        assert_eq!(
            pick(&autopilot_tab, Some("Work.md"), true, &[]),
            picked("Plans/Plan - X.md", Why::Plan),
        );
        let plain = [("@tab_note", "Other.md"), ("@tab_note_last", "Last.md")];
        assert_eq!(
            pick(&plain, Some("Work.md"), true, &[]),
            picked("Work.md", Why::Autopilot),
            "a plain tab while autopilot works a note"
        );
        assert_eq!(
            pick(&plain, None, true, &[]),
            picked("Other.md", Why::Tab),
            "autopilot off"
        );
        assert_eq!(
            pick(&[("@tab_note_last", "Last.md")], None, true, &[]),
            picked("Last.md", Why::Tab),
        );
        assert_eq!(
            pick(
                &linked,
                Some("Work.md"),
                true,
                &["Mine.md", "Plans/Plan - X.md"]
            ),
            picked("Work.md", Why::Autopilot),
            "a note that isn't there falls through"
        );
        assert_eq!(
            pick(&[("@tab_plan_note", "Plans/Plan - X.md")], None, true, &[]),
            None,
            "a plan counts only on autopilot's tab"
        );
        assert_eq!(
            pick(&linked, Some("Work.md"), false, &[]),
            picked("Work.md", Why::Tab),
            "with the setting off, the tab's note, as before"
        );
        assert_eq!(pick(&[], None, true, &[]), None);
    }

    #[test]
    fn a_note_is_a_path_in_the_notes_directory() {
        assert!(in_notes("Work/Plan.md"));
        assert!(!in_notes("../Plan.md"));
        assert!(!in_notes("Work/../../x.md"));
        assert!(!in_notes("/etc/passwd"));
        assert!(!in_notes(""));
    }
}
