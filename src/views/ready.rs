//! Whether the branch's PR is ready to merge (M-m; tmux/pr-ready.sh): a
//! verdict first, then each thing that decides it, in a box over the
//! workspace, ending with what can be done about it (see the failing
//! checks, or have the tab's Claude resolve the conflicts).
//!
//! Asked in the order that can end the answer soonest. A merged or closed
//! PR says so and stops. A PR already in the merge queue reports its place
//! there and stops too: its checks, conflicts and review were settled by
//! getting in, and GitHub still calls it mergeable, so the rest would only
//! say "ready" about something already on its way. For anything else the
//! verdict is GitHub's own mergeStateStatus, so it agrees with the merge
//! button, with the lines under it explaining it. Checks are split into
//! required ones and all of them, since a failing optional check (a preview
//! deploy) doesn't block the merge and shouldn't read as if it did.
//!
//! Where pr-ready.sh asked gh three times (the PR, its queue entry and
//! checks counted by outcome, and `gh pr checks --required` one by one),
//! one GraphQL query gives it all here: the counts by state, and each of
//! the first hundred checks with whether it is required. GitHub works out
//! mergeability lazily, so an UNKNOWN is asked again, up to three times a
//! second apart, as pr-ready.sh did.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::LazyLock;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use regex::Regex;
use serde_json::{Value, json};

use super::pager::{self, Content};
use crate::integrations::github;
use crate::mux::input::{Key, KeyCode, Mods, Mouse};
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use crate::ui::{Look, Outcome, Overlay, Step};
use crate::util;

const SOURCE: &str = "M-m PR check";

const QUERY: &str = "query($owner: String!, $name: String!, $number: Int!) { repository(owner: $owner, name: $name) { \
    pullRequest(number: $number) { number title url state isDraft mergeable mergeStateStatus reviewDecision \
    autoMergeRequest { enabledAt } mergeQueueEntry { state position } \
    commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100) { \
    checkRunCountsByState { state count } statusContextCountsByState { state count } \
    nodes { __typename \
      ... on CheckRun { name status conclusion detailsUrl isRequired(pullRequestNumber: $number) } \
      ... on StatusContext { context state targetUrl description isRequired(pullRequestNumber: $number) } \
    } } } } } } } } }";

/// What the answer offers to do, by the number the box gives back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    SeeFailing = 0,
    ResolveConflicts = 1,
    Close = 2,
}

impl Choice {
    pub fn from_index(i: usize) -> Choice {
        match i {
            0 => Choice::SeeFailing,
            1 => Choice::ResolveConflicts,
            _ => Choice::Close,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Choice::SeeFailing => "See the failing checks",
            Choice::ResolveConflicts => "Type /resolve-conflicts into this tab's Claude",
            Choice::Close => "Close",
        }
    }
}

/// The answer, as the box shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub lines: Vec<Line<'static>>,
    pub choices: Vec<Choice>,
}

impl Answer {
    /// Its text, for the tests.
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn green() -> Style {
    Style::new().fg(Color::Green)
}
fn red() -> Style {
    Style::new().fg(Color::Red)
}
fn yellow() -> Style {
    Style::new().fg(Color::Yellow)
}
fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}
fn bold(style: Style) -> Style {
    style.add_modifier(Modifier::BOLD)
}

/// A check's state in gh's buckets, as `gh pr checks` says them.
fn bucket(state: &str) -> &'static str {
    match state {
        "SUCCESS" | "NEUTRAL" | "COMPLETED" => "pass",
        "SKIPPED" => "skipping",
        "CANCELLED" => "cancel",
        "IN_PROGRESS" | "PENDING" | "QUEUED" | "WAITING" | "EXPECTED" => "pending",
        _ => "fail",
    }
}

/// One check's bucket: a check run by its conclusion once it has
/// completed, a status by its state.
fn check_bucket(node: &Value) -> &'static str {
    let text = |k: &str| node.get(k).and_then(Value::as_str).unwrap_or("");
    if node.get("__typename").and_then(Value::as_str) == Some("CheckRun") {
        if text("status") != "COMPLETED" {
            return "pending";
        }
        return bucket(text("conclusion"));
    }
    bucket(text("state"))
}

/// Counts by bucket, `3 pass, 1 fail`, in gh's order; "none" for none.
fn counts(pairs: impl Iterator<Item = (&'static str, u64)>) -> String {
    let mut by: Vec<(&str, u64)> = Vec::new();
    for (b, n) in pairs.filter(|(_, n)| *n > 0) {
        match by.iter_mut().find(|(k, _)| *k == b) {
            Some((_, total)) => *total += n,
            None => by.push((b, n)),
        }
    }
    let order = ["pass", "fail", "pending", "cancel", "skipping"];
    by.sort_by_key(|(b, _)| order.iter().position(|o| o == b).unwrap_or(9));
    if by.is_empty() {
        return "none".into();
    }
    by.iter()
        .map(|(b, n)| format!("{n} {b}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The answer from what GitHub said of the PR.
pub fn answer_from(pr: &Value) -> Answer {
    let text = |k: &str| pr.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut lines = vec![
        Line::from(Span::styled(
            format!(
                "#{} {}",
                pr.get("number").map(util::js_string).unwrap_or_default(),
                text("title")
            ),
            bold(Style::new()),
        )),
        Line::from(Span::styled(text("url"), dim())),
        Line::default(),
    ];
    let said = |lines: &mut Vec<Line<'static>>, parts: Vec<(String, Style)>| {
        lines.push(Line::from(
            parts
                .into_iter()
                .map(|(t, s)| Span::styled(t, s))
                .collect::<Vec<_>>(),
        ));
    };
    match text("state").as_str() {
        "MERGED" => {
            said(&mut lines, vec![("Already merged.".into(), bold(green()))]);
            return Answer {
                lines,
                choices: Vec::new(),
            };
        }
        "CLOSED" => {
            said(
                &mut lines,
                vec![("Closed without merging.".into(), bold(red()))],
            );
            return Answer {
                lines,
                choices: Vec::new(),
            };
        }
        _ => {}
    }
    if let Some(entry) = pr.get("mergeQueueEntry").filter(|e| e.is_object()) {
        let state = entry.get("state").and_then(Value::as_str).unwrap_or("");
        let position = entry
            .get("position")
            .map(util::js_string)
            .unwrap_or_default();
        let at = format!(" at position {position}");
        let verdict = match state {
            "AWAITING_CHECKS" => vec![
                ("In the merge queue".into(), bold(green())),
                (format!("{at}, waiting on the queue's checks."), green()),
            ],
            "MERGEABLE" => vec![
                ("In the merge queue".into(), bold(green())),
                (format!("{at}, passed and waiting its turn."), green()),
            ],
            "LOCKED" => vec![
                ("In the merge queue".into(), bold(green())),
                (format!("{at}, merging now."), green()),
            ],
            "UNMERGEABLE" => vec![
                (
                    format!("In the merge queue{at}, but it cannot merge"),
                    bold(red()),
                ),
                (": it will be removed unless that changes.".into(), red()),
            ],
            other => vec![
                ("In the merge queue".into(), bold(green())),
                (
                    format!("{at} ({}).", other.to_lowercase().replace('_', " ")),
                    green(),
                ),
            ],
        };
        said(&mut lines, verdict);
        return Answer {
            lines,
            choices: Vec::new(),
        };
    }
    let merge_state = text("mergeStateStatus");
    let verdict: Vec<(String, Style)> = match merge_state.as_str() {
        "CLEAN" => vec![("Ready to merge.".into(), bold(green()))],
        "HAS_HOOKS" => vec![
            ("Ready to merge".into(), bold(green())),
            (" (the repo runs merge hooks).".into(), green()),
        ],
        "UNSTABLE" => vec![
            ("Ready to merge".into(), bold(green())),
            (
                ", though some optional checks are not passing.".into(),
                yellow(),
            ),
        ],
        "BEHIND" => vec![
            ("Not ready:".into(), bold(yellow())),
            (
                " the branch is behind its base and must be updated.".into(),
                yellow(),
            ),
        ],
        "BLOCKED" => vec![
            ("Not ready:".into(), bold(red())),
            (" blocked by a required review or check.".into(), red()),
        ],
        "DIRTY" => vec![
            ("Not ready:".into(), bold(red())),
            (" it has merge conflicts.".into(), red()),
        ],
        "DRAFT" => vec![
            ("Not ready:".into(), bold(yellow())),
            (" it is still a draft.".into(), yellow()),
        ],
        _ => vec![
            (
                "GitHub has not worked out whether it can merge yet.".into(),
                bold(yellow()),
            ),
            (" Try again in a moment.".into(), dim()),
        ],
    };
    said(&mut lines, verdict);
    lines.push(Line::default());
    let contexts = pr
        .pointer("/commits/nodes/0/commit/statusCheckRollup/contexts")
        .cloned()
        .unwrap_or(Value::Null);
    let state_counts = |key: &str| -> Vec<(&'static str, u64)> {
        contexts
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|c| {
                (
                    bucket(c.get("state").and_then(Value::as_str).unwrap_or("")),
                    c.get("count").and_then(Value::as_u64).unwrap_or(0),
                )
            })
            .collect()
    };
    let all = counts(
        state_counts("checkRunCountsByState")
            .into_iter()
            .chain(state_counts("statusContextCountsByState")),
    );
    let nodes: Vec<Value> = contexts
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let required: (String, Style) =
        if matches!(merge_state.as_str(), "CLEAN" | "HAS_HOOKS" | "UNSTABLE") {
            ("all passing".into(), green())
        } else {
            let shown = counts(
                nodes
                    .iter()
                    .filter(|n| n.get("isRequired").and_then(Value::as_bool) == Some(true))
                    .map(|n| (check_bucket(n), 1)),
            );
            (shown, Style::new())
        };
    let review = match text("reviewDecision").as_str() {
        "APPROVED" => ("approved".to_string(), green()),
        "CHANGES_REQUESTED" => ("changes requested".into(), red()),
        "REVIEW_REQUIRED" => ("review required".into(), yellow()),
        _ => ("none required".into(), dim()),
    };
    let mergeable = text("mergeable");
    let conflicts = match mergeable.as_str() {
        "MERGEABLE" => ("none".to_string(), green()),
        "CONFLICTING" => ("conflicts with the base".into(), red()),
        _ => ("unknown".into(), dim()),
    };
    let draft = if pr.get("isDraft").and_then(Value::as_bool) == Some(true) {
        ("yes".to_string(), yellow())
    } else {
        ("no".into(), Style::new())
    };
    let auto = if pr.get("autoMergeRequest").is_some_and(|a| !a.is_null()) {
        ("on".to_string(), green())
    } else {
        ("off".into(), Style::new())
    };
    for (label, (value, style)) in [
        ("Required checks", required),
        ("All checks", (all.clone(), Style::new())),
        ("Review", review),
        ("Conflicts", conflicts),
        ("Draft", draft),
        ("Auto-merge", auto),
    ] {
        said(
            &mut lines,
            vec![(format!("{label:<17} "), Style::new()), (value, style)],
        );
    }
    let mut choices = Vec::new();
    if all.contains("fail") {
        choices.push(Choice::SeeFailing);
    }
    if mergeable == "CONFLICTING" || merge_state == "DIRTY" {
        choices.push(Choice::ResolveConflicts);
    }
    Answer { lines, choices }
}

/// The PR for `dir`'s branch as M-m's query gives it, asked again while
/// GitHub hasn't worked out whether it can merge.
async fn pr_state(dir: &Path) -> Result<(github::Repo, Value), String> {
    let found = github::pr_for_branch(dir, SOURCE)
        .await
        .map_err(|e| e.to_string())?;
    let Some(pr) = found else {
        return Err("No pull request for this branch.".into());
    };
    let repo = github::repo_of(dir).await.map_err(|e| e.to_string())?;
    let mut answer = Value::Null;
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let data = github::graphql(
            QUERY,
            json!({ "owner": repo.owner, "name": repo.name, "number": pr.number }),
            SOURCE,
        )
        .await
        .map_err(|e| e.to_string())?;
        answer = data
            .pointer("/repository/pullRequest")
            .cloned()
            .unwrap_or(Value::Null);
        if answer.get("mergeable").and_then(Value::as_str) != Some("UNKNOWN") {
            break;
        }
    }
    Ok((repo, answer))
}

/// M-m's answer for `dir`, or why there is none.
pub async fn check(dir: PathBuf) -> Result<Answer, String> {
    let (_, pr) = pr_state(&dir).await?;
    Ok(answer_from(&pr))
}

static JOB: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/actions/runs/\d+/job/(\d+)").unwrap());
static STAMP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{4}-\d\d-\d\dT[0-9:.]+Z ").unwrap());

/// The failing checks as Markdown: each with its description and link, and
/// a GitHub Actions job's last 25 lines of log, as pr-ready.sh's
/// `failing` showed them in less.
pub async fn failing_markdown(dir: &Path) -> Result<String, String> {
    let (repo, pr) = pr_state(dir).await?;
    let nodes: Vec<Value> = pr
        .pointer("/commits/nodes/0/commit/statusCheckRollup/contexts/nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = format!(
        "# Failing checks of #{}\n\n",
        pr.get("number").map(util::js_string).unwrap_or_default()
    );
    let mut any = false;
    for node in nodes.iter().filter(|n| check_bucket(n) == "fail") {
        any = true;
        let text = |k: &str| {
            node.get(k)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let name = if text("name").is_empty() {
            text("context")
        } else {
            text("name")
        };
        let link = if text("detailsUrl").is_empty() {
            text("targetUrl")
        } else {
            text("detailsUrl")
        };
        out.push_str(&format!("## {name}\n\n"));
        if !text("description").is_empty() {
            out.push_str(&format!("{}\n\n", text("description")));
        }
        if !link.is_empty() {
            out.push_str(&format!("{link}\n\n"));
        }
        if let Some(job) = JOB.captures(&link).map(|c| c[1].to_string()) {
            let log = github::rest(
                "GET",
                &format!("/repos/{repo}/actions/jobs/{job}/logs"),
                None,
                "M-m failing log",
            )
            .await;
            if let Ok(Value::String(log)) = log {
                let lines: Vec<&str> = log.lines().collect();
                let tail: Vec<String> = lines[lines.len().saturating_sub(25)..]
                    .iter()
                    .map(|l| STAMP.replace(l, "").into_owned())
                    .collect();
                out.push_str(&format!("```\n{}\n```\n\n", tail.join("\n")));
            }
        }
    }
    if !any {
        out.push_str("_No check is failing now._\n");
    }
    Ok(out)
}

/// `switchboard pr-view --checks [dir]`: the failing checks, paged.
pub fn checks_main(dir: PathBuf) -> ExitCode {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(match util::block_on(failing_markdown(&dir)) {
            Ok(text) => Content::Markdown {
                text,
                place: super::pr::PLACE.into(),
            },
            Err(why) => Content::Failed(why),
        });
    });
    pager::run(
        Content::Loading("Reading the checks...".into()),
        Some(rx),
        None,
    )
}

// ── The box ─────────────────────────────────────────────────────────────────

/// M-m's box: "Reaching out to GitHub..." until the answer comes, then the
/// answer and what can be done about it, moved through with j and k (or
/// the arrows, Tab, the wheel) and taken with Enter or a click; with
/// nothing to do, any key closes it, as pr-ready.sh's popup did.
pub struct ReadyBox {
    answer: Option<Result<Answer, String>>,
    cursor: usize,
    hover: Option<usize>,
    drawn: Option<Drawn>,
}

impl Default for ReadyBox {
    fn default() -> ReadyBox {
        ReadyBox::new()
    }
}

impl ReadyBox {
    pub fn new() -> ReadyBox {
        ReadyBox {
            answer: None,
            cursor: 0,
            hover: None,
            drawn: None,
        }
    }

    /// The answer has come.
    pub fn answered(&mut self, answer: Result<Answer, String>) {
        self.answer = Some(answer);
        self.cursor = 0;
    }

    fn choices(&self) -> Vec<Choice> {
        match &self.answer {
            Some(Ok(a)) if !a.choices.is_empty() => {
                let mut c = a.choices.clone();
                c.push(Choice::Close);
                c
            }
            _ => Vec::new(),
        }
    }

    /// The index of the first option's line among the box's lines.
    fn first_option(&self) -> usize {
        match &self.answer {
            Some(Ok(a)) => a.lines.len() + 1,
            _ => 0,
        }
    }
}

impl Overlay for ReadyBox {
    fn kind(&self) -> &'static str {
        "ready"
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let mut lines: Vec<BoxLine> = Vec::new();
        match &self.answer {
            None => lines.push(BoxLine::text("Reaching out to GitHub...", look.dim())),
            Some(Err(why)) => {
                for l in draw::wrap(why, 70) {
                    lines.push(BoxLine::text(l, look.text()));
                }
            }
            Some(Ok(answer)) => {
                for l in &answer.lines {
                    lines.push(BoxLine::spans(l.spans.clone()));
                }
            }
        }
        let choices = self.choices();
        if !choices.is_empty() {
            lines.push(BoxLine::blank());
            for (i, c) in choices.iter().enumerate() {
                let chosen = i == self.cursor;
                let mut spans = if chosen {
                    vec![Span::raw(" "), look.pointer(), Span::raw(" ")]
                } else {
                    vec![Span::raw("   ")]
                };
                spans.push(Span::styled(
                    c.label(),
                    if chosen {
                        look.bold_bright()
                    } else {
                        look.text()
                    },
                ));
                let fill = if chosen {
                    Some(look.theme.highlight)
                } else if self.hover == Some(i) {
                    Some(look.theme.panel)
                } else {
                    None
                };
                lines.push(BoxLine::spans(spans).fill(fill).clickable());
            }
        }
        let keys = if choices.is_empty() {
            if self.answer.is_none() {
                "esc close"
            } else {
                "any key closes"
            }
        } else {
            "j k move  ↵ choose  esc close"
        };
        let mut spec = BoxSpec::new("Ready to merge?", &lines);
        spec.keys = Some(keys);
        spec.width = 60;
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn key(&mut self, key: Key, _look: &Look) -> Step {
        let choices = self.choices();
        if self.answer.is_none() {
            return match key.code {
                KeyCode::Esc | KeyCode::Char('q') => Step::Close(Outcome::Dismissed),
                KeyCode::Char('c') if key.mods == Mods::CTRL => Step::Close(Outcome::Dismissed),
                _ => Step::Same,
            };
        }
        if choices.is_empty() {
            return Step::Close(Outcome::Dismissed);
        }
        let last = choices.len() - 1;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => Step::Close(Outcome::Dismissed),
            KeyCode::Char('j') | KeyCode::Down => {
                self.cursor = (self.cursor + 1).min(last);
                Step::Redraw
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.cursor = self.cursor.saturating_sub(1);
                Step::Redraw
            }
            // Tab and Shift-Tab go round, as menu.sh's stops did.
            KeyCode::Tab => {
                self.cursor = if self.cursor >= last {
                    0
                } else {
                    self.cursor + 1
                };
                Step::Redraw
            }
            KeyCode::BackTab => {
                self.cursor = if self.cursor == 0 {
                    last
                } else {
                    self.cursor - 1
                };
                Step::Redraw
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                Step::Close(Outcome::Chose(choices[self.cursor] as usize))
            }
            _ => Step::Same,
        }
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        let Some(drawn) = &self.drawn else {
            return Step::Same;
        };
        let hit = drawn.hit(mouse.col, mouse.row);
        let choices = self.choices();
        let first = self.first_option();
        let option = match hit {
            Hit::Line(i, _) if i >= first && i - first < choices.len() => Some(i - first),
            _ => None,
        };
        match draw::gesture(&mouse) {
            Gesture::Click if look.mouse => {
                if hit == Hit::Outside || hit == Hit::Close {
                    return Step::Close(Outcome::Dismissed);
                }
                match option {
                    Some(i) => Step::Close(Outcome::Chose(choices[i] as usize)),
                    None if choices.is_empty() && self.answer.is_some() => {
                        Step::Close(Outcome::Dismissed)
                    }
                    None => Step::Same,
                }
            }
            Gesture::WheelDown if !choices.is_empty() => {
                self.cursor = (self.cursor + 1).min(choices.len() - 1);
                Step::Redraw
            }
            Gesture::WheelUp if !choices.is_empty() => {
                self.cursor = self.cursor.saturating_sub(1);
                Step::Redraw
            }
            Gesture::Move if look.hover => {
                if option == self.hover {
                    Step::Same
                } else {
                    self.hover = option;
                    if let Some(i) = option {
                        self.cursor = i;
                    }
                    Step::Redraw
                }
            }
            _ => Step::Same,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(state: &str, merge_state: &str, mergeable: &str) -> Value {
        json!({
            "number": 12, "title": "Fix the login", "url": "https://github.com/acme/w/pull/12",
            "state": state, "isDraft": false, "mergeable": mergeable, "mergeStateStatus": merge_state,
            "reviewDecision": "REVIEW_REQUIRED", "autoMergeRequest": null, "mergeQueueEntry": null,
            "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "contexts": {
                "checkRunCountsByState": [{ "state": "SUCCESS", "count": 3 }, { "state": "FAILURE", "count": 1 }, { "state": "QUEUED", "count": 0 }],
                "statusContextCountsByState": [{ "state": "PENDING", "count": 1 }],
                "nodes": [
                    { "__typename": "CheckRun", "name": "test", "status": "COMPLETED", "conclusion": "FAILURE", "isRequired": true },
                    { "__typename": "CheckRun", "name": "lint", "status": "COMPLETED", "conclusion": "SUCCESS", "isRequired": true },
                    { "__typename": "StatusContext", "context": "preview", "state": "PENDING", "isRequired": false }
                ]
            } } } }] }
        })
    }

    #[test]
    fn a_blocked_pr_says_why_and_offers_what_can_be_done() {
        let answer = answer_from(&pr("OPEN", "DIRTY", "CONFLICTING"));
        let text = answer.text();
        assert!(text.starts_with("#12 Fix the login\nhttps://github.com/acme/w/pull/12\n\nNot ready: it has merge conflicts."), "{text}");
        assert!(text.contains("Required checks   1 pass, 1 fail"), "{text}");
        assert!(
            text.contains("All checks        3 pass, 1 fail, 1 pending"),
            "{text}"
        );
        assert!(text.contains("Review            review required"), "{text}");
        assert!(
            text.contains("Conflicts         conflicts with the base"),
            "{text}"
        );
        assert_eq!(
            answer.choices,
            vec![Choice::SeeFailing, Choice::ResolveConflicts]
        );
    }

    #[test]
    fn a_ready_pr_needs_no_required_count_and_merged_or_queued_ones_stop_early() {
        let answer = answer_from(&pr("OPEN", "CLEAN", "MERGEABLE"));
        assert!(answer.text().contains("Ready to merge.\n"));
        assert!(answer.text().contains("Required checks   all passing"));
        let merged = answer_from(&pr("MERGED", "CLEAN", "MERGEABLE"));
        assert!(merged.text().ends_with("Already merged."));
        assert!(merged.choices.is_empty());
        let mut queued = pr("OPEN", "CLEAN", "MERGEABLE");
        queued["mergeQueueEntry"] = json!({ "state": "AWAITING_CHECKS", "position": 2 });
        assert!(
            answer_from(&queued)
                .text()
                .ends_with("In the merge queue at position 2, waiting on the queue's checks.")
        );
    }

    #[test]
    fn the_box_takes_a_choice_and_closes_on_any_key_with_none() {
        let mut b = ReadyBox::new();
        let look = Look::default();
        assert_eq!(
            b.key(Key::new(KeyCode::Char('x'), Mods::NONE), &look),
            Step::Same,
            "still asking"
        );
        b.answered(Ok(answer_from(&pr("OPEN", "DIRTY", "CONFLICTING"))));
        assert_eq!(
            b.key(Key::new(KeyCode::Char('j'), Mods::NONE), &look),
            Step::Redraw
        );
        assert_eq!(
            b.key(Key::new(KeyCode::Enter, Mods::NONE), &look),
            Step::Close(Outcome::Chose(Choice::ResolveConflicts as usize))
        );
        let mut done = ReadyBox::new();
        done.answered(Err("No pull request for this branch.".into()));
        assert_eq!(
            done.key(Key::new(KeyCode::Char('x'), Mods::NONE), &look),
            Step::Close(Outcome::Dismissed)
        );
    }
}
