//! The note-watch skill's Monitor, `switchboard watch-note`, run on a note in
//! a temporary directory with short timings: an item added once it has
//! settled, none of those already there, and a line when the note goes.

use crate::common;

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use common::{BIN, TempDir};

#[test]
fn an_item_added_is_said_once_settled_and_the_note_going_is_said_too() {
    let tmp = TempDir::new("watch-note");
    let note = tmp.path().join("Work.md");
    std::fs::write(&note, "# Work\n\n- [ ] already here\n").unwrap();
    let mut child = Command::new(BIN)
        .arg("watch-note")
        .arg(&note)
        .env("NOTE_WATCH_POLL_MS", "40")
        .env("NOTE_WATCH_SETTLE_MS", "150")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (tx, rx) = mpsc::channel();
    let out = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    // watch-note says nothing as it reads the note first, so probe items are
    // added, each kept, until one is said new: a probe there before that
    // first read is taken as already there and never said, and a later one is.
    let deadline = common::deadline(Duration::from_secs(10));
    let mut probes = String::new();
    for n in 0.. {
        assert!(
            std::time::Instant::now() < deadline,
            "watch-note never started"
        );
        probes.push_str(&format!("- [ ] probe {n}\n"));
        switchboard::util::write_atomic(&note, format!("# Work\n\n- [ ] already here\n{probes}"))
            .unwrap();
        if let Ok(line) = rx.recv_timeout(Duration::from_secs(1) * common::patience()) {
            assert!(line.starts_with("NEW: probe "), "said at the start: {line}");
            break;
        }
    }
    // Any other probe settling meanwhile is said before the save below
    // takes them all away; those lines are passed over.
    // Saved as the notes app saves (`util::write_atomic`): written beside
    // it and moved into its place, so a look at it never finds it emptied
    // half way through a write, which would read as its items all gone and
    // then all new.
    let saving = tmp.path().join("Work.md.saving");
    std::fs::write(
        &saving,
        "# Work\n\n- [ ] already here\n- [ ] something new  \n- [ ] \n",
    )
    .unwrap();
    std::fs::rename(&saving, &note).unwrap();
    let line = std::iter::repeat_with(|| {
        rx.recv_timeout(Duration::from_secs(5) * common::patience())
            .expect("a line")
    })
    .find(|line| !line.starts_with("NEW: probe "))
    .unwrap();
    assert_eq!(line, "NEW: something new");
    std::fs::remove_file(&note).unwrap();
    let line = rx
        .recv_timeout(Duration::from_secs(5) * common::patience())
        .expect("a line");
    assert_eq!(
        line,
        format!("GONE: {} was moved or deleted", note.display())
    );
    assert_eq!(child.wait().unwrap().code(), Some(0));
}

#[test]
fn a_note_that_cant_be_read_is_gone_from_the_start() {
    let out = Command::new(BIN)
        .args(["watch-note", "/no/such/note.md"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "GONE: /no/such/note.md can't be read\n"
    );
    let usage = Command::new(BIN).arg("watch-note").output().unwrap();
    assert_eq!(usage.status.code(), Some(1));
}
