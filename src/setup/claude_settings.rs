//! The `claude` part: what earlier versions put in your own
//! `~/.claude/settings.json` taken out, since Switchboard reaches Claude
//! Code as its plugin alone (src/setup/plugin.rs), which carries the hooks
//! and draws the statusline in its panes, for the Claude sessions there.
//! `assets/claude-settings.json` is Switchboard's part: the hooks that run
//! `switchboard hook`, the statusline, and the permissions its skills and
//! loops need to run unattended, which the plugin's files are made from.
//!
//! Taken out: any hook running one of the asset's commands, with the groups
//! and events that leaves empty, your other hooks kept, since left in they
//! would run twice in a pane; the statusline earlier versions wrote,
//! Switchboard's own taken out and `IN_PANES` taken off one of yours; and
//! the allow rules they added that let Claude do more than it should
//! (`RETIRED_RULES`), each only as it was written, so a rule of yours that
//! differs stays. The other permissions they added stay, though the plugin
//! carries them now (src/setup/permissions.rs): one of them can't be told
//! from the same rule of yours, which Claude Code's "always allow" writes
//! there too.
//!
//! Nothing else in the file is touched (Claude Code writes `/model` and
//! "always allow" there), so running it again changes nothing, and a copy
//! of the file before is left beside it.

use std::fs;
use std::path::PathBuf;

use anyhow::{Result, bail};
use serde_json::{Map, Value};

use super::{Report, home_tilde, stamp};
use crate::{paths, util};

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

/// The allow rules earlier versions added that let Claude do more than
/// read: running any script under the skills (one that loads a file Claude
/// writes, or a path with `..`); git log, diff and fetch with any
/// arguments, whose options write a file, run a program or fetch from any
/// address (Claude Code runs git's read-only forms without a rule, by its
/// own checks); and the commands that change a setting, take the landing
/// lock, write the review's record or message you, with any arguments.
const RETIRED_RULES: &[&str] = &[
    "Bash(node ~/.claude/skills/*)",
    "Bash(node ~/.local/share/switchboard/plugin/skills/*)",
    "Bash(node ~/.local/share/switchboard/claude/plugin/skills/*)",
    "Bash(git fetch *)",
    "Bash(git fetch origin *)",
    "Bash(git log *)",
    "Bash(git diff *)",
    "Bash(codex-review *)",
    "Bash(slack-relay notify *)",
    "Bash(switchboard setting *)",
    "Bash(switchboard land-lock *)",
];

fn is_retired_rule(rule: &Value) -> bool {
    rule.as_str().is_some_and(|r| RETIRED_RULES.contains(&r))
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

/// What earlier versions put before a statusline of yours, running
/// `switchboard statusline` in Switchboard's panes and yours everywhere
/// else.
pub const IN_PANES: &str = "if [ -n \"$SWITCHBOARD\" ] && command -v switchboard >/dev/null; then exec switchboard statusline; fi; ";

/// Whether `settings` has what an earlier version added: its statusline or
/// one of its hooks. Not a permission naming a `switchboard` command, which
/// Claude Code's "always allow" may have written there for you, nor one of
/// `RETIRED_RULES`, which are taken out wherever they are.
pub fn has_ours(settings: &Value) -> bool {
    let hooks = hook_commands(&ours()["hooks"]);
    let statusline = settings["statusLine"]["command"]
        .as_str()
        .is_some_and(|c| c.contains("switchboard statusline"));
    statusline
        || hook_commands(&settings["hooks"])
            .iter()
            .any(|c| hooks.contains(c))
}

/// Whether your settings file has what an earlier version added.
pub fn set_up() -> bool {
    util::read_json(settings_file()).is_some_and(|s| has_ours(&s))
}

/// Your hooks without the ones earlier versions added (the asset's
/// commands), and whether any went. A group or event that leaves empty goes
/// too; one that was empty already is yours and stays.
fn without_ours(hooks: &Map<String, Value>, ours: &Value) -> (Map<String, Value>, bool) {
    let our_commands = hook_commands(&ours["hooks"]);
    let taken = |h: &Value| {
        h["command"]
            .as_str()
            .is_some_and(|c| our_commands.iter().any(|o| o == c))
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

/// Your settings without what earlier versions added: the hooks, the
/// statusline (Switchboard's own taken out, `IN_PANES` taken off one of
/// yours, whose other fields stay; one written by hand around `switchboard
/// statusline` is yours) and `RETIRED_RULES`.
pub fn without_switchboard(current: &Map<String, Value>, ours: &Value) -> Map<String, Value> {
    let mut out = current.clone();
    if let Some(Value::Array(allow)) = out.get_mut("permissions").and_then(|p| p.get_mut("allow")) {
        allow.retain(|rule| !is_retired_rule(rule));
    }
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
    if let Some(command) = out
        .get("statusLine")
        .and_then(|s| s.get("command"))
        .and_then(Value::as_str)
    {
        let yours = command.strip_prefix(IN_PANES).unwrap_or(command);
        if yours.trim() == "switchboard statusline" {
            out.shift_remove("statusLine");
        } else if yours.len() < command.len() {
            let yours = Value::String(yours.to_string());
            out["statusLine"]["command"] = yours;
        }
    }
    out
}

/// Whether your settings file has an earlier version's hooks: what a pane
/// named no plugin ran, the old way.
pub fn has_hooks() -> bool {
    let ours = hook_commands(&ours()["hooks"]);
    util::read_json(settings_file())
        .is_some_and(|s| hook_commands(&s["hooks"]).iter().any(|c| ours.contains(c)))
}

/// The part; left while the workspace's server runs another build
/// (`super::old_way_waits`), whose panes may run the hooks in your file.
pub fn run(r: &mut Report) -> Result<()> {
    if super::old_way_waits() {
        return Ok(());
    }
    let file = settings_file();
    let shown = home_tilde(&file);
    let current = match fs::read_to_string(&file) {
        Ok(text) if text.trim().is_empty() => return Ok(()),
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => map,
            _ => bail!("{shown} isn't a JSON object; left alone"),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => bail!("{shown} can't be read: {e}"),
    };
    let left = without_switchboard(&current, &ours());
    if left == current {
        return Ok(());
    }
    if !r.check {
        let backup = file.with_file_name(format!("settings.json.bak.{}", stamp()));
        fs::copy(&file, &backup)?;
        let text = format!("{}\n", util::json_pretty(&Value::Object(left.clone())));
        util::write_atomic(&file, text)?;
    }
    if left.get("hooks") != current.get("hooks") {
        r.change(format!(
            "take the hooks earlier versions added out of {shown}, since Switchboard's plugin runs them in its panes"
        ));
    }
    if left.get("permissions") != current.get("permissions") {
        r.change(format!(
            "take the allow rules earlier versions added and Switchboard dropped out of {shown}"
        ));
    }
    if left.get("statusLine") != current.get("statusLine") {
        r.change(format!(
            "take the statusline earlier versions added out of {shown}, a statusline of yours kept, since Switchboard's plugin draws its own in its panes"
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

    #[test]
    fn the_hooks_earlier_versions_added_are_taken_out_and_yours_kept() {
        let product = ours();
        let yours = object(json!({
            "model": "sonnet",
            "permissions": { "allow": ["Bash(make *)", "Bash(git log *)", "Bash(switchboard tabs)"] },
            "statusLine": { "type": "command", "command": "my-statusline" },
            "hooks": {
                "Stop": [
                    { "hooks": [{ "type": "command", "command": "say done" }] },
                    { "hooks": [{ "type": "command", "command": "switchboard hook Stop" }] }
                ],
                "PreToolUse": product["hooks"]["PreToolUse"].clone(),
                "PostToolUse": []
            }
        }));
        let left = without_switchboard(&yours, &product);
        assert_eq!(left["model"], "sonnet");
        assert_eq!(left["statusLine"], yours["statusLine"]);
        assert_eq!(
            left["permissions"]["allow"],
            json!(["Bash(make *)", "Bash(switchboard tabs)"]),
            "a retired rule goes, one of Switchboard's still in use stays"
        );
        assert_eq!(
            left["hooks"],
            json!({
                "Stop": [{ "hooks": [{ "type": "command", "command": "say done" }] }],
                "PostToolUse": []
            }),
            "your hook kept, Switchboard's gone, an event of yours left empty kept"
        );
        assert_eq!(without_switchboard(&left, &product), left, "once");
        assert!(!has_ours(&Value::Object(left)));
        let only = object(json!({ "hooks": { "Stop": product["hooks"]["Stop"].clone() } }));
        assert!(without_switchboard(&only, &product).get("hooks").is_none());
        for before in [json!({}), json!({ "model": "opus" })] {
            assert_eq!(
                without_switchboard(&object(before.clone()), &product),
                object(before)
            );
        }
    }

    #[test]
    fn switchboard_s_statusline_goes_and_one_of_yours_is_given_back() {
        let product = ours();
        let gone = object(
            json!({ "statusLine": { "type": "command", "command": "switchboard statusline" } }),
        );
        assert!(has_ours(&Value::Object(gone.clone())));
        assert!(
            without_switchboard(&gone, &product)
                .get("statusLine")
                .is_none()
        );
        let wrapped = object(json!({
            "statusLine": { "type": "command", "command": format!("{IN_PANES}a | b"), "padding": 2 }
        }));
        assert_eq!(
            without_switchboard(&wrapped, &product)["statusLine"],
            json!({ "type": "command", "command": "a | b", "padding": 2 })
        );
        // Written by hand around it, it is yours.
        let by_hand = object(json!({
            "statusLine": { "type": "command", "command": "switchboard statusline | my-formatter" }
        }));
        assert_eq!(without_switchboard(&by_hand, &product), by_hand);
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
