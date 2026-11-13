//! Switchboard's permissions for the Claude in its panes, as a plugin: the
//! rules in `assets/claude-settings.json` that its skills and instructions
//! need to run unattended, without a line in any settings file of yours.
//! New with the plugin's hooks modules; the old way (Settings' "Load
//! Switchboard as a plugin" off) still merges every rule into
//! `~/.claude/settings.json` (src/setup/claude_settings.rs).
//!
//! Claude Code takes permission rules from its settings files alone: a
//! plugin's own settings keep only `agent`, and a `settings.read` hook's
//! answer, which carries the statusline, doesn't reach its permission
//! check. So the rules reach a pane's Claude two ways, each through Claude
//! Code's own decision:
//!
//! - A skill's rules are its own, in its `allowed-tools` front matter, which
//!   Claude Code matches as it matches a rule of yours for the turn that runs
//!   the skill, a subagent it starts and every tick of a `/loop` running it
//!   included; a deny or ask rule of yours still wins. Each skill lists the
//!   asset's rules for what it names, and what the skills it hands off to
//!   name (`switchboard skill-path <name>`, read and followed in its turn).
//! - What Switchboard's instructions (`assets/switchboard.md`) have Claude
//!   run on any turn, and the rules that deny, are a hooks module's
//!   (`permissions.js` beside this file, written into the plugin's
//!   `hooks/` by `plugin::files`): a `tool.check` hook, which Claude Code
//!   runs as it decides whether a call may run, in subagents too, with its
//!   own verdict to hand. It allows one of the asset's `switchboard`
//!   commands that the instructions name (`every_turn`), as one simple
//!   command, only when that verdict is the plain ask of a call no rule,
//!   hook, check or limit decided, so a rule of yours, a read outside the
//!   working directories or an organization's limit still decides, and
//!   auto mode's classifier is never asked about it; and it refuses what
//!   the asset's deny rules name (reading a `.env` file, with a tool or a
//!   shell command that reads one, and `git pull`), whatever the verdict.
//!   Settings' `pane_permissions` off leaves every call to Claude Code.
//!
//! Matching a shell command outside Claude Code is where a matcher goes
//! wrong (compound commands, wrappers, redirects), so the module's allows
//! are kept to Switchboard's own commands, which it takes only alone, and
//! its denies err toward refusing. Both lists come from the asset as the
//! plugin is written, never a copy of their own.

use serde_json::Value;

/// The module's file name, beside `hooks/modules.json` in the plugin.
pub const MODULE: &str = "permissions.js";

/// The module, its lists empty.
const SOURCE: &str = include_str!("permissions.js");

/// The lines of `SOURCE` that `module` fills in.
const ALLOW_LINE: &str = "const ALLOW = [];";
const DENY_LINE: &str = "const DENY = [];";

/// The asset's rules of one kind (`allow`, `deny`).
fn rules(kind: &str) -> Vec<String> {
    super::claude_settings::ours()["permissions"][kind]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// What a rule names in a skill's or the instructions' words, and whether
/// the word must end there: a shell rule's command (`switchboard show` for
/// `Bash(switchboard show *)`, ending there; `gh pr view` for `Bash(gh pr
/// view*)`, which runs on), and a connector tool's own name
/// (`slack_read_channel`).
pub fn key(rule: &str) -> Option<(&str, bool)> {
    if let Some(command) = rule.strip_prefix("Bash(").and_then(|r| r.strip_suffix(')')) {
        return Some(if let Some(c) = command.strip_suffix(" *") {
            (c, true)
        } else if let Some(c) = command.strip_suffix('*') {
            (c, false)
        } else {
            (command, true)
        });
    }
    rule.starts_with("mcp__")
        .then(|| rule.rsplit("__").next())
        .flatten()
        .map(|tool| (tool, true))
}

/// Whether `text` names what `rule` allows, as a word of its own.
pub fn named_in(rule: &str, text: &str) -> bool {
    let Some((key, ends)) = key(rule) else {
        return false;
    };
    let part_of_word = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    text.match_indices(key).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + key.len()..].chars().next();
        !before.is_some_and(part_of_word) && !(ends && after.is_some_and(part_of_word))
    })
}

/// The rules the module allows on any turn: the asset's `switchboard`
/// commands that Switchboard's instructions have Claude run.
pub fn every_turn() -> Vec<String> {
    let part = crate::cmd::claude_context::part();
    rules("allow")
        .into_iter()
        .filter(|r| r.starts_with("Bash(switchboard ") && named_in(r, &part))
        .collect()
}

/// The module as the plugin holds it: `SOURCE` with its lists filled in.
pub fn module() -> String {
    let list = |rules: Vec<String>| serde_json::to_string(&rules).unwrap_or_else(|_| "[]".into());
    SOURCE
        .replacen(
            ALLOW_LINE,
            &format!("const ALLOW = {};", list(every_turn())),
            1,
        )
        .replacen(
            DENY_LINE,
            &format!("const DENY = {};", list(rules("deny"))),
            1,
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::{Path, PathBuf};

    fn skills_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/skills")
    }

    #[test]
    fn a_rule_is_named_as_a_word_of_its_own() {
        assert_eq!(
            key("Bash(switchboard show *)"),
            Some(("switchboard show", true))
        );
        assert_eq!(key("Bash(gh pr view*)"), Some(("gh pr view", false)));
        assert_eq!(
            key("Bash(switchboard tabs)"),
            Some(("switchboard tabs", true))
        );
        assert_eq!(
            key("mcp__claude_ai_Slack__slack_read_channel"),
            Some(("slack_read_channel", true))
        );
        assert_eq!(key("Read(.env)"), None);

        assert!(named_in(
            "Bash(switchboard set *)",
            "run `switchboard set @x y`"
        ));
        assert!(!named_in(
            "Bash(switchboard set *)",
            "`switchboard setting relay_send_x`"
        ));
        assert!(named_in(
            "Bash(switchboard setting relay_*)",
            "`switchboard setting relay_send_<kind>`"
        ));
        assert!(named_in("Bash(gh pr view*)", "gh pr view --json"));
        assert!(!named_in(
            "mcp__claude_ai_Slack__slack_search_public",
            "slack_search_public_and_private"
        ));
    }

    #[test]
    fn every_turn_is_the_switchboard_commands_the_instructions_name() {
        assert_eq!(
            every_turn(),
            [
                "Bash(switchboard skill-path *)",
                "Bash(switchboard slack-relay sends *)",
                "Bash(switchboard tabs)",
                "Bash(switchboard diff-story)",
                "Bash(switchboard diff-story *)",
                "Bash(switchboard show *)",
                "Bash(switchboard set *)",
                "Bash(switchboard setting relay_*)",
            ]
        );
    }

    #[test]
    fn the_module_takes_every_deny_rule_and_its_lists_are_filled_in() {
        // The shapes permissions.js reads: a file wherever it is, or a
        // command; a rule of another shape would deny nothing.
        let shape = regex::Regex::new(r"^(Read\((\./|\*\*/)?[^/]+\)|Bash\(.+\))$").unwrap();
        for rule in rules("deny") {
            assert!(shape.is_match(&rule), "{rule}");
        }
        assert_eq!(SOURCE.matches(ALLOW_LINE).count(), 1);
        assert_eq!(SOURCE.matches(DENY_LINE).count(), 1);
        let module = module();
        assert!(!module.contains(ALLOW_LINE) && !module.contains(DENY_LINE));
        assert!(module.contains(r#""Bash(switchboard tabs)""#));
        assert!(module.contains(r#""Read(.env)""#) && module.contains(r#""Bash(git pull)""#));
        assert!(module.contains("['switchboard', 'setting', SETTING]"));
        assert!(module.contains("const SETTING = 'pane_permissions';"));
    }

    /// A skill's `allowed-tools`, as a YAML list in its front matter.
    fn allowed_tools(skill: &str) -> Vec<String> {
        let text = fs::read_to_string(skills_dir().join(skill).join("SKILL.md")).unwrap();
        let front = text
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .map(|(front, _)| front)
            .unwrap_or_default();
        front
            .lines()
            .skip_while(|l| !l.starts_with("allowed-tools:"))
            .skip(1)
            .map_while(|l| l.strip_prefix("  - "))
            .map(str::to_string)
            .collect()
    }

    /// The words of a skill's own Markdown files.
    fn words_of(skill: &str) -> String {
        let mut text = String::new();
        let mut dirs = vec![skills_dir().join(skill)];
        while let Some(dir) = dirs.pop() {
            for entry in fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if entry.file_name() != "node_modules" {
                        dirs.push(path);
                    }
                } else if path.extension().is_some_and(|e| e == "md") {
                    text.push_str(&fs::read_to_string(&path).unwrap());
                }
            }
        }
        text
    }

    #[test]
    fn each_skill_carries_the_rules_for_what_it_and_the_skills_it_follows_name() {
        let allow = rules("allow");
        let handoff = regex::Regex::new(r"skill-path(?: --own)? ([a-z-]+)").unwrap();
        let skills: Vec<String> = fs::read_dir(skills_dir())
            .unwrap()
            .flatten()
            .filter(|e| e.path().join("SKILL.md").is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        let words: BTreeMap<&str, String> =
            skills.iter().map(|s| (s.as_str(), words_of(s))).collect();
        for skill in &skills {
            // The skill and every skill it reads and follows, in turn.
            let mut followed = BTreeSet::from([skill.as_str()]);
            let mut todo = vec![skill.as_str()];
            while let Some(s) = todo.pop() {
                for c in handoff.captures_iter(&words[s]) {
                    let name = c.get(1).unwrap().as_str();
                    if let Some((known, _)) = words.get_key_value(name)
                        && followed.insert(known)
                    {
                        todo.push(known);
                    }
                }
            }
            let carried = allowed_tools(skill);
            for rule in &carried {
                assert!(
                    allow.contains(rule),
                    "{skill} carries {rule}, not in the asset"
                );
            }
            for rule in &allow {
                if followed.iter().any(|s| named_in(rule, &words[s])) {
                    assert!(
                        carried.contains(rule),
                        "{skill} names {rule} but doesn't carry it"
                    );
                }
            }
        }
    }

    /// The module run under node, where it is installed: what it allows
    /// and refuses, as Claude Code would ask it.
    #[test]
    fn the_module_decides_as_its_rules_say() {
        let Some(node) = crate::util::which("node") else {
            eprintln!("no node on the PATH: the module's decisions aren't tried");
            return;
        };
        let dir = std::env::temp_dir().join(format!("sb-permissions-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("permissions.mjs"), module()).unwrap();
        fs::write(
            dir.join("ask.mjs"),
            "import { allows, denied } from './permissions.mjs';\n\
             const cases = JSON.parse(process.argv[2]);\n\
             console.log(JSON.stringify(cases.map(([tool, input]) =>\n\
               tool === 'allow' ? allows(input) : denied(tool, input) !== null)));\n",
        )
        .unwrap();
        let allow = [
            ("switchboard tabs", true),
            ("switchboard tabs x", false),
            ("switchboard show @tab_note", true),
            ("switchboard show", false),
            (
                "switchboard set @tab_name 'Review loop: settings (layout); rm -rf ~'",
                true,
            ),
            ("switchboard set @tab_name \"Probe $(id)\"", false),
            ("switchboard set @x `id`", false),
            ("switchboard diff-story", true),
            ("switchboard diff-story --path", true),
            ("switchboard setting relay_send_finished off", true),
            ("switchboard setting relay_send_finished", true),
            ("switchboard setting review_security", false),
            ("switchboard slack-relay sends quiet", true),
            ("switchboard slack-relay notify x", false),
            ("switchboard skill-path review-loop", true),
            ("switchboard tabs; printenv X", false),
            ("switchboard tabs && rm x", false),
            ("switchboard tabs | sh", false),
            ("switchboard tabs > /tmp/x", false),
            ("switchboard show $HOME", false),
            ("switchboard show *", false),
            ("FOO=1 switchboard tabs", false),
            ("/tmp/switchboard tabs", false),
            ("switchboard tabs\nrm x", false),
            ("switchboard show a\\ b", false),
            ("switchboard codex-review", false),
            ("git status", false),
        ];
        let deny = [
            ("Bash", "git pull", true),
            ("Bash", "git pull --rebase", true),
            ("Bash", "cd x && git pull", true),
            ("Bash", "git -C /x pull", true),
            ("Bash", "/usr/bin/git pull", true),
            ("Bash", "git log --grep pull", false),
            ("Bash", "echo git pull", false),
            ("Bash", "git fetch", false),
            ("Bash", "cat .env", true),
            ("Bash", "head -5 app/.env.local", true),
            ("Bash", "grep KEY .env.production", true),
            ("Bash", "sort < .env", true),
            ("Bash", "cd app && cat '.env'", true),
            ("Bash", "cp .env.example .env", false),
            ("Bash", "echo X >> .env", false),
            ("Bash", "ls -a", false),
            ("Bash", "cat .envrc", false),
            ("Bash", "cat <<EOF", false),
            ("Bash", "cat .e\"\"nv", true),
            ("Bash", "c'a't .env", true),
            ("Bash", "git \"pull\"", true),
            ("Bash", "echo \"git pull\"", false),
            ("Read", "/a/.ENV", true),
            ("Read", ".env", true),
            ("Read", "/a/b/.env.local", true),
            ("Read", "/a/b/.envrc", false),
            ("Read", "/a/.environment", false),
            ("Read", "/a/README.md", false),
            ("Grep", ".env", true),
            ("Glob", "**/.env", true),
        ];
        let input = |tool: &str, s: &str| match tool {
            "Bash" => serde_json::json!({ "command": s }),
            "Read" => serde_json::json!({ "file_path": s }),
            "Grep" => serde_json::json!({ "pattern": "KEY", "path": s }),
            _ => serde_json::json!({ "pattern": s }),
        };
        let cases: Vec<Value> = allow
            .iter()
            .map(|(c, _)| serde_json::json!(["allow", c]))
            .chain(
                deny.iter()
                    .map(|(tool, s, _)| serde_json::json!([tool, input(tool, s)])),
            )
            .collect();
        let out = std::process::Command::new(node)
            .arg(dir.join("ask.mjs"))
            .arg(serde_json::to_string(&cases).unwrap())
            .output()
            .unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let got: Vec<bool> = serde_json::from_slice(&out.stdout).unwrap();
        let want: Vec<bool> = allow
            .iter()
            .map(|(_, w)| *w)
            .chain(deny.iter().map(|(_, _, w)| *w))
            .collect();
        let wrong: Vec<String> = cases
            .iter()
            .zip(got.iter().zip(&want))
            .filter(|(_, (g, w))| g != w)
            .map(|(c, (g, _))| format!("{c} gave {g}"))
            .collect();
        assert!(wrong.is_empty(), "{wrong:#?}");
    }
}
