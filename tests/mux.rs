//! The multiplexer as someone uses it: a server of the test's own, a client
//! attached to it, keys typed and the screen read back.

mod common;

use common::{TestServer, stand_in};

const ALT: &str = "\x1b";

fn panes(list: &serde_json::Value) -> usize {
    list["sessions"][0]["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["panes"].as_array().unwrap().len())
        .sum()
}

fn tabs(list: &serde_json::Value) -> usize {
    list["sessions"][0]["tabs"].as_array().unwrap().len()
}

#[test]
fn attaching_shows_a_shell_and_the_status_line() {
    let server = TestServer::start();
    let mut client = server.attach(80, 24);
    let screen = client.wait_text("$");
    assert!(
        screen[23].contains("main"),
        "the status line names the session: {:?}",
        screen[23]
    );
    client.type_text("echo hello-$((40+2))\r");
    client.wait_text("hello-42");
}

#[test]
fn splitting_tabs_and_closing() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    // M-L: a pane to the right.
    client.type_text(&format!("{ALT}L"));
    client.wait_for("a second pane", |_| panes(&server.list()) == 2);
    let screen = client.wait_for("a line between the panes", |s| {
        s.iter().take(28).any(|l| l.contains('│'))
    });
    assert!(screen.iter().filter(|l| l.contains('$')).count() >= 1);
    // A new tab from a command, then M-[ back to the first.
    server.ok(&["new-tab", "-n", "second"]);
    client.wait_text("2:second");
    assert_eq!(tabs(&server.list()), 2);
    client.type_text(&format!("{ALT}["));
    client.wait_for("the first tab current", |_| {
        server.list()["sessions"][0]["tabs"][0]["current"] == true
    });
    // M-w closes the focused pane; the tab keeps its other.
    client.type_text(&format!("{ALT}w"));
    client.wait_for("one pane left in the first tab", |_| {
        server.list()["sessions"][0]["tabs"][0]["panes"]
            .as_array()
            .unwrap()
            .len()
            == 1
    });
}

#[test]
fn dragging_the_line_between_panes_resizes_them() {
    let server = TestServer::start();
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    client.type_text(&format!("{ALT}L"));
    client.wait_for("a second pane", |_| panes(&server.list()) == 2);
    let list = server.list();
    let width = |list: &serde_json::Value| {
        list["sessions"][0]["tabs"][0]["panes"][0]["cols"]
            .as_u64()
            .unwrap()
    };
    let before = width(&list);
    let side = list["sessions"][0]["sidebar"]["columns"]
        .as_u64()
        .unwrap_or(0);
    // The line is the column after the left pane; the mouse's are 1-based.
    let line = side + before + 1;
    let to = line - 10;
    client.send(format!("\x1b[<0;{line};5M\x1b[<32;{to};5M\x1b[<0;{to};5m").as_bytes());
    client.wait_for("the left pane narrower by ten", |_| {
        width(&server.list()) == before - 10
    });
    // Back the other way, past where it was.
    let back = to + 15;
    client.send(format!("\x1b[<0;{to};5M\x1b[<32;{back};5M\x1b[<0;{back};5m").as_bytes());
    client.wait_for("the left pane wider by five", |_| {
        width(&server.list()) == before + 5
    });
}

#[test]
fn an_old_tmux_conf_and_keys_json_s_mistakes_are_said_on_attaching() {
    let mut server = TestServer::start();
    let config = server.home.join(".config/switchboard");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("tmux.conf"), "bind -n M-g new-window\n").unwrap();
    std::fs::write(config.join("keys.json"), r#"{"root": {"M-g": "fly"}}"#).unwrap();
    server.restart();
    let mut client = server.attach(120, 30);
    client.wait_text("tmux.conf isn't read any more");
    client.wait_text("fly isn't an action");
    client.type_text("x");
    // A change to keys.json takes effect on the server's next tick.
    std::fs::write(config.join("keys.json"), r#"{"root": {"M-g": "new-tab"}}"#).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2500));
    client.type_text(&format!("{ALT}g"));
    client.wait_for("a tab from M-g", |_| tabs(&server.list()) == 2);
}

#[test]
fn tmux_s_own_prefix_keys_move_between_tabs_and_panes_and_close_a_tab_after_asking() {
    let server = TestServer::start();
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    let prefix = format!("{ALT}b");
    let current = |list: &serde_json::Value| {
        list["sessions"][0]["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .position(|t| t["current"] == true)
    };
    let active = |list: &serde_json::Value| {
        let tab = &list["sessions"][0]["tabs"][0]["panes"];
        tab.as_array()
            .unwrap()
            .iter()
            .position(|p| p["active"] == true)
    };
    // o and ; between the panes of a tab.
    client.type_text(&format!("{ALT}L"));
    client.wait_for("a second pane", |_| panes(&server.list()) == 2);
    client.wait_for("the new pane focused", |_| {
        active(&server.list()) == Some(1)
    });
    client.type_text(&format!("{prefix}o"));
    client.wait_for("round to the first pane", |_| {
        active(&server.list()) == Some(0)
    });
    client.type_text(&format!("{prefix};"));
    client.wait_for("back to the last pane", |_| {
        active(&server.list()) == Some(1)
    });
    // n, p and the numbers between tabs.
    server.ok(&["new-tab", "-n", "second"]);
    client.wait_text("2:second");
    client.type_text(&format!("{prefix}1"));
    client.wait_for("the first tab", |_| current(&server.list()) == Some(0));
    client.type_text(&format!("{prefix}n"));
    client.wait_for("the next tab", |_| current(&server.list()) == Some(1));
    client.type_text(&format!("{prefix}p"));
    client.wait_for("the previous tab", |_| current(&server.list()) == Some(0));
    // &: the tab closed once you say so.
    client.type_text(&format!("{prefix}2"));
    client.wait_for("the second tab", |_| current(&server.list()) == Some(1));
    client.type_text(&format!("{prefix}&"));
    client.wait_text("Close tab 2 second?");
    client.type_text("\r");
    client.wait_for("one tab left", |_| tabs(&server.list()) == 1);
}

#[test]
fn a_program_ending_closes_its_tab() {
    let server = TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    server.ok(&["new-tab", "--", "sh", "-c", "exit 0"]);
    client.wait_for("the short-lived tab gone", |_| tabs(&server.list()) == 1);
}

#[test]
fn send_keys_and_capture_pane() {
    let server = TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    let list = server.list();
    let pane = list["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["send-keys", "-t", &pane, "-l", "echo captured-text"]);
    server.ok(&["send-keys", "-t", &pane, "Enter"]);
    client.wait_text("captured-text");
    let text = server.ok(&["capture-pane", "-t", &pane]);
    assert!(text.contains("captured-text"), "{text}");
}

#[test]
fn the_first_tab_starts_where_switchboard_was_run() {
    let server = TestServer::start();
    let project = server.dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let mut client = common::TestClient::attach_in(&server.socket(), None, 80, 24, Some(&project));
    client.wait_text("$");
    let list = server.list();
    assert_eq!(
        list["sessions"][0]["tabs"][0]["panes"][0]["cwd"],
        project.to_string_lossy().as_ref(),
        "{list}"
    );
    // A second session from a directory that has gone starts at home.
    let gone = server.dir.path().join("gone");
    let mut other =
        common::TestClient::attach_in(&server.socket(), Some("work"), 80, 24, Some(&gone));
    other.wait_text("$");
    let list = server.list();
    let work = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "work")
        .expect("the work session");
    assert_eq!(
        work["tabs"][0]["panes"][0]["cwd"],
        server.home.to_string_lossy().as_ref(),
        "{list}"
    );
}

#[test]
fn a_session_takes_the_size_of_the_terminal_left_when_another_goes() {
    let server = TestServer::start();
    let mut big = server.attach(120, 40);
    big.wait_text("$");
    let mut small = server.attach(80, 24);
    small.wait_text("$");
    let size = |server: &TestServer| {
        let list = server.list();
        (
            list["sessions"][0]["cols"].as_u64().unwrap(),
            list["sessions"][0]["rows"].as_u64().unwrap(),
        )
    };
    assert_eq!(size(&server), (80, 24), "the one attached last");
    small.detach();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while size(&server) != (120, 40) {
        assert!(
            std::time::Instant::now() < deadline,
            "still {:?}",
            size(&server)
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
fn switch_client_takes_the_terminal_to_another_session_s_tab() {
    let server = TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    // A second session, made by a client that then goes.
    let mut other = common::TestClient::attach(&server.socket(), Some("work"), 80, 24);
    other.wait_text("$");
    other.detach();
    let list = server.list();
    let work = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "work")
        .expect("the work session")["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["switch-client", "-t", &work]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while server.list()["clients"][0]["session"] != "work" {
        assert!(
            std::time::Instant::now() < deadline,
            "{:?}",
            server.list()["clients"]
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
fn detaching_leaves_it_running_and_attaching_again_shows_it() {
    let mut server = TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    client.type_text("echo still-here\r");
    client.wait_text("still-here");
    // The prefix, then d.
    client.type_text(&format!("{ALT}b"));
    client.type_text("d");
    let said = client.wait_exit().unwrap_or_default();
    assert!(said.contains("detached"), "{said}");
    assert!(server.running());
    let mut again = server.attach(80, 24);
    again.wait_text("still-here");
}

#[test]
fn options_are_set_on_panes_and_tabs() {
    let server = TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    let list = server.list();
    let pane = list["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let tab = list["sessions"][0]["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["set", "-t", &pane, "@agent_state", "working"]);
    server.ok(&["set", "-t", &tab, "@tab_name", "Shaping: things"]);
    assert_eq!(
        server.ok(&["show", "-t", &pane, "@agent_state"]).trim(),
        "working"
    );
    assert_eq!(
        server.ok(&["show", "-t", &tab, "@tab_name"]).trim(),
        "Shaping: things"
    );
    server.ok(&["set", "-u", "-t", &pane, "@agent_state"]);
    assert_eq!(server.ok(&["show", "-t", &pane, "@agent_state"]), "");
}

#[test]
fn killing_the_server_ends_its_clients() {
    let mut server = TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    server.ok(&["kill-server"]);
    client.wait_exit();
    assert!(
        server.wait_exit(std::time::Duration::from_secs(5)),
        "the server ended"
    );
}

#[test]
fn a_command_with_no_server_says_so() {
    let dir = common::TempDir::new("sbt-none");
    let out = std::process::Command::new(common::BIN)
        .arg("list")
        .env("HOME", dir.path())
        .env("SWITCHBOARD_STATE_DIR", dir.path().join("s"))
        .env_remove("SWITCHBOARD")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no server is running"));
}

/// Where a pane is: its tab, its place among the tab's panes, its size.
fn place(list: &serde_json::Value, pane: &str) -> Option<(String, usize, u64, u64)> {
    list["sessions"][0]["tabs"]
        .as_array()?
        .iter()
        .find_map(|t| {
            let panes = t["panes"].as_array()?;
            let at = panes.iter().position(|p| p["id"] == pane)?;
            Some((
                t["id"].as_str()?.to_string(),
                at,
                panes[at]["cols"].as_u64()?,
                panes[at]["rows"].as_u64()?,
            ))
        })
}

#[test]
fn respawn_starts_another_program_in_the_same_pane() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    // A program that takes a second to end once hung up, so its end comes
    // after the one started in its place.
    let pane = server
        .ok(&[
            "split",
            "-d",
            "--",
            "sh",
            "-c",
            "trap 'sleep 1; exit 0' HUP; echo first-program; while :; do sleep 0.1; done",
        ])
        .trim()
        .to_string();
    client.wait_for("the first program", |_| {
        server
            .ok(&["capture-pane", "-t", &pane])
            .contains("first-program")
    });
    let before = place(&server.list(), &pane).expect("the pane is listed");

    // Still running, it is kept unless -k says to end it, as tmux did.
    let (code, _, err) = server.run(&["respawn-pane", "-t", &pane, "--", "sh", "-c", "exit 0"]);
    assert_eq!(code, 1);
    assert!(err.contains("still running"), "{err}");

    server.ok(&[
        "respawn-pane",
        "-k",
        "-t",
        &pane,
        "--",
        "sh",
        "-c",
        "echo second-program; sleep 30",
    ]);
    client.wait_for("the second program in its place", |_| {
        let text = server.ok(&["capture-pane", "-t", &pane]);
        text.contains("second-program") && !text.contains("first-program")
    });
    // The first program's end has come by now, and the pane is still there,
    // where it was, as big as it was.
    std::thread::sleep(std::time::Duration::from_millis(1800));
    assert_eq!(place(&server.list(), &pane), Some(before.clone()));
    assert_eq!(panes(&server.list()), 2);

    // `switchboard respawn` does the same for the skills, ending what runs.
    server.ok(&[
        "respawn",
        "-t",
        &pane,
        "sh",
        "-c",
        "echo third-program; sleep 30",
    ]);
    client.wait_for("the third program", |_| {
        server
            .ok(&["capture-pane", "-t", &pane])
            .contains("third-program")
    });
    assert_eq!(place(&server.list(), &pane), Some(before));
}

/// The id of the first pane of the first tab.
fn first_pane(server: &TestServer) -> String {
    server.list()["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Whether `needle` is anywhere in what the client was sent.
fn sent(raw: &[u8], needle: &[u8]) -> bool {
    raw.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn graphics_commands_reach_the_terminal_only_from_a_pane_allowed_them() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let pane = first_pane(&server);
    // Written with printf's octal escapes, so the line typed holds no ESC,
    // and the word it ends with split by quotes, so what is waited for is
    // what the command printed, never its line being typed.
    let print = |payload: &str, done: &str| {
        let (head, tail) = done.split_at(done.len() / 2);
        server.ok(&[
            "send-keys",
            "-t",
            &pane,
            "-l",
            &format!("printf '\\033_Ga=T,q=2;{payload}\\033\\\\'; echo {head}''{tail}"),
        ]);
        server.ok(&["send-keys", "-t", &pane, "Enter"]);
    };
    print("Tk9QRQ==", "printed-unmarked");
    client.wait_text("printed-unmarked");
    client.screen();
    assert!(
        !sent(&client.raw, b"\x1b_Ga=T,q=2;Tk9QRQ==\x1b\\"),
        "no program's command is passed"
    );

    server.ok(&["set", "-t", &pane, "@allow_passthrough", "1"]);
    print("WUVT", "printed-marked");
    client.wait_text("printed-marked");
    client.wait_for("the command passed on", |_| true);
    assert!(sent(&client.raw, b"\x1b_Ga=T,q=2;WUVT\x1b\\"));

    // The placeholder cells the command's image is drawn over reach the
    // terminal as the program wrote them, marks and colour.
    server.ok(&[
        "send-keys",
        "-t",
        &pane,
        "-l",
        "printf '\\033[38;2;0;0;42m\\364\\216\\273\\256\\314\\205\\314\\215\\033[0m\\n'",
    ]);
    server.ok(&["send-keys", "-t", &pane, "Enter"]);
    client.wait_for("the placeholder cell drawn", |s| {
        s.iter().any(|l| l.contains('\u{10EEEE}'))
    });
    assert!(sent(&client.raw, "\u{10EEEE}\u{305}\u{30D}".as_bytes()));
    let (line, col) = client.find_cell('\u{10EEEE}').expect("the cell");
    assert_eq!(client.marks(line, col), vec!['\u{305}', '\u{30D}']);
    assert_eq!(client.fg_rgb(line, col), Some((0, 0, 42)));
}

#[test]
fn the_cmd_keys_reach_only_a_pane_marked_for_them() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let pane = first_pane(&server);
    server.ok(&["send-keys", "-t", &pane, "-l", "cat -v"]);
    server.ok(&["send-keys", "-t", &pane, "Enter"]);
    // Cmd-S as Ghostty sends it, then a line to show what got through.
    client.type_text("\x1b[115;8u");
    client.type_text("unmarked\r");
    client.wait_text("unmarked");
    assert!(
        !client.screen().iter().any(|l| l.contains("115;8u")),
        "dropped, not read as a Ctrl key"
    );
    server.ok(&["set", "-t", &pane, "@notes_cmd_keys", "1"]);
    client.type_text("\x1b[115;8u");
    client.type_text("marked\r");
    client.wait_text("^[[115;8umarked");
}

#[test]
fn ctrl_shift_p_reaches_a_program_that_asked_for_it_and_a_shell_gets_ctrl_p() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    // Ctrl-Shift-P as a terminal sends it under the kitty keyboard protocol,
    // and under modifyOtherKeys, to a program that asked for neither: the
    // byte Ctrl-P types, from both.
    let pane = first_pane(&server);
    server.ok(&["send-keys", "-t", &pane, "-l", "cat -v"]);
    server.ok(&["send-keys", "-t", &pane, "Enter"]);
    client.type_text("\x1b[112;6u\x1b[27;6;112~plain\r");
    client.wait_text("^P^Pplain");

    // A program that asks for keys told apart, either way, gets it whole.
    for (ask, said) in [("\\033[>1u", "kitty"), ("\\033[>4;2m", "xterm")] {
        let script = format!("printf '{ask}ready-{said}\\n'; exec cat -v");
        server.ok(&["new-tab", "-n", said, "--", "sh", "-c", &script]);
        client.wait_text(&format!("ready-{said}"));
        client.type_text("\x1b[112;6u");
        client.type_text(&format!("{said}\r"));
        client.wait_text(&format!("^[[112;6u{said}"));
    }

    // Notes asks for them, and opens its palette on Ctrl-Shift-P.
    let notes = server.home.join("Notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(notes.join("Plan.md"), "# Plan\n\n- [ ] a thing\n").unwrap();
    server.ok(&["new-tab", "-n", "notes", "--", common::BIN, "notes"]);
    client.wait_text("Plan");
    client.type_text("\x1b[112;6u");
    client.wait_text("command>");
}

/// Stand-ins for the desktop's clipboard tools, first on the PATH, so a
/// copy reaches a file in the test's home (`copied`) rather than the real
/// clipboard.
fn stand_in_clipboard(server: &TestServer) {
    stand_in::desktop(
        &server.home.join("bin"),
        stand_in::CLIPBOARD,
        &server.home.join("clipboard-log"),
        &server.home.join("copied"),
    );
}

/// Waits until the client has been sent `text` for the clipboard (OSC 52).
fn wait_copied(client: &mut common::TestClient, text: &str) {
    let osc = format!(
        "\x1b]52;c;{}\x07",
        switchboard::util::base64(text.as_bytes())
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !sent(&client.raw, osc.as_bytes()) {
        assert!(
            std::time::Instant::now() < deadline,
            "{text:?} wasn't copied"
        );
        client.screen();
    }
}

#[test]
fn copy_mode_selects_with_the_keyboard_and_copies_through_the_clipboard() {
    let server = TestServer::start();
    stand_in_clipboard(&server);
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    client.type_text("echo copy-me-please\r");
    client.wait_for("the output", |s| s.iter().any(|l| l == "copy-me-please"));
    // The prefix, then [: copy mode, its cursor on the prompt under the
    // output. Up a line, to its start, v, to its end, y.
    client.type_text(&format!("{ALT}b"));
    client.type_text("[");
    client.wait_text("[0]");
    client.type_text("k0v$y");
    wait_copied(&mut client, "copy-me-please");
    // y leaves copy mode.
    client.wait_for("copy mode left", |s| !s.iter().any(|l| l.contains("[0]")));

    // ? looks up for what is typed in its box; e stretches the selection to
    // the end of the word found.
    client.type_text(&format!("{ALT}b"));
    client.type_text("[");
    client.wait_text("[0]");
    client.type_text("?");
    client.wait_text("Search up");
    client.type_text("echo\r");
    client.wait_for("the box closed", |s| {
        !s.iter().any(|l| l.contains("Search up"))
    });
    client.type_text("vey");
    wait_copied(&mut client, "echo");

    // q leaves without copying.
    client.type_text(&format!("{ALT}b"));
    client.type_text("[");
    client.wait_text("[0]");
    client.type_text("q");
    client.wait_for("copy mode left", |s| !s.iter().any(|l| l.contains("[0]")));
}

#[test]
fn a_panes_program_asking_the_terminals_colours_is_told_its_own() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let script = r#"read -r go; printf '\033]11;?\007\033]4;4;?\007'; IFS= read -rs -d $'\a' bg; IFS= read -rs -d $'\a' blue; echo "bg:${bg#*;} blue:${blue#*;*;}"; sleep 30"#;
    server.ok(&["new-tab", "-n", "ask", "--", "bash", "-c", script]);
    client.wait_for("the tab open", |_| {
        server.list()["sessions"][0]["tabs"]
            .as_array()
            .unwrap()
            .len()
            == 2
    });
    // The terminal's answers to the server's questions as it attached.
    client.send(b"\x1b]11;rgb:fafa/f8f8/f0f0\x1b\\\x1b]4;4;rgb:81/a2/be\x1b\\");
    client.type_text("go\r");
    client.wait_text("bg:rgb:fafa/f8f8/f0f0 blue:rgb:8181/a2a2/bebe");
}

#[test]
fn a_panes_program_is_told_the_terminals_own_cell() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    // A program that asks its window's size in pixels once told to.
    let script = r#"read -r go; printf '\033[14t'; IFS= read -rs -d t reply; echo "px:${reply#*[}"; sleep 30"#;
    server.ok(&["new-tab", "-n", "ask", "--", "bash", "-c", script]);
    client.wait_for("the tab open", |_| {
        server.list()["sessions"][0]["tabs"]
            .as_array()
            .unwrap()
            .len()
            == 2
    });
    // A terminal with cells 10 pixels wide and 20 high says so as it resizes.
    client.resize_pixels(100, 30, 1000, 600);
    client.type_text("go\r");
    let shown = client.wait_text("px:4;");
    let line = shown.iter().find(|l| l.contains("px:4;")).unwrap();
    let said: Vec<u64> = line
        .split("px:4;")
        .nth(1)
        .unwrap()
        .split(';')
        .map(|n| n.trim().parse().unwrap())
        .collect();
    let pane = &server.list()["sessions"][0]["tabs"][1]["panes"][0];
    let (cols, rows) = (
        pane["cols"].as_u64().unwrap(),
        pane["rows"].as_u64().unwrap(),
    );
    assert_eq!(said, vec![rows * 20, cols * 10], "{line}");
}
