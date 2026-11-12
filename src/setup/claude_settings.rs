//! The `claude` part: Switchboard's part of Claude Code's settings
//! (`assets/claude-settings.json`: the hooks that run `switchboard hook`,
//! the statusline, and the permissions its skills and loops need to run
//! unattended) merged into your own `~/.claude/settings.json`. Ported from
//! setup.sh's `claude_settings`, whose jq this follows step for step.
//!
//! A plugin can't carry a statusline or permission rules, so those are
//! merged in whichever way setup puts Switchboard (src/setup/way.rs). The
//! hooks follow the way. Loaded as a plugin, Switchboard's plugin carries
//! them (src/setup/plugin.rs) for the Claude sessions in its panes alone,
//! so they are taken out of your file where earlier versions added them:
//! any hook running one of their commands, or one the tmux version's setup
//! added (those running `agent-state.sh`), the groups and events that
//! leaves empty with it, and your other hooks kept; left in, they would run
//! twice in a pane. The old way, they replace any hook running the same
//! command, and any the tmux version's setup added, for every Claude of
//! yours.
//!
//! Your file is never a link and stays yours: Claude Code writes `/model`
//! and "always allow" there, and nothing else in it is touched. The
//! statusline takes the place of none, of Switchboard's own, or of the tmux
//! version's (`tmux_versions`), recognised by the link its setup made into
//! a Switchboard clone, never by the script's name, which Claude Code's
//! `/statusline` gives a script of yours too; any other statusline is yours,
//! kept for every Claude outside Switchboard's panes, with `switchboard
//! statusline` run in them (`IN_PANES`), since Claude Code gives a
//! session's context, cost and limits to its statusline alone; the
//! permissions are added to yours. So running it again changes nothing, and
//! a copy of the file before is left beside it. As a plugin, the guided
//! setup runs this part only on a file that has Switchboard's part already
//! (`set_up`), never asking to add it: `switchboard setup claude` does. A
//! settings file that is a link into a clone's `claude/settings.json`, as
//! the whole setup once made it, becomes your own file with what it held
//! first (from the clone's history, once a pull has removed the file).
//! Claude Code reads no settings file after yours, so this is the user layer
//! for Claude: your own settings are the file itself.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};
use serde_json::{Map, Value};

use super::way::Way;
use super::{Report, exists, home_tilde, is_link, leftovers, stamp};
use crate::{paths, util, worktree};

/// Switchboard's part, as compiled in, and the hooks the plugin carries.
pub const ASSET: &str = include_str!("../../assets/claude-settings.json");

/// Switchboard's part, read: the clone's, else the compiled one
/// (src/assets.rs).
pub fn ours() -> Value {
    crate::assets::json("claude-settings.json", ASSET)
}

/// `~/.claude/settings.json`.
pub fn settings_file() -> PathBuf {
    paths::claude_dir().join("settings.json")
}

/// A hook the tmux version's setup added, which Switchboard's replaced.
fn retired(command: &str) -> bool {
    command.contains("agent-state.sh")
}

/// Every hook's command in a `hooks` object: `[.[][].hooks[].command]`.
fn hook_commands(hooks: &Value) -> Vec<String> {
    let mut commands = Vec::new();
    for groups in hooks.as_object().into_iter().flat_map(Map::values) {
        for group in groups.as_array().into_iter().flatten() {
            for hook in group["hooks"].as_array().into_iter().flatten() {
                if let Some(c) = hook["command"].as_str() {
                    commands.push(c.to_string());
                }
            }
        }
    }
    commands
}

/// What runs `switchboard statusline` in Switchboard's panes, put before a
/// statusline of yours, which runs everywhere else. A command list rather
/// than `&& ... ||`, so yours can be a pipeline or a list of its own.
pub const IN_PANES: &str = "if [ -n \"$SWITCHBOARD\" ] && command -v switchboard >/dev/null; then exec switchboard statusline; fi; ";

/// Whether a statusline command is Switchboard's: its own, or one that runs
/// it in its panes, as `with_yours` writes it or as written by hand in any
/// shape (`exec`, or piped through a formatter of yours).
fn is_ours(command: &str) -> bool {
    command.contains("switchboard statusline")
}

/// Your statusline, run outside Switchboard's panes, with Switchboard's in
/// them, so the Sessions and Usage views get what Claude Code gives a
/// statusline alone; your other fields (`padding`) kept.
fn with_yours(yours: &Value) -> Value {
    let mut out = yours.clone();
    if let Some(command) = yours["command"].as_str()
        && !is_ours(command)
    {
        out["command"] = Value::String(format!("{IN_PANES}{command}"));
    }
    out
}

/// Whether `settings` has Switchboard's part, or what an earlier version
/// added: set up before, so the guided setup and a new build bring it up to
/// date without asking. Its statusline, which this part always leaves
/// running `switchboard statusline` in the panes, or a hook of an earlier
/// version. Not a permission naming a `switchboard` command, which Claude
/// Code's "always allow" may have written there for you.
pub fn has_ours(settings: &Value) -> bool {
    let hooks = hook_commands(&ours()["hooks"]);
    let statusline = settings["statusLine"]["command"]
        .as_str()
        .is_some_and(is_ours);
    statusline
        || hook_commands(&settings["hooks"])
            .iter()
            .any(|c| retired(c) || hooks.contains(c))
}

/// Whether your settings file has Switchboard's part.
pub fn set_up() -> bool {
    util::read_json(settings_file()).is_some_and(|s| has_ours(&s))
}

/// What your settings file links to when that isn't there, so Claude Code
/// reads none of your settings, and whether it is the shape of the tmux
/// version's link into its clone's `claude/settings.json`, which this part
/// makes your own from what it held. Said by the guided setup as a plugin,
/// which looks at the file no further without Switchboard's part in it.
pub fn dead_link() -> Option<(PathBuf, bool)> {
    let file = settings_file();
    if file.exists() {
        return None;
    }
    let target = fs::read_link(&file).ok()?;
    let into_clone = target.to_string_lossy().ends_with("/claude/settings.json");
    Some((target, into_clone))
}

/// Whether your settings file has Switchboard's hooks, or the tmux
/// version's: what a pane given the plugin would run twice.
pub fn has_hooks() -> bool {
    util::read_json(settings_file()).is_some_and(|s| {
        let ours = hook_commands(&ours()["hooks"]);
        hook_commands(&s["hooks"])
            .iter()
            .any(|c| retired(c) || ours.contains(c))
    })
}

/// Your hooks without the ones earlier versions added (the asset's
/// commands, and the tmux version's), and whether any went. A group or
/// event that leaves empty goes too; one that was empty already is yours
/// and stays.
fn without_ours(hooks: &Map<String, Value>, ours: &Value) -> (Map<String, Value>, bool) {
    let our_commands = hook_commands(&ours["hooks"]);
    let taken = |h: &Value| {
        h["command"]
            .as_str()
            .is_some_and(|c| retired(c) || our_commands.iter().any(|o| o == c))
    };
    let mut out = Map::new();
    let mut any = false;
    for (event, groups) in hooks {
        let Value::Array(list) = groups else {
            out.insert(event.clone(), groups.clone());
            continue;
        };
        let mut kept = Vec::new();
        let mut emptied = false;
        for group in list {
            match group.get("hooks") {
                Some(Value::Array(each)) if each.iter().any(taken) => {
                    any = true;
                    let left: Vec<Value> = each.iter().filter(|h| !taken(h)).cloned().collect();
                    if left.is_empty() {
                        emptied = true;
                        continue;
                    }
                    let mut group = group.clone();
                    group["hooks"] = Value::Array(left);
                    kept.push(group);
                }
                _ => kept.push(group.clone()),
            }
        }
        if kept.is_empty() && emptied {
            continue;
        }
        out.insert(event.clone(), Value::Array(kept));
    }
    (out, any)
}

/// Your hooks with Switchboard's, as the jq did: yours without any running
/// one of Switchboard's commands (or the old setup's), groups left empty
/// dropped, then Switchboard's appended under each event, events in order
/// of their names.
fn with_ours(current: Option<&Value>, ours: &Value) -> Map<String, Value> {
    let our_commands = hook_commands(&ours["hooks"]);
    let replaced = |c: &str| retired(c) || our_commands.iter().any(|o| o == c);
    let mut hooks = match current {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    };
    for groups in hooks.values_mut() {
        let Value::Array(list) = groups else { continue };
        for group in list.iter_mut() {
            if let Some(Value::Array(each)) = group.get_mut("hooks") {
                each.retain(|h| !h["command"].as_str().is_some_and(replaced));
            }
        }
        list.retain(|g| !matches!(g.get("hooks"), Some(Value::Array(each)) if each.is_empty()));
    }
    if let Some(theirs) = ours["hooks"].as_object() {
        let mut events: Vec<&String> = theirs.keys().collect();
        events.sort();
        for event in events {
            let slot = hooks
                .entry(event.clone())
                .or_insert_with(|| Value::Array(Vec::new()));
            if !slot.is_array() {
                *slot = Value::Array(Vec::new());
            }
            if let (Value::Array(list), Some(add)) = (slot, theirs[event].as_array()) {
                list.extend(add.iter().cloned());
            }
        }
    }
    hooks.retain(|_, v| match v {
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        Value::Null => false,
        _ => true,
    });
    hooks
}

/// Your settings with Switchboard's merged in the way given: its statusline
/// and permissions either way, and its hooks added the old way, taken out
/// as a plugin. `tmux_versions` says whether a statusline command is the
/// tmux version's, which gives way to Switchboard's.
pub fn merge(
    current: &Map<String, Value>,
    ours: &Value,
    way: Way,
    tmux_versions: impl Fn(&str) -> bool,
) -> Map<String, Value> {
    let mut out = current.clone();

    match way {
        Way::Old => {
            let hooks = with_ours(out.get("hooks"), ours);
            out.insert("hooks".into(), Value::Object(hooks));
        }
        Way::Plugin => {
            if let Some(Value::Object(hooks)) = out.get("hooks") {
                let (left, any) = without_ours(hooks, ours);
                if any {
                    if left.is_empty() {
                        out.shift_remove("hooks");
                    } else {
                        out.insert("hooks".into(), Value::Object(left));
                    }
                }
            }
        }
    }

    // The statusline: Switchboard's in place of none, its own or the tmux
    // version's, else yours run outside its panes.
    let command = out
        .get("statusLine")
        .and_then(|s| s.get("command"))
        .and_then(Value::as_str);
    let replaced = command.is_none_or(|command| {
        let yours = command.strip_prefix(IN_PANES).unwrap_or(command);
        command.trim() == "switchboard statusline" || (!is_ours(yours) && tmux_versions(yours))
    });
    if replaced {
        out.insert("statusLine".into(), ours["statusLine"].clone());
    } else if let Some(yours) = out.get("statusLine") {
        let wrapped = with_yours(yours);
        out.insert("statusLine".into(), wrapped);
    }

    // Permissions: yours first, then each of Switchboard's you don't have.
    let mut permissions = match out.get("permissions") {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    };
    for key in ["allow", "deny"] {
        let mut list = permissions
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let add: Vec<Value> = ours["permissions"][key]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| !list.contains(p))
            .cloned()
            .collect();
        list.extend(add);
        permissions.insert(key.into(), Value::Array(list));
    }
    out.insert("permissions".into(), Value::Object(permissions));
    out
}

/// `~/.claude/settings.json` as it stands for the leftovers part: the file,
/// or, while it is still the tmux version's link leading nowhere (this part
/// not yet run), what that link held, from the clone's history; `Null` when
/// there is none, and `None` when it is there but can't be read.
pub fn as_claude_reads() -> Option<Value> {
    let file = settings_file();
    if !super::exists(&file) {
        return Some(Value::Null);
    }
    if is_link(&file) && !file.exists() {
        let target = fs::read_link(&file).ok()?;
        let target = file
            .parent()
            .map_or(target.clone(), |dir| dir.join(&target));
        return serde_json::from_slice(&held_by_link(&target)?).ok();
    }
    util::read_json(&file)
}

/// What a link into a clone's `claude/settings.json` held: the file it leads
/// to, else, in a clone pulled since the repository dropped that file, the
/// file from git, the commit before the one that removed it, as `claude_md`
/// finds a clone's old rules.
fn held_by_link(target: &Path) -> Option<Vec<u8>> {
    fs::read(target).ok().or_else(|| {
        let repo = target.parent()?.parent()?;
        let removed = worktree::git_at(
            repo,
            &[
                "log",
                "-1",
                "--format=%H",
                "--diff-filter=D",
                "--",
                "claude/settings.json",
            ],
        )
        .ok()
        .filter(|h| !h.is_empty())?;
        let mut cmd = Command::new("git");
        cmd.arg("-C")
            .arg(repo)
            .args(["show", &format!("{removed}^:claude/settings.json")]);
        util::run_command(&mut cmd).ok().map(String::into_bytes)
    })
}

/// The statusline the tmux version's settings had, run from the link its
/// setup made at `~/.claude/statusline-command.sh` into its clone.
const TMUX_STATUSLINE: &str = "bash ~/.claude/statusline-command.sh";

/// Whether a statusline command runs the tmux version's script. It names
/// `~/.claude/statusline-command.sh` (from `~/`, `$HOME/`, `${HOME}/` or in
/// full), and that is still the link the tmux version's setup made into a
/// Switchboard clone's `claude/` (`leftovers::is_clone`), leading somewhere
/// or, the clone pulled past the rewrite, nowhere; or nothing is there any
/// more (the `leftovers` part removes that link) and the command is the one
/// its settings had, which now runs nothing. The script's name alone counts
/// for nothing: Claude Code's `/statusline` writes a script of yours by the
/// same name, and a file, or a link anywhere else (your dotfiles'), is
/// yours.
fn tmux_versions(command: &str) -> bool {
    let script = paths::claude_dir().join("statusline-command.sh");
    let full = script.to_string_lossy();
    let named = [
        "~/.claude/statusline-command.sh",
        "$HOME/.claude/statusline-command.sh",
        "${HOME}/.claude/statusline-command.sh",
        &full,
    ]
    .iter()
    .any(|name| command.contains(name));
    if !named {
        return false;
    }
    if let Ok(target) = fs::read_link(&script) {
        // A relative link leads from the folder it is in.
        let target = script
            .parent()
            .map_or(target.clone(), |dir| dir.join(&target));
        return target
            .file_name()
            .is_some_and(|name| name == "statusline-command.sh")
            && target
                .parent()
                .filter(|dir| dir.file_name().is_some_and(|name| name == "claude"))
                .and_then(Path::parent)
                .is_some_and(leftovers::is_clone);
    }
    let command = command.trim();
    !exists(&script) && (command == TMUX_STATUSLINE || command == format!("bash {full}"))
}

/// The part.
pub fn run(r: &mut Report) -> Result<()> {
    let file = settings_file();
    let shown = home_tilde(&file);
    if is_link(&file)
        && let Ok(target) = fs::read_link(&file)
        && target.to_string_lossy().ends_with("/claude/settings.json")
    {
        if !r.check {
            let target = file
                .parent()
                .map_or(target.clone(), |dir| dir.join(&target));
            let Some(held) = held_by_link(&target) else {
                bail!(
                    "{shown} links to {}, which isn't there and can't be found in its clone's history; left alone",
                    target.display()
                );
            };
            fs::remove_file(&file)?;
            util::write_atomic(&file, held)?;
        }
        r.change(format!("make {shown} your own, from the one it linked to"));
    }
    let current = match fs::read_to_string(&file) {
        Ok(text) if text.trim().is_empty() => Map::new(),
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => map,
            _ => bail!("{shown} isn't a JSON object; left alone"),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(e) => bail!("{shown} can't be read: {e}"),
    };
    let way = super::way::current();
    let merged = merge(&current, &ours(), way, tmux_versions);
    if merged == current {
        return Ok(());
    }
    if !r.check {
        if file.exists() {
            let backup = file.with_file_name(format!("settings.json.bak.{}", stamp()));
            fs::copy(&file, &backup)?;
        }
        let text = format!("{}\n", util::json_pretty(&Value::Object(merged.clone())));
        util::write_atomic(&file, text)?;
    }
    if way == Way::Old {
        r.change(format!(
            "add Switchboard's hooks, statusline and permissions to {shown}"
        ));
        return Ok(());
    }
    if merged.get("hooks") != current.get("hooks") {
        r.change(format!(
            "take the hooks earlier versions added out of {shown}, since Switchboard's plugin runs them in its panes"
        ));
    }
    let rest = |m: &Map<String, Value>| {
        let mut m = m.clone();
        m.shift_remove("hooks");
        m
    };
    if rest(&merged) != rest(&current) {
        r.change(format!(
            "add Switchboard's statusline and permissions to {shown}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    /// No statusline is the tmux version's.
    fn no_tmux(_: &str) -> bool {
        false
    }

    /// Yours: a model, permissions, a statusline, a hook of your own beside
    /// the tmux version's and Switchboard's, and an event of yours left
    /// empty.
    fn yours(product: &Value) -> Map<String, Value> {
        object(json!({
            "model": "sonnet",
            "permissions": { "allow": ["Bash(make *)", "Bash(git log *)"] },
            "statusLine": { "type": "command", "command": "my-statusline" },
            "hooks": {
                "Stop": [
                    { "hooks": [
                        { "type": "command", "command": "say done" },
                        { "type": "command", "command": "bash ~/.config/tmux/agent-state.sh set done" }
                    ] },
                    { "hooks": [{ "type": "command", "command": "switchboard hook Stop" }] }
                ],
                "PreToolUse": product["hooks"]["PreToolUse"].clone(),
                "PostToolUse": []
            }
        }))
    }

    fn statusline_and_permissions_merged(merged: &Map<String, Value>, product: &Value) {
        assert_eq!(merged["model"], "sonnet");
        assert_eq!(
            merged["statusLine"]["command"],
            format!("{IN_PANES}my-statusline"),
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
        assert_eq!(merged["permissions"]["allow"], Value::Array(allow));
        assert_eq!(
            merged["permissions"]["deny"],
            product["permissions"]["deny"]
        );
    }

    #[test]
    fn the_old_way_adds_switchboard_s_hooks_once_keeping_yours() {
        let product = ours();
        let yours = yours(&product);
        let merged = merge(&yours, &product, Way::Old, no_tmux);
        statusline_and_permissions_merged(&merged, &product);
        let mut stop = vec![json!({ "hooks": [{ "type": "command", "command": "say done" }] })];
        stop.extend(product["hooks"]["Stop"].as_array().unwrap().iter().cloned());
        assert_eq!(
            merged["hooks"]["Stop"],
            Value::Array(stop),
            "your hook kept, the old setup's gone, Switchboard's once"
        );
        assert_eq!(
            merged["hooks"]["PreToolUse"],
            product["hooks"]["PreToolUse"]
        );
        assert_eq!(
            merge(&merged, &product, Way::Old, no_tmux),
            merged,
            "a second merge changes nothing"
        );
        assert!(has_ours(&Value::Object(merged)));
        assert!(!has_ours(&json!({ "hooks": { "Stop": [] } })));
    }

    #[test]
    fn as_a_plugin_the_hooks_earlier_versions_added_are_taken_out_and_yours_kept() {
        let product = ours();
        let yours = yours(&product);
        let merged = merge(&yours, &product, Way::Plugin, no_tmux);
        statusline_and_permissions_merged(&merged, &product);
        assert_eq!(
            merged["hooks"],
            json!({
                "Stop": [{ "hooks": [{ "type": "command", "command": "say done" }] }],
                "PostToolUse": []
            }),
            "your hook kept, the old setup's and Switchboard's gone, an event of yours left empty kept"
        );
        assert_eq!(
            merge(&merged, &product, Way::Plugin, no_tmux),
            merged,
            "a second merge changes nothing"
        );
        assert!(
            has_ours(&Value::Object(merged.clone())),
            "by its statusline"
        );
        // A permission of Switchboard's alone, as "always allow" writes it,
        // isn't its part.
        assert!(!has_ours(
            &json!({ "permissions": { "allow": ["Bash(switchboard tabs)"] } })
        ));

        // Nothing left of the hooks but Switchboard's: no hooks at all.
        let only = object(json!({ "hooks": { "Stop": product["hooks"]["Stop"].clone() } }));
        assert!(
            merge(&only, &product, Way::Plugin, no_tmux)
                .get("hooks")
                .is_none()
        );
        // None to start with: none added.
        assert!(
            merge(&Map::new(), &product, Way::Plugin, no_tmux)
                .get("hooks")
                .is_none()
        );
        // And back the old way, Switchboard's again.
        let back = merge(&merged, &product, Way::Old, no_tmux);
        assert_eq!(back["hooks"]["PreToolUse"], product["hooks"]["PreToolUse"]);
    }

    #[test]
    fn none_and_the_tmux_version_s_are_replaced_and_one_by_its_script_s_name_alone_kept() {
        let product = ours();
        let tmux = |c: &str| c == TMUX_STATUSLINE;
        for way in [Way::Old, Way::Plugin] {
            for before in [
                json!({}),
                json!({ "statusLine": { "type": "command", "command": "switchboard statusline" } }),
                json!({ "statusLine": { "type": "command", "command": TMUX_STATUSLINE } }),
                json!({ "statusLine": { "type": "command", "command": format!("{IN_PANES}{TMUX_STATUSLINE}") } }),
            ] {
                let merged = merge(&object(before), &product, way, tmux);
                assert_eq!(merged["statusLine"]["command"], "switchboard statusline");
            }
            // The name Claude Code's /statusline gives a script of yours.
            for command in [
                TMUX_STATUSLINE,
                "bash \"$HOME/.claude/statusline-command.sh\"",
            ] {
                let before =
                    object(json!({ "statusLine": { "type": "command", "command": command } }));
                let merged = merge(&before, &product, way, no_tmux);
                assert_eq!(
                    merged["statusLine"]["command"],
                    format!("{IN_PANES}{command}"),
                    "yours, outside Switchboard's panes"
                );
                assert_eq!(merge(&merged, &product, way, no_tmux), merged);
            }
        }
    }

    #[test]
    fn a_statusline_that_already_runs_switchboard_s_in_its_panes_is_left_alone() {
        let product = ours();
        let by_hand = "[ -n \"$SWITCHBOARD\" ] && command -v switchboard >/dev/null && exec switchboard statusline || exec bash ~/.claude/statusline-command.sh";
        let piped = "if [ -n \"$SWITCHBOARD\" ]; then switchboard statusline | my-formatter; else my-statusline; fi";
        for command in [
            by_hand.to_string(),
            piped.to_string(),
            "switchboard statusline | my-formatter".to_string(),
            format!("{IN_PANES}my-statusline"),
        ] {
            let before = object(json!({
                "statusLine": { "type": "command", "command": command, "padding": 0 }
            }));
            let merged = merge(&before, &product, Way::Plugin, no_tmux);
            assert_eq!(merged["statusLine"], before["statusLine"]);
            assert!(has_ours(&Value::Object(before)));
        }
    }

    #[test]
    fn a_statusline_of_yours_keeps_its_other_fields() {
        let before = object(json!({
            "statusLine": { "type": "command", "command": "a | b", "padding": 2 }
        }));
        let merged = merge(&before, &ours(), Way::Plugin, no_tmux);
        assert_eq!(
            merged["statusLine"],
            json!({ "type": "command", "command": format!("{IN_PANES}a | b"), "padding": 2 })
        );
    }

    #[test]
    fn every_hook_runs_switchboard_hook_with_its_event() {
        let product = ours();
        for (event, groups) in product["hooks"].as_object().unwrap() {
            for group in groups.as_array().unwrap() {
                for hook in group["hooks"].as_array().unwrap() {
                    assert_eq!(hook["command"], format!("switchboard hook {event}"));
                }
            }
        }
    }
}
