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
//! The PR is laid out as board 13 draws it (`board`,
//! through `super::board`): the description, the conversation's comments
//! and its open review threads, each in a box as GitHub draws them (boards
//! 131 and 132), with the PR's checks, reviewers and files beside them,
//! all from the same query.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Value, json};

use super::board::{self, Board, Comment, Do, Part, Piece, Place, Reply};
use super::follow;
use super::pager::{self, Content};
use super::ready;
use crate::integrations::github;
use crate::photos::{Service, Who};
use crate::ui::kit::{Mark, Tone};
use crate::{paths, util};

/// What asked, for the request audit.
const SOURCE: &str = "M-v PR view";
/// Where a diagram that can't be drawn here is shown properly.
pub const PLACE: &str = "M-r on GitHub";

const QUERY: &str = "query($owner: String!, $name: String!, $number: Int!) { repository(owner: $owner, name: $name) { \
    pullRequest(number: $number) { number title url state isDraft createdAt author { login } \
    baseRefName headRefName body additions deletions changedFiles commits { totalCount } \
    mergeable mergeStateStatus reviewDecision autoMergeRequest { enabledAt } mergeQueueEntry { state position } \
    last: commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100) { \
      checkRunCountsByState { state count } statusContextCountsByState { state count } \
      pageInfo { hasNextPage endCursor } nodes { __typename \
      ... on CheckRun { name status conclusion title isRequired(pullRequestNumber: $number) } \
      ... on StatusContext { context state description isRequired(pullRequestNumber: $number) } } } } } } } \
    reviewRequests(first: 20) { nodes { requestedReviewer { ... on User { login } ... on Team { name } } } } \
    latestReviews(first: 20) { nodes { author { login } state } } \
    reviewThreads(first: 100) { pageInfo { hasNextPage endCursor } nodes { isResolved path line \
      comments(first: 1) { totalCount nodes { author { login } body url createdAt diffHunk } } \
      latest: comments(last: 1) { nodes { author { login } body createdAt } } } } \
    files(first: 100) { nodes { path additions deletions } } \
    comments(first: 100) { nodes { author { login __typename } body createdAt url } } } } }";

/// The PR's review threads after the first hundred, a page at a time, for
/// the column to count the open ones among them all: GitHub can't be asked
/// for only the unresolved ones.
const THREADS_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!, $after: String) { \
    repository(owner: $owner, name: $name) { pullRequest(number: $number) { \
    reviewThreads(first: 100, after: $after) { pageInfo { hasNextPage endCursor } nodes { isResolved path line \
      comments(first: 1) { totalCount nodes { author { login } body url createdAt diffHunk } } \
      latest: comments(last: 1) { nodes { author { login } body createdAt } } } } } } }";
/// The most pages of review threads read, five hundred threads.
const THREAD_PAGES: usize = 5;

/// The PR as Markdown: pr-view.js's header over its description, and
/// under the header whether it can merge (`ready::verdict`).
pub fn markdown(pr: &Value) -> String {
    if ready::is_branch(pr) {
        return branch_markdown(pr);
    }
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
    let verdict = ready::verdict(pr, rollup(pr))
        .map(|v| match v.reasons_text() {
            r if r.is_empty() => format!("**{}**\n\n", v.said),
            r => format!("**{}** {r}\n\n", v.said),
        })
        .unwrap_or_default();
    format!(
        "# {} #{number}\n\n_{state} · {author} wants to merge `{}` into `{}`_\n\n{verdict}---\n\n{}",
        text("title"),
        text("headRefName"),
        text("baseRefName"),
        text("body")
    )
}

/// A branch with no PR as Markdown: how its CI went on GitHub, each check.
fn branch_markdown(copy: &Value) -> String {
    let v = ready::branch_verdict(copy);
    let branch = str_at(copy, "/branch");
    let mut out = format!(
        "# {branch}\n\n_No PR_\n\n**{}** {}\n\n",
        v.said,
        v.reasons_text()
    );
    for c in nodes(copy, &format!("/checks{}/nodes", ready::CONTEXTS)) {
        out.push_str(&format!("- {} {}\n", check_mark(c).look().0, check_name(c)));
    }
    out
}

/// The PR's checks listed by name, each with its mark, what needs you
/// first: the failures, then what is still running, as the board's panel
/// lists them (the phone's PR view reads them too).
pub fn checks_listed(pr: &Value) -> Vec<(Mark, String)> {
    let mut marked: Vec<(Mark, String)> = nodes(pr, &format!("{ROLLUP}/nodes"))
        .iter()
        .map(|c| (check_mark(c), check_name(c).to_string()))
        .collect();
    marked.sort_by_key(|(m, _)| match m {
        Mark::Failed => 0,
        Mark::Pending => 1,
        _ => 2,
    });
    marked
}

/// The PR's checks, the first hundred by name with every check counted.
fn rollup(pr: &Value) -> &Value {
    pr.pointer(ROLLUP).unwrap_or(&Value::Null)
}

/// Where the view's query puts its checks.
const ROLLUP: &str = "/last/nodes/0/commit/statusCheckRollup/contexts";

/// A check's name: a check run's, or a status's context.
fn check_name(c: &Value) -> &str {
    match str_at(c, "/name") {
        "" => str_at(c, "/context"),
        n => n,
    }
}

/// How many of a list the panel shows before "N more".
const FILES: usize = 8;
/// How many open review threads the column shows in boxes before saying
/// how many more GitHub has.
const THREADS: usize = 20;

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

/// The name a GitHub login shows as: itself, or "someone" for a deleted
/// account, which GitHub answers with no author.
fn name_of(login: &str) -> String {
    if login.is_empty() {
        "someone".into()
    } else {
        login.into()
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

/// The PR as board 13 draws it, its conversation as boards 131 and 132
/// do: its head, then its description, its comments and its open review
/// threads each in a box, with its checks, reviewers and files beside them.
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
    // Whether it can merge: a chip after its state, the sentence and its
    // reasons on a line under the head (board 150).
    let verdict = ready::verdict(pr, rollup(pr));
    let mut head = vec![Piece::Chip(state.into(), tone)];
    if let Some(v) = &verdict {
        head.push(Piece::Chip(v.chip_text(), v.tone));
    }
    head.extend([
        Piece::Name(format!("#{number}"), Tone::Accent),
        Piece::Text(" ".into(), Tone::Text),
        Piece::Name(str_at(pr, "/title").into(), Tone::Text),
    ]);
    let mut main = vec![Part::Row(head), Part::Row(wants)];
    if let Some(v) = &verdict {
        main.push(verdict_row(v));
    }
    main.extend([Part::Rule, Part::Blank]);
    let url = str_at(pr, "/url");
    let on_github = |url: &str| {
        Some(url)
            .filter(|u| !u.is_empty())
            .map(|u| ("Open it on GitHub".to_string(), u.to_string()))
    };
    let body = str_at(pr, "/body");
    main.push(Part::Comment(Box::new(Comment {
        key: format!("{url}#description"),
        who: (!login.is_empty()).then(|| Who::new(Service::GitHub, login)),
        name: author.into(),
        verb: "opened this".into(),
        when: board::when(str_at(pr, "/createdAt"), now),
        chip: Some(("Author".into(), Tone::Dim)),
        place: None,
        body: if body.trim().is_empty() {
            "_No description._".into()
        } else {
            body.into()
        },
        replies: Vec::new(),
        link: on_github(url),
        folds: false,
        whole: false,
        chosen: false,
    })));
    // The conversation after it, each comment in a box, then the open
    // review threads, each with its line of code and its latest reply.
    for (i, c) in nodes(pr, "/comments/nodes").iter().enumerate() {
        let who = str_at(c, "/author/login");
        let chip = if str_at(c, "/author/__typename") == "Bot" || who.ends_with("[bot]") {
            Some(("bot".into(), Tone::Dim))
        } else if !who.is_empty() && who == login {
            Some(("Author".into(), Tone::Dim))
        } else {
            None
        };
        main.extend([
            Part::Blank,
            Part::Comment(Box::new(Comment {
                key: match str_at(c, "/url") {
                    "" => format!("{url}#comment-{i}"),
                    u => u.into(),
                },
                who: (!who.is_empty()).then(|| Who::new(Service::GitHub, who)),
                name: name_of(who),
                verb: "commented".into(),
                when: board::when(str_at(c, "/createdAt"), now),
                chip,
                place: None,
                body: str_at(c, "/body").into(),
                replies: Vec::new(),
                link: on_github(str_at(c, "/url")),
                folds: true,
                whole: false,
                chosen: false,
            })),
        ]);
    }
    let open: Vec<&Value> = nodes(pr, "/reviewThreads/nodes")
        .iter()
        .filter(|t| t.get("isResolved").and_then(Value::as_bool) != Some(true))
        .collect();
    for (i, t) in open.iter().take(THREADS).enumerate() {
        let first = nodes(t, "/comments/nodes")
            .first()
            .cloned()
            .unwrap_or(Value::Null);
        let who = str_at(&first, "/author/login");
        let replies = count(t, "/comments/totalCount").saturating_sub(1);
        let latest = nodes(t, "/latest/nodes")
            .last()
            .cloned()
            .unwrap_or(Value::Null);
        let reply = (replies > 0).then(|| {
            let by = str_at(&latest, "/author/login");
            let when = board::when(str_at(&latest, "/createdAt"), now);
            let mut said = "replied".to_string();
            for more in [when, format!("{replies} replies")] {
                if !more.is_empty() && (replies > 1 || !more.ends_with("replies")) {
                    said.push_str(&format!(" · {more}"));
                }
            }
            Reply {
                who: (!by.is_empty()).then(|| Who::new(Service::GitHub, by)),
                name: name_of(by),
                said,
                body: str_at(&latest, "/body").into(),
            }
        });
        main.extend([
            Part::Blank,
            Part::Comment(Box::new(Comment {
                key: match str_at(&first, "/url") {
                    "" => format!("{url}#thread-{i}"),
                    u => u.into(),
                },
                who: (!who.is_empty()).then(|| Who::new(Service::GitHub, who)),
                name: name_of(who),
                verb: "commented".into(),
                when: board::when(str_at(&first, "/createdAt"), now),
                chip: None,
                place: Some(Place {
                    path: str_at(t, "/path").into(),
                    line: t.get("line").and_then(Value::as_u64),
                    code: str_at(&first, "/diffHunk")
                        .lines()
                        .last()
                        .filter(|l| !l.trim().is_empty())
                        .map(str::to_string),
                }),
                body: str_at(&first, "/body").into(),
                replies: reply.into_iter().collect(),
                link: on_github(str_at(&first, "/url")),
                folds: true,
                whole: false,
                chosen: false,
            })),
        ]);
    }
    if open.len() > THREADS {
        main.extend([
            Part::Blank,
            Part::Row(vec![Piece::Text(
                format!(
                    "{} more open review comments on GitHub",
                    open.len() - THREADS
                ),
                Tone::Dim,
            )]),
        ]);
    }

    let mut panel = Vec::new();
    let checks = nodes(pr, "/last/nodes/0/commit/statusCheckRollup/contexts/nodes");
    if !checks.is_empty() {
        let mut marked: Vec<(Mark, &str, bool)> = checks
            .iter()
            .map(|c| {
                let required = c.get("isRequired").and_then(Value::as_bool) == Some(true);
                (check_mark(c), check_name(c), required)
            })
            .collect();
        // What needs you first: the failures, then what is still running.
        marked.sort_by_key(|(m, _, _)| match m {
            Mark::Failed => 0,
            Mark::Pending => 1,
            _ => 2,
        });
        // The counts GitHub gives for every check, of which only the
        // first 100 come by name; those listed where the counts are missing.
        let mut by_state: Vec<(&str, u64)> = Vec::new();
        for kind in ["checkRunCountsByState", "statusContextCountsByState"] {
            for n in nodes(pr, &format!("{ROLLUP}/{kind}")) {
                by_state.push((str_at(n, "/state"), count(n, "/count")));
            }
        }
        let listed = |m: Mark| marked.iter().filter(|(k, _, _)| *k == m).count() as u64;
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
        let flow = |label: Option<&str>, required: Option<bool>| {
            let mut groups: Vec<Vec<Piece>> = label
                .map(|l| vec![Piece::Text(l.into(), Tone::Dim)])
                .into_iter()
                .collect();
            groups.extend(
                marked
                    .iter()
                    .filter(|(_, _, r)| required.is_none_or(|want| *r == want))
                    .map(|(m, name, _)| {
                        vec![Piece::Mark(*m), Piece::Text(format!(" {name}"), Tone::Text)]
                    }),
            );
            Part::Flow(groups)
        };
        // The required checks apart from the rest (board 150), where GitHub
        // said which are required and some are.
        if ready::required(rollup(pr)).is_some_and(|(_, total)| total > 0) {
            panel.push(flow(Some("Required"), Some(true)));
            if marked.iter().any(|(_, _, r)| !r) {
                panel.push(flow(Some("Optional"), Some(false)));
            }
        } else {
            panel.push(flow(None, None));
        }
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
            ("Tab", "next box"),
            ("Enter", "its menu"),
            ("/", "search"),
            ("r", "read again"),
            ("M-r", "on GitHub"),
            ("m", "view actions"),
            ("M-d", "its diff"),
            ("M-c", "back to Claude"),
        ]
        .map(|(k, w)| (k.to_string(), w.to_string()))
        .to_vec(),
        actions: actions(str_at(pr, "/url"), verdict.as_ref()),
        title: format!("PR #{number}"),
    }
}

/// The view's menu besides reading it again: the page on GitHub, then
/// what can be done about the verdict, as M-m's box offered it (board 154).
fn actions(url: &str, verdict: Option<&ready::Verdict>) -> Vec<(String, Do)> {
    let mut out = Vec::new();
    if !url.is_empty() {
        out.push(("Open it on GitHub".to_string(), Do::Open(url.into())));
    }
    // A PR opened by its number (another branch's, `--pr`) is no PR of the
    // tab's own checkout, which these two would act on.
    let own = !NUMBERED.load(std::sync::atomic::Ordering::Relaxed);
    if own && verdict.is_some_and(|v| v.conflicts) {
        out.push((
            RESOLVE.to_string(),
            Do::Press(super::Command::ResolveConflicts.name().into()),
        ));
    }
    if own && verdict.is_some_and(|v| v.failing) {
        out.push((
            FAILING.to_string(),
            Do::Press(super::Command::FailingChecks.name().into()),
        ));
    }
    if SEVERAL.load(std::sync::atomic::Ordering::Relaxed) {
        out.push((
            crate::mux::keys::Action::TabPrs.about(),
            Do::Press(crate::mux::keys::Action::TabPrs.name()),
        ));
    }
    out
}

/// Whether the view's tab followed several PRs as it opened (`pr-view
/// --several`), which puts "Show another of this tab's PRs" in its menu.
pub static SEVERAL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the view shows a PR by its number (`main_numbered`), another
/// branch's than the tab's own checkout.
static NUMBERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The menu's words for the two things M-m's box offered.
pub const RESOLVE: &str = "Type /resolve-conflicts into this tab's Claude";
pub const FAILING: &str = "See the failing checks";

/// The line under the head: the verdict's sentence, then its reasons, dim.
fn verdict_row(v: &ready::Verdict) -> Part {
    let mut row = vec![Piece::Name(v.said.clone(), v.tone)];
    if !v.reasons.is_empty() {
        row.push(Piece::Text(format!("  {}", v.reasons_text()), Tone::Dim));
    }
    Part::Row(row)
}

/// How many checks the board of a branch with no PR lists before "N more".
const BRANCH_CHECKS: usize = 20;

/// A branch with no PR as board 153 draws it: its CI on the commit GitHub
/// has for it, the verdict first, then each check, and how this tab
/// stands against that commit.
pub fn branch_board(copy: &Value) -> Board {
    let v = ready::branch_verdict(copy);
    let branch = str_at(copy, "/branch");
    let mut head = vec![
        Piece::Chip("No PR".into(), Tone::Dim),
        Piece::Chip(v.chip_text(), v.tone),
        Piece::Name(branch.into(), Tone::Text),
    ];
    if let (Some(n), Some(default)) = (
        copy.get("aheadOfDefault").and_then(Value::as_u64),
        copy.get("defaultBranch").and_then(Value::as_str),
    ) {
        head.push(Piece::Text(
            format!(
                "  {n} commit{} ahead of {default}",
                if n == 1 { "" } else { "s" }
            ),
            Tone::Dim,
        ));
    }
    let mut main = vec![Part::Row(head), Part::Blank, verdict_row(&v), Part::Blank];
    let checks = nodes(copy, &format!("/checks{}/nodes", ready::CONTEXTS));
    if !checks.is_empty() {
        let passed = checks
            .iter()
            .filter(|c| check_mark(c) == Mark::Passed)
            .count();
        main.push(Part::Label(
            "Checks".into(),
            Some(format!("{passed} of {}", checks.len())),
        ));
        for c in checks.iter().take(BRANCH_CHECKS) {
            let about = match str_at(c, "/description") {
                "" => str_at(c, "/title"),
                d => d,
            };
            main.push(Part::Ends(
                vec![
                    Piece::Mark(check_mark(c)),
                    Piece::Text(format!(" {}", check_name(c)), Tone::Text),
                ],
                vec![Piece::Text(about.into(), Tone::Dim)],
            ));
        }
        if checks.len() > BRANCH_CHECKS {
            main.push(board::more(checks.len() - BRANCH_CHECKS));
        }
    }
    let mut actions = Vec::new();
    let (owner, name) = (str_at(copy, "/owner"), str_at(copy, "/name"));
    let on_github = copy.get("checks").is_some_and(Value::is_object);
    if !owner.is_empty() && !name.is_empty() && on_github {
        actions.push((
            "Open it on GitHub".to_string(),
            Do::Open(format!("https://github.com/{owner}/{name}/tree/{branch}")),
        ));
    }
    if v.failing {
        actions.push((
            FAILING.to_string(),
            Do::Press(super::Command::FailingChecks.name().into()),
        ));
    }
    Board {
        main,
        panel: Vec::new(),
        place: PLACE.into(),
        keys: [
            ("j/k", "scroll"),
            ("r", "read again"),
            ("m", "view actions"),
            ("M-c", "back to Claude"),
        ]
        .map(|(k, w)| (k.to_string(), w.to_string()))
        .to_vec(),
        actions,
        title: branch.into(),
    }
}

/// What the view shows of a PR: board 13.
fn content(pr: &Value) -> Content {
    if ready::is_branch(pr) {
        Content::Board(branch_board(pr))
    } else {
        Content::Board(board(pr, jiff::Timestamp::now()))
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
    // Every check when one failed or is running, so the required ones are
    // counted among them all, and whether it can merge once GitHub knows.
    if let Some(first) = pr.pointer(ROLLUP).cloned() {
        let (contexts, _) = ready::pr_checks(repo, number, first)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        if let Some(c) = pr.pointer_mut(ROLLUP) {
            *c = contexts;
        }
    }
    ready::settle(repo, number, &mut pr).await;
    Ok(pr)
}

/// The PR for the branch checked out in `dir`, else the branch's own CI
/// on GitHub (`ready::branch_ci`), or why there is neither, in words for
/// the person.
pub async fn for_dir(dir: &Path) -> Result<Value, String> {
    let found = github::branch_pr(dir, SOURCE)
        .await
        .map_err(|e| e.to_string())?;
    let read = match found {
        Some((repo, pr)) => by_number(&repo, pr.number, SOURCE)
            .await
            .map_err(|e| e.to_string())?,
        None => ready::branch_ci(dir).await?,
    };
    Ok(read)
}

/// The PR view: the last copy at once, then the PR read again, and again
/// as it changes (`follow`).
pub fn main(dir: PathBuf) -> ExitCode {
    let cache = cache_file(&dir);
    let cached = cache.as_ref().and_then(util::read_json);
    let initial = match &cached {
        Some(pr) => content(pr),
        None => Content::Loading("Reading the PR...".into()),
    };
    // The branch checked out is read each time, so its file is too: a
    // branch switched to since never has its PR kept as the first one's.
    let file = {
        let dir = dir.clone();
        move || cache_file(&dir)
    };
    // The read as the view opens asks mission control's PR views to look
    // again too, as M-m's answer did, so they agree with it; the reads
    // after it don't, since the lookups' own answers ask for most of them.
    let first = std::sync::atomic::AtomicBool::new(true);
    let read = move || {
        let read = util::block_on(for_dir(&dir));
        if first.swap(false, std::sync::atomic::Ordering::Relaxed) && read.is_ok() {
            util::append_quietly(paths::state_dir().join("github"), "\n");
        }
        read
    };
    pager::run_following(initial, follow::start(cached, file, read, content))
}

/// The PR view on one PR by its number, whatever the directory's branch:
/// a tab's current PR when its session opened it from another branch or
/// repository (src/tabs/prs.rs), its copy kept under its repository and
/// number.
pub fn main_numbered(repo: github::Repo, number: u64) -> ExitCode {
    NUMBERED.store(true, std::sync::atomic::Ordering::Relaxed);
    let cache = Some(
        paths::cache_dir()
            .join("pr-view")
            .join(format!("{}%{}#{number}.json", repo.owner, repo.name)),
    );
    let cached = cache.as_ref().and_then(util::read_json);
    let initial = match &cached {
        Some(pr) => content(pr),
        None => Content::Loading("Reading the PR...".into()),
    };
    let file = move || cache.clone();
    let read = move || util::block_on(by_number(&repo, number, SOURCE)).map_err(|e| e.to_string());
    pager::run_following(initial, follow::start(cached, file, read, content))
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
                    { "author": { "login": "ana" }, "body": "The test reads only small.json.", "createdAt": "2026-11-08T09:00:00Z" }
                ] }, "latest": { "nodes": [{ "author": { "login": "priya" }, "body": "will add it", "createdAt": "2026-11-08T11:00:00Z" }] } },
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
            // The description in a box of its own, the avatar beside it.
            "P    ╭─",
            "     │ priya opened this",
            "Author   ⋯ │",
            "     │ Summary",
            "     │ Adds two fixture sets.",
            // A line down the side joins it to the next box.
            "\n          │ ",
            "S    ╭─",
            "     │ sam commented · 2h ago",
            "     │ Nice.",
            "✕ deploy   ◌ test (macos)",
            "A  ana  requested again",
            "L  lee  approved",
            "J  jo  requested",
            "C  core  requested",
            // An open review thread in its box: its place, its words,
            // its latest reply inside it.
            "     │ ana commented · 3h ago",
            "     │ tests/notes_bench.rs line 38",
            "     │ The test reads only small.json.",
            "     │ P  priya  replied · 1h ago · 2 replies",
            "     │    will add it",
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
        assert!(!screen.contains("COMMENTS"), "no labels between the boxes");
        assert!(
            row("Benchmark fixtures").contains("CHECKS"),
            "the panel starts by the head"
        );
        assert!(!screen.contains("a.rs"), "a resolved thread is left out");
        assert_eq!(
            screen.matches("A  ana").count(),
            1,
            "a reviewer asked again is listed once"
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
        // follow the conversation, so each can be scrolled to.
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
        // A code line wider than its box runs on past the box's side,
        // where it scrolls across.
        let mut wide = rich();
        wide["reviewThreads"]["nodes"][0]["comments"]["nodes"][0]["body"] =
            json!(format!("```\n{}\n```", "y".repeat(80)));
        let pager_wide = pager::Pager::new(Content::Board(board(&wide, now)), 130, 60);
        let lines = pager_wide.screen();
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("       │ ") && l.contains(&"y".repeat(40))),
            "{lines:#?}"
        );
        // Narrower, with no panel beside it, each avatar goes into its
        // box's header bar (board 132) and the panel's sections follow.
        pager.resize(80, 200);
        let lines = pager.screen();
        let at = |s: &str| {
            lines
                .iter()
                .position(|l| l.contains(s))
                .unwrap_or_else(|| panic!("{s} in {lines:#?}"))
        };
        assert!(at("Nice.") < at("CHECKS"));
        assert!(lines[at("CHECKS")].starts_with("  CHECKS"), "{lines:#?}");
        assert!(lines[at("sam commented")].starts_with("  │ S  sam commented"));
        assert!(lines[at("sam commented") + 2].starts_with("  │ Nice."));
        assert!(lines.iter().any(|l| l == "    │"), "the line down the side");
    }

    #[test]
    fn tab_chooses_a_box_and_enter_opens_its_menu_which_unfolds_a_long_one() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let now = "2026-11-08T12:00:00Z".parse().unwrap();
        let mut pr = rich();
        let long: Vec<String> = (1..=20).map(|i| format!("Line {i}.")).collect();
        pr["comments"]["nodes"][0]["body"] = json!(long.join("\n\n"));
        pr["comments"]["nodes"][0]["url"] = json!("https://github.com/o/r/pull/41#issuecomment-1");
        pr["comments"]["nodes"].as_array_mut().unwrap().push(json!({
            "author": { "login": "ci-bot", "__typename": "Bot" },
            "body": "See [the run](https://ci.example/run/7).", "createdAt": "2026-11-08T11:00:00Z"
        }));
        let mut pager = pager::Pager::new(Content::Board(board(&pr, now)), 130, 80);
        let screen = pager.screen().join("\n");
        // A long comment is folded, saying how much more it has.
        assert!(screen.contains("Line 1."), "{screen}");
        assert!(!screen.contains("Line 20."), "{screen}");
        assert!(
            screen.contains("▾ 31 more lines · Enter shows them"),
            "{screen}"
        );
        assert!(screen.contains("ci-bot commented · 1h ago") && screen.contains("bot "));
        // Tab chooses the description's box, Enter opens its menu.
        pager.key(key(KeyCode::Tab));
        pager.key(key(KeyCode::Enter));
        let menu = pager.frame_with_menu();
        assert!(menu.contains("priya's description"), "{menu}");
        assert!(menu.contains("Copy its text"));
        pager.key(key(KeyCode::Esc));
        assert!(!pager.menu_open());
        // The next box is sam's, folded: its menu shows it whole first.
        pager.key(key(KeyCode::Tab));
        pager.key(key(KeyCode::Enter));
        let menu = pager.frame_with_menu();
        assert!(
            menu.contains("sam's comment") && menu.contains("Show all of it"),
            "{menu}"
        );
        assert!(menu.contains("Open it on GitHub"));
        pager.key(key(KeyCode::Enter));
        let screen = pager.screen().join("\n");
        assert!(screen.contains("Line 20."), "{screen}");
        assert!(!screen.contains("more lines"));
        // Shift-Tab goes back, wrapping from the first box to the last.
        pager.key(key(KeyCode::BackTab));
        pager.key(key(KeyCode::BackTab));
        pager.key(key(KeyCode::Enter));
        assert!(pager.frame_with_menu().contains("ana's comment"));
        pager.key(key(KeyCode::Esc));
        // A link inside a box is found by a search and kept as a link.
        pager.key(key(KeyCode::Char('/')));
        for c in "the run".chars() {
            pager.key(key(KeyCode::Char(c)));
        }
        pager.key(key(KeyCode::Enter));
        assert!(!pager.frame().contains("Pattern not found"));
        assert!(
            pager
                .doc()
                .links
                .iter()
                .any(|l| l == "https://ci.example/run/7")
        );
        let line = pager
            .doc()
            .lines
            .iter()
            .find(|l| l.plain().contains("the run"))
            .unwrap();
        let col = line.plain().find("the run").unwrap();
        assert!(
            line.link_at(crate::ui::draw::width(&line.plain()[..col]))
                .is_some()
        );
        // A search for what a fold hides shows the comment whole and finds it.
        let mut pager = pager::Pager::new(Content::Board(board(&pr, now)), 130, 80);
        pager.key(key(KeyCode::Char('/')));
        for c in "Line 19.".chars() {
            pager.key(key(KeyCode::Char(c)));
        }
        pager.key(key(KeyCode::Enter));
        assert!(!pager.frame().contains("Pattern not found"));
        assert!(pager.screen().iter().any(|l| l.contains("Line 19.")));
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

    /// `rich` as GitHub answers whether it can merge: `merge_state`, its
    /// checks marked required or not.
    fn merging(merge_state: &str, mergeable: &str) -> Value {
        let mut pr = rich();
        pr["url"] = json!("https://github.com/acme/w/pull/41");
        pr["mergeable"] = json!(mergeable);
        pr["mergeStateStatus"] = json!(merge_state);
        pr["reviewDecision"] = json!("CHANGES_REQUESTED");
        let nodes = &mut pr["last"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["nodes"];
        nodes[0]["isRequired"] = json!(true);
        nodes[1]["isRequired"] = json!(false);
        nodes[2]["isRequired"] = json!(true);
        pr
    }

    fn screen_of(pr: &Value) -> String {
        let now = "2026-11-08T12:00:00Z".parse().unwrap();
        pager::Pager::new(Content::Board(board(pr, now)), 130, 40)
            .screen()
            .join("\n")
    }

    #[test]
    fn a_pr_with_conflicts_says_so_in_its_head_and_offers_resolve_conflicts() {
        let pr = merging("DIRTY", "CONFLICTING");
        let screen = screen_of(&pr);
        let head = screen.lines().find(|l| l.contains("#41")).unwrap();
        assert!(
            head.contains("Open") && head.contains("✕ Conflicts with main"),
            "{head}"
        );
        assert!(
            screen.contains(
                "Not ready: it has merge conflicts.  required 1 of 2 passed · changes requested"
            ),
            "{screen}"
        );
        // The panel's checks, the required ones apart.
        let required = screen.lines().find(|l| l.contains("Required")).unwrap();
        assert!(
            required.contains("✕ deploy") && required.contains("✓ fmt"),
            "{required}"
        );
        let optional = screen.lines().find(|l| l.contains("Optional")).unwrap();
        assert!(optional.contains("◌ test (macos)"), "{optional}");
        let now = "2026-11-08T12:00:00Z".parse().unwrap();
        let labels: Vec<String> = board(&pr, now)
            .actions
            .into_iter()
            .map(|(l, _)| l)
            .collect();
        assert_eq!(labels, ["Open it on GitHub", RESOLVE, FAILING]);
        let footer = pager::Pager::new(Content::Board(board(&pr, now)), 130, 40).frame();
        assert!(footer.contains("view actions"), "on the footer");
    }

    #[test]
    fn a_pr_in_the_merge_queue_shows_its_place_and_a_clean_one_says_it_can_merge() {
        let mut queued = merging("CLEAN", "MERGEABLE");
        queued["mergeQueueEntry"] = json!({ "state": "AWAITING_CHECKS", "position": 2 });
        let screen = screen_of(&queued);
        assert!(screen.contains("▲ In the merge queue"), "{screen}");
        assert!(
            screen.contains("In the merge queue at position 2.  waiting on the queue's checks"),
            "{screen}"
        );
        let mut clean = merging("CLEAN", "MERGEABLE");
        clean["reviewDecision"] = json!("APPROVED");
        let screen = screen_of(&clean);
        assert!(screen.contains("✓ Ready to merge"), "{screen}");
        assert!(
            screen.contains("Ready to merge.  no conflicts · required checks passing · approved"),
            "{screen}"
        );
        let now = "2026-11-08T12:00:00Z".parse().unwrap();
        let labels: Vec<String> = board(&clean, now)
            .actions
            .into_iter()
            .map(|(l, _)| l)
            .collect();
        assert!(!labels.iter().any(|l| l == RESOLVE), "{labels:?}");
        // A copy kept before it was asked whether it can merge has no verdict.
        assert!(!screen_of(&rich()).contains("Ready to merge"));
    }

    #[test]
    fn a_branch_with_no_pr_shows_its_ci_and_how_this_tab_stands() {
        let copy = json!({
            "noPr": true, "owner": "acme", "name": "w", "branch": "notes-bench",
            "complete": true, "local": { "ahead": 1 }, "defaultBranch": "main", "aheadOfDefault": 4,
            "checks": { "commits": { "nodes": [{ "commit": { "oid": "3fa91c2aaaa", "statusCheckRollup": { "contexts": {
                "checkRunCountsByState": [{ "state": "SUCCESS", "count": 1 }, { "state": "FAILURE", "count": 1 }],
                "statusContextCountsByState": [],
                "nodes": [
                    { "__typename": "CheckRun", "name": "fmt", "status": "COMPLETED", "conclusion": "SUCCESS" },
                    { "__typename": "CheckRun", "name": "test (macos)", "status": "COMPLETED", "conclusion": "FAILURE", "title": "tests::redraw_large failed" }
                ]
            } } } }] } }
        });
        let pager = pager::Pager::new(Content::Board(branch_board(&copy)), 130, 20);
        let screen = pager.screen().join("\n");
        for want in [
            "No PR",
            "✕ CI failing",
            "notes-bench  4 commits ahead of main",
            "Failing: 1 of 2.  CI on notes-bench at 3fa91c2 · this tab is 1 commit ahead of what GitHub checked",
            "✓ fmt",
            "✕ test (macos)",
            "tests::redraw_large failed",
        ] {
            assert!(screen.contains(want), "{want:?} in:\n{screen}");
        }
        let labels: Vec<String> = branch_board(&copy)
            .actions
            .into_iter()
            .map(|(l, _)| l)
            .collect();
        assert_eq!(labels, ["Open it on GitHub", FAILING]);
        // A pushed branch with no checks is still on GitHub.
        let mut quiet = copy.clone();
        quiet["checks"]["commits"]["nodes"][0]["commit"]["statusCheckRollup"] = Value::Null;
        let labels: Vec<String> = branch_board(&quiet)
            .actions
            .into_iter()
            .map(|(l, _)| l)
            .collect();
        assert_eq!(labels, ["Open it on GitHub"]);
        assert!(markdown(&copy).contains("**Failing: 1 of 2.**"));
    }

    #[test]
    fn m_opens_the_menu_and_its_choices_go_to_the_workspace() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let key = |c: KeyCode| KeyEvent::new(c, KeyModifiers::NONE);
        let now = "2026-11-08T12:00:00Z".parse().unwrap();
        let pr = merging("DIRTY", "CONFLICTING");
        let mut pager = pager::Pager::new(Content::Board(board(&pr, now)), 130, 40);
        assert_eq!(pager.key(key(KeyCode::Char('m'))), pager::Act::Redraw);
        assert!(pager.menu_open());
        let drawn = pager.frame_with_menu();
        assert!(
            drawn.contains("PR #41") && drawn.contains(RESOLVE),
            "the menu drawn"
        );
        // Opened on what it offers now, /resolve-conflicts first.
        assert_eq!(
            pager.key(key(KeyCode::Enter)),
            pager::Act::Press(super::super::Command::ResolveConflicts.name().into())
        );
        assert!(!pager.menu_open());
        pager.key(key(KeyCode::Char('m')));
        pager.key(key(KeyCode::Down));
        assert_eq!(
            pager.key(key(KeyCode::Enter)),
            pager::Act::Press(super::super::Command::FailingChecks.name().into())
        );
        pager.key(key(KeyCode::Char('m')));
        assert_eq!(pager.key(key(KeyCode::Esc)), pager::Act::Redraw);
        assert!(!pager.menu_open(), "Esc closes it");
        // A ticket's board has no menu.
        let mut plain = pager::Pager::new(
            Content::Markdown {
                text: "x".into(),
                place: String::new(),
            },
            80,
            10,
        );
        assert_eq!(plain.key(key(KeyCode::Char('m'))), pager::Act::Same);
    }
}
