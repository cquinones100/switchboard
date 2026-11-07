//! The views as someone uses them, against a server of the test's own and a
//! stand-in for GitHub on 127.0.0.1 (as tests/tabs.rs has one): M-v opens
//! the branch's PR description in a pane to the right, running `switchboard
//! pr-view`, and M-v again closes it; M-r where there is no repository says
//! why it can't open the PR. And `switchboard open-url` opens a link in its
//! site's Chrome profile, with a stand-in for Chrome writing down what it
//! was given (test/open-url.test.js's test, with its `open`). A link a
//! program in a pane writes as OSC 8 reaches the client as one.

mod common;

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::{BIN, TempDir};
use serde_json::json;
// What only the tests against a stand-in for GitHub use, which need the
// `net` feature.
#[cfg(feature = "net")]
use {common::TestServer, serde_json::Value, std::path::Path};

/// Settings for the tests: no desktop notifications or tips, and these.
#[cfg(feature = "net")]
fn settings(server: &TestServer, extra: Value) {
    let dir = server.home.join(".config/switchboard");
    std::fs::create_dir_all(&dir).unwrap();
    let mut all = json!({"notifications": false, "tips": false});
    for (k, v) in extra.as_object().unwrap() {
        all[k] = v.clone();
    }
    std::fs::write(dir.join("settings.json"), all.to_string()).unwrap();
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
#[cfg(feature = "net")]
fn panes(server: &TestServer) -> Vec<Value> {
    server.list()["sessions"][0]["tabs"][0]["panes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[cfg(feature = "net")]
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
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    /// A stand-in for GitHub's GraphQL endpoint: the PR on a branch for a
    /// question about pull requests, its description for one about a pull
    /// request by its number.
    fn fake_github() -> String {
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
                let pr = json!({
                    "number": 12, "url": "https://github.com/acme/widgets/pull/12",
                    "title": "Fix the login", "state": "OPEN", "isDraft": false,
                    "baseRefName": "main", "headRefName": "feature-x",
                    "headRepositoryOwner": {"login": "acme"}, "author": {"login": "jo"},
                    "body": "## Why\n\nThe token went **stale**.\n\n```mermaid\nflowchart LR\n  A[Login] --> B[Token]\n```\n"
                });
                let answer = if asked.contains("pullRequests") {
                    json!({"data": {"repository": {"pullRequests": {"nodes": [pr]}}}})
                } else {
                    json!({"data": {"repository": {"pullRequest": pr}}})
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
    fn m_v_opens_the_pr_to_the_right_and_closes_it_again() {
        let url = fake_github();
        let server = TestServer::start_with_env(&[
            ("GH_TOKEN", "test-token"),
            ("SWITCHBOARD_GITHUB_GRAPHQL", &url),
            ("SWITCHBOARD_GITHUB_API", &url),
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
}

#[test]
fn open_url_opens_a_link_in_its_sites_chrome_profile() {
    let dir = TempDir::new("sbo");
    let home = dir.path().join("home");
    let bin = home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let opened = dir.path().join("opened");
    // Chrome on Linux, `open` on macOS: each writes down what it was given.
    for name in ["google-chrome", "open"] {
        let script = bin.join(name);
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n",
                opened.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
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
            format!("-na Google Chrome --args --profile-directory={profile} {url}")
        } else {
            format!("--profile-directory={profile} {url}")
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
