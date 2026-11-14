//! Routines (src/routines) on a server of the test's own: a routine
//! written to the state directory's `routines/` as the Routines view writes
//! it, run when it comes due as a headless stand-in `claude` that writes
//! down how it was started and what it was given on stdin, and answers
//! with the JSON in `$HOME/claude-reply`, as `claude -p --output-format
//! json` does.

use crate::common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::stand_in::Claude;
use common::{TestClient, TestServer};
use serde_json::{Value, json};

/// A server with a stand-in claude, a client attached and a directory for
/// the routines to run in.
fn set_up() -> (TestServer, TestClient, PathBuf) {
    let server = TestServer::start();
    common::write_settings(
        &server.home,
        json!({"notifications": false, "tips": false, "github": false, "linear": false}),
    );
    // Each start a block: a line "---", then an argument a line. While
    // `claude-hold` is there, it waits.
    Claude::new()
        .answer(
            r#"{ printf '%s\n' ---; printf '%s\n' "$@"; } >> "$HOME/claude-args"
cat > "$HOME/claude-stdin"
while [ -f "$HOME/claude-hold" ]; do sleep 0.1; done
cat "$HOME/claude-reply""#,
        )
        .install(&server.home.join("bin"));
    reply(&server, json!({}));
    let work = server.home.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let mut client = server.attach(120, 40);
    client.wait_text("$");
    (server, client, work.canonicalize().unwrap())
}

/// What the stand-in answers next: a run that went well in session
/// `sess-1`, `more` changing its fields.
fn reply(server: &TestServer, more: Value) {
    let mut r = json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "result": "\nThe build is green.\nMore below.",
        "session_id": "sess-1",
        "total_cost_usd": 0.123,
        "permission_denials": [],
    });
    for (k, v) in more.as_object().unwrap() {
        r[k] = v.clone();
    }
    std::fs::write(server.home.join("claude-reply"), r.to_string()).unwrap();
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
        "whenMissed": "onWaking",
        "prompt": "Check the build and say how it went.",
        "model": "",
        "tools": ["Read"],
        "paused": false,
        "session": "",
        "handled": at - 60,
    });
    let file = server.state.join("routines/r1.json");
    if let Ok(was) = std::fs::read_to_string(&file)
        && let Ok(was) = serde_json::from_str::<Value>(&was)
    {
        for kept in ["session", "sessionDir", "tools", "needs"] {
            r[kept] = was[kept].clone();
        }
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

/// The value after `flag` in a start's arguments.
fn after<'a>(start: &'a [String], flag: &str) -> Option<&'a str> {
    let at = start.iter().position(|a| a == flag)?;
    start.get(at + 1).map(String::as_str)
}

/// How many tabs are open.
fn tab_count(server: &TestServer) -> usize {
    server.list()["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| s["tabs"].as_array().map_or(0, Vec::len))
        .sum()
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

#[test]
fn a_due_routine_runs_headless_records_its_reply_and_cost_and_resumes_its_session() {
    let (server, _client, work) = set_up();
    let tabs = tab_count(&server);

    // Due a moment from now, since one due before the server's first tick
    // was missed.
    write_routine(&server, &work, now() + 1, json!({}));
    wait(&server, "the run recorded", || {
        runs(&server).contains("\tran\t")
    });
    let line = runs(&server);
    assert!(line.contains("\t0.12\tran\tThe build is green."), "{line}");
    assert_eq!(tab_count(&server), tabs, "no tab opened");
    let first = &starts(&server)[0];
    assert_eq!(first[0], "-p");
    assert_eq!(after(first, "--output-format"), Some("json"));
    assert_eq!(after(first, "--permission-mode"), Some("dontAsk"));
    assert_eq!(after(first, "--allowedTools"), Some("Read"));
    // The prompt, then the tools it may use, a line each here.
    assert_eq!(
        after(first, "--append-system-prompt"),
        Some("Check the build and say how it went.")
    );
    assert!(first.iter().any(|l| l.contains("only these tools: Read.")));
    assert!(!first.contains(&"--resume".to_string()), "{first:?}");
    assert_eq!(
        std::fs::read_to_string(server.home.join("claude-stdin")).unwrap(),
        "Run Daily check"
    );
    assert_eq!(routine(&server)["session"], "sess-1");

    // The next run resumes its session.
    reply(&server, json!({"result": "Green again."}));
    write_routine(&server, &work, now(), json!({}));
    wait(&server, "the second run", || {
        runs(&server).contains("Green again.")
    });
    assert_eq!(after(&starts(&server)[1], "--resume"), Some("sess-1"));

    // Due while that one still runs: skipped.
    std::fs::write(server.home.join("claude-hold"), "").unwrap();
    reply(&server, json!({"result": "Third."}));
    write_routine(&server, &work, now(), json!({}));
    wait(&server, "the third run started", || {
        starts(&server).len() == 3
    });
    write_routine(&server, &work, now(), json!({}));
    wait(&server, "the run skipped", || {
        runs(&server).contains("\tbusy\tits last run was still running")
    });
    std::fs::remove_file(server.home.join("claude-hold")).unwrap();
    wait(&server, "the third run", || {
        runs(&server).contains("Third.")
    });
    assert_eq!(starts(&server).len(), 3);
}

#[test]
fn a_run_still_going_when_the_server_starts_again_is_taken_up() {
    let (mut server, client, work) = set_up();
    std::fs::write(server.home.join("claude-hold"), "").unwrap();
    reply(&server, json!({"result": "Outlived the server."}));
    write_routine(&server, &work, now() + 1, json!({}));
    wait(&server, "its run kept", || {
        server.state.join("routines/run-r1.live").exists()
    });
    // Routines turned off meanwhile: the run is still seen to its end.
    common::write_settings(
        &server.home,
        json!({"notifications": false, "tips": false, "github": false, "linear": false, "routines": false}),
    );
    drop(client);
    server.restart();
    std::fs::remove_file(server.home.join("claude-hold")).unwrap();
    wait(&server, "the run recorded", || {
        runs(&server).contains("Outlived the server.")
    });
    assert_eq!(routine(&server)["session"], "sess-1");
    assert!(!server.state.join("routines/run-r1.live").exists());
    assert_eq!(starts(&server).len(), 1);
}

#[test]
fn a_run_that_reaches_for_a_tool_not_allowed_fails_naming_it_until_allowed() {
    let (server, mut client, work) = set_up();
    reply(
        &server,
        json!({"result": "Done anyway.", "permission_denials": [{"tool_name": "Bash", "tool_use_id": "t1", "tool_input": {"command": "ls"}}]}),
    );
    write_routine(&server, &work, now() + 1, json!({}));
    wait(&server, "the failed run", || {
        runs(&server).contains("\tfailed\tneeds Bash")
    });
    assert_eq!(routine(&server)["needs"], json!(["Bash"]));

    // Its menu's first option allows it and runs it again.
    reply(&server, json!({"result": "Ran with Bash."}));
    server.ok(&["press", "mission-routines"]);
    client.wait_text("Daily check");
    client.type_text("\r");
    client.wait_text("Allow Bash and run it again");
    client.type_text("\r");
    wait(&server, "the run allowed", || {
        runs(&server).contains("Ran with Bash.")
    });
    assert_eq!(routine(&server)["tools"], json!(["Read", "Bash"]));
    assert_eq!(routine(&server)["needs"], json!([]));
    let last = starts(&server).pop().unwrap();
    assert_eq!(after(&last, "--allowedTools"), Some("Read"));
    assert!(last.contains(&"Bash".to_string()), "{last:?}");

    // Its last session opens in a tab of its own, in its directory.
    let tabs = tab_count(&server);
    let runs_so_far = starts(&server).len();
    server.ok(&["press", "mission-routines"]);
    client.wait_text("Daily check");
    client.type_text("\r");
    client.wait_text("Open its last session in a tab");
    client.type_text("/last session\r");
    wait(&server, "its session's tab", || {
        tab_count(&server) == tabs + 1
    });
    wait(&server, "claude resumed there", || {
        starts(&server).len() == runs_so_far + 1
    });
    assert_eq!(starts(&server).pop().unwrap(), ["--resume", "sess-1"]);
}

#[test]
fn a_run_missed_while_asleep_runs_on_waking_within_the_hour_else_is_skipped() {
    let (server, _client, work) = set_up();

    // Missed twenty minutes ago, run on waking.
    reply(&server, json!({"result": "Done late."}));
    write_routine(&server, &work, now() - 1200, json!({}));
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
}

#[test]
fn a_routine_from_before_the_allowed_tools_is_given_them_once() {
    let (server, _client, work) = set_up();
    write_routine(&server, &work, now() + 3600, json!({}));
    // As a build before wrote it: no tools, and its tab's fields.
    let file = server.state.join("routines/r1.json");
    let mut old = routine(&server);
    old.as_object_mut().unwrap().remove("tools");
    old["tabPolicy"] = json!("sameTab");
    old["whenBusy"] = json!("wait");
    std::fs::write(&file, old.to_string()).unwrap();
    wait(&server, "its tools given", || {
        routine(&server)["tools"] == json!(["Read", "Glob", "Grep"])
    });
}

/// The routines in the state directory, by name: whether each is paused,
/// and when it runs.
fn routines_by_name(server: &TestServer) -> Vec<(String, bool, String)> {
    let mut out: Vec<(String, bool, String)> = std::fs::read_dir(server.state.join("routines"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .map(|e| {
            let r: Value =
                serde_json::from_str(&std::fs::read_to_string(e.path()).unwrap()).unwrap();
            (
                r["name"].as_str().unwrap().to_string(),
                r["paused"].as_bool().unwrap(),
                r["when"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn the_starter_routines_are_made_once_running_as_what_they_replace_did() {
    let mut server = TestServer::start_with_starters(
        r#"{"todo_note": true, "todo_note_time": "7", "view_slack": false}"#,
    );
    let mut client = server.attach(120, 40);
    client.wait_text("$");
    wait(&server, "the starters made", || {
        server.state.join("routines/.starters").exists()
    });
    let made = routines_by_name(&server);
    assert_eq!(
        made,
        [
            ("Calendar".into(), true, "every 30m".into()),
            ("Meeting notes".into(), true, "every 30m 7am-9pm".into()),
            ("Slack".into(), true, "every 15m".into()),
            (
                "Today's todo note".into(),
                false,
                "every 1h 7-11pm weekdays".into()
            ),
        ]
    );
    // One deleted stays deleted: a server started again makes none.
    let slack = std::fs::read_dir(server.state.join("routines"))
        .unwrap()
        .flatten()
        .find(|e| std::fs::read_to_string(e.path()).is_ok_and(|t| t.contains("\"Slack\"")))
        .unwrap()
        .path();
    std::fs::remove_file(slack).unwrap();
    drop(client);
    server.restart();
    let mut client = server.attach(120, 40);
    client.wait_text("$");
    std::thread::sleep(Duration::from_secs(2) * common::patience());
    assert_eq!(routines_by_name(&server).len(), 3);
}

#[test]
fn a_note_a_routine_wrote_is_put_in_the_notifications_and_one_outside_the_notes_refused() {
    let server = TestServer::start();
    let notes = server.home.join("Notes/Todos");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("Today's Todo 2026-11-13.md"),
        "# Today's Todo 2026-11-13\n",
    )
    .unwrap();
    let (code, _, err) = server.run_binary(&[
        "new-note",
        "Todos/Today's Todo 2026-11-13.md",
        "today's to-do list",
    ]);
    assert_eq!(code, 0, "{err}");
    let kept: Value = serde_json::from_str(
        &std::fs::read_to_string(server.state.join("new-notes.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(kept[0]["path"], "Todos/Today's Todo 2026-11-13.md");
    assert_eq!(kept[0]["title"], "Today's Todo 2026-11-13");
    assert_eq!(kept[0]["source"], "today's to-do list");
    std::fs::write(server.home.join("elsewhere.md"), "# Not a note\n").unwrap();
    assert_eq!(server.run_binary(&["new-note", "../elsewhere.md"]).0, 1);
}
