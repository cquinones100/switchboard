//! GitHub issues as autopilot's work, beside or instead of the note's
//! checklist (the mode's `sources`): the repository's open issues assigned
//! to you and carrying the mode's label (`issueLabel`, `autopilot` by
//! default), each taken only when you opened it or the label was added by
//! you or someone with write access to the repository, so nobody outside
//! it can queue work for your tabs by opening an issue. No tmux-version
//! file is behind it.
//!
//! An issue is ready as a planned note item is: its body is its plan. It
//! joins the queue as an item of its own (`item_of`), whose first line is
//! `<owner>/<repo>#<number>: <title>` and whose line is 0, a line no note
//! item has, which is how the rest tells one apart (`issue_of`); with both
//! sources the note's items come first, then the issues oldest first.
//! `wtn --issue <number>` (src/cmd/worktree_from_issue.rs) opens a tab on
//! one, on a branch `issue/<number>-<slug>` recorded as the issue's
//! (`branch.<branch>.wtnIssue`), so an issue with a worktree, or one wtn
//! started in the last half hour (its claims file, under `github:<repo>`),
//! isn't offered again.
//!
//! The issues are read through `integrations::github` (so the `github`
//! setting stops them and the request audit counts them), off the server's
//! task, by the same look that reads the note, and kept in
//! `autopilot-issues.json` in the state directory for `FRESH_MS`, so the
//! checks every few seconds ask GitHub at most every two minutes; a failed
//! read is kept as long, with why. Once a tab on an issue lands, the server
//! comments on it with the commit and closes it, and once one finds its
//! issue a duplicate, comments that it is one (never what the tab named,
//! which may be a note's words) and leaves it open,
//! each asked first unless the tab lands without asking (src/autopilot/serve.rs).

use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Result, anyhow};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cmd::worktree_from_note::Item;
use crate::integrations::github::{self, Repo};
use crate::{util, worktree};

/// What the request audit names as asking.
pub const SOURCE: &str = "autopilot: GitHub issues";

/// How long a read of the issues is kept before GitHub is asked again.
pub const FRESH_MS: i64 = 2 * 60 * 1000;

/// The issues a page of the search holds, and the most pages one read
/// takes, so the issues past the first page (some left out as claimed,
/// skipped or not yours) are reached too.
const PAGE: usize = 100;
const PAGES: usize = 5;

/// The file the last read is kept in, in the state directory.
pub const FILE: &str = "autopilot-issues.json";

/// One issue autopilot may take.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    /// `owner/name`.
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub url: String,
}

impl Issue {
    /// Its first line as the queue holds it: `<owner>/<repo>#<number>:
    /// <title>`, the title on one line.
    pub fn text(&self) -> String {
        format!(
            "{}#{}: {}",
            self.repo,
            self.number,
            worktree::one_line(&self.title)
        )
    }

    /// `<owner>/<repo>#<number>`, as a tab and a branch record it.
    pub fn reference(&self) -> String {
        format!("{}#{}", self.repo, self.number)
    }
}

/// The queue's item for an issue: line 0, which no note's item has.
pub fn item_of(issue: &Issue) -> Item {
    Item {
        line: 0,
        text: issue.text(),
        needs_choice: false,
        after: Vec::new(),
        waits_on: None,
    }
}

// `<owner>/<repo>#<number>`, at the start of an item's first line or alone.
static REFERENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z0-9._-]+/[A-Za-z0-9._-]+)#(\d+)").unwrap());

/// The repository and number of an issue's item, by its first line; `None`
/// for a note's item (any with a line of its own).
pub fn issue_of(item: &Item) -> Option<(String, u64)> {
    if item.line != 0 {
        return None;
    }
    let m = REFERENCE.captures(&item.text)?;
    let rest = &item.text[m[0].len()..];
    if !rest.starts_with(": ") {
        return None;
    }
    Some((m[1].to_string(), m[2].parse().ok()?))
}

/// The repository and number `<owner>/<repo>#<number>` names, alone.
pub fn parse_reference(text: &str) -> Option<(Repo, u64)> {
    let m = REFERENCE.captures(text.trim())?;
    if m[0].len() != text.trim().len() {
        return None;
    }
    let repo = github::parse_name_with_owner(&m[1])?;
    Some((repo, m[2].parse().ok()?))
}

/// An issue as GitHub's search answered it, before it is judged.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub issue: Issue,
    /// Who opened it.
    pub author: String,
    /// Who last added the label, as its timeline says.
    pub labeler: Option<String>,
}

/// The issues in a search's answer (`QUERY`'s data), oldest first as asked,
/// and who is asking (`viewer`).
pub fn read_answer(repo: &Repo, data: &Value, label: &str) -> (String, Vec<Found>) {
    let viewer = data
        .pointer("/viewer/login")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let text = |v: &Value, at: &str| {
        v.pointer(at)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let found = data
        .pointer("/search/nodes")
        .and_then(Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|n| {
                    let number = n.get("number").and_then(Value::as_u64)?;
                    let labeler = n
                        .pointer("/timelineItems/nodes")
                        .and_then(Value::as_array)
                        .and_then(|events| {
                            events
                                .iter()
                                .rev()
                                .find(|e| {
                                    e.pointer("/label/name")
                                        .and_then(Value::as_str)
                                        .is_some_and(|l| l.eq_ignore_ascii_case(label))
                                })
                                .and_then(|e| e.pointer("/actor/login"))
                                .and_then(Value::as_str)
                                .map(str::to_string)
                        });
                    Some(Found {
                        issue: Issue {
                            repo: repo.to_string(),
                            number,
                            title: text(n, "/title"),
                            body: text(n, "/body"),
                            url: text(n, "/url"),
                        },
                        author: text(n, "/author/login"),
                        labeler,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    (viewer, found)
}

/// Whether an issue may be taken: you opened it, or the label was added by
/// you or by one of `writers` (those with write access); never one whose
/// labeler can't be told.
pub fn takeable(found: &Found, viewer: &str, writers: &HashSet<String>) -> bool {
    let same = |who: &str| !who.is_empty() && who.eq_ignore_ascii_case(viewer);
    if same(&found.author) {
        return true;
    }
    found
        .labeler
        .as_deref()
        .is_some_and(|l| same(l) || writers.contains(&l.to_lowercase()))
}

/// The labelers whose access must be asked about: those of issues neither
/// opened nor labeled by `viewer`, each once, lowercase.
fn to_ask(found: &[Found], viewer: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for f in found {
        let Some(l) = &f.labeler else { continue };
        let l = l.to_lowercase();
        if f.author.eq_ignore_ascii_case(viewer) || l == viewer.to_lowercase() || out.contains(&l) {
            continue;
        }
        // A login as GitHub allows them, which keeps anything else out of
        // the path; a bot's (`name[bot]`) is never asked about, so never
        // taken.
        if l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            out.push(l);
        }
    }
    out
}

const QUERY: &str = "query($q: String!, $first: Int!, $after: String) {
  viewer { login }
  search(query: $q, type: ISSUE, first: $first, after: $after) {
    pageInfo { hasNextPage endCursor }
    nodes {
      ... on Issue {
        number title body url
        author { login }
        timelineItems(itemTypes: [LABELED_EVENT], last: 100) {
          nodes { ... on LabeledEvent { label { name } actor { login } } }
        }
      }
    }
  }
}";

/// The search that finds them: the repository's open issues assigned to
/// whoever asks and labeled `label`, oldest first.
pub fn search_of(repo: &Repo, label: &str) -> String {
    format!("repo:{repo} is:issue is:open assignee:@me label:\"{label}\" sort:created-asc")
}

/// Whether a label can be searched for as typed.
pub fn label_ok(label: &str) -> bool {
    let label = label.trim();
    !label.is_empty() && !label.contains(['"', '\\', '\n'])
}

/// The issues of `repo` autopilot may take now, asked of GitHub.
pub async fn fetch(repo: &Repo, label: &str) -> Result<Vec<Issue>> {
    if !label_ok(label) {
        return Err(anyhow!("the issue label \"{label}\" can't be searched for"));
    }
    let variables = json!({ "q": search_of(repo, label), "first": PAGE });
    let mut data = github::graphql(QUERY, variables.clone(), SOURCE).await?;
    let first = data.pointer("/search").cloned().unwrap_or(Value::Null);
    let paged = github::rest_of(QUERY, &variables, "/search", &first, PAGES, SOURCE).await?;
    data["search"] = json!({ "nodes": paged.nodes });
    let (viewer, found) = read_answer(repo, &data, label);
    let mut writers = HashSet::new();
    for login in to_ask(&found, &viewer) {
        let path = format!("/repos/{repo}/collaborators/{login}/permission");
        // One GitHub doesn't know as a collaborator has no access; any other
        // failure says nothing of it, so the read fails and is tried again.
        let answer = match github::rest("GET", &path, None, SOURCE).await {
            Ok(answer) => answer,
            Err(e) if crate::net::kind_of(&e) == Some(&crate::net::ErrorKind::NotFound) => {
                continue;
            }
            Err(e) => return Err(e),
        };
        if matches!(
            answer.get("permission").and_then(Value::as_str),
            Some("admin" | "maintain" | "write")
        ) {
            writers.insert(login);
        }
    }
    Ok(found
        .into_iter()
        .filter(|f| takeable(f, &viewer, &writers))
        .map(|f| f.issue)
        .collect())
}

/// Runs a request to its end from a blocking thread of the server's
/// runtime, or a command with none.
pub fn wait_for<T>(future: impl std::future::Future<Output = T>) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle.block_on(future),
        Err(_) => util::block_on(future),
    }
}

/// The last read, as kept in `FILE`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Kept {
    /// When it was read (ms).
    at: i64,
    /// The main clone and label it was read for.
    dir: String,
    label: String,
    issues: Vec<Issue>,
    /// Why it failed; empty when it didn't.
    #[serde(default)]
    error: String,
}

/// The issues autopilot may take for the repository at `dir`, from the
/// last read while it is fresh, else asked of GitHub and kept; off the
/// server's task only, since it may ask GitHub.
pub fn issues(dir: &Path, label: &str, state: &Path, now_ms: i64) -> Result<Vec<Issue>, String> {
    // One read at a time, so the board's look and the opening look, which
    // can start together, share a read rather than each asking GitHub.
    static READING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _reading = READING.lock().unwrap_or_else(|e| e.into_inner());
    let file = state.join(FILE);
    let dir_text = dir.to_string_lossy().into_owned();
    let kept: Option<Kept> = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    if let Some(k) = kept.filter(|k| {
        k.dir == dir_text && k.label == label && (0..FRESH_MS).contains(&(now_ms - k.at))
    }) {
        return if k.error.is_empty() {
            Ok(k.issues)
        } else {
            Err(k.error)
        };
    }
    let read = wait_for(async {
        let repo = github::repo_of(dir).await?;
        fetch(&repo, label).await
    })
    .map_err(|e| e.to_string());
    let kept = Kept {
        at: now_ms,
        dir: dir_text,
        label: label.to_string(),
        issues: read.clone().unwrap_or_default(),
        error: read.clone().err().unwrap_or_default(),
    };
    if let Ok(text) = serde_json::to_string_pretty(&kept) {
        let _ = util::write_atomic(&file, text);
    }
    read
}

/// Whether issue `number` of `repo` still stands as the search found it:
/// open, assigned to whoever asks, and carrying `label`, asked of the issue
/// itself, since the search's index can lag a label taken off.
pub async fn still_stands(repo: &Repo, number: u64, label: &str) -> Result<bool> {
    let me = github::rest("GET", "/user", None, SOURCE).await?;
    let login = me.get("login").and_then(Value::as_str).unwrap_or_default();
    let issue = github::rest(
        "GET",
        &format!("/repos/{repo}/issues/{number}"),
        None,
        SOURCE,
    )
    .await?;
    let open = issue.get("state").and_then(Value::as_str) == Some("open");
    let names = |key: &str, field: &str| -> Vec<String> {
        issue
            .get(key)
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|v| v.get(field).and_then(Value::as_str))
                    .map(str::to_lowercase)
                    .collect()
            })
            .unwrap_or_default()
    };
    Ok(open
        && !login.is_empty()
        && names("assignees", "login").contains(&login.to_lowercase())
        && names("labels", "name").contains(&label.to_lowercase()))
}

/// The read kept forgotten, so the next look asks GitHub again: once an
/// issue's tab lands or is skipped.
pub fn forget(state: &Path) {
    let _ = std::fs::remove_file(state.join(FILE));
}

/// What the claims file says of an issue wtn started: its note is
/// `github:<owner>/<repo>`, its text `#<number>`.
pub fn claim_note(repo: &str) -> String {
    format!("github:{repo}")
}

/// The issues no worktree is working on, nor wtn started in the last half
/// hour, nor skipped (`skip`, by their items' first lines). `dir` is any
/// directory of the repository, `claims` wtn's claims file's text.
pub fn unclaimed(
    dir: &Path,
    issues: Vec<Issue>,
    claims: &str,
    skip: &HashSet<String>,
    now: i64,
) -> Vec<Issue> {
    let checked_out: HashSet<String> = worktree::worktrees(dir)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|w| w.branch)
        .collect();
    let config = worktree::git_at(dir, &["config", "--get-regexp", r"^branch\..*\.wtnissue$"])
        .unwrap_or_default();
    let worked: HashSet<String> = config
        .lines()
        .filter_map(|l| l.split_once(' '))
        .filter_map(|(key, value)| {
            let branch = key.strip_prefix("branch.")?.strip_suffix(".wtnissue")?;
            checked_out
                .contains(branch)
                .then(|| value.trim().to_lowercase())
        })
        .collect();
    issues
        .into_iter()
        .filter(|i| {
            let lately = crate::cmd::worktree_from_note::claimed_lately(
                claims,
                None,
                &claim_note(&i.repo),
                now,
            );
            // Skipped by its reference, whatever its title says now.
            let skipped_as = format!("{}: ", i.reference());
            !worked.contains(&i.reference().to_lowercase())
                && !lately.contains(&format!("#{}", i.number))
                && !skip.iter().any(|t| t.starts_with(&skipped_as))
        })
        .collect()
}

/// The comment left on an issue its tab landed.
pub fn landed_comment(hash: &str) -> String {
    if hash.is_empty() {
        "Landed by Switchboard's autopilot.".to_string()
    } else {
        format!("Landed in {hash} by Switchboard's autopilot.")
    }
}

/// The comment left on an issue its tab found a duplicate: never what the
/// tab named, which may be a note's words, and nothing in a note leaves the
/// computer.
pub fn duplicate_comment() -> String {
    "Switchboard's autopilot left this issue open: it found the same work already started or done elsewhere.".to_string()
}

/// Comments on an issue, and closes it as done when `close`; with
/// `landed`, only once that commit is on the repository's default branch
/// on GitHub, so a landing that never reached it closes nothing.
pub async fn write_back(
    repo: &Repo,
    number: u64,
    comment: &str,
    close: bool,
    landed: Option<&str>,
) -> Result<()> {
    if let Some(hash) = landed {
        if hash.is_empty() || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(anyhow!("\"{hash}\" isn't a commit"));
        }
        // On the default branch, not only somewhere on GitHub (a review
        // branch), since that is where a landing that closes an issue goes.
        let default = github::default_branch(repo, SOURCE).await?;
        let compare = github::rest(
            "GET",
            &format!("/repos/{repo}/compare/{default}...{hash}"),
            None,
            SOURCE,
        )
        .await
        .map_err(|e| anyhow!("{repo} has no commit {hash} on GitHub: {e}"))?;
        if !matches!(
            compare.get("status").and_then(Value::as_str),
            Some("identical" | "behind")
        ) {
            return Err(anyhow!("{hash} isn't on {repo}'s {default} yet"));
        }
    }
    github::rest(
        "POST",
        &format!("/repos/{repo}/issues/{number}/comments"),
        Some(&json!({ "body": comment })),
        SOURCE,
    )
    .await?;
    if close {
        github::rest(
            "PATCH",
            &format!("/repos/{repo}/issues/{number}"),
            Some(&json!({ "state": "closed", "state_reason": "completed" })),
            SOURCE,
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> Repo {
        Repo {
            owner: "acme".into(),
            name: "widgets".into(),
        }
    }

    fn answer() -> Value {
        json!({
            "viewer": { "login": "Me" },
            "search": { "nodes": [
                { "number": 3, "title": "Mine", "body": "b3", "url": "u3",
                  "author": { "login": "me" }, "timelineItems": { "nodes": [] } },
                { "number": 4, "title": "Labeled by a writer", "body": "", "url": "u4",
                  "author": { "login": "stranger" }, "timelineItems": { "nodes": [
                      { "label": { "name": "autopilot" }, "actor": { "login": "stranger" } },
                      { "label": { "name": "bug" }, "actor": { "login": "stranger" } },
                      { "label": { "name": "Autopilot" }, "actor": { "login": "Writer" } }
                  ] } },
                { "number": 5, "title": "Outside", "body": "", "url": "u5",
                  "author": { "login": "stranger" }, "timelineItems": { "nodes": [
                      { "label": { "name": "autopilot" }, "actor": { "login": "stranger" } }
                  ] } },
                { "number": 6, "title": "Labeled by me", "body": "", "url": "u6",
                  "author": { "login": "stranger" }, "timelineItems": { "nodes": [
                      { "label": { "name": "autopilot" }, "actor": { "login": "me" } }
                  ] } },
                {}
            ] }
        })
    }

    #[test]
    fn only_yours_or_labeled_by_a_writer_are_taken() {
        let (viewer, found) = read_answer(&repo(), &answer(), "autopilot");
        assert_eq!(viewer, "Me");
        assert_eq!(found.len(), 4);
        assert_eq!(found[1].labeler.as_deref(), Some("Writer"));
        assert_eq!(to_ask(&found, &viewer), vec!["writer", "stranger"]);
        let writers: HashSet<String> = ["writer".to_string()].into();
        let taken: Vec<u64> = found
            .iter()
            .filter(|f| takeable(f, &viewer, &writers))
            .map(|f| f.issue.number)
            .collect();
        assert_eq!(taken, vec![3, 4, 6]);
    }

    #[test]
    fn an_issues_item_is_told_from_a_notes_by_its_line() {
        let issue = Issue {
            repo: "acme/widgets".into(),
            number: 12,
            title: "Fix the\nthing".into(),
            body: String::new(),
            url: String::new(),
        };
        let item = item_of(&issue);
        assert_eq!(item.text, "acme/widgets#12: Fix the thing");
        assert_eq!(issue_of(&item), Some(("acme/widgets".into(), 12)));
        let note_item = Item { line: 4, ..item };
        assert_eq!(issue_of(&note_item), None);
        let (r, n) = parse_reference("acme/widgets#12").unwrap();
        assert_eq!((r.to_string().as_str(), n), ("acme/widgets", 12));
        assert!(parse_reference("acme/widgets#12 and more").is_none());
    }

    #[test]
    fn the_search_asks_for_your_open_labeled_issues_oldest_first() {
        assert_eq!(
            search_of(&repo(), "autopilot"),
            "repo:acme/widgets is:issue is:open assignee:@me label:\"autopilot\" sort:created-asc"
        );
        assert!(!label_ok("a\"b"));
        assert!(!label_ok(" "));
    }

    #[test]
    fn an_issue_with_a_worktree_or_claimed_lately_or_skipped_is_left_out() {
        let dir = std::env::temp_dir().join(format!("sb-issues-{}", std::process::id()));
        let issue = |n: u64| Issue {
            repo: "acme/widgets".into(),
            number: n,
            title: format!("t{n}"),
            body: String::new(),
            url: String::new(),
        };
        let now = 1_000_000;
        let claims = format!("{}\tgithub:acme/widgets\t#2\n", now - 10);
        let skip: HashSet<String> = [issue(3).text()].into();
        let left = unclaimed(
            &dir,
            vec![issue(1), issue(2), issue(3)],
            &claims,
            &skip,
            now,
        );
        assert_eq!(left, vec![issue(1)]);
    }
}
