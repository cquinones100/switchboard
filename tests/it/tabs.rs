//! What a tab is called and the color it wears, as someone sees it: a pane
//! that moves into a git repository gives its tab the repository's name and
//! branch, drawn in the project's accent color; a name given to the tab
//! wins; and with GitHub and Linear on, the branch's PR number and ticket
//! title arrive from the background, here from a stand-in for each service
//! on 127.0.0.1.

use crate::common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use common::TestServer;
use serde_json::{Value, json};
use switchboard::tabs::{self, Shade};

/// Settings for the tests: no desktop notifications, and these.
fn settings(server: &TestServer, extra: Value) {
    let mut all = json!({"notifications": false});
    for (k, v) in extra.as_object().unwrap() {
        all[k] = v.clone();
    }
    common::write_settings(&server.home, all);
}

/// Runs git in `dir` with an identity of its own.
fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// A repository at `dir` with one commit, on `branch`.
fn repo(dir: &Path, branch: &str) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    git(
        dir,
        &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
    );
    git(dir, &["commit", "-q", "--allow-empty", "-m", "first"]);
}

fn first_tab(server: &TestServer) -> Value {
    server.list()["sessions"][0]["tabs"][0].clone()
}

/// Waits for the first tab's entry in `list` to satisfy `pred`.
fn wait_tab(server: &TestServer, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    let deadline = common::deadline(std::time::Duration::from_secs(15));
    loop {
        let tab = first_tab(server);
        if pred(&tab) {
            return tab;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {tab}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_tab_in_a_repository_wears_its_name_and_color_and_a_given_name_wins() {
    let server = TestServer::start();
    settings(
        &server,
        json!({"github": false, "linear": false, "tab_colors_apart": false}),
    );
    let work = server.dir.path().join("shop");
    repo(&work, "feature-x");

    let mut client = server.attach(100, 30);
    client.wait_text("$");
    client.type_text(&format!("cd {}\r", work.display()));
    let screen = client.wait_for("the repository's name in the tab", |s| {
        s[29].ends_with("1:shop:feature-x")
    });

    // Drawn in the project's color, the same one accent-color.sh gave.
    let expected = tabs::accent_color_of(&work, Shade::Soft);
    assert_eq!(
        expected,
        tabs::accent_color("shop | branch:feature-x", Shade::Soft)
    );
    let col = screen[29].find("shop:feature-x").unwrap();
    assert_eq!(
        client.fg_rgb(29, col),
        Some((expected.r, expected.g, expected.b))
    );
    let tab = first_tab(&server);
    assert_eq!(tab["label"], "shop:feature-x");
    assert_eq!(tab["color"], expected.hex());
    assert_eq!(tab["path"], work.to_string_lossy().as_ref());
    assert_eq!(tab["pr"], Value::Null);
    let id = tab["id"].as_str().unwrap().to_string();
    assert_eq!(
        server.ok(&["show", "-t", &id, "@tab_label"]).trim(),
        "shop:feature-x"
    );
    // A pane named for a tab's option means its tab, as /walkthrough's
    // record.js asks for the label of the pane it runs in.
    let pane = tab["panes"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(
        server.ok(&["show", "-t", &pane, "@tab_label"]).trim(),
        "shop:feature-x"
    );

    // A name given to the tab wins, still in the project's color.
    server.ok(&["rename-tab", "-t", &id, "My work"]);
    let screen = client.wait_for("the given name", |s| s[29].ends_with("1:My work"));
    let col = screen[29].find("My work").unwrap();
    assert_eq!(
        client.fg_rgb(29, col),
        Some((expected.r, expected.g, expected.b))
    );
    assert_eq!(first_tab(&server)["label"], "My work");

    // Taken away, the directory names it again; `@tab_name`, as M-p's
    // "Rename the tab" set it, names it too.
    server.ok(&["rename-tab", "-t", &id, ""]);
    client.wait_for("the directory's name again", |s| {
        s[29].ends_with("1:shop:feature-x")
    });
    server.ok(&["set", "-t", &id, "@tab_name", "Shaping: things"]);
    client.wait_for("the @tab_name", |s| s[29].ends_with("1:Shaping: things"));

    // A branch of its own's name follows the checkout, and the trunk isn't named.
    server.ok(&["set", "-t", &id, "-u", "@tab_name"]);
    git(&work, &["checkout", "-q", "-b", "main"]);
    client.wait_for("the trunk's tab", |s| s[29].ends_with("1:shop"));
    let main = tabs::accent_color("shop | branch:main", Shade::Soft);
    wait_tab(&server, "the trunk's color", |t| t["color"] == main.hex());

    // With tab colors vivid, the stronger color from before.
    settings(
        &server,
        json!({"github": false, "linear": false, "tab_colors_apart": false, "tab_colors": "vivid"}),
    );
    let vivid = tabs::accent_color("shop | branch:main", Shade::Vivid);
    assert_ne!(vivid, main);
    wait_tab(&server, "the vivid color", |t| t["color"] == vivid.hex());

    // With project colors off, every tab is gray.
    settings(
        &server,
        json!({"github": false, "linear": false, "tab_colors_apart": false, "accent_colors": false}),
    );
    wait_tab(&server, "the gray", |t| t["color"] == "8a8a8a");
}

#[test]
fn a_tab_keeps_its_name_after_its_worktree_is_removed_until_it_moves() {
    let server = TestServer::start();
    settings(
        &server,
        json!({"github": false, "linear": false, "tab_colors_apart": false}),
    );
    let main = server.dir.path().join("app");
    repo(&main, "main");
    let worktree = main.join(".claude/worktrees/fix-login");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "fix-login",
            worktree.to_str().unwrap(),
        ],
    );

    let mut client = server.attach(100, 30);
    client.wait_text("$");
    client.type_text(&format!("cd {}\r", worktree.display()));
    let named = wait_tab(&server, "the worktree's name", |t| {
        t["path"] == worktree.to_string_lossy().as_ref()
    });
    assert!(
        named["label"].as_str().unwrap().contains("fix-login"),
        "{named}"
    );

    // Removed under the pane, as queue-and-clean's cleanup does before the
    // pane restarts in the main clone: the tab reads as it did, not as the
    // main clone it climbs to.
    git(
        &main,
        &["worktree", "remove", "--force", worktree.to_str().unwrap()],
    );
    std::thread::sleep(Duration::from_millis(2_500) * common::patience());
    let kept = first_tab(&server);
    assert_eq!(kept["label"], named["label"]);
    assert_eq!(kept["color"], named["color"]);
    let left = worktree.parent().unwrap();
    assert_eq!(kept["path"], left.to_string_lossy().as_ref());

    // In a directory that exists again, the name follows it.
    client.type_text(&format!("cd {}\r", main.display()));
    wait_tab(&server, "the main clone's name", |t| t["label"] == "app");
}

#[test]
fn m_shift_t_shows_the_tabs_whole_title_and_its_branch_and_copies_it() {
    let server = TestServer::start();
    settings(
        &server,
        json!({"github": false, "linear": false, "tab_colors_apart": false}),
    );
    let work = server.dir.path().join("shop");
    repo(&work, "feature-x");
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    client.type_text(&format!("cd {}\r", work.display()));
    client.wait_for("the repository's name in the tab", |s| {
        s[29].ends_with("1:shop:feature-x")
    });
    let id = first_tab(&server)["id"].as_str().unwrap().to_string();
    let long = "A tab's name long enough that the status line and the sidebar would cut it short somewhere";
    server.ok(&["set", "-t", &id, "@tab_name", long]);
    client.type_text("\x1bT");
    let screen = client.wait_text("Copy the title");
    let text = screen.join("\n");
    assert!(text.contains("Tab 1"), "{text}");
    assert!(text.contains("Branch     feature-x"), "{text}");
    assert!(text.contains("somewhere"), "the title whole: {text}");
    client.type_text("y");
    client.wait_text("Copied the tab's title");
    client.type_text("\r");
    client.wait_for("the box closed", |s| {
        !s.iter().any(|l| l.contains("Copied the tab's title"))
    });

    // A note item's tab: named with the item cut short, its whole first
    // line its full title, which the box shows while the name is that.
    let item = "Copy Claude's last reply into the clipboard, with a key of its own and a tip";
    let short = tabs::title::short_name(item);
    server.ok(&["set", "-t", &id, "@tab_name", &short]);
    server.ok(&["set", "-t", &id, "@tab_title", item]);
    client.wait_for("the short name", |s| s[29].contains("clipboard…"));
    client.type_text("\x1bT");
    let text = client.wait_text("Copy the title").join("\n");
    assert!(text.contains("a tip"), "the item whole: {text}");
    client.type_text("\x1b");
    client.wait_for("the box closed", |s| {
        !s.iter().any(|l| l.contains("Copy the title"))
    });
    // Renamed since, even to the item's first words, it shows its name.
    server.ok(&["set", "-t", &id, "@tab_name", "Copy Claude's last reply"]);
    client.wait_for("the new name", |s| {
        s[29].ends_with("Copy Claude's last reply")
    });
    client.type_text("\x1bT");
    let text = client.wait_text("Copy the title").join("\n");
    assert!(!text.contains("a tip"), "{text}");
}

#[cfg(feature = "net")]
mod from_the_services {
    use super::*;
    use crate::common::http::{Fake, Reply};

    /// Stand-ins for GitHub and Linear (tests/common/http.rs), each as
    /// strict as the real one about how it is asked: the branch's PR from
    /// GitHub's GraphQL, its ticket from Linear's.
    fn fake_services() -> (Fake, Fake) {
        let github = Fake::github("test-token", |request| {
            (request.path() == "/graphql" && request.text().contains("pullRequests")).then(|| {
                Reply::json(
                    200,
                    json!({"data": {"repository": {"pullRequests": {"nodes": [{
                        "number": 12, "url": "https://github.com/acme/widgets/pull/12",
                        "title": "Fix the login", "state": "OPEN", "isDraft": false,
                        "baseRefName": "main", "headRefName": "eng-42-fix-login",
                        "headRepositoryOwner": {"login": "acme"}
                    }]}}}}),
                )
            })
        });
        let linear = Fake::linear("test-key", |_| {
            Some(Reply::json(
                200,
                json!({"data": {"issue": {
                    "id": "i1", "identifier": "ENG-42", "title": "Fix the login",
                    "url": "https://linear.app/acme/issue/ENG-42",
                    "state": {"name": "In Progress"}, "assignee": null
                }}}),
            ))
        });
        (github, linear)
    }

    #[test]
    fn the_pr_number_and_ticket_title_arrive_from_the_background() {
        let (github, linear) = fake_services();
        let server = TestServer::start_with_env(&[
            ("GH_TOKEN", "test-token"),
            ("LINEAR_API_KEY", "test-key"),
            ("SWITCHBOARD_GITHUB_GRAPHQL", &github.graphql_url()),
            ("SWITCHBOARD_GITHUB_API", &github.url),
            ("SWITCHBOARD_LINEAR_API", &linear.graphql_url()),
        ]);
        settings(&server, json!({}));
        let work = server.dir.path().join("widgets");
        repo(&work, "eng-42-fix-login");
        git(
            &work,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/widgets.git",
            ],
        );

        let mut client = server.attach(100, 30);
        client.wait_text("$");
        client.type_text(&format!("cd {}\r", work.display()));
        client.wait_for("the PR and the ticket's title", |s| {
            s[29].ends_with("1:#12 ENG-42: Fix the login")
        });
        let tab = first_tab(&server);
        assert_eq!(tab["label"], "#12 ENG-42: Fix the login");
        assert_eq!(tab["pr"], 12);

        // A name of its own keeps the PR in front.
        let id = tab["id"].as_str().unwrap().to_string();
        server.ok(&["rename-tab", "-t", &id, "Login"]);
        client.wait_for("the name after the PR", |s| s[29].ends_with("1:#12 Login"));
    }
}

#[test]
fn the_tabs_command_names_each_panes_claude_and_its_address() {
    let server = TestServer::start();
    settings(
        &server,
        json!({"github": false, "linear": false, "tab_colors_apart": false}),
    );
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    server.ok(&["new-tab", "-d", "-n", "plain"]);
    // The first tab's shell stands in for Claude: a session file by its pid,
    // as Claude Code writes one, with the name `ListAgents` shows.
    let pid = first_tab(&server)["panes"][0]["pid"].as_u64().unwrap();
    let sessions = server.home.join(".claude/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let socket = format!("/tmp/cc-socks/{pid}.sock");
    std::fs::write(
        sessions.join(format!("{pid}.json")),
        json!({"pid": pid, "sessionId": "s-1", "entrypoint": "cli", "name": "work-81",
               "nameSource": "derived", "messagingSocketPath": socket})
        .to_string(),
    )
    .unwrap();
    wait_tab(&server, "the session read", |t| {
        t["panes"][0]["claude"]["session"] == "s-1"
    });

    let list = server.list();
    let lines = switchboard::cmd::tabs::lines(&list);
    let rows: Vec<Vec<&str>> = lines.lines().map(|l| l.split('\t').collect()).collect();
    assert_eq!(rows.len(), 2, "{lines}");
    assert_eq!(
        rows[0][6..],
        ["s-1", &pid.to_string(), "work-81", &format!("uds:{socket}")],
        "{lines}"
    );
    assert_eq!(rows[1][6..], ["", "", "", ""], "no Claude, blank: {lines}");

    let objects = switchboard::cmd::tabs::objects(&list);
    assert_eq!(objects[0]["session"], "s-1");
    assert_eq!(objects[0]["address"], format!("uds:{socket}"));
    assert_eq!(objects[1]["agent"], Value::Null);
}

/// A tab's `@tab_dir` (the review tab's PR worktree, src/cmd/work.rs) is the
/// directory it is about while its pane works elsewhere: the tab is named
/// from it and `list` gives it as the pane's, while a split still starts in
/// the pane's own directory, and handing the pane over (`respawn`, as /wt
/// does) forgets it.
#[test]
fn a_tab_with_a_tab_dir_is_named_from_it_and_forgets_it_when_handed_over() {
    let server = TestServer::start();
    settings(
        &server,
        json!({"github": false, "linear": false, "tab_colors_apart": false}),
    );
    let main = server.dir.path().join("app");
    repo(&main, "main");
    let worktree = main.join(".claude/worktrees/fix-login");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "fix-login",
            worktree.to_str().unwrap(),
        ],
    );
    let (main, worktree) = (
        main.canonicalize().unwrap(),
        worktree.canonicalize().unwrap(),
    );

    let mut client = server.attach(100, 30);
    client.wait_text("$");
    client.type_text(&format!("cd {}\r", main.display()));
    let tab = wait_tab(&server, "the main clone's name", |t| t["label"] == "app");
    let id = tab["id"].as_str().unwrap().to_string();
    let pane = tab["panes"][0]["id"].as_str().unwrap().to_string();

    server.ok(&["set", "-t", &id, "@tab_dir", worktree.to_str().unwrap()]);
    let named = wait_tab(&server, "the worktree's name", |t| {
        t["label"].as_str().is_some_and(|l| l.contains("fix-login"))
    });
    assert_eq!(named["path"], worktree.to_string_lossy().as_ref());
    assert_eq!(
        named["panes"][0]["cwd"],
        worktree.to_string_lossy().as_ref()
    );

    // A program started from the pane starts where the pane is.
    let split = server.ok(&[
        "split",
        "-t",
        &pane,
        "-D",
        "-d",
        "--",
        "sh",
        "-c",
        "pwd; sleep 30",
    ]);
    let split = split.trim().to_string();
    let deadline = common::deadline(Duration::from_secs(15));
    loop {
        let screen = server.ok(&["capture-pane", "-t", &split]);
        if screen.lines().any(|l| l.trim() == main.to_string_lossy()) {
            break;
        }
        assert!(Instant::now() < deadline, "the split's pwd: {screen}");
        std::thread::sleep(Duration::from_millis(100));
    }

    // Handed over, the tab is about its pane's directory again.
    server.ok(&["respawn", "-t", &pane, "-c", main.to_str().unwrap(), "sh"]);
    wait_tab(&server, "the main clone's name again", |t| {
        t["label"] == "app"
    });
    assert_eq!(
        server.ok(&["show", "-t", &id, "@tab_dir"]).trim(),
        "",
        "forgotten"
    );
}

/// Every tab's number, the sessions and their tabs in order.
fn numbers(server: &TestServer) -> Vec<i64> {
    server.list()["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["tabs"].as_array().unwrap().clone())
        .map(|t| t["index"].as_i64().unwrap())
        .collect()
}

/// The current tab's number in the first session.
fn current_number(server: &TestServer) -> i64 {
    server.list()["sessions"][0]["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["current"] == true)
        .unwrap()["index"]
        .as_i64()
        .unwrap()
}

#[test]
fn a_tab_keeps_its_number_while_open_and_a_new_one_takes_the_smallest_free() {
    let mut server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let second = server.ok(&["new-tab", "-d"]);
    let second = second.split_whitespace().next().unwrap().to_string();
    server.ok(&["new-tab", "-d"]);
    server.ok(&["new-tab", "-d"]);
    assert_eq!(
        numbers(&server),
        [1, 4, 3, 2],
        "each opened after the first"
    );
    server.ok(&["kill-tab", "-t", &second]);
    assert_eq!(numbers(&server), [1, 4, 3], "the others keep theirs");

    // M-3 goes to the tab numbered 3, wherever it is; M-2, numbering none,
    // stays.
    server.ok(&["press", "select-tab-3"]);
    client.wait_for("tab 3 in front", |_| current_number(&server) == 3);
    server.ok(&["press", "select-tab-2"]);
    assert_eq!(current_number(&server), 3, "no tab is numbered 2");

    // The next tab takes the smallest number free, 2.
    server.ok(&["new-tab", "-d"]);
    assert_eq!(numbers(&server), [1, 4, 3, 2]);
    // A tab in a second session takes a number no tab in the first has.
    let mut other = common::TestClient::attach(&server.socket(), Some("work"), 80, 24);
    other.wait_text("$");
    other.detach();
    assert_eq!(numbers(&server), [1, 4, 3, 2, 5]);

    // An upgrade and a restart keep every number, the gap left by a closed
    // tab too.
    let fourth = server.list()["sessions"][0]["tabs"][1]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["kill-tab", "-t", &fourth]);
    let (code, out, err) = server.run(&["upgrade"]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(numbers(&server), [1, 3, 2, 5], "after an upgrade");
    server.ok(&["save"]);
    drop(client);
    server.restart();
    assert_eq!(numbers(&server), [1, 3, 2, 5], "after a restore");
    server.ok(&["new-tab", "-d"]);
    assert!(numbers(&server).contains(&4), "4 is free for the next");
}

/// Every tab's id, `@tab_hue` and color, the sessions and their tabs in
/// order, once every tab has a hue of its own.
fn hues(server: &TestServer) -> Vec<(String, i64, String)> {
    let deadline = common::deadline(Duration::from_secs(15));
    loop {
        let tabs: Vec<Value> = server.list()["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|s| s["tabs"].as_array().unwrap().clone())
            .collect();
        let hues: Vec<(String, i64, String)> = tabs
            .iter()
            .filter_map(|t| {
                let hue = t["options"]["@tab_hue"].as_str()?.parse().ok()?;
                let color = t["color"].as_str()?.to_string();
                Some((t["id"].as_str()?.to_string(), hue, color))
            })
            .collect();
        if hues.len() == tabs.len() {
            return hues;
        }
        assert!(
            Instant::now() < deadline,
            "a tab never took a hue: {tabs:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// How far apart two tab colors (hex) are drawn around the hue circle, in
/// degrees: the hue of the color itself, since the shell's steps draw two
/// hue numbers of one segment in one color.
fn apart_by(a: &str, b: &str) -> i64 {
    let hue = |hex: &str| {
        let c = |i: usize| f64::from(u8::from_str_radix(&hex[i..i + 2], 16).unwrap());
        let (r, g, b) = (c(0), c(2), c(4));
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let h = if max == r {
            (g - b) / (max - min)
        } else if max == g {
            2.0 + (b - r) / (max - min)
        } else {
            4.0 + (r - g) / (max - min)
        };
        (h * 60.0).rem_euclid(360.0)
    };
    let d = (hue(a) - hue(b)).rem_euclid(360.0);
    d.min(360.0 - d).round() as i64
}

#[test]
fn each_tab_takes_a_hue_of_its_own_and_keeps_it() {
    let mut server = TestServer::start();
    settings(&server, json!({"github": false, "linear": false}));
    let work = server.dir.path().join("shop");
    repo(&work, "main");
    let work_dir = work.to_str().unwrap();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    // Three tabs in one directory, each far from the others' hues, and
    // drawn in its own.
    client.type_text(&format!("cd {work_dir}\r"));
    client.wait_for("the first tab in the repository", |s| {
        s[29].ends_with("1:shop")
    });
    server.ok(&["new-tab", "-d", "-c", work_dir]);
    server.ok(&["new-tab", "-d", "-c", work_dir]);
    let before = hues(&server);
    assert_eq!(before.len(), 3);
    for (i, (_, _, a)) in before.iter().enumerate() {
        for (_, _, b) in &before[i + 1..] {
            assert!(apart_by(a, b) >= 85, "{before:?}");
        }
    }
    let tabs = server.list()["sessions"][0]["tabs"].clone();
    for (t, (id, hue, color)) in tabs.as_array().unwrap().iter().zip(&before) {
        let step = tabs::color::Step::of(t["options"]["@tab_step"].as_str().unwrap_or(""));
        let tint = tabs::color::Tint::new(*hue, step);
        assert_eq!(
            *color,
            tabs::color::tab_color(tint, Shade::Soft).hex(),
            "{id}"
        );
    }

    // Closing one leaves the others theirs, and a new tab takes the hue
    // farthest from those left.
    server.ok(&["kill-tab", "-t", &before[1].0]);
    let left: Vec<_> = before
        .iter()
        .filter(|t| t.0 != before[1].0)
        .cloned()
        .collect();
    assert_eq!(hues(&server), left);
    server.ok(&["new-tab", "-d", "-c", work_dir]);
    let now = hues(&server);
    assert!(left.iter().all(|t| now.contains(t)), "{now:?}");
    let new = now.iter().find(|t| !left.contains(t)).unwrap();
    assert!(left.iter().all(|t| apart_by(&new.2, &t.2) >= 85), "{now:?}");

    // An upgrade and a restart keep every tab's hue.
    let (code, out, err) = server.run(&["upgrade"]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(hues(&server), now, "after an upgrade");
    server.ok(&["save"]);
    drop(client);
    server.restart();
    assert_eq!(hues(&server), now, "after a restore");

    // With each tab's own hue off, the tabs of one directory share its color.
    settings(
        &server,
        json!({"github": false, "linear": false, "tab_colors_apart": false}),
    );
    let home = tabs::accent_color("shop | branch:main", Shade::Soft).hex();
    let deadline = common::deadline(Duration::from_secs(15));
    while !hues(&server).iter().all(|(_, _, color)| *color == home) {
        assert!(Instant::now() < deadline, "{:?}", hues(&server));
        std::thread::sleep(Duration::from_millis(50));
    }
    // And the statusline is given no tab's tint.
    assert_eq!(server.ok(&["show", "-t", &now[0].0, "@tab_tint"]), "");
}

#[test]
fn claude_s_statusline_draws_its_tab_s_hue() {
    let server = TestServer::start();
    settings(&server, json!({"github": false, "linear": false}));
    let work = server.dir.path().join("shop");
    repo(&work, "main");
    let work_dir = work.to_str().unwrap();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    server.ok(&["new-tab", "-d", "-c", work_dir]);
    let tabs_now = hues(&server);
    let (id, hue, _) = tabs_now.iter().find(|t| t.0 != "@1").unwrap();
    let step = server.ok(&["show", "-t", id, "@tab_step"]);
    let tint = tabs::color::Tint::new(*hue, tabs::color::Step::of(&step));
    // The statusline as Claude Code runs it in the tab's pane, and outside
    // the workspace.
    let payload = json!({
        "model": { "display_name": "Opus" },
        "workspace": { "current_dir": work_dir },
    })
    .to_string();
    let out = server.dir.path().join("line");
    let script = format!(
        "printf '%s' '{payload}' | {} statusline > {} && unset SWITCHBOARD SWITCHBOARD_PANE && printf '%s' '{payload}' | {} statusline > {}.outside; exec sleep 600",
        common::BIN,
        out.display(),
        common::BIN,
        out.display()
    );
    server.ok(&["new-tab", "-d", "-c", work_dir, "--", "sh", "-c", &script]);
    // The new tab's own hue, not the second's.
    let third = hues(&server)
        .into_iter()
        .find(|t| !tabs_now.contains(t))
        .unwrap();
    let outside = PathBuf::from(format!("{}.outside", out.display()));
    let deadline = common::deadline(Duration::from_secs(15));
    while std::fs::read_to_string(&outside).map_or(true, |s| s.is_empty()) {
        assert!(Instant::now() < deadline, "the statusline never ran");
        std::thread::sleep(Duration::from_millis(50));
    }
    let step = server.ok(&["show", "-t", &third.0, "@tab_step"]);
    let own = tabs::color::line_color(
        tabs::color::Tint::new(third.1, tabs::color::Step::of(&step)),
        Shade::Soft,
    );
    let line = std::fs::read_to_string(&out).unwrap();
    assert!(
        line.starts_with(&format!("\x1b[38;2;{};{};{}mOpus", own.r, own.g, own.b)),
        "{line:?}"
    );
    assert_ne!(
        tint,
        tabs::color::Tint::new(third.1, tabs::color::Step::of(&step))
    );
    let home = tabs::statusline_color("shop | branch:main", Shade::Soft);
    let line = std::fs::read_to_string(&outside).unwrap();
    assert!(
        line.starts_with(&format!("\x1b[38;2;{};{};{}mOpus", home.r, home.g, home.b)),
        "{line:?}"
    );
}
