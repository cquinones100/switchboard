//! Routines (src/routines) on a server of the test's own: a routine
//! written to the state directory's `routines/` as the Routines view writes
//! it, run when it comes due in a tab running a stand-in `claude` that
//! writes down how it was started, and its run ended by the Stop hook its
//! Claude would send, sent over the socket as `switchboard hook` sends it.

use crate::common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::stand_in::Claude;
use common::{TestClient, TestServer};
use serde_json::{Value, json};
use switchboard::mux::proto::ClientMsg;

/// A server with a stand-in claude, a client attached and a directory for
/// the routines to run in.
fn set_up() -> (TestServer, TestClient, PathBuf) {
    let server = TestServer::start();
    common::write_settings(
        &server.home,
        json!({"notifications": false, "tips": false, "github": false, "linear": false}),
    );
    // Each start a block: a line "---", then an argument a line.
    Claude::new()
        .answer(r#"{ printf '%s\n' ---; printf '%s\n' "$@"; } >> "$HOME/claude-args""#)
        .install(&server.home.join("bin"));
    // The turn typed into a tab's shell (the stand-in has exited) runs no
    // command-not-found helper of the system's, which would stand in front
    // of the pane for a while, as Ubuntu's does.
    std::fs::write(
        server.home.join(".bashrc"),
        "command_not_found_handle() { return 127; }\n",
    )
    .unwrap();
    std::fs::write(
        server.home.join(".zshrc"),
        "command_not_found_handler() { return 127; }\n",
    )
    .unwrap();
    let work = server.home.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let mut client = server.attach(120, 40);
    client.wait_text("$");
    (server, client, work.canonicalize().unwrap())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// A routine named "Daily check" in `dir`, due once at `at`, written as the
/// view writes it; `more` changes its fields. Gives `at`.
fn write_routine(server: &TestServer, dir: &Path, at: i64, more: Value) -> i64 {
    let mut r = json!({
        "id": "r1",
        "name": "Daily check",
        "dir": dir,
        "when": "",
        "schedule": { "kind": "once", "at": at },
        "tabPolicy": "sameTab",
        "whenBusy": "wait",
        "whenMissed": "onWaking",
        "prompt": "Check the build and say how it went.",
        "model": "",
        "paused": false,
        "session": "",
        "handled": at - 60,
    });
    let file = server.state.join("routines/r1.json");
    if let Ok(was) = std::fs::read_to_string(&file)
        && let Ok(was) = serde_json::from_str::<Value>(&was)
    {
        r["session"] = was["session"].clone();
        r["sessionDir"] = was["sessionDir"].clone();
    }
    for (k, v) in more.as_object().unwrap() {
        r[k] = v.clone();
    }
    let dir = server.state.join("routines");
    std::fs::create_dir_all(&dir).unwrap();
    let staged = dir.join(".r1.json.tmp");
    std::fs::write(&staged, r.to_string()).unwrap();
    std::fs::rename(staged, file).unwrap();
    at
}

fn routine(server: &TestServer) -> Value {
    let text = std::fs::read_to_string(server.state.join("routines/r1.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn runs(server: &TestServer) -> String {
    std::fs::read_to_string(server.state.join("routines/runs-r1.tsv")).unwrap_or_default()
}

/// How the stand-in claude was started, each start's arguments.
fn starts(server: &TestServer) -> Vec<Vec<String>> {
    std::fs::read_to_string(server.home.join("claude-args"))
        .unwrap_or_default()
        .split("---\n")
        .filter(|b| !b.is_empty())
        .map(|b| b.lines().map(str::to_string).collect())
        .collect()
}

/// The routine's tab and its first pane, while it is open; its pane's
/// shell made to count as Claude, as a session file of Claude Code's for
/// its process does, which `agent.rs`'s tests write the same way.
fn routine_tab(server: &TestServer) -> Option<(String, String)> {
    server.list()["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|s| s["tabs"].as_array().cloned().unwrap_or_default())
        .find(|t| t["options"]["@routine"] == "r1")
        .map(|t| {
            let pane = &t["panes"][0];
            if let Some(pid) = pane["pid"].as_u64() {
                let dir = server.home.join(".claude/sessions");
                std::fs::create_dir_all(&dir).unwrap();
                let file = json!({"pid": pid, "entrypoint": "cli", "sessionId": "sess-1"});
                std::fs::write(dir.join(format!("{pid}.json")), file.to_string()).unwrap();
            }
            (
                t["id"].as_str().unwrap().to_string(),
                pane["id"].as_str().unwrap().to_string(),
            )
        })
}

fn wait(server: &TestServer, what: &str, mut done: impl FnMut() -> bool) {
    let deadline = common::deadline(Duration::from_secs(20));
    while !done() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}\nserver log: {}",
            server.log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A Claude Code hook from `pane`, its JSON on stdin, as `switchboard
/// hook` sends it.
fn hook(server: &TestServer, pane: &str, event: &str, payload: Value) {
    let msg = ClientMsg::Command {
        args: vec!["hook".into(), event.into()],
        cwd: server.dir.path().to_path_buf(),
        pane: Some(pane.to_string()),
        stdin: Some(payload.to_string().into_bytes()),
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = switchboard::mux::socket::run_command_at(&server.socket(), &msg, &mut out, &mut err);
    assert_eq!(
        i32::from(code),
        0,
        "hook {event}: {}",
        String::from_utf8_lossy(&err)
    );
}

#[test]
fn a_due_routine_opens_its_tab_then_types_into_it_waits_while_it_works_and_resumes_it_closed() {
    let (server, _client, work) = set_up();

    // Due, with no tab: one opens running Claude with the prompt and the
    // turn, marked as the routine's and named after it. Due a moment from
    // now, since one due before the server's first tick was missed.
    write_routine(&server, &work, now() + 1, json!({}));
    wait(&server, "the routine's tab", || {
        routine_tab(&server).is_some()
    });
    let (tab, pane) = routine_tab(&server).unwrap();
    wait(&server, "claude started", || !starts(&server).is_empty());
    assert_eq!(
        starts(&server)[0],
        [
            "--append-system-prompt",
            "Check the build and say how it went.",
            "Run Daily check"
        ]
    );
    let listed = server.list();
    let opts = listed["sessions"][0]["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == tab.as_str())
        .unwrap()["options"]
        .clone();
    assert_eq!(opts["@tab_name"], "Daily check");

    // Its Claude's session is kept, and its Stop ends the run.
    hook(
        &server,
        &pane,
        "SessionStart",
        json!({"session_id": "sess-1", "source": "startup"}),
    );
    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        json!({"session_id": "sess-1", "prompt": "Run Daily check"}),
    );
    hook(
        &server,
        &pane,
        "Stop",
        json!({"session_id": "sess-1", "last_assistant_message": "\nThe build is green.\nMore below."}),
    );
    wait(&server, "the run recorded", || {
        runs(&server).contains("\tran\t")
    });
    assert!(
        runs(&server).contains("The build is green."),
        "{}",
        runs(&server)
    );
    assert_eq!(routine(&server)["session"], "sess-1");

    // Due again with its tab idle: typed into it, no new Claude.
    write_routine(&server, &work, now(), json!({}));
    wait(&server, "the turn typed", || {
        server
            .ok(&["capture-pane", "-t", &pane])
            .contains("Run Daily check")
    });
    assert_eq!(starts(&server).len(), 1, "{:?}", starts(&server));
    hook(
        &server,
        &pane,
        "Stop",
        json!({"session_id": "sess-1", "last_assistant_message": "Green again."}),
    );
    wait(&server, "the second run", || {
        runs(&server).contains("Green again.")
    });

    // Busy, it waits for the turn to end.
    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        json!({"session_id": "sess-1", "prompt": "something of yours"}),
    );
    let at = write_routine(&server, &work, now(), json!({}));
    wait(&server, "the run taken as due", || {
        routine(&server)["handled"].as_i64() == Some(at)
    });
    std::thread::sleep(Duration::from_secs(3) * common::patience());
    assert_eq!(
        runs(&server).lines().count(),
        2,
        "nothing recorded while it waits"
    );
    hook(
        &server,
        &pane,
        "Stop",
        json!({"session_id": "sess-1", "last_assistant_message": "Your thing."}),
    );
    // Once the turn of yours ends, the waiting run starts: its own Stop is
    // the next one recorded. A Stop before it starts is no run's, so it is
    // sent until one is taken.
    wait(&server, "the waiting run, then its Stop", || {
        hook(
            &server,
            &pane,
            "Stop",
            json!({"session_id": "sess-1", "last_assistant_message": "Third."}),
        );
        runs(&server).contains("Third.")
    });
    assert!(
        !runs(&server).contains("Your thing."),
        "your own turn is no run"
    );

    // Its words changed: its Claude starts again with them, resuming.
    write_routine(
        &server,
        &work,
        now(),
        json!({"prompt": "Check the deploy too."}),
    );
    wait(&server, "claude started again", || {
        starts(&server).len() == 2
    });
    routine_tab(&server);
    assert_eq!(
        starts(&server)[1],
        [
            "--resume",
            "sess-1",
            "--append-system-prompt",
            "Check the deploy too.",
            "Run Daily check"
        ]
    );
    hook(
        &server,
        &pane,
        "Stop",
        json!({"session_id": "sess-1", "last_assistant_message": "Fourth."}),
    );
    wait(&server, "the fourth run", || {
        runs(&server).contains("Fourth.")
    });

    // Closed, the next run opens a tab resuming its session.
    server.ok(&["kill-tab", "-t", &tab]);
    write_routine(
        &server,
        &work,
        now(),
        json!({"prompt": "Check the deploy too."}),
    );
    wait(&server, "claude resumed in a new tab", || {
        starts(&server).len() == 3
    });
    assert_eq!(starts(&server)[2][..2], ["--resume", "sess-1"]);
    let (again, _) = routine_tab(&server).unwrap();
    assert_ne!(again, tab);
}

#[test]
fn a_run_missed_while_asleep_runs_on_waking_within_the_hour_else_is_skipped() {
    let (server, _client, work) = set_up();

    // Missed twenty minutes ago, run on waking.
    write_routine(&server, &work, now() - 1200, json!({}));
    wait(&server, "the late run's tab", || {
        routine_tab(&server).is_some()
    });
    wait(&server, "claude started", || !starts(&server).is_empty());
    let (_, pane) = routine_tab(&server).unwrap();
    hook(
        &server,
        &pane,
        "Stop",
        json!({"session_id": "s", "last_assistant_message": "Done late."}),
    );
    wait(&server, "the late run", || {
        runs(&server).contains("\tlate\t")
    });
    assert!(
        runs(&server).contains("ran on waking · Done late."),
        "{}",
        runs(&server)
    );

    // Set to skip: recorded as missed, nothing run.
    write_routine(&server, &work, now() - 1200, json!({"whenMissed": "skip"}));
    wait(&server, "the missed run", || {
        runs(&server).contains("\tmissed\t")
    });
    assert!(runs(&server).contains("so skipped"), "{}", runs(&server));
    assert_eq!(starts(&server).len(), 1);

    // Its tab busy and set to skip then: skipped at once.
    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        json!({"session_id": "s", "prompt": "yours"}),
    );
    write_routine(&server, &work, now(), json!({"whenBusy": "skip"}));
    wait(&server, "the busy run skipped", || {
        runs(&server).contains("\tbusy\t")
    });
}
