//! What the integration tests share: a server of their own, in a temporary
//! home under /tmp (short, since a socket's path can't be over 104 bytes on
//! macOS), never the real one; commands run against it; and a client that
//! attaches over the socket and reads what the server draws through a
//! terminal emulator, so a test can look at the screen as a person would.

#![allow(dead_code)]

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use switchboard::mux::proto::{self, ClientMsg, ServerMsg};
use switchboard::ui::headless::Screen;

pub mod http;
pub mod stand_in;

pub const BIN: &str = env!("CARGO_BIN_EXE_switchboard");

static NEXT: AtomicU32 = AtomicU32::new(0);

/// How many times longer than on an idle machine the tests wait for
/// anything: the one-minute load average over the cores, rounded up, at
/// least 1 and at most 8, read at each wait, since a run raises the load it
/// starts at, so a busy machine makes a test slower rather than failed.
/// `SWITCHBOARD_TEST_PATIENCE=<n>` sets it by hand.
pub fn patience() -> u32 {
    let mut load = [0f64; 3];
    // SAFETY: getloadavg writes at most the 3 samples asked for.
    let read = unsafe { libc::getloadavg(load.as_mut_ptr(), 3) };
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    patience_from(
        std::env::var("SWITCHBOARD_TEST_PATIENCE").ok().as_deref(),
        if read > 0 { load[0] } else { 0.0 },
        cores,
    )
}

/// `patience` from what it reads: the variable when it is a number, else the
/// load over the cores, either held to 1 to 8.
pub fn patience_from(given: Option<&str>, load: f64, cores: usize) -> u32 {
    if let Some(n) = given.and_then(|g| g.trim().parse::<u32>().ok()) {
        return n.clamp(1, 8);
    }
    ((load / cores.max(1) as f64).ceil() as u32).clamp(1, 8)
}

/// `base` from now, times `patience`.
pub fn deadline(base: Duration) -> Instant {
    Instant::now() + base * patience()
}

/// The settings every test server starts with, under any a test gives: the
/// day's notes and the Slack lookups off, since a server making the day's
/// standup note on a Monday shows its "New note" in the sidebar, which a
/// test reading the screen or counting notifications then trips on; and
/// keeping the computer awake, which would start (a stand-in) caffeinate
/// whenever a test's pane works. A test of one of them turns it on.
pub const QUIET: &[(&str, bool)] = &[
    ("meeting_notes", false),
    ("standup_note", false),
    ("standup_summary", false),
    ("todo_note", false),
    ("slack_background", false),
    ("keep_awake", false),
    ("turn_summaries", false),
];

/// Writes `home`'s `settings.json` whole: `QUIET` with `settings` (a JSON
/// object) over it, so a test's settings never turn the day's notes back on.
pub fn write_settings(home: &Path, settings: serde_json::Value) {
    write_settings_to(&home.join(".config/switchboard/settings.json"), settings);
}

/// As `write_settings`, to `file`: one a server is given in
/// `SWITCHBOARD_SETTINGS` in place of its home's.
pub fn write_settings_to(file: &Path, settings: serde_json::Value) {
    let mut all: serde_json::Map<String, serde_json::Value> = QUIET
        .iter()
        .map(|(k, v)| (k.to_string(), serde_json::Value::Bool(*v)))
        .collect();
    all.extend(
        settings
            .as_object()
            .expect("settings are an object")
            .clone(),
    );
    switchboard::util::write_atomic(file, serde_json::Value::Object(all).to_string()).unwrap();
}

/// A temporary directory, removed when dropped.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(prefix: &str) -> TempDir {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = PathBuf::from("/tmp").join(format!("{prefix}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        TempDir(dir.canonicalize().expect("it is there"))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A server of the test's own.
pub struct TestServer {
    pub dir: TempDir,
    pub home: PathBuf,
    pub state: PathBuf,
    child: Option<Child>,
    /// More environment for the server, as a test's stand-in services' addresses.
    extra_env: Vec<(String, String)>,
    /// The binary the server is started from: `BIN`, or a copy of the
    /// test's own that it can put something else in place of.
    bin: PathBuf,
}

impl TestServer {
    pub fn start() -> TestServer {
        TestServer::start_with_env(&[])
    }

    /// A server whose environment has these variables too.
    pub fn start_with_env(env: &[(&str, &str)]) -> TestServer {
        TestServer::start_with(env, None)
    }

    /// A server whose home's `settings.json` (what Settings keeps of
    /// yours) is `settings` from the start.
    pub fn start_with_settings(settings: &str) -> TestServer {
        TestServer::start_with(&[], Some(settings))
    }

    /// A server with these variables in its environment and `settings` as
    /// its home's `settings.json` from the start.
    pub fn start_with_env_and_settings(env: &[(&str, &str)], settings: &str) -> TestServer {
        TestServer::start_with(env, Some(settings))
    }

    fn start_with(env: &[(&str, &str)], settings: Option<&str>) -> TestServer {
        let dir = TempDir::new("sbt");
        let home = dir.path().join("home");
        let state = dir.path().join("s");
        std::fs::create_dir_all(&home).unwrap();
        // The login shell's own prompt: a Mac's /etc/profile sets one naming
        // the computer over PS1, long enough to wrap what a test types.
        std::fs::write(home.join(".profile"), "PS1='$ '\n").unwrap();
        if let Some(settings) = settings {
            write_settings(
                &home,
                serde_json::from_str(settings).expect("settings in JSON"),
            );
        }
        let mut server = TestServer {
            dir,
            home,
            state,
            child: None,
            extra_env: env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            bin: PathBuf::from(BIN),
        };
        server.launch();
        server
    }

    /// A server started from a copy of the binary in the test's own
    /// directory (a hard link where it can be, else a copy), which the test
    /// may replace, as `switchboard update` replaces yours; gives the copy's
    /// path too.
    pub fn start_from_copy() -> (TestServer, PathBuf) {
        TestServer::start_from_copy_with_env(&[])
    }

    /// As `start_from_copy`, its environment with these variables too.
    pub fn start_from_copy_with_env(env: &[(&str, &str)]) -> (TestServer, PathBuf) {
        let dir = TempDir::new("sbt");
        let home = dir.path().join("home");
        let state = dir.path().join("s");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join(".profile"), "PS1='$ '\n").unwrap();
        let bin_dir = dir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let bin = bin_dir.join("switchboard");
        if std::fs::hard_link(BIN, &bin).is_err() {
            std::fs::copy(BIN, &bin).expect("a copy of the binary");
        }
        let mut server = TestServer {
            dir,
            home,
            state,
            child: None,
            extra_env: env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            bin: bin.clone(),
        };
        server.launch();
        (server, bin)
    }

    /// A server with no state directory of its own, as yours runs: its
    /// state in its home's `~/.local/state/switchboard` (the variable set
    /// empty, which counts as not set).
    pub fn start_in_home() -> TestServer {
        let dir = TempDir::new("sbt");
        let home = dir.path().join("home");
        let state = home.join(".local/state/switchboard");
        std::fs::create_dir_all(&home).unwrap();
        let mut server = TestServer {
            dir,
            home,
            state,
            child: None,
            extra_env: vec![("SWITCHBOARD_STATE_DIR".into(), String::new())],
            bin: PathBuf::from(BIN),
        };
        server.launch();
        server
    }

    /// Ends the server and starts another in the same home and state, as
    /// after a restart of the machine.
    pub fn restart(&mut self) {
        self.stop();
        self.launch();
    }

    /// Strict stand-ins for the programs the server may run that the test
    /// gave none of, first on its PATH, so a test never reaches the real one
    /// on the computer it runs on: a Claude Code and a gh that aren't signed
    /// in (a real Claude on a Mac signs in through the Keychain, not `~`, so
    /// its connector calls would reach the person's own services), and the
    /// desktop's clipboard and openers, which write to the test's home
    /// rather than the person's clipboard and desktop, and caffeinate, which
    /// would hold the person's Mac awake. A test's own, written over them,
    /// wins; lazygit is left out, since a test shows the key saying it isn't
    /// installed.
    fn stand_ins(&self) {
        let bin = self.home.join("bin");
        if !bin.join("claude").exists() {
            stand_in::Claude::signed_out().install(&bin);
        }
        if !bin.join("gh").exists() {
            stand_in::Gh::new().install(&bin);
        }
        let desktop: Vec<&str> = stand_in::OPENERS
            .iter()
            .chain(stand_in::CLIPBOARD)
            .chain(stand_in::CHROMES)
            .chain(&["osascript", "caffeinate"])
            .copied()
            .filter(|tool| !bin.join(tool).exists())
            .collect();
        stand_in::desktop(
            &bin,
            &desktop,
            &self.home.join("desktop-log"),
            &self.home.join("clipboard"),
        );
    }

    /// Starts the server, its output appended to `server.log`, and waits
    /// for it to listen.
    fn launch(&mut self) {
        self.stand_ins();
        let settings = match self
            .extra_env
            .iter()
            .find(|(k, _)| k == "SWITCHBOARD_SETTINGS")
        {
            Some((_, file)) if !file.is_empty() => PathBuf::from(file),
            _ => self.home.join(".config/switchboard/settings.json"),
        };
        if !settings.exists() {
            write_settings_to(&settings, serde_json::json!({}));
        }
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.path().join("server.log"))
            .unwrap();
        let mut cmd = Command::new(&self.bin);
        cmd.arg("server")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        Self::env(&mut cmd, &self.home, &self.state);
        cmd.envs(self.extra_env.iter().map(|(k, v)| (k, v)));
        self.child = Some(cmd.spawn().expect("the server starts"));
        let deadline = deadline(Duration::from_secs(10));
        while UnixStream::connect(self.socket()).is_err() {
            assert!(
                Instant::now() < deadline,
                "the server didn't listen: {}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Asks the server to end, then makes sure it has.
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = UnixStream::connect(self.socket()).map(|mut s| {
                s.write_all(&proto::encode_client(&ClientMsg::Command {
                    args: vec!["kill-server".into()],
                    cwd: PathBuf::from("/"),
                    pane: None,
                    stdin: None,
                }))
            });
            // Waits for it to end, killing it only past the deadline, so a
            // slow exit on a busy machine isn't a server left dying on the
            // socket the next start uses.
            let deadline = deadline(Duration::from_secs(3));
            while Instant::now() < deadline && child.try_wait().ok().flatten().is_none() {
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// The environment every process of the test runs with: its own home and
    /// state, a plain shell with a prompt that is easy to find, and the
    /// home's `bin` first on the PATH, for the stand-ins for programs.
    fn env(cmd: &mut Command, home: &Path, state: &Path) {
        let path = std::env::var("PATH").unwrap_or_default();
        cmd.env("HOME", home)
            .env("PATH", format!("{}:{path}", home.join("bin").display()))
            .env("SWITCHBOARD_STATE_DIR", state)
            // The computer's own idle time isn't the test's: it goes by
            // what it typed.
            .env("SWITCHBOARD_PRESENCE", "unknown")
            // Nor its logind, which keeping a Linux computer awake asks.
            .env(
                "DBUS_SYSTEM_BUS_ADDRESS",
                "unix:path=/nonexistent/system_bus_socket",
            )
            .env("SHELL", "/bin/sh")
            .env("ENV", "")
            .env("PS1", "$ ")
            // M-p's restart builds no checkout, this repository least of all:
            // cargo gives a test build.rs's values for both, the real main
            // clone among them.
            .env("SWITCHBOARD_CHECKOUT", "")
            .env("SWITCHBOARD_MAIN_CLONE", "")
            .env_remove("SWITCHBOARD")
            .env_remove("SWITCHBOARD_PANE")
            .env_remove("SWITCHBOARD_SETTINGS")
            .env_remove("CLAUDE_CODE_PLUGIN_DIRS")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            // A file opened in your editor opens in the test's `vi`.
            .env_remove("VISUAL")
            .env_remove("EDITOR")
            // No token of the person running the tests reaches a stand-in
            // service, which would refuse it as GitHub refuses a wrong one;
            // a test gives its own (`start_with_env`).
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("LINEAR_API_KEY");
    }

    pub fn socket(&self) -> PathBuf {
        self.state.join("sock")
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("server.log")).unwrap_or_default()
    }

    /// `switchboard <args>` against this server: (code, stdout, stderr). A
    /// command the server runs (`commands::NAMES`) is asked of it over the
    /// socket from the test's own process, since the binary does nothing more
    /// for one and a full run would otherwise start thousands of processes,
    /// each inspected by a security agent where there is one; anything else
    /// is the binary run.
    pub fn run(&self, args: &[&str]) -> (i32, String, String) {
        let is_server_command = args
            .first()
            .is_some_and(|name| switchboard::mux::commands::NAMES.contains(name));
        if is_server_command {
            self.ask(args)
        } else {
            self.run_binary(args)
        }
    }

    /// A server command sent over the socket as the binary sends it, from
    /// the test's temporary directory and no pane: its code, what it printed
    /// and what it said went wrong, as the binary would exit and print them.
    pub fn ask(&self, args: &[&str]) -> (i32, String, String) {
        let msg = ClientMsg::Command {
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: self.dir.path().to_path_buf(),
            pane: None,
            stdin: None,
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code =
            switchboard::mux::socket::run_command_at(&self.socket(), &msg, &mut out, &mut err);
        (
            i32::from(code),
            String::from_utf8_lossy(&out).into_owned(),
            String::from_utf8_lossy(&err).into_owned(),
        )
    }

    /// The built `switchboard` run as a process with `args`: a program of
    /// its own, or a test of the process itself.
    pub fn run_binary(&self, args: &[&str]) -> (i32, String, String) {
        let mut cmd = Command::new(BIN);
        cmd.args(args).stdin(Stdio::null());
        Self::env(&mut cmd, &self.home, &self.state);
        let out = cmd.output().expect("switchboard runs");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// As `run`, failing the test unless it succeeds; gives its stdout.
    pub fn ok(&self, args: &[&str]) -> String {
        let (code, out, err) = self.run(args);
        assert_eq!(
            code,
            0,
            "switchboard {args:?} failed: {err}\nserver log: {}",
            self.log()
        );
        out
    }

    /// The server's state, as `switchboard list` gives it.
    pub fn list(&self) -> serde_json::Value {
        serde_json::from_str(&self.ok(&["list"])).expect("list gives JSON")
    }

    pub fn attach(&self, cols: u16, rows: u16) -> TestClient {
        TestClient::attach(&self.socket(), None, cols, rows)
    }

    /// A terminal attaching that names itself `term` (its `TERM`).
    pub fn attach_as(&self, term: &str, cols: u16, rows: u16) -> TestClient {
        TestClient::attach_with(&self.socket(), None, cols, rows, None, term)
    }

    /// The server's process's pid.
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// Whether the server's process is still running.
    pub fn running(&mut self) -> bool {
        self.child
            .as_mut()
            .is_some_and(|c| c.try_wait().ok().flatten().is_none())
    }

    /// Waits for the server to end by itself.
    pub fn wait_exit(&mut self, timeout: Duration) -> bool {
        let deadline = deadline(timeout);
        while Instant::now() < deadline {
            if !self.running() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }
}

impl Drop for TestServer {
    /// Ends the server, then fails the test if a stand-in on its PATH
    /// refused a call: the code made one the real program would refuse,
    /// however quietly it took the failure.
    fn drop(&mut self) {
        self.stop();
        if !std::thread::panicking() {
            stand_in::assert_none_refused(&self.home.join("bin"));
        }
    }
}

/// A client attached over the socket, keeping what it was sent as a screen
/// (`switchboard::ui::headless::Screen`, the headless client's).
pub struct TestClient {
    stream: UnixStream,
    rx: mpsc::Receiver<ServerMsg>,
    term: Screen,
    pub exit: Option<Option<String>>,
    /// Every byte of output, for tests that look for an escape sequence.
    pub raw: Vec<u8>,
}

impl TestClient {
    pub fn attach(socket: &Path, session: Option<&str>, cols: u16, rows: u16) -> TestClient {
        TestClient::attach_in(socket, session, cols, rows, None)
    }

    /// A terminal attaching from a directory, as `switchboard` run there.
    pub fn attach_in(
        socket: &Path,
        session: Option<&str>,
        cols: u16,
        rows: u16,
        cwd: Option<&Path>,
    ) -> TestClient {
        TestClient::attach_with(socket, session, cols, rows, cwd, "xterm-256color")
    }

    /// A terminal attaching from a directory, naming itself `term`.
    pub fn attach_with(
        socket: &Path,
        session: Option<&str>,
        cols: u16,
        rows: u16,
        cwd: Option<&Path>,
        term: &str,
    ) -> TestClient {
        let mut stream = UnixStream::connect(socket).expect("the server answers");
        stream
            .write_all(&proto::encode_client(&ClientMsg::Attach {
                session: session.map(str::to_string),
                cols,
                rows,
                term: term.into(),
                xpixel: 0,
                ypixel: 0,
                cwd: cwd.map(|d| d.to_string_lossy().into_owned()),
            }))
            .unwrap();
        let mut reader = stream.try_clone().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(Some(msg)) = proto::read_server(&mut reader) {
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });
        let term = Screen::new(cols, rows);
        TestClient {
            stream,
            rx,
            term,
            exit: None,
            raw: Vec::new(),
        }
    }

    pub fn send(&mut self, bytes: &[u8]) {
        self.stream
            .write_all(&proto::encode_client(&ClientMsg::Input(bytes.to_vec())))
            .unwrap();
    }

    /// Bytes written to the server as they are, a frame or a part of one.
    pub fn write_raw(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).unwrap();
    }

    pub fn type_text(&mut self, text: &str) {
        self.send(text.as_bytes());
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.resize_pixels(cols, rows, 0, 0);
    }

    /// A resize from a terminal that says its window's size in pixels too.
    pub fn resize_pixels(&mut self, cols: u16, rows: u16, xpixel: u16, ypixel: u16) {
        self.term.resize(cols, rows);
        self.stream
            .write_all(&proto::encode_client(&ClientMsg::Resize {
                cols,
                rows,
                xpixel,
                ypixel,
            }))
            .unwrap();
    }

    pub fn detach(&mut self) {
        let _ = self
            .stream
            .write_all(&proto::encode_client(&ClientMsg::Detach));
    }

    /// Reads whatever the server has sent, waiting up to `wait` for the first.
    fn pump(&mut self, wait: Duration) {
        let first = match self.rx.recv_timeout(wait) {
            Ok(m) => m,
            Err(_) => return,
        };
        let mut next = Some(first);
        while let Some(msg) = next {
            match msg {
                ServerMsg::Output(bytes) => {
                    self.raw.extend_from_slice(&bytes);
                    self.term.feed(&bytes);
                }
                ServerMsg::Exit { message } => self.exit = Some(message),
                ServerMsg::Reply { .. } => {}
            }
            next = self.rx.try_recv().ok();
        }
    }

    /// The screen as lines of text, a character a cell, trailing blanks
    /// trimmed.
    pub fn screen(&mut self) -> Vec<String> {
        self.pump(Duration::from_millis(50));
        self.term.lines()
    }

    /// The colour a cell's text is drawn in, when it is given as red, green
    /// and blue.
    pub fn fg_rgb(&mut self, line: usize, col: usize) -> Option<(u8, u8, u8)> {
        self.pump(Duration::from_millis(20));
        self.term.fg_rgb(line, col)
    }

    /// The first cell on the screen holding `c`, as (line, column).
    pub fn find_cell(&mut self, c: char) -> Option<(usize, usize)> {
        self.pump(Duration::from_millis(20));
        self.term.find_cell(c)
    }

    /// The combining marks a cell's character carries.
    pub fn marks(&mut self, line: usize, col: usize) -> Vec<char> {
        self.pump(Duration::from_millis(20));
        self.term
            .raw_cell(line, col)
            .zerowidth()
            .map(<[char]>::to_vec)
            .unwrap_or_default()
    }

    /// Waits until the screen satisfies `pred`, failing with the screen after `timeout`.
    pub fn wait_for(&mut self, what: &str, pred: impl Fn(&[String]) -> bool) -> Vec<String> {
        let deadline = deadline(Duration::from_secs(10));
        loop {
            let screen = self.screen();
            if pred(&screen) {
                return screen;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; the screen:\n{}",
                screen.join("\n")
            );
            self.pump(Duration::from_millis(50));
        }
    }

    /// Waits until some line of the screen contains `text`.
    pub fn wait_text(&mut self, text: &str) -> Vec<String> {
        self.wait_for(&format!("{text:?} on the screen"), |s| {
            s.iter().any(|l| l.contains(text))
        })
    }

    /// Waits for the server to tell this client to end.
    pub fn wait_exit(&mut self) -> Option<String> {
        let deadline = deadline(Duration::from_secs(10));
        while self.exit.is_none() {
            assert!(Instant::now() < deadline, "the client wasn't told to end");
            self.pump(Duration::from_millis(50));
        }
        self.exit.clone().flatten()
    }

    /// Waits until what the client was sent holds `needle`, failing with
    /// `what` after ten seconds.
    pub fn wait_sent(&mut self, what: &str, needle: &[u8]) {
        let deadline = deadline(Duration::from_secs(10));
        while !self.raw.windows(needle.len()).any(|w| w == needle) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.pump(Duration::from_millis(50));
        }
    }
}

impl TestServer {
    /// Has `pane` show the four-colour picture (`four_colours`), four cells
    /// by two, given as Notes gives one, each row of it after "Q7"; gives
    /// the line and column of its top left on the client's screen once the
    /// shell has printed it.
    pub fn show_picture(&self, client: &mut TestClient, pane: &str) -> (usize, usize) {
        let png = self.dir.path().join("four.png");
        four_colours(&png, 40);
        let file = self.dir.path().join("picture.out");
        std::fs::write(&file, picture_bytes(&png, 42, 4, 2, "Q7")).unwrap();
        self.ok(&[
            "send-keys",
            "-t",
            pane,
            "-l",
            &format!("cat {}; echo pic''-done", file.display()),
        ]);
        self.ok(&["send-keys", "-t", pane, "Enter"]);
        let screen = client.wait_text("pic-done");
        let line = screen
            .iter()
            .position(|l| l.contains("Q7"))
            .unwrap_or_else(|| panic!("the picture's line: {}", screen.join("\n")));
        // Columns, not bytes: the sidebar's line before it is three bytes.
        let before = &screen[line][..screen[line].find("Q7").unwrap()];
        (line, before.chars().count() + 2)
    }
}

/// What Notes writes to show a picture, into a file a pane can `cat`: the
/// PNG given by its path as kitty's virtual placement `id`, `cols` by
/// `rows` cells, then its placeholder cells, coloured with its id, each
/// row on a line of its own after `lead`.
pub fn picture_bytes(png: &Path, id: u32, cols: u16, rows: u16, lead: &str) -> Vec<u8> {
    use switchboard::notes::images::Picture;
    let mut out = format!(
        "\x1b_Ga=T,U=1,f=100,t=f,q=2,i={id},c={cols},r={rows};{}\x1b\\",
        switchboard::util::base64(png.to_string_lossy().as_bytes())
    );
    for row in 0..rows {
        out.push_str(&format!(
            "{lead}\x1b[38;2;{};{};{}m",
            (id >> 16) & 0xff,
            (id >> 8) & 0xff,
            id & 0xff
        ));
        for col in 0..cols {
            out.push_str(&Picture::cell(row, col));
        }
        out.push_str("\x1b[0m\r\n");
    }
    out.into_bytes()
}

/// Whether `needle` is anywhere in what a client was sent.
pub fn sent(raw: &[u8], needle: &[u8]) -> bool {
    raw.windows(needle.len()).any(|w| w == needle)
}

/// The sixel string a client was sent at a cell (0-based), up to its end.
pub fn sixel_at(raw: &[u8], line: usize, col: usize) -> Option<String> {
    let at = format!("\x1b[{};{}H\x1bP", line + 1, col + 1);
    let raw = String::from_utf8_lossy(raw);
    let start = raw.find(&at)? + at.len() - 2;
    let end = raw[start..].find("\x1b\\")? + start + 2;
    Some(raw[start..end].to_string())
}

/// Every sixel a client was sent, in order: the cell (0-based) the cursor
/// was moved to just before it, as the server always moves it, and the
/// size its raster attributes give in pixels, width and height.
pub fn sixels(raw: &[u8]) -> Vec<((usize, usize), (u32, u32))> {
    const START: &str = "\x1bP0;1;0q\"1;1;";
    let raw = String::from_utf8_lossy(raw);
    let mut out = Vec::new();
    let mut rest = &raw[..];
    while let Some(i) = rest.find(START) {
        let at = rest[..i]
            .rfind("\x1b[")
            .and_then(|j| {
                let (row, col) = rest[j + 2..i].strip_suffix('H')?.split_once(';')?;
                Some((
                    row.parse::<usize>().ok()?.checked_sub(1)?,
                    col.parse::<usize>().ok()?.checked_sub(1)?,
                ))
            })
            .unwrap_or_else(|| panic!("a sixel with no cursor move before it, at byte {i}"));
        rest = &rest[i + START.len()..];
        let size = rest
            .split('#')
            .next()
            .and_then(|s| s.split_once(';'))
            .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
            .unwrap_or_else(|| panic!("a sixel with no size, at byte {i}"));
        out.push((at, size));
    }
    out
}

/// A PNG of four colours, a quarter each (red, green, blue, yellow from
/// the top left), `size` pixels a side.
pub fn four_colours(file: &Path, size: u32) {
    image::RgbaImage::from_fn(size, size, |x, y| {
        let half = size / 2;
        image::Rgba(match (x < half, y < half) {
            (true, true) => [255, 0, 0, 255],
            (false, true) => [0, 255, 0, 255],
            (true, false) => [0, 0, 255, 255],
            (false, false) => [255, 255, 0, 255],
        })
    })
    .save(file)
    .expect("a picture written");
}
