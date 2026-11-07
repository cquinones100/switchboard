//! What a tab is called and the color it wears, as someone sees it: a pane
//! that moves into a git repository gives its tab the repository's name and
//! branch, drawn in the project's accent color; a name given to the tab
//! wins; and with GitHub and Linear on, the branch's PR number and ticket
//! title arrive from the background, here from a stand-in for both services
//! on 127.0.0.1.

mod common;

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use common::TestServer;
use serde_json::{Value, json};
use switchboard::tabs;

/// Settings for the tests: no desktop notifications, and these.
fn settings(server: &TestServer, extra: Value) {
    let dir = server.home.join(".config/switchboard");
    std::fs::create_dir_all(&dir).unwrap();
    let mut all = json!({"notifications": false});
    for (k, v) in extra.as_object().unwrap() {
        all[k] = v.clone();
    }
    std::fs::write(dir.join("settings.json"), all.to_string()).unwrap();
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
    let deadline = Instant::now() + Duration::from_secs(15);
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
    settings(&server, json!({"github": false, "linear": false}));
    let work = server.dir.path().join("shop");
    repo(&work, "feature-x");

    let mut client = server.attach(100, 30);
    client.wait_text("$");
    client.type_text(&format!("cd {}\r", work.display()));
    let screen = client.wait_for("the repository's name in the tab", |s| {
        s[29].ends_with("1:shop:feature-x")
    });

    // Drawn in the project's color, the same one accent-color.sh gave.
    let expected = tabs::accent_color_of(&work);
    assert_eq!(expected, tabs::accent_color("shop | branch:feature-x"));
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
    let main = tabs::accent_color("shop | branch:main");
    wait_tab(&server, "the trunk's color", |t| t["color"] == main.hex());

    // With project colors off, every tab is gray.
    settings(
        &server,
        json!({"github": false, "linear": false, "accent_colors": false}),
    );
    wait_tab(&server, "the gray", |t| t["color"] == "8a8a8a");
}

#[test]
fn m_shift_t_shows_the_tabs_whole_title_and_its_branch_and_copies_it() {
    let server = TestServer::start();
    settings(&server, json!({"github": false, "linear": false}));
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
}

#[cfg(feature = "net")]
mod from_the_services {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    /// A stand-in for GitHub's and Linear's GraphQL endpoints, answering
    /// whatever it is asked, on a connection each: a PR for a question about
    /// pull requests, a ticket for one about an issue.
    fn fake_services() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let mut reader = BufReader::new(stream);
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                let asked = String::from_utf8_lossy(&body);
                let answer = if asked.contains("pullRequests") {
                    json!({"data": {"repository": {"pullRequests": {"nodes": [{
                        "number": 12, "url": "https://github.com/acme/widgets/pull/12",
                        "title": "Fix the login", "state": "OPEN", "isDraft": false,
                        "baseRefName": "main", "headRefName": "eng-42-fix-login",
                        "headRepositoryOwner": {"login": "acme"}
                    }]}}}})
                } else {
                    json!({"data": {"issue": {
                        "id": "i1", "identifier": "ENG-42", "title": "Fix the login",
                        "url": "https://linear.app/acme/issue/ENG-42",
                        "state": {"name": "In Progress"}, "assignee": null
                    }}})
                }
                .to_string();
                let mut stream = reader.into_inner();
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                    answer.len()
                );
            }
        });
        url
    }

    #[test]
    fn the_pr_number_and_ticket_title_arrive_from_the_background() {
        let url = fake_services();
        let server = TestServer::start_with_env(&[
            ("GH_TOKEN", "test-token"),
            ("LINEAR_API_KEY", "test-key"),
            ("SWITCHBOARD_GITHUB_GRAPHQL", &url),
            ("SWITCHBOARD_GITHUB_API", &url),
            ("SWITCHBOARD_LINEAR_API", &url),
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
