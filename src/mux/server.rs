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

use super::compose::{self, Frame, Screen, TabStatus};
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
}

impl Session {
    /// The area panes are laid out in: all but the status line.
    pub fn main_area(&self) -> Rect {
        Rect::new(0, 0, self.cols.max(1), self.rows.saturating_sub(1).max(1))
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

/// Scrolling back through a pane's history with the keyboard.
#[derive(Clone, Copy, Debug)]
pub struct CopyMode {
    /// Entered with the wheel: scrolled back to the bottom, it ends.
    pub exit_at_bottom: bool,
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
}

/// What is shown over the tab, taking the keys while it is.
#[derive(Clone, Debug)]
pub enum Overlay {
    /// The list of keys, scrolled this far.
    Keys { scroll: usize },
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
    tab_spans: compose::TabSpans,
    pub overlay: Option<Overlay>,
    pub activity: Instant,
}

pub struct Server {
    pub(super) tx: mpsc::UnboundedSender<Event>,
    pub(super) pane_tx: mpsc::UnboundedSender<PaneEvent>,
    pub sessions: Vec<Session>,
    pub tabs: HashMap<TabId, Tab>,
    pub panes: HashMap<PaneId, PaneSlot>,
    pub clients: BTreeMap<ClientId, Client>,
    pub bindings: Bindings,
    next_tab: u32,
    next_pane: u32,
    pub quit: bool,
    /// When a key was last pressed in any client, for telling whether you're
    /// here where the desktop can't say.
    pub last_input: Instant,
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
    for p in problems {
        eprintln!("keys.json: {p}");
    }
    loop {
        tokio::select! {
            Some(event) = rx.recv() => server.handle(event),
            Some(event) = pane_rx.recv() => server.pane_event(event),
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
            next_tab: 1,
            next_pane: 1,
            quit: false,
            last_input: Instant::now(),
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
            }
            Event::Tick => self.refresh_names(),
            Event::Quit => self.quit = true,
        }
    }

    pub fn pane_event(&mut self, event: PaneEvent) {
        match event {
            PaneEvent::Output(id, bytes) => {
                let Some(slot) = self.panes.get_mut(&id) else {
                    return;
                };
                let notices = slot.pane.process(&bytes);
                let tab = slot.tab;
                for notice in notices {
                    match notice {
                        TermNotice::Clipboard(text) => self.to_clipboard(None, &text),
                        TermNotice::Title(_) | TermNotice::Bell => {}
                    }
                }
                self.dirty_tab(tab);
            }
            PaneEvent::Exited(id, code) => {
                if let Some(slot) = self.panes.get_mut(&id) {
                    slot.pane.set_exited(code);
                }
                self.remove_pane(id);
            }
        }
    }

    pub fn shutdown(&mut self) {
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
    fn spawn_pane(
        &mut self,
        tab: TabId,
        cwd: Option<PathBuf>,
        command: Option<Vec<String>>,
        cols: u16,
        rows: u16,
    ) -> Result<PaneId> {
        let id = self.alloc_pane();
        let env = vec![
            (
                "SWITCHBOARD".to_string(),
                socket::socket_path().to_string_lossy().into_owned(),
            ),
            ("SWITCHBOARD_PANE".to_string(), id.to_string()),
        ];
        let spec = PaneSpawn {
            id: Some(id),
            command,
            cwd,
            env,
            cols: cols.max(1),
            rows: rows.max(1),
            scrollback: HISTORY,
        };
        let pane = Pane::spawn(spec, self.pane_tx.clone())?;
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

    /// Makes a session with one tab, unless it is there.
    pub fn ensure_session(&mut self, name: &str, cols: u16, rows: u16) -> Result<()> {
        if self.session(name).is_some() {
            return Ok(());
        }
        let tab = TabId(self.next_tab);
        self.next_tab += 1;
        let area = Rect::new(0, 0, cols.max(1), rows.saturating_sub(1).max(1));
        let pane = self.spawn_pane(tab, Some(paths::home()), None, area.width, area.height)?;
        self.tabs.insert(tab, new_tab(tab, pane));
        self.sessions.push(Session {
            name: name.to_string(),
            tabs: vec![tab],
            current: tab,
            last: None,
            cols,
            rows,
        });
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
        } else {
            self.remove_tab(tab_id);
        }
    }

    /// Ends every pane of a tab and takes it out.
    pub fn kill_tab(&mut self, tab: TabId) {
        let panes = self
            .tabs
            .get(&tab)
            .map(|t| t.layout.panes())
            .unwrap_or_default();
        for p in panes {
            if let Some(mut slot) = self.panes.remove(&p) {
                slot.pane.kill();
            }
        }
        self.remove_tab(tab);
    }

    fn remove_tab(&mut self, tab: TabId) {
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
        if s.current != tab && s.tabs.contains(&tab) {
            s.last = Some(s.current);
            s.current = tab;
        }
        self.refresh_name(tab);
        self.relayout(tab);
        self.dirty_session(session);
    }

    /// Focuses a pane, its tab made current in its session.
    pub fn focus_pane(&mut self, pane: PaneId) {
        let Some(tab_id) = self.panes.get(&pane).map(|s| s.tab) else {
            return;
        };
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
        let Some(area) = self.session_of_tab(tab_id).map(Session::main_area) else {
            return;
        };
        let Some(tab) = self.tabs.get(&tab_id) else {
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

    fn refresh_name(&mut self, tab_id: TabId) {
        let Some(tab) = self.tabs.get(&tab_id) else {
            return;
        };
        let dir = self.panes.get(&tab.active).and_then(|s| s.pane.cwd());
        let name = dir
            .map(|d| {
                if d == paths::home() {
                    "~".to_string()
                } else {
                    d.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "/".into())
                }
            })
            .unwrap_or_default();
        let tab = self.tabs.get_mut(&tab_id).expect("checked above");
        if tab.auto_name != name {
            tab.auto_name = name;
            self.dirty_tab(tab_id);
        }
    }

    fn refresh_names(&mut self) {
        let current: Vec<TabId> = self.sessions.iter().map(|s| s.current).collect();
        for t in current {
            self.refresh_name(t);
        }
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
            tab_spans: Vec::new(),
            overlay: None,
            activity: Instant::now(),
        };
        self.clients.insert(id, client);
        self.size_session_to(id);
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
        let Some(s) = self.session_mut(&name) else {
            return;
        };
        if (s.cols, s.rows) == (cols, rows) {
            return;
        }
        s.cols = cols;
        s.rows = rows;
        let tabs = s.tabs.clone();
        for t in tabs {
            self.relayout(t);
        }
        self.dirty_session(&name);
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
        for p in parsed {
            self.dispatch(id, p);
        }
        if self.clients.get(&id).is_some_and(|c| c.parser.pending()) {
            let tx = self.tx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(input::ESC_TIMEOUT).await;
                let _ = tx.send(Event::FlushInput(id));
            });
        }
    }

    /// The current pane of a client's session.
    pub fn current_pane(&self, id: ClientId) -> Option<PaneId> {
        let c = self.clients.get(&id)?;
        let s = self.session(&c.session)?;
        Some(self.tabs.get(&s.current)?.active)
    }

    fn dispatch(&mut self, id: ClientId, parsed: input::Parsed) {
        if let Some(overlay) = self.clients.get(&id).and_then(|c| c.overlay.clone()) {
            if let Input::Key(key) = parsed.input {
                self.overlay_key(id, overlay, key);
            }
            return;
        }
        match parsed.input {
            Input::Key(key) => self.key(id, key, &parsed.raw),
            Input::Mouse(m) => self.mouse(id, m),
            Input::Paste(text) => {
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
                if let Some(slot) = self.current_pane(id).and_then(|p| self.panes.get_mut(&p)) {
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
        if let Some(action) = self.bindings.root.get(&key).cloned() {
            self.act(id, action);
            return;
        }
        let Some(pane) = self.current_pane(id) else {
            return;
        };
        if self.panes.get(&pane).is_some_and(|s| s.copy.is_some()) {
            self.copy_key(pane, key);
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
        match action {
            Action::SelectPane(dir) => {
                let (Some(pane), Some(tab), Some(s)) =
                    (pane, self.tabs.get(&current_tab), self.session(&session))
                else {
                    return;
                };
                if let Some(next) = tab.layout.neighbor(s.main_area(), pane, dir) {
                    self.focus_pane(next);
                }
            }
            Action::PrevTab | Action::NextTab => {
                let at = tabs.iter().position(|t| *t == current_tab).unwrap_or(0);
                let n = tabs.len();
                let next = if action == Action::NextTab {
                    (at + 1) % n
                } else {
                    (at + n - 1) % n
                };
                self.select_tab(&session, tabs[next]);
            }
            Action::LastTab => {
                if let Some(last) = self.session(&session).and_then(|s| s.last) {
                    self.select_tab(&session, last);
                }
            }
            Action::SelectTab(n) => {
                if let Some(t) = tabs.get(n.wrapping_sub(1)) {
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
                    self.kill_pane(p);
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
                    slot.copy = Some(CopyMode {
                        exit_at_bottom: false,
                    });
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
            Action::ListKeys => {
                if let Some(c) = self.clients.get_mut(&id) {
                    c.overlay = Some(Overlay::Keys { scroll: 0 });
                    c.dirty = true;
                }
            }
        }
    }

    /// Keys while scrolling back: vi's, as tmux's copy mode with
    /// `mode-keys vi`.
    fn copy_key(&mut self, pane: PaneId, key: Key) {
        let Some(slot) = self.panes.get_mut(&pane) else {
            return;
        };
        let (_, rows) = slot.pane.size();
        let half = (rows / 2).max(1) as i32;
        let page = rows.max(1) as i32;
        let plain = key.mods == Mods::default();
        let ctrl = key.mods
            == Mods {
                ctrl: true,
                ..Mods::default()
            };
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc if plain => {
                slot.pane.scroll_to_bottom();
                slot.pane.select_clear();
                slot.copy = None;
            }
            KeyCode::Char('k') | KeyCode::Up if plain => slot.pane.scroll(1),
            KeyCode::Char('j') | KeyCode::Down if plain => slot.pane.scroll(-1),
            KeyCode::Char('u') if ctrl => slot.pane.scroll(half),
            KeyCode::Char('d') if ctrl => slot.pane.scroll(-half),
            KeyCode::Char('b') if ctrl => slot.pane.scroll(page),
            KeyCode::Char('f') if ctrl => slot.pane.scroll(-page),
            KeyCode::PageUp => slot.pane.scroll(page),
            KeyCode::PageDown => slot.pane.scroll(-page),
            KeyCode::Char('g') if plain => slot.pane.scroll(i32::MAX / 2),
            KeyCode::Char('G') => slot.pane.scroll_to_bottom(),
            _ => {}
        }
        if slot.copy.is_some_and(|c| c.exit_at_bottom) && slot.pane.scrolled() == 0 {
            slot.copy = None;
        }
        let tab = slot.tab;
        self.dirty_tab(tab);
    }

    fn leave_copy(&mut self, pane: PaneId) {
        if let Some(slot) = self.panes.get_mut(&pane)
            && (slot.copy.take().is_some() || slot.pane.scrolled() > 0)
        {
            slot.pane.scroll_to_bottom();
            slot.pane.select_clear();
        }
    }

    fn overlay_key(&mut self, id: ClientId, overlay: Overlay, key: Key) {
        let Some(c) = self.clients.get_mut(&id) else {
            return;
        };
        match overlay {
            Overlay::Keys { scroll } => {
                let next = match key.code {
                    KeyCode::Char('j') | KeyCode::Down => Some(scroll + 1),
                    KeyCode::Char('k') | KeyCode::Up => Some(scroll.saturating_sub(1)),
                    KeyCode::PageDown | KeyCode::Char(' ') => Some(scroll + 10),
                    KeyCode::PageUp => Some(scroll.saturating_sub(10)),
                    _ => None,
                };
                c.overlay = next.map(|scroll| Overlay::Keys { scroll });
                c.dirty = true;
            }
        }
    }

    // ── The mouse ────────────────────────────────────────────────────────────

    /// The pane under a cell of a client's screen, and the cell in that
    /// pane's own coordinates.
    fn pane_at(&self, id: ClientId, col: u16, row: u16) -> Option<(PaneId, u16, u16)> {
        let c = self.clients.get(&id)?;
        let s = self.session(&c.session)?;
        let tab = self.tabs.get(&s.current)?;
        let area = s.main_area();
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
        let Some(c) = self.clients.get(&id) else {
            return;
        };
        let status_row = c.rows.saturating_sub(1);
        if m.row == status_row && m.kind == MouseKind::Press && m.button == MouseButton::Left {
            let hit = c
                .tab_spans
                .iter()
                .find(|(start, end, _)| m.col >= *start && m.col < *end)
                .map(|(_, _, i)| *i);
            let session = c.session.clone();
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
            return;
        };
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
                        });
                    }
                    slot.pane
                        .scroll(if up { WHEEL_LINES } else { -WHEEL_LINES });
                    if slot.copy.is_some_and(|c| c.exit_at_bottom) && slot.pane.scrolled() == 0 {
                        slot.copy = None;
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

    fn forward_mouse(&mut self, pane: PaneId, m: Mouse) {
        if let Some(slot) = self.panes.get_mut(&pane)
            && let Some(bytes) = input::encode_mouse(&m, &slot.pane.modes())
        {
            let _ = slot.pane.write(&bytes);
        }
    }

    // ── Drawing ──────────────────────────────────────────────────────────────

    pub fn draw_dirty(&mut self) {
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
        let Some(c) = self.clients.get(&id) else {
            return;
        };
        let mut frame = Frame::new(c.cols.max(1), c.rows.max(1));
        let spans = match self.session(&c.session) {
            Some(s) => self.compose(&mut frame, s, c),
            None => Vec::new(),
        };
        let c = self.clients.get_mut(&id).expect("checked above");
        c.tab_spans = spans;
        c.dirty = false;
        let bytes = c.screen.render(&frame);
        let _ = c.tx.send(ServerMsg::Output(bytes));
    }

    fn compose(&self, frame: &mut Frame, s: &Session, c: &Client) -> compose::TabSpans {
        let rows = frame.area().height;
        let cols = frame.area().width;
        let main = s.main_area().intersection(frame.area());
        let mut cursor = None;
        if let Some(tab) = self.tabs.get(&s.current) {
            let placement = if tab.zoomed {
                super::layout::Placement {
                    panes: vec![(tab.active, s.main_area())],
                    borders: Vec::new(),
                }
            } else {
                tab.layout.place(s.main_area())
            };
            let mut focused_rect = None;
            for (pid, rect) in &placement.panes {
                let rect = rect.intersection(main);
                let Some(slot) = self.panes.get(pid) else {
                    continue;
                };
                let focused = *pid == tab.active && c.overlay.is_none();
                let at = slot.pane.render(
                    &mut frame.buf,
                    rect,
                    &PaneView {
                        focused,
                        dim: false,
                    },
                );
                if *pid == tab.active {
                    focused_rect = Some(rect);
                    if slot.copy.is_none() {
                        cursor = at;
                    }
                } else {
                    compose::tint_inactive(&mut frame.buf, rect);
                }
                if slot.pane.scrolled() > 0 || slot.copy.is_some() {
                    let label = format!("[{}]", slot.pane.scrolled());
                    let x = rect.right().saturating_sub(label.len() as u16);
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
                compose::draw_borders(&mut frame.buf, &placement, focused_rect);
            }
        }
        let tabs: Vec<TabStatus> = s
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(i, t)| {
                self.tabs.get(t).map(|tab| TabStatus {
                    index: i + 1,
                    name: tab.display_name().to_string(),
                    current: *t == s.current,
                })
            })
            .collect();
        let status = Rect::new(0, rows.saturating_sub(1), cols, 1);
        let spans = compose::draw_status(&mut frame.buf, status, &s.name, &tabs);
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
        if let Some(Overlay::Keys { scroll }) = &c.overlay {
            self.draw_keys(frame, *scroll);
            cursor = None;
        }
        frame.cursor = cursor;
        spans
    }

    fn draw_keys(&self, frame: &mut Frame, scroll: usize) {
        let area = frame.area();
        let width = area.width.saturating_sub(8).clamp(20, 90);
        let height = area.height.saturating_sub(4).max(5);
        let outer = Rect::new(
            (area.width.saturating_sub(width)) / 2,
            (area.height.saturating_sub(height)) / 2,
            width,
            height,
        );
        let inner = compose::draw_box(&mut frame.buf, outer, "Keys");
        let list = self.bindings.list();
        let key_width = list.iter().map(|(k, _)| k.len()).max().unwrap_or(0) as u16 + 2;
        for (row, (keys, about)) in list
            .iter()
            .skip(scroll)
            .take(inner.height as usize)
            .enumerate()
        {
            let y = inner.y + row as u16;
            let line = Rect::new(inner.x, y, inner.width, 1);
            let x = compose::put(
                &mut frame.buf,
                inner.x + 1,
                line,
                keys,
                Style::new()
                    .fg(compose::colors::ACCENT)
                    .add_modifier(Modifier::BOLD),
            );
            let _ = x;
            compose::put(
                &mut frame.buf,
                inner.x + 1 + key_width,
                line,
                about,
                Style::new(),
            );
        }
    }
}

fn new_tab(id: TabId, pane: PaneId) -> Tab {
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
