//! The skills a tab can run, for the Skill form (lib/slack-relay/
//! skills.js). Slack takes any message starting with "/" for one of its
//! own commands and never sends it, so a skill can't be typed into a
//! thread as it is at the keyboard. The tab's screen and the buttons under
//! a finished turn offer a Skill form instead: a menu of the skills that
//! tab can run, and an Arguments field for what follows the name, which
//! says what the skill picked takes when its file has an argument-hint.
//! The menu is the tab's own:
//!
//! - the skills its session was told about, which Claude Code writes into
//!   the transcript as `skill_listing` attachments: the whole list when the
//!   session starts, then any added since;
//! - skills only you can invoke, which that list leaves out, from the skill
//!   and command folders Claude Code reads: yours in ~/.claude, and the
//!   project's in .claude in the tab's directory and the directories above.
//!
//! A Slack menu holds at most 100 options, and a project can have more
//! skills than that, so each says where it came from: the ones you use
//! most first, then yours, then the project's, then Claude Code's own that
//! finish without asking anything (`BUILT_IN`; one that opens a picker
//! would wait on a key Slack can't press), then everything else the session
//! lists, which is where a long list is cut. The form has a field for a
//! name the menu left out.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::config::Paths;
use crate::util;

pub static SKILL_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\w.-]+(?::[\w.-]+)?$").expect("a regex"));
/// A Slack menu holds at most 100 options.
pub const MAX_SKILLS: usize = 100;
pub const SOURCES: &[&str] = &["frequent", "yours", "project", "claude", "session"];

/// Claude Code's own commands worth running from Slack: (name,
/// description, argument hint).
pub const BUILT_IN: &[(&str, &str, &str)] = &[
    (
        "compact",
        "Summarize the conversation so far to free up context",
        "[what the summary should keep]",
    ),
    ("clear", "Start a new conversation in the tab", ""),
    ("context", "Show how the context is used", ""),
    ("cost", "Show this session's cost and duration", ""),
    ("usage", "Show your plan's usage limits", ""),
];

/// The skills you use most, of those the tab can run, from the count and
/// last use Claude Code keeps for each in ~/.claude.json (`skillUsage`).
/// Each use counts for half as much every `USE_HALF_LIFE_DAYS`, so a skill
/// you used a lot months ago gives way to one you use now. At most
/// `FREQUENT` of them, each used at least `MIN_USES` times.
pub const FREQUENT: usize = 8;
pub const MIN_USES: f64 = 2.0;
const USE_HALF_LIFE_DAYS: f64 = 30.0;

pub fn usage_scores(paths: &Paths) -> HashMap<String, f64> {
    let Some(usage) = util::read_json(paths.home.join(".claude.json"))
        .and_then(|v| v.get("skillUsage").cloned())
        .and_then(|v| v.as_object().cloned())
    else {
        return HashMap::new();
    };
    let now = util::now_ms() as f64;
    usage
        .into_iter()
        .filter_map(|(name, use_)| {
            let count = use_.get("usageCount").and_then(Value::as_f64)?;
            if count < MIN_USES {
                return None;
            }
            let last = use_
                .get("lastUsedAt")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let age = (now - last) / (USE_HALF_LIFE_DAYS * 86_400_000.0);
            Some((name, count * 0.5f64.powf(age)))
        })
        .collect()
}

fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max {
        format!("{}…", flat.chars().take(max - 1).collect::<String>())
    } else {
        flat
    }
}

/// A skill or command file's description and argument-hint, from its
/// front matter.
fn describe(file: &Path) -> (String, String) {
    static QUOTES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"^["'>|-]\s*|["']$"#).expect("a regex"));
    let Some(text) = util::read_text(file) else {
        return (String::new(), String::new());
    };
    let head: String = text.chars().take(4000).collect();
    let front = head
        .strip_prefix("---")
        .and_then(|rest| rest.find("\n---").map(|end| rest[..end].to_string()))
        .unwrap_or_default();
    let field = |key: &str| {
        front
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{key}:")))
            .map(|v| QUOTES.replace_all(v.trim(), "").into_owned())
            .unwrap_or_default()
    };
    (field("description"), field("argument-hint"))
}

/// The .claude folders that apply to a directory: its own and its
/// parents', up to (not including) your home, which is read as yours.
fn project_folders(home: &Path, dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut at = Some(dir);
    while let Some(d) = at {
        if d == home || d.parent().is_none() {
            break;
        }
        out.push(d.join(".claude"));
        at = d.parent();
    }
    out
}

/// One skill in the menu.
#[derive(Clone, Debug, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub hint: String,
    pub source: String,
}

/// What a tab can run: the ones you use most, most used first, then yours,
/// the project's and the rest, each sorted by name.
pub fn skills_for(paths: &Paths, dir: Option<&Path>, transcript: Option<&Path>) -> Vec<Skill> {
    let mut found: Vec<Skill> = Vec::new();
    let mut add = |name: &str, description: &str, source: &str, hint: &str| {
        if !SKILL_NAME.is_match(name) {
            return;
        }
        match found.iter_mut().find(|s| s.name == name) {
            None => found.push(Skill {
                name: name.to_string(),
                description: one_line(description, 150),
                hint: one_line(hint, 150),
                source: source.to_string(),
            }),
            Some(known) if known.description.is_empty() => {
                known.description = one_line(description, 150)
            }
            Some(_) => {}
        }
    };
    let mut folders = vec![(paths.claude_dir.clone(), "yours")];
    if let Some(dir) = dir {
        folders.extend(
            project_folders(&paths.home, dir)
                .into_iter()
                .map(|f| (f, "project")),
        );
    }
    for (folder, source) in folders {
        let mut skills: Vec<_> = std::fs::read_dir(folder.join("skills"))
            .map(|r| r.flatten().collect())
            .unwrap_or_default();
        skills.sort_by_key(|e| e.file_name());
        for entry in skills {
            let file = entry.path().join("SKILL.md");
            if !file.exists() {
                continue;
            }
            let (description, hint) = describe(&file);
            add(
                &entry.file_name().to_string_lossy(),
                &description,
                source,
                &hint,
            );
        }
        let mut commands: Vec<_> = std::fs::read_dir(folder.join("commands"))
            .map(|r| r.flatten().collect())
            .unwrap_or_default();
        commands.sort_by_key(|e| e.file_name());
        for entry in commands {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(name) = name.strip_suffix(".md") else {
                continue;
            };
            let (description, hint) = describe(&entry.path());
            add(name, &description, source, &hint);
        }
    }
    // The session's list has no argument hints; only the files do.
    let listed = transcript.map(listed_skills).unwrap_or_default();
    for (name, description, hint) in BUILT_IN {
        add(name, description, "claude", hint);
    }
    for (name, description) in &listed {
        add(name, description, "session", "");
    }
    let scores = usage_scores(paths);
    let mut used: Vec<&Skill> = found
        .iter()
        .filter(|s| scores.contains_key(&s.name))
        .collect();
    used.sort_by(|a, b| scores[&b.name].total_cmp(&scores[&a.name]));
    let frequent: Vec<String> = used.iter().take(FREQUENT).map(|s| s.name.clone()).collect();
    let mut out: Vec<Skill> = found
        .iter()
        .cloned()
        .map(|mut s| {
            if frequent.contains(&s.name) {
                s.source = "frequent".into();
            }
            s
        })
        .collect();
    let rank = |s: &Skill| {
        SOURCES
            .iter()
            .position(|x| *x == s.source)
            .unwrap_or(SOURCES.len())
    };
    out.sort_by(|a, b| {
        rank(a).cmp(&rank(b)).then_with(|| {
            if a.source == "frequent" {
                scores[&b.name].total_cmp(&scores[&a.name])
            } else {
                std::cmp::Ordering::Equal
            }
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        })
    });
    out
}

/// The skills a session's transcript lists, in `skill_listing`
/// attachments: the whole list at the start, then any added since.
fn listed_skills(transcript: &Path) -> Vec<(String, String)> {
    static LINE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^- (\S+?)(?: \([^)\n]*\))?: (.*)$").expect("a regex"));
    let Some(text) = util::read_text(transcript) else {
        return Vec::new();
    };
    let mut listed: Vec<(String, String)> = Vec::new();
    for line in text.lines().filter(|l| l.contains("\"skill_listing\"")) {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(attachment) = entry.get("attachment") else {
            continue;
        };
        if attachment.get("type").and_then(Value::as_str) != Some("skill_listing") {
            continue;
        }
        let content = attachment
            .get("content")
            .map(util::js_string)
            .unwrap_or_default();
        let described: Vec<(String, String)> = LINE
            .captures_iter(&content)
            .map(|c| (c[1].to_string(), c[2].to_string()))
            .collect();
        let names: Vec<String> = match attachment.get("names").and_then(Value::as_array) {
            Some(names) => names
                .iter()
                .filter_map(|n| n.as_str().map(str::to_string))
                .collect(),
            None => described.iter().map(|(n, _)| n.clone()).collect(),
        };
        let entries: Vec<(String, String)> = names
            .into_iter()
            .map(|n| {
                let d = described
                    .iter()
                    .find(|(m, _)| *m == n)
                    .map(|(_, d)| d.clone())
                    .unwrap_or_default();
                (n, d)
            })
            .collect();
        if attachment.get("isInitial").is_some_and(util::truthy) {
            listed = entries;
        } else {
            listed.extend(entries);
        }
    }
    listed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_tabs_skills_come_from_its_sessions_list_and_the_skill_folders() {
        let root = std::env::temp_dir().join(format!("relay-skills-{}", std::process::id()));
        let home = root.join("home");
        let paths = Paths::under(&home, &root.join("s"));
        let project = root.join("project");
        std::fs::create_dir_all(project.join(".claude/commands")).unwrap();
        std::fs::write(
            project.join(".claude/commands/deploy.md"),
            "---\ndescription: Deploy this project\n---\nDeploy it.\n",
        )
        .unwrap();
        std::fs::create_dir_all(home.join(".claude/skills/shape")).unwrap();
        std::fs::write(
            home.join(".claude/skills/shape/SKILL.md"),
            "---\nname: shape\ndescription: \"Shape a ticket\"\nargument-hint: \"[idea or ticket ID]\"\ndisable-model-invocation: true\n---\n",
        )
        .unwrap();
        let transcript = root.join("t.jsonl");
        let listing = |initial: bool, content: &str| {
            json!({ "type": "attachment", "attachment": { "type": "skill_listing", "isInitial": initial, "content": content } })
                .to_string()
        };
        std::fs::write(
            &transcript,
            [
                listing(true, "- review-loop: Review in a loop\n- old: gone"),
                listing(
                    true,
                    "- review-loop (plugin): Review in a loop\n- shape: Shape a ticket",
                ),
                listing(false, "- added: Added later"),
            ]
            .join("\n"),
        )
        .unwrap();
        std::fs::write(
            home.join(".claude.json"),
            json!({ "skillUsage": { "review-loop": { "usageCount": 9, "lastUsedAt": util::now_ms() } } }).to_string(),
        )
        .unwrap();
        let skills = skills_for(&paths, Some(&project), Some(&transcript));
        let summary: Vec<(&str, &str)> = skills
            .iter()
            .map(|s| (s.name.as_str(), s.source.as_str()))
            .collect();
        assert_eq!(
            &summary[..3],
            &[
                ("review-loop", "frequent"),
                ("shape", "yours"),
                ("deploy", "project")
            ]
        );
        assert!(summary.contains(&("compact", "claude")));
        assert!(summary.contains(&("added", "session")));
        assert!(
            !summary.iter().any(|(n, _)| *n == "old"),
            "a later full list replaces the first"
        );
        let shape = skills.iter().find(|s| s.name == "shape").unwrap();
        assert_eq!(
            (shape.description.as_str(), shape.hint.as_str()),
            ("Shape a ticket", "[idea or ticket ID]")
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
