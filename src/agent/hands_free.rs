//! Autopilot's hands-free mode at the hooks: in a tab marked
//! `@autopilot_handsfree` (src/autopilot/serve.rs's `HANDS_FREE`), the
//! server answers two of Claude Code's hooks with what Claude Code reads
//! from a hook's output, which `switchboard hook` prints, the one output it
//! ever makes:
//!
//! - PreToolUse for `AskUserQuestion` is denied, the server choosing each
//!   question's option labeled (Recommended), else its first (`choose`),
//!   and its reason naming the choices for Claude to go on with, unless a
//!   question is marked `[always ask]` at the start of its header or its
//!   text (what a skill marks as a question that must reach you), which goes
//!   through to be asked as usual. The server writes each choice's
//!   `Decided hands-free: <question> → <choice>` line itself (`record`),
//!   off its task: at the end of the item's plan note, or, with no plan,
//!   indented under the item in its note, read and written under the note
//!   ID's lock with its modified time kept (`id::rewrite`), so the open
//!   Notes app takes it in as any change made from outside. Those lines are
//!   what the landing counts as decided hands-free
//!   (src/autopilot/landed.rs's `decided_in`), so the server, not Claude,
//!   keeps that record; a question asked again marked `[always ask]`, one
//!   Claude found it must ask after all, takes its line back. In a GitHub
//!   issue's tab, which has no note, Claude says the lines in its reply,
//!   and a question marked `[plan]` (`PLAN`), one about what the issue
//!   says should be built (its steps, its options or its scope), has its
//!   choice written to the issue by the server too (`plan_choices`,
//!   src/autopilot/serve.rs's `autopilot_write_decided`); the rest, how
//!   to run a test or which file to read first, writes nothing to GitHub.
//! - Stop, on a turn whose last message asks something without saying
//!   `[always ask]` (what Claude says of a question a rule or skill says
//!   must reach you), is blocked, its
//!   reason telling Claude to carry on with what it recommends and land
//!   the one way `switchboard landing` prints for the repository, at most
//!   `NUDGES` times a tab (an item, since autopilot opens a tab an item),
//!   counted in `@autopilot_nudged`, and never once the tab has landed.
//!   Claude Code runs StopFailure, not Stop, for a turn an API error ended,
//!   so an error is never blocked.
//!
//! Anything that isn't one of those answers nothing, so an unmarked tab, a
//! subagent's hook, or a server that doesn't answer leaves Claude asking.
//!
//! One more answer holds in every tab, hands-free or not: the landing's
//! push after your yes, `switchboard land-lock push --asked`, which no
//! allow rule names (each names `land-lock push` exactly), is asked
//! (`ask_push`) as well unless the tab lands without asking (`@land`, from
//! Settings' land_without_asking, the tab's menu or autopilot's "Land
//! without asking"), so Claude Code asks it as it asked the
//! `git push` it replaced. The push without `--asked` needs no ask: the
//! command pushes only when its own gate passes (src/cmd/land_push.rs),
//! and exits asking otherwise. A tab that lands without asking is left to
//! that gate and the repository's pre-push hook, since whether the change
//! pushed is the one reviewed takes git, which the server's task never
//! runs.

use std::io;
use std::path::{Path, PathBuf};

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use crate::notes::id;

/// What marks a question a skill says must always reach you.
pub const ALWAYS_ASK: &str = "[always ask]";

/// What marks a question about an issue's plan, whose decision the issue
/// records.
pub const PLAN: &str = "[plan]";

/// The most turns a tab's Claude is told to carry on past a question.
pub const NUDGES: u32 = 3;

/// What still stops a hands-free tab, in both reasons.
const EXCEPTIONS: &str = "Only what must be asked still stops: what the user's or the repository's CLAUDE.md says to ask about (deleting data, end-to-end tests, a push anywhere but the landing), a decision that approves a design board (one naming a board, or waiting until it is reviewed), a question a skill you follow says to stop and ask (an unclear conflict, a failure the branch didn't cause, untracked files of someone else's, or one it heads `[always ask]`), whatever its wording, and a permission that was denied.";

/// A checklist item, ticked or not, and its first line.
static ITEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^- \[[ xX]\] (.*\S.*)$").unwrap());

/// What marks the option a question box recommends.
const RECOMMENDED: &str = "(Recommended)";

/// A question box's questions with the option the server takes for each:
/// the one labeled (Recommended), else the first. A question with no
/// options is left out.
pub fn choose(payload: &Value) -> Vec<(String, String)> {
    chosen(payload)
        .into_iter()
        .map(|(_, q, c)| (q, c))
        .collect()
}

/// The choices of a question box's questions marked `[plan]`, the ones an
/// issue's tab writes to its issue.
pub fn plan_choices(payload: &Value) -> Vec<(String, String)> {
    chosen(payload)
        .into_iter()
        .filter(|(plan, ..)| *plan)
        .map(|(_, q, c)| (q, c))
        .collect()
}

/// `choose`'s choices, each with whether its question is marked `[plan]`,
/// the mark taken off the question.
fn chosen(payload: &Value) -> Vec<(bool, String, String)> {
    let label = |o: &Value| o.get("label").and_then(Value::as_str).map(str::to_string);
    questions(payload)
        .iter()
        .filter_map(|q| {
            let plan = marked(q, PLAN);
            let question = q.get("question").and_then(Value::as_str)?.trim();
            let question = question.strip_prefix(PLAN).unwrap_or(question);
            let options = q.get("options").and_then(Value::as_array)?;
            let chosen = options
                .iter()
                .find(|o| label(o).is_some_and(|l| l.contains(RECOMMENDED)))
                .or_else(|| options.first())
                .and_then(label)?;
            Some((plan, one_line(question), one_line(&chosen)))
        })
        .collect()
}

/// Whether a question starts its header or its text with `mark`.
fn marked(question: &Value, mark: &str) -> bool {
    ["header", "question"].iter().any(|k| {
        question
            .get(*k)
            .and_then(Value::as_str)
            .is_some_and(|t| t.trim_start().starts_with(mark))
    })
}

/// A question box's questions, as Claude Code passes them.
fn questions(payload: &Value) -> &[Value] {
    payload
        .pointer("/tool_input/questions")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// Text on one line, as a note's line holds it.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The line recording a choice, which the landing counts.
pub fn decided_line(question: &str, choice: &str) -> String {
    format!("Decided hands-free: {question} → {choice}")
}

/// Where a hands-free tab's decided lines are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    /// By the server, into the item's notes.
    Server,
    /// By Claude, under the item in its plan or note.
    Note,
    /// By Claude, in its reply: a GitHub issue's tab, which has no note.
    Reply,
}

impl Record {
    /// What Claude is told of writing the lines down.
    fn words(self) -> &'static str {
        match self {
            Record::Server => {
                "The server writes each `Decided hands-free: <question> → <choice>` line into the item's plan note (with no plan, under the item in its note) itself: don't write it."
            }
            Record::Note => {
                "Write `Decided hands-free: <question> → <choice>` under the item in its plan note (with no plan, indented under the item in its note)."
            }
            Record::Reply => {
                "Say each as a `Decided hands-free: <question> → <choice>` line in your reply: this tab works a GitHub issue, with no note to write it in. Head a question about what the issue says should be built (its steps, its options or its scope) `[plan]`, and the server writes its decision to the issue; leave the rest unmarked."
            }
        }
    }
}

/// The deny for a question box, as PreToolUse's output: the server's
/// choices, and where the lines recording them are written (`record`).
pub fn deny_question(chosen: &[(String, String)], record: Record) -> String {
    let choices = chosen
        .iter()
        .map(|(q, c)| format!("{q} → {c}"))
        .collect::<Vec<_>>()
        .join("; ");
    let record = record.words();
    let reason = format!(
        "Hands-free (autopilot): don't ask. Chose: {choices}. {record} Go on with these choices. {EXCEPTIONS} For one of those, ask again with `{ALWAYS_ASK}` as the question's header, which takes the decided line back."
    );
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    })
    .to_string()
}

/// The block for a turn that ended on a question, as Stop's output.
pub fn block_stop(record: Record) -> String {
    let write = match record {
        Record::Reply => {
            "say each decision as a `Decided hands-free: <question> → <choice>` line in your reply"
        }
        _ => {
            "write `Decided hands-free: <question> → <choice>` under the item in its plan note (with no plan, indented under the item in its note)"
        }
    };
    let reason = format!(
        "Hands-free (autopilot): don't wait for an answer. Carry on with what you recommend; {write}; then go on to land it the one way `switchboard landing` prints for the repository, as queue-and-clean does, never by a `gh pr merge` or a push to the default branch that way doesn't make. {EXCEPTIONS} For one of those, stop again with `{ALWAYS_ASK}` in your message, saying which rule or skill says it must be asked."
    );
    json!({ "decision": "block", "reason": reason }).to_string()
}

/// The ask for the landing's push in a tab that doesn't land without
/// asking, as PreToolUse's output: Claude Code asks it whatever an allow
/// rule says.
pub fn ask_push() -> String {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "ask",
            "permissionDecisionReason": "The landing's push to the branch it lands on is asked, since this tab doesn't land without asking.",
        }
    })
    .to_string()
}

/// Whether a Bash call runs the landing's push after your yes,
/// `switchboard land-lock push --asked`, as a command of its own or chained
/// to others; any other push is no rule's and is judged as before.
pub fn is_landing_push(payload: &Value) -> bool {
    let Some(command) = payload
        .pointer("/tool_input/command")
        .and_then(Value::as_str)
    else {
        return false;
    };
    command.split(['\n', ';', '&', '|', '(', ')']).any(|part| {
        // Words as the shell hands them on, its quotes and escapes gone, so
        // `'--asked'` is `--asked` here as it is to the command.
        let words: Vec<String> = part
            .split_whitespace()
            .map(|w| {
                w.chars()
                    .filter(|c| !matches!(c, '\'' | '"' | '\\'))
                    .collect()
            })
            .collect();
        let has = |word: &str| words.iter().any(|w| w == word);
        let program = words.first().map(String::as_str).unwrap_or_default();
        (program == "switchboard" || program.ends_with("/switchboard"))
            && words.get(1).map(String::as_str) == Some("land-lock")
            && has("push")
            && has("--asked")
    })
}

/// Whether a Bash call runs the landing push wt's push rule allowed before
/// `land-lock push`, `git push <remote> HEAD:<branch>`, which a session
/// started then still has allowed after an upgrade: asked, as before, in a
/// tab of autopilot's while it doesn't land without asking.
pub fn is_old_landing_push(payload: &Value) -> bool {
    let Some(command) = payload
        .pointer("/tool_input/command")
        .and_then(Value::as_str)
    else {
        return false;
    };
    command.split(['\n', ';', '&', '|', '(', ')']).any(|part| {
        let words: Vec<&str> = part.split_whitespace().collect();
        matches!(words.as_slice(), ["git", "push", remote, to]
            if !remote.starts_with('-') && to.starts_with("HEAD:"))
    })
}

/// Whether a question box marks one of its questions always asked.
pub fn always_asked(payload: &Value) -> bool {
    questions(payload).iter().any(|q| marked(q, ALWAYS_ASK))
}

/// A question box's questions, the `[always ask]` mark taken off, as a
/// decided line names them.
pub fn asked_questions(payload: &Value) -> Vec<String> {
    questions(payload)
        .iter()
        .filter_map(|q| q.get("question").and_then(Value::as_str))
        .map(|q| one_line(q.trim_start().trim_start_matches(ALWAYS_ASK)))
        .collect()
}

/// Where a tab's decided lines go: the notes directory, the tab's plan
/// note (`@tab_plan_note`, a path in it), the ID of the item's note
/// (`@autopilot_note`), the tab's note (`@tab_note`), and the item's
/// first line (`@autopilot_item`).
pub struct Place {
    pub notes: PathBuf,
    pub plan: Option<String>,
    pub note_id: Option<String>,
    pub note: Option<String>,
    pub item: String,
}

/// What `record` does to the decided lines.
pub enum Change {
    /// Adds a line for each choice.
    Decide(Vec<(String, String)>),
    /// Takes back the lines of these questions, asked after all.
    TakeBack(Vec<String>),
}

/// Writes a change to the decided lines into the item's plan note, else
/// under the item in its note: whether there was a place for it (a plan, or
/// a note still holding the item), so a decision with nowhere to go is
/// said rather than lost. Reads the notes directory, so it runs off the
/// server's task.
pub fn record(place: &Place, change: &Change) -> io::Result<bool> {
    let inside = |rel: &String| crate::notes::serve::in_notes(rel).then(|| place.notes.join(rel));
    let note = place
        .note_id
        .as_deref()
        .and_then(|i| id::find_by_id(i, &place.notes))
        .or_else(|| place.note.as_ref().and_then(inside))
        .filter(|f| f.is_file());
    let plan = place
        .plan
        .as_ref()
        .and_then(inside)
        .filter(|f| f.is_file())
        .or_else(|| {
            let text = std::fs::read_to_string(note.as_ref()?).ok()?;
            crate::cmd::worktree_from_note::item_plan_file(&place.notes, &text, &place.item)
        });
    let (file, under): (&Path, Option<&str>) = match (&plan, &note) {
        (Some(plan), _) => (plan, None),
        (None, Some(note)) => (note, Some(&place.item)),
        (None, None) => return Ok(false),
    };
    if let Some(item) = under {
        let text = std::fs::read_to_string(file)?;
        if item_block(&text.split('\n').collect::<Vec<_>>(), item).is_none() {
            return Ok(false);
        }
    }
    id::rewrite(file, |text| match change {
        Change::Decide(chosen) => {
            let lines: Vec<String> = chosen.iter().map(|(q, c)| decided_line(q, c)).collect();
            match under {
                None => with_decided_at_end(text, &lines),
                Some(item) => with_decided_under_item(text, item, &lines),
            }
        }
        Change::TakeBack(questions) => without_decided(text, under, questions),
    })?;
    Ok(true)
}

/// A plan note's text with `lines` added as items at its end, before its
/// ID; `None` when it holds them all already.
pub fn with_decided_at_end(text: &str, lines: &[String]) -> Option<String> {
    let mut body: Vec<String> = id::lines_of(&id::body_of(text))
        .into_iter()
        .map(str::to_string)
        .collect();
    let new: Vec<String> = lines
        .iter()
        .map(|l| format!("- {l}"))
        .filter(|l| !body.contains(l))
        .collect();
    if new.is_empty() {
        return None;
    }
    while body.last().is_some_and(|l| l.trim().is_empty()) {
        body.pop();
    }
    body.extend(new);
    Some(match id::in_text(text) {
        Some(note_id) => id::file_of(&body, &note_id),
        None => format!("{}\n", body.join("\n")),
    })
}

/// A note's text with `lines` indented under the item whose first line is
/// `item`, after what is indented under it already; `None` when the item
/// isn't there or holds them all.
pub fn with_decided_under_item(text: &str, item: &str, lines: &[String]) -> Option<String> {
    let mut all: Vec<String> = text.split('\n').map(str::to_string).collect();
    let (at, end) = item_block(&all, item)?;
    let under = &all[at + 1..end];
    let new: Vec<String> = lines
        .iter()
        .map(|l| format!("  - {l}"))
        .filter(|l| !under.contains(l))
        .collect();
    if new.is_empty() {
        return None;
    }
    all.splice(end..end, new);
    Some(all.join("\n"))
}

/// The item whose first line is `item` among a note's lines: its row, and
/// the row past what is indented under it, trailing blank lines left out.
fn item_block<S: AsRef<str>>(lines: &[S], item: &str) -> Option<(usize, usize)> {
    let at = lines.iter().position(|l| {
        ITEM.captures(l.as_ref())
            .is_some_and(|m| m[1].trim() == item)
    })?;
    let mut end = at + 1;
    while lines.get(end).is_some_and(|l| {
        let l = l.as_ref();
        l.trim().is_empty() || l.starts_with(char::is_whitespace)
    }) {
        end += 1;
    }
    while end > at + 1 && lines[end - 1].as_ref().trim().is_empty() {
        end -= 1;
    }
    Some((at, end))
}

/// A note's text without the decided lines of `questions`, only those under
/// the item whose first line is `under` when it names one, so another
/// item's decisions stay; `None` when it holds none.
pub fn without_decided(text: &str, under: Option<&str>, questions: &[String]) -> Option<String> {
    let prefixes: Vec<String> = questions
        .iter()
        .map(|q| format!("- {}", decided_line(q, "")))
        .collect();
    let lines: Vec<&str> = text.split('\n').collect();
    let (from, to) = match under {
        Some(item) => {
            let (at, end) = item_block(&lines, item)?;
            (at + 1, end)
        }
        None => (0, lines.len()),
    };
    let kept: Vec<&str> = lines
        .iter()
        .enumerate()
        .filter(|(i, l)| {
            !((from..to).contains(i)
                && prefixes
                    .iter()
                    .any(|p| l.trim_start().starts_with(p.as_str())))
        })
        .map(|(_, l)| *l)
        .collect();
    (kept.len() < lines.len()).then(|| kept.join("\n"))
}

/// Whether a turn's last message is one to send back: it ends on a
/// question and doesn't say `[always ask]`.
pub fn sends_back(payload: &Value) -> bool {
    let marked = payload
        .get("last_assistant_message")
        .and_then(Value::as_str)
        .is_some_and(|m| m.contains(ALWAYS_ASK));
    !marked && ends_on_question(payload)
}

/// Whether a turn's last message ends on a question: its last line before
/// any list of options under it, past markdown's emphasis,
/// ending in a question mark.
pub fn ends_on_question(payload: &Value) -> bool {
    let Some(last) = payload
        .get("last_assistant_message")
        .and_then(Value::as_str)
    else {
        return false;
    };
    last.lines()
        .rev()
        .map(|l| l.trim().trim_end_matches(['*', '_', '`', ')']).trim_end())
        .filter(|l| !l.is_empty())
        // The last line, a list of options under the question passed over.
        .find(|l| !is_option(l) || l.ends_with('?'))
        .is_some_and(|l| l.ends_with('?'))
}

/// Whether a line is an option of a numbered or lettered list (`1.`, `2)`,
/// `a.`), the way choices are offered; a bulleted line may as well be an
/// answer or a conclusion, so it is no option.
fn is_option(line: &str) -> bool {
    let marker = line.trim_start_matches(|c: char| c.is_ascii_digit());
    let numbered =
        marker.len() < line.len() && (marker.starts_with(". ") || marker.starts_with(") "));
    // Checked, since a line's second character may take more than a byte.
    let lettered = line.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && matches!(line.get(1..3), Some(". " | ") "));
    numbered || lettered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_landing_push_is_one() {
        let bash = |c: &str| json!({"tool_input": {"command": c}});
        assert!(is_landing_push(&bash("switchboard land-lock push --asked")));
        assert!(is_landing_push(&bash(
            "git fetch && switchboard  land-lock push --asked Make it"
        )));
        assert!(is_landing_push(&bash(
            "~/.local/bin/switchboard land-lock --target ux push --asked"
        )));
        assert!(
            !is_landing_push(&bash("switchboard land-lock push")),
            "the push its own gate decides needs no ask"
        );
        assert!(!is_landing_push(&bash("git push origin HEAD:main")));
        for quoted in [
            "switchboard land-lock push '--asked'",
            "switchboard land-lock push \"--asked\"",
            "switchboard land-lock push \\--asked",
            "'switchboard' 'land-lock' push --asked",
        ] {
            assert!(is_landing_push(&bash(quoted)), "{quoted}");
        }
        assert!(is_old_landing_push(&bash(
            "git fetch && git push origin HEAD:main"
        )));
        assert!(!is_old_landing_push(&bash(
            "git push --force origin HEAD:refs/heads/ci/land-ab12"
        )));
        assert!(!is_landing_push(&bash(
            "git push --force origin HEAD:refs/heads/ci/land-ab12"
        )));
        assert!(!is_landing_push(&bash(
            "git push origin --delete ci/land-ab12"
        )));
        assert!(!is_landing_push(&bash("git push")));
        assert!(!is_landing_push(&json!({})));
    }

    #[test]
    fn the_recommended_option_is_chosen_else_the_first() {
        let payload = json!({"tool_input": {"questions": [
            {"header": "Way", "question": "Which way?", "options": [{"label": "A"}, {"label": "B (Recommended)"}]},
            {"header": "Name", "question": "Which\nname?", "options": [{"label": "X"}, {"label": "Y"}]},
            {"header": "None", "question": "Nothing to pick?", "options": []},
        ]}});
        assert_eq!(
            choose(&payload),
            [
                ("Which way?".to_string(), "B (Recommended)".to_string()),
                ("Which name?".to_string(), "X".to_string()),
            ]
        );
        assert!(choose(&json!({})).is_empty());
        assert!(plan_choices(&payload).is_empty());
        let plan = json!({"tool_input": {"questions": [
            {"header": "[plan] Scope", "question": "Widen it?", "options": [{"label": "No"}]},
            {"header": "Test", "question": "Which test first?", "options": [{"label": "Unit"}]},
            {"header": "Way", "question": "[plan] Which way?", "options": [{"label": "A"}]},
        ]}});
        assert_eq!(
            plan_choices(&plan),
            [
                ("Widen it?".to_string(), "No".to_string()),
                ("Which way?".to_string(), "A".to_string()),
            ]
        );
        assert_eq!(choose(&plan).len(), 3);
        let asked = json!({"tool_input": {"questions": [
            {"header": "[always ask]", "question": "[always ask] Push to main?"},
        ]}});
        assert_eq!(asked_questions(&asked), ["Push to main?"]);
    }

    #[test]
    fn a_decided_line_goes_at_the_plan_s_end_before_its_id() {
        let line = vec![decided_line("Which way?", "B")];
        let plan = "# Plan\n\n## Steps\n\n1. Do it.\n\n<!-- note-id: n-0123456789ab -->\n";
        let written = with_decided_at_end(plan, &line).unwrap();
        assert_eq!(
            written,
            "# Plan\n\n## Steps\n\n1. Do it.\n- Decided hands-free: Which way? → B\n\n<!-- note-id: n-0123456789ab -->\n"
        );
        assert_eq!(with_decided_at_end(&written, &line), None, "once only");
        assert_eq!(
            with_decided_at_end("# Plan\n1. Do it.\n", &line).unwrap(),
            "# Plan\n1. Do it.\n- Decided hands-free: Which way? → B\n"
        );
        let back = without_decided(&written, None, &["Which way?".to_string()]).unwrap();
        assert_eq!(
            back,
            "# Plan\n\n## Steps\n\n1. Do it.\n\n<!-- note-id: n-0123456789ab -->\n"
        );
        assert_eq!(
            without_decided(&back, None, &["Which way?".to_string()]),
            None
        );
    }

    #[test]
    fn with_no_plan_a_decided_line_goes_under_its_item() {
        let line = vec![decided_line("Which way?", "B")];
        let note =
            "# Notes\n- [ ] FEAT: Other\n- [ ] FEAT: Build X\n  - Seen today\n\n- [x] Done\n";
        assert_eq!(
            with_decided_under_item(note, "FEAT: Build X", &line).unwrap(),
            "# Notes\n- [ ] FEAT: Other\n- [ ] FEAT: Build X\n  - Seen today\n  - Decided hands-free: Which way? → B\n\n- [x] Done\n"
        );
        assert_eq!(with_decided_under_item(note, "FEAT: Missing", &line), None);
        let two = "- [x] FEAT: Old\n  - Decided hands-free: Which way? → A\n- [ ] FEAT: Build X\n  - Decided hands-free: Which way? → B\n";
        assert_eq!(
            without_decided(two, Some("FEAT: Build X"), &["Which way?".to_string()]).unwrap(),
            "- [x] FEAT: Old\n  - Decided hands-free: Which way? → A\n- [ ] FEAT: Build X\n",
            "another item's decision stays"
        );
    }

    #[test]
    fn a_question_marked_always_asked_goes_through() {
        let ask = |header: &str, question: &str| {
            json!({"tool_input": {"questions": [
                {"header": "Other", "question": "Fine?"},
                {"header": header, "question": question},
            ]}})
        };
        assert!(always_asked(&ask("[always ask]", "Push to main?")));
        assert!(always_asked(&ask("Push", "[always ask] Push to main?")));
        assert!(!always_asked(&ask("Push", "Push to main?")));
        assert!(!always_asked(&json!({})));
    }

    #[test]
    fn only_a_turn_ending_on_a_question_is_carried_on() {
        let last = |m: &str| json!({ "last_assistant_message": m });
        assert!(ends_on_question(&last(
            "Done the first part.\n\nShould I go on with B?"
        )));
        assert!(ends_on_question(&last("**Which do you want?**\n")));
        assert!(ends_on_question(&last("Which one?\n1. A\n2. B")));
        assert!(!ends_on_question(&last(
            "Landed abc1234. Is it fine? Yes.\n\nAll done.\nNothing left.\nBye."
        )));
        assert!(!ends_on_question(&last("Waiting on the tests.")));
        assert!(!ends_on_question(&json!({})));
        assert!(sends_back(&last("Should I do B too?")));
        assert!(!ends_on_question(&last(
            "Should I add another feature?\n- No; that is outside scope.\n- The work is complete."
        )));
        assert!(!ends_on_question(&last(
            "Should I add another feature?\nNo; that is outside its scope.\nThe work is complete."
        )));
        assert!(ends_on_question(&last(
            "Which approach should I use?\n1. A (Recommended)\n2. B\n3. C"
        )));
        assert!(
            ends_on_question(&last(
                "Implemented the fix.\n\n- Should I add regression coverage?"
            )) && !ends_on_question(&last("I’m done.\nI’d say so.")),
            "no panic on a curly apostrophe"
        );
        assert!(
            !sends_back(&last(
                "[always ask] CLAUDE.md says to ask before end-to-end tests. May I run them?"
            )),
            "a question a rule says must be asked stands"
        );
    }

    #[test]
    fn the_answers_are_what_claude_code_reads() {
        let deny: Value =
            serde_json::from_str(&deny_question(&[("Q?".into(), "A".into())], Record::Server))
                .unwrap();
        assert_eq!(deny["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(deny["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert!(
            deny["hookSpecificOutput"]["permissionDecisionReason"]
                .as_str()
                .unwrap()
                .contains("Chose: Q? → A.")
        );
        let block: Value = serde_json::from_str(&block_stop(Record::Note)).unwrap();
        assert_eq!(block["decision"], "block");
        assert!(block["reason"].as_str().unwrap().contains("[always ask]"));
        for reason in [
            deny["hookSpecificOutput"]["permissionDecisionReason"].as_str(),
            block["reason"].as_str(),
        ] {
            assert!(
                reason.unwrap().contains("approves a design board"),
                "a design review is among what is still asked"
            );
        }
    }
}
