//! `switchboard demo`, end to end: laid out in a temporary root (never
//! /tmp's own `sb-demo-<uid>`), its server started with no client, its
//! tabs, the PRs and tickets its services answer, its stand-in Claude
//! waiting on you, its stand-ins writing to its own desktop log, nothing in
//! its home leading out of it, and `--clean` removing it. The demo makes
//! its own environment from nothing, so it reaches neither your home nor a
//! service whatever the test's environment holds.

use crate::common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use common::{BIN, TempDir};
use serde_json::Value;
use switchboard::demo;

/// A demo in a temporary root, cleaned however the test ends.
struct Demo {
    dir: TempDir,
    home: PathBuf,
}

impl Demo {
    fn new() -> Demo {
        let dir = TempDir::new("sbd");
        let home = demo::home_in(dir.path());
        Demo { dir, home }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(BIN)
            .args(args)
            .env_clear()
            .env("HOME", self.dir.path())
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("SHELL", "/bin/sh")
            .env("TERM", "xterm-256color")
            .env("SWITCHBOARD_DEMO_ROOT", self.dir.path())
            .stdin(Stdio::null())
            .output()
            .expect("switchboard runs")
    }

    fn list(&self) -> Value {
        serde_json::from_str(&demo::ask(&self.home, &["list"]).expect("the demo's server answers"))
            .expect("list is JSON")
    }

    /// Waits for `pred` to hold of the demo's tabs.
    fn wait_for(&self, what: &str, pred: impl Fn(&[Value]) -> bool) -> Vec<Value> {
        let deadline = common::deadline(Duration::from_secs(60));
        loop {
            let tabs: Vec<Value> = self.list()["sessions"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|s| s["tabs"].as_array().cloned().unwrap_or_default())
                .collect();
            if pred(&tabs) {
                return tabs;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; the tabs: {tabs:#?}"
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

impl Drop for Demo {
    fn drop(&mut self) {
        let _ = self.run(&["demo", "--clean"]);
    }
}

fn named<'a>(tabs: &'a [Value], name: &str) -> Option<&'a Value> {
    tabs.iter().find(|t| t["name"] == name)
}

#[test]
fn the_demo_runs_on_made_up_data_and_cleans_up_after_itself() {
    let d = Demo::new();
    let out = d.run(&["demo", "--detached"]);
    assert!(
        out.status.success(),
        "demo --detached failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("headless"));

    // Its tabs, a PR on each Claude tab from the GitHub stand-in.
    let tabs = d.wait_for("the PRs on the Claude tabs", |tabs| {
        [
            "Rate-limit the login endpoint",
            "Dark mode toggle",
            "API quickstart guide",
        ]
        .iter()
        .all(|n| named(tabs, n).is_some_and(|t| t["facts"]["pr"]["number"].is_u64()))
    });
    assert_eq!(
        named(&tabs, "Dark mode toggle").unwrap()["facts"]["pr"]["checks"]["fail"],
        1
    );
    assert!(named(&tabs, "Notes").is_some());

    // The stand-in Claude asks you something within its first turn, and
    // takes a digit alone as its answer, as mission control sends one.
    let tabs = d.wait_for("the guide's tab asking", |tabs| {
        named(tabs, "API quickstart guide").is_some_and(|t| t["agent"] == "waiting")
    });
    let asking = named(&tabs, "API quickstart guide").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    demo::ask(&d.home, &["send-keys", "-t", &asking, "-l", "2"]).unwrap();
    d.wait_for("the guide's tab answered", |tabs| {
        named(tabs, "API quickstart guide").is_some_and(|t| t["agent"] != "waiting")
    });

    // The tickets from the Linear stand-in.
    let sources = demo::ask(&d.home, &["sources"]).unwrap();
    assert!(sources.contains("ABC-20"), "{sources}");

    // A link opened in the demo goes to its desktop log.
    let opened = Command::new(d.home.join(".local/bin/open"))
        .arg("https://github.com/acme/api/pull/128")
        .env_clear()
        .env("HOME", &d.home)
        .status()
        .unwrap();
    assert!(opened.success());
    let log = std::fs::read_to_string(demo::desktop_log(&d.home)).unwrap();
    assert!(log.contains("acme/api/pull/128"), "{log}");

    // A pane's login shell finds the stand-ins first, whatever the
    // system's startup files put on its PATH.
    let which = d.home.join("which-open");
    demo::ask(
        &d.home,
        &[
            "send-keys",
            "-t",
            "@1",
            "-l",
            &format!("command -v open > {}", which.display()),
        ],
    )
    .unwrap();
    demo::ask(&d.home, &["send-keys", "-t", "@1", "Enter"]).unwrap();
    let deadline = common::deadline(Duration::from_secs(10));
    while std::fs::read_to_string(&which)
        .unwrap_or_default()
        .trim()
        .is_empty()
    {
        assert!(Instant::now() < deadline, "the shell never answered");
        std::thread::sleep(Duration::from_millis(100));
    }
    let found = std::fs::read_to_string(&which).unwrap();
    assert!(
        found.trim().ends_with("/.local/bin/open") && found.contains("sb-demo-"),
        "{found}"
    );

    // Nothing the stand-in services were asked was refused.
    assert!(
        !demo::own_state(&d.home).join("refused.log").exists(),
        "{}",
        std::fs::read_to_string(demo::own_state(&d.home).join("refused.log")).unwrap_or_default()
    );

    // Nothing in its home leads out of it.
    let home = d.home.canonicalize().unwrap();
    assert_eq!(demo::links_out(&home, &home), Vec::<PathBuf>::new());

    let out = d.run(&["demo", "--clean"]);
    assert!(out.status.success());
    assert!(!d.home.exists());
    assert!(std::os::unix::net::UnixStream::connect(demo::socket_of(&d.home)).is_err());
}

#[test]
fn a_demo_home_that_is_a_link_is_refused() {
    let d = Demo::new();
    std::os::unix::fs::symlink(d.dir.path().join("elsewhere"), &d.home).unwrap();
    let out = d.run(&["demo", "--detached"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("is a link"));
    assert!(
        Path::new(&d.home)
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn the_tour_saves_a_board_for_each_step() {
    let d = Demo::new();
    let tour = d.dir.path().join("tour");
    let out = d.run(&[
        "demo",
        "--tour",
        &tour.to_string_lossy(),
        "--size",
        "140x40",
    ]);
    assert!(
        out.status.success(),
        "demo --tour failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    for step in demo::tour::STEPS {
        assert!(
            tour.join(format!("{}.txt", step.name)).is_file(),
            "{}",
            step.name
        );
    }
    let code_prs = std::fs::read_to_string(tour.join("code-prs.txt")).unwrap();
    assert!(code_prs.contains("Rate-limit the login"), "{code_prs}");
}
