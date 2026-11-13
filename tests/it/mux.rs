//! The multiplexer as someone uses it: a server of the test's own, a client
//! attached to it, keys typed and the screen read back.

use crate::common;

use common::{TestServer, sent, sixel_at, stand_in};

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
    // A new tab from a command, then M-{ back to the first.
    server.ok(&["new-tab", "-n", "second"]);
    client.wait_text("2:second");
    assert_eq!(tabs(&server.list()), 2);
    client.type_text(&format!("{ALT}{{"));
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
fn new_tab_comes_to_the_front_and_with_d_stays_behind() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let current = |i: usize| server.list()["sessions"][0]["tabs"][i]["current"] == true;
    server.ok(&["new-tab", "-n", "asked"]);
    assert!(current(1), "a tab you asked for is in front");
    server.ok(&["new-tab", "-d", "-n", "behind"]);
    assert_eq!(tabs(&server.list()), 3);
    assert!(current(1), "-d leaves the tab in front where it is");
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
fn ctrl_alt_shift_h_shrinks_the_left_pane() {
    let server = TestServer::start();
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    client.type_text(&format!("{ALT}L"));
    client.wait_for("a second pane", |_| panes(&server.list()) == 2);
    let width = |list: &serde_json::Value| {
        list["sessions"][0]["tabs"][0]["panes"][0]["cols"]
            .as_u64()
            .unwrap()
    };
    let before = width(&server.list());
    // Into the left pane, which has no divider on its left: C-M-S-h, as the
    // kitty protocol sends it, moves its right one left.
    client.send(format!("{ALT}[1;3D").as_bytes());
    client.send(b"\x1b[104;8u");
    client.wait_for("the left pane narrower by five", |_| {
        width(&server.list()) == before - 5
    });
    client.send(b"\x1b[108;8u");
    client.wait_for("the left pane back as it was", |_| {
        width(&server.list()) == before
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
    std::thread::sleep(std::time::Duration::from_millis(2500) * common::patience());
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
    // With -D, what is drawn dimmed is left blank.
    server.ok(&[
        "send-keys",
        "-t",
        &pane,
        "-l",
        "printf 'typed-%s \\033[2mhinted-%s\\033[0m\\n' x y",
    ]);
    server.ok(&["send-keys", "-t", &pane, "Enter"]);
    client.wait_text("typed-x hinted-y");
    let typed = server.ok(&["capture-pane", "-D", "-t", &pane]);
    assert!(typed.contains("typed-x"), "{typed}");
    assert!(!typed.contains("hinted-y"), "{typed}");
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
    let deadline = common::deadline(std::time::Duration::from_secs(5));
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
    let deadline = common::deadline(std::time::Duration::from_secs(5));
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

/// The tests ask the server from their own process (`TestServer::run`), so
/// what that gives has to be what the binary run as a process gives.
#[test]
fn a_command_asked_in_process_answers_as_the_binary_does() {
    let server = TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    let tab = server.list()["sessions"][0]["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["set", "-t", &tab, "@tab_name", "asked"]);
    for args in [
        &["show", "-t", &tab, "@tab_name"][..],
        &["list-keys"],
        &["select-tab", "-t", "@999"],
        &["select-tab"],
        &["list", "--help"],
    ] {
        let asked = server.run(args);
        assert_eq!(asked, server.run_binary(args), "switchboard {args:?}");
        assert_eq!(asked, server.ask(args), "switchboard {args:?}");
    }
    let (code, out, _) = server.run(&["show", "-t", &tab, "@tab_name"]);
    assert_eq!((code, out.trim()), (0, "asked"));
    let (code, out, err) = server.run(&["select-tab", "-t", "@999"]);
    assert_ne!(code, 0, "a tab that isn't there");
    assert!(out.is_empty() && !err.is_empty(), "{out:?} {err:?}");
    // A program of its own is still the binary run.
    let (code, _, err) = server.run(&["no-such-command"]);
    assert_eq!(code, 2);
    assert!(err.contains("no command named no-such-command"), "{err}");
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
    std::thread::sleep(std::time::Duration::from_millis(1800) * common::patience());
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
    // With kitty's protocol, the default, the terminal is asked nothing
    // more and sent no sixel.
    assert!(!sent(&client.raw, b"\x1b[16t"));
    assert!(!sent(&client.raw, b"\x1bP"));
}

#[test]
fn with_sixel_chosen_the_server_draws_a_pane_s_picture_where_its_placeholders_are() {
    let server = TestServer::start_with_settings(r#"{"image_protocol": "sixel"}"#);
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    client.wait_sent("the terminal asked for its cell", b"\x1b[16t");
    // Its answer, a cell of 13 by 25 pixels (Windows Terminal's is its
    // sixel's virtual cell, 10 by 20, which a terminal that doesn't
    // answer is taken to have too).
    client.send(b"\x1b[6;25;13t");
    let pane = first_pane(&server);
    server.ok(&["set", "-t", &pane, "@allow_passthrough", "1"]);
    let (line, col) = server.show_picture(&mut client, &pane);
    // Drawn at the placeholders' top left: four cells of 13 pixels wide,
    // two of 25 tall made a whole number of six-row bands.
    let at = format!("\x1b[{};{}H\x1bP0;1;0q\"1;1;52;48", line + 1, col + 1);
    client.wait_sent("the picture drawn as sixel", at.as_bytes());
    let sixel = sixel_at(&client.raw, line, col).unwrap();
    for colour in ["100;0;0", "0;100;0", "0;0;100", "100;100;0"] {
        assert!(
            sixel.contains(&format!(";2;{colour}")),
            "{colour}: {sixel:?}"
        );
    }
    // The command went no further, and the placeholders reached the
    // terminal as blanks.
    assert!(!sent(&client.raw, b"\x1b_G"), "no kitty command passed on");
    assert!(!sent(&client.raw, "\u{10EEEE}".as_bytes()));
    assert!(
        client.screen()[line].ends_with("Q7"),
        "the placeholders blank after the line's text"
    );

    // The pane's screen cleared: the picture's cells are erased, taking it
    // away, and it isn't drawn again.
    let before = client.raw.len();
    server.ok(&["send-keys", "-t", &pane, "-l", "printf '\\033[H\\033[2J'"]);
    server.ok(&["send-keys", "-t", &pane, "Enter"]);
    client.wait_for("the picture's line cleared", |s| {
        !s.iter().any(|l| l.contains("Q7"))
    });
    let erased = format!("\x1b[{};{}H\x1b[4X", line + 1, col + 1);
    client.wait_sent("its cells erased", erased.as_bytes());
    assert!(
        !sent(&client.raw[before..], b"\x1bP"),
        "and not drawn again"
    );
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

    // Notes asks for them, and opens the workspace's palette on
    // Ctrl-Shift-P, listing its own commands first.
    let notes = server.home.join("Notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(notes.join("Plan.md"), "# Plan\n\n- [ ] a thing\n").unwrap();
    server.ok(&["new-tab", "-n", "notes", "--", common::BIN, "notes"]);
    client.wait_text("Plan");
    client.type_text("\x1b[112;6u");
    client.wait_text("action>");
    client.wait_text("Notes: Find a note");
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
    let deadline = common::deadline(std::time::Duration::from_secs(10));
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

#[test]
fn the_tests_wait_longer_on_a_busy_machine() {
    assert!(common::patience() >= 1);
    assert_eq!(common::patience_from(Some("3"), 0.5, 10), 3);
    assert_eq!(common::patience_from(Some("0"), 0.5, 10), 1);
    assert_eq!(common::patience_from(Some("lots"), 14.0, 10), 2);
    assert_eq!(common::patience_from(Some("20"), 0.5, 10), 8);
    assert_eq!(common::patience_from(None, 14.0, 10), 2);
    assert_eq!(common::patience_from(None, 1.0, 10), 1);
    assert_eq!(common::patience_from(None, 0.0, 10), 1);
    assert_eq!(common::patience_from(None, 200.0, 10), 8);
}

/// A server with a first tab and two more opened behind it, gone to from
/// the first to the third and then the second.
fn three_tabs_visited(server: &TestServer) {
    server.ok(&["new-tab", "-d", "-n", "second"]);
    server.ok(&["new-tab", "-d", "-n", "third"]);
    let id = |i: usize| {
        server.list()["sessions"][0]["tabs"][i]["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let (second, third) = (id(1), id(2));
    server.ok(&["select-tab", "-t", &third]);
    server.ok(&["select-tab", "-t", &second]);
}

fn is_current(server: &TestServer, i: usize) -> bool {
    server.list()["sessions"][0]["tabs"][i]["current"] == true
}

#[test]
fn alt_braces_go_back_and_forward_through_the_tabs_visited() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    three_tabs_visited(&server);
    for (key, i, what) in [
        ("{", 2, "back to the third tab"),
        ("{", 0, "back to the first tab"),
        ("}", 2, "forward to the third tab"),
    ] {
        client.type_text(&format!("{ALT}{key}"));
        client.wait_for(what, |_| is_current(&server, i));
    }
}

#[test]
fn with_tab_back_forward_off_alt_braces_go_to_the_tab_beside() {
    let server = TestServer::start_with_settings(r#"{"tab_back_forward": false}"#);
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    three_tabs_visited(&server);
    client.type_text(&format!("{ALT}{{"));
    client.wait_for("the first tab, on the left", |_| is_current(&server, 0));
}

#[test]
fn a_pane_scrolled_back_shows_a_scroll_bar_until_it_is_at_the_bottom() {
    let server = TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    client.type_text("seq 1 300\r");
    // All of it out and the prompt back, so the wheel has a history to
    // scroll into.
    client.wait_for("the prompt after the output", |s| {
        s.iter()
            .position(|l| l.trim() == "300")
            .is_some_and(|at| s[at + 1..].iter().any(|l| l.contains('$')))
    });
    let thumb_rows = |screen: &[String]| {
        screen
            .iter()
            .filter(|l| l.trim_end_matches(' ').ends_with('┃'))
            .count()
    };
    assert_eq!(thumb_rows(&client.screen()), 0, "no bar on the live screen");
    client.send(b"\x1b[<64;40;10M");
    let screen = client.wait_for("the scroll bar", |s| thumb_rows(s) > 0);
    assert!(
        screen.iter().any(|l| l.ends_with('│')),
        "a track beside the thumb: {screen:#?}"
    );
    // Dragging the thumb to the bottom ends the wheel's copy mode, so what
    // is typed reaches the shell again.
    client.send(b"\x1b[<0;80;3M\x1b[<32;80;23M\x1b[<0;80;23m");
    client.wait_for("the bar gone at the bottom", |s| thumb_rows(s) == 0);
    client.type_text("echo typed-$((40+2))\r");
    client.wait_text("typed-42");
}

/// Each pane's columns in the first tab.
fn widths(list: &serde_json::Value) -> Vec<u64> {
    list["sessions"][0]["tabs"][0]["panes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["cols"].as_u64().unwrap())
        .collect()
}

#[test]
fn a_lone_pane_is_capped_on_a_wide_screen_and_a_split_uses_it_all() {
    let server = TestServer::start();
    let mut client = server.attach(365, 40);
    client.wait_text("$");
    client.wait_for("the lone pane at 165 columns", |_| {
        widths(&server.list()) == [165]
    });
    // The rule at the pane's right edge, against the sidebar.
    let side = server.list()["sessions"][0]["sidebar"]["columns"]
        .as_u64()
        .unwrap_or(0) as usize;
    client.wait_for("the rule at the pane's right edge", |screen| {
        screen.iter().any(|row| {
            let cells: Vec<char> = row.chars().collect();
            cells.get(side + 165) == Some(&'│')
        })
    });
    client.type_text(&format!("{ALT}L"));
    client.wait_for("two panes sharing the whole width", |_| {
        let w = widths(&server.list());
        w.len() == 2 && w.iter().sum::<u64>() + 1 == 365 - side as u64
    });
    let second = server.list()["sessions"][0]["tabs"][0]["panes"][1]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["kill-pane", "-t", &second]);
    client.wait_for("back to one pane, capped again", |_| {
        widths(&server.list()) == [165]
    });
    common::write_settings(&server.home, serde_json::json!({"lone_pane_width": "off"}));
    client.wait_for("off, the pane fills the width", |_| {
        widths(&server.list()) == [365 - side as u64]
    });
}
