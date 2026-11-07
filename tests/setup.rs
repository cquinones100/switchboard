//! `switchboard setup`'s parts, which must be safe to run again, ported from
//! test/setup.test.js's Claude parts, and `switchboard statusline`, from
//! what claude/statusline-command.sh did. Each runs in a temporary HOME, so
//! nothing reaches your own ~/.claude or ~/.config.

mod common;

use std::fs;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use common::{BIN, TempDir};

/// A home of the test's own, with `bin` for its stand-ins for programs.
struct Home {
    _dir: TempDir,
    home: PathBuf,
}

impl Home {
    fn new(prefix: &str) -> Home {
        let dir = TempDir::new(prefix);
        let home = dir.path().join("home");
        fs::create_dir_all(home.join("bin")).unwrap();
        Home { _dir: dir, home }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.home.join(rel)
    }

    fn command(&self, program: &str) -> Command {
        let mut cmd = Command::new(program);
        let path = std::env::var("PATH").unwrap_or_default();
        cmd.env("HOME", &self.home)
            .env(
                "PATH",
                format!("{}:{path}", self.home.join("bin").display()),
            )
            .env(
                "SWITCHBOARD_STATE_DIR",
                self.home.join(".local/state/switchboard"),
            )
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@e")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@e")
            .env_remove("SWITCHBOARD")
            .env_remove("SWITCHBOARD_PANE")
            .env_remove("SWITCHBOARD_SETTINGS")
            .env_remove("SWITCHBOARD_PULL_ONLY")
            // The terminal and the services the guided setup looks at are
            // the test's: none.
            .env_remove("TERM_PROGRAM")
            .env_remove("LC_TERMINAL")
            .env_remove("KITTY_WINDOW_ID")
            .env_remove("ALACRITTY_WINDOW_ID")
            .env_remove("ALACRITTY_SOCKET")
            .env_remove("XTERM_VERSION")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("LINEAR_API_KEY");
        cmd
    }

    /// `switchboard <args>`, its stdin closed.
    fn run(&self, args: &[&str]) -> Output {
        self.command(BIN)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    /// `switchboard <args>` given `input` on stdin.
    fn run_with(&self, args: &[&str], input: &str) -> Output {
        let mut child = self
            .command(BIN)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    /// A new repository on `branch`, without `init -b`, which older git
    /// lacks.
    fn init(&self, dir: &Path, branch: &str) {
        let d = dir.to_str().unwrap();
        self.git(&["init", "-q", d]);
        let head = format!("refs/heads/{branch}");
        self.git(&["-C", d, "symbolic-ref", "HEAD", &head]);
    }

    fn git(&self, args: &[&str]) {
        let out = self.command("git").args(args).output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A program on the test's PATH: `switchboard` linked to the binary,
    /// anything else a script.
    fn stand_in(&self, name: &str, script: &str) {
        let file = self.path("bin").join(name);
        fs::write(&file, script).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn ok(out: &Output) -> String {
    assert!(
        out.status.success(),
        "failed: {}\n{}",
        stdout(out),
        stderr(out)
    );
    stdout(out)
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn backups(dir: &Path, prefix: &str) -> usize {
    fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
        .count()
}

fn product() -> Value {
    serde_json::from_str(&read(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/claude-settings.json"),
    ))
    .unwrap()
}

#[test]
fn claude_merges_switchboard_s_settings_into_yours_once_and_makes_a_linked_file_your_own() {
    let h = Home::new("setup-claude");
    let file = h.path(".claude/settings.json");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    // Yours: a model, a permission, a hook of your own beside the tmux
    // version's, and a statusline of your own.
    let yours = json!({
        "model": "sonnet",
        "permissions": { "allow": ["Bash(make *)", "Bash(git log *)"] },
        "statusLine": { "type": "command", "command": "my-statusline" },
        "hooks": { "Stop": [{ "hooks": [
            { "type": "command", "command": "say done" },
            { "type": "command", "command": "bash ~/.config/tmux/agent-state.sh set done" }
        ] }] }
    });
    fs::write(&file, yours.to_string()).unwrap();

    let first = h.run(&["setup", "claude"]);
    assert!(ok(&first).contains(
        "add Switchboard's hooks, statusline and permissions to ~/.claude/settings.json"
    ));
    let merged: Value = serde_json::from_str(&read(&file)).unwrap();
    let product = product();
    assert_eq!(merged["model"], "sonnet");
    assert_eq!(
        merged["statusLine"], yours["statusLine"],
        "a statusline of your own stays"
    );
    let mut allow = vec![json!("Bash(make *)"), json!("Bash(git log *)")];
    allow.extend(
        product["permissions"]["allow"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| *p != "Bash(git log *)")
            .cloned(),
    );
    assert_eq!(
        merged["permissions"]["allow"],
        Value::Array(allow),
        "yours first, each once"
    );
    assert_eq!(
        merged["permissions"]["deny"],
        product["permissions"]["deny"]
    );
    let mut stop = vec![json!({ "hooks": [{ "type": "command", "command": "say done" }] })];
    stop.extend(product["hooks"]["Stop"].as_array().unwrap().iter().cloned());
    assert_eq!(
        merged["hooks"]["Stop"],
        Value::Array(stop),
        "yours kept, the old one replaced"
    );
    assert_eq!(
        merged["hooks"]["PreToolUse"],
        product["hooks"]["PreToolUse"]
    );
    assert_eq!(
        backups(&h.path(".claude"), "settings.json.bak."),
        1,
        "a copy of yours beside it"
    );

    assert_eq!(
        stdout(&h.run(&["setup", "--check", "claude"])),
        "claude: up to date\n"
    );
    assert_eq!(ok(&h.run(&["setup", "claude"])), "claude: up to date\n");
    assert_eq!(
        backups(&h.path(".claude"), "settings.json.bak."),
        1,
        "nothing written again"
    );

    // A settings file linked into a clone, as the whole setup made it,
    // becomes yours, the clone's file untouched.
    let clone = h.path("switchboard/claude");
    fs::create_dir_all(&clone).unwrap();
    fs::write(clone.join("settings.json"), "{\"model\":\"opus\"}\n").unwrap();
    fs::remove_file(&file).unwrap();
    std::os::unix::fs::symlink(clone.join("settings.json"), &file).unwrap();
    let linked = ok(&h.run(&["setup", "claude"]));
    assert!(
        linked.contains("make ~/.claude/settings.json your own"),
        "{linked}"
    );
    assert!(
        !fs::symlink_metadata(&file)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(read(&clone.join("settings.json")), "{\"model\":\"opus\"}\n");
    let own: Value = serde_json::from_str(&read(&file)).unwrap();
    assert_eq!(own["model"], "opus");
    assert_eq!(own["statusLine"]["command"], "switchboard statusline");

    // Linked into a clone pulled since the repository dropped the file, it
    // is taken from the clone's history, as it was before the removal.
    let old = h.path("old-clone");
    h.init(&old, "main");
    let o = old.to_str().unwrap();
    fs::create_dir_all(old.join("claude")).unwrap();
    fs::write(old.join("claude/settings.json"), "{\"model\":\"haiku\"}\n").unwrap();
    h.git(&["-C", o, "add", "claude/settings.json"]);
    h.git(&["-C", o, "commit", "-q", "-m", "settings"]);
    h.git(&["-C", o, "rm", "-q", "claude/settings.json"]);
    h.git(&["-C", o, "commit", "-q", "-m", "removed"]);
    fs::remove_file(&file).unwrap();
    std::os::unix::fs::symlink(old.join("claude/settings.json"), &file).unwrap();
    let dangling = ok(&h.run(&["setup", "claude"]));
    assert!(
        dangling.contains("make ~/.claude/settings.json your own"),
        "{dangling}"
    );
    let own: Value = serde_json::from_str(&read(&file)).unwrap();
    assert_eq!(own["model"], "haiku");
    assert_eq!(own["statusLine"]["command"], "switchboard statusline");

    // A file that isn't JSON is left alone, and said.
    fs::write(&file, "{ not json").unwrap();
    let bad = h.run(&["setup", "claude"]);
    assert!(!bad.status.success());
    assert!(stderr(&bad).contains("isn't a JSON object; left alone"));
    assert_eq!(read(&file), "{ not json");
}

#[test]
fn claude_md_imports_your_rules_from_a_file_or_a_repository_and_switchboard_s_part_once() {
    let h = Home::new("setup-md");
    let file = h.path(".claude/CLAUDE.md");
    let config = h.path(".config/switchboard/claude.json");
    let run = |args: &[&str]| h.run(args);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::create_dir_all(config.parent().unwrap()).unwrap();

    // A CLAUDE.md of your own, no rules named: only the import is added,
    // once, and Switchboard's part written where it points.
    fs::write(&file, "# Mine\n\n- be brief\n").unwrap();
    ok(&run(&["setup", "claude-md"]));
    assert_eq!(
        read(&file),
        "# Mine\n\n- be brief\n\n@~/.local/share/switchboard/switchboard.md\n"
    );
    assert!(read(&h.path(".local/share/switchboard/switchboard.md")).contains("switchboard tabs"));
    assert_eq!(ok(&run(&["setup", "claude_md"])), "claude-md: up to date\n");

    // The tmux version's import is pointed at the new one.
    fs::write(
        &file,
        "# Mine\n@~/switchboard/claude/switchboard.md\n- more\n",
    )
    .unwrap();
    let pointed = ok(&run(&["setup", "claude-md"]));
    assert!(
        pointed.contains("point your ~/.claude/CLAUDE.md's import"),
        "{pointed}"
    );
    assert_eq!(
        read(&file),
        "# Mine\n@~/.local/share/switchboard/switchboard.md\n- more\n"
    );

    // Rules in a file named in claude.json: your old CLAUDE.md is moved
    // aside, and the new one imports both.
    fs::write(h.path("my-rules.md"), "# My rules\n").unwrap();
    fs::write(&config, r#"{"path": "~/my-rules.md"}"#).unwrap();
    let named = ok(&run(&["setup", "claude-md"]));
    assert!(
        named.contains("move your ~/.claude/CLAUDE.md aside"),
        "{named}"
    );
    let written = read(&file);
    assert!(
        written.starts_with("<!-- Written by Switchboard's setup"),
        "{written}"
    );
    assert!(
        written.ends_with("\n\n@~/my-rules.md\n@~/.local/share/switchboard/switchboard.md\n"),
        "{written}"
    );
    assert_eq!(backups(&h.path(".claude"), "CLAUDE.md.bak."), 1);
    assert_eq!(ok(&run(&["setup", "claude-md"])), "claude-md: up to date\n");

    // Rules in a repository: cloned, and its file imported.
    let work = h.path("rules-work");
    let origin = h.path("rules.git");
    h.init(&work, "main");
    fs::write(work.join("CLAUDE.md"), "# From my repo\n").unwrap();
    let w = work.to_str().unwrap();
    h.git(&["-C", w, "add", "."]);
    h.git(&["-C", w, "commit", "-q", "-m", "rules"]);
    h.git(&["clone", "-q", "--bare", w, origin.to_str().unwrap()]);
    fs::write(&config, json!({ "repo": origin }).to_string()).unwrap();
    ok(&run(&["setup", "claude-md"]));
    let clone = h.path(".local/share/switchboard/claude-md");
    assert!(read(&file).ends_with(
        "\n@~/.local/share/switchboard/claude-md/CLAUDE.md\n@~/.local/share/switchboard/switchboard.md\n"
    ));
    assert_eq!(read(&clone.join("CLAUDE.md")), "# From my repo\n");
    assert_eq!(
        backups(&h.path(".claude"), "CLAUDE.md.bak."),
        1,
        "its own file isn't moved aside"
    );

    // One run at a time: a run finding the lock held waits for it.
    let lock = h.path(".local/state/switchboard/claude-md.lock");
    fs::create_dir_all(lock.parent().unwrap()).unwrap();
    let held = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock)
        .unwrap();
    // SAFETY: flock on a descriptor the test owns.
    assert_eq!(unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX) }, 0);
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1500));
        drop(held);
    });
    let started = Instant::now();
    ok(&run(&["setup", "claude-md"]));
    assert!(
        started.elapsed() > Duration::from_millis(1200),
        "waited for the holder"
    );
    releaser.join().unwrap();

    // The update check's run only pulls: a repository not cloned yet is left.
    fs::remove_dir_all(&clone).unwrap();
    let pull_only = h
        .command(BIN)
        .args(["setup", "claude-md"])
        .env("SWITCHBOARD_PULL_ONLY", "true")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    ok(&pull_only);
    assert!(!clone.exists());
    ok(&run(&["setup", "claude-md"]));

    // Another repository: the old clone is moved aside, the new one cloned.
    fs::write(work.join("CLAUDE.md"), "# From my other repo\n").unwrap();
    h.git(&["-C", w, "commit", "-q", "-am", "other"]);
    let other = h.path("other.git");
    let o = other.to_str().unwrap();
    h.git(&["clone", "-q", "--bare", w, o]);
    fs::write(&config, json!({ "repo": other }).to_string()).unwrap();
    ok(&run(&["setup", "claude-md"]));
    assert_eq!(read(&clone.join("CLAUDE.md")), "# From my other repo\n");
    assert_eq!(
        backups(&h.path(".local/share/switchboard"), "claude-md.bak."),
        1
    );

    // A repository keeping it in a folder: the one nested least deep.
    h.git(&["-C", w, "rm", "-q", "CLAUDE.md"]);
    fs::create_dir_all(work.join("claude/deeper")).unwrap();
    fs::write(work.join("claude/CLAUDE.md"), "# Nested\n").unwrap();
    fs::write(work.join("claude/deeper/CLAUDE.md"), "# Deeper\n").unwrap();
    h.git(&["-C", w, "add", "."]);
    h.git(&["-C", w, "commit", "-q", "-m", "nested"]);
    h.git(&["-C", w, "push", "-q", o, "main"]);
    ok(&run(&["setup", "claude-md"]));
    assert!(read(&file).contains("\n@~/.local/share/switchboard/claude-md/claude/CLAUDE.md\n"));

    // Two at that depth: which is yours, asked for, and the file left as it was.
    let before = read(&file);
    fs::create_dir_all(work.join("other")).unwrap();
    fs::write(work.join("other/CLAUDE.md"), "# Another\n").unwrap();
    h.git(&["-C", w, "add", "."]);
    h.git(&["-C", w, "commit", "-q", "-m", "two"]);
    h.git(&["-C", w, "push", "-q", o, "main"]);
    let two = run(&["setup", "claude-md"]);
    assert!(!two.status.success());
    assert!(
        stderr(&two).contains(
            "more than one CLAUDE.md (claude/CLAUDE.md, other/CLAUDE.md): name yours with \"file\""
        ),
        "{}",
        stderr(&two)
    );
    assert_eq!(read(&file), before);
    fs::write(
        &config,
        json!({ "repo": other, "file": "other/CLAUDE.md" }).to_string(),
    )
    .unwrap();
    ok(&run(&["setup", "claude-md"]));
    assert!(read(&file).contains("claude-md/other/CLAUDE.md\n"));

    // A file named that isn't there: said, not imported as nothing.
    fs::write(
        &config,
        json!({ "repo": other, "file": "gone.md" }).to_string(),
    )
    .unwrap();
    assert!(
        stderr(&run(&["setup", "claude-md"]))
            .contains("your rules aren't at ~/.local/share/switchboard/claude-md/gone.md")
    );

    // A folder named with accents and quotes, which git would print escaped.
    h.git(&["-C", w, "rm", "-q", "-r", "claude", "other"]);
    let odd = "résumé \"notes\"";
    fs::create_dir_all(work.join(odd)).unwrap();
    fs::write(work.join(odd).join("CLAUDE.md"), "# Odd\n").unwrap();
    h.git(&["-C", w, "add", "."]);
    h.git(&["-C", w, "commit", "-q", "-m", "odd"]);
    h.git(&["-C", w, "push", "-q", o, "main"]);
    fs::write(&config, json!({ "repo": other }).to_string()).unwrap();
    ok(&run(&["setup", "claude-md"]));
    assert!(read(&file).contains(&format!("claude-md/{odd}/CLAUDE.md\n")));

    // None at all (a folder named CLAUDE.md holds no rules): said.
    h.git(&["-C", w, "rm", "-q", "-r", odd]);
    fs::create_dir_all(work.join("CLAUDE.md")).unwrap();
    fs::write(work.join("CLAUDE.md/README.md"), "not rules\n").unwrap();
    h.git(&["-C", w, "add", "."]);
    h.git(&["-C", w, "commit", "-q", "-m", "none"]);
    h.git(&["-C", w, "push", "-q", o, "main"]);
    let none = run(&["setup", "claude-md"]);
    assert!(!none.status.success());
    assert!(stderr(&none).contains("no CLAUDE.md in your rules' repository"));

    // The guided setup lists it as left to do, with Claude Code there.
    h.stand_in("claude", "#!/bin/sh\n");
    let guided = run(&["setup", "--check"]);
    assert!(
        stdout(&guided).contains("✗ your CLAUDE.md: no CLAUDE.md in your rules' repository"),
        "{}",
        stdout(&guided)
    );
}

#[test]
fn claude_md_adds_gh_as_a_credential_helper_of_a_rules_clone_on_github_once() {
    let h = Home::new("setup-md-gh");
    // A space in gh's path, which the helper has to quote.
    let bin = h.path("my bin");
    fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    fs::write(
        &gh,
        format!(
            "#!/bin/sh\n[ \"$1 $2\" = \"auth token\" ] && [ -e \"{}/SIGNED_IN\" ]\n",
            h.home.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    let url = "https://gist.github.com/abc123.git";
    fs::create_dir_all(h.path(".config/switchboard")).unwrap();
    fs::write(
        h.path(".config/switchboard/claude.json"),
        json!({ "repo": url }).to_string(),
    )
    .unwrap();
    let clone = h.path(".local/share/switchboard/claude-md");
    let c = clone.to_str().unwrap();
    h.init(&clone, "main");
    fs::write(clone.join("CLAUDE.md"), "# Rules\n").unwrap();
    h.git(&["-C", c, "add", "."]);
    h.git(&["-C", c, "commit", "-q", "-m", "rules"]);
    h.git(&["-C", c, "remote", "add", "origin", url]);
    h.git(&["-C", c, "config", "credential.helper", "osxkeychain"]);
    // No network: the clone's update goes to a proxy that isn't there.
    let run = |args: &[&str]| {
        let path = std::env::var("PATH").unwrap_or_default();
        h.command(BIN)
            .args(args)
            .env("PATH", format!("{}:{path}", bin.display()))
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "http.proxy")
            .env("GIT_CONFIG_VALUE_0", "http://127.0.0.1:9")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let helpers = || {
        let out = h
            .command("git")
            .args([
                "-C",
                c,
                "config",
                "--local",
                "--get-all",
                "credential.helper",
            ])
            .output()
            .unwrap();
        stdout(&out)
    };

    run(&["setup", "claude-md"]);
    assert_eq!(
        helpers(),
        "osxkeychain\n",
        "not signed in to gh: the clone's own kept"
    );

    fs::write(h.path("SIGNED_IN"), "").unwrap();
    assert!(
        stdout(&run(&["setup", "--check", "claude-md"]))
            .contains("add gh as a credential helper for your CLAUDE.md's repository")
    );
    ok(&run(&["setup", "claude-md"]));
    assert_eq!(
        helpers(),
        format!("osxkeychain\n!'{}' auth git-credential\n", gh.display()),
        "gh's, quoted, after the clone's own"
    );
    assert!(!stdout(&run(&["setup", "claude-md"])).contains("credential helper"));
}

#[test]
fn skills_are_written_keeping_one_of_your_own_and_brought_up_to_date() {
    let h = Home::new("setup-skills");
    let skills = h.path(".claude/skills");
    let shipped: Vec<String> =
        fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/skills"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
    // Your own wt, a shape linked from a clone by its old name, a
    // note-watch of your own linked from dotfiles laid out the same way,
    // and a today linked relatively.
    fs::create_dir_all(skills.join("wt")).unwrap();
    fs::write(skills.join("wt/SKILL.md"), "mine\n").unwrap();
    std::os::unix::fs::symlink(
        "/somewhere/editor-configs/claude/skills/shape",
        skills.join("shape"),
    )
    .unwrap();
    fs::create_dir_all(h.path("dotfiles/claude/skills/note-watch")).unwrap();
    std::os::unix::fs::symlink(
        h.path("dotfiles/claude/skills/note-watch"),
        skills.join("note-watch"),
    )
    .unwrap();
    fs::create_dir_all(h.path("dotfiles/claude/skills/today")).unwrap();
    std::os::unix::fs::symlink("../../dotfiles/claude/skills/today", skills.join("today")).unwrap();

    let check = stdout(&h.run(&["setup", "--check", "skills"]));
    assert!(
        check.contains("would: write ~/.claude/skills/review-loop"),
        "{check}"
    );
    assert!(
        !skills.join("review-loop").exists(),
        "--check changes nothing"
    );

    let first = ok(&h.run(&["setup", "skills"]));
    assert!(
        first.contains("replace the link ~/.claude/skills/shape"),
        "{first}"
    );
    for name in shipped
        .iter()
        .filter(|n| !["wt", "note-watch", "today"].contains(&n.as_str()))
    {
        let dir = skills.join(name);
        assert!(
            !fs::symlink_metadata(&dir).unwrap().file_type().is_symlink(),
            "{name}"
        );
        assert_eq!(
            read(&dir.join("SKILL.md")),
            read(
                &Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("assets/skills")
                    .join(name)
                    .join("SKILL.md")
            )
        );
        assert!(dir.join(".switchboard-skill").is_file());
    }
    assert_eq!(
        read(&skills.join("wt/SKILL.md")),
        "mine\n",
        "your own is kept"
    );
    assert_eq!(
        fs::read_link(skills.join("note-watch")).unwrap(),
        h.path("dotfiles/claude/skills/note-watch")
    );
    assert_eq!(
        fs::read_link(skills.join("today")).unwrap(),
        Path::new("../../dotfiles/claude/skills/today")
    );
    assert!(
        skills.join("walkthrough/scripts/record.js").is_file(),
        "the scripts that stay with their skills"
    );
    assert!(!skills.join("today/scripts").exists());
    assert_eq!(ok(&h.run(&["setup", "skills"])), "skills: up to date\n");

    // One changed by hand, a file a newer version no longer ships, and the
    // packages a skill installed for itself: the first two put right, the
    // packages left.
    let review = skills.join("review-loop");
    fs::write(review.join("SKILL.md"), "edited\n").unwrap();
    fs::write(review.join("old.md"), "old\n").unwrap();
    let mut list = read(&review.join(".switchboard-skill"));
    list.push_str("old.md\n");
    fs::write(review.join(".switchboard-skill"), list).unwrap();
    fs::create_dir_all(skills.join("walkthrough/scripts/node_modules/x")).unwrap();
    fs::write(
        skills.join("walkthrough/scripts/node_modules/x/index.js"),
        "",
    )
    .unwrap();
    let again = ok(&h.run(&["setup", "skills"]));
    assert!(
        again.contains("bring ~/.claude/skills/review-loop up to date (1 file written, 1 no longer shipped removed)"),
        "{again}"
    );
    assert!(!review.join("old.md").exists());
    assert!(
        skills
            .join("walkthrough/scripts/node_modules/x/index.js")
            .exists()
    );
    assert_eq!(ok(&h.run(&["setup", "skills"])), "skills: up to date\n");

    // One that can't be written doesn't stop the rest, and is said at the end.
    fs::remove_dir_all(&review).unwrap();
    fs::remove_dir_all(skills.join("wt")).unwrap();
    fs::set_permissions(&skills, fs::Permissions::from_mode(0o555)).unwrap();
    let probe = fs::create_dir(skills.join("probe")).is_ok();
    let failed = h.run(&["setup", "skills"]);
    fs::set_permissions(&skills, fs::Permissions::from_mode(0o755)).unwrap();
    if probe {
        eprintln!("skipped the failure: this user writes anywhere");
        return;
    }
    assert!(!failed.status.success());
    assert!(
        stderr(&failed)
            .contains("could not write these skills into ~/.claude/skills: review-loop wt"),
        "{}",
        stderr(&failed)
    );
}

/// Every file under a directory, with its text, for seeing that a run
/// changed nothing.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut all = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if e.file_type().unwrap().is_dir() {
                stack.push(p);
            } else {
                all.push((p.clone(), fs::read(&p).unwrap_or_default()));
            }
        }
    }
    all.sort();
    all
}

#[test]
fn the_guided_setup_asks_first_changes_nothing_on_a_closed_input_and_nothing_the_second_time() {
    let h = Home::new("setup-guided");
    std::os::unix::fs::symlink(BIN, h.path("bin/switchboard")).unwrap();
    h.stand_in("claude", "#!/bin/sh\n");

    // A closed input says no to everything.
    let closed = ok(&h.run(&["setup"]));
    assert!(closed.contains("[Y/n]"), "{closed}");
    assert!(closed.contains("– Switchboard's skills"), "{closed}");
    assert!(!h.path(".claude").exists(), "nothing written: {closed}");

    // Yes to each, by Enter or y.
    let yes = h.run_with(&["setup"], "\ny\nY\n");
    let said = ok(&yes);
    assert!(
        said.contains("✓ added Switchboard's Claude Code settings"),
        "{said}"
    );
    assert!(
        said.contains("✓ your CLAUDE.md imports Switchboard's part"),
        "{said}"
    );
    assert!(said.contains("✓ installed Switchboard's skills"), "{said}");
    assert!(said.ends_with("Ready.\n"), "{said}");

    // A second run asks nothing and changes nothing.
    let before = snapshot(&h.home);
    let second = ok(&h.run(&["setup"]));
    assert!(!second.contains("[Y/n]"), "{second}");
    let claude = second.split("\n\n").next().unwrap();
    assert_eq!(claude.matches('✓').count(), 5, "{second}");
    assert_eq!(snapshot(&h.home), before, "nothing changed");
    for part in ["claude", "claude-md", "skills"] {
        assert_eq!(
            ok(&h.run(&["setup", part])),
            format!("{part}: up to date\n")
        );
    }

    // A part already set up takes what is new without asking.
    fs::remove_dir_all(h.path(".claude/skills/wt")).unwrap();
    let newer = ok(&h.run(&["setup"]));
    assert!(newer.contains("write ~/.claude/skills/wt"), "{newer}");
    assert!(
        newer.contains("✓ Switchboard's skills, brought up to date"),
        "{newer}"
    );

    // Without switchboard on the PATH, the hooks couldn't run: left to do.
    fs::remove_file(h.path("bin/switchboard")).unwrap();
    let missing = h
        .command(BIN)
        .arg("setup")
        .env("PATH", format!("{}:/usr/bin:/bin", h.path("bin").display()))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(stdout(&missing).contains("Left to do:\n  ✗ switchboard on your PATH"));
}

#[test]
fn a_new_build_brings_its_parts_to_what_is_set_up_once() {
    let h = Home::new("setup-refresh");
    // Nothing set up: nothing is written but the build's mark.
    ok(&h.run(&["setup", "--refresh"]));
    assert!(!h.path(".claude").exists());
    assert!(h.path(".local/share/switchboard/setup-build").is_file());

    // Set up by an older build, whose skills and part of CLAUDE.md differ.
    ok(&h.run(&["setup", "skills"]));
    ok(&h.run(&["setup", "claude-md"]));
    fs::write(h.path(".claude/skills/wt/SKILL.md"), "an older wt\n").unwrap();
    fs::write(h.path(".local/share/switchboard/switchboard.md"), "older\n").unwrap();
    fs::write(
        h.path(".local/share/switchboard/setup-build"),
        "0.0.1 (old)\n",
    )
    .unwrap();
    let said = ok(&h.run(&["setup", "--refresh"]));
    assert_eq!(said, "", "quiet");
    assert_ne!(read(&h.path(".claude/skills/wt/SKILL.md")), "an older wt\n");
    assert_ne!(
        read(&h.path(".local/share/switchboard/switchboard.md")),
        "older\n"
    );
    // Claude Code's settings, never set up, are left for setup to ask about.
    assert!(!h.path(".claude/settings.json").exists());

    // Once per build: what is changed after is left alone.
    fs::write(h.path(".claude/skills/wt/SKILL.md"), "edited\n").unwrap();
    ok(&h.run(&["setup", "--refresh"]));
    assert_eq!(read(&h.path(".claude/skills/wt/SKILL.md")), "edited\n");
}

#[test]
fn what_the_tmux_version_left_is_tidied_after_asking_and_once() {
    use std::os::unix::fs::symlink;
    let h = Home::new("setup-leftovers");
    // A clone of the tmux version pulled past the rewrite: its bin/, tmux/
    // and claude/ gone, and the links setup.sh made into them left.
    let clone = h.path("switchboard");
    fs::create_dir_all(clone.join("assets/skills")).unwrap();
    fs::write(clone.join("Cargo.toml"), "").unwrap();
    for dir in [".local/bin", ".config/tmux", ".claude/hooks"] {
        fs::create_dir_all(h.path(dir)).unwrap();
    }
    symlink(
        clone.join("bin/worktree-from-ticket"),
        h.path(".local/bin/worktree-from-ticket"),
    )
    .unwrap();
    symlink(
        clone.join("bin/skill-path"),
        h.path(".local/bin/skill-path"),
    )
    .unwrap();
    symlink(
        clone.join("tmux/agent-state.sh"),
        h.path(".config/tmux/agent-state.sh"),
    )
    .unwrap();
    symlink(
        clone.join("claude/hooks/enforce.sh"),
        h.path(".claude/hooks/enforce.sh"),
    )
    .unwrap();
    symlink(clone.join("claude/commands"), h.path(".claude/commands")).unwrap();
    symlink(
        clone.join("claude/statusline-command.sh"),
        h.path(".claude/statusline-command.sh"),
    )
    .unwrap();
    // A link of your own leading nowhere, and one into the clone that still
    // leads somewhere, are left alone.
    symlink(h.path("elsewhere/bin/tool"), h.path(".local/bin/tool")).unwrap();
    fs::write(clone.join("Cargo.lock"), "").unwrap();
    symlink(clone.join("Cargo.lock"), h.path(".local/bin/kept")).unwrap();
    // ~/.zshrc as setup.sh left it, through a link into your dotfiles.
    fs::create_dir_all(h.path("dotfiles")).unwrap();
    let zshrc = "export A=1\neval \"$(worktree-from-ticket init zsh)\"\neval \"$(worktree-from-note init zsh)\"\n";
    fs::write(h.path("dotfiles/zshrc"), zshrc).unwrap();
    symlink(h.path("dotfiles/zshrc"), h.path(".zshrc")).unwrap();

    let check = ok(&h.run(&["setup", "--check", "leftovers"]));
    assert!(
        check.contains("would: remove ~/.local/bin/worktree-from-ticket"),
        "{check}"
    );
    assert!(check.contains("~/.zshrc"), "{check}");
    assert!(h.path(".local/bin/skill-path").symlink_metadata().is_ok());

    // The guided setup asks, and a closed input says no.
    let closed = String::from_utf8_lossy(&h.run(&["setup"]).stdout).into_owned();
    assert!(closed.contains("From the tmux version"), "{closed}");
    assert!(closed.contains("[Y/n]"), "{closed}");
    assert!(h.path(".claude/commands").symlink_metadata().is_ok());
    assert_eq!(read(&h.path("dotfiles/zshrc")), zshrc);

    // Yes, and each goes.
    let yes = String::from_utf8_lossy(&h.run_with(&["setup"], "y\n").stdout).into_owned();
    assert!(yes.contains("✓ tidied what the tmux version left"), "{yes}");
    for gone in [
        ".local/bin/worktree-from-ticket",
        ".local/bin/skill-path",
        ".config/tmux/agent-state.sh",
        ".claude/hooks/enforce.sh",
        ".claude/commands",
        ".claude/statusline-command.sh",
    ] {
        assert!(h.path(gone).symlink_metadata().is_err(), "{gone}");
    }
    assert!(h.path(".local/bin/tool").symlink_metadata().is_ok());
    assert!(h.path(".local/bin/kept").symlink_metadata().is_ok());
    assert!(
        h.path(".zshrc")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        read(&h.path("dotfiles/zshrc")),
        "export A=1\neval \"$(switchboard worktree-from-ticket init zsh)\"\neval \"$(switchboard worktree-from-note init zsh)\"\n"
    );
    assert_eq!(backups(&h.path("dotfiles"), "zshrc.bak."), 1);

    // Nothing left: the part is up to date and the guided setup says nothing of it.
    assert_eq!(
        ok(&h.run(&["setup", "leftovers"])),
        "leftovers: up to date\n"
    );
    let again = String::from_utf8_lossy(&h.run(&["setup"]).stdout).into_owned();
    assert!(!again.contains("From the tmux version"), "{again}");
}

#[test]
fn parts_are_listed_and_an_unknown_one_refused() {
    let h = Home::new("setup-parts");
    let parts = ok(&h.run(&["setup", "--parts"]));
    let names: Vec<&str> = parts
        .lines()
        .map(|l| l.split('\t').next().unwrap())
        .collect();
    assert_eq!(
        names,
        ["claude", "claude-md", "skills", "terminal", "leftovers"]
    );
    assert!(
        parts
            .lines()
            .all(|l| l.split('\t').nth(1).is_some_and(|a| !a.is_empty()))
    );
    let unknown = h.run(&["setup", "tmux"]);
    assert_eq!(unknown.status.code(), Some(1));
    assert!(stderr(&unknown).contains("Unknown part: tmux"));
}

// ── The statusline ──────────────────────────────────────────────────────────

fn statusline(h: &Home, payload: &Value, in_workspace: bool) -> String {
    let mut cmd = h.command(BIN);
    cmd.arg("statusline");
    if in_workspace {
        cmd.env("SWITCHBOARD", "/tmp/sock");
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    ok(&child.wait_with_output().unwrap())
}

#[test]
fn the_statusline_keeps_each_session_s_context_and_spend_for_the_views() {
    let h = Home::new("statusline");
    let state = h.path(".local/state/switchboard");
    fs::create_dir_all(h.path(".claude/sessions")).unwrap();
    fs::write(
        h.path(".claude/sessions/4242.json"),
        r#"{"pid":4242,"sessionId":"sess-1","cwd":"/w"}"#,
    )
    .unwrap();
    let payload = json!({
        "session_id": "sess-1",
        "model": { "display_name": "Opus" },
        "workspace": { "current_dir": h.home.to_string_lossy() },
        "context_window": { "used_percentage": 12.5 },
        "cost": { "total_cost_usd": 1.25 },
        "rate_limits": { "five_hour": { "used_percentage": 30, "resets_at": 1900000000 } }
    });
    let line = statusline(&h, &payload, false);
    assert!(line.contains("Opus"), "{line}");
    assert!(
        line.contains(" | \x1b[38;2;"),
        "outside the workspace, the directory: {line:?}"
    );
    assert!(line.contains("~"), "{line:?}");
    assert!(
        line.ends_with("Context: \x1b[97m12.5% used\x1b[0m"),
        "{line:?}"
    );
    assert!(
        !statusline(&h, &payload, true).contains('~'),
        "the tab names the directory"
    );

    assert_eq!(read(&state.join("context/sess-1")), "12.5\tOpus");
    let today = Command::new("date").arg("+%F").output().unwrap();
    let today = stdout(&today).trim().to_string();
    assert_eq!(
        read(&state.join("usage/spend-sess-1")),
        format!("{today}\t1.2500\t1.25\t4242")
    );
    assert_eq!(
        read(&state.join("usage/limits")),
        r#"{"five_hour":{"used_percentage":30,"resets_at":1900000000}}"#
    );
    let mode = fs::metadata(state.join("usage/spend-sess-1"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o077, 0, "yours alone");

    // A later draw adds what was spent since; a session id that couldn't be
    // a file name writes nothing.
    let mut later = payload.clone();
    later["cost"]["total_cost_usd"] = json!(2.0);
    statusline(&h, &later, true);
    assert_eq!(
        read(&state.join("usage/spend-sess-1")),
        format!("{today}\t2.0000\t2\t4242")
    );
    let mut odd = payload.clone();
    odd["session_id"] = json!("../x");
    statusline(&h, &odd, true);
    assert!(!state.join("x").exists() && !state.join("context/../x").exists());
}

// ── What the skills ask of the server ───────────────────────────────────────

#[test]
fn respawn_puts_a_program_in_the_pane_s_place_and_tabs_lists_it() {
    let server = common::TestServer::start();
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    let list = server.list();
    let tab = list["sessions"][0]["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let old = list["sessions"][0]["tabs"][0]["panes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.ok(&["set", "-t", &tab, "@tab_name", "Shaping: a thing"]);
    server.ok(&[
        "respawn",
        "-t",
        &old,
        "-c",
        "/tmp",
        "sh",
        "-c",
        "echo respawned-$((6*7)); sleep 30",
    ]);
    client.wait_text("respawned-42");
    let list = server.list();
    let panes = list["sessions"][0]["tabs"][0]["panes"].as_array().unwrap();
    assert_eq!(panes.len(), 1, "the new program in the old one's place");
    let new = panes[0]["id"].as_str().unwrap();
    assert_eq!(new, old, "in the same pane, as tmux's respawn-pane kept it");
    assert_eq!(panes[0]["active"], true);
    assert_eq!(
        list["sessions"][0]["tabs"][0]["id"],
        tab.as_str(),
        "the same tab"
    );

    let tabs = server.ok(&["tabs"]);
    let fields: Vec<&str> = tabs.trim_end().split('\t').collect();
    assert_eq!(fields.len(), 6, "{tabs:?}");
    assert_eq!(fields[0], "1");
    assert!(fields[1].ends_with(&format!(":{tab}.{new}")), "{tabs:?}");
    assert_eq!(fields[2], "Shaping: a thing");
    assert_eq!(fields[5], "/tmp");
}
