//! `switchboard-diff` alone: the binary run in a terminal of its own (a
//! PTY), in a repository, with no Switchboard server, no settings file and
//! no Claude, under a home made for the test. It draws the branch's changes
//! against the default branch, keeps its files in its own folders, and
//! leaves nothing of a workspace behind.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

const BINARY: &str = env!("CARGO_BIN_EXE_switchboard-diff");

/// How long anything is waited for, on a computer however busy.
const PATIENCE: Duration = Duration::from_secs(60);

/// A folder for the test under `/tmp`, removed when it ends.
struct Tmp(PathBuf);

impl Tmp {
    fn new(name: &str) -> Tmp {
        let dir = PathBuf::from(format!("/tmp/sbd-{name}-{}", std::process::id()));
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
/// system's own programs on the PATH (git among them), no workspace to
/// reach, and settings that turn off everything the diff would ask Claude
/// for, so no `claude` is ever started, whatever the computer has.
fn env(home: &Path) -> Vec<(&'static str, String)> {
    let settings = home.join("diff-settings.json");
    std::fs::write(
        &settings,
        r#"{"diff_order": false, "diff_story": false, "diff_ask": false}"#,
    )
    .unwrap();
    vec![
        ("HOME", home.to_string_lossy().into_owned()),
        ("PATH", "/usr/bin:/bin".into()),
        (
            "SWITCHBOARD_SETTINGS",
            settings.to_string_lossy().into_owned(),
        ),
        ("TERM", "xterm-256color".into()),
        ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ("GIT_AUTHOR_NAME", "Test".into()),
        ("GIT_AUTHOR_EMAIL", "test@example.com".into()),
        ("GIT_COMMITTER_NAME", "Test".into()),
        ("GIT_COMMITTER_EMAIL", "test@example.com".into()),
    ]
}

fn git(dir: &Path, home: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_clear()
        .envs(env(home))
        .output()
        .expect("git runs");
    assert!(
        status.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

/// A repository on a branch that changes one file of main's and adds one.
fn repository(tmp: &Tmp) -> PathBuf {
    let (repo, home) = (tmp.0.join("repo"), tmp.home());
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &home, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("alpha.txt"), "one\ntwo\n").unwrap();
    git(&repo, &home, &["add", "."]);
    git(&repo, &home, &["commit", "-q", "-m", "Start"]);
    git(&repo, &home, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(repo.join("alpha.txt"), "one\nthree\n").unwrap();
    std::fs::write(repo.join("beta.txt"), "new\n").unwrap();
    git(&repo, &home, &["add", "."]);
    git(
        &repo,
        &home,
        &["commit", "-q", "-m", "Change alpha, add beta"],
    );
    repo
}

#[test]
fn it_shows_a_branch_s_changes_in_a_terminal_with_no_server() {
    let tmp = Tmp::new("alone");
    let home = tmp.home();
    let repo = repository(&tmp);

    // A cache folder left open to other users is tightened as it starts.
    {
        use std::os::unix::fs::PermissionsExt;
        let cache = home.join(".cache/switchboard-diff");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o755)).unwrap();
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
    command.arg(&repo);
    command.cwd(&repo);
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

    // Both files of the branch's change are listed, drawn by the diff itself.
    let mut drawn = String::new();
    let deadline = Instant::now() + PATIENCE;
    while !(drawn.contains("alpha.txt") && drawn.contains("beta.txt")) {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left.max(Duration::from_millis(1))) {
            Ok(bytes) => drawn.push_str(&String::from_utf8_lossy(&bytes)),
            Err(_) => panic!("the diff never listed the branch's files; it drew: {drawn:?}"),
        }
    }

    // q ends it, the terminal given back.
    writer.write_all(b"q").unwrap();
    writer.flush().unwrap();
    let deadline = Instant::now() + PATIENCE;
    let status = loop {
        match child.try_wait().unwrap() {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                panic!("q didn't end the diff");
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    assert!(status.success(), "{status:?}");

    // Its files are its own, and nothing of a workspace was made: no
    // server was started, asked or needed.
    assert!(
        home.join(".cache/switchboard-diff/diff-base").is_dir(),
        "the base it compared with is kept in its own cache"
    );
    // Its folders are its owner's alone, so what a story says of the
    // repository is no other user's to read.
    for own in [".cache/switchboard-diff", ".local/state/switchboard-diff"] {
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
fn it_names_itself_and_its_usage() {
    let tmp = Tmp::new("usage");
    let run = |args: &[&str]| {
        Command::new(BINARY)
            .args(args)
            .env_clear()
            .envs(env(&tmp.home()))
            .output()
            .expect("it runs")
    };
    let version = run(&["--version"]);
    assert!(version.status.success());
    assert!(
        String::from_utf8_lossy(&version.stdout).starts_with("switchboard-diff "),
        "{version:?}"
    );
    let bad = run(&["--no-such-flag"]);
    assert_eq!(bad.status.code(), Some(2));
    let said = String::from_utf8_lossy(&bad.stderr);
    assert!(said.starts_with("Usage: switchboard-diff "), "{said}");
    assert!(run(&["--help"]).status.success());
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
        "the diff never depends on the multiplexer"
    );
}
