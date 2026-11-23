//! A tab's own views on the phone (boards 189 and 190): its PR, its ticket
//! and its note, read only, as the PR view (M-v), the ticket view (M-i) and
//! M-e show them, from what the workspace already holds: the lookups'
//! facts of the tab's PR and the copy the PR view last read, the Tickets
//! view's Linear tickets and the copy the ticket view last read, and the
//! note's file. Nothing here asks GitHub or Linear. The rows the panel's
//! list gives the tab's section, and what each view's route answers, are
//! built here as plain data; `phone::serve` puts them on the server. New
//! with the tab's views on the phone; no tmux-version file is behind it.

use serde_json::{Value, json};

use crate::facts::{Merge, PrFacts, PrState};
use crate::ui::kit::{Mark, Tone};
use crate::views::ticket::Ticket;

use super::drawer::tone_name;

/// What a tab can show on the phone, in the order its panel opens them,
/// `tab_opens_beside`'s.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Pr,
    Ticket,
    Note,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Pr => "pr",
            Kind::Ticket => "ticket",
            Kind::Note => "note",
        }
    }

    pub fn of(name: &str) -> Option<Kind> {
        [Kind::Pr, Kind::Ticket, Kind::Note]
            .into_iter()
            .find(|k| k.name() == name)
    }
}

/// A fact shown as a chip, in its tone.
fn chip(text: &str, tone: Tone) -> Value {
    json!({ "text": text, "tone": tone_name(tone) })
}

/// A row of the panel's list in the tab's section: its kind, its name,
/// a line of what it holds and its mark.
fn row(kind: Kind, title: String, line: String, mark: Mark) -> Value {
    let (glyph, tone) = mark.look();
    json!({
        "kind": kind.name(),
        "title": title,
        "line": line,
        "mark": glyph,
        "tone": tone_name(tone),
    })
}

/// A PR's state as its chip says it: merged, closed, a draft or open.
fn pr_state(state: PrState, draft: bool) -> (&'static str, Tone) {
    match state {
        PrState::Merged => ("merged", Tone::Landing),
        PrState::Closed => ("closed", Tone::Failed),
        PrState::Open if draft => ("draft", Tone::Dim),
        PrState::Open => ("open", Tone::Done),
    }
}

/// GitHub's review decision in words, none while there is none.
fn review(decision: Option<&str>) -> Option<(&'static str, Tone)> {
    match decision? {
        "CHANGES_REQUESTED" => Some(("changes asked", Tone::Failed)),
        "APPROVED" => Some(("approved", Tone::Done)),
        "REVIEW_REQUIRED" => Some(("review needed", Tone::Needs)),
        _ => None,
    }
}

/// The PR's state as the view and the row read it: from the lookups'
/// facts, else from the copy the PR view last read.
struct PrRead {
    title: String,
    url: String,
    state: PrState,
    draft: bool,
    queued: bool,
    review: Option<String>,
    /// Passed, failing and running, counted.
    checks: Option<(u64, u64, u64)>,
}

fn read_pr(facts: Option<&PrFacts>, copy: Option<&Value>) -> PrRead {
    if let Some(f) = facts {
        return PrRead {
            title: f.title.clone(),
            url: f.url.clone(),
            state: f.state,
            draft: f.draft,
            queued: matches!(f.merge, Some(Merge::Queued { .. })),
            review: f.review_decision.clone(),
            checks: f
                .checks
                .map(|c| (c.pass.into(), c.fail.into(), c.pending.into())),
        };
    }
    let text = |k: &str| {
        copy.and_then(|c| c.get(k))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let listed = copy
        .map(crate::views::pr::checks_listed)
        .unwrap_or_default();
    let count = |m: Mark| listed.iter().filter(|(k, _)| *k == m).count() as u64;
    PrRead {
        title: text("title"),
        url: text("url"),
        state: match text("state").as_str() {
            "MERGED" => PrState::Merged,
            "CLOSED" => PrState::Closed,
            _ => PrState::Open,
        },
        draft: copy.and_then(|c| c.get("isDraft")).and_then(Value::as_bool) == Some(true),
        queued: copy
            .and_then(|c| c.get("mergeQueueEntry"))
            .is_some_and(Value::is_object),
        review: Some(text("reviewDecision")).filter(|r| !r.is_empty()),
        checks: (!listed.is_empty()).then(|| {
            (
                count(Mark::Passed),
                count(Mark::Failed),
                count(Mark::Pending),
            )
        }),
    }
}

/// The checks summed up: how many failing or running of how many, in
/// their tone; none with no checks.
fn checks_said(checks: Option<(u64, u64, u64)>) -> Option<(String, Tone)> {
    let (pass, fail, pending) = checks?;
    let total = pass + fail + pending;
    if total == 0 {
        return None;
    }
    Some(if fail > 0 {
        (format!("{fail} of {total} failing"), Tone::Failed)
    } else if pending > 0 {
        (format!("{pending} of {total} running"), Tone::Working)
    } else {
        (format!("{total} passed"), Tone::Done)
    })
}

/// The tab's PR as a row of the panel's list: `PR #41`, what asks for you
/// (changes asked, checks failing or running), else its state.
pub fn pr_row(number: u64, facts: Option<&PrFacts>) -> Value {
    let pr = read_pr(facts, None);
    let mut line = Vec::new();
    if let Some((words, _)) = review(pr.review.as_deref()) {
        line.push(words.to_string());
    }
    let (_, fail, pending) = pr.checks.unwrap_or_default();
    if fail > 0 {
        line.push(format!("{fail} failing"));
    } else if pending > 0 {
        line.push(format!("{pending} running"));
    }
    if pr.queued {
        line.push("in the merge queue".into());
    }
    if line.is_empty() && facts.is_some() {
        line.push(pr_state(pr.state, pr.draft).0.to_string());
    }
    let changes = pr.review.as_deref() == Some("CHANGES_REQUESTED");
    let mark = match pr.state {
        PrState::Merged => Mark::Passed,
        PrState::Closed => Mark::Idle,
        PrState::Open if fail > 0 || changes => Mark::Failed,
        PrState::Open if pr.queued => Mark::Queued,
        PrState::Open if pending > 0 => Mark::Working,
        PrState::Open if pr.checks.is_some() => Mark::Passed,
        PrState::Open => Mark::Idle,
    };
    row(Kind::Pr, format!("PR #{number}"), line.join(" · "), mark)
}

/// The tab's PR as its view on the phone shows it, board 189's first
/// phone: its number, title, state and review as chips, its checks, the
/// failing and running ones by name, and its description. `copy` is the
/// PR view's last copy, used only when it is of this PR; `body` the
/// description the PR board last read, for when it isn't.
pub fn pr_json(
    number: u64,
    facts: Option<&PrFacts>,
    copy: Option<&Value>,
    body: Option<&str>,
) -> Value {
    let copy = copy.filter(|c| c.get("number").and_then(Value::as_u64) == Some(number));
    let pr = read_pr(facts, copy);
    // Its state only when something read it: a PR the lookups haven't
    // answered for, and the PR view hasn't read, may be merged or closed.
    let mut chips = Vec::new();
    if facts.is_some() || copy.is_some() {
        let (state, tone) = pr_state(pr.state, pr.draft);
        chips.push(chip(state, tone));
    }
    if pr.queued {
        chips.push(chip("in the merge queue", Tone::Landing));
    }
    if let Some((words, tone)) = review(pr.review.as_deref()) {
        chips.push(chip(words, tone));
    }
    // By name the failing and running ones, the passed ones on one line,
    // from the PR view's copy while it counts them as the lookups do: an
    // older copy would name none of the ones failing now.
    let listed = copy
        .map(crate::views::pr::checks_listed)
        .unwrap_or_default();
    let count = |m: Mark| listed.iter().filter(|(k, _)| *k == m).count() as u64;
    let agrees = pr.checks.is_none_or(|(pass, fail, pending)| {
        (
            count(Mark::Passed),
            count(Mark::Failed),
            count(Mark::Pending),
        ) == (pass, fail, pending)
    });
    let listed = if agrees { listed } else { Vec::new() };
    let mut rows: Vec<Value> = listed
        .iter()
        .filter(|(m, _)| *m != Mark::Passed)
        .map(|(m, name)| {
            let (glyph, tone) = m.look();
            json!({ "mark": glyph, "tone": tone_name(tone), "name": name })
        })
        .collect();
    let passed: Vec<&str> = listed
        .iter()
        .filter(|(m, _)| *m == Mark::Passed)
        .map(|(_, n)| n.as_str())
        .collect();
    if let Some(first) = passed.first() {
        let name = match passed.len() {
            1 => first.to_string(),
            n => format!("{first} and {} more", n - 1),
        };
        let (glyph, tone) = Mark::Passed.look();
        rows.push(json!({ "mark": glyph, "tone": tone_name(tone), "name": name }));
    }
    let said = checks_said(pr.checks);
    let description = copy
        .and_then(|c| c.get("body"))
        .and_then(Value::as_str)
        .or(body);
    json!({
        "heading": format!("PR #{number}"),
        "number": number,
        "title": pr.title,
        "url": pr.url,
        "chips": chips,
        "checks": said.map(|(words, tone)| json!({
            "said": words,
            "tone": tone_name(tone),
            "rows": rows,
        })),
        "body": description,
    })
}

/// A Linear ticket's state in words and its mark, from its `state`.
fn linear_state(issue: &Value) -> Option<(String, Mark)> {
    let name = issue.pointer("/state/name").and_then(Value::as_str)?;
    let mark = match issue.pointer("/state/type").and_then(Value::as_str) {
        Some("completed") => Mark::Passed,
        Some("canceled") => Mark::Failed,
        Some("started") => Mark::Working,
        _ => Mark::Comment,
    };
    Some((name.to_string(), mark))
}

/// A GitHub issue's state in words and its mark: open, closed as done or
/// as not planned, as the ticket view says it.
fn issue_state(issue: &Value) -> Option<(String, Mark)> {
    let state = issue.get("state").and_then(Value::as_str)?;
    let reason = issue.get("state_reason").and_then(Value::as_str);
    Some(match (state, reason) {
        ("open", _) => ("open".into(), Mark::Comment),
        (_, Some("not_planned")) => ("closed: not planned".into(), Mark::Failed),
        _ => ("closed".into(), Mark::Passed),
    })
}

/// The tab's ticket as a row of the panel's list: `Ticket #163` or
/// `Ticket ENG-42`, its state when the Tickets view's lookup holds it.
pub fn ticket_row(short: &str, held: Option<&Value>) -> Value {
    let (line, mark) = held
        .and_then(linear_state)
        .unwrap_or((String::new(), Mark::Comment));
    row(Kind::Ticket, format!("Ticket {short}"), line, mark)
}

/// The tab's ticket as its view on the phone shows it: its name, title,
/// state as a chip and its text, from `copy`, the ticket held or last
/// read for its view (a Linear ticket's fields, or a GitHub issue's);
/// only its name and link while there is none.
pub fn ticket_json(ticket: &Ticket, copy: Option<&Value>) -> Value {
    let text = |k: &str| {
        copy.and_then(|c| c.get(k))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let (heading, state, body, url) = match ticket {
        Ticket::Issue { number, .. } => (
            format!("Issue #{number}"),
            copy.and_then(issue_state),
            text("body"),
            text("html_url").or_else(|| ticket.url()),
        ),
        // A project's copy is the project view's: its name and status.
        Ticket::Project(_) => (
            ticket.shown(),
            copy.and_then(|c| linear_state(&json!({ "state": c.get("status") }))),
            text("content").or_else(|| text("description")),
            text("url"),
        ),
        _ => (
            ticket.shown(),
            copy.and_then(linear_state),
            text("description"),
            text("url"),
        ),
    };
    json!({
        "heading": heading,
        "title": text("title").or_else(|| text("name")).unwrap_or_default(),
        "url": url,
        "chips": state.map(|(words, mark)| vec![chip(&words, mark.look().1)]).unwrap_or_default(),
        "body": body,
        "read": copy.is_some(),
    })
}

/// A note's title: its file's name without `.md`, as Notes lists it.
pub fn note_title(note: &str) -> String {
    let name = note.rsplit('/').next().unwrap_or(note);
    name.strip_suffix(".md").unwrap_or(name).to_string()
}

/// The tab's note as a row of the panel's list: `Note` and its title.
pub fn note_row(note: &str) -> Value {
    row(Kind::Note, "Note".into(), note_title(note), Mark::Note)
}

/// The tab's note as its view on the phone shows it: its title and its
/// Markdown without its ID line (`id::strip`), each checklist's boxes
/// drawn as Notes draws them.
pub fn note_json(note: &str, text: &str) -> Value {
    let text = crate::notes::id::strip(text);
    let text: Vec<String> = text
        .split('\n')
        .map(|line| {
            let indent = line.len() - line.trim_start().len();
            let (lead, rest) = line.split_at(indent);
            for (box_, drawn) in [("- [ ] ", "- ☐ "), ("- [x] ", "- ☑ "), ("- [X] ", "- ☑ ")]
            {
                if let Some(item) = rest.strip_prefix(box_) {
                    return format!("{lead}{drawn}{item}");
                }
            }
            line.to_string()
        })
        .collect();
    json!({
        "heading": note_title(note),
        "title": note_title(note),
        "body": text.join("\n"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::Checks;
    use crate::integrations::github::Repo;

    fn facts() -> PrFacts {
        PrFacts {
            number: 41,
            title: "Time the redraw".into(),
            head: "redraw".into(),
            draft: false,
            repo: "acme/notes".into(),
            url: "https://github.com/acme/notes/pull/41".into(),
            state: PrState::Open,
            merge: None,
            checks: Some(Checks {
                pass: 2,
                fail: 1,
                pending: 0,
            }),
            review_decision: Some("CHANGES_REQUESTED".into()),
            additions: 1,
            deletions: 1,
        }
    }

    fn check(name: &str, conclusion: &str) -> Value {
        json!({ "__typename": "CheckRun", "name": name, "status": "COMPLETED", "conclusion": conclusion })
    }

    #[test]
    fn a_prs_row_says_what_asks_for_you() {
        let r = pr_row(41, Some(&facts()));
        assert_eq!(r["title"], "PR #41");
        assert_eq!(r["line"], "changes asked · 1 failing");
        assert_eq!(r["tone"], "failed");
        let mut merged = facts();
        merged.state = PrState::Merged;
        merged.review_decision = None;
        merged.checks = None;
        assert_eq!(pr_row(41, Some(&merged))["line"], "merged");
        assert_eq!(pr_row(41, None)["line"], "");
    }

    #[test]
    fn a_prs_view_takes_the_views_copy_only_when_it_is_of_that_pr() {
        let copy = json!({
            "number": 41,
            "body": "Time it on both fixtures.",
            "last": { "nodes": [{ "commit": { "statusCheckRollup": { "contexts": { "nodes": [
                check("Formatting", "SUCCESS"), check("Clippy", "FAILURE"), check("Docs", "SUCCESS"),
            ] } } } }] },
        });
        let v = pr_json(41, Some(&facts()), Some(&copy), Some("the board's"));
        assert_eq!(v["title"], "Time the redraw");
        assert_eq!(v["body"], "Time it on both fixtures.");
        assert_eq!(v["checks"]["said"], "1 of 3 failing");
        let rows: Vec<&str> = v["checks"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["name"].as_str().unwrap())
            .collect();
        assert_eq!(rows, ["Clippy", "Formatting and 1 more"]);
        let chips: Vec<&str> = v["chips"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["text"].as_str().unwrap())
            .collect();
        assert_eq!(chips, ["open", "changes asked"]);
        // A copy counting otherwise than the lookups names none of them.
        let mut newer = facts();
        newer.checks = Some(Checks {
            pass: 1,
            fail: 2,
            pending: 0,
        });
        let v = pr_json(41, Some(&newer), Some(&copy), None);
        assert_eq!(
            (v["checks"]["said"].as_str(), v["checks"]["rows"].clone()),
            (Some("2 of 3 failing"), json!([]))
        );
        // Another PR's copy is left out, the board's description taken.
        let v = pr_json(42, Some(&facts()), Some(&copy), Some("the board's"));
        assert_eq!(v["body"], "the board's");
        assert_eq!(v["checks"]["rows"], json!([]));
    }

    #[test]
    fn a_tickets_view_reads_an_issue_or_a_linear_ticket() {
        let issue = Ticket::Issue {
            repo: Repo {
                owner: "acme".into(),
                name: "notes".into(),
            },
            number: 163,
        };
        let v = ticket_json(
            &issue,
            Some(&json!({ "title": "Time it", "body": "Both.", "state": "open" })),
        );
        assert_eq!(v["heading"], "Issue #163");
        assert_eq!(v["body"], "Both.");
        assert_eq!(v["chips"][0]["text"], "open");
        assert_eq!(v["url"], "https://github.com/acme/notes/issues/163");
        let linear = Ticket::Linear("ENG-42".into());
        let held = json!({ "identifier": "ENG-42", "title": "Fix", "description": "Why.", "state": { "name": "In Progress", "type": "started" } });
        let v = ticket_json(&linear, Some(&held));
        assert_eq!(
            (v["heading"].as_str(), v["body"].as_str()),
            (Some("ENG-42"), Some("Why."))
        );
        assert_eq!(v["chips"][0]["tone"], "working");
        assert_eq!(ticket_row("ENG-42", Some(&held))["line"], "In Progress");
        assert_eq!(ticket_json(&linear, None)["body"], Value::Null);
        assert_eq!(ticket_json(&linear, None)["read"], false);
        assert_eq!(pr_json(41, None, None, None)["chips"], json!([]));
    }

    #[test]
    fn a_notes_view_leaves_its_id_out_and_draws_its_boxes() {
        let v = note_json(
            "plans/Plan - Redraw.md",
            "# Plan\n- [x] Read\n  - [ ] Time\n<!-- note-id: n-0123456789ab -->\n",
        );
        assert_eq!(v["title"], "Plan - Redraw");
        assert_eq!(v["body"], "# Plan\n- ☑ Read\n  - ☐ Time\n");
        assert_eq!(note_row("Plan - Redraw.md")["line"], "Plan - Redraw");
    }
}
