//! Restarting the server on a new build without ending anything in it:
//! `switchboard upgrade`. A new build only takes effect once the server
//! starts again, which used to end every pane's program (a Claude session
//! mid-turn, a build, a shell). Here the server replaces itself with the
//! binary at the path it was started from (`execv`, so its pid stays the
//! same), handing the new build every pane's terminal: the master's
//! descriptor is kept open across the exec, so the panes' programs, still
//! the process's children, never see their terminal close, get no SIGHUP,
//! and go on untouched. The new build takes each one up (`Pane::adopt`)
//! rather than starting anything, draws what it showed again
//! (src/mux/replay.rs), and tells its program its size again, so a
//! full-screen program draws itself again.
//!
//! What goes across is a file in the state directory (`handover.json`,
//! readable only by you, removed once read) in the saved layout's shape
//! (src/mux/persist.rs), with what a handover needs beside it: each pane's
//! descriptor, program, size, screen and the input still to be written to
//! it; the listening socket and the server's lock, by descriptor, so no
//! connection is refused and no other server starts in between; what each
//! pane's Claude was doing; the pictures each pane's program gave the
//! terminal, which a program gives once and the new build may have to
//! draw as sixel (src/mux/sixel); and the connection of the `switchboard
//! upgrade` that asked, which the new build answers.
//!
//! Attached terminals stay attached without noticing: each connection is
//! handed over by descriptor too, with its session and size. A connection
//! first stops reading (what the terminal sends meanwhile waits in the
//! socket for the new build), then, once the panes are paused and the last
//! frame drawn, writes what is queued for it and gives back the frame it
//! had half read and whatever it couldn't write in time, which the new
//! build reads and writes first; so the stream of frames never breaks,
//! and a client from before this feature, which knows nothing of it, goes
//! on as it was. The new build draws each screen again whole.
//!
//! Safety first, since a build that can't take the panes up loses them all:
//! before anything stops, the new binary is asked `server
//! --handover-version`, and only one that answers the version this server
//! writes is exec'd (a build from before this feature doesn't know the
//! flag). The panes are then paused (src/mux/pty.rs) and the commands under
//! way answered, so nothing read is lost and nothing half-written; should
//! anything fail before the exec, or the exec itself, every descriptor is
//! closed on exec again, the panes resume, the socket listens again, and
//! the server goes on as it was, saying why. The new build keeps whatever
//! it can take up, says in its log and to whoever asked what it couldn't,
//! and never ends a program it took up.

use std::ffi::CString;
use std::io::{self, Write as _};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

use super::pane::{HandedPane, Pane, PaneAdopt};
use super::persist::Saved;
use super::proto::{self, ServerMsg};
use super::server::{Answer, ClientId, Event, Held, Hold, Loop, PaneSlot, Reply, Server};
use super::{PaneId, pty};
use crate::agent::PaneAgent;
use crate::{paths, settings};

/// The handover file's shape, which `server --handover-version` prints: a
/// build is exec'd only when it prints the one this build writes.
pub const VERSION: u32 = 1;
/// The file, in the state directory.
pub const FILE: &str = "handover.json";
/// How long the new binary has to say its version: the first run of a
/// file just written can take seconds (a security agent looking at it, or
/// a busy disk paging it in), so this only stops a build that hangs.
const CHECK_WAIT: Duration = Duration::from_secs(30);
/// How long the panes have to pause and the commands under way to be
/// answered before the handover goes ahead without them.
const SETTLE: Duration = Duration::from_secs(1);
/// How long the connection that asked has to be handed back.
const ASKER_WAIT: Duration = Duration::from_secs(1);

/// The binary the server was started from, as its path was then: on Linux
/// the running binary's own name points at a deleted file once a new one
/// is put in its place, so it is read as the server starts.
static STARTED_FROM: OnceLock<PathBuf> = OnceLock::new();

/// Notes which binary the server was started from, for an upgrade later.
pub fn remember_binary() {
    if let Ok(path) = crate::util::exe() {
        super::latest::remember(&path);
        let _ = STARTED_FROM.set(path);
    }
}

/// The path the server was started from, which an upgrade execs as it is
/// on disk now.
pub fn binary() -> Option<PathBuf> {
    STARTED_FROM.get().cloned()
}

/// What one server hands the build it replaces itself with.
#[derive(Debug, Serialize, Deserialize)]
pub struct Handover {
    pub version: u32,
    /// The build handing over, as `switchboard --version` names it.
    #[serde(default)]
    pub from: String,
    /// What M-p's build adds to the message after the restart (a fetch
    /// that failed), else empty.
    #[serde(default)]
    pub note: String,
    /// The listening socket's descriptor.
    pub listener: RawFd,
    /// The server lock's descriptor.
    #[serde(default)]
    pub lock: Option<RawFd>,
    /// Every session and tab, and every pane's options, as persist saves
    /// them (`persist::layout_now`).
    pub layout: Saved,
    #[serde(default)]
    pub panes: Vec<HandedPane>,
    /// The connection of the `switchboard upgrade` that asked, answered by
    /// the new build.
    #[serde(default)]
    pub asker: Option<RawFd>,
    /// The terminal whose key asked, told once it is done.
    #[serde(default)]
    pub client: Option<u32>,
    /// A cell's size in pixels, as the terminal attached last said.
    #[serde(default)]
    pub cell: Option<(u16, u16)>,
    /// What each pane's Claude was doing.
    #[serde(default)]
    pub agents: Vec<(PaneId, PaneAgent)>,
    /// The terminals attached.
    #[serde(default)]
    pub clients: Vec<HandedClient>,
    /// The next pane's and tab's numbers, so one closed before isn't given
    /// again.
    #[serde(default)]
    pub next_pane: u32,
    #[serde(default)]
    pub next_tab: u32,
    /// The pictures each pane's program gave the terminal (src/mux/sixel),
    /// given once, which the new build draws as sixel.
    #[serde(default)]
    pub images: Vec<(PaneId, Vec<crate::mux::sixel::kitty::Handed>)>,
    /// A cell's size in pixels as the terminal answered when asked.
    #[serde(default)]
    pub asked_cell: Option<(u16, u16)>,
    /// The links in what you sent each pane's Claude (src/views/given.rs).
    #[serde(default)]
    pub given: Vec<(PaneId, Vec<String>)>,
    /// The computer held awake by hand, until you say (src/awake).
    #[serde(default)]
    pub awake_by_hand: bool,
}

/// An attached terminal handed over: its connection by descriptor, where it
/// was in the stream of frames, and what it was showing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandedClient {
    pub id: u32,
    pub fd: RawFd,
    pub session: String,
    pub cols: u16,
    pub rows: u16,
    /// What its terminal says it is, for whether it draws images; none
    /// from a build before it was kept (text, then, until it attaches
    /// again).
    #[serde(default)]
    pub term: String,
    /// How long since it was last used, so the one used last still sizes
    /// its session.
    #[serde(default)]
    pub idle_ms: u64,
    /// Bytes read of a frame not yet whole.
    #[serde(default)]
    pub input: Vec<u8>,
    /// Bytes of frames not yet written.
    #[serde(default)]
    pub output: Vec<u8>,
}

/// Writes the handover, readable only by you, in place of any left before.
pub fn write_file(file: &Path, handover: &Handover) -> io::Result<()> {
    let json = serde_json::to_vec(handover).map_err(io::Error::other)?;
    match std::fs::remove_file(file) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(file)?;
    out.write_all(&json)?;
    out.sync_all()
}

/// Reads a handover and removes its file, refusing one of another version.
pub fn read_file(file: &Path) -> Result<Handover> {
    let text = std::fs::read(file).with_context(|| format!("reading {}", file.display()));
    let _ = std::fs::remove_file(file);
    let handover: Handover =
        serde_json::from_slice(&text?).with_context(|| format!("reading {}", file.display()))?;
    if handover.version != VERSION {
        bail!(
            "it is version {}, and this build reads {VERSION}",
            handover.version
        );
    }
    Ok(handover)
}

/// Whether `binary` takes a handover this build writes: it must answer
/// `server --handover-version` with `VERSION` within `CHECK_WAIT`. Gives
/// why not, in words.
pub fn check(binary: &Path) -> Result<(), String> {
    let mut child = Command::new(binary)
        .args(["server", "--handover-version"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{} couldn't be run ({e})", paths::tilde(binary)))?;
    let deadline = Instant::now() + CHECK_WAIT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{} didn't say whether it can take over",
                    paths::tilde(binary)
                ));
            }
        }
    };
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = io::Read::read_to_string(&mut stdout, &mut out);
    }
    match out.trim().parse::<u32>() {
        Ok(v) if status.success() && v == VERSION => Ok(()),
        Ok(v) if status.success() => Err(format!(
            "{} takes over in format {v}, and this server hands over in {VERSION}",
            paths::tilde(binary)
        )),
        _ => Err(format!(
            "{} can't take over from a running server (a build from before upgrades)",
            paths::tilde(binary)
        )),
    }
}

/// Replaces this process with `binary`, as `switchboard server --handover
/// <file>`; returns only if that fails, with why.
fn exec(binary: &Path, file: &Path) -> io::Error {
    use std::os::unix::ffi::OsStrExt;
    let cstr =
        |bytes: &[u8]| CString::new(bytes).map_err(|_| io::Error::other("a path with a NUL"));
    let path = match cstr(binary.as_os_str().as_bytes()) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let file = match cstr(file.as_os_str().as_bytes()) {
        Ok(f) => f,
        Err(e) => return e,
    };
    // Named `switchboard`, whatever the file is called, so it runs the
    // server rather than a command a link's name would choose.
    let args = [
        c"switchboard".to_owned(),
        c"server".to_owned(),
        c"--handover".to_owned(),
        file,
    ];
    let mut argv: Vec<*const libc::c_char> = args.iter().map(|a| a.as_ptr()).collect();
    argv.push(std::ptr::null());
    // SAFETY: signals blocked in this thread would stay blocked in the new
    // program, so none are; execv reads the NUL-terminated strings and the
    // NULL-terminated list, which outlive the call.
    unsafe {
        let mut none: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut none);
        libc::pthread_sigmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
        libc::execv(path.as_ptr(), argv.as_ptr());
    }
    io::Error::last_os_error()
}

// ── The old server's side ───────────────────────────────────────────────────

/// What the server keeps of an upgrade under way.
#[derive(Default)]
pub struct Upgrade {
    pending: Option<Pending>,
    /// The handover has begun: another upgrade asked meanwhile is refused.
    handing: bool,
    /// M-p's restart is building first (src/mux/latest.rs).
    building: bool,
}

struct Pending {
    /// The `switchboard upgrade` waiting for its answer.
    asker: Option<oneshot::Sender<Answer>>,
    /// The terminal whose key asked.
    client: Option<ClientId>,
    /// The new binary said it can take over.
    checked: bool,
    /// The binary the restart execs.
    on: PathBuf,
    /// What M-p's build adds to the message after the restart.
    note: String,
}

/// What the check off the server's task found.
#[derive(Debug)]
pub enum UpgradeEvent {
    Checked(Result<(), String>),
    /// M-p's main clone brought up to date, its build begun.
    Building(ClientId, PathBuf),
    /// M-p's build for the terminal that asked: the binary to restart on,
    /// `None` when there is nothing new, or why the build failed.
    Built(ClientId, Result<super::latest::Brought, String>),
}

/// Whether a command's arguments ask for an upgrade (and not its usage).
pub fn asks_upgrade(args: &[String]) -> bool {
    args.first().is_some_and(|a| a == "upgrade")
        && !matches!(args.get(1).map(String::as_str), Some("--help" | "-h"))
}

impl Server {
    /// An upgrade asked for, by `switchboard upgrade` (answered on `asker`)
    /// or a key on `client`: the new binary (`on`, else the path the
    /// server started from) is checked off the server's task, and the
    /// handover runs from the server's loop once it passes.
    pub fn upgrade_asked(
        &mut self,
        asker: Option<oneshot::Sender<Answer>>,
        client: Option<ClientId>,
        on: Option<PathBuf>,
    ) {
        let on = on.or_else(binary);
        let pending = Pending {
            asker,
            client,
            checked: false,
            on: on.clone().unwrap_or_default(),
            note: String::new(),
        };
        if self.upgrade.pending.is_some() || self.upgrade.handing || self.upgrade.building {
            self.upgrade_refused(pending, "it is restarting on the new build already");
            return;
        }
        let Some(binary) = on else {
            self.upgrade_refused(pending, "the binary it was started from isn't known");
            return;
        };
        self.upgrade.pending = Some(pending);
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let checked = check(&binary);
            let _ = tx.send(Event::Upgrade(UpgradeEvent::Checked(checked)));
        });
    }

    /// M-p's "Restart Switchboard on the latest build on disk": builds the
    /// checkout first, off the server's task (src/mux/latest.rs), then
    /// restarts as `switchboard upgrade` does, only when the binary on disk
    /// is a new one.
    pub fn latest_asked(&mut self, client: ClientId) {
        if self.upgrade.pending.is_some() || self.upgrade.handing || self.upgrade.building {
            self.say(
                Some(client),
                "Switchboard is building or restarting already.",
            );
            return;
        }
        let Some(binary) = binary() else {
            self.say(
                Some(client),
                "Switchboard can't restart on the new build: the binary it was started from isn't known.",
            );
            return;
        };
        let build_first = settings::is_on(super::latest::SETTING);
        if build_first && let Some(checkout) = super::latest::checkout() {
            let text = if checkout.main_clone {
                format!("Updating {}…", paths::tilde(&checkout.dir))
            } else {
                format!("Building Switchboard in {}…", paths::tilde(&checkout.dir))
            };
            self.say(Some(client), &text);
        }
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        self.upgrade.building = true;
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let building = |dir: &std::path::Path| {
                let _ = tx.send(Event::Upgrade(UpgradeEvent::Building(
                    client,
                    dir.to_path_buf(),
                )));
            };
            let result =
                super::latest::bring_up_to_date(&paths::home(), &binary, build_first, &building);
            let _ = tx.send(Event::Upgrade(UpgradeEvent::Built(client, result)));
        });
    }

    pub fn upgrade_event(&mut self, event: UpgradeEvent) {
        let checked = match event {
            UpgradeEvent::Checked(checked) => checked,
            UpgradeEvent::Building(client, dir) => {
                self.say(
                    Some(client),
                    &format!("Building Switchboard in {}…", paths::tilde(&dir)),
                );
                return;
            }
            UpgradeEvent::Built(client, result) => {
                self.upgrade.building = false;
                match result {
                    Ok(super::latest::Brought { on: Some(on), note }) => {
                        self.upgrade_asked(None, Some(client), Some(on));
                        if let Some(pending) = self.upgrade.pending.as_mut() {
                            pending.note = note;
                        }
                    }
                    Ok(super::latest::Brought { on: None, note }) => self.say(
                        Some(client),
                        format!("Already on the latest build. {note}").trim_end(),
                    ),
                    Err(why) => self.say(
                        Some(client),
                        &format!("The build failed: {why}. Switchboard goes on as it was."),
                    ),
                }
                return;
            }
        };
        let Some(pending) = self.upgrade.pending.as_mut() else {
            return;
        };
        match checked {
            Ok(()) => pending.checked = true,
            Err(why) => {
                if let Some(pending) = self.upgrade.pending.take() {
                    self.upgrade_refused(pending, &why);
                }
            }
        }
    }

    /// Whether the handover is due: asked for, and the new binary checked.
    pub(super) fn upgrade_ready(&self) -> bool {
        self.upgrade.pending.as_ref().is_some_and(|p| p.checked)
    }

    fn upgrade_refused(&mut self, pending: Pending, why: &str) {
        let text =
            format!("Switchboard can't restart on the new build: {why}. It goes on as it was.");
        eprintln!("switchboard server: {text}");
        match pending.asker {
            Some(asker) => {
                let _ = asker.send(Answer::Reply(Reply::fail(text)));
            }
            None => self.say(pending.client, &text),
        }
    }
}

/// Runs whatever has come in and draws, as the server's loop does.
fn drain(server: &mut Server, lp: &mut Loop<'_>) {
    while let Ok(event) = lp.pane_rx.try_recv() {
        server.pane_event(event);
    }
    while let Ok(event) = lp.rx.try_recv() {
        server.handle(event);
    }
    server.draw_dirty();
}

/// The server's loop, for up to `SETTLE`, until every pane is paused and
/// the commands under way answered: a pane started meanwhile is paused
/// too, and a terminal attached meanwhile stops reading (`paused` holds
/// those told).
async fn settle(
    server: &mut Server,
    lp: &mut Loop<'_>,
    paused: &mut std::collections::HashSet<ClientId>,
) -> bool {
    let deadline = Instant::now() + SETTLE;
    loop {
        drain(server, lp);
        for slot in server.panes.values() {
            slot.pane.pause();
        }
        for (id, c) in &server.clients {
            if paused.insert(*id) {
                let _ = c.hold.send(Hold::Pause);
            }
        }
        let parked = server.panes.values().all(|s| s.pane.parked());
        let answered = lp.busy.load(Ordering::SeqCst) == 0;
        if (parked && answered) || Instant::now() >= deadline {
            drain(server, lp);
            return parked;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// The connection that asked, handed back by its task to be answered by
/// the new build.
async fn take_asker(asker: oneshot::Sender<Answer>) -> Option<OwnedFd> {
    let (tx, rx) = oneshot::channel();
    asker.send(Answer::Keep(tx)).ok()?;
    let stream = tokio::time::timeout(ASKER_WAIT, rx).await.ok()?.ok()?;
    stream.into_std().ok().map(OwnedFd::from)
}

/// Answers a `switchboard upgrade` on its connection.
async fn answer(stream: OwnedFd, reply: Reply) {
    let std = std::os::unix::net::UnixStream::from(stream);
    if std.set_nonblocking(true).is_err() {
        return;
    }
    let Ok(mut stream) = tokio::net::UnixStream::from_std(std) else {
        return;
    };
    let msg = ServerMsg::Reply {
        code: reply.code,
        stdout: reply.stdout.into_bytes(),
        stderr: reply.stderr.into_bytes(),
    };
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        let _ = stream.write_all(&proto::encode_server(&msg)).await;
        let _ = stream.shutdown().await;
    })
    .await;
}

/// The handover, from the server's loop once an upgrade is due: everything
/// paused and answered, the handover file written, every descriptor it
/// names left open across the exec, and the new build exec'd in this
/// process's place. Returns only when that couldn't be done, with
/// everything going on as before.
pub(super) async fn hand_over(server: &mut Server, lp: &mut Loop<'_>) {
    let Some(pending) = server.upgrade.pending.take() else {
        return;
    };
    let binary = pending.on.clone();
    let pending_note = pending.note.clone();
    server.upgrade.handing = true;
    let client = pending.client;
    let asker = match pending.asker {
        Some(asker) => take_asker(asker).await,
        None => None,
    };
    let Some(listener) = lp.acceptor.stop().await else {
        server.upgrade.handing = false;
        lp.acceptor.restart(None);
        fail(
            server,
            asker,
            client,
            "its socket couldn't be handed over".into(),
        )
        .await;
        return;
    };
    // The terminals' keys wait in their sockets from here on.
    let mut paused = std::collections::HashSet::new();
    let parked = settle(server, lp, &mut paused).await;
    let why_not = if server.quit {
        Some("the server is ending".to_string())
    } else if server.sessions.is_empty() {
        Some("no tabs are left to keep".to_string())
    } else if !parked {
        Some("a pane's terminal didn't stop in time".to_string())
    } else {
        None
    };
    if let Some(why) = why_not {
        resume(server, lp, listener, Vec::new());
        fail(server, asker, client, why).await;
        return;
    }
    super::persist::finish_writing(server);
    let held = take_clients(server).await;

    let file = paths::state_dir().join(FILE);
    let mut panes = Vec::new();
    for slot in server.panes.values_mut() {
        panes.push(slot.pane.hand_over());
    }
    let handover = Handover {
        version: VERSION,
        from: crate::update::version_line(),
        note: pending_note,
        listener: listener.as_raw_fd(),
        lock: super::socket::lock_fd(),
        layout: super::persist::layout_now(server),
        panes,
        asker: asker.as_ref().map(AsRawFd::as_raw_fd),
        client: client.map(|c| c.0),
        cell: server.terminal.cell,
        agents: server.agents.records(),
        next_pane: server.next_pane,
        next_tab: server.next_tab,
        images: server.sixel.hand_over(),
        asked_cell: server.terminal.asked_cell,
        given: server.views.given.records(),
        awake_by_hand: server.awake.by_hand,
        clients: held
            .iter()
            .filter_map(|(id, held)| {
                let c = server.clients.get(id)?;
                Some(HandedClient {
                    id: id.0,
                    fd: held.stream.as_raw_fd(),
                    session: c.session.clone(),
                    cols: c.cols,
                    rows: c.rows,
                    term: c.term.clone(),
                    idle_ms: c.activity.elapsed().as_millis() as u64,
                    // What its parser held came before the frame half read.
                    input: [server.held_keys(*id), held.input.clone()].concat(),
                    output: held.output.clone(),
                })
            })
            .collect(),
    };
    let mut fds: Vec<RawFd> = vec![handover.listener];
    fds.extend(handover.lock);
    fds.extend(handover.asker);
    fds.extend(handover.panes.iter().map(|p| p.fd));
    fds.extend(handover.clients.iter().map(|c| c.fd));
    let why = match write_file(&file, &handover) {
        Err(e) => format!(
            "the handover couldn't be written to {} ({e})",
            paths::tilde(&file)
        ),
        Ok(()) => {
            eprintln!(
                "switchboard server: restarting on {} in place, handing over {} panes and {} terminals",
                paths::tilde(&binary),
                handover.panes.len(),
                handover.clients.len()
            );
            // `caffeinate -w` would wait on this pid for the new build too.
            server.awake_hand_over();
            let opened = fds.iter().all(|fd| pty::set_cloexec(*fd, false).is_ok());
            let failed = if opened {
                exec(&binary, &file)
            } else {
                io::Error::other("its descriptors couldn't be kept open")
            };
            for fd in &fds {
                let _ = pty::set_cloexec(*fd, true);
            }
            let _ = std::fs::remove_file(&file);
            format!("{} couldn't be started ({failed})", paths::tilde(&binary))
        }
    };
    resume(server, lp, listener, held);
    fail(server, asker, client, why).await;
}

/// Every attached terminal's connection, given back by its task once what
/// is queued for it is written (for a moment at most). One leaving anyway
/// is let go; one that doesn't answer is closed with the old process.
async fn take_clients(server: &mut Server) -> Vec<(ClientId, Held)> {
    let mut asked = Vec::new();
    for (id, c) in &server.clients {
        let (tx, rx) = oneshot::channel();
        if c.hold.send(Hold::Give(tx)).is_ok() {
            asked.push((*id, rx));
        }
    }
    let mut held = Vec::new();
    for (id, rx) in asked {
        match tokio::time::timeout(ASKER_WAIT, rx).await {
            Ok(Ok(Some(h))) => held.push((id, h)),
            _ => {
                server.clients.remove(&id);
            }
        }
    }
    held
}

/// After a handover that didn't happen: the panes read and write again, the
/// terminals' connections are served again (`held`, given back, or those
/// only paused), and the socket takes connections again.
fn resume(
    server: &mut Server,
    lp: &mut Loop<'_>,
    listener: std::os::unix::net::UnixListener,
    held: Vec<(ClientId, Held)>,
) {
    server.upgrade.handing = false;
    for slot in server.panes.values() {
        slot.pane.resume();
    }
    if held.is_empty() {
        for c in server.clients.values() {
            let _ = c.hold.send(Hold::Resume);
        }
    }
    for (id, held) in held {
        server.resume_client(id, held);
    }
    lp.acceptor.restart(Some(listener));
}

/// Says why a handover didn't happen, to whoever asked.
async fn fail(server: &mut Server, asker: Option<OwnedFd>, client: Option<ClientId>, why: String) {
    let text = format!("Switchboard can't restart on the new build: {why}. It goes on as it was.");
    eprintln!("switchboard server: {text}");
    match asker {
        Some(stream) => answer(stream, Reply::fail(text)).await,
        None => server.say(client, &text),
    }
}

// ── The new build's side ────────────────────────────────────────────────────

/// Takes up what a server handed over, after the server is set up and
/// before its loop starts: each pane (`Pane::adopt`), then the sessions and
/// tabs around them, each pane's Claude's state, and the size of a cell;
/// then each program is told its size again, and whoever asked is told how
/// it went. A pane that can't be taken up is said in the log and to whoever
/// asked, and its terminal is left open, so its program isn't hung up.
pub fn adopt(server: &mut Server, handover: Handover) {
    let mut lost: Vec<String> = Vec::new();
    let opts: std::collections::HashMap<PaneId, _> = handover
        .layout
        .sessions
        .iter()
        .flat_map(|s| s.tabs.iter())
        .flat_map(|t| t.panes.iter())
        .map(|p| (p.id, p.opts.clone()))
        .collect();
    let extended_keys = settings::is_on("extended_keys");
    server.terminal.cell = handover.cell;
    for handed in handover.panes {
        let id = handed.id;
        let taken = adopt_pane(server, handed, extended_keys);
        match taken {
            Ok(mut pane) => {
                if let Some(cell) = server.terminal.cell {
                    pane.set_cell(cell);
                }
                pane.set_colors(server.terminal.colors);
                server.panes.insert(
                    id,
                    PaneSlot {
                        pane,
                        tab: super::TabId(0),
                        opts: opts.get(&id).cloned().unwrap_or_default(),
                        copy: None,
                    },
                );
            }
            Err(e) => lost.push(format!("pane {id} ({e:#})")),
        }
    }
    server.next_pane = server.next_pane.max(handover.next_pane);
    server.next_tab = server.next_tab.max(handover.next_tab);
    server.terminal.asked_cell = handover.asked_cell;
    let taken: std::collections::HashSet<PaneId> = server.panes.keys().copied().collect();
    server.sixel.take_up(handover.images, &taken);
    super::persist::adopt_layout(server, handover.layout);
    for (id, agent) in handover.agents {
        if server.panes.contains_key(&id) {
            server.agents.put(id, agent);
        }
    }
    for (id, links) in handover.given {
        if server.panes.contains_key(&id) {
            server.views.given.put(id, links);
        }
    }
    server.awake.by_hand = handover.awake_by_hand;
    // The one used longest ago first, so the one used last sizes its
    // session, as it did.
    let mut clients = handover.clients;
    clients.sort_by_key(|c| std::cmp::Reverse(c.idle_ms));
    for c in clients {
        if !pty::is_open(c.fd) {
            lost.push(format!("terminal {}", c.id));
            continue;
        }
        let _ = pty::set_cloexec(c.fd, true);
        // SAFETY: handed over open, and owned by nothing else here.
        let stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(c.fd) };
        if let Err(e) = server.adopt_client(
            ClientId(c.id),
            stream,
            &c.session,
            c.cols,
            c.rows,
            c.term,
            Duration::from_millis(c.idle_ms),
            c.input,
            c.output,
        ) {
            lost.push(format!("terminal {} ({e:#})", c.id));
        }
    }
    for slot in server.panes.values() {
        slot.pane.remind_size();
    }
    let tabs = server.tabs.len();
    let mut text = format!(
        "Switchboard restarted on {} in place of {}, keeping {} {} in {} {}.",
        crate::update::version_line(),
        if handover.from.is_empty() {
            "the build before".to_string()
        } else {
            handover.from
        },
        server.panes.len(),
        if server.panes.len() == 1 {
            "pane"
        } else {
            "panes"
        },
        tabs,
        if tabs == 1 { "tab" } else { "tabs" },
    );
    if !lost.is_empty() {
        text.push_str(&format!(" It couldn't take up {}.", lost.join(", ")));
    }
    if !handover.note.is_empty() {
        text.push_str(&format!(" {}", handover.note));
    }
    eprintln!("switchboard server: {text}");
    let client = handover.client.map(ClientId);
    match handover.asker.filter(|fd| pty::is_open(*fd)) {
        Some(fd) => {
            let _ = pty::set_cloexec(fd, true);
            // SAFETY: handed over open, and owned by nothing else here.
            let stream = unsafe { OwnedFd::from_raw_fd(fd) };
            let reply = if lost.is_empty() {
                Reply::ok(format!("{text}\n"))
            } else {
                Reply {
                    code: 1,
                    stdout: String::new(),
                    stderr: format!("{text}\n"),
                }
            };
            tokio::spawn(answer(stream, reply));
            // What was lost is said on the screen too, whoever asked.
            if !lost.is_empty() {
                server.say(client, &text);
            }
        }
        None => server.say(client, &text),
    }
}

/// One pane taken up. The program goes on whatever happens: the terminal is
/// taken up through a copy of its descriptor, and the one handed over is
/// closed only once the copy is in use, so a pane that can't be taken up
/// leaves its terminal open.
fn adopt_pane(server: &Server, handed: HandedPane, extended_keys: bool) -> Result<Pane> {
    if !pty::is_open(handed.fd) {
        bail!("its terminal's descriptor {} isn't open", handed.fd);
    }
    let _ = pty::set_cloexec(handed.fd, true);
    let copy = pty::duplicate(handed.fd).context("copying its terminal's descriptor")?;
    let pane = Pane::adopt(
        PaneAdopt {
            id: handed.id,
            master: copy,
            pid: handed.pid,
            cols: handed.cols,
            rows: handed.rows,
            scrollback: super::server::HISTORY,
            extended_keys,
            input: handed.input,
            hung_up: handed.hung_up,
            screen: handed.screen,
            tail: handed.tail,
        },
        server.pane_tx.clone(),
    )?;
    // SAFETY: the descriptor handed over, owned by nothing else, now that
    // the pane holds its copy.
    drop(unsafe { OwnedFd::from_raw_fd(handed.fd) });
    Ok(pane)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::persist::{SavedSession, SavedTab};
    use crate::mux::{TabId, layout::Layout};
    use std::os::unix::fs::PermissionsExt;

    fn sample() -> Handover {
        Handover {
            version: VERSION,
            from: "0.1.0 (abc)".into(),
            note: String::new(),
            listener: 7,
            lock: Some(3),
            layout: Saved {
                version: 1,
                sessions: vec![SavedSession {
                    name: "main".into(),
                    cols: 100,
                    rows: 30,
                    current: TabId(1),
                    last: None,
                    sidebar_repo: None,
                    sidebar_repo_open: false,
                    tabs: vec![SavedTab {
                        id: TabId(1),
                        name: String::new(),
                        layout: Layout::Pane(PaneId(4)),
                        active: PaneId(4),
                        last_pane: None,
                        zoomed: false,
                        opts: Default::default(),
                        panes: vec![crate::mux::persist::SavedPane::new(PaneId(4))],
                    }],
                }],
            },
            panes: vec![HandedPane {
                id: PaneId(4),
                fd: 9,
                pid: Some(1234),
                cols: 80,
                rows: 24,
                input: b"typed\r".to_vec(),
                hung_up: false,
                screen: "\x1b[1mhello\x1b[0m\r\n$ ".into(),
                tail: b"\x1b[3".to_vec(),
            }],
            asker: Some(11),
            client: Some(2),
            cell: Some((9, 18)),
            agents: vec![(PaneId(4), PaneAgent::default())],
            clients: vec![HandedClient {
                id: 2,
                fd: 12,
                session: "main".into(),
                cols: 100,
                rows: 30,
                term: "xterm-ghostty".into(),
                idle_ms: 1500,
                input: vec![1, 0, 0],
                output: b"\x1b[H".to_vec(),
            }],
            next_pane: 9,
            next_tab: 4,
            images: vec![(
                PaneId(4),
                vec![crate::mux::sixel::kitty::Handed {
                    id: 77,
                    path: "/tmp/notes/.attachments/a.png".into(),
                    placed: true,
                    cols: Some(30),
                    rows: Some(6),
                }],
            )],
            asked_cell: Some((10, 20)),
            given: vec![(PaneId(4), vec!["https://a.example/1".into()])],
            awake_by_hand: true,
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sb-upgrade-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_handover_reads_back_as_written_once_and_privately() {
        let dir = temp_dir("file");
        let file = dir.join(FILE);
        std::fs::write(&file, "left from before").unwrap();
        write_file(&file, &sample()).unwrap();
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "what panes show is yours alone");
        let read = read_file(&file).unwrap();
        assert!(!file.exists(), "removed once read");
        assert_eq!(read.panes, sample().panes);
        assert_eq!(read.clients, sample().clients);
        assert_eq!(read.layout, sample().layout);
        assert_eq!(
            (read.listener, read.lock, read.asker),
            (7, Some(3), Some(11))
        );
        assert_eq!(read.cell, Some((9, 18)));
        assert_eq!(read.images, sample().images);
        assert_eq!(read.asked_cell, Some((10, 20)));
        assert!(read.awake_by_hand);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_handover_from_before_pictures_were_carried_reads_with_none() {
        let mut json = serde_json::to_value(sample()).unwrap();
        let fields = json.as_object_mut().unwrap();
        fields.remove("images");
        fields.remove("asked_cell");
        fields.remove("awake_by_hand");
        let read: Handover = serde_json::from_value(json).unwrap();
        assert!(read.images.is_empty());
        assert_eq!(read.asked_cell, None);
        assert!(!read.awake_by_hand, "nothing held by hand");
    }

    #[test]
    fn another_version_or_garbage_is_refused() {
        let dir = temp_dir("refused");
        let file = dir.join(FILE);
        let mut other = sample();
        other.version = VERSION + 1;
        write_file(&file, &other).unwrap();
        assert!(read_file(&file).is_err());
        std::fs::write(&file, "{").unwrap();
        assert!(read_file(&file).is_err());
        assert!(!file.exists(), "removed even so");
        assert!(read_file(&file).is_err(), "and none is no handover");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_a_binary_that_answers_this_version_passes() {
        use crate::stand_in::Release;
        let dir = temp_dir("check");
        // Stand-in builds, each a switchboard in a folder of its own, as
        // strict as one about its name and commands.
        let build =
            |name: &str, release: Release| release.install(&dir.join(name).join("switchboard"));
        let right = build("right", Release::new("0.3.0").handover(VERSION));
        assert_eq!(check(&right), Ok(()));
        let other = build("other", Release::new("0.4.0").handover(VERSION + 1));
        assert!(check(&other).unwrap_err().contains("format"));
        // One from before upgrades, whose server knew no such option.
        let old = build("old", Release::new("0.1.0"));
        assert!(check(&old).unwrap_err().contains("before upgrades"));
        assert!(
            check(&dir.join("missing"))
                .unwrap_err()
                .contains("couldn't be run")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_the_upgrade_itself_is_taken_from_the_commands() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(asks_upgrade(&args(&["upgrade"])));
        assert!(!asks_upgrade(&args(&["upgrade", "--help"])));
        assert!(!asks_upgrade(&args(&["list"])));
        assert!(!asks_upgrade(&[]));
    }
}
