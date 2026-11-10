//! The views as someone uses them, against a server of the test's own and a
//! stand-in for GitHub on 127.0.0.1 (as tests/tabs.rs has one): M-v opens
//! the branch's PR description in a pane to the right, running `switchboard
//! pr-view`, and M-v again closes it; M-r where there is no repository says
//! why it can't open the PR. And `switchboard open-url` opens a link in its
//! site's Chrome profile, with a stand-in for Chrome writing down what it
//! was given (test/open-url.test.js's test, with its `open`). A link a
//! program in a pane writes as OSC 8 reaches the client as one, and M-p's
//! link commands list a pane's links and images, copying one, reading one
//! beside it, or opening a file through a stand-in for the desktop's opener.

mod common;

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::{BIN, TempDir, stand_in};
use serde_json::json;
// What only the tests against a stand-in for GitHub use, which need the
// `net` feature.
use {common::TestServer, serde_json::Value, std::path::Path};

/// Settings for the tests: no desktop notifications or tips, and these.
fn settings(server: &TestServer, extra: Value) {
    let mut all = json!({"notifications": false, "tips": false});
    for (k, v) in extra.as_object().unwrap() {
        all[k] = v.clone();
    }
    common::write_settings(&server.home, all);
}

/// Runs git in `dir` with an identity of its own.
#[cfg(feature = "net")]
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

/// The first tab's panes, as `list` gives them.
fn panes(server: &TestServer) -> Vec<Value> {
    server.list()["sessions"][0]["tabs"][0]["panes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn wait_panes(server: &TestServer, n: usize) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let found = panes(server);
        if found.len() == n {
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "waiting for {n} panes: {found:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(feature = "net")]
mod with_github {
    use super::*;
    use crate::common::http::{Fake, Reply};

    /// A stand-in for GitHub (tests/common/http.rs), as strict as GitHub
    /// about how it is asked: the PR on a branch for a question about pull
    /// requests, its description for one about a pull request by its number.
    fn fake_github() -> Fake {
        Fake::github("test-token", |request| {
            if request.path() != "/graphql" {
                return None;
            }
            let asked = request.text();
            let pr = json!({
                "number": 12, "url": "https://github.com/acme/widgets/pull/12",
                "title": "Fix the login", "state": "OPEN", "isDraft": false,
                "baseRefName": "main", "headRefName": "feature-x",
                "headRepositoryOwner": {"login": "acme"}, "author": {"login": "jo"},
                "commits": {"totalCount": 2}, "changedFiles": 1,
                "files": {"nodes": [{"path": "src/login.rs", "additions": 3, "deletions": 1}]},
                "body": "## Why\n\nThe token went **stale**.\n\n```mermaid\nflowchart LR\n  A[Login] --> B[Token]\n```\n"
            });
            let answer = if asked.contains("pullRequests") {
                json!({"data": {"repository": {"pullRequests": {"nodes": [pr]}}}})
            } else {
                json!({"data": {"repository": {"pullRequest": pr}}})
            };
            Some(Reply::json(200, answer))
        })
    }

    #[test]
    fn m_v_opens_the_pr_to_the_right_and_closes_it_again() {
        let fake = fake_github();
        let server = TestServer::start_with_env(&[
            ("GH_TOKEN", "test-token"),
            ("SWITCHBOARD_GITHUB_GRAPHQL", &fake.graphql_url()),
            ("SWITCHBOARD_GITHUB_API", &fake.url),
        ]);
        settings(&server, json!({"linear": false}));
        let work = server.dir.path().join("widgets");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q"]);
        git(&work, &["symbolic-ref", "HEAD", "refs/heads/feature-x"]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        git(
            &work,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/widgets.git",
            ],
        );

        let mut client = server.attach(140, 36);
        client.wait_text("$");

        // Outside a repository, M-r says why it can't open the PR.
        client.send(b"\x1br");
        client.wait_text("Can't open the PR");
        client.wait_text("isn't in a git repository");
        client.type_text("x");

        client.type_text(&format!("cd {}\r", work.display()));
        let tab = server.list()["sessions"][0]["tabs"][0].clone();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !panes(&server)[0]["cwd"]
            .as_str()
            .is_some_and(|d| d.ends_with("widgets"))
        {
            assert!(Instant::now() < deadline, "the shell didn't cd: {tab}");
            std::thread::sleep(Duration::from_millis(50));
        }

        client.send(b"\x1bv");
        let screen = client.wait_text("Fix the login #12");
        let row = screen
            .iter()
            .find(|l| l.contains("Fix the login #12"))
            .unwrap();
        let col = row[..row.find("Fix the login").unwrap()].chars().count();
        assert!(col > 70, "drawn in a pane on the right: {row:?}");
        client.wait_text("jo wants to merge");
        client.wait_text("Why");
        client.wait_text("│ Login ├");
        let shown = wait_panes(&server, 2);
        assert_eq!(shown[1]["options"]["@pr_view"], "1", "{shown:?}");
        assert_eq!(shown[1]["active"], true, "the view has the focus");
        assert!(
            shown[1]["cols"].as_u64().unwrap() < shown[0]["cols"].as_u64().unwrap(),
            "it takes less than half: {shown:?}"
        );
        // Kept for the next time, at once.
        let cache = server.home.join(".cache/switchboard/pr-view");
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);

        client.send(b"\x1bv");
        let shown = wait_panes(&server, 1);
        assert!(shown[0]["options"].get("@pr_view").is_none());
        client.wait_for("the view gone", |s| {
            !s.iter().any(|l| l.contains("Fix the login"))
        });
    }

    #[test]
    fn with_ux2_m_v_shows_the_pr_s_board_in_claude_s_place() {
        let fake = fake_github();
        // Set from the start, so M-v never finds the server still without it.
        let server = TestServer::start_with_env_and_settings(
            &[
                ("GH_TOKEN", "test-token"),
                ("SWITCHBOARD_GITHUB_GRAPHQL", &fake.graphql_url()),
                ("SWITCHBOARD_GITHUB_API", &fake.url),
            ],
            &json!({"notifications": false, "tips": false, "linear": false, "ux2": true, "sidebar": false, "photos": false}).to_string(),
        );
        let work = server.dir.path().join("widgets");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q"]);
        git(&work, &["symbolic-ref", "HEAD", "refs/heads/feature-x"]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        git(
            &work,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/widgets.git",
            ],
        );
        let mut client = server.attach(140, 36);
        client.wait_for("the header", |s| s[2].contains("M-c Claude"));
        client.wait_text("$");
        client.type_text(&format!("cd {}\r", work.display()));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !panes(&server)[0]["cwd"]
            .as_str()
            .is_some_and(|d| d.ends_with("widgets"))
        {
            assert!(Instant::now() < deadline, "the shell didn't cd");
            std::thread::sleep(Duration::from_millis(50));
        }

        client.send(b"\x1bv");
        // The board: its head, the description, and the panel beside it
        // with the PR's files, the footer's keys under them.
        let screen = client.wait_text("FILES");
        let head = screen
            .iter()
            .find(|l| l.contains("#12 Fix the login"))
            .unwrap();
        assert!(head.contains("Open"), "{head:?}");
        client.wait_text("jo wants to merge 2 commits into main from feature-x");
        let file = client.wait_text("src/login.rs");
        let row = file.iter().find(|l| l.contains("src/login.rs")).unwrap();
        assert!(
            row.contains('│') && row.trim_end().ends_with("+3 −1"),
            "{row:?}"
        );
        client.wait_text("M-r on GitHub");
        // In Claude's place: the only pane, the header's PR key lit.
        let shown = wait_panes(&server, 1);
        assert_eq!(shown[0]["options"]["@pr_view"], "1", "{shown:?}");

        client.send(b"\x1bv");
        client.wait_for("the board gone", |s| !s.iter().any(|l| l.contains("FILES")));
    }

    #[test]
    fn the_branch_s_ticket_and_pr_are_copied_and_its_merge_queue_opened() {
        let fake = fake_github();
        let server = TestServer::start_with_env(&[
            ("GH_TOKEN", "test-token"),
            ("SWITCHBOARD_GITHUB_GRAPHQL", &fake.graphql_url()),
            ("SWITCHBOARD_GITHUB_API", &fake.url),
            ("LINEAR_WORKSPACE", "acme"),
        ]);
        settings(&server, json!({"sidebar": false}));
        let did = server.dir.path().join("did");
        desktop_stand_ins(&server.home, &did);
        let work = server.dir.path().join("widgets");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q"]);
        git(&work, &["symbolic-ref", "HEAD", "refs/heads/eng-42-login"]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        git(
            &work,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/widgets.git",
            ],
        );
        let mut client = server.attach(120, 30);
        client.wait_text("$");
        client.type_text(&format!("cd {}\r", work.display()));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !panes(&server)[0]["cwd"]
            .as_str()
            .is_some_and(|d| d.ends_with("widgets"))
        {
            assert!(Instant::now() < deadline, "the shell didn't cd");
            std::thread::sleep(Duration::from_millis(50));
        }

        for (command, copied) in [
            ("copy-ticket-id", "Copied ENG-42"),
            (
                "copy-ticket-url",
                "Copied https://linear.app/acme/issue/ENG-42",
            ),
            ("copy-pr-number", "Copied 12"),
            (
                "copy-pr-url",
                "Copied https://github.com/acme/widgets/pull/12",
            ),
        ] {
            server.ok(&["press", command]);
            client.wait_text(copied);
            client.type_text("x");
            client.wait_for("the box closed", |s| {
                !s.iter().any(|l| l.contains("Copied"))
            });
        }

        // The queue of the PR's base, in the browser.
        server.ok(&["press", "open-merge-queue"]);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let opened = std::fs::read_to_string(&did).unwrap_or_default();
            if opened.contains("https://github.com/acme/widgets/queue/main") {
                break;
            }
            assert!(Instant::now() < deadline, "nothing opened it: {opened:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[test]
fn open_url_opens_a_link_in_its_sites_chrome_profile() {
    let dir = TempDir::new("sbo");
    let home = dir.path().join("home");
    let bin = home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let opened = dir.path().join("opened");
    // Chrome on Linux, `open` on macOS: each writes down what it was given,
    // after its name.
    stand_in::desktop(
        &bin,
        &["google-chrome", "open"],
        &opened,
        &dir.path().join("clipboard"),
    );
    // Chrome's profiles, where it keeps them on either.
    let local_state = json!({"profile": {"info_cache": {
        "Default": {"name": "Person 1"}, "Profile 2": {"name": "Work"}
    }}});
    for at in [
        ".config/google-chrome/Local State",
        "Library/Application Support/Google/Chrome/Local State",
    ] {
        let file = home.join(at);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, local_state.to_string()).unwrap();
    }
    let config = home.join(".config/switchboard");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("browser.json"),
        json!({"hosts": {"linear.app": "Work"}, "default": "Default"}).to_string(),
    )
    .unwrap();

    let open = |url: &str| -> String {
        let _ = std::fs::remove_file(&opened);
        let path = std::env::var("PATH").unwrap_or_default();
        let status = Command::new(BIN)
            .args(["open-url", url])
            .env("HOME", &home)
            .env("PATH", format!("{}:{path}", bin.display()))
            .env("SWITCHBOARD_STATE_DIR", dir.path().join("s"))
            .env_remove("SWITCHBOARD_SETTINGS")
            .stdin(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        // Chrome is left running on its own, so what it wrote comes a
        // moment after.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(text) = std::fs::read_to_string(&opened)
                && text.ends_with('\n')
            {
                return text.trim().to_string();
            }
            assert!(Instant::now() < deadline, "nothing opened {url}");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let chrome = |profile: &str, url: &str| {
        if cfg!(target_os = "macos") {
            format!("open -na Google Chrome --args --profile-directory={profile} {url}")
        } else {
            format!("google-chrome --profile-directory={profile} {url}")
        }
    };
    let ticket = "https://linear.app/x/issue/AB-1";
    assert_eq!(
        open(ticket),
        chrome("Profile 2", ticket),
        "the site's profile, named as Chrome shows it"
    );
    let pr = "https://github.com/o/r/pull/12345#linear.app";
    assert_eq!(open(pr), chrome("Default", pr), "else the default profile");
    stand_in::assert_none_refused(&bin);
}

#[test]
fn a_link_a_program_writes_reaches_the_terminal_as_a_link() {
    let server = TestServer::start();
    settings(&server, json!({"sidebar": false}));
    let mut client = server.attach(100, 20);
    client.wait_text("$");
    client.type_text(
        "printf '\\033]8;;https://example.com/osc\\033\\\\%s\\033]8;;\\033\\\\\\n' linked-text\r",
    );
    client.wait_for("the link's text", |s| {
        s.iter()
            .any(|l| l.ends_with("linked-text") && !l.contains('\''))
    });
    let raw = String::from_utf8_lossy(&client.raw).into_owned();
    let open = raw
        .rfind("\x1b]8;;https://example.com/osc\x1b\\")
        .expect("the link sent on as OSC 8");
    let text = raw[open..].find("linked-text").expect("its text inside") + open;
    let close = raw[text..].find("\x1b]8;;\x1b\\").expect("then closed") + text;
    assert!(open < text && text < close);
}

/// Stand-ins for what copies and opens on each desktop, each as strict as
/// the real one about how it is called and writing down what it was given
/// in `did`, so a test touches neither the clipboard nor the desktop.
fn desktop_stand_ins(home: &Path, did: &Path) {
    let tools: Vec<&str> = stand_in::OPENERS
        .iter()
        .chain(stand_in::CLIPBOARD)
        .copied()
        .collect();
    stand_in::desktop(&home.join("bin"), &tools, did, &home.join("clipboard"));
}

#[test]
fn the_link_commands_list_what_the_pane_shows_and_copy_read_or_open_it() {
    let server = TestServer::start();
    settings(&server, json!({"sidebar": false}));
    let did = server.dir.path().join("did");
    desktop_stand_ins(&server.home, &did);
    let work = server.dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("shot.png"), b"png").unwrap();

    let mut client = server.attach(120, 30);
    client.wait_text("$");
    // The shell's prompt waited for, so the next line isn't typed into the
    // middle of it.
    client.type_text(&format!("cd {} && echo in''-work\r", work.display()));
    client.wait_text("in-work");
    // Nothing listens on port 9, so reading a link fails at once, reaching
    // nothing.
    client.type_text(
        "printf 'see http://127.0.0.1:9/plain, \\033]8;;http://127.0.0.1:9/docs\\033\\\\the docs\\033]8;;\\033\\\\ and shot.png\\n'\r",
    );
    client.wait_for("the printed line", |s| {
        s.iter()
            .any(|l| l.starts_with("see http") && l.ends_with("shot.png"))
    });

    // Copy: the newest first, a hyperlink with its text.
    server.ok(&["press", "copy-link"]);
    let screen = client.wait_text("the docs  http://127.0.0.1:9/docs");
    // In the picker's box, not the typed line above it, which holds the
    // same links.
    let row = |s: &[String], text: &str| {
        s.iter()
            .position(|l| l.contains('│') && l.contains(text))
            .unwrap()
    };
    assert!(
        row(&screen, "the docs  http") < row(&screen, "http://127.0.0.1:9/plain"),
        "newest first:\n{}",
        screen.join("\n")
    );
    client.wait_text("ctrl-o open in browser");
    client.type_text("\r");
    client.wait_text("Copied http://127.0.0.1:9/docs");
    let raw = String::from_utf8_lossy(&client.raw).into_owned();
    assert!(
        raw.contains("\x1b]52;c;aHR0cDovLzEyNy4wLjAuMTo5L2RvY3M="),
        "to the terminal's clipboard"
    );
    client.type_text("x");

    // Open, with Ctrl-O: read beside the pane instead.
    server.ok(&["press", "open-link"]);
    client.wait_text("ctrl-o read it beside");
    client.type_text("plain\x0f");
    let shown = wait_panes(&server, 2);
    let view = shown
        .iter()
        .find(|p| p["options"]["@link_view"] == "1")
        .expect("a link view");
    assert_eq!(view["options"]["@link_url"], "http://127.0.0.1:9/plain");

    // An image the pane names, opened in its app.
    let first = shown
        .iter()
        .find(|p| p["options"].get("@link_view").is_none())
        .unwrap();
    server.ok(&["select-pane", "-t", first["id"].as_str().unwrap()]);
    server.ok(&["press", "open-media"]);
    client.wait_text("Images and videos");
    client.wait_text("shot.png");
    client.type_text("\r");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let opened = std::fs::read_to_string(&did).unwrap_or_default();
        if opened
            .lines()
            .any(|l| l.ends_with("work/shot.png") && !l.contains("copy"))
        {
            break;
        }
        assert!(Instant::now() < deadline, "nothing opened it: {opened:?}");
        std::thread::sleep(Duration::from_millis(50));
    }

    // In the notes pane, the links are the note's, since Notes shows a
    // link's text alone.
    let notes = server.home.join("Notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(notes.join(".notes.json"), r#"{"last": "Plan.md"}"#).unwrap();
    std::fs::write(
        notes.join("Plan.md"),
        "# Plan\n\n- [ ] Read [the RFC](https://rfc.example/1)\n",
    )
    .unwrap();
    let id = first["id"].as_str().unwrap();
    server.ok(&["set", "-t", id, "@notes_pane", "1"]);
    server.ok(&["press", "read-link"]);
    // One of one in the picker, whatever the pane shows beside its box.
    let screen = client.wait_text("the RFC  https://rfc.example/1");
    assert!(
        screen.iter().any(|l| l.contains("│   1/1 ")),
        "the note's links alone:\n{}",
        screen.join("\n")
    );
}

/// Runs `switchboard hook <event>` in `pane` of the test's server with
/// `input` on its stdin, as Claude Code runs its hooks there.
fn hook(server: &TestServer, pane: &str, event: &str, input: Value) {
    use std::io::Write;
    let mut child = Command::new(BIN)
        .args(["hook", event])
        .env("HOME", &server.home)
        .env("SWITCHBOARD_STATE_DIR", &server.state)
        .env("SWITCHBOARD", server.socket())
        .env("SWITCHBOARD_PANE", pane)
        .env_remove("SWITCHBOARD_SETTINGS")
        .env_remove("SLACK_RELAY_HELPER")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("switchboard runs");
    let _ = child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input.to_string().as_bytes());
    let out = child.wait_with_output().expect("switchboard ends");
    assert!(out.status.success(), "hook {event}: {}", server.log());
}

#[test]
fn the_link_commands_list_the_links_you_gave_the_panes_claude() {
    let server = TestServer::start();
    settings(&server, json!({"sidebar": false}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    let pane = panes(&server)[0]["id"].as_str().unwrap().to_string();
    // Nothing listens on port 9, and nothing here reads a link anyway.
    let prompt = |text: &str| json!({"hook_event_name": "UserPromptSubmit", "prompt": text});

    // A link in a prompt, which Claude Code would show as "[Pasted text
    // #1]": on no screen, and listed.
    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        prompt("look at this:\nhttp://127.0.0.1:9/pasted."),
    );
    server.ok(&["press", "copy-link"]);
    client.wait_text("you gave it  http://127.0.0.1:9/pasted");
    client.type_text("\r");
    client.wait_text("Copied http://127.0.0.1:9/pasted");
    client.type_text("x");

    // A fresh session starts with none.
    hook(&server, &pane, "SessionStart", json!({"source": "clear"}));
    server.ok(&["press", "copy-link"]);
    client.wait_text("No links in this pane");
    client.type_text("x");

    // One the pane shows too is listed once, as the pane's.
    hook(
        &server,
        &pane,
        "UserPromptSubmit",
        prompt("http://127.0.0.1:9/pasted and http://127.0.0.1:9/shot.png"),
    );
    client.type_text("echo http://127.0.0.1:9/pasted\r");
    client.wait_for("the echoed link", |s| {
        s.iter().any(|l| l.starts_with("http://127.0.0.1:9/pasted"))
    });
    server.ok(&["press", "copy-link"]);
    let screen = client.wait_text("you gave it  http://127.0.0.1:9/shot.png");
    assert!(
        screen.iter().any(|l| l.contains("│   2/2 "))
            && !screen
                .iter()
                .any(|l| l.contains("you gave it  http://127.0.0.1:9/pasted")),
        "the pane's link once, then the prompt's:\n{}",
        screen.join("\n")
    );
    // A lone Esc is taken once nothing follows it, so the list is waited
    // out before the next command.
    client.type_text("\x1b");
    client.wait_for("the list closed", |s| {
        !s.iter().any(|l| l.contains("Copy a link"))
    });

    // A link to a picture among them is one of the pane's pictures.
    server.ok(&["press", "open-media"]);
    client.wait_text("Images and videos");
    client.wait_text("you gave it  http://127.0.0.1:9/shot.png");
}

#[test]
fn with_prompt_links_off_the_link_commands_list_only_what_the_pane_shows() {
    let server = TestServer::start();
    settings(&server, json!({"sidebar": false, "prompt_links": false}));
    let mut client = server.attach(120, 30);
    client.wait_text("$");
    let pane = panes(&server)[0]["id"].as_str().unwrap().to_string();
    // The server reads the settings again a moment after they change, so
    // a list that still shows the link is closed and asked for again.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        hook(
            &server,
            &pane,
            "UserPromptSubmit",
            json!({"prompt": "see http://127.0.0.1:9/pasted"}),
        );
        server.ok(&["press", "copy-link"]);
        let screen = client.wait_for("an answer", |s| {
            s.iter()
                .any(|l| l.contains("No links in this pane") || l.contains("you gave it"))
        });
        if screen.iter().any(|l| l.contains("No links in this pane")) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "still listed:\n{}",
            screen.join("\n")
        );
        client.type_text("\x1b");
        hook(&server, &pane, "SessionStart", json!({"source": "clear"}));
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// What the image views keep, under a test's home.
fn image_cache(home: &Path) -> switchboard::views::image::cache::Cache {
    switchboard::views::image::cache::Cache::at(home.join(".cache/switchboard"))
}

#[test]
fn the_palette_clears_the_image_cache_after_a_yes() {
    let server = TestServer::start();
    settings(&server, json!({"sidebar": false}));
    let cache = image_cache(&server.home);
    let picture = cache
        .keep_asset("https://example.com/a.png", "png", b"picture")
        .unwrap();
    let text = server.home.join(".cache/switchboard/pr-image/ENG-1.json");
    std::fs::create_dir_all(text.parent().unwrap()).unwrap();
    std::fs::write(&text, "{}").unwrap();
    let other = server.home.join(".cache/switchboard/tab-names.json");
    std::fs::write(&other, "{}").unwrap();

    let mut client = server.attach(100, 30);
    client.wait_text("$");
    server.ok(&["press", "clear-image-cache"]);
    client.wait_text("Clear the PR and ticket image cache? [Y/n]");
    client.type_text("n");
    client.wait_for("the question gone", |s| {
        !s.iter().any(|l| l.contains("image cache?"))
    });
    assert!(picture.exists() && text.exists(), "no is no");

    server.ok(&["press", "clear-image-cache"]);
    client.wait_text("Clear the PR and ticket image cache? [Y/n]");
    client.type_text("\r");
    client.wait_text("The PR and ticket image cache is cleared.");
    assert!(!cache.assets().exists(), "the copies gone");
    assert!(!text.exists(), "and the text last read");
    assert!(other.exists(), "nothing else of the cache's");
}

/// `switchboard pr-image --ticket ENG-1` with `PR_IMAGE_DUMP`, in a home of
/// the test's own, with no terminal (so its pane is 100 by 50 cells of 16
/// by 32 pixels) and nothing on its PATH, Chrome included; Linear moved to
/// a port nothing listens on.
#[cfg(target_os = "linux")]
fn dump_ticket(home: &Path, out: &Path) -> std::process::Output {
    use std::os::unix::process::CommandExt;
    let bin = home.join("empty-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let mut cmd = Command::new(BIN);
    cmd.args(["pr-image", "--ticket", "ENG-1"])
        .env_clear()
        .env("HOME", home)
        .env("PATH", &bin)
        .env("PR_IMAGE_DUMP", out)
        .env("SWITCHBOARD_LINEAR_API", "http://127.0.0.1:9")
        .stdin(Stdio::null());
    // SAFETY: setsid alone, between fork and exec, so the program has no
    // terminal to ask its size of.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.output().expect("pr-image runs")
}

/// A ticket opened again at the same size shows the pages kept, without
/// looking for Chrome (Linux alone, where the PATH is where Chrome is
/// looked for, so a failure can't start a real one).
#[cfg(target_os = "linux")]
#[test]
fn a_ticket_drawn_before_shows_from_the_cache_without_chrome() {
    use switchboard::views::image::page::Parts;
    use switchboard::views::image::render::{Shot, View};
    let dir = TempDir::new("sb-image-cache");
    let home = dir.path().join("home");
    let parts = Parts {
        title: "Fix the login".into(),
        number: "ENG-1".into(),
        html: "<p>hi</p>".into(),
        ..Parts::default()
    };
    let text = home.join(".cache/switchboard/pr-image/ENG-1.json");
    std::fs::create_dir_all(text.parent().unwrap()).unwrap();
    std::fs::write(&text, serde_json::to_string(&parts).unwrap()).unwrap();
    let drawn = dir.path().join("page-1-1.png");
    std::fs::write(&drawn, "page one").unwrap();
    let view = View::new(100, 50, (16, 32));
    let shot = Shot {
        file: drawn,
        rows: 10,
        from: 0.0,
    };
    image_cache(&home).save_pages(&parts, &view, &[shot]);

    let out = dir.path().join("dump");
    let run = dump_ticket(&home, &out);
    assert!(run.status.success(), "{run:?}");
    assert_eq!(
        std::fs::read_to_string(out.join("page-1.png")).unwrap(),
        "page one"
    );

    // With the cache turned off, it has to be drawn, and there is no
    // Chrome to draw it.
    let config = home.join(".config/switchboard");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("settings.json"), r#"{"image_cache": false}"#).unwrap();
    let run = dump_ticket(&home, &dir.path().join("dump-off"));
    assert!(!run.status.success());
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("needs Google Chrome or Chromium"),
        "{run:?}"
    );
}
