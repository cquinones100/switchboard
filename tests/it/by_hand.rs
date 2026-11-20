// What a command typed by hand in a pane (by you in a shell, or by Claude's
// Bash tool, which may run `switchboard set` and `switchboard respawn`
// without asking) may do: set options on its own tab but Switchboard's own
// marks, name another tab, and respawn its own tab's pane as one of the
// worktree commands or Claude resuming a session; Switchboard's own programs
// asking the same keep doing what they need.

use crate::common::{self, TestServer};

/// The new tab's pane, from `new-tab`'s answer (`@<tab> %<pane>`).
fn new_tab(server: &TestServer, name: &str) -> (String, String) {
    let out = server.ok(&["new-tab", "-d", "-n", name]);
    let mut ids = out.split_whitespace().map(str::to_string);
    (ids.next().unwrap(), ids.next().unwrap())
}

fn shown(server: &TestServer, target: &str, name: &str) -> String {
    server.ok(&["show", "-t", target, name]).trim().to_string()
}

#[test]
fn a_command_typed_in_a_pane_sets_its_own_tab_and_no_marks_of_switchboards() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (_, mine) = new_tab(&server, "mine");
    let (other_tab, other) = new_tab(&server, "other");

    for (option, value) in [
        ("@land", "1"),
        ("@land_work", "1"),
        ("@autopilot_land", "1"),
        ("@autopilot_handsfree", "1"),
        ("@allow_passthrough", "on"),
        ("@notes_pane", "1"),
    ] {
        let (code, _, err) = server.typed_in(&mine, &["set", option, value]);
        assert_eq!(code, 1, "{option}: {err}");
        assert!(err.contains("Switchboard's own"), "{option}: {err}");
        assert_eq!(shown(&server, &mine, option), "", "{option}");
    }
    client.wait_text("Switchboard's own");
    let (code, _, err) = server.typed_in(&mine, &["set", "-u", "@land"]);
    assert_eq!(code, 1, "{err}");

    // The landing's own mark, and anything else on its own tab, it sets.
    server.typed_in(&mine, &["set", "@autopilot_pushed", "abc1234"]);
    assert_eq!(shown(&server, &mine, "@autopilot_pushed"), "abc1234");
    let (code, _, err) = server.typed_in(&mine, &["set", "@tab_note", "Mine.md"]);
    assert_eq!(code, 0, "{err}");

    // Another tab: its name alone.
    let (code, _, err) = server.typed_in(&mine, &["set", "-t", &other_tab, "@tab_name", "x"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(shown(&server, &other_tab, "@tab_name"), "x");
    for target in [other_tab.as_str(), other.as_str()] {
        let (code, _, err) = server.typed_in(&mine, &["set", "-t", target, "@tab_note", "N.md"]);
        assert_eq!(code, 1, "{target}: {err}");
        assert!(err.contains("own tab only"), "{err}");
    }
    assert_eq!(shown(&server, &other_tab, "@tab_note"), "");

    // Switchboard's own programs ask without typing it, as Notes marks its
    // pane and sets another tab's note.
    server.ok(&["set", "-t", &mine, "@allow_passthrough", "1"]);
    server.ok(&["set", "-t", &other_tab, "@tab_note", "N.md"]);
    assert_eq!(shown(&server, &other_tab, "@tab_note"), "N.md");
}

#[test]
fn a_mistyped_command_in_a_pane_fails_on_its_stderr_with_no_box_and_a_refusal_still_shows_one() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (_, mine) = new_tab(&server, "mine");
    let (other_tab, _) = new_tab(&server, "other");

    // The target left out, so `-t` takes the option's name.
    let (code, _, err) = server.typed_in(&mine, &["set", "-t", "@tab_name", "x"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("isn't a pane"), "{err}");
    // Typed after the command answered, so drawn after any box it opened.
    client.type_text("echo mark-$((40+2))\r");
    let screen = client.wait_text("mark-42");
    assert!(
        !screen.iter().any(|line| line.contains("isn't a pane")),
        "{screen:#?}"
    );

    let (code, _, err) = server.typed_in(&mine, &["set", "-t", &other_tab, "@tab_note", "N.md"]);
    assert_eq!(code, 1, "{err}");
    client.wait_text("sets @tab_note on its own tab");
}

#[test]
fn a_respawn_typed_in_a_pane_starts_only_the_worktree_commands_or_claude_resuming() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let (_, mine) = new_tab(&server, "mine");
    let (_, other) = new_tab(&server, "other");

    let program = |words: &[&str]| {
        let mut args = vec!["respawn-program", "-t"];
        args.extend_from_slice(words);
        args.iter().map(|s| s.to_string()).collect::<Vec<_>>()
    };
    let typed = |words: Vec<String>| {
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        server.typed_in(&mine, &words)
    };

    let (code, _, err) = typed(program(&[&mine, "--", "sh", "-c", "echo pwned"]));
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("starts only wt, wtb or claude"), "{err}");
    let (code, _, err) = typed(program(&[&other, "--", "claude", "--resume", "abc"]));
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("its own tab only"), "{err}");
    // The raw command reaches no program typed in a pane either.
    let (code, _, err) = server.typed_in(&mine, &["respawn-pane", "-k", "--", "sh", "-c", "id"]);
    assert_eq!(code, 1, "{err}");
    assert!(
        !server.ok(&["capture-pane", "-t", &mine]).contains("pwned"),
        "nothing it refused ran"
    );

    // Claude resuming a session, in its own tab, it starts.
    let (code, _, err) = typed(program(&[&mine, "--", "claude", "--resume", "abc"]));
    assert_eq!(code, 0, "{err}");
}

/// A command that runs `function` as a tab the worktree commands open
/// would (`worktree::tab_running`'s shape), the function missing here, so
/// it does nothing but wait.
fn running(function: &str) -> Vec<String> {
    [
        "sh",
        "-i",
        "-c",
        &format!("true init sh; {function} \"$@\"; exec sleep 600"),
        function,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[test]
fn a_tab_opening_a_worktrees_work_takes_land_from_the_setting() {
    let open = |server: &TestServer, function: &str| {
        let mut words = vec!["new-tab".to_string(), "-d".into(), "--".into()];
        words.extend(running(function));
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        let out = server.ok(&words);
        out.split_whitespace().next().unwrap().to_string()
    };

    let on = TestServer::start_with_settings(r#"{ "land_without_asking": true }"#);
    let mut client = on.attach(100, 30);
    client.wait_text("$");
    for function in ["wt", "wtb"] {
        let tab = open(&on, function);
        assert_eq!(shown(&on, &tab, "@land"), "1", "{function}");
    }
    let review = open(&on, "wtpr");
    assert_eq!(shown(&on, &review, "@land"), "", "a PR's review isn't work");
    let (plain, pane) = new_tab(&on, "plain");
    assert_eq!(shown(&on, &plain, "@land"), "", "nor a plain tab");
    // Handed a worktree's work, as /wt hands its tab over.
    let mut words = vec![
        "respawn-pane".to_string(),
        "-k".into(),
        "-t".into(),
        pane.clone(),
        "--".into(),
    ];
    words.extend(running("wt"));
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    on.ok(&words);
    assert_eq!(shown(&on, &plain, "@land"), "1", "handed the work");

    // The setting turned off reaches the tabs opened for work, and on again
    // marks them again; a plain tab stays as it is.
    let (other, _) = new_tab(&on, "other");
    let follows = |value: bool, want: &str| {
        common::write_settings(
            &on.home,
            serde_json::json!({ "land_without_asking": value }),
        );
        let deadline = common::deadline(std::time::Duration::from_secs(10));
        while shown(&on, &plain, "@land") != want {
            assert!(
                std::time::Instant::now() < deadline,
                "@land never became {want:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(shown(&on, &review, "@land"), "", "a PR's review left alone");
        assert_eq!(shown(&on, &other, "@land"), "", "a plain tab left alone");
    };
    follows(false, "");
    follows(true, "1");

    let off = TestServer::start();
    let mut client = off.attach(100, 30);
    client.wait_text("$");
    let tab = open(&off, "wt");
    assert_eq!(shown(&off, &tab, "@land"), "", "the setting off");
}
