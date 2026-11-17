//! `switchboard demo`, end to end: laid out in a temporary root (never
//! /tmp's own `sb-demo-<uid>`), its server started with no client, its
//! tabs, the PRs and tickets its services answer, its stand-in Claude
//! waiting on you, its stand-ins writing to its own desktop log, nothing in
//! its home leading out of it, and `--clean` removing it. The demo makes
//! its own environment from nothing, so it reaches neither your home nor a
//! service whatever the test's environment holds.

use crate::common;

use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
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
    // The first tab, the shell, starts in the demo's ~/code, so nothing
    // names it for the demo's home folder.
    let first_dir = tabs[0]["panes"][0]["cwd"].as_str().unwrap_or_default();
    assert_eq!(
        std::fs::canonicalize(first_dir).ok(),
        std::fs::canonicalize(d.home.join("code")).ok(),
        "{first_dir}"
    );

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
fn the_demos_sidebar_has_a_tab_in_every_group_and_its_agent_keeps_the_input_box_at_the_bottom() {
    let d = Demo::new();
    let out = d.run(&["demo", "--detached"]);
    assert!(
        out.status.success(),
        "demo --detached failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stage_of = |tabs: &[Value], name: &str| {
        named(tabs, name)
            .and_then(|t| t["stage"]["key"].as_str())
            .unwrap_or("")
            .to_string()
    };
    let wanted = [
        ("API quickstart guide", "needs"),
        ("Sessions in Redis", "working"),
        ("Password reset email queue", "review"),
        ("Billing history CSV", "ci"),
        ("Changelog page", "landing"),
        ("Avatar upload on Safari", "merged"),
    ];
    let tabs = d.wait_for("a tab in each of the sidebar's groups", |tabs| {
        wanted
            .iter()
            .all(|(name, stage)| stage_of(tabs, name) == *stage)
    });

    // The stand-in drawn as a generic coding agent: its tool calls after
    // `●`, the result under `└`, and the input box in the pane's bottom
    // rows, its hint at most third from the bottom with the statusline under it,
    // however little the conversation above fills.
    let tab = named(&tabs, "Sessions in Redis").unwrap();
    let working = tab["id"].as_str().unwrap().to_string();
    let rows = tab["panes"][0]["rows"].as_u64().unwrap() as usize;
    let deadline = common::deadline(Duration::from_secs(30));
    loop {
        let screen = demo::ask(&d.home, &["capture-pane", "-t", &working]).unwrap();
        let lines: Vec<&str> = screen.lines().collect();
        let hint_row = lines.iter().position(|l| l.contains("? for shortcuts"));
        let drawn = ["Coding agent", "●", "└", "│ >"]
            .iter()
            .all(|w| screen.contains(w));
        if let Some(hint_row) = hint_row.filter(|_| drawn) {
            assert!(hint_row + 3 >= rows, "{screen}");
            assert!(!screen.contains("Claude"), "{screen}");
            assert!(!screen.contains("stand-in"), "{screen}");
            break;
        }
        assert!(Instant::now() < deadline, "the stand-in's screen: {screen}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn a_demo_laid_out_by_an_older_layout_is_laid_out_again() {
    let d = Demo::new();
    let first = d.run(&["demo", "--detached"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    // As a build from before the layout was written left it: an empty
    // marker, and none of this layout's worktrees.
    let marker = demo::own_state(&d.home).join("laid-out");
    std::fs::write(&marker, "").unwrap();
    let worktree = demo::worktree(&d.home, "abc-15-dark-mode-toggle");
    std::fs::remove_dir_all(&worktree).unwrap();
    let again = d.run(&["demo", "--detached"]);
    let said = String::from_utf8_lossy(&again.stderr);
    assert!(again.status.success(), "{said}");
    assert!(said.contains("laid out by an older build"), "{said}");
    assert!(worktree.is_dir(), "{said}");
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), demo::LAYOUT);
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

/// Walks the tour of a demo of its own at `size` and gives the folder its
/// boards are in: the tour succeeded (a wait that failed says in its error
/// what the screen showed), every step's board is there with the text it
/// waits for, and every board it says it saved is there, a view of mission
/// control each besides the steps'.
fn toured(d: &Demo, size: &str) -> PathBuf {
    let tour = d.dir.path().join("tour");
    let out = d.run(&["demo", "--tour", &tour.to_string_lossy(), "--size", size]);
    assert!(
        out.status.success(),
        "demo --tour at {size} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Each step reached its board: the text it waits for is on it.
    let steps = demo::tour::STEPS.iter().chain(demo::tour::LAST);
    for step in steps.clone() {
        let board =
            std::fs::read_to_string(tour.join(format!("{}.txt", step.name))).unwrap_or_default();
        assert!(board.contains(step.wait), "{}: {board}", step.name);
    }
    // None skipped: as many boards as it says, each view's among them.
    let said = String::from_utf8_lossy(&out.stdout).into_owned();
    let boards = std::fs::read_dir(&tour)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "txt"))
        .count();
    assert!(said.contains(&format!("Saved {boards} boards")), "{said}");
    assert!(
        boards > steps.count() + 10,
        "{boards} boards: mission control's views are missing"
    );
    tour
}

#[test]
fn the_tour_saves_a_board_for_each_step() {
    let d = Demo::new();
    // The size the tour walks when none is given.
    let tour = toured(&d, "200x56");
    // The views that have nothing to show without the demo's own data.
    for (board, text) in [
        ("work-routines", "Triage new tickets"),
        ("work-reopen", "Spike: a GraphQL gateway"),
        ("work-autopilot", "Password reset"),
        ("day-reminders", "2 reminders"),
    ] {
        let shown = std::fs::read_to_string(tour.join(format!("{board}.txt"))).unwrap_or_default();
        assert!(shown.contains(text), "{board}: {shown}");
    }
    // No board shows the demo's temporary home: its paths are written from `~`.
    for step in demo::tour::STEPS {
        let board = std::fs::read_to_string(tour.join(format!("{}.txt", step.name))).unwrap();
        for temporary in ["/private", "sb-demo"] {
            assert!(
                !board.contains(temporary),
                "{} shows {temporary}: {board}",
                step.name
            );
        }
    }
    // The waiting agent's welcome box is a box, painted again at the tour's
    // size while it waits on a choice: its second line starts under its
    // first, not where the first ended.
    let waiting = std::fs::read_to_string(tour.join("tab-waiting.txt")).unwrap();
    let lines: Vec<&str> = waiting.lines().collect();
    let column = |line: &str, edge: &str| line.find(edge).map(|at| line[..at].chars().count());
    let name = lines
        .iter()
        .position(|l| l.contains("◆ Coding agent"))
        .unwrap_or_else(|| panic!("no welcome box: {waiting}"));
    assert_eq!(
        column(lines[name], "│ ◆"),
        column(lines[name - 1], "╭─"),
        "{waiting}"
    );

    // The PR's card, its title wrapped over two lines.
    let code_prs = std::fs::read_to_string(tour.join("code-prs.txt")).unwrap();
    assert!(
        code_prs.contains("Rate-limit the") && code_prs.contains("login endpoint"),
        "{code_prs}"
    );

    // The Usage view has a day of spend by tab, and the request audit.
    let usage = std::fs::read_to_string(tour.join("system-usage.txt")).unwrap();
    for tab in [
        "Rate-limit the login",
        "Dark mode toggle",
        "API quickstart guide",
    ] {
        assert!(usage.contains(tab), "{tab}: {usage}");
    }
    let state = d.home.join(".local/state/switchboard");
    let today = switchboard::util::ymd(&switchboard::util::local_now());
    for file in [
        format!("usage/spent-{today}.tsv"),
        format!("requests/{today}.tsv"),
    ] {
        let text = std::fs::read_to_string(state.join(&file)).unwrap_or_default();
        assert!(!text.is_empty(), "{file}");
    }

    // The calendar's events have who is invited and an agenda.
    let calendar: Value =
        serde_json::from_str(&std::fs::read_to_string(state.join("calendar.json")).unwrap())
            .unwrap();
    let events = calendar["events"].as_array().unwrap();
    assert!(
        events
            .iter()
            .all(|e| e["attendees"].as_array().is_some_and(|a| !a.is_empty())
                && e["description"].as_str().is_some_and(|t| !t.is_empty())),
        "{calendar}"
    );
}

#[test]
fn the_tour_saves_every_board_at_a_narrower_size() {
    let d = Demo::new();
    toured(&d, "160x45");
}

#[test]
fn the_demo_runs_on_the_build_it_was_started_from() {
    let d = Demo::new();
    let detached = |d: &Demo| {
        let out = d.run(&["demo", "--detached"]);
        assert!(
            out.status.success(),
            "demo --detached failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    detached(&d);

    // A server of this build is kept as it is.
    demo::ask(&d.home, &["set", "-t", "@1", "@kept", "yes"]).unwrap();
    detached(&d);
    assert_eq!(
        demo::ask(&d.home, &["show", "-t", "@1", "@kept"])
            .unwrap()
            .trim(),
        "yes"
    );

    // One answering another build is ended and started again on this one,
    // the demo's data kept.
    let note = d.home.join("Notes/Demo Notes.md");
    let mut text = std::fs::read_to_string(&note).unwrap();
    text.push_str("- [ ] Kept across the restart\n");
    std::fs::write(&note, &text).unwrap();
    demo::ask(&d.home, &["kill-server"]).unwrap();
    let socket = demo::socket_of(&d.home);
    let deadline = common::deadline(Duration::from_secs(10));
    while UnixStream::connect(&socket).is_ok() {
        assert!(Instant::now() < deadline, "the demo's server never ended");
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = std::fs::remove_file(&socket);
    let old = old_server(&socket);
    let said = detached(&d);
    assert!(
        said.contains("started by another build (0.0.1 (000000000000))"),
        "{said}"
    );
    assert!(
        old.join().unwrap(),
        "the old build's server was never ended"
    );
    assert_eq!(
        demo::ask(&d.home, &["server-version"]).unwrap().trim(),
        switchboard::update::version_line()
    );
    let kept = std::fs::read_to_string(&note).unwrap();
    assert!(kept.contains("Kept across the restart"), "{kept}");
}

/// A stand-in for a demo server an older build started on `socket`: it
/// answers `version` with an old one, and ends on `kill-server`, giving
/// whether it was asked to.
fn old_server(socket: &Path) -> std::thread::JoinHandle<bool> {
    use switchboard::mux::proto::{self, ClientMsg, ServerMsg};
    let listener = UnixListener::bind(socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let socket = socket.to_path_buf();
    std::thread::spawn(move || {
        let deadline = common::deadline(Duration::from_secs(60));
        while Instant::now() < deadline {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            };
            stream.set_nonblocking(false).unwrap();
            let Ok(Some(ClientMsg::Command { args, .. })) = proto::read_client(&mut stream) else {
                continue;
            };
            let (code, stdout) = match args.first().map(String::as_str) {
                Some("server-version") => (0, b"0.0.1 (000000000000)\n".to_vec()),
                Some("kill-server") => (0, Vec::new()),
                _ => (1, Vec::new()),
            };
            let _ = stream.write_all(&proto::encode_server(&ServerMsg::Reply {
                code,
                stdout,
                stderr: Vec::new(),
            }));
            if args.first().is_some_and(|a| a == "kill-server") {
                let _ = std::fs::remove_file(&socket);
                return true;
            }
        }
        false
    })
}
