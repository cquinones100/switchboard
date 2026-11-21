//! A landing in its tab's thread, as UX 2.0 board 51 draws it: while a tab
//! holds the landing lock or waits for it, its thread's header says so in
//! place of its state ("🟣 Landing since 2:12 PM · 1st in the queue"), with
//! its PR's checks and review when it has one, and, once it holds the
//! lock, queue-and-clean's steps as the Landing view's cells (rebased,
//! tests, review, leak-check, push); and queue-and-clean's push question
//! is asked as two buttons, Yes, push it and No, where no answer is no.
//!
//! What it shows comes from the server's `list`: the tab's `landing` (its
//! place, since when, the branch and the landing's record, read from the
//! landing lock's files on the server's tick, src/sources/landing.rs), its
//! PR's facts, and the step queue-and-clean last named on its pane
//! (`land-lock phase`, its agent record's `phase`). No file is read here.

use serde_json::Value;

use super::post::Button;
use super::tabs::{Tab, hash_of};
use crate::facts::PrFacts;
use crate::mission::answer::Prompt;
use crate::sidebar::text::ordinal;

/// A tab's direct landing, from `list`'s `landing` and its pane's phase.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LandingAt {
    pub holding: bool,
    /// Its place in the queue, 1 holding the lock.
    pub line: u32,
    /// Since when it holds the lock or waits, in ms.
    pub since: i64,
    pub branch: String,
    /// Waiting first in line while its change is reviewed again.
    pub reviewing: bool,
    /// What the landing recorded of itself (`land-lock record`).
    pub record: Value,
    /// The step queue-and-clean last named (`land-lock phase`).
    pub phase: String,
}

/// `list`'s `landing` for a tab, or `None` when it isn't landing.
pub fn landing_of(value: Option<&Value>) -> Option<LandingAt> {
    let v = value.filter(|v| v.is_object())?;
    Some(LandingAt {
        holding: v["holding"].as_bool() == Some(true),
        line: v["line"].as_u64().unwrap_or(1) as u32,
        since: v["since"].as_i64().unwrap_or(0),
        branch: v["branch"].as_str().unwrap_or("").to_string(),
        reviewing: v["reviewing"].as_bool() == Some(true),
        record: v["record"].clone(),
        phase: String::new(),
    })
}

/// The header's lines for a landing tab, in place of its state's line: its
/// place, its PR, and its steps once it holds the lock.
pub fn header_lines(tab: &Tab, landing: &LandingAt) -> String {
    let since = if landing.since > 0 {
        format!(
            " since {}",
            super::homes::when(landing.since as f64 / 1000.0)
        )
    } else {
        String::new()
    };
    let what = if landing.holding {
        "Landing"
    } else if landing.reviewing {
        "Reviewing again"
    } else {
        "Waiting to land"
    };
    let mut out = format!(
        "🟣 {what}{since} · {} in the queue",
        ordinal(landing.line.max(1))
    );
    if let Some(pr) = &tab.pr {
        out.push('\n');
        out.push_str(&pr_line(pr));
    }
    if landing.holding {
        out.push('\n');
        out.push_str(&cells(landing, tab.state == "waiting"));
    }
    out
}

/// The header's notification line for a landing tab: its place, as the
/// header's first landing line says it without its time.
pub fn notice_words(landing: &LandingAt) -> String {
    let what = if landing.holding {
        "landing"
    } else if landing.reviewing {
        "reviewing again"
    } else {
        "waiting to land"
    };
    format!("{what}, {} in the queue", ordinal(landing.line.max(1)))
}

/// "PR #57 · ✓ 9 checks · approved", from what the server last heard of it.
pub fn pr_line(pr: &PrFacts) -> String {
    let mut parts = vec![format!("PR #{}", pr.number)];
    if let Some(c) = &pr.checks {
        let total = c.pass + c.fail + c.pending;
        if c.fail > 0 {
            parts.push(format!("✕ {} of {total} checks failing", c.fail));
        } else if c.pending > 0 {
            parts.push(format!("🔵 {} of {total} checks running", c.pending));
        } else if total > 0 {
            parts.push(format!("✓ {total} checks"));
        }
    }
    match pr.review_decision.as_deref() {
        Some("APPROVED") => parts.push("approved".into()),
        Some("CHANGES_REQUESTED") => parts.push("changes requested".into()),
        Some("REVIEW_REQUIRED") => parts.push("review required".into()),
        _ => {}
    }
    parts.join(" · ")
}

/// The steps of a landing that holds the lock, each with its mark: ✓ done,
/// 🔵 under way, 🟡 waiting on you, ✕ failed, ⚪ still to come. What the
/// record says of the tests, the review and the leak-check, and the phase
/// the step it is at; `waiting` is the tab waiting on you, which marks
/// the push waiting only at the push (`<lock> push` asking again), since
/// queue-and-clean also asks while holding the lock about a conflict.
pub fn cells(landing: &LandingAt, waiting: bool) -> String {
    let phase = landing.phase.as_str();
    let record = &landing.record;
    let number = |v: &Value, k: &str| v.get(k).and_then(Value::as_u64);
    let rebased = if phase.starts_with("rebasing") {
        "🔵 rebasing".to_string()
    } else if phase.starts_with("merge conflict") {
        "✕ rebase".into()
    } else {
        "✓ rebased".into()
    };
    let tests_failed = number(&record["tests"], "failed").unwrap_or(0) > 0
        || record["tests"]["ok"].as_bool() == Some(false);
    let tests = if phase.starts_with("tests failed") || phase.starts_with("check failed") {
        "✕ tests".to_string()
    } else if phase.contains("testing") || phase.starts_with("checking") {
        "🔵 tests".into()
    } else if tests_failed {
        "✕ tests".into()
    } else {
        "✓ tests".into()
    };
    let clean = number(&record["review"], "clean_rounds").unwrap_or(0);
    let review = if phase.starts_with("review blocked") {
        "✕ review".to_string()
    } else if phase.starts_with("reviewing") {
        "🔵 review".into()
    } else if clean >= 2 {
        format!("✓ review, {clean} rounds clean")
    } else if record["review"].is_object() {
        format!(
            "🔵 review, round {}",
            number(&record["review"], "round").unwrap_or(1)
        )
    } else {
        "✓ review".into()
    };
    let leak = if number(&record["leak"], "found").unwrap_or(0) > 0 {
        "✕ leak-check"
    } else {
        "✓ leak-check"
    };
    let pushed = phase.starts_with("applying") || phase.starts_with("cleaning");
    let push = if pushed {
        "✓ pushed"
    } else if phase.starts_with("asking") || (waiting && phase.starts_with("pushing")) {
        "🟡 push"
    } else if phase.starts_with("pushing") {
        "🔵 push"
    } else {
        "⚪ push"
    };
    [rebased.as_str(), &tests, &review, leak, push].join(" · ")
}

/// queue-and-clean's push question: its options **Push to <branch>** and
/// **Not yet**, with their numbers. `None` for any other prompt.
pub fn push_question(found: &Prompt) -> Option<PushQuestion> {
    if found.multi {
        return None;
    }
    let push = found
        .options
        .iter()
        .find(|o| o.label.trim().starts_with("Push to "))?;
    let no = found.options.iter().find(|o| o.label.trim() == "Not yet")?;
    Some(PushQuestion {
        asks: format!("{}?", push.label.trim()),
        yes: push.n,
        no: no.n,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct PushQuestion {
    /// "Push to main?"
    pub asks: String,
    pub yes: u32,
    pub no: u32,
}

/// What the push question's alert shows under its heading: Claude's
/// question, what the landing recorded of its review and leak-check, and
/// that no answer is no.
pub fn push_text(found: &Prompt, landing: Option<&LandingAt>) -> String {
    let mut out = found.question.trim().to_string();
    if let Some(l) = landing {
        let clean = l.record["review"]["clean_rounds"].as_u64().unwrap_or(0);
        let leak = l.record["leak"]["found"].as_u64();
        let mut said = Vec::new();
        match clean {
            0 | 1 => {}
            2 => said.push("Reviewed clean twice".to_string()),
            n => said.push(format!("Reviewed clean {n} times")),
        }
        match leak {
            Some(0) => said.push("leak-check found none".into()),
            Some(n) => said.push(format!("leak-check found {n}")),
            None => {}
        }
        if !said.is_empty() {
            out.push_str(&format!("\n{}.", said.join("; ")));
        }
    }
    out.push_str("\n_As the desktop's [Y/n]: no answer is no, so nothing pushes by itself._");
    out
}

/// Yes, push it and No, each pressing its option as `option_buttons`'
/// buttons do, so a press answers only this question in this wait.
pub fn push_buttons(tab: &Tab, found: &Prompt, q: &PushQuestion) -> Vec<Button> {
    let key = hash_of(found);
    let value =
        |n: u32| serde_json::json!({ "a": "opt", "w": tab.id, "n": n, "s": tab.since, "k": key });
    vec![
        Button::new("Yes, push it", value(q.yes)).style("primary"),
        Button::new("No", value(q.no)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{Checks, PrState};
    use crate::mission::answer::PromptOption;
    use serde_json::json;

    fn holding(phase: &str, record: Value) -> LandingAt {
        LandingAt {
            holding: true,
            line: 1,
            since: 0,
            branch: "main".into(),
            reviewing: false,
            record,
            phase: phase.into(),
        }
    }

    #[test]
    fn a_landing_s_cells_follow_its_record_and_phase() {
        let clean = json!({
            "tests": { "passed": 412, "failed": 0, "total": 412 },
            "review": { "round": 2, "clean_rounds": 2 },
            "leak": { "found": 0 },
        });
        assert_eq!(
            cells(&holding("pushing", clean.clone()), true),
            "✓ rebased · ✓ tests · ✓ review, 2 rounds clean · ✓ leak-check · 🟡 push"
        );
        assert!(
            cells(&holding("rebasing again", clean.clone()), true).ends_with("⚪ push"),
            "a question about a conflict isn't the push's"
        );
        assert_eq!(
            cells(&holding("rebasing again", clean.clone()), false),
            "🔵 rebasing · ✓ tests · ✓ review, 2 rounds clean · ✓ leak-check · ⚪ push"
        );
        assert_eq!(
            cells(&holding("tests failed: 2", clean.clone()), false),
            "✓ rebased · ✕ tests · ✓ review, 2 rounds clean · ✓ leak-check · ⚪ push"
        );
        assert_eq!(
            cells(&holding("cleaning up", clean), false),
            "✓ rebased · ✓ tests · ✓ review, 2 rounds clean · ✓ leak-check · ✓ pushed"
        );
    }

    #[test]
    fn a_pr_line_says_its_checks_and_review() {
        let pr = PrFacts {
            number: 57,
            repo: "o/r".into(),
            url: String::new(),
            state: PrState::Open,
            merge: None,
            checks: Some(Checks {
                pass: 9,
                fail: 0,
                pending: 0,
            }),
            review_decision: Some("APPROVED".into()),
            additions: 0,
            deletions: 0,
        };
        assert_eq!(pr_line(&pr), "PR #57 · ✓ 9 checks · approved");
    }

    #[test]
    fn only_queue_and_clean_s_push_question_is_one() {
        let prompt = |labels: &[&str]| Prompt {
            question: "Land `Add the harness's warm-up runs`?".into(),
            options: labels
                .iter()
                .enumerate()
                .map(|(i, l)| PromptOption {
                    n: i as u32 + 1,
                    label: l.to_string(),
                    checked: None,
                    detail: String::new(),
                })
                .collect(),
            multi: false,
            free: None,
            cursor: None,
            more: Vec::new(),
        };
        let q = push_question(&prompt(&["Push to main", "Not yet", "Type something."])).unwrap();
        assert_eq!((q.asks.as_str(), q.yes, q.no), ("Push to main?", 1, 2));
        assert!(push_question(&prompt(&["Yes", "No"])).is_none());
        let found = prompt(&["Push to main", "Not yet"]);
        let landing = holding(
            "asking to land",
            json!({ "review": { "clean_rounds": 2 }, "leak": { "found": 0 } }),
        );
        assert_eq!(
            push_text(&found, Some(&landing)),
            "Land `Add the harness's warm-up runs`?\nReviewed clean twice; leak-check found none.\n_As the desktop's [Y/n]: no answer is no, so nothing pushes by itself._"
        );
    }
}
