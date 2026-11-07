//! A pane's terminal as the server holds it: the PTY's master side, the
//! program running on it, and two threads, one reading what the program
//! writes and one writing what is typed to it.
//!
//! portable-pty opens the PTY and starts the program (in a session of its
//! own, the PTY its controlling terminal); from then on the master is a
//! descriptor of the server's own and the program a pid, read, written,
//! resized (TIOCSWINSZ), asked which group is in front (tcgetpgrp) and
//! waited on (waitpid) with libc alone. So a pane can be taken up from a
//! descriptor and a pid as well as started: what a server hands over as it
//! replaces itself with a new build (src/mux/upgrade.rs), whose programs
//! are still the process's children, since exec keeps the pid.
//!
//! What is written to the program goes through the writer thread rather
//! than straight to the PTY: a program that stops reading its input (a long
//! build, a stopped process) fills the terminal's input buffer, and a write
//! then waits until it reads again. tmux never waits on a pane, and neither
//! may the server, so its writes are queued and the thread waits instead.
//!
//! Both threads wait on the master with `select` (macOS's `poll` doesn't
//! work on a terminal, as tmux found), together with a pipe that pausing
//! writes to, so either can be stopped between two reads or two writes.
//! Paused, nothing more is read from the program or written to it: what
//! waits to be written is kept, and whatever the program writes meanwhile
//! stays in the terminal for whoever reads it next, this server once it
//! resumes or the build it hands the descriptor to.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tokio::sync::mpsc::UnboundedSender;

use super::{PaneEvent, PaneId};

/// Bytes read from the PTY at a time.
const READ_SIZE: usize = 64 * 1024;

/// The readers still running, in a pane or let go of: each ends once its
/// program has been reaped.
static READING: AtomicUsize = AtomicUsize::new(0);

/// How many programs this process still waits on, its panes' and those of
/// panes closed but not yet ended: a server ending waits a moment for them
/// (src/mux/server.rs).
pub fn programs_running() -> usize {
    READING.load(Ordering::SeqCst)
}

/// Opens a PTY of `size` and starts `cmd` on it, giving the master's
/// descriptor (closed on exec, so no other program the server starts holds
/// it) and the program's pid. The program's end of the terminal is closed
/// here once it has its own, so reading the master ends once the program
/// and whatever it started have closed theirs.
pub fn open(cmd: CommandBuilder, size: PtySize) -> Result<(OwnedFd, Option<u32>)> {
    let pair = native_pty_system()
        .openpty(size)
        .context("opening a terminal for the pane")?;
    let child = pair
        .slave
        .spawn_command(cmd)
        .context("starting the pane's program")?;
    drop(pair.slave);
    let pid = child.process_id();
    let master = pair
        .master
        .as_raw_fd()
        .context("the terminal has no descriptor")
        .and_then(|fd| duplicate(fd).context("keeping the terminal's descriptor"));
    let master = match master {
        Ok(master) => master,
        Err(e) => {
            if let Some(pid) = pid.and_then(|p| libc::pid_t::try_from(p).ok()) {
                // SAFETY: a signal to the child just started, not yet reaped.
                unsafe { libc::kill(pid, libc::SIGHUP) };
            }
            return Err(e);
        }
    };
    // portable-pty's own copy of the master is closed; its writer was never
    // taken, so nothing writes an end-of-file to the program as it goes. The
    // child's handle waits on nothing as it goes: the reader waits by pid.
    drop(pair.master);
    drop(child);
    Ok((master, pid))
}

/// A descriptor's copy, closed on exec.
fn duplicate(fd: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: F_DUPFD_CLOEXEC gives a new descriptor, owned from here on.
    let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if copy < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: just made, and owned by nothing else.
    Ok(unsafe { OwnedFd::from_raw_fd(copy) })
}

/// Sets or clears a descriptor's close-on-exec flag: cleared on the ones
/// handed to a new build, set again on them once it has them (and on them
/// all again if the exec fails), so no other program inherits them.
pub fn set_cloexec(fd: RawFd, on: bool) -> io::Result<()> {
    // SAFETY: reads and sets the flags of a descriptor the caller holds.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let flags = if on {
        flags | libc::FD_CLOEXEC
    } else {
        flags & !libc::FD_CLOEXEC
    };
    // SAFETY: as above.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Makes a descriptor's reads and writes return at once rather than wait.
fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: reads and sets the status flags of a descriptor held here.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Whether a descriptor is open in this process.
pub fn is_open(fd: RawFd) -> bool {
    // SAFETY: F_GETFD only reads; a descriptor that isn't open is EBADF.
    fd >= 0 && unsafe { libc::fcntl(fd, libc::F_GETFD) } >= 0
}

/// A pipe both of whose ends are closed on exec and never wait.
fn wake_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let (read, write) = io::pipe()?;
    let (read, write) = (OwnedFd::from(read), OwnedFd::from(write));
    set_nonblocking(read.as_raw_fd())?;
    set_nonblocking(write.as_raw_fd())?;
    Ok((read, write))
}

/// A pane's terminal: its master, its program, and its two threads.
pub struct Pty {
    shared: Arc<Shared>,
    pid: Option<u32>,
}

/// What the pane and its threads share.
struct Shared {
    master: OwnedFd,
    /// Written to when the pane is paused, so a thread waiting on the
    /// master wakes; emptied when it resumes.
    wake_read: OwnedFd,
    wake_write: OwnedFd,
    state: Mutex<State>,
    turn: Condvar,
    /// Set once the reader has reaped the program, after which its pid may
    /// be another process's and is never signalled.
    reaped: AtomicBool,
}

#[derive(Default)]
struct State {
    /// What waits to be written to the program, in order.
    input: Vec<u8>,
    paused: bool,
    /// The pane let go of: the writer ends (the reader goes on to the end,
    /// so the program never blocks on a full terminal).
    closed: bool,
    /// Each thread waiting for the pause to end.
    reader_parked: bool,
    writer_parked: bool,
    /// The reader has read the terminal's end: the program and everything
    /// it started have closed it, and it reads no more.
    hung_up: bool,
    /// The writer has ended, the terminal no longer taking writes.
    writer_gone: bool,
}

/// The lock, whether or not a thread panicked holding it: what it guards
/// is only ever changed whole.
fn lock(mutex: &Mutex<State>) -> MutexGuard<'_, State> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What a wait on the master came to.
#[derive(Debug, PartialEq, Eq)]
enum Woke {
    /// The master can be read (or written).
    Ready,
    /// The pane was paused.
    Paused,
    /// The wait itself failed.
    Failed,
}

/// Waits until `fd` can be read (or written, with `write`) or `wake` can be
/// read, with `select`, which waits on a terminal on every Unix; `poll` for
/// a descriptor past what `select` can name.
fn wait(fd: RawFd, write: bool, wake: RawFd) -> Woke {
    let limit = libc::FD_SETSIZE as RawFd;
    loop {
        let (ready, woken) = if fd < limit && wake < limit {
            // SAFETY: the sets are zeroed and filled with descriptors under
            // FD_SETSIZE before select reads them.
            unsafe {
                let mut reads: libc::fd_set = std::mem::zeroed();
                let mut writes: libc::fd_set = std::mem::zeroed();
                libc::FD_ZERO(&mut reads);
                libc::FD_ZERO(&mut writes);
                libc::FD_SET(wake, &mut reads);
                if write {
                    libc::FD_SET(fd, &mut writes);
                } else {
                    libc::FD_SET(fd, &mut reads);
                }
                let n = libc::select(
                    fd.max(wake) + 1,
                    &mut reads,
                    &mut writes,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                if n < 0 {
                    (None, false)
                } else {
                    let ready = if write {
                        libc::FD_ISSET(fd, &writes)
                    } else {
                        libc::FD_ISSET(fd, &reads)
                    };
                    (Some(ready), libc::FD_ISSET(wake, &reads))
                }
            }
        } else {
            let mut fds = [
                libc::pollfd {
                    fd,
                    events: if write { libc::POLLOUT } else { libc::POLLIN },
                    revents: 0,
                },
                libc::pollfd {
                    fd: wake,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            // SAFETY: polls the two pollfds given.
            let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
            if n < 0 {
                (None, false)
            } else {
                (Some(fds[0].revents != 0), fds[1].revents != 0)
            }
        };
        match ready {
            None if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted => continue,
            None => return Woke::Failed,
            Some(_) if woken => return Woke::Paused,
            Some(true) => return Woke::Ready,
            Some(false) => continue,
        }
    }
}

impl Shared {
    /// Waits out a pause, if the pane is paused: the thread says it has
    /// stopped, so the server knows nothing more will be read or written.
    fn park(&self, reader: bool) {
        let mut st = lock(&self.state);
        if !st.paused {
            return;
        }
        fn flag(st: &mut State, reader: bool) -> &mut bool {
            if reader {
                &mut st.reader_parked
            } else {
                &mut st.writer_parked
            }
        }
        *flag(&mut st, reader) = true;
        self.turn.notify_all();
        while st.paused && (reader || !st.closed) {
            st = self
                .turn
                .wait(st)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        *flag(&mut st, reader) = false;
    }
}

impl Pty {
    /// Takes up a terminal: `master` (made non-blocking and closed on exec
    /// here), the program on it (`pid`), and what waits to be written to it;
    /// `hung_up` when the program had already closed it, so it is only
    /// waited on. Starts the reader, which sends what the program writes
    /// and its end to `events` as pane `id`'s program `generation`, and the
    /// writer. Nothing is done to the program if this fails.
    pub fn start(
        master: OwnedFd,
        pid: Option<u32>,
        input: Vec<u8>,
        hung_up: bool,
        id: PaneId,
        generation: u64,
        events: UnboundedSender<PaneEvent>,
    ) -> io::Result<Pty> {
        set_cloexec(master.as_raw_fd(), true)?;
        set_nonblocking(master.as_raw_fd())?;
        let (wake_read, wake_write) = wake_pipe()?;
        let shared = Arc::new(Shared {
            master,
            wake_read,
            wake_write,
            state: Mutex::new(State {
                input,
                hung_up,
                ..State::default()
            }),
            turn: Condvar::new(),
            reaped: AtomicBool::new(false),
        });
        let writer = shared.clone();
        std::thread::Builder::new()
            .name(format!("pane {id} writer"))
            .spawn(move || write_loop(&writer))?;
        let reader = shared.clone();
        READING.fetch_add(1, Ordering::SeqCst);
        let started = std::thread::Builder::new()
            .name(format!("pane {id} reader"))
            .spawn(move || {
                read_loop(&reader, id, generation, pid, &events);
                READING.fetch_sub(1, Ordering::SeqCst);
            });
        if let Err(e) = started {
            READING.fetch_sub(1, Ordering::SeqCst);
            // The writer ends as the pane would have let it go.
            lock(&shared.state).closed = true;
            shared.turn.notify_all();
            return Err(e);
        }
        Ok(Pty { shared, pid })
    }

    /// The master's descriptor, for handing it over.
    pub fn fd(&self) -> RawFd {
        self.shared.master.as_raw_fd()
    }

    /// The program's pid.
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Whether the program has been reaped (its pid may be another's now).
    pub fn reaped(&self) -> bool {
        self.shared.reaped.load(Ordering::Acquire)
    }

    /// Whether the program has closed the terminal: it reads no more.
    pub fn hung_up(&self) -> bool {
        lock(&self.shared.state).hung_up
    }

    /// Queues bytes for the program. It fails once the program has been
    /// reaped or the terminal no longer takes writes.
    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let gone = || io::Error::new(io::ErrorKind::BrokenPipe, "the pane's program has ended");
        if self.reaped() {
            return Err(gone());
        }
        let mut st = lock(&self.shared.state);
        if st.writer_gone {
            return Err(gone());
        }
        if bytes.is_empty() {
            return Ok(());
        }
        st.input.extend_from_slice(bytes);
        drop(st);
        self.shared.turn.notify_all();
        Ok(())
    }

    /// What waits to be written to the program.
    pub fn pending_input(&self) -> Vec<u8> {
        lock(&self.shared.state).input.clone()
    }

    /// Gives the terminal its size, which the program learns of by SIGWINCH
    /// when it changed.
    pub fn resize(&self, size: PtySize) -> io::Result<()> {
        let ws = libc::winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: size.pixel_width,
            ws_ypixel: size.pixel_height,
        };
        // SAFETY: TIOCSWINSZ reads the winsize given.
        if unsafe {
            libc::ioctl(
                self.fd(),
                libc::TIOCSWINSZ as _,
                &ws as *const libc::winsize,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// The process group in front on the terminal.
    pub fn foreground_pgid(&self) -> Option<u32> {
        // SAFETY: tcgetpgrp only reads.
        let pgid = unsafe { libc::tcgetpgrp(self.fd()) };
        u32::try_from(pgid).ok().filter(|&p| p > 0)
    }

    /// Hangs the program up, as closing its terminal does: SIGHUP to the
    /// group in front (a program the shell started, in a group of its own)
    /// and to the program's own, which leads its session; the shell passes
    /// it on to its other jobs. Nothing once the program has been reaped.
    pub fn hang_up(&self) {
        if self.reaped() {
            return;
        }
        let pid = self.pid.and_then(|pid| libc::pid_t::try_from(pid).ok());
        if let Some(front) = self.foreground_pgid()
            && let Ok(front) = libc::pid_t::try_from(front)
            && Some(front) != pid
        {
            // SAFETY: a signal to a process group of this terminal.
            unsafe { libc::killpg(front, libc::SIGHUP) };
        }
        if let Some(pid) = pid {
            // SAFETY: a signal to the group of a child not yet reaped, else
            // to the child alone.
            unsafe {
                if libc::killpg(pid, libc::SIGHUP) != 0 {
                    libc::kill(pid, libc::SIGHUP);
                }
            }
        }
    }

    /// Tells the program in front its size again (SIGWINCH), so a
    /// full-screen program draws itself again though its size is the same,
    /// which a resize alone doesn't signal.
    pub fn remind_size(&self) {
        if self.reaped() {
            return;
        }
        if let Some(front) = self
            .foreground_pgid()
            .and_then(|g| libc::pid_t::try_from(g).ok())
        {
            // SAFETY: a signal to the group in front on this terminal.
            unsafe { libc::killpg(front, libc::SIGWINCH) };
        }
    }

    /// Stops reading from the program and writing to it, between two reads
    /// and two writes; `parked` says when both threads have.
    pub fn pause(&self) {
        let mut st = lock(&self.shared.state);
        if st.paused {
            return;
        }
        st.paused = true;
        let byte = *b"p";
        // SAFETY: a write of one byte to the pipe's non-blocking end; the
        // pipe is empty while not paused, so it has room.
        unsafe { libc::write(self.shared.wake_write.as_raw_fd(), byte.as_ptr().cast(), 1) };
        drop(st);
        self.shared.turn.notify_all();
    }

    /// Whether, paused, both threads have stopped: nothing more will be read
    /// from the program or written to it until it resumes.
    pub fn parked(&self) -> bool {
        let st = lock(&self.shared.state);
        st.paused
            && (st.reader_parked || st.hung_up)
            && (st.writer_parked || st.writer_gone || st.closed)
    }

    /// Reads and writes again after a pause.
    pub fn resume(&self) {
        let mut st = lock(&self.shared.state);
        if !st.paused {
            return;
        }
        let mut buf = [0u8; 16];
        // SAFETY: reads from the pipe's non-blocking end until it is empty.
        while unsafe {
            libc::read(
                self.shared.wake_read.as_raw_fd(),
                buf.as_mut_ptr().cast(),
                buf.len(),
            )
        } > 0
        {}
        st.paused = false;
        drop(st);
        self.shared.turn.notify_all();
    }
}

/// The pane let go of: the writer ends; the reader reads on to the end.
impl Drop for Pty {
    fn drop(&mut self) {
        lock(&self.shared.state).closed = true;
        self.shared.turn.notify_all();
    }
}

/// Reads the program's output until the terminal's other end is closed
/// (EIO on Linux, the end of the file on macOS), then waits for the program
/// and says how it ended. Once the server stops listening it keeps reading
/// and dropping what it reads, so the program never blocks on a full
/// terminal.
fn read_loop(
    shared: &Shared,
    id: PaneId,
    generation: u64,
    pid: Option<u32>,
    events: &UnboundedSender<PaneEvent>,
) {
    let fd = shared.master.as_raw_fd();
    let mut buf = vec![0u8; READ_SIZE];
    let mut listening = true;
    if !lock(&shared.state).hung_up {
        loop {
            match wait(fd, false, shared.wake_read.as_raw_fd()) {
                Woke::Paused => {
                    shared.park(true);
                    continue;
                }
                Woke::Failed => break,
                Woke::Ready => {}
            }
            // SAFETY: reads into a buffer that outlives the call.
            let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n > 0 {
                let bytes = buf[..n as usize].to_vec();
                if listening
                    && events
                        .send(PaneEvent::Output(id, generation, bytes))
                        .is_err()
                {
                    listening = false;
                }
                continue;
            }
            if n == 0 {
                break;
            }
            match io::Error::last_os_error().kind() {
                io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock => {}
                _ => break,
            }
        }
        lock(&shared.state).hung_up = true;
        shared.turn.notify_all();
    }
    let code = pid.and_then(wait_for);
    shared.reaped.store(true, Ordering::Release);
    let _ = events.send(PaneEvent::Exited(id, generation, code));
}

/// Waits for a child to end: its exit code, or `None` for a signal (or a
/// pid that isn't a child of this process's, already reaped).
fn wait_for(pid: u32) -> Option<i32> {
    let pid = libc::pid_t::try_from(pid).ok()?;
    let mut status = 0;
    loop {
        // SAFETY: waits on the one child named, filling `status`.
        let got = unsafe { libc::waitpid(pid, &mut status, 0) };
        if got == pid {
            break;
        }
        if got < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return None;
    }
    libc::WIFEXITED(status).then(|| libc::WEXITSTATUS(status))
}

/// Writes what the server queued for the program, in order, until the pane
/// is let go of or the terminal can't be written to. Paused, it puts back
/// what it hadn't written yet, ahead of anything queued since.
fn write_loop(shared: &Shared) {
    let fd = shared.master.as_raw_fd();
    'next: loop {
        let mut chunk = {
            let mut st = lock(&shared.state);
            loop {
                if st.closed {
                    return;
                }
                if st.paused {
                    drop(st);
                    shared.park(false);
                    st = lock(&shared.state);
                    continue;
                }
                if !st.input.is_empty() {
                    break std::mem::take(&mut st.input);
                }
                st = shared
                    .turn
                    .wait(st)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        };
        let mut at = 0;
        while at < chunk.len() {
            match wait(fd, true, shared.wake_read.as_raw_fd()) {
                Woke::Paused => {
                    let mut st = lock(&shared.state);
                    if st.paused {
                        let rest = chunk.split_off(at);
                        st.input.splice(0..0, rest);
                        continue 'next;
                    }
                    continue;
                }
                Woke::Failed => break,
                Woke::Ready => {}
            }
            let rest = &chunk[at..];
            // SAFETY: writes from a slice that outlives the call.
            let n = unsafe { libc::write(fd, rest.as_ptr().cast(), rest.len()) };
            if n > 0 {
                at += n as usize;
                continue;
            }
            if n == 0 {
                break;
            }
            match io::Error::last_os_error().kind() {
                io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock => {}
                _ => break,
            }
        }
        if at < chunk.len() {
            let mut st = lock(&shared.state);
            st.writer_gone = true;
            st.input.clear();
            drop(st);
            shared.turn.notify_all();
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    fn size(cols: u16, rows: u16) -> PtySize {
        PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    fn sh(script: &str) -> CommandBuilder {
        let mut cmd = CommandBuilder::from_argv(vec!["/bin/sh".into(), "-c".into(), script.into()]);
        cmd.env("TERM", "dumb");
        cmd.cwd(std::env::temp_dir());
        cmd
    }

    /// What the reader has sent, as text, until `done` holds of it (or five
    /// seconds pass, which fails the test); gives the exit too, if it came.
    fn read_until(
        rx: &mut UnboundedReceiver<PaneEvent>,
        seen: &mut String,
        done: impl Fn(&str) -> bool,
    ) -> Option<Option<i32>> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match rx.try_recv() {
                Ok(PaneEvent::Output(_, _, bytes)) => {
                    seen.push_str(&String::from_utf8_lossy(&bytes))
                }
                Ok(PaneEvent::Exited(_, _, code)) => return Some(code),
                Err(_) => {
                    if done(seen) {
                        return None;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "timed out; read so far: {seen:?}"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }

    fn until(what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_program_is_read_written_and_waited_for() {
        let (master, pid) = open(sh("read line; echo got:$line; exit 4"), size(40, 5)).unwrap();
        let (tx, mut rx) = unbounded_channel();
        let pty = Pty::start(master, pid, Vec::new(), false, PaneId(1), 1, tx).unwrap();
        pty.write(b"hello\r").unwrap();
        let mut seen = String::new();
        let mut end = read_until(&mut rx, &mut seen, |s| s.contains("got:hello"));
        while end.is_none() {
            end = read_until(&mut rx, &mut seen, |_| false);
        }
        assert_eq!(end, Some(Some(4)), "{seen:?}");
        assert!(pty.reaped());
        assert!(pty.write(b"x").is_err(), "nothing is written after the end");
    }

    #[test]
    fn paused_nothing_is_read_or_written_and_nothing_is_lost() {
        let (master, pid) = open(
            sh("stty -echo; while read line; do echo \"got:$line\"; done"),
            size(40, 5),
        )
        .unwrap();
        let (tx, mut rx) = unbounded_channel();
        let pty = Pty::start(master, pid, Vec::new(), false, PaneId(2), 1, tx).unwrap();
        let mut seen = String::new();
        pty.write(b"one\r").unwrap();
        read_until(&mut rx, &mut seen, |s| s.contains("got:one"));

        pty.pause();
        until("both threads to stop", || pty.parked());
        pty.write(b"two\r").unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(rx.try_recv().is_err(), "nothing is read while paused");
        assert_eq!(pty.pending_input(), b"two\r", "nor written");

        pty.resume();
        read_until(&mut rx, &mut seen, |s| s.contains("got:two"));
        assert!(!pty.parked());
        pty.hang_up();
        while read_until(&mut rx, &mut seen, |_| false).is_none() {}
    }

    #[test]
    fn a_terminal_handed_on_is_taken_up_where_it_was() {
        let (master, pid) = open(
            sh("stty -echo; while read line; do echo \"got:$line\"; done"),
            size(40, 5),
        )
        .unwrap();
        let (tx, mut rx) = unbounded_channel();
        let first = Pty::start(master, pid, Vec::new(), false, PaneId(3), 1, tx).unwrap();
        let mut seen = String::new();
        first.write(b"before\r").unwrap();
        read_until(&mut rx, &mut seen, |s| s.contains("got:before"));

        // Paused, as a server is before it hands its panes over, with
        // something still to type; the program writes on meanwhile.
        first.pause();
        until("both threads to stop", || first.parked());
        first.write(b"queued\r").unwrap();
        let handed = duplicate(first.fd()).unwrap();
        let input = first.pending_input();

        let (tx, mut rx) = unbounded_channel();
        let second = Pty::start(handed, pid, input, false, PaneId(3), 2, tx).unwrap();
        let mut seen = String::new();
        read_until(&mut rx, &mut seen, |s| s.contains("got:queued"));
        assert_eq!(second.foreground_pgid(), pid, "the same program in front");
        second.write(b"after\r").unwrap();
        read_until(&mut rx, &mut seen, |s| s.contains("got:after"));

        // The first lets go without hanging anything up; the second ends it.
        drop(first);
        second.hang_up();
        let mut end = None;
        while end.is_none() {
            end = read_until(&mut rx, &mut seen, |_| false);
        }
        assert_eq!(end, Some(None), "ended by its hangup");
    }

    #[test]
    fn a_resize_reaches_the_program_and_the_size_is_told_again() {
        let script = "trap 'stty size' WINCH; stty size; while :; do sleep 0.05; done";
        let (master, pid) = open(sh(script), size(40, 5)).unwrap();
        let (tx, mut rx) = unbounded_channel();
        let pty = Pty::start(master, pid, Vec::new(), false, PaneId(4), 1, tx).unwrap();
        let mut seen = String::new();
        read_until(&mut rx, &mut seen, |s| s.contains("5 40"));
        pty.resize(size(50, 7)).unwrap();
        read_until(&mut rx, &mut seen, |s| s.contains("7 50"));
        let count = seen.matches("7 50").count();
        pty.remind_size();
        read_until(&mut rx, &mut seen, |s| s.matches("7 50").count() > count);
        pty.hang_up();
        while read_until(&mut rx, &mut seen, |_| false).is_none() {}
    }

    #[test]
    fn descriptors_are_closed_on_exec_until_handed_over() {
        let (master, pid) = open(sh("sleep 5"), size(20, 3)).unwrap();
        let fd = master.as_raw_fd();
        let cloexec = |fd| unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC != 0;
        assert!(cloexec(fd));
        set_cloexec(fd, false).unwrap();
        assert!(!cloexec(fd));
        let (tx, _rx) = unbounded_channel();
        let pty = Pty::start(master, pid, Vec::new(), false, PaneId(5), 1, tx).unwrap();
        assert!(cloexec(pty.fd()), "taken up, it is closed on exec again");
        assert!(is_open(pty.fd()));
        assert!(!is_open(-1));
        pty.hang_up();
    }
}
