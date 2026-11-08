//! M-d as someone uses it, against a server of the test's own: in a branch
//! of a throwaway repository (its origin a bare one beside it, so the base
//! is fetched without the network), M-d puts `switchboard diff` in the
//! pane's place, its story told by a stand-in claude that streams a canned
//! answer (test/diff-story.test.js's stand-in), the list headed by its
//! chapters; q ends the diff and the pane is back, its shell as it was.
//! And `switchboard diff --ahead` tells the same story with nothing drawn,
//! keeping it where M-d finds it, once per commit. lazygit (a stand-in)
//! takes a pane's place the same way, from M-p's command and from mission
//! control's menu on a tab, as the diff does from that menu. M-p's
//! working tree command shows the changes not yet committed, those not
//! staged (a file git doesn't know yet among them) and those staged. `c`
//! in the diff goes through the branch's commits one at a time. M-p's
//! "Clear this tab's diff cache" removes the story, order and PR base kept
//! for the tab's branch after a yes, keeping a story being told. With the
//! diff_uncommitted setting on, M-d runs to the working tree: what isn't
//! committed is marked ● and listed in a last chapter after the commits'
//! story, edits made while the diff is open come in on R, and a branch with
//! nothing to show gets a box saying so in place of the diff.

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
        "#!/bin/sh\ncat > /dev/null\necho \"asked $*\" >> '{}'\n",
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

/// How many stories the stand-in was asked for, told by the story's own
/// prompt: the server asks its claude other things meanwhile (a connector's
/// lookup, which streams too, a few seconds after it starts), and Haiku's
/// order of the files, so counting every ask, or every streamed one, counts
/// whatever a slow run lets in.
fn stories_asked(home: &Path) -> usize {
    std::fs::read_to_string(home.join("claude-asked"))
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains("telling it as a short story"))
        .count()
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
    assert_eq!(
        stories_asked(&server.home),
        1,
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

    // ga's box takes a question pasted whole, and Shift-Enter adds a line:
    // the workspace passes both on as the diff asked for them (bracketed
    // paste, keys told apart).
    client.type_text("ga");
    client.wait_text("Ask about this diff");
    client.send(b"\x1b[200~First line\nSecond line\x1b[201~");
    client.send(b"\x1b[13;2u");
    client.type_text("Third");
    let shown = client.wait_text("Third");
    for line in ["First line", "Second line"] {
        assert!(shown.iter().any(|l| l.contains(line)), "{line}: {shown:?}");
    }
    client.send(b"\x1b");
    client.wait_for("the box closed", |s| {
        !s.iter().any(|l| l.contains("Ask about this diff"))
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

    // M-d again opens the story kept, with no new ask for it.
    let before = stories_asked(&server.home);
    client.send(b"\x1bd");
    client.wait_text("1/2 Change the core");
    assert_eq!(
        stories_asked(&server.home),
        before,
        "the story came from the cache; asked:\n{}",
        std::fs::read_to_string(server.home.join("claude-asked")).unwrap_or_default()
    );
    // M-d pressed in the diff closes it too.
    client.send(b"\x1bd");
    wait_until("the pane back again", || {
        panes(&server)[0]["id"] == first.as_str()
    });
}

/// Whether a row of the screen holding `name` (the list's row for it) is
/// marked ● as not committed yet.
fn marked(screen: &[String], name: &str) -> bool {
    screen
        .iter()
        .filter(|l| l.contains(name))
        .any(|l| l.contains('●'))
}

#[test]
fn m_d_lists_whats_not_committed_marked_in_a_last_chapter_and_r_reads_it_again() {
    let server = TestServer::start();
    settings(&server);
    stand_in_claude(&server.home);
    let work = repository(server.dir.path());
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    client.type_text(&format!("cd {}\r", work.display()));
    wait_until("the shell to cd", || {
        panes(&server)[0]["cwd"]
            .as_str()
            .is_some_and(|d| d.ends_with("work"))
    });

    // A clean branch: its commits' story, nothing marked.
    client.send(b"\x1bd");
    let screen = client.wait_text("2/2 Follow it");
    assert!(!screen.iter().any(|l| l.contains('●')), "{screen:?}");
    // Edits made while the diff is open come in on R: b.js changed again
    // and a file git doesn't know yet, in a last chapter, each marked.
    std::fs::write(work.join("src/b.js"), "export const b = 3;\n").unwrap();
    std::fs::write(work.join("src/c.js"), "export const c = 1;\n").unwrap();
    client.type_text("R");
    let screen = client.wait_text("3/3 Not committed yet");
    let at = |text: &str| screen.iter().position(|l| l.contains(text)).unwrap();
    assert!(
        at("1/3 Change the core") < at("2/3 Follow it")
            && at("2/3 Follow it") < at("3/3 Not committed yet"),
        "the commits' chapters, then the last: {screen:?}"
    );
    assert!(at("3/3 Not committed yet") < at("c.js"), "{screen:?}");
    assert!(
        at("b.js") < at("3/3 Not committed yet"),
        "a file in both keeps its committed chapter: {screen:?}"
    );
    assert!(
        marked(&screen, "b.js") && marked(&screen, "c.js"),
        "{screen:?}"
    );
    assert!(!marked(&screen, "a.js"), "{screen:?}");
    // The commits' story kept: editing tells no new one.
    assert_eq!(stories_asked(&server.home), 1);
    // The new file, the file on disk on its side, its bar saying so.
    client.type_text("G");
    let screen = client.wait_text("export const c = 1;");
    assert!(
        screen[0].contains("c.js") && screen[0].contains("● uncommitted"),
        "{:?}",
        screen[0]
    );
    client.type_text("q");
    wait_until("the pane back", || {
        panes(&server)[0]["options"].get("@in_place").is_none()
    });
}

#[test]
fn m_d_says_when_there_is_nothing_to_show_and_shows_uncommitted_changes_alone() {
    let server = TestServer::start();
    settings(&server);
    stand_in_claude(&server.home);
    let work = repository(server.dir.path());
    // No commits past the base, and nothing changed.
    git(&work, &["reset", "-q", "--hard", "origin/main"]);
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    let first = panes(&server)[0]["id"].as_str().unwrap().to_string();
    client.type_text(&format!("cd {}\r", work.display()));
    wait_until("the shell to cd", || {
        panes(&server)[0]["cwd"]
            .as_str()
            .is_some_and(|d| d.ends_with("work"))
    });
    let back = |server: &TestServer| {
        wait_until("the pane back", || {
            let now = panes(server);
            now.len() == 1 && now[0]["id"] == first.as_str()
        });
    };

    client.send(b"\x1bd");
    client.wait_text("Nothing to show: no commits past origin/main and no uncommitted changes");
    back(&server);
    client.type_text("\x1b");
    client.wait_for("the box closed", |s| {
        !s.iter().any(|l| l.contains("Nothing to show"))
    });

    // A file git doesn't know yet: the uncommitted chapter alone, with no
    // story asked for.
    std::fs::write(work.join("notes.txt"), "a thought\n").unwrap();
    client.send(b"\x1bd");
    let screen = client.wait_text("1/1 Not committed yet");
    assert!(marked(&screen, "notes.txt"), "{screen:?}");
    client.wait_text("a thought");
    // Neither a story nor Haiku's order: the only asks are the server's
    // own, a connector's lookup, which names its tool.
    let asked = std::fs::read_to_string(server.home.join("claude-asked")).unwrap_or_default();
    assert!(
        asked.lines().all(|l| l.contains("mcp__")),
        "Haiku isn't asked: {asked}"
    );
    client.type_text("q");
    back(&server);

    // With the setting off, the commits alone, which have nothing to show.
    let dir = server.home.join(".config/switchboard");
    let all = json!({"notifications": false, "tips": false, "github": false, "linear": false, "diff_uncommitted": false});
    std::fs::write(dir.join("settings.json"), all.to_string()).unwrap();
    client.send(b"\x1bd");
    let screen = client.wait_text("Nothing to show: no commits past origin/main");
    assert!(
        !screen.iter().any(|l| l.contains("no uncommitted changes")),
        "{screen:?}"
    );
    back(&server);
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

#[test]
fn lazygit_and_the_diff_take_the_panes_place_from_the_palette_and_a_tabs_menu() {
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

    // With no lazygit installed, the command says so.
    let installed = Command::new("sh")
        .args(["-c", "command -v lazygit"])
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !installed {
        server.ok(&["press", "lazygit"]);
        client.wait_text("lazygit isn't installed");
        client.type_text("x");
    }

    // A stand-in lazygit, which waits until C-c, lazygit's own quit.
    let bin = server.home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let lazygit = bin.join("lazygit");
    std::fs::write(
        &lazygit,
        "#!/bin/sh\necho \"stand-in lazygit in $(pwd)\"\nexec sleep 60\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&lazygit, std::fs::Permissions::from_mode(0o755)).unwrap();
    server.ok(&["press", "lazygit"]);
    client.wait_text("stand-in lazygit in");
    let shown = panes(&server);
    assert_eq!(shown.len(), 1, "in the pane's place: {shown:?}");
    assert_eq!(shown[0]["options"]["@in_place"], "lazygit");
    assert_eq!(shown[0]["options"]["@parked_pane"], first.as_str());
    server.ok(&["press", "lazygit"]);
    wait_until("the pane back after C-c", || {
        panes(&server)[0]["id"] == first.as_str()
    });

    // Mission control's menu on the tab: lazygit, then the diff, each
    // reached with j, since a menu's options have no letters of their own.
    let take = |client: &mut common::TestClient, label: &str| {
        for _ in 0..20 {
            let screen = client.wait_text(label);
            if screen.iter().any(|l| l.contains('›') && l.contains(label)) {
                client.type_text("\r");
                return;
            }
            let before = screen.join("\n");
            client.type_text("j");
            client.wait_for("the cursor to move", |s| s.join("\n") != before);
        }
        panic!("{label} not reached");
    };
    client.type_text("\x1b ");
    client.wait_text("MISSION CONTROL");
    client.type_text("\r");
    client.wait_text("Go to tab 1");
    take(&mut client, "lazygit");
    client.wait_text("stand-in lazygit in");
    assert_eq!(panes(&server)[0]["options"]["@in_place"], "lazygit");
    server.ok(&["press", "lazygit"]);
    wait_until("the pane back again", || {
        panes(&server)[0]["id"] == first.as_str()
    });
    client.type_text("\x1b ");
    client.wait_text("MISSION CONTROL");
    client.type_text("\r");
    take(&mut client, "Its diff (M-d)");
    wait_until("the diff in the pane's place", || {
        panes(&server)[0]["options"]["@in_place"] == "diff"
    });
    client.wait_text("export const a = 2;");
}

#[test]
fn the_working_trees_changes_show_staged_and_not_in_their_own_lists() {
    let server = TestServer::start();
    let dir = server.home.join(".config/switchboard");
    std::fs::create_dir_all(&dir).unwrap();
    let all = json!({"notifications": false, "tips": false, "github": false, "linear": false, "diff_order": false});
    std::fs::write(dir.join("settings.json"), all.to_string()).unwrap();
    let work = repository(server.dir.path());
    std::fs::write(work.join("src/a.js"), "export const a = 3;\n").unwrap();
    git(&work, &["add", "src/a.js"]);
    std::fs::write(work.join("src/a.js"), "export const a = 4;\n").unwrap();
    std::fs::write(work.join("new.txt"), "brand new\n").unwrap();

    let mut client = server.attach(140, 36);
    client.wait_text("$");
    client.type_text(&format!("cd {}\r", work.display()));
    wait_until("the shell to cd", || {
        panes(&server)[0]["cwd"]
            .as_str()
            .is_some_and(|d| d.ends_with("work"))
    });
    server.ok(&["press", "diff-working-tree"]);
    let screen = client.wait_text("Staged changes (1)");
    let at = |text: &str| screen.iter().position(|l| l.contains(text)).unwrap();
    assert!(at("Changes (2)") < at("Staged changes (1)"), "{screen:?}");
    client.wait_text("brand new");
    client.type_text("j");
    let screen = client.wait_text("export const a = 4;");
    let row = screen.iter().find(|l| l.contains("a = 4;")).unwrap();
    assert!(
        row.contains("export const a = 3;"),
        "the index beside the file: {row:?}"
    );
    client.type_text("j");
    let screen = client.wait_text("export const a = 2;");
    let row = screen.iter().find(|l| l.contains("a = 3;")).unwrap();
    assert!(
        row.contains("export const a = 2;"),
        "HEAD beside the index: {row:?}"
    );
    assert_eq!(panes(&server)[0]["options"]["@in_place"], "diff");
    client.send(b"\x1bd");
    wait_until("the pane back", || {
        panes(&server)[0]["options"].get("@in_place").is_none()
    });
}

#[test]
fn c_in_the_diff_goes_through_the_branchs_commits_one_at_a_time() {
    let server = TestServer::start();
    let dir = server.home.join(".config/switchboard");
    std::fs::create_dir_all(&dir).unwrap();
    let all = json!({"notifications": false, "tips": false, "github": false, "linear": false, "diff_order": false});
    std::fs::write(dir.join("settings.json"), all.to_string()).unwrap();
    let work = repository(server.dir.path());
    std::fs::write(work.join("src/b.js"), "export const b = 3;\n").unwrap();
    git(&work, &["commit", "-q", "-am", "Bump b again"]);

    let mut client = server.attach(140, 36);
    client.wait_text("$");
    client.type_text(&format!("cd {}\r", work.display()));
    wait_until("the shell to cd", || {
        panes(&server)[0]["cwd"]
            .as_str()
            .is_some_and(|d| d.ends_with("work"))
    });
    client.send(b"\x1bd");
    client.wait_text("Changes (2)");
    client.type_text("c");
    client.wait_text("Commit 1 of 2");
    client.wait_text("Change both");
    let screen = client.wait_text("export const a = 2;");
    let row = screen.iter().find(|l| l.contains("a = 2;")).unwrap();
    assert!(
        row.contains("export const a = 1;"),
        "the first commit's own change: {row:?}"
    );
    client.type_text("]p");
    client.wait_text("Commit 2 of 2");
    client.wait_text("Bump b again");
    let screen = client.wait_text("export const b = 3;");
    let row = screen.iter().find(|l| l.contains("b = 3;")).unwrap();
    assert!(
        row.contains("export const b = 2;"),
        "the second commit's own change: {row:?}"
    );
    client.type_text("c");
    client.wait_text("Changes (2)");
    client.type_text("q");
    wait_until("the pane back", || {
        panes(&server)[0]["options"].get("@in_place").is_none()
    });
}

#[test]
fn clearing_the_tabs_diff_cache_removes_its_story_order_and_base_after_a_yes() {
    let server = TestServer::start();
    settings(&server);
    stand_in_claude(&server.home);
    let work = repository(server.dir.path());
    // The story told ahead, as M-d finds it, and the base kept with it.
    let path = std::env::var("PATH").unwrap_or_default();
    let out = Command::new(BIN)
        .args(["diff", "--ahead"])
        .arg(&work)
        .env("HOME", &server.home)
        .env(
            "PATH",
            format!("{}:{path}", server.home.join("bin").display()),
        )
        .env("SWITCHBOARD_STATE_DIR", server.dir.path().join("ahead"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("SWITCHBOARD")
        .env_remove("SWITCHBOARD_PANE")
        .env_remove("SWITCHBOARD_SETTINGS")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cache = server.home.join(".cache/switchboard");
    let story_dir = cache.join("diff-story");
    let story = std::fs::read_dir(&story_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "txt"))
        .expect("a story kept");
    std::fs::write(story_dir.join("unrelated.txt"), "another diff's story\n").unwrap();
    let based = || std::fs::read_dir(cache.join("diff-base")).map_or(0, |d| d.count());
    assert_eq!(based(), 1, "the PR base kept for the worktree");
    // Haiku's order of its files, where M-d keeps it.
    let change = switchboard::diff::git::read_change(&work, "origin/main").unwrap();
    let order = switchboard::diff::order::order_file(
        &change.top,
        &change.left,
        &change.right,
        &switchboard::diff::order::ordered(&change),
    );
    let order = cache.join("diff-order").join(order.file_name().unwrap());
    std::fs::create_dir_all(order.parent().unwrap()).unwrap();
    std::fs::write(&order, r#"["src/a.js", "src/b.js"]"#).unwrap();
    // And of the diff M-d opens with the diff_uncommitted setting, from the
    // same merge base to the working tree.
    let whole = switchboard::diff::git::read_to_work_tree(&work, "origin/main").unwrap();
    let (left, right) = whole.order_sides();
    let working_order = switchboard::diff::order::order_file(
        &whole.top,
        left,
        right,
        &switchboard::diff::order::ordered(&whole),
    );
    let working_order = cache
        .join("diff-order")
        .join(working_order.file_name().unwrap());
    assert_ne!(working_order, order);
    std::fs::write(&working_order, r#"["src/b.js", "src/a.js"]"#).unwrap();

    let mut client = server.attach(140, 36);
    client.wait_text("$");
    client.type_text(&format!("cd {}\r", work.display()));
    let tab = server.list()["sessions"][0]["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    wait_until("the tab named from the work", || {
        server
            .ok(&["show", "-t", &tab, "@tab_path"])
            .trim()
            .ends_with("work")
    });
    let clear = |client: &mut common::TestClient, said: &str| {
        server.ok(&["press", "clear-diff-cache"]);
        client.wait_text("Clear this tab's diff cache?");
        client.type_text("\r");
        client.wait_text(said);
        client.type_text("\x1b");
        client.wait_for("the box closed", |s| {
            !s.iter().any(|l| l.contains("diff cache is cleared"))
        });
    };
    // While another program tells it, the story is kept.
    let partial = story.with_extension("partial");
    std::fs::write(&partial, "{}").unwrap();
    clear(
        &mut client,
        "This tab's diff cache is cleared: 3 files. Its story is being told",
    );
    assert!(story.exists(), "the story being told kept");
    assert!(!order.exists(), "the order gone");
    assert!(!working_order.exists(), "the working tree's order gone too");
    assert_eq!(based(), 0, "the PR base gone");
    // Then the base is asked for again, and cleared with the story.
    std::fs::remove_file(&partial).unwrap();
    clear(&mut client, "This tab's diff cache is cleared: 2 files.");
    assert!(!story.exists(), "the story gone");
    assert!(story_dir.join("unrelated.txt").exists(), "another's kept");
    assert_eq!(based(), 0);
    // Nothing told again.
    let kept: Vec<String> = std::fs::read_dir(&story_dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(kept, ["unrelated.txt"]);
}
