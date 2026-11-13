//! The `plugin` part: Switchboard's skills, hooks and instructions as a
//! Claude Code plugin, `~/.local/share/switchboard/plugin`, which the Claude
//! sessions in Switchboard's panes load and Claude elsewhere doesn't. New
//! with this build: earlier ones (and setup.sh before them) wrote the hooks
//! into your `~/.claude/settings.json`, an import of Switchboard's part into
//! your `~/.claude/CLAUDE.md` and the skills into `~/.claude/skills`, so
//! every Claude of yours had them; the `claude`, `claude-md` and `skills`
//! parts now take those out.
//!
//! The plugin is:
//!
//! - `.claude-plugin/plugin.json`: its name, `switchboard`, the build's
//!   version, a line about it, and `hooks/modules.json` named as more
//!   hooks;
//! - `skills/`: every skill the binary carries (`assets/skills`, compiled in
//!   by build.rs), as the skills part wrote them, named `/switchboard:<name>`
//!   by Claude Code, and `/<name>` too while no skill of a project's or of
//!   yours has that name;
//! - `hooks/hooks.json`: `assets/claude-settings.json`'s hooks as they are
//!   (`switchboard hook <Event>`, how a tab knows its Claude's state), made
//!   from the asset here rather than kept twice, and a SessionStart hook
//!   running `switchboard claude-context`, which gives the session
//!   Switchboard's part of its instructions (src/cmd/claude_context.rs);
//! - `hooks/modules.json` and `hooks/statusline.js`: the statusline, as
//!   a hooks module (what Claude Code calls a mod, which it runs inside
//!   itself from 2.1.287) whose `settings.read` hook puts the asset's
//!   statusline (`switchboard statusline`) over the settings Claude Code
//!   has merged from its files. That answer is held in memory, so the
//!   sessions that load the plugin draw Switchboard's statusline, every
//!   other Claude yours, and no file changes; Settings' `pane_statusline`
//!   off, which it asks `switchboard setting` for, has it leave the
//!   statusline alone. Listed in a file plugin.json names rather than a
//!   `modules` key in hooks.json, so a Claude Code that can't load a module
//!   still reads the hooks.
//!
//! A plugin's own settings can't hold a statusline (Claude Code takes only
//! `agent` from them), hence the module; and the same answer for the
//! permission rules doesn't reach Claude Code's permission check, so as a
//! plugin those are in no file Switchboard writes. Every pane's environment
//! names the plugin in `CLAUDE_CODE_PLUGIN_DIRS` while Settings'
//! `claude_plugin` is on (`env_value`, src/mux/server.rs), which Claude Code
//! 2.1.280 and later load for the session as `--plugin-dir` does, beside
//! your own plugins, whoever starts it there.
//!
//! It is Switchboard's own, under `~/.local/share/switchboard`, so the part
//! is never asked about: `switchboard setup` writes it in every mode, and so
//! does the server as it starts, off its task. Each file is written whole
//! (`util::write_atomic`) and only when it differs; `.switchboard-plugin`
//! lists what was written, so a file a newer build no longer ships is
//! removed and what a skill made for itself (`npm ci`'s `node_modules`
//! beside its scripts) is kept, as the skills part kept it. plugin.json is
//! written last, so a first write cut short leaves no plugin to load.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde_json::{Value, json};

use super::{Report, home_tilde};
use crate::{paths, util};

/// The plugin's name, which Claude Code puts before each of its skills'
/// (`/switchboard:wt`).
pub const NAME: &str = "switchboard";

/// The variable naming the plugin directories Claude Code loads for a
/// session, `:`-separated.
pub const ENV: &str = "CLAUDE_CODE_PLUGIN_DIRS";

/// The oldest Claude Code that reads `ENV`.
pub const CLAUDE_VERSION: (u64, u64, u64) = (2, 1, 280);

/// The oldest Claude Code that runs a plugin's hooks module, which draws
/// Switchboard's statusline in its panes.
pub const MODULE_VERSION: (u64, u64, u64) = (2, 1, 287);

/// The hooks file, named by plugin.json, that lists the plugin's hooks
/// modules.
const MODULES: &str = "hooks/modules.json";

/// The statusline's module, beside the file that lists it.
const STATUSLINE_MODULE: &str = "statusline.js";

/// What the SessionStart hook runs for Switchboard's instructions.
pub const CONTEXT_COMMAND: &str = "switchboard claude-context";

/// The list of what was written, at the plugin's top.
pub const LIST: &str = ".switchboard-plugin";

/// `~/.local/share/switchboard/plugin`.
pub fn dir() -> PathBuf {
    paths::plugin_dir()
}

/// `hooks/hooks.json`: the asset's hooks, and the instructions as a session
/// starts.
pub fn hooks() -> Value {
    let mut hooks = super::claude_settings::ours()["hooks"].clone();
    if let Some(events) = hooks.as_object_mut() {
        let start = events
            .entry("SessionStart")
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Value::Array(groups) = start {
            groups.push(json!({ "hooks": [{ "type": "command", "command": CONTEXT_COMMAND }] }));
        }
    }
    json!({ "hooks": hooks })
}

/// `.claude-plugin/plugin.json`.
pub fn manifest() -> Value {
    json!({
        "name": NAME,
        "description": "Switchboard's skills, the hooks that tell the workspace what each Claude is doing, its statusline and its instructions, for Claude in Switchboard's panes",
        "version": env!("CARGO_PKG_VERSION"),
        "hooks": format!("./{MODULES}"),
    })
}

/// `hooks/statusline.js`: the module that layers the asset's statusline
/// over the settings Claude Code merged from its files, read after every
/// hook beneath it has answered (`next`). A read of one file as it stands
/// (`e.source`) is left as it came, so it is the file's own.
pub fn statusline_module() -> String {
    let statusline = super::claude_settings::ours()["statusLine"].clone();
    format!(
        "// Switchboard's statusline in its panes, written by `switchboard setup`
// (src/setup/plugin.rs). Claude Code takes a statusline from its settings
// alone, so this puts Switchboard's over the settings it merged from its
// files, in memory: no file changes, and only the sessions that load this
// plugin, the ones in Switchboard's panes, draw it.
const statusLine = {};

// Settings' pane_statusline ('Switchboard's statusline in its panes'), as
// Switchboard reads it: off leaves the statusline alone, so the panes draw
// yours too. Asked as Claude Code reads its settings, not as it draws.
async function wanted($) {{
  try {{
    const {{ exitCode, stdout }} = await $.process.run(
      ['switchboard', 'setting', 'pane_statusline'],
      {{ timeoutMs: 5000 }},
    );
    return !(exitCode === 0 && stdout.trim() === 'off');
  }} catch {{
    return true;
  }}
}}

export function register(on) {{
  on('settings.read', async ($, e, next) => {{
    const read = await next(e);
    if (e.source || !read || !('value' in read)) return read;
    if (!(await wanted($))) return read;
    return {{ value: {{ ...read.value, statusLine }} }};
  }});
}}
",
        serde_json::to_string(&statusline).unwrap_or_else(|_| "null".into())
    )
}

/// Every file of the plugin, by its path within it, plugin.json last.
pub fn files() -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = crate::assets::skills()
        .into_iter()
        .map(|(path, bytes)| (format!("skills/{path}"), bytes))
        .collect();
    files.push((
        "hooks/hooks.json".into(),
        format!("{}\n", util::json_pretty(&hooks())).into_bytes(),
    ));
    files.push((
        MODULES.into(),
        format!(
            "{}\n",
            util::json_pretty(&json!({ "modules": [format!("./{STATUSLINE_MODULE}")] }))
        )
        .into_bytes(),
    ));
    files.push((
        format!("hooks/{STATUSLINE_MODULE}"),
        statusline_module().into_bytes(),
    ));
    files.push((
        ".claude-plugin/plugin.json".into(),
        format!("{}\n", util::json_pretty(&manifest())).into_bytes(),
    ));
    files
}

/// The part: the plugin written to `~/.local/share/switchboard/plugin`.
pub fn run(r: &mut Report) -> Result<()> {
    write_to(&dir(), r)
}

/// The plugin written as the server starts, saying nothing but what went
/// wrong.
pub fn write_quietly() -> Result<()> {
    let mut report = Report::new(false);
    report.silent = true;
    run(&mut report)
}

/// How often the server looks at the clone's assets (src/assets.rs).
pub const ASSETS_LOOK: std::time::Duration = std::time::Duration::from_secs(10);

/// The server's look at the clone the build came from: every
/// `ASSETS_LOOK`, off its task, a hash of the paths, sizes and modified
/// times of the clone's
/// skills, Claude settings and part of CLAUDE.md, and the plugin and the
/// parts already set up brought up to date (`switchboard setup --refresh`, whose
/// mark hashes what they are made from) when it moved,
/// or when where the assets come from changed (Settings'
/// `assets_from_checkout` turned on or off), so a skill landed in the clone
/// reaches the Claude sessions started after it with no rebuild or restart.
/// A build with no clone only looks at the setting, reading nothing else.
pub async fn follow_clone() {
    let mut seen = tokio::task::spawn_blocking(look).await.ok();
    let mut every = tokio::time::interval(ASSETS_LOOK);
    every.tick().await;
    // After a refresh that failed, the looks to let pass before the next
    // try, so a part that keeps failing starts a process once a minute.
    let mut failed_looks_left = 0u32;
    loop {
        every.tick().await;
        let Ok(now) = tokio::task::spawn_blocking(look).await else {
            continue;
        };
        if seen.as_ref() == Some(&now) || failed_looks_left > 0 {
            failed_looks_left = failed_looks_left.saturating_sub(1);
            continue;
        }
        let from = now.0.clone();
        let checkout = from
            .as_deref()
            .and_then(std::path::Path::parent)
            .map(std::path::Path::to_path_buf);
        // The plugin and every part already set up (Claude's settings, the
        // part of CLAUDE.md and the skills the old way), by the writers a new
        // build's first run uses, which keep what is yours: in a process of
        // its own, `switchboard setup --refresh`, since a run settles the
        // way setup puts Switchboard once (src/setup/way.rs) and Settings
        // may have moved it since the server's last refresh.
        let refreshed = tokio::task::spawn_blocking(move || {
            // A server started from a worktree's build outlives the
            // worktree: then the `switchboard` on the PATH refreshes.
            let exe = crate::util::exe()
                .ok()
                .filter(|exe| exe.is_file())
                .or_else(|| crate::util::which("switchboard"))
                .ok_or_else(|| std::io::Error::other("no switchboard to run"))?;
            // The clone this server chose, which a binary from the PATH
            // would choose for itself; none when it reads what it carries.
            let checkout = checkout.map(std::path::PathBuf::into_os_string);
            std::process::Command::new(exe)
                .args(["setup", "--refresh"])
                .env("SWITCHBOARD_CHECKOUT", checkout.unwrap_or_default())
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .status()
        })
        .await;
        // Remembered only once it worked, so a refresh that failed is tried
        // again at the next look.
        if !matches!(refreshed, Ok(Ok(status)) if status.success()) {
            failed_looks_left = 5;
        } else {
            seen = Some(now);
            match from {
                Some(dir) => eprintln!(
                    "switchboard server: Switchboard's parts brought up to date from {}",
                    home_tilde(&dir)
                ),
                None => eprintln!(
                    "switchboard server: Switchboard's parts brought back to what the binary carries"
                ),
            }
        }
    }
}

/// Where the assets come from now, and what is there.
fn look() -> (Option<std::path::PathBuf>, Option<u64>) {
    let dir = crate::assets::dir();
    let seen = dir.as_deref().map(crate::assets::fingerprint);
    (dir, seen)
}

/// The plugin written into `dir`: each file that differs, the ones a newer
/// build no longer ships removed, every one tried and the ones that
/// couldn't be said together at the end.
pub fn write_to(dir: &Path, r: &mut Report) -> Result<()> {
    let shown = home_tilde(dir);
    let list_file = dir.join(LIST);
    let before: Option<Vec<String>> = fs::read_to_string(&list_file).ok().map(|text| {
        text.lines()
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect()
    });
    let files = files();
    let mut failed = Vec::new();
    let mut written = 0;
    for (rel, bytes) in &files {
        let file = dir.join(rel);
        if fs::read(&file).is_ok_and(|now| now == *bytes) {
            continue;
        }
        if !r.check
            && let Err(e) = util::write_atomic(&file, bytes)
        {
            failed.push(format!("{rel} ({e})"));
            continue;
        }
        written += 1;
    }
    let mut gone = 0;
    for rel in before.iter().flatten() {
        if files.iter().any(|(f, _)| f == rel) || rel.split('/').any(|p| p == ".." || p.is_empty())
        {
            continue;
        }
        let file = dir.join(rel);
        if !file.is_file() {
            continue;
        }
        if !r.check {
            match fs::remove_file(&file) {
                Ok(()) => {
                    // The folders it leaves empty go with it.
                    let mut parent = file.parent();
                    while let Some(p) = parent.filter(|p| *p != dir) {
                        if fs::remove_dir(p).is_err() {
                            break;
                        }
                        parent = p.parent();
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    failed.push(format!("{rel} ({e})"));
                    continue;
                }
            }
        }
        gone += 1;
    }
    let list: String = files.iter().map(|(f, _)| format!("{f}\n")).collect();
    if !r.check
        && fs::read_to_string(&list_file).ok().as_deref() != Some(list.as_str())
        && let Err(e) = util::write_atomic(&list_file, &list)
    {
        failed.push(format!("{LIST} ({e})"));
    }
    let source = crate::assets::source_words();
    if before.is_none() {
        r.change(format!(
            "write Switchboard's plugin for Claude Code to {shown}, its skills and hooks {source}"
        ));
    } else if written + gone > 0 {
        let mut what = Vec::new();
        if written > 0 {
            what.push(format!(
                "{written} file{} written",
                if written == 1 { "" } else { "s" }
            ));
        }
        if gone > 0 {
            what.push(format!("{gone} no longer shipped removed"));
        }
        r.change(format!(
            "bring {shown} up to date ({}), its skills and hooks {source}",
            what.join(", ")
        ));
    }
    if failed.is_empty() {
        return Ok(());
    }
    bail!("could not write these in {shown}: {}", failed.join(", "))
}

/// `CLAUDE_CODE_PLUGIN_DIRS` for a pane: what the server was started with,
/// kept, and the plugin's directory after it, unless it is there already
/// (written out, or from `~`).
pub fn env_value(inherited: Option<&str>, plugin: &Path, home: &Path) -> String {
    let mut parts: Vec<String> = inherited
        .unwrap_or_default()
        .split(':')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect();
    if !parts.iter().any(|p| names(p, plugin, home)) {
        parts.push(plugin.to_string_lossy().into_owned());
    }
    parts.join(":")
}

/// Whether an entry of the variable is the plugin's directory, written out
/// or from `~`.
fn names(entry: &str, plugin: &Path, home: &Path) -> bool {
    let path = match entry.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            home.join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(entry),
    };
    path.components().eq(plugin.components())
}

/// `CLAUDE_CODE_PLUGIN_DIRS` for a Claude started now, from the value it
/// has: the plugin's directory added while Settings' `claude_plugin` is
/// `on`, taken out while it isn't, any other folder kept; `None` to leave
/// it unset. A new pane's (src/mux/server.rs) and `claude-plugin-dirs`'s.
pub fn value_now(current: Option<&str>, on: bool, plugin: &Path, home: &Path) -> Option<String> {
    if on {
        Some(env_value(current, plugin, home))
    } else {
        without_plugin(current, plugin, home)
    }
}

/// `CLAUDE_CODE_PLUGIN_DIRS` without the plugin's directory, the rest
/// kept; `None` when nothing is left.
pub fn without_plugin(inherited: Option<&str>, plugin: &Path, home: &Path) -> Option<String> {
    let kept: Vec<&str> = inherited
        .unwrap_or_default()
        .split(':')
        .filter(|p| !p.is_empty() && !names(p, plugin, home))
        .collect();
    (!kept.is_empty()).then(|| kept.join(":"))
}

/// A command whose environment can be changed: std's and tokio's.
pub trait Environment {
    fn set_var(&mut self, key: &str, value: &str);
    fn remove_var(&mut self, key: &str);
}

impl Environment for std::process::Command {
    fn set_var(&mut self, key: &str, value: &str) {
        self.env(key, value);
    }
    fn remove_var(&mut self, key: &str) {
        self.env_remove(key);
    }
}

impl Environment for tokio::process::Command {
    fn set_var(&mut self, key: &str, value: &str) {
        self.env(key, value);
    }
    fn remove_var(&mut self, key: &str) {
        self.env_remove(key);
    }
}

/// A `claude` Switchboard runs for itself (`claude -p`: the diff's story
/// and questions, the connectors, Notes' squash, today's todo note, the
/// relay's summaries, codex-review's reviewer) started without the plugin,
/// which a program in a pane would otherwise pass on from the pane's
/// environment: its hooks would speak for the tab, and its instructions and
/// skills are nothing such a run needs. Other plugin directories you set
/// are kept. Every place that starts one calls this.
pub fn headless(cmd: &mut impl Environment) {
    let inherited = std::env::var(ENV).ok();
    match without_plugin(inherited.as_deref(), &dir(), &paths::home()) {
        Some(value) => cmd.set_var(ENV, &value),
        None => cmd.remove_var(ENV),
    }
}

/// A skill's name without the plugin's in front: `switchboard:wt` and `wt`
/// are both `wt`.
pub fn bare(skill: &str) -> &str {
    skill
        .strip_prefix(NAME)
        .and_then(|rest| rest.strip_prefix(':'))
        .unwrap_or(skill)
}

/// Whether a prompt runs the skill `name`, typed bare or with the plugin's
/// name: `/today`, `/switchboard:today args`.
pub fn runs(prompt: &str, name: &str) -> bool {
    let Some(rest) = prompt.trim_start().strip_prefix('/') else {
        return false;
    };
    let rest = bare(rest);
    rest.strip_prefix(name)
        .is_some_and(|after| after.is_empty() || after.starts_with(char::is_whitespace))
}

/// Claude Code's version, from what `claude --version` printed
/// (`2.1.319 (Claude Code)`).
pub fn claude_version(said: &str) -> Option<(u64, u64, u64)> {
    let word = said.split_whitespace().next()?;
    let mut parts = word.split('.').map(|p| p.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    Some(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sb-plugin-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn quiet(check: bool) -> Report {
        let mut r = Report::new(check);
        r.silent = true;
        r
    }

    #[test]
    fn its_hooks_are_the_asset_s_and_the_instructions_as_a_session_starts() {
        let asset = super::super::claude_settings::ours();
        let ours = hooks();
        let events = ours["hooks"].as_object().unwrap();
        for (event, groups) in asset["hooks"].as_object().unwrap() {
            let mine = events[event].as_array().unwrap();
            for group in groups.as_array().unwrap() {
                assert!(
                    mine.contains(group),
                    "{event}'s hooks as the asset has them"
                );
            }
        }
        assert_eq!(
            events.keys().collect::<Vec<_>>(),
            asset["hooks"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            "no event of its own"
        );
        let start = events["SessionStart"].as_array().unwrap();
        assert_eq!(
            start.len(),
            asset["hooks"]["SessionStart"].as_array().unwrap().len() + 1
        );
        assert_eq!(
            start.last().unwrap()["hooks"][0]["command"],
            CONTEXT_COMMAND
        );
        assert!(
            crate::cmd::COMMANDS
                .iter()
                .any(|c| format!("switchboard {}", c.name) == CONTEXT_COMMAND),
            "a command the binary has"
        );
        assert_eq!(manifest()["name"], NAME);
        let all = files();
        assert_eq!(all.last().unwrap().0, ".claude-plugin/plugin.json");
        assert!(all.iter().any(|(f, _)| f == "skills/wt/SKILL.md"));
    }

    #[test]
    fn it_is_written_once_put_right_and_what_is_no_longer_shipped_removed() {
        let dir = temp("write");
        let mut r = quiet(true);
        write_to(&dir, &mut r).unwrap();
        assert_eq!(r.changes, 1);
        assert!(!dir.exists(), "--check writes nothing");

        let mut r = quiet(false);
        write_to(&dir, &mut r).unwrap();
        assert_eq!(r.changes, 1);
        for (rel, bytes) in files() {
            assert_eq!(fs::read(dir.join(&rel)).unwrap(), bytes, "{rel}");
        }
        let mut r = quiet(false);
        write_to(&dir, &mut r).unwrap();
        assert_eq!(r.changes, 0, "nothing the second time");

        // A file changed, one a newer build no longer ships, and the
        // packages a skill installed for itself.
        fs::write(dir.join("skills/wt/SKILL.md"), "edited\n").unwrap();
        fs::write(dir.join("skills/wt/old.md"), "old\n").unwrap();
        fs::create_dir_all(dir.join("skills/gone")).unwrap();
        fs::write(dir.join("skills/gone/SKILL.md"), "gone\n").unwrap();
        let mut list = fs::read_to_string(dir.join(LIST)).unwrap();
        list.push_str("skills/wt/old.md\nskills/gone/SKILL.md\n");
        fs::write(dir.join(LIST), list).unwrap();
        let modules = dir.join("skills/walkthrough/scripts/node_modules/x");
        fs::create_dir_all(&modules).unwrap();
        fs::write(modules.join("index.js"), "").unwrap();
        let mut r = quiet(false);
        write_to(&dir, &mut r).unwrap();
        assert_eq!(r.changes, 1);
        assert_ne!(
            fs::read_to_string(dir.join("skills/wt/SKILL.md")).unwrap(),
            "edited\n"
        );
        assert!(!dir.join("skills/wt/old.md").exists());
        assert!(!dir.join("skills/gone").exists(), "its folder went with it");
        assert!(modules.join("index.js").exists(), "the packages are kept");
        let mut r = quiet(false);
        write_to(&dir, &mut r).unwrap();
        assert_eq!(r.changes, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pane_s_variable_keeps_what_was_there_and_names_the_plugin_once() {
        let home = Path::new("/home/me");
        let plugin = home.join(paths::PLUGIN);
        let p = plugin.to_string_lossy().into_owned();
        assert_eq!(env_value(None, &plugin, home), p);
        assert_eq!(env_value(Some(""), &plugin, home), p);
        assert_eq!(
            env_value(Some("/opt/mine"), &plugin, home),
            format!("/opt/mine:{p}")
        );
        assert_eq!(
            env_value(Some(&format!("/opt/mine:{p}")), &plugin, home),
            format!("/opt/mine:{p}"),
            "not added twice"
        );
        assert_eq!(
            env_value(
                Some("~/.local/share/switchboard/plugin/:/opt/b"),
                &plugin,
                home
            ),
            "~/.local/share/switchboard/plugin/:/opt/b",
            "nor when named from ~"
        );
        assert_eq!(
            env_value(Some("::/opt/a::"), &plugin, home),
            format!("/opt/a:{p}")
        );
    }

    #[test]
    fn a_claude_started_now_takes_the_plugin_as_the_setting_says() {
        let home = Path::new("/home/u");
        let plugin = home.join(".local/share/switchboard/plugin");
        let p = plugin.to_string_lossy().into_owned();
        let stale = format!("/opt/mine:{p}");
        assert_eq!(
            value_now(Some(&stale), true, &plugin, home),
            Some(stale.clone())
        );
        assert_eq!(
            value_now(Some("/opt/mine"), true, &plugin, home),
            Some(stale.clone()),
            "on: added where a shell from before had it out"
        );
        assert_eq!(
            value_now(Some(&stale), false, &plugin, home),
            Some("/opt/mine".to_string()),
            "off: taken out, the rest kept"
        );
        assert_eq!(value_now(Some(&p), false, &plugin, home), None);
        assert_eq!(value_now(None, false, &plugin, home), None);
    }

    #[test]
    fn a_headless_claude_gets_every_plugin_directory_but_switchboard_s() {
        let home = Path::new("/home/me");
        let plugin = home.join(paths::PLUGIN);
        let p = plugin.to_string_lossy().into_owned();
        assert_eq!(without_plugin(None, &plugin, home), None);
        assert_eq!(without_plugin(Some(&p), &plugin, home), None);
        assert_eq!(
            without_plugin(
                Some(&format!(
                    "/opt/a:{p}:~/.local/share/switchboard/plugin/:/opt/b"
                )),
                &plugin,
                home
            )
            .as_deref(),
            Some("/opt/a:/opt/b")
        );
        assert_eq!(
            without_plugin(Some("/opt/a"), &plugin, home).as_deref(),
            Some("/opt/a")
        );
        let mut cmd = std::process::Command::new("true");
        headless(&mut cmd);
    }

    #[test]
    fn a_skill_is_known_by_its_bare_name_or_the_plugin_s() {
        assert_eq!(bare("switchboard:wt"), "wt");
        assert_eq!(bare("wt"), "wt");
        assert_eq!(bare("switchboarding"), "switchboarding");
        assert!(runs("/today", "today"));
        assert!(runs("  /switchboard:today now", "today"));
        assert!(!runs("/todays", "today"));
        assert!(!runs("/other:today", "today"));
        assert!(!runs("today", "today"));
    }

    #[test]
    fn claude_code_s_version_is_read_from_what_it_says() {
        assert_eq!(claude_version("2.1.319 (Claude Code)\n"), Some((2, 1, 319)));
        assert!(claude_version("2.1.279 (Claude Code)").unwrap() < CLAUDE_VERSION);
        assert_eq!(claude_version(""), None);
        assert_eq!(claude_version("Claude Code"), None);
    }
}
