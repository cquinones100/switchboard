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
//! tick, and one with fewer panes than the last written once it has held
//! `SHRINK_HOLD`, so the panes of a shutting-down machine going one by one
//! (each shell hung up in turn) are never caught half gone, and no more often
//! than every `MIN_GAP`. What panes show changes all the time, so it is read again only
//! every `CONTENTS_EVERY`, as continuum saved once a minute, and when the
//! server ends or `switchboard save` asks. A server ending with no tabs left
//! writes nothing, so the last workspace comes back, as continuum restored its
//! last save. The file before each write is kept as `layout.json.1`, which a
//! restore reads when a crash left no `layout.json`, and a server starting
//! fresh keeps the larger of the two as `layout.json.start` before anything
//! is written, until its next fresh start, so the saves written after a start
//! that brought back too little (or nothing) can't take what was there with
//! them; `switchboard restore-layout` brings it back. Writing runs on a thread
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
//! has that program typed the same way, as does a pane running Notes (`switchboard
//! notes`, which was Neovim), and any other comes back as its shell,
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
use super::trail::Trail;
use super::{PaneId, TabId, procinfo};
use crate::{paths, settings, util};

/// The file, in the state directory.
pub const FILE: &str = "layout.json";
/// The one before it.
const PREVIOUS: &str = "layout.json.1";
/// The save found as the server last started fresh, the larger of `FILE`
/// and `PREVIOUS` then, kept until the next fresh start, so the two saves
/// written after a start can't take the workspace from before it with them.
pub const START: &str = "layout.json.start";
/// Where a file that couldn't be read is put.
const SET_ASIDE: &str = "layout.json.bad";
/// The file's shape; one of another version is set aside, not guessed at.
const VERSION: u32 = 1;
/// Lines of history kept per pane, above its screen.
pub const CONTENT_LINES: usize = 2000;
/// The least time between two writes.
const MIN_GAP: Duration = Duration::from_secs(10);
/// How long a layout with fewer panes than the last one written must hold
/// before it is written, so the panes a shutting-down computer hangs up one
/// by one never reach the file.
const SHRINK_HOLD: Duration = Duration::from_secs(60);
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
    /// The tabs visited each way (src/mux/trail.rs), handed over to a new
    /// build only: a server started fresh starts with none.
    #[serde(default, skip_serializing_if = "Trail::is_empty")]
    pub trail: Trail,
    pub tabs: Vec<SavedTab>,
    /// The repository its sidebar is filtered to (src/sidebar's
    /// `Side::repo`), and whether its Other repos was unfolded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_repo: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sidebar_repo_open: bool,
    /// The sidebar's groups folded with `z` or a click, by their stages'
    /// keys, and whether its notifications were.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sidebar_folded: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sidebar_notices_folded: bool,
}

/// A session's sidebar folds as they are kept (`SavedSession`).
fn folds_of(side: Option<&crate::sidebar::Side>) -> (Vec<String>, bool) {
    side.map_or((Vec::new(), false), |x| {
        (
            x.folded.iter().map(|s| s.key().to_string()).collect(),
            x.notices_folded,
        )
    })
}

/// A session's sidebar folds taken back from what was kept.
fn restore_folds(side: &mut crate::sidebar::Side, folded: &[String], notices_folded: bool) {
    side.folded = folded
        .iter()
        .filter_map(|k| crate::sidebar::Stage::from_key(k))
        .collect();
    side.notices_folded = notices_folded;
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

/// How many panes a save holds.
fn pane_count(saved: &Saved) -> usize {
    saved
        .sessions
        .iter()
        .flat_map(|s| &s.tabs)
        .map(|t| t.layout.panes().len())
        .sum()
}

/// How many tabs a save holds.
fn tab_count(saved: &Saved) -> usize {
    saved.sessions.iter().map(|s| s.tabs.len()).sum()
}

/// As a server starts fresh, before anything is written: keeps the larger of
/// `layout.json` and `layout.json.1` (`layout.json` when they are alike) as
/// `layout.json.start`, whether or not it is restored, so a restore that
/// brings back less than was there, or none at all, still leaves it to bring
/// back with `switchboard restore-layout`. With neither to read, the one kept
/// at an earlier start stays.
pub fn keep_start(dir: &Path) {
    let best = [PREVIOUS, FILE]
        .into_iter()
        .filter_map(|name| {
            let text = fs::read_to_string(dir.join(name)).ok()?;
            let panes = pane_count(&parse(&text).ok()?);
            Some((panes, text))
        })
        .max_by_key(|(panes, _)| *panes);
    let Some((_, text)) = best else {
        return;
    };
    let file = dir.join(START);
    let kept = util::write_atomic(&file, &text)
        .and_then(|()| fs::set_permissions(&file, fs::Permissions::from_mode(0o600)));
    if let Err(e) = kept {
        eprintln!(
            "switchboard server: keeping the layout from before the start as {}: {e}",
            paths::tilde(&file)
        );
    }
}

/// How long a layout with fewer panes than the last one written holds before
/// it is written: `SHRINK_HOLD`, or `SWITCHBOARD_SHRINK_HOLD_MS` in its place,
/// which the tests set.
fn shrink_hold() -> Duration {
    std::env::var("SWITCHBOARD_SHRINK_HOLD_MS")
        .ok()
        .and_then(|ms| ms.trim().parse().ok())
        .map_or(SHRINK_HOLD, Duration::from_millis)
}

/// Whether a layout of `panes` panes, that many for `held_for`, waits to be
/// written: it has fewer than the last one written and hasn't held `hold`.
fn shrink_waits(written: Option<usize>, panes: usize, held_for: Duration, hold: Duration) -> bool {
    written.is_some_and(|n| panes < n) && held_for < hold
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

/// Whether a pane sits at its own shell's prompt: the process in front
/// is the pane's own, and a shell (not a tool started in its place, as
/// the diff is, nor an editor a shell `exec`ed).
pub(crate) fn at_its_shell(pane: &Pane) -> bool {
    match (pane.foreground_pgid(), pane.pid()) {
        (Some(front), Some(own)) if front == own => {
            procinfo::command_of(front).is_some_and(|c| is_shell(program_name(&c)))
        }
        _ => false,
    }
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
    // Notes, which ran in Neovim, came back as resurrect brought Neovim back.
    let notes = name == "switchboard" && argv.len() == 2 && argv[1] == "notes";
    (notes || RESTARTABLE.contains(&name)).then(|| {
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
    /// How many panes the last file written held.
    written_panes: Option<usize>,
    /// The layout as the last tick found it, its panes, how many of them
    /// the save holds, and since when it has held that many, which moving
    /// between tabs and panes doesn't change.
    seen: Option<u64>,
    seen_panes: HashSet<PaneId>,
    seen_count: usize,
    count_since: Option<Instant>,
    /// Whether `switchboard kill-server` ended the server, rather than a
    /// signal, as a shutting-down computer sends.
    ended_by_hand: bool,
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
/// may hear of some before it is told to end), or the last tick found fewer
/// panes than were written and they haven't held `SHRINK_HOLD`, unless
/// `switchboard kill-server` ended it (`ended_by_hand`), when the save from
/// before they went stays.
pub fn shutdown(server: &mut Server) {
    let mut p = std::mem::take(&mut server.persist);
    if !server.sessions.is_empty() {
        let now: HashSet<PaneId> = server.panes.keys().copied().collect();
        let held_for = p.count_since.map_or(Duration::ZERO, |t| t.elapsed());
        let shrinking = !p.ended_by_hand
            && shrink_waits(p.written_panes, p.seen_count, held_for, shrink_hold());
        if p.seen_panes.is_subset(&now)
            && !shrinking
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

/// Waits for a save being written to be done, before the server hands
/// itself to a new build (src/mux/upgrade.rs), which would end the thread
/// writing it; the next save starts the thread again.
pub fn finish_writing(server: &mut Server) {
    if let Some(w) = server.persist.writer.take() {
        drop(w.tx);
        let _ = w.thread.join();
    }
}

/// `switchboard kill-server`: the server ends because you asked, so the last
/// save is made even while a layout that lost panes is held.
pub fn ended_by_hand(server: &mut Server) {
    server.persist.ended_by_hand = true;
}

/// How many panes the last save held, handed to a new build
/// (src/mux/upgrade.rs).
pub fn written_panes(server: &Server) -> Option<usize> {
    server.persist.written_panes
}

/// Takes up the count `written_panes` handed over, so a layout that lost
/// panes before an upgrade holds `SHRINK_HOLD` from it, counted again from
/// the new build's first tick.
pub fn take_written_panes(server: &mut Server, written: Option<usize>) {
    server.persist.written_panes = written;
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
        let count = pane_count(&shape_saved);
        if self.count_since.is_none() || count != self.seen_count {
            self.count_since = Some(Instant::now());
        }
        self.seen = Some(shape);
        self.seen_panes = server.panes.keys().copied().collect();
        self.seen_count = count;
        if !held || self.written_at.is_some_and(|t| t.elapsed() < MIN_GAP) {
            return;
        }
        let held_for = self.count_since.map_or(Duration::ZERO, |t| t.elapsed());
        if shrink_waits(self.written_panes, self.seen_count, held_for, shrink_hold()) {
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
        self.written_panes = Some(self.seen_count);
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
        let panes = pane_count(&shape_saved);
        let json = self.full(shape_saved, with_text);
        self.written = Some(hash_of(&json));
        self.written_shape = Some(shape);
        self.written_panes = Some(panes);
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
                        // Where its program started, which `claude --resume` finds
                        // its session by, rather than where Claude moved to
                        // since; never a directory since removed.
                        cwd: slot
                            .pane
                            .cwd()
                            .and_then(|d| util::nearest_existing(&d))
                            .map(|d| d.to_string_lossy().into_owned()),
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
            let side = server.sidebar.sides.get(&s.name);
            sessions.push(SavedSession {
                name: s.name.clone(),
                cols: s.cols,
                rows: s.rows,
                current: s.current,
                last: s.last,
                trail: Trail::default(),
                tabs,
                sidebar_repo: side.and_then(|x| x.repo.clone()),
                sidebar_repo_open: side.is_some_and(|x| x.repo.is_some() && x.repo_open),
                sidebar_folded: folds_of(side).0,
                sidebar_notices_folded: folds_of(side).1,
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

impl Server {
    /// Whether a restore has commands still to type into its panes (a
    /// Claude to resume, a program to start again), for what would start
    /// something in a pane to wait on (src/routines).
    pub(crate) fn restore_typing(&self) -> bool {
        self.persist
            .restored
            .as_ref()
            .is_some_and(|r| !r.typing.is_empty())
    }
}

/// When the server starts: keeps the save found as `layout.json.start`
/// (`keep_start`), and brings back the last save, if the server has no
/// sessions yet and `auto_restore` is on.
pub fn restore(server: &mut Server) {
    if !server.sessions.is_empty() {
        return;
    }
    let dir = paths::state_dir();
    keep_start(&dir);
    if !settings::is_on("auto_restore") {
        return;
    }
    let Some(saved) = load_from(&dir) else {
        return;
    };
    let mut p = std::mem::take(&mut server.persist);
    p.restore(server, saved);
    server.persist = p;
    server.number_tabs();
}

/// Who asked for `restore-layout`, answered once its file is read.
pub enum Asker {
    /// `switchboard restore-layout`, answered on its socket.
    Command(tokio::sync::oneshot::Sender<super::server::Answer>),
    /// M-p's "Restore the layout from before the last start", said in a box.
    Client(super::server::ClientId),
}

/// A save `restore-layout` read off the server's task, and the Claude
/// sessions running then.
pub struct LayoutRead {
    asker: Asker,
    /// Whether it is the state directory's own `layout.json.start`, the
    /// only save whose commands are typed again.
    own: bool,
    saved: Result<Saved, String>,
    sessions: HashMap<u32, String>,
}

/// Reads and parses a save for `restore-layout`, saying what is wrong.
fn read_layout(file: &Path) -> Result<Saved, String> {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(format!(
                "there is no {} to restore from",
                paths::tilde(file)
            ));
        }
        Err(e) => return Err(format!("couldn't read {}: {e}", paths::tilde(file))),
    };
    parse(&text).map_err(|e| format!("{} isn't a saved layout: {e:#}", paths::tilde(file)))
}

/// What `restore_layout` says it did.
pub fn opened_words(tabs: usize) -> String {
    format!(
        "Brought back {tabs} tab{} from before the last start.",
        if tabs == 1 { "" } else { "s" }
    )
}

impl Server {
    /// `switchboard restore-layout [<file>]`, and M-p's "Restore the layout
    /// from before the last start": reads the save (`layout.json.start`
    /// unless a file is given) in a task of its own, since it can be
    /// megabytes of the panes' history, then `restore_layout_read` brings it
    /// back.
    pub(crate) fn restore_layout_asked(&mut self, asker: Asker, file: Option<PathBuf>) {
        let own = file.is_none();
        let file = file.unwrap_or_else(|| paths::state_dir().join(START));
        let tx = self.tx.clone();
        let read = move || {
            let saved = read_layout(&file);
            let sessions = claude_sessions();
            let _ = tx.send(super::server::Event::RestoreLayout(Box::new(LayoutRead {
                asker,
                own,
                saved,
                sessions,
            })));
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::spawn_blocking(read);
        } else {
            read();
        }
    }

    /// The save read: brought back and answered.
    pub(crate) fn restore_layout_read(&mut self, read: LayoutRead) {
        let LayoutRead {
            asker,
            own,
            saved,
            sessions,
        } = read;
        let result = saved.and_then(|saved| restore_layout(self, saved, &sessions, own));
        match asker {
            Asker::Command(reply) => {
                let answer = match result {
                    Ok(n) => super::server::Reply::ok(format!("{}\n", opened_words(n))),
                    Err(e) => super::server::Reply::fail(e),
                };
                let _ = reply.send(super::server::Answer::Reply(answer));
            }
            Asker::Client(client) => {
                let words = match result {
                    Ok(n) => opened_words(n),
                    Err(e) => format!("The layout wasn't restored: {e}."),
                };
                self.say(Some(client), &words);
            }
        }
    }
}

/// Brings back the sessions, tabs and panes of a save beside the one tab the
/// workspace has, as a server start does, giving how many tabs it opened.
/// It refuses while the workspace has other tabs, since their Claude
/// sessions would be resumed twice, and when the save has no more tabs than
/// the workspace. A tab or pane whose id is taken is given a new one, and a
/// pane whose Claude session already runs in the workspace (`sessions`, the
/// one tab kept may be one the start brought back) comes back as its shell
/// alone. A tool and the pane it stood in for (src/diff/in_place.rs) come
/// back as two shells that don't name each other. Only the state directory's
/// own save (`own`) has its Claude sessions and programs typed again and
/// its panes' text shown; one named by hand comes back as fresh shells, with
/// ids the server gives.
fn restore_layout(
    server: &mut Server,
    mut saved: Saved,
    sessions: &HashMap<u32, String>,
    own: bool,
) -> Result<usize, String> {
    let open = server
        .sessions
        .iter()
        .flat_map(|s| &s.tabs)
        .filter(|t| !server.is_notices_tab(**t))
        .count();
    if open > 1 {
        return Err(format!(
            "the workspace has {open} tabs; close all but one first, so no Claude session is resumed twice"
        ));
    }
    let in_file = tab_count(&saved);
    if in_file <= open {
        return Err(format!(
            "the saved layout has {in_file} tab{}, no more than the workspace has",
            if in_file == 1 { "" } else { "s" }
        ));
    }
    let running: HashSet<String> = server
        .panes
        .values()
        .filter_map(|slot| claude_of(&slot.pane, sessions))
        .collect();
    for pane in saved
        .sessions
        .iter_mut()
        .flat_map(|s| s.tabs.iter_mut())
        .flat_map(|t| t.panes.iter_mut())
    {
        // A file named by hand can come from anywhere, so nothing it names
        // is typed and none of its text reaches a terminal: each pane comes
        // back as a fresh shell.
        if !own {
            pane.command = None;
            pane.claude = None;
            pane.text = None;
        }
        if pane.claude.as_ref().is_some_and(|c| running.contains(c)) {
            pane.claude = None;
        }
    }
    renumber(server, &mut saved, !own);
    let before = server.tabs.len();
    let mut p = std::mem::take(&mut server.persist);
    p.restore(server, saved);
    server.persist = p;
    server.number_tabs();
    let names: Vec<String> = server.sessions.iter().map(|s| s.name.clone()).collect();
    for name in names {
        server.dirty_session(&name);
    }
    Ok(server.tabs.len().saturating_sub(before))
}

/// Gives each tab and pane of a save whose id the server already uses (or
/// the save uses twice) a new one, so it can be restored beside them; with
/// `fresh`, every one, from the server's own counters, so a file named by
/// hand can't choose the ids the server goes on to give.
fn renumber(server: &Server, saved: &mut Saved, fresh: bool) {
    let tabs = saved.sessions.iter().flat_map(|s| &s.tabs);
    let (mut next_tab, mut next_pane) = if fresh {
        (server.next_tab, server.next_pane)
    } else {
        (
            tabs.clone()
                .map(|t| t.id.0.saturating_add(1))
                .fold(server.next_tab, u32::max),
            tabs.flat_map(|t| t.layout.panes())
                .map(|p| p.0.saturating_add(1))
                .fold(server.next_pane, u32::max),
        )
    };
    let mut used_tabs: HashSet<TabId> = server.tabs.keys().copied().collect();
    let mut used_panes: HashSet<PaneId> = server.panes.keys().copied().collect();
    for ss in &mut saved.sessions {
        let mut moved: HashMap<TabId, TabId> = HashMap::new();
        for st in &mut ss.tabs {
            if fresh || !used_tabs.insert(st.id) {
                let new = TabId(next_tab);
                next_tab += 1;
                used_tabs.insert(new);
                moved.insert(st.id, new);
                st.id = new;
            }
            let mut map: HashMap<PaneId, PaneId> = HashMap::new();
            for old in st.layout.panes() {
                if !fresh && used_panes.insert(old) {
                    continue;
                }
                let new = PaneId(next_pane);
                next_pane += 1;
                used_panes.insert(new);
                map.insert(old, new);
            }
            // All at once, so a new id that is also an old one isn't taken
            // for it.
            relabel(&mut st.layout, &map);
            for pane in &mut st.panes {
                pane.id = map.get(&pane.id).copied().unwrap_or(pane.id);
            }
            st.active = map.get(&st.active).copied().unwrap_or(st.active);
            st.last_pane = st.last_pane.map(|p| map.get(&p).copied().unwrap_or(p));
        }
        ss.current = moved.get(&ss.current).copied().unwrap_or(ss.current);
        ss.last = ss.last.map(|t| moved.get(&t).copied().unwrap_or(t));
    }
}

/// A layout's panes given the ids `map` gives them.
fn relabel(layout: &mut Layout, map: &HashMap<PaneId, PaneId>) {
    match layout {
        Layout::Pane(id) => *id = map.get(id).copied().unwrap_or(*id),
        Layout::Split { children, .. } => {
            for child in children {
                relabel(child, map);
            }
        }
    }
}

impl Persist {
    fn restore(&mut self, server: &mut Server, saved: Saved) {
        let mut used_tabs = HashSet::new();
        let mut used_panes = HashSet::new();
        let mut feeds: Vec<(PaneId, String)> = Vec::new();
        let mut typing = Vec::new();
        let mut expect = HashMap::new();
        for ss in saved.sessions {
            if ss.name.is_empty() {
                continue;
            }
            // `restore-layout` brings tabs back beside a session's own.
            let existing = server.session(&ss.name).map(|s| (s.cols, s.rows));
            let (cols, rows) = existing.unwrap_or((ss.cols, ss.rows));
            let cols = if cols == 0 { 80 } else { cols };
            let rows = if rows == 0 { 24 } else { rows };
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
            if let Some(session) = server.session_mut(&ss.name)
                && existing.is_some()
            {
                session.tabs.extend(tabs.iter().copied());
                for t in tabs {
                    server.relayout(t);
                    server.refresh_name(t);
                }
                continue;
            }
            let current = if tabs.contains(&ss.current) {
                ss.current
            } else {
                tabs[0]
            };
            let last = ss.last.filter(|t| tabs.contains(t) && *t != current);
            let sidebar = server.sidebar_columns(cols);
            let side = server.sidebar.side(&ss.name);
            restore_folds(side, &ss.sidebar_folded, ss.sidebar_notices_folded);
            side.repo = ss.sidebar_repo;
            side.repo_open = ss.sidebar_repo_open;
            server.sessions.push(Session {
                name: ss.name,
                tabs: tabs.clone(),
                current,
                last,
                trail: Trail::default(),
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

/// What makes a pane a tool standing in for another, or one waiting behind
/// a tool (src/diff/in_place.rs): a restored pane is a shell, and its ids may
/// be given to others (`renumber`), so none of them comes back.
const TOOL_MARKS: &[&str] = &[
    crate::diff::in_place::PARKED_PANE,
    crate::diff::in_place::IN_PLACE,
    crate::diff::in_place::PANE_PARKED,
];

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
                .filter(|(name, _)| {
                    !super::respawn::PROGRAM_MARKS.contains(&name.as_str())
                        && !TOOL_MARKS.contains(&name.as_str())
                })
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

// ── Handing over ────────────────────────────────────────────────────────────

/// The layout as it is, for handing the server to a new build
/// (src/mux/upgrade.rs) in this file's shape: every tab, the Notifications
/// window too, with its layout as it stands (a tool in a pane's place
/// still in it) and every pane of it, those parked behind a tool too, each
/// with its options. Nothing is read from the panes' processes, since the
/// panes go on as they are.
pub fn layout_now(server: &Server) -> Saved {
    let sessions = server
        .sessions
        .iter()
        .map(|s| SavedSession {
            name: s.name.clone(),
            cols: s.cols,
            rows: s.rows,
            current: s.current,
            last: s.last,
            trail: s.trail.clone(),
            sidebar_repo: server
                .sidebar
                .sides
                .get(&s.name)
                .and_then(|x| x.repo.clone()),
            sidebar_repo_open: server
                .sidebar
                .sides
                .get(&s.name)
                .is_some_and(|x| x.repo.is_some() && x.repo_open),
            sidebar_folded: folds_of(server.sidebar.sides.get(&s.name)).0,
            sidebar_notices_folded: folds_of(server.sidebar.sides.get(&s.name)).1,
            tabs: s
                .tabs
                .iter()
                .filter_map(|t| server.tabs.get(t))
                .map(|tab| SavedTab {
                    id: tab.id,
                    name: tab.name.clone(),
                    layout: tab.layout.clone(),
                    active: tab.active,
                    last_pane: tab.last_pane,
                    zoomed: tab.zoomed,
                    opts: tab.opts.clone(),
                    panes: server
                        .panes_with_parked(tab.id)
                        .into_iter()
                        .filter_map(|id| {
                            let slot = server.panes.get(&id)?;
                            Some(SavedPane {
                                opts: slot.opts.clone(),
                                ..SavedPane::new(id)
                            })
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect();
    Saved {
        version: VERSION,
        sessions,
    }
}

/// Builds the sessions and tabs of a layout handed over (`layout_now`)
/// around the panes already taken up into `server.panes`, each given its
/// tab and options. A pane that wasn't taken up is left out of its tab's
/// layout, a tab none of whose panes was is left out, and a pane taken up
/// whose tab couldn't be built gets a tab of its own, so no running program
/// is left without a place.
pub fn adopt_layout(server: &mut Server, saved: Saved) {
    let mut placed: HashSet<PaneId> = HashSet::new();
    for ss in saved.sessions {
        if ss.name.is_empty() || server.session(&ss.name).is_some() {
            continue;
        }
        let cols = if ss.cols == 0 { 80 } else { ss.cols };
        let rows = if ss.rows == 0 { 24 } else { ss.rows };
        let mut tabs = Vec::new();
        for st in ss.tabs {
            if server.tabs.contains_key(&st.id) {
                continue;
            }
            let ours: Vec<PaneId> = st
                .panes
                .iter()
                .map(|p| p.id)
                .filter(|id| server.panes.contains_key(id) && !placed.contains(id))
                .collect();
            let mut layout = st.layout;
            layout.tidy();
            for id in layout.panes() {
                // The last of a layout can't be taken out; it is left out
                // below instead.
                if !ours.contains(&id) {
                    layout.remove(id);
                }
            }
            let mut in_layout: Vec<PaneId> = layout
                .panes()
                .into_iter()
                .filter(|id| ours.contains(id))
                .collect();
            if in_layout.is_empty() {
                // Only panes parked behind a tool that is gone: the first
                // takes the tool's place.
                let Some(&first) = ours.first() else {
                    continue;
                };
                layout = Layout::new(first);
                in_layout = vec![first];
                if let Some(slot) = server.panes.get_mut(&first) {
                    slot.opts.remove(crate::diff::in_place::PANE_PARKED);
                }
            }
            let mut tab = new_tab(st.id, in_layout[0]);
            tab.name = st.name;
            tab.active = if in_layout.contains(&st.active) {
                st.active
            } else {
                in_layout[0]
            };
            tab.last_pane = st
                .last_pane
                .filter(|p| in_layout.contains(p) && *p != tab.active);
            tab.zoomed = st.zoomed && in_layout.len() > 1;
            tab.opts = st.opts;
            tab.layout = layout;
            for id in &ours {
                if let Some(slot) = server.panes.get_mut(id) {
                    slot.tab = st.id;
                }
                placed.insert(*id);
            }
            server.tabs.insert(st.id, tab);
            server.next_tab = server.next_tab.max(st.id.0.saturating_add(1));
            tabs.push(st.id);
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
        let side = server.sidebar.side(&ss.name);
        restore_folds(side, &ss.sidebar_folded, ss.sidebar_notices_folded);
        side.repo = ss.sidebar_repo;
        side.repo_open = ss.sidebar_repo_open;
        let trail = ss.trail.keeping(&tabs);
        server.sessions.push(Session {
            name: ss.name,
            tabs,
            current,
            last,
            trail,
            cols,
            rows,
            sidebar,
        });
    }
    // Panes whose tab couldn't be built, each in a tab of its own.
    let mut homeless: Vec<PaneId> = server
        .panes
        .keys()
        .copied()
        .filter(|id| !placed.contains(id))
        .collect();
    homeless.sort();
    for id in homeless {
        if server.sessions.is_empty() {
            server.sessions.push(Session {
                name: super::server::DEFAULT_SESSION.to_string(),
                tabs: Vec::new(),
                current: TabId(0),
                last: None,
                trail: Trail::default(),
                cols: 80,
                rows: 24,
                sidebar: server.sidebar_columns(80),
            });
        }
        let tab = TabId(server.next_tab);
        server.next_tab += 1;
        if let Some(slot) = server.panes.get_mut(&id) {
            slot.tab = tab;
            slot.opts.remove(crate::diff::in_place::PANE_PARKED);
        }
        server.tabs.insert(tab, new_tab(tab, id));
        let s = &mut server.sessions[0];
        if s.tabs.is_empty() {
            s.current = tab;
        }
        s.tabs.push(tab);
    }
    for id in server.panes.keys() {
        server.next_pane = server.next_pane.max(id.0.saturating_add(1));
    }
    server.number_tabs();
    let tabs: Vec<TabId> = server.tabs.keys().copied().collect();
    for t in tabs {
        server.relayout(t);
        server.refresh_name(t);
    }
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
                trail: Default::default(),
                sidebar_repo: None,
                sidebar_repo_open: false,
                sidebar_folded: Vec::new(),
                sidebar_notices_folded: false,
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
    fn a_shrinking_layout_waits_for_the_hold_and_a_growing_one_doesnt() {
        let hold = Duration::from_secs(60);
        let soon = Duration::from_secs(5);
        assert!(shrink_waits(Some(3), 2, soon, hold), "a pane gone waits");
        assert!(
            !shrink_waits(Some(3), 2, hold, hold),
            "once held, it is written"
        );
        assert!(
            !shrink_waits(Some(3), 3, soon, hold),
            "the same size doesn't wait"
        );
        assert!(
            !shrink_waits(Some(3), 4, soon, hold),
            "a larger one doesn't wait"
        );
        assert!(
            !shrink_waits(None, 1, soon, hold),
            "nothing written yet, nothing waits"
        );
    }

    #[test]
    fn a_start_keeps_the_larger_save_and_keeps_the_last_with_none() {
        let dir = temp_dir("start");
        let big = sample();
        let mut small = sample();
        small.sessions[0].tabs.truncate(1);
        fs::write(dir.join(FILE), util::json_pretty(&small)).unwrap();
        fs::write(dir.join(PREVIOUS), util::json_pretty(&big)).unwrap();
        keep_start(&dir);
        let kept = || parse(&fs::read_to_string(dir.join(START)).unwrap()).unwrap();
        assert_eq!(kept(), big, "the larger of the two is kept");
        let mode = fs::metadata(dir.join(START)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        fs::remove_file(dir.join(FILE)).unwrap();
        fs::remove_file(dir.join(PREVIOUS)).unwrap();
        keep_start(&dir);
        assert_eq!(kept(), big, "with nothing to read, the one kept stays");
        let mut alike = sample();
        alike.sessions[0].name = "newer".into();
        fs::write(dir.join(FILE), util::json_pretty(&alike)).unwrap();
        fs::write(dir.join(PREVIOUS), util::json_pretty(&big)).unwrap();
        keep_start(&dir);
        assert_eq!(kept(), alike, "of two alike, layout.json is kept");
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
        let notes = SavedPane {
            command: Some(vec![
                "/home/jo/.local/bin/switchboard".into(),
                "notes".into(),
            ]),
            ..SavedPane::new(PaneId(0))
        };
        assert_eq!(
            command_line(&notes).as_deref(),
            Some("/home/jo/.local/bin/switchboard notes")
        );
        let other = SavedPane {
            command: Some(vec!["switchboard".into(), "wt".into(), "feature".into()]),
            ..SavedPane::new(PaneId(0))
        };
        assert_eq!(command_line(&other), None, "only Notes of the binary's own");
    }

    #[test]
    fn shells_are_known_by_name() {
        assert_eq!(program_name("-zsh"), "zsh");
        assert_eq!(program_name("/usr/bin/nvim"), "nvim");
        assert!(is_shell("bash") && is_shell("fish"));
        assert!(!is_shell("nvim"));
    }
}
