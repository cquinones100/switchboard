//! The queue for heavy work: only so many builds, test runs and package
//! builds go at once across every pane, each kind with its cap in Settings
//! ("Builds at once", "Test runs at once", "Package builds at once"), so
//! that ten tabs building together share the cores instead of each cargo
//! taking all of them and every build running slower. New with the port.
//!
//! A slot is a lock file, `<state>/slots/<kind>/<n>.lock`, held with an
//! exclusive `flock` by the process that runs the command (`switchboard
//! slot`, or a `cargo`, `npm` or `pnpm` stand-in, src/cmd/slot.rs) for the
//! command's life, so a command that crashes or is killed lets its slot go
//! with its process: no server, Claude or tokens take part, and nothing is
//! left to clean. The lock is never handed to the command (Rust opens files
//! close-on-exec), since a daemon a build starts (sccache's server) would
//! hold it on for as long as it lives.
//!
//! Waiters go in order of arrival: each writes a ticket in `queue/`, named
//! by the time it came and its pid and held locked as long as it waits, and
//! takes a free slot only while fewer tickets older than its own are live
//! than slots are free. A ticket whose lock can be taken belongs to a
//! waiter that is gone, and is removed. Waiting checks again every half
//! second.
//!
//! The server is told as a stand-in in a pane starts waiting, takes its slot
//! and lets it go (`switchboard slot-report`, at most a second, never
//! failing the command), and keeps what it was told (`Board`), so the
//! sidebar, the cards and the Resources view show who holds each slot and
//! who waits without reading the lock files: a waiting tab's mark is ◌ and
//! its line "waits for a build slot, 2nd" (UX 2.0 board 79), and Resources
//! lists each kind's slots (board 80). A report whose process is gone is
//! dropped on the server's tick.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub mod serve;

use crate::mux::PaneId;
use crate::paths;

/// What a slot is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    /// Compiling: `cargo build`, `check`, `clippy`, `doc`, the compile half
    /// of `cargo test`.
    Build,
    /// Running test binaries: the run half of `cargo test`, `cargo nextest
    /// run`.
    Tests,
    /// `npm ci`, `npm install`, `npm run build`, `pnpm install`, `pnpm
    /// build`.
    Js,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Build, Kind::Tests, Kind::Js];

    /// Its name, as `switchboard slot` takes it and its folder is named.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Build => "build",
            Kind::Tests => "tests",
            Kind::Js => "js",
        }
    }

    pub fn from_name(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == name)
    }

    /// The setting holding its cap.
    pub fn setting(self) -> &'static str {
        match self {
            Kind::Build => "slots_build",
            Kind::Tests => "slots_tests",
            Kind::Js => "slots_js",
        }
    }

    /// What a waiter is waiting for, in "Waiting for a <words>".
    pub fn words(self) -> &'static str {
        match self {
            Kind::Build => "build slot",
            Kind::Tests => "test slot",
            Kind::Js => "package build slot",
        }
    }

    /// Its column's heading in the Resources view.
    pub fn heading(self) -> &'static str {
        match self {
            Kind::Build => "Builds",
            Kind::Tests => "Test runs",
            Kind::Js => "Package builds",
        }
    }
}

/// The switch over the whole queue.
pub const SETTING: &str = "slot_queue";

/// The environment variable a command holding a slot runs with, naming the
/// kind: a command it starts that would take a slot runs at once, so cargo
/// run from a build script, or an npm script running cargo, can't wait on
/// the slot its own parent holds.
pub const HELD: &str = "SWITCHBOARD_SLOT";

/// How often a waiter looks again.
const POLL: Duration = Duration::from_millis(500);

/// The cores this computer has.
pub fn cores() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// The cap a setting's value gives: a number as written, `auto` (the
/// build's default) a quarter of the cores and at least one. 0 is off.
pub fn cap_of(value: Option<&serde_json::Value>, cores: usize) -> usize {
    let auto = (cores / 4).max(1);
    match value.and_then(|v| v.as_str()) {
        Some("auto") | None => auto,
        Some(text) => text.parse().unwrap_or(auto),
    }
}

/// The kind's cap now, from Settings; 0 when the queue is off.
pub fn cap(kind: Kind) -> usize {
    if !crate::settings::is_on(SETTING) {
        return 0;
    }
    cap_of(crate::settings::cached(kind.setting()).as_ref(), cores())
}

/// The jobs each holder of one of `cap` slots gets: the cores shared out.
pub fn share(cap: usize, cores: usize) -> usize {
    (cores / cap.max(1)).max(1)
}

/// The folder of a kind's slots.
pub fn dir(kind: Kind) -> PathBuf {
    paths::state_dir().join("slots").join(kind.name())
}

/// A slot taken: held until this is dropped, or the process ends.
#[derive(Debug)]
pub struct Held {
    _lock: File,
    pub index: usize,
    /// The cap when it was taken.
    pub cap: usize,
}

/// A place in line, kept while waiting.
struct Ticket {
    path: PathBuf,
    _lock: File,
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn open(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))
}

/// Locks `path` when no one holds it: the file, still locked, or `None`.
fn try_hold(path: &Path) -> Result<Option<File>> {
    let file = open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => {
            Err(e).with_context(|| format!("locking {}", path.display()))
        }
    }
}

fn take_ticket(queue: &Path) -> Result<Ticket> {
    fs::create_dir_all(queue).with_context(|| format!("making {}", queue.display()))?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = queue.join(format!("{nanos:024}-{:010}", std::process::id()));
    let lock = open(&path)?;
    lock.lock()
        .with_context(|| format!("locking {}", path.display()))?;
    Ok(Ticket { path, _lock: lock })
}

/// How many live waiters came before `mine`; a gone waiter's ticket is
/// removed on the way.
fn ahead_of(queue: &Path, mine: &Path) -> usize {
    let Some(my_name) = mine.file_name() else {
        return 0;
    };
    let Ok(entries) = fs::read_dir(queue) else {
        return 0;
    };
    let mut ahead = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.as_os_str() >= my_name {
            continue;
        }
        let path = entry.path();
        match try_hold(&path) {
            Ok(Some(_gone)) => {
                let _ = fs::remove_file(&path);
            }
            Ok(None) => ahead += 1,
            Err(_) => {}
        }
    }
    ahead
}

/// How many slots numbered `cap` or more are still held: taken while the
/// cap was higher, they count against it until their commands end.
fn held_above(slots: &Path, cap: usize) -> usize {
    let Ok(entries) = fs::read_dir(slots) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let index: usize = name.to_str()?.strip_suffix(".lock")?.parse().ok()?;
            (index >= cap).then(|| e.path())
        })
        .filter(|path| matches!(try_hold(path), Ok(None)))
        .count()
}

/// One look: a free slot, when `ahead` waiters before this one, and the
/// holders above a cap lowered since they took theirs, leave one over for
/// it.
fn try_take(slots: &Path, cap: usize, ahead: usize) -> Result<Option<Held>> {
    let ahead = ahead + held_above(slots, cap);
    let mut free = Vec::new();
    for index in 0..cap {
        if let Some(lock) = try_hold(&slots.join(format!("{index}.lock")))? {
            free.push(Held {
                _lock: lock,
                index,
                cap,
            });
        }
    }
    if free.len() > ahead {
        Ok(free.into_iter().next())
    } else {
        Ok(None)
    }
}

/// Takes one of the slots in `slots`, as many as `cap` says at each look,
/// waiting in line for one; `waiting` is called once, with the place in
/// line (1 for next), when none is free at first. `None` once `cap` says 0
/// (the queue turned off for the kind while it waited): the command runs
/// with no slot.
pub fn take_in(
    slots: &Path,
    mut cap: impl FnMut() -> usize,
    mut waiting: impl FnMut(usize),
) -> Result<Option<Held>> {
    fs::create_dir_all(slots).with_context(|| format!("making {}", slots.display()))?;
    let queue = slots.join("queue");
    let ticket = take_ticket(&queue)?;
    let mut said = false;
    loop {
        let cap = cap();
        if cap == 0 {
            return Ok(None);
        }
        let ahead = ahead_of(&queue, &ticket.path);
        if let Some(mut held) = try_take(slots, cap, ahead)? {
            held.cap = cap;
            return Ok(Some(held));
        }
        if !said {
            waiting(ahead + 1);
            said = true;
        }
        std::thread::sleep(POLL);
    }
}

/// Takes one of the kind's slots, as `take_in` does.
pub fn take(
    kind: Kind,
    cap: impl FnMut() -> usize,
    waiting: impl FnMut(usize),
) -> Result<Option<Held>> {
    take_in(&dir(kind), cap, waiting)
}

/// The kind's cap from Settings as the file is now, for a waiter to look
/// again: a change made while it waits applies to it.
pub fn cap_now(kind: Kind) -> usize {
    crate::settings::reload_settings();
    cap(kind)
}

/// "1st", "2nd", "3rd", "11th".
pub fn ordinal(n: usize) -> String {
    crate::sidebar::text::ordinal(u32::try_from(n).unwrap_or(u32::MAX))
}

/// Where a command stands with its slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Hold {
    Waiting,
    /// Holding slot `index` (from 0) of `cap`.
    Holding {
        index: usize,
        cap: usize,
    },
}

/// A command in a pane as it told the server: its process, pane, kind,
/// where it stands, since when (seconds) and its words (`cargo build
/// --release`). `came` orders the waiters (milliseconds).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Use {
    pub pid: u32,
    /// The command's own process while it holds its slot (the stand-in's
    /// child), which stopping it ends.
    pub child: Option<u32>,
    pub pane: PaneId,
    pub kind: Kind,
    pub hold: Hold,
    pub since: i64,
    pub came: i64,
    pub command: String,
}

/// A tab's slot as its row and card show it: the one it waits for, else
/// one it holds, with its place in line while it waits.
#[derive(Clone, Debug, PartialEq)]
pub struct TabSlot {
    pub kind: Kind,
    pub hold: Hold,
    /// Its place in line, from 1, while it waits.
    pub place: usize,
    pub since: i64,
    pub command: String,
}

impl TabSlot {
    pub fn waiting(&self) -> bool {
        self.hold == Hold::Waiting
    }

    /// The sidebar's line while it waits: "waits for a build slot, 2nd".
    pub fn line(&self) -> String {
        format!("waits for a {}, {}", self.kind.words(), ordinal(self.place))
    }

    /// The card's chip: "waits for a build slot · 2nd", or the slot held,
    /// "test slot 1 of 2".
    pub fn chip(&self) -> String {
        match self.hold {
            Hold::Waiting => format!(
                "waits for a {} · {}",
                self.kind.words(),
                ordinal(self.place)
            ),
            Hold::Holding { index, cap } => {
                format!("{} {} of {cap}", self.kind.words(), index + 1)
            }
        }
    }

    /// What the Resources table says the tab runs while it waits: "cargo
    /// build, waiting for a build slot".
    pub fn waiting_words(&self) -> String {
        format!(
            "{}, waiting for a {}",
            program_words(&self.command),
            self.kind.words()
        )
    }
}

/// A command's program and subcommand: "cargo build" of "cargo build
/// --release".
pub fn program_words(command: &str) -> String {
    command
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every process under `root` in `list` (pid, parent), the nearest first:
/// what stopping a command stops with it.
pub fn descendants(list: &[(u32, u32)], root: u32) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    let mut i = 0;
    let mut parents = vec![root];
    while i < parents.len() {
        let parent = parents[i];
        for &(pid, ppid) in list {
            if ppid == parent && pid != root && !out.contains(&pid) {
                out.push(pid);
                parents.push(pid);
            }
        }
        i += 1;
    }
    out
}

/// One kind's slots as the Resources view lists them: the cap, those
/// holding one in slot order, and those waiting in the order they came,
/// each with its place.
#[derive(Clone, Debug, PartialEq)]
pub struct KindNow {
    pub kind: Kind,
    pub cap: usize,
    pub holders: Vec<Use>,
    pub waiters: Vec<(usize, Use)>,
}

/// What the stand-ins in the panes told the server, handed over in an
/// upgrade (src/mux/upgrade.rs), since a stand-in tells it again only as
/// its command ends.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Board {
    #[serde(default)]
    uses: Vec<Use>,
}

impl Board {
    /// A command's report, in place of its last.
    pub fn report(&mut self, u: Use) {
        self.uses.retain(|o| o.pid != u.pid);
        self.uses.push(u);
    }

    /// A command's slot let go; whether it had one.
    pub fn end(&mut self, pid: u32) -> bool {
        let before = self.uses.len();
        self.uses.retain(|o| o.pid != pid);
        self.uses.len() != before
    }

    /// Keeps the reports whose process `alive` says still runs; whether any
    /// went.
    pub fn retain(&mut self, alive: impl Fn(u32) -> bool) -> bool {
        let before = self.uses.len();
        self.uses.retain(|o| alive(o.pid));
        self.uses.len() != before
    }

    /// Keeps the reports of panes `here` says are still there.
    pub fn keep_panes(&mut self, here: impl Fn(PaneId) -> bool) {
        self.uses.retain(|u| here(u.pane));
    }

    pub fn is_empty(&self) -> bool {
        self.uses.is_empty()
    }

    /// The report a process made, if any.
    pub fn get(&self, pid: u32) -> Option<&Use> {
        self.uses.iter().find(|u| u.pid == pid)
    }

    fn waiters(&self, kind: Kind) -> Vec<&Use> {
        let mut w: Vec<&Use> = self
            .uses
            .iter()
            .filter(|u| u.kind == kind && u.hold == Hold::Waiting)
            .collect();
        w.sort_by_key(|u| (u.came, u.pid));
        w
    }

    /// The slot of a tab with these panes: one waited for (the first in
    /// line), else one held.
    pub fn of_tab(&self, panes: &[PaneId]) -> Option<TabSlot> {
        let waiting = Kind::ALL.into_iter().find_map(|kind| {
            self.waiters(kind)
                .into_iter()
                .enumerate()
                .find(|(_, u)| panes.contains(&u.pane))
                .map(|(i, u)| (i + 1, u))
        });
        let (place, u) = waiting.or_else(|| {
            self.uses
                .iter()
                .find(|u| panes.contains(&u.pane) && u.hold != Hold::Waiting)
                .map(|u| (0, u))
        })?;
        Some(TabSlot {
            kind: u.kind,
            hold: u.hold,
            place,
            since: u.since,
            command: u.command.clone(),
        })
    }

    /// Each kind's slots now, with its cap from `cap`.
    pub fn kinds(&self, cap: impl Fn(Kind) -> usize) -> Vec<KindNow> {
        Kind::ALL
            .into_iter()
            .map(|kind| {
                let mut holders: Vec<Use> = self
                    .uses
                    .iter()
                    .filter(|u| u.kind == kind && u.hold != Hold::Waiting)
                    .cloned()
                    .collect();
                holders.sort_by_key(|u| match u.hold {
                    Hold::Holding { index, .. } => index,
                    Hold::Waiting => usize::MAX,
                });
                let waiters = self
                    .waiters(kind)
                    .into_iter()
                    .enumerate()
                    .map(|(i, u)| (i + 1, u.clone()))
                    .collect();
                KindNow {
                    kind,
                    cap: cap(kind),
                    holders,
                    waiters,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Instant;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sb-slots-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn with_a_cap_of_two_a_third_taker_waits_until_one_lets_go() {
        let d = scratch("two");
        let a = take_in(&d, || 2, |_| panic!("first waits"))
            .unwrap()
            .unwrap();
        let _b = take_in(&d, || 2, |_| panic!("second waits"))
            .unwrap()
            .unwrap();
        let (tx, rx) = mpsc::channel();
        let path = d.clone();
        let third = std::thread::spawn(move || {
            take_in(&path, || 2, |place| tx.send(place).unwrap())
                .unwrap()
                .unwrap()
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), 1);
        assert!(!third.is_finished());
        let freed = a.index;
        drop(a);
        let c = third.join().unwrap();
        assert_eq!(c.index, freed);
    }

    #[test]
    fn a_waiter_follows_its_cap_as_it_changes_and_goes_on_without_a_slot_once_it_is_off() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let d = scratch("cap");
        let _held = take_in(&d, || 1, |_| {}).unwrap().unwrap();
        let cap = Arc::new(AtomicUsize::new(1));
        let (path, seen) = (d.clone(), cap.clone());
        let (tx, rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            take_in(
                &path,
                || seen.load(Ordering::SeqCst),
                |place| tx.send(place).unwrap(),
            )
            .unwrap()
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), 1);
        // Raised: the waiter takes the new slot.
        cap.store(2, Ordering::SeqCst);
        let got = waiter.join().unwrap().unwrap();
        assert_eq!((got.index, got.cap), (1, 2));
        // Off: a waiter goes on with none.
        let (path, seen) = (d.clone(), cap.clone());
        let (tx, rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            take_in(
                &path,
                || seen.load(Ordering::SeqCst),
                |place| tx.send(place).unwrap(),
            )
            .unwrap()
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), 1);
        cap.store(0, Ordering::SeqCst);
        assert!(waiter.join().unwrap().is_none());
    }

    #[test]
    fn a_commands_descendants_are_everything_under_it() {
        let list = [(10, 1), (11, 10), (12, 11), (13, 1), (14, 12)];
        assert_eq!(descendants(&list, 10), [11, 12, 14]);
        assert!(descendants(&list, 13).is_empty());
    }

    #[test]
    fn a_holder_above_a_lowered_cap_still_counts_against_it() {
        let d = scratch("lowered");
        let low = take_in(&d, || 2, |_| {}).unwrap().unwrap();
        let high = take_in(&d, || 2, |_| {}).unwrap().unwrap();
        assert_eq!((low.index, high.index), (0, 1));
        drop(low);
        // Lowered to one: slot 1 still runs, so slot 0 isn't taken.
        assert!(try_take(&d, 1, 0).unwrap().is_none());
        drop(high);
        // Another test's process starting holds a lock it inherited until
        // it runs its program, so the slot comes free within a moment.
        let deadline = Instant::now() + Duration::from_secs(5);
        let held = loop {
            if let Some(held) = try_take(&d, 1, 0).unwrap() {
                break held;
            }
            assert!(Instant::now() < deadline, "slot 0 never came free");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(held.index, 0);
    }

    #[test]
    fn a_killed_holders_slot_is_free_at_once() {
        let d = scratch("killed");
        let lock = d.join("0.lock");
        // A process of its own holds the slot, as a stand-in would, and is
        // killed: a child forked here locks the file and says so down a
        // pipe, using only calls safe between fork and exec.
        let path = std::ffi::CString::new(lock.as_os_str().as_encoded_bytes()).unwrap();
        let mut pipe = [0; 2];
        // SAFETY: two descriptors for the pipe's ends.
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        // SAFETY: the child runs only open, flock, write and pause, each safe
        // after a fork in a process with threads, and never returns.
        let child = unsafe { libc::fork() };
        assert!(child >= 0);
        if child == 0 {
            // SAFETY: as above, in the child alone.
            unsafe {
                let fd = libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CREAT, 0o600);
                if fd >= 0 && libc::flock(fd, libc::LOCK_EX) == 0 {
                    libc::write(pipe[1], b"h".as_ptr().cast(), 1);
                }
                loop {
                    libc::pause();
                }
            }
        }
        let mut said = [0u8; 1];
        // SAFETY: reading one byte into a buffer of one, then closing both
        // ends this process holds.
        unsafe {
            libc::close(pipe[1]);
            assert_eq!(libc::read(pipe[0], said.as_mut_ptr().cast(), 1), 1);
            libc::close(pipe[0]);
        }
        assert_eq!(&said, b"h");
        assert!(try_take(&d, 1, 0).unwrap().is_none());
        // SAFETY: the child forked above, killed and reaped.
        unsafe {
            libc::kill(child, libc::SIGKILL);
            libc::waitpid(child, std::ptr::null_mut(), 0);
        }
        let start = Instant::now();
        let held = take_in(&d, || 1, |_| panic!("waited for a dead holder"))
            .unwrap()
            .unwrap();
        assert_eq!(held.index, 0);
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn waiters_go_in_the_order_they_came() {
        let d = scratch("order");
        let first = take_in(&d, || 1, |_| {}).unwrap().unwrap();
        let (done, order) = mpsc::channel();
        let mut waiters = Vec::new();
        for n in 0..3 {
            let path = d.clone();
            let done = done.clone();
            let (placed, wait_placed) = mpsc::channel();
            waiters.push(std::thread::spawn(move || {
                let held = take_in(&path, || 1, |place| placed.send(place).unwrap())
                    .unwrap()
                    .unwrap();
                done.send(n).unwrap();
                // Long enough for the next in line to find the slot taken.
                std::thread::sleep(Duration::from_millis(50));
                drop(held);
            }));
            // Each in line before the next comes.
            assert_eq!(
                wait_placed.recv_timeout(Duration::from_secs(10)).unwrap(),
                n + 1
            );
        }
        drop(first);
        let got: Vec<usize> = (0..3)
            .map(|_| order.recv_timeout(Duration::from_secs(10)).unwrap())
            .collect();
        assert_eq!(got, vec![0, 1, 2]);
        for w in waiters {
            w.join().unwrap();
        }
    }

    #[test]
    fn a_gone_waiters_ticket_holds_no_one_up() {
        let d = scratch("gone");
        let queue = d.join("queue");
        fs::create_dir_all(&queue).unwrap();
        // An older ticket nobody holds: its waiter is gone.
        fs::write(queue.join(format!("{:024}-{:010}", 1, 1)), "").unwrap();
        let held = take_in(&d, || 1, |_| panic!("waited behind a gone waiter"))
            .unwrap()
            .unwrap();
        assert_eq!(held.index, 0);
        assert_eq!(fs::read_dir(&queue).unwrap().count(), 0);
    }

    #[test]
    fn caps_read_from_settings_values() {
        let v = |s: &str| serde_json::Value::String(s.into());
        assert_eq!(cap_of(Some(&v("auto")), 14), 3);
        assert_eq!(cap_of(Some(&v("auto")), 2), 1);
        assert_eq!(cap_of(None, 16), 4);
        assert_eq!(cap_of(Some(&v("0")), 16), 0);
        assert_eq!(cap_of(Some(&v("2")), 16), 2);
        assert_eq!(share(3, 14), 4);
        assert_eq!(share(0, 14), 14);
        assert_eq!(share(20, 14), 1);
    }

    fn report(pid: u32, pane: u32, kind: Kind, hold: Hold, came: i64) -> Use {
        Use {
            pid,
            child: None,
            pane: PaneId(pane),
            kind,
            hold,
            since: came,
            came,
            command: "cargo build --release".into(),
        }
    }

    #[test]
    fn the_board_puts_waiters_in_the_order_they_came_and_a_tab_waiting_first() {
        let mut board = Board::default();
        let held = |index, cap| Hold::Holding { index, cap };
        board.report(report(1, 10, Kind::Build, held(0, 1), 5));
        board.report(report(3, 30, Kind::Build, Hold::Waiting, 9));
        board.report(report(2, 20, Kind::Build, Hold::Waiting, 7));
        board.report(report(4, 30, Kind::Tests, held(1, 2), 8));
        let tab = board.of_tab(&[PaneId(30)]).unwrap();
        assert_eq!(tab.line(), "waits for a build slot, 2nd");
        assert_eq!(tab.chip(), "waits for a build slot · 2nd");
        assert_eq!(tab.waiting_words(), "cargo build, waiting for a build slot");
        let holder = board.of_tab(&[PaneId(10)]).unwrap();
        assert!(!holder.waiting());
        assert_eq!(holder.chip(), "build slot 1 of 1");
        let kinds = board.kinds(|k| if k == Kind::Build { 1 } else { 2 });
        let pids = |us: &[Use]| us.iter().map(|u| u.pid).collect::<Vec<_>>();
        assert_eq!(pids(&kinds[0].holders), [1]);
        let waiting: Vec<(usize, u32)> =
            kinds[0].waiters.iter().map(|(p, u)| (*p, u.pid)).collect();
        assert_eq!(waiting, [(1, 2), (2, 3)]);
        assert_eq!(pids(&kinds[1].holders), [4]);
        // A waiter taking its slot reports again in place of its wait.
        board.report(report(2, 20, Kind::Build, held(0, 1), 7));
        assert_eq!(board.of_tab(&[PaneId(30)]).unwrap().place, 1);
        assert!(board.end(1));
        assert!(board.retain(|pid| pid != 4));
        assert_eq!(board.of_tab(&[PaneId(10)]), None);
    }

    #[test]
    fn places_read_as_ordinals() {
        let got: Vec<String> = [1, 2, 3, 4, 11, 12, 13, 21, 22, 103]
            .into_iter()
            .map(ordinal)
            .collect();
        assert_eq!(
            got,
            [
                "1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "103rd"
            ]
        );
    }
}
