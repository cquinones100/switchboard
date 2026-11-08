//! Working a note continuously (src/autopilot) as someone turns it on,
//! against a server of the test's own: M-p's "Work the note continuously"
//! asked from a pane in a throwaway repository, the note picked and the
//! counts kept as they were, and the server opens a background tab for the
//! note's first item running `wtn`, marked as the mode's, then one for the
//! next, until the note has nothing left, which it says; its menu's "Land
//! without asking" marks its tabs after a yes; and it follows the note by
//! its stable ID once it is renamed outside Notes. A stand-in
//! `claude` in the test home's `bin` says where it ran, and the notes are in
//! a directory of the test's own, so nothing reads your real ones.

mod common;

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use common::{TempDir, TestServer};
use serde_json::{Value, json};

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn wait(what: &str, server: &TestServer, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(40);
    while !done() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}\nserver log: {}",
            server.log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The tabs the mode opened, by the item each was opened for.
fn opened(server: &TestServer) -> Vec<Value> {
    server.list()["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|s| s["tabs"].as_array().cloned().unwrap_or_default())
        .filter(|t| t["options"]["@autopilot"].is_string())
        .collect()
}

#[test]
fn turning_it_on_opens_a_tab_for_the_notes_next_item_then_the_next_and_says_when_none_is_left() {
    let notes = TempDir::new("sbn");
    let note = notes.path().join("Work.md");
    std::fs::write(&note, "# Work\n\n- [ ] First thing\n- [ ] Second thing\n").unwrap();
    // A note with nothing left to tick, behind "More notes…".
    std::fs::write(notes.path().join("Done.md"), "# Done\n\n- [x] All of it\n").unwrap();
    // Changed long enough ago to have settled.
    std::fs::File::options()
        .write(true)
        .open(&note)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(120))
        .unwrap();
    let server = TestServer::start_with_env(&[("NOTES_DIR", notes.path().to_str().unwrap())]);
    let config = server.home.join(".config/switchboard");
    std::fs::create_dir_all(&config).unwrap();
    // Whether you're at the computer is the desktop's to say, which a test
    // can't be sure of, so it doesn't pause while you're away.
    let settings = json!({"notifications": false, "tips": false, "github": false, "linear": false, "pause_when_away": false});
    std::fs::write(config.join("settings.json"), settings.to_string()).unwrap();
    let bin = server.home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let claude = bin.join("claude");
    std::fs::write(
        &claude,
        "#!/bin/sh\necho \"$PWD\" >> \"$HOME/claude-ran\"\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    let repo = server.home.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    let repo = repo.canonicalize().unwrap();

    let mut client = server.attach(120, 40);
    client.wait_text("$");
    server.ok(&["new-tab", "-c", repo.to_str().unwrap()]);
    client.type_text("\x1bp");
    client.wait_text("action>");
    client.type_text("continuously");
    client.wait_text("Work the note continuously");
    client.type_text("\r");
    let screen = client.wait_text("More notes…");
    assert!(
        screen
            .iter()
            .any(|l| l.contains("Work which note continuously?"))
    );
    assert!(
        !screen.iter().any(|l| l.contains("Done")),
        "a note with nothing unticked is behind More notes…"
    );
    client.type_text("\r");
    client.wait_text("Tabs working at once");
    client.type_text("\r");
    client.wait_text("Tabs open at most");
    client.type_text("\r");
    client.wait_text("Spend limit");
    client.type_text("\r");
    client.wait_text("Working Work: 3 at a time, up to 5 tabs, in ~/repo");
    client.wait_text("AUTO Work");

    let mode: Value = serde_json::from_str(
        &std::fs::read_to_string(server.state.join("autopilot.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(mode["on"], true);
    assert_eq!(mode["note"], "Work.md");
    assert_eq!(mode["repo"], repo.to_str().unwrap());
    assert_eq!(
        (mode["slots"].clone(), mode["cap"].clone()),
        (json!(3), json!(5))
    );
    // The note by its stable ID too, which it was given as the mode was set
    // up, so a rename anywhere is followed.
    let note_id = switchboard::notes::id::of_file(&note).expect("the note given an ID");
    assert_eq!(mode["noteId"], note_id.as_str());

    wait("a tab for the first item", &server, || {
        opened(&server)
            .iter()
            .any(|t| t["options"]["@autopilot_item"] == "First thing")
    });
    let first = opened(&server).remove(0);
    assert_eq!(first["options"]["@tab_name"], "First thing");
    assert_eq!(first["options"]["@tab_note"], "Work.md");
    assert_eq!(
        first["options"]["@autopilot_note"],
        note_id.as_str(),
        "its item held in its own note alone"
    );
    assert_eq!(first["current"], false, "opened in the background");
    let tree = repo.join(".claude/worktrees/note-first-thing");
    wait("Claude in the item's worktree", &server, || {
        std::fs::read_to_string(server.home.join("claude-ran"))
            .is_ok_and(|t| t.lines().any(|l| Path::new(l) == tree))
    });
    wait("the tab marked as in its worktree", &server, || {
        opened(&server)
            .iter()
            .any(|t| t["options"]["@autopilot_in"] == "1")
    });

    wait("a tab for the next item", &server, || {
        opened(&server)
            .iter()
            .any(|t| t["options"]["@autopilot_item"] == "Second thing")
    });
    wait("it to say the note has nothing left", &server, || {
        std::fs::read_to_string(server.state.join("autopilot.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .is_some_and(|m| m["said"] == "Nothing left to take in Work")
    });
    assert_eq!(opened(&server).len(), 2, "one tab an item, each once");

    // Its menu's "Land without asking", after a yes naming what it skips
    // and keeps: its tabs not landed yet are marked, and its line says so.
    // Mission control is open still, its line above the cards: k reaches
    // it, and Enter opens its menu.
    client.type_text("kkkk");
    client.type_text("\r");
    client.wait_text("Land without asking");
    client.type_text("jjj\r");
    let screen = client.wait_text("without asking to push?");
    assert!(
        screen
            .iter()
            .any(|l| l.contains("Kept: the tests, two clean review rounds")),
        "{}",
        screen.join("\n")
    );
    client.type_text("\r");
    wait("its tabs marked to land without asking", &server, || {
        std::fs::read_to_string(server.state.join("autopilot.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .is_some_and(|m| m["landWithoutAsking"] == true)
            && opened(&server)
                .iter()
                .all(|t| t["options"]["@autopilot_land"] == "1")
    });
    client.wait_text("lands on its own");

    // Renamed outside Notes: followed by its ID, its new path kept.
    std::fs::create_dir_all(notes.path().join("Projects")).unwrap();
    std::fs::rename(&note, notes.path().join("Projects/Renamed.md")).unwrap();
    wait("the note followed to its new path", &server, || {
        std::fs::read_to_string(server.state.join("autopilot.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .is_some_and(|m| m["note"] == "Projects/Renamed.md" && m["noteId"] == note_id.as_str())
    });
}
