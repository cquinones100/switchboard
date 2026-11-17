//! `switchboard-notes` alone: the binary run in a terminal of its own (a
//! PTY), on a notes directory, with no Switchboard server and no settings
//! file, under a home made for the test. It opens a note of `~/Notes`,
//! answers `switchboard-notes shown` from its own socket, keeps its files
//! in its own folders, and leaves nothing of a workspace behind.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

const BINARY: &str = env!("CARGO_BIN_EXE_switchboard-notes");

/// How long anything is waited for, on a computer however busy.
const PATIENCE: Duration = Duration::from_secs(60);

/// A folder for the test under `/tmp` (short, since the app's socket is in
/// it and a Unix socket's path can't be long), removed when it ends.
struct Tmp(PathBuf);

impl Tmp {
    fn new(name: &str) -> Tmp {
        let dir = PathBuf::from(format!("/tmp/sbn-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("home")).unwrap();
        Tmp(dir)
    }

    fn home(&self) -> PathBuf {
        self.0.join("home")
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The environment of everything the test runs: the home made for it, the
/// system's own programs on the PATH, no workspace to reach, no notes
/// directory named (so `~/Notes` is the one), and no settings file.
fn env(home: &Path) -> Vec<(&'static str, String)> {
    vec![
        ("HOME", home.to_string_lossy().into_owned()),
        ("PATH", "/usr/bin:/bin".into()),
        (
            "SWITCHBOARD_SETTINGS",
            home.join("no-settings.json").to_string_lossy().into_owned(),
        ),
        ("TERM", "xterm-256color".into()),
        // As in a pane of a workspace, which it never asks: alone, the pane
        // it is in changes nothing, its socket included.
        (
            "SWITCHBOARD",
            home.join("no-server.sock").to_string_lossy().into_owned(),
        ),
        ("SWITCHBOARD_PANE", "%7".into()),
    ]
}

fn run(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(BINARY)
        .args(args)
        .env_clear()
        .envs(env(home))
        .output()
        .expect("it runs")
}

#[test]
fn it_opens_a_note_in_a_terminal_with_no_server() {
    let tmp = Tmp::new("alone");
    let home = tmp.home();
    let notes = home.join("Notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("Groceries.md"),
        "# Groceries\n\n- [ ] apples\n- [x] bread\n",
    )
    .unwrap();

    // A state folder left open to other users is tightened as it starts.
    {
        use std::os::unix::fs::PermissionsExt;
        let state = home.join(".local/state/switchboard-notes");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 40,
            cols: 140,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("a PTY");
    let mut command = CommandBuilder::new(BINARY);
    command.cwd(&home);
    command.env_clear();
    for (key, value) in env(&home) {
        command.env(key, value);
    }
    let mut child = pty.slave.spawn_command(command).expect("it starts");
    drop(pty.slave);
    let mut reader = pty.master.try_clone_reader().unwrap();
    let mut writer = pty.master.take_writer().unwrap();
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    // The note is drawn as Notes shows it, by the app itself: its title,
    // and its checklist's items with their boxes in place of the markup.
    let mut drawn = String::new();
    let deadline = Instant::now() + PATIENCE;
    let whole = |drawn: &str| {
        ["Groceries", "☐", "apples", "☑", "bread"]
            .iter()
            .all(|part| drawn.contains(part))
    };
    while !whole(&drawn) {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left.max(Duration::from_millis(1))) {
            Ok(bytes) => drawn.push_str(&String::from_utf8_lossy(&bytes)),
            Err(_) => panic!("Notes never drew the note; it drew: {drawn:?}"),
        }
    }

    // The running app answers a second `switchboard-notes` from a socket of
    // its own, with no server between them.
    let shown = run(&home, &["shown"]);
    assert!(shown.status.success(), "{shown:?}");
    assert_eq!(
        String::from_utf8_lossy(&shown.stdout).trim(),
        "Groceries.md"
    );
    assert!(
        home.join(".local/state/switchboard-notes/notes.sock")
            .exists(),
        "its socket is in its own state folder"
    );

    // :q ends it, the terminal given back.
    writer.write_all(b":q\r").unwrap();
    writer.flush().unwrap();
    let deadline = Instant::now() + PATIENCE;
    let status = loop {
        match child.try_wait().unwrap() {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                panic!(":q didn't end Notes");
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    assert!(status.success(), "{status:?}");

    // The note is as it was: nothing typed, nothing saved over it but the
    // stable ID the app gives a note that has none, its last line.
    let text = std::fs::read_to_string(notes.join("Groceries.md")).unwrap();
    assert!(
        text.starts_with("# Groceries\n\n- [ ] apples\n- [x] bread\n"),
        "{text:?}"
    );

    // Its folders are its owner's alone, and nothing of a workspace was
    // made: no server was started, asked or needed.
    for own in [".local/state/switchboard-notes", ".cache/switchboard-notes"] {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(home.join(own))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "{own}");
    }
    for workspace in [
        ".local/state/switchboard",
        ".cache/switchboard",
        ".config/switchboard",
    ] {
        assert!(!home.join(workspace).exists(), "{workspace} was made");
    }
}

#[test]
fn it_names_itself_and_its_usage_and_asks_only_an_app_that_runs() {
    let tmp = Tmp::new("usage");
    let home = tmp.home();
    let version = run(&home, &["--version"]);
    assert!(version.status.success());
    assert!(
        String::from_utf8_lossy(&version.stdout).starts_with("switchboard-notes "),
        "{version:?}"
    );
    let bad = run(&home, &["--no-such-flag"]);
    assert_eq!(bad.status.code(), Some(2));
    let said = String::from_utf8_lossy(&bad.stderr);
    assert!(said.starts_with("Usage: switchboard-notes "), "{said}");
    assert!(run(&home, &["--help"]).status.success());
    // With no app running, there is none to ask, and it says so by its
    // own name.
    let shown = run(&home, &["shown"]);
    assert!(!shown.status.success());
    assert!(
        String::from_utf8_lossy(&shown.stderr).starts_with("switchboard-notes: "),
        "{shown:?}"
    );
    // A note outside the notes directory is never opened.
    let outside = run(&home, &["open", "../secret.md"]);
    assert_eq!(outside.status.code(), Some(2));
}

#[test]
fn of_the_workspace_s_crates_it_depends_on_the_api_and_the_drawing_alone() {
    let manifest =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    let mut by_path: Vec<&str> = manifest
        .lines()
        .filter(|line| !line.trim_start().starts_with('#') && line.contains("path = \"../"))
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    by_path.sort_unstable();
    assert_eq!(by_path, ["switchboard-plugin-api", "switchboard-ui"]);
    assert!(
        !manifest
            .lines()
            .any(|line| line.starts_with("switchboard = ")),
        "Notes never depends on the multiplexer"
    );
}
