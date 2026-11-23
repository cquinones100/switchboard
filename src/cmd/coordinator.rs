//! The coordinator's two programs (src/coordinator), the ways its session
//! reaches GitHub without asking, each checking in code what its skill
//! only says, so text it read on GitHub can't steer it past them:
//!
//! - `coordinator-order <owner/repo#number> <1-4>`: gives the issue that
//!   `priority:` label (`issues::set_priority`, as the Autopilot board's
//!   Move up and Move down do), the order autopilot takes it in, on a
//!   repository you can push to, changing nothing else of it.
//! - `coordinator-post issue -C <clone> --title <title> --body-file <file>`
//!   and `coordinator-post comment -C <clone> <number> --body-file <file>`:
//!   files an issue (assigned to you) or comments on one, only on the
//!   GitHub repository of the clone's origin and only one you can push to,
//!   only with a body file in the coordinator's posts folder
//!   (`coordinator::posts_dir`, the one place it writes without asking), and only when leak-check finds nothing in
//!   the title and body but bare `#<number>`s, the repository's own issues
//!   and PRs; prints the address of what it posted.
//!
//! New, with no tmux-version file behind it. Both refuse while Settings'
//! `github` is off, as the workspace's own use of GitHub does.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Value, json};

use crate::integrations::github::{self, Repo};
use crate::util;

const SOURCE: &str = "coordinator";

fn fail(what: &str, why: &str) -> ExitCode {
    eprintln!("{what}: {why}");
    ExitCode::from(1)
}

/// `coordinator-order <owner/repo#number> <1-4>`.
pub fn order_main(args: Vec<String>) -> ExitCode {
    const NAME: &str = "coordinator-order";
    let usage = || {
        eprintln!("usage: switchboard {NAME} <owner/repo#number> <1-4>");
        ExitCode::from(2)
    };
    let [reference, priority] = args.as_slice() else {
        return usage();
    };
    let Some(priority) = priority.parse::<u8>().ok().filter(|p| (1..=4).contains(p)) else {
        return usage();
    };
    let Some((repo, number)) = crate::autopilot::issues::parse_reference(reference) else {
        return fail(NAME, &format!("{reference} isn't <owner/repo#number>"));
    };
    let set = util::block_on(async {
        pushable(&repo).await?;
        crate::autopilot::issues::set_priority(&repo, number, priority).await
    });
    match set {
        Ok(()) => {
            println!(
                "{repo}#{number}: {}",
                crate::autopilot::issues::priority_label(priority)
            );
            ExitCode::SUCCESS
        }
        Err(e) => fail(NAME, &format!("{e:#}")),
    }
}

/// Refuses a repository you can't push to, so nothing changes on someone
/// else's.
async fn pushable(repo: &Repo) -> anyhow::Result<()> {
    let about = github::rest("GET", &format!("/repos/{repo}"), None, SOURCE).await?;
    if about.pointer("/permissions/push") != Some(&Value::Bool(true)) {
        anyhow::bail!("{repo} isn't a repository you can push to");
    }
    Ok(())
}

/// What `coordinator-post` posts.
#[derive(Debug, PartialEq, Eq)]
enum Post {
    Issue { title: String },
    Comment { number: u64 },
}

/// Its arguments: what it posts, the clone, and the body's file.
fn post_args(args: &[String]) -> Option<(Post, PathBuf, PathBuf)> {
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let clone = PathBuf::from(value("-C")?);
    let body = PathBuf::from(value("--body-file")?);
    let post = match args.first()?.as_str() {
        "issue" if args.len() == 7 => Post::Issue {
            title: value("--title")?,
        },
        "comment" if args.len() == 6 => Post::Comment {
            number: args.get(3)?.parse().ok()?,
        },
        _ => return None,
    };
    Some((post, clone, body))
}

/// The leak-check's hits in `text` that aren't a bare `#<number>`, which on
/// a repository's own issue names its own issues and PRs.
fn hits_kept(name: &str, text: &str, words: &[String]) -> Vec<String> {
    crate::cmd::leak_check::file_hits(name, text, words)
        .into_iter()
        .filter(|h| h.kind != "PR number")
        .map(|h| h.shown)
        .collect()
}

/// Whether `file` is in `folder`, both by their real paths.
fn inside(file: &Path, folder: &Path) -> bool {
    match (file.canonicalize(), folder.canonicalize()) {
        (Ok(f), Ok(d)) => f.starts_with(d),
        _ => false,
    }
}

/// `coordinator-post issue|comment ...`.
pub fn post_main(args: Vec<String>) -> ExitCode {
    const NAME: &str = "coordinator-post";
    let Some((post, clone, body_file)) = post_args(&args) else {
        eprintln!(
            "usage: switchboard {NAME} issue -C <clone> --title <title> --body-file <file>\n       switchboard {NAME} comment -C <clone> <number> --body-file <file>"
        );
        return ExitCode::from(2);
    };
    let own = crate::coordinator::posts_dir();
    if !inside(&body_file, &own) {
        return fail(
            NAME,
            &format!("the body's file must be in {}", own.display()),
        );
    }
    let body = match std::fs::read_to_string(&body_file) {
        Ok(body) => body,
        Err(e) => return fail(NAME, &format!("can't read {}: {e}", body_file.display())),
    };
    use crate::cmd::leak_check;
    let words = match leak_check::read_words(&leak_check::words_file()) {
        Ok(words) => words,
        Err(e) => return fail(NAME, &format!("leak-check couldn't look: {e}")),
    };
    let mut found = hits_kept("body", &body, &words);
    if let Post::Issue { title } = &post {
        found.extend(hits_kept("title", title, &words));
    }
    if !found.is_empty() {
        for hit in &found {
            println!("{hit}");
        }
        return fail(NAME, "leak-check found the above; ask before posting");
    }
    let posted = util::block_on(async {
        // Origin's repository itself, never an upstream or gh's default,
        // and only one you can push to, so nothing goes on someone else's.
        let origin = crate::worktree::git_at(&clone, &["config", "--get", "remote.origin.url"])
            .map_err(|e| anyhow::anyhow!("{} has no origin: {e}", clone.display()))?;
        let repo: Repo = github::parse_remote(origin.trim())
            .ok_or_else(|| anyhow::anyhow!("{}'s origin isn't on GitHub", clone.display()))?;
        pushable(&repo).await?;
        match &post {
            Post::Issue { title } => {
                let me = github::rest("GET", "/user", None, SOURCE).await?;
                let login = me.get("login").and_then(Value::as_str).unwrap_or_default();
                let mut issue = json!({ "title": title, "body": body });
                if !login.is_empty() {
                    issue["assignees"] = json!([login]);
                }
                github::rest(
                    "POST",
                    &format!("/repos/{repo}/issues"),
                    Some(&issue),
                    SOURCE,
                )
                .await
            }
            Post::Comment { number } => {
                github::rest(
                    "POST",
                    &format!("/repos/{repo}/issues/{number}/comments"),
                    Some(&json!({ "body": body })),
                    SOURCE,
                )
                .await
            }
        }
    });
    match posted {
        Ok(answer) => {
            println!(
                "{}",
                answer
                    .get("html_url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            );
            ExitCode::SUCCESS
        }
        Err(e) => fail(NAME, &format!("{e:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn its_arguments_are_read_strictly() {
        let issue = post_args(&args(&[
            "issue",
            "-C",
            "/r",
            "--title",
            "T",
            "--body-file",
            "/b",
        ]));
        assert_eq!(
            issue,
            Some((
                Post::Issue { title: "T".into() },
                PathBuf::from("/r"),
                PathBuf::from("/b")
            ))
        );
        let comment = post_args(&args(&["comment", "-C", "/r", "12", "--body-file", "/b"]));
        assert_eq!(comment.map(|c| c.0), Some(Post::Comment { number: 12 }));
        assert_eq!(
            post_args(&args(&["comment", "-C", "/r", "x", "--body-file", "/b"])),
            None
        );
        assert_eq!(
            post_args(&args(&[
                "issue",
                "-C",
                "/r",
                "--title",
                "T",
                "--body-file",
                "/b",
                "-R",
                "x/y"
            ])),
            None,
            "nothing more is taken"
        );
    }

    #[test]
    fn a_bare_issue_number_passes_and_anything_else_found_stops_it() {
        // Built at run time, so the leak-check of this file finds none.
        let numbers = format!("Follows #{} and #{}.", 4000 + 2, 4000 + 7);
        assert!(hits_kept("body", &numbers, &[]).is_empty());
        let ticket = format!("See {}-{}.", "DEF", 42);
        assert_eq!(hits_kept("body", &ticket, &[]).len(), 1);
        let listed = hits_kept("body", "Made at Initech.", &["initech".to_string()]);
        assert!(
            listed.len() == 1 && listed[0].contains("listed word"),
            "{listed:?}"
        );
    }

    #[test]
    fn a_body_outside_its_folder_is_refused() {
        let dir = std::env::temp_dir().join(format!("sb-coord-post-{}", std::process::id()));
        let own = dir.join("own");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(own.join("b.md"), "x").unwrap();
        std::fs::write(dir.join("key"), "x").unwrap();
        assert!(inside(&own.join("b.md"), &own));
        assert!(!inside(&dir.join("key"), &own));
        assert!(!inside(&own.join("../key"), &own));
        let _ = std::fs::remove_dir_all(dir);
    }
}
