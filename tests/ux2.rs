//! UX 2.0 as someone sees it, against a server of the test's own: with its
//! setting on, no status line, and a header over the tab's panes naming it
//! with the keys of what it can show in Claude's place; with it off again,
//! the status line back and the panes the whole height. Its keys are the
//! kit's keycaps, ending in half circles on a terminal that draws them.
//! With pictures drawn as sixel (src/mux/sixel), never over the header or
//! on the last row a pane now reaches, never from mission control's copy of
//! a tab's screen, and drawn by the image view in Claude's place.

mod common;

use common::TestServer;
use serde_json::{Value, json};

/// Settings for the tests: no sidebar, desktop notifications or tips, and
/// these.
fn settings(server: &TestServer, extra: Value) {
    let dir = server.home.join(".config/switchboard");
    std::fs::create_dir_all(&dir).unwrap();
    let mut all = json!({"notifications": false, "tips": false, "sidebar": false});
    for (k, v) in extra.as_object().unwrap() {
        all[k] = v.clone();
    }
    std::fs::write(dir.join("settings.json"), all.to_string()).unwrap();
}

#[test]
fn with_ux2_a_header_names_the_tab_in_place_of_the_status_line() {
    let server = TestServer::start();
    settings(&server, json!({"ux2": true}));
    let mut client = server.attach(90, 24);
    client.wait_text("$");
    let screen = client.wait_for("the header and no status line", |s| {
        s[2].contains("M-c Claude") && !s[23].contains("main")
    });
    assert!(
        screen[2].contains("M-d  Diff") && screen[2].contains("M-v  PR"),
        "every view's key: {:?}",
        screen[2]
    );
    assert!(
        screen[3].starts_with("───"),
        "a rule under the keys: {:?}",
        screen[3]
    );
    // The pane takes the rest: all but the header's four rows.
    client.wait_for("the pane under the header", |_| {
        server.list()["sessions"][0]["tabs"][0]["panes"][0]["rows"] == 20
    });
    // Off again: the status line back, the pane the whole height but it.
    settings(&server, json!({}));
    client.wait_for("the status line back", |s| {
        s[23].contains("main") && !s[2].contains("M-c Claude")
    });
    client.wait_for("the pane the whole height", |_| {
        server.list()["sessions"][0]["tabs"][0]["panes"][0]["rows"] == 23
    });
}

#[test]
fn with_ux2_and_message_boxes_off_a_message_is_said_in_a_box() {
    let server = TestServer::start();
    settings(&server, json!({"ux2": true, "message_boxes": false}));
    let mut client = server.attach(90, 24);
    client.wait_for("the header", |s| s[2].contains("M-c Claude"));
    let pid = server.pid();
    // With no status line to put it on, the message goes in a box, and the
    // server goes on.
    server.ok(&["say", "Nothing needs you."]);
    client.wait_text("Nothing needs you.");
    assert_eq!(server.pid(), pid);
    server.list();
}

#[test]
fn with_ux2_a_ghostty_terminal_gets_half_circles_and_another_gets_spaces() {
    let server = TestServer::start();
    settings(&server, json!({"ux2": true}));
    let mut ghostty = server.attach_as("xterm-ghostty", 90, 24);
    let screen = ghostty.wait_for("the header's keycaps", |s| s[2].contains("M-d"));
    assert!(
        screen[2].contains("\u{E0B6}M-d\u{E0B4} Diff"),
        "half circles at the key's ends: {:?}",
        screen[2]
    );
    let mut other = server.attach_as("xterm-256color", 90, 24);
    let screen = other.wait_for("the header's keycaps", |s| s[2].contains("M-d"));
    assert!(
        screen[2].contains(" M-d  Diff") && !screen[2].contains('\u{E0B6}'),
        "spaces in their place: {:?}",
        screen[2]
    );
}

// ── Pictures drawn as sixel ─────────────────────────────────────────────────

/// A server with UX 2.0 on and pictures drawn as sixel, and a terminal
/// attached that answered its cell is 13 by 25 pixels; gives the first
/// pane, allowed to send pictures.
fn sixel_server(cols: u16, rows: u16) -> (TestServer, common::TestClient, String) {
    let server = TestServer::start_with_settings(
        r#"{"ux2": true, "image_protocol": "sixel", "notifications": false, "tips": false, "sidebar": false}"#,
    );
    let mut client = server.attach(cols, rows);
    client.wait_for("the header", |s| s[2].contains("M-c Claude"));
    client.wait_sent("the terminal asked for its cell", b"\x1b[16t");
    client.send(b"\x1b[6;25;13t");
    let pane = server.list()["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["set", "-t", &pane, "@allow_passthrough", "1"]);
    (server, client, pane)
}

/// Has `pane` show the four-colour picture (`common::four_colours`) as
/// Notes gives one, id 42, four cells by two: the pane cleared, the
/// picture's rows put at the pane's row `at` (1-based) and the one under
/// it, each after "Q7" and moved to rather than reached by a newline, so
/// nothing scrolls but `scroll` rows scrolled up after; then `done` said
/// on the pane's tenth row, which is waited for.
fn picture_at(
    server: &TestServer,
    client: &mut common::TestClient,
    pane: &str,
    at: u16,
    scroll: u16,
    done: &str,
) {
    use switchboard::notes::images::Picture;
    let png = server.dir.path().join("four.png");
    common::four_colours(&png, 40);
    let mut out = format!(
        "\x1b_Ga=T,U=1,f=100,t=f,q=2,i=42,c=4,r=2;{}\x1b\\\x1b[H\x1b[2J",
        switchboard::util::base64(png.to_string_lossy().as_bytes())
    );
    for row in 0..2 {
        out.push_str(&format!("\x1b[{};1HQ7\x1b[38;2;0;0;42m", at + row));
        for col in 0..4 {
            out.push_str(&Picture::cell(row, col));
        }
        out.push_str("\x1b[0m");
    }
    if scroll > 0 {
        out.push_str(&format!("\x1b[{scroll}S"));
    }
    out.push_str("\x1b[10;1H");
    let file = server.dir.path().join(format!("picture-{at}-{scroll}.out"));
    std::fs::write(&file, out).unwrap();
    let (head, tail) = done.split_at(done.len() / 2);
    server.ok(&[
        "send-keys",
        "-t",
        pane,
        "-l",
        &format!("cat {}; echo {head}''{tail}", file.display()),
    ]);
    server.ok(&["send-keys", "-t", pane, "Enter"]);
    client.wait_text(done);
}

#[test]
fn with_ux2_a_picture_drawn_as_sixel_is_cut_under_the_header_and_short_of_the_last_row() {
    let (server, mut client, pane) = sixel_server(100, 30);
    // The picture's first row scrolled up out of the pane: its second is
    // the pane's top row, the screen's fifth, under the header's four, and
    // drawn there alone (one cell of 25 pixels, made 24, whole bands),
    // the picture's lower half: blue and yellow, no red.
    picture_at(&server, &mut client, &pane, 1, 1, "under-header");
    let screen = client.screen();
    assert!(screen[3].starts_with("───"), "the header: {screen:?}");
    assert!(screen[4].starts_with("Q7"), "{screen:?}");
    client.wait_sent(
        "its second row drawn at the pane's top",
        b"\x1b[5;3H\x1bP0;1;0q\"1;1;52;24",
    );
    let sixel = common::sixel_at(&client.raw, 4, 2).unwrap();
    assert!(sixel.contains(";2;0;0;100") && sixel.contains(";2;100;100;0"));
    assert!(!sixel.contains(";2;100;0;0"), "{sixel:?}");
    // With no status line the pane reaches the screen's last row, where a
    // sixel would scroll the screen: a picture whose second row is there
    // is drawn as its first row alone, the last row left blank.
    let before = client.raw.len();
    picture_at(&server, &mut client, &pane, 25, 0, "at-bottom");
    client.wait_sent(
        "its first row drawn above the last",
        b"\x1b[29;3H\x1bP0;1;0q\"1;1;52;24",
    );
    let screen = client.screen();
    assert_eq!(screen[29], "Q7", "the last row's cells blank: {screen:?}");
    // Nothing ever drawn over the header or reaching the last row (its
    // cells 25 pixels tall), and no placeholder sent.
    let drawn = common::sixels(&client.raw);
    for ((line, _), (_, height)) in &drawn {
        assert!(*line >= 4, "over the header: {drawn:?}");
        let rows = (*height as usize).div_ceil(25);
        assert!(line + rows < 30, "on the last row: {drawn:?}");
    }
    assert!(
        common::sixels(&client.raw[before..]).contains(&((28, 2), (52, 24))),
        "{drawn:?}"
    );
    assert!(!common::sent(&client.raw, "\u{10EEEE}".as_bytes()));
}

#[test]
fn with_ux2_mission_control_s_cards_show_a_picture_s_cells_blank_and_draw_no_piece_of_it() {
    let (server, mut client, pane) = sixel_server(140, 36);
    client.wait_text("$");
    let (line, col) = server.show_picture(&mut client, &pane);
    let at = format!("\x1b[{};{}H\x1bP0;1;0q\"1;1;52;48", line + 1, col + 1);
    client.wait_sent("the picture drawn", at.as_bytes());
    // Mission control over the tab, its card showing the pane's last
    // lines, the picture's cells among them, copied from the pane: blank
    // there, and drawn nowhere, the picture under mission control erased.
    let before = client.raw.len();
    client.type_text("\x1b ");
    client.wait_text("MISSION CONTROL");
    let erased = format!("\x1b[{};{}H\x1b[4X", line + 1, col + 1);
    client.wait_sent("the picture erased", erased.as_bytes());
    let screen = client.wait_for("the card drawn whole", |s| {
        s.iter().filter(|l| l.contains("│ Q7")).count() == 2
    });
    assert!(
        common::sixels(&client.raw[before..]).is_empty(),
        "no piece of it drawn over mission control: {:?}\n{}",
        common::sixels(&client.raw[before..]),
        screen.join("\n")
    );
    assert!(!common::sent(&client.raw, "\u{10EEEE}".as_bytes()));
}

#[test]
fn with_ux2_the_ticket_image_view_in_claude_s_place_draws_its_page_as_sixel() {
    use switchboard::views::image::page::Parts;
    use switchboard::views::image::render::{Shot, View};
    // Linear moved to a port nothing listens on: the view shows the
    // ticket as last read and drawn, from the cache, and its read again
    // fails quietly.
    let server = TestServer::start_with_env(&[("SWITCHBOARD_LINEAR_API", "http://127.0.0.1:9")]);
    settings(
        &server,
        json!({"ux2": true, "image_protocol": "sixel", "github": false}),
    );
    let parts = Parts {
        title: "Fix the login".into(),
        number: "ENG-1".into(),
        html: "<p>hi</p>".into(),
        ..Parts::default()
    };
    let cache = server.home.join(".cache/switchboard");
    let text = cache.join("pr-image/ENG-1.json");
    std::fs::create_dir_all(text.parent().unwrap()).unwrap();
    std::fs::write(&text, serde_json::to_string(&parts).unwrap()).unwrap();
    let page = server.dir.path().join("page.png");
    common::four_colours(&page, 64);
    // A page four rows tall, kept for the pane the view takes: Claude's,
    // 100 by 26 under the header, its cells 8 by 16 pixels (the view
    // draws twice that).
    let view = View::new(100, 26, (16, 32));
    let shot = Shot {
        file: page,
        rows: 4,
        from: 0.0,
    };
    switchboard::views::image::cache::Cache::at(cache).save_pages(&parts, &view, &[shot]);

    let mut client = server.attach(100, 30);
    client.wait_for("the header", |s| s[2].contains("M-c Claude"));
    client.wait_text("$");
    let pane = server.list()["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["set", "-t", &pane, "@tab_name", "ENG-1 the login"]);
    server.ok(&["press", "ticket-image"]);
    // In the pane's place, the only pane, allowed to send pictures.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let list = server.list();
        let panes = list["sessions"][0]["tabs"][0]["panes"].as_array().unwrap();
        let options = &panes[0]["options"];
        if panes.len() == 1 && options["@in_place"] == "ticket-image" {
            assert_eq!(options["@allow_passthrough"], "1", "{list}");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{list}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // Its page drawn as sixel from the pane's top left, under the header:
    // its 100 by 4 cells of the sixel's 10 by 20 pixels, made whole bands.
    client.wait_sent("the page drawn", b"\x1b[5;1H\x1bP0;1;0q\"1;1;1000;78");
    assert!(!common::sent(&client.raw, "\u{10EEEE}".as_bytes()));
}

/// `switchboard hook <event>` in `pane`, as Claude Code runs it.
fn hook(server: &TestServer, pane: &str, event: &str, input: Value) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(common::BIN)
        .args(["hook", event])
        .env("HOME", &server.home)
        .env("SWITCHBOARD_STATE_DIR", &server.state)
        .env("SWITCHBOARD", server.socket())
        .env("SWITCHBOARD_PANE", pane)
        .env_remove("SLACK_RELAY_HELPER")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("switchboard runs");
    let _ = child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input.to_string().as_bytes());
    assert!(child.wait().expect("the hook ends").success());
}

#[test]
fn with_ux2_m_h_shows_the_tabs_history_under_its_screen_and_moves_through_time() {
    let server = TestServer::start();
    settings(&server, json!({"ux2": true}));
    let mut client = server.attach(120, 40);
    client.wait_for("the header", |s| s[2].contains("M-h  History"));
    let pane = server.list()["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // A transcript of twenty minutes: a turn, then a commit.
    let now = jiff::Timestamp::now().as_second();
    let at = |ago: i64| jiff::Timestamp::from_second(now - ago).unwrap().to_string();
    let transcript = server.home.join("t.jsonl");
    let lines = [
        json!({"type": "user", "timestamp": at(1200), "message": {"content": "fix it"}}),
        json!({"type": "assistant", "timestamp": at(600), "message": {
            "usage": {"input_tokens": 40_000},
            "content": [{"type": "tool_use", "id": "c1", "name": "Bash", "input": {"command": "git commit -m x"}}]}}),
        json!({"type": "user", "timestamp": at(590), "message": {"content": [
            {"type": "tool_result", "tool_use_id": "c1", "content": "[main a1c3e52] x"}]}}),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(&transcript, text).unwrap();
    let payload = json!({"session_id": "s1", "transcript_path": transcript, "cwd": server.home});
    hook(&server, &pane, "SessionStart", payload.clone());
    hook(&server, &pane, "UserPromptSubmit", payload);
    client.send(b"\x1bh");
    let screen = client.wait_for("the timeline", |s| {
        s.iter().any(|l| l.contains("• a1c3e52"))
    });
    assert!(screen.iter().any(|l| l.contains("h/l move in time")));
    assert!(screen.iter().any(|l| l.trim_start().starts_with("state")));
    assert!(screen.iter().any(|l| l.contains("20% now")), "{screen:#?}");
    // The turn before now, which the header says is the moment shown.
    client.send(b"[");
    client.wait_for("the moment shown in the header", |s| {
        s[0].contains("viewing ")
    });
    client.send(b"\x1b");
    client.wait_for("back to now", |s| !s[0].contains("viewing "));
    client.send(b"\x1b");
    client.wait_for("closed", |s| {
        !s.iter().any(|l| l.contains("h/l move in time"))
    });
}

#[test]
fn with_ux2_m_h_brings_a_claude_parked_behind_a_tool_back_beside_it() {
    let server = TestServer::start();
    settings(&server, json!({"ux2": true}));
    common::stand_in::lazygit(&server.home.join("bin"));
    let mut client = server.attach(120, 40);
    client.wait_for("the header", |s| s[2].contains("M-h  History"));
    let pane = server.list()["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let transcript = server.home.join("t.jsonl");
    let line = json!({"type": "user", "timestamp": jiff::Timestamp::now().to_string(), "message": {"content": "fix it"}});
    std::fs::write(&transcript, format!("{line}\n")).unwrap();
    let payload = json!({"session_id": "s1", "transcript_path": transcript, "cwd": server.home});
    hook(&server, &pane, "SessionStart", payload.clone());
    hook(&server, &pane, "UserPromptSubmit", payload);
    server.ok(&["press", "lazygit"]);
    let in_layout = |list: &Value| -> Vec<String> {
        list["sessions"][0]["tabs"][0]["panes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p["id"].as_str().map(str::to_string))
            .collect()
    };
    let start = std::time::Instant::now();
    while in_layout(&server.list()).contains(&pane) {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "lazygit never took Claude's place"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // A shell split beside lazygit has the focus.
    server.ok(&["split"]);
    let start = std::time::Instant::now();
    while in_layout(&server.list()).len() < 2 {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "no split"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    client.send(b"\x1bh");
    client.wait_for("the history", |s| {
        s.iter().any(|l| l.contains("h/l move in time"))
    });
    // Claude is beside lazygit again, with the focus, so typing reaches it.
    let list = server.list();
    assert!(in_layout(&list).contains(&pane), "{list:#}");
    let tab = &list["sessions"][0]["tabs"][0];
    let active = tab["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["active"] == true)
        .and_then(|p| p["id"].as_str());
    assert_eq!(active, Some(pane.as_str()), "{tab:#}");
}
