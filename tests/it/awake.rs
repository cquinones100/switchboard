//! Keeping the computer awake (src/awake), against a server of the test's
//! own: a pane's Claude marked working (`set @agent_state`, as its hooks
//! mark it), what `list` says holds the computer awake, and, on a Mac, the
//! stand-in `caffeinate` on the test's PATH started and ended. On Linux the
//! test's server can't reach logind (`DBUS_SYSTEM_BUS_ADDRESS`), so it holds
//! nothing, and only what it decided is read. The grace after everything
//! goes idle is cut short with `SWITCHBOARD_AWAKE_GRACE_MS`.

use crate::common;

use std::time::{Duration, Instant};

use common::TestServer;
use serde_json::Value;

const MAC: bool = cfg!(target_os = "macos");

/// A server with `settings`, its grace a few seconds.
fn server(settings: &str) -> TestServer {
    let grace = (3000 * common::patience()).to_string();
    TestServer::start_with_env_and_settings(&[("SWITCHBOARD_AWAKE_GRACE_MS", &grace)], settings)
}

/// Waits until `list`'s `awake` satisfies `pred`; gives it.
fn wait_awake(server: &TestServer, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    let deadline = common::deadline(Duration::from_secs(15));
    loop {
        let awake = server.list()["awake"].clone();
        if pred(&awake) {
            return awake;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {awake}\nserver log: {}",
            server.log()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The stand-in caffeinate's calls, and the pid each ran as.
fn caffeinates(server: &TestServer) -> (Vec<String>, Vec<u32>) {
    let log = std::fs::read_to_string(server.home.join("desktop-log")).unwrap_or_default();
    let calls = log
        .lines()
        .filter(|l| l.starts_with("caffeinate -"))
        .map(str::to_string)
        .collect();
    let pids = log
        .lines()
        .filter_map(|l| l.strip_prefix("caffeinate is ")?.trim().parse().ok())
        .collect();
    (calls, pids)
}

fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only asks whether the process is there.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Waits for the `n`th caffeinate to have started; gives its pid.
fn wait_caffeinate(server: &TestServer, n: usize) -> u32 {
    let deadline = common::deadline(Duration::from_secs(10));
    loop {
        if let Some(pid) = caffeinates(server).1.get(n - 1) {
            return *pid;
        }
        assert!(
            Instant::now() < deadline,
            "caffeinate number {n} never started: {:?}\nserver log: {}",
            caffeinates(server),
            server.log()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The first tab's first pane, its shell standing in for Claude: a
/// session file for its pid, as Claude Code writes one.
fn claude_pane(server: &TestServer) -> String {
    let pane = server.list()["sessions"][0]["tabs"][0]["panes"][0].clone();
    let pid = pane["pid"].as_u64().unwrap();
    let dir = server.home.join(".claude/sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let file = serde_json::json!({"pid": pid, "entrypoint": "cli", "sessionId": "s-1"});
    std::fs::write(dir.join(format!("{pid}.json")), file.to_string()).unwrap();
    pane["id"].as_str().unwrap().to_string()
}

#[test]
fn a_working_tab_keeps_the_computer_awake_until_a_grace_after_it_idles() {
    let server = server(r#"{"keep_awake": true, "notifications": false}"#);
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let pane = claude_pane(&server);
    assert_eq!(
        server.list()["awake"],
        serde_json::json!({"holding": false, "why": null, "by_hand": false}),
        "nothing holds it while nothing works"
    );

    server.ok(&["set", "-t", &pane, "@agent_state", "working"]);
    wait_awake(&server, "the working tab", |a| a["why"] == "1 tab working");
    if MAC {
        wait_awake(&server, "the hold", |a| a["holding"] == true);
        let pid = wait_caffeinate(&server, 1);
        assert!(alive(pid));
        let server_pid = server.pid().unwrap();
        assert_eq!(
            caffeinates(&server).0,
            vec![format!("caffeinate -is -w {server_pid}")],
            "taken once, watching the server"
        );
        // A turn between its tool calls doesn't let go.
        server.ok(&["set", "-u", "-t", &pane, "@agent_state"]);
        wait_awake(&server, "nothing working", |a| a["why"].is_null());
        assert_eq!(
            server.list()["awake"]["holding"],
            true,
            "held through the grace"
        );
        wait_awake(&server, "let go after the grace", |a| a["holding"] == false);
        assert!(!alive(pid), "caffeinate ended");
        assert_eq!(caffeinates(&server).1.len(), 1, "taken once");
    } else {
        server.ok(&["set", "-u", "-t", &pane, "@agent_state"]);
        wait_awake(&server, "nothing working", |a| a["why"].is_null());
        assert_eq!(
            server.list()["awake"]["holding"],
            false,
            "logind can't be reached"
        );
    }
}

#[test]
fn by_hand_it_holds_whatever_runs_through_an_upgrade_and_the_setting_off_lets_work_go() {
    let server = server(r#"{"keep_awake": false, "notifications": false}"#);
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let pane = claude_pane(&server);
    server.ok(&["set", "-t", &pane, "@agent_state", "working"]);

    // Held by hand, the setting off and a tab working or not.
    server.ok(&["press", "keep-awake"]);
    let awake = server.list()["awake"].clone();
    assert_eq!(
        (&awake["why"], &awake["by_hand"]),
        (&"until you say".into(), &true.into())
    );
    let first = MAC.then(|| {
        wait_awake(&server, "the hold", |a| a["holding"] == true);
        wait_caffeinate(&server, 1)
    });

    // Kept by hand across a restart on a new build, whose exec lets the
    // old caffeinate go (it waits on the same pid) and which takes its own.
    let (code, out, err) = server.run(&["upgrade"]);
    assert_eq!(code, 0, "{out}{err}\nserver log: {}", server.log());
    wait_awake(&server, "the hold taken up", |a| {
        a["by_hand"] == true && a["why"] == "until you say"
    });
    if let Some(first) = first {
        assert!(
            !alive(first),
            "the old build's caffeinate let go before the exec"
        );
        let second = wait_caffeinate(&server, 2);
        assert!(alive(second));
        wait_awake(&server, "the new build's hold", |a| a["holding"] == true);
    }

    // Pressed again: back to the setting, which is off, so the working tab
    // holds nothing.
    server.ok(&["press", "keep-awake"]);
    let awake = server.list()["awake"].clone();
    assert_eq!(
        (&awake["why"], &awake["by_hand"]),
        (&Value::Null, &false.into())
    );
    if MAC {
        wait_awake(&server, "let go after the grace", |a| a["holding"] == false);
    }
}
