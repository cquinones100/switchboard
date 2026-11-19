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
//! On GitHub too, so a second computer signed in as you leaves it alone:
//! wtn, in a tab autopilot opened, claims the issue before it makes the
//! worktree (`claim`), labeling it `autopilot: working` (`WORKING`, made
//! on the repository the first time) and commenting with this
//! computer's claim ID and the branch. The ID is six random hex digits made
//! once and kept in `autopilot-claim.json` among your files that stay on
//! this computer (`CLAIM_FILE`), so nothing about the computer is in it. An
//! issue with the label whose newest claim (a comment of yours) names
//! another ID is passed over (`unclaimed`) and refused by wtn, unless the
//! claim is older than six hours with nothing pushed on its branch
//! (`STALE_SECS`), which wtn then releases before claiming it. The server
//! releases this computer's claim (`release`), taking the label off, once
//! autopilot has closed the issue its tab landed (the landing's comment
//! names the claim), or as the tab closes before landing (saying so in a
//! comment with the ID); a landing whose issue stays open keeps its claim,
//! so no other computer takes the landed work again.
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

/// The label an issue carries while a computer's autopilot works on it.
pub const WORKING: &str = "autopilot: working";

/// The file this computer's claim ID is kept in, among your files that stay
/// on it (`files::LOCAL`).
pub const CLAIM_FILE: &str = "autopilot-claim.json";

/// How long a claim holds with nothing pushed on its branch.
pub const STALE_SECS: i64 = 6 * 60 * 60;

/// The comments read for an issue's newest claim.
const COMMENTS: usize = 50;

/// One issue autopilot may take.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    /// `owner/name`.
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub url: String,
    /// The claim it carries on GitHub, while it is labeled `WORKING`.
    #[serde(default)]
    pub claim: Option<Claim>,
}

/// A computer's claim on an issue, from the newest claim comment of yours
/// on it while it carries the `WORKING` label.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    /// The claiming computer's ID.
    pub id: String,
    /// The branch it named.
    pub branch: String,
    /// When it was made, in seconds.
    pub at: i64,
    /// Older than `STALE_SECS` with nothing pushed on its branch, so free
    /// to take.
    #[serde(default)]
    pub stale: bool,
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
                            claim: claim_of(n, &viewer),
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
        // (an Enterprise Managed User's carries `_<shortcode>`)
        if l.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
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
        labels(first: 50) { nodes { name } }
        comments(last: 50) { nodes { body createdAt author { login } } }
      }
    }
  }
}";

// One issue's claim, asked of the issue itself.
const CLAIM_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!, $last: Int!) {
  viewer { login }
  repository(owner: $owner, name: $name) {
    issue(number: $number) {
      labels(first: 50) { nodes { name } }
      comments(last: $last) { nodes { body createdAt author { login } } }
    }
  }
}";

// A claim comment: `claim_comment`'s words.
static CLAIM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^Autopilot is working on this \(claim ([0-9a-f]{6}), branch ([A-Za-z0-9._/-]+)\)")
        .unwrap()
});

// A release comment: `release_comment`'s words.
static RELEASE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^Autopilot stopped working on this \(claim [0-9a-f]{6}\)").unwrap()
});

/// The claim an issue's node carries (its `labels` and `comments`): none
/// unless it is labeled `WORKING`, else the newest claim comment `viewer`
/// wrote, since only you claim for your computers; none when it has none.
pub fn claim_of(node: &Value, viewer: &str) -> Option<Claim> {
    let labeled = node
        .pointer("/labels/nodes")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|l| l.get("name").and_then(Value::as_str))
        .any(|l| l.eq_ignore_ascii_case(WORKING));
    if !labeled || viewer.is_empty() {
        return None;
    }
    node.pointer("/comments/nodes")
        .and_then(Value::as_array)?
        .iter()
        .rev()
        .filter(|c| {
            c.pointer("/author/login")
                .and_then(Value::as_str)
                .is_some_and(|a| a.eq_ignore_ascii_case(viewer))
        })
        // The newest claim or release decides: a release after a claim ends
        // it, whatever becomes of the label after.
        .find_map(|c| {
            let body = c.get("body").and_then(Value::as_str)?.trim();
            if RELEASE.is_match(body) {
                return Some(None);
            }
            let m = CLAIM.captures(body)?;
            let at = c
                .get("createdAt")
                .and_then(Value::as_str)
                .and_then(|d| d.parse::<jiff::Timestamp>().ok())?
                .as_second();
            Some(Some(Claim {
                id: m[1].to_string(),
                branch: m[2].to_string(),
                at,
                stale: false,
            }))
        })
        .flatten()
}

/// The comment that claims an issue for computer `id`, working on `branch`.
pub fn claim_comment(id: &str, branch: &str) -> String {
    format!("Autopilot is working on this (claim {id}, branch {branch}).")
}

/// The comment that releases claim `id`, saying why.
pub fn release_comment(id: &str, why: &str) -> String {
    format!("Autopilot stopped working on this (claim {id}): {why}.")
}

/// This computer's claim ID, kept in `CLAIM_FILE`, made the first time:
/// six random hex digits, nothing of the computer in them.
pub fn claim_id() -> Result<String> {
    if let Some(id) = own_claim_id()? {
        return Ok(id);
    }
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_i64(util::now_ms());
    hasher.write_u32(std::process::id());
    let id = format!("{:06x}", hasher.finish() & 0xff_ffff);
    let file = crate::files::file(CLAIM_FILE);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    util::write_atomic(&file, format!("{{\n  \"id\": \"{id}\"\n}}\n"))?;
    Ok(id)
}

/// This computer's claim ID as `CLAIM_FILE` has it, none before it is made
/// or once it is cleared.
pub fn own_claim_id() -> Result<Option<String>> {
    let file = crate::files::file(CLAIM_FILE);
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let unreadable = || {
        anyhow!(
            "{} has no claim ID of six hex digits (0-9, a-f) as \"id\"",
            crate::files::shown(CLAIM_FILE)
        )
    };
    let object = serde_json::from_str::<Value>(&text)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(unreadable)?;
    // Cleared in Settings (no `id`, or an empty one), it is made again as if
    // it never was; anything else is left for you to mend.
    let id = match object.get("id") {
        None => return Ok(None),
        Some(Value::String(id)) if id.trim().is_empty() => return Ok(None),
        Some(Value::String(id)) => Some(id.clone()),
        Some(_) => None,
    };
    let id = id
        .filter(|id| id.len() == 6 && id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')))
        .ok_or_else(unreadable)?;
    Ok(Some(id))
}

/// Whether `branch` of `repo` is on GitHub, which a claim's push puts it.
async fn branch_pushed(repo: &Repo, branch: &str) -> Result<bool> {
    if branch.is_empty()
        || !branch
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
    {
        return Ok(false);
    }
    match github::rest(
        "GET",
        &format!("/repos/{repo}/branches/{branch}"),
        None,
        SOURCE,
    )
    .await
    {
        Ok(_) => Ok(true),
        Err(e) if crate::net::kind_of(&e) == Some(&crate::net::ErrorKind::NotFound) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Whether a claim made at `at` is old enough to be stale, by `now`
/// (seconds), its branch still to be asked about.
pub fn old_claim(claim: &Claim, now: i64) -> bool {
    now - claim.at >= STALE_SECS
}

/// A claim marked stale when it is old and nothing was pushed on its branch.
async fn mark_stale(repo: &Repo, claim: &mut Claim, now: i64) -> Result<()> {
    if old_claim(claim, now) && !branch_pushed(repo, &claim.branch).await? {
        claim.stale = true;
    }
    Ok(())
}

/// Issue `number`'s claim as it stands now, asked of the issue itself.
pub async fn current_claim(repo: &Repo, number: u64) -> Result<Option<Claim>> {
    let data = github::graphql(
        CLAIM_QUERY,
        json!({ "owner": repo.owner, "name": repo.name, "number": number, "last": COMMENTS }),
        SOURCE,
    )
    .await?;
    let viewer = data
        .pointer("/viewer/login")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let issue = data
        .pointer("/repository/issue")
        .filter(|v| !v.is_null())
        .ok_or_else(|| anyhow!("{repo} has no issue {number}"))?;
    Ok(claim_of(issue, viewer))
}

/// Issue `number`'s claim as it stands now while it holds: none when it
/// has none or it is stale.
pub async fn live_claim(repo: &Repo, number: u64, now: i64) -> Result<Option<Claim>> {
    let Some(mut c) = current_claim(repo, number).await? else {
        return Ok(None);
    };
    mark_stale(repo, &mut c, now).await?;
    Ok((!c.stale).then_some(c))
}

/// Claims issue `number` for this computer, `id`, as its tab starts on
/// `branch`: refused while another computer's claim holds; another's that
/// is stale released first; one of this computer's for the same branch
/// made again, its six hours starting with this tab. Then asked again, so of two computers claiming
/// at once only the newest claim's goes on.
pub async fn claim(repo: &Repo, number: u64, id: &str, branch: &str, now: i64) -> Result<()> {
    let held = |c: &Claim| {
        anyhow!(
            "{repo}#{number} is claimed by another computer's autopilot (claim {})",
            c.id
        )
    };
    let comment = |body: String| async move {
        github::rest(
            "POST",
            &format!("/repos/{repo}/issues/{number}/comments"),
            Some(&json!({ "body": body })),
            SOURCE,
        )
        .await
    };
    if let Some(mut c) = current_claim(repo, number).await? {
        // This computer's own is made again below, so the six hours start
        // with this tab and another computer doesn't find it stale.
        if c.id != id {
            mark_stale(repo, &mut c, now).await?;
            if !c.stale {
                return Err(held(&c));
            }
            comment(release_comment(
                &c.id,
                &format!("older than 6 hours with nothing pushed on {}", c.branch),
            ))
            .await?;
        }
    }
    ensure_label(repo).await?;
    github::rest(
        "POST",
        &format!("/repos/{repo}/issues/{number}/labels"),
        Some(&json!({ "labels": [WORKING] })),
        SOURCE,
    )
    .await?;
    comment(claim_comment(id, branch)).await?;
    // Started only on a claim GitHub shows as this computer's.
    match current_claim(repo, number).await? {
        Some(c) if c.id == id && c.branch == branch => Ok(()),
        Some(c) => Err(held(&c)),
        None => Err(anyhow!(
            "{repo}#{number} doesn't show this computer's claim yet"
        )),
    }
}

/// `WORKING` as a path segment.
fn label_path() -> String {
    WORKING.replace(':', "%3A").replace(' ', "%20")
}

/// The `WORKING` label made on `repo` when it has none yet; one another
/// computer made meanwhile is taken as made.
async fn ensure_label(repo: &Repo) -> Result<()> {
    let path = format!("/repos/{repo}/labels/{}", label_path());
    match github::rest("GET", &path, None, SOURCE).await {
        Ok(_) => return Ok(()),
        Err(e) if crate::net::kind_of(&e) == Some(&crate::net::ErrorKind::NotFound) => {}
        Err(e) => return Err(e),
    }
    let made = github::rest(
        "POST",
        &format!("/repos/{repo}/labels"),
        Some(&json!({
            "name": WORKING,
            "color": "fbca04",
            "description": "Switchboard's autopilot is working on this",
        })),
        SOURCE,
    )
    .await;
    if let Err(e) = made {
        github::rest("GET", &path, None, SOURCE)
            .await
            .map_err(|_| e)?;
    }
    Ok(())
}

/// Releases this computer's claim, `id`, on issue `number`: its label
/// taken off, and `why` said in a comment naming the claim (none when a
/// comment of its own says so, as a landing's does). Nothing while the
/// newest claim isn't this computer's. Whether it released one.
pub async fn release(repo: &Repo, number: u64, id: &str, why: Option<&str>) -> Result<bool> {
    if current_claim(repo, number)
        .await?
        .is_none_or(|c| c.id != id)
    {
        return Ok(false);
    }
    let label = label_path();
    match github::rest(
        "DELETE",
        &format!("/repos/{repo}/issues/{number}/labels/{label}"),
        None,
        SOURCE,
    )
    .await
    {
        Ok(_) => {}
        Err(e) if crate::net::kind_of(&e) == Some(&crate::net::ErrorKind::NotFound) => {}
        Err(e) => return Err(e),
    }
    if let Some(why) = why {
        github::rest(
            "POST",
            &format!("/repos/{repo}/issues/{number}/comments"),
            Some(&json!({ "body": release_comment(id, why) })),
            SOURCE,
        )
        .await?;
    }
    Ok(true)
}

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
    let mut taken: Vec<Issue> = found
        .into_iter()
        .filter(|f| takeable(f, &viewer, &writers))
        .map(|f| f.issue)
        .collect();
    let now = util::now_secs();
    for issue in &mut taken {
        // A branch that can't be asked about leaves its claim holding, and
        // the other issues read.
        if let Some(c) = issue.claim.as_mut()
            && let Err(e) = mark_stale(repo, c, now).await
        {
            eprintln!(
                "autopilot: couldn't tell whether {}'s claim is stale: {e}",
                issue.reference()
            );
        }
    }
    Ok(taken)
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

/// Issue `number` of `repo` as it stands now, asked of the issue itself,
/// since the search's index can lag a label taken off or a body edited: its
/// body while it is open, assigned to whoever asks and carrying `label`,
/// else none.
pub async fn still_stands(repo: &Repo, number: u64, label: &str) -> Result<Option<String>> {
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
    let stands = open
        && !login.is_empty()
        && names("assignees", "login").contains(&login.to_lowercase())
        && names("labels", "name").contains(&label.to_lowercase());
    Ok(stands.then(|| {
        issue
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }))
}

/// Issue `number` of `repo` while it is open, whoever it is assigned to
/// and however it is labeled, for an issue started by hand; `None` for one
/// closed, a pull request, or none of that number.
pub async fn open_issue(repo: &Repo, number: u64) -> Result<Option<Issue>> {
    let path = format!("/repos/{repo}/issues/{number}");
    let issue = match github::rest("GET", &path, None, "wtn: the issue").await {
        Ok(issue) => issue,
        Err(e) if crate::net::kind_of(&e) == Some(&crate::net::ErrorKind::NotFound) => {
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    let text = |key: &str| {
        issue
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let open = text("state") == "open" && issue.get("pull_request").is_none();
    Ok(open.then(|| Issue {
        repo: repo.to_string(),
        number,
        title: text("title"),
        body: text("body"),
        url: text("html_url"),
        claim: None,
    }))
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
/// hour, nor skipped (`skip`, by their items' first lines), nor claimed on
/// GitHub by another computer than `own` (this one's claim ID, empty
/// before it has one) while that claim holds. `dir` is any directory of
/// the repository, `claims` wtn's claims file's text.
pub fn unclaimed(
    dir: &Path,
    issues: Vec<Issue>,
    claims: &str,
    skip: &HashSet<String>,
    own: &str,
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
            let elsewhere = i.claim.as_ref().is_some_and(|c| !c.stale && c.id != own);
            !worked.contains(&i.reference().to_lowercase())
                && !elsewhere
                && !lately.contains(&format!("#{}", i.number))
                && !skip.iter().any(|t| t.starts_with(&skipped_as))
        })
        .collect()
}

/// The comment left on an issue its tab landed, naming the claim it
/// releases (`claim`, empty for none).
pub fn landed_comment(hash: &str, claim: &str) -> String {
    let landed = if hash.is_empty() {
        "Landed by Switchboard's autopilot".to_string()
    } else {
        format!("Landed in {hash} by Switchboard's autopilot")
    };
    if claim.is_empty() {
        format!("{landed}.")
    } else {
        format!("{landed} (claim {claim}).")
    }
}

/// The comment left on an issue its tab found a duplicate: never what the
/// tab named, which may be a note's words, and nothing in a note leaves the
/// computer.
pub fn duplicate_comment() -> String {
    "Switchboard's autopilot left this issue open: it found the same work already started or done elsewhere.".to_string()
}

/// That commit `hash` is a landing's: on `repo`'s default branch on
/// GitHub and made since its tab opened (`since`, seconds).
pub async fn landed_on_default(repo: &Repo, hash: &str, since: i64) -> Result<()> {
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
    // Made since the tab opened (`since`, seconds), so a commit that was
    // there before names no landing of its.
    let commit = github::rest(
        "GET",
        &format!("/repos/{repo}/commits/{hash}"),
        None,
        SOURCE,
    )
    .await?;
    let made = commit
        .pointer("/commit/committer/date")
        .and_then(Value::as_str)
        .and_then(|d| d.parse::<jiff::Timestamp>().ok())
        .map(|t| t.as_second());
    if made.is_none_or(|m| m < since) {
        return Err(anyhow!("{hash} was there before its tab opened"));
    }
    Ok(())
}

/// Comments on an issue, and closes it as done when `close`; with
/// `landed` (a commit and when its tab opened), only once that commit is on
/// the repository's default branch on GitHub and was made since, so a
/// landing that never reached it closes nothing. Whether the issue is
/// closed once it is done, so a claim is let go only from a closed issue.
pub async fn write_back(
    repo: &Repo,
    number: u64,
    comment: &str,
    close: bool,
    landed: Option<(&str, i64)>,
    still: impl Fn() -> bool,
) -> Result<bool> {
    if let Some((hash, since)) = landed {
        landed_on_default(repo, hash, since).await?;
    }
    // One closed meanwhile (by a maintainer, as not planned, say) keeps
    // what they decided: nothing is written on it.
    let now = github::rest(
        "GET",
        &format!("/repos/{repo}/issues/{number}"),
        None,
        SOURCE,
    )
    .await?;
    // Checked again just before each write, since asking GitHub takes a
    // while: `still` false (the source turned off meanwhile) writes nothing
    // more.
    let open = now.get("state").and_then(Value::as_str) == Some("open");
    if !open || !still() {
        return Ok(!open);
    }
    github::rest(
        "POST",
        &format!("/repos/{repo}/issues/{number}/comments"),
        Some(&json!({ "body": comment })),
        SOURCE,
    )
    .await?;
    if close && still() {
        github::rest(
            "PATCH",
            &format!("/repos/{repo}/issues/{number}"),
            Some(&json!({ "state": "closed", "state_reason": "completed" })),
            SOURCE,
        )
        .await?;
        return Ok(true);
    }
    Ok(false)
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
            claim: None,
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
            claim: None,
        };
        let claimed = |n: u64, id: &str, stale: bool| Issue {
            claim: Some(Claim {
                id: id.into(),
                branch: format!("issue/{n}-t"),
                at: 1,
                stale,
            }),
            ..issue(n)
        };
        let now = 1_000_000;
        let claims = format!("{}\tgithub:acme/widgets\t#2\n", now - 10);
        let skip: HashSet<String> = [issue(3).text()].into();
        let left = unclaimed(
            &dir,
            vec![
                issue(1),
                issue(2),
                issue(3),
                claimed(4, "bbbbbb", false),
                claimed(5, "aaaaaa", false),
                claimed(6, "bbbbbb", true),
            ],
            &claims,
            &skip,
            "aaaaaa",
            now,
        );
        let numbers: Vec<u64> = left.iter().map(|i| i.number).collect();
        assert_eq!(
            numbers,
            vec![1, 5, 6],
            "another computer's live claim passed over"
        );
    }

    #[test]
    fn a_claim_is_the_newest_claim_comment_of_yours_while_the_label_is_on() {
        let node = |labels: Value| {
            json!({
                "labels": { "nodes": labels },
                "comments": { "nodes": [
                    { "body": claim_comment("aaaaaa", "issue/1-a"), "createdAt": "2026-11-18T10:00:00Z", "author": { "login": "me" } },
                    { "body": claim_comment("bbbbbb", "issue/1-b"), "createdAt": "2026-11-18T11:00:00Z", "author": { "login": "Me" } },
                    { "body": claim_comment("cccccc", "issue/1-c"), "createdAt": "2026-11-18T12:00:00Z", "author": { "login": "stranger" } },
                    { "body": "Thanks!", "createdAt": "2026-11-18T13:00:00Z", "author": { "login": "me" } }
                ] }
            })
        };
        let claim = claim_of(&node(json!([{ "name": "Autopilot: Working" }])), "me").unwrap();
        assert_eq!(claim.id, "bbbbbb");
        assert_eq!(claim.branch, "issue/1-b");
        assert_eq!(
            claim.at,
            "2026-11-18T11:00:00Z"
                .parse::<jiff::Timestamp>()
                .unwrap()
                .as_second()
        );
        assert!(!old_claim(&claim, claim.at + STALE_SECS - 1));
        assert!(old_claim(&claim, claim.at + STALE_SECS));
        assert_eq!(
            claim_of(&node(json!([{ "name": "autopilot" }])), "me"),
            None
        );
        // Released after, it holds no more, the label put back or not.
        let mut released = node(json!([{ "name": "autopilot: working" }]));
        released["comments"]["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "body": release_comment("bbbbbb", "its tab closed before it landed"), "createdAt": "2026-11-18T14:00:00Z", "author": { "login": "me" } }));
        assert_eq!(claim_of(&released, "me"), None);
        // Nothing of the computer in what is written on GitHub.
        assert_eq!(
            claim_comment("a3f9c2", "issue/12-slug"),
            "Autopilot is working on this (claim a3f9c2, branch issue/12-slug)."
        );
        assert_eq!(
            release_comment("a3f9c2", "its tab closed before it landed"),
            "Autopilot stopped working on this (claim a3f9c2): its tab closed before it landed."
        );
        assert_eq!(
            landed_comment("abc1234", "a3f9c2"),
            "Landed in abc1234 by Switchboard's autopilot (claim a3f9c2)."
        );
        assert_eq!(landed_comment("", ""), "Landed by Switchboard's autopilot.");
    }
}
