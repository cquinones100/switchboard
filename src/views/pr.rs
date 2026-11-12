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
//!
//! With UX 2.0 on, the PR is laid out as board 13 draws it (`board`,
//! through `super::board`): the description and the conversation's comments
//! with the PR's checks, reviewers, open review threads and files beside
//! them, all from the same query.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Value, json};

use super::board::{self, Board, Part, Piece};
use super::pager::{self, Content};
use crate::integrations::github;
use crate::photos::{Service, Who};
use crate::ui::kit::{Mark, Tone};
use crate::{paths, settings, util};

/// What asked, for the request audit.
const SOURCE: &str = "M-v PR view";
/// Where a diagram that can't be drawn here is shown properly.
pub const PLACE: &str = "M-r on GitHub";

const QUERY: &str = "query($owner: String!, $name: String!, $number: Int!) { repository(owner: $owner, name: $name) { \
    pullRequest(number: $number) { number title url state isDraft createdAt author { login } \
    baseRefName headRefName body additions deletions changedFiles commits { totalCount } \
    last: commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100) { \
      checkRunCountsByState { state count } statusContextCountsByState { state count } nodes { __typename \
      ... on CheckRun { name status conclusion } ... on StatusContext { context state } } } } } } } \
    reviewRequests(first: 20) { nodes { requestedReviewer { ... on User { login } ... on Team { name } } } } \
    latestReviews(first: 20) { nodes { author { login } state } } \
    reviewThreads(first: 100) { pageInfo { hasNextPage endCursor } nodes { isResolved path line \
      comments(first: 1) { totalCount nodes { author { login } body } } \
      latest: comments(last: 1) { nodes { author { login } body } } } } \
    files(first: 100) { nodes { path additions deletions } } \
    comments(first: 100) { nodes { author { login } body createdAt } } } } }";

/// The PR's review threads after the first hundred, a page at a time, for
/// the panel to count the open ones among them all: GitHub can't be asked
/// for only the unresolved ones.
const THREADS_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!, $after: String) { \
    repository(owner: $owner, name: $name) { pullRequest(number: $number) { \
    reviewThreads(first: 100, after: $after) { pageInfo { hasNextPage endCursor } nodes { isResolved path line \
      comments(first: 1) { totalCount nodes { author { login } body } } \
      latest: comments(last: 1) { nodes { author { login } body } } } } } } }";
/// The most pages of review threads read, five hundred threads.
const THREAD_PAGES: usize = 5;

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

/// How many of a list the panel shows before "N more".
const FILES: usize = 8;
const THREADS: usize = 4;

fn str_at<'a>(v: &'a Value, path: &str) -> &'a str {
    v.pointer(path).and_then(Value::as_str).unwrap_or("")
}

fn nodes<'a>(v: &'a Value, path: &str) -> &'a [Value] {
    v.pointer(path)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn count(v: &Value, path: &str) -> u64 {
    v.pointer(path).and_then(Value::as_u64).unwrap_or(0)
}

/// A GitHub login's avatar.
fn avatar(login: &str) -> Piece {
    Piece::Avatar(Some(Who::new(Service::GitHub, login)), login.to_string())
}

/// What a GitHub login wrote, beside their Large avatar: their name and
/// `then` (when, or where) at its first row, `body` under them.
fn said(login: &str, then: String, body: &str) -> Part {
    Part::Said {
        who: Some(Who::new(Service::GitHub, login)),
        name: login.into(),
        head: vec![
            Piece::Name(login.into(), Tone::Text),
            Piece::Text(then, Tone::Dim),
        ],
        body: body.into(),
        indent: 0,
    }
}

/// A check's state, as the kit marks it.
fn check_mark(check: &Value) -> Mark {
    let state = match check.get("__typename").and_then(Value::as_str) {
        Some("StatusContext") => str_at(check, "/state"),
        _ if str_at(check, "/status") != "COMPLETED" => "PENDING",
        _ => str_at(check, "/conclusion"),
    };
    state_mark(state)
}

/// A check's state or conclusion as GitHub names it, as the kit marks it;
/// one not finished yet (queued, in progress, waiting) is pending.
fn state_mark(state: &str) -> Mark {
    match state {
        "SUCCESS" | "NEUTRAL" | "SKIPPED" => Mark::Passed,
        "FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE"
        | "STALE" => Mark::Failed,
        _ => Mark::Pending,
    }
}

/// The PR as board 13 draws it: its head, its description through
/// Markdown and its conversation's comments, with its checks, reviewers,
/// open review threads and files beside them.
pub fn board(pr: &Value, now: jiff::Timestamp) -> Board {
    let login = str_at(pr, "/author/login");
    let author = Some(login).filter(|a| !a.is_empty()).unwrap_or("someone");
    let (state, tone) = match (
        pr.get("isDraft").and_then(Value::as_bool),
        str_at(pr, "/state"),
    ) {
        (Some(true), _) => ("Draft", Tone::Dim),
        (_, "MERGED") => ("Merged", Tone::Landing),
        (_, "CLOSED") => ("Closed", Tone::Failed),
        _ => ("Open", Tone::Done),
    };
    let number = pr.get("number").map(util::js_string).unwrap_or_default();
    let commits = count(pr, "/commits/totalCount");
    let mut wants = Vec::new();
    if !login.is_empty() {
        wants.extend([avatar(login), Piece::Text(" ".into(), Tone::Text)]);
    }
    wants.extend([
        Piece::Text(
            format!(
                "{author} wants to merge {commits} commit{} into ",
                if commits == 1 { "" } else { "s" }
            ),
            Tone::Dim,
        ),
        Piece::Name(str_at(pr, "/baseRefName").into(), Tone::Text),
        Piece::Text(" from ".into(), Tone::Dim),
        // The branch without its owner, to fit beside the avatar.
        Piece::Name(
            str_at(pr, "/headRefName")
                .strip_prefix(&format!("{login}/"))
                .filter(|_| !login.is_empty())
                .unwrap_or(str_at(pr, "/headRefName"))
                .into(),
            Tone::Text,
        ),
        Piece::Text(" · ".into(), Tone::Dim),
        Piece::Text(format!("+{}", count(pr, "/additions")), Tone::Done),
        Piece::Text(format!(" −{}", count(pr, "/deletions")), Tone::Failed),
    ]);
    let mut main = vec![
        Part::Row(vec![
            Piece::Chip(state.into(), tone),
            Piece::Name(format!("#{number}"), Tone::Accent),
            Piece::Text(" ".into(), Tone::Text),
            Piece::Name(str_at(pr, "/title").into(), Tone::Text),
        ]),
        Part::Row(wants),
        Part::Rule,
        Part::Blank,
    ];
    if !login.is_empty() {
        let when = board::when(str_at(pr, "/createdAt"), now);
        main.push(said(
            login,
            format!(" opened this {when}").trim_end().into(),
            "",
        ));
    }
    let body = str_at(pr, "/body");
    main.push(Part::Text {
        markdown: if body.trim().is_empty() {
            "_No description._".into()
        } else {
            body.into()
        },
        indent: 0,
    });
    let comments = nodes(pr, "/comments/nodes");
    if !comments.is_empty() {
        main.extend([
            Part::Blank,
            Part::Label("Comments".into(), Some(comments.len().to_string())),
        ]);
        for c in comments {
            let who = str_at(c, "/author/login");
            main.extend([
                Part::Blank,
                said(
                    who,
                    format!("  {}", board::when(str_at(c, "/createdAt"), now)),
                    str_at(c, "/body"),
                ),
            ]);
        }
    }

    let mut panel = Vec::new();
    let checks = nodes(pr, "/last/nodes/0/commit/statusCheckRollup/contexts/nodes");
    if !checks.is_empty() {
        let mut marked: Vec<(Mark, &str)> = checks
            .iter()
            .map(|c| {
                let name = match str_at(c, "/name") {
                    "" => str_at(c, "/context"),
                    n => n,
                };
                (check_mark(c), name)
            })
            .collect();
        // What needs you first: the failures, then what is still running.
        marked.sort_by_key(|(m, _)| match m {
            Mark::Failed => 0,
            Mark::Pending => 1,
            _ => 2,
        });
        // The counts GitHub gives for every check, of which only the
        // first 100 come by name; those listed where the counts are missing.
        let rollup = "/last/nodes/0/commit/statusCheckRollup/contexts";
        let mut by_state: Vec<(&str, u64)> = Vec::new();
        for kind in ["checkRunCountsByState", "statusContextCountsByState"] {
            for n in nodes(pr, &format!("{rollup}/{kind}")) {
                by_state.push((str_at(n, "/state"), count(n, "/count")));
            }
        }
        let listed = |m: Mark| marked.iter().filter(|(k, _)| *k == m).count() as u64;
        let (total, passed, failed) = if by_state.is_empty() {
            (
                marked.len() as u64,
                listed(Mark::Passed),
                listed(Mark::Failed),
            )
        } else {
            let sum = |f: &dyn Fn(Mark) -> bool| {
                by_state
                    .iter()
                    .filter(|(s, _)| f(state_mark(s)))
                    .map(|(_, n)| n)
                    .sum::<u64>()
            };
            (
                sum(&|_| true),
                sum(&|m| m == Mark::Passed),
                sum(&|m| m == Mark::Failed),
            )
        };
        let unlisted_failures = failed.saturating_sub(listed(Mark::Failed));
        let unlisted = total.saturating_sub(marked.len() as u64);
        panel.push(Part::Label(
            "Checks".into(),
            Some(format!("{passed} of {total}")),
        ));
        panel.push(Part::Flow(
            marked
                .into_iter()
                .map(|(m, name)| vec![Piece::Mark(m), Piece::Text(format!(" {name}"), Tone::Text)])
                .collect(),
        ));
        if unlisted > 0 {
            let mut row = vec![Piece::Text(format!("{unlisted} more"), Tone::Dim)];
            if unlisted_failures > 0 {
                row.push(Piece::Text(
                    format!(", {unlisted_failures} failing"),
                    Tone::Failed,
                ));
            }
            panel.push(Part::Row(row));
        }
        panel.push(Part::Blank);
    }
    let mut reviewers: Vec<Part> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    let requested: Vec<&str> = nodes(pr, "/reviewRequests/nodes")
        .iter()
        .map(|r| str_at(r, "/requestedReviewer/login"))
        .filter(|l| !l.is_empty())
        .collect();
    for r in nodes(pr, "/latestReviews/nodes") {
        let who = str_at(r, "/author/login");
        let (what, tone) = match str_at(r, "/state") {
            "APPROVED" => ("approved", Tone::Done),
            "CHANGES_REQUESTED" => ("changes asked", Tone::Failed),
            "DISMISSED" => ("dismissed", Tone::Dim),
            _ => ("commented", Tone::Dim),
        };
        seen.push(who);
        let mut row = vec![avatar(who), Piece::Text(format!(" {who}  "), Tone::Text)];
        // Asked again since: the request open now, not the review before.
        if requested.contains(&who) {
            row.push(Piece::Text("requested again".into(), Tone::Needs));
        } else {
            row.push(Piece::Text(what.into(), tone));
        }
        reviewers.push(Part::Row(row));
    }
    for r in nodes(pr, "/reviewRequests/nodes") {
        let login = str_at(r, "/requestedReviewer/login");
        if !login.is_empty() && seen.contains(&login) {
            continue;
        }
        let (face, name) = match login {
            "" => (
                Piece::Avatar(None, str_at(r, "/requestedReviewer/name").into()),
                str_at(r, "/requestedReviewer/name"),
            ),
            l => (avatar(l), l),
        };
        reviewers.push(Part::Row(vec![
            face,
            Piece::Text(format!(" {name}  "), Tone::Text),
            Piece::Text("requested".into(), Tone::Needs),
        ]));
    }
    if !reviewers.is_empty() {
        panel.push(Part::Label("Reviewers".into(), None));
        panel.extend(reviewers);
        panel.push(Part::Blank);
    }
    let open: Vec<&Value> = nodes(pr, "/reviewThreads/nodes")
        .iter()
        .filter(|t| t.get("isResolved").and_then(Value::as_bool) != Some(true))
        .collect();
    if !open.is_empty() {
        panel.push(Part::Label(
            "Review comments".into(),
            Some(format!("{} open", open.len())),
        ));
        for t in open.iter().take(THREADS) {
            let first = nodes(t, "/comments/nodes")
                .first()
                .cloned()
                .unwrap_or(Value::Null);
            let who = str_at(&first, "/author/login");
            let place = match t.get("line").and_then(Value::as_u64) {
                Some(line) => format!("{}:{line}", str_at(t, "/path")),
                None => str_at(t, "/path").to_string(),
            };
            panel.extend([
                Part::Blank,
                said(who, format!("  {place}"), str_at(&first, "/body")),
            ]);
            let replies = count(t, "/comments/totalCount").saturating_sub(1);
            if replies > 0 {
                let last = nodes(t, "/latest/nodes")
                    .last()
                    .cloned()
                    .unwrap_or(Value::Null);
                let said = str_at(&last, "/body")
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("");
                panel.push(Part::Row(vec![Piece::Text(
                    format!(
                        "   {replies} repl{} · {}: {said}",
                        if replies == 1 { "y" } else { "ies" },
                        str_at(&last, "/author/login")
                    ),
                    Tone::Dim,
                )]));
            }
        }
        if open.len() > THREADS {
            panel.push(board::more(open.len() - THREADS));
        }
        panel.push(Part::Blank);
    }
    let files = nodes(pr, "/files/nodes");
    if !files.is_empty() {
        let total = count(pr, "/changedFiles").max(files.len() as u64) as usize;
        panel.push(Part::Label("Files".into(), Some(total.to_string())));
        for f in files.iter().take(FILES) {
            let mut lines = vec![Piece::Text(
                format!("+{}", count(f, "/additions")),
                Tone::Done,
            )];
            if count(f, "/deletions") > 0 {
                lines.push(Piece::Text(
                    format!(" −{}", count(f, "/deletions")),
                    Tone::Failed,
                ));
            }
            panel.push(Part::Ends(
                vec![Piece::Text(str_at(f, "/path").into(), Tone::Dim)],
                lines,
            ));
        }
        if total > FILES {
            panel.push(board::more(total - FILES));
        }
    }
    Board {
        main,
        panel,
        place: PLACE.into(),
        keys: [
            ("j/k", "scroll"),
            ("/", "search"),
            ("M-r", "on GitHub"),
            ("M-m", "ready?"),
            ("M-d", "its diff"),
            ("M-c", "back to Claude"),
        ]
        .map(|(k, w)| (k.to_string(), w.to_string()))
        .to_vec(),
    }
}

/// What the view shows of a PR: board 13 with UX 2.0 on, else its
/// Markdown, as pr-view.js laid it out.
fn content(pr: &Value) -> Content {
    if settings::is_on("ux2") {
        Content::Board(board(pr, jiff::Timestamp::now()))
    } else {
        Content::Markdown {
            text: markdown(pr),
            place: PLACE.into(),
        }
    }
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

/// A PR by its number, with the fields `gh pr view --json` gave, and every
/// review thread (up to `THREAD_PAGES` pages of them).
pub async fn by_number(repo: &github::Repo, number: u64, source: &str) -> anyhow::Result<Value> {
    let variables = json!({ "owner": repo.owner, "name": repo.name, "number": number });
    let data = github::graphql(QUERY, variables.clone(), source).await?;
    let mut pr = data
        .pointer("/repository/pullRequest")
        .filter(|pr| pr.is_object())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("GitHub has no PR #{number} in {repo}."))?;
    if let Some(first) = pr.get("reviewThreads").cloned() {
        let paged = github::rest_of(
            THREADS_QUERY,
            &variables,
            "/repository/pullRequest/reviewThreads",
            &first,
            THREAD_PAGES,
            source,
        )
        .await?;
        pr["reviewThreads"]["nodes"] = Value::Array(paged.nodes);
    }
    Ok(pr)
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
        Some(pr) => content(pr),
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
                let _ = tx.send(content(&pr));
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

    fn rich() -> Value {
        json!({
            "number": 41, "title": "Benchmark fixtures for Notes' redraw", "state": "OPEN", "isDraft": false,
            "author": { "login": "priya" }, "baseRefName": "main", "headRefName": "priya/notes-bench",
            "body": "## Summary\n\nAdds two fixture sets.", "additions": 812, "deletions": 14, "changedFiles": 10,
            "commits": { "totalCount": 4 },
            "last": { "nodes": [{ "commit": { "statusCheckRollup": { "contexts": { "nodes": [
                { "__typename": "CheckRun", "name": "fmt", "status": "COMPLETED", "conclusion": "SUCCESS" },
                { "__typename": "CheckRun", "name": "test (macos)", "status": "IN_PROGRESS", "conclusion": null },
                { "__typename": "StatusContext", "context": "deploy", "state": "FAILURE" }
            ] } } } }] },
            "reviewRequests": { "nodes": [{ "requestedReviewer": { "login": "ana" } }, { "requestedReviewer": { "login": "jo" } }, { "requestedReviewer": { "name": "core" } }] },
            "latestReviews": { "nodes": [{ "author": { "login": "ana" }, "state": "CHANGES_REQUESTED" }, { "author": { "login": "lee" }, "state": "APPROVED" }] },
            "reviewThreads": { "nodes": [
                { "isResolved": false, "path": "tests/notes_bench.rs", "line": 38, "comments": { "totalCount": 3, "nodes": [
                    { "author": { "login": "ana" }, "body": "The test reads only small.json." }
                ] }, "latest": { "nodes": [{ "author": { "login": "priya" }, "body": "will add it" }] } },
                { "isResolved": true, "path": "a.rs", "line": 1, "comments": { "totalCount": 1, "nodes": [{ "author": { "login": "ana" }, "body": "Done." }] } }
            ] },
            "files": { "nodes": (0..10).map(|i| json!({ "path": format!("f{i}.rs"), "additions": 10 + i, "deletions": i % 2 })).collect::<Vec<_>>() },
            "comments": { "nodes": [{ "author": { "login": "sam" }, "body": "Nice.", "createdAt": "2026-11-08T10:00:00Z" }] }
        })
    }

    #[test]
    fn the_boards_column_has_the_cursor_and_the_panel_keeps_still() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let now = "2026-11-08T12:00:00Z".parse().unwrap();
        let mut pager =
            pager::Pager::new(Content::Board(board(&rich(), now)), 130, 40).with_cursor(true);
        let first = pager.frame();
        assert!(first.contains("›"), "the cursor drawn on the board");
        pager.key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_ne!(pager.frame(), first, "j moves it");
        assert_eq!(
            pager.top(),
            0,
            "the column stays while the cursor is in sight"
        );
    }

    #[test]
    fn the_wants_to_merge_line_starts_with_the_authors_avatar() {
        let now = "2026-11-08T12:00:00Z".parse().unwrap();
        let wants = |pr: &Value| match &board(pr, now).main[1] {
            Part::Row(pieces) => pieces.clone(),
            other => panic!("not a row: {other:?}"),
        };
        let pieces = wants(&rich());
        assert_eq!(
            pieces[0],
            Piece::Avatar(Some(Who::new(Service::GitHub, "priya")), "priya".into())
        );
        assert_eq!(
            pieces[2],
            Piece::Text("priya wants to merge 4 commits into ".into(), Tone::Dim)
        );
        let mut nameless = rich();
        nameless["author"] = Value::Null;
        let pieces = wants(&nameless);
        assert!(!pieces.iter().any(|p| matches!(p, Piece::Avatar(..))));
        assert_eq!(
            pieces[0],
            Piece::Text("someone wants to merge 4 commits into ".into(), Tone::Dim)
        );
    }

    #[test]
    fn the_board_has_the_head_description_comments_and_the_panel() {
        let now = "2026-11-08T12:00:00Z".parse().unwrap();
        let mut pager = pager::Pager::new(Content::Board(board(&rich(), now)), 130, 40);
        let screen = pager.screen().join("\n");
        for want in [
            "#41 Benchmark fixtures for Notes' redraw",
            "priya wants to merge 4 commits into main from notes-bench · +812 −14",
            "P    priya opened this",
            "Summary",
            "Adds two fixture sets.",
            "COMMENTS",
            "S    sam  2h ago",
            "       Nice.",
            "✕ deploy   ◌ test (macos)",
            "A  ana  requested again",
            "L  lee  approved",
            "J  jo  requested",
            "C  core  requested",
            "A    ana  tests/notes_bench.rs:38",
            "       The test reads only",
            "2 replies · priya: will add it",
            "FILES",
            "f0.rs",
            "2 more",
        ] {
            assert!(screen.contains(want), "{want:?} in:\n{screen}");
        }
        let row = |s: &str| {
            screen
                .lines()
                .find(|l| l.contains(s))
                .unwrap_or_default()
                .to_string()
        };
        assert!(row("CHECKS").trim_end().ends_with("1 of 3"), "{screen}");
        assert!(row("REVIEW COMMENTS").trim_end().ends_with("1 open"));
        assert!(
            row("Benchmark fixtures").contains("CHECKS"),
            "the panel starts by the head"
        );
        assert!(!screen.contains("a.rs:1"), "a resolved thread is left out");
        assert_eq!(
            screen.matches("A  ana").count() + screen.matches("A    ana").count(),
            2,
            "a reviewer asked again is listed once, and once in the threads"
        );
        assert!(pager.frame().contains("M-r"), "its keys on the footer");
        // A search finds what only the panel says, and lights it there.
        let key = |c: char| {
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(c),
                crossterm::event::KeyModifiers::NONE,
            )
        };
        pager.key(key('/'));
        for c in "core".chars() {
            pager.key(key(c));
        }
        pager.key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        let frame = pager.frame();
        assert!(!frame.contains("Pattern not found"));
        assert!(frame.contains("7;38;5;") && frame.contains("core"), "lit");
        // Shorter than the panel, the sections that don't fit beside it
        // follow the description, so each can be scrolled to.
        pager.resize(130, 12);
        let lines = pager.screen();
        assert!(lines[0].contains("CHECKS"), "{lines:#?}");
        pager.key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('G'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(
            pager.screen().iter().any(|l| l.contains("2 more")),
            "{:#?}",
            pager.screen()
        );
        // A thread with a code line wider than the panel follows the
        // description, where it scrolls across.
        let mut wide = rich();
        wide["reviewThreads"]["nodes"][0]["comments"]["nodes"][0]["body"] =
            json!(format!("```\n{}\n```", "y".repeat(80)));
        let pager_wide = pager::Pager::new(Content::Board(board(&wide, now)), 130, 60);
        let lines = pager_wide.screen();
        let row = lines
            .iter()
            .find(|l| l.contains("REVIEW COMMENTS"))
            .unwrap();
        assert!(row.starts_with("  REVIEW COMMENTS"), "{lines:#?}");
        assert!(
            lines
                .iter()
                .any(|l| l.contains("CHECKS") && l.contains('│'))
        );
        // Narrower, the panel's sections follow the description.
        pager.resize(80, 200);
        let lines = pager.screen();
        let at = |s: &str| {
            lines
                .iter()
                .position(|l| l.contains(s))
                .unwrap_or_else(|| panic!("{s} in {lines:#?}"))
        };
        assert!(at("Nice.") < at("CHECKS"));
        assert!(!lines.iter().any(|l| l.contains('│')));
    }

    #[test]
    fn the_checks_count_every_check_and_say_what_is_unlisted() {
        let mut pr = rich();
        let contexts = &mut pr["last"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"];
        contexts["checkRunCountsByState"] = json!([
            { "state": "SUCCESS", "count": 1 }, { "state": "IN_PROGRESS", "count": 1 }, { "state": "FAILURE", "count": 2 }
        ]);
        contexts["statusContextCountsByState"] = json!([{ "state": "FAILURE", "count": 1 }]);
        let now = "2026-11-08T12:00:00Z".parse().unwrap();
        let pager = pager::Pager::new(Content::Board(board(&pr, now)), 130, 40);
        let screen = pager.screen().join("\n");
        let row = |s: &str| {
            screen
                .lines()
                .find(|l| l.contains(s))
                .unwrap_or_default()
                .to_string()
        };
        assert!(row("CHECKS").trim_end().ends_with("1 of 5"), "{screen}");
        assert!(screen.contains("2 more, 2 failing"), "{screen}");
    }

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
