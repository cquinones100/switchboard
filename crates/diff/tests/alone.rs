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
    // Written once: a test that changes them keeps what it wrote.
    if !settings.exists() {
        std::fs::write(
            &settings,
            r#"{"diff_order": false, "diff_story": false, "diff_ask": false}"#,
        )
        .unwrap();
    }
    vec![
        ("HOME", home.to_string_lossy().into_owned()),
        ("PATH", "/usr/bin:/bin".into()),
        (
            "SWITCHBOARD_DIFF_SETTINGS",
            settings.to_string_lossy().into_owned(),
        ),
        ("TERM", "xterm-256color".into()),
        // No look for a newer release, whatever the build: only a test
        // with a stand-in for GitHub turns it on.
        ("SWITCHBOARD_UPDATES", "off".into()),
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

/// What sets a terminal's mouse pointer to a shape, by its CSS name.
fn pointer(name: &str) -> String {
    format!("\x1b]22;{name}\x1b\\")
}

#[test]
fn alone_it_sets_the_pointer_in_a_terminal_that_takes_it_and_puts_the_beam_back() {
    let tmp = Tmp::new("pointer");
    let home = tmp.home();
    let repo = repository(&tmp);
    let mut diff = Running::start(&repo, &home, &[("TERM", "xterm-ghostty".into())]);
    diff.wait_drawn("listed the branch's files", &["alpha.txt", "beta.txt"]);
    // The mouse down the list's rows: a hand over a file's.
    for row in 1..=12 {
        diff.send(format!("\x1b[<35;4;{row}M").as_bytes());
    }
    diff.wait_drawn(
        "made the pointer a hand over a file's row",
        &[&pointer("pointer")],
    );
    let drawn = diff.quit_reading();
    let (beam, hand) = (
        drawn.rfind(&pointer("text")),
        drawn.rfind(&pointer("pointer")),
    );
    assert!(beam > hand, "the beam is put back as it quits: {drawn:?}");
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

    // Both files of the branch's change are listed, drawn by the diff itself.
    let mut diff = Running::start(&repo, &home, &[]);
    diff.wait_drawn("listed the branch's files", &["alpha.txt", "beta.txt"]);
    // A terminal that doesn't say it takes the pointer's shapes is sent none.
    for row in 1..=12 {
        diff.send(format!("\x1b[<35;4;{row}M").as_bytes());
    }

    // q ends it, the terminal given back.
    let drawn = diff.quit_reading();
    assert!(!drawn.contains("\x1b]22;"), "{drawn:?}");

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

/// The diff running in a terminal of its own, and what it has drawn.
struct Running {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    rx: mpsc::Receiver<Vec<u8>>,
    drawn: String,
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

impl Running {
    /// `switchboard-diff` on the repository, with `extra` in its environment.
    fn start(repo: &Path, home: &Path, extra: &[(&str, String)]) -> Running {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 50,
                cols: 160,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("a PTY");
        let mut command = CommandBuilder::new(BINARY);
        command.arg(repo);
        command.cwd(repo);
        command.env_clear();
        for (key, value) in env(home) {
            command.env(key, value);
        }
        for (key, value) in extra {
            command.env(key, value);
        }
        let child = pty.slave.spawn_command(command).expect("it starts");
        drop(pty.slave);
        let mut reader = pty.master.try_clone_reader().unwrap();
        let writer = pty.master.take_writer().unwrap();
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Running {
            child,
            writer,
            rx,
            drawn: String::new(),
            _master: pty.master,
        }
    }

    /// Waits until it has drawn `text` since the last wait.
    fn wait_for(&mut self, text: &str) {
        let deadline = Instant::now() + PATIENCE;
        while !self.drawn.contains(text) {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left.max(Duration::from_millis(1))) {
                Ok(bytes) => self.drawn.push_str(&String::from_utf8_lossy(&bytes)),
                Err(_) => panic!("it never drew {text:?}; it drew: {:?}", self.drawn),
            }
        }
        self.drawn.clear();
    }

    fn send(&mut self, keys: &[u8]) {
        self.writer.write_all(keys).unwrap();
        self.writer.flush().unwrap();
    }

    /// Waits until what it has written holds each of `words`, keeping it.
    fn wait_drawn(&mut self, what: &str, words: &[&str]) {
        let deadline = Instant::now() + PATIENCE;
        while !words.iter().all(|w| self.drawn.contains(w)) {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left.max(Duration::from_millis(1))) {
                Ok(bytes) => self.drawn.push_str(&String::from_utf8_lossy(&bytes)),
                Err(_) => panic!("the diff never {what}; it drew: {:?}", self.drawn),
            }
        }
    }

    /// q, the program ended, and everything it wrote, read until the
    /// terminal's reader ends with it.
    fn quit_reading(mut self) -> String {
        self.send(b"q");
        let deadline = Instant::now() + PATIENCE;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left.max(Duration::from_millis(1))) {
                Ok(bytes) => self.drawn.push_str(&String::from_utf8_lossy(&bytes)),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let _ = self.child.kill();
                    panic!("q didn't end the diff");
                }
            }
        }
        let status = self.child.wait().unwrap();
        assert!(status.success(), "{status:?}");
        std::mem::take(&mut self.drawn)
    }

    /// q, and the program ended.
    fn quit(mut self) {
        self.send(b"q");
        let deadline = Instant::now() + PATIENCE;
        loop {
            match self.child.try_wait().unwrap() {
                Some(status) => {
                    assert!(status.success(), "{status:?}");
                    return;
                }
                None if Instant::now() >= deadline => {
                    let _ = self.child.kill();
                    panic!("q didn't end the diff");
                }
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn alone_m_m_opens_its_settings_and_a_change_is_kept_in_the_settings_file() {
    let tmp = Tmp::new("settings");
    let home = tmp.home();
    let repo = repository(&tmp);
    let mut diff = Running::start(&repo, &home, &[]);
    // Its own header, and the row of buttons with Settings in it.
    diff.wait_for("alpha.txt");
    // M-m: the settings view, its options by their names.
    diff.send(b"\x1bm");
    diff.wait_for("Diff file list wide");
    // The file list's width, turned off: written at once, beside what the
    // file held, the version before kept.
    let settings = home.join("diff-settings.json");
    diff.send(b"jjjjjjj\r");
    let read = || -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap_or_default())
            .unwrap_or_default()
    };
    wait_until("the setting was never written", || {
        read()["diff_list_wide"] == false
    });
    assert_eq!(read()["diff_story"], false, "the file's other keys stay");
    assert!(home.join("diff-settings.json.bak").exists());
    diff.wait_for("changed");
    // Enter again: back to its default, the key left out.
    diff.send(b"\r");
    wait_until("the setting was never put back", || {
        read().get("diff_list_wide").is_none()
    });
    // Esc goes back to the diff, and q ends it.
    diff.send(b"\x1b");
    diff.wait_for("alpha.txt");
    diff.quit();
}

#[cfg(feature = "net")]
/// A stand-in for GitHub's releases on 127.0.0.1: the list, and each file
/// of the one release of the diff it holds, which a test may change.
struct Releases {
    api: String,
    lists: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    files: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>>,
    archive: String,
}

#[cfg(feature = "net")]
impl Releases {
    fn start() -> Releases {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let target = switchboard_plugin_api::release::this_target().expect("a target");
        let archive = format!("switchboard-diff-9.9.0-{target}.tar.gz");
        let asset = |name: String| {
            serde_json::json!({
                "name": name,
                "browser_download_url": format!("http://127.0.0.1:{port}/dl/{name}"),
            })
        };
        let list = serde_json::json!([
            // Switchboard's own release, which is none of the diff's.
            {"tag_name": "v99.0.0", "assets": []},
            {
                "tag_name": "diff-v9.9.0",
                "published_at": "2026-11-01T12:00:00Z",
                "assets": [
                    asset(archive.clone()),
                    asset(format!("{archive}.sha256")),
                    asset(format!("{archive}.sig")),
                ],
            },
        ])
        .to_string();
        let lists = Arc::new(AtomicUsize::new(0));
        let files = Arc::new(Mutex::new(
            std::collections::HashMap::<String, Vec<u8>>::new(),
        ));
        let (counted, served) = (lists.clone(), files.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut asked = Vec::new();
                let mut buf = [0u8; 1024];
                while !asked.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => asked.extend_from_slice(&buf[..n]),
                    }
                }
                let asked = String::from_utf8_lossy(&asked).to_string();
                let path = asked.split_whitespace().nth(1).unwrap_or("").to_string();
                let body = if path.starts_with("/repos/") {
                    counted.fetch_add(1, Ordering::SeqCst);
                    Some(list.clone().into_bytes())
                } else {
                    path.strip_prefix("/dl/")
                        .and_then(|name| served.lock().unwrap().get(name).cloned())
                };
                let (status, body) = match body {
                    Some(body) => ("200 OK", body),
                    None => ("404 Not Found", Vec::new()),
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        Releases {
            api: format!("http://127.0.0.1:{port}"),
            lists,
            files,
            archive,
        }
    }

    fn listed(&self) -> usize {
        self.lists.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn put(&self, name: &str, bytes: Vec<u8>) {
        self.files.lock().unwrap().insert(name.to_string(), bytes);
    }
}

// With the diff's HTTPS client, its `net` feature.
#[cfg(feature = "net")]
#[test]
fn a_newer_release_is_said_in_the_header_and_asked_for_once_a_day() {
    let tmp = Tmp::new("newer");
    let home = tmp.home();
    let repo = repository(&tmp);
    let releases = Releases::start();
    let looks = [
        ("SWITCHBOARD_GITHUB_API", releases.api.clone()),
        ("SWITCHBOARD_UPDATES", "on".to_string()),
    ];

    // The first run asks GitHub, after it has drawn, and says what is out.
    let mut diff = Running::start(&repo, &home, &looks);
    diff.wait_for("9.9.0 is out · switchboard-diff update");
    // Its settings view has a row to update.
    diff.send(b"\x1bm");
    diff.wait_for("Update to 9.9.0");
    diff.send(b"\x1b");
    diff.wait_for("alpha.txt");
    diff.quit();
    assert_eq!(releases.listed(), 1);
    let kept = home.join(".cache/switchboard-diff/update.json");
    assert!(kept.exists(), "what the look found is kept in its cache");

    // The same day, it says so again from what it kept, asking nothing.
    let mut diff = Running::start(&repo, &home, &looks);
    diff.wait_for("9.9.0 is out");
    diff.quit();
    assert_eq!(releases.listed(), 1);

    // With the check off nothing is asked, whatever was kept.
    std::fs::remove_file(&kept).unwrap();
    std::fs::write(
        home.join("diff-settings.json"),
        r#"{"diff_order": false, "diff_story": false, "diff_ask": false, "diff_update_check": false}"#,
    )
    .unwrap();
    let mut diff = Running::start(&repo, &home, &looks);
    diff.wait_for("alpha.txt");
    diff.quit();
    assert_eq!(releases.listed(), 1);
    assert!(!kept.exists());
}

// With the diff's HTTPS client, its `net` feature.
#[cfg(feature = "net")]
#[test]
fn update_refuses_an_archive_not_signed_with_a_release_key_and_installs_one_that_is() {
    use ssh_key::{HashAlg, LineEnding, PrivateKey, private::Ed25519Keypair};
    use switchboard_plugin_api::release::{SIGNATURE_NAMESPACE, sha256_hex};

    let tmp = Tmp::new("update");
    let home = tmp.home();
    let releases = Releases::start();
    // The installed copy: this build, in a folder of its own.
    let bin = tmp.0.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let installed = bin.join("switchboard-diff");
    std::fs::copy(BINARY, &installed).unwrap();
    let before = std::fs::read(&installed).unwrap();

    // The release: an archive holding a switchboard-diff that says 9.9.0.
    let archive = {
        let script = b"#!/bin/sh\necho 'switchboard-diff 9.9.0'\n";
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        {
            let mut tar = tar::Builder::new(&mut gz);
            let mut header = tar::Header::new_gnu();
            header.set_size(script.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(
                &mut header,
                "switchboard-diff-9.9.0/switchboard-diff",
                &script[..],
            )
            .unwrap();
            tar.finish().unwrap();
        }
        gz.finish().unwrap()
    };
    let name = releases.archive.clone();
    let key = PrivateKey::from(Ed25519Keypair::from_seed(&[3; 32]));
    let other = PrivateKey::from(Ed25519Keypair::from_seed(&[4; 32]));
    let signed_by = |key: &PrivateKey| {
        key.sign(SIGNATURE_NAMESPACE, HashAlg::Sha512, &archive)
            .unwrap()
            .to_pem(LineEnding::LF)
            .unwrap()
            .into_bytes()
    };
    releases.put(&name, archive.clone());
    releases.put(
        &format!("{name}.sha256"),
        format!("{}  {name}\n", sha256_hex(&archive)).into_bytes(),
    );
    releases.put(&format!("{name}.sig"), signed_by(&other));

    let run = |args: &[&str]| {
        Command::new(&installed)
            .args(args)
            .env_clear()
            .envs(env(&home))
            .env("SWITCHBOARD_GITHUB_API", &releases.api)
            // A debug build takes a release key from here too, the tests'.
            .env(
                "SWITCHBOARD_RELEASE_KEY",
                key.public_key().to_openssh().unwrap(),
            )
            .output()
            .expect("it runs")
    };
    let said = |out: &std::process::Output| {
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    };

    // Asked only whether one is out, it says so and changes nothing.
    let check = run(&["update", "--check"]);
    assert!(check.status.success(), "{}", said(&check));
    assert!(said(&check).contains("switchboard-diff 9.9.0 is out"));
    // A build from a clone isn't replaced unasked.
    let unforced = run(&["update"]);
    assert!(unforced.status.success(), "{}", said(&unforced));
    assert!(
        said(&unforced).contains("built from a clone"),
        "{}",
        said(&unforced)
    );
    assert_eq!(std::fs::read(&installed).unwrap(), before);

    // Signed by another key than a release key: refused, nothing changed.
    let refused = run(&["update", "--force"]);
    assert!(!refused.status.success(), "{}", said(&refused));
    assert!(
        said(&refused).contains("isn't signed by"),
        "{}",
        said(&refused)
    );
    assert_eq!(std::fs::read(&installed).unwrap(), before);

    // A checksum that isn't the archive's: refused too.
    releases.put(&format!("{name}.sig"), signed_by(&key));
    releases.put(
        &format!("{name}.sha256"),
        format!("{}  {name}\n", sha256_hex(b"another archive")).into_bytes(),
    );
    let refused = run(&["update", "--force"]);
    assert!(!refused.status.success(), "{}", said(&refused));
    assert!(
        said(&refused).contains("isn't the file"),
        "{}",
        said(&refused)
    );
    assert_eq!(std::fs::read(&installed).unwrap(), before);

    // Signed by the release key, with its checksum: put in the binary's
    // place, which then says the new version.
    releases.put(
        &format!("{name}.sha256"),
        format!("{}  {name}\n", sha256_hex(&archive)).into_bytes(),
    );
    let updated = run(&["update", "--force"]);
    assert!(updated.status.success(), "{}", said(&updated));
    assert!(
        said(&updated).contains("to switchboard-diff 9.9.0"),
        "{}",
        said(&updated)
    );
    let version = Command::new(&installed).arg("--version").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        "switchboard-diff 9.9.0"
    );
}
