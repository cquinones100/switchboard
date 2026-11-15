//! Autopilot's hands-free mode at the hooks: in a tab marked
//! `@autopilot_handsfree` (src/autopilot/serve.rs's `HANDS_FREE`), the
//! server answers two of Claude Code's hooks with what Claude Code reads
//! from a hook's output, which `switchboard hook` prints, the one output it
//! ever makes:
//!
//! - PreToolUse for `AskUserQuestion` is denied, its reason telling Claude
//!   to take the recommended option and write the decision into the item's
//!   plan, unless a question is marked `[always ask]` at the start of its
//!   header or its text (what a skill marks as a question that must reach
//!   you), which goes through to be asked as usual.
//! - Stop, on a turn whose last message asks something without saying
//!   `[always ask]` (what Claude says of a question a rule or skill says
//!   must reach you), is blocked, its
//!   reason telling Claude to carry on with what it recommends, at most
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
//! (`ask_push`) as well unless the tab is one of autopilot's with "Land without
//! asking" (`@autopilot_land`) on, so Claude Code asks it as it asked the
//! `git push` it replaced. The push without `--asked` needs no ask: the
//! command pushes only when its own gate passes (src/cmd/land_push.rs),
//! and exits asking otherwise. A tab that lands without asking is left to
//! that gate and the repository's pre-push hook, since whether the change
//! pushed is the one reviewed takes git, which the server's task never
//! runs.

use serde_json::{Value, json};

/// What marks a question a skill says must always reach you.
pub const ALWAYS_ASK: &str = "[always ask]";

/// The most turns a tab's Claude is told to carry on past a question.
pub const NUDGES: u32 = 3;

/// What still stops a hands-free tab, in both reasons.
const EXCEPTIONS: &str = "Only what must be asked still stops: what the user's or the repository's CLAUDE.md says to ask about (deleting data, end-to-end tests, a push anywhere but the landing), a decision that approves a design board (one naming a board, or waiting until it is reviewed), a question a skill you follow says to stop and ask (an unclear conflict, a failure the branch didn't cause, untracked files of someone else's, or one it heads `[always ask]`), whatever its wording, and a permission that was denied.";

/// The deny for a question box, as PreToolUse's output.
pub fn deny_question() -> String {
    let reason = format!(
        "Hands-free (autopilot): don't ask. Take the option marked (Recommended), else the first; write `Decided hands-free: <question> → <choice>` under the item in its plan note (with no plan, indented under the item in its note); then go on. {EXCEPTIONS} For one of those, ask again with `{ALWAYS_ASK}` as the question's header."
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
pub fn block_stop() -> String {
    let reason = format!(
        "Hands-free (autopilot): don't wait for an answer. Carry on with what you recommend; write `Decided hands-free: <question> → <choice>` under the item in its plan note (with no plan, indented under the item in its note); then go on to land it. {EXCEPTIONS} For one of those, stop again with `{ALWAYS_ASK}` in your message, saying which rule or skill says it must be asked."
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

/// Whether a Bash call runs the landing push wtn's push rule allowed before
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
    payload
        .pointer("/tool_input/questions")
        .and_then(Value::as_array)
        .is_some_and(|qs| {
            qs.iter().any(|q| {
                ["header", "question"].iter().any(|k| {
                    q.get(*k)
                        .and_then(Value::as_str)
                        .is_some_and(|t| t.trim_start().starts_with(ALWAYS_ASK))
                })
            })
        })
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
        let deny: Value = serde_json::from_str(&deny_question()).unwrap();
        assert_eq!(deny["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(deny["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert!(
            deny["hookSpecificOutput"]["permissionDecisionReason"]
                .as_str()
                .unwrap()
                .contains("Decided hands-free")
        );
        let block: Value = serde_json::from_str(&block_stop()).unwrap();
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
