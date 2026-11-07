//! The server: `switchboard server`, started by the first `switchboard` and
//! left running when every client has gone, as tmux's server was. One task
//! owns everything (the sessions, their tabs, every pane, the clients) and
//! takes `Event`s from everything else: the connections, each pane's reader
//! thread, the timers. It handles them in order and then draws the clients
//! whose screens changed, so no state is ever shared or locked, and a slow
//! lookup elsewhere never holds up a keystroke.
//!
//! A session is a set of tabs; each attached client shows one session's
//! current tab. Tabs are sized to the session's most recently active client,
//! as tmux's `window-size latest` did.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};

use super::compose::{self, Frame, Screen};
use super::input::{self, Input, Key, KeyCode, Mods, Mouse, MouseButton, MouseKind, Parser};
use super::keys::{Action, Bindings};
use super::layout::Layout;
use super::pane::{Pane, PaneSpawn, PaneView};
use super::proto::{self, ClientMsg, ServerMsg};
use super::{Direction, PaneEvent, PaneId, TabId, TermNotice, socket};
use crate::{paths, util};

/// The session `switchboard` attaches to when none is named.
pub const DEFAULT_SESSION: &str = "main";
/// Lines of history each pane keeps (tmux.conf's `history-limit`).
pub const HISTORY: usize = 10_000;
/// How often tab names are read again from their panes' directories.
const TICK: Duration = Duration::from_secs(2);
/// Two clicks this close together are a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// How often whether you're at the computer is read again.
const PRESENCE_EVERY: Duration = Duration::from_secs(60);
/// Idle this long counts as away (the sidebar daemon's AWAY_MS).
pub const AWAY_MS: f64 = 10.0 * 60_000.0;
/// Lines the wheel scrolls at a time.
const WHEEL_LINES: i32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClientId(pub u32);

/// What the server task is told.
pub enum Event {
    /// A terminal attached.
    Attach {
        id: ClientId,
        session: Option<String>,
        cols: u16,
        rows: u16,
        tx: mpsc::UnboundedSender<ServerMsg>,
    },
    Input(ClientId, Vec<u8>),
    Resize(ClientId, u16, u16),
    /// A client left or its connection ended.
    Gone(ClientId),
    /// A command from a shell or a hook, answered on `reply`.
    Command {
        args: Vec<String>,
        cwd: PathBuf,
        pane: Option<String>,
        stdin: Option<Vec<u8>>,
        reply: oneshot::Sender<Reply>,
    },
    /// Time for a client's held ESC to count as the Escape key.
    FlushInput(ClientId),
    /// A tab focused `agent::SEEN_DELAY` ago, to clear what it showed as
    /// finished if it is still in front (src/agent).
    AgentSeen(TabId),
    /// Whether you're at the computer, as last read (see `away`).
    Presence(crate::presence::Presence),
    /// A PR or ticket lookup for the tab names answered (src/tabs).
    TabLookup(crate::tabs::Answer),
    /// A GitHub, Linear or landing-queue lookup answered (src/sources).
    Sources(crate::sources::Answer),
    /// The overlays' timers (src/ui).
    Ui(crate::ui::UiEvent),
    /// What the usage views' work off the task found (src/usage).
    Usage(crate::usage::UsageEvent),
    /// The calendar, Slack and the notes written for you answered (src/day).
    Day(crate::day::DayEvent),
    /// The sidebar's timers (src/sidebar).
    Sidebar(crate::sidebar::SidebarEvent),
    /// The rest of a key that starts work, back from off the task
    /// (src/cmd/work.rs).
    Work(crate::cmd::work::WorkEvent),
    /// Mission control's timers and background work (src/mission).
    Mission(crate::mission::MissionEvent),
    /// What the views' lookups found (src/views).
    Views(crate::views::ViewsEvent),
    /// A look for a new release answered (src/update).
    Update(crate::update::UpdateEvent),
    /// The status line's counts answered (src/status).
    Status(crate::status::StatusEvent),
    /// The rest of what a Notes key does, back from off the task
    /// (src/notes/serve.rs).
    Notes(crate::notes::serve::NotesEvent),
    /// What working a note continuously found off the task (src/autopilot).
    Autopilot(crate::autopilot::AutopilotEvent),
    Tick,
    Quit,
}

/// A command's answer: its exit code, what it printed and what it said went wrong.
#[derive(Debug, Default)]
pub struct Reply {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Reply {
    pub fn ok(stdout: impl Into<String>) -> Reply {
        Reply {
            code: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    pub fn fail(why: impl Into<String>) -> Reply {
        let mut stderr = why.into();
        if !stderr.ends_with('\n') {
            stderr.push('\n');
        }
        Reply {
            code: 1,
            stdout: String::new(),
            stderr,
        }
    }
}

pub struct Session {
    pub name: String,
    pub tabs: Vec<TabId>,
    pub current: TabId,
    pub last: Option<TabId>,
    /// The size its tabs are laid out for: its latest client's.
    pub cols: u16,
    pub rows: u16,
    /// The columns the sidebar takes on the left, its line beside the panes
    /// included; 0 with none (src/sidebar).
    pub sidebar: u16,
}

impl Session {
    /// The area panes are laid out in: all but the status line (its one or
    /// two rows, src/status) and the sidebar.
    pub fn main_area(&self) -> Rect {
        let side = self.sidebar.min(self.cols.saturating_sub(1));
        Rect::new(
            side,
            0,
            self.cols.saturating_sub(side).max(1),
            self.rows.saturating_sub(crate::status::lines()).max(1),
        )
    }

    /// All but the status line: a zoomed pane takes it, covering the
    /// sidebar, as a zoomed tmux pane covered the sidebar's.
    pub fn full_area(&self) -> Rect {
        Rect::new(
            0,
            0,
            self.cols.max(1),
            self.rows.saturating_sub(crate::status::lines()).max(1),
        )
    }

    /// Where a tab's panes go: the whole of it when zoomed.
    pub fn area_for(&self, zoomed: bool) -> Rect {
        if zoomed {
            self.full_area()
        } else {
            self.main_area()
        }
    }
}

pub struct Tab {
    pub id: TabId,
    /// A name given to it; empty to name it after its pane's directory.
    pub name: String,
    /// The name from its pane's directory, read on focus and every `TICK`.
    pub auto_name: String,
    pub layout: Layout,
    pub active: PaneId,
    pub last_pane: Option<PaneId>,
    pub zoomed: bool,
    /// Options set on it by commands (`@tab_label` and the like).
    pub opts: BTreeMap<String, String>,
}

impl Tab {
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.auto_name
        } else {
            &self.name
        }
    }
}

/// Scrolling back through a pane's history, and selecting in it with the
/// keyboard (tmux's copy mode with `mode-keys vi`).
#[derive(Clone, Debug, Default)]
pub struct CopyMode {
    /// Entered with the wheel: scrolled back to the bottom, it ends, until a
    /// key moves its cursor.
    pub exit_at_bottom: bool,
    /// The last search, for n and N: what was looked for, and whether
    /// forward (`/`) or back (`?`).
    pub search: Option<(String, bool)>,
}

pub struct PaneSlot {
    pub pane: Pane,
    pub tab: TabId,
    pub opts: BTreeMap<String, String>,
    pub copy: Option<CopyMode>,
}

/// A drag in progress, by what the press landed on.
#[derive(Clone, Copy, Debug)]
enum Drag {
    /// Selecting text in a pane that doesn't take the mouse.
    Select(PaneId),
    /// A pane that takes the mouse gets the drag.
    Forward(PaneId),
    /// A line between panes, moved with the mouse: its tab, the panes
    /// before and after it, whether it runs down, and where it is now.
    Border {
        tab: TabId,
        before: PaneId,
        after: PaneId,
        vertical: bool,
        at: u16,
    },
}

pub struct Client {
    pub id: ClientId,
    pub tx: mpsc::UnboundedSender<ServerMsg>,
    pub session: String,
    pub cols: u16,
    pub rows: u16,
    parser: Parser,
    prefix: bool,
    screen: Screen,
    pub dirty: bool,
    drag: Option<Drag>,
    last_click: Option<(Instant, u16, u16, u8)>,
    /// Where the status line's button and tabs were drawn, for clicks.
    status_hits: crate::status::Hits,
    /// What is shown over the tab, the top one taking the input (src/ui).
    pub overlays: crate::ui::Stack,
    pub activity: Instant,
    /// Counts the options taken in mission control's menus, so the keys read
    /// with the one that took an option are dropped rather than reaching
    /// what is in front next (the daemon's `optionsTaken`).
    pub options_taken: u32,
}

pub struct Server {
    pub(crate) tx: mpsc::UnboundedSender<Event>,
    pub(super) pane_tx: mpsc::UnboundedSender<PaneEvent>,
    pub sessions: Vec<Session>,
    pub tabs: HashMap<TabId, Tab>,
    pub panes: HashMap<PaneId, PaneSlot>,
    pub clients: BTreeMap<ClientId, Client>,
    pub bindings: Bindings,
    /// What each pane's Claude is doing (src/agent).
    pub agents: crate::agent::Agents,
    /// What GitHub and the landing queue say of each tab (src/facts.rs),
    /// written by the sources, read by the sidebar.
    pub facts: crate::facts::Facts,
    /// The sidebar's notifications, by the source that gave them
    /// ("calendar", "new-notes", "habits", "autopilot"): each source sets
    /// its whole list, which the sidebar shows under the tabs.
    pub notices: BTreeMap<String, Vec<crate::items::Item>>,
    /// What each tab is called and the color it wears (src/tabs).
    pub tab_names: crate::tabs::TabNames,
    /// What GitHub and Linear say for the views, and the lookups behind
    /// `facts` (src/sources).
    pub sources: crate::sources::Sources,
    pub(super) next_tab: u32,
    pub(super) next_pane: u32,
    pub quit: bool,
    /// When a key was last pressed in any client, for telling whether you're
    /// here where the desktop can't say.
    pub last_input: Instant,
    /// The settings file's modification time when last read, so a change
    /// made in the Settings view or by hand takes effect on the next tick.
    settings_seen: Option<std::time::SystemTime>,
    /// Whether you're at the computer, read at most every `PRESENCE_EVERY`
    /// off the server's task (src/presence.rs).
    presence: Option<crate::presence::Presence>,
    presence_asked: Option<Instant>,
    /// Saving the layout and restoring it (`persist`).
    pub(super) persist: super::persist::Persist,
    /// What the overlays keep across clients: the tips shown (src/ui).
    pub ui: crate::ui::Ui,
    /// How the workspace is used: habits, resources, sessions, closed tabs,
    /// plugins (src/usage).
    pub usage: crate::usage::Usage,
    /// Your day: the calendar, Slack, the notes written for you (src/day).
    pub day: crate::day::Day,
    /// The sidebar's state: what was read of each pane's Claude, each
    /// session's focus and cursor, where each client's was drawn
    /// (src/sidebar).
    pub sidebar: crate::sidebar::Sidebar,
    /// Mission control's state across clients: the worktrees the Work map
    /// lists (src/mission).
    pub mission: crate::mission::MissionState,
    /// Each client's search everything, as its sources answer (src/views).
    pub views: crate::views::Views,
    /// What the health light says (src/status).
    pub status: crate::status::Status,
    /// The look for a new release, and its box (src/update).
    pub update: crate::update::Updates,
    /// The clients waiting on C-w's next key (src/extras).
    pub extras: crate::extras::Extras,
    /// Working a note continuously (src/autopilot).
    pub autopilot: crate::autopilot::Autopilot,
}

// ── Running it ──────────────────────────────────────────────────────────────

/// `switchboard server`.
pub fn main(args: Vec<String>) -> ExitCode {
    if !args.is_empty() {
        eprintln!("Usage: switchboard server   (started for you by switchboard)");
        return ExitCode::from(2);
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("switchboard server: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("switchboard server: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<()> {
    let listener = socket::bind()?;
    let (tx, mut rx) = mpsc::unbounded_channel::<Event>();
    let (pane_tx, mut pane_rx) = mpsc::unbounded_channel::<PaneEvent>();
    tokio::spawn(accept(listener, tx.clone()));
    spawn_signals(tx.clone());
    let ticker = tx.clone();
    tokio::spawn(async move {
        let mut every = tokio::time::interval(TICK);
        loop {
            every.tick().await;
            if ticker.send(Event::Tick).is_err() {
                break;
            }
        }
    });
    let mut server = Server::new(tx, pane_tx);
    let (bindings, problems) = Bindings::load();
    server.bindings = bindings;
    for p in &problems {
        eprintln!("keys.json: {p}");
    }
    server.note_at_start(&problems);
    server.sidebar_load();
    // The status line's rows, before any tab is laid out.
    server.status_tick();
    super::persist::restore(&mut server);
    loop {
        // A program holding a frame back (synchronized output) that never
        // says it is done gets it shown at its deadline.
        let deadline = server.sync_deadline();
        let held = async move {
            match deadline {
                Some(at) => tokio::time::sleep_until(at.into()).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            Some(event) = rx.recv() => server.handle(event),
            Some(event) = pane_rx.recv() => server.pane_event(event),
            () = held => server.flush_syncs(),
            else => break,
        }
        // Whatever else has come in meanwhile, before drawing once for all of it.
        for _ in 0..256 {
            match pane_rx.try_recv() {
                Ok(event) => server.pane_event(event),
                Err(_) => break,
            }
        }
        while let Ok(event) = rx.try_recv() {
            server.handle(event);
        }
        server.draw_dirty();
        if server.quit {
            break;
        }
    }
    server.shutdown();
    let _ = std::fs::remove_file(socket::socket_path());
    Ok(())
}

fn spawn_signals(tx: mpsc::UnboundedSender<Event>) {
    use tokio::signal::unix::{SignalKind, signal};
    for kind in [
        SignalKind::terminate(),
        SignalKind::interrupt(),
        SignalKind::hangup(),
    ] {
        let tx = tx.clone();
        if let Ok(mut s) = signal(kind) {
            tokio::spawn(async move {
                s.recv().await;
                let _ = tx.send(Event::Quit);
            });
        }
    }
}

static NEXT_CLIENT: AtomicU32 = AtomicU32::new(1);

async fn accept(listener: tokio::net::UnixListener, tx: mpsc::UnboundedSender<Event>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(connection(stream, tx.clone()));
            }
            Err(e) => {
                eprintln!("switchboard server: accepting a connection: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn connection(stream: tokio::net::UnixStream, tx: mpsc::UnboundedSender<Event>) {
    let (mut r, mut w) = stream.into_split();
    let Ok(Some(first)) = proto::read_client_async(&mut r).await else {
        return;
    };
    match first {
        ClientMsg::Attach {
            session,
            cols,
            rows,
            term: _,
        } => {
            let id = ClientId(NEXT_CLIENT.fetch_add(1, Ordering::Relaxed));
            let (otx, mut orx) = mpsc::unbounded_channel::<ServerMsg>();
            if tx
                .send(Event::Attach {
                    id,
                    session,
                    cols,
                    rows,
                    tx: otx,
                })
                .is_err()
            {
                return;
            }
            let writer = tokio::spawn(async move {
                while let Some(msg) = orx.recv().await {
                    let exit = matches!(msg, ServerMsg::Exit { .. });
                    if w.write_all(&proto::encode_server(&msg)).await.is_err() || exit {
                        break;
                    }
                }
                let _ = w.shutdown().await;
            });
            loop {
                match proto::read_client_async(&mut r).await {
                    Ok(Some(ClientMsg::Input(bytes))) => {
                        let _ = tx.send(Event::Input(id, bytes));
                    }
                    Ok(Some(ClientMsg::Resize { cols, rows })) => {
                        let _ = tx.send(Event::Resize(id, cols, rows));
                    }
                    Ok(Some(ClientMsg::Detach)) | Ok(None) | Err(_) => break,
                    Ok(Some(_)) => {}
                }
            }
            let _ = tx.send(Event::Gone(id));
            let _ = writer.await;
        }
        ClientMsg::Command {
            args,
            cwd,
            pane,
            stdin,
        } => {
            let (rtx, rrx) = oneshot::channel();
            if tx
                .send(Event::Command {
                    args,
                    cwd,
                    pane,
                    stdin,
                    reply: rtx,
                })
                .is_err()
            {
                return;
            }
            let reply = rrx
                .await
                .unwrap_or_else(|_| Reply::fail("the server ended"));
            let msg = ServerMsg::Reply {
                code: reply.code,
                stdout: reply.stdout.into_bytes(),
                stderr: reply.stderr.into_bytes(),
            };
            let _ = w.write_all(&proto::encode_server(&msg)).await;
            let _ = w.shutdown().await;
        }
        _ => {}
    }
}

// ── The state ───────────────────────────────────────────────────────────────

impl Server {
    pub fn new(
        tx: mpsc::UnboundedSender<Event>,
        pane_tx: mpsc::UnboundedSender<PaneEvent>,
    ) -> Server {
        Server {
            tx,
            pane_tx,
            sessions: Vec::new(),
            tabs: HashMap::new(),
            panes: HashMap::new(),
            clients: BTreeMap::new(),
            bindings: Bindings::default(),
            agents: crate::agent::Agents::default(),
            facts: crate::facts::Facts::default(),
            notices: BTreeMap::new(),
            tab_names: Default::default(),
            sources: Default::default(),
            next_tab: 1,
            next_pane: 1,
            quit: false,
            last_input: Instant::now(),
            settings_seen: None,
            presence: None,
            presence_asked: None,
            persist: Default::default(),
            ui: Default::default(),
            usage: Default::default(),
            day: Default::default(),
            sidebar: Default::default(),
            mission: Default::default(),
            views: Default::default(),
            status: Default::default(),
            update: Default::default(),
            extras: Default::default(),
            autopilot: Default::default(),
        }
    }

    pub fn handle(&mut self, event: Event) {
        match event {
            Event::Attach {
                id,
                session,
                cols,
                rows,
                tx,
            } => self.attach(id, session, cols, rows, tx),
            Event::Input(id, bytes) => self.input(id, &bytes),
            Event::Resize(id, cols, rows) => self.resize_client(id, cols, rows),
            Event::Gone(id) => {
                self.clients.remove(&id);
            }
            Event::Command {
                args,
                cwd,
                pane,
                stdin,
                reply,
            } => {
                let answer = self.command(&args, &cwd, pane.as_deref(), stdin.as_deref());
                let _ = reply.send(answer);
            }
            Event::FlushInput(id) => {
                let parsed = match self.clients.get_mut(&id) {
                    Some(c) if c.parser.pending() => c.parser.flush(),
                    _ => return,
                };
                for p in parsed {
                    self.dispatch(id, p);
                }
                // Still held: a paste whose end hasn't come. It is let go
                // once nothing more of it has come for PASTE_TIMEOUT.
                if self.clients.get(&id).is_some_and(|c| c.parser.pending()) {
                    self.flush_later(id, input::PASTE_TIMEOUT);
                }
            }
            Event::AgentSeen(tab) => self.agent_seen(tab),
            Event::Presence(p) => self.presence = Some(p),
            Event::TabLookup(answer) => self.tab_lookup(answer),
            Event::Sources(answer) => self.source_answer(answer),
            Event::Ui(event) => self.ui_event(event),
            Event::Usage(event) => self.usage_event(event),
            Event::Day(event) => self.day_event(event),
            Event::Sidebar(event) => self.sidebar_event(event),
            Event::Work(event) => self.work_event(event),
            Event::Mission(event) => self.mission_event(event),
            Event::Views(event) => self.views_event(event),
            Event::Update(event) => self.update_event(event),
            Event::Status(event) => self.status_event(event),
            Event::Notes(event) => self.notes_event(event),
            Event::Autopilot(event) => self.autopilot_event(event),
            Event::Tick => {
                self.reload_settings_if_changed();
                self.reload_keys_if_changed();

                self.status_tick();
                self.ask_presence();
                self.refresh_names();
                self.sources_tick();
                self.day_tick();
                self.sidebar_tick();
                super::persist::tick(self);
                self.usage_tick();
                self.mission_tick();
                self.diff_tick();
                self.update_tick();
                self.autopilot_tick();
            }
            Event::Quit => self.quit = true,
        }
    }

    pub fn pane_event(&mut self, event: PaneEvent) {
        match event {
            PaneEvent::Output(id, generation, bytes) => {
                // Output of a program `respawn` has replaced is dropped.
                let Some(slot) = self
                    .panes
                    .get_mut(&id)
                    .filter(|s| s.pane.generation() == generation)
                else {
                    return;
                };
                let allowed = super::passthrough::graphics_allowed(&slot.opts);
                slot.pane.pass_graphics(allowed);
                let notices = slot.pane.process(&bytes);
                let tab = slot.tab;
                self.sidebar.output(id);
                let mut graphics = Vec::new();
                for notice in notices {
                    match notice {
                        TermNotice::Clipboard(text) => self.to_clipboard(None, &text),
                        TermNotice::Graphics(command) => graphics.push(command),
                        TermNotice::Title(_) | TermNotice::Bell => {}
                    }
                }
                self.pass_graphics(tab, graphics);
                self.dirty_tab(tab);
            }
            PaneEvent::Exited(id, generation, code) => {
                match self.panes.get_mut(&id) {
                    // A program `respawn` replaced ending late leaves the
                    // new one where it is.
                    Some(slot) if slot.pane.generation() != generation => return,
                    Some(slot) => slot.pane.set_exited(code),
                    None => {}
                }
                self.remove_pane(id);
            }
        }
    }

    /// Reads the settings again when their file has changed (or gone), so
    /// `settings::is_on` follows the file, as the sidebar daemon's watch did,
    /// and draws every client again with them.
    fn reload_settings_if_changed(&mut self) {
        let seen = std::fs::metadata(crate::settings::settings_file())
            .and_then(|m| m.modified())
            .ok();
        if seen == self.settings_seen {
            return;
        }
        let first = self.settings_seen.is_none() && seen.is_some();
        self.settings_seen = seen;
        crate::settings::reload_settings();
        if !first {
            for c in self.clients.values_mut() {
                c.dirty = true;
            }
        }
    }

    /// Sets a source's notifications (an empty list takes them away) and
    /// draws every client again when they changed.
    pub fn set_notices(&mut self, source: &str, items: Vec<crate::items::Item>) {
        let changed =
            self.notices.get(source).map(Vec::as_slice).unwrap_or(&[]) != items.as_slice();
        if items.is_empty() {
            self.notices.remove(source);
        } else {
            self.notices.insert(source.to_string(), items);
        }
        if changed {
            for c in self.clients.values_mut() {
                c.dirty = true;
            }
        }
    }

    /// Reads whether you're at the computer again, once `PRESENCE_EVERY` has
    /// passed, off the server's task, as the sidebar daemon checked once a
    /// minute.
    fn ask_presence(&mut self) {
        if self
            .presence_asked
            .is_some_and(|at| at.elapsed() < PRESENCE_EVERY)
        {
            return;
        }
        self.presence_asked = Some(Instant::now());
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let p = crate::presence::read_presence_async().await;
            let _ = tx.send(Event::Presence(p));
        });
    }

    /// Whether you're away, for pausing the lookups that only matter while
    /// you're here (GitHub, Linear, the calendar feed; `pause_when_away`
    /// setting): no client attached, the screen locked, or idle `AWAY_MS` by
    /// the desktop's reckoning, or, where the desktop can't say, by the last
    /// key pressed in a client.
    pub fn away(&self) -> bool {
        crate::settings::is_on("pause_when_away") && self.away_now()
    }

    /// Whether you're away, whatever `pause_when_away` says, for what the
    /// daemon paused on its own reading of presence (working a note
    /// continuously).
    pub fn away_now(&self) -> bool {
        if self.clients.is_empty() {
            return true;
        }
        let mut p = self.presence.unwrap_or_default();
        if p.idle_ms.is_none() {
            p.idle_ms = Some(self.last_input.elapsed().as_millis() as f64);
        }
        crate::presence::away_by(&p, AWAY_MS)
    }

    /// The soonest a pane's held frame must be shown.
    pub fn sync_deadline(&self) -> Option<Instant> {
        self.panes
            .values()
            .filter_map(|s| s.pane.sync_deadline())
            .min()
    }

    /// Shows the frames held past their deadline.
    pub fn flush_syncs(&mut self) {
        let now = Instant::now();
        let due: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|(_, s)| s.pane.sync_deadline().is_some_and(|d| d <= now))
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            let Some(slot) = self.panes.get_mut(&id) else {
                continue;
            };
            let notices = slot.pane.flush_sync();
            let tab = slot.tab;
            for notice in notices {
                if let TermNotice::Clipboard(text) = notice {
                    self.to_clipboard(None, &text);
                }
            }
            self.dirty_tab(tab);
        }
    }

    pub fn shutdown(&mut self) {
        super::persist::shutdown(self);
        for client in self.clients.values() {
            let _ = client.tx.send(ServerMsg::Exit {
                message: Some("[the server ended]".into()),
            });
        }
        for slot in self.panes.values_mut() {
            slot.pane.kill();
        }
    }

    // ── Sessions, tabs, panes ────────────────────────────────────────────────

    pub fn session(&self, name: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.name == name)
    }

    pub fn session_mut(&mut self, name: &str) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.name == name)
    }

    /// The session a tab belongs to.
    pub fn session_of_tab(&self, tab: TabId) -> Option<&Session> {
        self.sessions.iter().find(|s| s.tabs.contains(&tab))
    }

    fn alloc_pane(&mut self) -> PaneId {
        let id = PaneId(self.next_pane);
        self.next_pane += 1;
        id
    }

    /// Starts a pane: `command` (the login shell when `None`) in `cwd`.
    pub(crate) fn spawn_pane(
        &mut self,
        tab: TabId,
        cwd: Option<PathBuf>,
        command: Option<Vec<String>>,
        cols: u16,
        rows: u16,
    ) -> Result<PaneId> {
        let id = self.alloc_pane();
        self.spawn_pane_as(id, tab, cwd, command, cols, rows)
    }

    /// Starts a pane under an id given to it, as a restore does.
    pub(super) fn spawn_pane_as(
        &mut self,
        id: PaneId,
        tab: TabId,
        cwd: Option<PathBuf>,
        command: Option<Vec<String>>,
        cols: u16,
        rows: u16,
    ) -> Result<PaneId> {
        let pane = self.start_program(id, cwd, command, cols, rows)?;
        self.panes.insert(
            id,
            PaneSlot {
                pane,
                tab,
                opts: BTreeMap::new(),
                copy: None,
            },
        );
        Ok(id)
    }

    /// A program started for pane `id`, as every pane's is: `command` (the
    /// login shell when `None`) in `cwd`, its environment naming the server
    /// and the pane.
    pub(crate) fn start_program(
        &self,
        id: PaneId,
        cwd: Option<PathBuf>,
        command: Option<Vec<String>>,
        cols: u16,
        rows: u16,
    ) -> Result<Pane> {
        let mut env = vec![
            (
                "SWITCHBOARD".to_string(),
                socket::socket_path().to_string_lossy().into_owned(),
            ),
            ("SWITCHBOARD_PANE".to_string(), id.to_string()),
        ];
        // gh's `--web`, from any pane, opens in the Chrome profile
        // browser.json gives the site, as every link Switchboard opens does
        // (tmux.conf's `setenv -g GH_BROWSER`). GH_BROWSER, not BROWSER,
        // which other programs read too.
        if let Ok(exe) = crate::util::exe() {
            env.push((
                "GH_BROWSER".to_string(),
                format!(
                    "{} open-url",
                    crate::worktree::shell_quote(&exe.to_string_lossy())
                ),
            ));
        }
        let spec = PaneSpawn {
            id: Some(id),
            command,
            cwd,
            env,
            cols: cols.max(1),
            rows: rows.max(1),
            scrollback: HISTORY,
            extended_keys: crate::settings::is_on("extended_keys"),
        };
        Pane::spawn(spec, self.pane_tx.clone())
    }

    /// Makes a session with one tab, unless it is there.
    pub fn ensure_session(&mut self, name: &str, cols: u16, rows: u16) -> Result<()> {
        if self.session(name).is_some() {
            return Ok(());
        }
        let tab = TabId(self.next_tab);
        self.next_tab += 1;
        let session = Session {
            name: name.to_string(),
            tabs: vec![tab],
            current: tab,
            last: None,
            cols,
            rows,
            sidebar: self.sidebar_columns(cols),
        };
        let area = session.main_area();
        let pane = self.spawn_pane(tab, Some(paths::home()), None, area.width, area.height)?;
        self.tabs.insert(tab, new_tab(tab, pane));
        self.sessions.push(session);
        self.refresh_name(tab);
        Ok(())
    }

    /// A new tab in a session, after its current one, made current unless
    /// `background`; gives the tab and its pane.
    pub fn new_tab(
        &mut self,
        session: &str,
        cwd: Option<PathBuf>,
        command: Option<Vec<String>>,
        name: Option<String>,
        background: bool,
    ) -> Result<(TabId, PaneId)> {
        let Some(s) = self.session(session) else {
            return Err(anyhow!("no session named {session}"));
        };
        let area = s.main_area();
        let tab = TabId(self.next_tab);
        self.next_tab += 1;
        let pane = self.spawn_pane(tab, cwd, command, area.width, area.height)?;
        let mut t = new_tab(tab, pane);
        t.name = name.unwrap_or_default();
        self.tabs.insert(tab, t);
        let s = self
            .session_mut(session)
            .expect("the session was just found");
        let at = s
            .tabs
            .iter()
            .position(|t| *t == s.current)
            .map(|i| i + 1)
            .unwrap_or(s.tabs.len());
        s.tabs.insert(at, tab);
        if !background {
            s.last = Some(s.current);
            s.current = tab;
        }
        // Opened from the Notifications window, it goes before it.
        self.notices_last(session);
        self.refresh_name(tab);
        self.dirty_session(session);
        Ok((tab, pane))
    }

    /// A new pane on a side of `target`, in `cwd` (the target's directory
    /// when `None`); focused unless `background`.
    pub fn split(
        &mut self,
        target: PaneId,
        side: Direction,
        cwd: Option<PathBuf>,
        command: Option<Vec<String>>,
        percent: Option<u16>,
        background: bool,
    ) -> Result<PaneId> {
        let tab_id = self
            .panes
            .get(&target)
            .map(|s| s.tab)
            .ok_or_else(|| anyhow!("no pane {target}"))?;
        let cwd = cwd.or_else(|| self.panes.get(&target).and_then(|s| s.pane.cwd()));
        let area = self
            .session_of_tab(tab_id)
            .map(Session::main_area)
            .unwrap_or(Rect::new(0, 0, 80, 24));
        let pane = self.spawn_pane(
            tab_id,
            cwd,
            command,
            (area.width / 2).max(1),
            (area.height / 2).max(1),
        )?;
        let tab = self.tabs.get_mut(&tab_id).expect("a pane's tab is there");
        tab.zoomed = false;
        tab.layout.split(target, pane, side, percent);
        if !background {
            self.focus_pane(pane);
        }
        self.relayout(tab_id);
        Ok(pane)
    }

    /// Moves a pane out of its tab into a tab of its own, after that one, and
    /// makes it current, as tmux's `break-pane` did; gives the new tab. The
    /// pane goes on running as it was.
    pub fn break_pane(&mut self, pane: PaneId) -> Result<TabId> {
        self.break_pane_to(pane, true)
    }

    /// `break_pane`, the new tab made current only with `select` (tmux's
    /// `break-pane -d` without it).
    pub fn break_pane_to(&mut self, pane: PaneId, select: bool) -> Result<TabId> {
        let from = self
            .panes
            .get(&pane)
            .map(|s| s.tab)
            .ok_or_else(|| anyhow!("no pane {pane}"))?;
        let session = self
            .session_of_tab(from)
            .map(|s| s.name.clone())
            .ok_or_else(|| anyhow!("{from} is in no session"))?;
        let tab = self
            .tabs
            .get_mut(&from)
            .ok_or_else(|| anyhow!("no tab {from}"))?;
        if !tab.layout.remove(pane) {
            return Err(anyhow!("{pane} is the only pane in its tab"));
        }
        if tab.active == pane {
            tab.active = tab
                .last_pane
                .filter(|p| *p != pane && tab.layout.contains(*p))
                .unwrap_or_else(|| tab.layout.panes()[0]);
        }
        if tab.last_pane == Some(pane) {
            tab.last_pane = None;
        }
        tab.zoomed = false;
        let id = TabId(self.next_tab);
        self.next_tab += 1;
        self.tabs.insert(id, new_tab(id, pane));
        if let Some(slot) = self.panes.get_mut(&pane) {
            slot.tab = id;
        }
        if let Some(s) = self.session_mut(&session) {
            let at = s
                .tabs
                .iter()
                .position(|t| *t == from)
                .map_or(s.tabs.len(), |i| i + 1);
            s.tabs.insert(at, id);
        }
        self.relayout(from);
        self.refresh_name(from);
        if select {
            self.select_tab(&session, id);
        } else {
            self.relayout(id);
            self.refresh_name(id);
            self.dirty_session(&session);
        }
        Ok(id)
    }

    /// Takes a pane out of its tab without ending its program, for putting
    /// it in another (tmux's `join-pane` took a pane from its window so):
    /// the tab's focus moves to another of its panes, and the tab goes when
    /// the pane was its last. Gives the tab it was in.
    pub fn take_out(&mut self, pane: PaneId) -> Option<TabId> {
        let from = self.panes.get(&pane)?.tab;
        let tab = self.tabs.get_mut(&from)?;
        if !tab.layout.remove(pane) {
            self.remove_tab(from);
            return Some(from);
        }
        if tab.active == pane {
            tab.active = tab
                .last_pane
                .filter(|p| *p != pane && tab.layout.contains(*p))
                .unwrap_or_else(|| tab.layout.panes()[0]);
        }
        if tab.last_pane == Some(pane) {
            tab.last_pane = None;
        }
        tab.zoomed = false;
        self.relayout(from);
        self.refresh_name(from);
        self.dirty_tab(from);
        Some(from)
    }

    /// Ends a pane's program and takes it out (its tab too, when it was the last).
    pub fn kill_pane(&mut self, pane: PaneId) {
        if let Some(slot) = self.panes.get_mut(&pane) {
            slot.pane.kill();
        }
        self.remove_pane(pane);
    }

    /// Takes a pane out of its tab; the tab goes when it was its last, and
    /// the session when that was its last tab.
    pub fn remove_pane(&mut self, pane: PaneId) {
        // A tool in a pane's place gives the place back (src/diff/in_place.rs).
        if self.give_place_back(pane) {
            return;
        }
        let Some(slot) = self.panes.remove(&pane) else {
            return;
        };
        let tab_id = slot.tab;
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return;
        };
        if tab.layout.remove(pane) {
            if tab.active == pane {
                tab.active = tab
                    .last_pane
                    .filter(|p| *p != pane && tab.layout.contains(*p))
                    .unwrap_or_else(|| tab.layout.panes()[0]);
            }
            if tab.last_pane == Some(pane) {
                tab.last_pane = None;
            }
            tab.zoomed = false;
            self.relayout(tab_id);
            self.dirty_tab(tab_id);
            // Notes left alone in a tab it was brought beside goes home
            // (src/notes/serve.rs).
            self.notes_orphan(tab_id);
        } else {
            self.remove_tab(tab_id);
        }
    }

    /// Ends every pane of a tab and takes it out.
    pub fn kill_tab(&mut self, tab: TabId) {
        let panes = self.panes_with_parked(tab);
        for p in panes {
            if let Some(mut slot) = self.panes.remove(&p) {
                slot.pane.kill();
            }
        }
        self.remove_tab(tab);
    }

    fn remove_tab(&mut self, tab: TabId) {
        self.tab_closing(tab);
        self.tabs.remove(&tab);
        let Some(si) = self.sessions.iter().position(|s| s.tabs.contains(&tab)) else {
            return;
        };
        let s = &mut self.sessions[si];
        let at = s.tabs.iter().position(|t| *t == tab).unwrap_or(0);
        s.tabs.retain(|t| *t != tab);
        if s.last == Some(tab) {
            s.last = None;
        }
        if s.tabs.is_empty() {
            let name = s.name.clone();
            self.sessions.remove(si);
            self.session_ended(&name);
            return;
        }
        if s.current == tab {
            // As tmux does: the tab you were on before, else the one beside it.
            s.current = s.last.take().unwrap_or(s.tabs[at.min(s.tabs.len() - 1)]);
        }
        let name = s.name.clone();
        self.dirty_session(&name);
    }

    /// A session has no tabs left: its clients move to another session, or
    /// end; with no sessions left, the server ends too.
    fn session_ended(&mut self, name: &str) {
        let other = self.sessions.first().map(|s| s.name.clone());
        for client in self.clients.values_mut().filter(|c| c.session == name) {
            match &other {
                Some(o) => {
                    client.session = o.clone();
                    client.screen.invalidate();
                    client.dirty = true;
                }
                None => {
                    let _ = client.tx.send(ServerMsg::Exit {
                        message: Some("[exited]".into()),
                    });
                }
            }
        }
        if other.is_none() {
            self.clients.clear();
            self.quit = true;
        }
    }

    pub fn select_tab(&mut self, session: &str, tab: TabId) {
        let Some(s) = self.session_mut(session) else {
            return;
        };
        let switched = s.current != tab && s.tabs.contains(&tab);
        if switched {
            s.last = Some(s.current);
            s.current = tab;
        }
        self.refresh_name(tab);
        self.relayout(tab);
        self.dirty_session(session);
        self.agent_focus(tab);
        if switched {
            self.tab_switched(session);
        }
    }

    /// Focuses a pane, its tab made current in its session, the focus taken
    /// out of the session's sidebar.
    pub fn focus_pane(&mut self, pane: PaneId) {
        let Some(tab_id) = self.panes.get(&pane).map(|s| s.tab) else {
            return;
        };
        self.sidebar_leave_tab(tab_id);
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return;
        };
        let old = tab.active;
        if old != pane {
            tab.last_pane = Some(old);
            tab.active = pane;
            if tab.zoomed {
                tab.zoomed = false;
            }
            self.send_focus(old, false);
            self.send_focus(pane, true);
            self.relayout(tab_id);
        }
        if let Some(name) = self.session_of_tab(tab_id).map(|s| s.name.clone()) {
            self.select_tab(&name, tab_id);
        }
        self.refresh_name(tab_id);
        self.dirty_tab(tab_id);
    }

    fn send_focus(&mut self, pane: PaneId, focused: bool) {
        if let Some(slot) = self.panes.get_mut(&pane)
            && let Some(bytes) = input::encode_focus(focused, &slot.pane.modes())
        {
            let _ = slot.pane.write(&bytes);
        }
    }

    /// Lays a tab's panes out for its session's size, resizing each pane.
    pub fn relayout(&mut self, tab_id: TabId) {
        let Some(tab) = self.tabs.get(&tab_id) else {
            return;
        };
        let Some(area) = self.session_of_tab(tab_id).map(|s| s.area_for(tab.zoomed)) else {
            return;
        };
        let rects: Vec<(PaneId, Rect)> = if tab.zoomed {
            vec![(tab.active, area)]
        } else {
            tab.layout.place(area).panes
        };
        for (pane, rect) in rects {
            if let Some(slot) = self.panes.get_mut(&pane) {
                slot.pane.resize(rect.width.max(1), rect.height.max(1));
            }
        }
    }

    /// Names a tab from its pane's directory (src/tabs).
    pub(super) fn refresh_name(&mut self, tab_id: TabId) {
        self.name_tab(tab_id);
    }

    /// Names every tab, as window-names.sh did for every window of the
    /// session: a lookup answered for one directory can rename any of them.
    fn refresh_names(&mut self) {
        self.name_all_tabs();
    }

    // ── Clients ──────────────────────────────────────────────────────────────

    fn attach(
        &mut self,
        id: ClientId,
        session: Option<String>,
        cols: u16,
        rows: u16,
        tx: mpsc::UnboundedSender<ServerMsg>,
    ) {
        let name = session.unwrap_or_else(|| {
            self.sessions
                .first()
                .map(|s| s.name.clone())
                .unwrap_or_else(|| DEFAULT_SESSION.to_string())
        });
        if let Err(e) = self.ensure_session(&name, cols, rows) {
            let _ = tx.send(ServerMsg::Exit {
                message: Some(format!("couldn't start a shell: {e}")),
            });
            return;
        }
        let client = Client {
            id,
            tx,
            session: name.clone(),
            cols,
            rows,
            parser: Parser::new(),
            prefix: false,
            screen: Screen::default(),
            dirty: true,
            drag: None,
            last_click: None,
            status_hits: Default::default(),
            overlays: Default::default(),
            activity: Instant::now(),
            options_taken: 0,
        };
        // For the `system` theme: the terminal's background (src/status).
        Server::ask_background(&client.tx);
        self.clients.insert(id, client);
        self.size_session_to(id);
        if let Some(tab) = self.session(&name).map(|s| s.current) {
            self.agent_focus(tab);
        }
        self.update_attached(id);
        self.extras_attached(id);
    }

    fn resize_client(&mut self, id: ClientId, cols: u16, rows: u16) {
        let Some(c) = self.clients.get_mut(&id) else {
            return;
        };
        c.cols = cols;
        c.rows = rows;
        c.screen.invalidate();
        c.dirty = true;
        c.activity = Instant::now();
        self.size_session_to(id);
    }

    /// Sizes a client's session to it, as its most recently active client.
    fn size_session_to(&mut self, id: ClientId) {
        let Some(c) = self.clients.get(&id) else {
            return;
        };
        let (name, cols, rows) = (c.session.clone(), c.cols, c.rows);
        let sidebar = self.sidebar_columns(cols);
        let Some(s) = self.session_mut(&name) else {
            return;
        };
        if (s.cols, s.rows) == (cols, rows) {
            return;
        }
        s.cols = cols;
        s.rows = rows;
        s.sidebar = sidebar;
        let tabs = s.tabs.clone();
        for t in tabs {
            self.relayout(t);
        }
        self.dirty_session(&name);
    }

    /// Shows a client another session, as tmux's `switch-client`.
    pub fn switch_client(&mut self, id: ClientId, session: &str) {
        let Some(tab) = self.session(session).map(|s| s.current) else {
            return;
        };
        let Some(c) = self.clients.get_mut(&id) else {
            return;
        };
        if c.session == session {
            return;
        }
        c.session = session.to_string();
        c.screen.invalidate();
        c.dirty = true;
        self.size_session_to(id);
        self.agent_focus(tab);
    }

    /// The client acting now: the most recently active one.
    pub fn latest_client(&self) -> Option<ClientId> {
        self.clients
            .values()
            .max_by_key(|c| c.activity)
            .map(|c| c.id)
    }

    pub fn dirty_session(&mut self, name: &str) {
        for c in self.clients.values_mut().filter(|c| c.session == name) {
            c.dirty = true;
        }
    }

    pub fn dirty_tab(&mut self, tab: TabId) {
        let Some(name) = self
            .session_of_tab(tab)
            .filter(|s| s.current == tab)
            .map(|s| s.name.clone())
        else {
            return;
        };
        self.dirty_session(&name);
    }

    /// Puts text on the clipboard: through the clients' terminals (OSC 52),
    /// which reaches the computer you're at even over SSH, and the desktop's
    /// own clipboard where there is one.
    pub fn to_clipboard(&mut self, client: Option<ClientId>, text: &str) {
        let osc = format!("\x1b]52;c;{}\x07", util::base64(text.as_bytes()));
        let targets: Vec<ClientId> = match client.or_else(|| self.latest_client()) {
            Some(c) => vec![c],
            None => Vec::new(),
        };
        for id in targets {
            if let Some(c) = self.clients.get(&id) {
                let _ = c.tx.send(ServerMsg::Output(osc.clone().into_bytes()));
            }
        }
        let text = text.to_string();
        tokio::task::spawn_blocking(move || {
            let _ = crate::platform::copy(&text);
        });
    }

    // ── Input ────────────────────────────────────────────────────────────────

    fn input(&mut self, id: ClientId, bytes: &[u8]) {
        self.last_input = Instant::now();
        let parsed = match self.clients.get_mut(&id) {
            Some(c) => {
                c.activity = Instant::now();
                c.parser.feed(bytes)
            }
            None => return,
        };
        self.size_session_to(id);
        let taken = |s: &Server| s.clients.get(&id).map(|c| c.options_taken);
        for p in parsed {
            let before = taken(self);
            self.dispatch(id, p);
            // An option taken: the rest of the same read was typed ahead of
            // it, and goes nowhere.
            if taken(self) != before {
                break;
            }
        }
        if self.clients.get(&id).is_some_and(|c| c.parser.pending()) {
            self.flush_later(id, input::ESC_TIMEOUT);
        }
    }

    /// Asks for what a client's parser holds to be let go after `after`.
    fn flush_later(&self, id: ClientId, after: Duration) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            let _ = tx.send(Event::FlushInput(id));
        });
    }

    /// Sends the server an event after `after`, for whatever has to look
    /// again in a moment (the agent state's "seen a second later").
    pub fn send_later(&self, after: Duration, event: Event) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let tx = self.tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            let _ = tx.send(event);
        });
    }

    /// The current pane of a client's session.
    pub fn current_pane(&self, id: ClientId) -> Option<PaneId> {
        let c = self.clients.get(&id)?;
        let s = self.session(&c.session)?;
        Some(self.tabs.get(&s.current)?.active)
    }

    pub(crate) fn dispatch(&mut self, id: ClientId, parsed: input::Parsed) {
        // The terminal's answer about its background, asked as it attached,
        // is the server's (src/status), whatever is up.
        if parsed.input == Input::Unknown && self.status_reply(&parsed.raw) {
            return;
        }
        // Mission control takes them while it is the box on top
        // (src/mission); any other overlay up takes them (src/ui).
        let Some(parsed) = self.mission_input(id, parsed) else {
            return;
        };
        let Some(parsed) = self.overlay_input(id, parsed) else {
            return;
        };
        match parsed.input {
            Input::Key(key) => self.key(id, key, &parsed.raw),
            Input::Mouse(m) => self.mouse(id, m),
            Input::Paste(text) => {
                if self.sidebar_paste(id, &text) {
                    return;
                }
                if let Some(pane) = self.current_pane(id) {
                    self.leave_copy(pane);
                    if let Some(slot) = self.panes.get_mut(&pane) {
                        let bytes = input::encode_paste(&text, &slot.pane.modes());
                        let _ = slot.pane.write(&bytes);
                    }
                }
            }
            Input::FocusIn | Input::FocusOut => {
                if let Some(pane) = self.current_pane(id) {
                    self.send_focus(pane, parsed.input == Input::FocusIn);
                }
            }
            Input::Unknown => {
                if let Some(slot) = self.current_pane(id).and_then(|p| self.panes.get_mut(&p))
                    && input::passes_unknown(&parsed.raw, &slot.pane.modes())
                {
                    let _ = slot.pane.write(&parsed.raw);
                }
            }
        }
    }

    fn key(&mut self, id: ClientId, key: Key, raw: &[u8]) {
        let prefix_pending = self.clients.get(&id).is_some_and(|c| c.prefix);
        if prefix_pending {
            if let Some(c) = self.clients.get_mut(&id) {
                c.prefix = false;
            }
            if let Some(action) = self.bindings.prefix.get(&key).cloned() {
                let pressed = format!("{} {}", self.bindings.prefix_key.notation(), key.notation());
                self.count_key(&pressed);
                self.act(id, action);
            }
            return;
        }
        if key == self.bindings.prefix_key {
            if let Some(c) = self.clients.get_mut(&id) {
                c.prefix = true;
            }
            return;
        }
        // C-w's next key, Alt-arrows with pane_arrows off, and Option's
        // characters where it isn't Alt (src/extras).
        let fate = self.extras_key(id, key);
        if fate == crate::extras::KeyFate::Taken {
            return;
        }
        if fate == crate::extras::KeyFate::Bindings
            && let Some(action) = self.bindings.root.get(&key).cloned()
        {
            self.count_key(&key.notation());
            self.act(id, action);
            return;
        }
        // Easy mode's Cmd keys reach only a pane that takes them.
        if super::passthrough::is_cmd_key(&key) {
            self.cmd_key(id, raw);
            return;
        }
        if self.sidebar_key(id, key, raw) {
            return;
        }
        let Some(pane) = self.current_pane(id) else {
            return;
        };
        // The Notifications window's list takes its pane's keys (src/sidebar).
        if self.notices_key(id, pane, key) {
            return;
        }
        if self.panes.get(&pane).is_some_and(|s| s.copy.is_some()) {
            self.copy_key(id, pane, key);
            return;
        }
        if let Some(slot) = self.panes.get_mut(&pane) {
            if slot.pane.scrolled() > 0 {
                slot.pane.scroll_to_bottom();
            }
            let bytes = input::encode_key(&key, &slot.pane.modes());
            let bytes = if bytes.is_empty() {
                raw.to_vec()
            } else {
                bytes
            };
            let _ = slot.pane.write(&bytes);
        }
    }

    pub fn act(&mut self, id: ClientId, action: Action) {
        let Some(session) = self.clients.get(&id).map(|c| c.session.clone()) else {
            return;
        };
        let Some(s) = self.session(&session) else {
            return;
        };
        let current_tab = s.current;
        let tabs = s.tabs.clone();
        let pane = self.tabs.get(&current_tab).map(|t| t.active);
        // The sidebar's part first: M-s, and a key pressed in it (src/sidebar).
        let (done, side_focused) = self.sidebar_before_action(id, &action);
        if done {
            return;
        }
        // A command of a service turned off in Settings does nothing but
        // say so once, as settings.sh bound every key tagged #github or
        // #linear to the service's tip.
        for service in ["github", "linear"] {
            if action.tags().contains(&service) && !crate::settings::is_on(service) {
                self.service_off(id, service);
                return;
            }
        }
        let after = action.clone();
        match action {
            Action::SelectPane(dir) => {
                let (Some(pane), Some(tab), Some(s)) =
                    (pane, self.tabs.get(&current_tab), self.session(&session))
                else {
                    return;
                };
                if let Some(next) = tab.layout.neighbor(s.main_area(), pane, dir) {
                    self.focus_pane(next);
                } else if dir == super::Direction::Left && !tab.zoomed {
                    self.sidebar_from_left(id);
                }
            }
            Action::PrevTab | Action::NextTab => {
                // The Notifications window, last, is no tab to go round to.
                let tabs: Vec<TabId> = tabs
                    .iter()
                    .copied()
                    .filter(|t| !self.is_notices_tab(*t))
                    .collect();
                let n = tabs.len();
                let next = match tabs.iter().position(|t| *t == current_tab) {
                    Some(at) if action == Action::NextTab => (at + 1) % n,
                    Some(at) => (at + n - 1) % n,
                    None if action == Action::NextTab => 0,
                    None => n.saturating_sub(1),
                };
                if let Some(t) = tabs.get(next) {
                    self.select_tab(&session, *t);
                }
            }
            Action::LastTab => {
                if let Some(last) = self.session(&session).and_then(|s| s.last) {
                    self.select_tab(&session, last);
                }
            }
            Action::SelectTab(n) => {
                if let Some(t) = tabs
                    .get(n.wrapping_sub(1))
                    .filter(|t| !self.is_notices_tab(**t))
                {
                    self.select_tab(&session, *t);
                }
            }
            Action::NewTab => {
                let cwd = pane
                    .and_then(|p| self.panes.get(&p))
                    .and_then(|s| s.pane.cwd());
                if let Err(e) = self.new_tab(&session, cwd, None, None, false) {
                    eprintln!("new tab: {e}");
                }
            }
            Action::Split(dir) => {
                if let Some(p) = pane
                    && let Err(e) = self.split(p, dir, None, None, None, false)
                {
                    eprintln!("split: {e}");
                }
            }
            Action::ClosePane => {
                if let Some(p) = pane {
                    self.close_pane(id, p);
                }
            }
            Action::Zoom => {
                if let Some(tab) = self.tabs.get_mut(&current_tab)
                    && (tab.layout.panes().len() > 1 || tab.zoomed)
                {
                    tab.zoomed = !tab.zoomed;
                }
                self.relayout(current_tab);
                self.dirty_tab(current_tab);
            }
            Action::Resize(dir, cells) => {
                let area = self.session(&session).map(Session::main_area);
                if let (Some(p), Some(area), Some(tab)) =
                    (pane, area, self.tabs.get_mut(&current_tab))
                {
                    tab.layout.resize(area, p, dir, cells);
                }
                self.relayout(current_tab);
                self.dirty_tab(current_tab);
            }
            Action::Detach => {
                if let Some(c) = self.clients.remove(&id) {
                    let _ = c.tx.send(ServerMsg::Exit {
                        message: Some(format!("[detached (from session {session})]")),
                    });
                }
            }
            Action::CopyMode => {
                if let Some(slot) = pane.and_then(|p| self.panes.get_mut(&p)) {
                    slot.copy = Some(CopyMode::default());
                    slot.pane.copy_begin();
                }
                self.dirty_tab(current_tab);
            }
            Action::SendPrefix => {
                let key = self.bindings.prefix_key;
                if let Some(slot) = pane.and_then(|p| self.panes.get_mut(&p)) {
                    let bytes = input::encode_key(&key, &slot.pane.modes());
                    let _ = slot.pane.write(&bytes);
                }
            }
            Action::ListKeys => self.list_keys(id),
            Action::NextNeedsYou => self.next_needs_you(id),
            Action::FocusClaude => self.focus_claude(current_tab),
            Action::TogglePin => self.toggle_pin(current_tab),
            Action::Palette => self.open_palette(id),
            Action::ToggleTheme => self.toggle_theme(id),
            Action::ResetTips => self.reset_tips(),
            Action::NewClaudeTab
            | Action::WorkOnTickets
            | Action::WorkOnBranch
            | Action::WorkOnPr
            | Action::WorkOnNote
            | Action::ContinueShaping => self.work_action(id, action, pane),
            Action::ShapeNote => self.shape_note(id, false),
            Action::ShapeNewTicket => self.shape_note(id, true),
            Action::PrImage => self.image_view(id, false),
            Action::TicketImage => self.image_view(id, true),
            Action::MissionControl => self.toggle_mission(id),
            Action::FindTab => {
                // Doing what the jump tip teaches marks it shown.
                self.tip_seen("jump");
                self.open_mission(
                    id,
                    crate::mission::Opening::Picker(crate::mission::Scope::Tabs),
                )
            }
            Action::ReviveWorktree => self.open_mission(
                id,
                crate::mission::Opening::Picker(crate::mission::Scope::Untabbed),
            ),
            Action::Settings => self.open_mission(id, crate::mission::Opening::Settings),
            Action::RemindTab => self.remind_tab(id, current_tab),
            Action::View(command) => self.view_command(id, command),
            Action::Diff(command) => self.diff_command(id, command),
            Action::SlackRelay => self.slack_relay(true),
            Action::StopSlackRelay => self.slack_relay(false),
            Action::Update(command) => self.update_command(id, command),
            Action::Notes(command) => self.notes_command(id, command, pane),
            Action::Extra(command) => self.extras_command(id, command, pane),
            Action::Autopilot(command) => self.autopilot_command(id, command, pane),
            // Taken by `sidebar_before_action`.
            Action::FocusSidebar | Action::ToggleSidebar => {}
        }
        self.sidebar_after_action(id, side_focused, &after);
    }

    /// Keys in copy mode: vi's, as tmux's copy-mode-vi had them. h j k l,
    /// w b e (W B E), 0 $ ^, g G and H M L move its cursor, C-u C-d C-b C-f
    /// (and the page keys) the view and the cursor with it, C-e and C-y the
    /// view alone; v (or Space), V and C-v start a selection of characters,
    /// lines or a rectangle, which the cursor's moves stretch; y (or Enter)
    /// copies it through the clipboard the mouse's selections go to and
    /// leaves; / and ? ask what to look for down or up, and n and N look
    /// again either way; Esc takes a selection away, else leaves, as q does.
    fn copy_key(&mut self, id: ClientId, pane: PaneId, key: Key) {
        use super::pane::{Motion, SelectKind};
        let k = key.normalized();
        let Some(slot) = self.panes.get_mut(&pane) else {
            return;
        };
        let (_, rows) = slot.pane.size();
        let half = (rows / 2).max(1) as i32;
        let page = rows.max(1) as i32;
        let plain = k.mods == Mods::NONE;
        let ctrl = k.mods == Mods::CTRL;
        let tab = slot.tab;
        // The wheel's copy mode becomes the keyboard's at its first key.
        slot.pane.copy_begin();
        if let Some(c) = slot.copy.as_mut() {
            c.exit_at_bottom = false;
        }
        let motion = match k.code {
            KeyCode::Char('h') | KeyCode::Left if plain => Some(Motion::Left),
            KeyCode::Char('l') | KeyCode::Right if plain => Some(Motion::Right),
            KeyCode::Char('k') | KeyCode::Up if plain => Some(Motion::Up),
            KeyCode::Char('j') | KeyCode::Down if plain => Some(Motion::Down),
            KeyCode::Char('w') if plain => Some(Motion::WordNext),
            KeyCode::Char('b') if plain => Some(Motion::WordBack),
            KeyCode::Char('e') if plain => Some(Motion::WordEnd),
            KeyCode::Char('W') if plain => Some(Motion::BigWordNext),
            KeyCode::Char('B') if plain => Some(Motion::BigWordBack),
            KeyCode::Char('E') if plain => Some(Motion::BigWordEnd),
            KeyCode::Char('0') | KeyCode::Home if plain => Some(Motion::LineStart),
            KeyCode::Char('$') | KeyCode::End if plain => Some(Motion::LineEnd),
            KeyCode::Char('^') if plain => Some(Motion::FirstChar),
            KeyCode::Char('g') if plain => Some(Motion::Top),
            KeyCode::Char('G') if plain => Some(Motion::Bottom),
            KeyCode::Char('H') if plain => Some(Motion::ScreenTop),
            KeyCode::Char('M') if plain => Some(Motion::ScreenMiddle),
            KeyCode::Char('L') if plain => Some(Motion::ScreenBottom),
            KeyCode::Char('u') if ctrl => Some(Motion::Scroll(half)),
            KeyCode::Char('d') if ctrl => Some(Motion::Scroll(-half)),
            KeyCode::Char('b') if ctrl => Some(Motion::Scroll(page)),
            KeyCode::Char('f') if ctrl => Some(Motion::Scroll(-page)),
            KeyCode::PageUp => Some(Motion::Scroll(page)),
            KeyCode::PageDown => Some(Motion::Scroll(-page)),
            _ => None,
        };
        if let Some(m) = motion {
            slot.pane.copy_move(m);
            self.dirty_tab(tab);
            return;
        }
        match k.code {
            KeyCode::Char('y') if ctrl => slot.pane.scroll(1),
            KeyCode::Char('e') if ctrl => slot.pane.scroll(-1),
            KeyCode::Char('v' | ' ') if plain => slot.pane.copy_select(SelectKind::Chars),
            KeyCode::Char('V') if plain => slot.pane.copy_select(SelectKind::Lines),
            KeyCode::Char('v') if ctrl => slot.pane.copy_select(SelectKind::Block),
            KeyCode::Char('y') | KeyCode::Enter if plain => {
                let Some(text) = slot.pane.selection_text() else {
                    return;
                };
                self.leave_copy(pane);
                self.to_clipboard(Some(id), &text);
            }
            KeyCode::Esc if plain && slot.pane.selection_text().is_some() => {
                slot.pane.select_clear();
            }
            KeyCode::Char('q') | KeyCode::Esc if plain => self.leave_copy(pane),
            KeyCode::Char('c') if ctrl => self.leave_copy(pane),
            KeyCode::Char(c @ ('/' | '?')) if plain => self.copy_ask_search(id, pane, c == '/'),
            KeyCode::Char(c @ ('n' | 'N')) if plain => {
                let Some((pattern, forward)) = slot.copy.as_ref().and_then(|c| c.search.clone())
                else {
                    return;
                };
                let forward = forward == (c == 'n');
                if !slot.pane.copy_search(&pattern, forward) {
                    self.say(Some(id), &format!("Pattern not found: {pattern}"));
                }
            }
            _ => {}
        }
        self.dirty_tab(tab);
    }

    /// `/` and `?` in copy mode: what to look for, asked in a box, then
    /// looked for down or up from the cursor.
    fn copy_ask_search(&mut self, id: ClientId, pane: PaneId, forward: bool) {
        let title = if forward { "Search down" } else { "Search up" };
        let ask = crate::mission::ask::Ask::new(
            title,
            "Text, or a regular expression; a capital makes case count.",
        );
        self.open(
            id,
            Box::new(ask),
            Some(Box::new(move |server: &mut Server, id, outcome| {
                let crate::ui::Outcome::Picked(p) = outcome else {
                    return;
                };
                let Some(slot) = server.panes.get_mut(&pane) else {
                    return;
                };
                let Some(copy) = slot.copy.as_mut() else {
                    return;
                };
                copy.search = Some((p.value.clone(), forward));
                let found = slot.pane.copy_search(&p.value, forward);
                let tab = slot.tab;
                if !found {
                    server.say(Some(id), &format!("Pattern not found: {}", p.value));
                }
                server.dirty_tab(tab);
            })),
        );
    }

    fn leave_copy(&mut self, pane: PaneId) {
        if let Some(slot) = self.panes.get_mut(&pane)
            && (slot.copy.take().is_some() || slot.pane.scrolled() > 0)
        {
            slot.pane.copy_end();
            let tab = slot.tab;
            self.dirty_tab(tab);
        }
    }

    // ── The mouse ────────────────────────────────────────────────────────────

    /// The pane under a cell of a client's screen, and the cell in that
    /// pane's own coordinates.
    fn pane_at(&self, id: ClientId, col: u16, row: u16) -> Option<(PaneId, u16, u16)> {
        let c = self.clients.get(&id)?;
        let s = self.session(&c.session)?;
        let tab = self.tabs.get(&s.current)?;
        let area = s.area_for(tab.zoomed);
        let rects = if tab.zoomed {
            vec![(tab.active, area)]
        } else {
            tab.layout.place(area).panes
        };
        rects
            .into_iter()
            .find(|(_, r)| col >= r.x && col < r.right() && row >= r.y && row < r.bottom())
            .map(|(p, r)| (p, col - r.x, row - r.y))
    }

    fn mouse(&mut self, id: ClientId, m: Mouse) {
        if self.sidebar_mouse(id, m) {
            return;
        }
        let Some(c) = self.clients.get(&id) else {
            return;
        };
        let status_row = c.rows.saturating_sub(crate::status::lines());
        if m.row >= status_row && m.kind == MouseKind::Press && m.button == MouseButton::Left {
            // The tabs' row: the ⌘ Commands button opens the palette, a tab
            // is gone to; the second status line takes no clicks.
            let hit = (m.row == status_row)
                .then(|| c.status_hits.at(m.col))
                .flatten();
            let session = c.session.clone();
            if self.status_button(id, hit) {
                return;
            }
            let hit = match hit {
                Some(crate::status::Hit::Tab(i)) => Some(i),
                _ => None,
            };
            if let Some(tab) = hit.and_then(|i| {
                self.session(&session)
                    .and_then(|s| s.tabs.get(i - 1).copied())
            }) {
                self.select_tab(&session, tab);
            }
            return;
        }
        let drag = c.drag;
        match (m.kind, drag) {
            (MouseKind::Drag, Some(Drag::Border { .. })) => {
                self.drag_border(id, m);
                return;
            }
            (MouseKind::Release, Some(Drag::Border { .. })) => {
                if let Some(c) = self.clients.get_mut(&id) {
                    c.drag = None;
                }
                return;
            }
            (MouseKind::Drag, Some(Drag::Select(pane))) => {
                if let Some((p, col, row)) = self
                    .pane_at(id, m.col, m.row)
                    .filter(|(p, _, _)| *p == pane)
                    && let Some(slot) = self.panes.get_mut(&p)
                {
                    slot.pane.select_extend(col, row);
                    let tab = slot.tab;
                    self.dirty_tab(tab);
                }
                return;
            }
            (MouseKind::Drag | MouseKind::Release, Some(Drag::Forward(pane))) => {
                if let Some((p, col, row)) = self
                    .pane_at(id, m.col, m.row)
                    .filter(|(p, _, _)| *p == pane)
                {
                    self.forward_mouse(p, Mouse { col, row, ..m });
                }
                if m.kind == MouseKind::Release
                    && let Some(c) = self.clients.get_mut(&id)
                {
                    c.drag = None;
                }
                return;
            }
            (MouseKind::Release, Some(Drag::Select(pane))) => {
                if let Some(c) = self.clients.get_mut(&id) {
                    c.drag = None;
                }
                let text = self
                    .panes
                    .get(&pane)
                    .and_then(|s| s.pane.selection_text())
                    .filter(|t| !t.is_empty());
                if let Some(text) = text {
                    self.to_clipboard(Some(id), &text);
                }
                return;
            }
            _ => {}
        }
        let Some((pane, col, row)) = self.pane_at(id, m.col, m.row) else {
            if m.kind == MouseKind::Press && m.button == MouseButton::Left {
                self.press_border(id, m.col, m.row);
            }
            return;
        };
        // The Notifications window's list takes the mouse over its pane.
        if self.notices_mouse(id, pane, row, m) {
            return;
        }
        let modes = match self.panes.get(&pane) {
            Some(s) => s.pane.modes(),
            None => return,
        };
        let takes_mouse = modes.mouse != super::MouseReporting::Off
            && self.panes.get(&pane).is_some_and(|s| s.copy.is_none());
        match (m.kind, m.button) {
            (MouseKind::Press, MouseButton::WheelUp | MouseButton::WheelDown) => {
                let up = m.button == MouseButton::WheelUp;
                if takes_mouse {
                    self.forward_mouse(pane, Mouse { col, row, ..m });
                } else if modes.alt_screen && modes.alternate_scroll {
                    let key = Key::new(
                        if up { KeyCode::Up } else { KeyCode::Down },
                        Mods::default(),
                    );
                    if let Some(slot) = self.panes.get_mut(&pane) {
                        let bytes = input::encode_key(&key, &modes).repeat(WHEEL_LINES as usize);
                        let _ = slot.pane.write(&bytes);
                    }
                } else if let Some(slot) = self.panes.get_mut(&pane) {
                    // As tmux.conf's wheel does: into copy mode, out again at the bottom.
                    if up && slot.copy.is_none() {
                        slot.copy = Some(CopyMode {
                            exit_at_bottom: true,
                            search: None,
                        });
                    }
                    slot.pane
                        .scroll(if up { WHEEL_LINES } else { -WHEEL_LINES });
                    if slot.copy.as_ref().is_some_and(|c| c.exit_at_bottom)
                        && slot.pane.scrolled() == 0
                    {
                        slot.copy = None;
                        slot.pane.copy_end();
                    }
                    let tab = slot.tab;
                    self.dirty_tab(tab);
                }
            }
            (MouseKind::Press, button) => {
                if self.current_pane(id) != Some(pane) {
                    self.focus_pane(pane);
                }
                if takes_mouse {
                    self.forward_mouse(pane, Mouse { col, row, ..m });
                    if let Some(c) = self.clients.get_mut(&id) {
                        c.drag = Some(Drag::Forward(pane));
                    }
                } else if button == MouseButton::Left {
                    let now = Instant::now();
                    let count = match self.clients.get(&id).and_then(|c| c.last_click) {
                        Some((at, x, y, n))
                            if now.duration_since(at) < DOUBLE_CLICK
                                && x == m.col
                                && y == m.row =>
                        {
                            n % 3 + 1
                        }
                        _ => 1,
                    };
                    if let Some(c) = self.clients.get_mut(&id) {
                        c.last_click = Some((now, m.col, m.row, count));
                        c.drag = Some(Drag::Select(pane));
                    }
                    if let Some(slot) = self.panes.get_mut(&pane) {
                        match count {
                            1 => slot.pane.select_start(col, row, m.mods.alt),
                            n => slot.pane.select_semantic(col, row, n == 3),
                        }
                        let tab = slot.tab;
                        self.dirty_tab(tab);
                    }
                }
            }
            (MouseKind::Move, _) if takes_mouse && modes.mouse == super::MouseReporting::Motion => {
                self.forward_mouse(pane, Mouse { col, row, ..m });
            }
            _ => {}
        }
    }

    /// A press on a line between panes, which a drag then moves.
    fn press_border(&mut self, id: ClientId, col: u16, row: u16) {
        let Some(s) = self.clients.get(&id).and_then(|c| self.session(&c.session)) else {
            return;
        };
        let Some(tab) = self.tabs.get(&s.current).filter(|t| !t.zoomed) else {
            return;
        };
        let Some((before, after, vertical)) = tab.layout.place(s.main_area()).border_at(col, row)
        else {
            return;
        };
        let drag = Drag::Border {
            tab: tab.id,
            before,
            after,
            vertical,
            at: if vertical { col } else { row },
        };
        if let Some(c) = self.clients.get_mut(&id) {
            c.drag = Some(drag);
        }
    }

    /// The line a press took, moved to where the mouse is: the pane on the
    /// side it moves toward grown into the other, by the cells the other can
    /// spare, as tmux's MouseDrag1Border resized them.
    fn drag_border(&mut self, id: ClientId, m: Mouse) {
        let Some(c) = self.clients.get(&id) else {
            return;
        };
        let Some(Drag::Border {
            tab,
            before,
            after,
            vertical,
            at,
        }) = c.drag
        else {
            return;
        };
        let Some(area) = self
            .session(&c.session)
            .filter(|s| s.current == tab)
            .map(Session::main_area)
        else {
            return;
        };
        let to = if vertical { m.col } else { m.row };
        if to == at {
            return;
        }
        let (pane, dir) = match (to > at, vertical) {
            (true, true) => (before, super::Direction::Right),
            (false, true) => (after, super::Direction::Left),
            (true, false) => (before, super::Direction::Down),
            (false, false) => (after, super::Direction::Up),
        };
        let Some(t) = self.tabs.get_mut(&tab) else {
            return;
        };
        t.layout.resize(area, pane, dir, to.abs_diff(at));
        // Where the line went: the cells the other side could spare may
        // have stopped it short of the mouse.
        let now = t
            .layout
            .place(area)
            .rect(before)
            .map(|r| if vertical { r.right() } else { r.bottom() });
        if let (Some(now), Some(c)) = (now, self.clients.get_mut(&id))
            && let Some(Drag::Border { at, .. }) = &mut c.drag
        {
            *at = now;
        }
        self.relayout(tab);
        self.dirty_tab(tab);
    }

    fn forward_mouse(&mut self, pane: PaneId, m: Mouse) {
        if let Some(slot) = self.panes.get_mut(&pane)
            && let Some(bytes) = input::encode_mouse(&m, &slot.pane.modes())
        {
            let _ = slot.pane.write(&bytes);
        }
    }

    // ── Drawing ──────────────────────────────────────────────────────────────

    pub fn draw_dirty(&mut self) {
        self.sidebar_before_draw();
        let ids: Vec<ClientId> = self
            .clients
            .values()
            .filter(|c| c.dirty)
            .map(|c| c.id)
            .collect();
        for id in ids {
            self.draw(id);
        }
    }

    fn draw(&mut self, id: ClientId) {
        self.mission_before_draw(id);
        let Some(c) = self.clients.get(&id) else {
            return;
        };
        let mut frame = Frame::new(c.cols.max(1), c.rows.max(1));
        let session = c.session.clone();
        let (hits, side) = match self.session(&c.session) {
            Some(s) => self.compose(&mut frame, s, c),
            None => (Default::default(), None),
        };
        let c = self.clients.get_mut(&id).expect("checked above");
        frame.cover_links(|frame| c.overlays.draw(frame));
        c.status_hits = hits;
        c.dirty = false;
        let bytes = c.screen.render(&frame);
        let _ = c.tx.send(ServerMsg::Output(bytes));
        self.sidebar_drawn(id, &session, side);
        self.sync_motion(id);
    }

    fn compose(
        &self,
        frame: &mut Frame,
        s: &Session,
        c: &Client,
    ) -> (crate::status::Hits, Option<crate::sidebar::Drawn>) {
        let rows = frame.area().height;
        let cols = frame.area().width;
        let palette = crate::status::theme::palette();
        // With the sidebar focused, no pane has the focus.
        let side_focused = self.sidebar.focused(&s.name);
        let mut cursor = None;
        if let Some(tab) = self.tabs.get(&s.current) {
            let area = s.area_for(tab.zoomed);
            let main = area.intersection(frame.area());
            let placement = if tab.zoomed {
                super::layout::Placement {
                    panes: vec![(tab.active, area)],
                    borders: Vec::new(),
                }
            } else {
                tab.layout.place(area)
            };
            let mut focused_rect = None;
            for (pid, rect) in &placement.panes {
                let rect = rect.intersection(main);
                let Some(slot) = self.panes.get(pid) else {
                    continue;
                };
                let active = *pid == tab.active && !side_focused;
                let focused = active && c.overlays.is_empty();
                // The Notifications window's pane is drawn over with its
                // list (src/sidebar/notices.rs).
                let at = if self.is_notices_pane(*pid) {
                    self.notices_draw(&mut frame.buf, rect, &s.name, c.id);
                    None
                } else {
                    let at = slot.pane.render(
                        &mut frame.buf,
                        rect,
                        &PaneView {
                            focused,
                            dim: false,
                        },
                    );
                    slot.pane.put_links(rect, frame.area(), &mut frame.links);
                    at
                };
                if active {
                    focused_rect = Some(rect);
                    if slot.copy.is_none() {
                        cursor = at;
                        frame.cursor_style = Some(slot.pane.cursor_style());
                    } else if let Some((x, y)) = slot.pane.copy_cursor().filter(|_| focused) {
                        // Copy mode's own cursor, a block, as tmux drew it.
                        let (x, y) = (rect.x + x, rect.y + y);
                        if x < rect.right() && y < rect.bottom() {
                            cursor = Some((x, y));
                            frame.cursor_style =
                                Some(crossterm::cursor::SetCursorStyle::SteadyBlock);
                        }
                    }
                } else {
                    compose::tint_inactive(&mut frame.buf, rect, palette.inactive_bg);
                }
                if slot.pane.scrolled() > 0 || slot.copy.is_some() {
                    let label = format!("[{}]", slot.pane.scrolled());
                    let x = rect.right().saturating_sub(label.len() as u16);
                    frame
                        .links
                        .clear_area(Rect::new(x, rect.y, label.len() as u16, 1));
                    compose::put(
                        &mut frame.buf,
                        x,
                        Rect::new(x, rect.y, label.len() as u16, 1),
                        &label,
                        Style::new()
                            .bg(compose::colors::ACCENT)
                            .fg(ratatui::style::Color::Indexed(232)),
                    );
                }
            }
            if placement.panes.len() > 1 {
                // The focused pane's lines in the tab's color, as
                // update-colors.sh set pane-active-border-style.
                let focus = self
                    .tab_color(s.current)
                    .map(Into::into)
                    .unwrap_or(compose::colors::ACCENT);
                compose::draw_borders(
                    &mut frame.buf,
                    &placement,
                    focused_rect,
                    focus,
                    palette.border,
                );
            }
        }
        let side = self.sidebar_draw(&mut frame.buf, s, c.id);
        let hits = self.draw_status_rows(frame, s);
        let status = Rect::new(0, rows.saturating_sub(crate::status::lines()), cols, 1);
        if c.prefix {
            compose::put(
                &mut frame.buf,
                cols.saturating_sub(3),
                status,
                " ^ ",
                Style::new()
                    .fg(compose::colors::ACCENT)
                    .add_modifier(Modifier::BOLD),
            );
        }
        frame.cursor = cursor;
        (hits, side)
    }
}

pub(super) fn new_tab(id: TabId, pane: PaneId) -> Tab {
    Tab {
        id,
        name: String::new(),
        auto_name: String::new(),
        layout: Layout::new(pane),
        active: pane,
        last_pane: None,
        zoomed: false,
        opts: BTreeMap::new(),
    }
}
