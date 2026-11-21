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
fn attaching_shows_a_shell_under_the_tabs_header() {
    let server = TestServer::start();
    let mut client = server.attach(80, 24);
    let screen = client.wait_text("$");
    assert!(
        screen[0].contains("1 home"),
        "the header names the tab: {:?}",
        screen[0]
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
    client.wait_text("2 second");
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
fn alt_shift_bracket_in_the_kitty_form_goes_back_and_forward() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    client.wait_text("2 second");
    let current = |i: usize| server.list()["sessions"][0]["tabs"][i]["current"] == true;
    // Ghostty's Alt+Shift+[ without the alternate key: Shift and `[`.
    client.type_text("\x1b[91;4u");
    client.wait_for("the first tab current", |_| current(0));
    // With the alternate key, as the client asks for: `]` shifted to `}`.
    client.type_text("\x1b[93:125;4u");
    client.wait_for("the second tab current again", |_| current(1));
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
    client.wait_text("2 second");
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
    // A terminal that draws what a pane passes on: one that gets no
    // pictures has their cells blank (src/mux/graphics.rs).
    let mut client = server.attach_as("xterm-ghostty", 100, 30);
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
    client.wait_text("╭─ Actions");
    client.wait_text("Notes: Start work on this");
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
fn only_the_pane_in_front_puts_text_on_the_clipboard() {
    let server = TestServer::start();
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    let osc = |text: &str| {
        format!(
            "\x1b]52;c;{}\x07",
            switchboard::util::base64(text.as_bytes())
        )
    };
    // A program in a tab behind copies, then says it did.
    let behind = format!(
        "printf '{}'; echo copied-behind; sleep 30",
        osc("from-behind")
            .replace('\x1b', "\\033")
            .replace('\x07', "\\007")
    );
    server.ok(&["new-tab", "-d", "-n", "behind", "--", "sh", "-c", &behind]);
    let pane = |i: usize| {
        server.list()["sessions"][0]["tabs"][i]["panes"][0]["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let (front, back) = (pane(0), pane(1));
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    while !server
        .ok(&["capture-pane", "-t", &back])
        .contains("copied-behind")
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the tab behind never ran"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // The pane in front copies.
    let print = |text: &str| {
        server.ok(&[
            "send-keys",
            "-t",
            &front,
            "-l",
            &format!(
                "printf '\\033]52;c;%s\\007' {}; echo printed-{text}",
                switchboard::util::base64(text.as_bytes())
            ),
        ]);
        server.ok(&["send-keys", "-t", &front, "Enter"]);
    };
    print("from-front");
    wait_copied(&mut client, "from-front");
    assert!(
        !sent(&client.raw, osc("from-behind").as_bytes()),
        "a tab behind never reaches the clipboard"
    );

    // Nor does the pane in front while it is in copy mode, its live output
    // out of sight.
    // Typing into the pane leaves copy mode, so the program waits for a
    // file to copy once copy mode is on.
    let go = server.home.join("go-copy");
    server.ok(&[
        "send-keys",
        "-t",
        &front,
        "-l",
        &format!(
            "while [ ! -e '{}' ]; do sleep 0.05; done; printf '\\033]52;c;%s\\007' {}; echo printed-in-copy-mode",
            go.display(),
            switchboard::util::base64(b"in-copy-mode")
        ),
    ]);
    server.ok(&["send-keys", "-t", &front, "Enter"]);
    client.type_text(&format!("{ALT}b"));
    client.type_text("[");
    client.wait_text("[0]");
    std::fs::write(&go, "").unwrap();
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    while !server
        .ok(&["capture-pane", "-t", &front])
        .lines()
        .any(|l| l == "printed-in-copy-mode")
    {
        assert!(std::time::Instant::now() < deadline, "never printed");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    client.type_text("q");
    client.wait_for("copy mode left", |s| !s.iter().any(|l| l.contains("[0]")));
    assert!(!sent(&client.raw, osc("in-copy-mode").as_bytes()));

    // Nor does the pane in front while mission control is over it.
    client.type_text(&format!("{ALT} "));
    client.wait_text("MISSION CONTROL");
    print("under-mission");
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    while !server
        .ok(&["capture-pane", "-t", &front])
        .lines()
        .any(|l| l == "printed-under-mission")
    {
        assert!(std::time::Instant::now() < deadline, "never printed");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    client.type_text("\x1b");
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    assert!(!sent(&client.raw, osc("under-mission").as_bytes()));

    // Off, the pane in front's copy is ignored too. The sidebar going shows
    // the settings were read again.
    assert_ne!(server.list()["sessions"][0]["sidebar"]["columns"], 0);
    common::write_settings(
        &server.home,
        serde_json::json!({"pane_clipboard": false, "sidebar": false}),
    );
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    while server.list()["sessions"][0]["sidebar"]["columns"] != 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the settings weren't read"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    print("while-off");
    client.wait_for("printed", |s| s.iter().any(|l| l == "printed-while-off"));
    assert!(!sent(&client.raw, osc("while-off").as_bytes()));
}

#[test]
fn a_panes_program_asking_the_terminals_colours_is_told_its_own() {
    let server = TestServer::start();
    // Wide enough that the pane beside the sidebar holds the answer's line
    // whole.
    let mut client = server.attach(200, 30);
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
    // Each half on its own, so a line wrapped between them still shows it.
    client.wait_text("bg:rgb:fafa/f8f8/f0f0");
    client.wait_text("blue:rgb:8181/a2a2/bebe");
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
    let thumb = screen
        .iter()
        .position(|l| l.trim_end_matches(' ').ends_with('┃'))
        .unwrap()
        + 1;
    client.send(format!("\x1b[<0;80;{thumb}M\x1b[<32;80;24M\x1b[<0;80;24m").as_bytes());
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
fn a_lone_pane_fills_a_wide_screen_with_no_cap_chosen() {
    let server = TestServer::start();
    let mut client = server.attach(365, 40);
    client.wait_text("$");
    client.wait_for("the lone pane across the whole width", |_| {
        let list = server.list();
        let side = list["sessions"][0]["sidebar"]["columns"]
            .as_u64()
            .unwrap_or(0);
        widths(&list) == [365 - side]
    });
}

#[test]
fn a_lone_pane_is_capped_on_a_wide_screen_and_a_split_uses_it_all() {
    let server = TestServer::start_with_settings(r#"{"lone_pane_width": "165"}"#);
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

/// Where `text` is on the screen, as (column, row) in cells.
fn cell_of(screen: &[String], text: &str) -> Option<(usize, usize)> {
    screen.iter().enumerate().find_map(|(y, row)| {
        let at = row.find(text)?;
        Some((row[..at].chars().count(), y))
    })
}

#[test]
fn the_spare_columns_beside_a_capped_pane_list_what_can_open_there() {
    let server = TestServer::start_with_settings(r#"{"lone_pane_width": "165"}"#);
    let mut client = server.attach(365, 40);
    client.wait_text("$");
    let side = server.list()["sessions"][0]["sidebar"]["columns"]
        .as_u64()
        .unwrap_or(0) as usize;
    // Centered right of the rule, each row its bound key; the tab has no
    // PR or ticket, so neither row shows.
    let screen = client.wait_text("SIDE PANEL");
    let (x, _) = cell_of(&screen, "SIDE PANEL").unwrap();
    let spare = 365 - (side + 166);
    let middle = side + 166 + spare / 2;
    assert!(
        x > side + 166 && x < middle,
        "at {x}:\n{}",
        screen.join("\n")
    );
    let (note_x, note_y) = cell_of(&screen, "Open the tab's note").unwrap();
    assert!(screen[note_y].contains("M-e"), "{}", screen[note_y]);
    assert!(note_x > side + 166);
    assert!(cell_of(&screen, "The PR").is_none());
    let (_, link_y) = cell_of(&screen, "Read a link").unwrap();
    assert!(screen[link_y].contains("M-u"), "{}", screen[link_y]);
    assert!(cell_of(&screen, "C-M-S-l to widen").is_some());
    // A click on a row does what its key does.
    let (sx, sy) = cell_of(&screen, "Split to the right").unwrap();
    let (col, row) = (sx + 1, sy + 1);
    client.send(format!("\x1b[<0;{col};{row}M\x1b[<0;{col};{row}m").as_bytes());
    client.wait_for("split, the hints gone", |s| {
        widths(&server.list()).len() == 2 && cell_of(s, "SIDE PANEL").is_none()
    });
    let second = server.list()["sessions"][0]["tabs"][0]["panes"][1]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["kill-pane", "-t", &second]);
    client.wait_text("SIDE PANEL");
    // A ticket the tab's name gives, as M-i finds it, gets its row.
    let tab = server.list()["sessions"][0]["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["set", "-t", &tab, "@tab_name", "ABC-123 settings page"]);
    client.wait_text("The ticket");
    // A key rebound in keys.json is the one shown.
    let config = server.home.join(".config/switchboard");
    std::fs::write(
        config.join("keys.json"),
        r#"{"root": {"M-e": null, "M-n": "notes-beside"}}"#,
    )
    .unwrap();
    client.wait_for("the rebound key", |s| {
        cell_of(s, "Open the tab's note").is_some_and(|(_, y)| s[y].contains("M-n"))
    });
    // Off, the spare columns are plain.
    common::write_settings(
        &server.home,
        serde_json::json!({"lone_pane_width": "165", "spare_hints": false}),
    );
    client.wait_for("no hints", |s| cell_of(s, "SIDE PANEL").is_none());
}

#[test]
fn a_box_about_the_tab_opens_beside_a_narrow_pane_as_a_pane() {
    let server = TestServer::start_with_settings(r#"{"lone_pane_width": "165"}"#);
    let mut client = server.attach(365, 40);
    client.wait_text("SIDE PANEL");
    let side = server.list()["sessions"][0]["sidebar"]["columns"]
        .as_u64()
        .unwrap_or(0) as usize;
    let rule = side + 165;
    // M-u's links picker: its title on the first row right of the rule,
    // its keys on the last row, no box edge and no shadow, the hints gone.
    server.ok(&["press", "read-link"]);
    let screen = client.wait_text("Read a link beside this pane");
    let (x, y) = cell_of(&screen, "Read a link beside this pane").unwrap();
    assert!(x > rule && y == 4, "at {x},{y}:\n{}", screen.join("\n"));
    assert!(screen[4].contains("tab 1 · "), "{}", screen[4]);
    let (kx, ky) = cell_of(&screen, "esc close").unwrap();
    assert!(kx > rule && ky > 30, "keys at {kx},{ky}");
    assert!(cell_of(&screen, "SIDE PANEL").is_none());
    assert!(
        !screen.iter().any(|l| l.contains('╭') || l.contains('▀')),
        "no box edge or shadow:\n{}",
        screen.join("\n")
    );
    // Esc closes it and brings the hints back.
    client.send(b"\x1b");
    client.wait_for("the hints back", |s| {
        cell_of(s, "Read a link beside this pane").is_none() && cell_of(s, "SIDE PANEL").is_some()
    });
    // Off, it goes in the middle of the screen, over the pane.
    drop(client);
    let server = TestServer::start_with_settings(
        r#"{"lone_pane_width": "165", "boxes_beside_narrow_pane": false}"#,
    );
    let mut client = server.attach(365, 40);
    client.wait_text("SIDE PANEL");
    server.ok(&["press", "read-link"]);
    let screen = client.wait_text("Read a link beside this pane");
    let (x, _) = cell_of(&screen, "Read a link beside this pane").unwrap();
    assert!(x < rule, "at {x}:\n{}", screen.join("\n"));
}

#[test]
fn the_tabs_menu_opens_beside_a_narrow_pane_as_a_pane() {
    let server = TestServer::start_with_settings(r#"{"lone_pane_width": "165"}"#);
    let mut client = server.attach(365, 40);
    client.wait_text("SIDE PANEL");
    let side = server.list()["sessions"][0]["sidebar"]["columns"]
        .as_u64()
        .unwrap_or(0) as usize;
    let rule = side + 165;
    // The tab's own menu: on the first row right of the rule, under a
    // header naming the tab, with no box edge.
    let (press, title) = ("tab-title", "Tab 1");
    server.ok(&["press", press]);
    let screen = client.wait_text(title);
    let (x, y) = cell_of(&screen, title).unwrap();
    assert!(
        x > rule && y == 4,
        "{press} at {x},{y}:\n{}",
        screen.join("\n")
    );
    assert!(screen[4].contains("tab 1 · "), "{}", screen[4]);
    assert!(
        !screen.iter().any(|l| l.contains('╭') || l.contains('▀')),
        "{press}: no box edge or shadow:\n{}",
        screen.join("\n")
    );
    client.send(b"\x1b");
    client.wait_for("the hints back", |s| {
        cell_of(s, title).is_none() && cell_of(s, "SIDE PANEL").is_some()
    });
}

#[test]
fn the_tabs_menu_goes_in_the_middle_with_too_few_columns_beside_the_pane() {
    let server = TestServer::start_with_settings(r#"{"lone_pane_width": "80"}"#);
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    client.wait_for("the lone pane at 80 columns", |_| {
        widths(&server.list()) == [80]
    });
    let side = server.list()["sessions"][0]["sidebar"]["columns"]
        .as_u64()
        .unwrap_or(0) as usize;
    server.ok(&["press", "tab-title"]);
    let screen = client.wait_text("Tab 1");
    let (x, _) = cell_of(&screen, "Tab 1").unwrap();
    assert!(x < side + 80, "at {x}:\n{}", screen.join("\n"));
}

#[test]
fn a_box_not_about_the_tab_goes_in_the_middle_beside_a_narrow_pane() {
    let server = TestServer::start_with_settings(r#"{"lone_pane_width": "165"}"#);
    let mut client = server.attach(365, 40);
    client.wait_text("SIDE PANEL");
    let side = server.list()["sessions"][0]["sidebar"]["columns"]
        .as_u64()
        .unwrap_or(0) as usize;
    let rule = side + 165;
    // A message, the palette and M-F's search, each over the pane, the
    // hints left beside it.
    let corner = |s: &[String]| cell_of(s, "╭").map(|(x, _)| x);
    server.ok(&["say", "Something to say"]);
    let screen = client.wait_text("Something to say");
    let (x, _) = cell_of(&screen, "Something to say").unwrap();
    assert!(x < rule, "at {x}:\n{}", screen.join("\n"));
    assert!(cell_of(&screen, "SIDE PANEL").is_some());
    client.send(b"\x1b");
    client.wait_for("the message gone", |s| {
        cell_of(s, "Something to say").is_none()
    });
    for (press, words) in [
        ("palette", "esc close"),
        ("search-everything", "Search everything"),
    ] {
        server.ok(&["press", press]);
        let screen = client.wait_text(words);
        let x = corner(&screen).unwrap_or(usize::MAX);
        assert!(x < rule, "{press} at {x}:\n{}", screen.join("\n"));
        client.send(b"\x1b");
        client.wait_for("it closed", |s| corner(s).is_none());
    }
}

/// What the desktop's opener (the test's `open`, or `xdg-open` on Linux) was
/// asked to open, in order.
fn opened(server: &TestServer) -> Vec<String> {
    std::fs::read_to_string(server.home.join("desktop-log"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            l.strip_prefix("open ")
                .or_else(|| l.strip_prefix("xdg-open "))
        })
        .map(str::to_string)
        .collect()
}

/// Waits until the test's `open` has been asked for `count` files.
fn wait_opened(server: &TestServer, count: usize) -> Vec<String> {
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    loop {
        let files = opened(server);
        if files.len() >= count {
            return files;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{count} files opened: {files:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// A picture of the test's own in its directory, by its full path.
fn picture_at(server: &TestServer, name: &str) -> String {
    let file = server.dir.path().join(name);
    common::four_colours(&file, 64);
    real(&file)
}

/// A file's path with links followed, as `look` hands it on.
fn real(file: &std::path::Path) -> String {
    file.canonicalize()
        .expect("the file is there")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn look_shows_pictures_beside_a_narrow_pane_stacked_when_there_are_two() {
    let server = TestServer::start_with_settings(r#"{"lone_pane_width": "80"}"#);
    let mut client = server.attach_as("xterm-ghostty", 240, 40);
    client.wait_text("SIDE PANEL");
    let side = server.list()["sessions"][0]["sidebar"]["columns"]
        .as_u64()
        .unwrap_or(0) as usize;
    let rule = side + 80;
    let shot = picture_at(&server, "shot.png");
    // One picture: a pane right of the rule, its header naming the file
    // and the tab, the hints gone, drawn inline, nothing opened outside.
    server.ok(&["look", &shot]);
    let screen = client.wait_text("shot.png   tab 1 · ");
    let (x, y) = cell_of(&screen, "shot.png   tab 1 · ").unwrap();
    assert!(x > rule && y == 4, "at {x},{y}:\n{}", screen.join("\n"));
    assert!(cell_of(&screen, "SIDE PANEL").is_none());
    let (kx, ky) = cell_of(&screen, "M-Esc close").unwrap();
    assert!(kx > rule && ky > 30, "keys at {kx},{ky}");
    assert!(
        !screen.iter().any(|l| l.contains('╭') || l.contains('▀')),
        "no box edge or shadow:\n{}",
        screen.join("\n")
    );
    client.wait_sent("the picture's data, inline", b"t=d");
    assert!(opened(&server).is_empty(), "{:?}", opened(&server));
    // The pane keeps its keys while they show.
    let pane = server.list()["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    client.type_text("echo typed-beside\r");
    client.wait_text("typed-beside");
    client.wait_text("shot.png   tab 1 · ");
    // M-Esc closes them and brings the hints back.
    client.send(b"\x1b\x1b");
    client.wait_for("the hints back", |s| {
        cell_of(s, "M-Esc close").is_none() && cell_of(s, "SIDE PANEL").is_some()
    });
    // Two, named by their place in the directory the command runs in:
    // stacked, each under its name, the first above the second, headed
    // as a before and after since `--before-after` says they are one.
    picture_at(&server, "before.png");
    picture_at(&server, "after.png");
    server.ok(&[
        "look",
        "-t",
        &pane,
        "--before-after",
        "before.png",
        "after.png",
    ]);
    let screen = client.wait_text("Before and after   tab 1 · ");
    let (bx, by) = cell_of(&screen, "before.png").unwrap();
    let (ax, ay) = cell_of(&screen, "after.png").unwrap();
    assert!(bx > rule && ax == bx, "names at {bx} and {ax}");
    assert!(by > 0 && ay > by + 8, "names on rows {by} and {ay}");
    // A click on them closes them.
    let (col, row) = (bx + 6, by + 5);
    let click = format!("\x1b[<0;{col};{row}M\x1b[<0;{col};{row}m");
    client.send(click.as_bytes());
    client.wait_for("the pictures gone", |s| {
        cell_of(s, "Before and after").is_none() && cell_of(s, "SIDE PANEL").is_some()
    });
    // Shown while a box is up, they go under it: the box keeps its keys.
    server.ok(&["press", "palette"]);
    client.wait_for("the palette with its list", |s| {
        cell_of(s, "action>").is_some() && s.iter().any(|l| l.contains("› "))
    });
    server.ok(&["look", &shot]);
    client.wait_text("shot.png   tab 1 · ");
    client.type_text("zzqq");
    client.wait_text("zzqq");
    let typed = server.ok(&["capture-pane", "-t", &pane]);
    assert!(
        !typed.contains("zzqq"),
        "the palette took the keys: {typed}"
    );
    client.send(b"\x1b");
    client.wait_for("the palette gone", |s| cell_of(s, "zzqq").is_none());
    client.send(b"\x1b\x1b");
    client.wait_for("the pictures gone", |s| cell_of(s, "M-Esc close").is_none());
    // Shown again, a split takes the columns and they go with them.
    server.ok(&["look", &shot]);
    client.wait_text("shot.png   tab 1 · ");
    server.ok(&["split", "-R"]);
    client.wait_for("the pictures gone with the columns", |s| {
        cell_of(s, "M-Esc close").is_none()
    });
    assert!(opened(&server).is_empty(), "{:?}", opened(&server));
}

#[test]
fn pictures_beside_a_pane_stay_up_through_the_keys_typed_into_it() {
    let server = TestServer::start_with_settings(r#"{"lone_pane_width": "80"}"#);
    let mut client = server.attach_as("xterm-ghostty", 240, 40);
    client.wait_text("SIDE PANEL");
    let pane = server.list()["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // `cat -v` shows each key the pane is handed, the arrows by their bytes.
    client.type_text("cat -v\r");
    let shot = picture_at(&server, "shot.png");
    server.ok(&["look", &shot]);
    client.wait_text("shot.png   tab 1 · ");
    // Each key's echo drawn means the key was handled and the screen drawn
    // after it; Enter has `cat` print the line, so it shows twice.
    let line = "abc^[[A^[[B^[[C^[[D";
    let keys: [(&str, &[u8], &str, usize); 6] = [
        ("letters", b"abc", "abc", 1),
        ("Up", b"\x1b[A", "abc^[[A", 1),
        ("Down", b"\x1b[B", "abc^[[A^[[B", 1),
        ("Right", b"\x1b[C", "abc^[[A^[[B^[[C", 1),
        ("Left", b"\x1b[D", line, 1),
        ("Enter", b"\r", line, 2),
    ];
    for (what, bytes, echo, times) in keys {
        client.send(bytes);
        let screen = client.wait_for(&format!("{what} in the pane"), |s| {
            s.iter().filter(|l| l.contains(echo)).count() >= times
        });
        assert!(
            cell_of(&screen, "M-Esc close").is_some(),
            "the pictures closed on {what}:\n{}",
            screen.join("\n")
        );
    }
    // M-Esc alone closes them, and the pane never sees it.
    client.send(b"\x1b\x1b");
    client.wait_for("the pictures gone", |s| {
        cell_of(s, "M-Esc close").is_none() && cell_of(s, "SIDE PANEL").is_some()
    });
    // A key after it, once the pane shows it, means M-Esc would have too.
    client.type_text("after\r");
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    let typed = loop {
        let typed = server.ok(&["capture-pane", "-t", &pane]);
        if typed.contains("after") {
            break typed;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the key after M-Esc reached the pane: {typed}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert!(!typed.contains("^[^["), "M-Esc reached the pane: {typed}");
}

#[test]
fn look_opens_the_desktops_viewer_where_no_picture_shows_beside_the_pane() {
    let server = TestServer::start_with_settings(r#"{"lone_pane_width": "80"}"#);
    let mut client = server.attach_as("xterm-ghostty", 240, 40);
    client.wait_text("SIDE PANEL");
    let shot = picture_at(&server, "shot.png");
    let other = picture_at(&server, "other.png");
    // A video always goes to the desktop's viewer.
    let clip = server.dir.path().join("clip.mp4");
    std::fs::write(&clip, b"a video").unwrap();
    let clip = real(&clip);
    server.ok(&["look", &clip]);
    assert_eq!(wait_opened(&server, 1), std::slice::from_ref(&clip));
    // Anything but a picture, a video or a PDF is refused, and a file
    // that isn't there.
    let script = server.dir.path().join("run.command");
    std::fs::write(&script, "echo ran").unwrap();
    let (code, _, err) = server.run(&["look", &shot, script.to_str().unwrap()]);
    assert_ne!(code, 0);
    assert!(err.contains("isn't a picture, a video or a PDF"), "{err}");
    // A link named as a picture is the file it leads to.
    let link = server.dir.path().join("link.png");
    std::os::unix::fs::symlink(&script, &link).unwrap();
    let (code, _, err) = server.run(&["look", link.to_str().unwrap()]);
    assert_ne!(code, 0);
    assert!(err.contains("isn't a picture, a video or a PDF"), "{err}");
    let (code, _, err) = server.run(&["look", "nowhere.png"]);
    assert_ne!(code, 0);
    assert!(err.contains("no file"), "{err}");
    // A before and after is two files, never one or three.
    for files in [vec![shot.as_str()], vec![&shot, &shot, &shot]] {
        let mut args = vec!["look", "--before-after"];
        args.extend(files);
        let (code, _, err) = server.run(&args);
        assert_ne!(code, 0);
        assert!(err.contains("usage: switchboard look"), "{err}");
    }
    assert_eq!(opened(&server).len(), 1, "none of them opened anything");
    // With a split there are no columns beside the pane: each is opened.
    server.ok(&["split", "-R"]);
    client.wait_for("the hints gone", |s| cell_of(s, "SIDE PANEL").is_none());
    server.ok(&["look", &shot, &other]);
    assert_eq!(
        wait_opened(&server, 3),
        [clip.clone(), shot.clone(), other.clone()]
    );
    assert!(cell_of(&client.screen(), "M-Esc close").is_none());
    drop(client);

    // Off, a picture opens in the desktop's viewer beside a narrow pane too.
    let server = TestServer::start_with_settings(
        r#"{"lone_pane_width": "80", "images_beside_pane": false}"#,
    );
    let mut client = server.attach_as("xterm-ghostty", 240, 40);
    client.wait_text("SIDE PANEL");
    let shot = picture_at(&server, "shot.png");
    server.ok(&["look", &shot]);
    assert_eq!(wait_opened(&server, 1), [shot]);
    assert!(cell_of(&client.screen(), "M-Esc close").is_none());
    drop(client);

    // And so does one for a terminal that draws none of the server's images.
    let server = TestServer::start_with_settings(r#"{"lone_pane_width": "80"}"#);
    let mut client = server.attach(240, 40);
    client.wait_text("SIDE PANEL");
    let shot = picture_at(&server, "shot.png");
    server.ok(&["look", &shot]);
    assert_eq!(wait_opened(&server, 1), [shot]);
    assert!(cell_of(&client.screen(), "M-Esc close").is_none());
}

#[test]
fn look_from_a_tab_behind_waits_beside_its_pane_until_the_tab_is_in_front() {
    let server = TestServer::start_with_settings(r#"{"lone_pane_width": "80"}"#);
    let mut client = server.attach_as("xterm-ghostty", 240, 40);
    client.wait_text("SIDE PANEL");
    let shot = picture_at(&server, "shot.png");
    let tab = |i: usize| {
        server.list()["sessions"][0]["tabs"][i]["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let first = tab(0);
    let pane = server.list()["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // Asked from the first tab while another is in front: no viewer, and
    // nothing shown over the tab in front, whose pane keeps its keys.
    server.ok(&["new-tab", "-n", "second"]);
    assert!(is_current(&server, 1));
    server.ok(&["look", "-t", &pane, &shot]);
    client.type_text("echo typed-in-second\r");
    client.wait_text("typed-in-second");
    assert!(cell_of(&client.screen(), "M-Esc close").is_none());
    // Back on the first tab, it shows beside the pane.
    server.ok(&["select-tab", "-t", &first]);
    client.wait_text("shot.png   tab 1 · ");
    client.send(b"\x1b\x1b");
    client.wait_for("the pictures gone", |s| cell_of(s, "M-Esc close").is_none());
    // Asked while mission control is up: shown once it closes.
    client.type_text(&format!("{ALT} "));
    client.wait_text("MISSION CONTROL");
    server.ok(&["look", "-t", &pane, &shot]);
    client.type_text("\x1b");
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    client.wait_text("shot.png   tab 1 · ");
    // Asked for from the second tab while the first shows its own: the
    // first keeps them, and the second shows its own once in front.
    let second_pane = server.list()["sessions"][0]["tabs"][1]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["look", "-t", &second_pane, &shot]);
    server.ok(&["select-tab", "-t", &tab(1)]);
    client.wait_text("shot.png   tab 2 · ");
    // Over the first tab's, hidden, they still take M-Esc.
    client.send(b"\x1b\x1b");
    client.wait_for("the second tab's pictures gone", |s| {
        cell_of(s, "M-Esc close").is_none()
    });
    server.ok(&["select-tab", "-t", &first]);
    client.wait_text("shot.png   tab 1 · ");
    assert!(opened(&server).is_empty(), "{:?}", opened(&server));
}

/// Settings' `lone_pane_width` as the server wrote it to the test's file.
fn lone_setting(server: &TestServer) -> Option<String> {
    let file = server.home.join(".config/switchboard/settings.json");
    let text = std::fs::read_to_string(file).ok()?;
    let settings: serde_json::Value = serde_json::from_str(&text).ok()?;
    settings["lone_pane_width"].as_str().map(str::to_string)
}

/// The first session's tab `n`'s panes' widths.
fn tab_widths(server: &TestServer, n: usize) -> Vec<u64> {
    server.list()["sessions"][0]["tabs"][n]["panes"]
        .as_array()
        .map(|panes| panes.iter().filter_map(|p| p["cols"].as_u64()).collect())
        .unwrap_or_default()
}

#[test]
fn a_lone_panes_edge_drags_and_moves_with_the_resize_keys_for_its_tab_alone() {
    let server = TestServer::start_with_settings(r#"{"lone_pane_width": "165"}"#);
    let mut client = server.attach(365, 40);
    client.wait_text("$");
    client.wait_for("the lone pane at 165 columns", |_| {
        widths(&server.list()) == [165]
    });
    let side = server.list()["sessions"][0]["sidebar"]["columns"]
        .as_u64()
        .unwrap_or(0) as usize;
    // The rule is at column side + 165 from 0; the mouse counts from 1.
    let (rule, to) = (side + 166, side + 201);
    client.send(format!("\x1b[<0;{rule};5M\x1b[<32;{to};5M\x1b[<0;{to};5m").as_bytes());
    client.wait_for("the pane dragged to 200 columns", |_| {
        widths(&server.list()) == [200]
    });
    assert_eq!(lone_setting(&server).as_deref(), Some("165"));
    client.wait_text("This tab's pane is now 200 columns");
    client.send(b"\x1b");
    client.wait_for("the message closed", |s| {
        !s.iter().any(|l| l.contains("This tab's pane is now"))
    });
    // C-M-S-h moves the edge back 5 columns, as it moves a divider.
    client.send(b"\x1b[104;8u");
    client.wait_for("the pane 5 columns narrower", |_| {
        widths(&server.list()) == [195]
    });
    assert_eq!(lone_setting(&server).as_deref(), Some("165"));
    // C-M-S-l the other way; a new tab with one pane takes the setting's
    // width, the first keeping its own.
    client.send(b"\x1b[108;8u");
    client.wait_for("the pane 5 columns wider", |_| {
        widths(&server.list()) == [200]
    });
    let first = server.list()["sessions"][0]["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        server.ok(&["show", "-t", &first, "@lone_width"]).trim(),
        "200",
        "kept with the tab's options, which the layout saves"
    );
    server.ok(&["new-tab"]);
    client.wait_for("the new tab at the setting's width", |_| {
        tab_widths(&server, 1) == [165]
    });
    // The new tab's rule dragged to the screen's edge: it alone fills it.
    let rule = side + 166;
    client.send(format!("\x1b[<0;{rule};5M\x1b[<32;365;5M\x1b[<0;365;5m").as_bytes());
    client.wait_for("the new tab filling the width", |_| {
        tab_widths(&server, 1) == [365 - side as u64]
    });
    assert_eq!(tab_widths(&server, 0), [200]);
    assert_eq!(lone_setting(&server).as_deref(), Some("165"));
    client.wait_text("This tab's pane now fills the width");
    client.send(b"\x1b");
    client.wait_for("the message closed", |s| {
        !s.iter().any(|l| l.contains("now fills the width"))
    });
    // "Use the default width", on a key of its own, gives it the setting's
    // again.
    std::fs::write(
        server.home.join(".config/switchboard/keys.json"),
        r#"{"root": {"M-z": "default-width"}}"#,
    )
    .unwrap();
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    while !server.ok(&["list-keys"]).contains("M-z") {
        assert!(std::time::Instant::now() < deadline, "keys.json not read");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    client.type_text(&format!("{ALT}z"));
    client.wait_for("the new tab back at the setting's width", |_| {
        tab_widths(&server, 1) == [165]
    });
}

#[test]
fn the_state_and_config_folders_are_the_owners_alone() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    // The test made the state folder open (umask's 0755) before the server
    // started, as an earlier build would have; settings.json's folder was
    // made through util::write_atomic.
    let server = TestServer::start_in_home();
    assert_eq!(mode(&server.state), 0o700, "{}", server.log());
    assert_eq!(mode(&server.home.join(".config/switchboard")), 0o700);
    // The cache, missing, is made so, for the parts that make folders in it.
    assert_eq!(mode(&server.home.join(".cache/switchboard")), 0o700);
}

/// A program that asked for the focus (1004) hears it leave as another tab
/// comes to the front and come back with its own, as it does between a
/// tab's panes, and as tmux told a window's pane with focus-events on.
#[test]
fn a_tab_switch_tells_a_pane_that_asked_of_the_focus() {
    let server = TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    let first = server.list()["sessions"][0]["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // cat -v shows the escapes it is sent: ^[[O out, ^[[I in.
    // "asked" on a line of its own says the request before it was read.
    client.type_text("printf '\\033[?1004h'; echo asked; cat -v\r");
    client.wait_for("the pane to have asked for the focus", |s| {
        s.iter().any(|l| l.trim() == "asked")
    });
    server.ok(&["new-tab"]);
    client.wait_for("the new tab in front", |s| {
        !s.iter().any(|l| l.contains("cat -v"))
    });
    server.ok(&["select-tab", "-t", &first]);
    client.wait_text("^[[O^[[I");
    // Left for the other tab by going to it, and back as that one closes.
    let second = server.list()["sessions"][0]["tabs"][1]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["select-tab", "-t", &second]);
    client.wait_for("the other tab in front", |s| {
        !s.iter().any(|l| l.contains("cat -v"))
    });
    server.ok(&["kill-tab", "-t", &second]);
    client.wait_text("^[[O^[[I^[[O^[[I");
}
