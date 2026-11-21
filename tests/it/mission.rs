//! Mission control as a person uses it, against a server of the test's own
//! (the tmux version's test/tmux-sidebar.test.js, for its mission client's
//! keys): M-Space opens it over the whole screen, `]` moves to the next
//! view, Enter on a tab's card opens its menu and "Go to tab" goes there,
//! the Settings view changes a setting in the settings file, M-j opens the
//! Work map as a picker of the open tabs, Reopen lists the worktrees no tab
//! has open and its menus open and remove them, and q closes it. With UX 2.0 and
//! many tabs, moving about leaves what it shows standing, while a tab's new
//! output and a tab closing still show. With UX 2.0, the Resources board ends
//! a tab's busy process from its menu after a yes, and a number on a card
//! waiting on you answers its prompt. `/` searches a card's menu.

use crate::common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::stand_in::Claude;
use common::{TestClient, TestServer};
use serde_json::{Value, json};

const ALT: &str = "\x1b";

/// Settings for the tests: no desktop notifications, no tips, no sidebar
/// beside the panes, and only the views a test looks at.
fn settings(server: &TestServer, extra: Value) {
    let mut all = json!({
        "notifications": false,
        "tips": false,
        "sidebar": false,
        "github": false,
        "linear": false,
        "view_calendar": false,
        "view_slack": false,
        "view_relay": false,
        "view_reopen": false,
        "view_resources": false,
        "usage": false,
        "view_keys": false,
        // The Reminders view too, which a test of reminders turns on.
        "reminders": false,
        "standup_note": false,
        "new_notes": false,
    });
    for (k, v) in extra.as_object().unwrap() {
        all[k] = v.clone();
    }
    common::write_settings(&server.home, all);
}

fn tabs(list: &Value) -> Vec<Value> {
    list["sessions"][0]["tabs"].as_array().unwrap().clone()
}

/// Waits until `list` satisfies `pred`.
fn wait_list(server: &TestServer, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    loop {
        let list = server.list();
        if pred(&list) {
            return list;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {list}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A server with two tabs, the second in front, and a client on it.
fn two_tabs(extra: Value) -> (TestServer, TestClient) {
    let server = TestServer::start();
    settings(&server, extra);
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    // The settings taken up: the sidebar gone from beside the panes.
    wait_list(&server, "the settings read", |l| {
        l["sessions"][0]["sidebar"]["columns"] == 0
    });
    server.ok(&["new-tab", "-n", "second"]);
    client.wait_text("2:second");
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
    (server, client)
}

fn open(client: &mut TestClient) -> Vec<String> {
    client.type_text(&format!("{ALT} "));
    client.wait_text("MISSION CONTROL")
}

/// Takes a menu's option by its label, as the daemon's menus were used:
/// j down to it, Enter. Its options have no letters of their own.
fn pick(client: &mut TestClient, label: &str) {
    // Waits for the menu's cursor itself, since the label may already be
    // on the screen in the view behind it, and a `j` sent before the menu
    // opens moves the view instead.
    for _ in 0..20 {
        let screen = client.wait_for(&format!("a menu offering {label}"), |s| {
            menu_cursor(s).is_some()
                && menu_rows(s).is_some_and(|rows| rows.iter().any(|l| l.contains(label)))
        });
        let at = menu_cursor(&screen).unwrap();
        if at.contains(label) {
            client.type_text("\r");
            return;
        }
        client.type_text("j");
        client.wait_for("the cursor to move", |s| {
            menu_cursor(s).is_some_and(|now| now != at)
        });
    }
    panic!("{label} not reached");
}

/// The screen once it reads the same twice, a moment apart.
fn steady(client: &mut TestClient) -> Vec<String> {
    let deadline = common::deadline(Duration::from_secs(10));
    let mut last = client.screen();
    loop {
        std::thread::sleep(Duration::from_millis(300) * common::patience());
        let now = client.screen();
        if now == last {
            return now;
        }
        assert!(
            Instant::now() < deadline,
            "the screen never settled:\n{}",
            now.join("\n")
        );
        last = now;
    }
}

/// The rows of the menu open, inside its box: its keys (`↵ choose`) give
/// the box's edges, and its top is the last row above them with `╭` at
/// the left edge; each row cut to the box's columns, so neither the view
/// behind nor a card's own `›` counts.
fn menu_rows(screen: &[String]) -> Option<Vec<String>> {
    let keys = screen.iter().rposition(|l| l.contains("↵ choose"))?;
    let row: Vec<char> = screen[keys].chars().collect();
    let at = row
        .windows(8)
        .position(|w| w.iter().collect::<String>() == "↵ choose")?;
    let left = row[..at].iter().rposition(|c| *c == '│')?;
    let right = row[at..].iter().position(|c| *c == '│')? + at;
    let top = screen[..keys]
        .iter()
        .rposition(|l| l.chars().nth(left) == Some('╭'))?;
    Some(
        screen[top + 1..keys]
            .iter()
            .map(|l| l.chars().skip(left + 1).take(right - left - 1).collect())
            .collect(),
    )
}

/// The option the open menu's `›` is on.
fn menu_cursor(screen: &[String]) -> Option<String> {
    menu_rows(screen)?.iter().find_map(|l| {
        let (_, after) = l.split_once('›')?;
        Some(after.trim().to_string())
    })
}

#[test]
fn m_space_opens_it_over_the_screen_and_q_closes_it() {
    let (_server, mut client) = two_tabs(json!({}));
    let screen = open(&mut client);
    assert!(screen[0].contains("MISSION CONTROL"), "{:?}", screen[0]);
    assert!(screen[0].contains(" Tabs 2 "), "{:?}", screen[0]);
    // Each tab a card, in a box for its stage: neither has a Claude.
    assert!(
        screen.iter().any(|l| l.contains("OTHER 2")),
        "{}",
        screen.join("\n")
    );
    assert!(screen.iter().any(|l| l.contains("second")));
    assert!(
        screen.last().unwrap().contains("hjkl move"),
        "the keys at the bottom: {:?}",
        screen.last()
    );
    client.type_text("q");
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    // M-Space again opens it, and again closes it.
    open(&mut client);
    client.type_text(&format!("{ALT} "));
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
}

#[test]
fn a_bracket_moves_to_the_next_view_and_round() {
    let (_server, mut client) = two_tabs(json!({}));
    open(&mut client);
    // After the tabs: the notifications, Routines, then Settings.
    client.type_text("]");
    client.wait_text("No notifications");
    client.type_text("]");
    client.wait_text("No routines yet");
    client.type_text("]");
    let screen = client.wait_text("INTEGRATIONS");
    assert!(
        screen.last().unwrap().contains("Enter its choices"),
        "{:?}",
        screen.last()
    );
    client.type_text("[");
    client.wait_text("No routines yet");
    client.type_text("[");
    client.wait_text("No notifications");
    client.type_text("[");
    client.wait_text("OTHER 2");
}

#[test]
fn the_landing_view_opens_with_nothing_landing_saying_so_and_goes_once_left() {
    let (server, mut client) = two_tabs(json!({}));
    // M-p's "Mission control: Landing", by its name in keys.json.
    server.ok(&["press", "mission-landing"]);
    let screen = client.wait_text("Nothing is landing now.");
    assert!(
        screen[0].contains("Landing"),
        "kept while in front: {:?}",
        screen[0]
    );
    // Left for Settings, it goes, nothing landing: back is Routines.
    client.type_text("]");
    client.wait_text("INTEGRATIONS");
    client.type_text("[");
    let screen = client.wait_text("No routines yet");
    assert!(!screen[0].contains("Landing"), "{:?}", screen[0]);
}

#[test]
fn with_ux2_moving_through_many_tabs_still_shows_their_output_and_a_tab_closing() {
    let server = TestServer::start();
    settings(&server, json!({"ux2": true}));
    let mut client = server.attach(200, 50);
    client.wait_text("$");
    for n in 2..=6 {
        server.ok(&["new-tab", "-n", &format!("tab{n}")]);
    }
    let list = wait_list(&server, "six tabs", |l| tabs(l).len() == 6);
    let id = |at: usize, key: &str| -> String {
        let tab = &tabs(&list)[at];
        let value = if key == "pane" {
            &tab["panes"][0]["id"]
        } else {
            &tab["id"]
        };
        value.as_str().unwrap().to_string()
    };
    // Pinned, each is a card under Needs you, showing its screen as a tab
    // with no Claude does; unpinned they would be lines on the shelf.
    for at in 0..6 {
        server.ok(&["set", "-t", &id(at, "tab"), "@pinned", "1"]);
    }
    open(&mut client);
    client.wait_text("NEEDS YOU");
    // Three cards along, Enter opens the fourth tab's menu.
    for _ in 0..3 {
        client.type_text("l");
    }
    client.type_text("\r");
    client.wait_text("Go to tab 4");
    client.type_text("\x1b");
    client.wait_for("the menu closed", |s| {
        !s.iter().any(|l| l.contains("Go to tab 4"))
    });
    // Back to the first card: a tab's output shows on its card as it comes.
    client.type_text("g");
    server.ok(&[
        "send-keys",
        "-t",
        &id(0, "pane"),
        "echo shown-$((6*7))",
        "Enter",
    ]);
    client.wait_text("shown-42");
    // A tab closing takes its card away, and the cursor stays on a card.
    server.ok(&["kill-tab", "-t", &id(2, "tab")]);
    client.wait_for("the third tab's card gone", |s| {
        !s.iter().any(|l| l.contains("tab3"))
    });
    // The last card, still tab 6: a tab keeps its number when another closes.
    client.type_text("G");
    client.type_text("\r");
    client.wait_text("Go to tab 6");
}

#[test]
fn with_ux2_a_waiting_cards_menu_answers_its_prompt_and_a_number_does_not() {
    let server = TestServer::start();
    settings(&server, json!({"ux2": true}));
    let mut client = server.attach(160, 50);
    client.wait_text("$");
    let tab = &tabs(&server.list())[0];
    let pane = tab["panes"][0]["id"].as_str().unwrap().to_string();
    let pid = tab["panes"][0]["pid"].as_u64().unwrap();
    // The pane's shell counts as its Claude: a session file names it.
    let sessions = server.home.join(".claude/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join(format!("{pid}.json")),
        json!({"pid": pid, "sessionId": "s-1", "entrypoint": "cli"}).to_string(),
    )
    .unwrap();
    // A hook from a pane the server doesn't yet know runs Claude is let go.
    wait_list(&server, "the pane's Claude", |l| {
        !tabs(l)[0]["panes"][0]["claude"].is_null()
    });
    // A permission prompt on its screen, then one key read as it comes.
    let prompt = server.dir.path().join("prompt.txt");
    std::fs::write(
        &prompt,
        "Bash command

  git push origin main

Do you want to proceed?
❯ 1. Yes
  2. Yes, and don't ask again
  3. No
",
    )
    .unwrap();
    server.ok(&[
        "send-keys",
        "-t",
        &pane,
        &format!(
            "clear; cat {}; stty -icanon min 1; x=$(dd bs=1 count=1 2>/dev/null); stty icanon; echo pressed-$x",
            prompt.display()
        ),
        "Enter",
    ]);
    client.wait_text("3. No");
    hook(
        &server,
        &pane,
        "PermissionRequest",
        json!({"tool_name": "Bash", "tool_input": {"command": "git push origin main"}}),
    );
    open(&mut client);
    client.wait_text("NEEDS YOU");
    client.wait_text("BASH COMMAND");
    client.wait_text("Yes, and don't ask again");
    // A number is no answer from mission control: Enter's menu has them.
    client.type_text("2");
    client.type_text("\r");
    // The request whole above its answers.
    client.wait_text("git push origin main");
    pick(&mut client, "Answer: Yes, and don't ask again");
    client.type_text("q");
    client.wait_text("pressed-2");
}

#[test]
fn space_opens_the_chosen_cards_menu_with_nothing_focused() {
    let (_server, mut client) = two_tabs(json!({}));
    open(&mut client);
    client.type_text(" ");
    client.wait_text("Go to tab 1");
}

// The context-menu key (S-F10, sent as its CSI form) opens what a right
// click would: the chosen card's menu, as Enter does.
#[test]
fn shift_f10_opens_the_chosen_cards_menu() {
    let (_server, mut client) = two_tabs(json!({}));
    open(&mut client);
    client.send(b"\x1b[21;2~");
    client.wait_text("Go to tab 1");
    // Picked from the palette over it, the same, mission control kept.
    client.type_text("\x1b");
    client.wait_for("the menu closed", |s| {
        !s.iter().any(|l| l.contains("Go to tab 1"))
    });
    client.type_text(&format!("{ALT}p"));
    client.wait_text("action>");
    client.type_text("menu a right click would");
    client.wait_text("› Open the menu a right click would");
    client.type_text("\r");
    let screen = client.wait_text("Go to tab 1");
    assert!(!closed(&screen), "still open:\n{}", screen.join("\n"));
}

#[test]
fn slash_searches_a_cards_menu_and_enter_takes_the_match() {
    let (server, mut client) = two_tabs(json!({}));
    open(&mut client);
    client.type_text("\r");
    client.wait_text("Go to tab 1");
    client.type_text("/go to");
    let screen = client.wait_text("› go to▏");
    assert!(
        !screen.iter().any(|l| l.contains("Rename it")),
        "{screen:#?}"
    );
    client.type_text("\r");
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
}

#[test]
fn enter_on_a_tabs_card_and_go_to_tab_goes_there() {
    let (server, mut client) = two_tabs(json!({}));
    open(&mut client);
    // The cursor starts on the first card: tab 1's, whose menu Enter opens.
    client.type_text("\r");
    let screen = client.wait_text("Go to tab 1");
    assert!(screen.iter().any(|l| l.contains("Rename it")));
    assert!(screen.iter().any(|l| l.contains("Close the tab")));
    // Keys typed with the Enter that takes an option go nowhere, rather
    // than into the tab it went to.
    client.type_text("\rxyz");
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    std::thread::sleep(Duration::from_millis(300) * common::patience());
    assert!(
        !client.screen().iter().any(|l| l.contains("xyz")),
        "{:#?}",
        client.screen()
    );
    // l to the second card, Enter twice goes there. A menu's letters take
    // nothing of their own: g there is no "Go to tab", and j moves.
    open(&mut client);
    client.type_text("l");
    client.type_text("\r");
    client.wait_text("Go to tab 2");
    client.type_text("g");
    client.type_text("j");
    client.wait_for("the cursor on the second option", |s| {
        s.iter().any(|l| l.contains('›') && l.contains("Peek at"))
    });
    assert_eq!(
        tabs(&server.list())[0]["current"],
        true,
        "still on the first"
    );
    client.type_text("k");
    client.wait_for("the cursor back on the first", |s| {
        s.iter()
            .any(|l| l.contains('›') && l.contains("Go to tab 2"))
    });
    client.type_text("\r");
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
}

#[test]
fn the_settings_view_changes_a_setting_in_the_settings_file() {
    let (server, mut client) = two_tabs(json!({}));
    open(&mut client);
    // v and a view's name go there.
    client.type_text("vsett\r");
    let screen = client.wait_text("INTEGRATIONS");
    assert!(
        screen
            .iter()
            .any(|l| l.contains("GitHub") && l.contains(" off ")),
        "{}",
        screen.join("\n")
    );
    // The cursor rests on the first setting, GitHub, not on its category's
    // heading; Enter's menu turns it on.
    client.type_text("\r");
    pick(&mut client, "Turn it on");
    let file = server.home.join(".config/switchboard/settings.json");
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    loop {
        let saved: Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap_or_default();
        if saved.get("github").is_none() && saved["linear"] == false {
            break;
        }
        assert!(Instant::now() < deadline, "not written: {saved}");
        std::thread::sleep(Duration::from_millis(50));
    }
    client.wait_for("GitHub on", |s| {
        s.iter().any(|l| l.contains("GitHub") && l.contains(" on "))
    });
    // / narrows it; Esc clears the filter, then closes it.
    client.type_text("/push notif\r");
    // Push notifications is on the screen before the filter too, so what
    // is waited for is the rest gone.
    client.wait_for("the list narrowed", |s| {
        s.iter().any(|l| l.contains("Push notifications"))
            && !s.iter().any(|l| l.contains("INTEGRATIONS"))
    });
    client.type_text("\x1b");
    client.wait_text("INTEGRATIONS");
    client.type_text("\x1b");
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
}

/// The Settings board's category on the chosen surface, `▌` at the
/// screen's left, by its name.
fn chosen_category(screen: &[String]) -> Option<String> {
    screen.iter().find_map(|l| {
        // Right of the column of groups and views.
        let (_, view) = l.split_once('│')?;
        let name = view.strip_prefix("▌")?.trim_start().split("  ").next()?;
        Some(name.to_string())
    })
}

#[test]
fn with_ux2_c_w_h_moves_into_the_settings_categories_and_c_w_l_back_to_their_options() {
    let server = TestServer::start();
    settings(&server, json!({"ux2": true}));
    let mut client = server.attach(184, 40);
    client.wait_text("$");
    client.wait_text("M-c Claude");
    open(&mut client);
    client.type_text("vsett\r");
    client.wait_text("CATEGORIES");
    client.wait_text("panes");
    // C-w h: the categories have the focus, the one in front chosen.
    client.type_text("\x17h");
    let screen = client.wait_text("its options");
    let first = chosen_category(&screen).unwrap_or_else(|| panic!("{}", screen.join("\n")));
    // j walks to the next category, its options shown beside.
    client.type_text("j");
    let screen = client.wait_for("the next category", |s| {
        chosen_category(s).is_some_and(|c| c != first)
    });
    let next = chosen_category(&screen).unwrap();
    assert!(
        screen.iter().any(|l| l.contains(&format!("│ {next}"))),
        "{}",
        screen.join("\n")
    );
    // C-w l: back into the options, the category resting.
    client.type_text("\x17l");
    client.wait_for("the options focused", |s| {
        chosen_category(s).is_none() && !s.join("\n").contains("its options")
    });
    // Any other key after C-w is taken as itself: / starts a filter.
    client.type_text("\x17/");
    client.wait_text("/▏");
}

#[test]
fn your_claude_md_row_names_where_the_rules_come_from_and_runs_setup() {
    let (server, mut client) = two_tabs(json!({}));
    std::fs::write(server.home.join("rules.md"), "Be brief.\n").unwrap();
    // The Claude Code setup asks the version of: a stand-in.
    Claude::new()
        .version("2.1.319")
        .install(&server.home.join("bin"));
    open(&mut client);
    client.type_text("vsett\r");
    client.wait_text("INTEGRATIONS");
    client.type_text("/own rules, which\r");
    client.wait_text("local file");
    // The cursor rests on the row, not its category's heading.
    client.type_text("\r");
    pick(&mut client, "Change it");
    client.wait_text("Where your CLAUDE.md's rules come from");
    client.type_text("j\r");
    client.wait_text("Your CLAUDE.md: a file");
    client.type_text("~/rules.md\r");
    let file = server.home.join(".config/switchboard/claude.json");
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    loop {
        let saved: Value = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        if saved["path"] == "~/rules.md" {
            break;
        }
        assert!(Instant::now() < deadline, "not written: {saved}");
        std::thread::sleep(Duration::from_millis(50));
    }
    // Setup's claude-md part ran: ~/.claude/CLAUDE.md imports the file.
    let claude_md = server.home.join(".claude/CLAUDE.md");
    let deadline = common::deadline(std::time::Duration::from_secs(20));
    while !std::fs::read_to_string(&claude_md).is_ok_and(|t| t.contains("rules.md")) {
        assert!(Instant::now() < deadline, "setup didn't write CLAUDE.md");
        std::thread::sleep(Duration::from_millis(100));
    }
    client.wait_text("Your CLAUDE.md: ~/rules.md");
}

#[test]
fn m_j_picks_an_open_tab_from_the_work_map() {
    let (server, mut client) = two_tabs(json!({}));
    wait_list(&server, "the first tab named", |l| {
        tabs(l)[0]["name"].as_str().is_some_and(|n| !n.is_empty())
    });
    client.type_text(&format!("{ALT}j"));
    let screen = client.wait_text("Work map · open tabs");
    assert!(screen.iter().any(|l| l.contains("tab 1")));
    // Typing filters the rows; Enter opens the chosen one's menu, as the
    // daemon's picker did, its diff and lazygit among what it offers.
    client.type_text("second");
    client.wait_for("one row", |s| !s.iter().any(|l| l.contains("tab 1 ")));
    client.type_text("\r");
    let screen = client.wait_text("Go to tab 2");
    assert!(screen.iter().any(|l| l.contains("Its diff (M-d)")));
    assert!(screen.iter().any(|l| l.contains("lazygit")));
    // Esc closes the menu, not the picker, which types on.
    client.type_text("\x1b");
    client.wait_for("the menu closed", |s| {
        !s.iter().any(|l| l.contains("Go to tab 2"))
    });
    client.type_text("x");
    client.wait_text("secondx");
    client.type_text("\x7f");
    client.wait_for("the filter as it was", |s| {
        !s.iter().any(|l| l.contains("secondx"))
    });
    // Enter twice goes there.
    client.type_text("\r");
    client.wait_text("Go to tab 2");
    client.type_text("\r");
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
}

/// Runs git in `cwd`, with none of your own config.
fn git(cwd: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@e")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@e")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

#[test]
fn reopen_lists_the_worktrees_no_tab_has_open_and_its_menus_open_and_remove_them() {
    let server = TestServer::start();
    settings(&server, json!({ "ux2": true, "view_reopen": true }));
    let repo = server.dir.path().join("repo");
    git(server.dir.path(), &["init", "-q", repo.to_str().unwrap()]);
    git(&repo, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    let clean = server.dir.path().join("wt-clean");
    let dirty = server.dir.path().join("wt-dirty");
    let opened = server.dir.path().join("wt-opened");
    for (dir, branch) in [
        (&clean, "feat-clean"),
        (&dirty, "feat-dirty"),
        (&opened, "feat-opened"),
    ] {
        git(
            &repo,
            &["worktree", "add", "-q", "-b", branch, dir.to_str().unwrap()],
        );
    }
    std::fs::write(dirty.join("new.txt"), "work").unwrap();
    let mut client = server.attach(160, 40);
    client.wait_text("$");
    server.ok(&["new-tab", "-c", repo.to_str().unwrap()]);
    // Reopen lists them under their heading, with no Work map in the bar.
    open(&mut client);
    client.type_text("vreopen\r");
    let screen = client.wait_text("feat-clean");
    let all = screen.join("\n");
    assert!(
        all.to_lowercase().contains("worktrees with no tab"),
        "{all}"
    );
    assert!(
        all.contains("feat-dirty") && all.contains("feat-opened"),
        "{all}"
    );
    assert!(!all.contains("Map ") && !all.contains("Work map"), "{all}");
    client.type_text("q");
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    // M-o's picker: a clean worktree removed after a yes.
    let menu = |client: &mut TestClient, name: &str| {
        client.type_text(&format!("{ALT}o"));
        client.wait_text(name);
        client.type_text(name);
        client.type_text("\r");
        client.wait_text("Open a tab here and resume its Claude");
    };
    menu(&mut client, "feat-clean");
    pick(&mut client, "Remove the worktree…");
    client.wait_text("[Y/n]");
    client.type_text("\r");
    let deadline = common::deadline(Duration::from_secs(10));
    while clean.exists() {
        assert!(Instant::now() < deadline, "the clean worktree removed");
        std::thread::sleep(Duration::from_millis(50));
    }
    // Said once the removal's task has answered, so the next key goes to
    // the view rather than to the busy box.
    client.wait_text("Removed wt-clean.");
    // One with a change is kept, saying so.
    client.type_text("\x1b");
    menu(&mut client, "feat-dirty");
    pick(&mut client, "Remove the worktree…");
    client.wait_text("[Y/n]");
    client.type_text("\r");
    client.wait_text("changes not committed");
    assert!(dirty.exists());
    // Its first option opens a tab there.
    client.type_text("\x1b");
    menu(&mut client, "feat-opened");
    client.type_text("\r");
    let want = opened.canonicalize().unwrap();
    wait_list(&server, "a tab in the worktree", |l| {
        tabs(l).iter().any(|t| {
            t["path"]
                .as_str()
                .is_some_and(|p| std::path::Path::new(p).canonicalize().ok() == Some(want.clone()))
        })
    });
    // M-j still picks an open tab, its view named Map in UX 2.0's bar.
    client.type_text(&format!("{ALT}j"));
    client.wait_for("M-j's picker of the open tabs", |s| {
        s.iter().any(|l| l.contains(" Map ")) && s.iter().any(|l| l.contains("tab 1"))
    });
}

#[test]
fn tab_in_a_picker_goes_round_its_rows_and_stops_on_easy_modes_close_button() {
    let (server, mut client) = two_tabs(json!({}));
    wait_list(&server, "the tabs named", |l| {
        tabs(l)
            .iter()
            .all(|t| t["name"].as_str().is_some_and(|n| !n.is_empty()))
    });
    // Two rows: Tab twice goes round to the first, whose menu Enter opens.
    client.type_text(&format!("{ALT}j"));
    client.wait_text("Work map · open tabs");
    client.type_text("\r");
    let screen = client.wait_text("› Go to tab");
    let first = screen
        .iter()
        .find_map(|l| l.split("› ").nth(1))
        .map(|rest| rest.trim_end_matches([' ', '│']).to_string())
        .unwrap();
    client.type_text("\x1b");
    client.wait_for("the menu closed", |s| !s.iter().any(|l| l.contains("› Go")));
    client.type_text("\t\t\r");
    client.wait_text(&format!("› {first}"));
    client.type_text("\x1b");
    client.wait_for("the menu closed", |s| !s.iter().any(|l| l.contains("› Go")));
    client.type_text("\x1b");
    client.wait_for("the picker closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    // With easy mode's close button, Tab past the last row stops on it,
    // and Enter there closes the picker.
    settings(&server, json!({ "easy_keys": true, "menu_mouse": true }));
    std::thread::sleep(Duration::from_millis(2000) * common::patience());
    client.type_text(&format!("{ALT}j"));
    client.wait_text("Work map · open tabs");
    client.type_text("\t\t\r");
    client.wait_for("the picker closed by its ×", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    assert_eq!(tabs(&server.list())[1]["current"], true, "nowhere gone");
}

#[test]
fn a_reminder_set_from_a_tabs_menu_puts_it_under_reminders() {
    let (server, mut client) = two_tabs(json!({"reminders": true}));
    open(&mut client);
    client.type_text("\r");
    pick(&mut client, "Remind me about it later");
    client.wait_text("In 20 minutes");
    client.type_text("\r");
    client.wait_text("What for?");
    client.type_text("check the build\r");
    let list = wait_list(&server, "the reminder set", |l| {
        tabs(l)[0]["options"]["@remind_at"].is_string()
    });
    assert_eq!(tabs(&list)[0]["options"]["@remind_note"], "check the build");
    let screen = client.wait_text("REMINDERS 1");
    assert!(
        screen.iter().any(|l| l.contains("remind in 2")),
        "{}",
        screen.join("\n")
    );
}

#[test]
fn a_time_typed_into_the_reminder_list_wins_over_the_row_it_resembles() {
    let (server, mut client) = two_tabs(json!({"reminders": true}));
    open(&mut client);
    client.type_text("\r");
    pick(&mut client, "Remind me about it later");
    client.wait_text("Or type a time");
    // "2h" matches "In 20 minutes" and "In 3 hours" by their numbers and
    // letters, but names a time of its own.
    client.type_text("2h\r");
    client.wait_text("What for?");
    client.type_text("\r");
    let list = wait_list(&server, "the reminder set", |l| {
        tabs(l)[0]["options"]["@remind_at"].is_string()
    });
    let at: i64 = tabs(&list)[0]["options"]["@remind_at"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let now = jiff::Timestamp::now().as_second();
    assert!((at - now - 7200).abs() < 60, "{at} is two hours from {now}");
}

#[test]
fn the_calendar_view_draws_today_as_a_day_down_the_hours() {
    let server = TestServer::start();
    settings(&server, json!({"view_calendar": true}));
    let now = jiff::Zoned::now();
    let date = format!("{:04}-{:02}-{:02}", now.year(), now.month(), now.day());
    std::fs::create_dir_all(&server.state).unwrap();
    std::fs::write(
        server.state.join("calendar.json"),
        json!({
            "date": date,
            "updated": now.timestamp().as_millisecond(),
            "events": [
                { "title": "Planning", "start": format!("{date}T09:00:00"), "end": format!("{date}T10:00:00"), "you": "going", "link": "https://calendar.example/1" },
                { "title": "Offsite", "allDay": true, "you": "going" },
            ],
        })
        .to_string(),
    )
    .unwrap();
    let mut client = server.attach(140, 40);
    client.wait_text("$");
    open(&mut client);
    client.type_text("vcalendar\r");
    let screen = client.wait_text("Planning");
    let has = |text: &str| screen.iter().any(|l| l.contains(text));
    assert!(has("all day   Offsite"), "{}", screen.join("\n"));
    assert!(has("   8 AM  ┈"), "the hours down the side");
    assert!(has("   9 AM"));
    assert!(has("Planning"));
    // The chosen event is the first: the all-day one; j goes to the next.
    client.type_text("j\r");
    client.wait_text("Open in Google Calendar");
    // The view's own menu goes to today, a day back or a day on.
    client.type_text("\x1b");
    client.wait_for("the menu closed", |s| {
        !s.iter().any(|l| l.contains("Open in Google Calendar"))
    });
    client.type_text("m");
    pick(&mut client, "Go to today");
    client.wait_text("Planning");
}

/// What a line of the screen shows right of the sidebar's edge, if any,
/// the view's scroll bar at its right left out.
fn pane_text(line: &str) -> &str {
    let line = line.trim_end().trim_end_matches(['│', '┃']);
    line.rsplit('│').next().unwrap_or(line).trim()
}

#[test]
fn a_ci_jobs_kept_log_reads_from_its_end_without_its_timestamps() {
    // What the CI view's "Read the failing log" runs beside the pane.
    let server = TestServer::start();
    settings(&server, json!({}));
    let log = server.dir.path().join("77.log");
    let lines: String = (1..=60)
        .map(|n| format!("2026-11-02T10:00:{:02}.0000000Z step {n}\n", n % 60))
        .collect();
    std::fs::write(
        &log,
        lines + "2026-11-02T10:01:00.0000000Z ##[error]it broke\n",
    )
    .unwrap();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    client.type_text(&format!("{} view --log {}\r", common::BIN, log.display()));
    let screen = client.wait_text("Error: it broke");
    assert!(
        screen.iter().any(|l| pane_text(l) == "step 60"),
        "{}",
        screen.join("\n")
    );
    assert!(!screen.iter().any(|l| l.contains("2026-11-02T")));
    assert!(
        !screen.iter().any(|l| pane_text(l) == "step 1"),
        "opened at its end"
    );
    client.type_text("q");
    client.wait_text("$");
}

/// A Claude session file for `pid`, as Claude Code writes one, so the
/// pane's shell stands in for Claude.
fn session_file(home: &std::path::Path, pid: u64, session: &str) {
    let dir = home.join(".claude/sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let file = json!({"pid": pid, "entrypoint": "cli", "sessionId": session});
    std::fs::write(dir.join(format!("{pid}.json")), file.to_string()).unwrap();
}

/// `switchboard hook <event>` in `pane`, as Claude Code runs it.
fn hook(server: &TestServer, pane: &str, event: &str, input: Value) {
    use std::io::Write;
    let mut child = std::process::Command::new(common::BIN)
        .args(["hook", event])
        .env("HOME", &server.home)
        .env("SWITCHBOARD_STATE_DIR", &server.state)
        .env("SWITCHBOARD", server.socket())
        .env("SWITCHBOARD_PANE", pane)
        .env_remove("SLACK_RELAY_HELPER")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("switchboard runs");
    let _ = child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input.to_string().as_bytes());
    assert!(child.wait().expect("the hook ends").success());
}

/// How many times the screen changes before `until`, looked at every 50ms,
/// stopping early once it has changed `enough` times.
fn changes(client: &mut TestClient, until: Instant, enough: usize) -> usize {
    let mut last = client.screen();
    let mut n = 0;
    while n < enough && Instant::now() < until {
        let now = client.screen();
        if now != last {
            n += 1;
            last = now;
        }
    }
    n
}

#[test]
fn a_tab_whose_claude_is_behind_a_tool_in_its_pane_s_place_keeps_its_stage() {
    let (server, mut client) = two_tabs(json!({}));
    common::stand_in::lazygit(&server.home.join("bin"));
    // The first tab's shell stands in for a Claude, lazygit in its place.
    let tab = tabs(&server.list())[0].clone();
    let id = tab["panes"][0]["id"].as_str().unwrap().to_string();
    session_file(
        &server.home,
        tab["panes"][0]["pid"].as_u64().unwrap(),
        "s-parked",
    );
    server.ok(&["select-tab", "-t", tab["id"].as_str().unwrap()]);
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    server.ok(&["press", "lazygit"]);
    wait_list(&server, "lazygit in the pane's place", |l| {
        tabs(l)[0]["panes"][0]["options"]["@in_place"] == "lazygit"
    });
    hook(&server, &id, "UserPromptSubmit", json!({"prompt": "go on"}));
    open(&mut client);
    // Its card under Working, the other tab alone under Other.
    let screen = client.wait_text("WORKING 1");
    assert!(
        screen.iter().any(|l| l.contains("OTHER 1")),
        "{}",
        screen.join("\n")
    );
}

#[test]
fn behind_a_box_the_cards_spinners_turn_at_most_every_half_second() {
    let (server, mut client) = two_tabs(json!({}));
    // The sidebar under mission control, whose spinner draws the screen
    // again as often as it turns.
    settings(&server, json!({"sidebar": true}));
    wait_list(&server, "the sidebar back", |l| {
        l["sessions"][0]["sidebar"]["columns"] != 0
    });
    // The first tab's shell stands in for a Claude at work.
    let pane = tabs(&server.list())[0]["panes"][0].clone();
    let id = pane["id"].as_str().unwrap().to_string();
    session_file(&server.home, pane["pid"].as_u64().unwrap(), "s-spin");
    hook(&server, &id, "UserPromptSubmit", json!({"prompt": "go on"}));
    open(&mut client);
    client.wait_text("WORKING 1");
    // Five turns within 2 seconds, which a spinner held to every 500ms can't
    // make, the window scaled by patience so a busy computer makes it slower
    // rather than failed.
    let free = changes(&mut client, common::deadline(Duration::from_secs(2)), 5);
    assert!(free > 4, "the card's spinner turns: {free} changes");
    // A menu over it: the spinners behind turn at most every 500ms, and
    // the box, merged into the frame, is never sent again under itself, so
    // it can't flicker.
    client.type_text("\r");
    client.wait_text("Go to tab");
    let sent = client.raw.len();
    let boxed = changes(
        &mut client,
        Instant::now() + Duration::from_millis(1500),
        usize::MAX,
    );
    assert!(boxed <= 4, "{boxed} changes behind the menu in 1.5s");
    let after = String::from_utf8_lossy(&client.raw[sent..]).into_owned();
    assert!(
        !after.contains("Go to tab"),
        "the box sent again: {after:?}"
    );
    assert!(
        after.is_empty() || (after.starts_with("\x1b[?2026h") && after.ends_with("\x1b[?2026l")),
        "each frame as synchronized output"
    );
}

#[test]
fn m_slash_opens_every_key_and_enter_presses_one_in_the_pane_below() {
    let (server, mut client) = two_tabs(json!({ "view_keys": true }));
    client.type_text(&format!("{ALT}/"));
    let screen = client.wait_text("MISSION CONTROL");
    let all = screen.join("\n");
    assert!(all.contains("PANES"), "the workspace's by tag: {all}");
    assert!(
        all.contains("jk move  Enter actions  m view actions"),
        "{all}"
    );
    // The filter finds a key by what it does, the cursor on it rather than
    // on its group's heading.
    client.type_text("/split pane to the right\r");
    client.wait_for("the list narrowed", |s| {
        s.iter()
            .any(|l| l.contains("M-S-L") && l.contains("Split pane to the right"))
            && !s.iter().any(|l| l.contains("Split pane to the left"))
    });
    // Enter opens its menu, pressing it first, so Enter twice splits the
    // pane mission control was over.
    client.type_text("\r");
    client.wait_text("Press it here");
    client.type_text("\r");
    wait_list(&server, "the pane split", |l| {
        tabs(l)[1]["panes"].as_array().is_some_and(|p| p.len() == 2)
    });
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
}

#[test]
fn with_the_keys_view_off_m_slash_reaches_the_pane() {
    let (server, mut client) = two_tabs(json!({ "view_keys": false }));
    let pane = tabs(&server.list())[1]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["send-keys", "-t", &pane, "-l", "cat -v"]);
    server.ok(&["send-keys", "-t", &pane, "Enter"]);
    client.type_text(&format!("{ALT}/after\r"));
    client.wait_text("^[/after");
    assert!(
        !client
            .screen()
            .iter()
            .any(|l| l.contains("MISSION CONTROL"))
    );
}

// A Slack message's notification has the Slack view's menu: the message
// whole above the choices, since the row cuts it, and its dismissing worded
// as there.
#[test]
fn a_slack_notifications_menu_is_the_slack_views_with_the_message_whole() {
    let server = TestServer::start();
    std::fs::create_dir_all(&server.state).unwrap();
    let long = "the end of a first line too long for its row ".repeat(4);
    // Sent now: a dismissal is kept two days past its message.
    let ts = format!("{}.000100", switchboard::util::now_ms() / 1000);
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": 1, "items": [{
            "kind": "dm", "channel": "D1", "from": "Ada", "where": "Ada",
            "text": format!("{long}cut here\nits second line\nits third line"),
            "ts": ts, "link": "https://slack.example/m",
        }] })
        .to_string(),
    )
    .unwrap();
    settings(&server, json!({ "view_slack": true }));
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    open(&mut client);
    client.type_text("]");
    client.wait_text("DM Ada");
    // Enter opens a notification's menu.
    client.type_text("j");
    client.type_text("\r");
    let screen = client.wait_text("Dismiss it, until its thread gets a reply");
    let all = screen.join("\n");
    let row = |text: &str| {
        screen
            .iter()
            .position(|l| l.contains(text))
            .unwrap_or_else(|| panic!("{text} isn't shown: {all}"))
    };
    let choices = row("Open it in Slack");
    assert!(row("cut here") < choices, "its first line whole: {all}");
    assert!(row("its second line") < row("its third line"), "{all}");
    assert!(row("its third line") < choices, "above the choices: {all}");
    assert!(
        row("Open a tab for it") < row("Send it to a tab")
            && row("Send it to a tab") < row("Copy its link")
            && row("Copy its link") < row("Dismiss it, until"),
        "{all}"
    );
    assert!(!all.contains("Dismiss every message shown"), "{all}");
    assert!(
        !all.replace("Dismiss it, until", "").contains("Dismiss it"),
        "one way to dismiss it: {all}"
    );
    // Dismissed as the Slack view dismisses it: until its thread gets more.
    pick(&mut client, "Dismiss it, until its thread gets a reply");
    // Kept in the history, out of the sidebar.
    client.wait_text("dismissed");
    let gone = std::fs::read_to_string(server.state.join("slack-dismissed.json")).unwrap();
    assert!(gone.contains(&format!("D1:{ts}")), "{gone}");
}

// A click on a Slack notification opens its menu on "Show it in the Slack
// view", and taking it shows the Slack board on the message's own row: its
// section, never the first one's.
#[test]
fn a_click_on_a_slack_notification_shows_it_in_the_slack_view() {
    let server = TestServer::start();
    std::fs::create_dir_all(&server.state).unwrap();
    let secs = switchboard::util::now_ms() / 1000;
    let ts = |ago: i64| format!("{}.000100", secs - ago);
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": secs * 1000, "items": [
            { "kind": "dm", "channel": "D1", "from": "Ada", "where": "Ada",
                "text": "hello from ada", "ts": ts(100), "link": "https://slack.example/a" },
            { "kind": "channel", "channel": "C1", "from": "Ben", "where": "#dev",
                "text": "a deploy question", "ts": ts(500), "link": "https://slack.example/b" },
        ] })
        .to_string(),
    )
    .unwrap();
    settings(
        &server,
        json!({ "view_slack": true, "ux2": true, "slack_threads": true,
            "slack_background": false, "photos": false }),
    );
    let mut client = server.attach(204, 40);
    client.wait_text("$");
    open(&mut client);
    client.type_text("vnotifications\r");
    // Ben's channel is the second section, so the board opens on it only
    // by the cursor put on its row.
    let (row_text, text, other) = ("Ben · #dev", "a deploy question", "hello from ada");
    let screen = client.wait_for("the Notifications view in front", |s| {
        header(s).trim_start().starts_with("Notifications")
            && s.iter().any(|l| l.contains(row_text))
    });
    let (row, line) = screen
        .iter()
        .enumerate()
        .find(|(_, l)| l.contains(row_text))
        .unwrap();
    let col = line[..line.find(row_text).unwrap()].chars().count() + 2;
    let click = format!("\x1b[<0;{col};{}M\x1b[<0;{col};{}m", row + 1, row + 1);
    // A click chooses the row, and one on the chosen row opens its menu.
    let chosen = |s: &[String]| s.iter().any(|l| l.contains("▌") && l.contains(row_text));
    if !chosen(&screen) {
        client.send(click.as_bytes());
        client.wait_for("the row chosen", |s| chosen(s));
    }
    client.send(click.as_bytes());
    let screen = client.wait_text("Show it in the Slack view");
    assert!(
        screen
            .iter()
            .any(|l| l.contains('›') && l.contains("Show it in the Slack view")),
        "the click's pick first: {}",
        screen.join("\n")
    );
    client.type_text("\r");
    let screen = client.wait_for("the Slack board on its section", |s| {
        !s.iter().any(|l| l.contains("Show it in the Slack view"))
            && s.iter().any(|l| l.contains(text))
            && !s.iter().any(|l| l.contains(other))
    });
    let chosen = screen.iter().find(|l| l.contains(text)).unwrap();
    assert!(chosen.contains('▌'), "{}", screen.join("\n"));
    // Leaving marks seen the section it showed, never the first one it
    // passed on the way.
    client.type_text("\x1b");
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    let seen = server.state.join("slack-seen-items.json");
    let deadline = common::deadline(Duration::from_secs(5));
    while !seen.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let kept = std::fs::read_to_string(&seen).unwrap_or_default();
    assert!(kept.contains(&format!("C1:{}", ts(500))), "{kept}");
    assert!(!kept.contains(&format!("D1:{}", ts(100))), "{kept}");
    assert!(
        !server.state.join("slack-seen").exists(),
        "the look as a whole isn't seen"
    );
}

#[test]
fn the_notifications_views_menu_dismisses_every_notification_shown() {
    let notes = common::TempDir::new("sbda");
    std::fs::write(notes.path().join("Plan.md"), "# Plan\n").unwrap();
    let server = TestServer::start_with_env(&[("NOTES_DIR", notes.path().to_str().unwrap())]);
    std::fs::create_dir_all(&server.state).unwrap();
    let ts = format!("{}.000100", switchboard::util::now_ms() / 1000);
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": 1, "items": [{
            "kind": "dm", "channel": "D1", "from": "Ada", "where": "Ada",
            "text": "a question", "ts": ts, "link": "https://slack.example/m",
        }] })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        server.state.join("new-notes.json"),
        json!([{ "path": "Plan.md", "title": "Plan", "source": "Fathom",
            "at": switchboard::util::now_ms() }])
        .to_string(),
    )
    .unwrap();
    settings(&server, json!({ "view_slack": true, "new_notes": true }));
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    open(&mut client);
    client.type_text("]");
    client.wait_text("DM Ada");
    client.wait_text("Plan");
    client.type_text("j");
    client.type_text("m");
    pick(&mut client, "Dismiss every notification shown");
    client.wait_text("Dismissed 2 notifications.");
    let gone = std::fs::read_to_string(server.state.join("slack-dismissed.json")).unwrap();
    assert!(gone.contains(&format!("D1:{ts}")), "{gone}");
    let left = std::fs::read_to_string(server.state.join("new-notes.json")).unwrap();
    assert!(!left.contains("Plan.md"), "{left}");
}

/// A server with three notes written for you, Alpha the newest and Charlie
/// the oldest, each a notification.
fn three_new_notes() -> (TestServer, common::TempDir) {
    let notes = common::TempDir::new("sbtn");
    let now = switchboard::util::now_ms();
    let mut written = Vec::new();
    for (n, name) in ["Alpha", "Bravo", "Charlie"].iter().enumerate() {
        std::fs::write(
            notes.path().join(format!("{name}.md")),
            format!("# {name}\n"),
        )
        .unwrap();
        written.push(json!({ "path": format!("{name}.md"), "title": name,
            "source": "Fathom", "at": now - n as i64 * 1000 }));
    }
    let server = TestServer::start_with_env(&[("NOTES_DIR", notes.path().to_str().unwrap())]);
    std::fs::create_dir_all(&server.state).unwrap();
    std::fs::write(
        server.state.join("new-notes.json"),
        Value::from(written).to_string(),
    )
    .unwrap();
    settings(&server, json!({ "new_notes": true }));
    (server, notes)
}

/// The open menu holds `text` (its title or the notification above its
/// choices), closed again with Esc.
fn menu_holds(client: &mut TestClient, text: &str) {
    client.type_text("\r");
    client.wait_for(&format!("the menu of {text}"), |s| {
        menu_rows(s).is_some_and(|rows| rows.iter().any(|l| l.contains(text)))
    });
    client.type_text("\x1b");
    client.wait_for("the menu closed", |s| menu_rows(s).is_none());
}

#[test]
fn j_and_k_in_the_notifications_view_wrap_from_its_last_to_its_first_and_back() {
    let (server, _notes) = three_new_notes();
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    open(&mut client);
    client.type_text("]");
    client.wait_text("Charlie");
    // From the last, j goes round, past the day's heading, to the first.
    client.type_text("G");
    menu_holds(&mut client, "Charlie");
    client.type_text("j");
    menu_holds(&mut client, "Alpha");
    // And k from the first back round to the last.
    client.type_text("k");
    menu_holds(&mut client, "Charlie");
    // The wheel stops there, as in every menu.
    let screen = client.screen();
    let row = screen
        .iter()
        .position(|l| l.contains("Charlie"))
        .unwrap_or_else(|| panic!("{}", screen.join("\n")));
    let col = screen[row].chars().take_while(|c| *c != 'C').count();
    client.send(format!("\x1b[<65;{};{}M", col + 1, row + 1).as_bytes());
    std::thread::sleep(Duration::from_millis(300));
    menu_holds(&mut client, "Charlie");
}

#[test]
fn a_notifications_menu_dismisses_every_notification_shown() {
    let (server, _notes) = three_new_notes();
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    open(&mut client);
    client.type_text("]");
    client.wait_text("Charlie");
    client.type_text("\r");
    pick(&mut client, "Dismiss every notification shown");
    client.wait_text("Dismissed 3 notifications.");
    let left = std::fs::read_to_string(server.state.join("new-notes.json")).unwrap();
    for name in ["Alpha", "Bravo", "Charlie"] {
        assert!(!left.contains(name), "{left}");
    }
}

#[test]
fn the_only_notification_shown_has_no_dismissing_of_every_one_in_its_menu() {
    let notes = common::TempDir::new("sbon");
    std::fs::write(notes.path().join("Plan.md"), "# Plan\n").unwrap();
    let server = TestServer::start_with_env(&[("NOTES_DIR", notes.path().to_str().unwrap())]);
    std::fs::create_dir_all(&server.state).unwrap();
    std::fs::write(
        server.state.join("new-notes.json"),
        json!([{ "path": "Plan.md", "title": "Plan", "source": "Fathom",
            "at": switchboard::util::now_ms() }])
        .to_string(),
    )
    .unwrap();
    settings(&server, json!({ "new_notes": true }));
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    open(&mut client);
    client.type_text("]");
    client.wait_text("Plan");
    client.type_text("\r");
    let screen = client.wait_for("its menu", |s| {
        menu_rows(s).is_some_and(|rows| rows.iter().any(|l| l.contains("Open it in Notes")))
    });
    let rows = menu_rows(&screen).unwrap().join("\n");
    assert!(!rows.contains("Dismiss every notification shown"), "{rows}");
}

// "Send it to a tab" and "Paste it into a tab" on a notification: a tab
// running Claude picked, the notification's words and link pasted into it,
// that tab in front; a tab with no Claude isn't offered. The second tab's
// "Claude" is `dd` writing every byte it gets to a file, in place of its
// shell (its pid kept, so its session file still names it), so the test
// sees whether Enter followed the paste. What the file holds is returned.
/// What the second tab shows before the hand-off: a numbered list in
/// Claude's reply with nothing waiting, a permission prompt its Claude waits
/// on, the same asked in auto mode before it counts as waiting, or a draft
/// in its prompt box, or a dialog before its session starts.
#[derive(Clone, Copy, PartialEq)]
enum Before {
    Reply,
    Asking,
    AskingInAutoMode,
    Draft,
    Dialog,
}

fn hand_to_a_tab(choice: &str, title: &str, says: &str, before: Before) -> String {
    let server = TestServer::start();
    std::fs::create_dir_all(&server.state).unwrap();
    let ts = format!("{}.000100", switchboard::util::now_ms() / 1000);
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": 1, "items": [{
            "kind": "dm", "channel": "D1", "from": "Ada", "where": "Ada",
            "text": "a question", "ts": ts, "link": "https://slack.example/m",
        }] })
        .to_string(),
    )
    .unwrap();
    settings(&server, json!({ "view_slack": true }));
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    server.ok(&["new-tab", "-n", "third"]);
    client.wait_text("3:third");
    let got = server.home.join("got");
    // Written once the terminal is raw, just before dd starts reading.
    let ready = server.home.join("ready");
    let list = server.list();
    let second = tabs(&list)[1]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let command = format!(
        "exec sh -c 'stty raw -echo; printf \"{}\"; : > {}; exec dd of={} bs=1 2>/dev/null'",
        match before {
            Before::Asking | Before::AskingInAutoMode | Before::Reply => {
                r"\r\n Do you want to proceed?\r\n ❯ 1. Yes\r\n   2. No\r\n"
            }
            Before::Draft => r"\r\n────────\r\n❯ half typed\r\n────────\r\n",
            Before::Dialog => {
                r"\r\n Do you trust the files in this folder?\r\n\r\n ❯ 1. Yes, proceed\r\n   2. No, exit\r\n\r\n Enter to confirm · Esc to exit\r\n"
            }
        },
        ready.display(),
        got.display()
    );
    // Typed once its shell has drawn its prompt, so none of it is lost.
    let deadline = common::deadline(Duration::from_secs(10));
    while !server.ok(&["capture-pane", "-t", &second]).contains('$') {
        assert!(
            Instant::now() < deadline,
            "the second tab's shell didn't start"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    server.ok(&["send-keys", "-t", &second, &command, "Enter"]);
    let deadline = common::deadline(Duration::from_secs(10));
    while !ready.exists() {
        let screen = server.ok(&["capture-pane", "-t", &second]);
        assert!(Instant::now() < deadline, "dd didn't start: {screen}");
        std::thread::sleep(Duration::from_millis(50));
    }
    // The second tab's dd counts as its Claude; the third runs none.
    let pid = tabs(&list)[1]["panes"][0]["pid"].as_u64().unwrap();
    session_file(&server.home, pid, "s-second");
    wait_list(&server, "the second tab's Claude", |l| {
        !tabs(l)[1]["panes"][0]["claude"].is_null()
    });
    if before == Before::AskingInAutoMode {
        hook(
            &server,
            &second,
            "PermissionRequest",
            json!({"tool_name": "Bash", "permission_mode": "auto",
                "tool_input": {"command": "ls"}}),
        );
        wait_list(&server, "the second tab still working", |l| {
            tabs(l)[1]["panes"][0]["agent"]["state"] == "working"
        });
    }
    if before == Before::Asking {
        hook(
            &server,
            &second,
            "PermissionRequest",
            json!({"tool_name": "Bash", "tool_input": {"command": "ls"}}),
        );
        wait_list(&server, "the second tab waiting", |l| {
            tabs(l)[1]["panes"][0]["agent"]["state"] == "waiting"
        });
    }
    open(&mut client);
    client.type_text("]");
    client.wait_text("DM Ada");
    client.type_text("j");
    client.type_text("\r");
    pick(&mut client, choice);
    let screen = client.wait_text(title);
    let all = screen.join("\n");
    assert!(
        all.contains(says),
        "the picker says what choosing does: {all}"
    );
    assert!(all.contains("2  second"), "{all}");
    assert!(
        !all.contains("3  third"),
        "a tab with no Claude isn't offered: {all}"
    );
    client.type_text("\r");
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
    match before {
        Before::Asking | Before::AskingInAutoMode | Before::Dialog => {
            client.wait_text("is asking something");
            return std::fs::read_to_string(&got).unwrap_or_default();
        }
        Before::Draft => {
            client.wait_text("had a draft");
        }
        Before::Reply => {}
    }
    // Enter follows the paste after a moment, so the file is read once it
    // has the whole message and has stopped growing.
    let deadline = common::deadline(Duration::from_secs(10));
    let mut last = String::new();
    loop {
        let now = std::fs::read_to_string(&got).unwrap_or_default();
        if now.contains("> a question") && now == last {
            assert!(now.contains("Slack message from Ada"), "{now:?}");
            assert!(now.contains("(https://slack.example/m):"), "{now:?}");
            return now;
        }
        assert!(Instant::now() < deadline, "not pasted: {now:?}");
        last = now;
        std::thread::sleep(common::deadline(Duration::from_millis(400)) - Instant::now());
    }
}

#[test]
fn send_it_to_a_tab_pastes_a_notification_into_the_tab_picked_and_sends_it() {
    let got = hand_to_a_tab(
        "Send it to a tab",
        "Send to: Slack: Ada",
        "Sends it as written",
        Before::Reply,
    );
    assert!(got.ends_with("> a question\r"), "Enter after it: {got:?}");
}

#[test]
fn send_it_to_a_tab_sends_nothing_to_a_claude_asking_for_permission() {
    let got = hand_to_a_tab(
        "Send it to a tab",
        "Send to: Slack: Ada",
        "Sends it as written",
        Before::Asking,
    );
    assert_eq!(got, "", "no Enter to pick its option");
}

#[test]
fn send_it_to_a_tab_sends_nothing_to_a_claude_asking_in_auto_mode() {
    let got = hand_to_a_tab(
        "Send it to a tab",
        "Send to: Slack: Ada",
        "Sends it as written",
        Before::AskingInAutoMode,
    );
    assert_eq!(got, "", "no Enter to pick its option");
}

#[test]
fn send_it_to_a_tab_sends_nothing_to_a_claude_showing_a_dialog() {
    let got = hand_to_a_tab(
        "Send it to a tab",
        "Send to: Slack: Ada",
        "Sends it as written",
        Before::Dialog,
    );
    assert_eq!(got, "", "no Enter to pick its option");
}

#[test]
fn send_it_to_a_tab_pastes_into_a_draft_without_sending() {
    let got = hand_to_a_tab(
        "Send it to a tab",
        "Send to: Slack: Ada",
        "Sends it as written",
        Before::Draft,
    );
    assert!(got.ends_with("> a question"), "no Enter after it: {got:?}");
}

#[test]
fn paste_it_into_a_tab_leaves_a_notification_unsent_in_the_tab_picked() {
    let got = hand_to_a_tab(
        "Paste it into a tab",
        "Paste into: Slack: Ada",
        "Pastes it without sending",
        Before::Reply,
    );
    assert!(got.ends_with("> a question"), "no Enter after it: {got:?}");
}

// A view a notification's "Show it in" led to is a place on the trail, and
// the one left by "Open a tab for it": M-Tab from the new tab comes back
// to it, however long the directory was asked for, and back from it goes
// to the Notifications view.
#[test]
fn m_tab_returns_to_the_view_a_notification_led_to_after_a_tab_opened_for_its_item() {
    let server = TestServer::start();
    Claude::new()
        .answer("exec sleep 30")
        .install(&server.home.join("bin"));
    std::fs::create_dir_all(&server.state).unwrap();
    let secs = switchboard::util::now_ms() / 1000;
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": secs * 1000, "items": [
            { "kind": "dm", "channel": "D1", "from": "Ada", "where": "Ada",
                "text": "hello from ada", "ts": format!("{}.000100", secs - 100),
                "link": "https://slack.example/a" },
        ] })
        .to_string(),
    )
    .unwrap();
    settings(
        &server,
        json!({ "view_slack": true, "ux2": true, "slack_threads": true,
            "slack_background": false, "photos": false }),
    );
    let mut client = server.attach(204, 40);
    client.wait_text("$");
    open(&mut client);
    client.type_text("vnotifications\r");
    let on = |view: &'static str| {
        move |s: &[String]| {
            header(s).trim_start().starts_with(view) && s.iter().any(|l| l.contains("Ada"))
        }
    };
    client.wait_for("the Notifications view in front", on("Notifications"));
    client.type_text("\r");
    pick(&mut client, "Show it in the Slack view");
    client.wait_for("the Slack view in front", on("Slack"));
    client.type_text("\r");
    pick(&mut client, "Open a tab for it");
    client.wait_text("A tab for it");
    client.type_text("\r");
    wait_list(&server, "the tab for it in front", |l| {
        tabs(l).len() == 2 && tabs(l)[1]["current"] == true
    });
    client.wait_for("mission control closed", closed);
    // Back from the new tab is the Slack view, over the tab it was opened
    // over, and back from that the Notifications view it was reached from.
    client.type_text(&format!("{ALT}{{"));
    client.wait_for("the Slack view again", on("Slack"));
    wait_list(&server, "the first tab under it", |l| {
        tabs(l)[0]["current"] == true
    });
    client.type_text(&format!("{ALT}{{"));
    client.wait_for("the Notifications view before it", on("Notifications"));
    let new_tab_in_front = |client: &mut TestClient| {
        client.wait_for("mission control closed", closed);
        wait_list(&server, "the new tab in front", |l| {
            tabs(l)[1]["current"] == true
        });
    };
    client.type_text(&format!("{ALT}}}"));
    client.wait_for("the Slack view, forward", on("Slack"));
    client.type_text(&format!("{ALT}}}"));
    new_tab_in_front(&mut client);
    // M-Tab from the tab is the Slack view too, and again the tab.
    client.type_text(&format!("{ALT}\t"));
    client.wait_for("the Slack view by M-Tab", on("Slack"));
    client.type_text(&format!("{ALT}\t"));
    new_tab_in_front(&mut client);
}

// A tab picker left with no tab chosen lets the view go: the tab under it
// is the place, and M-Tab from it the view.
#[test]
fn m_tab_returns_to_a_view_left_by_send_it_to_a_tab_with_no_tab_chosen() {
    let server = TestServer::start();
    std::fs::create_dir_all(&server.state).unwrap();
    let ts = format!("{}.000100", switchboard::util::now_ms() / 1000);
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": 1, "items": [{
            "kind": "dm", "channel": "D1", "from": "Ada", "where": "Ada",
            "text": "a question", "ts": ts, "link": "https://slack.example/m",
        }] })
        .to_string(),
    )
    .unwrap();
    settings(&server, json!({ "view_slack": true }));
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    client.wait_text("2:second");
    // The second tab's shell counts as its Claude, so the picker has a tab.
    let pid = tabs(&server.list())[1]["panes"][0]["pid"].as_u64().unwrap();
    session_file(&server.home, pid, "s-second");
    wait_list(&server, "the second tab's Claude", |l| {
        !tabs(l)[1]["panes"][0]["claude"].is_null()
    });
    open(&mut client);
    client.type_text("vslack\r");
    client.wait_text("a question");
    client.type_text("\r");
    pick(&mut client, "Send it to a tab");
    client.wait_text("Send to: Slack: Ada");
    client.type_text("\x1b");
    client.wait_for("the picker gone", |s| {
        closed(s) && !s.iter().any(|l| l.contains("Send to: Slack: Ada"))
    });
    client.type_text(&format!("{ALT}\t"));
    client.wait_text("MISSION CONTROL");
    client.wait_text("a question");
    client.type_text(&format!("{ALT}\t"));
    client.wait_for("mission control closed", closed);
    wait_list(&server, "still the second tab", |l| {
        tabs(l)[1]["current"] == true
    });
}

// "Open a tab for it" on a notification: a directory asked for, then a new
// tab named for it whose Claude is told about it; a note's by its name and
// path alone.
#[test]
fn open_a_tab_for_it_starts_claude_told_about_the_notification() {
    let notes = common::TempDir::new("sbtf");
    std::fs::write(notes.path().join("Plan.md"), "# Plan\nsecret words\n").unwrap();
    let server = TestServer::start_with_env(&[("NOTES_DIR", notes.path().to_str().unwrap())]);
    // Its args written aside and moved into place, so the wait for the file
    // finds it whole.
    Claude::new()
        .answer("for a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$HOME/claude-args.part\"\nmv \"$HOME/claude-args.part\" \"$HOME/claude-args\"\nexec sleep 30")
        .install(&server.home.join("bin"));
    std::fs::create_dir_all(&server.state).unwrap();
    std::fs::write(
        server.state.join("new-notes.json"),
        json!([{ "path": "Plan.md", "title": "Plan", "source": "Fathom",
            "at": switchboard::util::now_ms() }])
        .to_string(),
    )
    .unwrap();
    settings(&server, json!({ "new_notes": true }));
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    open(&mut client);
    client.type_text("]");
    client.wait_text("Plan  shown");
    client.type_text("j");
    client.type_text("\r");
    pick(&mut client, "Open a tab for it");
    client.wait_text("A tab for it");
    client.type_text("\r");
    let list = wait_list(&server, "the note's tab", |l| tabs(l).len() == 2);
    assert_eq!(tabs(&list)[1]["options"]["@tab_name"], "Note: Plan");
    let args = server.home.join("claude-args");
    let deadline = common::deadline(Duration::from_secs(10));
    while !args.exists() {
        assert!(
            Instant::now() < deadline,
            "the stand-in Claude didn't start"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let args = std::fs::read_to_string(&args).unwrap();
    let path = notes.path().join("Plan.md");
    assert!(
        args.contains(&format!("Note: Plan ({})", path.display())),
        "{args}"
    );
    assert!(args.contains("don't read it unless I ask"), "{args}");
    assert!(!args.contains("secret words"), "{args}");
}

// With UX 2.0 the Slack view is board 21: the chosen message's thread,
// read through the connector once when it isn't kept, beside the list.
#[test]
fn choosing_a_slack_message_reads_its_thread_once_and_draws_it_beside() {
    let server = TestServer::start();
    std::fs::create_dir_all(&server.state).unwrap();
    let secs = switchboard::util::now_ms() / 1000;
    let first = format!("{}.000100", secs - 600);
    let reply = format!("{}.000100", secs - 60);
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": secs * 1000, "items": [{
            "kind": "thread", "channel": "C1", "from": "Lin", "user": "ULIN",
            "where": "#dev", "text": "an answer", "ts": reply, "thread": first,
            "link": "https://slack.example/m",
        }] })
        .to_string(),
    )
    .unwrap();
    let thread = format!(
        "=== THREAD PARENT MESSAGE ===\nFrom: Me (UME)\nMessage TS: {first}\nmy first question\n\n=== THREAD REPLIES (1 total) ===\n\n--- Reply 1 of 1 ---\nFrom: Lin (ULIN)\nMessage TS: {reply}\nan answer\n"
    );
    let read = json!({ "type": "assistant", "message": { "content": [{
        "type": "tool_use", "id": "t1", "name": "mcp__claude_ai_Slack__slack_read_thread", "input": {},
    }] } });
    let result = json!({ "type": "user", "message": { "content": [{
        "type": "tool_result", "tool_use_id": "t1",
        "content": [{ "type": "text", "text": json!({ "messages": thread }).to_string() }],
    }] } });
    let calls = server.home.join("thread-reads");
    Claude::new()
        .answer(&format!(
            "echo read >> {}\nprintf '%s\\n' {} {}",
            common::stand_in::sh_quote(&calls.to_string_lossy()),
            common::stand_in::sh_quote(&read.to_string()),
            common::stand_in::sh_quote(&result.to_string()),
        ))
        .install(&server.home.join("bin"));
    settings(
        &server,
        json!({ "view_slack": true, "ux2": true, "slack_threads": true, "slack_background": false, "photos": false }),
    );
    let mut client = server.attach(204, 40);
    client.wait_text("$");
    open(&mut client);
    client.type_text("vslack\r");
    let screen = client.wait_text("my first question");
    let all = screen.join("\n");
    assert!(all.contains("◇ Threads"), "{all}");
    assert!(all.contains("thread · 1 reply"), "{all}");
    let kept = std::fs::read_to_string(server.state.join("slack-threads.json")).unwrap();
    assert!(kept.contains(&format!("C1:{first}")), "{kept}");
    // Moving and coming back reads nothing again: it is kept.
    client.type_text("k");
    client.type_text("j");
    client.wait_text("my first question");
    let reads = std::fs::read_to_string(&calls).unwrap();
    assert_eq!(reads.lines().count(), 1, "read once");
}

// With UX 2.0 the Slack view lists its sections as Slack's sidebar does
// (board 21), the chosen one's messages beside them, newest first: j in
// the sections moves to the next and shows its messages, l then j moves
// within them and stops at the section's end, and a click on a section
// shows its messages.
#[test]
fn the_slack_views_sections_choose_the_messages_beside_them() {
    let server = TestServer::start();
    std::fs::create_dir_all(&server.state).unwrap();
    let secs = switchboard::util::now_ms() / 1000;
    let ts = |ago: i64| format!("{}.000100", secs - ago);
    let said = |kind: &str, channel: &str, place: &str, from: &str, text: &str, ago: i64| {
        json!({ "kind": kind, "channel": channel, "where": place, "from": from,
            "user": format!("U{from}"), "text": text, "ts": ts(ago) })
    };
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": secs * 1000, "items": [
            said("mention", "C1", "#dev", "Ada", "mention words", 300),
            said("dm", "D1", "Bo", "Bo", "dm words", 200),
            said("channel", "C2", "#ops", "Dee", "ops older", 100),
            said("channel", "C2", "#ops", "Cy", "ops newer", 10),
        ] })
        .to_string(),
    )
    .unwrap();
    settings(
        &server,
        json!({ "view_slack": true, "ux2": true, "slack_threads": false, "slack_background": false, "photos": false }),
    );
    let mut client = server.attach(204, 40);
    client.wait_text("$");
    open(&mut client);
    // UX 2.0 taken up first, its views grouped, so the view opens as the
    // board, on its first section.
    client.wait_for("the views grouped", |s| {
        s.iter().any(|l| l.starts_with(" WORK ")) && s.iter().any(|l| l.starts_with(" SYSTEM "))
    });
    client.type_text("vslack\r");
    // The board once UX 2.0 is taken up.
    let screen = client.wait_text("@ Mentions");
    let all = screen.join("\n");
    assert!(
        all.contains("mention words") && all.contains("# ops"),
        "{all}"
    );
    assert!(!all.contains("dm words"), "another section's: {all}");
    // h gives the sections the focus, and j walks them.
    client.type_text("h");
    client.type_text("j");
    client.wait_for("Bo's messages", |s| {
        let all = s.join("\n");
        all.contains("dm words") && !all.contains("mention words")
    });
    client.type_text("j");
    let screen = client.wait_text("ops newer");
    let all = screen.join("\n");
    let at = |s: &str| all.find(s).unwrap_or_else(|| panic!("{s} in\n{all}"));
    assert!(at("ops newer") < at("ops older"), "newest first: {all}");
    // l gives the messages the focus; j moves within them, and stops at
    // the section's end.
    let chosen = |s: &[String], from: &str| {
        s.iter()
            .any(|l| l.contains(&format!("▌{}", &from[..1])) && l.contains(from))
    };
    client.type_text("l");
    client.type_text("j");
    client.wait_for("Dee chosen", |s| chosen(s, "Dee"));
    client.type_text("j");
    std::thread::sleep(Duration::from_millis(300) * common::patience());
    let screen = client.screen();
    assert!(chosen(&screen, "Dee"), "{}", screen.join("\n"));
    assert!(!screen.join("\n").contains("mention words"));
    // A click on Mentions shows its messages.
    let row = screen
        .iter()
        .position(|l| l.contains("@ Mentions"))
        .unwrap_or_else(|| panic!("Mentions' row:\n{}", screen.join("\n")));
    let col = screen[row]
        .chars()
        .collect::<Vec<_>>()
        .windows(2)
        .position(|w| w == ['@', ' '])
        .unwrap();
    client.send(
        format!(
            "\x1b[<0;{};{}M\x1b[<0;{};{}m",
            col + 3,
            row + 1,
            col + 3,
            row + 1
        )
        .as_bytes(),
    );
    client.wait_text("mention words");
    // The wheel over the sections walks them.
    client.send(format!("\x1b[<65;{};{}M", col + 3, row + 1).as_bytes());
    client.wait_for("Bo's messages", |s| {
        let all = s.join("\n");
        all.contains("dm words") && !all.contains("mention words")
    });
}

// Board 21 lists a section one row per thread: three new replies in one
// thread are one row, headed by its first message, while the thread
// beside it shows each reply, and Enter on the row opens its menu.
#[test]
fn the_slack_board_lists_a_threads_replies_as_one_row() {
    let server = TestServer::start();
    std::fs::create_dir_all(&server.state).unwrap();
    let secs = switchboard::util::now_ms() / 1000;
    let ts = |ago: i64| format!("{}.000100", secs - ago);
    let root = ts(900);
    let reply = |from: &str, text: &str, ago: i64| {
        json!({ "kind": "channel", "channel": "C1", "where": "#dev", "from": from,
            "user": format!("U{from}"), "text": text, "ts": ts(ago), "thread": root,
            "link": "https://slack.example/m" })
    };
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": secs * 1000, "items": [
            reply("Ben", "reply one", 300),
            reply("Cy", "reply two", 200),
            reply("Ben", "reply three", 100),
        ] })
        .to_string(),
    )
    .unwrap();
    let said = |from: &str, text: &str, at: &str| json!({ "user": format!("U{from}"), "from": from, "text": text, "ts": at });
    std::fs::write(
        server.state.join("slack-threads.json"),
        json!({ format!("C1:{root}"): {
            "messages": [
                said("Ana", "the question", &root),
                said("Ben", "reply one", &ts(300)),
                said("Cy", "reply two", &ts(200)),
                said("Ben", "reply three", &ts(100)),
            ],
            "replies": 3,
            "read": secs * 1000,
        } })
        .to_string(),
    )
    .unwrap();
    settings(
        &server,
        json!({ "view_slack": true, "ux2": true, "slack_threads": true, "slack_background": false, "photos": false }),
    );
    let mut client = server.attach(204, 40);
    client.wait_text("$");
    open(&mut client);
    client.wait_for("the views grouped", |s| {
        s.iter().any(|l| l.starts_with(" WORK ")) && s.iter().any(|l| l.starts_with(" SYSTEM "))
    });
    client.type_text("vslack\r");
    let screen = client.wait_text("reply three");
    let all = screen.join("\n");
    // Past the column of groups and views, then the sections.
    let list = |l: &String| l.split('│').nth(2).unwrap_or("").to_string();
    let listed: Vec<String> = screen.iter().map(list).collect();
    assert!(listed.iter().any(|l| l.contains("the question")), "{all}");
    assert!(
        !listed
            .iter()
            .any(|l| l.contains("reply one") || l.contains("reply three")),
        "the replies aren't rows of their own: {all}"
    );
    assert!(
        listed
            .iter()
            .any(|l| l.contains("3 replies") && l.contains("3 new")),
        "{all}"
    );
    let beside: Vec<String> = screen
        .iter()
        .map(|l| l.split('│').nth(3).unwrap_or("").to_string())
        .collect();
    for words in ["reply one", "reply two", "reply three"] {
        assert!(
            beside.iter().any(|l| l.contains(words)),
            "{words} beside: {all}"
        );
    }
    client.type_text("\r");
    client.wait_text("Dismiss it, until its thread gets a reply");
}

// The Slack board shows one section at a time, so leaving it marks seen
// only the messages of the sections it showed, not the whole look.
#[test]
fn leaving_the_slack_board_marks_only_the_sections_shown_seen() {
    let server = TestServer::start();
    std::fs::create_dir_all(&server.state).unwrap();
    let secs = switchboard::util::now_ms() / 1000;
    let (mention, dm) = (
        format!("{}.000100", secs - 300),
        format!("{}.000100", secs - 200),
    );
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": secs * 1000, "items": [
            { "kind": "mention", "channel": "C1", "where": "#dev", "from": "Ada", "user": "UAda",
              "text": "mention words", "ts": mention },
            { "kind": "dm", "channel": "D1", "where": "Bo", "from": "Bo", "user": "UBo",
              "text": "dm words", "ts": dm },
        ] })
        .to_string(),
    )
    .unwrap();
    settings(
        &server,
        json!({ "view_slack": true, "ux2": true, "slack_threads": false, "slack_background": false, "photos": false }),
    );
    let mut client = server.attach(204, 40);
    client.wait_text("$");
    open(&mut client);
    // UX 2.0 taken up first, its views grouped, so the view opens as the
    // board, on its first section.
    client.wait_for("the views grouped", |s| {
        s.iter().any(|l| l.starts_with(" WORK ")) && s.iter().any(|l| l.starts_with(" SYSTEM "))
    });
    client.type_text("vslack\r");
    client.wait_text("@ Mentions");
    client.wait_text("mention words");
    client.type_text("\x1b");
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    let seen = server.state.join("slack-seen-items.json");
    let deadline = common::deadline(Duration::from_secs(5));
    while !seen.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let kept = std::fs::read_to_string(&seen).unwrap_or_default();
    assert!(kept.contains(&format!("C1:{mention}")), "{kept}");
    assert!(!kept.contains(&format!("D1:{dm}")), "{kept}");
    assert!(
        !server.state.join("slack-seen").exists(),
        "the look as a whole isn't seen"
    );
}

// A notification no view lists as an item is shown whole above its choices
// in the Notifications view, as the Notifications window shows it: a landing
// made without asking says what it was, then leads to its tab.
#[test]
fn a_notification_no_view_lists_is_whole_above_its_choices() {
    let server = TestServer::start();
    settings(&server, json!({ "autopilot_landing_notices": true }));
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    let pane = server.list()["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let said = std::process::Command::new(common::BIN)
        .args(["land-lock", "autolanded", "abc1234", "Make", "the", "thing"])
        .current_dir(server.dir.path())
        .env("HOME", &server.home)
        .env("SWITCHBOARD_STATE_DIR", &server.state)
        .env("SWITCHBOARD", server.socket())
        .env("SWITCHBOARD_PANE", &pane)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        said.status.success(),
        "{}",
        String::from_utf8_lossy(&said.stderr)
    );
    open(&mut client);
    client.type_text("]");
    client.wait_text("Landed abc1234: Make the thing");
    client.type_text("j");
    client.type_text("\r");
    let screen = client.wait_text("Dismiss it");
    let all = screen.join("\n");
    let row = |text: &str| {
        screen
            .iter()
            .position(|l| l.contains(text))
            .unwrap_or_else(|| panic!("{text} isn't shown: {all}"))
    };
    assert!(
        row("Landed on the default branch without asking") < row("Go to its tab"),
        "what it was above its choices: {all}"
    );
    assert!(row("Go to its tab") < row("Dismiss it"), "{all}");
    pick(&mut client, "Dismiss it");
    client.wait_text("dismissed");
}

// The Notifications view keeps every notification of the last two weeks by
// day: the days before yesterday past the first screenful fold until their
// menu's "Show them", and the view's menu narrows it to a kind.
#[test]
fn the_notifications_history_folds_old_days_and_its_menu_narrows_it_to_a_kind() {
    let mut server = TestServer::start();
    settings(&server, json!({}));
    let now = switchboard::util::now_ms();
    let entry = |source: &str, id: &str, title: &str, at: i64| {
        json!({ "source": source, "id": id, "at": at, "sidebar": "older",
            "item": { "id": id, "title": title } })
    };
    let dir = server.state.join("notifications");
    std::fs::create_dir_all(&dir).unwrap();
    let day_of = |ms: i64| switchboard::sidebar::history::day_of(ms, &jiff::tz::TimeZone::system());
    let today: Vec<Value> = (0..30)
        .map(|n| entry("slack", &format!("s{n}"), &format!("Slack {n}"), now - n))
        .collect();
    std::fs::write(
        dir.join(format!("{}.json", day_of(now))),
        Value::from(today).to_string(),
    )
    .unwrap();
    let old = now - 4 * 86_400_000;
    std::fs::write(
        dir.join(format!("{}.json", day_of(old))),
        json!([entry("inbox", "i1", "Old inbox one", old)]).to_string(),
    )
    .unwrap();
    server.restart();
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    open(&mut client);
    client.type_text("]");
    client.wait_text("Slack 0");
    client.type_text("G");
    client.wait_text("1 more, folded");
    assert!(!client.screen().join("\n").contains("Old inbox one"));
    // A folded day is an item: its menu shows it.
    client.type_text("\r");
    pick(&mut client, "Show them");
    client.wait_text("Old inbox one");
    // The PRs, of which there are none, then the Inbox, from `m`'s menu;
    // a number is no key of the view's.
    client.type_text("3");
    client.type_text("m");
    client.wait_text("Dismiss every notification shown");
    pick(&mut client, "Show only PRs");
    client.wait_text("No notifications in the last two weeks.");
    client.type_text("m");
    pick(&mut client, "Show only Inbox");
    let screen = client.wait_text("Old inbox one");
    assert!(
        !screen.join("\n").contains("Slack 0"),
        "{}",
        screen.join("\n")
    );
}

#[test]
fn with_ux2_resources_ends_a_tabs_busy_process_from_its_menu_after_a_yes() {
    let server = TestServer::start();
    settings(&server, json!({"view_resources": true, "ux2": true}));
    let mut client = server.attach(174, 36);
    client.wait_text("$");
    // A process pegging a core in the tab.
    client.type_text("yes > /dev/null\r");
    open(&mut client);
    client.type_text("vresources\r");
    // The tab's row, chosen, with its busiest process under it.
    client.wait_text("CPU, LAST 2 MINUTES");
    client.wait_for("the busy process read", |s| {
        s.iter().any(|l| l.contains("yes ") && l.contains('%'))
    });
    client.type_text("\r");
    client.wait_text("Go to tab 1");
    pick(&mut client, "End yes");
    client.wait_text("End yes in tab 1?");
    client.type_text("\r");
    client.wait_text("Ended yes.");
    // The shell has its prompt back.
    let deadline = common::deadline(Duration::from_secs(10));
    loop {
        let pids: Vec<u32> = server.list()["sessions"][0]["tabs"][0]["panes"][0]["pid"]
            .as_u64()
            .map(|p| vec![p as u32])
            .unwrap_or_default();
        let still = pids.iter().any(|&shell| {
            switchboard::mux::procinfo::children_of(shell)
                .iter()
                .any(|&c| switchboard::mux::procinfo::command_of(c).as_deref() == Some("yes"))
        });
        if !still {
            break;
        }
        assert!(Instant::now() < deadline, "yes still runs");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_key_found_with_a_question_mark_is_not_pressed_and_enter_shows_it_in_the_list() {
    let (server, mut client) = two_tabs(json!({"view_keys": true}));
    client.type_text(&format!("{ALT}/"));
    client.wait_text("PANES");
    // The view's menu looks a key up without pressing it: j is the
    // sidebar's.
    client.type_text("m");
    pick(&mut client, "Find a key by pressing it");
    client.wait_text("Find a key");
    client.type_text("j");
    client.wait_text("Sidebar: ");
    // M-w, which closes mission control elsewhere, is looked up here.
    client.type_text(&format!("{ALT}w"));
    client.wait_text("Close the pane");
    client.type_text(&format!("{ALT}L"));
    let screen = client.wait_text("Before it, M-w");
    assert!(
        screen.iter().any(|l| l.contains("Split pane to the right")),
        "{}",
        screen.join("\n")
    );
    assert!(
        screen.iter().any(|l| l.contains("Before it, M-w")),
        "{}",
        screen.join("\n")
    );
    // Enter shows it in the list, where Enter's menu presses it, splitting
    // the pane. Nothing was pressed while the box was up.
    let panes = |server: &TestServer| {
        tabs(&server.list())[1]["panes"]
            .as_array()
            .map_or(0, Vec::len)
    };
    assert_eq!(panes(&server), 1);
    client.type_text("\r");
    client.wait_for("the box closed", |s| {
        !s.iter().any(|l| l.contains("Find a key"))
    });
    client.type_text("\r");
    pick(&mut client, "Press it here");
    wait_list(&server, "the pane split", |l| {
        tabs(l)[1]["panes"].as_array().is_some_and(|p| p.len() == 2)
    });
}

/// With UX 2.0 the Inbox is a board: each notification by who acted, the
/// chosen one in its ticket beside the list with its own comment marked
/// new. Enter offers the Tickets board's menu without "Mark it read",
/// marking it read in Linear as the menu closes, but for its last choice,
/// which takes it out of the view and leaves it unread there; `o` and `x`
/// are no keys of the view's.
#[test]
fn with_ux2_the_inbox_shows_each_in_its_ticket_and_its_menu_acts_on_it() {
    use common::http::{Fake, Reply};
    let at = |minutes: i64| {
        (jiff::Timestamp::now() - jiff::SignedDuration::from_mins(minutes)).to_string()
    };
    let (ben, ana) = (at(10), at(20));
    // What Linear was told is read, which it then leaves out.
    let read = std::sync::Mutex::new(Vec::<String>::new());
    let linear = Fake::linear("test-key", move |request| {
        let query = request.query();
        let answer = if query.contains("notificationUpdate") {
            let id = request.json()["variables"]["id"]
                .as_str()
                .unwrap_or("")
                .to_string();
            read.lock().unwrap().push(id);
            json!({ "data": { "notificationUpdate": { "success": true } } })
        } else if query.contains("notifications(") {
            let issue = |n: u32, title: &str| json!({ "identifier": format!("ABC-{n}"), "title": title, "url": format!("https://linear.app/acme/issue/ABC-{n}") });
            json!({ "data": { "notifications": { "nodes": [
                { "id": "n1", "type": "issueNewComment", "createdAt": ben, "actor": { "id": "u-ben", "name": "Ben" },
                  "issue": issue(7, "Hold the landing lock"),
                  "comment": { "id": "c-2", "body": "the new one", "url": "https://linear.app/acme/issue/ABC-7#c-2" } },
                { "id": "n2", "type": "issueAssignedToYou", "createdAt": ana, "actor": { "id": "u-ana", "name": "Ana" },
                  "readAt": read.lock().unwrap().contains(&"n2".to_string()).then(|| ana.clone()),
                  "issue": issue(8, "Count the requests"), "comment": null },
            ] } } })
        } else if query.contains("issue(id") {
            let id = request.json()["variables"]["id"]
                .as_str()
                .unwrap_or("")
                .to_string();
            json!({ "data": { "issue": {
                "identifier": id, "title": "A ticket", "url": "https://linear.app/acme/issue/x",
                "updatedAt": jiff::Timestamp::now().to_string(), "state": { "name": "In Progress", "type": "started" },
                "priorityLabel": "High", "description": "What it is about.",
                "comments": { "nodes": [
                    { "id": "c-1", "body": "an earlier thought", "createdAt": at(30), "user": { "id": "u-kai", "name": "Kai" } },
                    { "id": "c-2", "body": "the new one", "createdAt": at(10), "user": { "id": "u-ben", "name": "Ben" } },
                ] }
            } } })
        } else {
            return None;
        };
        Some(Reply::json(200, answer))
    });
    let server = TestServer::start_with_env(&[
        ("LINEAR_API_KEY", "test-key"),
        ("SWITCHBOARD_LINEAR_API", &linear.graphql_url()),
    ]);
    settings(
        &server,
        json!({"ux2": true, "linear": true, "view_inbox": true, "photos": false}),
    );
    let mut client = server.attach(194, 40);
    client.wait_text("$");
    open(&mut client);
    client.type_text("vinbox\r");
    client.wait_text("Ben commented");
    client.wait_text("an earlier thought");
    client.wait_text("Ben  10m ago");
    client.type_text("ox");
    client.type_text("\r");
    pick(&mut client, "Dismiss it here: it stays unread in Linear");
    client.wait_for("ABC-7 out of the view", |s| {
        !s.join("\n").contains("Ben commented") && s.join("\n").contains("assigned to you by Ana")
    });
    client.type_text("\r");
    let menu = client.wait_text("Start work on it in a new tab");
    assert!(
        !menu.join("\n").contains("Mark it read"),
        "{}",
        menu.join("\n")
    );
    client.type_text("\x1b");
    client.wait_for("ABC-8 marked read and out of the view", |s| {
        !s.join("\n").contains("assigned to you by Ana")
    });
    assert!(
        linear.seen().iter().any(
            |r| r.query().contains("notificationUpdate") && r.json()["variables"]["id"] == "n2"
        ),
        "marked read in Linear"
    );
    assert!(
        !linear.seen().iter().any(
            |r| r.query().contains("notificationUpdate") && r.json()["variables"]["id"] == "n1"
        ),
        "a dismissed one stays unread in Linear"
    );
}

/// A server with UX 2.0 on and two tabs, the second in front, and a
/// client on it.
fn two_tabs_ux2() -> (TestServer, TestClient) {
    let server = TestServer::start();
    settings(&server, json!({"ux2": true, "reminders": true}));
    let mut client = server.attach(160, 40);
    client.wait_text("$");
    // UX 2.0 taken up: each tab has a header.
    client.wait_text("M-c Claude");
    server.ok(&["new-tab", "-n", "second"]);
    wait_list(&server, "the second tab in front", |l| {
        tabs(l).len() == 2 && tabs(l)[1]["current"] == true
    });
    client.wait_text("second");
    (server, client)
}

/// The first tab's id, for `set -t`.
fn first_tab_id(server: &TestServer) -> String {
    tabs(&server.list())[0]["id"].as_str().unwrap().to_string()
}

fn remind_at(list: &Value, tab: usize) -> Option<i64> {
    tabs(list)[tab]["options"]["@remind_at"]
        .as_str()
        .and_then(|v| v.parse().ok())
}

/// The box saying what was done (`Server::say`, mission control being
/// closed) put away.
fn said(client: &mut TestClient, words: &str) {
    client.wait_text(words);
    client.type_text("\x1b");
    client.wait_for("the message gone", |s| !s.iter().any(|l| l.contains(words)));
}

/// M-p's "Remind me about this tab", which with UX 2.0 opens the box.
fn remind_box(client: &mut TestClient, title: &str) {
    client.type_text(&format!("{ALT}p"));
    client.wait_text("Actions");
    client.type_text("remind me about this");
    client.wait_text("Remind me about this tab later");
    client.type_text("\r");
    client.wait_text(title);
}

#[test]
fn with_ux2_the_reminder_box_sets_a_time_offered_a_date_typed_and_clears_it() {
    let (server, mut client) = two_tabs_ux2();
    remind_box(&mut client, "Remind me about 2 second");
    client.wait_text("In 3 hours");
    // ↓ ↓ to "In 3 hours", Tab to What for, a note, Enter.
    client.send(b"\x1b[B");
    client.send(b"\x1b[B");
    client.type_text("\t");
    client.type_text("the handoff\r");
    let list = wait_list(&server, "the reminder set", |l| remind_at(l, 1).is_some());
    let now = jiff::Timestamp::now().as_second();
    let at = remind_at(&list, 1).unwrap();
    assert!(
        (at - now - 3 * 3600).abs() < 60,
        "{at} is three hours from {now}"
    );
    assert_eq!(tabs(&list)[1]["options"]["@remind_note"], "the handoff");
    said(&mut client, "Reminder set for 2 second");

    remind_box(&mut client, "Change the reminder for 2 second");
    client.wait_text("Set now:");
    client.type_text("nov 20 3pm");
    client.wait_text("✓ ");
    client.type_text("\r");
    let local = jiff::Zoned::now();
    let expected = mission_when("nov 20 3pm", &local);
    let list = wait_list(&server, "the date set", |l| {
        remind_at(l, 1) == Some(expected)
    });
    assert_eq!(tabs(&list)[1]["options"]["@remind_note"], "the handoff");
    said(&mut client, "Reminder set for 2 second");

    remind_box(&mut client, "Change the reminder for 2 second");
    client.type_text(&format!("{ALT}x"));
    let list = wait_list(&server, "the reminder cleared", |l| {
        remind_at(l, 1).is_none()
    });
    assert!(tabs(&list)[1]["options"]["@remind_note"].is_null());
}

/// When `text` names, as the server reads it.
fn mission_when(text: &str, now: &jiff::Zoned) -> i64 {
    switchboard::mission::remind::when(text, now).unwrap()
}

#[test]
fn with_ux2_the_reminders_view_moves_its_day_and_month_and_its_menu_goes() {
    let (server, mut client) = two_tabs_ux2();
    let id = first_tab_id(&server);
    let soon = (jiff::Timestamp::now().as_second() + 120).to_string();
    server.ok(&["set", "-t", &id, "@remind_at", &soon]);
    server.ok(&["set", "-t", &id, "@remind_note", "look at the build"]);
    open(&mut client);
    client.type_text("vreminders\r");
    client.wait_text("1 reminder");
    client.wait_text("look at the build");
    let today = jiff::Zoned::now().date();
    let title = |d: jiff::civil::Date| {
        format!(
            "{:?}, {} {}",
            d.weekday(),
            switchboard::mission::remind::MONTHS[(d.month() - 1) as usize],
            d.day()
        )
    };
    let month = |d: jiff::civil::Date| {
        format!(
            "{} {}",
            switchboard::mission::remind::MONTHS[(d.month() - 1) as usize].to_uppercase(),
            d.year()
        )
    };
    use switchboard::mission::reminders::step;
    client.type_text("l");
    client.wait_text(&title(step(today, 1, 0)));
    client.type_text("j");
    client.wait_text(&title(step(today, 8, 0)));
    client.type_text("k");
    client.type_text("h");
    client.wait_text(&title(today));
    client.type_text("L");
    client.wait_text(&month(step(today, 0, 1)));
    client.type_text("H");
    client.wait_text(&month(today));
    // t is no key: the view's menu goes back to today.
    client.type_text("L");
    client.type_text("t");
    client.type_text("m");
    pick(&mut client, "Go to today");
    client.wait_text(&title(today));
    client.wait_text("look at the build");
    // x and g are no keys of the view's: Enter's menu is its tab's, going
    // there first.
    client.type_text("xg");
    client.type_text("\r");
    client.wait_text("Change or clear its reminder");
    client.type_text("\r");
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    assert!(remind_at(&server.list(), 0).is_some(), "x cleared nothing");
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
}

// `m` opens the view's own menu in every view, ending with mission
// control's: several views at once, and this view's settings, which shows
// Settings filtered by the view's name.
#[test]
fn m_opens_the_views_menu_ending_with_its_settings() {
    let (_server, mut client) = two_tabs_ux2();
    open(&mut client);
    client.type_text("m");
    let screen = client.wait_text("This view's settings");
    let at = |text: &str| {
        screen
            .iter()
            .position(|l| l.contains(text))
            .unwrap_or_else(|| panic!("{text}: {}", screen.join("\n")))
    };
    assert!(at("Several views at once") < at("This view's settings"));
    pick(&mut client, "This view's settings");
    client.wait_text("/Tabs");
}

#[test]
fn the_view_menu_picks_views_to_show_at_once_and_esc_goes_back_to_one() {
    let server = TestServer::start();
    settings(&server, json!({ "ux2": true }));
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    wait_list(&server, "two tabs", |l| tabs(l).len() == 2);
    // UX 2.0 taken up: each tab has its header.
    client.wait_text("M-c Claude");
    open(&mut client);
    // The box, from the view's menu: none of board 11's four is on here,
    // so nothing is picked.
    client.type_text("m");
    pick(&mut client, "Several views at once");
    client.wait_text("Views at once");
    client.wait_text("☐ Tabs");
    // The tabs, then the next view, picked and shown.
    client.type_text(" j \r");
    let screen = client.wait_text("2 views at once");
    assert!(
        screen
            .iter()
            .any(|l| l.contains(" TABS") && l.contains("2 tabs") && l.contains('│')),
        "{}",
        screen.join("\n")
    );
    client.wait_text("tab next view");
    // Kept for next time, after a restart too.
    let kept = server.state.join("mission-grid.json");
    let choice: Value = serde_json::from_str(&std::fs::read_to_string(&kept).unwrap()).unwrap();
    assert_eq!(choice["views"][0], "tabs", "{choice}");
    assert_eq!(choice["views"].as_array().unwrap().len(), 2, "{choice}");
    // Tab gives the focus to the other view and back.
    client.type_text("\t");
    client.type_text("\t");
    // Opened again, it shows the last choice.
    client.type_text("m");
    pick(&mut client, "Several views at once");
    let screen = client.wait_text("Views at once");
    assert!(
        screen
            .iter()
            .any(|l| l.contains("☑ Tabs") && l.contains('1')),
        "{}",
        screen.join("\n")
    );
    client.type_text("\r");
    client.wait_text("2 views at once");
    // Esc goes back to one view, a second closes mission control.
    client.type_text("\x1b");
    client.wait_for("one view again", |s| {
        s.iter().any(|l| l.contains("MISSION CONTROL"))
            && !s.iter().any(|l| l.contains("views at once"))
    });
    client.type_text("\x1b");
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
}

/// A server with three tabs, the first in front, and a client on it.
fn three_tabs(extra: Value) -> (TestServer, TestClient) {
    let (server, mut client) = two_tabs(extra);
    server.ok(&["new-tab", "-n", "third"]);
    client.wait_text("3:third");
    client.type_text(&format!("{ALT}1"));
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    (server, client)
}

fn closed(s: &[String]) -> bool {
    !s.iter().any(|l| l.contains("MISSION CONTROL"))
}

#[test]
fn m_number_closes_it_and_goes_to_that_tab_as_keys_json_binds_it() {
    let (server, mut client) = three_tabs(json!({}));
    open(&mut client);
    client.type_text(&format!("{ALT}3"));
    client.wait_for("mission control closed", closed);
    wait_list(&server, "the third tab in front", |l| {
        tabs(l)[2]["current"] == true
    });
    // While a filter is typed too.
    open(&mut client);
    client.type_text("/");
    client.type_text(&format!("{ALT}1"));
    client.wait_for("mission control closed", closed);
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    client.type_text(&format!("{ALT}3"));
    wait_list(&server, "the third tab in front", |l| {
        tabs(l)[2]["current"] == true
    });
    // A number with no tab closes it and stays where it was.
    open(&mut client);
    client.type_text(&format!("{ALT}9"));
    client.wait_for("mission control closed", closed);
    assert_eq!(tabs(&server.list())[2]["current"], true, "nowhere gone");
    // M-3 bound to something else does that instead.
    let config = server.home.join(".config/switchboard");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("keys.json"), r#"{"root": {"M-3": "new-tab"}}"#).unwrap();
    std::thread::sleep(Duration::from_millis(2500) * common::patience());
    client.type_text(&format!("{ALT}1"));
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    open(&mut client);
    client.type_text(&format!("{ALT}3"));
    let screen = client.screen();
    assert!(!closed(&screen), "still open:\n{}", screen.join("\n"));
    assert_eq!(tabs(&server.list())[0]["current"], true, "no tab selected");
}

#[test]
fn m_p_opens_the_palette_over_it_and_a_pick_runs_there_or_in_the_pane() {
    let (server, mut client) = two_tabs(json!({}));
    open(&mut client);
    // Esc leaves the view as it was.
    client.type_text(&format!("{ALT}p"));
    let screen = client.wait_text("action>");
    assert!(!closed(&screen), "beneath it:\n{}", screen.join("\n"));
    client.type_text("\x1b");
    let screen = client.wait_for("the palette gone", |s| {
        !s.iter().any(|l| l.contains("action>"))
    });
    assert!(!closed(&screen), "still open:\n{}", screen.join("\n"));
    // One of its views shows in it.
    client.type_text(&format!("{ALT}p"));
    client.wait_text("action>");
    client.type_text("mission control: landing");
    client.wait_text("› Mission control: Landing");
    client.type_text("\r");
    client.wait_text("Nothing is landing now.");
    // Anything else runs in the pane it was opened over, once closed.
    client.type_text(&format!("{ALT}p"));
    client.wait_text("action>");
    client.type_text("split pane above");
    client.wait_text("› Split pane above");
    client.type_text("\r");
    client.wait_for("mission control closed", closed);
    wait_list(&server, "the second tab split", |l| {
        tabs(l)[1]["panes"].as_array().unwrap().len() == 2
    });
}

#[test]
fn m_p_lists_views_beyond_the_old_twelve_and_opens_or_switches_to_them() {
    let (_server, mut client) = two_tabs(json!({"reminders": true}));
    // Closed, it opens mission control on the view picked.
    client.type_text(&format!("{ALT}p"));
    client.wait_text("action>");
    client.type_text("mission control: reminders");
    client.wait_text("› Mission control: Reminders");
    client.type_text("\r");
    client.wait_text("No reminders.");
    // Open, it switches to the one picked.
    client.type_text(&format!("{ALT}p"));
    client.wait_text("action>");
    client.type_text("mission control: notifications");
    client.wait_text("› Mission control: Notifications");
    client.type_text("\r");
    let screen = client.wait_text("No notifications");
    assert!(!closed(&screen), "still open:\n{}", screen.join("\n"));
}

#[test]
fn with_ux2_a_click_on_a_tab_in_the_sidebar_closes_it_and_goes_there() {
    // The tabs sidebar kept beside mission control: off, the sidebar lists
    // its views instead (tests/it/mission_nav.rs).
    let server = TestServer::start();
    settings(
        &server,
        json!({"ux2": true, "sidebar": true, "mission_keep_tabs_sidebar": true}),
    );
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    server.ok(&["new-tab", "-n", "second"]);
    server.ok(&["new-tab", "-n", "third"]);
    let list = wait_list(&server, "three tabs and the sidebar", |l| {
        tabs(l).len() == 3 && l["sessions"][0]["sidebar"]["columns"].as_u64().unwrap_or(0) > 0
    });
    let cols = list["sessions"][0]["sidebar"]["columns"].as_u64().unwrap() as usize;
    // UX 2.0 taken up: the tab bar gone from the bottom.
    client.wait_for("the tab bar gone", |s| {
        !s.iter().any(|l| l.contains("2:second"))
    });
    let screen = open(&mut client);
    let row = screen
        .iter()
        .position(|l| l.chars().take(cols).collect::<String>().contains("second"))
        .unwrap_or_else(|| panic!("the second tab's row:\n{}", screen.join("\n")));
    // The wheel over the sidebar leaves mission control open.
    client.send(format!("\x1b[<65;3;{}M", row + 1).as_bytes());
    std::thread::sleep(Duration::from_millis(300) * common::patience());
    let screen = client.screen();
    assert!(!closed(&screen), "still open:\n{}", screen.join("\n"));
    client.send(format!("\x1b[<0;3;{}M\x1b[<0;3;{}m", row + 1, row + 1).as_bytes());
    client.wait_for("mission control closed", closed);
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
}

/// The header over the view in front, naming it and its group, right of
/// the column listing the groups and views.
fn header(s: &[String]) -> String {
    s.first()
        .and_then(|l| l.split('│').nth(1))
        .unwrap_or_default()
        .to_string()
}

#[test]
fn with_ux2_m_brackets_move_between_groups_braces_do_nothing_and_m_braces_close_it() {
    let (server, mut client) = two_tabs_ux2();
    let screen = open(&mut client);
    assert!(header(&screen).contains("   Work"), "{}", screen.join("\n"));
    // } moves nothing, so M-] after it goes from Work to Day, not to System.
    client.type_text("}");
    client.type_text(&format!("{ALT}]"));
    client.wait_for("Day's views", |s| header(s).contains("   Day"));
    assert_eq!(
        tabs(&server.list())[1]["current"],
        true,
        "the tab unchanged"
    );
    // { moves nothing either, so M-[ after it goes back to Work.
    client.type_text("{");
    client.type_text(&format!("{ALT}["));
    client.wait_for("Work's views", |s| header(s).contains("   Work"));
    // M-{ goes back through the views visited, Day's, then Work's, then
    // closes it on the tab it opened over, then goes to the first.
    client.type_text(&format!("{ALT}{{"));
    client.wait_for("Day's views", |s| header(s).contains("   Day"));
    client.type_text(&format!("{ALT}{{"));
    client.wait_for("Work's views", |s| header(s).contains("   Work"));
    client.type_text(&format!("{ALT}{{"));
    client.wait_for("mission control closed", closed);
    assert_eq!(tabs(&server.list())[1]["current"], true, "the second tab");
    client.type_text(&format!("{ALT}{{"));
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
}

/// Where a text first shows on the screen, from the top: its row and the
/// column it starts at.
fn find_on(screen: &[String], text: &str) -> Option<(usize, usize)> {
    screen.iter().enumerate().find_map(|(row, line)| {
        let at = line.find(text)?;
        Some((row, line[..at].chars().count()))
    })
}

/// A left click at a 0-based cell.
fn click(client: &mut TestClient, (row, col): (usize, usize)) {
    client.send(
        format!(
            "\x1b[<0;{};{}M\x1b[<0;{};{}m",
            col + 1,
            row + 1,
            col + 1,
            row + 1
        )
        .as_bytes(),
    );
}

/// The top-left corner of the box with this title.
fn corner_of(screen: &[String], title: &str) -> (usize, usize) {
    find_on(screen, &format!("╭─ {title} "))
        .unwrap_or_else(|| panic!("no box {title}:\n{}", screen.join("\n")))
}

/// Whether the box with this title is in the middle of the screen, as a
/// box opened by a key is.
fn in_the_middle(screen: &[String], title: &str) -> bool {
    let (top, left) = corner_of(screen, title);
    let bottom = (top..screen.len())
        .find(|&r| screen[r].chars().nth(left) == Some('╰'))
        .unwrap();
    top == (screen.len() - (bottom + 1 - top)) / 2
}

#[test]
fn a_menu_a_click_opens_opens_beside_the_click_and_enters_in_the_middle() {
    let (_server, mut client) = two_tabs(json!({}));
    let screen = open(&mut client);
    let card = find_on(&screen, "second").unwrap();
    // A click chooses the card, a second opens its menu, beside the click.
    click(&mut client, card);
    click(&mut client, card);
    let screen = client.wait_text("Go to tab 2");
    assert_eq!(corner_of(&screen, "2 second"), (card.0 + 1, card.1 + 1));
    // A question an option's click opens opens beside that option.
    let option = find_on(&screen, "Close the tab").unwrap();
    click(&mut client, option);
    let screen = client.wait_text("Close tab 2 second?");
    // With no room to its right, it opens to the option's left.
    let (top, left) = corner_of(&screen, "Are you sure?");
    let right = screen[top]
        .chars()
        .skip(left)
        .position(|c| c == '╮')
        .unwrap()
        + left;
    assert_eq!(
        (top, right),
        (option.0 + 1, option.1 - 1),
        "{}",
        screen.join("\n")
    );
    client.type_text("n");
    client.wait_for("the question closed", |s| {
        !s.iter().any(|l| l.contains("Close tab 2"))
    });
    // Enter opens it in the middle.
    client.type_text("\r");
    let screen = client.wait_text("Go to tab 2");
    assert!(in_the_middle(&screen, "2 second"), "{}", screen.join("\n"));
}

#[test]
fn with_boxes_near_click_off_a_clicks_menu_opens_in_the_middle() {
    let (_server, mut client) = two_tabs(json!({"boxes_near_click": false}));
    let screen = open(&mut client);
    let card = find_on(&screen, "second").unwrap();
    click(&mut client, card);
    click(&mut client, card);
    let screen = client.wait_text("Go to tab 2");
    assert!(in_the_middle(&screen, "2 second"), "{}", screen.join("\n"));
}

/// A right press at a cell of the screen, counted from 0.
fn right_press(client: &mut TestClient, col: usize, row: usize) {
    client.send(
        format!(
            "\x1b[<2;{};{}M\x1b[<2;{};{}m",
            col + 1,
            row + 1,
            col + 1,
            row + 1
        )
        .as_bytes(),
    );
}

/// The column, counted from 0, where `text` starts on a screen's line.
fn column_of(line: &str, text: &str) -> usize {
    let at = line.find(text).unwrap();
    switchboard::ui::draw::width(&line[..at])
}

// A right press on an item opens its menu, on a view's empty part the
// view's, and on the bar nothing; with a menu open, inside it nothing and
// outside it closes the menu without opening another.
#[test]
fn a_right_press_opens_an_items_menu_or_the_views() {
    let notes = common::TempDir::new("sbrc");
    std::fs::write(notes.path().join("Plan.md"), "# Plan\n").unwrap();
    let server = TestServer::start_with_env(&[("NOTES_DIR", notes.path().to_str().unwrap())]);
    std::fs::create_dir_all(&server.state).unwrap();
    std::fs::write(
        server.state.join("new-notes.json"),
        json!([{ "path": "Plan.md", "title": "Plan", "source": "Fathom",
            "at": switchboard::util::now_ms() }])
        .to_string(),
    )
    .unwrap();
    settings(&server, json!({ "new_notes": true }));
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    open(&mut client);
    client.type_text("]");
    let screen = client.wait_text("Plan  shown");
    let row = screen
        .iter()
        .rposition(|l| l.contains("Plan  shown"))
        .unwrap();
    let col = column_of(&screen[row], "Plan  shown") + 4;
    let menu_up = |s: &[String]| s.iter().any(|l| l.contains("Open it in Notes"));
    let view_menu_up = |s: &[String]| {
        s.iter()
            .any(|l| l.contains("Dismiss every notification shown"))
    };

    // On the notification: its menu, its options as Enter's.
    right_press(&mut client, col, row);
    let screen = client.wait_for("the notification's menu", menu_up);
    assert!(!view_menu_up(&screen), "{}", screen.join("\n"));
    // Inside the menu: nothing.
    let inside = screen
        .iter()
        .position(|l| l.contains("Open it in Notes"))
        .unwrap();
    right_press(
        &mut client,
        column_of(&screen[inside], "Open it in Notes"),
        inside,
    );
    std::thread::sleep(Duration::from_millis(300) * common::patience());
    let screen = client.screen();
    assert!(menu_up(&screen), "still open:\n{}", screen.join("\n"));
    // Outside it: closed, and nothing else opened.
    right_press(&mut client, 139, 30);
    client.wait_for("the menu closed", |s| !menu_up(s));
    std::thread::sleep(Duration::from_millis(300) * common::patience());
    let screen = client.screen();
    assert!(!view_menu_up(&screen), "{}", screen.join("\n"));
    assert!(
        !closed(&screen),
        "mission control open:\n{}",
        screen.join("\n")
    );

    // On the view's empty part: the view's menu.
    right_press(&mut client, col, 30);
    let screen = client.wait_for("the view's menu", view_menu_up);
    assert!(!menu_up(&screen), "{}", screen.join("\n"));
    client.type_text("\x1b");
    client.wait_for("the view's menu closed", |s| !view_menu_up(s));

    // On the bar: nothing.
    let screen = client.screen();
    let bar = screen
        .iter()
        .position(|l| l.contains("MISSION CONTROL"))
        .unwrap();
    right_press(&mut client, column_of(&screen[bar], "MISSION CONTROL"), bar);
    std::thread::sleep(Duration::from_millis(300) * common::patience());
    let screen = client.screen();
    assert!(
        !menu_up(&screen) && !view_menu_up(&screen) && !closed(&screen),
        "{}",
        screen.join("\n")
    );
}

// The Slack view with Slack's connector down offers to ask Claude Code
// again from its menu, and once it says Slack is connected the view leaves
// the connector's line and its menu the option.
#[test]
fn the_slack_views_menu_checks_a_connector_down_again() {
    let server = TestServer::start();
    std::fs::create_dir_all(&server.state).unwrap();
    let now = switchboard::util::now_ms();
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": now, "items": [{
            "kind": "dm", "channel": "D1", "from": "Lin", "user": "ULIN",
            "where": "DM", "text": "a message", "ts": format!("{}.000100", now / 1000 - 60),
        }] })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        server.state.join("connectors.json"),
        json!({ "Slack": { "ok": false, "at": now, "checked": now,
            "message": "Slack isn't connected in Claude: run /mcp in Claude and sign in to Slack." } })
        .to_string(),
    )
    .unwrap();
    let asked = server.home.join("mcp-gets");
    Claude::new()
        .answer(&format!(
            "case $CLAUDE_CALL in\n  'mcp get')\n    echo asked >> {}\n    printf 'claude.ai Slack:\\n  Status: \\342\\234\\224 Connected\\n' ;;\n  *) exit 1 ;;\nesac",
            common::stand_in::sh_quote(&asked.to_string_lossy()),
        ))
        .install(&server.home.join("bin"));
    settings(
        &server,
        json!({ "view_slack": true, "ux2": true, "slack_background": true, "photos": false }),
    );
    let mut client = server.attach(160, 36);
    client.wait_text("$");
    open(&mut client);
    client.type_text("vslack\r");
    client.wait_text("Slack isn't connected in Claude");
    client.type_text("m");
    pick(&mut client, "Check Slack again");
    client.wait_text("Slack is connected again.");
    client.wait_for("the connector's line gone", |s| {
        !s.join("\n").contains("isn't connected in Claude")
    });
    let status = std::fs::read_to_string(server.state.join("connectors-down")).unwrap();
    assert_eq!(status, "", "Slack is no longer down");
    assert_eq!(std::fs::read_to_string(&asked).unwrap().lines().count(), 1);
    client.type_text("m");
    let menu = client.wait_text("view actions");
    assert!(
        !menu.join("\n").contains("Check Slack again"),
        "{}",
        menu.join("\n")
    );
}

// A reconnect in /mcp writes Claude Code's config, which has the server ask
// about the connectors down at once rather than after fifteen minutes.
#[test]
fn a_change_to_claude_codes_config_asks_about_a_connector_down() {
    let server = TestServer::start();
    std::fs::create_dir_all(&server.state).unwrap();
    let now = switchboard::util::now_ms();
    std::fs::write(
        server.state.join("connectors.json"),
        json!({ "Fathom": { "ok": false, "at": now, "checked": now, "message": "m" } }).to_string(),
    )
    .unwrap();
    std::fs::write(server.state.join("connectors-down"), "Fathom\t1\n").unwrap();
    Claude::new()
        .answer(
            "case $CLAUDE_CALL in\n  'mcp get') printf 'claude.ai Fathom:\\n  Status: \\342\\234\\224 Connected\\n' ;;\n  *) exit 1 ;;\nesac",
        )
        .install(&server.home.join("bin"));
    settings(&server, json!({}));
    let config = server.home.join(".claude.json");
    std::fs::write(&config, "{}").unwrap();
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    // Changed until the server, past its first sight of it, asks.
    let deadline = common::deadline(Duration::from_secs(30));
    let mut second = 0;
    loop {
        second += 1;
        let file = std::fs::File::options().write(true).open(&config).unwrap();
        file.set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(second))
            .unwrap();
        if std::fs::read_to_string(server.state.join("connectors-down")).unwrap() == "" {
            break;
        }
        assert!(Instant::now() < deadline, "Fathom is still down");
        std::thread::sleep(Duration::from_millis(500) * common::patience());
    }
    client.wait_text("Fathom is connected again.");
}

/// With UX 2.0 the pane beside the Tickets board widens with the pane
/// resize key (C-M-S-h by default), the board narrowing, and keeps its
/// width once mission control is opened again, read from
/// `mission-side.json`.
#[test]
fn the_pane_beside_the_tickets_board_resizes_with_the_pane_keys_and_keeps_its_width() {
    use common::http::{Fake, Reply};
    let linear = Fake::linear("test-key", |request| {
        if !request.query().contains("assignedIssues") {
            return None;
        }
        let issue = json!({
            "identifier": "ABC-7", "title": "Hold the landing lock",
            "description": "What it is about.", "url": "https://linear.app/acme/issue/ABC-7",
            "priority": 2, "priorityLabel": "High", "updatedAt": jiff::Timestamp::now().to_string(),
            "completedAt": null, "canceledAt": null,
            "state": { "name": "Todo", "type": "unstarted" }, "project": null,
        });
        Some(Reply::json(
            200,
            json!({ "data": { "viewer": {
                "assignedIssues": { "nodes": [issue] }, "canceled": { "nodes": [] },
            } } }),
        ))
    });
    let server = TestServer::start_with_env(&[
        ("LINEAR_API_KEY", "test-key"),
        ("SWITCHBOARD_LINEAR_API", &linear.graphql_url()),
    ]);
    settings(
        &server,
        json!({"ux2": true, "linear": true, "photos": false}),
    );
    let mut client = server.attach(200, 40);
    client.wait_text("$");
    open(&mut client);
    client.type_text("vtickets\r");
    let edge = |s: &[String]| {
        s.iter()
            .find(|l| l.contains("What it is about."))
            .map(|l| column_of(l, "│ What it is about."))
    };
    let before = client.wait_for("the ticket's description beside the board", |s| {
        edge(s).is_some()
    });
    let before = edge(&before).unwrap();
    client.type_text("\x1b[104;8u");
    client.wait_for("the pane five columns wider", |s| {
        edge(s) == Some(before - 5)
    });
    let kept =
        std::fs::read_to_string(server.state.join("mission-side.json")).expect("the width kept");
    let kept: Value = serde_json::from_str(&kept).unwrap();
    assert!(kept["tickets"].as_u64().is_some(), "{kept}");
    client.type_text("q");
    client.wait_for("mission control closed", |s| edge(s).is_none());
    open(&mut client);
    client.type_text("vtickets\r");
    client.wait_for("the width kept on opening again", |s| {
        edge(s) == Some(before - 5)
    });
}

#[test]
fn on_a_wide_screen_one_view_is_capped_against_the_sidebar_and_its_side_takes_no_clicks() {
    let server = TestServer::start();
    settings(&server, json!({ "lone_pane_width": "165" }));
    let mut client = server.attach(365, 40);
    client.wait_text("$");
    wait_list(&server, "the settings read", |l| {
        l["sessions"][0]["sidebar"]["columns"] == 0
    });
    open(&mut client);
    // 165 columns from the left edge, a rule at its right, nothing beyond.
    let screen = client.wait_for("the view against its rule", |screen| {
        screen.iter().all(|row| {
            let cells: Vec<char> = row.chars().collect();
            cells.get(165) == Some(&'│') && cells[166..].iter().all(|c| *c == ' ')
        })
    });
    assert!(screen.iter().any(|l| l.contains("MISSION CONTROL")));
    // A click in the spare columns leaves mission control as it was, once
    // what the tab's card learns late (its program, read on a tick) has
    // drawn.
    client.wait_for("the card's program", |s| {
        s.iter()
            .any(|l| l.contains("idle for") && l.contains(" · "))
    });
    let before = steady(&mut client).join("\n");
    client.send(b"\x1b[<0;200;10M\x1b[<0;200;10m");
    client.send(b"\x1b[<0;340;10M\x1b[<0;340;10m");
    std::thread::sleep(Duration::from_millis(300) * common::patience());
    assert_eq!(client.screen().join("\n"), before);
}

#[test]
fn the_resize_keys_move_a_lone_views_right_edge_leaving_the_lone_width() {
    let server = TestServer::start();
    settings(&server, json!({ "lone_pane_width": "165" }));
    let mut client = server.attach(365, 40);
    client.wait_text("$");
    wait_list(&server, "the settings read", |l| {
        l["sessions"][0]["sidebar"]["columns"] == 0
    });
    open(&mut client);
    let rule_at =
        |screen: &[String], col: usize| screen.iter().any(|row| row.chars().nth(col) == Some('│'));
    client.wait_for("the view against its rule", |s| rule_at(s, 165));
    // C-M-S-l, with no pane beside the list, moves the view's edge.
    client.send(b"\x1b[108;8u");
    client.wait_for("the rule 5 columns further", |s| rule_at(s, 170));
    // Mission control's own width, the setting left as it was.
    wait_saved(&server, "the lone width", |saved| {
        saved["lone_pane_width"] == "165"
    });
    // Closed and opened again, it keeps its width.
    client.send(b"\x1b");
    client.wait_for("closed", closed);
    open(&mut client);
    client.wait_for("the view at its own width again", |s| rule_at(s, 170));
}

/// The settings file as written, once `pred` holds of it.
fn wait_saved(server: &TestServer, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    let file = server.home.join(".config/switchboard/settings.json");
    let deadline = common::deadline(Duration::from_secs(10));
    loop {
        let saved: Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap_or_default();
        if pred(&saved) {
            return saved;
        }
        assert!(Instant::now() < deadline, "{what} not written: {saved}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn with_ux2_the_slack_relay_view_sets_what_reaches_you() {
    let server = TestServer::start();
    settings(&server, json!({ "ux2": true, "view_relay": true }));
    let mut client = server.attach(164, 40);
    client.wait_text("$");
    client.wait_text("M-c Claude");
    client.type_text(&format!("{ALT}p"));
    client.wait_text("Actions");
    client.type_text("mission control: slack relay");
    client.wait_text("› Mission control: Slack relay");
    client.type_text("\r");
    let screen = client.wait_text("WHAT REACHES YOU");
    assert!(
        screen.iter().any(|l| l.contains("Not set up")),
        "{}",
        screen.join("\n")
    );
    // Down past the presets and two kinds to Finished turns; Enter's menu
    // offers its other choices.
    client.type_text("jjjjjj");
    client.wait_for("Finished turns chosen", |s| {
        s.iter()
            .filter_map(|l| l.split_once('│'))
            .any(|(_, view)| view.starts_with("▌") && view.contains("Finished turns"))
    });
    client.type_text("\r");
    pick(&mut client, "Don't send");
    wait_saved(&server, "Finished turns off", |s| {
        s["relay_send_finished"] == "off"
    });
    // Said in a box, which the next key closes and goes no further.
    client.wait_text("Finished turns: Don't send");
    client.type_text("\x1b");
    client.wait_for("the box closed", |s| {
        !s.iter().any(|l| l.contains("Finished turns: Don't send"))
    });
    client.wait_for("its changed chip", |s| {
        s.iter()
            .any(|l| l.contains("Finished turns") && l.contains("changed"))
    });
    // A preset applied writes its settings and lights.
    client.type_text("kkkkk");
    client.wait_for("the first preset chosen", |s| {
        s.iter()
            .any(|l| l.contains("what you must answer or fix notifies"))
    });
    client.type_text("\r");
    pick(&mut client, "Use Only what needs me");
    wait_saved(&server, "the preset", |s| {
        s["relay_send_milestones"] == "off" && s["relay_send_autopilot"] == "landings"
    });
    client.wait_text("Only what needs me: ");
    client.type_text("\x1b");
    client.wait_for("the box closed", |s| {
        !s.iter().any(|l| l.contains("Only what needs me: "))
    });
    // A click on a switch's part sets it at once.
    let screen = client.wait_for("milestones off", |s| {
        s.iter()
            .any(|l| l.contains("Commits, pushes and pull requests") && l.contains("Off"))
    });
    let row = screen
        .iter()
        .position(|l| l.contains("Commits, pushes and pull requests"))
        .unwrap();
    let col = screen[row]
        .chars()
        .collect::<Vec<_>>()
        .windows(6)
        .position(|w| w.iter().collect::<String>() == "Notify")
        .unwrap();
    client.send(
        format!(
            "\x1b[<0;{};{}M\x1b[<0;{};{}m",
            col + 2,
            row + 1,
            col + 2,
            row + 1
        )
        .as_bytes(),
    );
    wait_saved(&server, "a click's choice", |s| {
        s["relay_send_milestones"] == "notify"
    });
}

#[test]
fn c_w_j_and_k_move_between_the_sections_a_view_stacks() {
    let server = TestServer::start();
    settings(&server, json!({}));
    let mut client = server.attach(160, 40);
    client.wait_text("$");
    open(&mut client);
    client.type_text("vsett\r");
    client.wait_text("INTEGRATIONS");
    // While C-w waits, the footer names j and k for a view of sections.
    client.type_text("\x17");
    client.wait_text("next, last section");
    // C-w j: the next section's first option, its menu naming it.
    client.type_text("j\r");
    client.wait_text("─ Push notifications ─");
    client.type_text("\x1b");
    client.wait_for("the menu closed", |s| {
        !s.iter().any(|l| l.contains("─ Push notifications ─"))
    });
    // C-w k: back to the first section's first option.
    client.type_text("\x17k\r");
    client.wait_text("─ GitHub ─");
}

/// Runs a command in a tab's first pane, by its place, and waits for the
/// pane to show `done` at the end of its output.
fn run_in(server: &TestServer, tab: usize, command: &str, done: &str) {
    let pane = tabs(&server.list())[tab]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // Keys typed before the shell has started can be lost as it sets up
    // the terminal, so its prompt comes first.
    let deadline = common::deadline(Duration::from_secs(10));
    loop {
        let screen = server.ok(&["capture-pane", "-t", &pane]);
        if screen.contains('$') {
            break;
        }
        assert!(Instant::now() < deadline, "no prompt: {screen}");
        std::thread::sleep(Duration::from_millis(50));
    }
    server.ok(&["send-keys", "-t", &pane, "-l", command]);
    server.ok(&["send-keys", "-t", &pane, "Enter"]);
    let deadline = common::deadline(Duration::from_secs(10));
    loop {
        let screen = server.ok(&["capture-pane", "-t", &pane]);
        if screen.lines().any(|l| l.trim() == done) {
            break;
        }
        assert!(Instant::now() < deadline, "{done} not shown: {screen}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn m_j_finds_a_tab_by_what_its_panes_show() {
    let (server, mut client) = two_tabs(json!({}));
    // Printed by halves, so the command typed doesn't hold the word.
    run_in(
        &server,
        0,
        "printf 'a zeb%sword here\\n' ra; seq 1 100",
        "100",
    );
    client.type_text(&format!("{ALT}j"));
    client.wait_text("Work map · open tabs");
    client.type_text("zebraword");
    // The first tab found by its pane, the line under its row.
    let screen = client.wait_text("a zebraword here");
    assert!(
        !screen.iter().any(|l| l.contains("second")),
        "only the tab that shows it: {}",
        screen.join("\n")
    );
    // Its menu goes to the match: the pane scrolled back in copy mode.
    client.type_text("\r");
    pick(&mut client, "Go to the match");
    client.wait_for("mission control closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    client.wait_for("the match in sight, scrolled back", |s| {
        s.iter().any(|l| l.contains("a zebraword here"))
            && s.iter().any(|l| {
                l.split('[').skip(1).any(|r| {
                    r.split_once(']')
                        .is_some_and(|(n, _)| n.parse::<u32>().is_ok_and(|n| n > 0))
                })
            })
    });
}

#[test]
fn m_j_ranks_a_tab_found_by_name_above_one_found_by_its_panes() {
    let (server, mut client) = two_tabs(json!({}));
    run_in(&server, 0, "printf 'sec%s\\n' ond", "second");
    client.type_text(&format!("{ALT}j"));
    client.wait_text("Work map · open tabs");
    client.type_text("second");
    // Both tabs show before the filter is taken up too, so wait for it.
    let screen = client.wait_for("both tabs found", |s| {
        s.iter().any(|l| l.contains("/second"))
            && s.iter().any(|l| l.contains("tab 1"))
            && s.iter().any(|l| l.contains("tab 2"))
    });
    let named = screen.iter().position(|l| l.contains("tab 2"));
    let by_pane = screen.iter().position(|l| l.contains("tab 1"));
    assert!(
        named.is_some() && by_pane.is_some() && named < by_pane,
        "{}",
        screen.join("\n")
    );
}

#[test]
fn m_j_matches_names_alone_with_the_panes_search_off() {
    let (server, mut client) = two_tabs(json!({ "tab_search_panes": false }));
    run_in(&server, 0, "printf 'zeb%sword\\n' ra", "zebraword");
    client.type_text(&format!("{ALT}j"));
    client.wait_text("Work map · open tabs");
    client.type_text("zebraword");
    client.wait_for("no row", |s| {
        !s.iter().any(|l| l.contains("tab 1") || l.contains("tab 2"))
    });
    std::thread::sleep(Duration::from_millis(500) * common::patience());
    let screen = client.screen();
    assert!(
        !screen.iter().any(|l| l.contains("tab 1")),
        "{}",
        screen.join("\n")
    );
}

#[test]
fn m_j_reads_only_the_lines_set_back_in_each_pane() {
    let (server, mut client) = two_tabs(json!({ "tab_search_lines": "200" }));
    run_in(
        &server,
        0,
        "printf 'old%sword\\n' zebra; seq 1 500; printf 'new%sword\\n' zebra",
        "newzebraword",
    );
    client.type_text(&format!("{ALT}j"));
    client.wait_text("Work map · open tabs");
    client.type_text("newzebra");
    client.wait_text("newzebraword");
    // The word 500 lines back is past the 200 read.
    for _ in 0.."newzebra".len() {
        client.type_text("\x7f");
    }
    client.type_text("oldzebra");
    client.wait_text("oldzebra");
    std::thread::sleep(Duration::from_millis(500) * common::patience());
    let screen = client.screen();
    assert!(
        !screen.iter().any(|l| l.contains("oldzebraword")),
        "{}",
        screen.join("\n")
    );
}

/// Two tabs and a third named `zebraword`, the second in front again, the
/// first tab's pane showing "a zebraword here" and the second's the word
/// too; M-F asked for it, its Tabs answered.
fn search_everything_for_tabs(extra: Value) -> (TestServer, TestClient, Vec<String>) {
    let mut extra = extra;
    for off in ["linear", "github", "search_slack"] {
        extra[off] = json!(false);
    }
    let (server, mut client) = two_tabs(extra);
    server.ok(&["new-tab", "-n", "zebraword"]);
    wait_list(&server, "the third tab in front", |l| {
        tabs(l).len() == 3 && tabs(l)[2]["current"] == true
    });
    let second = tabs(&server.list())[1]["id"].as_str().unwrap().to_string();
    server.ok(&["select-tab", "-t", &second]);
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
    // Printed by halves, so the command typed doesn't hold the word.
    run_in(
        &server,
        0,
        "printf 'a zeb%sword here\\n' ra; seq 1 100",
        "100",
    );
    run_in(
        &server,
        1,
        "printf 'in the zeb%sword tab\\n' ra",
        "in the zebraword tab",
    );
    client.type_text(&format!("{ALT}F"));
    client.wait_text("Search everything");
    client.type_text("zebraword\r");
    let screen = client.wait_for("the tabs found", |s| {
        s.iter().any(|l| l.contains(" found"))
            && !s.iter().any(|l| l.contains("searching…"))
            && s.iter().any(|l| l.contains("by name"))
    });
    (server, client, screen)
}

#[test]
fn m_f_finds_tabs_by_name_and_by_what_their_panes_show() {
    let (server, mut client, screen) = search_everything_for_tabs(json!({}));
    let named = screen.iter().position(|l| l.contains("by name"));
    let by_pane = screen.iter().position(|l| l.contains("a zebraword here"));
    assert!(
        named.is_some() && by_pane.is_some() && named < by_pane,
        "the named tab first, then the one its pane shows it: {}",
        screen.join("\n")
    );
    let here = screen
        .iter()
        .position(|l| l.contains("2 second · this tab"));
    assert!(
        here.is_some() && by_pane < here,
        "the tab it was opened from last, marked: {}",
        screen.join("\n")
    );
    // Enter goes to the tab chosen.
    client.type_text("\r");
    wait_list(&server, "the named tab in front", |l| {
        tabs(l)[2]["current"] == true
    });
}

#[test]
fn m_f_brings_up_a_tab_at_its_match_on_ctrl_o() {
    let (server, mut client, _) = search_everything_for_tabs(json!({}));
    // Down to the chips, the first result, the tab found by its pane, and Ctrl-O.
    client.type_text("\x1b[B\x1b[B\x1b[B\x0f");
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    client.wait_for("the match in sight, scrolled back", |s| {
        s.iter().any(|l| l.contains("a zebraword here"))
            && s.iter().any(|l| {
                l.split('[').skip(1).any(|r| {
                    r.split_once(']')
                        .is_some_and(|(n, _)| n.parse::<u32>().is_ok_and(|n| n > 0))
                })
            })
    });
}

/// One tab whose Claude's transcript holds `zebraword` in a prompt, older
/// than anything its pane shows, and a tool's output holding `yakword`,
/// with the hooks having named it; M-F asked for `query` from that tab.
fn search_this_tabs_conversation(
    extra: Value,
    query: &str,
) -> (TestServer, TestClient, Vec<String>) {
    let mut extra = extra;
    for off in ["linear", "github", "search_slack"] {
        extra[off] = json!(false);
    }
    let server = TestServer::start();
    settings(&server, extra);
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    wait_list(&server, "the settings read", |l| {
        l["sessions"][0]["sidebar"]["columns"] == 0
    });
    let pane = tabs(&server.list())[0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let now = jiff::Timestamp::now().as_second();
    let at = |ago: i64| jiff::Timestamp::from_second(now - ago).unwrap().to_string();
    let lines = [
        json!({"type": "user", "timestamp": at(3 * 3600), "message": {"content": "is the zebraword set?"}}),
        json!({"type": "assistant", "timestamp": at(3 * 3600 - 5), "message": {"content": [
            {"type": "text", "text": "Looking."},
            {"type": "tool_use", "id": "c1", "name": "Bash", "input": {"command": "ls"}}]}}),
        json!({"type": "user", "timestamp": at(3 * 3600 - 4), "message": {"content": [
            {"type": "tool_result", "tool_use_id": "c1", "content": "yakword"}]}}),
    ];
    let transcript = server.home.join("t.jsonl");
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(&transcript, text).unwrap();
    let payload = json!({"session_id": "s1", "transcript_path": transcript, "cwd": server.home});
    hook(&server, &pane, "SessionStart", payload.clone());
    hook(&server, &pane, "UserPromptSubmit", payload);
    client.type_text(&format!("{ALT}F"));
    client.wait_text("Search everything");
    client.type_text(&format!("{query}\r"));
    let screen = client.wait_for("the sources answered", |s| {
        s.iter().any(|l| l.contains(" found")) && !s.iter().any(|l| l.contains("searching…"))
    });
    (server, client, screen)
}

#[test]
fn m_f_finds_this_tab_by_what_was_said_in_its_conversation() {
    let (_server, mut client, _) = search_this_tabs_conversation(json!({}), "zebraword");
    // Its row, and the preview beside the results saying when it was said
    // and showing the line.
    let screen = client.wait_for("this tab, found in its conversation", |s| {
        s.iter().any(|l| l.contains("1 home · this tab"))
            && s.iter()
                .any(|l| l.contains("Said in its Claude's conversation 3h 0m ago"))
            && s.iter().any(|l| l.contains("is the zebraword set?"))
    });
    assert!(
        !screen.iter().any(|l| l.contains("No results")),
        "{}",
        screen.join("\n")
    );
    // Enter opens the tab's history at the moment it was said.
    client.type_text("\r");
    client.wait_for("the history at that moment", |s| {
        s.iter().any(|l| l.contains("h/l move in time")) && s.iter().any(|l| l.contains("viewing "))
    });
}

#[test]
fn m_f_leaves_out_tool_output_and_conversations_with_their_setting_off() {
    let (_server, _client, screen) = search_this_tabs_conversation(json!({}), "yakword");
    assert!(
        !screen.iter().any(|l| l.contains("this tab")),
        "a tool's output isn't the conversation: {}",
        screen.join("\n")
    );
    let (_server, _client, screen) =
        search_this_tabs_conversation(json!({"tab_search_conversation": false}), "zebraword");
    assert!(
        !screen.iter().any(|l| l.contains("said 3h")),
        "{}",
        screen.join("\n")
    );
}

/// `note: zebraword` asks the notes alone: Linear's stand-in, running and
/// asked by the same search without the prefix, hears nothing of it.
#[test]
fn m_f_scoped_to_notes_asks_no_other_source() {
    use common::http::{Fake, Reply};
    let linear = Fake::linear("test-key", |request| {
        request.text().contains("searchIssues").then(|| {
            Reply::json(
                200,
                json!({ "data": { "searchIssues": { "nodes": [
                    { "identifier": "ABC-9", "title": "Zebraword in Linear",
                      "url": "https://linear.app/acme/issue/ABC-9", "state": { "name": "Todo" } }
                ] } } }),
            )
        })
    });
    let server = TestServer::start_with_env(&[
        ("LINEAR_API_KEY", "test-key"),
        ("SWITCHBOARD_LINEAR_API", &linear.graphql_url()),
    ]);
    settings(
        &server,
        json!({ "linear": true, "github": false, "search_slack": false }),
    );
    let notes = server.home.join("Notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("Zebra plan.md"),
        "# Zebra plan\n\nthe zebraword here\n",
    )
    .unwrap();
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    let searched = || {
        linear
            .seen()
            .iter()
            .filter(|r| r.text().contains("searchIssues"))
            .count()
    };
    client.type_text(&format!("{ALT}F"));
    client.wait_text("Search everything");
    client.type_text("NOTE: zebraword\r");
    let screen = client.wait_for("the note found", |s| {
        s.iter().any(|l| l.contains("Search notes: zebraword"))
            && s.iter().any(|l| l.contains("Zebra plan"))
    });
    assert!(
        !screen.iter().any(|l| l.contains("searching…")),
        "{}",
        screen.join("\n")
    );
    std::thread::sleep(Duration::from_millis(500) * common::patience());
    assert_eq!(searched(), 0, "Linear isn't asked");
    // Unscoped, the same search asks Linear too.
    client.type_text("\x1b");
    client.wait_for("the box closed", |s| {
        !s.iter().any(|l| l.contains("Zebra plan"))
    });
    client.type_text(&format!("{ALT}F"));
    client.wait_text("Search everything");
    client.type_text("zebraword\r");
    client.wait_text("Zebraword in Linear");
    assert_eq!(searched(), 1);
}

/// A server whose Linear stand-in finds "Zebraword in Linear", with GitHub
/// and Slack off, and how many searches Linear was sent.
fn search_with_linear() -> (TestServer, TestClient, common::http::Fake) {
    use common::http::{Fake, Reply};
    let linear = Fake::linear("test-key", |request| {
        request.text().contains("searchIssues").then(|| {
            Reply::json(
                200,
                json!({ "data": { "searchIssues": { "nodes": [
                    { "identifier": "ABC-9", "title": "Zebraword in Linear",
                      "url": "https://linear.app/acme/issue/ABC-9", "state": { "name": "Todo" } }
                ] } } }),
            )
        })
    });
    let server = TestServer::start_with_env(&[
        ("LINEAR_API_KEY", "test-key"),
        ("SWITCHBOARD_LINEAR_API", &linear.graphql_url()),
    ]);
    settings(
        &server,
        json!({ "linear": true, "github": false, "search_slack": false }),
    );
    let mut client = server.attach(140, 36);
    client.wait_text("$");
    wait_list(&server, "the settings read", |l| {
        l["sessions"][0]["sidebar"]["columns"] == 0
    });
    (server, client, linear)
}

fn linear_searches(linear: &common::http::Fake) -> usize {
    linear
        .seen()
        .iter()
        .filter(|r| r.text().contains("searchIssues"))
        .count()
}

/// A click on Linear's chip drops its rows and leaves it unasked by the
/// next search, and the chips stay as they were for the next M-F.
#[test]
fn m_f_a_chip_clicked_off_leaves_its_source_unasked_and_is_kept() {
    let (server, mut client, linear) = search_with_linear();
    client.type_text(&format!("{ALT}F"));
    client.wait_text("Search everything");
    client.type_text("zebraword\r");
    let screen = client.wait_text("Zebraword in Linear");
    assert_eq!(linear_searches(&linear), 1);
    // The chips' line holds All, then a chip per source Settings has on.
    let (row, col) = find_on(&screen, "Linear 1").expect("Linear's chip with its count");
    assert!(
        screen[row].contains("All") && screen[row].contains("Notes"),
        "{}",
        screen.join("\n")
    );
    click(&mut client, (row, col + 1));
    client.wait_for("Linear's rows gone", |s| {
        s.iter().any(|l| l.contains("Search tabs, notes:"))
            && !s.iter().any(|l| l.contains("Zebraword in Linear"))
    });
    // Searched again, Linear is sent nothing.
    client.type_text("s\r");
    client.wait_text("zebrawords");
    client.wait_for("the search done", |s| {
        s.iter().any(|l| l.contains(" found"))
    });
    std::thread::sleep(Duration::from_millis(500) * common::patience());
    assert_eq!(linear_searches(&linear), 1, "Linear isn't asked");
    // The next M-F opens with Linear's chip off.
    client.type_text("\x1b");
    client.wait_for("the box closed", |s| {
        !s.iter().any(|l| l.contains("Search everything"))
    });
    client.type_text(&format!("{ALT}F"));
    client.wait_text("Search tabs, notes:");
    let kept = std::fs::read_to_string(server.state.join("search.json")).unwrap();
    assert!(kept.contains("linear"), "{kept}");
}

/// Tab reaches each chip from the field, Space turns the one it is on off,
/// and All turns every one back on.
#[test]
fn m_f_tab_reaches_each_chip() {
    let (_server, mut client, _linear) = search_with_linear();
    client.type_text(&format!("{ALT}F"));
    client.wait_text("Search everything");
    // Field, All, Tabs, Notes, Linear.
    client.type_text("\t\t\t\t ");
    client.wait_text("Search tabs, notes:");
    // Notes, off too.
    client.type_text("\x1b[Z ");
    client.wait_text("Search tabs:");
    // Tabs, then All.
    client.type_text("\x1b[Z\x1b[Z ");
    client.wait_for("every source lit by All", |s| {
        s.iter().any(|l| l.contains("Search: ")) && !s.iter().any(|l| l.contains("Search tabs"))
    });
}

/// M-p's "Search notes" opens M-F's box with the notes' prefix typed.
#[test]
fn search_notes_opens_the_box_with_its_prefix_typed() {
    let (server, mut client) = two_tabs(json!({}));
    server.ok(&["press", "search-notes"]);
    client.wait_text("Search notes: ");
}

#[test]
fn m_f_matches_tab_names_alone_with_the_panes_search_off() {
    let (_server, _client, screen) =
        search_everything_for_tabs(json!({ "tab_search_panes": false }));
    assert!(
        !screen.iter().any(|l| l.contains("a zebraword here")),
        "{}",
        screen.join("\n")
    );
}

#[test]
fn back_forward_and_m_tab_step_through_mission_control_views_as_well_as_tabs() {
    let (server, mut client) = two_tabs(json!({}));
    let first = first_tab_id(&server);
    server.ok(&["select-tab", "-t", &first]);
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    // From the first tab to the Routines view, then to the second tab.
    server.ok(&["press", "mission-routines"]);
    client.wait_text("No routines yet");
    client.type_text(&format!("{ALT}2"));
    client.wait_for("mission control closed", closed);
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
    // Back opens Routines over the first tab, back again closes it there.
    client.type_text(&format!("{ALT}{{"));
    client.wait_text("No routines yet");
    wait_list(&server, "the first tab under it", |l| {
        tabs(l)[0]["current"] == true
    });
    client.type_text(&format!("{ALT}{{"));
    client.wait_for("mission control closed", closed);
    // Forward twice reaches the second tab through Routines.
    client.type_text(&format!("{ALT}}}"));
    client.wait_text("No routines yet");
    client.type_text(&format!("{ALT}}}"));
    client.wait_for("mission control closed", closed);
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
    // M-Tab goes to the place before, Routines, and again back to the tab.
    client.type_text(&format!("{ALT}\t"));
    client.wait_text("No routines yet");
    client.type_text(&format!("{ALT}\t"));
    client.wait_for("mission control closed", closed);
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
}

/// A view left with Esc is the place before the tab it returns to.
#[test]
fn m_tab_returns_to_a_view_left_with_esc() {
    let (server, mut client) = two_tabs(json!({}));
    let first = first_tab_id(&server);
    server.ok(&["select-tab", "-t", &first]);
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    server.ok(&["press", "mission-routines"]);
    client.wait_text("No routines yet");
    client.type_text("\x1b");
    client.wait_for("mission control closed", closed);
    client.type_text(&format!("{ALT}\t"));
    client.wait_text("No routines yet");
    wait_list(&server, "the first tab under it", |l| {
        tabs(l)[0]["current"] == true
    });
    client.type_text(&format!("{ALT}\t"));
    client.wait_for("mission control closed", closed);
    wait_list(&server, "still the first tab", |l| {
        tabs(l)[0]["current"] == true
    });
}

/// With back and forward off, M-Tab goes to the last tab, past a view
/// left with Esc.
#[test]
fn m_tab_goes_to_the_last_tab_past_a_view_with_back_and_forward_off() {
    let (server, mut client) = two_tabs(json!({ "tab_back_forward": false }));
    let first = first_tab_id(&server);
    server.ok(&["select-tab", "-t", &first]);
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    server.ok(&["press", "mission-routines"]);
    client.wait_text("No routines yet");
    client.type_text("\x1b");
    client.wait_for("mission control closed", closed);
    client.type_text(&format!("{ALT}\t"));
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
    let screen = client.screen();
    assert!(closed(&screen), "no view opened:\n{}", screen.join("\n"));
}

#[test]
fn back_passes_over_a_view_turned_off_since() {
    let (server, mut client) = two_tabs(json!({}));
    let first = first_tab_id(&server);
    server.ok(&["select-tab", "-t", &first]);
    wait_list(&server, "the first tab in front", |l| {
        tabs(l)[0]["current"] == true
    });
    server.ok(&["press", "mission-notifications"]);
    client.wait_text("No notifications");
    client.type_text(&format!("{ALT}2"));
    client.wait_for("mission control closed", closed);
    wait_list(&server, "the second tab in front", |l| {
        tabs(l)[1]["current"] == true
    });
    // The view turned off, and the sidebar on to see the settings read.
    settings(
        &server,
        json!({"sidebar_notifications": false, "sidebar": true}),
    );
    wait_list(&server, "the settings read", |l| {
        l["sessions"][0]["sidebar"]["columns"] != 0
    });
    client.type_text(&format!("{ALT}{{"));
    wait_list(&server, "back to the first tab", |l| {
        tabs(l)[0]["current"] == true
    });
    let screen = client.screen();
    assert!(closed(&screen), "no view opened:\n{}", screen.join("\n"));
}

// No letter keys on the footers (boards 18, 22, 28, 29): on a screen too
// narrow for the Reminders lists beside the month, the view's menu shows
// them, as `i` does unshown.
#[test]
fn the_reminders_menu_shows_the_lists_a_narrow_screen_keeps_behind_i() {
    let server = TestServer::start();
    settings(&server, json!({ "ux2": true, "reminders": true }));
    let mut client = server.attach(120, 36);
    client.wait_text("$");
    open(&mut client);
    client.type_text("vreminders\r");
    let screen = client.wait_text("H L month");
    let all = screen.join("\n");
    assert!(!all.contains("this month and later"), "{all}");
    client.type_text("m");
    pick(&mut client, "Show this month and later");
    client.wait_for("the menu closed", |s| {
        !s.join("\n").contains("Several views")
    });
    client.type_text("m");
    pick(&mut client, "Back to the chosen day");
}

// The Slack reply box types as every box does: a line, S-Enter (as kitty's
// keyboard protocol sends it, and as `ESC CR` from a terminal without it),
// a dash and more words all land in it, and Enter sends them as typed:
// with "Slack messages go out as" at sent at once, after a yes, through
// the connector's send tool alone.
#[test]
fn the_slack_reply_box_takes_s_enter_then_a_dash_and_the_words_after() {
    let server = TestServer::start();
    let calls = slack_connector(
        &server,
        "mcp__claude_ai_Slack__slack_send_message",
        "Message sent",
        false,
    );
    std::fs::create_dir_all(&server.state).unwrap();
    let secs = switchboard::util::now_ms() / 1000;
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": secs * 1000, "items": [
            { "kind": "channel", "channel": "C1", "from": "Ben", "where": "#dev",
                "text": "a deploy question", "ts": format!("{}.000100", secs - 500),
                "link": "https://slack.example/b" },
        ] })
        .to_string(),
    )
    .unwrap();
    settings(
        &server,
        json!({ "view_slack": true, "ux2": true, "slack_threads": false,
            "slack_background": false, "photos": false, "slack_reply": true,
            "slack_send_as": "now" }),
    );
    let mut client = server.attach(204, 40);
    slack_reply_box_open(&mut client);
    client.wait_text("as you, from your Slack sign-in");
    client.type_text("On it");
    client.send(b"\x1b[13;2u");
    client.type_text("- the menu");
    client.send(b"\x1b\r");
    client.type_text("- the refresh");
    let screen = client.wait_text("- the refresh");
    let at = |text: &str| screen.iter().position(|l| l.contains(text));
    let (first, second, third) = (at("On it"), at("- the menu"), at("- the refresh"));
    assert!(
        first.is_some() && first < second && second < third,
        "each on a line of its own: {}",
        screen.join("\n")
    );
    // Enter on the text sends, which asks before anything goes, with the
    // reply's first line.
    client.type_text("\r");
    let asked = client.wait_text("\"On it\"").join("\n");
    assert!(asked.contains("[Y/n]"), "{asked}");
    assert!(!calls.exists(), "nothing goes before the yes");
    client.type_text("\r");
    client.wait_text("Sent to #dev.");
    let asked = std::fs::read_to_string(&calls).unwrap();
    assert!(
        asked.contains("ToolSearch,mcp__claude_ai_Slack__slack_send_message "),
        "{asked}"
    );
    assert!(!asked.contains("slack_send_message_draft"), "{asked}");
}

/// A Claude whose Slack connector answers `tool` with `result` (an error
/// of the tool's with `is_error`), in `server`'s home; gives the file it
/// writes each prompt's arguments in, a line a call.
fn slack_connector(
    server: &TestServer,
    tool: &str,
    result: &str,
    is_error: bool,
) -> std::path::PathBuf {
    let used = json!({ "type": "assistant", "message": { "content": [{
        "type": "tool_use", "id": "t1", "name": tool, "input": {},
    }] } });
    let said = json!({ "type": "user", "message": { "content": [{
        "type": "tool_result", "tool_use_id": "t1", "is_error": is_error,
        "content": [{ "type": "text", "text": result }],
    }] } });
    let calls = server.home.join("connector-calls");
    Claude::new()
        .answer(&format!(
            "case $CLAUDE_CALL in\n  print)\n    printf '%s\\n' \"$*\" >> {}\n    printf '%s\\n' {} {} ;;\nesac",
            common::stand_in::sh_quote(&calls.to_string_lossy()),
            common::stand_in::sh_quote(&used.to_string()),
            common::stand_in::sh_quote(&said.to_string()),
        ))
        .install(&server.home.join("bin"));
    calls
}

/// Opens the reply box on the thread of the Slack view's one message, from
/// Ben. The server reads its settings again a moment after they are
/// written, so the message's menu is opened again until it offers replies.
fn slack_reply_box_open(client: &mut TestClient) {
    client.wait_text("$");
    open(client);
    client.type_text("vslack\r");
    client.wait_text("a deploy question");
    let deadline = common::deadline(Duration::from_secs(10));
    loop {
        client.type_text("\r");
        let menu = client.wait_text("Dismiss every message shown");
        if menu.iter().any(|l| l.contains("Reply in Ben's thread")) {
            break;
        }
        assert!(Instant::now() < deadline, "the menu offers no replies");
        client.type_text("\x1b");
        client.wait_for("the menu closed", |s| {
            !s.iter().any(|l| l.contains("Dismiss every message shown"))
        });
        std::thread::sleep(Duration::from_millis(100));
    }
    pick(client, "Reply in Ben's thread");
}

/// The Slack view with one message in #dev from Ben, replies on, and the
/// reply box open on its thread with "On it" typed; gives the message's ts.
fn slack_reply_typed(server: &TestServer, client: &mut TestClient) -> String {
    slack_reply_box_open(client);
    client.wait_text("as a draft in Slack, for you to send there");
    client.type_text("On it");
    client.wait_text("On it");
    let kept = std::fs::read_to_string(server.state.join("slack.json")).unwrap();
    let kept: Value = serde_json::from_str(&kept).unwrap();
    kept["items"][0]["ts"].as_str().unwrap().to_string()
}

/// A server whose Slack view holds one message in #dev from Ben, with
/// replies on and how they go out left at its default, a draft.
fn slack_with_a_message() -> TestServer {
    let server = TestServer::start();
    std::fs::create_dir_all(&server.state).unwrap();
    let secs = switchboard::util::now_ms() / 1000;
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": secs * 1000, "items": [
            { "kind": "channel", "channel": "C1", "from": "Ben", "where": "#dev",
                "text": "a deploy question", "ts": format!("{}.000100", secs - 500),
                "link": "https://acme.slack.com/archives/C1/p1" },
        ] })
        .to_string(),
    )
    .unwrap();
    settings(
        &server,
        json!({ "view_slack": true, "ux2": true, "slack_threads": false,
            "slack_background": false, "photos": false, "slack_reply": true }),
    );
    server
}

/// The lines of the test's desktop log that opened `url`.
fn opened(server: &TestServer, url: &str) -> usize {
    std::fs::read_to_string(server.home.join("desktop-log"))
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains(url))
        .count()
}

// By default a reply leaves as a draft in Slack: Send makes it with the
// connector's draft tool alone, asking nothing since nothing is sent,
// opens the thread in Slack, and leaves a box saying where the draft is,
// which offers to go there again.
#[test]
fn a_slack_reply_is_left_as_a_draft_in_slack_and_its_thread_opened() {
    let server = slack_with_a_message();
    let calls = slack_connector(
        &server,
        "mcp__claude_ai_Slack__slack_send_message_draft",
        r#"{"channel_link":"https://acme.slack.com/archives/C1","draft_id":"Dr1"}"#,
        false,
    );
    let mut client = server.attach(204, 40);
    let ts = slack_reply_typed(&server, &mut client);
    client.type_text("\r");
    let screen = client.wait_text("Take me to the draft").join("\n");
    assert!(screen.contains("Your reply is a draft in #dev"), "{screen}");
    assert!(screen.contains("Send it from Slack."), "{screen}");
    assert!(!screen.contains("[Y/n]"), "{screen}");
    assert!(
        screen.lines().any(|l| l.contains('›') && l.contains("OK")),
        "OK is chosen as it opens: {screen}"
    );
    let asked = std::fs::read_to_string(&calls).unwrap();
    assert_eq!(asked.lines().count(), 1, "{asked}");
    assert!(
        asked.contains("ToolSearch,mcp__claude_ai_Slack__slack_send_message_draft "),
        "the draft tool alone: {asked}"
    );
    assert!(
        asked.contains(&format!("\"thread_ts\":\"{ts}\"")),
        "{asked}"
    );
    assert!(asked.contains("\"channel_id\":\"C1\""), "{asked}");
    let thread = format!(
        "https://acme.slack.com/archives/C1/p{}?thread_ts={ts}&cid=C1",
        ts.replace('.', "")
    );
    assert_eq!(opened(&server, &thread), 1, "the thread, opened once");
    pick(&mut client, "Take me to the draft");
    let deadline = common::deadline(Duration::from_secs(10));
    while opened(&server, &thread) < 2 {
        assert!(Instant::now() < deadline, "the draft's link opened again");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        std::fs::read_to_string(&calls).unwrap().lines().count(),
        1,
        "no second draft"
    );
}

// Slack keeps one draft a conversation: with one unsent there, the box
// says so and offers to go to it, and the reply box comes back with what
// was typed.
#[test]
fn a_draft_already_in_the_conversation_is_said_and_the_text_kept() {
    let server = slack_with_a_message();
    slack_connector(
        &server,
        "mcp__claude_ai_Slack__slack_send_message_draft",
        "attached_draft_exists",
        true,
    );
    let mut client = server.attach(204, 40);
    slack_reply_typed(&server, &mut client);
    client.type_text("\r");
    let screen = client.wait_text("Take me to it").join("\n");
    assert!(
        screen.contains("Slack already has an unsent draft in this conversation"),
        "{screen}"
    );
    assert_eq!(opened(&server, "acme.slack.com"), 0, "nothing opened");
    client.type_text("\r");
    let screen = client
        .wait_text("as a draft in Slack, for you to send there")
        .join("\n");
    assert!(screen.contains("On it"), "the text kept: {screen}");
}

/// A Slack of the test's own for your own sign-in's calls, its token
/// `xoxp-test`, and a server that reaches it with that token, signed in.
fn slack_signed_in(
    answer: impl Fn(&common::http::Request) -> Option<common::http::Reply> + Send + Sync + 'static,
    signed_in: bool,
) -> (common::http::Fake, TestServer) {
    let slack = common::http::Fake::start(
        common::http::Service::Slack {
            token: "xoxp-test".into(),
        },
        answer,
    );
    let server = TestServer::start_with_env(&[
        ("SWITCHBOARD_SLACK_API", &format!("{}/api", slack.url)),
        ("SWITCHBOARD_SLACK_USER_TOKEN", "xoxp-test"),
    ]);
    std::fs::create_dir_all(&server.state).unwrap();
    if signed_in {
        std::fs::write(
            server.state.join("slack-user.json"),
            json!({ "team": "T1", "url": "https://example.slack.com/", "user": "U1" }).to_string(),
        )
        .unwrap();
    }
    (slack, server)
}

/// The calls a stand-in Slack was asked, each as its method and API method.
fn slack_calls(slack: &common::http::Fake) -> Vec<String> {
    slack
        .seen()
        .iter()
        .map(|r| format!("{} {}", r.method, r.path().trim_start_matches("/api/")))
        .collect()
}

// With your own Slack sign-in, the Slack view lists your reminders that
// are due under Reminders, a completed one left out, and "Mark it
// complete" marks one complete in Slack only after a yes.
#[test]
fn your_slack_reminders_are_listed_and_marked_complete_after_a_yes() {
    let secs = switchboard::util::now_ms() / 1000;
    let (slack, server) = slack_signed_in(
        move |request| {
            let body = match request.path() {
                "/api/reminders.list" => json!({ "ok": true, "reminders": [
                    { "id": "Rm1", "text": "water the plants", "time": secs - 600, "complete_ts": 0 },
                    { "id": "Rm2", "text": "already done", "time": secs - 900, "complete_ts": secs - 60 },
                ] }),
                "/api/reminders.complete" => json!({ "ok": true }),
                _ => return None,
            };
            Some(common::http::Reply::json(200, body))
        },
        true,
    );
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": secs * 1000, "items": [
            { "kind": "mention", "channel": "C1", "where": "#dev", "from": "Ada", "user": "UAda",
              "text": "mention words", "ts": format!("{}.000100", secs - 300) },
        ] })
        .to_string(),
    )
    .unwrap();
    settings(
        &server,
        json!({ "view_slack": true, "ux2": true, "slack_threads": false, "slack_reminders": true, "photos": false }),
    );
    let mut client = server.attach(204, 40);
    client.wait_text("$");
    open(&mut client);
    client.wait_for("the views grouped", |s| {
        s.iter().any(|l| l.starts_with(" WORK ")) && s.iter().any(|l| l.starts_with(" SYSTEM "))
    });
    client.type_text("vslack\r");
    // Going to the view reads the reminders, which get a section.
    client.wait_text("◷ Reminders");
    client.type_text("h");
    client.type_text("j");
    let screen = client.wait_text("water the plants");
    let all = screen.join("\n");
    assert!(!all.contains("already done"), "a completed one: {all}");
    assert!(!all.contains("need your Slack sign-in"), "signed in: {all}");
    // Its menu: completing it asks first, and a yes tells Slack.
    client.type_text("l");
    client.type_text("\r");
    pick(&mut client, "Mark it complete");
    client.wait_text("Mark this reminder complete in Slack?");
    assert!(
        !slack_calls(&slack)
            .iter()
            .any(|c| c.ends_with("reminders.complete")),
        "nothing before the yes"
    );
    client.type_text("\r");
    let deadline = common::deadline(Duration::from_secs(10));
    while !slack_calls(&slack)
        .iter()
        .any(|c| c == "POST reminders.complete")
    {
        assert!(
            Instant::now() < deadline,
            "asked: {:?}",
            slack_calls(&slack)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let asked = slack.seen();
    let completed = asked
        .iter()
        .find(|r| r.path() == "/api/reminders.complete")
        .unwrap();
    assert_eq!(completed.json(), json!({ "reminder": "Rm1" }));
    // Off the view at once.
    client.wait_for("the reminder gone", |s| {
        !s.join("\n").contains("water the plants")
    });
}

// Enter on a message in the Slack view marks its conversation read in
// Slack up to it, only with "Mark what you open read in Slack" on, and
// only past where Slack has it read.
#[test]
fn opening_a_slack_message_marks_it_read_in_slack_only_with_the_setting_on() {
    let secs = switchboard::util::now_ms() / 1000;
    let ts = format!("{}.000100", secs - 300);
    let (slack, server) = slack_signed_in(
        |request| {
            let body = match request.path() {
                "/api/conversations.info" => {
                    json!({ "ok": true, "channel": { "id": "D1", "last_read": "1.000000" } })
                }
                "/api/conversations.mark" => json!({ "ok": true }),
                _ => return None,
            };
            Some(common::http::Reply::json(200, body))
        },
        true,
    );
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": secs * 1000, "items": [
            { "kind": "dm", "channel": "D1", "where": "Bo", "from": "Bo", "user": "UBo",
              "text": "dm words", "ts": ts, "link": "https://slack.example/m" },
        ] })
        .to_string(),
    )
    .unwrap();
    let with = |mark: bool| {
        settings(
            &server,
            json!({ "view_slack": true, "ux2": true, "slack_threads": false, "slack_mark_read": mark, "photos": false }),
        );
    };
    with(false);
    let mut client = server.attach(204, 40);
    client.wait_text("$");
    open(&mut client);
    client.wait_for("the views grouped", |s| {
        s.iter().any(|l| l.starts_with(" WORK ")) && s.iter().any(|l| l.starts_with(" SYSTEM "))
    });
    client.type_text("vslack\r");
    client.wait_text("dm words");
    // Off: its menu opens, and Slack hears nothing.
    client.type_text("\r");
    client.wait_text("Open it in Slack");
    client.type_text("\x1b");
    std::thread::sleep(Duration::from_millis(500) * common::patience());
    assert_eq!(slack_calls(&slack), Vec::<String>::new());
    // On: opening it again marks it read up to it.
    with(true);
    let deadline = common::deadline(Duration::from_secs(15));
    loop {
        client.type_text("\r");
        client.wait_text("Open it in Slack");
        client.type_text("\x1b");
        std::thread::sleep(Duration::from_millis(300) * common::patience());
        if slack_calls(&slack)
            .iter()
            .any(|c| c == "POST conversations.mark")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "asked: {:?}",
            slack_calls(&slack)
        );
    }
    assert_eq!(
        slack_calls(&slack),
        ["GET conversations.info", "POST conversations.mark"],
        "once, however often it is opened"
    );
    let asked = slack.seen();
    assert_eq!(asked[0].target, "/api/conversations.info?channel=D1");
    assert_eq!(asked[1].json(), json!({ "channel": "D1", "ts": ts }));
}

// With no Slack sign-in of your own, the Slack view says how to make one
// on a line under its header, lists its messages as before, and asks Slack
// nothing.
#[test]
fn without_a_slack_sign_in_the_slack_view_says_how_to_make_one_and_asks_nothing() {
    let secs = switchboard::util::now_ms() / 1000;
    let (slack, server) = slack_signed_in(|_| None, false);
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": secs * 1000, "items": [
            { "kind": "dm", "channel": "D1", "where": "Bo", "from": "Bo", "user": "UBo",
              "text": "dm words", "ts": format!("{}.000100", secs - 300) },
        ] })
        .to_string(),
    )
    .unwrap();
    settings(
        &server,
        json!({ "view_slack": true, "ux2": true, "slack_threads": false, "slack_reminders": true,
            "slack_mark_read": true, "photos": false }),
    );
    let mut client = server.attach(204, 40);
    client.wait_text("$");
    open(&mut client);
    client.wait_for("the views grouped", |s| {
        s.iter().any(|l| l.starts_with(" WORK ")) && s.iter().any(|l| l.starts_with(" SYSTEM "))
    });
    client.type_text("vslack\r");
    let screen = client.wait_text(
        "Slack reminders and read marks need your Slack sign-in: switchboard setup slack-user",
    );
    assert!(screen.join("\n").contains("dm words"));
    client.type_text("\r");
    client.wait_text("Dismiss it, until its thread gets a reply");
    std::thread::sleep(Duration::from_millis(500) * common::patience());
    assert_eq!(slack_calls(&slack), Vec::<String>::new());
}

/// A server whose Slack look holds one thread, its first message with a
/// video and a picture kept on this computer, and the Slack view on; a
/// `qlmanage` that does nothing, so no thumbnail is asked of the real one.
fn slack_thread_with_media() -> (TestServer, Vec<PathBuf>) {
    let server = TestServer::start();
    let bin = server.home.join("bin");
    std::fs::write(bin.join("qlmanage"), "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(bin.join("qlmanage"), PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::create_dir_all(&server.state).unwrap();
    let secs = switchboard::util::now_ms() / 1000;
    let root = format!("{}.000100", secs - 900);
    let reply = format!("{}.000100", secs - 100);
    std::fs::write(
        server.state.join("slack.json"),
        json!({ "checked": secs * 1000, "items": [
            { "kind": "channel", "channel": "C1", "where": "#dev", "from": "Ben", "user": "UBen",
              "text": "see the recording", "ts": reply, "thread": root,
              "link": "https://slack.example/m" },
        ] })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        server.state.join("slack-threads.json"),
        json!({ format!("C1:{root}"): {
            "messages": [
                { "user": "UAna", "from": "Ana", "text": "the question", "ts": root, "files": [
                    { "id": "F1VIDEO", "name": "clip.mp4", "mimetype": "video/mp4" },
                    { "id": "F2SHOT", "name": "shot.png", "mimetype": "image/png" },
                ] },
                { "user": "UBen", "from": "Ben", "text": "see the recording", "ts": reply },
            ],
            "replies": 1,
            "read": secs * 1000,
        } })
        .to_string(),
    )
    .unwrap();
    let files = server.home.join(".cache/switchboard/slack-files");
    std::fs::create_dir_all(&files).unwrap();
    let kept = vec![files.join("F1VIDEO.mp4"), files.join("F2SHOT.png")];
    std::fs::write(&kept[0], b"\0\0\0\x18ftypmp42").unwrap();
    std::fs::write(&kept[1], b"\x89PNG\r\n\x1a\n").unwrap();
    settings(
        &server,
        json!({ "view_slack": true, "ux2": true, "slack_threads": true,
            "slack_background": false, "photos": false }),
    );
    (server, kept)
}

/// The kept Slack files the desktop was asked to show, in order: by Quick
/// Look started through `open` on macOS, by the desktop's opener on Linux.
fn shown_files(server: &TestServer) -> Vec<String> {
    std::fs::read_to_string(server.home.join("desktop-log"))
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains("slack-files"))
        .map(str::to_string)
        .collect()
}

fn wait_shown(server: &TestServer, count: usize) -> Vec<String> {
    let until = common::deadline(Duration::from_secs(10));
    loop {
        let shown = shown_files(server);
        if shown.len() >= count {
            return shown;
        }
        assert!(Instant::now() < until, "{count} files shown: {shown:?}");
        std::thread::sleep(Duration::from_millis(50) * common::patience());
    }
}

/// Whether `line` asks the desktop to show `file` and nothing else: on
/// macOS Quick Look, brought to the front by `open` (a `qlmanage -p` the
/// server starts itself never puts its window on the screen).
fn shows(line: &str, file: &Path) -> bool {
    let file = file.display();
    if cfg!(target_os = "macos") {
        let quick_look =
            "/System/Library/Frameworks/QuickLook.framework/Versions/A/Resources/qlmanage.app";
        line == format!("open -n -a {quick_look} --args -p {file}")
    } else {
        line.ends_with(&format!(" {file}"))
    }
}

// A Slack message's "Play its video or show its picture" plays the video
// of its thread, and picked again shows the picture after it, each said in
// mission control's footer.
#[test]
fn a_slack_messages_menu_plays_its_video_then_shows_its_picture() {
    let (server, kept) = slack_thread_with_media();
    let mut client = server.attach(204, 40);
    client.wait_text("$");
    open(&mut client);
    client.type_text("vslack\r");
    client.wait_text("the question");
    client.type_text("\r");
    pick(&mut client, "Play its video or show its picture");
    client.wait_text("Playing clip.mp4 (1 of 2)");
    let shown = wait_shown(&server, 1);
    assert!(shows(&shown[0], &kept[0]), "{shown:?}");
    client.type_text("\r");
    pick(&mut client, "Play its video or show its picture");
    client.wait_text("Showing shot.png (2 of 2)");
    let shown = wait_shown(&server, 2);
    assert!(shows(&shown[1], &kept[1]), "{shown:?}");
}

// The same choice in the message's notification's menu plays it too.
#[test]
fn a_slack_notifications_menu_plays_its_video() {
    let (server, kept) = slack_thread_with_media();
    let mut client = server.attach(204, 40);
    client.wait_text("$");
    open(&mut client);
    client.type_text("vnotifications\r");
    client.wait_for("the Notifications view in front", |s| {
        header(s).trim_start().starts_with("Notifications") && s.iter().any(|l| l.contains("Ben"))
    });
    // The notification may be listed before its thread is read, and a menu
    // keeps the choices it opened with, so it is opened again until the
    // thread's files are in it.
    let until = common::deadline(Duration::from_secs(10));
    loop {
        client.type_text("\r");
        let screen = client.wait_text("Dismiss it");
        if screen.iter().any(|l| l.contains("Play its video")) {
            break;
        }
        assert!(Instant::now() < until, "{}", screen.join("\n"));
        client.type_text("\x1b");
        client.wait_for("the menu closed", |s| {
            !s.iter().any(|l| l.contains("Dismiss it"))
        });
        std::thread::sleep(Duration::from_millis(100) * common::patience());
    }
    pick(&mut client, "Play its video or show its picture");
    client.wait_text("Playing clip.mp4 (1 of 2)");
    let shown = wait_shown(&server, 1);
    assert!(shows(&shown[0], &kept[0]), "{shown:?}");
}

// A file the desktop couldn't show says so in a box, and in the server's
// log with the file's ID, so the choice never does nothing.
#[test]
fn a_slack_file_that_cant_be_shown_says_why() {
    let (server, _kept) = slack_thread_with_media();
    let bin = server.home.join("bin");
    // Under WSL a path Windows can't be given is never handed to Explorer,
    // whose exit says nothing.
    for tool in ["open", "xdg-open", "gio", "wslview", "wslpath"] {
        std::fs::write(bin.join(tool), "#!/bin/sh\necho 'no-display' >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(bin.join(tool), PermissionsExt::from_mode(0o755)).unwrap();
    }
    let mut client = server.attach(204, 40);
    client.wait_text("$");
    open(&mut client);
    client.type_text("vslack\r");
    client.wait_text("the question");
    client.type_text("\r");
    pick(&mut client, "Play its video or show its picture");
    client.wait_text("Couldn't show clip.mp4");
    let log = server.log();
    assert!(
        log.contains("Slack file F1VIDEO") && log.contains("showing clip.mp4"),
        "{log}"
    );
}

// With UX 2.0 the Tabs view follows the sidebar's repository filter: the
// head names it, the other repository's tabs fold under one Other repos
// line, closed, that Enter opens, and "Every repository" in the view's
// menu clears the filter; M-j's picker lists every open tab all along.
#[test]
fn with_ux2_the_tabs_view_follows_the_sidebar_s_repository_filter() {
    let server = TestServer::start();
    settings(&server, json!({"ux2": true}));
    let alpha = server.dir.path().join("alpha");
    let beta = server.dir.path().join("beta");
    for dir in [&alpha, &beta] {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q"]);
        git(dir, &["commit", "-q", "--allow-empty", "-m", "start"]);
    }
    let alpha = alpha.canonicalize().unwrap();
    let beta = beta.canonicalize().unwrap();
    let mut client = server.attach(160, 40);
    client.wait_text("$");
    server.ok(&[
        "new-tab",
        "-d",
        "-c",
        &alpha.to_string_lossy(),
        "-n",
        "ant-work",
    ]);
    server.ok(&[
        "new-tab",
        "-d",
        "-c",
        &beta.to_string_lossy(),
        "-n",
        "bee-work",
    ]);
    wait_list(&server, "three tabs", |l| tabs(l).len() == 3);
    server.ok(&["sidebar-repo", &alpha.to_string_lossy()]);
    wait_list(&server, "the filter set", |l| {
        l["sessions"][0]["sidebar"]["repo"] == json!(alpha.to_string_lossy())
    });

    open(&mut client);
    let screen = client.wait_for("the Tabs view filtered", |s| {
        s.iter().any(|l| l.contains("Filtered to alpha"))
            && s.iter().any(|l| l.contains("OTHER REPOS"))
            && s.iter().any(|l| l.contains("ant-work"))
    });
    assert!(
        !screen.iter().any(|l| l.contains("bee-work")),
        "{screen:#?}"
    );

    // Its label is the last place; Enter there opens it.
    client.type_text("G");
    client.type_text("\r");
    client.wait_text("bee-work");

    // M-j's picker lists every open tab, the filter on.
    client.type_text(&format!("{ALT}j"));
    client.wait_for("M-j's picker of every tab", |s| {
        s.iter().any(|l| l.contains("ant-work")) && s.iter().any(|l| l.contains("bee-work"))
    });
    client.type_text("\x1b");
    client.wait_for("the picker closed", |s| {
        !s.iter().any(|l| l.contains("MISSION CONTROL"))
    });

    // "Every repository" in the view's menu clears the filter.
    open(&mut client);
    client.wait_text("Filtered to alpha");
    client.type_text("m");
    pick(&mut client, "Every repository");
    wait_list(&server, "the filter cleared", |l| {
        l["sessions"][0]["sidebar"]["repo"].is_null()
    });
    client.wait_for("every tab shown", |s| {
        !s.iter()
            .any(|l| l.contains("Filtered to") || l.contains("OTHER REPOS"))
            && s.iter().any(|l| l.contains("bee-work"))
    });
}
