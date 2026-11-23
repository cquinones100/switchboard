//! `land-lock open-pr --title <title> --body-file <file>`: the branch's
//! first push and its PR, for a tab that lands without asking in a
//! repository landing through a PR (queue-and-clean's "No PR", **Open a
//! PR** taken without asking), checked by code rather than left to Claude
//! Code's auto mode, whose classifier refuses a first push and `gh pr
//! create` however clean the review was. New code, with nothing of the tmux
//! version's before it.
//!
//! It pushes and opens the PR only when every check passes, read at the
//! moment of pushing: the tab's `@land` is `1`, the repository's landing
//! runs the review loop and its record holds two clean rounds of this very
//! change (`codex-review --recorded`'s patch ID equal to the branch's from
//! where it forked), the change bumps no version (a manifest's own
//! `version` line added, which publishes a release once merged and is
//! always asked about), and, in Switchboard's clone alone
//! (`dev::is_switchboard_clone`), `leak-check` finds nothing in the
//! branch's commits nor in the PR's title and body. Anything else exits 6
//! with a `why:` line each and leak-check's hits, pushing nothing, for
//! queue-and-clean's question. Then it pushes the branch to `origin` under
//! its own name, never forced, with the repository's hooks run as for any
//! push, and opens the PR with `gh pr create` on the branch it lands on,
//! printing the PR's number (`#<n>`) and its address alone on a line, which
//! `agent::prs` reads as a PR the session opened.
//!
//! The body is read only from a temporary directory (Claude's scratchpad
//! is one), never a file of yours. The plugin's permission hook allows it
//! alone with those two options
//! (src/setup/permissions.js), so the classifier is never asked about it,
//! while a rule of yours still decides.

use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use crate::cmd::land_push::{self, Onto};
use crate::{repos, worktree};

/// The PR waits on your answer.
const ASK: u8 = 6;

/// What the checks read, gathered before they decide.
#[derive(Debug, Clone, Default)]
pub struct Checks {
    /// The tab's `@land` is `1`.
    pub land: bool,
    /// The repository's `review` is `review-loop`.
    pub review_loop: bool,
    /// The patch the review recorded clean, and the branch's.
    pub recorded: Option<String>,
    pub current: Option<String>,
    /// The change adds a manifest's version line.
    pub bumps_version: bool,
    /// `leak-check` found nothing in the commits, and in the title and body.
    pub leak_clean: bool,
}

/// Why it asks, one reason each check that failed; empty when it opens the
/// PR.
pub fn why_ask(checks: &Checks) -> Vec<&'static str> {
    let mut why = Vec::new();
    if !checks.land {
        why.push("the tab doesn't land without asking");
    }
    if !checks.review_loop {
        why.push("the repository's landing has no review loop");
    }
    let same = matches!((&checks.recorded, &checks.current), (Some(r), Some(c)) if !r.is_empty() && r == c);
    if !same {
        why.push("the change isn't the one the review found clean twice");
    }
    if checks.bumps_version {
        why.push("the change bumps a version, which is always asked about");
    }
    if !checks.leak_clean {
        why.push("leak-check found something, or couldn't look");
    }
    why
}

/// Whether a diff (`git diff -U0`'s) adds a manifest's own version line:
/// `version = "…"` in a `Cargo.toml` or `pyproject.toml`, or `"version":`
/// in a `package.json`, never a dependency's (`serde = { version = … }`).
pub fn bumps_version(diff: &str) -> bool {
    let mut manifest = false;
    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("+++ ") {
            let name = path.rsplit('/').next().unwrap_or_default();
            manifest = matches!(name, "Cargo.toml" | "pyproject.toml" | "package.json");
            continue;
        }
        let Some(added) = line.strip_prefix('+') else {
            continue;
        };
        let added = added.trim_start();
        let toml = added
            .strip_prefix("version")
            .is_some_and(|rest| rest.trim_start().starts_with('='));
        let json = added
            .strip_prefix("\"version\"")
            .is_some_and(|rest| rest.trim_start().starts_with(':'));
        if manifest && (toml || json) {
            return true;
        }
    }
    false
}

const USAGE: &str = "usage: land-lock open-pr --title <title> --body-file <file>";

pub(crate) fn main(
    common: &Path,
    here: &Path,
    target: Option<String>,
    args: &[String],
) -> ExitCode {
    let (title, body_file) = match args {
        [t, title, b, body] if t == "--title" && b == "--body-file" => (title, body),
        [b, body, t, title] if t == "--title" && b == "--body-file" => (title, body),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    if title.trim().is_empty() {
        eprintln!("land-lock open-pr: the title is empty");
        return ExitCode::from(2);
    }
    // Only a file written for it, in a temporary directory (Claude's
    // scratchpad is one), so a run the hook lets through unasked can't
    // publish a file of yours as the PR's body.
    let temporary = [std::env::temp_dir(), "/tmp".into()];
    if !in_one_of(Path::new(body_file), &temporary) {
        eprintln!(
            "land-lock open-pr: {body_file} isn't in a temporary directory: write the body there"
        );
        return ExitCode::from(2);
    }
    let body = match std::fs::read_to_string(body_file) {
        Ok(body) => body,
        Err(e) => {
            eprintln!("land-lock open-pr: {body_file}: {e}");
            return ExitCode::from(2);
        }
    };
    match open(common, here, target, title, body_file, &body) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("land-lock open-pr: {e}");
            ExitCode::FAILURE
        }
    }
}

fn open(
    common: &Path,
    here: &Path,
    target: Option<String>,
    title: &str,
    body_file: &str,
    body: &str,
) -> Result<ExitCode, String> {
    let branch = worktree::git_at(here, &["branch", "--show-current"])
        .ok()
        .filter(|b| !b.is_empty())
        .ok_or("not on a branch")?;
    let onto: Onto = land_push::onto(common, here, target)?;
    if onto.name == branch {
        return Err(format!("{branch} is the branch it would open a PR onto"));
    }
    land_push::fetch(here, &onto)?;
    let base = onto.base();
    let fork = worktree::git_at(here, &["merge-base", &base, "HEAD"])
        .map_err(|e| format!("no merge base with {base}: {e}"))?;
    let range = format!("{fork}..HEAD");
    // The branch's own issue named to close is this repository's, no leak.
    let own_issue = worktree::git_at(
        here,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()
    .and_then(|common| Path::new(&common).parent().map(Path::to_path_buf))
    .and_then(|main| crate::cmd::worktree_from_issue::issue_of_branch(&main, &branch))
    .and_then(|i| crate::autopilot::issues::parse_reference(&i))
    .map(|(_, number)| number);
    let checked_body = without_closing(body, own_issue);
    let leak =
        crate::dev::is_switchboard_clone(here).then(|| leaks(here, &range, title, &checked_body));
    let leak_clean = leak.as_ref().is_none_or(|(clean, _)| *clean);
    let hits = leak.map(|(_, hits)| hits).unwrap_or_default();
    let diff = worktree::git_at(
        here,
        &[
            "diff",
            "-U0",
            "--no-ext-diff",
            "--no-textconv",
            &fork,
            "HEAD",
        ],
    )
    .unwrap_or_default();
    let steps = repos::landing_for(here, false)?.steps;
    let checks = Checks {
        land: land_push::tab_option(crate::mux::server::LAND) == "1",
        review_loop: steps.review == "review-loop",
        recorded: land_push::this_program(here, &["codex-review", "--recorded"])
            .filter(|o| o.status.success())
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .split_whitespace()
                    .nth(2)
                    .map(str::to_string)
            }),
        current: land_push::this_program(here, &["codex-review", "--patch-id", &fork, "HEAD"])
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()),
        bumps_version: bumps_version(&diff),
        leak_clean,
    };
    let why = why_ask(&checks);
    if !why.is_empty() {
        println!("ask: {title}");
        for reason in why {
            println!("why: {reason}");
        }
        for hit in hits.lines().filter(|l| !l.trim().is_empty()) {
            println!("leak: {hit}");
        }
        return Ok(ExitCode::from(ASK));
    }
    worktree::count_fetch(here, "origin", "land-lock open-pr");
    let (pushed, said) = land_push::git(
        here,
        &[
            "push",
            "--set-upstream",
            "origin",
            &format!("HEAD:refs/heads/{branch}"),
        ],
    );
    if !pushed {
        return Err(format!("git push: {said}"));
    }
    // git's own lines, which the Slack relay reads a push from.
    eprintln!("{said}");
    // Named with its owner, so gh finds it when it opens the PR in the
    // project a fork's clone was forked from.
    let head = worktree::git_at(here, &["config", "--get", "remote.origin.url"])
        .ok()
        .and_then(|url| crate::integrations::github::parse_remote(url.trim()))
        .map(|repo| format!("{}:{branch}", repo.owner))
        .unwrap_or_else(|| branch.clone());
    let out = Command::new("gh")
        .args([
            "pr", "create", "--base", &onto.name, "--head", &head, "--title", title,
        ])
        .args(["--body-file", body_file])
        .current_dir(here)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("gh pr create: {e}"))?;
    crate::requests::count("GitHub", "gh", "land-lock open-pr");
    let printed = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        return Err(format!(
            "pushed, but gh pr create failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let address = printed
        .lines()
        .map(str::trim)
        .rfind(|l| l.contains("/pull/"))
        .ok_or_else(|| format!("pushed, but gh pr create printed no PR: {}", printed.trim()))?;
    let number = address
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    println!("#{number}");
    println!("{address}");
    Ok(ExitCode::SUCCESS)
}

/// The body without its line closing the branch's own issue
/// (`Closes #<issue>`), which leak-check would take for a PR number.
fn without_closing(body: &str, issue: Option<u64>) -> String {
    let Some(issue) = issue else {
        return body.to_string();
    };
    let closing = format!("Closes #{issue}");
    body.lines()
        .filter(|l| l.trim() != closing)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `file`, its links followed, is inside one of `dirs` (theirs
/// followed too).
fn in_one_of(file: &Path, dirs: &[std::path::PathBuf]) -> bool {
    let Ok(file) = file.canonicalize() else {
        return false;
    };
    dirs.iter()
        .filter_map(|d| d.canonicalize().ok())
        .any(|d| file.starts_with(d))
}

/// leak-check on the branch's commits and on the PR's title and body:
/// whether both came back clean, and the hits.
fn leaks(here: &Path, range: &str, title: &str, body: &str) -> (bool, String) {
    let file = std::env::temp_dir().join(format!("land-open-pr-{}.md", std::process::id()));
    let text_written = std::fs::write(&file, format!("{title}\n\n{body}")).is_ok();
    let runs = [
        land_push::this_program(here, &["leak-check", range]),
        text_written
            .then(|| {
                land_push::this_program(here, &["leak-check", "--file", &file.to_string_lossy()])
            })
            .flatten(),
    ];
    let _ = std::fs::remove_file(&file);
    let clean = runs
        .iter()
        .all(|o| o.as_ref().is_some_and(|o| o.status.success()));
    let hits = runs
        .iter()
        .flatten()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|h| !h.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    (clean, hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passing() -> Checks {
        Checks {
            land: true,
            review_loop: true,
            recorded: Some("abc".into()),
            current: Some("abc".into()),
            bumps_version: false,
            leak_clean: true,
        }
    }

    type Change = fn(&mut Checks);

    #[test]
    fn it_opens_the_pr_only_when_every_check_passes() {
        assert!(why_ask(&passing()).is_empty());
        let cases: [(Change, &str); 5] = [
            (|c| c.land = false, "doesn't land without asking"),
            (|c| c.review_loop = false, "no review loop"),
            (
                |c| c.current = Some("def".into()),
                "isn't the one the review found clean",
            ),
            (|c| c.bumps_version = true, "bumps a version"),
            (|c| c.leak_clean = false, "leak-check"),
        ];
        for (change, said) in cases {
            let mut checks = passing();
            change(&mut checks);
            let why = why_ask(&checks);
            assert!(why.len() == 1 && why[0].contains(said), "{why:?}");
        }
        let mut none = passing();
        none.recorded = None;
        none.current = None;
        assert_eq!(why_ask(&none).len(), 1, "no record is no clean review");
    }

    #[test]
    fn only_the_line_closing_the_branch_s_own_issue_is_left_out_of_leak_check() {
        let body = "Fix it, as #123 did.\n\nCloses #12345";
        assert_eq!(without_closing(body, Some(12345)), "Fix it, as #123 did.\n");
        assert_eq!(without_closing(body, Some(12346)), body);
        assert_eq!(without_closing(body, None), body);
    }

    #[test]
    fn the_body_is_read_only_from_a_temporary_directory() {
        let tmp = std::env::temp_dir().join(format!("open-pr-body-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("in")).unwrap();
        std::fs::write(tmp.join("in/body.md"), "x").unwrap();
        std::fs::write(tmp.join("other.md"), "x").unwrap();
        let dirs = [tmp.join("in")];
        assert!(in_one_of(&tmp.join("in/body.md"), &dirs));
        assert!(!in_one_of(&tmp.join("other.md"), &dirs));
        assert!(!in_one_of(&tmp.join("in/../other.md"), &dirs));
        std::os::unix::fs::symlink(tmp.join("other.md"), tmp.join("in/link.md")).unwrap();
        assert!(!in_one_of(&tmp.join("in/link.md"), &dirs), "a link out");
        assert!(!in_one_of(&tmp.join("in/missing.md"), &dirs));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_version_bump_is_a_manifest_s_own_version_line_added() {
        let cargo = "+++ b/crates/diff/Cargo.toml\n@@ -3 +3 @@\n-version = \"0.1.0\"\n+version = \"0.2.0\"\n";
        assert!(bumps_version(cargo));
        let npm = "+++ b/package.json\n@@ -2 +2 @@\n+  \"version\": \"1.2.0\",\n";
        assert!(bumps_version(npm));
        let dependency = "+++ b/Cargo.toml\n@@ -9 +9 @@\n+serde = { version = \"1\" }\n";
        assert!(!bumps_version(dependency));
        let elsewhere = "+++ b/src/main.rs\n@@ -1 +1 @@\n+version = 3;\n";
        assert!(!bumps_version(elsewhere));
        let removed = "+++ b/Cargo.toml\n@@ -3 +3 @@\n-version = \"0.1.0\"\n";
        assert!(!bumps_version(removed));
    }
}
