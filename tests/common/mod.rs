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

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;
use switchboard::mux::proto::{self, ClientMsg, ServerMsg};

pub const BIN: &str = env!("CARGO_BIN_EXE_switchboard");

static NEXT: AtomicU32 = AtomicU32::new(0);

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
}

impl TestServer {
    pub fn start() -> TestServer {
        let dir = TempDir::new("sbt");
        let home = dir.path().join("home");
        let state = dir.path().join("s");
        std::fs::create_dir_all(&home).unwrap();
        let log = std::fs::File::create(dir.path().join("server.log")).unwrap();
        let mut cmd = Command::new(BIN);
        cmd.arg("server")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        Self::env(&mut cmd, &home, &state);
        let child = cmd.spawn().expect("the server starts");
        let server = TestServer {
            dir,
            home,
            state,
            child: Some(child),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while UnixStream::connect(server.socket()).is_err() {
            assert!(
                Instant::now() < deadline,
                "the server didn't listen: {}",
                server.log()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        server
    }

    /// The environment every process of the test runs with: its own home and
    /// state, a plain shell with a prompt that is easy to find.
    fn env(cmd: &mut Command, home: &Path, state: &Path) {
        cmd.env("HOME", home)
            .env("SWITCHBOARD_STATE_DIR", state)
            .env("SHELL", "/bin/sh")
            .env("ENV", "")
            .env("PS1", "$ ")
            .env_remove("SWITCHBOARD")
            .env_remove("SWITCHBOARD_PANE")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE");
    }

    pub fn socket(&self) -> PathBuf {
        self.state.join("sock")
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("server.log")).unwrap_or_default()
    }

    /// `switchboard <args>` against this server: (code, stdout, stderr).
    pub fn run(&self, args: &[&str]) -> (i32, String, String) {
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

    /// Whether the server's process is still running.
    pub fn running(&mut self) -> bool {
        self.child
            .as_mut()
            .is_some_and(|c| c.try_wait().ok().flatten().is_none())
    }

    /// Waits for the server to end by itself.
    pub fn wait_exit(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
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
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = UnixStream::connect(self.socket()).map(|mut s| {
                s.write_all(&proto::encode_client(&ClientMsg::Command {
                    args: vec!["kill-server".into()],
                    cwd: PathBuf::from("/"),
                    pane: None,
                    stdin: None,
                }))
            });
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline && child.try_wait().ok().flatten().is_none() {
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct Size(usize, usize);

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.1
    }
    fn screen_lines(&self) -> usize {
        self.1
    }
    fn columns(&self) -> usize {
        self.0
    }
}

/// A client attached over the socket, keeping what it was sent as a screen.
pub struct TestClient {
    stream: UnixStream,
    rx: mpsc::Receiver<ServerMsg>,
    term: Term<VoidListener>,
    parser: Processor,
    pub exit: Option<Option<String>>,
    /// Every byte of output, for tests that look for an escape sequence.
    pub raw: Vec<u8>,
}

impl TestClient {
    pub fn attach(socket: &Path, session: Option<&str>, cols: u16, rows: u16) -> TestClient {
        let mut stream = UnixStream::connect(socket).expect("the server answers");
        stream
            .write_all(&proto::encode_client(&ClientMsg::Attach {
                session: session.map(str::to_string),
                cols,
                rows,
                term: "xterm-256color".into(),
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
        let term = Term::new(
            Config::default(),
            &Size(cols as usize, rows as usize),
            VoidListener,
        );
        TestClient {
            stream,
            rx,
            term,
            parser: Processor::new(),
            exit: None,
            raw: Vec::new(),
        }
    }

    pub fn send(&mut self, bytes: &[u8]) {
        self.stream
            .write_all(&proto::encode_client(&ClientMsg::Input(bytes.to_vec())))
            .unwrap();
    }

    pub fn type_text(&mut self, text: &str) {
        self.send(text.as_bytes());
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.term.resize(Size(cols as usize, rows as usize));
        self.stream
            .write_all(&proto::encode_client(&ClientMsg::Resize { cols, rows }))
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
                    self.parser.advance(&mut self.term, &bytes);
                }
                ServerMsg::Exit { message } => self.exit = Some(message),
                ServerMsg::Reply { .. } => {}
            }
            next = self.rx.try_recv().ok();
        }
    }

    /// The screen as lines of text, trailing blanks trimmed.
    pub fn screen(&mut self) -> Vec<String> {
        self.pump(Duration::from_millis(50));
        let grid = self.term.grid();
        (0..grid.screen_lines())
            .map(|y| {
                let row = &grid[Line(y as i32)];
                let line: String = (0..grid.columns()).map(|x| row[Column(x)].c).collect();
                line.trim_end().to_string()
            })
            .collect()
    }

    /// Waits until the screen satisfies `pred`, failing with the screen after `timeout`.
    pub fn wait_for(&mut self, what: &str, pred: impl Fn(&[String]) -> bool) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(10);
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
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.exit.is_none() {
            assert!(Instant::now() < deadline, "the client wasn't told to end");
            self.pump(Duration::from_millis(50));
        }
        self.exit.clone().flatten()
    }
}
