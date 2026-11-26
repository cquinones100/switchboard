//! What a Claude tab's screen shows that a remote front-end acts on, read
//! the same way by the server answering the protocol and by the relay
//! deciding what to post: a draft left unsent in the prompt box, a dialog
//! Claude Code shows before its session starts, and a prompt's id, the
//! hash of its question and options a button or card carries so a tap
//! answers only the prompt it was made for. Moved from the relay's
//! src/relay/tabs.rs (tabs.js), which the phone and the server had reached
//! into for them. A prompt itself is mission control's
//! (`mission::answer`), which everything answering one shares.

use std::sync::LazyLock;

use regex::Regex;

use crate::mission::answer::{self, Press, Prompt, PromptOption};
use crate::mux::input::{Key, KeyCode, Mods};

/// What is typed and unsent in Claude Code's prompt box, if anything, from
/// a pane's screen (`capture`) and the same screen with what is dimmed left
/// blank (`capture_typed`), line for line: the box is the lines from the
/// last rule with the prompt mark at the line's start under it (`❯`, or
/// `>` in older versions, which also drew sides) to the screen's last
/// rule. A draft's own lines are indented, so a rule or a `>` in one isn't
/// taken for the box's. The text is
/// read from the second, since Claude Code shows its placeholder and a
/// suggested next prompt dimmed in an empty box.
pub fn draft_in(screen: &str, typed: &str) -> Option<String> {
    let lines: Vec<&str> = screen.lines().collect();
    let typed: Vec<&str> = typed.lines().collect();
    let rule = |l: &&str| {
        let t = l.trim();
        t.chars().count() >= 3 && t.chars().all(|c| ('\u{2500}'..='\u{257F}').contains(&c))
    };
    let marked = |i: usize| {
        let line = lines[i + 1];
        let line = line.strip_prefix('│').unwrap_or(line);
        line.strip_prefix(' ')
            .unwrap_or(line)
            .starts_with(['❯', '>'])
    };
    let bottom = lines.iter().rposition(rule)?;
    let top = (0..bottom.saturating_sub(1))
        .rev()
        .find(|&i| rule(&lines[i]) && marked(i))?;
    // Only a box drawn with sides has them taken off its lines.
    let sided = lines[top].trim_start().starts_with('╭');
    let unboxed = |l: &str| {
        let l = l.trim();
        let l = if sided {
            let l = l.strip_prefix('│').unwrap_or(l);
            l.strip_suffix('│').unwrap_or(l)
        } else {
            l
        };
        l.trim().to_string()
    };
    let text: Vec<String> = (top + 1..bottom)
        .map(|i| {
            let line = unboxed(typed.get(i).unwrap_or(&""));
            if i == top + 1 {
                line.strip_prefix(['❯', '>'])
                    .unwrap_or(&line)
                    .trim()
                    .to_string()
            } else {
                line
            }
        })
        .collect();
    let text = text.join("\n").trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// The time left in the countdown Claude Code adds to a question in an
/// unattended session ("Claude Code will automatically block this request
/// in 1:03, ..."), which changes every second.
static COUNTDOWN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(block\s+this\s+request\s+in)\s+\d+:\d{2}").unwrap());

/// A short fingerprint of a prompt or dialog: its question and every
/// option. A button records the one it was made for, so a press only ever
/// answers that same prompt. The countdown's time is left out, so a press
/// stays good while it runs down.
pub fn prompt_hash(question: &str, options: &[PromptOption]) -> String {
    let question = COUNTDOWN.replace_all(question, "$1");
    let text = format!(
        "{question}\n{}",
        options
            .iter()
            .map(|o| format!("{}. {}", o.n, o.label))
            .collect::<Vec<_>>()
            .join("\n")
    );
    crate::tabs::color::sha256(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()[..16]
        .to_string()
}

/// A prompt's id: its question and options, and, where the step it shows
/// was read (as the phone reads it), that step too, so an answer meant for
/// another step is refused.
pub fn hash_of(found: &Prompt) -> String {
    match found.shown {
        Some(at) => prompt_hash(&format!("{}\nstep {at}", found.question), &found.options),
        None => prompt_hash(&found.question, &found.options),
    }
}

/// A dialog Claude Code shows before a session starts ("trust this
/// folder", "new MCP server found"): options listed one per line,
/// unnumbered, the current one marked ❯, above an "Enter to confirm" line.
#[derive(Clone, Debug, PartialEq)]
pub struct Dialog {
    pub question: String,
    pub options: Vec<PromptOption>,
    pub cursor: u32,
}

impl Dialog {
    pub fn hash(&self) -> String {
        prompt_hash(&self.question, &self.options)
    }

    /// It as a prompt of one choice, for showing it as a prompt shows.
    pub fn as_prompt(&self) -> Prompt {
        Prompt {
            question: self.question.clone(),
            options: self.options.clone(),
            multi: false,
            preview: false,
            free: None,
            cursor: None,
            more: Vec::new(),
            steps: Vec::new(),
            shown: None,
            review: Vec::new(),
        }
    }

    /// What choosing `picked` takes: up or down to it from the option the
    /// cursor is on, then Enter.
    pub fn presses(&self, picked: &PromptOption) -> Vec<Press> {
        let moves = picked.n as i64 - self.cursor as i64;
        let way = if moves > 0 {
            KeyCode::Down
        } else {
            KeyCode::Up
        };
        std::iter::repeat_n(
            Press::Key(Key::new(way, Mods::NONE)),
            moves.unsigned_abs() as usize,
        )
        .chain([Press::Key(Key::new(KeyCode::Enter, Mods::NONE))])
        .collect()
    }
}

/// What clearing a draft of `draft` in a prompt box on `screen` takes:
/// Ctrl-U once a line and once more, `i` first in vim's Normal mode.
pub fn clearing(screen: &str, draft: &str) -> Vec<Press> {
    let clears =
        (0..=draft.lines().count()).map(|_| Press::Key(Key::new(KeyCode::Char('u'), Mods::CTRL)));
    answer::into_prompt(screen, clears.collect())
}

/// The dialog a screen shows, or `None`. No hook runs before the session
/// starts, so a tab showing one has no state; it is read from the screen.
/// The options are the lines aligned with the text after ❯, so the
/// question and notes above them, set less deep, aren't taken for one.
pub fn dialog_in(text: &str) -> Option<Dialog> {
    let all: Vec<&str> = text.split('\n').collect();
    let lines: Vec<Vec<char>> = all
        .iter()
        .enumerate()
        .filter(|(i, l)| !l.trim().is_empty() && (*i == 0 || all[i - 1] != **l))
        .map(|(_, l)| l.chars().collect())
        .collect();
    let joined = |l: &[char]| l.iter().collect::<String>();
    let footer = lines
        .iter()
        .rposition(|l| joined(l).contains("Enter to confirm"))?;
    if footer + 2 < lines.len() {
        return None;
    }
    let marked = |l: &[char]| {
        l.iter().position(|c| *c == '❯').is_some_and(|at| {
            l[at + 1..].first().is_some_and(|c| c.is_whitespace())
                && l[at + 1..].iter().any(|c| !c.is_whitespace())
        })
    };
    let cursor_line = lines[..footer].iter().rposition(|l| marked(l))?;
    let line = &lines[cursor_line];
    let mark = line.iter().position(|c| *c == '❯')?;
    let column = mark + 1 + line[mark + 1..].iter().position(|c| !c.is_whitespace())?;
    let is_option = |l: &[char]| {
        l.iter().position(|c| *c == '❯') == Some(mark)
            || (l.len() > column
                && l[..column].iter().all(|c| c.is_whitespace())
                && l[column] != ' ')
    };
    let mut start = cursor_line;
    while start > 0 && is_option(&lines[start - 1]) {
        start -= 1;
    }
    let mut end = cursor_line;
    while end + 1 < footer && is_option(&lines[end + 1]) {
        end += 1;
    }
    let options: Vec<PromptOption> = lines[start..=end]
        .iter()
        .enumerate()
        .map(|(i, l)| PromptOption {
            n: i as u32 + 1,
            label: joined(l).replacen('❯', "", 1).trim().to_string(),
            checked: None,
            detail: String::new(),
        })
        .collect();
    if options.len() < 2 {
        return None;
    }
    static RULE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[\s\x{2500}-\x{257F}—-]+$").expect("a regex"));
    let mut above: Vec<String> = Vec::new();
    let mut i = start;
    while i > 0 && above.len() < 3 {
        i -= 1;
        let l = joined(&lines[i]);
        if RULE.is_match(&l) {
            break;
        }
        above.insert(0, l.trim().to_string());
    }
    Some(Dialog {
        question: above.join(" "),
        options,
        cursor: (cursor_line - start) as u32 + 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompts_hash_leaves_out_the_countdown_and_nothing_else() {
        let options = [
            PromptOption {
                n: 1,
                label: "Yes".into(),
                checked: None,
                detail: String::new(),
            },
            PromptOption {
                n: 2,
                label: "No".into(),
                checked: None,
                detail: String::new(),
            },
        ];
        let asking = |rest: &str| {
            format!(
                "Dangerous rm operation\n⚠️ Claude Code will automatically block this request {rest}, to avoid holding up an unattended session"
            )
        };
        let first = prompt_hash(&asking("in 1:03"), &options);
        assert_eq!(first, prompt_hash(&asking("in 0:51"), &options));
        assert_eq!(first, prompt_hash(&asking("in\n10:00"), &options));
        assert_ne!(first, prompt_hash(&asking("soon, in 1:03"), &options));
        assert_ne!(
            first,
            prompt_hash(&asking("in 1:03").replace("rm", "mv"), &options)
        );
    }

    const BOX: &str =
        "⏺ Done.\n\n────────\n{}\n────────\n  Opus 5.5 | Context: 10.0% used\n  -- INSERT --";

    fn boxed(lines: &str) -> String {
        BOX.replace("{}", lines)
    }

    #[test]
    fn the_prompt_box_gives_what_is_typed_in_it() {
        let bare = boxed("❯");
        assert_eq!(draft_in(&bare, &bare), None);
        let ruled = boxed("❯ explain this\n  ────────\n  example");
        assert_eq!(
            draft_in(&ruled, &ruled).as_deref(),
            Some("explain this\n────────\nexample")
        );
        let nested = boxed("❯ explain this\n  ────────\n  >");
        assert_eq!(
            draft_in(&nested, &nested).as_deref(),
            Some("explain this\n────────\n>")
        );
        let bar = boxed("❯ │");
        assert_eq!(draft_in(&bar, &bar).as_deref(), Some("│"));
        let quoted = boxed("❯ >");
        assert_eq!(draft_in(&quoted, &quoted).as_deref(), Some(">"));
        let typed = boxed("❯ send that to #stream");
        assert_eq!(
            draft_in(&typed, &typed).as_deref(),
            Some("send that to #stream")
        );
        let two = boxed("❯ first line\n  second line");
        assert_eq!(
            draft_in(&two, &two).as_deref(),
            Some("first line\nsecond line")
        );
        let sided = "╭────╮\n│ > go on   │\n╰────╯\n  ? for shortcuts";
        assert_eq!(draft_in(sided, sided).as_deref(), Some("go on"));
    }

    #[test]
    fn a_dimmed_placeholder_or_suggestion_isnt_a_draft() {
        let shown = boxed("❯ Try \"fix lint errors\"");
        let typed = boxed("❯                     ");
        assert_eq!(draft_in(&shown, &typed), None);
    }

    #[test]
    fn a_question_or_a_screen_without_the_box_has_no_draft() {
        let question = "Do you want to proceed?\n❯ 1. Yes\n  2. No\n\nEsc to cancel";
        assert_eq!(draft_in(question, question), None);
        let asked = "────────\n Allow rm?\n❯ 1. Yes\n  2. No\n────────";
        assert_eq!(draft_in(asked, asked), None);
        assert_eq!(draft_in("$ ls\nfile", "$ ls\nfile"), None);
    }
    #[test]
    fn dialogs_before_a_session_starts_are_read() {
        // Both captured from real tabs.
        let trust = [
            "─".repeat(60).as_str(),
            " Accessing workspace:",
            " /work/new-repo",
            " Quick safety check: Is this a project you created or one you trust?",
            " Claude Code'll be able to read, edit, and execute files here.",
            " Security guide",
            " ❯ No, exit",
            "   Yes, I trust this folder",
            " Enter to confirm · Esc to cancel",
        ]
        .join("\n");
        let found = dialog_in(&trust).unwrap();
        assert_eq!(
            found.question,
            "Quick safety check: Is this a project you created or one you trust? Claude Code'll be able to read, edit, and execute files here. Security guide"
        );
        assert_eq!(
            found
                .options
                .iter()
                .map(|o| o.label.as_str())
                .collect::<Vec<_>>(),
            ["No, exit", "Yes, I trust this folder"]
        );
        assert_eq!(found.cursor, 1);
        let mcp = [
            "  New MCP server found in this project: some-server",
            "  MCP servers may execute code or access system resources. All tool calls require approval.",
            "    Use this MCP server",
            "    Use this and all future MCP servers in this project",
            "  ❯ Continue without using this MCP server",
            "  Enter to confirm · Esc to cancel",
        ]
        .join("\n");
        let found = dialog_in(&mcp).unwrap();
        assert_eq!(
            found
                .options
                .iter()
                .map(|o| o.label.as_str())
                .collect::<Vec<_>>(),
            [
                "Use this MCP server",
                "Use this and all future MCP servers in this project",
                "Continue without using this MCP server"
            ]
        );
        assert_eq!(found.cursor, 3);
        // Not at the bottom of the screen: an old dialog, already answered.
        assert!(dialog_in(&format!("{trust}\n● Working on it\n> next\n✻ Thinking")).is_none());
    }
}
