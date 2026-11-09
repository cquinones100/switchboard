//! Autopilot (src/autopilot) as someone turns it on,
//! against a server of the test's own: M-p's "Autopilot: work a note"
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

use common::stand_in::Claude;
use common::{TempDir, TestClient, TestServer};
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

/// A server with a note "Work" in a notes directory of its own, a stand-in
/// claude and a repository landing straight on its branch, the mode set up
/// from M-p in a tab there with the counts kept as they were; `more`
/// settings added to the test's own.
fn set_up(
    more: Value,
) -> (
    TempDir,
    std::path::PathBuf,
    TestServer,
    TestClient,
    std::path::PathBuf,
) {
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
    let mut settings = json!({"notifications": false, "tips": false, "github": false, "linear": false, "pause_when_away": false});
    settings
        .as_object_mut()
        .unwrap()
        .extend(more.as_object().unwrap().clone());
    std::fs::write(config.join("settings.json"), settings.to_string()).unwrap();
    Claude::new()
        .answer("echo \"$PWD\" >> \"$HOME/claude-ran\"")
        .install(&server.home.join("bin"));
    let repo = server.home.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    git(&repo, &["branch", "team/next"]);
    let repo = repo.canonicalize().unwrap();

    // Wide enough for the Autopilot view's board beside the sidebar.
    let cols = if more.get("ux2").is_some() { 170 } else { 120 };
    let mut client = server.attach(cols, 40);
    client.wait_text("$");
    if more.get("ux2").is_some() {
        client.wait_text("M-c Claude");
    }
    server.ok(&["new-tab", "-c", repo.to_str().unwrap()]);
    client.type_text("\x1bp");
    client.wait_text("Actions");
    client.type_text("autopilot");
    client.wait_text("Autopilot: work a note");
    client.type_text("\r");
    // A repository landing through a PR, as one with no GitHub and no
    // repos.json does, is refused.
    client.wait_text("Autopilot lands only straight onto a branch for now");
    std::fs::write(
        config.join("repos.json"),
        json!({ repo.join(".git").to_str().unwrap(): { "landing": "direct" } }).to_string(),
    )
    .unwrap();
    client.type_text("\x1b\x1b");
    client.type_text("\x1bp");
    client.wait_text("Actions");
    client.type_text("autopilot");
    client.wait_text("Autopilot: work a note");
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
    // Where it lands: the default branch first, so Enter keeps it.
    let screen = client.wait_text("Another branch…");
    assert!(
        screen
            .iter()
            .any(|l| l.contains("main (the default branch)"))
    );
    assert!(screen.iter().any(|l| l.contains("team/next")));
    client.type_text("\r");
    client.wait_text("Tabs working at once");
    client.type_text("\r");
    client.wait_text("Tabs open at most");
    client.type_text("\r");
    client.wait_text("Spend limit");
    client.type_text("\r");
    client.wait_text(
        "Working Work: 3 at a time, up to 5 tabs, in ~/repo, landing on the default branch",
    );
    (notes, note, server, client, repo)
}

#[test]
fn with_ux2_it_has_its_own_view_in_place_of_the_line_leading_the_tabs() {
    let (_notes, _note, server, mut client, _repo) = set_up(json!({ "ux2": true }));
    // M-p's command opened mission control on the Autopilot board, its
    // heading saying what the line said.
    let screen = client.wait_text("Waiting on you");
    assert!(
        screen
            .iter()
            .any(|l| l.contains("AUTO Work") && l.contains("of 5 open · asks before landing")),
        "{screen:#?}"
    );
    client.type_text("\x1b");
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("Waiting on you"))
    });
    // The Tabs view leads with the cards alone, the Autopilot view among
    // the views while the mode is on.
    client.type_text("\x1b ");
    client.wait_text("Autopilot");
    std::thread::sleep(Duration::from_millis(300));
    let screen = client.screen();
    assert!(!screen.iter().any(|l| l.contains("AUTO ")), "{screen:#?}");
    drop(server);
}

#[test]
fn with_ux2_its_command_opens_the_board_and_m_or_its_button_opens_the_menu() {
    let (_notes, _note, server, mut client, _repo) = set_up(json!({ "ux2": true }));
    client.wait_text("Waiting on you");
    client.type_text("\x1b");
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("Waiting on you"))
    });
    // M-p's command with the mode on shows the board, no menu over it.
    client.type_text("\x1bp");
    client.wait_text("Actions");
    client.type_text("autopilot");
    client.wait_text("Autopilot: work a note");
    client.type_text("\r");
    client.wait_text("☰ Menu");
    std::thread::sleep(Duration::from_millis(300));
    let screen = client.screen();
    assert!(
        !screen
            .iter()
            .any(|l| l.contains("Pause: open no more tabs")),
        "{screen:#?}"
    );
    // m opens it.
    client.type_text("m");
    client.wait_text("Pause: open no more tabs");
    client.type_text("\x1b");
    client.wait_for("menu closed", |s| {
        !s.iter().any(|l| l.contains("Pause: open no more tabs"))
    });
    // So does a click on the heading's button.
    let (row, col) = client.find_cell('☰').unwrap();
    client.send(
        format!(
            "\x1b[<0;{};{}M\x1b[<0;{};{}m",
            col + 1,
            row + 1,
            col + 1,
            row + 1
        )
        .as_bytes(),
    );
    client.wait_text("Pause: open no more tabs");
    drop(server);
}

#[test]
fn turning_it_on_opens_a_tab_for_the_notes_next_item_then_the_next_until_none_is_left() {
    let (notes, note, server, mut client, repo) = set_up(json!({}));
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
    assert_eq!(mode["target"], "", "the default branch");

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
    client.wait_text("nothing left to take");
    assert_eq!(opened(&server).len(), 2, "one tab an item, each once");
    let mode: Value = serde_json::from_str(
        &std::fs::read_to_string(server.state.join("autopilot.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        mode["said"], "",
        "with its tabs still open, no notification says it has stopped"
    );

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

    // Its menu's "Hands-free", after a yes naming what it decides and what
    // still stops: its tabs are marked, and its line says so.
    client.type_text("\r");
    client.wait_text("Hands-free: decide for me");
    client.type_text("jjjj\r");
    let screen = client.wait_text("decide for you, hands-free?");
    assert!(
        screen
            .iter()
            .any(|l| l.contains("Still asked: what your rules or a skill say")),
        "{}",
        screen.join("\n")
    );
    client.type_text("\r");
    wait("its tabs marked hands-free", &server, || {
        std::fs::read_to_string(server.state.join("autopilot.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .is_some_and(|m| m["handsFree"] == true && m["landWithoutAsking"] == true)
            && opened(&server)
                .iter()
                .all(|t| t["options"]["@autopilot_handsfree"] == "1")
    });
    client.wait_text("hands-free");

    // Its menu's "Change the target branch…": the tabs it opens from now
    // land on the branch picked, which its line names.
    client.type_text("\r");
    client.wait_text("Change the target branch…");
    client.type_text("jjjjj\r");
    client.wait_text("Another branch…");
    client.type_text("team/next\r");
    wait("the target kept", &server, || {
        std::fs::read_to_string(server.state.join("autopilot.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .is_some_and(|m| m["target"] == "team/next")
    });
    client.wait_text("→ team/next");

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
