//! The branch's pull request as the PR view shows it (tmux/pr-view.sh's
//! `show`, pr-view.js's `prepare`): its title and number, a line saying
//! whose branch goes into which, and its description, as GitHub's "Files
//! changed" page heads it.
//!
//! The copy last read for the branch is shown at once, from
//! `~/.cache/switchboard/pr-view/` (one file per repository and branch, as
//! pr-view.sh named it), and the PR is read again in the background; when it
//! has changed, the pager lays it out again where it was. The PR is found as
//! the tab names find one (`github::branch_pr`: in origin, or in a fork's
//! clone the project its `upstream` remote names), and its description
//! read with one more GraphQL query, through GitHub's API rather than `gh`.
//! When there is none to show, it says plainly why, as gh-open.sh's box
//! did: not a repository, no remote on GitHub, no token, no PR yet.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Value, json};

use super::pager::{self, Content};
use crate::integrations::github;
use crate::{paths, util};

/// What asked, for the request audit.
const SOURCE: &str = "M-v PR view";
/// Where a diagram that can't be drawn here is shown properly.
pub const PLACE: &str = "M-r on GitHub";

const QUERY: &str = "query($owner: String!, $name: String!, $number: Int!) { repository(owner: $owner, name: $name) { \
                     pullRequest(number: $number) { number title url state isDraft author { login } \
                     baseRefName headRefName body } } }";

/// The PR as Markdown: pr-view.js's header over its description.
pub fn markdown(pr: &Value) -> String {
    let text = |key: &str| pr.get(key).and_then(Value::as_str).unwrap_or("");
    let state = if pr.get("isDraft").and_then(Value::as_bool) == Some(true) {
        "Draft".to_string()
    } else {
        text("state").to_string()
    };
    let author = pr
        .pointer("/author/login")
        .and_then(Value::as_str)
        .unwrap_or("someone");
    let number = pr.get("number").map(util::js_string).unwrap_or_default();
    format!(
        "# {} #{number}\n\n_{state} · {author} wants to merge `{}` into `{}`_\n\n---\n\n{}",
        text("title"),
        text("headRefName"),
        text("baseRefName"),
        text("body")
    )
}

/// The file the last copy of a checkout's PR is kept in: its repository's
/// directory and branch, each `/` a `%`, as pr-view.sh named it.
pub fn cache_file(dir: &Path) -> Option<PathBuf> {
    let place = crate::tabs::git::locate(dir)?;
    let key = format!("{}:{}", place.git_dir.display(), place.branch).replace('/', "%");
    Some(
        paths::cache_dir()
            .join("pr-view")
            .join(format!("{key}.json")),
    )
}

/// A PR by its number, with the fields `gh pr view --json` gave.
pub async fn by_number(repo: &github::Repo, number: u64, source: &str) -> anyhow::Result<Value> {
    let data = github::graphql(
        QUERY,
        json!({ "owner": repo.owner, "name": repo.name, "number": number }),
        source,
    )
    .await?;
    data.pointer("/repository/pullRequest")
        .filter(|pr| pr.is_object())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("GitHub has no PR #{number} in {repo}."))
}

/// The PR for the branch checked out in `dir`, or why there is none, in
/// words for the person.
pub async fn for_dir(dir: &Path) -> Result<Value, String> {
    let found = github::branch_pr(dir, SOURCE)
        .await
        .map_err(|e| e.to_string())?;
    let Some((repo, pr)) = found else {
        let branch = crate::tabs::git::locate(dir)
            .map(|p| p.branch)
            .unwrap_or_else(|| "no branch".into());
        return Err(format!(
            "No PR for this branch ({branch}). Push it and open one, then M-v again."
        ));
    };
    by_number(&repo, pr.number, SOURCE)
        .await
        .map_err(|e| e.to_string())
}

/// The PR view: the last copy at once, then the PR read again.
pub fn main(dir: PathBuf) -> ExitCode {
    let cache = cache_file(&dir);
    let cached = cache.as_ref().and_then(util::read_json);
    let initial = match &cached {
        Some(pr) => Content::Markdown {
            text: markdown(pr),
            place: PLACE.into(),
        },
        None => Content::Loading("Reading the PR...".into()),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        match util::block_on(for_dir(&dir)) {
            Ok(pr) => {
                if cached.as_ref() == Some(&pr) {
                    return;
                }
                if let Some(file) = &cache {
                    if let Some(parent) = file.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = util::write_atomic(file, util::json_compact(&pr));
                }
                let _ = tx.send(Content::Markdown {
                    text: markdown(&pr),
                    place: PLACE.into(),
                });
            }
            // With a copy shown, it stays; only with nothing to show is
            // the reason the view.
            Err(why) if cached.is_none() => {
                let _ = tx.send(Content::Failed(why));
            }
            Err(_) => {}
        }
    });
    pager::run(initial, Some(rx), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_says_whose_branch_goes_where() {
        let pr = json!({
            "number": 12, "title": "Fix the login", "url": "https://github.com/acme/w/pull/12",
            "state": "OPEN", "isDraft": false, "author": { "login": "jo" },
            "baseRefName": "main", "headRefName": "eng-42-fix", "body": "It works now."
        });
        assert_eq!(
            markdown(&pr),
            "# Fix the login #12\n\n_OPEN · jo wants to merge `eng-42-fix` into `main`_\n\n---\n\nIt works now."
        );
        let draft = json!({ "number": 3, "title": "T", "isDraft": true, "state": "OPEN", "author": null, "body": null });
        assert!(markdown(&draft).starts_with("# T #3\n\n_Draft · someone wants"));
    }
}
