//! Where the server listens, and reaching it: connecting, starting it when
//! none answers, and making sure only one listens.
//!
//! The socket is `<state dir>/sock`, or whatever `SWITCHBOARD` names: the
//! server sets that in every pane, as tmux set `TMUX`, so a command or hook
//! run in a pane reaches the server that runs the pane even when a try of a
//! work in progress keeps its state somewhere else. The state directory is
//! made readable by its owner alone and the socket is too, since anything
//! that reaches the socket can type into every pane.
//!
//! Only one server runs per state directory. A lock (`flock` on
//! `server.lock`, which the kernel lets go of however the process ends) says
//! one is running even in the moment before it listens, so two clients
//! starting a server at once get one server, the second giving up on the
//! lock while the first goes on. A socket file with nothing answering on it
//! is what a server that died leaves behind, and the next one takes its
//! place.
//!
//! A client that finds no server starts one: `switchboard server`, in a
//! session of its own (`setsid`) so closing the terminal that started it
//! doesn't hang it up, its output appended to `server.log`, which is where
//! to look when it doesn't come up.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitCode, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

use super::proto::{self, ClientMsg, ServerMsg};
use crate::paths;

/// How long a client waits for a server it started to listen.
const START_WAIT: Duration = Duration::from_secs(5);
/// How often it tries meanwhile.
const START_POLL: Duration = Duration::from_millis(50);
/// How long it goes on trying once the server it started has exited: one
/// that lost the race for the lock to another client's exits at once while
/// the winner may not be listening yet.
const AFTER_EXIT_WAIT: Duration = Duration::from_secs(1);

/// What a second server is told, and what a client is told when one is.
const ALREADY_RUNNING: &str = "a Switchboard server is already running";

/// The locks this process holds on its state directories' `server.lock`,
/// kept open for its life: closing one would let another server start.
static LOCKS: Mutex<Vec<File>> = Mutex::new(Vec::new());

/// The server's socket: `SWITCHBOARD` (set in every pane, so a command run in
/// one reaches the server that runs it), else `<state dir>/sock`.
pub fn socket_path() -> PathBuf {
    match std::env::var_os("SWITCHBOARD") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => paths::state_dir().join("sock"),
    }
}

/// A connection to the running server.
pub fn connect() -> std::io::Result<UnixStream> {
    UnixStream::connect(socket_path())
}

/// A connection to the running server, starting one (`switchboard server`,
/// detached in a session of its own, its output to `<state dir>/server.log`)
/// when none answers, and waiting up to a few seconds for it to listen.
pub fn connect_or_start() -> Result<UnixStream> {
    let path = socket_path();
    if let Ok(stream) = UnixStream::connect(&path) {
        return Ok(stream);
    }
    let dir = paths::state_dir();
    make_private_dir(&dir)?;
    let log = dir.join("server.log");
    connect_or_start_at(&path, &log, start_server, START_WAIT)
}

/// The server's listening socket, for `switchboard server`: refuses when
/// another server answers on it, takes the place of a socket left by one
/// that died, and is readable only by its owner.
///
/// It must be called on a tokio runtime, since the listener is registered
/// with the runtime's reactor; called anywhere else it gives an error. The
/// lock it takes is held until the process ends.
pub fn bind() -> Result<tokio::net::UnixListener> {
    let (listener, lock) = bind_at(&paths::state_dir(), &socket_path())?;
    LOCKS.lock().unwrap_or_else(|e| e.into_inner()).push(lock);
    Ok(listener)
}

/// Runs a command in the server (`ClientMsg::Command`), passing stdin when
/// it isn't a terminal, printing the reply's output and giving its code.
///
/// It never starts a server: a command that changes a tab means nothing to
/// a server that has none, and a hook run outside Switchboard should cost
/// nothing. Stdin is read whole when it is a pipe, a socket or a file, as a
/// hook's JSON is given (Claude Code hands it a socket), and only for the
/// commands that take it (`commands::TAKES_STDIN`); a terminal or
/// `/dev/null` is not read at all.
pub fn run_command(args: Vec<String>) -> ExitCode {
    let cwd = std::env::current_dir()
        .ok()
        .or_else(|| std::env::var_os("PWD").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/"));
    let pane = std::env::var("SWITCHBOARD_PANE")
        .ok()
        .filter(|p| !p.is_empty());
    // Only a command that takes input reads it: one run where stdin is a pipe
    // nobody closes (some tools hand their commands one) would otherwise wait
    // for it forever.
    let takes_input = args
        .first()
        .is_some_and(|a| super::commands::TAKES_STDIN.contains(&a.as_str()));
    let stdin = if takes_input && is_data(libc::STDIN_FILENO) {
        let mut input = Vec::new();
        io::stdin()
            .lock()
            .read_to_end(&mut input)
            .ok()
            .map(|_| input)
    } else {
        None
    };
    let msg = ClientMsg::Command {
        args,
        cwd,
        pane,
        stdin,
    };
    let code = run_command_at(
        &socket_path(),
        &msg,
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    );
    ExitCode::from(code)
}

/// How long `send_quiet` waits on the server: Claude Code waits on its hooks,
/// so a server that has stopped answering mustn't hold Claude up for long.
const QUIET_WAIT: Duration = Duration::from_secs(1);

/// Runs a command in the server and gives its reply's code, printing nothing
/// and waiting at most `QUIET_WAIT` on each read and write: for `switchboard
/// hook`, which Claude Code runs on every tool call. `None` when no server
/// answered. Like `run_command`, it never starts a server.
pub fn send_quiet(args: Vec<String>, pane: Option<String>, stdin: Option<Vec<u8>>) -> Option<i32> {
    ask_quiet(args, pane, stdin).map(|(code, _, _)| code)
}

/// As `send_quiet`, giving the whole answer (its code, what it printed and
/// what it said went wrong): for `switchboard claude-context`, which Claude
/// Code waits on as a session starts.
pub fn ask_quiet(
    args: Vec<String>,
    pane: Option<String>,
    stdin: Option<Vec<u8>>,
) -> Option<(i32, Vec<u8>, Vec<u8>)> {
    let cwd = std::env::current_dir()
        .ok()
        .or_else(|| std::env::var_os("PWD").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/"));
    let mut stream = UnixStream::connect(socket_path()).ok()?;
    stream.set_read_timeout(Some(QUIET_WAIT)).ok()?;
    stream.set_write_timeout(Some(QUIET_WAIT)).ok()?;
    let msg = ClientMsg::Command {
        args,
        cwd,
        pane,
        stdin,
    };
    exchange(&mut stream, &msg).ok()
}

/// Runs a command in the server and gives its answer, its code, what it
/// printed and what it said went wrong, for a program that reads the answer
/// rather than passing it on (`wt-jump` reading `list`, `wt-batch` opening
/// tabs). The caller's pane is its `SWITCHBOARD_PANE`, as for `run_command`.
/// An error when no server answers; like `run_command`, it never starts one.
pub fn ask(args: &[&str]) -> Result<(i32, String, String)> {
    let cwd = std::env::current_dir()
        .ok()
        .or_else(|| std::env::var_os("PWD").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/"));
    let pane = std::env::var("SWITCHBOARD_PANE")
        .ok()
        .filter(|p| !p.is_empty());
    let mut stream =
        UnixStream::connect(socket_path()).context("no Switchboard server is running")?;
    let msg = ClientMsg::Command {
        args: args.iter().map(|a| a.to_string()).collect(),
        cwd,
        pane,
        stdin: None,
    };
    let (code, stdout, stderr) = exchange(&mut stream, &msg)?;
    Ok((
        code,
        String::from_utf8_lossy(&stdout).into_owned(),
        String::from_utf8_lossy(&stderr).into_owned(),
    ))
}

/// As `ask`, for a command that must succeed: what it printed, or an error
/// carrying what it said went wrong.
pub fn ask_ok(args: &[&str]) -> Result<String> {
    let (code, stdout, stderr) = ask(args)?;
    if code != 0 {
        bail!(
            "{}",
            stderr.trim().lines().next().unwrap_or("the server refused")
        );
    }
    Ok(stdout)
}

/// Whether this process runs in a pane of a running Switchboard: its
/// `SWITCHBOARD` and `SWITCHBOARD_PANE` set, as tmux's `TMUX` and
/// `TMUX_PANE` told a script it was inside tmux.
pub fn in_pane() -> bool {
    let set = |var: &str| std::env::var_os(var).is_some_and(|v| !v.is_empty());
    set("SWITCHBOARD") && set("SWITCHBOARD_PANE")
}

/// Whether stdin holds data to read: a pipe, a socket or a file, as a
/// hook's JSON is given (Claude Code hands it a socket), not a terminal.
pub fn stdin_is_data() -> bool {
    is_data(libc::STDIN_FILENO)
}

/// `run_command` on the socket at `path`, writing to `out` and `err`, for the
/// tests to run against a server of their own.
fn run_command_at(path: &Path, msg: &ClientMsg, out: &mut impl Write, err: &mut impl Write) -> u8 {
    let Ok(mut stream) = UnixStream::connect(path) else {
        let _ = writeln!(err, "switchboard: no server is running");
        let _ = err.flush();
        return 1;
    };
    match exchange(&mut stream, msg) {
        Ok((code, stdout, stderr)) => {
            // A reader that has gone away (`| head`) is no reason to say
            // anything more.
            let _ = out.write_all(&stdout).and_then(|_| out.flush());
            let _ = err.write_all(&stderr).and_then(|_| err.flush());
            code.clamp(0, 255) as u8
        }
        Err(e) => {
            let _ = writeln!(err, "switchboard: {e}");
            let _ = err.flush();
            1
        }
    }
}

/// Sends a command and waits for its reply. Output and other messages
/// before it are passed over; an `Exit` before it says why there is none.
fn exchange(stream: &mut UnixStream, msg: &ClientMsg) -> Result<(i32, Vec<u8>, Vec<u8>)> {
    stream
        .write_all(&proto::encode_client(msg))
        .context("couldn't reach the server")?;
    loop {
        match proto::read_server(stream) {
            Ok(Some(ServerMsg::Reply {
                code,
                stdout,
                stderr,
            })) => return Ok((code, stdout, stderr)),
            Ok(Some(ServerMsg::Exit { message })) => {
                return Err(anyhow!(
                    "{}",
                    message.unwrap_or_else(|| "the server ended the command".into())
                ));
            }
            Ok(Some(ServerMsg::Output(_))) => {}
            Ok(None) => bail!("the server closed the connection without an answer"),
            Err(e) => bail!("the server's answer couldn't be read: {e}"),
        }
    }
}

/// Whether a file descriptor holds data to pass on: a pipe, a socket (as
/// Claude Code hands a hook its JSON) or a regular file, not a terminal or
/// a device.
fn is_data(fd: libc::c_int) -> bool {
    // SAFETY: fstat only fills the struct it is given; a closed descriptor
    // is an error, not undefined.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut st) } != 0 {
        return false;
    }
    let kind = st.st_mode & libc::S_IFMT;
    kind == libc::S_IFIFO || kind == libc::S_IFREG || kind == libc::S_IFSOCK
}

/// Makes a directory (and those above it) readable by its owner alone. One
/// that is already there is left as it is: the state directory holds more
/// than the socket, and someone may have reasons for its mode.
fn make_private_dir(dir: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("couldn't make {}", paths::tilde(dir)))
}

/// Starts `switchboard server` in the background, detached from this
/// terminal, its output appended to `log`.
fn start_server(log: &Path) -> io::Result<Child> {
    let exe = std::env::current_exe()?;
    let out = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(log)?;
    let err = out.try_clone()?;
    let mut command = std::process::Command::new(exe);
    command
        .arg("server")
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err);
    // SAFETY: setsid is async-signal-safe, so it may run between fork and
    // exec; it fails only for a process group leader, which a child just
    // forked is not.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn()
}

/// Connects to `path`, starting a server with `start` when nothing answers,
/// then trying every `START_POLL` until `wait` has passed. The server isn't
/// waited on: it outlives this client. Should it exit, it is reaped (so it
/// doesn't linger as a zombie for the client's life) and the client tries a
/// little longer in case another client's server won the lock, then gives up
/// with its exit status.
fn connect_or_start_at(
    path: &Path,
    log: &Path,
    start: impl FnOnce(&Path) -> io::Result<Child>,
    wait: Duration,
) -> Result<UnixStream> {
    if let Ok(stream) = UnixStream::connect(path) {
        return Ok(stream);
    }
    let mut child = start(log).with_context(|| {
        format!(
            "couldn't start the Switchboard server (its log is {})",
            paths::tilde(log)
        )
    })?;
    let began = Instant::now();
    let mut deadline = began + wait;
    let mut exited = None;
    loop {
        if let Ok(stream) = UnixStream::connect(path) {
            return Ok(stream);
        }
        if exited.is_none()
            && let Ok(Some(status)) = child.try_wait()
        {
            exited = Some(status);
            deadline = deadline.min(Instant::now() + AFTER_EXIT_WAIT.min(wait));
        }
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        std::thread::sleep(START_POLL.min(deadline - now));
    }
    let how = match exited.map(|s| s.code()) {
        Some(Some(code)) => format!("exited with {code}"),
        Some(None) => "was killed".to_string(),
        None => format!("didn't listen within {} seconds", wait.as_secs_f32()),
    };
    bail!(
        "the Switchboard server {how}; {} may say why",
        paths::tilde(log)
    )
}

/// Takes the server lock in `dir`, then the socket at `path`, as `bind`
/// says. The lock is given back for the caller to keep.
fn bind_at(dir: &Path, path: &Path) -> Result<(tokio::net::UnixListener, File)> {
    // Checked first, since tokio's listener would panic outside a runtime
    // rather than say so.
    if tokio::runtime::Handle::try_current().is_err() {
        bail!("the server's socket can only be made on a tokio runtime");
    }
    make_private_dir(dir)?;
    let lock = take_lock(&dir.join("server.lock"))?;
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if UnixStream::connect(path).is_ok() {
                bail!(ALREADY_RUNNING);
            }
            if !meta.file_type().is_socket() {
                bail!(
                    "{} is in the way of the server's socket",
                    paths::tilde(path)
                );
            }
            // Nothing answers: a server that died left it.
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("couldn't remove the old socket {}", paths::tilde(path))
                    });
                }
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(e).with_context(|| format!("couldn't look at {}", paths::tilde(path)));
        }
    }
    let listener = tokio::net::UnixListener::bind(path)
        .with_context(|| format!("couldn't listen on {}", paths::tilde(path)))?;
    // The directory is the owner's alone already when Switchboard made it;
    // this covers a socket placed elsewhere with `SWITCHBOARD`.
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("couldn't make {} private", paths::tilde(path)))?;
    Ok((listener, lock))
}

/// An exclusive lock on `path`, which the kernel drops when the file is
/// closed or the process ends, so a server that crashed never leaves it
/// held. It is opened close-on-exec, so no pane's program holds it on.
fn take_lock(path: &Path) -> Result<File> {
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("couldn't open {}", paths::tilde(path)))?;
    // SAFETY: flock on a descriptor this function owns.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
            bail!(ALREADY_RUNNING);
        }
        return Err(e).with_context(|| format!("couldn't lock {}", paths::tilde(path)));
    }
    Ok(file)
}

/// The descriptor of the lock this process holds, for handing it to the
/// build the server replaces itself with (src/mux/upgrade.rs): the lock
/// belongs to the open file, so the new build holds it from the start and
/// no other server can take it in between.
pub fn lock_fd() -> Option<std::os::fd::RawFd> {
    LOCKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .first()
        .map(AsRawFd::as_raw_fd)
}

/// The listening socket and the lock a server handed over, by descriptor,
/// for `switchboard server --handover`: both closed on exec again, the lock
/// kept for the process's life, the socket listening on as it was, so no
/// connection made meanwhile was refused. Like `bind`, it must be called on
/// a tokio runtime.
pub fn adopt(
    listener: std::os::fd::RawFd,
    lock: Option<std::os::fd::RawFd>,
) -> Result<tokio::net::UnixListener> {
    use std::os::fd::FromRawFd;
    if tokio::runtime::Handle::try_current().is_err() {
        bail!("the server's socket can only be taken up on a tokio runtime");
    }
    if let Some(lock) = lock.filter(|fd| super::pty::is_open(*fd)) {
        let _ = super::pty::set_cloexec(lock, true);
        // SAFETY: the descriptor was handed over open, and nothing else here
        // owns it.
        let file = unsafe { File::from_raw_fd(lock) };
        LOCKS.lock().unwrap_or_else(|e| e.into_inner()).push(file);
    }
    if !super::pty::is_open(listener) {
        bail!("the listening socket handed over (descriptor {listener}) isn't open");
    }
    super::pty::set_cloexec(listener, true).context("taking up the listening socket")?;
    // SAFETY: as above.
    let std = unsafe { std::os::unix::net::UnixListener::from_raw_fd(listener) };
    std.set_nonblocking(true)
        .context("taking up the listening socket")?;
    tokio::net::UnixListener::from_std(std).context("taking up the listening socket")
}

/// Serializes the tests, here and in the other parts of `mux`, that change
/// the environment, since tests run on parallel threads and the environment
/// is the process's.
#[cfg(test)]
pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
pub(crate) mod testing {
    //! What the `mux` tests share: a directory of their own under `/tmp`
    //! (short, since a socket's path can't be over 104 bytes on macOS),
    //! removed when the test ends, and the environment set for a while.

    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(crate) struct TempDir(PathBuf);

    impl TempDir {
        pub(crate) fn new() -> TempDir {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = PathBuf::from(format!("/tmp/sb-mux-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("a temporary directory");
            TempDir(dir)
        }

        pub(crate) fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Sets (or, with `None`, removes) environment variables until dropped,
    /// then puts back what was there. Hold `ENV_LOCK` while it lives.
    pub(crate) struct EnvVars(Vec<(&'static str, Option<OsString>)>);

    impl EnvVars {
        pub(crate) fn set(vars: &[(&'static str, Option<&str>)]) -> EnvVars {
            let mut old = Vec::new();
            for (key, value) in vars {
                old.push((*key, std::env::var_os(key)));
                // SAFETY: the tests that change the environment hold
                // ENV_LOCK, and nothing else in them reads it from C.
                unsafe {
                    match value {
                        Some(v) => std::env::set_var(key, v),
                        None => std::env::remove_var(key),
                    }
                }
            }
            EnvVars(old)
        }
    }

    impl Drop for EnvVars {
        fn drop(&mut self) {
            for (key, value) in self.0.drain(..).rev() {
                // SAFETY: as in `set`.
                unsafe {
                    match value {
                        Some(v) => std::env::set_var(key, v),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{EnvVars, TempDir};
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::MutexGuard;

    fn env_lock() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// What `attempt` gives once it succeeds, tried for up to two seconds: a
    /// lock let go of can still be held for a moment by a process another
    /// test forked while it was open, until that process execs and the
    /// copy it was given closes.
    fn eventually<T, E: std::fmt::Display>(mut attempt: impl FnMut() -> Result<T, E>) -> T {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match attempt() {
                Ok(value) => return value,
                Err(e) if Instant::now() >= deadline => panic!("still failing: {e}"),
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }

    #[test]
    fn the_socket_is_where_switchboard_says() {
        let _env = env_lock();
        let tmp = TempDir::new();
        let state = tmp.path().join("state");
        let _vars = EnvVars::set(&[
            ("SWITCHBOARD", Some("/tmp/elsewhere/sock")),
            ("SWITCHBOARD_STATE_DIR", Some(state.to_str().unwrap())),
        ]);
        assert_eq!(socket_path(), PathBuf::from("/tmp/elsewhere/sock"));
    }

    #[test]
    fn the_socket_is_in_the_state_directory_otherwise() {
        let _env = env_lock();
        let tmp = TempDir::new();
        let state = tmp.path().join("state");
        let _vars = EnvVars::set(&[
            ("SWITCHBOARD", None),
            ("SWITCHBOARD_STATE_DIR", Some(state.to_str().unwrap())),
        ]);
        assert_eq!(socket_path(), state.join("sock"));
        drop(_vars);

        let _vars = EnvVars::set(&[
            ("SWITCHBOARD", Some("")),
            ("SWITCHBOARD_STATE_DIR", Some(state.to_str().unwrap())),
        ]);
        assert_eq!(socket_path(), state.join("sock"));
    }

    #[test]
    fn bind_listens_privately_and_only_once() {
        let _env = env_lock();
        let tmp = TempDir::new();
        let state = tmp.path().join("state");
        let _vars = EnvVars::set(&[
            ("SWITCHBOARD", None),
            ("SWITCHBOARD_STATE_DIR", Some(state.to_str().unwrap())),
        ]);
        let rt = runtime();
        let _guard = rt.enter();
        let listener = bind().unwrap();
        let sock = state.join("sock");
        assert_eq!(mode_of(&state), 0o700);
        assert_eq!(mode_of(&sock), 0o600);
        assert!(connect().is_ok(), "a client reaches it");

        // The lock refuses a second server even if the socket were gone.
        let err = bind().unwrap_err();
        assert_eq!(err.to_string(), ALREADY_RUNNING);
        fs::remove_file(&sock).unwrap();
        assert_eq!(bind().unwrap_err().to_string(), ALREADY_RUNNING);
        drop(listener);
    }

    #[test]
    fn bind_refuses_a_socket_something_answers_on() {
        let _env = env_lock();
        let tmp = TempDir::new();
        let state = tmp.path().join("state");
        fs::create_dir_all(&state).unwrap();
        let sock = state.join("sock");
        let other = UnixListener::bind(&sock).unwrap();
        let _vars = EnvVars::set(&[
            ("SWITCHBOARD", None),
            ("SWITCHBOARD_STATE_DIR", Some(state.to_str().unwrap())),
        ]);
        let rt = runtime();
        let _guard = rt.enter();
        assert_eq!(bind().unwrap_err().to_string(), ALREADY_RUNNING);
        assert!(sock.exists(), "the live socket is left alone");

        // Refusing gave the lock back, so once the other one has gone the
        // socket it left is taken over.
        drop(other);
        let listener = eventually(bind);
        assert!(UnixStream::connect(&sock).is_ok());
        drop(listener);
    }

    #[test]
    fn bind_takes_the_place_of_a_stale_socket() {
        let tmp = TempDir::new();
        let sock = tmp.path().join("sock");
        drop(UnixListener::bind(&sock).unwrap());
        assert!(sock.exists());

        let rt = runtime();
        let _guard = rt.enter();
        let (listener, _lock) = eventually(|| bind_at(tmp.path(), &sock));
        assert!(UnixStream::connect(&sock).is_ok());
        assert_eq!(mode_of(&sock), 0o600);
        drop(listener);
    }

    #[test]
    fn bind_leaves_a_file_that_is_not_a_socket() {
        let tmp = TempDir::new();
        let sock = tmp.path().join("sock");
        fs::write(&sock, "keep me").unwrap();
        let rt = runtime();
        let _guard = rt.enter();
        let err = bind_at(tmp.path(), &sock).unwrap_err();
        assert!(err.to_string().contains("in the way"), "{err}");
        assert_eq!(fs::read_to_string(&sock).unwrap(), "keep me");
    }

    #[test]
    fn bind_outside_a_runtime_says_so() {
        let tmp = TempDir::new();
        let err = bind_at(tmp.path(), &tmp.path().join("sock")).unwrap_err();
        assert!(err.to_string().contains("tokio runtime"), "{err}");
    }

    #[test]
    fn the_lock_is_held_by_one_holder_at_a_time() {
        let tmp = TempDir::new();
        let path = tmp.path().join("server.lock");
        let first = take_lock(&path).unwrap();
        assert_eq!(take_lock(&path).unwrap_err().to_string(), ALREADY_RUNNING);
        drop(first);
        eventually(|| take_lock(&path));
    }

    #[test]
    fn a_private_directory_is_made_with_its_parents() {
        let tmp = TempDir::new();
        let dir = tmp.path().join("a/b");
        make_private_dir(&dir).unwrap();
        assert_eq!(mode_of(&dir), 0o700);
        // Made again, it is left as it is.
        make_private_dir(&dir).unwrap();
    }

    /// A server for one command: accepts a connection, reads the command,
    /// hands it back through the join handle, and answers with `answer`.
    fn one_command_server(
        sock: &Path,
        answer: Vec<ServerMsg>,
    ) -> std::thread::JoinHandle<Option<ClientMsg>> {
        let listener = UnixListener::bind(sock).unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let msg = proto::read_client(&mut stream).unwrap();
            for reply in answer {
                stream.write_all(&proto::encode_server(&reply)).unwrap();
            }
            msg
        })
    }

    fn command(args: &[&str]) -> ClientMsg {
        ClientMsg::Command {
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: PathBuf::from("/home/someone/repo"),
            pane: Some("%3".into()),
            stdin: Some(b"{\"hook_event_name\":\"Stop\"}".to_vec()),
        }
    }

    #[test]
    fn a_command_prints_its_reply_and_gives_its_code() {
        let tmp = TempDir::new();
        let sock = tmp.path().join("sock");
        let server = one_command_server(
            &sock,
            vec![
                ServerMsg::Output(b"ignored".to_vec()),
                ServerMsg::Reply {
                    code: 3,
                    stdout: b"renamed\n".to_vec(),
                    stderr: b"a warning\n".to_vec(),
                },
            ],
        );
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let msg = command(&["tab", "rename", "x"]);
        assert_eq!(run_command_at(&sock, &msg, &mut out, &mut err), 3);
        assert_eq!(out, b"renamed\n");
        assert_eq!(err, b"a warning\n");
        assert_eq!(server.join().unwrap(), Some(msg));
    }

    #[test]
    fn a_reply_code_is_kept_to_what_a_process_can_exit_with() {
        for (code, want) in [(0, 0), (255, 255), (300, 255), (-5, 0)] {
            let tmp = TempDir::new();
            let sock = tmp.path().join("sock");
            let server = one_command_server(
                &sock,
                vec![ServerMsg::Reply {
                    code,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                }],
            );
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let got = run_command_at(&sock, &command(&["x"]), &mut out, &mut err);
            assert_eq!(got, want, "code {code}");
            server.join().unwrap();
        }
    }

    #[test]
    fn a_command_with_no_server_says_so() {
        let tmp = TempDir::new();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run_command_at(
            &tmp.path().join("sock"),
            &command(&["x"]),
            &mut out,
            &mut err,
        );
        assert_eq!(code, 1);
        assert!(out.is_empty());
        assert_eq!(err, b"switchboard: no server is running\n");
    }

    #[test]
    fn a_command_the_server_drops_fails() {
        let tmp = TempDir::new();
        let sock = tmp.path().join("sock");
        let server = one_command_server(&sock, Vec::new());
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(
            run_command_at(&sock, &command(&["x"]), &mut out, &mut err),
            1
        );
        server.join().unwrap();
        let said = String::from_utf8(err).unwrap();
        assert!(said.contains("without an answer"), "{said}");
    }

    #[test]
    fn a_command_the_server_ends_says_why() {
        let tmp = TempDir::new();
        let sock = tmp.path().join("sock");
        let server = one_command_server(
            &sock,
            vec![ServerMsg::Exit {
                message: Some("the server is shutting down".into()),
            }],
        );
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(
            run_command_at(&sock, &command(&["x"]), &mut out, &mut err),
            1
        );
        server.join().unwrap();
        assert_eq!(err, b"switchboard: the server is shutting down\n");
    }

    #[test]
    fn pipes_sockets_and_files_count_as_input() {
        let tmp = TempDir::new();
        let file = File::create(tmp.path().join("input")).unwrap();
        assert!(is_data(file.as_raw_fd()));

        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        assert!(is_data(fds[0]));
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }

        let null = File::open("/dev/null").unwrap();
        assert!(!is_data(null.as_raw_fd()), "a device is not input");
        let (a, _b) = UnixStream::pair().unwrap();
        assert!(is_data(a.as_raw_fd()), "Claude Code hands a hook a socket");
        assert!(!is_data(-1), "nor is a closed descriptor");
    }

    #[test]
    fn a_client_reaches_a_server_already_there_without_starting_one() {
        let tmp = TempDir::new();
        let sock = tmp.path().join("sock");
        let _listener = UnixListener::bind(&sock).unwrap();
        let stream = connect_or_start_at(
            &sock,
            &tmp.path().join("server.log"),
            |_| Err(io::Error::other("started a server")),
            Duration::from_millis(200),
        );
        assert!(stream.is_ok());
    }

    #[test]
    fn a_client_waits_for_the_server_it_started() {
        let tmp = TempDir::new();
        let sock = tmp.path().join("sock");
        let log = tmp.path().join("server.log");
        let (tx, rx) = std::sync::mpsc::channel();
        let at = sock.clone();
        let stream = connect_or_start_at(
            &sock,
            &log,
            move |log| {
                assert!(log.ends_with("server.log"));
                // The "server" listens a moment after starting, and its
                // process (here a stand-in that ends at once) isn't it.
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(150));
                    let listener = UnixListener::bind(&at).unwrap();
                    let _ = rx.recv();
                    drop(listener);
                });
                std::process::Command::new("true").spawn()
            },
            Duration::from_secs(5),
        );
        assert!(stream.is_ok(), "{:?}", stream.err());
        let _ = tx.send(());
    }

    #[test]
    fn a_server_that_never_listens_points_to_its_log() {
        let tmp = TempDir::new();
        let sock = tmp.path().join("sock");
        let log = tmp.path().join("server.log");
        let began = Instant::now();
        let err = connect_or_start_at(
            &sock,
            &log,
            |_| {
                std::process::Command::new("sh")
                    .args(["-c", "exit 3"])
                    .spawn()
            },
            Duration::from_millis(400),
        )
        .unwrap_err()
        .to_string();
        assert!(began.elapsed() < Duration::from_secs(3));
        assert!(err.contains("exited with 3"), "{err}");
        assert!(err.contains("server.log"), "{err}");

        let err = connect_or_start_at(
            &sock,
            &log,
            |_| std::process::Command::new("sleep").arg("2").spawn(),
            Duration::from_millis(200),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("didn't listen"), "{err}");
        assert!(err.contains("server.log"), "{err}");
    }

    #[test]
    fn a_server_that_cannot_start_says_so() {
        let tmp = TempDir::new();
        let err = connect_or_start_at(
            &tmp.path().join("sock"),
            &tmp.path().join("server.log"),
            |_| Err(io::Error::from(io::ErrorKind::NotFound)),
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("couldn't start"), "{err:#}");
    }
}
