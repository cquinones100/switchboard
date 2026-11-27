//! Switchboard's permissions for the Claude in its panes, as a plugin: the
//! rules in `assets/claude-settings.json` that its skills and instructions
//! need to run unattended, without a line in any settings file of yours.
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
//!   commands that the instructions name (`every_turn`), and the landing's
//!   own, `switchboard land-lock open-pr` and `item-done` (`LANDING`),
//!   which queue-and-clean runs on a turn that read its skill rather than
//!   started it, where its `allowed-tools` don't apply, each as one simple
//!   command, only when that verdict is the plain ask of a call no rule,
//!   hook, check or limit decided, so a rule of yours, a read outside the
//!   working directories or an organization's limit still decides, and
//!   auto mode's classifier is never asked about it; and it refuses what
//!   the asset's deny rules name (reading a `.env` file, with a tool or a
//!   shell command that reads one, `git pull`, and the options that make
//!   an allowed git command write a file or run a program: `--output`,
//!   `--ext-diff`, `--upload-pack`, and fetch's `--multiple`, which fetches
//!   from addresses), whatever the verdict; and it refuses what the
//!   repository's way to land (`switchboard landing`) forbids, `gh pr
//!   merge` where GitHub doesn't merge its PRs (but a stacked PR's) and a
//!   push to its default branch where it doesn't land straight on it,
//!   naming the way and Settings' Repositories.
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

/// The asset's rules the module allows besides `every_turn`: the landing's
/// own, which `/queue-and-clean` runs, often on a turn that read its skill
/// rather than started it, where auto mode's classifier would otherwise
/// judge a command that pushes and opens a PR (`land-lock open-pr`, which
/// checks in code that the tab lands without asking and its review came
/// back clean) or one that comments on an issue and removes a worktree
/// (`item-done`). The module takes each only with the options
/// queue-and-clean gives it (`SHAPES` in permissions.js), never
/// `item-done`'s worktree path or `--checked-in`, whose branch it takes on
/// trust.
const LANDING: &[&str] = &[
    "Bash(switchboard item-done *)",
    "Bash(switchboard land-lock open-pr *)",
];

/// Every rule the module allows: `every_turn`'s and `LANDING`.
fn allowed() -> Vec<String> {
    let mut all = every_turn();
    all.extend(LANDING.iter().map(|r| r.to_string()));
    all
}

/// The module as the plugin holds it: `SOURCE` with its lists filled in.
pub fn module() -> String {
    let list = |rules: Vec<String>| serde_json::to_string(&rules).unwrap_or_else(|_| "[]".into());
    SOURCE
        .replacen(
            ALLOW_LINE,
            &format!("const ALLOW = {};", list(allowed())),
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
                "Bash(switchboard look *)",
                "Bash(switchboard set *)",
                "Bash(switchboard setting relay_*)",
                "Bash(switchboard landing)",
                "Bash(switchboard landing *)",
                "Bash(switchboard autopilot-duplicate *)",
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
        assert!(module.contains(r#""Bash(switchboard item-done *)""#));
        assert!(module.contains(r#""Bash(switchboard land-lock open-pr *)""#));
        let allow = rules("allow");
        for rule in LANDING {
            assert!(allow.iter().any(|r| r == rule), "{rule} isn't the asset's");
        }
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
            ("switchboard landing", true),
            ("switchboard landing --json", true),
            ("switchboard landing --start=direct", false),
            ("switchboard landing --json --start direct", false),
            ("switchboard landing /tmp/x", false),
            (
                "switchboard item-done --says 'Make the hook allow it.' --close-issue",
                true,
            ),
            (
                "switchboard item-done --says 'Done.' --hash abc1234 --without-asking",
                true,
            ),
            ("switchboard item-done", false),
            (
                "switchboard land-lock open-pr --title 'Allow item-done in auto mode' --body-file /tmp/body.md",
                true,
            ),
            (
                "switchboard land-lock open-pr --title 'x' --body-file b.md --draft",
                false,
            ),
            (
                "switchboard land-lock open-pr --title 'x' --body-file b.md && git push -f",
                false,
            ),
            ("switchboard land-lock push", false),
            (
                r"switchboard item-done --says 'Keep the tab'\''s note.' --close-issue",
                true,
            ),
            (
                "switchboard item-done --says \"Keep the tab's note.\" --merged-pr-head abc --hash abc1234",
                true,
            ),
            ("switchboard item-done --says 'Done.' --hash", false),
            ("switchboard item-done --says 'Done.' /tmp/other", false),
            (
                "switchboard item-done --says 'Done.' --checked-in main",
                false,
            ),
            ("switchboard item-done --says 'Done.' --hash=abc", false),
            (r"switchboard show a\'b", true),
            (r"switchboard show a\", false),
            (
                "switchboard item-done --says 'Done.' --close-issue && rm -rf x",
                false,
            ),
            ("cd x && switchboard item-done --says 'Done.'", false),
            ("switchboard item-done --says \"$(id)\"", false),
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
            ("Bash", "git log --output=x", true),
            ("Bash", "git log -p --output x", true),
            ("Bash", "git diff HEAD~1 --output=/tmp/x", true),
            ("Bash", "git -C /x diff --ext-diff", true),
            ("Bash", "git log -p \"--ext-diff\"", true),
            ("Bash", "git diff --ext\\-diff", true),
            ("Bash", "git fetch --upload-pack=x", true),
            ("Bash", "git fetch origin --upl='sh -c id'", true),
            ("Bash", "cd x && git fetch -q --upload-pack x origin", true),
            ("Bash", "git log --oneline -5", false),
            ("Bash", "git diff --stat main...HEAD", false),
            ("Bash", "git fetch origin --multiple https://x/y", true),
            ("Bash", "git fetch origin --m upstream", true),
            ("Bash", "git log -1 --format='tformat:a;b' --output=x", true),
            (
                "Bash",
                "git fetch origin 'main;' --upload-pack='sh -c id'",
                true,
            ),
            ("Bash", r"git log a\;b --output=x", true),
            ("Bash", "git log --format=\"%h (%s)\" -5", false),
            ("Bash", "echo 'git pull; x'", false),
            ("Bash", r#"git log --format="a\";b" --output=x"#, true),
            ("Bash", r#"git log --format='a\' --output=x"#, true),
            ("Bash", "git log --out\\\nput=x", true),
            ("Bash", "git log \"--out\\\nput=x\"", true),
            ("Bash", "git log $'--output=x'", true),
            ("Bash", "git fetch origin $'--upload-pack=x'", true),
            ("Bash", "git log --grep=output -5", false),
            ("Bash", r"git log $'--out\x70ut=x'", true),
            ("Bash", "git diff $(git merge-base main HEAD)", true),
            ("Bash", "git fetch origin `echo x`", true),
            ("Bash", "git log --format='%h $x' -5", false),
            ("Bash", "git status $X", false),
            ("Bash", "git diff --out{put,put}=x", true),
            ("Bash", "git diff --out{p..p}ut=x", true),
            ("Bash", "git log --out*", true),
            ("Bash", "git log origin/main..@{u} --oneline", false),
            ("Bash", "git diff -- '*.rs'", false),
            ("Bash", "git log 2>&1 --output=x", true),
            ("Bash", "git log &>/dev/null --output=x", true),
            ("Bash", "git log -1 >&2 --output=x", true),
            ("Bash", "git log -1 2>&1 | head", false),
            ("Bash", "git fetch origin && git log --max-count=5", false),
            ("Bash", "git log -1; git log --output=x", true),
            ("Bash", "2>/dev/null git log --output=x", true),
            ("Bash", "git log -1 && printf '%s\\n' \"$PWD\"", false),
            ("Bash", "echo $X; git log --format=a -1", false),
            ("Bash", "git log -1 2>&1 $X", true),
            ("Bash", "2> /dev/null git log --output=x", true),
            ("Bash", "git 2>/dev/null log --output=x", true),
            ("Bash", "git log -1 > out.txt", false),
            ("Bash", "2> /dev/null git log $X", true),
            ("Bash", "git 2>&1 log --output=x", true),
            ("Bash", "OPT=--output=x; git 2>/dev/null log \"$OPT\"", true),
            (
                "Bash",
                "OPT=--output=x; git -c user.name='a log ;' log \"$OPT\"",
                true,
            ),
            (
                "Bash",
                "OPT=--output=x; git log -1; git l\"\"og \"$OPT\"",
                true,
            ),
            ("Bash", r"&> /dev/null git log $'--out\x70ut=x'", true),
            ("Bash", "&> /dev/null git log --output=x", true),
            ("Bash", "git -c $CONFIG log", true),
            ("Bash", "X=1; git log -1", false),
            ("Bash", "# '\ngit log $'--out\\x70ut=x'", true),
            ("Bash", "git log -1 # --output=x", false),
            ("Bash", "git log --grep=a#b --output=x", true),
            ("Bash", r"git log --format=a\ #b --output=x", true),
            ("Bash", "cat <<EOF\nhi\nEOF", false),
            ("Bash", "HOME=--output=x; git log ~", true),
            ("Bash", "git diff HEAD~1 --stat", false),
            (
                "Bash",
                "git commit -m \"$(cat <<'EOF'\nFix git diff --output=x rendering\nEOF\n)\"",
                false,
            ),
            ("Bash", "echo \"git log --output=x\"", false),
            ("Bash", "cd \"$repo\" && git diff --stat", false),
            ("Bash", "cd \"$repo\" && git diff $X", true),
            (
                "Bash",
                "git commit -m 'Fix\n\ngit diff --output=file bypassed it'",
                false,
            ),
            ("Bash", "git commit -m \"Fix\ngit diff $X\"", false),
            ("Bash", "cd \"$repo\" && git 2>/dev/null diff --stat", false),
            ("Bash", "cd x && git 2>/dev/null diff $X", true),
            ("Bash", "git diff-tree --no-commit-id -r \"$commit\"", false),
            ("Bash", "git diff-tree --output=x", false),
            ("Bash", "git diff -- \"$file\"", false),
            ("Bash", "git log --oneline -- $f", false),
            ("Bash", "git log $X -- f", true),
            ("Bash", "git log --$X", true),
            (
                "Bash",
                "cat > script.sh <<'EOF'\ngit diff \"$@\"\nEOF",
                false,
            ),
            (
                "Bash",
                "cat > s.sh <<EOF\ngit log $X\nEOF\ngit log $Y",
                true,
            ),
            ("Bash", "cat <<-'EOF'\n\tgit log --output=x\n\tEOF", false),
            ("Bash", "cat <<EOF\n$(cat .env)\nEOF", true),
            ("Bash", "cat <<EOF\n$(git pull)\nEOF", true),
            ("Bash", "# <<'EOF'\ncat .env", true),
            ("Bash", "echo '<<\"EOF\"'\ngit pull", true),
            ("Bash", "cat <<'EOF'\ngit pull", true),
            ("Bash", "cat <<EOF\n# $(cat .env)\nEOF", true),
            ("Bash", "cat <<EOF\n# $(git pull)\nEOF", true),
            ("Bash", "ls # git pull later", false),
            ("Bash", "cat <<<'true'\ncat .env\ntrue", true),
            ("Bash", "cat <<<'true'\ngit log $X\ntrue", true),
            ("Bash", "git fetch origin main", false),
            ("Bash", "git log; echo --output", false),
            ("Bash", "env git pull", true),
            ("Bash", "command git pull", true),
            ("Bash", "env FOO=1 git pull", true),
            ("Bash", "cat .env", true),
            ("Bash", "rg -F '.env' README.md", false),
            ("Bash", "grep .env README.md", false),
            ("Bash", "grep -e .env -e KEY README.md", false),
            ("Bash", "grep -f .env README.md", true),
            ("Bash", "rg -e KEY .env", true),
            ("Bash", "sed -n 1p .env", true),
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

    /// The module's landing check under node, with the lookups answered:
    /// what each way refuses, and that it names the way and Settings' row.
    #[test]
    fn the_module_refuses_what_the_way_to_land_forbids() {
        let Some(node) = crate::util::which("node") else {
            eprintln!("no node on the PATH: the module's landing check isn't tried");
            return;
        };
        let dir = std::env::temp_dir().join(format!("sb-landing-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("permissions.mjs"), module()).unwrap();
        // Each case: the way, the PR's base (read by nothing: the hook asks
        // GitHub nothing), the branch checked out (and
        // pushed to by a bare push), the command.
        fs::write(
            dir.join("ask.mjs"),
            "import { landingRefused } from './permissions.mjs';\n\
             const cases = JSON.parse(process.argv[2]);\n\
             const out = [];\n\
             for (const [way, base, branch, command] of cases) {\n\
               const look = {\n\
                 landing: async () => way && { lands: way, defaultBranch: 'main', repo: 'acme/app' },\n\
                 branch: async (_, ref) => ref === 'HEAD' ? branch : branch && 'origin/' + branch,\n\
               };\n\
               out.push(await landingRefused(command, look));\n\
             }\n\
             console.log(JSON.stringify(out));\n",
        )
        .unwrap();
        let cases = [
            ("switchboard-queue", "main", "x", "gh pr merge 1", true),
            (
                "switchboard-queue",
                "main",
                "x",
                "git push origin main",
                true,
            ),
            (
                "switchboard-queue",
                "main",
                "x",
                "git push origin HEAD:main",
                true,
            ),
            (
                "switchboard-queue",
                "main",
                "x",
                "git push -u origin HEAD:refs/heads/main",
                true,
            ),
            (
                "switchboard-queue",
                "main",
                "x",
                "git push --force origin +main",
                true,
            ),
            ("switchboard-queue", "main", "main", "git push", true),
            (
                "switchboard-queue",
                "main",
                "main",
                "git push origin HEAD",
                true,
            ),
            (
                "switchboard-queue",
                "main",
                "x",
                "git push --all origin",
                true,
            ),
            (
                "switchboard-queue",
                "main",
                "x",
                "cd y && git -C /r push origin 'main' 2>&1",
                true,
            ),
            ("switchboard-queue", "main", "x", "git push", false),
            (
                "switchboard-queue",
                "main",
                "x",
                "git push origin HEAD:x",
                false,
            ),
            (
                "switchboard-queue",
                "main",
                "x",
                "git push origin main-fix",
                false,
            ),
            (
                "switchboard-queue",
                "main",
                "x",
                "git commit -m 'git push origin main'",
                false,
            ),
            ("switchboard-queue", "main", "x", "gh pr view 1", false),
            (
                "switchboard-queue",
                "feat/base",
                "x",
                "gh pr merge 2 --auto --match-head-commit abc --squash",
                false,
            ),
            (
                "switchboard-queue",
                "main",
                "x",
                "gh pr merge 2 --auto --squash",
                true,
            ),
            (
                "switchboard-queue",
                "main",
                "x",
                "gh pr merge 2 --squash",
                true,
            ),
            (
                "switchboard-queue",
                "main",
                "x",
                "git status; gh pr merge 1 --squash",
                true,
            ),
            (
                "switchboard-queue",
                "main",
                "x",
                "git push alice HEAD:main",
                false,
            ),
            (
                "switchboard-queue",
                "main",
                "main",
                "git push --tags",
                false,
            ),
            (
                "switchboard-queue",
                "main",
                "main",
                "git push origin --tags",
                false,
            ),
            (
                "switchboard-queue",
                "main",
                "main",
                "git push --follow-tags",
                true,
            ),
            ("switchboard-queue", "main", "x", "git push --all", true),
            (
                "switchboard-queue",
                "main",
                "x",
                "gh pr merge -R acme/other 5",
                false,
            ),
            (
                "switchboard-queue",
                "main",
                "x",
                "gh pr merge --repo=acme/other 5",
                false,
            ),
            (
                "switchboard-queue",
                "main",
                "x",
                "GH_REPO=acme/other gh pr merge 5",
                false,
            ),
            (
                "switchboard-queue",
                "main",
                "x",
                "gh pr merge https://github.com/acme/other/pull/5",
                false,
            ),
            (
                "switchboard-queue",
                "main",
                "x",
                "gh pr merge 5 --disable-auto",
                false,
            ),
            (
                "direct",
                "feat/base",
                "x",
                "gh pr merge 3 --auto --match-head-commit abc --merge",
                false,
            ),
            ("direct", "main", "x", "git push origin main", false),
            ("direct", "main", "x", "gh pr merge 1", true),
            (
                "auto-merge",
                "main",
                "x",
                "gh pr merge 1 --auto --squash",
                false,
            ),
            ("auto-merge", "main", "x", "git push origin main", true),
            ("github-queue", "main", "x", "gh pr merge 1", false),
            ("service", "feat/base", "x", "gh pr merge 1 --auto", true),
            ("service", "main", "x", "git push origin HEAD:main", true),
        ];
        let mut input: Vec<Value> = cases
            .iter()
            .map(|(way, base, branch, c, _)| serde_json::json!([way, base, branch, c]))
            .collect();
        // No way set: nothing refused.
        input.push(serde_json::json!([
            null,
            "main",
            "main",
            "git push origin main"
        ]));
        let out = std::process::Command::new(node)
            .arg(dir.join("ask.mjs"))
            .arg(serde_json::to_string(&input).unwrap())
            .output()
            .unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let got: Vec<Option<String>> = serde_json::from_slice(&out.stdout).unwrap();
        let mut wrong = Vec::new();
        for ((way, _, _, command, refused), why) in cases.iter().zip(&got) {
            if why.is_some() != *refused {
                wrong.push(format!("{way}: {command} gave {why:?}"));
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
        assert_eq!(got.last(), Some(&None));
        let why = got[0].as_deref().unwrap();
        assert!(
            why.contains("acme/app lands by Switchboard's merge queue (`switchboard-queue`)")
                && why.contains("Settings' Repositories"),
            "{why}"
        );
        assert!(got[1].as_deref().unwrap().contains("default branch, main"));
    }
}
