//! The pull requests a Claude session opens or looks at, read from its
//! own tool calls as they end: Claude Code hands every `PostToolUse` to the
//! server (`switchboard hook`), with the command and what it printed, so a
//! Bash call running `gh pr create` or `gh pr view` gives the PRs whose
//! addresses its output carries, from any branch, worktree or repository,
//! which the tab then follows (`tabs::prs`). New code, with nothing of the tmux version's before it.
//!
//! Only those commands count, and only the address each prints of its own
//! PR: from `gh pr create`, a line that is the address alone (what it ends
//! with); from either, `gh pr view`'s `url:` line or the `"url"` field of
//! a JSON object `--json` prints (never an array, as `gh pr list` prints),
//! read up to the `--` before the PR's body. A line that is an address
//! alone counts only after `gh pr create`, since `gh pr view` with `--jq`
//! or `--template` prints a body with no `--` before it. A PR linked from a
//! body or a comment, or a command that merely mentions an address (an
//! `echo`, a `curl`, a PR read through the API), names none, since the
//! session hasn't opened it.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::integrations::github::Repo;

/// A pull request by its repository and number.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Opened {
    pub repo: Repo,
    pub number: u64,
    /// Made by the call: the address alone on its line that `gh pr create`
    /// ends with, rather than a PR looked at.
    pub made: bool,
}

/// The PRs a `PostToolUse` hook's JSON says its call opened or looked at,
/// each once, in the order its output gave them.
pub fn opened(payload: &Value) -> Vec<Opened> {
    static GH_PR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?:^|[\s;&|(`])gh\s+pr\s+(?:create|view)\b").unwrap());
    static URL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(?:url:\s*)?https://github\.com/([\w.-]+)/([\w.-]+)/pull/(\d+)/?$").unwrap()
    });
    if payload.get("tool_name").and_then(Value::as_str) != Some("Bash") {
        return Vec::new();
    }
    let command = payload
        .pointer("/tool_input/command")
        .and_then(Value::as_str)
        .unwrap_or("");
    static CREATE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?:^|[\s;&|(`])gh\s+pr\s+create\b").unwrap());
    if !GH_PR.is_match(command) {
        return Vec::new();
    }
    let creates = CREATE.is_match(command);
    let response = payload.get("tool_response").unwrap_or(&Value::Null);
    let output = match response {
        Value::String(text) => text.clone(),
        other => ["stdout", "stderr"]
            .iter()
            .filter_map(|k| other.get(k).and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
    };
    let mut found: Vec<Opened> = Vec::new();
    // gh pr view's metadata ends at `--`, its body and comments after it.
    for line in output.lines().map(str::trim).take_while(|l| *l != "--") {
        // A JSON object of gh pr view --json gives its own url field; an
        // array (gh pr list) or a body's text gives none.
        let address = if line.starts_with('{') {
            serde_json::from_str::<Value>(line)
                .ok()
                .and_then(|v| v.get("url").and_then(Value::as_str).map(str::to_string))
        } else if creates || line.starts_with("url:") {
            // An address alone on its line is gh pr create's only.
            Some(line.to_string())
        } else {
            None
        };
        let made = creates && line.starts_with("https://");
        let Some(at) = address.as_deref().and_then(|a| URL.captures(a)) else {
            continue;
        };
        let Ok(number) = at[3].parse() else {
            continue;
        };
        let pr = Opened {
            repo: Repo {
                owner: at[1].to_string(),
                name: at[2].to_string(),
            },
            number,
            made,
        };
        if !found
            .iter()
            .any(|f| f.repo == pr.repo && f.number == pr.number)
        {
            found.push(pr);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn bash(command: &str, stdout: &str) -> Value {
        json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": command},
            "tool_response": {"stdout": stdout, "stderr": "", "interrupted": false},
        })
    }

    fn pr(owner: &str, name: &str, number: u64) -> Opened {
        Opened {
            repo: Repo {
                owner: owner.into(),
                name: name.into(),
            },
            number,
            made: false,
        }
    }

    fn by_create(owner: &str, name: &str, number: u64) -> Opened {
        Opened {
            made: true,
            ..pr(owner, name, number)
        }
    }

    #[test]
    fn a_gh_pr_create_gives_the_pr_it_printed() {
        let made = bash(
            "gh pr create --base main --title 'Map' --body x",
            "Creating pull request for web-142 into main in acme/web-app\n\nhttps://github.com/acme/web-app/pull/2040\n",
        );
        assert_eq!(opened(&made), vec![by_create("acme", "web-app", 2040)]);
    }

    #[test]
    fn another_repository_s_pr_counts_too() {
        let made = bash(
            "cd ~/code/oncall-tools && gh pr create --draft --fill",
            "https://github.com/acme/oncall-tools/pull/318",
        );
        assert_eq!(opened(&made), vec![by_create("acme", "oncall-tools", 318)]);
        let viewed = bash(
            "gh pr view 41 --repo acme/notes",
            "title:\tFixtures\nurl:\thttps://github.com/acme/notes/pull/41\n",
        );
        assert_eq!(opened(&viewed), vec![pr("acme", "notes", 41)]);
    }

    #[test]
    fn a_command_that_only_mentions_an_address_gives_none() {
        let url = "https://github.com/acme/web-app/pull/2040";
        assert!(opened(&bash(&format!("echo {url}"), url)).is_empty());
        assert!(opened(&bash("gh api repos/acme/web-app/pulls/2040", url)).is_empty());
        assert!(opened(&bash("git log --grep 'gh pr create'", url)).is_empty());
        let mut read = bash("gh pr create", url);
        read["tool_name"] = json!("Read");
        assert!(opened(&read).is_empty());
    }

    #[test]
    fn a_pr_linked_from_a_body_or_a_comment_gives_none() {
        let viewed = bash(
            "gh pr view 41 --comments",
            "title:\tFixtures\nurl:\thttps://github.com/acme/notes/pull/41\n--\nStacked on https://github.com/acme/notes/pull/40\nhttps://github.com/acme/notes/pull/39\n",
        );
        assert_eq!(opened(&viewed), vec![pr("acme", "notes", 41)]);
        let json = bash(
            "gh pr view 41 --json url,body",
            r#"{"body":"Reverts https://github.com/other/repo/pull/9","url":"https://github.com/acme/notes/pull/41"}"#,
        );
        assert_eq!(opened(&json), vec![pr("acme", "notes", 41)]);
        let jq = bash(
            "gh pr view 41 --json body --jq .body",
            "Stacked on this one:\nhttps://github.com/acme/notes/pull/40\n",
        );
        assert!(opened(&jq).is_empty());
        let listed = bash(
            "gh pr view 41 --json url && gh pr list --json number,url",
            "{\"url\":\"https://github.com/acme/notes/pull/41\"}\n[{\"number\":7,\"url\":\"https://github.com/acme/notes/pull/7\"}]",
        );
        assert_eq!(opened(&listed), vec![pr("acme", "notes", 41)]);
    }

    #[test]
    fn each_pr_once_in_the_order_printed() {
        let made = bash(
            "gh pr create && gh pr view",
            "https://github.com/a/b/pull/2\nhttps://github.com/a/b/pull/1\nhttps://github.com/a/b/pull/2",
        );
        assert_eq!(
            opened(&made),
            vec![by_create("a", "b", 2), by_create("a", "b", 1)]
        );
    }
}
