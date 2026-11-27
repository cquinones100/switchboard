//! The coordinator's two programs (src/coordinator), the ways its session
//! reaches GitHub without asking, each checking in code what its skill
//! only says, so text it read on GitHub can't steer it past them:
//!
//! - `coordinator-order <owner/repo#number>... [--left <owner/repo#number>...]`:
//!   gives Next's issues `priority: 1` and those that left Next
//!   `priority: 4` (`issues::set_priority`, as the Autopilot board's Move up
//!   and Move down do), on repositories you can push to, changing nothing
//!   else of them; then tells the server (`coordinator-ordered`) in one go,
//!   which puts Next's first in autopilot's queue in their order and
//!   takes the others out.
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
use crate::mux::socket;
use crate::util;

const SOURCE: &str = "coordinator";

fn fail(what: &str, why: &str) -> ExitCode {
    eprintln!("{what}: {why}");
    ExitCode::from(1)
}

/// Issues by their repository and number.
type Issues = Vec<(Repo, u64)>;

/// Its arguments: Next's issues in order, and after `--left` those that
/// left Next; at least one, every one an `<owner/repo#number>`.
fn order_args(args: &[String]) -> Option<(Issues, Issues)> {
    let (next, left) = match args.iter().position(|a| a == "--left") {
        Some(at) => (&args[..at], &args[at + 1..]),
        None => (args, &[][..]),
    };
    let parse = |refs: &[String]| -> Option<Issues> {
        refs.iter()
            .map(|r| crate::autopilot::issues::parse_reference(r))
            .collect()
    };
    let (next, left) = (parse(next)?, parse(left)?);
    (!next.is_empty() || !left.is_empty()).then_some((next, left))
}

/// `coordinator-order <owner/repo#number>... [--left <owner/repo#number>...]`.
pub fn order_main(args: Vec<String>) -> ExitCode {
    const NAME: &str = "coordinator-order";
    use crate::autopilot::issues::{HIGHEST, LOWEST, set_priority};
    let Some((next, left)) = order_args(&args) else {
        eprintln!(
            "usage: switchboard {NAME} <owner/repo#number>... [--left <owner/repo#number>...]"
        );
        return ExitCode::from(2);
    };
    // Every issue looked up before any label is written, so a wrong one
    // changes nothing; then those that left Next first, so a failure after
    // leaves none of them first; and the server told of what was written
    // even when a later one failed.
    // A repository you can't push to, or an issue GitHub doesn't have, is
    // left out for good, saying which; any other failure changes nothing.
    let looked = util::block_on(async {
        let mut refused: Vec<(String, String)> = Vec::new();
        let mut checked: Vec<String> = Vec::new();
        for (repo, _) in next.iter().chain(&left) {
            let name = repo.to_string();
            if checked.contains(&name) || refused.iter().any(|(r, _)| *r == name) {
                continue;
            }
            match pushable(repo).await {
                Ok(()) => checked.push(name),
                Err(e) if e.downcast_ref::<NotPushable>().is_some() => {
                    refused.push((name, format!("{e:#}")));
                }
                Err(e) if crate::net::kind_of(&e) == Some(&crate::net::ErrorKind::NotFound) => {
                    let why = format!("GitHub has no repository {name} you can see");
                    refused.push((name, why));
                }
                Err(e) => return Err(e),
            }
        }
        let mut skipped = Vec::new();
        let looked_up = |list: &Issues| {
            let list = list.clone();
            let refused = refused.clone();
            async move {
                let mut kept = Vec::new();
                let mut skipped = Vec::new();
                for (repo, number) in list {
                    let reference = format!("{repo}#{number}");
                    if let Some((_, why)) = refused.iter().find(|(r, _)| *r == repo.to_string()) {
                        skipped.push(format!("skipped {reference}: {why}"));
                        continue;
                    }
                    let path = format!("/repos/{repo}/issues/{number}");
                    match github::rest("GET", &path, None, SOURCE).await {
                        Ok(issue) => {
                            // The queue's item for it, as autopilot words
                            // an issue (`Issue::text`).
                            let title = issue
                                .get("title")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            let text = format!("{reference}: {}", crate::worktree::one_line(title));
                            kept.push((repo, number, text));
                        }
                        Err(e)
                            if crate::net::kind_of(&e)
                                == Some(&crate::net::ErrorKind::NotFound) =>
                        {
                            skipped.push(format!("skipped {reference}: GitHub has no such issue"));
                        }
                        Err(e) => return Err(e),
                    }
                }
                anyhow::Ok((kept, skipped))
            }
        };
        let (next, more) = looked_up(&next).await?;
        skipped.extend(more);
        let (left, more) = looked_up(&left).await?;
        skipped.extend(more);
        anyhow::Ok((next, left, skipped))
    });
    let (next, left, skipped) = match looked {
        Ok(looked) => looked,
        Err(e) => return fail(NAME, &format!("{e:#}; no label changed")),
    };
    for line in &skipped {
        println!("{line}");
    }
    let mut taken_out = Vec::new();
    let mut first = Vec::new();
    let written = util::block_on(async {
        for (repo, number, _) in &left {
            set_priority(repo, *number, LOWEST).await?;
            taken_out.push(format!("{repo}#{number}"));
        }
        for (repo, number, text) in &next {
            set_priority(repo, *number, HIGHEST).await?;
            first.push(text.clone());
        }
        anyhow::Ok(())
    });
    if first.is_empty() && taken_out.is_empty() {
        return match written {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => fail(NAME, &format!("{e:#}; no label changed")),
        };
    }
    // Told at once, so autopilot never opens from half of the new order.
    let (put, out) = (first.len(), taken_out.len());
    let mut asked = vec!["coordinator-ordered".to_string()];
    asked.extend(first);
    if !taken_out.is_empty() {
        asked.push("--left".into());
        asked.extend(taken_out);
    }
    let asked: Vec<&str> = asked.iter().map(String::as_str).collect();
    let told = socket::ask(&asked);
    println!("{put} issue(s) first in autopilot's queue, in order; {out} taken out");
    let told = match told {
        Ok((0, _, _)) => Ok(()),
        Ok((_, _, err)) => Err(err.trim().to_string()),
        Err(e) => Err(format!("{e:#}")),
    };
    match (written, told) {
        (Ok(()), Ok(())) => ExitCode::SUCCESS,
        (Err(e), _) => fail(NAME, &format!("{e:#}; only the issues above were done")),
        (_, Err(e)) => fail(NAME, &format!("labeled, but autopilot wasn't told: {e}")),
    }
}

/// Refuses a repository you can't push to, so nothing changes on someone
/// else's.
async fn pushable(repo: &Repo) -> anyhow::Result<()> {
    let about = github::rest("GET", &format!("/repos/{repo}"), None, SOURCE).await?;
    if about.pointer("/permissions/push") != Some(&Value::Bool(true)) {
        return Err(NotPushable(repo.to_string()).into());
    }
    Ok(())
}

/// A repository you can't push to.
#[derive(Debug)]
struct NotPushable(String);

impl std::fmt::Display for NotPushable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} isn't a repository you can push to", self.0)
    }
}

impl std::error::Error for NotPushable {}

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
        let origin = crate::worktree::git(&clone, &["config", "--get", "remote.origin.url"])
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
    fn order_takes_next_in_its_order_and_what_left_it() {
        let (next, left) = order_args(&args(&["o/r#3", "o/r#1", "--left", "o/r#2"])).unwrap();
        let named = |v: &Issues| {
            v.iter()
                .map(|(r, n)| format!("{r}#{n}"))
                .collect::<Vec<_>>()
        };
        assert_eq!(named(&next), ["o/r#3", "o/r#1"]);
        assert_eq!(named(&left), ["o/r#2"]);
        assert!(order_args(&args(&["--left", "o/r#2"])).is_some());
        assert!(order_args(&args(&[])).is_none());
        assert!(
            order_args(&args(&["o/r#3", "1"])).is_none(),
            "a priority is no issue"
        );
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
