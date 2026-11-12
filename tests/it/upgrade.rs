//! Restarting the server on a new build in place (`switchboard upgrade`): a
//! server of the test's own (its "new build" the same binary) is given a
//! second tab, a split, options and programs that keep writing, then
//! upgraded; it must keep its pid, every tab, pane, layout and option, and
//! every program running untouched, still its child, still writing, and the
//! terminal attached must stay attached, its keys still reaching the panes.

use crate::common;

use std::time::{Duration, Instant};

use common::TestServer;
use common::stand_in::Release;
use serde_json::Value;
use switchboard::mux::proto::{self, ClientMsg};

/// A program that says its pid, then writes a tick every 100ms.
const TICKER: &str = "echo pid=$$; i=0; while :; do echo tick$i; i=$((i+1)); sleep 0.1; done";

/// The highest tick in a pane's text.
fn last_tick(text: &str) -> Option<u64> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter_map(|w| w.strip_prefix("tick")?.parse().ok())
        .max()
}

/// What a test compares before and after: each session's tabs, in order,
/// with their ids, names, options and whether current or zoomed, and each
/// pane's id, program, size and options.
fn shape(list: &Value) -> Value {
    let sessions: Vec<Value> = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            let tabs: Vec<Value> = s["tabs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| {
                    let panes: Vec<Value> = t["panes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|p| {
                            serde_json::json!([
                                p["id"],
                                p["pid"],
                                p["cols"],
                                p["rows"],
                                p["active"],
                                p["options"]
                            ])
                        })
                        .collect();
                    serde_json::json!([
                        t["id"],
                        t["name"],
                        t["current"],
                        t["zoomed"],
                        t["options"],
                        panes
                    ])
                })
                .collect();
            serde_json::json!([s["name"], tabs])
        })
        .collect();
    Value::Array(sessions)
}

/// Every pane's program's pid.
fn pids(list: &Value) -> Vec<u32> {
    list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["tabs"].as_array().unwrap().iter())
        .flat_map(|t| t["panes"].as_array().unwrap().iter())
        .map(|p| p["pid"].as_u64().expect("a pane's pid") as u32)
        .collect()
}

/// A process's parent, as `ps` says.
fn parent_of(pid: u32) -> Option<u32> {
    let out = std::process::Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only asks whether the process is there.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Waits until a pane has written a tick past `after`.
fn wait_tick(server: &TestServer, pane: &str, after: u64) -> u64 {
    let deadline = common::deadline(Duration::from_secs(10));
    loop {
        let text = server.ok(&["capture-pane", "-t", pane, "-S", "-200"]);
        if let Some(n) = last_tick(&text).filter(|n| *n > after) {
            return n;
        }
        assert!(
            Instant::now() < deadline,
            "{pane} wrote no tick past {after}:\n{text}\nserver log: {}",
            server.log()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn an_upgrade_keeps_every_pane_running_and_the_server_its_pid() {
    let server = TestServer::start();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let out = server.ok(&["new-tab", "-n", "ticker", "--", "sh", "-c", TICKER]);
    let mut ids = out.split_whitespace();
    let (tab, ticker) = (
        ids.next().unwrap().to_string(),
        ids.next().unwrap().to_string(),
    );
    let beside = server
        .ok(&["split", "-t", &ticker, "-R", "-d", "--", "sh", "-c", TICKER])
        .trim()
        .to_string();
    server.ok(&["set", "-t", &tab, "@tab_note", "kept across"]);
    server.ok(&["set", "-t", &beside, "@mine", "too"]);
    client.wait_text("tick3");
    let before = server.list();
    let server_pid = server.pid().unwrap();
    for pid in pids(&before) {
        assert_eq!(
            parent_of(pid),
            Some(server_pid),
            "{pid} is the server's child"
        );
    }
    let ticked = last_tick(&server.ok(&["capture-pane", "-t", &ticker])).unwrap();
    // A paste begun, which the server holds for its end, and a frame half
    // sent as the server hands over: the rest of both follows once it has,
    // and the new build reads them whole.
    client.type_text("\x1b[200~hello");
    let typed = proto::encode_client(&ClientMsg::Input(b"world\x1b[201~ carried over".to_vec()));
    client.write_raw(&typed[..3]);

    let (code, out, err) = server.run(&["upgrade"]);
    assert_eq!(code, 0, "{out}{err}\nserver log: {}", server.log());
    assert!(out.contains("restarted"), "{out}");
    assert!(out.contains("keeping 3 panes in 2 tabs"), "{out}");
    client.write_raw(&typed[3..]);

    // The same process, every program in it its child as before, running.
    let after = server.list();
    assert_eq!(server.pid(), Some(server_pid));
    assert_eq!(
        shape(&after),
        shape(&before),
        "the same tabs, panes and options"
    );
    for pid in pids(&after) {
        assert!(alive(pid), "{pid} still runs");
        assert_eq!(
            parent_of(pid),
            Some(server_pid),
            "{pid} is still the server's child"
        );
    }
    assert!(
        !server.state.join("handover.json").exists(),
        "the handover is removed once read"
    );
    // Each goes on writing, from where it was: what it showed is there too.
    let text = server.ok(&["capture-pane", "-t", &ticker, "-S", "-200"]);
    assert!(text.contains("pid="), "what it showed came back: {text}");
    wait_tick(&server, &ticker, ticked);
    wait_tick(&server, &beside, ticked);

    // The terminal attached before is still attached, drawn to by the new
    // build, its keys (the one half sent too) reaching the pane in front,
    // whose terminal echoes them, and its resizes taken.
    client.wait_for("ticks on the terminal attached before", |s| {
        s.iter().any(|l| last_tick(l).is_some_and(|n| n > ticked))
    });
    client.wait_text("helloworld carried over");
    assert!(client.exit.is_none(), "it was never told to end");
    client.resize(90, 30);
    let deadline = common::deadline(Duration::from_secs(10));
    while server.list()["sessions"][0]["tabs"][1]["panes"][0]["cols"]
        == before["sessions"][0]["tabs"][1]["panes"][0]["cols"]
    {
        assert!(Instant::now() < deadline, "the resize wasn't taken");
        std::thread::sleep(Duration::from_millis(50));
    }

    // A terminal attaching now sees it all.
    let mut fresh = server.attach(100, 30);
    fresh.wait_for("ticks on a terminal attached after", |s| {
        s.iter().any(|l| last_tick(l).is_some_and(|n| n > ticked))
    });
    // And a pane started now is numbered after every one there has been,
    // one closed before the upgrade too.
    let gone = server
        .ok(&["split", "-t", &beside, "-d"])
        .trim()
        .to_string();
    server.ok(&["kill-pane", "-t", &gone]);
    let (code, out, err) = server.run(&["upgrade"]);
    assert_eq!(code, 0, "{out}{err}");
    let (_, pane) = server
        .ok(&["new-tab", "-d"])
        .split_once(' ')
        .map(|(t, p)| (t.to_string(), p.trim().to_string()))
        .unwrap();
    let n = |id: &str| id.trim_start_matches('%').parse::<u32>().unwrap();
    assert!(n(&pane) > n(&gone), "{pane} after {gone}");
    // An upgrade again goes as well.
    let (code, out, err) = server.run(&["upgrade"]);
    assert_eq!(code, 0, "{out}{err}\nserver log: {}", server.log());
    assert_eq!(server.pid(), Some(server_pid));
    let again = server.list();
    assert_eq!(pids(&again).len(), 4);

    // M-p's action, with nothing to build and the same binary on disk,
    // says so in the terminal used last and restarts nothing.
    server.ok(&["press", "upgrade"]);
    fresh.wait_text("Already on the latest build");
    assert_eq!(server.log().matches("Switchboard restarted on").count(), 3);
    assert_eq!(server.pid(), Some(server_pid));
    assert_eq!(pids(&server.list()), pids(&again));
    assert!(client.exit.is_none());
}

/// Puts a copy of the test's binary at `to` and runs it once: a file
/// just written can take longer than the restart's check allows to run
/// the first time (a security agent looks at it, or a busy disk pages it
/// in), and the check refusing it would fail the test.
fn copy_of_bin(to: &std::path::Path) {
    std::fs::copy(common::BIN, to).unwrap();
    let ran = std::process::Command::new(to)
        .arg("--version")
        .output()
        .unwrap();
    assert!(ran.status.success());
}

/// Waits for the message a restart leaves, failing with the server's log,
/// which says why a restart was refused once the message saying so is gone.
fn wait_restarted(server: &TestServer, client: &mut common::TestClient) {
    // A restart writes and runs a binary of 100MB or so, twice when it
    // builds first, each new file looked at by a security agent where
    // there is one.
    let deadline = common::deadline(Duration::from_secs(30));
    while !client
        .screen()
        .iter()
        .any(|l| l.contains("Switchboard restarted on"))
    {
        assert!(
            Instant::now() < deadline,
            "the restart's message didn't show; the server's log:\n{}",
            server.log()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // Put away, so the next restart's wait doesn't take this one's message
    // for its own: a press says nothing in a box while the status line or
    // the sidebar shows the build.
    client.send(b"\x1b");
    client.wait_for("the restart's message put away", |s| {
        !s.iter().any(|l| l.contains("Switchboard restarted on"))
    });
}

/// A stand-in for cargo that "builds" `$1` into the target directory it is
/// given, saying where as cargo's JSON does, or fails as cargo does when
/// the checkout holds `broken`.
const CARGO: &str = r#"#!/bin/sh
echo "$*" > "$CARGO_TARGET_DIR/args"
if [ -e broken ]; then
  echo "   Compiling switchboard v0.0.0" >&2
  echo "error: could not compile \`switchboard\` (bin \"switchboard\") due to 1 previous error" >&2
  exit 101
fi
if [ -e new ]; then
  rm -f "$CARGO_TARGET_DIR/switchboard"
  cp "$(cat new)" "$CARGO_TARGET_DIR/switchboard"
fi
echo "{\"reason\":\"compiler-artifact\",\"target\":{\"name\":\"switchboard\",\"kind\":[\"bin\"]},\"executable\":\"$CARGO_TARGET_DIR/switchboard\"}"
echo "{\"reason\":\"build-finished\",\"success\":true}"
"#;

#[test]
fn m_p_restarts_only_on_a_new_binary() {
    let (server, bin) = TestServer::start_from_copy();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let server_pid = server.pid().unwrap();

    // With nothing to build: the same binary says so, another one in its
    // place is restarted on.
    server.ok(&["press", "upgrade"]);
    client.wait_text("Already on the latest build");
    std::fs::remove_file(&bin).unwrap();
    copy_of_bin(&bin);
    server.ok(&["press", "upgrade"]);
    wait_restarted(&server, &mut client);
    assert_eq!(server.pid(), Some(server_pid));
    assert!(client.exit.is_none());
}

#[test]
fn m_p_builds_the_checkout_and_installs_it_before_restarting() {
    let dir = common::TempDir::new("sbc");
    let checkout = dir.path().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("Cargo.toml"), "").unwrap();
    let (server, bin) = TestServer::start_from_copy_with_env(&[(
        "SWITCHBOARD_CHECKOUT",
        checkout.to_str().unwrap(),
    )]);
    // Not a clone of yours: built only once trusted-dirs lists it.
    let trusted = server.home.join(".config/switchboard/trusted-dirs");
    std::fs::create_dir_all(trusted.parent().unwrap()).unwrap();
    std::fs::write(&trusted, format!("{}\n", checkout.display())).unwrap();
    let target = checkout.join("target");
    std::fs::create_dir_all(&target).unwrap();
    common::stand_in::install(
        &server.home.join("bin/cargo"),
        &CARGO.replace("$CARGO_TARGET_DIR", target.to_str().unwrap()),
    );
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let server_pid = server.pid().unwrap();

    // A binary put in place by hand since the server started is the latest:
    // restarted on with nothing built over it.
    std::fs::remove_file(&bin).unwrap();
    copy_of_bin(&bin);
    server.ok(&["press", "upgrade"]);
    wait_restarted(&server, &mut client);
    assert!(!target.join("args").exists(), "cargo wasn't run");

    // A binary gone from the server's path is built again, not taken for
    // one put there by hand.
    std::fs::remove_file(&bin).unwrap();
    std::fs::write(checkout.join("new"), common::BIN).unwrap();
    server.ok(&["press", "upgrade"]);
    wait_restarted(&server, &mut client);
    assert!(target.join("args").exists(), "the checkout was built");
    assert!(bin.exists(), "the build is in its place again");
    std::fs::remove_file(target.join("args")).unwrap();
    std::fs::remove_file(checkout.join("new")).unwrap();

    // A build that fails says so, and nothing changes.
    std::fs::write(checkout.join("broken"), "").unwrap();
    server.ok(&["press", "upgrade"]);
    client.wait_text("The build failed");
    assert_eq!(
        std::fs::read_to_string(target.join("args")).unwrap().trim(),
        "build --release --locked --message-format=json-render-diagnostics"
    );
    std::fs::remove_file(checkout.join("broken")).unwrap();

    // A build the same as the binary the server runs: nothing to restart on.
    std::fs::write(checkout.join("new"), common::BIN).unwrap();
    server.ok(&["press", "upgrade"]);
    client.wait_text("Already on the latest build");

    // A build that can't take over is put in place, then refused; pressed
    // again, the checkout is built again rather than the refused build
    // taken for one put there by hand.
    let refused = dir.path().join("refused");
    common::stand_in::install(
        &refused,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = server ]; then echo 99; else exec {} \"$@\"; fi\n",
            common::BIN
        ),
    );
    std::fs::write(checkout.join("new"), refused.to_str().unwrap()).unwrap();
    server.ok(&["press", "upgrade"]);
    client.wait_text("format 99");
    std::fs::remove_file(target.join("args")).unwrap();

    // A build that differs is put in place of the server's binary, then
    // restarted on.
    let wrapper = dir.path().join("wrapper");
    common::stand_in::install(
        &wrapper,
        &format!("#!/bin/sh\nexec {} \"$@\"\n", common::BIN),
    );
    std::fs::write(checkout.join("new"), wrapper.to_str().unwrap()).unwrap();
    server.ok(&["press", "upgrade"]);
    wait_restarted(&server, &mut client);
    assert!(target.join("args").exists(), "the checkout was built again");
    assert_eq!(
        std::fs::read(&bin).unwrap(),
        std::fs::read(&wrapper).unwrap(),
        "the build is in place of the server's binary"
    );
    assert_eq!(server.pid(), Some(server_pid));
    assert!(client.exit.is_none());
}

/// What cargo writes at the top of a target directory.
const CARGO_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by cargo.\n";

/// A server started from a copy in a folder cargo marks as its target
/// directory, with a checkout to build whose stand-in cargo "builds" the
/// test's own binary; gives the server, its binary, the checkout's target
/// and the temporary folder holding the checkout.
fn in_a_shared_target_dir() -> (
    TestServer,
    std::path::PathBuf,
    std::path::PathBuf,
    common::TempDir,
) {
    let dir = common::TempDir::new("sbc");
    let checkout = dir.path().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("Cargo.toml"), "").unwrap();
    let (server, bin) = TestServer::start_from_copy_with_env(&[(
        "SWITCHBOARD_CHECKOUT",
        checkout.to_str().unwrap(),
    )]);
    std::fs::write(bin.parent().unwrap().join("CACHEDIR.TAG"), CARGO_TAG).unwrap();
    let trusted = server.home.join(".config/switchboard/trusted-dirs");
    std::fs::create_dir_all(trusted.parent().unwrap()).unwrap();
    std::fs::write(&trusted, format!("{}\n", checkout.display())).unwrap();
    let target = checkout.join("target");
    std::fs::create_dir_all(&target).unwrap();
    common::stand_in::install(
        &server.home.join("bin/cargo"),
        &CARGO.replace("$CARGO_TARGET_DIR", target.to_str().unwrap()),
    );
    std::fs::write(checkout.join("new"), common::BIN).unwrap();
    (server, bin, target, dir)
}

#[test]
fn m_p_moves_a_server_off_a_shared_cargo_target_directory() {
    let (server, bin, target, _dir) = in_a_shared_target_dir();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let server_pid = server.pid().unwrap();

    // With no installed build, the restart lands on a copy in the cache,
    // the build put there.
    server.ok(&["press", "upgrade"]);
    wait_restarted(&server, &mut client);
    assert!(target.join("args").exists(), "the checkout was built");
    let copy = server.home.join(".cache/switchboard/build/switchboard");
    assert_eq!(
        std::fs::read(&copy).unwrap(),
        std::fs::read(common::BIN).unwrap(),
        "the build is in the cache's copy"
    );
    std::fs::remove_file(target.join("args")).unwrap();

    // Another worktree's build written over the target directory's file no
    // longer reaches the restart: the server runs from the copy.
    std::fs::remove_file(&bin).unwrap();
    common::stand_in::install(
        &bin,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = server ]; then echo 99; else exec {} \"$@\"; fi\n",
            common::BIN
        ),
    );
    server.ok(&["press", "upgrade"]);
    client.wait_text("Already on the latest build");
    assert!(target.join("args").exists(), "the checkout was built");
    assert!(!client.screen().iter().any(|l| l.contains("format 99")));
    assert_eq!(server.pid(), Some(server_pid));
    assert!(client.exit.is_none());
}

#[test]
fn m_p_moves_a_server_off_a_target_directory_onto_the_installed_build() {
    let (mut server, _bin, target, _dir) = in_a_shared_target_dir();
    let installed = server.home.join(".local/bin/switchboard");
    std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
    copy_of_bin(&installed);
    // Installed before the server started: the build goes there.
    server.restart();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let server_pid = server.pid().unwrap();

    server.ok(&["press", "upgrade"]);
    wait_restarted(&server, &mut client);
    assert!(target.join("args").exists(), "the checkout was built");
    assert!(
        !server.home.join(".cache/switchboard/build").exists(),
        "no copy was made in the cache"
    );
    assert_eq!(server.pid(), Some(server_pid));
    assert!(client.exit.is_none());
}

#[test]
fn a_build_that_cant_take_over_is_refused_and_nothing_changes() {
    let (server, bin) = TestServer::start_from_copy();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    let ticker = server
        .ok(&["new-tab", "--", "sh", "-c", TICKER])
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_string();
    client.wait_text("tick2");
    let before = server.list();

    // Another build in its place (the copy a link to the test's binary,
    // which is left alone), one that takes over in another format.
    std::fs::remove_file(&bin).unwrap();
    Release::new("9.9.9").handover(99).install(&bin);
    let (code, out, err) = server.run(&["upgrade"]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(
        err.contains("format 99") && err.contains("goes on as it was"),
        "{err}"
    );

    // One from before upgrades, which doesn't know the question.
    Release::new("0.1.0").install(&bin);
    let (code, _, err) = server.run(&["upgrade"]);
    assert_eq!(code, 1);
    assert!(err.contains("before upgrades"), "{err}");

    // Nothing stopped: the same tabs, programs and terminal.
    let after = server.list();
    assert_eq!(shape(&after), shape(&before));
    for pid in pids(&after) {
        assert!(alive(pid), "{pid} still runs");
    }
    let ticked = last_tick(&server.ok(&["capture-pane", "-t", &ticker])).unwrap();
    wait_tick(&server, &ticker, ticked);
    client.wait_for("ticks after the refusal", |s| {
        s.iter().any(|l| last_tick(l).is_some_and(|n| n > ticked))
    });
    assert!(client.exit.is_none(), "the terminal stays attached");
}

#[test]
fn a_picture_given_before_an_upgrade_is_drawn_as_sixel_after_it() {
    let (server, bin) = TestServer::start_from_copy();
    common::write_settings(&server.home, serde_json::json!({"image_protocol": "sixel"}));
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    // A cell of 13 by 25 pixels, as the terminal answered once asked.
    client.wait_sent("the terminal asked for its cell", b"\x1b[16t");
    client.send(b"\x1b[6;25;13t");
    let pane = server.list()["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["set", "-t", &pane, "@allow_passthrough", "1"]);
    let (line, col) = server.show_picture(&mut client, &pane);
    let at = format!("\x1b[{};{}H\x1bP0;1;0q\"1;1;52;48", line + 1, col + 1);
    client.wait_sent("the picture drawn", at.as_bytes());

    let (code, out, err) = server.run(&["upgrade"]);
    assert_eq!(code, 0, "{out}{err}\nserver log: {}", server.log());
    // The program gave the picture once, before: the new build knows it,
    // and the cell its terminal said, and draws it for a terminal
    // attaching now, which says nothing of its cell.
    let mut fresh = server.attach(100, 30);
    fresh.wait_sent("the picture drawn after the upgrade", at.as_bytes());
    fresh.detach();

    // M-p's restart, which builds first (the test's server has no
    // checkout to build) and restarts on the binary put in place since,
    // hands the picture on the same way.
    std::fs::remove_file(&bin).unwrap();
    copy_of_bin(&bin);
    server.ok(&["press", "upgrade"]);
    wait_restarted(&server, &mut client);
    let mut third = server.attach(100, 30);
    third.wait_sent("the picture drawn after M-p's restart", at.as_bytes());
}

#[test]
fn the_new_binary_says_its_version_whatever_its_file_is_called() {
    let dir = common::TempDir::new("sbv");
    let new = dir.path().join(".switchboard.new-1");
    std::fs::copy(common::BIN, &new).unwrap();
    let said = switchboard::update::install::says_version(&new).unwrap();
    assert!(said.starts_with(env!("CARGO_PKG_VERSION")), "{said}");
}

#[test]
fn m_p_restarts_a_target_directory_server_on_a_build_installed_since_as_it_is() {
    let (server, _bin, target, _dir) = in_a_shared_target_dir();
    let mut client = server.attach(100, 30);
    client.wait_text("$");
    // Another worktree's build installed after the server started.
    let installed = server.home.join(".local/bin/switchboard");
    std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
    copy_of_bin(&installed);
    let before = std::fs::metadata(&installed).unwrap().modified().unwrap();

    server.ok(&["press", "upgrade"]);
    wait_restarted(&server, &mut client);
    assert!(!target.join("args").exists(), "nothing was built over it");
    assert_eq!(
        std::fs::metadata(&installed).unwrap().modified().unwrap(),
        before
    );
    assert!(client.exit.is_none());
}

/// Runs git in `dir`, failing the test when it fails.
fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-C"])
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Whether the screen says `text`, a box's wrapped lines read as one.
fn says(client: &mut common::TestClient, text: &str) -> bool {
    let words = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let screen = client
        .screen()
        .iter()
        // A box's borders and shadow are box-drawing and block characters.
        .map(|l| l.replace(|c| ('\u{2500}'..='\u{259f}').contains(&c), " "))
        .collect::<Vec<_>>()
        .join(" ");
    words(&screen).contains(&words(text))
}

#[test]
fn m_p_brings_the_main_clone_up_to_date_and_builds_it() {
    let dir = common::TempDir::new("sbm");
    let origin = dir.path().join("origin.git");
    let main = dir.path().join("main");
    let other = dir.path().join("other");
    git(
        dir.path(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    git(
        dir.path(),
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    std::fs::write(other.join("Cargo.toml"), "").unwrap();
    std::fs::write(other.join(".gitignore"), "target\nnew\n").unwrap();
    git(&other, &["add", "."]);
    git(&other, &["commit", "-q", "-m", "one"]);
    git(&other, &["push", "-q", "origin", "HEAD:main"]);
    git(
        dir.path(),
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            main.to_str().unwrap(),
        ],
    );
    let (server, bin) =
        TestServer::start_from_copy_with_env(&[("SWITCHBOARD_MAIN_CLONE", main.to_str().unwrap())]);
    let trusted = server.home.join(".config/switchboard/trusted-dirs");
    std::fs::create_dir_all(trusted.parent().unwrap()).unwrap();
    std::fs::write(&trusted, format!("{}\n", main.display())).unwrap();
    let target = main.join("target");
    std::fs::create_dir_all(&target).unwrap();
    common::stand_in::install(
        &server.home.join("bin/cargo"),
        &CARGO.replace("$CARGO_TARGET_DIR", target.to_str().unwrap()),
    );
    let wrapper = |name: &str| {
        let path = dir.path().join(name);
        common::stand_in::install(
            &path,
            &format!("#!/bin/sh\n# {name}\nexec {} \"$@\"\n", common::BIN),
        );
        path
    };
    let mut client = server.attach(160, 40);
    client.wait_text("$");

    // Uncommitted changes: said, and nothing built.
    std::fs::write(main.join("file"), "").unwrap();
    server.ok(&["press", "upgrade"]);
    let deadline = common::deadline(Duration::from_secs(10));
    while !says(&mut client, "has uncommitted changes; commit or stash them") {
        assert!(Instant::now() < deadline, "{:?}", client.screen());
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!target.join("args").exists(), "cargo wasn't run");
    std::fs::remove_file(main.join("file")).unwrap();

    // Behind origin: fast-forwarded, then built and restarted on.
    git(&other, &["commit", "-q", "--allow-empty", "-m", "two"]);
    git(&other, &["push", "-q", "origin", "HEAD:main"]);
    std::fs::write(main.join("new"), wrapper("first").to_str().unwrap()).unwrap();
    server.ok(&["press", "upgrade"]);
    wait_restarted(&server, &mut client);
    assert_eq!(git(&main, &["log", "-1", "--format=%s"]), "two");
    assert!(target.join("args").exists(), "the main clone was built");
    assert_eq!(
        std::fs::read(&bin).unwrap(),
        std::fs::read(dir.path().join("first")).unwrap()
    );

    // An origin that can't be reached: built as it is, and said after the
    // restart's message.
    std::fs::rename(&origin, dir.path().join("gone.git")).unwrap();
    std::fs::remove_file(target.join("args")).unwrap();
    std::fs::write(main.join("new"), wrapper("second").to_str().unwrap()).unwrap();
    server.ok(&["press", "upgrade"]);
    let deadline = common::deadline(Duration::from_secs(30));
    while !says(&mut client, "(couldn't fetch origin:") {
        assert!(
            Instant::now() < deadline,
            "{:?}\n{}",
            client.screen(),
            server.log()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(target.join("args").exists(), "the main clone was built");
}

/// A stand-in cargo that builds slowly, a crate every few tenths of a
/// second, as `CARGO` does otherwise.
const SLOW_CARGO: &str = r#"#!/bin/sh
echo "$*" > "$CARGO_TARGET_DIR/args"
for n in 1 2 3 4 5 6 7 8 9 10 11 12; do
  echo "{\"reason\":\"compiler-artifact\",\"target\":{\"name\":\"crate$n\",\"kind\":[\"lib\"]},\"executable\":null}"
  sleep "$(cat pace 2>/dev/null || echo 0.4)"
done
if [ -e broken ]; then
  echo "error[E0308]: mismatched types" >&2
  echo "  --> src/x.rs:1:1" >&2
  exit 101
fi
rm -f "$CARGO_TARGET_DIR/switchboard"
cp "$(cat new)" "$CARGO_TARGET_DIR/switchboard"
echo "{\"reason\":\"compiler-artifact\",\"target\":{\"name\":\"switchboard\",\"kind\":[\"bin\"]},\"executable\":\"$CARGO_TARGET_DIR/switchboard\"}"
"#;

/// The screen's row holding `text`, and the column it starts at, 1-based
/// for a mouse report.
fn spot(screen: &[String], text: &str) -> Option<(usize, usize)> {
    screen
        .iter()
        .enumerate()
        .find_map(|(y, l)| l.find(text).map(|x| (l[..x].chars().count() + 1, y + 1)))
}

#[test]
fn with_ux2_the_restart_builds_in_the_sidebars_row_without_blocking() {
    let dir = common::TempDir::new("sbc");
    let checkout = dir.path().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("Cargo.toml"), "").unwrap();
    let (server, _bin) = TestServer::start_from_copy_with_env(&[(
        "SWITCHBOARD_CHECKOUT",
        checkout.to_str().unwrap(),
    )]);
    common::write_settings(
        &server.home,
        serde_json::json!({ "ux2": true, "tips": false }),
    );
    let trusted = server.home.join(".config/switchboard/trusted-dirs");
    std::fs::create_dir_all(trusted.parent().unwrap()).unwrap();
    std::fs::write(&trusted, format!("{}\n", checkout.display())).unwrap();
    let target = checkout.join("target");
    std::fs::create_dir_all(&target).unwrap();
    common::stand_in::install(
        &server.home.join("bin/cargo"),
        &SLOW_CARGO.replace("$CARGO_TARGET_DIR", target.to_str().unwrap()),
    );
    let mut client = server.attach(120, 30);
    client.wait_text("$");

    // A failing build: the row says so in red until it is clicked, and
    // its steps then show cargo's error.
    std::fs::write(checkout.join("broken"), "").unwrap();
    server.ok(&["press", "upgrade"]);
    let screen = client.wait_text("Building");
    assert!(
        !screen.iter().any(|l| l.contains("Building Switchboard in")),
        "no box opens on the press: {screen:?}"
    );
    // Keys typed while it builds reach the pane.
    client.type_text("echo typed-while-building\r");
    client.wait_for("the typed line's output", |s| {
        s.iter()
            .filter(|l| l.contains("typed-while-building"))
            .count()
            >= 2
    });
    let screen = wait_long(&server, &mut client, "Build failed · click for why");
    assert!(!screen.iter().any(|l| l.contains("The build failed")));
    let (x, y) = spot(&screen, "Build failed").unwrap();
    client.send(format!("\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m").as_bytes());
    let screen = client.wait_text("error[E0308]: mismatched types");
    assert!(
        screen
            .iter()
            .any(|l| l.contains("Keys still reach the tab")),
        "{screen:?}"
    );
    client.send(b"\x1b[<0;60;20M\x1b[<0;60;20m");
    client.wait_for("the steps closed and the row gone", |s| {
        !s.iter()
            .any(|l| l.contains("mismatched types") || l.contains("Build failed"))
    });
    std::fs::remove_file(checkout.join("broken")).unwrap();
    let counts = server.state.join("restart-build.json");
    assert!(!counts.exists(), "a failed build counts nothing");

    // A build that works: a click on its row, or a second press, opens
    // its steps, keys still reach the pane with them up, and after the
    // restart the row and the notifications say so.
    let wrapper = dir.path().join("wrapper");
    common::stand_in::install(
        &wrapper,
        &format!("#!/bin/sh\nexec {} \"$@\"\n", common::BIN),
    );
    // Slow enough that the steps are still up after the typing.
    std::fs::write(checkout.join("pace"), "1.5").unwrap();
    std::fs::write(checkout.join("new"), wrapper.to_str().unwrap()).unwrap();
    server.ok(&["press", "upgrade"]);
    let screen = client.wait_text("Building");
    let (x, y) = spot(&screen, "Building").unwrap();
    client.send(format!("\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m").as_bytes());
    client.wait_text("Keys still reach the tab");
    client.type_text("echo with-steps-up\r");
    // The box covers the pane's top lines, so the pane itself is read.
    let deadline = common::deadline(Duration::from_secs(10));
    while server
        .ok(&["capture-pane"])
        .matches("with-steps-up")
        .count()
        < 2
    {
        assert!(Instant::now() < deadline, "the keys didn't reach the pane");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        client
            .screen()
            .iter()
            .any(|l| l.contains("Keys still reach the tab"))
    );
    client.send(b"\x1b[<0;60;20M\x1b[<0;60;20m");
    client.wait_for("the steps closed", |s| {
        !s.iter().any(|l| l.contains("Keys still reach the tab"))
    });
    server.ok(&["press", "upgrade"]);
    client.wait_text("Keys still reach the tab");
    wait_restarted(&server, &mut client);
    client.wait_text("✓ Restarted on");
    let screen = client.screen();
    assert!(
        screen.iter().filter(|l| l.contains("Restarted on")).count() >= 2,
        "the row and the notification: {screen:?}"
    );
    let counts: Value = serde_json::from_str(&std::fs::read_to_string(&counts).unwrap()).unwrap();
    assert_eq!(counts[checkout.to_str().unwrap()], 13);
}

/// Waits for `text` on the screen as long as a slow stand-in build may
/// take on a busy computer.
fn wait_long(server: &TestServer, client: &mut common::TestClient, text: &str) -> Vec<String> {
    let deadline = common::deadline(Duration::from_secs(30));
    loop {
        let screen = client.screen();
        if screen.iter().any(|l| l.contains(text)) {
            return screen;
        }
        assert!(
            Instant::now() < deadline,
            "{text:?} didn't show: {screen:?}\nthe server's log:\n{}",
            server.log()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
