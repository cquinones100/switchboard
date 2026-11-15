//! The queue for heavy work, through the binary run as `cargo` (the
//! stand-in every pane finds first on its PATH) in a temporary home, with a
//! stand-in for the real cargo further down the PATH that writes what it
//! was asked to a log: with one build slot, three builds run one after
//! another, each with the cores as its jobs; and `cargo test` holds the
//! build slot only while it compiles, so a build goes while its tests run.

use crate::common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use common::{BIN, TempDir};
use serde_json::json;

/// The real cargo: says what it was asked, with `CARGO_BUILD_JOBS`; a
/// compile (anything but a test run) takes a moment and says if another
/// was going at once; a test run waits for `build-done`.
const REAL_CARGO: &str = r#"#!/bin/sh
case " $* " in
  *" test "*)
    case " $* " in
      *" --no-run "*) echo "compile-test" >> "$LOG"; sleep 0.3; exit 0 ;;
    esac
    echo "run-start" >> "$LOG"
    i=0
    while [ ! -e "$DONE" ] && [ $i -lt 600 ]; do sleep 0.1; i=$((i+1)); done
    echo "run-end" >> "$LOG"
    exit 0 ;;
esac
mkdir "$LOG.busy" 2>/dev/null || echo "overlap" >> "$LOG"
echo "start $* jobs=$CARGO_BUILD_JOBS slot=$SWITCHBOARD_SLOT" >> "$LOG"
sleep 0.3
echo "end" >> "$LOG"
rmdir "$LOG.busy"
touch "$DONE"
"#;

struct Home {
    dir: TempDir,
}

impl Home {
    fn new(settings: serde_json::Value) -> Home {
        let dir = TempDir::new("sb-slots");
        let home = dir.path();
        let (bin, real) = (home.join("bin"), home.join("real"));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(BIN, bin.join("cargo")).unwrap();
        let cargo = real.join("cargo");
        std::fs::write(&cargo, REAL_CARGO).unwrap();
        std::fs::set_permissions(&cargo, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        common::write_settings(home, settings);
        Home { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn log(&self) -> PathBuf {
        self.path().join("log")
    }

    fn lines(&self) -> Vec<String> {
        std::fs::read_to_string(self.log())
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    fn cargo(&self, args: &[&str]) -> Child {
        let home = self.path();
        Command::new(home.join("bin/cargo"))
            .args(args)
            .env_clear()
            .env("HOME", home)
            .env("SWITCHBOARD_STATE_DIR", home.join("state"))
            .env(
                "PATH",
                format!(
                    "{}:{}:/usr/bin:/bin",
                    home.join("bin").display(),
                    home.join("real").display()
                ),
            )
            .env("LOG", self.log())
            .env("DONE", home.join("build-done"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }

    fn wait_for(&self, what: &str, pred: impl Fn(&[String]) -> bool) {
        let deadline = common::deadline(Duration::from_secs(20));
        loop {
            let lines = self.lines();
            if pred(&lines) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "waiting for {what}; the log has {lines:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[test]
fn with_one_build_slot_three_builds_run_one_after_another() {
    let home = Home::new(json!({ "slots_build": "1" }));
    let builds: Vec<Child> = (0..3).map(|_| home.cargo(&["build", "--locked"])).collect();
    let mut said_waiting = false;
    for build in builds {
        let out = build.wait_with_output().unwrap();
        assert!(out.status.success(), "{out:?}");
        said_waiting |= String::from_utf8_lossy(&out.stderr).contains("Waiting for a build slot (");
    }
    let lines = home.lines();
    assert!(!lines.iter().any(|l| l == "overlap"), "{lines:?}");
    let cores = std::thread::available_parallelism().unwrap().get();
    let starts: Vec<&String> = lines.iter().filter(|l| l.starts_with("start")).collect();
    assert_eq!(starts.len(), 3, "{lines:?}");
    for start in starts {
        assert_eq!(
            start,
            &format!("start build --locked jobs={cores} slot=build")
        );
    }
    assert!(said_waiting, "a build said it was waiting");
}

#[test]
fn with_the_queue_off_builds_run_at_once_and_untouched() {
    let home = Home::new(json!({ "slot_queue": false }));
    let out = home.cargo(&["build"]).wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(home.lines()[0], "start build jobs= slot=");
}

#[test]
fn cargo_test_holds_the_build_slot_only_while_it_compiles() {
    let home = Home::new(json!({ "slots_build": "1", "slots_tests": "1" }));
    let mut test = home.cargo(&["test", "--", "--nocapture"]);
    home.wait_for("the test run to start", |l| {
        l.iter().any(|l| l == "run-start")
    });
    // The tests wait for this build, which could never start if the test
    // run still held the build slot.
    let build = home.cargo(&["build"]).wait_with_output().unwrap();
    assert!(build.status.success(), "{build:?}");
    assert!(test.wait().unwrap().success());
    let lines = home.lines();
    assert_eq!(
        lines,
        [
            "compile-test",
            "run-start",
            "start build jobs=14 slot=build",
            "end",
            "run-end"
        ]
        .map(|l| l.replace(
            "jobs=14",
            &format!(
                "jobs={}",
                std::thread::available_parallelism().unwrap().get()
            )
        ))
    );
}
