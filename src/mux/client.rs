//! The attached terminal: raw mode, the alternate screen, mouse, focus and
//! paste reporting on, and keys told apart (Ctrl-Shift-P from Ctrl-P) where
//! the terminal can; what is typed goes to the server, what the server
//! draws goes to the terminal; a resize is sent as it happens. However it
//! ends (detached, the server gone, a signal), the terminal is put back as
//! it was.
//!
//! The client knows nothing of keys or screens: the server parses what is
//! typed and draws every cell, as a tmux client only passes bytes along. So
//! it is three threads moving bytes. One reads the terminal and sends each
//! read as `Input`; one reads the server and writes each `Output` to the
//! terminal as it comes; one waits on a pipe that the signal handler writes
//! the signal's number to (a handler may do next to nothing, so the work is
//! done here), sending the new size on SIGWINCH and ending the attachment on
//! a signal that would otherwise kill the client with the terminal still
//! raw. The first thread to finish says why, and the main thread puts the
//! terminal back and reports it.
//!
//! Putting the terminal back happens once, from whichever comes first: the
//! guard's drop on the way out (a panic's unwinding included), or the panic
//! hook, which runs before unwinding so the panic's message lands on the
//! normal screen rather than on the alternate one about to be cleared. The
//! saved settings live in a static so both can reach them, and the screen
//! is written under the same lock, so nothing the server drew arrives after
//! the terminal is restored.

use std::fs::File;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, Once, mpsc};

use anyhow::{Context, Result, bail};

use super::proto::{self, ClientMsg, ServerMsg};
use super::socket;

/// Turned on once the terminal is raw: the alternate screen, mouse presses
/// and drags in SGR form, focus in and out, and bracketed paste.
const ENTER: &[u8] = b"\x1b[?1049h\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?1004h\x1b[?2004h";

/// Everything `ENTER` turned on turned off, and what the server may have
/// turned on through its output (every-motion mouse for hover, a hidden
/// cursor, colors) with it, then back to the normal screen.
const LEAVE: &[u8] = b"\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[?1004l\x1b[?2004l\x1b[?25h\x1b[0m\x1b[?1049l";

/// Asked of the terminal after `ENTER` while the `extended_keys` setting is
/// on, as tmux's `extended-keys on` asked it: xterm's modifyOtherKeys at
/// level 2, and the kitty keyboard protocol's flag that tells keys apart,
/// pushed onto its stack for the alternate screen. A terminal that knows
/// both uses kitty's; one that knows neither ignores them, and the keys
/// come as before. Either way the server reads every form (`input`).
const KEYS_ON: &[u8] = b"\x1b[>4;2m\x1b[>1u";

/// What `KEYS_ON` asked taken back: the flags popped off the alternate
/// screen's stack (so before `LEAVE` leaves it), modifyOtherKeys reset.
const KEYS_OFF: &[u8] = b"\x1b[<u\x1b[>4m";

/// How much is read from the terminal at once.
const READ_SIZE: usize = 4096;

/// The size assumed when the terminal won't say.
const DEFAULT_SIZE: (u16, u16) = (80, 24);

/// The signals the client catches: a resize, and those that would end it
/// with the terminal still raw.
const CAUGHT: [libc::c_int; 5] = [
    libc::SIGWINCH,
    libc::SIGTERM,
    libc::SIGHUP,
    libc::SIGINT,
    libc::SIGQUIT,
];

/// The terminal while it is attached: its descriptor for output and the
/// settings to put back on its input. `None` once it is back, so the screen
/// is no longer written.
struct Attached {
    input: RawFd,
    output: RawFd,
    saved: libc::termios,
    /// Whether `KEYS_ON` was asked, so `KEYS_OFF` takes it back.
    keys: bool,
}

static ATTACHED: Mutex<Option<Attached>> = Mutex::new(None);

/// The pipe's writing end, for the signal handler; -1 when none is set.
static SIGNAL_FD: AtomicI32 = AtomicI32::new(-1);

static PANIC_HOOK: Once = Once::new();

/// Why an attachment ended.
#[derive(Debug, PartialEq, Eq)]
enum End {
    /// The server said to end, with this to say.
    Exit(Option<String>),
    /// The terminal's input ended (or failed).
    StdinClosed,
    /// The terminal couldn't be written to.
    TerminalGone,
    /// The server closed the connection, or sent what isn't the protocol.
    ServerGone,
    /// A signal asked the client to end.
    Signal(libc::c_int),
}

impl End {
    /// Whether the server should be told the client is leaving: it is
    /// gone already in the other cases, or told the client to go.
    fn detaches(&self) -> bool {
        matches!(self, End::StdinClosed | End::TerminalGone | End::Signal(_))
    }
}

/// Attaches this terminal to `session` (the server's default when `None`),
/// starting the server when none runs, until the server says to end.
///
/// Inside a Switchboard pane (`SWITCHBOARD` names a server that answers) it
/// refuses, since a workspace drawn inside its own pane feeds on itself;
/// `SWITCHBOARD_NESTED=1` attaches anyway. A signal that ends it gives the
/// shell's code for one, 128 and its number; the server going away is an
/// error.
pub fn attach(session: Option<String>) -> Result<ExitCode> {
    if !is_tty(libc::STDIN_FILENO) || !is_tty(libc::STDOUT_FILENO) {
        bail!("switchboard needs a terminal");
    }
    if inside_switchboard(
        std::env::var_os("SWITCHBOARD").as_deref(),
        std::env::var_os("SWITCHBOARD_NESTED").as_deref(),
    ) {
        bail!("already inside Switchboard");
    }
    let stream = socket::connect_or_start()?;
    // Caught before the size is read and the terminal made raw, so a resize
    // in between is still sent and a signal still puts the terminal back.
    let (signals, signal_pipe) =
        Signals::install().context("couldn't set up the signal handlers")?;
    let (cols, rows) = window_size(libc::STDOUT_FILENO);
    let term = std::env::var("TERM").unwrap_or_default();
    let keys = crate::settings::is_on("extended_keys");
    let terminal = Terminal::enter(libc::STDIN_FILENO, libc::STDOUT_FILENO, keys)
        .context("couldn't set up the terminal")?;
    let hello = ClientMsg::Attach {
        session,
        cols,
        rows,
        term,
    };
    let end = run(
        stream,
        libc::STDIN_FILENO,
        hello,
        signal_pipe,
        (cols, rows),
        || window_size(libc::STDOUT_FILENO),
    );
    drop(terminal);
    drop(signals);
    match end {
        End::Exit(message) => {
            if let Some(message) = message {
                println!("{message}");
            }
            Ok(ExitCode::SUCCESS)
        }
        End::StdinClosed | End::TerminalGone => Ok(ExitCode::SUCCESS),
        End::ServerGone => bail!("the server went away"),
        End::Signal(sig) => Ok(ExitCode::from(128 + sig.clamp(0, 127) as u8)),
    }
}

/// Moves bytes between the terminal and the server until one side ends:
/// sends `hello`, then reads `input` (a terminal descriptor, duplicated so
/// its thread owns what it reads from), writes the server's output to the
/// attached terminal, and sends a resize for each SIGWINCH read from
/// `signals` when `measure` gives a size other than the last one sent.
fn run(
    stream: UnixStream,
    input: RawFd,
    hello: ClientMsg,
    signals: File,
    size: (u16, u16),
    measure: impl Fn() -> (u16, u16) + Send + 'static,
) -> End {
    let (Ok(writer), Ok(reader)) = (stream.try_clone(), stream.try_clone()) else {
        return End::ServerGone;
    };
    let writer = Arc::new(Mutex::new(writer));
    if send(&writer, &hello).is_err() {
        return End::ServerGone;
    }
    // SAFETY: `input` is open for the length of this call, and the
    // duplicate is the thread's own.
    let Ok(input) = unsafe { BorrowedFd::borrow_raw(input) }.try_clone_to_owned() else {
        return End::StdinClosed;
    };
    let (tx, rx) = mpsc::channel();

    let (to_server, done) = (Arc::clone(&writer), tx.clone());
    std::thread::spawn(move || {
        let _ = done.send(pump_input(FdReader(input), &to_server));
    });
    let done = tx.clone();
    std::thread::spawn(move || {
        let _ = done.send(pump_output(reader, write_screen));
    });
    let (to_server, done) = (Arc::clone(&writer), tx);
    std::thread::spawn(move || {
        if let Some(end) = pump_signals(signals, measure, &to_server, size) {
            let _ = done.send(end);
        }
    });

    // The input and output threads always say why they stopped, so this
    // only fails if both have died without a word.
    let end = rx.recv().unwrap_or(End::ServerGone);
    if end.detaches() {
        let _ = send(&writer, &ClientMsg::Detach);
    }
    // Ends the output thread; the input thread is left reading the
    // terminal, and goes with the process.
    let _ = stream.shutdown(Shutdown::Both);
    end
}

/// Sends a message, one frame in one write, so frames from the threads that
/// share the connection never interleave.
fn send<W: Write>(sock: &Mutex<W>, msg: &ClientMsg) -> io::Result<()> {
    let mut sock = sock.lock().unwrap_or_else(|e| e.into_inner());
    sock.write_all(&proto::encode_client(msg))
}

/// Reads what is typed and sends it as `Input`, until the input ends.
fn pump_input<R: Read, W: Write>(mut input: R, sock: &Mutex<W>) -> End {
    let mut buf = vec![0u8; READ_SIZE];
    loop {
        match input.read(&mut buf) {
            Ok(0) => return End::StdinClosed,
            Ok(n) => {
                if send(sock, &ClientMsg::Input(buf[..n].to_vec())).is_err() {
                    return End::ServerGone;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return End::StdinClosed,
        }
    }
}

/// Reads the server's messages and writes its output with `write`, until it
/// says to end or goes away.
fn pump_output<R: Read>(mut sock: R, mut write: impl FnMut(&[u8]) -> io::Result<()>) -> End {
    loop {
        match proto::read_server(&mut sock) {
            Ok(Some(ServerMsg::Output(bytes))) => {
                if write(&bytes).is_err() {
                    return End::TerminalGone;
                }
            }
            Ok(Some(ServerMsg::Exit { message })) => return End::Exit(message),
            // A command's reply means nothing to an attached terminal.
            Ok(Some(ServerMsg::Reply { .. })) => {}
            Ok(None) | Err(_) => return End::ServerGone,
        }
    }
}

/// Reads signal numbers from the pipe. Several resizes read together are
/// one measurement; a size the server already has isn't sent again. Any
/// other signal ends the attachment. `None` when the pipe closes, as it does
/// once the attachment is over.
fn pump_signals<R: Read, W: Write>(
    mut pipe: R,
    measure: impl Fn() -> (u16, u16),
    sock: &Mutex<W>,
    mut last: (u16, u16),
) -> Option<End> {
    let mut buf = [0u8; 64];
    loop {
        let n = match pipe.read(&mut buf) {
            Ok(0) => return None,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        let got = &buf[..n];
        if let Some(&sig) = got.iter().find(|&&b| b as libc::c_int != libc::SIGWINCH) {
            return Some(End::Signal(sig as libc::c_int));
        }
        let now = measure();
        if now != last {
            let (cols, rows) = now;
            if send(sock, &ClientMsg::Resize { cols, rows }).is_err() {
                return Some(End::ServerGone);
            }
            last = now;
        }
    }
}

/// Whether a server answers on the socket `SWITCHBOARD` names, unless
/// `SWITCHBOARD_NESTED` is `1`.
fn inside_switchboard(socket: Option<&std::ffi::OsStr>, nested: Option<&std::ffi::OsStr>) -> bool {
    if nested.is_some_and(|n| n == "1") {
        return false;
    }
    match socket {
        Some(path) if !path.is_empty() => UnixStream::connect(path).is_ok(),
        _ => false,
    }
}

fn is_tty(fd: RawFd) -> bool {
    // SAFETY: isatty only looks at the descriptor.
    unsafe { libc::isatty(fd) == 1 }
}

/// A terminal's size in columns and rows, or the usual 80 by 24 when it
/// won't say (or says zero, as some do before their window is drawn).
fn window_size(fd: RawFd) -> (u16, u16) {
    size_of(fd).unwrap_or(DEFAULT_SIZE)
}

fn size_of(fd: RawFd) -> Option<(u16, u16)> {
    // SAFETY: TIOCGWINSZ fills the winsize it is given.
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } != 0 {
        return None;
    }
    (ws.ws_col > 0 && ws.ws_row > 0).then_some((ws.ws_col, ws.ws_row))
}

/// The settings `cfmakeraw` gives: no line editing, echo, signals from keys
/// or translation of what is typed or written, eight-bit characters. A read
/// waits for one byte and no longer, which `cfmakeraw` sets on Linux and
/// leaves to the system elsewhere.
fn raw_mode(saved: &libc::termios) -> libc::termios {
    let mut raw = *saved;
    // SAFETY: cfmakeraw only changes the struct it is given.
    unsafe { libc::cfmakeraw(&mut raw) };
    raw.c_cc[libc::VMIN] = 1;
    raw.c_cc[libc::VTIME] = 0;
    raw
}

/// The terminal set up for attaching; dropping it puts it back.
struct Terminal;

impl Terminal {
    /// Saves `input`'s settings, makes it raw and turns the modes in `ENTER`
    /// on through `output`, and with `keys` asks for `KEYS_ON`. Any failure
    /// on the way puts back what was done.
    fn enter(input: RawFd, output: RawFd, keys: bool) -> io::Result<Terminal> {
        // SAFETY: tcgetattr fills the termios it is given.
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(input, &mut saved) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let raw = raw_mode(&saved);
        PANIC_HOOK.call_once(|| {
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                restore_terminal();
                previous(info);
            }));
        });
        *ATTACHED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Attached {
            input,
            output,
            saved,
            keys,
        });
        let terminal = Terminal;
        // SAFETY: tcsetattr reads the termios it is given.
        if unsafe { libc::tcsetattr(input, libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        write_screen(ENTER)?;
        if keys {
            write_screen(KEYS_ON)?;
        }
        Ok(terminal)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// Puts the attached terminal back, once: the modes off, the normal screen,
/// the settings it had. Nothing is written to it after.
fn restore_terminal() {
    let mut attached = ATTACHED.lock().unwrap_or_else(|e| e.into_inner());
    let Some(term) = attached.take() else {
        return;
    };
    // A terminal that has gone (its window closed) can't be put back, and
    // there is no one to tell.
    if term.keys {
        let _ = write_fd(term.output, KEYS_OFF);
    }
    let _ = write_fd(term.output, LEAVE);
    // SAFETY: tcsetattr reads the termios it is given.
    unsafe { libc::tcsetattr(term.input, libc::TCSANOW, &term.saved) };
}

/// Writes to the attached terminal, all of it before returning; an error once
/// the terminal has been put back.
fn write_screen(bytes: &[u8]) -> io::Result<()> {
    let attached = ATTACHED.lock().unwrap_or_else(|e| e.into_inner());
    match attached.as_ref() {
        Some(term) => write_fd(term.output, bytes),
        None => Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "the terminal has been put back",
        )),
    }
}

/// Writes all of `bytes` to a descriptor, unbuffered. One left non-blocking
/// by a program run before (Node leaves a terminal so) is waited on rather
/// than given up on.
fn write_fd(fd: RawFd, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        // SAFETY: writes from a slice that outlives the call.
        let n = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if n > 0 {
            bytes = &bytes[n as usize..];
            continue;
        }
        if n == 0 {
            return Err(io::Error::from(io::ErrorKind::WriteZero));
        }
        let e = io::Error::last_os_error();
        match e.kind() {
            io::ErrorKind::Interrupted => {}
            io::ErrorKind::WouldBlock => wait_for(fd, libc::POLLOUT),
            _ => return Err(e),
        }
    }
    Ok(())
}

/// Waits until a descriptor can be read or written (`events`).
fn wait_for(fd: RawFd, events: libc::c_short) {
    let mut pfd = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    // SAFETY: polls the one pollfd given. An error (a signal) just returns,
    // and the caller tries again.
    unsafe { libc::poll(&mut pfd, 1, -1) };
}

/// A terminal read straight from its descriptor, a read giving what one
/// `read` gives, so each keypress is sent as soon as it arrives.
struct FdReader(OwnedFd);

impl Read for FdReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            // SAFETY: reads into a slice that outlives the call.
            let n = unsafe { libc::read(self.0.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n >= 0 {
                return Ok(n as usize);
            }
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::WouldBlock {
                wait_for(self.0.as_raw_fd(), libc::POLLIN);
                continue;
            }
            return Err(e);
        }
    }
}

/// The signal handlers in place while attached; dropping it puts back the
/// ones there were and closes the pipe, which ends the thread reading it.
struct Signals {
    _write: OwnedFd,
    old: Vec<(libc::c_int, libc::sigaction)>,
}

impl Signals {
    /// Catches `CAUGHT`, each written as a byte to a pipe whose reading end
    /// is returned.
    fn install() -> io::Result<(Signals, File)> {
        let (read, write) = signal_pipe()?;
        SIGNAL_FD.store(write.as_raw_fd(), Ordering::SeqCst);
        let mut signals = Signals {
            _write: write,
            old: Vec::new(),
        };
        for sig in CAUGHT {
            // SAFETY: the action is filled before it is used, and the
            // handler does only what a handler may (see `on_signal`).
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
            action.sa_flags = libc::SA_RESTART;
            let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
            let ok = unsafe {
                libc::sigemptyset(&mut action.sa_mask);
                libc::sigaction(sig, &action, &mut old)
            } == 0;
            if !ok {
                // Dropping `signals` puts back those already caught.
                return Err(io::Error::last_os_error());
            }
            signals.old.push((sig, old));
        }
        Ok((signals, File::from(read)))
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
        for (sig, old) in self.old.drain(..) {
            // SAFETY: puts back the action sigaction gave.
            unsafe { libc::sigaction(sig, &old, std::ptr::null_mut()) };
        }
        SIGNAL_FD.store(-1, Ordering::SeqCst);
    }
}

/// A pipe for the signal handler: both ends closed on exec (std makes them
/// so as it opens them), so no program the process starts holds them, and
/// the writing end non-blocking, so a handler never waits on a pipe that is
/// full (the resizes already in it say enough).
fn signal_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let (read, write) = io::pipe()?;
    let (read, write) = (OwnedFd::from(read), OwnedFd::from(write));
    // SAFETY: fcntl reads and sets the flags of a descriptor owned here.
    let flags = unsafe { libc::fcntl(write.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(write.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok((read, write))
}

/// Writes the signal's number to the pipe. Only async-signal-safe work: an
/// atomic load and a `write`, with `errno` kept as it was for the code the
/// signal interrupted.
extern "C" fn on_signal(sig: libc::c_int) {
    let fd = SIGNAL_FD.load(Ordering::SeqCst);
    if fd < 0 {
        return;
    }
    // SAFETY: errno is this thread's; the byte outlives the write.
    unsafe {
        let errno = errno_location();
        let saved = *errno;
        let byte = sig as u8;
        libc::write(fd, (&byte as *const u8).cast(), 1);
        *errno = saved;
    }
}

#[cfg(target_os = "linux")]
unsafe fn errno_location() -> *mut libc::c_int {
    // SAFETY: always valid for the calling thread.
    unsafe { libc::__errno_location() }
}

#[cfg(not(target_os = "linux"))]
unsafe fn errno_location() -> *mut libc::c_int {
    // SAFETY: always valid for the calling thread.
    unsafe { libc::__error() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::socket::testing::TempDir;
    use std::io::Cursor;
    use std::os::fd::FromRawFd;
    use std::os::unix::net::UnixListener;
    use std::sync::MutexGuard;
    use std::time::{Duration, Instant};

    /// The tests that attach a terminal share the one `ATTACHED`.
    static TERMINAL_LOCK: Mutex<()> = Mutex::new(());

    fn terminal_lock() -> MutexGuard<'static, ()> {
        TERMINAL_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A pseudo-terminal: the master end the test reads and types into, the
    /// slave end the client sees as its terminal.
    struct Pty {
        master: OwnedFd,
        slave: OwnedFd,
    }

    impl Pty {
        fn open() -> Pty {
            // SAFETY: the usual opening of a pty pair; ptsname's buffer is
            // read before anything else calls it, the terminal tests being
            // serialized.
            unsafe {
                let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
                assert!(master >= 0, "posix_openpt");
                let master = OwnedFd::from_raw_fd(master);
                assert_eq!(libc::grantpt(master.as_raw_fd()), 0);
                assert_eq!(libc::unlockpt(master.as_raw_fd()), 0);
                let name = std::ffi::CStr::from_ptr(libc::ptsname(master.as_raw_fd())).to_owned();
                let slave = libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
                assert!(slave >= 0, "open the pty's slave");
                Pty {
                    master,
                    slave: OwnedFd::from_raw_fd(slave),
                }
            }
        }

        fn set_size(&self, cols: u16, rows: u16) {
            let ws = libc::winsize {
                ws_row: rows,
                ws_col: cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            assert_eq!(
                unsafe { libc::ioctl(self.slave.as_raw_fd(), libc::TIOCSWINSZ, &ws) },
                0
            );
        }

        fn settings(&self) -> libc::termios {
            let mut t: libc::termios = unsafe { std::mem::zeroed() };
            assert_eq!(
                unsafe { libc::tcgetattr(self.slave.as_raw_fd(), &mut t) },
                0
            );
            t
        }

        /// Types into the terminal.
        fn type_in(&self, bytes: &[u8]) {
            write_fd(self.master.as_raw_fd(), bytes).unwrap();
        }

        /// What the terminal was sent, read until it ends with `want` or a
        /// few seconds pass.
        fn read_until(&self, want: &[u8]) -> Vec<u8> {
            let mut got = Vec::new();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !got.ends_with(want) && Instant::now() < deadline {
                let mut pfd = libc::pollfd {
                    fd: self.master.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                if unsafe { libc::poll(&mut pfd, 1, 100) } <= 0 {
                    continue;
                }
                let mut buf = [0u8; 1024];
                let n = unsafe {
                    libc::read(self.master.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len())
                };
                if n <= 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n as usize]);
            }
            got
        }
    }

    fn cooked() -> libc::termios {
        let mut t: libc::termios = unsafe { std::mem::zeroed() };
        t.c_iflag = libc::ICRNL | libc::IXON | libc::BRKINT;
        t.c_oflag = libc::OPOST | libc::ONLCR;
        t.c_lflag = libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN;
        t.c_cflag = libc::CS7 | libc::PARENB | libc::CREAD;
        t.c_cc[libc::VMIN] = 0;
        t.c_cc[libc::VTIME] = 5;
        t
    }

    #[test]
    fn raw_mode_turns_off_what_the_terminal_does_itself() {
        let raw = raw_mode(&cooked());
        assert_eq!(raw.c_lflag & (libc::ICANON | libc::ECHO | libc::ISIG), 0);
        assert_eq!(raw.c_lflag & libc::IEXTEN, 0);
        assert_eq!(raw.c_iflag & (libc::ICRNL | libc::IXON | libc::BRKINT), 0);
        assert_eq!(raw.c_oflag & libc::OPOST, 0);
        assert_eq!(raw.c_cflag & libc::CSIZE, libc::CS8);
        assert_eq!(raw.c_cflag & libc::PARENB, 0);
        assert_eq!(raw.c_cflag & libc::CREAD, libc::CREAD, "still reads");
        assert_eq!(raw.c_cc[libc::VMIN], 1);
        assert_eq!(raw.c_cc[libc::VTIME], 0);
    }

    #[test]
    fn leaving_turns_off_everything_entering_turned_on() {
        let enter = String::from_utf8(ENTER.to_vec()).unwrap();
        let leave = String::from_utf8(LEAVE.to_vec()).unwrap();
        for mode in enter.split('\x1b').filter(|m| !m.is_empty()) {
            let off = format!("\x1b{}l", mode.strip_suffix('h').unwrap());
            assert!(leave.contains(&off), "{mode:?} is turned off");
        }
        assert!(leave.contains("\x1b[?1003l"), "every-motion mouse too");
        assert!(leave.contains("\x1b[?25h"), "the cursor shown");
        assert!(leave.ends_with("\x1b[?1049l"), "the normal screen last");
    }

    #[test]
    fn only_some_endings_tell_the_server() {
        assert!(End::StdinClosed.detaches());
        assert!(End::TerminalGone.detaches());
        assert!(End::Signal(libc::SIGTERM).detaches());
        assert!(!End::ServerGone.detaches());
        assert!(!End::Exit(None).detaches());
    }

    fn frames(bytes: &[u8]) -> Vec<ClientMsg> {
        let mut r = Cursor::new(bytes.to_vec());
        let mut out = Vec::new();
        while let Some(msg) = proto::read_client(&mut r).unwrap() {
            out.push(msg);
        }
        out
    }

    #[test]
    fn what_is_typed_is_sent_as_it_is_read() {
        let sock = Mutex::new(Vec::new());
        let end = pump_input(Cursor::new(b"ls\r\x1b[A".to_vec()), &sock);
        assert_eq!(end, End::StdinClosed);
        let sent = frames(&sock.into_inner().unwrap());
        assert_eq!(sent, vec![ClientMsg::Input(b"ls\r\x1b[A".to_vec())]);
    }

    #[test]
    fn typing_stops_when_the_server_is_gone() {
        struct Closed;
        impl Write for Closed {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let end = pump_input(Cursor::new(b"x".to_vec()), &Mutex::new(Closed));
        assert_eq!(end, End::ServerGone);
    }

    #[test]
    fn output_is_written_until_the_server_says_to_end() {
        let mut stream = Vec::new();
        stream.extend(proto::encode_server(&ServerMsg::Output(b"one".to_vec())));
        stream.extend(proto::encode_server(&ServerMsg::Reply {
            code: 0,
            stdout: Vec::new(),
            stderr: Vec::new(),
        }));
        stream.extend(proto::encode_server(&ServerMsg::Output(b"two".to_vec())));
        stream.extend(proto::encode_server(&ServerMsg::Exit {
            message: Some("[detached]".into()),
        }));
        stream.extend(proto::encode_server(&ServerMsg::Output(b"never".to_vec())));
        let mut written = Vec::new();
        let end = pump_output(Cursor::new(stream), |b| {
            written.push(b.to_vec());
            Ok(())
        });
        assert_eq!(end, End::Exit(Some("[detached]".into())));
        assert_eq!(written, vec![b"one".to_vec(), b"two".to_vec()]);
    }

    #[test]
    fn output_ends_when_the_server_goes_or_the_terminal_does() {
        let stream = proto::encode_server(&ServerMsg::Output(b"x".to_vec()));
        let end = pump_output(Cursor::new(stream.clone()), |_| Ok(()));
        assert_eq!(end, End::ServerGone);

        let end = pump_output(Cursor::new(stream[..3].to_vec()), |_| Ok(()));
        assert_eq!(end, End::ServerGone, "a frame cut short");

        let end = pump_output(Cursor::new(stream), |_| {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        });
        assert_eq!(end, End::TerminalGone);
    }

    #[test]
    fn resizes_are_sent_once_for_each_new_size() {
        let sizes = Mutex::new(vec![(100, 30), (100, 30), (120, 40)]);
        let measure = || sizes.lock().unwrap().remove(0);
        let winch = libc::SIGWINCH as u8;
        // Two resizes read together are one measurement; the second batch
        // finds the size unchanged; the third has a new one.
        let pipe = ChunkedReader(vec![vec![winch, winch], vec![winch], vec![winch]]);
        let sock = Mutex::new(Vec::new());
        assert_eq!(pump_signals(pipe, measure, &sock, (80, 24)), None);
        let sent = frames(&sock.into_inner().unwrap());
        assert_eq!(
            sent,
            vec![
                ClientMsg::Resize {
                    cols: 100,
                    rows: 30
                },
                ClientMsg::Resize {
                    cols: 120,
                    rows: 40
                },
            ]
        );
    }

    #[test]
    fn a_signal_to_end_ends_the_attachment() {
        let pipe = ChunkedReader(vec![vec![libc::SIGWINCH as u8, libc::SIGTERM as u8]]);
        let sock = Mutex::new(Vec::new());
        let end = pump_signals(pipe, || (80, 24), &sock, (80, 24));
        assert_eq!(end, Some(End::Signal(libc::SIGTERM)));
        assert!(sock.into_inner().unwrap().is_empty());
    }

    /// A reader giving each of its chunks to one read, as a pipe gives
    /// what was written to it since the last read.
    struct ChunkedReader(Vec<Vec<u8>>);

    impl Read for ChunkedReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.0.is_empty() {
                return Ok(0);
            }
            let chunk = self.0.remove(0);
            buf[..chunk.len()].copy_from_slice(&chunk);
            Ok(chunk.len())
        }
    }

    #[test]
    fn a_pane_of_a_running_server_is_inside_switchboard() {
        let tmp = TempDir::new();
        let sock = tmp.path().join("sock");
        let path = sock.as_os_str();
        assert!(!inside_switchboard(Some(path), None), "nothing listens yet");
        let _listener = UnixListener::bind(&sock).unwrap();
        assert!(inside_switchboard(Some(path), None));
        assert!(inside_switchboard(Some(path), Some("0".as_ref())));
        assert!(!inside_switchboard(Some(path), Some("1".as_ref())));
        assert!(!inside_switchboard(None, None));
        assert!(!inside_switchboard(Some("".as_ref()), None));
    }

    #[test]
    fn a_size_is_read_from_a_terminal_and_assumed_otherwise() {
        let pty = Pty::open();
        pty.set_size(132, 43);
        assert_eq!(window_size(pty.slave.as_raw_fd()), (132, 43));
        pty.set_size(0, 0);
        assert_eq!(window_size(pty.slave.as_raw_fd()), DEFAULT_SIZE);

        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        assert_eq!(window_size(fds[0]), DEFAULT_SIZE);
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }

    #[test]
    fn the_terminal_is_raw_while_attached_and_put_back_after() {
        let _lock = terminal_lock();
        let pty = Pty::open();
        let before = pty.settings();
        assert_ne!(before.c_lflag & libc::ICANON, 0, "a new pty is cooked");
        let fd = pty.slave.as_raw_fd();

        let terminal = Terminal::enter(fd, fd, false).unwrap();
        assert_eq!(pty.read_until(ENTER), ENTER);
        let raw = pty.settings();
        assert_eq!(raw.c_lflag & (libc::ICANON | libc::ECHO | libc::ISIG), 0);

        write_screen(b"drawn").unwrap();
        assert_eq!(pty.read_until(b"drawn"), b"drawn");

        drop(terminal);
        assert_eq!(pty.read_until(LEAVE), LEAVE);
        let after = pty.settings();
        assert_eq!(after.c_lflag, before.c_lflag);
        assert_eq!(after.c_iflag, before.c_iflag);
        assert_eq!(after.c_oflag, before.c_oflag);
        assert!(write_screen(b"late").is_err(), "nothing after it is back");
        // Putting it back again does nothing.
        restore_terminal();
    }

    #[test]
    fn extended_keys_are_asked_for_on_the_alternate_screen_and_taken_back_there() {
        let _lock = terminal_lock();
        let pty = Pty::open();
        let fd = pty.slave.as_raw_fd();
        let terminal = Terminal::enter(fd, fd, true).unwrap();
        let on = [ENTER, KEYS_ON].concat();
        assert_eq!(pty.read_until(&on), on);
        drop(terminal);
        let off = [KEYS_OFF, LEAVE].concat();
        assert_eq!(pty.read_until(&off), off);
    }

    #[test]
    fn a_terminal_that_is_not_one_is_refused() {
        let _lock = terminal_lock();
        let tmp = TempDir::new();
        let file = File::create(tmp.path().join("not-a-tty")).unwrap();
        assert!(Terminal::enter(file.as_raw_fd(), file.as_raw_fd(), false).is_err());
        assert!(ATTACHED.lock().unwrap().is_none());
    }

    /// The server's end of an attachment, for the tests that run one.
    struct FakeServer(UnixStream);

    impl FakeServer {
        fn next(&mut self) -> Option<ClientMsg> {
            proto::read_client(&mut self.0).unwrap()
        }

        fn send(&mut self, msg: &ServerMsg) {
            self.0.write_all(&proto::encode_server(msg)).unwrap();
        }

        /// The typed bytes, gathered over as many `Input` frames as they
        /// arrive in.
        fn typed(&mut self, len: usize) -> Vec<u8> {
            let mut got = Vec::new();
            while got.len() < len {
                match self.next() {
                    Some(ClientMsg::Input(bytes)) => got.extend(bytes),
                    other => panic!("expected input, got {other:?}"),
                }
            }
            got
        }
    }

    fn hello() -> ClientMsg {
        ClientMsg::Attach {
            session: Some("main".into()),
            cols: 80,
            rows: 24,
            term: "xterm-256color".into(),
        }
    }

    #[test]
    fn an_attachment_carries_keys_screens_and_resizes() {
        let _lock = terminal_lock();
        let pty = Pty::open();
        let fd = pty.slave.as_raw_fd();
        let terminal = Terminal::enter(fd, fd, false).unwrap();
        pty.read_until(ENTER);

        let (client, server) = UnixStream::pair().unwrap();
        let mut server = FakeServer(server);
        let (signal_read, signal_write) = signal_pipe().unwrap();
        let size = Arc::new(Mutex::new((80u16, 24u16)));
        let measured = Arc::clone(&size);
        let running = std::thread::spawn(move || {
            run(
                client,
                fd,
                hello(),
                File::from(signal_read),
                (80, 24),
                move || *measured.lock().unwrap(),
            )
        });

        assert_eq!(server.next(), Some(hello()));
        pty.type_in(b"echo hi\r");
        assert_eq!(server.typed(8), b"echo hi\r");

        server.send(&ServerMsg::Output(b"\x1b[Hscreen".to_vec()));
        assert_eq!(pty.read_until(b"screen"), b"\x1b[Hscreen");

        *size.lock().unwrap() = (100, 50);
        write_fd(signal_write.as_raw_fd(), &[libc::SIGWINCH as u8]).unwrap();
        assert_eq!(
            server.next(),
            Some(ClientMsg::Resize {
                cols: 100,
                rows: 50
            })
        );

        server.send(&ServerMsg::Exit {
            message: Some("[exited]".into()),
        });
        assert_eq!(running.join().unwrap(), End::Exit(Some("[exited]".into())));
        drop(terminal);
        assert!(pty.read_until(LEAVE).ends_with(LEAVE));
        // The client hung up; it said nothing more.
        assert_eq!(server.next(), None);
        drop(signal_write);
    }

    #[test]
    fn an_attachment_ended_by_a_signal_detaches() {
        let _lock = terminal_lock();
        let pty = Pty::open();
        let fd = pty.slave.as_raw_fd();
        let terminal = Terminal::enter(fd, fd, false).unwrap();

        let (client, server) = UnixStream::pair().unwrap();
        let mut server = FakeServer(server);
        let (signal_read, signal_write) = signal_pipe().unwrap();
        let running = std::thread::spawn(move || {
            run(
                client,
                fd,
                hello(),
                File::from(signal_read),
                (80, 24),
                || (80, 24),
            )
        });
        assert_eq!(server.next(), Some(hello()));
        write_fd(signal_write.as_raw_fd(), &[libc::SIGHUP as u8]).unwrap();
        assert_eq!(running.join().unwrap(), End::Signal(libc::SIGHUP));
        assert_eq!(server.next(), Some(ClientMsg::Detach));
        assert_eq!(server.next(), None);
        drop(terminal);
    }

    #[test]
    fn an_attachment_ends_when_the_server_goes() {
        let _lock = terminal_lock();
        let pty = Pty::open();
        let fd = pty.slave.as_raw_fd();
        let terminal = Terminal::enter(fd, fd, false).unwrap();

        let (client, server) = UnixStream::pair().unwrap();
        let mut server = FakeServer(server);
        let (signal_read, _signal_write) = signal_pipe().unwrap();
        let running = std::thread::spawn(move || {
            run(
                client,
                fd,
                hello(),
                File::from(signal_read),
                (80, 24),
                || (80, 24),
            )
        });
        assert_eq!(server.next(), Some(hello()));
        drop(server);
        assert_eq!(running.join().unwrap(), End::ServerGone);
        drop(terminal);
    }

    #[test]
    fn a_signal_reaches_the_pipe() {
        let _lock = terminal_lock();
        let (signals, mut pipe) = Signals::install().unwrap();
        // SAFETY: raising SIGWINCH, which the handler just installed takes;
        // its default is to be ignored, so nothing else in the tests
        // minds it.
        assert_eq!(unsafe { libc::raise(libc::SIGWINCH) }, 0);
        let mut byte = [0u8; 1];
        pipe.read_exact(&mut byte).unwrap();
        assert_eq!(byte[0] as libc::c_int, libc::SIGWINCH);
        drop(signals);
        // The pipe is closed once the handlers are put back.
        assert_eq!(pipe.read(&mut byte).unwrap(), 0);
        assert_eq!(SIGNAL_FD.load(Ordering::SeqCst), -1);
    }

    #[test]
    fn writes_wait_out_a_full_non_blocking_descriptor() {
        let (read, write) = signal_pipe().unwrap();
        // The writing end is non-blocking; a write larger than the pipe
        // holds has to wait for the reader rather than fail.
        let reader = std::thread::spawn(move || {
            let mut file = File::from(read);
            let mut all = Vec::new();
            file.read_to_end(&mut all).unwrap();
            all.len()
        });
        let big = vec![b'x'; 1 << 20];
        write_fd(write.as_raw_fd(), &big).unwrap();
        drop(write);
        assert_eq!(reader.join().unwrap(), 1 << 20);
    }
}
