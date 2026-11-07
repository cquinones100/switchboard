//! Saving the workspace's layout and bringing it back when the server next
//! starts: what tmux-resurrect and tmux-continuum did, with
//! `tmux/window-options.sh` (the tab options resurrect left out) folded in,
//! since here a tab's options are saved with the rest of it.
//!
//! What is saved, to `<state dir>/layout.json`: every session with its tabs in
//! order, its current and last tab and its size; each tab's name, split tree,
//! active pane, zoom and options; and each pane's id, directory, options, the
//! program in front when that isn't its shell, the Claude session it runs (a
//! process in front with a session file, `~/.claude/sessions/<pid>.json`), and,
//! with the `save_contents` setting on, its screen and the last
//! `CONTENT_LINES` of its history as text.
//!
//! It is written when it differs from what was written last, checked on the
//! server's tick. A change to the layout is written once it has held for a
//! tick, so the panes of a shutting-down machine going one by one (each shell
//! hung up in turn) are never caught half gone, and no more often than every
//! `MIN_GAP`. What panes show changes all the time, so it is read again only
//! every `CONTENTS_EVERY`, as continuum saved once a minute, and when the
//! server ends or `switchboard save` asks. A server ending with no tabs left
//! writes nothing, so the last workspace comes back, as continuum restored its
//! last save. The file before each write is kept as `layout.json.1`, which a
//! restore reads when a crash left no `layout.json`. Writing runs on a thread
//! of its own, since a file of a few megabytes of history mustn't hold up a
//! keystroke.
//!
//! It is read once, when the server starts with no sessions and `auto_restore`
//! is on: each session, tab and pane comes back with the same ids, so a pane
//! named `%12` in a note or a hook is the same pane after a restart. Every pane
//! starts the login shell in its saved directory ($HOME when that is gone),
//! its saved text fed to its emulator first so its history shows above the new
//! prompt, as resurrect's pane contents did. A Claude pane then has `claude
//! --resume <session>` typed into that shell, as resurrect's
//! `claude->claude-here` restored Claude through the shell, so the shell and
//! its PATH are there once Claude ends; a pane that ran one of `RESTARTABLE`
//! has that program typed the same way, and any other comes back as its shell,
//! since running a command again (a deploy, a migration) can do harm. A file
//! that can't be read or doesn't parse is said on stderr (the server's log)
//! and set aside as `layout.json.bad`, and the server starts fresh.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc as channel;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::layout::Layout;
use super::pane::Pane;
use super::server::{Server, Session, new_tab};
use super::{PaneId, TabId, procinfo};
use crate::{paths, settings, util};

/// The file, in the state directory.
pub const FILE: &str = "layout.json";
/// The one before it.
const PREVIOUS: &str = "layout.json.1";
/// Where a file that couldn't be read is put.
const SET_ASIDE: &str = "layout.json.bad";
/// The file's shape; one of another version is set aside, not guessed at.
const VERSION: u32 = 1;
/// Lines of history kept per pane, above its screen.
pub const CONTENT_LINES: usize = 2000;
/// The least time between two writes.
const MIN_GAP: Duration = Duration::from_secs(10);
/// How often what panes show is read again.
const CONTENTS_EVERY: Duration = Duration::from_secs(60);
/// How long after a restore the commands are typed: the shells have started
/// and drawn their prompts by then, so what is typed shows after the prompt
/// rather than echoed above it by the terminal.
const TYPE_AFTER: Duration = Duration::from_millis(500);
/// How long a restored pane keeps its saved Claude session or program in the
/// saves while it is still at its shell: `claude --resume` takes a moment to
/// start, and a save meanwhile mustn't forget the session.
const EXPECT_FOR: Duration = Duration::from_secs(120);
/// Programs started again in a restored pane: ones that only show something,
/// as resurrect's default list was, so running them twice does no harm.
const RESTARTABLE: &[&str] = &["nvim", "vim", "vi", "less", "man", "htop", "top"];
/// Programs that are a shell, so a pane at its prompt saves no program.
const SHELLS: &[&str] = &[
    "sh", "bash", "zsh", "fish", "dash", "ksh", "mksh", "tcsh", "csh", "nu", "xonsh", "elvish",
];

// ── The file ────────────────────────────────────────────────────────────────

/// The whole file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub version: u32,
    pub sessions: Vec<SavedSession>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedSession {
    pub name: String,
    #[serde(default)]
    pub cols: u16,
    #[serde(default)]
    pub rows: u16,
    pub current: TabId,
    #[serde(default)]
    pub last: Option<TabId>,
    pub tabs: Vec<SavedTab>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedTab {
    pub id: TabId,
    /// The name given to it; empty when it is named after its directory.
    #[serde(default)]
    pub name: String,
    pub layout: Layout,
    pub active: PaneId,
    #[serde(default)]
    pub last_pane: Option<PaneId>,
    #[serde(default)]
    pub zoomed: bool,
    #[serde(default)]
    pub opts: BTreeMap<String, String>,
    #[serde(default)]
    pub panes: Vec<SavedPane>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedPane {
    pub id: PaneId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub opts: BTreeMap<String, String>,
    /// The program in front and its arguments, when it isn't the shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// The Claude session it runs (its `sessionId`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<String>,
    /// Its screen and history as text, lines joined by newlines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl SavedPane {
    /// A pane with nothing saved but its id.
    pub fn new(id: PaneId) -> SavedPane {
        SavedPane {
            id,
            cwd: None,
            opts: BTreeMap::new(),
            command: None,
            claude: None,
            text: None,
        }
    }
}

/// Reads a file's text as a save, refusing one of another version.
pub fn parse(text: &str) -> Result<Saved> {
    let saved: Saved = serde_json::from_str(text)?;
    if saved.version != VERSION {
        bail!(
            "it is version {}, and this Switchboard reads {VERSION}",
            saved.version
        );
    }
    Ok(saved)
}

/// The save in a state directory: `layout.json`, else `layout.json.1` when a
/// crash came between setting the old one aside and writing the new. One that
/// can't be read is said on stderr and set aside, and nothing is restored.
pub fn load_from(dir: &Path) -> Option<Saved> {
    for name in [FILE, PREVIOUS] {
        let file = dir.join(name);
        let problem = match fs::read_to_string(&file) {
            Ok(text) => match parse(&text) {
                Ok(saved) => return Some(saved),
                Err(e) => e.to_string(),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => e.to_string(),
        };
        let aside = dir.join(SET_ASIDE);
        eprintln!(
            "switchboard server: couldn't restore the layout from {} ({problem}); it is set aside as {}, and the server starts fresh",
            paths::tilde(&file),
            paths::tilde(&aside)
        );
        let _ = fs::rename(&file, &aside);
        return None;
    }
    None
}

/// Writes a save, the one before kept beside it. The state directory is
/// readable by its owner alone, and so is the file, since what panes showed
/// can hold secrets.
fn write_file(dir: &Path, json: &str) -> io::Result<()> {
    let file = dir.join(FILE);
    match fs::rename(&file, dir.join(PREVIOUS)) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    util::write_atomic(&file, json)?;
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600))
}

// ── What a pane runs ────────────────────────────────────────────────────────

/// The running Claude sessions by pid, from their session files. A headless
/// Claude (`entrypoint` `sdk-cli`, a review loop's helper) is left out, since
/// resuming it as a conversation of its own would be wrong, and so is a
/// session id that couldn't be typed into a shell as it is.
fn claude_sessions() -> HashMap<u32, String> {
    let Ok(entries) = fs::read_dir(paths::claude_dir().join("sessions")) else {
        return HashMap::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let pid: u32 = name.to_str()?.strip_suffix(".json")?.parse().ok()?;
            let json = util::read_json(e.path())?;
            let headless = json
                .get("entrypoint")
                .and_then(Value::as_str)
                .is_some_and(|e| e.starts_with("sdk"));
            let id = json.get("sessionId").and_then(Value::as_str)?;
            (!headless && safe_id(id)).then(|| (pid, id.to_string()))
        })
        .collect()
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A program's name from its first argument: `-zsh` (a login shell) and
/// `/usr/bin/nvim` read as `zsh` and `nvim`.
fn program_name(argv0: &str) -> &str {
    let base = argv0.rsplit('/').next().unwrap_or(argv0);
    base.strip_prefix('-').unwrap_or(base)
}

fn is_shell(name: &str) -> bool {
    SHELLS.contains(&name)
        || std::env::var("SHELL")
            .ok()
            .is_some_and(|s| !s.is_empty() && program_name(&s) == name)
}

/// The program in front and its arguments, unless it is the pane's shell:
/// the one a shell started, else the pane's own when that isn't a shell
/// (a tab opened with a command).
fn program_of(pane: &Pane) -> Option<Vec<String>> {
    let who = match (pane.foreground_pgid(), pane.pid()) {
        (Some(front), _) => front,
        (None, Some(own)) => own,
        (None, None) => return None,
    };
    let args = procinfo::args_of(who)?;
    let name = program_name(args.first()?);
    (!name.is_empty() && !is_shell(name)).then_some(args)
}

/// The Claude session a pane runs: the one of a process in front.
fn claude_of(pane: &Pane, sessions: &HashMap<u32, String>) -> Option<String> {
    if sessions.is_empty() {
        return None;
    }
    // Claude started from the shell leads its group, so it is usually found
    // without reading the group's members.
    if let Some(id) = pane.foreground_pgid().and_then(|g| sessions.get(&g)) {
        return Some(id.clone());
    }
    pane.foreground_pids()
        .iter()
        .find_map(|pid| sessions.get(pid).cloned())
}

/// A word as a shell reads it back: as it is when it holds nothing a shell
/// treats specially, else in single quotes.
fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./:=@%+,".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// What to type into a restored pane's shell, if anything.
fn command_line(pane: &SavedPane) -> Option<String> {
    if let Some(id) = pane.claude.as_deref().filter(|id| safe_id(id)) {
        return Some(format!("claude --resume {id}"));
    }
    let argv = pane.command.as_ref()?;
    let name = program_name(argv.first()?);
    RESTARTABLE.contains(&name).then(|| {
        argv.iter()
            .map(|w| shell_word(w))
            .collect::<Vec<_>>()
            .join(" ")
    })
}

// ── Saving ──────────────────────────────────────────────────────────────────

/// What a restored pane ran, kept in the saves while the pane is still at
/// its shell.
#[derive(Debug)]
struct Expect {
    claude: Option<String>,
    command: Option<Vec<String>>,
}

#[derive(Debug)]
struct Restored {
    at: Instant,
    /// The lines still to be typed.
    typing: Vec<(PaneId, String)>,
    expect: HashMap<PaneId, Expect>,
}

struct Job {
    dir: PathBuf,
    json: String,
    done: Option<channel::SyncSender<io::Result<()>>>,
}

struct Writer {
    tx: channel::Sender<Job>,
    thread: JoinHandle<()>,
}

/// The server's part for saving and restoring: what was written, and what
/// panes showed when last read.
#[derive(Default)]
pub struct Persist {
    /// Hashes of the last file written, whole and without the panes' text.
    written: Option<u64>,
    written_shape: Option<u64>,
    written_at: Option<Instant>,
    /// When the last write was asked for, by the clock, for the health light
    /// (src/status/health.rs) to compare with the file's own time.
    asked_at: Option<std::time::SystemTime>,
    /// The layout as the last tick found it, and its panes.
    seen: Option<u64>,
    seen_panes: HashSet<PaneId>,
    /// What each pane showed when last read.
    contents: HashMap<PaneId, String>,
    contents_at: Option<Instant>,
    restored: Option<Restored>,
    writer: Option<Writer>,
}

fn hash_of(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// On the server's tick: types what a restore left to type, and saves when
/// the layout has changed and held, or what panes show is due to be read.
pub fn tick(server: &mut Server) {
    let mut p = std::mem::take(&mut server.persist);
    p.type_restored(server);
    p.tick(server);
    server.persist = p;
}

/// Before the server ends: a last save, unless panes have gone since the
/// last tick (the machine shutting down hangs every shell up, and the server
/// may hear of some before it is told to end), when the save from before
/// they went stays.
pub fn shutdown(server: &mut Server) {
    let mut p = std::mem::take(&mut server.persist);
    if !server.sessions.is_empty() {
        let now: HashSet<PaneId> = server.panes.keys().copied().collect();
        if p.seen_panes.is_subset(&now)
            && let Err(e) = p.save(server, true)
        {
            eprintln!("switchboard server: saving the layout: {e}");
        }
    }
    if let Some(w) = p.writer.take() {
        drop(w.tx);
        let _ = w.thread.join();
    }
    server.persist = p;
}

/// When the layout's last write was asked for; `None` before any.
pub fn last_asked(server: &Server) -> Option<std::time::SystemTime> {
    server.persist.asked_at
}

/// `switchboard save`: saves now, giving the file.
pub fn save_now(server: &mut Server) -> Result<PathBuf> {
    if server.sessions.is_empty() {
        bail!("there are no tabs to save");
    }
    let mut p = std::mem::take(&mut server.persist);
    let saved = p.save(server, true);
    server.persist = p;
    saved?;
    Ok(paths::state_dir().join(FILE))
}

impl Persist {
    fn tick(&mut self, server: &Server) {
        if server.sessions.is_empty() {
            self.seen = None;
            return;
        }
        let shape_saved = self.snapshot(server);
        let shape = hash_of(&util::json_compact(&shape_saved));
        let held = self.seen == Some(shape);
        self.seen = Some(shape);
        self.seen_panes = server.panes.keys().copied().collect();
        if !held || self.written_at.is_some_and(|t| t.elapsed() < MIN_GAP) {
            return;
        }
        let with_text = settings::is_on("save_contents");
        let due = with_text
            && self
                .contents_at
                .is_none_or(|t| t.elapsed() >= CONTENTS_EVERY);
        if self.written_shape == Some(shape) && !due {
            return;
        }
        if due {
            self.capture(server);
        }
        let json = self.full(shape_saved, with_text);
        let whole = hash_of(&json);
        if self.written == Some(whole) {
            return;
        }
        self.written = Some(whole);
        self.written_shape = Some(shape);
        self.written_at = Some(Instant::now());
        self.send(json, None);
    }

    /// Saves now, what panes show read again, waiting for the file when `wait`.
    fn save(&mut self, server: &Server, wait: bool) -> io::Result<()> {
        let with_text = settings::is_on("save_contents");
        if with_text {
            self.capture(server);
        }
        let shape_saved = self.snapshot(server);
        let shape = hash_of(&util::json_compact(&shape_saved));
        let json = self.full(shape_saved, with_text);
        self.written = Some(hash_of(&json));
        self.written_shape = Some(shape);
        self.written_at = Some(Instant::now());
        if !wait {
            self.send(json, None);
            return Ok(());
        }
        let (tx, rx) = channel::sync_channel(1);
        self.send(json, Some(tx));
        let result = rx
            .recv()
            .unwrap_or_else(|_| Err(io::Error::other("the writer ended")));
        if result.is_err() {
            // So the next tick tries again.
            self.written = None;
            self.written_shape = None;
        }
        result
    }

    /// The file's text: the layout with each pane's text, when kept.
    fn full(&self, mut saved: Saved, with_text: bool) -> String {
        if with_text {
            for pane in saved
                .sessions
                .iter_mut()
                .flat_map(|s| s.tabs.iter_mut())
                .flat_map(|t| t.panes.iter_mut())
            {
                pane.text = self.contents.get(&pane.id).cloned();
            }
        }
        format!("{}\n", util::json_pretty(&saved))
    }

    /// Reads what each pane shows. A pane showing a full-screen program
    /// keeps what it showed before it, since its frame would only be
    /// clutter above the program started again, and what lies beneath is
    /// the history worth keeping.
    fn capture(&mut self, server: &Server) {
        self.contents.retain(|id, _| server.panes.contains_key(id));
        for (id, slot) in &server.panes {
            if slot.pane.modes().alt_screen {
                continue;
            }
            let text = slot.pane.text(CONTENT_LINES);
            if text.is_empty() {
                self.contents.remove(id);
            } else {
                self.contents.insert(*id, text);
            }
        }
        self.contents_at = Some(Instant::now());
    }

    /// The layout as it is, without the panes' text.
    fn snapshot(&mut self, server: &Server) -> Saved {
        let sessions_by_pid = claude_sessions();
        if let Some(r) = &mut self.restored
            && r.at.elapsed() >= EXPECT_FOR
        {
            r.expect.clear();
        }
        let mut sessions = Vec::new();
        for s in &server.sessions {
            let mut tabs = Vec::new();
            // The Notifications window is made again when wanted, not restored.
            for tab in s
                .tabs
                .iter()
                .filter(|t| !server.is_notices_tab(**t))
                .filter_map(|t| server.tabs.get(t))
            {
                // A pane waiting behind a tool is saved in its place.
                let (layout, active, last_pane) = server.saved_layout(tab);
                let mut panes = Vec::new();
                for id in layout.panes() {
                    let Some(slot) = server.panes.get(&id) else {
                        continue;
                    };
                    let mut claude = claude_of(&slot.pane, &sessions_by_pid);
                    let mut command = if claude.is_some() {
                        None
                    } else {
                        program_of(&slot.pane)
                    };
                    // A restored pane still at its shell keeps what it ran;
                    // once it runs anything, that is what is saved.
                    if let Some(r) = &mut self.restored {
                        match r.expect.get(&id) {
                            Some(e) if claude.is_none() && command.is_none() => {
                                claude = e.claude.clone();
                                command = e.command.clone();
                            }
                            Some(_) => {
                                r.expect.remove(&id);
                            }
                            None => {}
                        }
                    }
                    panes.push(SavedPane {
                        id,
                        cwd: slot.pane.cwd().map(|d| d.to_string_lossy().into_owned()),
                        opts: slot.opts.clone(),
                        command,
                        claude,
                        text: None,
                    });
                }
                tabs.push(SavedTab {
                    id: tab.id,
                    name: tab.name.clone(),
                    layout,
                    active,
                    last_pane,
                    zoomed: tab.zoomed,
                    opts: tab.opts.clone(),
                    panes,
                });
            }
            sessions.push(SavedSession {
                name: s.name.clone(),
                cols: s.cols,
                rows: s.rows,
                current: s.current,
                last: s.last,
                tabs,
            });
        }
        Saved {
            version: VERSION,
            sessions,
        }
    }

    /// Hands a file to the writer thread, started the first time; written
    /// here when no thread can be.
    fn send(&mut self, json: String, done: Option<channel::SyncSender<io::Result<()>>>) {
        self.asked_at = Some(std::time::SystemTime::now());
        let job = Job {
            dir: paths::state_dir(),
            json,
            done,
        };
        if self.writer.is_none() {
            let (tx, rx) = channel::channel::<Job>();
            let thread = std::thread::Builder::new()
                .name("layout writer".into())
                .spawn(move || {
                    for job in rx {
                        run_job(job);
                    }
                });
            if let Ok(thread) = thread {
                self.writer = Some(Writer { tx, thread });
            }
        }
        match &self.writer {
            Some(w) => {
                if let Err(channel::SendError(job)) = w.tx.send(job) {
                    run_job(job);
                }
            }
            None => run_job(job),
        }
    }

    /// Types what a restore left to type, once the shells have had a moment.
    fn type_restored(&mut self, server: &mut Server) {
        let Some(r) = &mut self.restored else {
            return;
        };
        if r.typing.is_empty() || r.at.elapsed() < TYPE_AFTER {
            return;
        }
        for (id, line) in std::mem::take(&mut r.typing) {
            if let Some(slot) = server.panes.get_mut(&id) {
                let _ = slot.pane.write(format!("{line}\r").as_bytes());
            }
        }
    }
}

fn run_job(job: Job) {
    let result = write_file(&job.dir, &job.json);
    if let Err(e) = &result {
        eprintln!("switchboard server: saving the layout: {e}");
    }
    if let Some(done) = job.done {
        let _ = done.send(result);
    }
}

// ── Restoring ───────────────────────────────────────────────────────────────

/// When the server starts: brings back the last save, if the server has no
/// sessions yet and `auto_restore` is on.
pub fn restore(server: &mut Server) {
    if !server.sessions.is_empty() || !settings::is_on("auto_restore") {
        return;
    }
    let Some(saved) = load_from(&paths::state_dir()) else {
        return;
    };
    let mut p = std::mem::take(&mut server.persist);
    p.restore(server, saved);
    server.persist = p;
}

impl Persist {
    fn restore(&mut self, server: &mut Server, saved: Saved) {
        let mut used_tabs = HashSet::new();
        let mut used_panes = HashSet::new();
        let mut feeds: Vec<(PaneId, String)> = Vec::new();
        let mut typing = Vec::new();
        let mut expect = HashMap::new();
        for ss in saved.sessions {
            if ss.name.is_empty() || server.session(&ss.name).is_some() {
                continue;
            }
            let cols = if ss.cols == 0 { 80 } else { ss.cols };
            let rows = if ss.rows == 0 { 24 } else { ss.rows };
            let area = Rect::new(0, 0, cols, rows.saturating_sub(1).max(1));
            let mut tabs = Vec::new();
            for st in ss.tabs {
                if !used_tabs.insert(st.id) {
                    continue;
                }
                let id = st.id;
                let Some(restored) = restore_tab(server, st, area, &mut used_panes) else {
                    continue;
                };
                tabs.push(id);
                for pane in restored {
                    if let Some(line) = command_line(&pane) {
                        typing.push((pane.id, line));
                        expect.insert(
                            pane.id,
                            Expect {
                                claude: pane.claude.clone(),
                                command: pane.command.clone(),
                            },
                        );
                    }
                    if let Some(text) = pane.text.filter(|t| !t.is_empty()) {
                        feeds.push((pane.id, text));
                    }
                }
            }
            if tabs.is_empty() {
                continue;
            }
            let current = if tabs.contains(&ss.current) {
                ss.current
            } else {
                tabs[0]
            };
            let last = ss.last.filter(|t| tabs.contains(t) && *t != current);
            let sidebar = server.sidebar_columns(cols);
            server.sessions.push(Session {
                name: ss.name,
                tabs: tabs.clone(),
                current,
                last,
                cols,
                rows,
                sidebar,
            });
            for t in tabs {
                server.relayout(t);
                server.refresh_name(t);
            }
        }
        // Fed once each pane has its size, so the lines wrap as they will
        // be shown, and before the shell's first output is read, which
        // waits on the server's loop.
        for (id, text) in feeds {
            if let Some(slot) = server.panes.get_mut(&id) {
                let mut bytes = text.replace('\n', "\r\n");
                bytes.push_str("\r\n");
                slot.pane.process(bytes.as_bytes());
                self.contents.insert(id, text);
            }
        }
        self.contents_at = Some(Instant::now());
        self.restored = Some(Restored {
            at: Instant::now(),
            typing,
            expect,
        });
    }
}

/// Brings back one tab, giving its panes as saved; `None` when none of its
/// panes could be started. A pane that is in two places, or can't start, is
/// left out of the layout.
fn restore_tab(
    server: &mut Server,
    st: SavedTab,
    area: Rect,
    used: &mut HashSet<PaneId>,
) -> Option<Vec<SavedPane>> {
    let mut layout = st.layout;
    layout.tidy();
    let mut saved: HashMap<PaneId, SavedPane> = st.panes.into_iter().map(|p| (p.id, p)).collect();
    let rects = layout.place(area);
    let mut started = Vec::new();
    for id in layout.panes() {
        let ok = !used.contains(&id) && {
            let pane = saved.get(&id);
            let dir = pane
                .and_then(|p| p.cwd.as_deref())
                .map(PathBuf::from)
                .filter(|d| d.is_dir())
                .unwrap_or_else(paths::home);
            let rect = rects.rect(id).unwrap_or(area);
            match server.spawn_pane_as(id, st.id, Some(dir), None, rect.width, rect.height) {
                Ok(_) => true,
                Err(e) => {
                    eprintln!("switchboard server: restoring pane {id}: {e}");
                    false
                }
            }
        };
        if !ok {
            if layout.remove(id) {
                continue;
            }
            // It was the only one left, so nothing of the tab started.
            for p in started {
                if let Some(mut slot) = server.panes.remove(&p) {
                    slot.pane.kill();
                }
            }
            return None;
        }
        used.insert(id);
        server.next_pane = server.next_pane.max(id.0.saturating_add(1));
        let pane = saved.entry(id).or_insert_with(|| SavedPane::new(id));
        if let Some(slot) = server.panes.get_mut(&id) {
            // The pane comes back as its shell, so what said what its
            // program was, or allowed it, stays behind with it.
            slot.opts = pane
                .opts
                .iter()
                .filter(|(name, _)| !super::respawn::PROGRAM_MARKS.contains(&name.as_str()))
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect();
        }
        started.push(id);
    }
    let first = *started.first()?;
    let panes = layout.panes();
    let mut tab = new_tab(st.id, first);
    tab.name = st.name;
    tab.active = if panes.contains(&st.active) {
        st.active
    } else {
        first
    };
    tab.last_pane = st
        .last_pane
        .filter(|p| panes.contains(p) && *p != tab.active);
    tab.zoomed = st.zoomed && panes.len() > 1;
    tab.opts = st.opts;
    tab.layout = layout;
    server.tabs.insert(st.id, tab);
    server.next_tab = server.next_tab.max(st.id.0.saturating_add(1));
    Some(
        panes
            .into_iter()
            .filter_map(|id| saved.remove(&id))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Saved {
        let layout = Layout::Split {
            across: true,
            children: vec![Layout::Pane(PaneId(3)), Layout::Pane(PaneId(4))],
            shares: vec![0.5, 0.5],
        };
        Saved {
            version: VERSION,
            sessions: vec![SavedSession {
                name: "main".into(),
                cols: 120,
                rows: 40,
                current: TabId(2),
                last: Some(TabId(1)),
                tabs: vec![
                    SavedTab {
                        id: TabId(1),
                        name: String::new(),
                        layout: Layout::Pane(PaneId(1)),
                        active: PaneId(1),
                        last_pane: None,
                        zoomed: false,
                        opts: BTreeMap::new(),
                        panes: vec![SavedPane {
                            cwd: Some("/tmp".into()),
                            text: Some("$ echo hi\nhi\n$".into()),
                            ..SavedPane::new(PaneId(1))
                        }],
                    },
                    SavedTab {
                        id: TabId(2),
                        name: "work".into(),
                        layout,
                        active: PaneId(4),
                        last_pane: Some(PaneId(3)),
                        zoomed: true,
                        opts: BTreeMap::from([("@tab_note".into(), "Today".into())]),
                        panes: vec![
                            SavedPane {
                                claude: Some("0b8c-41aa".into()),
                                ..SavedPane::new(PaneId(3))
                            },
                            SavedPane {
                                command: Some(vec!["nvim".into(), "my file.rs".into()]),
                                opts: BTreeMap::from([("@x".into(), "y".into())]),
                                ..SavedPane::new(PaneId(4))
                            },
                        ],
                    },
                ],
            }],
        }
    }

    #[test]
    fn a_save_reads_back_as_written() {
        let saved = sample();
        let text = util::json_pretty(&saved);
        assert_eq!(parse(&text).unwrap(), saved);
        let json: Value = serde_json::from_str(&text).unwrap();
        let pane = &json["sessions"][0]["tabs"][0]["panes"][0];
        assert_eq!(pane["id"], 1, "ids are plain numbers");
        assert!(
            pane.get("claude").is_none() && pane.get("opts").is_none(),
            "what a pane doesn't have is left out: {pane}"
        );
        assert_eq!(
            json["sessions"][0]["tabs"][1]["layout"]["Split"]["across"],
            true
        );
    }

    #[test]
    fn what_is_left_out_reads_as_nothing() {
        let text = r#"{"version":1,"sessions":[{"name":"main","current":1,
            "tabs":[{"id":1,"layout":{"Pane":1},"active":1}]}]}"#;
        let saved = parse(text).unwrap();
        let tab = &saved.sessions[0].tabs[0];
        assert!(tab.panes.is_empty() && tab.name.is_empty() && !tab.zoomed);
        assert_eq!(saved.sessions[0].cols, 0);
    }

    #[test]
    fn another_version_or_garbage_is_refused() {
        assert!(parse("{").is_err());
        assert!(parse("[]").is_err());
        let mut saved = sample();
        saved.version = 99;
        assert!(parse(&util::json_compact(&saved)).is_err());
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sb-persist-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_file_that_doesnt_parse_is_set_aside() {
        let dir = temp_dir("bad");
        fs::write(dir.join(FILE), "{ not json").unwrap();
        fs::write(dir.join(PREVIOUS), util::json_compact(&sample())).unwrap();
        assert_eq!(load_from(&dir), None, "a bad save starts the server fresh");
        assert!(!dir.join(FILE).exists());
        assert_eq!(
            fs::read_to_string(dir.join(SET_ASIDE)).unwrap(),
            "{ not json"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn writing_keeps_the_one_before_and_a_lost_file_falls_back_to_it() {
        let dir = temp_dir("write");
        let first = sample();
        let mut second = sample();
        second.sessions[0].name = "other".into();
        write_file(&dir, &util::json_pretty(&first)).unwrap();
        write_file(&dir, &util::json_pretty(&second)).unwrap();
        assert_eq!(load_from(&dir), Some(second));
        let mode = fs::metadata(dir.join(FILE)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "what panes showed is yours alone");
        fs::remove_file(dir.join(FILE)).unwrap();
        assert_eq!(load_from(&dir), Some(first));
        fs::remove_dir_all(&dir).unwrap();
        let empty = temp_dir("none");
        assert_eq!(load_from(&empty), None, "no save, nothing restored");
        fs::remove_dir_all(&empty).unwrap();
    }

    #[test]
    fn claude_and_safe_programs_are_typed_again_and_nothing_else() {
        let tabs = &sample().sessions[0].tabs;
        assert_eq!(
            command_line(&tabs[1].panes[0]).as_deref(),
            Some("claude --resume 0b8c-41aa")
        );
        assert_eq!(
            command_line(&tabs[1].panes[1]).as_deref(),
            Some("nvim 'my file.rs'")
        );
        assert_eq!(command_line(&tabs[0].panes[0]), None);
        let build = SavedPane {
            command: Some(vec!["make".into(), "deploy".into()]),
            ..SavedPane::new(PaneId(0))
        };
        assert_eq!(command_line(&build), None, "a command isn't run again");
        let odd = SavedPane {
            claude: Some("x; rm -rf ~".into()),
            ..SavedPane::new(PaneId(0))
        };
        assert_eq!(command_line(&odd), None);
        let path = SavedPane {
            command: Some(vec!["/usr/bin/less".into(), "it's".into()]),
            ..SavedPane::new(PaneId(0))
        };
        assert_eq!(
            command_line(&path).as_deref(),
            Some(r"/usr/bin/less 'it'\''s'")
        );
    }

    #[test]
    fn shells_are_known_by_name() {
        assert_eq!(program_name("-zsh"), "zsh");
        assert_eq!(program_name("/usr/bin/nvim"), "nvim");
        assert!(is_shell("bash") && is_shell("fish"));
        assert!(!is_shell("nvim"));
    }
}
