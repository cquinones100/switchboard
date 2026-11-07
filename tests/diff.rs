//! M-d as someone uses it, against a server of the test's own: in a branch
//! of a throwaway repository (its origin a bare one beside it, so the base
//! is fetched without the network), M-d puts `switchboard diff` in the
//! pane's place, its story told by a stand-in claude that streams a canned
//! answer (test/diff-story.test.js's stand-in), the list headed by its
//! chapters; q ends the diff and the pane is back, its shell as it was.
//! And `switchboard diff --ahead` tells the same story with nothing drawn,
//! keeping it where M-d finds it, once per commit.

mod common;

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::{BIN, TestServer};
use serde_json::{Value, json};

/// Settings for the tests: no desktop notifications or tips, GitHub off
/// (so the base is origin's default branch, and nothing is told ahead).
fn settings(server: &TestServer) {
    let dir = server.home.join(".config/switchboard");
    std::fs::create_dir_all(&dir).unwrap();
    let all = json!({"notifications": false, "tips": false, "github": false, "linear": false});
    std::fs::write(dir.join("settings.json"), all.to_string()).unwrap();
}

/// Runs git in `dir` with an identity of its own.
fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// A repository whose `feature` branch changes two files since `main`, with
/// a bare origin: gives the clone.
fn repository(root: &Path) -> std::path::PathBuf {
    let seed = root.join("seed");
    std::fs::create_dir_all(seed.join("src")).unwrap();
    git(&seed, &["init", "-q"]);
    git(&seed, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    std::fs::write(seed.join("src/a.js"), "export const a = 1;\n").unwrap();
    std::fs::write(seed.join("src/b.js"), "export const b = 1;\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-q", "-m", "start"]);
    let origin = root.join("origin.git");
    git(
        root,
        &[
            "clone",
            "-q",
            "--bare",
            &seed.to_string_lossy(),
            &origin.to_string_lossy(),
        ],
    );
    let work = root.join("work");
    git(
        root,
        &[
            "clone",
            "-q",
            &origin.to_string_lossy(),
            &work.to_string_lossy(),
        ],
    );
    git(&work, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(work.join("src/a.js"), "export const a = 2;\n").unwrap();
    std::fs::write(work.join("src/b.js"), "export const b = 2;\n").unwrap();
    git(&work, &["commit", "-q", "-am", "Change both"]);
    work
}

/// A stand-in claude: reads what it is given, notes that it was asked,
/// and streams an answer a chapter at a time, then the whole of it.
fn stand_in_claude(home: &Path) {
    let bin = home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let pieces = [
        r#"{"chapters": [{"title": "Change the core", "why": "a changes.", "hunks": [{"id": "src/a.js#1", "note": "Check a."}]}"#,
        r#", {"title": "Follow it", "why": "b follows.", "hunks": ["src/b.js#1"]}]}"#,
    ];
    let mut lines: Vec<String> = pieces
        .iter()
        .map(|text| {
            json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}})
                .to_string()
        })
        .collect();
    lines.push(json!({"type": "result", "result": pieces.concat()}).to_string());
    let asked = home.join("claude-asked");
    let mut script = format!(
        "#!/bin/sh\ncat > /dev/null\necho asked >> '{}'\n",
        asked.display()
    );
    for line in lines {
        script.push_str(&format!("sleep 0.2\nprintf '%s\\n' '{line}'\n"));
    }
    let file = bin.join("claude");
    std::fs::write(&file, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn panes(server: &TestServer) -> Vec<Value> {
    server.list()["sessions"][0]["tabs"][0]["panes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn m_d_puts_the_diff_in_the_panes_place_and_q_puts_the_pane_back() {
    let server = TestServer::start();
    settings(&server);
    stand_in_claude(&server.home);
    let work = repository(server.dir.path());

    let mut client = server.attach(140, 36);
    client.wait_text("$");
    let first = panes(&server)[0]["id"].as_str().unwrap().to_string();
    client.type_text(&format!("cd {}\r", work.display()));
    wait_until("the shell to cd", || {
        panes(&server)[0]["cwd"]
            .as_str()
            .is_some_and(|d| d.ends_with("work"))
    });

    client.send(b"\x1bd");
    let screen = client.wait_text("1/2 Change the core");
    let row = screen
        .iter()
        .position(|l| l.contains("1/2 Change the core"))
        .unwrap();
    let later = screen
        .iter()
        .position(|l| l.contains("2/2 Follow it"))
        .unwrap();
    assert!(row < later, "the chapters in order: {screen:?}");
    client.wait_text("Check a.");
    let seen = client.wait_text("export const a = 2;");
    let row = seen
        .iter()
        .find(|l| l.contains("export const a = 2;"))
        .unwrap();
    assert!(
        row.contains("export const a = 1;"),
        "the old line beside the new: {row:?}"
    );
    let shown = panes(&server);
    assert_eq!(
        shown.len(),
        1,
        "in the pane's place, not beside it: {shown:?}"
    );
    assert_ne!(shown[0]["id"], first.as_str());
    assert_eq!(shown[0]["options"]["@in_place"], "diff");
    assert_eq!(shown[0]["options"]["@parked_pane"], first.as_str());
    assert!(
        std::fs::read_to_string(server.home.join("claude-asked")).is_ok(),
        "the story was asked of claude"
    );
    // Kept for the next time, under the diff's key.
    let kept = server.home.join(".cache/switchboard/diff-story");
    wait_until("the story kept", || {
        std::fs::read_dir(&kept)
            .map(|d| {
                d.flatten()
                    .any(|e| e.path().extension().is_some_and(|x| x == "txt"))
            })
            .unwrap_or(false)
    });

    client.type_text("q");
    wait_until("the pane back", || {
        let now = panes(&server);
        now.len() == 1 && now[0]["id"] == first.as_str()
    });
    let back = panes(&server);
    assert!(back[0]["options"].get("@pane_parked").is_none(), "{back:?}");
    client.type_text("echo pane-is-back\r");
    client.wait_text("pane-is-back");

    // M-d again opens the story kept, with no new ask.
    let asked = |home: &Path| {
        std::fs::read_to_string(home.join("claude-asked"))
            .unwrap_or_default()
            .lines()
            .count()
    };
    let before = asked(&server.home);
    client.send(b"\x1bd");
    client.wait_text("1/2 Change the core");
    assert_eq!(asked(&server.home), before, "the story came from the cache");
    // M-d pressed in the diff closes it too.
    client.send(b"\x1bd");
    wait_until("the pane back again", || {
        panes(&server)[0]["id"] == first.as_str()
    });
}

#[test]
fn the_story_is_told_ahead_once_per_commit_where_m_d_finds_it() {
    let dir = common::TempDir::new("sbd");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    stand_in_claude(&home);
    let work = repository(dir.path());
    let path = std::env::var("PATH").unwrap_or_default();
    let ahead = || {
        Command::new(BIN)
            .args(["diff", "--ahead"])
            .arg(&work)
            .env("HOME", &home)
            .env("PATH", format!("{}:{path}", home.join("bin").display()))
            .env("SWITCHBOARD_STATE_DIR", dir.path().join("s"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env_remove("SWITCHBOARD")
            .env_remove("SWITCHBOARD_PANE")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    std::fs::create_dir_all(home.join(".config/switchboard")).unwrap();
    std::fs::write(
        home.join(".config/switchboard/settings.json"),
        json!({"github": false}).to_string(),
    )
    .unwrap();
    let out = ahead();
    assert!(
        out.status.success(),
        "told: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let kept = home.join(".cache/switchboard/diff-story");
    let stories: Vec<_> = std::fs::read_dir(&kept).unwrap().flatten().collect();
    assert_eq!(stories.len(), 1, "one story kept, no progress left behind");
    let asked = std::fs::read_to_string(home.join("claude-asked")).unwrap();
    assert_eq!(asked.lines().count(), 1);
    assert!(ahead().status.success());
    assert_eq!(
        std::fs::read_to_string(home.join("claude-asked"))
            .unwrap()
            .lines()
            .count(),
        1,
        "once per commit"
    );
}
