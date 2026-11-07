//! Restarting the server on a new build in place (`switchboard upgrade`): a
//! server of the test's own (its "new build" the same binary) is given a
//! second tab, a split, options and programs that keep writing, then
//! upgraded; it must keep its pid, every tab, pane, layout and option, and
//! every program running untouched, still its child, still writing, and the
//! terminal attached must stay attached, its keys still reaching the panes.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use common::TestServer;
use serde_json::Value;
use switchboard::mux::proto::{self, ClientMsg};

/// A program that says its pid, then writes a tick every 100ms.
const TICKER: &str = "echo pid=$$; i=0; while :; do echo tick$i; i=$((i+1)); sleep 0.1; done";

/// The highest tick in a pane's text.
fn last_tick(text: &str) -> Option<u64> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter_map(|w| w.strip_prefix("tick")?.parse().ok())
        .max()
}

/// What a test compares before and after: each session's tabs, in order,
/// with their ids, names, options and whether current or zoomed, and each
/// pane's id, program, size and options.
fn shape(list: &Value) -> Value {
    let sessions: Vec<Value> = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            let tabs: Vec<Value> = s["tabs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| {
                    let panes: Vec<Value> = t["panes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|p| {
                            serde_json::json!([
                                p["id"],
                                p["pid"],
                                p["cols"],
                                p["rows"],
                                p["active"],
                                p["options"]
                            ])
                        })
                        .collect();
                    serde_json::json!([
                        t["id"],
                        t["name"],
                        t["current"],
                        t["zoomed"],
                        t["options"],
                        panes
                    ])
                })
                .collect();
            serde_json::json!([s["name"], tabs])
        })
        .collect();
    Value::Array(sessions)
}

/// Every pane's program's pid.
fn pids(list: &Value) -> Vec<u32> {
    list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["tabs"].as_array().unwrap().iter())
        .flat_map(|t| t["panes"].as_array().unwrap().iter())
        .map(|p| p["pid"].as_u64().expect("a pane's pid") as u32)
        .collect()
}

/// A process's parent, as `ps` says.
fn parent_of(pid: u32) -> Option<u32> {
    let out = std::process::Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only asks whether the process is there.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Waits until a pane has written a tick past `after`.
fn wait_tick(server: &TestServer, pane: &str, after: u64) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let text = server.ok(&["capture-pane", "-t", pane, "-S", "-200"]);
        if let Some(n) = last_tick(&text).filter(|n| *n > after) {
            return n;
        }
        assert!(
            Instant::now() < deadline,
            "{pane} wrote no tick past {after}:\n{text}\nserver log: {}",
            server.log()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn an_upgrade_keeps_every_pane_running_and_the_server_its_pid() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let out = server.ok(&["new-tab", "-n", "ticker", "--", "sh", "-c", TICKER]);
    let mut ids = out.split_whitespace();
    let (tab, ticker) = (
        ids.next().unwrap().to_string(),
        ids.next().unwrap().to_string(),
    );
    let beside = server
        .ok(&["split", "-t", &ticker, "-R", "-d", "--", "sh", "-c", TICKER])
        .trim()
        .to_string();
    server.ok(&["set", "-t", &tab, "@tab_note", "kept across"]);
    server.ok(&["set", "-t", &beside, "@mine", "too"]);
    client.wait_text("tick3");
    let before = server.list();
    let server_pid = server.pid().unwrap();
    for pid in pids(&before) {
        assert_eq!(
            parent_of(pid),
            Some(server_pid),
            "{pid} is the server's child"
        );
    }
    let ticked = last_tick(&server.ok(&["capture-pane", "-t", &ticker])).unwrap();
    // A key whose frame is half sent as the server hands over: the rest
    // follows once it has, and the new build reads it whole.
    let typed = proto::encode_client(&ClientMsg::Input(b"carried over".to_vec()));
    client.write_raw(&typed[..3]);

    let (code, out, err) = server.run(&["upgrade"]);
    assert_eq!(code, 0, "{out}{err}\nserver log: {}", server.log());
    assert!(out.contains("restarted"), "{out}");
    assert!(out.contains("keeping 3 panes in 2 tabs"), "{out}");
    client.write_raw(&typed[3..]);

    // The same process, every program in it its child as before, running.
    let after = server.list();
    assert_eq!(server.pid(), Some(server_pid));
    assert_eq!(
        shape(&after),
        shape(&before),
        "the same tabs, panes and options"
    );
    for pid in pids(&after) {
        assert!(alive(pid), "{pid} still runs");
        assert_eq!(
            parent_of(pid),
            Some(server_pid),
            "{pid} is still the server's child"
        );
    }
    assert!(
        !server.state.join("handover.json").exists(),
        "the handover is removed once read"
    );
    // Each goes on writing, from where it was: what it showed is there too.
    let text = server.ok(&["capture-pane", "-t", &ticker, "-S", "-200"]);
    assert!(text.contains("pid="), "what it showed came back: {text}");
    wait_tick(&server, &ticker, ticked);
    wait_tick(&server, &beside, ticked);

    // The terminal attached before is still attached, drawn to by the new
    // build, its keys (the one half sent too) reaching the pane in front,
    // whose terminal echoes them, and its resizes taken.
    client.wait_for("ticks on the terminal attached before", |s| {
        s.iter().any(|l| last_tick(l).is_some_and(|n| n > ticked))
    });
    client.wait_text("carried over");
    assert!(client.exit.is_none(), "it was never told to end");
    client.resize(90, 30);
    let deadline = Instant::now() + Duration::from_secs(10);
    while server.list()["sessions"][0]["tabs"][1]["panes"][0]["cols"]
        == before["sessions"][0]["tabs"][1]["panes"][0]["cols"]
    {
        assert!(Instant::now() < deadline, "the resize wasn't taken");
        std::thread::sleep(Duration::from_millis(50));
    }

    // A terminal attaching now sees it all.
    let mut fresh = server.attach(100, 30);
    fresh.wait_for("ticks on a terminal attached after", |s| {
        s.iter().any(|l| last_tick(l).is_some_and(|n| n > ticked))
    });
    // And a pane started now is numbered after those kept.
    let (_, pane) = server
        .ok(&["new-tab", "-d"])
        .split_once(' ')
        .map(|(t, p)| (t.to_string(), p.trim().to_string()))
        .unwrap();
    let n = |id: &str| id.trim_start_matches('%').parse::<u32>().unwrap();
    assert!(n(&pane) > n(&beside), "{pane} after {beside}");
    // An upgrade again goes as well.
    let (code, out, err) = server.run(&["upgrade"]);
    assert_eq!(code, 0, "{out}{err}\nserver log: {}", server.log());
    assert_eq!(server.pid(), Some(server_pid));
    let again = server.list();
    assert_eq!(pids(&again).len(), 4);

    // As M-p's action does it, in the terminal used last, which is told
    // how it went in a box.
    server.ok(&["press", "upgrade"]);
    fresh.wait_text("Switchboard restarted on");
    assert_eq!(server.log().matches("Switchboard restarted on").count(), 3);
    assert_eq!(server.pid(), Some(server_pid));
    assert_eq!(pids(&server.list()), pids(&again));
    assert!(client.exit.is_none());
}

#[test]
fn a_build_that_cant_take_over_is_refused_and_nothing_changes() {
    let (server, bin) = TestServer::start_from_copy();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let ticker = server
        .ok(&["new-tab", "--", "sh", "-c", TICKER])
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_string();
    client.wait_text("tick2");
    let before = server.list();

    // Another build in its place, one that takes over in another format.
    std::fs::remove_file(&bin).unwrap();
    std::fs::write(&bin, "#!/bin/sh\necho 99\n").unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (code, out, err) = server.run(&["upgrade"]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(
        err.contains("format 99") && err.contains("goes on as it was"),
        "{err}"
    );

    // One from before upgrades, which doesn't know the question.
    std::fs::write(
        &bin,
        "#!/bin/sh\necho 'Usage: switchboard server' >&2\nexit 2\n",
    )
    .unwrap();
    let (code, _, err) = server.run(&["upgrade"]);
    assert_eq!(code, 1);
    assert!(err.contains("before upgrades"), "{err}");

    // Nothing stopped: the same tabs, programs and terminal.
    let after = server.list();
    assert_eq!(shape(&after), shape(&before));
    for pid in pids(&after) {
        assert!(alive(pid), "{pid} still runs");
    }
    let ticked = last_tick(&server.ok(&["capture-pane", "-t", &ticker])).unwrap();
    wait_tick(&server, &ticker, ticked);
    client.wait_for("ticks after the refusal", |s| {
        s.iter().any(|l| last_tick(l).is_some_and(|n| n > ticked))
    });
    assert!(client.exit.is_none(), "the terminal stays attached");
}
