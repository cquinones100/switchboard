//! `switchboard try`, end to end, as bin/switchboard-try was tried by hand:
//! a worktree of a temporary home's own, a stand-in `cargo` that "builds"
//! this test's binary, and the try's home under /tmp, which each test
//! removes with `--clean` however it ends. Nothing reaches your real home or
//! server: the command runs with the temporary home as its HOME and no
//! `SWITCHBOARD`.

use crate::common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use common::stand_in::{self, Claude, sh_quote};
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
        // A cargo as strict as cargo about `cargo build`, which says it
        // built this test's binary.
        stand_in::cargo(
            &bin,
            &format!(
                "touch {built}\necho '{{\"reason\":\"compiler-artifact\",\"target\":{{\"name\":\"switchboard\"}},\"executable\":\"{BIN}\"}}'",
                built = sh_quote(&dir.path().join("built").to_string_lossy())
            ),
        );
        // A stand-in Claude Code, which the try's setup asks its version.
        Claude::new().version("2.1.319").install(&bin);
        write(
            &wt.join("Cargo.toml"),
            "[package]\nname = \"switchboard\"\n",
        );
        // A skill of the worktree's own, which the try's build reads from
        // it (src/assets.rs) in place of what it carries.
        write(&wt.join("assets/skills/wt/SKILL.md"), "the worktree's wt\n");
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
            // Claude Code's managed settings: none, rather than the
            // computer's own.
            .env(
                "SWITCHBOARD_CLAUDE_MANAGED_SETTINGS",
                self._dir.path().join("managed-settings.json"),
            )
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
    /// Ends the try, then fails the test if a stand-in refused a call.
    fn drop(&mut self) {
        let _ = self.try_(&["--clean"]);
        if !std::thread::panicking() {
            stand_in::assert_none_refused(&self.bin);
        }
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
    // The skills are the worktree's, the change under trial.
    assert_eq!(
        fs::read_to_string(plugin.join("skills/wt/SKILL.md")).unwrap(),
        "the worktree's wt\n"
    );
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

/// Marks the worktree as `wtpr --untrusted` marks a PR's: in its own
/// worktree config, which a lone clone has as its own.
fn mark_untrusted(wt: &Path) {
    git(wt, &["config", "extensions.worktreeConfig", "true"]);
    git(
        wt,
        &["config", "--worktree", "switchboard.untrusted", "true"],
    );
}

#[test]
fn someone_elses_pr_runs_only_sealed_whatever_trusted_dirs_says() {
    let s = Setup::new("sbtu");
    mark_untrusted(&s.wt);
    write(
        &s.real.join(".config/switchboard/trusted-dirs"),
        &format!("{}\n", s.wt.display()),
    );
    let out = s.try_(&["--print"]);
    assert_eq!(out.status.code(), Some(1));
    let why = String::from_utf8_lossy(&out.stderr);
    assert!(
        why.contains("whose code runs only sealed: switchboard try --sealed"),
        "{why}"
    );
    assert!(!s.built(), "nothing was built");

    for (args, said) in [
        (&["--run", "true"][..], "--run works only with --sealed"),
        (&["--fetch"][..], "--fetch works only with --sealed"),
        (&["--sealed", "--run"][..], "--run needs a command"),
    ] {
        let out = s.try_(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains(said));
    }
}

#[test]
fn sealed_refuses_on_linux_and_a_sealed_home_is_never_laid_out_with_your_files() {
    let s = Setup::new("sbts");
    if switchboard::platform::sandbox_ready().is_err() {
        // Linux: no sandbox without installing another program, said so,
        // and nothing built or made.
        let out = s.try_(&["--sealed", "--print"]);
        assert_eq!(out.status.code(), Some(1));
        let why = String::from_utf8_lossy(&out.stderr);
        assert!(why.contains("Linux has none"), "{why}");
        assert!(!s.built());
        let home = switchboard::cmd::try_::home_of(Path::new("/tmp"), &s.wt);
        assert!(!home.exists());
    }

    // A home made with --sealed, known by its rules in your cache, which its
    // code can't change: the open try won't lay it out with your files, and
    // --clean takes it and its rules away.
    let home = switchboard::cmd::try_::home_of(Path::new("/tmp"), &s.wt);
    let rules = s.real.join(".cache/switchboard/sealed");
    let name = home.file_name().unwrap().to_string_lossy().into_owned();
    write(&rules.join(format!("{name}.sb")), "(version 1)\n");
    write(&home.join("f"), "made by the try\n");
    let out = s.try_(&["--print"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("was made with --sealed"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!s.built());
    let cleaned = ok(&s.try_(&["--clean"]));
    assert!(cleaned.contains("Removed"), "{cleaned}");
    assert!(!home.exists());
    assert!(!rules.join(format!("{name}.sb")).exists());
}

/// A sealed try's sandbox, probed from inside with `--run`, as
/// test/switchboard-try.test.js probed bin/switchboard-try's. It needs
/// macOS's sandbox-exec and a Rust toolchain rustup finds, which CI's
/// Linux runners haven't: run it on a Mac with
/// `cargo test --test it try_:: -- --ignored`.
#[test]
#[ignore = "needs macOS's sandbox: cargo test --test it try_:: -- --ignored"]
fn a_sealed_try_reads_nothing_of_yours_writes_nothing_of_yours_and_reaches_no_socket_of_yours() {
    if switchboard::platform::sandbox_ready().is_err() {
        return;
    }
    let s = Setup::new("sbtx");
    write(
        &s.wt.join("Cargo.lock"),
        "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n[[package]]\nname = \"switchboard\"\nversion = \"0.1.0\"\n",
    );
    write(
        &s.wt.join("Cargo.toml"),
        "[package]\nname = \"switchboard\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&s.wt.join("src/main.rs"), "fn main() {}\n");
    // Kept by git, so the PR's worktree is a Switchboard clone too.
    write(&s.wt.join("assets/skills/x/SKILL.md"), "# x\n");
    git(&s.wt, &["add", "."]);
    git(&s.wt, &["commit", "-q", "-m", "a crate"]);
    // The PR, checked out untrusted as a worktree of the clone.
    let review = s.real.join(".cache/switchboard/pr-review/o-r/12345");
    fs::create_dir_all(review.parent().unwrap()).unwrap();
    git(
        &s.wt,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "pr-review/12345",
            review.to_str().unwrap(),
        ],
    );
    git(&s.wt, &["config", "extensions.worktreeConfig", "true"]);
    git(
        &review,
        &["config", "--worktree", "switchboard.untrusted", "true"],
    );
    write(&s.real.join("secret.txt"), "a secret\n");
    // A socket of yours, as Notes' or Neovim's, taking commands from
    // whoever connects.
    let sock = s.real.join("s.sock");
    let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    std::thread::spawn(move || for _ in listener.incoming() {});
    // Another try's home, which may link your files.
    let other = PathBuf::from("/tmp/sb-0000beef");
    write(&other.join("f"), "another try\n");

    let probe = format!(
        "cat {real}/secret.txt; echo \"read=$?\"; cat {other}/f; echo \"other=$?\"; touch {wt}/x; echo \"wrote=$?\"; echo \"token=${{GH_TOKEN:-none}}\"; /usr/bin/nc -U {sock} </dev/null; echo \"sock=$?\"; cat ~/.config/switchboard/settings.json",
        real = s.real.display(),
        other = other.display(),
        wt = review.display(),
        sock = sock.display(),
    );
    let out = Command::new(BIN)
        .args(["try", "--sealed"])
        .arg(&review)
        .args(["--run", "/bin/sh", "-c", &probe])
        .current_dir(&s.wt)
        .env("HOME", &s.real)
        .env("GH_TOKEN", "t0ken")
        .env_remove("SWITCHBOARD")
        .env_remove("SWITCHBOARD_PANE")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    let _ = fs::remove_dir_all(&other);
    assert!(
        said.contains("read=1"),
        "{said}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!said.contains("a secret"), "your home unreadable");
    assert!(said.contains("other=1"), "another try's home unreadable");
    assert!(said.contains("wrote=1"), "the worktree read-only");
    assert!(
        said.contains("token=none"),
        "an environment made from nothing"
    );
    assert!(
        !said.contains("sock=0"),
        "a socket of yours can't be connected to"
    );
    let settings: Value = serde_json::from_str(&said[said.find('{').unwrap()..]).unwrap();
    for key in ["github", "linear", "slack_background", "diff_story"] {
        assert_eq!(settings[key], json!(false), "{key} off");
    }
    let cleaned = Command::new(BIN)
        .args(["try", "--clean"])
        .arg(&review)
        .env("HOME", &s.real)
        .output()
        .unwrap();
    assert!(cleaned.status.success());
}

/// A `--run` again on the same worktree takes up the home the last one
/// made rather than making it from nothing, and `--clean` then takes the
/// home and its rules away. Needs macOS's sandbox, as the test above.
#[test]
#[ignore = "needs macOS's sandbox: cargo test --test it try_:: -- --ignored"]
fn a_second_sealed_run_reuses_the_home_the_first_made() {
    if switchboard::platform::sandbox_ready().is_err() {
        return;
    }
    let s = Setup::new("sbtr");
    write(
        &s.wt.join("Cargo.lock"),
        "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n[[package]]\nname = \"switchboard\"\nversion = \"0.1.0\"\n",
    );
    write(
        &s.wt.join("Cargo.toml"),
        "[package]\nname = \"switchboard\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&s.wt.join("src/main.rs"), "fn main() {}\n");
    git(&s.wt, &["add", "."]);
    git(&s.wt, &["commit", "-q", "-m", "a crate"]);
    let home = switchboard::cmd::try_::home_of(Path::new("/tmp"), &s.wt);
    let rules = s.real.join(".cache/switchboard/sealed").join(format!(
        "{}.sb",
        home.file_name().unwrap().to_string_lossy()
    ));
    let run = |what: &str| {
        let out = s.try_(&["--sealed", "--run", "/bin/sh", "-c", what]);
        assert!(out.status.success(), "{out:?}");
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    let first = run("touch \"$HOME/left\"");
    assert!(first.contains("from nothing"), "{first}");
    let second = run("test -e \"$HOME/left\"");
    assert!(!second.contains("from nothing"), "{second}");
    assert!(rules.exists());
    ok(&s.try_(&["--clean"]));
    assert!(!home.exists());
    assert!(!rules.exists());
}
