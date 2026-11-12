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

use crate::common;

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
    let deadline = common::deadline(std::time::Duration::from_secs(40));
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
    common::write_settings(&server.home, settings);
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
    // Named for changing or stopping the mode while it is on.
    client.wait_text("Autopilot: work a note: change or stop working");
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
fn with_ux2_the_boards_side_panel_shows_the_chosen_items_plan_under_its_stages() {
    let (notes, note, server, mut client, _repo) = set_up(json!({ "ux2": true }));
    client.wait_text("Waiting on you");
    // The first item's plan, in the plans folder beside the note, linked
    // under it; the second item links none.
    std::fs::create_dir_all(notes.path().join("Plans")).unwrap();
    std::fs::write(
        notes.path().join("Plans/Plan - First thing.md"),
        "# Plan - First thing\n\nItem: \"First thing\"\n\n## Today\n\nNothing shows the plan.\n\n## Steps\n\n1. Draw it beside the board.\n",
    )
    .unwrap();
    let text = std::fs::read_to_string(&note).unwrap();
    std::fs::write(
        &note,
        text.replace(
            "- [ ] First thing\n",
            "- [ ] First thing\n  Plan: [[Plan - First thing]]\n",
        ),
    )
    .unwrap();
    // Wide enough for the pane beside the board's columns.
    client.resize(230, 40);
    // Down and up the board's cards until the pane beside it says `text`.
    let mut seek = |text: &str| {
        for step in 0..80 {
            let screen = client.screen();
            if screen.iter().any(|l| l.contains(text)) {
                return screen;
            }
            client.type_text(if (step / 2) % 2 == 0 { "j" } else { "k" });
            std::thread::sleep(Duration::from_millis(250));
        }
        panic!("{text} never shown: {:#?}", client.screen());
    };
    let screen = seek("Plan - First thing");
    let line_of = |s: &str| {
        screen
            .iter()
            .position(|l| l.contains(s))
            .unwrap_or_else(|| panic!("{s} in {screen:#?}"))
    };
    // Under the card's stages, without the plan's own heading and Item line.
    assert!(line_of("leak check") < line_of("Plan - First thing"));
    assert!(line_of("Plan - First thing") < line_of("Nothing shows the plan."));
    assert!(line_of("Nothing shows the plan.") < line_of("1. Draw it beside the board."));
    assert!(
        !screen.iter().any(|l| l.contains("Item: \"First thing\"")),
        "{screen:#?}"
    );
    // A card whose item links no plan says so.
    seek("No plan");
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

/// A Claude Code hook in `pane`, as its Claude would send it.
fn hook(server: &TestServer, pane: &str, event: &str) {
    use std::io::Write;
    let mut child = Command::new(common::BIN)
        .args(["hook", event])
        .env("HOME", &server.home)
        .env("SWITCHBOARD_STATE_DIR", &server.state)
        .env("SWITCHBOARD", server.socket())
        .env("SWITCHBOARD_PANE", pane)
        .env_remove("SWITCHBOARD_SETTINGS")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(b"{}");
    assert!(child.wait().unwrap().success(), "hook {event}");
}

#[test]
fn a_landed_item_stays_under_landed_once_its_tab_is_yours_and_once_it_is_closed() {
    let (_notes, _note, server, mut client, repo) = set_up(json!({ "ux2": true }));
    client.wait_text("Waiting on you");
    let first = |server: &TestServer| {
        opened(server)
            .into_iter()
            .find(|t| t["options"]["@autopilot_item"] == "First thing")
    };
    wait("the first item's tab in its worktree", &server, || {
        first(&server).is_some_and(|t| t["options"]["@autopilot_in"] == "1")
    });
    let tab = first(&server).unwrap();
    let tab_id = tab["id"].as_str().unwrap().to_string();
    let pane = tab["panes"][0]["id"].as_str().unwrap().to_string();

    // queue-and-clean hands the tab back to the main clone once it lands.
    server.ok(&[
        "respawn-pane",
        "-k",
        "-t",
        &pane,
        "-c",
        repo.to_str().unwrap(),
    ]);
    wait("the tab marked landed", &server, || {
        first(&server).is_some_and(|t| t["options"]["@autopilot_left"].is_string())
    });
    client.wait_text("landed, back");
    let mode: Value = serde_json::from_str(
        &std::fs::read_to_string(server.state.join("autopilot.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(mode["landed"][0]["item"], "First thing", "{mode}");

    // A prompt typed in it since makes it yours: it keeps its item, and the
    // board keeps it under Landed.
    // Its Claude, resumed in the pane, is known by its session file.
    let pid = first(&server).unwrap()["panes"][0]["pid"].as_u64().unwrap();
    let sessions = server.home.join(".claude/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join(format!("{pid}.json")),
        json!({ "pid": pid, "entrypoint": "cli", "sessionId": "s-1" }).to_string(),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    hook(&server, &pane, "UserPromptSubmit");
    wait("the tab made yours", &server, || {
        first(&server).is_some_and(|t| t["options"]["@autopilot_yours"] == "1")
    });
    let tab = first(&server).unwrap();
    assert!(tab["options"]["@autopilot_land"].is_null());
    std::thread::sleep(Duration::from_millis(500));
    let screen = client.screen();
    assert!(
        screen.iter().any(|l| l.contains("landed, back")),
        "{screen:#?}"
    );

    // Closed, its landing is listed still, with the commit it landed.
    let head = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .unwrap();
    let landed = format!("landed {}", String::from_utf8_lossy(&head.stdout).trim());
    server.ok(&["kill-tab", "-t", &tab_id]);
    let screen = client.wait_text("╭ closed");
    assert!(
        screen.iter().any(|l| l.contains("First thing"))
            && screen.iter().any(|l| l.contains(&landed)),
        "{screen:#?}"
    );
}

/// Its menu opened with `m` on the board, the option starting with
/// `label` chosen by moving down to it.
fn choose(client: &mut TestClient, label: &str) {
    client.type_text("m");
    client.wait_text("Pause: open no more tabs");
    for _ in 0..20 {
        let screen = client.screen();
        if screen.iter().any(|l| l.contains('›') && l.contains(label)) {
            client.type_text("\r");
            return;
        }
        client.type_text("j");
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("no {label} in its menu: {:#?}", client.screen());
}

/// What autopilot.json holds now.
fn mode(server: &TestServer) -> Value {
    serde_json::from_str(&std::fs::read_to_string(server.state.join("autopilot.json")).unwrap())
        .unwrap()
}

#[test]
fn its_menu_sets_each_count_alone_and_changing_its_note_keeps_them() {
    let (_notes, _note, server, mut client, _repo) = set_up(json!({ "ux2": true }));
    client.wait_text("☰ Menu");
    let before = mode(&server);

    choose(&mut client, "Tabs working at once: 3…");
    client.wait_text("How many of its tabs work at once");
    client.type_text("\x7f4\r");
    client.wait_text("Tabs working at once: 4");
    let now = mode(&server);
    assert_eq!(now["slots"], 4);
    for kept in ["note", "noteId", "target", "cap", "spendLimit", "repo"] {
        assert_eq!(now[kept], before[kept], "{kept} kept");
    }

    // A cap below the slots comes back as the slots.
    choose(&mut client, "Most tabs open: 5…");
    client.wait_text("The most of its tabs open at once");
    client.type_text("\x7f2\r");
    client.wait_text("Most tabs open: 4, as many as work at once");
    assert_eq!(mode(&server)["cap"], 4);

    // What it said of a limit passed goes with a change of the limit.
    let mut said = mode(&server);
    said["said"] = json!("Stopped opening tabs: today's Claude spend passed $5");
    std::fs::write(server.state.join("autopilot.json"), said.to_string()).unwrap();
    choose(&mut client, "Spend limit: none…");
    client.wait_text("Open no more tabs once today's Claude spend passes");
    client.type_text("20\r");
    client.wait_text("Spend limit: $20 a day");
    let now = mode(&server);
    assert_eq!(now["spendLimit"], 20);
    assert_eq!(now["said"], "", "what it said of the limit passed goes");

    // "Change its note…" asks the note and the branch, the counts kept.
    choose(&mut client, "Change its note…");
    client.wait_text("Work which note continuously?");
    client.type_text("\r");
    client.wait_text("Another branch…");
    client.type_text("\r");
    let screen = client.wait_text("Working Work: 4 at a time, up to 4 tabs");
    assert!(
        !screen.iter().any(|l| l.contains("Tabs working at once")),
        "{screen:#?}"
    );
    let now = mode(&server);
    assert_eq!(
        (
            now["slots"].clone(),
            now["cap"].clone(),
            now["spendLimit"].clone()
        ),
        (json!(4), json!(4), json!(20))
    );
}

/// What the stand-in claude was last started with, a line an argument.
fn claude_args(home: &Path) -> String {
    std::fs::read_to_string(home.join("claude-args")).unwrap_or_default()
}

/// `wtn --item <line>` on the note "Work", through its shell function as a
/// tab runs it, in `repo`, from the pane given (none, outside a pane).
fn wtn(server: &TestServer, notes: &Path, repo: &Path, pane: Option<&str>, more: &[&str]) {
    let out = wtn_output(server, notes, repo, pane, more);
    assert!(
        out.status.success(),
        "wtn {more:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// What `wtn` gives, as above, succeeding or not.
fn wtn_output(
    server: &TestServer,
    notes: &Path,
    repo: &Path,
    pane: Option<&str>,
    more: &[&str],
) -> std::process::Output {
    let path = std::env::var("PATH").unwrap_or_default();
    let mut cmd = Command::new("bash");
    cmd.args([
        "-c",
        &format!(
            "eval \"$('{}' worktree-from-note init bash)\"; wtn \"$@\"",
            common::BIN
        ),
        "wtn",
        "--note",
        "Work",
    ])
    .args(more)
    .current_dir(repo)
    .env("HOME", &server.home)
    .env(
        "PATH",
        format!("{}:{path}", server.home.join("bin").display()),
    )
    .env("SWITCHBOARD_STATE_DIR", &server.state)
    .env("NOTES_DIR", notes)
    .env("GIT_CONFIG_NOSYSTEM", "1")
    .env_remove("SWITCHBOARD")
    .env_remove("SWITCHBOARD_PANE")
    .env_remove("SWITCHBOARD_SETTINGS")
    .env_remove("CLAUDE_CODE_PLUGIN_DIRS")
    .env_remove("WT_PROMPT")
    .env_remove("WTN_NOTE");
    if let Some(pane) = pane {
        cmd.env("SWITCHBOARD", server.socket())
            .env("SWITCHBOARD_PANE", pane);
    }
    cmd.output().unwrap()
}

#[test]
fn an_item_wtn_refuses_in_its_tab_is_skipped_with_why_and_the_tab_closed() {
    let notes = TempDir::new("sbn");
    std::fs::write(notes.path().join("Work.md"), "# Work\n\n- [ ] Taken\n").unwrap();
    let server = TestServer::start_with_env(&[("NOTES_DIR", notes.path().to_str().unwrap())]);
    common::write_settings(
        &server.home,
        json!({"notifications": false, "tips": false, "github": false, "linear": false}),
    );
    Claude::new().install(&server.home.join("bin"));
    let repo = server.home.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    // The item's branch claimed by other words.
    git(
        &repo,
        &[
            "config",
            "branch.note/taken.wtnItem",
            "n-0123456789ab: Taken!",
        ],
    );
    let repo = repo.canonicalize().unwrap();

    let mut client = server.attach(100, 30);
    client.wait_text("$");
    server.ok(&["new-tab", "-c", repo.to_str().unwrap()]);
    let tab = server.list()["sessions"][0]["tabs"][1].clone();
    let id = tab["id"].as_str().unwrap().to_string();
    let pane = tab["panes"][0]["id"].as_str().unwrap().to_string();
    for (option, value) in [
        ("@autopilot", "1"),
        ("@autopilot_item", "Taken"),
        ("@autopilot_note", "n-ffffffffffff"),
    ] {
        server.ok(&["set", "-t", &id, option, value]);
    }
    let out = wtn_output(&server, notes.path(), &repo, Some(&pane), &["--item", "3"]);
    assert!(!out.status.success());

    wait("the tab closed", &server, || opened(&server).is_empty());
    let mode = mode(&server);
    assert_eq!(
        mode["skipped"],
        json!([{ "note": "n-ffffffffffff", "text": "Taken" }]),
        "the item skipped in its note"
    );
    let said = mode["said"].as_str().unwrap();
    assert!(
        said.starts_with(
            "Skipped Taken: wtn refused it, note/taken is for another item: \"Taken!\""
        ),
        "{said}"
    );
}

#[test]
fn its_tab_starts_claude_with_the_landings_one_push_allowed_and_no_other_tab_does() {
    let notes = TempDir::new("sbn");
    std::fs::write(
        notes.path().join("Work.md"),
        "# Work\n\n- [ ] One\n- [ ] Two\n- [ ] Three\n- [ ] Four\n- [ ] Five\n- [ ] Six\n",
    )
    .unwrap();
    let server = TestServer::start_with_env(&[("NOTES_DIR", notes.path().to_str().unwrap())]);
    let config = server.home.join(".config/switchboard");
    std::fs::create_dir_all(&config).unwrap();
    let quiet = json!({"notifications": false, "tips": false, "github": false, "linear": false});
    common::write_settings(&server.home, quiet.clone());
    Claude::new()
        .answer("printf '%s\\n' \"$@\" > \"$HOME/claude-args\"")
        .install(&server.home.join("bin"));
    // A repository with its main and a branch `feature` on origin, and a
    // branch `local` that is on no remote.
    let origin = server.home.join("origin.git");
    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "-q", "--bare"]);
    let repo = server.home.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "origin", "main", "main:feature"]);
    git(&repo, &["fetch", "-q", "origin"]);
    git(
        &repo,
        &["branch", "-q", "--track", "feature", "origin/feature"],
    );
    git(&repo, &["branch", "-q", "local"]);
    let repo = repo.canonicalize().unwrap();
    let lands = |way: &str| {
        std::fs::write(
            config.join("repos.json"),
            json!({ repo.join(".git").to_str().unwrap(): { "landing": way } }).to_string(),
        )
        .unwrap();
    };
    lands("direct");

    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let tab = server.list()["sessions"][0]["tabs"][0].clone();
    let pane = tab["panes"][0]["id"].as_str().unwrap().to_string();
    let allowed = |branch: &str| {
        format!(
            "--settings\n{{\"permissions\":{{\"allow\":[\"Bash(git push origin HEAD:{branch})\"]}}}}\nWork on this checklist item"
        )
    };

    // A tab autopilot didn't open, and a run outside any pane: no rule.
    wtn(&server, notes.path(), &repo, Some(&pane), &["--item", "3"]);
    let args = claude_args(&server.home);
    assert!(args.starts_with("Work on this checklist item"), "{args}");
    wtn(&server, notes.path(), &repo, None, &["--item", "4"]);
    assert!(!claude_args(&server.home).contains("--settings"));

    // Autopilot's tab: the push onto origin's default branch, and nothing
    // else, allowed for the session.
    server.ok(&["set", "-t", tab["id"].as_str().unwrap(), "@autopilot", "1"]);
    wtn(&server, notes.path(), &repo, Some(&pane), &["--item", "5"]);
    let args = claude_args(&server.home);
    assert!(args.starts_with(&allowed("main")), "{args}");

    // Landing on a target: the remote branch it tracks; a target on no
    // remote lands by moving the local branch, which needs no rule.
    wtn(
        &server,
        notes.path(),
        &repo,
        Some(&pane),
        &["--item", "6", "--target", "feature"],
    );
    let args = claude_args(&server.home);
    assert!(args.starts_with(&allowed("feature")), "{args}");
    wtn(
        &server,
        notes.path(),
        &repo,
        Some(&pane),
        &["--item", "7", "--target", "local"],
    );
    assert!(!claude_args(&server.home).contains("--settings"));

    // A repository landing through a PR, and the setting off: no rule.
    lands("pr");
    wtn(&server, notes.path(), &repo, Some(&pane), &["--item", "8"]);
    assert!(!claude_args(&server.home).contains("--settings"));
    lands("direct");
    let mut off = quiet;
    off["autopilot_push_rule"] = json!(false);
    common::write_settings(&server.home, off);
    wtn(&server, notes.path(), &repo, Some(&pane), &["--item", "3"]);
    let args = claude_args(&server.home);
    assert!(args.starts_with("Work on this checklist item"), "{args}");
}
