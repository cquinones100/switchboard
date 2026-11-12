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
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant as TokioInstant;

use super::compose::{self, Frame, Screen};
use super::input::{self, Input, Key, KeyCode, Mods, Mouse, MouseButton, MouseKind, Parser};
use super::keys::{Action, Bindings};
use super::layout::Layout;
use super::pane::{Pane, PaneSpawn, PaneView};
use super::proto::{self, ClientMsg, ServerMsg};
use super::trail::Trail;
use super::{Direction, PaneEvent, PaneId, TabId, TermNotice, socket};
use crate::{paths, util};

/// The session `switchboard` attaches to when none is named.
pub const DEFAULT_SESSION: &str = "main";
/// Lines of history each pane keeps (tmux.conf's `history-limit`).
pub const HISTORY: usize = 10_000;
/// The tab option naming the directory a tab is about when its panes work
/// elsewhere: the review tab's PR worktree (src/cmd/work.rs's `review_pr`),
/// whose Claude stays in the main clone. Saved with the tab's options, and
/// taken away when a pane of the tab is handed over (`respawn`).
pub const TAB_DIR: &str = "@tab_dir";
/// A tab's number, the one it shows everywhere and `M-1` to `M-9` go to:
/// the smallest no other open tab had when it opened, kept while it is open
/// (saved with the layout and handed over with the tab's other options).
pub const TAB_NUMBER: &str = "@tab_number";
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
/// How long a server ending waits for its panes' programs to end once hung
/// up, before it goes and their terminals with it.
const HANG_UP_WAIT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClientId(pub u32);

/// Where a new tab opens, by one rule: a tab that something you did opens
/// (a key, a palette command, a menu's choice, a card's action, a command
/// run without `-d`) comes to the front, so it is what you see; one the
/// workspace opens on its own (autopilot's, the day's loop tabs, the
/// Notifications window, the Slack relay's) stays behind the one in front.
/// Several opened at once open behind and the first is fronted after
/// (`Server::front`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opened {
    Front,
    Background,
}

/// What the server task is told.
pub enum Event {
    /// A terminal attached.
    Attach {
        id: ClientId,
        session: Option<String>,
        cols: u16,
        rows: u16,
        /// What the terminal says it is (its `TERM`, `TERM_PROGRAM` and `COLORTERM`).
        term: String,
        /// Where the terminal's `switchboard` ran: a session made for it
        /// starts there.
        cwd: Option<PathBuf>,
        tx: mpsc::UnboundedSender<ServerMsg>,
        /// Its connection, for handing it to a new build.
        hold: mpsc::UnboundedSender<Hold>,
    },
    Input(ClientId, Vec<u8>),
    Resize(ClientId, u16, u16),
    /// A cell's width and height in pixels, from a terminal's window size
    /// when it said its pixels (src/mux/terminal.rs).
    Cell(u16, u16),
    /// A client left or its connection ended.
    Gone(ClientId),
    /// A command from a shell or a hook, answered on `reply`.
    Command {
        args: Vec<String>,
        cwd: PathBuf,
        pane: Option<String>,
        stdin: Option<Vec<u8>>,
        reply: oneshot::Sender<Answer>,
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
    /// What autopilot found off the task (src/autopilot).
    Autopilot(crate::autopilot::AutopilotEvent),
    /// The check of a new build to restart on answered (src/mux/upgrade.rs).
    Upgrade(super::upgrade::UpgradeEvent),
    /// A person's photo loaded, or failed to (src/photos).
    Photos(crate::photos::PhotosEvent),
    /// A transcript read for a tab's history (src/history).
    History(crate::history::serve::HistoryEvent),
    /// A picture's file looked at, or a piece of it drawn as sixel, off
    /// the task (src/mux/sixel).
    Sixel(super::sixel::SixelEvent),
    /// The hold keeping the computer awake taken, off the task (src/awake).
    Awake(crate::awake::serve::AwakeEvent),
    /// A routine's directory checked, off the task (src/routines).
    Routines(crate::routines::RoutinesEvent),
    Tick,
    Quit,
}

/// How a command is answered: with its reply, or, for `switchboard
/// upgrade`, by handing its connection back to be answered by the build the
/// server restarts on (src/mux/upgrade.rs).
#[derive(Debug)]
pub enum Answer {
    Reply(Reply),
    Keep(oneshot::Sender<tokio::net::UnixStream>),
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
    /// The tabs visited before and after the current one, for M-[ and M-].
    pub trail: Trail,
    /// The size its tabs are laid out for: its latest client's.
    pub cols: u16,
    pub rows: u16,
    /// The columns the sidebar takes on the left, its line beside the panes
    /// included; 0 with none (src/sidebar).
    pub sidebar: u16,
}

impl Session {
    /// The area panes are laid out in: all but the status line (its one or
    /// two rows, src/status), the sidebar, and with UX 2.0 the tab's header
    /// at the top (src/mux/header.rs).
    pub fn main_area(&self) -> Rect {
        let side = self.sidebar.min(self.cols.saturating_sub(1));
        let below = crate::status::lines();
        let top = crate::status::top().min(self.rows.saturating_sub(below + 1));
        Rect::new(
            side,
            top,
            self.cols.saturating_sub(side).max(1),
            self.rows.saturating_sub(below + top).max(1),
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

/// Copy mode the wheel brought up ends once the view is back at the bottom,
/// whether the wheel or the scroll bar took it there; copy mode entered by
/// hand stays until `q`.
fn leave_wheel_copy_at_bottom(slot: &mut PaneSlot) {
    if slot.copy.as_ref().is_some_and(|c| c.exit_at_bottom) && slot.pane.scrolled() == 0 {
        slot.copy = None;
        slot.pane.copy_end();
    }
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
    /// A scrolled-back pane's scroll bar, its thumb dragged from where
    /// (`kit::scroll_grab`).
    Scrollbar(PaneId, crate::ui::kit::Grab),
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
    /// Its connection's task, for handing it to a new build.
    pub(super) hold: mpsc::UnboundedSender<Hold>,
    pub session: String,
    pub cols: u16,
    pub rows: u16,
    /// What its terminal says it is, as it attached (`TERM`,
    /// `TERM_PROGRAM` and `COLORTERM`).
    pub term: String,
    parser: Parser,
    prefix: bool,
    pub(super) screen: Screen,
    pub dirty: bool,
    drag: Option<Drag>,
    last_click: Option<(Instant, u16, u16, u8)>,
    /// Where the status line's button and tabs were drawn, for clicks.
    status_hits: crate::status::Hits,
    /// Where UX 2.0's header drew its keys, for clicks, and the one under
    /// the mouse (src/mux/header.rs).
    pub header_keys: super::header::Keys,
    pub header_hover: Option<super::header::Button>,
    /// Where the left click being handled was pressed, while it is handled
    /// alone, so a box it opens opens beside it (`Server::open`); never
    /// set while a key is handled.
    pub pressed_at: Option<(u16, u16)>,
    /// What is shown over the tab, the top one taking the input (src/ui).
    pub overlays: crate::ui::Stack,
    pub activity: Instant,
    /// Counts the options taken in mission control's menus, so the keys read
    /// with the one that took an option are dropped rather than reaching
    /// what is in front next (the daemon's `optionsTaken`).
    pub options_taken: u32,
    /// Whether the server has set its terminal's own colors to the dark
    /// theme's (the `terminal_colors` setting), to be put back as it goes.
    pub terminal_colors: bool,
}

impl Client {
    /// Whether the server draws its images in this client's terminal
    /// (src/ui/images.rs): one that speaks kitty's graphics protocol, with
    /// the `server_images` setting on.
    pub fn graphics(&self) -> bool {
        crate::ui::images::speaks_kitty(&self.term) && crate::settings::is_on("server_images")
    }

    /// Whether its terminal draws truecolor, so the dark theme is drawn in
    /// the canvas's colors for it (src/status/theme.rs).
    pub fn truecolor(&self) -> bool {
        crate::status::theme::draws_truecolor(&self.term)
    }

    /// Tells its terminal's client to end with `message`, putting the
    /// terminal's own colors back first where the server set them.
    pub fn exit(&self, message: String) {
        if self.terminal_colors {
            let put_back = crate::status::theme::PUT_COLORS_BACK.to_vec();
            let _ = self.tx.send(ServerMsg::Output(put_back));
        }
        let _ = self.tx.send(ServerMsg::Exit {
            message: Some(message),
        });
    }
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
    /// here when the desktop can't see it: where it can't say, or the client
    /// is on another computer. The server's start until one is pressed.
    pub last_input: Instant,
    /// Whether a key has been pressed in a client since the server started:
    /// until one has, `last_input` is no key, and counts only where the
    /// desktop can't say how long you've been idle.
    pub typed: bool,
    /// The settings file's modification time when last read, so a change
    /// made in the Settings view or by hand takes effect on the next tick.
    settings_seen: Option<std::time::SystemTime>,
    /// What the desktop says of whether you're at the computer, read at
    /// most every `PRESENCE_EVERY` off the server's task (src/presence.rs).
    presence: crate::presence::Desktop,
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
    /// People's photos for their avatars, as they load (src/photos).
    pub photos: crate::photos::Photos,
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
    /// Autopilot (src/autopilot).
    pub autopilot: crate::autopilot::Autopilot,
    /// The computer kept awake while tabs work (src/awake).
    pub awake: crate::awake::Awake,
    /// The routines and their runs (src/routines).
    pub routines: crate::routines::Routines,
    /// What the terminal attached last says of itself: its cell's size
    /// (src/mux/terminal.rs).
    pub terminal: super::terminal::Terminal,
    /// A restart on a new build under way (src/mux/upgrade.rs).
    pub(super) upgrade: super::upgrade::Upgrade,
    /// The pictures panes' programs gave the terminal, and those drawn as
    /// sixel (src/mux/sixel).
    pub sixel: super::sixel::Sixel,
    /// What each pane's Claude session did, for its tab's history (src/history).
    pub history: crate::history::Histories,
}

// ── Running it ──────────────────────────────────────────────────────────────

/// `switchboard server`; with `--handover <file>`, the server a running one
/// replaced itself with (src/mux/upgrade.rs), taking up what it handed
/// over; `--handover-version` prints the handover's version this build
/// takes, which a running server asks before replacing itself with it.
pub fn main(args: Vec<String>) -> ExitCode {
    let handover = match args.as_slice() {
        [] => None,
        [flag] if flag == "--handover-version" => {
            println!("{}", super::upgrade::VERSION);
            return ExitCode::SUCCESS;
        }
        [flag, file] if flag == "--handover" => Some(PathBuf::from(file)),
        _ => {
            eprintln!("Usage: switchboard server   (started for you by switchboard)");
            return ExitCode::from(2);
        }
    };
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
    match runtime.block_on(run(handover)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("switchboard server: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(handover: Option<PathBuf>) -> Result<()> {
    super::upgrade::remember_binary();
    let (listener, handed) = match handover {
        None => (socket::bind()?, None),
        Some(file) => {
            let handed = super::upgrade::read_file(&file)
                .map_err(|e| anyhow!("taking over from the server before: {e:#}"))?;
            let listener = socket::adopt(handed.listener, handed.lock)?;
            (listener, Some(handed))
        }
    };
    let (tx, mut rx) = mpsc::unbounded_channel::<Event>();
    let (pane_tx, mut pane_rx) = mpsc::unbounded_channel::<PaneEvent>();
    let busy = Arc::new(AtomicUsize::new(0));
    let mut acceptor = Acceptor::start(listener, tx.clone(), busy.clone());
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
    // Switchboard's plugin, which Claude in the panes loads, as this build
    // has it: off the server's task, and before the restored panes' Claude
    // sessions take the time a shell and Claude take to start. Not by a
    // server with a state directory of its own (a test's, or a worktree's
    // build run by hand), which shares your home: there it would write over
    // the plugin every pane of your workspace loads.
    if !crate::setup::state_of_its_own() {
        tokio::task::spawn_blocking(|| {
            if let Err(e) = crate::setup::plugin::write_quietly() {
                eprintln!("switchboard server: Switchboard's plugin for Claude Code: {e:#}");
            }
        });
    }
    // A setting saved under its old name is read as its new one once,
    // before anything reads them.
    if let Err(e) = crate::settings::migrate() {
        eprintln!("switchboard server: settings: {e:#}");
    }
    crate::settings::reload_settings();
    let mut server = Server::new(tx, pane_tx);
    let (bindings, problems) = Bindings::load();
    server.bindings = bindings;
    for p in &problems {
        eprintln!("keys.json: {p}");
    }
    server.note_at_start(&problems);
    server.sidebar_load();
    server.notice_history_load();
    // The status line's rows, before any tab is laid out.
    server.status_tick();
    match handed {
        Some(handed) => super::upgrade::adopt(&mut server, handed),
        None => super::persist::restore(&mut server),
    }
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
        if server.upgrade_ready() {
            let mut lp = Loop {
                rx: &mut rx,
                pane_rx: &mut pane_rx,
                acceptor: &mut acceptor,
                busy: &busy,
            };
            // Returns only when the new build couldn't take over.
            super::upgrade::hand_over(&mut server, &mut lp).await;
        }
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

/// What the server's loop lends the handover (src/mux/upgrade.rs): its
/// channels, to go on handling events while everything settles, the
/// connections' acceptor, and how many connections are under way.
pub(super) struct Loop<'a> {
    pub rx: &'a mut mpsc::UnboundedReceiver<Event>,
    pub pane_rx: &'a mut mpsc::UnboundedReceiver<PaneEvent>,
    pub acceptor: &'a mut Acceptor,
    pub busy: &'a AtomicUsize,
}

/// The task taking connections on the server's socket, which can be stopped
/// and given the socket back, for handing it to a new build. A connection
/// made meanwhile waits in the socket's queue for whoever takes it next.
pub(super) struct Acceptor {
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<tokio::net::UnixListener>>,
    tx: mpsc::UnboundedSender<Event>,
    busy: Arc<AtomicUsize>,
}

impl Acceptor {
    fn start(
        listener: tokio::net::UnixListener,
        tx: mpsc::UnboundedSender<Event>,
        busy: Arc<AtomicUsize>,
    ) -> Acceptor {
        let mut acceptor = Acceptor {
            stop: None,
            task: None,
            tx,
            busy,
        };
        acceptor.run(listener);
        acceptor
    }

    fn run(&mut self, listener: tokio::net::UnixListener) {
        let (stop, stopped) = oneshot::channel();
        self.stop = Some(stop);
        self.task = Some(tokio::spawn(accept(
            listener,
            self.tx.clone(),
            self.busy.clone(),
            stopped,
        )));
    }

    /// Stops taking connections, giving the socket back.
    pub(super) async fn stop(&mut self) -> Option<std::os::unix::net::UnixListener> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let listener = self.task.take()?.await.ok()?;
        listener.into_std().ok()
    }

    /// Takes connections again, on the socket given back (after a handover
    /// that didn't happen).
    pub(super) fn restart(&mut self, listener: Option<std::os::unix::net::UnixListener>) {
        let Some(listener) = listener else {
            return;
        };
        match tokio::net::UnixListener::from_std(listener) {
            Ok(listener) => self.run(listener),
            Err(e) => eprintln!("switchboard server: listening again: {e}"),
        }
    }
}

/// A connection under way, counted until it is answered or attached, so a
/// handover waits for the commands under way to be answered.
struct Busy(Arc<AtomicUsize>);

impl Busy {
    fn new(count: &Arc<AtomicUsize>) -> Busy {
        count.fetch_add(1, Ordering::SeqCst);
        Busy(count.clone())
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn accept(
    listener: tokio::net::UnixListener,
    tx: mpsc::UnboundedSender<Event>,
    busy: Arc<AtomicUsize>,
    mut stopped: oneshot::Receiver<()>,
) -> tokio::net::UnixListener {
    loop {
        tokio::select! {
            biased;
            _ = &mut stopped => return listener,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let busy = Busy::new(&busy);
                    tokio::spawn(connection(stream, tx.clone(), busy));
                }
                Err(e) => {
                    eprintln!("switchboard server: accepting a connection: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
        }
    }
}

async fn connection(stream: tokio::net::UnixStream, tx: mpsc::UnboundedSender<Event>, busy: Busy) {
    let (mut r, mut w) = stream.into_split();
    let Ok(Some(first)) = proto::read_client_async(&mut r).await else {
        return;
    };
    match first {
        ClientMsg::Attach {
            session,
            cols,
            rows,
            term,
            xpixel,
            ypixel,
            cwd,
        } => {
            let id = ClientId(NEXT_CLIENT.fetch_add(1, Ordering::Relaxed));
            let (otx, orx) = mpsc::unbounded_channel::<ServerMsg>();
            let (htx, hrx) = mpsc::unbounded_channel::<Hold>();
            if tx
                .send(Event::Attach {
                    id,
                    session,
                    cols,
                    rows,
                    term,
                    cwd: cwd.map(PathBuf::from),
                    tx: otx,
                    hold: htx,
                })
                .is_err()
            {
                return;
            }
            if let Some((w, h)) = super::terminal::cell_of(cols, rows, xpixel, ypixel) {
                let _ = tx.send(Event::Cell(w, h));
            }
            drop(busy);
            // The first frame was read whole and nothing past it, so the
            // rest of the stream is the attachment's.
            let Ok(stream) = r.reunite(w) else {
                return;
            };
            serve_client(stream, id, tx, orx, hrx, Vec::new(), Vec::new()).await;
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
            let reply = match rrx.await {
                Ok(Answer::Reply(reply)) => reply,
                Ok(Answer::Keep(back)) => {
                    if let Ok(stream) = r.reunite(w) {
                        let _ = back.send(stream);
                    }
                    return;
                }
                Err(_) => Reply::fail("the server ended"),
            };
            let msg = ServerMsg::Reply {
                code: reply.code,
                stdout: reply.stdout.into_bytes(),
                stderr: reply.stderr.into_bytes(),
            };
            let _ = w.write_all(&proto::encode_server(&msg)).await;
            let _ = w.shutdown().await;
            drop(busy);
        }
        _ => {}
    }
}

/// What the server asks of an attached terminal's connection, to hand it to
/// a new build (src/mux/upgrade.rs).
#[derive(Debug)]
pub enum Hold {
    /// Read nothing more from the terminal: what it sends meanwhile waits
    /// in the socket for the new build.
    Pause,
    /// Read again: the handover didn't happen.
    Resume,
    /// Write what is queued (for a moment at most) and give the connection
    /// back with what is read of a frame and not yet written; `None` for a
    /// terminal leaving anyway.
    Give(oneshot::Sender<Option<Held>>),
}

/// An attached terminal's connection, given back by its task.
#[derive(Debug)]
pub struct Held {
    pub stream: std::os::unix::net::UnixStream,
    /// Bytes read of a frame not yet whole.
    pub input: Vec<u8>,
    /// Bytes of frames not yet written.
    pub output: Vec<u8>,
}

/// How long a connection given back has to write what is queued for it; the
/// rest goes to the new build to write.
const FLUSH: Duration = Duration::from_millis(500);
/// Output held for a terminal that isn't reading before what the server
/// sends waits in the channel instead.
const OUTPUT_HELD: usize = 1 << 20;

/// An attached terminal's connection: what it sends, read into a buffer of
/// the task's own and taken off it a frame at a time, goes to the server as
/// events; what the server sends is written as frames. It ends once the
/// terminal has left and the server has let it go. Reading into a buffer,
/// and writing from one, is what lets the connection stop between two
/// frames and be handed to a new build with the frame read or written half
/// (`Hold`), and taken up there again with them (`input`, `output`).
pub(super) async fn serve_client(
    stream: tokio::net::UnixStream,
    id: ClientId,
    tx: mpsc::UnboundedSender<Event>,
    mut out: mpsc::UnboundedReceiver<ServerMsg>,
    mut hold: mpsc::UnboundedReceiver<Hold>,
    mut input: Vec<u8>,
    mut output: Vec<u8>,
) {
    // Taking input, until the terminal leaves or a handover stops it.
    let mut reading = true;
    let mut left = false;
    let mut out_closed = false;
    let mut hold_closed = false;
    // An Exit frame queued, and how many bytes of the output end with it;
    // once it is written nothing more is.
    let mut exit_at: Option<usize> = None;
    let mut written_up = false;
    let leave = |left: &mut bool, reading: &mut bool| {
        *reading = false;
        if !*left {
            *left = true;
            let _ = tx.send(Event::Gone(id));
        }
    };
    let take = |input: &mut Vec<u8>, left: &mut bool, reading: &mut bool| loop {
        match proto::take_client(input) {
            Ok(Some(ClientMsg::Input(bytes))) => {
                let _ = tx.send(Event::Input(id, bytes));
            }
            Ok(Some(ClientMsg::Resize {
                cols,
                rows,
                xpixel,
                ypixel,
            })) => {
                let _ = tx.send(Event::Resize(id, cols, rows));
                if let Some((w, h)) = super::terminal::cell_of(cols, rows, xpixel, ypixel) {
                    let _ = tx.send(Event::Cell(w, h));
                }
            }
            Ok(Some(ClientMsg::Detach)) | Err(_) => {
                input.clear();
                leave(left, reading);
                return;
            }
            Ok(Some(_)) => {}
            Ok(None) => return,
        }
    };
    take(&mut input, &mut left, &mut reading);
    loop {
        if left && out_closed && (output.is_empty() || written_up) {
            break;
        }
        tokio::select! {
            biased;
            asked = hold.recv(), if !hold_closed => match asked {
                Some(Hold::Pause) => reading = false,
                Some(Hold::Resume) => {
                    reading = !left;
                    take(&mut input, &mut left, &mut reading);
                }
                Some(Hold::Give(back)) => {
                    while let Ok(msg) = out.try_recv() {
                        if exit_at.is_none() {
                            output.extend_from_slice(&proto::encode_server(&msg));
                            if matches!(msg, ServerMsg::Exit { .. }) {
                                exit_at = Some(output.len());
                            }
                        }
                    }
                    if left || written_up || exit_at.is_some() {
                        let _ = back.send(None);
                        return;
                    }
                    let deadline = TokioInstant::now() + FLUSH;
                    while !output.is_empty() {
                        match tokio::time::timeout_at(deadline, stream.writable()).await {
                            Ok(Ok(())) => match stream.try_write(&output) {
                                Ok(n) => {
                                    output.drain(..n);
                                }
                                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                                Err(_) => {
                                    let _ = back.send(None);
                                    return;
                                }
                            },
                            _ => break,
                        }
                    }
                    let held = stream.into_std().ok().map(|stream| Held {
                        stream,
                        input,
                        output,
                    });
                    let _ = back.send(held);
                    return;
                }
                None => hold_closed = true,
            },
            ready = stream.readable(), if reading => {
                if ready.is_err() {
                    leave(&mut left, &mut reading);
                    continue;
                }
                let mut buf = [0u8; 16 * 1024];
                match stream.try_read(&mut buf) {
                    Ok(0) => leave(&mut left, &mut reading),
                    Ok(n) => {
                        input.extend_from_slice(&buf[..n]);
                        take(&mut input, &mut left, &mut reading);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => leave(&mut left, &mut reading),
                }
            }
            msg = out.recv(), if !out_closed && output.len() < OUTPUT_HELD => match msg {
                Some(msg) => {
                    if exit_at.is_none() && !written_up {
                        output.extend_from_slice(&proto::encode_server(&msg));
                        if matches!(msg, ServerMsg::Exit { .. }) {
                            exit_at = Some(output.len());
                        }
                    }
                }
                None => out_closed = true,
            },
            ready = stream.writable(), if !output.is_empty() && !written_up => {
                if ready.is_err() {
                    written_up = true;
                    continue;
                }
                match stream.try_write(&output) {
                    Ok(n) => {
                        output.drain(..n);
                        if let Some(at) = exit_at.as_mut() {
                            *at = at.saturating_sub(n);
                            // The Exit written: the terminal is told it is
                            // done, and nothing more is written.
                            if *at == 0 {
                                written_up = true;
                                output.clear();
                                shut_write(&stream);
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => {
                        written_up = true;
                        output.clear();
                    }
                }
            }
            // Nothing left to wait on: the server let go of a terminal it
            // had stopped reading.
            else => break,
        }
    }
    shut_write(&stream);
}

/// Says to the peer that nothing more will be written.
fn shut_write(stream: &tokio::net::UnixStream) {
    use std::os::fd::AsRawFd;
    // SAFETY: shuts the writing half of a socket held here.
    unsafe { libc::shutdown(stream.as_raw_fd(), libc::SHUT_WR) };
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
            typed: false,
            settings_seen: None,
            presence: Default::default(),
            presence_asked: None,
            persist: Default::default(),
            ui: Default::default(),
            usage: Default::default(),
            day: Default::default(),
            photos: Default::default(),
            sidebar: Default::default(),
            mission: Default::default(),
            views: Default::default(),
            status: Default::default(),
            update: Default::default(),
            extras: Default::default(),
            terminal: Default::default(),
            autopilot: Default::default(),
            awake: Default::default(),
            routines: Default::default(),
            history: Default::default(),
            upgrade: Default::default(),
            sixel: super::sixel::Sixel::new(),
        }
    }

    pub fn handle(&mut self, event: Event) {
        // A client's keys count for mission control as `dispatch` says;
        // a spinner's turn changes only its frame.
        if !matches!(
            event,
            Event::Input(..)
                | Event::FlushInput(_)
                | Event::Mission(crate::mission::MissionEvent::Spin)
        ) {
            self.mission_changed();
        }
        match event {
            Event::Attach {
                id,
                session,
                cols,
                rows,
                term,
                cwd,
                tx,
                hold,
            } => self.attach(id, session, cols, rows, term, cwd, tx, hold),
            Event::Input(id, bytes) => self.input(id, &bytes),
            Event::Resize(id, cols, rows) => self.resize_client(id, cols, rows),
            Event::Cell(w, h) => self.terminal_cell((w, h)),
            Event::Gone(id) => {
                if let Some(c) = self.clients.remove(&id) {
                    self.client_left(&c.session);
                }
            }
            Event::Command {
                args,
                cwd,
                pane,
                stdin,
                reply,
            } => {
                // Answered once the build it restarts on has taken over.
                if super::upgrade::asks_upgrade(&args) {
                    self.upgrade_asked(Some(reply), None, None);
                    return;
                }
                let answer = self.command(&args, &cwd, pane.as_deref(), stdin.as_deref());
                let _ = reply.send(Answer::Reply(answer));
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
            Event::Presence(p) => self.presence.take(p),
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
            Event::Upgrade(event) => self.upgrade_event(event),
            Event::Photos(event) => self.photos_event(event),
            Event::History(event) => self.history_event(event),
            Event::Sixel(event) => self.sixel_event(event),
            Event::Awake(event) => self.awake_event(event),
            Event::Routines(event) => self.routines_event(event),
            Event::Tick => {
                self.reload_settings_if_changed();
                self.sixel_tick();
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
                self.routines_tick();
                self.awake_check();
                self.history_tick();
                self.notice_history_tick();
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
                self.history_output(id);
                if !allowed {
                    self.forget_images(id);
                }
                let mut graphics = Vec::new();
                for notice in notices {
                    match notice {
                        TermNotice::Clipboard(text) => self.to_clipboard(None, &text),
                        TermNotice::Graphics(command) => graphics.push(command),
                        TermNotice::Title(_) | TermNotice::Bell => {}
                    }
                }
                self.take_graphics(id, tab, graphics);
                self.dirty_tab(tab);
            }
            PaneEvent::Exited(id, generation, code) => {
                self.mission_changed();
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
        // Your PRs' news follows its setting at once, and the terminals'
        // own colors theirs.
        self.pr_notices();
        self.terminal_colors_sync();
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
        self.notice_history_record(source);
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
    /// setting): no client attached, the screen locked, or idle `AWAY_MS`
    /// both by the desktop's reckoning and since a key was pressed in a
    /// client, which counts for a client on another computer, whose keys the
    /// desktop never sees (src/presence.rs).
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
        let since = self.last_input.elapsed().as_secs_f64() * 1000.0;
        crate::presence::away_by(&self.presence.with_workspace(since, self.typed), AWAY_MS)
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
            client.exit("[the server ended]".into());
        }
        for slot in self.panes.values_mut() {
            slot.pane.kill();
        }
        // Each program, a closed pane's still ending too, is given a moment
        // to end while its terminal is still there: one that catches the
        // hangup and ends at its next turn (the diff, Notes, the views)
        // would otherwise find the terminal gone under it as the server's
        // end closes it, and crossterm, reading, then reads its end over and
        // over.
        let deadline = Instant::now() + HANG_UP_WAIT;
        while Instant::now() < deadline && super::pty::programs_running() > 0 {
            std::thread::sleep(Duration::from_millis(10));
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

    /// A tab's number (`TAB_NUMBER`), 0 for the Notifications window, which
    /// takes none.
    pub fn tab_number(&self, tab: TabId) -> usize {
        self.tabs
            .get(&tab)
            .and_then(|t| t.opts.get(TAB_NUMBER))
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    }

    /// The open tab numbered `n`, in any session.
    pub fn tab_numbered(&self, n: usize) -> Option<TabId> {
        if n == 0 {
            return None;
        }
        self.sessions
            .iter()
            .flat_map(|s| s.tabs.iter().copied())
            .find(|t| self.tab_number(*t) == n)
    }

    /// Gives every tab without a number (one just opened, or one from a
    /// build before numbers) the smallest no other open tab has, as
    /// `fill_numbers` says. The Notifications window takes none.
    pub fn number_tabs(&mut self) {
        let (notices, order): (Vec<TabId>, Vec<TabId>) = self
            .sessions
            .iter()
            .flat_map(|s| s.tabs.iter().copied())
            .partition(|t| self.is_notices_tab(*t));
        for t in notices {
            if let Some(tab) = self.tabs.get_mut(&t) {
                tab.opts.remove(TAB_NUMBER);
            }
        }
        let have: Vec<usize> = order.iter().map(|t| self.tab_number(*t)).collect();
        for ((tab, had), n) in order.into_iter().zip(&have).zip(fill_numbers(&have)) {
            if *had != n
                && let Some(t) = self.tabs.get_mut(&tab)
            {
                t.opts.insert(TAB_NUMBER.into(), n.to_string());
            }
        }
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
    /// login shell when `None`) in `cwd`, its environment naming the server,
    /// the pane and, while Settings' "Load Switchboard as a plugin" is on,
    /// Switchboard's plugin for the Claude Code that runs there.
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
        // Claude Code loads the plugin directories this names for the
        // session (src/setup/plugin.rs): Switchboard's skills, hooks and
        // instructions, for Claude here alone. Any the server was started
        // with are kept. Off, setup has put them in ~/.claude instead
        // (src/setup/way.rs), and the plugin would run them twice.
        let plugin = crate::setup::plugin::ENV;
        let inherited = std::env::var(plugin).ok();
        let now = crate::setup::plugin::value_now(
            inherited.as_deref(),
            crate::settings::is_on(crate::setup::way::SETTING),
            &paths::plugin_dir(),
            &paths::home(),
        );
        match now {
            Some(value) => env.push((plugin.to_string(), value)),
            None if inherited.is_some() => env.push((plugin.to_string(), String::new())),
            None => {}
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
        let mut pane = Pane::spawn(spec, self.pane_tx.clone())?;
        if let Some(cell) = self.terminal.cell {
            pane.set_cell(cell);
        }
        pane.set_colors(self.pane_colors());
        Ok(pane)
    }

    /// Makes a session with one tab, unless it is there.
    pub fn ensure_session(
        &mut self,
        name: &str,
        cols: u16,
        rows: u16,
        cwd: Option<PathBuf>,
    ) -> Result<()> {
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
            trail: Trail::default(),
            cols,
            rows,
            sidebar: self.sidebar_columns(cols),
        };
        let area = session.main_area();
        // In the directory asked from, while it is still there, else home.
        let dir = cwd.filter(|d| d.is_dir()).unwrap_or_else(paths::home);
        let pane = self.spawn_pane(tab, Some(dir), None, area.width, area.height)?;
        self.tabs.insert(tab, new_tab(tab, pane));
        self.sessions.push(session);
        self.number_tabs();
        self.refresh_name(tab);
        Ok(())
    }

    /// A new tab in a session, after its current one, made current when
    /// `opened` is `Opened::Front`; gives the tab and its pane.
    pub fn new_tab(
        &mut self,
        session: &str,
        cwd: Option<PathBuf>,
        command: Option<Vec<String>>,
        name: Option<String>,
        opened: Opened,
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
        self.number_tabs();
        let s = self
            .session_mut(session)
            .expect("the session was just found");
        if opened == Opened::Front {
            s.last = Some(s.current);
            s.trail.visited(s.current);
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
        let cwd = cwd.or_else(|| self.pane_own_dir(target));
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
        self.number_tabs();
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
        // Whichever way it goes, a pane parked behind a tool included, the
        // links you gave its Claude go with it (src/views/given.rs).
        self.views.given.forget(pane);
        // A tool in a pane's place gives the place back (src/diff/in_place.rs).
        if self.give_place_back(pane) {
            return;
        }
        let Some(slot) = self.panes.remove(&pane) else {
            return;
        };
        self.history.forget(pane);
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
            self.views.given.forget(p);
        }
        self.remove_tab(tab);
    }

    fn remove_tab(&mut self, tab: TabId) {
        self.tab_closing(tab);
        self.tabs.remove(&tab);
        // A tab closed drops out of the sidebar's select mode.
        for side in self.sidebar.sides.values_mut() {
            if let Some(picked) = side.select.as_mut() {
                picked.remove(&tab);
            }
        }
        let Some(si) = self.sessions.iter().position(|s| s.tabs.contains(&tab)) else {
            return;
        };
        let s = &mut self.sessions[si];
        let at = s.tabs.iter().position(|t| *t == tab).unwrap_or(0);
        s.tabs.retain(|t| *t != tab);
        if s.last == Some(tab) {
            s.last = None;
        }
        s.trail.forget(tab);
        if s.tabs.is_empty() {
            let name = s.name.clone();
            self.sessions.remove(si);
            self.session_ended(&name);
            return;
        }
        if s.current == tab {
            // As tmux does: the tab you were on before, else the one beside it.
            s.current = s.last.take().unwrap_or(s.tabs[at.min(s.tabs.len() - 1)]);
            // The tab it falls back to is a new place, as a visit is.
            s.trail.forward.clear();
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
                None => client.exit("[exited]".into()),
            }
        }
        if other.is_none() {
            self.clients.clear();
            self.quit = true;
        }
    }

    pub fn select_tab(&mut self, session: &str, tab: TabId) {
        self.select_tab_as(session, tab, false);
    }

    /// Goes to a tab, as `select_tab`; `stepping` when it is a step back or
    /// forward through the tabs visited, which moves along the session's
    /// trail rather than adding to it.
    fn select_tab_as(&mut self, session: &str, tab: TabId, stepping: bool) {
        let Some(s) = self.session_mut(session) else {
            return;
        };
        let switched = s.current != tab && s.tabs.contains(&tab);
        if switched {
            s.last = Some(s.current);
            if !stepping {
                s.trail.visited(s.current);
            }
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
        // A pane parked behind a tool in its place isn't drawn: the focus
        // goes to the tool, as it was left.
        let pane = if self.parked(pane) {
            match self.in_front_of(pane) {
                Some(tool) => tool,
                None => return,
            }
        } else {
            pane
        };
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

    /// Moves from a pane to the one beside it on `dir`'s side, or past the
    /// left edge of a tab that isn't zoomed into its session's sidebar: what
    /// `M-Left` and `C-w h` do, and what the diff and Notes ask at their
    /// edges (`switchboard select-pane -L`).
    pub fn select_pane_toward(&mut self, pane: PaneId, dir: super::Direction) {
        let Some(tab_id) = self.panes.get(&pane).map(|s| s.tab) else {
            return;
        };
        let (Some(tab), Some(s)) = (self.tabs.get(&tab_id), self.session_of_tab(tab_id)) else {
            return;
        };
        // Past the edge, round to the far side, as tmux's select-pane
        // wrapped: the sidebar is the leftmost, so left from the left edge
        // and right from the right one reach it.
        let sidebar = s.sidebar > 0 && !tab.zoomed;
        let side_ways = matches!(dir, super::Direction::Left | super::Direction::Right);
        let name = s.name.clone();
        if let Some(next) = tab.layout.neighbor(s.main_area(), pane, dir) {
            self.focus_pane(next);
        } else if sidebar && side_ways {
            self.sidebar_enter(&name, None);
        } else if let Some(next) = tab.layout.wrap(s.main_area(), pane, dir) {
            self.focus_pane(next);
        }
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

    #[allow(clippy::too_many_arguments)]
    fn attach(
        &mut self,
        id: ClientId,
        session: Option<String>,
        cols: u16,
        rows: u16,
        term: String,
        cwd: Option<PathBuf>,
        tx: mpsc::UnboundedSender<ServerMsg>,
        hold: mpsc::UnboundedSender<Hold>,
    ) {
        let name = session.unwrap_or_else(|| {
            self.sessions
                .first()
                .map(|s| s.name.clone())
                .unwrap_or_else(|| DEFAULT_SESSION.to_string())
        });
        if let Err(e) = self.ensure_session(&name, cols, rows, cwd) {
            let _ = tx.send(ServerMsg::Exit {
                message: Some(format!("couldn't start a shell: {e}")),
            });
            return;
        }
        let client = Client {
            id,
            tx,
            hold,
            session: name.clone(),
            cols,
            rows,
            term,
            parser: Parser::new(),
            prefix: false,
            screen: Screen::default(),
            dirty: true,
            drag: None,
            last_click: None,
            status_hits: Default::default(),
            header_keys: Default::default(),
            header_hover: None,
            pressed_at: None,
            overlays: Default::default(),
            activity: Instant::now(),
            options_taken: 0,
            terminal_colors: false,
        };
        // Its colours, for the panes' programs that ask (src/mux/terminal.rs),
        // the background's for the `system` theme too (src/status).
        Server::ask_background(&client.tx);
        self.clients.insert(id, client);
        self.terminal_colors_sync();
        self.size_session_to(id);
        if let Some(tab) = self.session(&name).map(|s| s.current) {
            self.agent_focus(tab);
        }
        self.update_attached(id);
        self.extras_attached(id);
    }

    /// A terminal a server handed over as it replaced itself with a new
    /// build (src/mux/upgrade.rs), attached as it was, to its session (the
    /// first, if its session is gone) at its size, its connection going on
    /// from the frame it was at; its screen is drawn again whole.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn adopt_client(
        &mut self,
        id: ClientId,
        stream: std::os::unix::net::UnixStream,
        session: &str,
        cols: u16,
        rows: u16,
        term: String,
        idle: Duration,
        input: Vec<u8>,
        output: Vec<u8>,
    ) -> Result<()> {
        NEXT_CLIENT.fetch_max(id.0.saturating_add(1), Ordering::Relaxed);
        let name = if self.session(session).is_some() {
            session.to_string()
        } else {
            self.sessions
                .first()
                .map(|s| s.name.clone())
                .ok_or_else(|| anyhow!("no session is left to show it"))?
        };
        stream.set_nonblocking(true)?;
        let stream = tokio::net::UnixStream::from_std(stream)?;
        let (otx, orx) = mpsc::unbounded_channel::<ServerMsg>();
        let (htx, hrx) = mpsc::unbounded_channel::<Hold>();
        tokio::spawn(serve_client(
            stream,
            id,
            self.tx.clone(),
            orx,
            hrx,
            input,
            output,
        ));
        // Whatever the build before asked of the terminal for hover is
        // asked off again, as a client new to this server has it.
        let _ = otx.send(ServerMsg::Output(b"\x1b[?1003l\x1b[?1002h".to_vec()));
        // The colors the build before may have set (`terminal_colors`) are
        // put back before the terminal is asked for its own, which the
        // `system` theme reads; they are set again once it is attached.
        if crate::status::theme::terminal_colors_on() {
            let put_back = crate::status::theme::PUT_COLORS_BACK.to_vec();
            let _ = otx.send(ServerMsg::Output(put_back));
        }
        Server::ask_background(&otx);
        let client = Client {
            id,
            tx: otx,
            hold: htx,
            session: name,
            cols,
            rows,
            term,
            parser: Parser::new(),
            prefix: false,
            screen: Screen::default(),
            dirty: true,
            drag: None,
            last_click: None,
            status_hits: Default::default(),
            header_keys: Default::default(),
            header_hover: None,
            pressed_at: None,
            overlays: Default::default(),
            activity: Instant::now()
                .checked_sub(idle)
                .unwrap_or_else(Instant::now),
            options_taken: 0,
            terminal_colors: false,
        };
        self.clients.insert(id, client);
        self.terminal_colors_sync();
        self.size_session_to(id);
        Ok(())
    }

    /// What a terminal sent that its parser holds unfinished (a paste not
    /// yet ended, an ESC), as a frame to be read again first by the build a
    /// handover gives the terminal to.
    pub(super) fn held_keys(&self, id: ClientId) -> Vec<u8> {
        match self.clients.get(&id).map(|c| c.parser.held()) {
            Some(held) if !held.is_empty() => {
                proto::encode_client(&ClientMsg::Input(held.to_vec()))
            }
            _ => Vec::new(),
        }
    }

    /// A terminal's connection given back for a handover that didn't
    /// happen, served again as it was, its screen drawn again whole.
    pub(super) fn resume_client(&mut self, id: ClientId, held: Held) {
        let Some(c) = self.clients.get_mut(&id) else {
            return;
        };
        let stream = match held
            .stream
            .set_nonblocking(true)
            .and_then(|()| tokio::net::UnixStream::from_std(held.stream))
        {
            Ok(stream) => stream,
            Err(e) => {
                eprintln!("switchboard server: taking a terminal's connection up again: {e}");
                self.clients.remove(&id);
                return;
            }
        };
        let (otx, orx) = mpsc::unbounded_channel::<ServerMsg>();
        let (htx, hrx) = mpsc::unbounded_channel::<Hold>();
        tokio::spawn(serve_client(
            stream,
            id,
            self.tx.clone(),
            orx,
            hrx,
            held.input,
            held.output,
        ));
        c.tx = otx;
        c.hold = htx;
        c.screen.invalidate();
        c.dirty = true;
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
        let left = std::mem::replace(&mut c.session, session.to_string());
        c.screen.invalidate();
        c.dirty = true;
        self.size_session_to(id);
        self.client_left(&left);
        self.agent_focus(tab);
    }

    /// A terminal left a session: the session takes the size of the one
    /// used last of those still showing it, as tmux's `window-size latest`
    /// did, rather than keeping the size of the one gone.
    pub fn client_left(&mut self, session: &str) {
        let latest = self
            .clients
            .values()
            .filter(|c| c.session == session)
            .max_by_key(|c| c.activity)
            .map(|c| c.id);
        if let Some(id) = latest {
            self.size_session_to(id);
        }
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
        self.typed = true;
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

    /// The directory a pane's tab is about: its tab's `@tab_dir` while that
    /// directory is there (the review tab's PR worktree, whose Claude works
    /// from the main clone), else the pane's own (`pane_own_dir`). The
    /// tab's name, color and PR, and the keys that open something for this
    /// directory (M-v, M-m, M-d), read this one. Never a directory that no
    /// longer exists (a worktree removed under it), its nearest ancestor
    /// that does taking its place.
    pub fn pane_dir(&self, pane: PaneId) -> Option<PathBuf> {
        crate::util::nearest_existing(&self.pane_dir_as_said(pane)?)
    }

    /// The directory a program started from the pane starts in (a split, a
    /// new tab, a respawn): the pane's own, never the tab's `@tab_dir`, so
    /// nothing starts in a worktree of someone else's PR.
    pub fn pane_own_dir(&self, pane: PaneId) -> Option<PathBuf> {
        crate::util::nearest_existing(&self.pane_own_dir_as_said(pane)?)
    }

    /// The directory `pane_dir` names, which may no longer exist (a
    /// worktree removed under the pane) before it climbs to the nearest
    /// ancestor that does.
    pub fn pane_dir_as_said(&self, pane: PaneId) -> Option<PathBuf> {
        let tab_dir = self
            .panes
            .get(&pane)
            .and_then(|slot| self.tabs.get(&slot.tab))
            .and_then(|tab| tab.opts.get(TAB_DIR))
            .map(PathBuf::from)
            .filter(|dir| dir.is_dir());
        tab_dir.or_else(|| self.pane_own_dir_as_said(pane))
    }

    /// The directory a pane is in as its Claude or its program has it:
    /// where its Claude last said it works, while that Claude is still in
    /// front, else the directory of its program (`Pane::cwd`), whose
    /// process stays in the directory it started in.
    fn pane_own_dir_as_said(&self, pane: PaneId) -> Option<PathBuf> {
        let slot = self.panes.get(&pane)?;
        let said = slot
            .pane
            .foreground_pgid()
            .and_then(|group| self.agents.dir(pane, group))
            .map(std::path::Path::to_path_buf);
        said.or_else(|| slot.pane.cwd())
    }

    /// Handles one input from a client; a left or right click's cell is
    /// kept on the client while it is handled, so a box it opens (a right
    /// click's menu) opens beside it.
    pub(crate) fn dispatch(&mut self, id: ClientId, parsed: input::Parsed) {
        let pressed_at = match parsed.input {
            Input::Mouse(m)
                if m.kind == MouseKind::Press
                    && matches!(m.button, MouseButton::Left | MouseButton::Right) =>
            {
                Some((m.col, m.row))
            }
            _ => None,
        };
        if let Some(c) = self.clients.get_mut(&id) {
            c.pressed_at = pressed_at;
        }
        self.dispatch_input(id, parsed);
        if let Some(c) = self.clients.get_mut(&id) {
            c.pressed_at = None;
        }
    }

    fn dispatch_input(&mut self, id: ClientId, parsed: input::Parsed) {
        // The terminal's answers about its colours, asked as it attached,
        // are the server's (src/mux/terminal.rs, and the background's for
        // the theme in src/status), whatever is up.
        if parsed.input == Input::Unknown {
            let colour = self.terminal_reply(&parsed.raw);
            if self.status_reply(&parsed.raw) || colour {
                self.mission_changed();
                return;
            }
        }
        // Mission control takes them while it is the box on top
        // (src/mission), and says itself what changed; any other overlay
        // up takes them (src/ui), and whatever they do may change what it
        // shows.
        let Some(parsed) = self.mission_input(id, parsed) else {
            return;
        };
        self.mission_changed();
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
            // Not counted for the habits, as tmux.conf's `used` alias was
            // on the root table's keys alone.
            if let Some(action) = self.bindings.prefix.get(&key).cloned() {
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
            // C-w, pressed all the time in shells and editors, isn't one of
            // the habits' keys, as tmux.conf counted it nowhere.
            if action != Action::Extra(crate::extras::Command::PaneNav) {
                self.count_key(&key.notation());
            }
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
        // Off in Settings, M-[ and M-] go to the tab on either side.
        let action = match action {
            Action::TabBack if !crate::settings::is_on("tab_back_forward") => Action::PrevTab,
            Action::TabForward if !crate::settings::is_on("tab_back_forward") => Action::NextTab,
            other => other,
        };
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
                if let Some(pane) = pane {
                    self.select_pane_toward(pane, dir);
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
            Action::TabBack | Action::TabForward => {
                let to = self.session_mut(&session).and_then(|s| {
                    let current = s.current;
                    if action == Action::TabBack {
                        s.trail.back(current)
                    } else {
                        s.trail.forward(current)
                    }
                });
                if let Some(t) = to {
                    self.select_tab_as(&session, t, true);
                }
            }
            Action::SelectTab(n) => {
                if let Some(t) = self.tab_numbered(n) {
                    self.go_to_tab(id, t);
                }
            }
            Action::NewTab => {
                let cwd = pane.and_then(|p| self.pane_own_dir(p));
                if let Err(e) = self.new_tab(&session, cwd, None, None, Opened::Front) {
                    self.say(Some(id), &format!("Couldn't open a tab: {e}"));
                }
            }
            Action::Split(dir) => {
                if let Some(p) = pane
                    && let Err(e) = self.split(p, dir, None, None, None, false)
                {
                    self.say(Some(id), &format!("Couldn't split the pane: {e}"));
                }
            }
            Action::ClosePane => {
                if let Some(p) = pane {
                    self.close_pane(id, p);
                }
            }
            Action::CloseTab => self.close_tab(id, current_tab),
            Action::NextPane => {
                let panes = self
                    .tabs
                    .get(&current_tab)
                    .map(|t| t.layout.panes())
                    .unwrap_or_default();
                let at = panes.iter().position(|p| Some(*p) == pane);
                if let Some(next) = at.map(|at| panes[(at + 1) % panes.len()])
                    && Some(next) != pane
                {
                    self.focus_pane(next);
                }
            }
            Action::LastPane => {
                let last = self.tabs.get(&current_tab).and_then(|t| t.last_pane);
                if let Some(last) =
                    last.filter(|p| self.panes.get(p).is_some_and(|s| s.tab == current_tab))
                {
                    self.focus_pane(last);
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
                    c.exit(format!("[detached (from session {session})]"));
                    self.client_left(&c.session);
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
            Action::NextIdle => self.next_idle(id),
            Action::FocusClaude => self.focus_claude(current_tab),
            Action::CopyReply => self.copy_reply(id, current_tab),
            Action::History => self.open_history(id),
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
            Action::ReviewPr => self.review_pr(id, pane),
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
            Action::TabTitle => self.show_tab_title(id, current_tab),
            Action::View(command) => self.view_command(id, command),
            Action::Diff(command) => self.diff_command(id, command),
            Action::SlackRelay => self.slack_relay(true),
            Action::StopSlackRelay => self.slack_relay(false),
            Action::Update(command) => self.update_command(id, command),
            Action::Upgrade => self.latest_asked(id),
            Action::KeepAwake => self.awake_by_hand(id),
            Action::Notes(command) => self.notes_command(id, command, pane),
            Action::Extra(command) => self.extras_command(id, command, pane),
            Action::Autopilot(command) => self.autopilot_command(id, command, pane),
            Action::Routines(command) => self.routines_command(id, command),
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

    pub(crate) fn leave_copy(&mut self, pane: PaneId) {
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

    /// Where a pane is on a client's screen, when its tab is the one shown
    /// there.
    pub(crate) fn pane_rect(&self, id: ClientId, pane: PaneId) -> Option<Rect> {
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
            .find(|(p, _)| *p == pane)
            .map(|(_, r)| r.intersection(area))
    }

    fn mouse(&mut self, id: ClientId, m: Mouse) {
        if self.sidebar_mouse(id, m) {
            return;
        }
        let Some(c) = self.clients.get(&id) else {
            return;
        };
        let status_row = c.rows.saturating_sub(crate::status::lines());
        // UX 2.0's header: a click on a key or the ⌘ Commands button does
        // what it does, and the one under the mouse is lit (hover_highlight).
        let header_hit = c
            .header_keys
            .iter()
            .find(|(r, _)| m.row == r.y && m.col >= r.x && m.col < r.right())
            .map(|(_, button)| *button);
        if m.kind == MouseKind::Move {
            let lit = header_hit.filter(|_| crate::settings::is_on("hover_highlight"));
            if c.header_hover != lit
                && let Some(c) = self.clients.get_mut(&id)
            {
                c.header_hover = lit;
                c.dirty = true;
            }
        }
        if let Some(button) = header_hit {
            if m.kind == MouseKind::Press && m.button == MouseButton::Left {
                self.act(id, button.action());
            }
            return;
        }
        let Some(c) = self.clients.get(&id) else {
            return;
        };
        // The wheel over the status line goes to the tab before or after,
        // as tmux's WheelUpStatus and WheelDownStatus did.
        if m.row >= status_row
            && m.kind == MouseKind::Press
            && matches!(m.button, MouseButton::WheelUp | MouseButton::WheelDown)
        {
            let next = m.button == MouseButton::WheelDown;
            self.act(
                id,
                if next {
                    Action::NextTab
                } else {
                    Action::PrevTab
                },
            );
            return;
        }
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
            if let Some(tab) = hit.and_then(|n| self.tab_numbered(n)) {
                self.select_tab(&session, tab);
            }
            return;
        }
        let drag = c.drag;
        match (m.kind, drag) {
            (MouseKind::Drag, Some(Drag::Scrollbar(pane, grab))) => {
                let rect = self.pane_rect(id, pane);
                if let (Some(rect), Some(slot)) = (rect, self.panes.get_mut(&pane)) {
                    let (total, _, visible) = slot.pane.scroll_extent();
                    let height = usize::from(rect.height.saturating_sub(1));
                    let row = usize::from(m.row.saturating_sub(rect.y + 1));
                    let top = crate::ui::kit::scroll_drag(total, visible, height, row, grab);
                    slot.pane.scroll_to_top_line(top);
                    // Back into the history after the bottom, as the wheel
                    // brings copy mode up again.
                    if slot.copy.is_none() && slot.pane.scrolled() > 0 {
                        slot.copy = Some(CopyMode {
                            exit_at_bottom: true,
                            search: None,
                        });
                    }
                    leave_wheel_copy_at_bottom(slot);
                    let tab = slot.tab;
                    self.dirty_tab(tab);
                }
                return;
            }
            (MouseKind::Release, Some(Drag::Scrollbar(..))) => {
                if let Some(c) = self.clients.get_mut(&id) {
                    c.drag = None;
                }
                return;
            }
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
        if self.notices_mouse(id, pane, col, row, m) {
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
                    leave_wheel_copy_at_bottom(slot);
                    let tab = slot.tab;
                    self.dirty_tab(tab);
                }
            }
            (MouseKind::Press, MouseButton::Left) if self.on_pane_scrollbar(pane, col, row) => {
                // A press on the bar focuses its pane, as one on its text does.
                if self.current_pane(id) != Some(pane) {
                    self.focus_pane(pane);
                }
                let mut grab = (0, 0);
                if let Some(slot) = self.panes.get_mut(&pane) {
                    let (total, top, visible) = slot.pane.scroll_extent();
                    let height = usize::from(slot.pane.size().1.saturating_sub(1));
                    let row = usize::from(row - 1);
                    grab = crate::ui::kit::scroll_grab(total, top, visible, height, row);
                    if let Some(to) = crate::ui::kit::scroll_press(total, top, visible, height, row)
                    {
                        slot.pane.scroll_to_top_line(to);
                    }
                    leave_wheel_copy_at_bottom(slot);
                    let tab = slot.tab;
                    self.dirty_tab(tab);
                }
                if let Some(c) = self.clients.get_mut(&id) {
                    c.drag = Some(Drag::Scrollbar(pane, grab));
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

    /// Whether a pane's cell (`col`, `row` from its top left) is on the
    /// scroll bar it shows while scrolled back: its last column, under
    /// the `[n]` label.
    fn on_pane_scrollbar(&self, pane: PaneId, col: u16, row: u16) -> bool {
        self.panes.get(&pane).is_some_and(|s| {
            s.pane.scrolled() > 0
                && row >= 1
                && col + 1 == s.pane.size().0
                && crate::ui::kit::scroll_bars()
        })
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

    /// Draws a client, in the canvas's colors when its terminal draws
    /// truecolor (src/status/theme.rs's `drawing_for`).
    fn draw(&mut self, id: ClientId) {
        let truecolor = self.clients.get(&id).is_some_and(Client::truecolor);
        crate::status::theme::drawing_for(truecolor, || self.draw_now(id));
    }

    fn draw_now(&mut self, id: ClientId) {
        self.mission_before_draw(id);
        self.history_before_draw(id);
        self.pictures_before_draw(id);
        let Some(c) = self.clients.get(&id) else {
            return;
        };
        let mut frame = Frame::new(c.cols.max(1), c.rows.max(1));
        if c.graphics() {
            frame.graphics = Some(self.terminal.cell.unwrap_or(super::pane::CELL));
        }
        let session = c.session.clone();
        let (hits, side, header_keys, panes) = match self.session(&c.session) {
            Some(s) => self.compose(&mut frame, s, c),
            None => Default::default(),
        };
        // With sixel chosen, the pictures' cells as the panes drew them,
        // before anything is drawn over them (src/mux/sixel).
        let pictures = self.pictures_drawn(&frame, &panes);
        let c = self.clients.get_mut(&id).expect("checked above");
        frame.cover_links(|frame| c.overlays.draw(frame));
        c.status_hits = hits;
        c.header_keys = header_keys;
        c.dirty = false;
        // Pictures as sixel where the panes' own placeholders still show,
        // once everything over the panes is drawn.
        self.place_pictures(&mut frame, pictures);
        let c = self.clients.get_mut(&id).expect("checked above");
        let bytes = c.screen.render(&frame);
        let _ = c.tx.send(ServerMsg::Output(bytes));
        self.sidebar_drawn(id, &session, side);
        self.sync_motion(id);
    }

    /// Draws a client's session into `frame`; gives where the status line's
    /// parts went, what the sidebar drew, where UX 2.0's header put its
    /// keys, and each pane drawn and where.
    fn compose(
        &self,
        frame: &mut Frame,
        s: &Session,
        c: &Client,
    ) -> (
        crate::status::Hits,
        Option<crate::sidebar::Drawn>,
        super::header::Keys,
        Vec<(PaneId, Rect)>,
    ) {
        let rows = frame.area().height;
        let cols = frame.area().width;
        let palette = crate::status::theme::palette();
        // With the sidebar focused, no pane has the focus.
        let side_focused = self.sidebar.focused(&s.name);
        let mut cursor = None;
        let mut drawn = Vec::new();
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
                let focused = active && c.overlays.lets_through();
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
                    drawn.push((*pid, rect));
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
                // Under the label, where in the scrollback the view is,
                // gone again at the bottom.
                if slot.pane.scrolled() > 0 && crate::ui::kit::scroll_bars() {
                    let (total, top, visible) = slot.pane.scroll_extent();
                    let x = rect.right().saturating_sub(1);
                    let height = rect.height.saturating_sub(1);
                    frame.links.clear_area(Rect::new(x, rect.y + 1, 1, height));
                    crate::ui::kit::draw_scrollbar(
                        &mut frame.buf,
                        (x, rect.y + 1, height),
                        (total, top, visible),
                        &c.overlays.look().theme,
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
        // UX 2.0's header over the tab's panes (src/mux/header.rs); a
        // zoomed pane covers it, as it covers the sidebar.
        let top = crate::status::top();
        let header_at = self
            .tabs
            .get(&s.current)
            .filter(|t| top > 0 && !t.zoomed)
            .map(|_| {
                let main = s.main_area();
                Rect::new(main.x, 0, main.width, top).intersection(frame.area())
            });
        let mut header_keys = super::header::Keys::new();
        if let Some(area) = header_at {
            let mut header = self.tab_header(s.current);
            (header.history_open, header.viewing) = self.history_shown(c.id, s.current);
            header_keys = super::header::draw(
                &mut frame.buf,
                area,
                &header,
                c.header_hover,
                palette,
                crate::ui::kit::caps(&c.term),
                c.overlays
                    .layers
                    .iter()
                    .any(|l| l.overlay.kind() == "palette"),
            );
        }
        let side = self.sidebar_draw(&mut frame.buf, s, c.id);
        let hits = self.draw_status_rows(frame, s);
        // The prefix's mark at the status line's right end, or with none
        // (UX 2.0), at the header's.
        let status = match header_at {
            Some(area) if crate::status::lines() == 0 => Rect::new(area.x, 0, area.width, 1),
            _ => Rect::new(0, rows.saturating_sub(crate::status::lines()), cols, 1),
        };
        if c.prefix && status.y < rows {
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
        (hits, side, header_keys, drawn)
    }
}

impl Server {
    /// What UX 2.0's header says about a tab (src/mux/header.rs), from what
    /// the server holds: its label, color and directory, its Claude's
    /// state, and what its focused pane shows.
    pub fn tab_header(&self, tab: TabId) -> super::header::Header {
        let shown = self
            .tabs
            .get(&tab)
            .and_then(|t| self.panes.get(&t.active))
            .map(|slot| super::header::Shown::of_pane(|k| slot.opts.get(k).cloned()))
            .unwrap_or(super::header::Shown::Claude);
        let state = self.tab_agent(tab);
        // How long the pane holding that state has held it.
        let since = self
            .tab_panes(tab)
            .into_iter()
            .filter_map(|p| self.agents.get(p))
            .find(|a| a.state.is_some() && a.state == state)
            .map(|a| a.since);
        let dir = self.tab_path(tab);
        super::header::Header {
            name: self.tab_label(tab),
            path: dir.map(crate::paths::tilde).unwrap_or_default(),
            branch: dir
                .and_then(|d| self.tab_names.names.branch(d))
                .unwrap_or_default()
                .to_string(),
            state,
            age: crate::sidebar::text::ago(since, crate::util::now_secs()),
            color: self.tab_color(tab).map(Into::into),
            shown,
            next: self.sidebar_needs_next(tab),
            commands: crate::settings::is_on("palette_button"),
            history: crate::settings::is_on("tab_history"),
            history_open: false,
            viewing: None,
            context: self.tab_context(tab).map(|c| c.used),
        }
    }

    /// What the statusline of the tab's Claude last said of its context.
    pub fn tab_context(&self, tab: TabId) -> Option<&crate::agent::Context> {
        self.tab_panes(tab)
            .into_iter()
            .filter_map(|p| self.agents.get(p))
            .find_map(|a| a.context.as_ref())
    }
}

/// The tabs' numbers, given the ones they have in order (0 for none): a
/// tab keeps its own, of two with the same one the first keeps it, and the
/// rest take the smallest free, in order, so the tabs of a build before
/// numbers go by their places as they did.
fn fill_numbers(have: &[usize]) -> Vec<usize> {
    let mut used = std::collections::HashSet::new();
    let kept: Vec<bool> = have.iter().map(|n| *n > 0 && used.insert(*n)).collect();
    let mut next = 1;
    have.iter()
        .zip(kept)
        .map(|(n, kept)| {
            if kept {
                return *n;
            }
            while used.contains(&next) {
                next += 1;
            }
            used.insert(next);
            next
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::fill_numbers;

    #[test]
    fn tabs_keep_their_numbers_and_the_rest_take_the_smallest_free() {
        assert_eq!(fill_numbers(&[0, 0, 0]), [1, 2, 3], "by place, as before");
        assert_eq!(fill_numbers(&[1, 0, 4]), [1, 2, 4]);
        assert_eq!(fill_numbers(&[0, 1]), [2, 1], "a kept one is never moved");
        assert_eq!(
            fill_numbers(&[3, 3, 0]),
            [3, 1, 2],
            "the first keeps a twice-held one"
        );
    }
}
