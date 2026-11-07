//! Notes in the workspace as a person uses it (the tmux version's
//! test/new-tab.test.js and what notes-open.sh did): M-N opens Notes in a tab
//! of its own, M-e brings it beside a pane and home again, leaving a pane in
//! its tab that says where it went, Notes left alone in a tab goes home, a
//! note's tab is opened through the palette's `press`, and each tab's note
//! is shown through the app's socket.
//!
//! The app is a stand-in (`NOTES_OPEN_COMMAND`, as the tmux version's tests
//! had), and so is its socket: a listener of the test's own at
//! `NOTES_SOCKET` that keeps each request, so nothing reaches your notes.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{TempDir, TestClient, TestServer};
use serde_json::{Value, json};

const ALT: &str = "\x1b";

struct Rig {
    server: TestServer,
    /// The notes directory, a socket and the settings, outside the server's
    /// home so they are there before it starts.
    dir: TempDir,
    /// What the stand-in app was asked, in order.
    heard: Arc<Mutex<Vec<String>>>,
}

impl Rig {
    fn new() -> Rig {
        let dir = TempDir::new("sbn");
        let notes = dir.path().join("Notes");
        std::fs::create_dir_all(notes.join("Work")).unwrap();
        std::fs::write(notes.join("Plan.md"), "# Plan\n\n- [ ] the first thing\n").unwrap();
        let settings = dir.path().join("settings.json");
        std::fs::write(
            &settings,
            json!({ "tips": false, "notifications": false, "ask_tab_note": false }).to_string(),
        )
        .unwrap();
        // The app's socket: each request kept, and answered as the app
        // answers.
        let socket = dir.path().join("n.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let heard = Arc::new(Mutex::new(Vec::new()));
        let keep = heard.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut line = String::new();
                if BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut line)
                    .is_ok()
                {
                    keep.lock().unwrap().push(line.trim_end().to_string());
                    let _ = stream.write_all(b"ok\n");
                }
            }
        });
        let path = |p: PathBuf| p.to_string_lossy().into_owned();
        let server = TestServer::start_with_env(&[
            ("NOTES_DIR", &path(notes)),
            ("NOTES_SOCKET", &path(socket)),
            ("NOTES_OPEN_COMMAND", "echo notes-stand-in; exec cat"),
            ("SWITCHBOARD_SETTINGS", &path(settings)),
        ]);
        Rig { server, dir, heard }
    }

    fn notes_dir(&self) -> PathBuf {
        self.dir.path().join("Notes")
    }

    fn list(&self) -> Value {
        self.server.list()
    }

    /// Waits until the server's list satisfies `pred`.
    fn wait(&self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let list = self.list();
            if pred(&list) {
                return list;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}: {list:#}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Waits until the app has been asked `request`.
    fn wait_heard(&self, request: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.heard.lock().unwrap().iter().any(|r| r == request) {
            assert!(
                Instant::now() < deadline,
                "the app wasn't asked {request:?}: {:?}",
                self.heard.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn heard(&self) -> Vec<String> {
        self.heard.lock().unwrap().clone()
    }
}

fn tabs(list: &Value) -> Vec<Value> {
    list["sessions"][0]["tabs"].as_array().unwrap().clone()
}

fn pane_ids(tab: &Value) -> Vec<String> {
    tab["panes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap().to_string())
        .collect()
}

/// The pane with an option set, and its tab's place in the list.
fn marked(list: &Value, option: &str) -> Option<(usize, String)> {
    tabs(list).iter().enumerate().find_map(|(i, t)| {
        t["panes"].as_array().unwrap().iter().find_map(|p| {
            (p["options"][option] == "1").then(|| (i, p["id"].as_str().unwrap().to_string()))
        })
    })
}

/// A client on the first tab, its pane's id, and Notes opened with M-N in
/// a tab of its own, then the first tab in front again.
fn notes_open(rig: &Rig) -> (TestClient, String, String) {
    let mut client = rig.server.attach(140, 36);
    client.wait_text("$");
    let first = pane_ids(&tabs(&rig.list())[0])[0].clone();
    client.type_text(&format!("{ALT}N"));
    let list = rig.wait("Notes in a tab of its own", |l| {
        tabs(l).len() == 2
            && tabs(l)[1]["options"]["@notes"] == "1"
            && marked(l, "@notes_pane").is_some()
    });
    assert_eq!(tabs(&list)[1]["own"], "Notes", "named Notes");
    assert_eq!(tabs(&list)[1]["current"], true, "and gone to");
    client.wait_text("notes-stand-in");
    let notes = marked(&list, "@notes_pane").unwrap().1;
    client.type_text(&format!("{ALT}1"));
    rig.wait("the first tab in front", |l| tabs(l)[0]["current"] == true);
    (client, first, notes)
}

#[test]
fn m_e_brings_notes_beside_a_pane_and_home_again_with_a_pane_left_in_its_place() {
    let rig = Rig::new();
    let (mut client, first, notes) = notes_open(&rig);
    let first_tab = tabs(&rig.list())[0]["id"].as_str().unwrap().to_string();
    rig.server
        .ok(&["set", "-t", &first_tab, "@tab_note", "Plan.md"]);

    // M-e: Notes beside the pane, a pane left in its own tab saying where
    // it went.
    client.type_text(&format!("{ALT}e"));
    let list = rig.wait("Notes beside the first pane", |l| {
        pane_ids(&tabs(l)[0]) == [first.clone(), notes.clone()]
    });
    assert_eq!(tabs(&list)[0]["options"]["@notes_beside"], "1");
    let (at, stand_in) = marked(&list, "@notes_placeholder").expect("a pane left in its place");
    assert_eq!(at, 1, "in Notes' own tab, which stays");
    assert_eq!(pane_ids(&tabs(&list)[1]), std::slice::from_ref(&stand_in));
    // The app is shown the tab's note, its list put away for the room.
    rig.wait_heard("sidebar off");
    rig.wait_heard("open Plan.md");
    let order = rig.heard();
    let pos = |r: &str| order.iter().position(|h| h == r).unwrap();
    assert!(pos("sidebar off") < pos("open Plan.md"), "{order:?}");
    // The pane left behind, and the sidebar, say where it went.
    client.wait_for("where Notes went", |_| {
        rig.server
            .ok(&["capture-pane", "-t", &stand_in])
            .contains("Notes is beside tab 1")
    });
    client.wait_text("Notes here");
    client.wait_text("Notes is beside tab 1");

    // M-e again, in Notes: home, in the place of the pane left there.
    client.type_text(&format!("{ALT}e"));
    let list = rig.wait("Notes home again", |l| {
        pane_ids(&tabs(l)[0]) == [first.clone()] && pane_ids(&tabs(l)[1]) == [notes.clone()]
    });
    assert!(marked(&list, "@notes_placeholder").is_none(), "{list:#}");
    assert_eq!(tabs(&list)[1]["options"]["@notes"], "1");
    rig.wait_heard("sidebar on");
}

#[test]
fn m_e_with_notes_not_open_starts_it_beside_the_pane_on_the_tabs_note() {
    let rig = Rig::new();
    std::fs::write(rig.notes_dir().join("Work/Ideas.md"), "# Ideas\n").unwrap();
    let mut client = rig.server.attach(140, 36);
    client.wait_text("$");
    let list = rig.list();
    let first_tab = tabs(&list)[0]["id"].as_str().unwrap().to_string();
    let first = pane_ids(&tabs(&list)[0])[0].clone();
    rig.server
        .ok(&["set", "-t", &first_tab, "@tab_note", "Work/Ideas.md"]);
    client.type_text(&format!("{ALT}e"));
    let list = rig.wait("Notes started beside the pane", |l| {
        pane_ids(&tabs(l)[0]).len() == 2 && marked(l, "@notes_pane").is_some()
    });
    let (at, notes) = marked(&list, "@notes_pane").unwrap();
    assert_eq!(at, 0, "in this tab, not one of its own");
    assert_eq!(pane_ids(&tabs(&list)[0]), [first, notes]);
    assert_eq!(tabs(&list)[0]["options"]["@notes_beside"], "1");
    assert_eq!(
        tabs(&list)[0]["options"]["@tab_note_arrived"],
        "Work/Ideas.md"
    );
    client.wait_text("notes-stand-in");
    // An app not yet running opens with the tab's note.
    let meta = std::fs::read_to_string(rig.notes_dir().join(".notes.json")).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&meta).unwrap()["last"],
        "Work/Ideas.md"
    );
}

#[test]
fn notes_left_alone_in_a_tab_it_was_brought_beside_goes_home() {
    let rig = Rig::new();
    let (mut client, first, notes) = notes_open(&rig);
    client.type_text(&format!("{ALT}e"));
    rig.wait("Notes beside the first pane", |l| {
        pane_ids(&tabs(l)[0]) == [first.clone(), notes.clone()]
    });
    // The tab's own pane closes: Notes goes home and the tab with it.
    rig.server.ok(&["kill-pane", "-t", &first]);
    let list = rig.wait("Notes home and the tab gone", |l| {
        tabs(l).len() == 1 && pane_ids(&tabs(l)[0]) == [notes.clone()]
    });
    assert_eq!(tabs(&list)[0]["options"]["@notes"], "1");
    assert!(marked(&list, "@notes_placeholder").is_none());

    // In its own tab it stays when a pane beside it closes.
    let other = rig
        .server
        .ok(&["split", "-d", "-t", &notes])
        .trim()
        .to_string();
    rig.wait("a pane beside Notes", |l| pane_ids(&tabs(l)[0]).len() == 2);
    rig.server.ok(&["kill-pane", "-t", &other]);
    rig.wait("Notes where it was", |l| {
        tabs(l).len() == 1 && pane_ids(&tabs(l)[0]) == [notes.clone()]
    });
}

#[test]
fn press_runs_an_action_by_name_and_opens_a_notes_tab() {
    let rig = Rig::new();
    // A stand-in Claude that keeps what it was started with.
    let bin = rig.server.home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let claude = bin.join("claude");
    std::fs::write(
        &claude,
        "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$HOME/claude-args\"\nexec sleep 30\n",
    )
    .unwrap();
    std::fs::set_permissions(&claude, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mut client = rig.server.attach(140, 36);
    client.wait_text("$");

    let (code, _, err) = rig.server.run(&["press", "nope"]);
    assert_eq!(code, 1);
    assert!(err.contains("no action named nope"), "{err}");
    let (code, _, err) = rig.server.run(&["press", "notes-beside", "Plan.md"]);
    assert_eq!(code, 1, "{err}");

    // The note's tab: none is linked to it, so which, a new one first.
    rig.server.ok(&["press", "note-tab", "Plan.md"]);
    client.wait_text("Which tab is Plan's?");
    client.type_text("\r");
    client.wait_text("A tab for Plan");
    client.type_text("\r");
    let list = rig.wait("the note's tab, with Notes beside its Claude", |l| {
        tabs(l).len() == 2
            && tabs(l)[1]["options"]["@tab_note"] == "Plan.md"
            && pane_ids(&tabs(l)[1]).len() == 2
    });
    assert_eq!(tabs(&list)[1]["options"]["@tab_name"], "Plan");
    assert_eq!(marked(&list, "@notes_pane").map(|m| m.0), Some(1));
    // Notes, just started, opens with the tab's note.
    let meta = std::fs::read_to_string(rig.notes_dir().join(".notes.json")).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&meta).unwrap()["last"],
        "Plan.md"
    );
    // Its Claude is told the note, in its system prompt.
    let args = rig.server.home.join("claude-args");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !args.exists() {
        assert!(
            Instant::now() < deadline,
            "the stand-in Claude didn't start"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let args = std::fs::read_to_string(&args).unwrap();
    let mut lines = args.lines();
    assert_eq!(lines.next(), Some("--append-system-prompt"));
    let context = lines.next().unwrap();
    assert!(
        context.contains("This tab is for the note \"Plan\""),
        "{context}"
    );
    assert!(context.contains("/Plan.md, which Notes shows beside you"));

    // Asked again, it goes to that tab: linked now.
    client.type_text(&format!("{ALT}1"));
    rig.wait("the first tab in front", |l| tabs(l)[0]["current"] == true);
    rig.server.ok(&["press", "note-tab", "Plan.md"]);
    rig.wait("the note's tab in front", |l| tabs(l)[1]["current"] == true);
    assert_eq!(tabs(&rig.list()).len(), 2, "no other tab opened");
}

/// A note written for you, as the meeting notes and the standup record it.
fn note_written(rig: &Rig, rel: &str, title: &str) {
    std::fs::create_dir_all(&rig.server.state).unwrap();
    std::fs::write(
        rig.server.state.join("new-notes.json"),
        json!([{ "path": rel, "title": title, "source": "Fathom", "at": 1 }]).to_string(),
    )
    .unwrap();
}

#[test]
fn enter_on_a_new_notes_notification_opens_the_note_in_notes() {
    let rig = Rig::new();
    // With the Notifications window off, Enter in the sidebar acts at once.
    std::fs::write(
        rig.dir.path().join("settings.json"),
        json!({ "tips": false, "notifications": false, "ask_tab_note": false, "notice_list": false })
            .to_string(),
    )
    .unwrap();
    note_written(&rig, "Plan.md", "Plan");
    let mut client = rig.server.attach(140, 36);
    client.wait_text("● Plan");
    // M-s into the sidebar, n to the notifications, Enter on the first.
    client.type_text(&format!("{ALT}s"));
    client.type_text("n");
    client.type_text("\r");
    rig.wait("Notes in a tab of its own", |l| {
        marked(l, "@notes_pane").is_some()
    });
    client.wait_text("notes-stand-in");
    // Notes wasn't running: the note is the one it opens with.
    let meta = std::fs::read_to_string(rig.notes_dir().join(".notes.json")).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&meta).unwrap()["last"],
        "Plan.md"
    );
    client.wait_for("the notification let go", |s| {
        !s.iter().any(|l| l.contains("● Plan"))
    });
    // Running, it is asked to show the note through its socket.
    std::fs::write(rig.notes_dir().join("Work/Later.md"), "# Later\n").unwrap();
    note_written(&rig, "Work/Later.md", "Later");
    client.type_text(&format!("{ALT}1"));
    client.wait_text("● Later");
    client.type_text(&format!("{ALT}s"));
    client.type_text("n");
    client.type_text("\r");
    rig.wait_heard("open Work/Later.md");
    rig.wait("Notes in front", |l| {
        marked(l, "@notes_pane").is_some_and(|(i, _)| tabs(l)[i]["current"] == true)
    });
}
