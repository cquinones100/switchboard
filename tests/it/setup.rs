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

    /// Settings' "Load Switchboard as a plugin" off: the old way.
    fn old_way(&self) {
        self.settings(json!({ "claude_plugin": false }));
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
fn a_statusline_script_of_your_own_stays_outside_switchboard_s_panes_and_the_tmux_version_s_gives_way()
 {
    let h = Home::new("setup-statusline");
    // The old way, which merges Switchboard's statusline into your settings.
    h.old_way();
    let file = h.path(".claude/settings.json");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    // The command Claude Code's /statusline writes, as the tmux version's
    // setup did too.
    let command = "bash \"$HOME/.claude/statusline-command.sh\"";
    let settings = json!({ "statusLine": { "type": "command", "command": command } });
    fs::write(&file, settings.to_string()).unwrap();

    // A script of your own, as /statusline makes it: kept outside
    // Switchboard's panes, with Switchboard's in them.
    let script = h.path(".claude/statusline-command.sh");
    fs::write(&script, "#!/bin/bash\necho mine\n").unwrap();
    ok(&h.run(&["setup", "claude"]));
    let merged: Value = serde_json::from_str(&read(&file)).unwrap();
    let wrapped = merged["statusLine"]["command"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        wrapped,
        format!("{}{command}", switchboard::setup::claude_settings::IN_PANES)
    );
    // Run as Claude Code runs it: yours outside a pane, Switchboard's in one.
    common::stand_in::install(&h.path("bin/switchboard"), "#!/bin/sh\necho ours\n");
    let draw = |pane: Option<&str>| {
        let mut sh = Command::new("sh");
        sh.args(["-c", &wrapped])
            .env("HOME", &h.home)
            .env("PATH", format!("{}:/usr/bin:/bin", h.path("bin").display()))
            .env_remove("SWITCHBOARD");
        if let Some(socket) = pane {
            sh.env("SWITCHBOARD", socket);
        }
        String::from_utf8(sh.output().unwrap().stdout).unwrap()
    };
    assert_eq!(draw(None), "mine\n");
    assert_eq!(draw(Some("/tmp/sock")), "ours\n");
    ok(&h.run(&["setup", "claude"]));
    let again: Value = serde_json::from_str(&read(&file)).unwrap();
    assert_eq!(
        again["statusLine"]["command"],
        wrapped.as_str(),
        "a second run changes nothing"
    );
    assert!(
        merged["permissions"]["allow"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
        "the permissions are still added: {merged}"
    );

    // A link into your dotfiles' claude/ is yours too.
    fs::create_dir_all(h.path("dotfiles/claude")).unwrap();
    fs::write(
        h.path("dotfiles/claude/statusline-command.sh"),
        "#!/bin/bash\necho mine\n",
    )
    .unwrap();
    fs::remove_file(&script).unwrap();
    std::os::unix::fs::symlink(h.path("dotfiles/claude/statusline-command.sh"), &script).unwrap();
    ok(&h.run(&["setup", "claude"]));
    let kept: Value = serde_json::from_str(&read(&file)).unwrap();
    assert_eq!(
        kept["statusLine"]["command"],
        wrapped.as_str(),
        "yours stays"
    );

    // The tmux version's, a link into its clone: Switchboard's takes its place.
    fs::remove_file(&script).unwrap();
    std::os::unix::fs::symlink("/gone/switchboard/claude/statusline-command.sh", &script).unwrap();
    ok(&h.run(&["setup", "claude"]));
    let merged: Value = serde_json::from_str(&read(&file)).unwrap();
    assert_eq!(merged["statusLine"]["command"], "switchboard statusline");

    // The link gone (the leftovers part removes it): the command the tmux
    // version's settings had runs nothing, and gives way; another is yours.
    fs::remove_file(&script).unwrap();
    for (before, after) in [
        (
            "bash ~/.claude/statusline-command.sh",
            "switchboard statusline".to_string(),
        ),
        (
            command,
            format!("{}{command}", switchboard::setup::claude_settings::IN_PANES),
        ),
    ] {
        let settings = json!({ "statusLine": { "type": "command", "command": before } });
        fs::write(&file, settings.to_string()).unwrap();
        ok(&h.run(&["setup", "claude"]));
        let merged: Value = serde_json::from_str(&read(&file)).unwrap();
        assert_eq!(merged["statusLine"]["command"], after.as_str(), "{before}");
    }

    // As a plugin, which draws Switchboard's in its panes itself, yours is
    // as it was before Switchboard's went around it, its other fields kept;
    // a second run changes nothing.
    fs::write(&script, "#!/bin/bash\necho mine\n").unwrap();
    let wrapped = json!({ "statusLine": { "type": "command", "command": wrapped, "padding": 1 } });
    fs::write(&file, wrapped.to_string()).unwrap();
    h.settings(json!({}));
    let said = ok(&h.run(&["setup", "claude"]));
    assert!(
        said.contains("take the statusline earlier versions added out of ~/.claude/settings.json"),
        "{said}"
    );
    let back: Value = serde_json::from_str(&read(&file)).unwrap();
    assert_eq!(
        back,
        json!({ "statusLine": { "type": "command", "command": command, "padding": 1 } })
    );
    ok(&h.run(&["setup", "claude"]));
    assert_eq!(serde_json::from_str::<Value>(&read(&file)).unwrap(), back);
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
fn the_plugin_moves_under_claude_and_leaves_a_link_where_earlier_builds_wrote_it() {
    let h = Home::new("setup-plugin-move");
    // An earlier build's plugin, with packages a skill installed for itself.
    let former = h.path(".local/share/switchboard/plugin");
    let packages = former.join("skills/walkthrough/scripts/node_modules/x");
    fs::create_dir_all(&packages).unwrap();
    fs::write(packages.join("index.js"), "").unwrap();
    let said = ok(&h.run(&["setup", "plugin"]));
    assert!(
        said.contains("move Switchboard's plugin from ~/.local/share/switchboard/plugin to ~/.local/share/switchboard/claude/plugin, a link left where it was for the tabs open now"),
        "{said}"
    );
    let now = h.path(".local/share/switchboard/claude/plugin");
    assert_eq!(fs::read_link(&former).unwrap(), now);
    assert!(now.join(".claude-plugin/plugin.json").is_file());
    assert!(
        now.join("skills/walkthrough/scripts/node_modules/x/index.js")
            .is_file()
    );
    assert_eq!(ok(&h.run(&["setup", "plugin"])), "plugin: up to date\n");
}

#[test]
fn as_a_plugin_a_settings_link_leading_nowhere_is_said_and_left_alone() {
    let h = Home::new("setup-linked");
    std::os::unix::fs::symlink(BIN, h.path("bin/switchboard")).unwrap();
    let is_link = |p: &Path| fs::symlink_metadata(p).unwrap().file_type().is_symlink();
    // The tmux version's link into its clone, which a pull past the rewrite
    // removed: Claude Code reads none of your settings, so it is left to do,
    // with how; the link itself is changed only when you ask.
    let file = h.path(".claude/settings.json");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(h.path("switchboard/claude/settings.json"), &file).unwrap();
    let said = h.run_with(&["setup"], "\n\n\n");
    assert!(!said.status.success());
    let said = stdout(&said);
    assert!(!said.contains("[Y/n]"), "{said}");
    assert!(
        said.contains("✗ ~/.claude/settings.json links to ~/switchboard/claude/settings.json, which isn't there, so Claude Code reads none of your settings: switchboard setup claude makes it a file of your own"),
        "{said}"
    );
    assert!(is_link(&file));

    // A link of yours into your dotfiles, without Switchboard's part, is
    // left as it is, and nothing said of it.
    let dotfiles = h.path("dotfiles/claude/settings.json");
    fs::create_dir_all(dotfiles.parent().unwrap()).unwrap();
    fs::write(&dotfiles, "{\"model\":\"opus\"}\n").unwrap();
    fs::remove_file(&file).unwrap();
    std::os::unix::fs::symlink(&dotfiles, &file).unwrap();
    let said = ok(&h.run_with(&["setup"], "\n\n\n"));
    assert!(!said.contains("✗"), "{said}");
    assert!(is_link(&file));
    assert_eq!(read(&dotfiles), "{\"model\":\"opus\"}\n");

    // That link leading nowhere is said too, but not as a clone's.
    fs::remove_file(&dotfiles).unwrap();
    let said = stdout(&h.run_with(&["setup"], "\n\n\n"));
    assert!(
        said.contains("✗ ~/.claude/settings.json links to ~/dotfiles/claude/settings.json, which isn't there, so Claude Code reads none of your settings\n"),
        "{said}"
    );
    assert!(!said.contains("makes it a file of your own"), "{said}");
    assert!(is_link(&file));
}

#[test]
fn claude_merges_switchboard_s_settings_into_yours_once_and_makes_a_linked_file_your_own() {
    let h = Home::new("setup-claude");
    // The old way, which adds the hooks too.
    h.old_way();
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
        merged["statusLine"]["command"],
        format!(
            "{}my-statusline",
            switchboard::setup::claude_settings::IN_PANES
        ),
        "a statusline of your own stays outside Switchboard's panes"
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
    // The old way, which imports Switchboard's part.
    h.old_way();
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

#[test]
fn skills_are_written_keeping_one_of_your_own_and_brought_up_to_date() {
    let h = Home::new("setup-skills");
    // The old way, which writes them into ~/.claude/skills.
    h.old_way();
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
    // setup.sh's link to a script the tmux version's clone no longer has.
    fs::create_dir_all(h.path(".local/bin")).unwrap();
    std::os::unix::fs::symlink(
        "/somewhere/editor-configs/bin/codex-review",
        h.path(".local/bin/codex-review"),
    )
    .unwrap();

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
    for project in fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join(".claude/skills"))
        .unwrap()
        .flatten()
    {
        let name = project.file_name();
        assert!(
            !skills.join(&name).exists(),
            "the repository's own {name:?} is never written"
        );
    }
    assert!(
        skills.join("walkthrough/scripts/record.js").is_file(),
        "the scripts that stay with their skills"
    );
    assert!(!skills.join("today/scripts").exists());
    // The commands a repository's own skill may run by their bare names run
    // this binary, the link that led nowhere replaced.
    let bin = fs::canonicalize(common::BIN).unwrap();
    for name in ["skill-path", "codex-review"] {
        assert_eq!(
            fs::canonicalize(h.path(".local/bin").join(name)).unwrap(),
            bin,
            "{name}: {first}"
        );
    }
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

#[test]
fn the_guided_setup_the_old_way_asks_for_each_part_in_claude_and_keeps_them_up_to_date() {
    let h = Home::new("setup-guided-old");
    std::os::unix::fs::symlink(BIN, h.path("bin/switchboard")).unwrap();
    h.old_way();

    let check = ok(&h.run(&["setup", "--check"]));
    assert!(
        check.contains("switchboard setup would offer: plugin, claude, claude-md, skills"),
        "{check}"
    );
    assert!(check.contains("– Switchboard as a plugin"), "{check}");

    // Yes to each, by Enter or y.
    let said = ok(&h.run_with(&["setup"], "\ny\nY\n"));
    assert!(
        said.contains("✓ made ~/.claude/settings.json with Switchboard's Claude Code settings"),
        "no settings of yours to keep a copy of: {said}"
    );
    assert!(
        said.contains("✓ your CLAUDE.md imports Switchboard's part"),
        "{said}"
    );
    assert!(said.contains("✓ installed Switchboard's skills"), "{said}");
    assert!(said.ends_with("Ready.\n"), "{said}");
    assert!(read(&h.path(".claude/settings.json")).contains("switchboard hook Stop"));
    assert_eq!(
        read(&h.path(".claude/CLAUDE.md")),
        "@~/.local/share/switchboard/switchboard.md\n"
    );
    assert!(h.path(".claude/skills/wt/.switchboard-skill").is_file());

    // A second run asks nothing and changes nothing.
    let before = snapshot(&h.home);
    let second = ok(&h.run(&["setup"]));
    assert!(!second.contains("[Y/n]"), "{second}");
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
}

/// An install made the old way by an earlier version, before the setting:
/// Switchboard's hooks, part of CLAUDE.md and skills in `~/.claude`, and no
/// choice in the settings file.
fn earlier_install(h: &Home) {
    h.old_way();
    ok(&h.run(&["setup", "claude", "claude-md", "skills"]));
    h.settings(json!({}));
}

#[test]
fn a_new_build_writes_the_plugin_and_keeps_an_earlier_install_the_old_way_moving_nothing() {
    let h = Home::new("setup-refresh");
    let plugin = h.path(".local/share/switchboard/claude/plugin");
    // Nothing set up: the plugin and the build's mark, nothing of yours.
    ok(&h.run(&["setup", "--refresh"]));
    assert!(!h.path(".claude").exists());
    assert!(plugin.join("skills/wt/SKILL.md").is_file());
    assert!(h.path(".local/share/switchboard/setup-build").is_file());
    assert_eq!(h.saved(), json!({}), "the plugin, by default");

    // Set up by an earlier version, whose skills and part of CLAUDE.md are
    // older than this build's.
    earlier_install(&h);
    fs::write(h.path(".claude/skills/wt/SKILL.md"), "an older wt\n").unwrap();
    fs::write(h.path(".local/share/switchboard/switchboard.md"), "older\n").unwrap();
    fs::write(
        h.path(".local/share/switchboard/setup-build"),
        "0.0.1 (old)\n",
    )
    .unwrap();
    let settings = read(&h.path(".claude/settings.json"));
    let said = ok(&h.run(&["setup", "--refresh"]));
    assert_eq!(said, "", "quiet");
    assert_eq!(
        h.saved()["claude_plugin"],
        json!(false),
        "the old way kept, so the panes don't load the plugin on top of the hooks"
    );
    assert!(
        h.path(".local/state/switchboard/claude-plugin-unasked")
            .is_file(),
        "noted as not asked, so the guided setup still asks"
    );
    assert_eq!(
        read(&h.path(".claude/settings.json")),
        settings,
        "nothing moved"
    );
    assert_ne!(read(&h.path(".claude/skills/wt/SKILL.md")), "an older wt\n");
    assert_ne!(
        read(&h.path(".local/share/switchboard/switchboard.md")),
        "older\n"
    );

    // Once per build: what is changed after is left alone.
    fs::write(h.path(".claude/skills/wt/SKILL.md"), "edited\n").unwrap();
    ok(&h.run(&["setup", "--refresh"]));
    assert_eq!(read(&h.path(".claude/skills/wt/SKILL.md")), "edited\n");
}

#[test]
fn as_a_plugin_a_new_build_takes_out_only_the_hooks_an_earlier_version_left() {
    let h = Home::new("setup-refresh-plugin");
    earlier_install(&h);
    ok(&h.run(&["setting", "claude_plugin", "on"]));
    // Chosen, so a new build doesn't keep the old way until asked.
    h.settings(json!({ "claude_plugin": true }));
    // An earlier version's hooks and statusline in your settings again.
    let file = h.path(".claude/settings.json");
    let mut earlier: Value = serde_json::from_str(&read(&file)).unwrap();
    earlier["statusLine"] = json!({ "type": "command", "command": "switchboard statusline" });
    earlier["hooks"] = json!({ "Stop": [{ "hooks": [{ "type": "command", "command": "switchboard hook Stop" }] }] });
    fs::write(&file, earlier.to_string()).unwrap();
    fs::write(
        h.path(".local/share/switchboard/setup-build"),
        "0.0.1 (old)\n",
    )
    .unwrap();
    // The hooks, which would run twice in a pane, go quietly; the rest of
    // your file waits for the guided setup, which says what it takes out.
    assert_eq!(ok(&h.run(&["setup", "--refresh"])), "", "quiet");
    let after: Value = serde_json::from_str(&read(&file)).unwrap();
    assert!(after.get("hooks").is_none(), "{after}");
    let mut kept = earlier.clone();
    kept.as_object_mut().unwrap().remove("hooks");
    assert_eq!(after, kept);
}

#[test]
fn the_guided_setup_asks_an_earlier_install_once_whether_to_load_switchboard_as_a_plugin() {
    let h = Home::new("setup-offer");
    std::os::unix::fs::symlink(BIN, h.path("bin/switchboard")).unwrap();
    earlier_install(&h);
    let installed = snapshot(&h.path(".claude"));
    const ASKED: &str = "Load Switchboard as a plugin in its panes?";

    // --check says it would ask, and changes nothing.
    let check = ok(&h.run(&["setup", "--check"]));
    assert!(check.contains("loading Switchboard as a plugin"), "{check}");
    assert_eq!(snapshot(&h.path(".claude")), installed);
    assert_eq!(h.saved(), json!({}));

    // A closed input says no: the old way, recorded, nothing moved.
    let no = ok(&h.run(&["setup"]));
    assert!(no.contains(ASKED), "{no}");
    assert_eq!(h.saved()["claude_plugin"], json!(false));
    assert_eq!(snapshot(&h.path(".claude")), installed);
    // Asked once.
    let again = ok(&h.run(&["setup"]));
    assert!(!again.contains(ASKED), "{again}");

    // Yes: moved, and the setting left on.
    h.settings(json!({}));
    let yes = ok(&h.run_with(&["setup"], "y\n"));
    assert!(
        yes.contains("✓ Switchboard loads as a plugin in its panes; a tab open now takes the change once it is reopened or the workspace restarts"),
        "{yes}"
    );
    assert!(yes.ends_with("Ready.\n"), "{yes}");
    assert_eq!(h.saved(), json!({}));
    assert!(!read(&h.path(".claude/settings.json")).contains("switchboard hook"));
    assert!(
        !h.path(".claude/CLAUDE.md").exists(),
        "setup made it only to import Switchboard's part"
    );
    assert!(!h.path(".claude/skills/wt").exists());
    let after = ok(&h.run(&["setup"]));
    assert!(!after.contains("[Y/n]"), "{after}");
}

#[test]
fn the_guided_setup_still_asks_an_earlier_install_a_new_build_kept_the_old_way() {
    let h = Home::new("setup-unasked");
    std::os::unix::fs::symlink(BIN, h.path("bin/switchboard")).unwrap();
    earlier_install(&h);
    let unasked = h.path(".local/state/switchboard/claude-plugin-unasked");
    const ASKED: &str = "Load Switchboard as a plugin in its panes?";
    const OPEN_TABS: &str = "a tab open now takes the change";

    // A new build's first run keeps the old way, noting no one was asked.
    ok(&h.run(&["setup", "--refresh"]));
    assert_eq!(h.saved()["claude_plugin"], json!(false));
    assert!(unasked.is_file());
    let installed = snapshot(&h.path(".claude"));

    // --check says the guided setup would ask, and changes nothing.
    let check = ok(&h.run(&["setup", "--check"]));
    assert!(check.contains("loading Switchboard as a plugin"), "{check}");
    assert!(unasked.is_file());

    // A closed input says no: the old way, as it was, and asked once.
    let no = ok(&h.run(&["setup"]));
    assert!(no.contains(ASKED), "{no}");
    assert!(
        !no.contains(OPEN_TABS),
        "nothing changed for the panes: {no}"
    );
    assert!(!unasked.exists());
    assert_eq!(h.saved()["claude_plugin"], json!(false));
    assert_eq!(snapshot(&h.path(".claude")), installed);
    let again = ok(&h.run(&["setup"]));
    assert!(!again.contains(ASKED), "{again}");

    // Kept unasked by another new build, then yes: moved, the setting on.
    h.settings(json!({}));
    fs::write(
        h.path(".local/share/switchboard/setup-build"),
        "0.0.1 (old)\n",
    )
    .unwrap();
    ok(&h.run(&["setup", "--refresh"]));
    assert!(unasked.is_file());
    let yes = ok(&h.run_with(&["setup"], "y\n"));
    assert!(
        yes.contains(&format!(
            "✓ Switchboard loads as a plugin in its panes; {OPEN_TABS}"
        )),
        "{yes}"
    );
    assert!(yes.ends_with("Ready.\n"), "{yes}");
    assert!(!unasked.exists());
    assert_eq!(h.saved(), json!({}));
    assert!(!read(&h.path(".claude/settings.json")).contains("switchboard hook"));
    assert!(!h.path(".claude/skills/wt").exists());
    assert!(!ok(&h.run(&["setup"])).contains(ASKED));

    // Changing the setting yourself is a choice too.
    fs::write(&unasked, "").unwrap();
    ok(&h.run(&["setting", "claude_plugin", "off"]));
    assert!(!unasked.exists());
}

#[test]
fn the_setting_moves_exactly_switchboard_s_parts_into_the_plugin_and_back() {
    let h = Home::new("setup-move");
    // Yours: settings with a model, a hook and a permission, a CLAUDE.md,
    // and a skill by one of Switchboard's names.
    fs::create_dir_all(h.path(".claude/skills/today")).unwrap();
    fs::write(h.path(".claude/skills/today/SKILL.md"), "mine\n").unwrap();
    fs::write(h.path(".claude/CLAUDE.md"), "# Mine\n- be brief\n").unwrap();
    let your_hooks =
        json!({ "Stop": [{ "hooks": [{ "type": "command", "command": "say done" }] }] });
    fs::write(
        h.path(".claude/settings.json"),
        json!({ "model": "opus", "permissions": { "allow": ["Bash(make *)"] }, "hooks": your_hooks })
            .to_string(),
    )
    .unwrap();
    let skills_left = || -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(h.path(".claude/skills"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    let settings =
        || -> Value { serde_json::from_str(&read(&h.path(".claude/settings.json"))).unwrap() };
    let up_to_date = || {
        for part in ["claude", "claude-md", "skills"] {
            assert_eq!(
                ok(&h.run(&["setup", part])),
                format!("{part}: up to date\n")
            );
        }
    };

    // The old way, set up.
    h.old_way();
    ok(&h.run(&["setup", "claude", "claude-md", "skills"]));
    assert!(read(&h.path(".claude/settings.json")).contains("switchboard hook Stop"));
    assert_eq!(
        read(&h.path(".claude/CLAUDE.md")),
        "# Mine\n- be brief\n\n@~/.local/share/switchboard/switchboard.md\n"
    );
    assert!(h.path(".claude/skills/wt/SKILL.md").is_file());
    // A file of yours in one of Switchboard's skill folders, and the
    // packages one installed for itself.
    fs::write(h.path(".claude/skills/wt/notes.md"), "mine\n").unwrap();
    let modules = h.path(".claude/skills/walkthrough/scripts/node_modules/x");
    fs::create_dir_all(&modules).unwrap();
    fs::write(modules.join("index.js"), "").unwrap();

    // On: Switchboard moves into its plugin, and only Switchboard's parts.
    // The tabs open now have the old environment, which it says how to
    // leave.
    const OPEN_TABS: &str = "a tab open now takes the change once it is reopened or the workspace restarts (switchboard kill-server, then switchboard)\n";
    let on = ok(&h.run(&["setting", "claude_plugin", "on"]));
    assert!(on.starts_with("on\n"), "{on}");
    assert!(
        on.ends_with(&format!(
            "Switchboard loads as a plugin in its panes, and none of it is in ~/.claude; {OPEN_TABS}"
        )),
        "{on}"
    );
    assert_eq!(h.saved(), json!({}));
    let s = settings();
    assert_eq!(s["hooks"], your_hooks, "yours kept, Switchboard's out");
    assert_eq!(s["model"], "opus");
    assert!(
        s.get("statusLine").is_none(),
        "Switchboard's statusline out, which its plugin draws: {s}"
    );
    // The permissions an earlier version added stay, which a plugin can't
    // carry yet.
    let allow = s["permissions"]["allow"].as_array().unwrap();
    assert!(allow.contains(&json!("Bash(make *)")));
    assert!(allow.contains(&json!("Bash(switchboard tabs)")));
    assert_eq!(read(&h.path(".claude/CLAUDE.md")), "# Mine\n- be brief\n");
    assert!(!h.path(".local/share/switchboard/switchboard.md").exists());
    // Switchboard's skills out, packages and all, but a folder holding a
    // file of yours, which stays with it, said.
    assert_eq!(skills_left(), ["today", "wt"]);
    assert_eq!(read(&h.path(".claude/skills/today/SKILL.md")), "mine\n");
    assert!(
        on.contains(
            "leave ~/.claude/skills/wt for the files in it Switchboard didn't write (notes.md)"
        ),
        "{on}"
    );
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
    // Again: nothing to move.
    let before = snapshot(&h.path(".claude"));
    ok(&h.run(&["setup", "--move"]));
    assert_eq!(snapshot(&h.path(".claude")), before);
    up_to_date();

    // Off: back into ~/.claude, yours kept.
    let off = ok(&h.run(&["setting", "claude_plugin", "off"]));
    assert!(
        off.ends_with(&format!("Switchboard's hooks, skills and instructions are in ~/.claude, for every Claude of yours; {OPEN_TABS}")),
        "{off}"
    );
    assert_eq!(h.saved()["claude_plugin"], json!(false));
    let s = settings();
    let text = s["hooks"].to_string();
    assert!(
        text.contains("say done") && text.contains("switchboard hook Stop"),
        "{text}"
    );
    assert_eq!(
        read(&h.path(".claude/CLAUDE.md")),
        "# Mine\n- be brief\n\n@~/.local/share/switchboard/switchboard.md\n"
    );
    assert!(h.path(".claude/skills/wt/.switchboard-skill").is_file());
    assert_eq!(
        read(&h.path(".claude/skills/wt/notes.md")),
        "mine\n",
        "Switchboard's wt written beside yours"
    );
    assert_eq!(read(&h.path(".claude/skills/today/SKILL.md")), "mine\n");
    let before = snapshot(&h.path(".claude"));
    ok(&h.run(&["setup", "--move"]));
    assert_eq!(snapshot(&h.path(".claude")), before);
    up_to_date();
}

#[test]
fn a_move_only_adds_or_takes_out_switchboard_s_import_in_a_claude_md_of_yours() {
    let h = Home::new("setup-move-md");
    // Your rules named, and a CLAUDE.md of yours that setup didn't write.
    fs::write(h.path("rules.md"), "- my rules\n").unwrap();
    fs::create_dir_all(h.path(".config/switchboard")).unwrap();
    fs::write(
        h.path(".config/switchboard/claude.json"),
        json!({ "path": "~/rules.md" }).to_string(),
    )
    .unwrap();
    fs::create_dir_all(h.path(".claude")).unwrap();
    fs::write(h.path(".claude/CLAUDE.md"), "# Mine\n").unwrap();
    let aside = || backups(&h.path(".claude"), "CLAUDE.md.bak");

    // Off: back into ~/.claude, asking nothing, so yours only gets the line.
    ok(&h.run(&["setting", "claude_plugin", "off"]));
    assert_eq!(
        read(&h.path(".claude/CLAUDE.md")),
        "# Mine\n\n@~/.local/share/switchboard/switchboard.md\n"
    );
    assert_eq!(aside(), 0, "never moved aside");
    assert!(h.path(".local/share/switchboard/switchboard.md").is_file());

    // On: the line taken out, yours as it was.
    ok(&h.run(&["setting", "claude_plugin", "on"]));
    assert_eq!(read(&h.path(".claude/CLAUDE.md")), "# Mine\n");
    assert_eq!(aside(), 0);

    // The part run by name still writes the import of your rules, yours
    // moved aside first.
    ok(&h.run(&["setup", "claude-md"]));
    assert_eq!(aside(), 1);
    assert!(read(&h.path(".claude/CLAUDE.md")).contains("@~/rules.md"));
}

#[test]
fn a_claude_code_too_old_for_the_plugin_keeps_the_old_way_whatever_the_setting() {
    let h = Home::new("setup-old-claude");
    h.claude_version("2.1.100");
    earlier_install(&h);
    let installed = snapshot(&h.path(".claude"));

    // Setup keeps the old way, says why, and turns the setting off.
    let said = ok(&h.run(&["setup", "claude", "claude-md", "skills"]));
    assert!(
        said.contains(
            "Claude Code 2.1.100 is older than 2.1.280, which loads Switchboard as a plugin"
        ),
        "{said}"
    );
    assert_eq!(snapshot(&h.path(".claude")), installed, "nothing taken out");
    assert_eq!(h.saved()["claude_plugin"], json!(false));

    // Turned on, it stays off, saying why.
    let on = ok(&h.run(&["setting", "claude_plugin", "on"]));
    assert!(on.contains("is older than 2.1.280"), "{on}");
    assert_eq!(h.saved()["claude_plugin"], json!(false));
    assert_eq!(snapshot(&h.path(".claude")), installed);

    // The guided setup says so too, and asks nothing about the plugin.
    std::os::unix::fs::symlink(BIN, h.path("bin/switchboard")).unwrap();
    let guided = ok(&h.run(&["setup"]));
    assert!(
        !guided.contains("Load Switchboard as a plugin in its panes?"),
        "{guided}"
    );

    // Once Claude Code is new enough, turning it on moves Switchboard over.
    h.claude_version("2.1.280");
    ok(&h.run(&["setting", "claude_plugin", "on"]));
    assert_eq!(h.saved(), json!({}));
    assert!(!read(&h.path(".claude/settings.json")).contains("switchboard hook"));
}

#[test]
fn managed_settings_that_turn_sideloading_off_keep_the_old_way_whatever_the_setting() {
    let h = Home::new("setup-managed");
    std::os::unix::fs::symlink(BIN, h.path("bin/switchboard")).unwrap();
    h.managed(json!({ "disableSideloadFlags": true }));

    // A new install: setup keeps Switchboard in ~/.claude, turns the setting
    // off, so no pane names the plugin, and says why.
    let said = ok(&h.run_with(&["setup"], "\n\n\n"));
    assert!(said.contains("set disableSideloadFlags"), "{said}");
    assert!(
        !said.contains("Load Switchboard as a plugin in its panes?"),
        "{said}"
    );
    assert_eq!(h.saved()["claude_plugin"], json!(false));
    assert!(read(&h.path(".claude/settings.json")).contains("switchboard hook Stop"));
    assert!(h.path(".claude/skills/wt/.switchboard-skill").is_file());
    let installed = snapshot(&h.path(".claude"));

    // Turned on, it stays off, saying why, and nothing moves.
    let on = ok(&h.run(&["setting", "claude_plugin", "on"]));
    assert!(on.contains("disableSideloadFlags"), "{on}");
    assert_eq!(h.saved()["claude_plugin"], json!(false));
    assert_eq!(snapshot(&h.path(".claude")), installed);

    // A policy that allows it: turning it on moves Switchboard over.
    h.managed(json!({ "disableSideloadFlags": false }));
    ok(&h.run(&["setting", "claude_plugin", "on"]));
    assert_eq!(h.saved(), json!({}));
    assert!(!read(&h.path(".claude/settings.json")).contains("switchboard hook"));
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
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
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
fn what_the_tmux_version_left_is_tidied_after_asking_and_once() {
    use std::os::unix::fs::symlink;
    let h = Home::new("setup-leftovers");
    // A clone of the tmux version pulled past the rewrite: its bin/, tmux/
    // and claude/ gone, and the links setup.sh made into them left.
    let clone = h.path("switchboard");
    fs::create_dir_all(clone.join("assets/skills")).unwrap();
    fs::write(clone.join("Cargo.toml"), "").unwrap();
    // Its history has the files Claude Code still uses.
    let c = clone.to_str().unwrap();
    h.init(&clone, "main");
    fs::create_dir_all(clone.join("claude/hooks")).unwrap();
    fs::create_dir_all(clone.join("claude/commands")).unwrap();
    fs::write(clone.join("claude/hooks/enforce.sh"), "echo guard\n").unwrap();
    fs::write(clone.join("claude/otel-headers.sh"), "echo headers\n").unwrap();
    fs::write(clone.join("claude/commands/pr-view.md"), "Open the PR\n").unwrap();
    std::process::Command::new("chmod")
        .args(["755"])
        .arg(clone.join("claude/hooks/enforce.sh"))
        .status()
        .unwrap();
    h.git(&["-C", c, "add", "-A"]);
    h.git(&["-C", c, "commit", "-q", "-m", "tmux"]);
    h.git(&["-C", c, "rm", "-q", "-r", "claude"]);
    h.git(&["-C", c, "commit", "-q", "-m", "rewrite"]);
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
    symlink(
        clone.join("claude/otel-headers.sh"),
        h.path(".claude/otel-headers.sh"),
    )
    .unwrap();
    // A hook the settings run but the history never had is left, and said.
    symlink(
        clone.join("claude/hooks/missing.sh"),
        h.path(".claude/hooks/missing.sh"),
    )
    .unwrap();
    fs::write(
        h.path(".claude/settings.json"),
        json!({
            "hooks": { "PreToolUse": [{ "hooks": [
                { "type": "command", "command": "bash ~/.claude/hooks/enforce.sh" },
                { "type": "command", "command": "bash $HOME/.claude/hooks/missing.sh" },
            ]}]},
            "otelHeadersHelper": "bash ~/.claude/otel-headers.sh",
            // The claude part moves this one onto switchboard statusline.
            "statusLine": { "type": "command", "command": "bash ~/.claude/statusline-command.sh" },
            "permissions": { "allow": ["Bash(~/.claude/statusline-command.sh)"] },
        })
        .to_string(),
    )
    .unwrap();
    // A link of your own leading nowhere, and one into the clone that still
    // leads somewhere, are left alone.
    symlink(h.path("elsewhere/bin/tool"), h.path(".local/bin/tool")).unwrap();
    fs::write(clone.join("Cargo.lock"), "").unwrap();
    symlink(clone.join("Cargo.lock"), h.path(".local/bin/kept")).unwrap();
    // ~/.zshrc as setup.sh left it, through a link into your dotfiles, with
    // the tmux server's command wrapped in a function of the binary's name.
    fs::create_dir_all(h.path("dotfiles")).unwrap();
    let zshrc = "export A=1\neval \"$(worktree-from-ticket init zsh)\"\neval \"$(worktree-from-note init zsh)\"\n\
                 # Switchboard runs on a tmux server of its own: `switchboard` opens it.\n\
                 switchboard() { command tmux -L switchboard new-session -A -s main \"$@\"; }\n";
    fs::write(h.path("dotfiles/zshrc"), zshrc).unwrap();
    symlink(h.path("dotfiles/zshrc"), h.path(".zshrc")).unwrap();

    let left = h.run(&["setup", "--check", "leftovers"]);
    assert!(
        stderr(&left).contains("~/.claude/hooks/missing.sh leads to"),
        "{}",
        stderr(&left)
    );
    // Taken out of the settings, the missing one goes as any other.
    let mut settings: serde_json::Value =
        serde_json::from_str(&read(&h.path(".claude/settings.json"))).unwrap();
    settings["hooks"]["PreToolUse"][0]["hooks"]
        .as_array_mut()
        .unwrap()
        .pop();
    fs::write(h.path(".claude/settings.json"), settings.to_string()).unwrap();

    let check = ok(&h.run(&["setup", "--check", "leftovers"]));
    assert!(
        check.contains("would: remove ~/.local/bin/worktree-from-ticket"),
        "{check}"
    );
    assert!(
        check.contains("would: take out the tmux version's worktree lines, and take out the switchboard function or alias that starts tmux in ~/.zshrc"),
        "{check}"
    );
    assert!(
        check.contains("would: replace ~/.claude/hooks/enforce.sh, which led to what the tmux version had, with a copy of it from ~/switchboard's history, since ~/.claude/settings.json runs it"),
        "{check}"
    );
    assert!(
        check.contains("would: replace ~/.claude/commands, which led to what the tmux version had, with a copy of it from ~/switchboard's history, since Claude Code reads it"),
        "{check}"
    );
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
        ".claude/hooks/missing.sh",
        ".claude/statusline-command.sh",
    ] {
        assert!(h.path(gone).symlink_metadata().is_err(), "{gone}");
    }
    // What Claude Code still uses is a file of its own, as it was.
    for (path, held) in [
        (".claude/hooks/enforce.sh", "echo guard\n"),
        (".claude/otel-headers.sh", "echo headers\n"),
        (".claude/commands/pr-view.md", "Open the PR\n"),
    ] {
        assert!(
            !h.path(path).symlink_metadata().unwrap().is_symlink(),
            "{path}"
        );
        assert_eq!(read(&h.path(path)), held, "{path}");
    }
    assert!(
        !h.path(".claude/commands")
            .symlink_metadata()
            .unwrap()
            .is_symlink()
    );
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &str| fs::metadata(h.path(p)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(".claude/hooks/enforce.sh"), 0o755);
        assert_eq!(mode(".claude/otel-headers.sh"), 0o644);
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
    assert_eq!(read(&h.path("dotfiles/zshrc")), "export A=1\n");
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
fn a_hooks_folder_the_old_settings_run_comes_back_whole() {
    use std::os::unix::fs::symlink;
    let h = Home::new("setup-leftovers-hooks");
    // The clone's guards went in two commits, the last of them a move, and
    // ~/.claude/settings.json and ~/.claude/hooks are still its links.
    let clone = h.path("switchboard");
    let c = clone.to_str().unwrap();
    h.init(&clone, "main");
    fs::create_dir_all(clone.join("assets/skills")).unwrap();
    fs::write(clone.join("Cargo.toml"), "").unwrap();
    fs::create_dir_all(clone.join("claude/hooks")).unwrap();
    fs::write(clone.join("claude/hooks/first.sh"), "echo first\n").unwrap();
    fs::write(clone.join("claude/hooks/second.sh"), "echo second\n").unwrap();
    fs::write(
        clone.join("claude/settings.json"),
        json!({ "hooks": { "PreToolUse": [{ "hooks": [
            { "type": "command", "command": "bash ~/.claude/hooks/first.sh" },
        ]}]}})
        .to_string(),
    )
    .unwrap();
    h.git(&["-C", c, "add", "-A"]);
    h.git(&["-C", c, "commit", "-q", "-m", "tmux"]);
    h.git(&[
        "-C",
        c,
        "rm",
        "-q",
        "claude/hooks/first.sh",
        "claude/settings.json",
    ]);
    h.git(&["-C", c, "commit", "-q", "-m", "rewrite"]);
    fs::create_dir_all(clone.join(".claude/hooks")).unwrap();
    h.git(&[
        "-C",
        c,
        "mv",
        "claude/hooks/second.sh",
        ".claude/hooks/second.sh",
    ]);
    h.git(&["-C", c, "commit", "-q", "-m", "move"]);
    // As a pull would leave it, with no empty folder where the guards were.
    fs::remove_dir_all(clone.join("claude")).unwrap();
    fs::create_dir_all(h.path(".claude")).unwrap();
    symlink(clone.join("claude/hooks"), h.path(".claude/hooks")).unwrap();
    symlink(
        clone.join("claude/settings.json"),
        h.path(".claude/settings.json"),
    )
    .unwrap();

    let done = ok(&h.run(&["setup", "leftovers"]));
    assert!(
        done.contains("replace ~/.claude/hooks, which led to what the tmux version had"),
        "{done}"
    );
    assert!(
        !h.path(".claude/hooks")
            .symlink_metadata()
            .unwrap()
            .is_symlink()
    );
    assert_eq!(read(&h.path(".claude/hooks/first.sh")), "echo first\n");
    assert_eq!(read(&h.path(".claude/hooks/second.sh")), "echo second\n");
}

#[test]
fn the_configs_the_clone_held_come_back_from_its_history() {
    use std::os::unix::fs::symlink;
    let h = Home::new("setup-leftovers-configs");
    // A clone whose rewrite removed the configs setup.sh linked, Neovim's
    // folder after one of its files went in an earlier commit.
    let clone = h.path("switchboard");
    let c = clone.to_str().unwrap();
    h.init(&clone, "main");
    fs::create_dir_all(clone.join("assets/skills")).unwrap();
    fs::write(clone.join("Cargo.toml"), "").unwrap();
    for (path, text) in [
        (
            "ghostty/config",
            "font-size = 14\ncommand = /bin/zsh -l -c '/opt/homebrew/bin/tmux new-session -A -s main'\n",
        ),
        ("karabiner/karabiner.json", "{}\n"),
        ("nvim/init.lua", "vim.o.number = true\n"),
        ("nvim/lua/old.lua", "old\n"),
        ("lazygit/config.yml", "gui: {}\n"),
        ("pgcli/config", "[main]\n"),
        ("foo/config", "renamed\n"),
        ("kitty/kitty.conf", "font_size 14\n"),
    ] {
        fs::create_dir_all(clone.join(path).parent().unwrap()).unwrap();
        fs::write(clone.join(path), text).unwrap();
    }
    h.git(&["-C", c, "add", "-A"]);
    h.git(&["-C", c, "commit", "-q", "-m", "tmux"]);
    h.git(&["-C", c, "rm", "-q", "nvim/lua/old.lua"]);
    h.git(&["-C", c, "commit", "-q", "-m", "earlier"]);
    for dir in [
        "ghostty",
        "karabiner",
        "nvim",
        "lazygit",
        "pgcli",
        "foo",
        "kitty",
    ] {
        h.git(&["-C", c, "rm", "-q", "-r", dir]);
    }
    h.git(&["-C", c, "commit", "-q", "-m", "rewrite"]);
    let link = |target: PathBuf, at: &str| {
        let at = h.path(at);
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        symlink(target, at).unwrap();
    };
    link(clone.join("ghostty/config"), ".config/ghostty/config");
    link(
        clone.join("karabiner/karabiner.json"),
        ".config/karabiner/karabiner.json",
    );
    link(clone.join("nvim"), ".config/nvim");
    link(
        clone.join("lazygit/config.yml"),
        "Library/Application Support/lazygit/config.yml",
    );
    link(clone.join("pgcli/config"), ".pgclirc");
    // A config folder that is a link into your dotfiles is looked through.
    link(clone.join("kitty/kitty.conf"), "dotfiles/kitty/kitty.conf");
    link(h.path("dotfiles/kitty"), ".config/kitty");
    // Into the clone's older name, gone: one its renamed history has, one
    // it never had.
    link(h.path("editor-configs/foo/config"), ".config/foo/config");
    link(h.path("editor-configs/bar/config"), ".config/bar/config");
    // One the renamed clone still has is left alone.
    link(
        h.path("editor-configs/Cargo.toml"),
        ".config/kept/Cargo.toml",
    );
    // A dead link of your own, a live one into the clone, and a dead one to
    // a path the clone's history never removed are left alone.
    link(h.path("elsewhere/config"), ".config/mine/config");
    link(clone.join("Cargo.toml"), ".config/live/config");
    link(clone.join("target/out"), ".config/built/out");

    let check = ok(&h.run(&["setup", "--check", "leftovers"]));
    assert!(
        check.contains("would: replace ~/.config/ghostty/config, which led into ~/switchboard, with its copy from ~/switchboard's history, its command starting switchboard in place of tmux"),
        "{check}"
    );
    assert!(
        check.contains("would: replace ~/.config/foo/config, which led into ~/editor-configs, with its copy from ~/switchboard's history"),
        "{check}"
    );
    assert!(
        check.contains("would: remove ~/.config/bar/config, which led to ~/editor-configs/bar/config, which no history has"),
        "{check}"
    );
    assert_eq!(check.matches("would: ").count(), 8, "{check}");
    assert!(
        h.path(".config/nvim")
            .symlink_metadata()
            .unwrap()
            .is_symlink()
    );

    let done = ok(&h.run(&["setup", "leftovers"]));
    for (path, held) in [
        (
            ".config/ghostty/config",
            format!("font-size = 14\ncommand = /bin/zsh -l -c '{BIN}'\n").as_str(),
        ),
        (".config/karabiner/karabiner.json", "{}\n"),
        (".config/nvim/init.lua", "vim.o.number = true\n"),
        (
            "Library/Application Support/lazygit/config.yml",
            "gui: {}\n",
        ),
        (".pgclirc", "[main]\n"),
        (".config/kitty/kitty.conf", "font_size 14\n"),
        (".config/foo/config", "renamed\n"),
    ] {
        assert!(
            !h.path(path).symlink_metadata().unwrap().is_symlink(),
            "{path}"
        );
        assert_eq!(read(&h.path(path)), held, "{path}");
    }
    assert!(
        !h.path(".config/nvim")
            .symlink_metadata()
            .unwrap()
            .is_symlink()
    );
    assert!(!h.path(".config/nvim/lua").exists(), "{done}");
    assert!(h.path(".config/bar/config").symlink_metadata().is_err());
    for kept in [
        ".config/mine/config",
        ".config/live/config",
        ".config/built/out",
        ".config/kept/Cargo.toml",
    ] {
        assert!(
            h.path(kept).symlink_metadata().unwrap().is_symlink(),
            "{kept}"
        );
    }
    assert_eq!(
        ok(&h.run(&["setup", "leftovers"])),
        "leftovers: up to date\n"
    );
}

#[test]
fn a_lazygit_config_is_written_only_where_lazygit_is_and_you_have_none() {
    let h = Home::new("setup-lazygit");
    std::os::unix::fs::symlink(BIN, h.path("bin/switchboard")).unwrap();
    let config = h.path(if cfg!(target_os = "macos") {
        "Library/Application Support/lazygit/config.yml"
    } else {
        ".config/lazygit/config.yml"
    });

    // Without lazygit, the part has nothing to set up and isn't offered.
    let without = h.run(&["setup", "lazygit"]);
    assert_eq!(without.status.code(), Some(1));
    assert!(stderr(&without).contains("lazygit isn't installed"));
    assert!(!stdout(&h.run(&["setup"])).contains("lazygit"));

    // With it, the guided setup asks, and a no writes nothing.
    common::stand_in::install(&h.path("bin/lazygit"), "#!/bin/sh\nexit 0\n");
    let closed = ok(&h.run(&["setup"]));
    assert!(
        closed.contains("Write a lazygit config") && closed.contains("switchboard setup lazygit"),
        "{closed}"
    );
    assert!(!config.exists());

    // Named, it writes the config once.
    let wrote = ok(&h.run(&["setup", "lazygit"]));
    assert!(wrote.contains("write a lazygit config"), "{wrote}");
    assert!(read(&config).contains("quitOnTopLevelReturn: true"));
    assert_eq!(ok(&h.run(&["setup", "lazygit"])), "lazygit: up to date\n");
    let guided = ok(&h.run(&["setup"]));
    assert!(
        guided.contains("✓ lazygit's config says what Esc does"),
        "{guided}"
    );

    // A config of your own is kept byte for byte, and the line to add said.
    fs::remove_file(&config).unwrap();
    let own = h.path(".config/jesseduffield/lazygit/config.yml");
    fs::create_dir_all(own.parent().unwrap()).unwrap();
    fs::write(&own, "gui:\n  sidePanelWidth: 0.2\n").unwrap();
    assert_eq!(ok(&h.run(&["setup", "lazygit"])), "lazygit: up to date\n");
    assert_eq!(read(&own), "gui:\n  sidePanelWidth: 0.2\n");
    assert!(!config.exists());
    let said = ok(&h.run(&["setup"]));
    assert!(
        said.contains(
            "add quitOnTopLevelReturn: true to ~/.config/jesseduffield/lazygit/config.yml"
        ),
        "{said}"
    );
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
            "lazygit",
            "leftovers"
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
        format!("{today}\t1.2500\t1.25\t4242\t2")
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
    h.settings(json!({ "ux2": true, "usage": true }));
    let line = statusline(&h, &payload, true);
    assert!(
        line.ends_with("Context: \x1b[97m12.5% used\x1b[0m"),
        "on by default, beside the Usage view's Sessions: {line:?}"
    );

    h.settings(json!({ "ux2": true, "usage": true, "statusline_context": false }));
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

    h.settings(json!({ "ux2": true, "usage": false, "statusline_context": false }));
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

#[test]
fn the_old_way_a_skill_the_clone_had_is_taken_out_once_the_build_goes_back_to_its_own() {
    let h = Home::new("setup-assets-old");
    h.old_way();
    let clone = h._dir.path().join("clone");
    fs::create_dir_all(clone.join("assets/skills/x")).unwrap();
    fs::write(clone.join("assets/skills/x/SKILL.md"), "the clone's x\n").unwrap();
    let setup = |h: &Home, args: &[&str]| {
        let mut cmd = h.command(BIN);
        cmd.env("SWITCHBOARD_CHECKOUT", &clone)
            .arg("setup")
            .args(args)
            .stdin(Stdio::null());
        ok(&cmd.output().unwrap())
    };
    let skills = |h: &Home| setup(h, &["skills"]);
    skills(&h);
    let x = h.path(".claude/skills/x");
    assert_eq!(read(&x.join("SKILL.md")), "the clone's x\n");

    // Renamed in the clone, the refresh the server runs takes the old name
    // out though no skill it ships is installed.
    fs::rename(clone.join("assets/skills/x"), clone.join("assets/skills/y")).unwrap();
    setup(&h, &["--refresh"]);
    assert!(!x.exists());
    assert_eq!(
        read(&h.path(".claude/skills/y/SKILL.md")),
        "the clone's x\n"
    );
    let x = h.path(".claude/skills/y");

    h.settings(json!({ "claude_plugin": false, "assets_from_checkout": false }));
    let back = skills(&h);
    assert!(
        back.contains("take the skills Switchboard no longer ships out of ~/.claude/skills (y)"),
        "{back}"
    );
    assert!(!x.exists());
    assert!(h.path(".claude/skills/wt/SKILL.md").is_file());
}
