//! `switchboard setup`'s parts, which must be safe to run again, ported from
//! test/setup.test.js's Claude parts, and `switchboard statusline`, from
//! what claude/statusline-command.sh did. Each runs in a temporary HOME, so
//! nothing reaches your own ~/.claude or ~/.config.

use crate::common;

use std::fs;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use common::stand_in::{Claude, Gh};
use common::{BIN, TempDir};

/// A home of the test's own, with `bin` for its stand-ins for programs.
struct Home {
    _dir: TempDir,
    home: PathBuf,
    /// The PATH its commands run with: `bin` first, then the test's own,
    /// without the computer's own lazygit.
    search_path: std::ffi::OsString,
}

/// The test's PATH with `bin` first and the computer's own lazygit, which
/// the guided setup would offer a config for, left out: a folder holding
/// one is replaced by a folder in `shadows` linking everything else in it,
/// so git and the rest beside it are still found. A test that wants lazygit
/// puts a stand-in in `bin`.
fn search_path(bin: &Path, shadows: &Path) -> std::ffi::OsString {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs = std::env::split_paths(&path).enumerate().map(|(i, dir)| {
        if !dir.join("lazygit").exists() {
            return dir;
        }
        let shadow = shadows.join(i.to_string());
        fs::create_dir_all(&shadow).unwrap();
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            if entry.file_name() != "lazygit" {
                let _ = std::os::unix::fs::symlink(entry.path(), shadow.join(entry.file_name()));
            }
        }
        shadow
    });
    std::env::join_paths(std::iter::once(bin.to_path_buf()).chain(dirs)).unwrap()
}

impl Home {
    /// A home with a stand-in Claude Code new enough for the plugin, so the
    /// real one is never run.
    fn new(prefix: &str) -> Home {
        let dir = TempDir::new(prefix);
        let home = dir.path().join("home");
        fs::create_dir_all(home.join("bin")).unwrap();
        let search_path = search_path(&home.join("bin"), &dir.path().join("path"));
        let h = Home {
            _dir: dir,
            home,
            search_path,
        };
        h.claude_version("2.1.319");
        // And a gh that isn't signed in, so the real one isn't asked.
        Gh::new().install(&h.path("bin"));
        h
    }

    /// The stand-in Claude Code, as strict as it about how it is called,
    /// saying it is `version`.
    fn claude_version(&self, version: &str) {
        Claude::new().version(version).install(&self.path("bin"));
    }

    /// Switchboard's own settings file, holding `settings`.
    fn settings(&self, settings: Value) {
        let file = self.path(".config/switchboard/settings.json");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, settings.to_string()).unwrap();
    }

    /// Where the test's Claude Code managed settings are, in place of the
    /// computer's own (`/etc/claude-code`), which no test reads: none until
    /// `managed` writes them.
    fn managed_file(&self) -> PathBuf {
        self._dir.path().join("managed-settings.json")
    }

    /// Claude Code's managed settings, as an organization's policy sets them.
    fn managed(&self, settings: Value) {
        fs::write(self.managed_file(), settings.to_string()).unwrap();
    }

    /// What Switchboard's settings file holds, `{}` when there is none.
    fn saved(&self) -> Value {
        fs::read_to_string(self.path(".config/switchboard/settings.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(json!({}))
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.home.join(rel)
    }

    fn command(&self, program: &str) -> Command {
        let mut cmd = Command::new(program);
        cmd.env("HOME", &self.home)
            .env("PATH", &self.search_path)
            .env(
                "SWITCHBOARD_STATE_DIR",
                self.home.join(".local/state/switchboard"),
            )
            .env("SWITCHBOARD_CLAUDE_MANAGED_SETTINGS", self.managed_file())
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
            // The assets are what the binary carries, not this
            // repository's on disk (src/assets.rs), unless a test names a
            // clone of its own.
            .env("SWITCHBOARD_CHECKOUT", "")
            .env_remove("CLAUDE_CODE_PLUGIN_DIRS")
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            // The terminal and the services the guided setup looks at are
            // the test's: none.
            .env_remove("TERM_PROGRAM")
            .env_remove("LC_TERMINAL")
            .env_remove("KITTY_WINDOW_ID")
            .env_remove("ALACRITTY_WINDOW_ID")
            .env_remove("ALACRITTY_SOCKET")
            .env_remove("XTERM_VERSION")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("LG_CONFIG_FILE")
            .env_remove("CONFIG_DIR")
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
}

impl Drop for Home {
    /// Fails the test if a stand-in on its PATH refused a call.
    fn drop(&mut self) {
        if !std::thread::panicking() {
            common::stand_in::assert_none_refused(&self.path("bin"));
        }
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
fn as_a_plugin_the_guided_setup_leaves_your_claude_settings_alone_and_asks_nothing() {
    let h = Home::new("setup-yours");
    std::os::unix::fs::symlink(BIN, h.path("bin/switchboard")).unwrap();
    // A settings file of your own, with a statusline Claude Code's
    // /statusline made, and none of Switchboard's part.
    let file = h.path(".claude/settings.json");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let settings = json!({
        "model": "opus",
        "statusLine": { "type": "command", "command": "bash \"$HOME/.claude/statusline-command.sh\"" },
    });
    fs::write(&file, settings.to_string()).unwrap();
    fs::write(
        h.path(".claude/statusline-command.sh"),
        "#!/bin/bash\necho mine\n",
    )
    .unwrap();
    let before = snapshot(&h.path(".claude"));

    // Enter, which would say yes to any question: none is asked, and
    // nothing of yours changes, run after run.
    for _ in 0..2 {
        let said = ok(&h.run_with(&["setup"], "\n\n\n"));
        assert!(!said.contains("[Y/n]"), "{said}");
        assert!(
            said.contains("✓ Switchboard's statusline in its panes, drawn by its plugin (mission control's Usage view), yours everywhere else"),
            "{said}"
        );
        assert!(said.ends_with("Ready.\n"), "{said}");
        assert_eq!(
            snapshot(&h.path(".claude")),
            before,
            "nothing of yours: {said}"
        );
    }
    let check = ok(&h.run(&["setup", "--check"]));
    assert!(!check.contains("would offer: plugin, claude"), "{check}");
    assert_eq!(snapshot(&h.path(".claude")), before);
    // Nor does the part asked for by name: as a plugin it adds nothing.
    ok(&h.run(&["setup", "claude"]));
    assert_eq!(snapshot(&h.path(".claude")), before);

    // The statusline an earlier version added, taken out, since the plugin
    // draws Switchboard's in its panes now.
    let earlier =
        json!({ "statusLine": { "type": "command", "command": "switchboard statusline" } });
    fs::write(&file, earlier.to_string()).unwrap();
    let said = ok(&h.run_with(&["setup"], "\n\n\n"));
    assert!(!said.contains("[Y/n]"), "{said}");
    assert!(
        said.contains("✓ what an earlier version added to your ~/.claude/settings.json and Switchboard's plugin carries now, taken out (a statusline of yours kept)"),
        "{said}"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&read(&file)).unwrap(),
        json!({})
    );

    // Settings' "Switchboard's statusline in its panes" off: said, and an
    // earlier statusline goes too, since yours is what the panes draw then.
    h.settings(json!({ "pane_statusline": false }));
    fs::write(&file, earlier.to_string()).unwrap();
    let said = ok(&h.run_with(&["setup"], "\n\n\n"));
    assert!(
        said.contains("– Switchboard's statusline in its panes, for mission control's Usage view: off in Settings' \"Switchboard's statusline in its panes\", so they draw yours"),
        "{said}"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&read(&file)).unwrap(),
        json!({})
    );
}

#[test]
fn claude_md_imports_your_rules_from_a_file_or_a_repository_and_takes_switchboard_s_part_out() {
    let h = Home::new("setup-md");
    let file = h.path(".claude/CLAUDE.md");
    let config = h.path(".config/switchboard/claude.json");
    let run = |args: &[&str]| h.run(args);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::create_dir_all(config.parent().unwrap()).unwrap();

    // A CLAUDE.md of your own, no rules named, importing Switchboard's part
    // as earlier versions added it: the import and the part's copy go,
    // once, yours stays.
    let part = h.path(".local/share/switchboard/switchboard.md");
    fs::create_dir_all(part.parent().unwrap()).unwrap();
    fs::write(&part, "older\n").unwrap();
    fs::write(
        &file,
        "# Mine\n\n- be brief\n\n@~/.local/share/switchboard/switchboard.md\n",
    )
    .unwrap();
    ok(&run(&["setup", "claude-md"]));
    assert_eq!(read(&file), "# Mine\n\n- be brief\n");
    assert!(!part.exists());
    assert_eq!(ok(&run(&["setup", "claude_md"])), "claude-md: up to date\n");

    // Rules in a file named in claude.json: your old CLAUDE.md is moved
    // aside, and the new one imports them.
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
    assert!(written.ends_with("\n\n@~/my-rules.md\n"), "{written}");
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
    assert!(read(&file).ends_with("\n@~/.local/share/switchboard/claude-md/CLAUDE.md\n"));
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

    // The guided setup lists it as left to do, with Claude Code there
    // (Home's stand-in).
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
    // A space in gh's path, which the helper has to quote; signed in once
    // SIGNED_IN is there.
    let bin = h.path("my bin");
    let gh = Gh::signed_in_while(&h.path("SIGNED_IN")).install(&bin);
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
    common::stand_in::assert_none_refused(&bin);
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
fn the_guided_setup_writes_the_plugin_and_changes_nothing_the_second_time() {
    let h = Home::new("setup-guided");
    std::os::unix::fs::symlink(BIN, h.path("bin/switchboard")).unwrap();
    let plugin = h.path(".local/share/switchboard/claude/plugin");

    // --check says what the setup would do, and writes nothing.
    let check = ok(&h.run(&["setup", "--check"]));
    assert!(
        // A Mac's terminal may be offered after it.
        check.contains("switchboard setup would offer: plugin"),
        "{check}"
    );
    assert!(!check.contains("would offer: plugin, claude"), "{check}");
    assert!(!h.path(".claude").exists(), "nothing written: {check}");
    assert!(!plugin.exists(), "nothing written: {check}");

    // The plugin, Switchboard's own, is written without asking, with the
    // statusline it draws in its panes, and your Claude Code settings are
    // left alone.
    let closed = ok(&h.run(&["setup"]));
    assert!(closed.contains("✓ Switchboard's plugin"), "{closed}");
    assert!(
        closed.contains("✓ Switchboard's statusline in its panes, drawn by its plugin"),
        "{closed}"
    );
    assert!(
        closed.contains("the Slack relay, for reaching your tabs from Slack"),
        "{closed}"
    );
    assert!(!h.path(".claude").exists(), "nothing of yours: {closed}");
    assert!(plugin.join(".claude-plugin/plugin.json").is_file());
    assert_eq!(
        read(&plugin.join("skills/wt/SKILL.md")),
        read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/skills/wt/SKILL.md"))
    );
    let hooks = read(&plugin.join("hooks/hooks.json"));
    assert!(
        hooks.contains("switchboard hook Stop") && hooks.contains("switchboard claude-context"),
        "{hooks}"
    );
    // The plugin's one hooks module, which hooks.json lists, importing the
    // statusline's file and the permissions'.
    let listed: Value = serde_json::from_str(&hooks).unwrap();
    assert_eq!(listed["modules"], json!(["./switchboard.js"]));
    let entry = read(&plugin.join("hooks/switchboard.js"));
    assert!(
        entry.contains("from './statusline.js'") && entry.contains("from './permissions.js'"),
        "{entry}"
    );
    let module = read(&plugin.join("hooks/statusline.js"));
    assert!(
        module.contains("on('settings.read'")
            && module.contains(r#""command":"switchboard statusline""#)
            && module.contains("['switchboard', 'setting', 'pane_statusline']"),
        "{module}"
    );
    // The permissions' module, its lists filled in from the asset.
    let module = read(&plugin.join("hooks/permissions.js"));
    assert!(
        module.contains("hook('tool.check', { tool: 'Bash' }"),
        "{module}"
    );
    assert!(
        module.contains(r#""Bash(switchboard tabs)""#) && module.contains(r#""Bash(git pull)""#),
        "{module}"
    );

    // Nor does asking for the part by name add anything to your settings.
    ok(&h.run(&["setup", "claude"]));
    assert!(!h.path(".claude").exists());
    assert!(!h.path(".claude/CLAUDE.md").exists());
    assert_eq!(h.saved(), json!({}), "the setting at its default, on");

    // A second run asks nothing and changes nothing.
    let before = snapshot(&h.home);
    let second = ok(&h.run(&["setup"]));
    assert!(!second.contains("[Y/n]"), "{second}");
    let claude = second.split("\n\n").next().unwrap();
    assert_eq!(claude.matches('✓').count(), 4, "{second}");
    assert_eq!(snapshot(&h.home), before, "nothing changed");
    for part in ["plugin", "claude", "claude-md"] {
        assert_eq!(
            ok(&h.run(&["setup", part])),
            format!("{part}: up to date\n")
        );
    }

    // The plugin takes what is new without asking.
    fs::write(plugin.join("skills/wt/SKILL.md"), "an older wt\n").unwrap();
    let newer = ok(&h.run(&["setup"]));
    assert!(
        newer.contains("bring ~/.local/share/switchboard/claude/plugin up to date (1 file written), its skills and hooks as the binary carries them"),
        "{newer}"
    );
    assert_ne!(read(&plugin.join("skills/wt/SKILL.md")), "an older wt\n");

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

/// A home set up the old way by an earlier version, with Settings' old
/// `claude_plugin` off: Switchboard's hooks, statusline (around one of
/// yours) and permissions in your settings beside your own, the import of
/// its part in a CLAUDE.md of yours, and its skills in `~/.claude/skills`,
/// one holding a file of yours and one the packages it installed, beside a
/// skill of your own by one of its names. What it returns is your own hook.
fn earlier_install(h: &Home) -> Value {
    h.settings(json!({ "claude_plugin": false }));
    let your_hook = json!({ "hooks": [{ "type": "command", "command": "say done" }] });
    let mut hooks = product()["hooks"].clone();
    hooks["Stop"]
        .as_array_mut()
        .unwrap()
        .insert(0, your_hook.clone());
    let settings = json!({
        "model": "opus",
        "permissions": { "allow": ["Bash(make *)", "Bash(switchboard tabs)", "Bash(git log *)"] },
        "statusLine": {
            "type": "command",
            "command": format!("{}my-statusline", switchboard::setup::claude_settings::IN_PANES),
            "padding": 1
        },
        "hooks": hooks
    });
    fs::create_dir_all(h.path(".claude/skills")).unwrap();
    fs::write(h.path(".claude/settings.json"), settings.to_string()).unwrap();
    fs::write(
        h.path(".claude/CLAUDE.md"),
        "# Mine\n- be brief\n\n@~/.local/share/switchboard/switchboard.md\n",
    )
    .unwrap();
    fs::create_dir_all(h.path(".local/share/switchboard")).unwrap();
    fs::write(h.path(".local/share/switchboard/switchboard.md"), "older\n").unwrap();
    for (name, files) in [
        ("wt", &["SKILL.md"][..]),
        ("walkthrough", &["SKILL.md", "scripts/record.js"]),
    ] {
        let dir = h.path(".claude/skills").join(name);
        for file in files {
            let path = dir.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "older\n").unwrap();
        }
        let list: String = files.iter().map(|f| format!("{f}\n")).collect();
        fs::write(dir.join(".switchboard-skill"), list).unwrap();
    }
    fs::write(h.path(".claude/skills/wt/notes.md"), "mine\n").unwrap();
    let modules = h.path(".claude/skills/walkthrough/scripts/node_modules/x");
    fs::create_dir_all(&modules).unwrap();
    fs::write(modules.join("index.js"), "").unwrap();
    fs::create_dir_all(h.path(".claude/skills/today")).unwrap();
    fs::write(h.path(".claude/skills/today/SKILL.md"), "mine\n").unwrap();
    json!({ "Stop": [your_hook] })
}

/// That nothing of Switchboard's is left in `~/.claude` and all of yours
/// is: your hook, model, statusline, permissions, CLAUDE.md, skill and file.
fn only_yours_left(h: &Home, your_hooks: &Value) {
    let s: Value = serde_json::from_str(&read(&h.path(".claude/settings.json"))).unwrap();
    assert_eq!(s["hooks"], *your_hooks, "{s}");
    assert_eq!(s["model"], "opus");
    assert_eq!(
        s["statusLine"],
        json!({ "type": "command", "command": "my-statusline", "padding": 1 })
    );
    assert_eq!(
        s["permissions"]["allow"],
        json!(["Bash(make *)", "Bash(switchboard tabs)"]),
        "the rule that let Claude do too much gone, the rest kept"
    );
    assert_eq!(read(&h.path(".claude/CLAUDE.md")), "# Mine\n- be brief\n");
    assert!(!h.path(".local/share/switchboard/switchboard.md").exists());
    let mut skills: Vec<String> = fs::read_dir(h.path(".claude/skills"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    skills.sort();
    assert_eq!(skills, ["today", "wt"], "packages and all");
    assert_eq!(read(&h.path(".claude/skills/today/SKILL.md")), "mine\n");
    let wt: Vec<_> = fs::read_dir(h.path(".claude/skills/wt"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(wt, ["notes.md"]);
    assert!(
        h.path(".local/share/switchboard/claude/plugin/skills/wt/SKILL.md")
            .is_file()
    );
}

#[test]
fn a_home_set_up_the_old_way_ends_set_up_as_the_plugin_on_a_new_build_s_first_run() {
    let h = Home::new("setup-refresh");
    // Nothing set up: the plugin and the build's mark, nothing of yours.
    ok(&h.run(&["setup", "--refresh"]));
    assert!(!h.path(".claude").exists());
    assert!(
        h.path(".local/share/switchboard/claude/plugin/skills/wt/SKILL.md")
            .is_file()
    );
    assert!(h.path(".local/share/switchboard/setup-build").is_file());

    let your_hooks = earlier_install(&h);
    fs::write(
        h.path(".local/share/switchboard/setup-build"),
        "0.0.1 (old)\n",
    )
    .unwrap();
    assert_eq!(ok(&h.run(&["setup", "--refresh"])), "", "quiet");
    only_yours_left(&h, &your_hooks);
    assert_eq!(backups(&h.path(".claude"), "settings.json.bak."), 1);
    for part in ["claude", "claude-md", "skills"] {
        assert_eq!(
            ok(&h.run(&["setup", part])),
            format!("{part}: up to date\n")
        );
    }

    // Once per build: what is changed after is left alone.
    fs::write(
        h.path(".claude/settings.json"),
        json!({ "statusLine": { "type": "command", "command": "switchboard statusline" } })
            .to_string(),
    )
    .unwrap();
    ok(&h.run(&["setup", "--refresh"]));
    assert!(read(&h.path(".claude/settings.json")).contains("switchboard statusline"));

    // A file that isn't JSON is left alone, and said.
    fs::write(h.path(".claude/settings.json"), "{ not json").unwrap();
    let bad = h.run(&["setup", "claude"]);
    assert!(!bad.status.success());
    assert!(stderr(&bad).contains("isn't a JSON object; left alone"));
    assert_eq!(read(&h.path(".claude/settings.json")), "{ not json");
}

#[test]
fn the_guided_setup_takes_an_earlier_install_out_of_claude_without_asking() {
    let h = Home::new("setup-earlier");
    std::os::unix::fs::symlink(BIN, h.path("bin/switchboard")).unwrap();
    let your_hooks = earlier_install(&h);
    let installed = snapshot(&h.path(".claude"));

    // --check says what it would take out, and changes nothing.
    let check = stdout(&h.run(&["setup", "--check"]));
    assert!(
        check.contains("your ~/.claude/settings.json has what an earlier version added there"),
        "{check}"
    );
    assert_eq!(snapshot(&h.path(".claude")), installed);

    // Closed input, which says no to any question: none is asked.
    let said = ok(&h.run(&["setup"]));
    assert!(!said.contains("[Y/n]"), "{said}");
    assert!(
        said.contains("✓ what an earlier version added to your ~/.claude/settings.json and Switchboard's plugin carries now, taken out (a statusline of yours kept)"),
        "{said}"
    );
    assert!(
        said.contains(
            "✓ took Switchboard's skills out of ~/.claude/skills: its plugin carries them"
        ),
        "{said}"
    );
    assert!(said.ends_with("Ready.\n"), "{said}");
    only_yours_left(&h, &your_hooks);
    let before = snapshot(&h.home);
    ok(&h.run(&["setup"]));
    assert_eq!(snapshot(&h.home), before, "nothing the second time");
}

#[test]
fn the_allow_rules_switchboard_dropped_go_once_and_a_rule_of_yours_that_differs_stays() {
    let h = Home::new("setup-retired");
    let file = h.path(".claude/settings.json");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let earlier = json!({
        "permissions": {
            "allow": [
                "Bash(make *)",
                "Bash(node ~/.local/share/switchboard/claude/plugin/skills/*)",
                "Bash(node ~/.claude/skills/*)",
                "Bash(git fetch *)",
                "Bash(switchboard setting *)",
                "Bash(git fetch origin main)",
                "Bash(gh pr list --json *)"
            ],
            "deny": ["Bash(rm *)"]
        }
    });
    fs::write(&file, earlier.to_string()).unwrap();
    fs::create_dir_all(h.path(".local/share/switchboard")).unwrap();
    fs::write(
        h.path(".local/share/switchboard/setup-build"),
        "0.0.1 (old)\n",
    )
    .unwrap();
    assert_eq!(ok(&h.run(&["setup", "--refresh"])), "", "quiet");
    assert_eq!(
        serde_json::from_str::<Value>(&read(&file)).unwrap(),
        json!({
            "permissions": {
                "allow": ["Bash(make *)", "Bash(git fetch origin main)", "Bash(gh pr list --json *)"],
                "deny": ["Bash(rm *)"]
            }
        }),
        "yours kept, in order, nothing added"
    );
    // Once: a second run changes nothing.
    let copies = backups(&h.path(".claude"), "settings.json.bak.");
    assert_eq!(ok(&h.run(&["setup", "claude"])), "claude: up to date\n");
    assert_eq!(backups(&h.path(".claude"), "settings.json.bak."), copies);
}

#[test]
fn a_claude_code_too_old_for_the_plugin_or_a_policy_against_it_is_said() {
    let h = Home::new("setup-blocked");
    std::os::unix::fs::symlink(BIN, h.path("bin/switchboard")).unwrap();
    let plugin = h.path(".local/share/switchboard/claude/plugin");
    let dirs = || ok(&h.run(&["claude-plugin-dirs"]));
    assert_eq!(dirs(), format!("{}\n", plugin.display()));

    // Too old: left to do, with how.
    h.claude_version("2.1.100");
    let old = h.run(&["setup"]);
    assert!(!old.status.success());
    assert!(
        stdout(&old).contains(
            "✗ Claude Code 2.1.100 is older than 2.1.280, which loads Switchboard's plugin"
        ),
        "{}",
        stdout(&old)
    );
    assert!(plugin.join(".claude-plugin/plugin.json").is_file());

    // A policy against sideloading: said, and no pane names the plugin, so
    // their Claude starts.
    h.claude_version("2.1.319");
    h.managed(json!({ "disableSideloadFlags": true }));
    let said = ok(&h.run(&["setup"]));
    assert!(said.contains("set disableSideloadFlags"), "{said}");
    assert_eq!(dirs(), "\n");
    h.managed(json!({ "disableSideloadFlags": false }));
    assert_eq!(dirs(), format!("{}\n", plugin.display()));
}

#[test]
fn claude_context_gives_an_interactive_session_switchboard_s_instructions_and_a_headless_one_nothing()
 {
    let h = Home::new("setup-context");
    let run = |entrypoint: Option<&str>, input: &str| -> Output {
        let mut cmd = h.command(BIN);
        cmd.arg("claude-context")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(e) = entrypoint {
            cmd.env("CLAUDE_CODE_ENTRYPOINT", e);
        }
        let mut child = cmd.spawn().unwrap();
        // One that has nothing to say may end before reading its input, so
        // a pipe it closed is no failure.
        let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
        child.wait_with_output().unwrap()
    };
    let part = read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/switchboard.md"));
    assert!(
        part.contains("# What I see in the workspace") && part.contains("switchboard diff-story")
    );
    let allowed = read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/claude-settings.json"));
    assert!(
        allowed.contains(r#""Bash(switchboard diff-story)""#),
        "diff-story runs without asking"
    );
    // What reaches you from the Slack relay is changed without asking, as
    // the instructions say to change it.
    assert!(
        allowed.contains(r#""Bash(switchboard setting relay_*)""#)
            && allowed.contains(r#""Bash(switchboard slack-relay sends *)""#)
    );
    assert!(
        part.contains("switchboard setting relay_send_<kind>")
            && part.contains("switchboard slack-relay sends")
    );
    let start = r#"{"session_id":"s1","hook_event_name":"SessionStart","source":"startup"}"#;

    let out = run(Some("cli"), start);
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    let answer: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(
        answer,
        json!({ "hookSpecificOutput": { "hookEventName": "SessionStart", "additionalContext": part } })
    );
    // Whatever its input.
    let odd = run(None, "not json");
    assert!(odd.status.success());
    assert_eq!(odd.stdout, out.stdout);

    // A headless session, by how Claude Code says it started it, or by
    // its session file: nothing, and still 0.
    let headless = run(Some("sdk-cli"), start);
    assert!(headless.status.success());
    assert!(headless.stdout.is_empty() && headless.stderr.is_empty());
    let sessions = h.path(".claude/sessions");
    fs::create_dir_all(&sessions).unwrap();
    fs::write(
        sessions.join("1.json"),
        json!({ "pid": 1, "sessionId": "s1", "entrypoint": "sdk-cli" }).to_string(),
    )
    .unwrap();
    let by_file = run(Some("cli"), start);
    assert!(by_file.status.success());
    assert!(by_file.stdout.is_empty());
}

#[test]
fn only_switchboard_s_lazygit_settings_are_written_never_a_config_of_yours() {
    let h = Home::new("setup-lazygit");
    std::os::unix::fs::symlink(BIN, h.path("bin/switchboard")).unwrap();
    let config = h.path(if cfg!(target_os = "macos") {
        "Library/Application Support/lazygit/config.yml"
    } else {
        ".config/lazygit/config.yml"
    });
    let switchboard_s = h.path(".local/share/switchboard/lazygit.yml");

    // Without lazygit, the part has nothing to set up and isn't offered.
    let without = h.run(&["setup", "lazygit"]);
    assert_eq!(without.status.code(), Some(1));
    assert!(stderr(&without).contains("lazygit isn't installed"));
    assert!(!stdout(&h.run(&["setup"])).contains("lazygit"));

    // With it, the guided setup asks nothing and writes Switchboard's copy
    // alone, never where lazygit reads its own config.
    common::stand_in::install(&h.path("bin/lazygit"), "#!/bin/sh\nexit 0\n");
    let guided = ok(&h.run(&["setup"]));
    assert!(
        guided.contains("lazygit in a pane's place gets Switchboard's settings"),
        "{guided}"
    );
    assert!(read(&switchboard_s).contains("quitOnTopLevelReturn: true"));
    assert!(!config.exists());
    assert_eq!(ok(&h.run(&["setup", "lazygit"])), "lazygit: up to date\n");

    // Out of date, named, it is written again; a config of yours is kept
    // byte for byte.
    fs::write(&switchboard_s, "old\n").unwrap();
    let own = h.path(".config/jesseduffield/lazygit/config.yml");
    fs::create_dir_all(own.parent().unwrap()).unwrap();
    fs::write(&own, "gui:\n  sidePanelWidth: 0.2\n").unwrap();
    let wrote = ok(&h.run(&["setup", "lazygit"]));
    assert!(
        wrote.contains("write Switchboard's lazygit settings"),
        "{wrote}"
    );
    assert!(read(&switchboard_s).contains("quitOnTopLevelReturn: true"));
    assert_eq!(read(&own), "gui:\n  sidePanelWidth: 0.2\n");
    assert!(!config.exists());
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
        [
            "plugin",
            "claude",
            "claude-md",
            "skills",
            "terminal",
            "lazygit"
        ]
    );
    assert!(
        parts
            .lines()
            .all(|l| l.split('\t').nth(1).is_some_and(|a| !a.is_empty()))
    );
    let unknown = h.run(&["setup", "tmux"]);
    assert_eq!(unknown.status.code(), Some(1));
    assert!(stderr(&unknown).contains("Unknown part: tmux"));
    // The Slack sign-in stores a token, so it has no dry run and runs with
    // no part: refused before it asks for anything.
    for args in [
        &["setup", "--check", "slack-user"][..],
        &["setup", "slack-user", "--check"],
        &["setup", "skills", "slack-user"],
    ] {
        let refused = h.run(args);
        assert_eq!(refused.status.code(), Some(1), "{args:?}");
        assert!(
            stderr(&refused).contains("slack-user runs alone"),
            "{args:?}: {}",
            stderr(&refused)
        );
        assert!(
            !String::from_utf8_lossy(&refused.stdout).contains("Paste the token"),
            "{args:?}"
        );
    }
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
        "rate_limits": {
            "five_hour": { "used_percentage": 30, "resets_at": 1900000000 },
            "spend_limit": {
                "used_percentage": 62.8,
                "resets_at": 1900000000,
                "used_usd": 314.12,
                "limit_usd": 500,
                "period": "monthly"
            }
        }
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
        format!("{today}\t1.2500\t1.25\t4242\t2")
    );
    assert_eq!(
        read(&state.join("usage/limits")),
        r#"{"five_hour":{"used_percentage":30,"resets_at":1900000000},"spend_limit":{"used_percentage":62.8,"resets_at":1900000000,"used_usd":314.12,"limit_usd":500,"period":"monthly"}}"#
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
        format!("{today}\t2.0000\t2\t4242\t2")
    );

    // Only a session a request just answered writes the limits: an idle
    // one's redraw at the same cost, still holding a week since reset,
    // leaves a fresher session's figure alone.
    let limits =
        |used: u32| json!({ "seven_day": { "used_percentage": used, "resets_at": 1900000000 } });
    let mut fresh = payload.clone();
    fresh["session_id"] = json!("sess-2");
    fresh["cost"]["total_cost_usd"] = json!(0.5);
    fresh["rate_limits"] = limits(11);
    statusline(&h, &fresh, true);
    let fresh_limits = r#"{"seven_day":{"used_percentage":11,"resets_at":1900000000}}"#;
    assert_eq!(read(&state.join("usage/limits")), fresh_limits);
    let mut stale = later.clone();
    stale["rate_limits"] = limits(100);
    statusline(&h, &stale, true);
    assert_eq!(
        read(&state.join("usage/limits")),
        fresh_limits,
        "an idle session's redraw"
    );
    stale["cost"]["total_cost_usd"] = json!(2.5);
    stale["rate_limits"] = limits(12);
    statusline(&h, &stale, true);
    assert_eq!(
        read(&state.join("usage/limits")),
        r#"{"seven_day":{"used_percentage":12,"resets_at":1900000000}}"#,
        "the same session, once a request answers it"
    );

    let mut odd = payload.clone();
    odd["session_id"] = json!("../x");
    statusline(&h, &odd, true);
    assert!(!state.join("x").exists() && !state.join("context/../x").exists());
}

#[test]
fn the_statusline_shows_the_context_in_the_workspace_unless_its_setting_is_off() {
    let h = Home::new("statusline-context");
    let payload = json!({
        "model": { "display_name": "Opus" },
        "workspace": { "current_dir": h.home.to_string_lossy() },
        "context_window": { "used_percentage": 12.5 }
    });
    h.settings(json!({ "usage": true }));
    let line = statusline(&h, &payload, true);
    assert!(
        line.ends_with("Context: \x1b[97m12.5% used\x1b[0m"),
        "on by default, beside the Usage view's Sessions: {line:?}"
    );

    h.settings(json!({ "usage": true, "statusline_context": false }));
    let line = statusline(&h, &payload, true);
    assert!(line.contains("Opus"), "{line:?}");
    assert!(
        !line.contains("Context:"),
        "off, the Usage view's Sessions show it: {line:?}"
    );
    assert!(
        statusline(&h, &payload, false).contains("Context:"),
        "a plain terminal has no Usage view"
    );

    h.settings(json!({ "usage": false, "statusline_context": false }));
    assert!(
        statusline(&h, &payload, true).contains("Context:"),
        "off with no Usage view, nothing else shows it"
    );
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
    // As the system reads the pane's directory: /private/tmp on a Mac.
    let tmp = std::fs::canonicalize("/tmp").unwrap();
    assert_eq!(fields[5], tmp.to_str().unwrap());
}

#[test]
fn a_build_reads_the_skills_and_instructions_of_the_clone_it_came_from_unless_turned_off() {
    let h = Home::new("setup-assets");
    let clone = h._dir.path().join("clone");
    fs::create_dir_all(clone.join("assets/skills/x")).unwrap();
    fs::write(clone.join("assets/skills/x/SKILL.md"), "the clone's x\n").unwrap();
    fs::write(clone.join("assets/switchboard.md"), "the clone's part\n").unwrap();
    let from_clone = |args: &[&str], input: Option<&str>| -> Output {
        let mut cmd = h.command(BIN);
        cmd.env("SWITCHBOARD_CHECKOUT", &clone)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.unwrap_or("").as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let plugin = h.path(".local/share/switchboard/claude/plugin");
    let start = r#"{"session_id":"s1","hook_event_name":"SessionStart","source":"startup"}"#;

    let wrote = ok(&from_clone(&["setup", "plugin"], None));
    assert!(wrote.contains("its skills and hooks from "), "{wrote}");
    assert_eq!(read(&plugin.join("skills/x/SKILL.md")), "the clone's x\n");
    assert!(
        !plugin.join("skills/wt").exists(),
        "only the clone's skills"
    );
    // Claude's hooks, which the clone doesn't have, are the binary's.
    assert!(read(&plugin.join("hooks/hooks.json")).contains("switchboard hook Stop"));
    assert_eq!(
        ok(&from_clone(&["skill-path", "--own", "x"], None)),
        "the clone's x\n"
    );
    let answer: Value =
        serde_json::from_slice(&from_clone(&["claude-context"], Some(start)).stdout).unwrap();
    assert_eq!(
        answer["hookSpecificOutput"]["additionalContext"],
        "the clone's part\n"
    );

    // An edit in the clone is taken up by the next run, with no new build.
    fs::write(clone.join("assets/skills/x/SKILL.md"), "x, edited\n").unwrap();
    ok(&from_clone(&["setup", "plugin"], None));
    assert_eq!(read(&plugin.join("skills/x/SKILL.md")), "x, edited\n");

    // Off, the build uses what it carries.
    h.settings(json!({ "assets_from_checkout": false }));
    let back = ok(&from_clone(&["setup", "plugin"], None));
    assert!(back.contains("as the binary carries them"), "{back}");
    assert!(!plugin.join("skills/x").exists());
    assert!(plugin.join("skills/wt/SKILL.md").is_file());
    assert!(
        !from_clone(&["skill-path", "--own", "x"], None)
            .status
            .success()
    );
    let answer: Value =
        serde_json::from_slice(&from_clone(&["claude-context"], Some(start)).stdout).unwrap();
    assert_ne!(
        answer["hookSpecificOutput"]["additionalContext"],
        "the clone's part\n"
    );
}
