//! The boxes over the workspace as someone uses them: the palette, a
//! message, a tip and the question before closing a busy pane, each through
//! a server and a client of the test's own, keys typed and the screen read
//! back.

use crate::common;

use common::TestServer;

const ALT: &str = "\x1b";

fn panes(list: &serde_json::Value) -> usize {
    list["sessions"][0]["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["panes"].as_array().unwrap().len())
        .sum()
}

/// The command in front in the session's focused pane.
/// Whether a pane's program is its shell: the test's /bin/sh, which a Mac
/// runs as bash.
fn is_shell(command: &str) -> bool {
    matches!(command, "sh" | "bash" | "dash")
}

/// Whether the pane on the right of two beside the sidebar (the one a split
/// just opened) shows its shell's prompt on a row of its own, which a login
/// shell draws once its profile has run.
fn right_pane_at_prompt(screen: &[String]) -> bool {
    screen.iter().any(|row| {
        row.matches('│').count() >= 2 && row.rsplit('│').next().map(str::trim) == Some("$")
    })
}

fn front_command(list: &serde_json::Value) -> String {
    let tabs = list["sessions"][0]["tabs"].as_array().unwrap();
    let tab = tabs.iter().find(|t| t["current"] == true).unwrap();
    let pane = tab["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["active"] == true)
        .unwrap();
    pane["command"].as_str().unwrap_or("").to_string()
}

#[test]
fn the_palette_filters_as_you_type_and_runs_the_pick() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    client.type_text(&format!("{ALT}p"));
    let screen = client.wait_text("action>");
    assert!(
        screen.iter().any(|l| l.contains("Actions")),
        "the box has its title:\n{}",
        screen.join("\n")
    );
    client.type_text("split");
    client.wait_for("only the splits listed", |s| {
        s.iter().any(|l| l.contains("Split pane above"))
            && !s.iter().any(|l| l.contains("Close the pane"))
    });
    let screen = client.screen();
    assert!(
        screen.iter().any(|l| l.contains("› Split pane above")),
        "the first is chosen:\n{}",
        screen.join("\n")
    );
    client.type_text("\r");
    client.wait_for("a second pane", |_| panes(&server.list()) == 2);
    client.wait_for("the palette gone", |s| {
        !s.iter().any(|l| l.contains("action>"))
    });
    // The pick is remembered, and what was typed before it.
    let recent =
        std::fs::read_to_string(server.state.join("palette/recent")).expect("the recent file");
    assert_eq!(recent.lines().next(), Some("split-up"));
    let queries = std::fs::read_to_string(server.state.join("palette/queries")).unwrap();
    assert_eq!(queries, "split\tsplit-up\n");
}

#[test]
fn a_message_shows_until_a_key_which_goes_no_further() {
    let server = TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    server.ok(&["say", "Nothing needs you."]);
    client.wait_text("Nothing needs you.");
    client.type_text("x");
    client.wait_for("the message gone", |s| {
        !s.iter().any(|l| l.contains("Nothing needs you."))
    });
    client.type_text("echo said-$((1+1))\r");
    client.wait_text("said-2");
    let pane = server.ok(&["capture-pane"]);
    assert!(
        !pane.contains("xecho"),
        "the key that closed the box didn't reach the shell:\n{pane}"
    );
}

#[test]
fn a_tip_shows_once_and_passes_its_key_on() {
    let server = TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    server.ok(&["tip", "pinned"]);
    let screen = client.wait_text("Pinned.");
    assert!(
        screen[0].ends_with("╮") && screen[0].contains("tip"),
        "in the top-right corner: {:?}",
        screen[0]
    );
    client.type_text("echo tipped-$((2+2))\r");
    client.wait_text("tipped-4");
    client.wait_for("the tip gone", |s| !s.iter().any(|l| l.contains("Pinned.")));
    let seen = std::fs::read_to_string(server.state.join("tips")).unwrap();
    assert_eq!(seen, "pinned\n");
    // Shown once: asked again, it stays away.
    server.ok(&["tip", "pinned"]);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!client.screen().iter().any(|l| l.contains("Pinned.")));
    // Until the tips are reset.
    server.ok(&["tip", "reset"]);
    server.ok(&["tip", "pinned"]);
    client.wait_text("Pinned.");
}

#[test]
fn closing_a_busy_pane_asks_first() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    // A shell at its prompt closes at once: waited for at its prompt, since
    // a login shell's profile can run a command of its own (Ubuntu's runs
    // `locale`) after the shell is in front, and before the prompt.
    client.type_text(&format!("{ALT}L"));
    client.wait_for("a second pane at its prompt", |s| {
        let list = server.list();
        panes(&list) == 2 && right_pane_at_prompt(s) && is_shell(&front_command(&list))
    });
    client.type_text(&format!("{ALT}w"));
    client.wait_for("the shell's pane closed", |_| panes(&server.list()) == 1);
    // One running a program asks.
    client.type_text(&format!("{ALT}L"));
    client.wait_for("a second pane at its prompt", |s| {
        let list = server.list();
        panes(&list) == 2 && right_pane_at_prompt(s) && is_shell(&front_command(&list))
    });
    client.type_text("sleep 100\r");
    client.wait_for("sleep in front", |_| {
        front_command(&server.list()) == "sleep"
    });
    client.type_text(&format!("{ALT}w"));
    let screen = client.wait_text("Close pane? [Y/n]");
    assert!(
        screen.iter().any(|l| l.contains("sleep 100")),
        "it says what runs:\n{}",
        screen.join("\n")
    );
    client.type_text("n");
    client.wait_for("the question gone", |s| {
        !s.iter().any(|l| l.contains("Close pane?"))
    });
    assert_eq!(panes(&server.list()), 2, "no is no");
    client.type_text(&format!("{ALT}w"));
    client.wait_text("Close pane? [Y/n]");
    client.type_text("\r");
    client.wait_for("the busy pane closed", |_| panes(&server.list()) == 1);
}

#[test]
fn the_list_of_keys_opens_over_the_tab() {
    let server = TestServer::start();
    let mut client = server.attach(100, 40);
    client.wait_text("$");
    client.type_text(&format!("{ALT}b"));
    client.type_text("?");
    let screen = client.wait_text("Split pane to the left");
    assert!(screen.iter().any(|l| l.contains("Keys")));
    client.type_text("q");
    client.wait_for("the list gone", |s| {
        !s.iter().any(|l| l.contains("Split pane to the left"))
    });
}

/// The current tab's number.
fn current_tab(list: &serde_json::Value) -> i64 {
    let tabs = list["sessions"][0]["tabs"].as_array().unwrap();
    let tab = tabs.iter().find(|t| t["current"] == true).unwrap();
    tab["index"].as_i64().unwrap()
}

/// The id of the session's focused pane.
fn active_pane(list: &serde_json::Value) -> String {
    let tabs = list["sessions"][0]["tabs"].as_array().unwrap();
    let tab = tabs.iter().find(|t| t["current"] == true).unwrap();
    tab["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["active"] == true)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn m_0_asks_for_a_tab_number_and_goes_there() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    server.ok(&["new-tab", "-n", "third"]);
    client.type_text(&format!("{ALT}1"));
    client.wait_for("the first tab current", |_| {
        current_tab(&server.list()) == 1
    });
    client.type_text(&format!("{ALT}0"));
    client.wait_text("Go to tab");
    client.type_text("3\r");
    client.wait_for("the third tab current", |_| {
        current_tab(&server.list()) == 3
    });
    // A number past the last tab is said.
    client.type_text(&format!("{ALT}0"));
    client.wait_text("Go to tab");
    client.type_text("12\r");
    client.wait_text("There is no tab 12.");
}

#[test]
fn ctrl_w_then_a_direction_moves_between_panes() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let left = active_pane(&server.list());
    client.type_text(&format!("{ALT}L"));
    client.wait_for("a second pane, focused", |_| {
        active_pane(&server.list()) != left
    });
    let right = active_pane(&server.list());
    client.type_text("\x17h");
    client.wait_for("the left pane focused", |_| {
        active_pane(&server.list()) == left
    });
    client.type_text("\x17l");
    client.wait_for("the right pane focused", |_| {
        active_pane(&server.list()) == right
    });
}

#[test]
fn select_pane_with_a_side_moves_from_a_pane_to_the_one_beside_it() {
    // What the diff and Notes ask at their edges.
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let left = active_pane(&server.list());
    client.type_text(&format!("{ALT}L"));
    client.wait_for("a second pane, focused", |_| {
        active_pane(&server.list()) != left
    });
    let right = active_pane(&server.list());
    server.ok(&["select-pane", "-t", &right, "-L"]);
    assert_eq!(active_pane(&server.list()), left);
    server.ok(&["select-pane", "-t", &left, "-R"]);
    assert_eq!(active_pane(&server.list()), right);
    // Nothing above: the pane stays.
    server.ok(&["select-pane", "-t", &right, "-U"]);
    assert_eq!(active_pane(&server.list()), right);
}

#[test]
fn the_palette_renames_the_tab_and_reports_how_it_is_used() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    server.ok(&["press", "rename-tab"]);
    client.wait_text("Rename the tab");
    client.type_text("billing\r");
    client.wait_for("the tab renamed", |_| {
        server.list()["sessions"][0]["tabs"][0]["label"] == "billing"
    });
    server.ok(&["press", "habits-report"]);
    let screen = client.wait_text("How you use Switchboard");
    assert!(
        screen
            .iter()
            .any(|l| l.contains("Nothing counted yet") || l.contains("Counting is off")),
        "the report says what it has:\n{}",
        screen.join("\n")
    );
    client.type_text("x");
    client.wait_for("the report gone", |s| {
        !s.iter().any(|l| l.contains("How you use Switchboard"))
    });
}

/// Whether `needle` is anywhere in what the client was sent.
fn sent(raw: &[u8], needle: &[u8]) -> bool {
    raw.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn a_picture_shows_full_size_over_its_pane_and_esc_goes_back() {
    let server = TestServer::start();
    let file = server.home.join("shot.png");
    image::RgbaImage::from_pixel(64, 32, image::Rgba([200, 40, 40, 255]))
        .save(&file)
        .unwrap();
    let file = file.to_str().unwrap();
    // A terminal that doesn't draw the server's images: refused, for the
    // asker to open it another way.
    let mut plain = server.attach(100, 30);
    plain.wait_text("$");
    let pane = server.list()["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (code, _, err) = server.run(&["picture", "-t", &pane, file]);
    assert_ne!(code, 0);
    assert!(err.contains("doesn't draw"), "{err}");
    plain.detach();
    // Gone before the next attaches, so the picture goes to that one.
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    while server.list()["clients"]
        .as_array()
        .is_some_and(|c| !c.is_empty())
    {
        assert!(std::time::Instant::now() < deadline, "the client stayed");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let mut client = server.attach_as("xterm-ghostty", 100, 30);
    client.wait_text("$");
    let (code, _, err) = server.run(&["picture", "-t", &pane, "/nowhere/shot.png"]);
    assert_ne!(code, 0, "a file that isn't there is refused");
    assert!(err.contains("no file"), "{err}");
    // A pane in a tab behind the one shown: refused, since it couldn't show.
    let made = server.ok(&["new-tab", "-d"]);
    let hidden = made
        .split_whitespace()
        .find(|w| w.starts_with('%'))
        .expect("the new pane's id");
    let (code, _, err) = server.run(&["picture", "-t", hidden, file]);
    assert_ne!(code, 0);
    assert!(err.contains("isn't on the screen"), "{err}");

    server.ok(&["picture", "-t", &pane, file]);
    client.wait_text("shot.png  Esc back");
    assert!(sent(&client.raw, b"\x1b_G"), "drawn with kitty graphics");
    assert!(sent(&client.raw, b"t=d"), "its data inline");

    // A key other than Esc stays with it, kept from the pane.
    client.type_text("x");
    client.wait_text("Esc back");
    client.type_text("\x1b");
    client.wait_for("the picture gone", |s| {
        !s.iter().any(|l| l.contains("Esc back"))
    });
    let screen = server.ok(&["capture-pane", "-t", &pane]);
    assert!(
        !screen.contains('x'),
        "the x never reached the shell: {screen}"
    );
}

#[test]
fn the_palette_in_ghostty_casts_its_shadow_as_an_image_under_the_text() {
    let server = TestServer::start();
    let mut client = server.attach_as("xterm-ghostty ghostty", 100, 30);
    client.wait_text("$");
    server.ok(&["press", "palette"]);
    client.wait_sent("the shadow placed under the text", b"z=-1,C=1");
    let screen = client.wait_for("the palette", |s| s.iter().any(|l| l.contains('╭')));
    assert!(
        !screen.iter().any(|l| l.contains('▘') || l.contains('▀')),
        "no text shadow beside the image:\n{}",
        screen.join("\n")
    );
}
