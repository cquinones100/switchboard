//! `switchboard try`, end to end, as bin/switchboard-try was tried by hand:
//! a worktree of a temporary home's own, a stand-in `cargo` that "builds"
//! this test's binary, and the try's home under /tmp, which each test
//! removes with `--clean` however it ends. Nothing reaches your real home or
//! server: the command runs with the temporary home as its HOME and no
//! `SWITCHBOARD`.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use common::{BIN, TempDir};
use serde_json::{Value, json};

/// A temporary home with a Switchboard worktree of its own in it, and a
/// stand-in cargo that says it built this test's binary.
struct Setup {
    _dir: TempDir,
    real: PathBuf,
    wt: PathBuf,
    bin: PathBuf,
}

impl Setup {
    fn new(prefix: &str) -> Setup {
        let dir = TempDir::new(prefix);
        let real = dir.path().join("real");
        let wt = dir.path().join("wt");
        let bin = dir.path().join("bin");
        for d in [&real, &wt, &bin] {
            fs::create_dir_all(d).unwrap();
        }
        let cargo = bin.join("cargo");
        fs::write(
            &cargo,
            format!(
                "#!/bin/sh\ntouch {built}\necho '{{\"reason\":\"compiler-artifact\",\"target\":{{\"name\":\"switchboard\"}},\"executable\":\"{BIN}\"}}'\n",
                built = dir.path().join("built").display()
            ),
        )
        .unwrap();
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
        // A stand-in Claude Code, which the try's setup asks its version.
        let claude = bin.join("claude");
        fs::write(&claude, "#!/bin/sh\necho '2.1.319 (Claude Code)'\n").unwrap();
        fs::set_permissions(&claude, fs::Permissions::from_mode(0o755)).unwrap();
        write(
            &wt.join("Cargo.toml"),
            "[package]\nname = \"switchboard\"\n",
        );
        fs::create_dir_all(wt.join("assets/skills")).unwrap();
        write(&wt.join("src/mux/mod.rs"), "\n");
        git(&wt, &["init", "-q"]);
        git(&wt, &["config", "user.email", "me@example.com"]);
        git(&wt, &["config", "user.name", "Me"]);
        git(&wt, &["add", "."]);
        git(&wt, &["commit", "-q", "-m", "first"]);
        // Some of your own, to be linked or copied.
        write(&real.join(".gitconfig"), "[user]\n");
        write(&real.join(".config/gh/hosts.yml"), "github.com:\n");
        write(
            &real.join(".config/switchboard/settings.json"),
            "{\"github\": false}\n",
        );
        write(
            &real.join(".claude/settings.json"),
            "{\"model\": \"opus\"}\n",
        );
        Setup {
            _dir: dir,
            real,
            wt,
            bin,
        }
    }

    fn try_(&self, args: &[&str]) -> Output {
        let path = std::env::var("PATH").unwrap_or_default();
        Command::new(BIN)
            .arg("try")
            .args(args)
            .current_dir(&self.wt)
            .stdin(Stdio::null())
            .env("HOME", &self.real)
            .env("PATH", format!("{}:{path}", self.bin.display()))
            .env("SHELL", "/bin/sh")
            .env("ENV", "")
            .env_remove("SWITCHBOARD")
            .env_remove("SWITCHBOARD_PANE")
            .env_remove("SWITCHBOARD_STATE_DIR")
            .env_remove("SWITCHBOARD_SETTINGS")
            .output()
            .expect("switchboard runs")
    }

    fn built(&self) -> bool {
        self._dir.path().join("built").exists()
    }

    /// The try's home: the one line of `--print` that names it.
    fn home(out: &Output) -> PathBuf {
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text
            .lines()
            .find_map(|l| l.strip_prefix("export HOME="))
            .unwrap_or_else(|| panic!("no HOME in {text}"));
        PathBuf::from(line.trim_matches('\''))
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = self.try_(&["--clean"]);
    }
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

fn ok(out: &Output) -> String {
    assert!(
        out.status.success(),
        "switchboard try failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn print_sets_a_home_up_and_clean_takes_it_away() {
    let s = Setup::new("sbtp");
    let out = s.try_(&["--print"]);
    let text = ok(&out);
    assert!(s.built(), "cargo was asked to build it");
    let home = Setup::home(&out);
    assert!(home.starts_with("/tmp"), "{home:?}");
    assert!(
        text.contains("unset SWITCHBOARD SWITCHBOARD_PANE"),
        "{text}"
    );
    let path = text
        .lines()
        .find_map(|l| l.strip_prefix("export PATH="))
        .unwrap_or_else(|| panic!("no PATH in {text}"));
    assert!(
        path.trim_start_matches('\'')
            .starts_with(&format!("{}/.local/bin:", home.display())),
        "{text}"
    );

    // Its own switchboard, the worktree, and yours linked.
    assert_eq!(
        fs::read_link(home.join(".local/bin/switchboard")).unwrap(),
        PathBuf::from(BIN)
    );
    assert_eq!(fs::read_link(home.join("switchboard")).unwrap(), s.wt);
    assert_eq!(
        fs::read_link(home.join(".gitconfig")).unwrap(),
        s.real.join(".gitconfig")
    );
    assert_eq!(
        fs::read_link(home.join(".config/gh")).unwrap(),
        s.real.join(".config/gh")
    );
    // Your settings copied, with what would duplicate your workspace off.
    let settings: Value = serde_json::from_str(
        &fs::read_to_string(home.join(".config/switchboard/settings.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(settings["github"], json!(false));
    assert_eq!(settings["meeting_notes"], json!(false));
    assert_eq!(settings["update_check"], json!("off"));
    // Its setup ran there: the try has its own plugin, with the hooks and
    // skills its panes' Claude loads, and its Claude settings carry the
    // statusline; yours are as they were.
    let claude = fs::read_to_string(home.join(".claude/settings.json")).unwrap();
    assert!(claude.contains("switchboard statusline"), "{claude}");
    assert!(!claude.contains("switchboard hook"), "{claude}");
    assert_eq!(
        fs::read_to_string(s.real.join(".claude/settings.json")).unwrap(),
        "{\"model\": \"opus\"}\n"
    );
    let plugin = home.join(".local/share/switchboard/plugin");
    assert!(plugin.join("skills/wt/SKILL.md").is_file());
    assert!(
        fs::read_to_string(plugin.join("hooks/hooks.json"))
            .unwrap()
            .contains("switchboard hook Stop")
    );
    assert!(!home.join(".claude/skills/wt").exists());
    assert!(!s.real.join(".claude/skills").exists());
    assert!(!s.real.join(".local/share/switchboard").exists());

    let cleaned = ok(&s.try_(&["--clean"]));
    assert!(cleaned.contains("Removed"), "{cleaned}");
    assert!(!home.exists());
    assert!(s.real.join(".gitconfig").is_file(), "yours is left alone");
    assert!(ok(&s.try_(&["--clean"])).contains("Nothing to clean"));
}

#[test]
fn someone_elses_commits_are_refused_until_the_worktree_is_listed() {
    let s = Setup::new("sbtr");
    write(&s.wt.join("x"), "theirs\n");
    git(&s.wt, &["add", "x"]);
    git(
        &s.wt,
        &[
            "-c",
            "user.email=them@example.com",
            "commit",
            "-q",
            "-m",
            "theirs",
        ],
    );
    let out = s.try_(&["--print"]);
    assert!(!out.status.success());
    let why = String::from_utf8_lossy(&out.stderr);
    assert!(why.contains("aren't yours"), "{why}");
    assert!(why.contains("trusted-dirs"), "{why}");
    assert!(!s.built(), "nothing was built");

    write(
        &s.real.join(".config/switchboard/trusted-dirs"),
        &format!("{}\n", s.wt.display()),
    );
    ok(&s.try_(&["--print"]));
    assert!(s.built());
}

#[test]
fn detached_starts_a_server_its_prefix_reaches() {
    let s = Setup::new("sbtd");
    let text = ok(&s.try_(&["--detached"]));
    let prefix = text
        .lines()
        .find_map(|l| l.trim().strip_suffix(" list"))
        .unwrap_or_else(|| panic!("no prefix in {text}"))
        .to_string();
    let out = Command::new("sh")
        .args(["-c", &format!("{prefix} list")])
        .env_remove("SWITCHBOARD")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let list = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let state: Value = serde_json::from_str(&list).expect("list gives JSON");
    assert!(
        !state["sessions"].as_array().unwrap().is_empty(),
        "a session with no client: {list}"
    );
    let cleaned = ok(&s.try_(&["--clean"]));
    assert!(cleaned.contains("stopped its server"), "{cleaned}");
}
