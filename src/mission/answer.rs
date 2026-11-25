//! Answering a tab from mission control (lib/sidebar/answer.js), as the
//! Slack relay answers one from a thread: the question or permission prompt
//! its Claude shows, read off its screen, and your answer pressed there. The
//! reading and pressing are the relay's own (lib/slack-relay/tabs.js's
//! `optionsIn`, `answerFor`, `answerSaid` and `answerPrompt`), kept here as
//! plain functions of the screen's text so the relay can share them when it
//! comes over. A tab showing no prompt takes what you type as a message,
//! pasted and sent with Enter. Anything typed into Claude's prompt, here or
//! elsewhere, gets `i` first while the screen shows the prompt in vim's
//! Normal mode (`into_prompt`, tmux/claude-pane.sh's `claude_type` and
//! lib/sidebar/tmux.js's `typeIntoClaude`).
//!
//! `AnswerBox` is the box over mission control (render.js's `answerModal`):
//! the question, its options numbered, and a line to type in. A number
//! answers with that option, `s` submits a question of several answers, `t`
//! types your own answer where the question takes one; Tab and Shift-Tab
//! move a highlight round the options (and easy mode's ×), Enter taking it.
//! `/` searches the options as it searches every menu (crates/ui/src/menu.rs's
//! `Filter`), each keeping its number, since that is what is pressed.
//! What is typed is a multi-line `ui::field`: S-Enter and M-Enter start a
//! new line, pasted with the rest, and Enter sends it.
//! It closes with what to press, which the server presses in the pane.

use std::sync::LazyLock;

use ratatui::buffer::Buffer;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use regex::Regex;

use crate::mux::input::{Key, KeyCode, Mods, Mouse};
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use crate::ui::field::{Edit, Field};
use crate::ui::menu::{Filter, Filtered, NO_MATCH, SEARCH_KEYS, wrap_step};
use crate::ui::{Look, Outcome, Overlay, Picked, Step};

/// How long a paste is given to land before Enter follows it: sent at once,
/// Enter can reach Claude Code before it has taken the paste in, and be
/// lost (tabs.js's `PASTE_SETTLE_MS`).
pub const PASTE_SETTLE_MS: u64 = 300;

static OPTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:[❯›>]\s*)?(\d{1,2})[.)]\s+(.*\S)\s*$").expect("a regex"));
static CHECKBOX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[([ ✔✓xX])\]\s+(.*)$").expect("a regex"));
static SUBMIT_ROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:❯\s*)?Submit$").expect("a regex"));
static RULE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\s\x{2500}-\x{257F}—-]+$").expect("a regex"));
static QUESTION_TABS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[☐☒]").expect("a regex"));
/// What a box's left edge is drawn with, on its rows top to bottom.
const BOX_EDGE: [char; 7] = ['│', '┃', '├', '┌', '└', '╭', '╰'];
static LABEL_TAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s{2,}.*$").expect("a regex"));
static SCROLL_PILL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\S\s+·\s+\S+ to scroll$").expect("a regex"));

/// Whether Claude Code's fullscreen view (its `tui` setting) is scrolled
/// back: its screen then ends in a pill such as "Waiting for your answer ·
/// fn+↓ to scroll" over the transcript, and what Claude waits on, a
/// permission prompt or a question, is below what the screen holds.
pub fn scrolled_back(text: &str) -> bool {
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .is_some_and(|l| SCROLL_PILL.is_match(l.trim_end()))
}

/// One numbered option of a prompt; `checked` for one that takes several
/// answers, ticked or not; `detail` the lines under its label (a question's
/// explainer of the choice, or the rest of a wrapped label), joined, empty
/// when it has none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptOption {
    pub n: u32,
    pub label: String,
    pub checked: Option<bool>,
    pub detail: String,
}

/// Where the ❯ is in a question Claude asked: on an option, or on Submit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cursor {
    Option(u32),
    Submit,
}

/// A prompt a tab shows: its question, its options, whether it takes
/// several answers, the option whose label your own answer replaces, and
/// where the ❯ is (only for a question Claude asked, which has `free`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub question: String,
    pub options: Vec<PromptOption>,
    pub multi: bool,
    /// Whether its options have a preview drawn in a box beside them, where
    /// a number only moves the ❯ and Enter chooses.
    pub preview: bool,
    pub free: Option<u32>,
    pub cursor: Option<Cursor>,
    /// The options whose screen shows more than their label: words cut
    /// from its end, or lines under it (a description, a wrapped label).
    pub more: Vec<u32>,
    /// The steps of a prompt of several questions, from the line of their
    /// names, Submit last; empty for a prompt of one question.
    pub steps: Vec<QuestionStep>,
    /// Which of `steps` is shown, read from the line's cells
    /// (`read_shown`), since its text loses the highlight that marks it;
    /// `None` when read from text alone.
    pub shown: Option<usize>,
    /// On Submit's step, Claude Code's review of the answers given.
    pub review: Vec<Reviewed>,
}

/// One step of a prompt of several questions: a question's name, as the
/// line of names shows it, and whether it is answered (☒), or Submit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionStep {
    pub name: String,
    pub answered: bool,
    pub submit: bool,
}

/// One answer on Submit's step: the question, the answer given, and the
/// step it was given on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reviewed {
    pub question: String,
    pub answer: String,
    pub step: Option<usize>,
}

/// Where to step in a prompt of several questions: back or forward, as ←
/// and → do, or to a step by its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepTo {
    Back,
    Forward,
    Named(String),
}

impl StepTo {
    /// `back`, `forward`, or a step's name.
    pub fn parse(text: &str) -> StepTo {
        match text {
            "back" => StepTo::Back,
            "forward" => StepTo::Forward,
            name => StepTo::Named(name.to_string()),
        }
    }
}

/// The steps on the line of a prompt's questions' names (`← ☐ Colors  ☒
/// Size  ✔ Submit →`): each name, ☒ answered, and Submit last. Only a line
/// ending in Submit has steps, and only with two questions or more, since
/// a prompt of one question shows its name alone.
fn steps_in(line: &str) -> Vec<QuestionStep> {
    const MARKS: [char; 3] = ['☐', '☒', '✔'];
    let chars: Vec<char> = line.chars().collect();
    let at: Vec<usize> = (0..chars.len())
        .filter(|&i| MARKS.contains(&chars[i]))
        .collect();
    let steps: Vec<QuestionStep> = at
        .iter()
        .enumerate()
        .map(|(k, &i)| {
            let end = at.get(k + 1).copied().unwrap_or(chars.len());
            let name: String = chars[i + 1..end].iter().collect();
            let name = name.trim().trim_end_matches('→').trim().to_string();
            QuestionStep {
                submit: chars[i] == '✔',
                answered: chars[i] == '☒',
                name,
            }
        })
        .collect();
    let questions = steps.iter().filter(|s| !s.submit).count();
    let ends_in_submit = steps.last().is_some_and(|s| s.submit) && questions == steps.len() - 1;
    if ends_in_submit && questions >= 2 {
        steps
    } else {
        Vec::new()
    }
}

/// Claude Code's review of the answers on Submit's step: under "Review
/// your answers", each question, then `→` and its answer, a line set in
/// past the arrow more of the answer. Each is matched to the answered
/// steps in order, which are the questions it lists.
fn review_in(lines: &[&str], steps: &[QuestionStep]) -> Vec<Reviewed> {
    let Some(title) = lines.iter().position(|l| l.trim() == "Review your answers") else {
        return Vec::new();
    };
    let indent = |l: &str| l.chars().take_while(|c| c.is_whitespace()).count();
    let mut out: Vec<Reviewed> = Vec::new();
    let mut question: Vec<&str> = Vec::new();
    let mut arrow_at: Option<usize> = None;
    for l in &lines[title + 1..] {
        let t = l.trim();
        if let Some(answer) = t.strip_prefix('→') {
            arrow_at = Some(indent(l));
            out.push(Reviewed {
                question: question.join(" "),
                answer: answer.trim().to_string(),
                step: None,
            });
            question.clear();
        } else if let (Some(at), true, Some(last)) = (arrow_at, question.is_empty(), out.last_mut())
            && indent(l) > at
        {
            last.answer.push(' ');
            last.answer.push_str(t);
        } else if t.starts_with('⚠') || t == "Some questions are unanswered" {
            continue;
        } else {
            arrow_at = None;
            question.push(t.trim_start_matches(['●', '•', '·', '-']).trim());
        }
    }
    let answered: Vec<usize> = (0..steps.len()).filter(|&i| steps[i].answered).collect();
    if answered.len() == out.len() {
        for (r, &step) in out.iter_mut().zip(&answered) {
            r.step = Some(step);
        }
    }
    out
}

/// Which step the line of names shows, given the screen's rows as drawn:
/// Claude Code draws the one shown on a background of its own, which the
/// text loses. The last row with the names is read.
pub fn read_shown(found: &mut Prompt, rows: &[Vec<Span<'_>>]) {
    if found.steps.is_empty() {
        return;
    }
    let Some(row) = rows.iter().rev().find(|r| {
        let text: String = r.iter().map(|s| s.content.as_ref()).collect();
        QUESTION_TABS.is_match(&text) && !steps_in(&text).is_empty()
    }) else {
        return;
    };
    let mut lit: Vec<String> = Vec::new();
    let mut joined = false;
    for span in row {
        let on = span
            .style
            .bg
            .is_some_and(|bg| bg != ratatui::style::Color::Reset);
        match (on, joined, lit.last_mut()) {
            (true, true, Some(last)) => last.push_str(&span.content),
            (true, _, _) => lit.push(span.content.to_string()),
            _ => {}
        }
        joined = on;
    }
    found.shown = lit.iter().find_map(|text| {
        let name = text.trim().trim_start_matches(['☐', '☒', '✔']).trim();
        (!name.is_empty())
            .then(|| found.steps.iter().position(|s| s.name == name))
            .flatten()
    });
}

/// What stepping takes in a prompt of several questions: ← back, → forward,
/// or as many of either as a named step lies from the one shown.
pub fn step_presses(found: &Prompt, to: &StepTo) -> Result<(Vec<Press>, String), String> {
    if found.steps.is_empty() {
        return Err("its prompt has no steps".into());
    }
    let last = found.steps.len() - 1;
    let target = match (to, found.shown) {
        (StepTo::Back, Some(0)) => return Err("it already shows the first step".into()),
        (StepTo::Forward, Some(at)) if at == last => {
            return Err("it already shows the last step".into());
        }
        (StepTo::Back, Some(at)) => at - 1,
        (StepTo::Forward, Some(at)) => at + 1,
        // Unread, a step could press past either end, where Claude Code
        // takes the key as nothing and the phone waits on a change.
        (_, None) => return Err("which step it shows can't be read".into()),
        (StepTo::Named(name), Some(_)) => found
            .steps
            .iter()
            .position(|s| &s.name == name)
            .ok_or_else(|| format!("\"{name}\" isn't one of its steps"))?,
    };
    let at = found.shown.unwrap_or(0);
    if target == at {
        return Err(format!("it already shows {}", found.steps[at].name));
    }
    let code = if target > at {
        KeyCode::Right
    } else {
        KeyCode::Left
    };
    // In the free option's field (an answer of your own typed into a
    // question of several answers leaves the ❯ there), ← and → move the
    // text's cursor, so ↑ leaves the field first, keeping what was typed.
    let in_field = found.free.is_some() && found.cursor == found.free.map(Cursor::Option);
    let leave = in_field.then(|| Press::Key(Key::new(KeyCode::Up, Mods::NONE)));
    let presses = leave
        .into_iter()
        .chain((0..target.abs_diff(at)).map(|_| Press::Key(Key::new(code, Mods::NONE))))
        .collect();
    Ok((presses, format!("Stepped to {}", found.steps[target].name)))
}

/// The numbered choices a waiting tab is showing (a permission prompt, or a
/// question Claude asked), or `None`. Read from the bottom of the screen:
/// the last run of lines numbered 1, 2, 3…, which may have description
/// lines between them, and the few lines above it as the question.
///
/// A question Claude asks with AskUserQuestion has more to it. Its options
/// end with "Type something", for an answer of your own, and a rule sets
/// "Chat about this" apart below; that last option above the rule is
/// `free`, found by its place. With several questions they come one at a
/// time, under a line of their names marked ☐ or ☒, read as `steps`, with
/// Submit's step reviewing the answers (`review`). A question that takes
/// several answers (`multi`) shows each option as [ ] or [✔], and a Submit
/// row after the options moves on. One whose options carry a preview draws
/// it in a box to their right (`preview`), its edge on the options' rows.
///
/// A screen scrolled back (`scrolled_back`) shows none: the numbered lines
/// on it are the transcript's, a question answered long ago, while the
/// prompt Claude waits on is out of sight below.
pub fn options_in(text: &str) -> Option<Prompt> {
    if scrolled_back(text) {
        return None;
    }
    let raw: Vec<&str> = text.split('\n').collect();
    // A preview beside the options is a box whose top corner, the first of
    // a box on its row, has words to its left (a prompt drawn in a box of
    // its own has none); every row with the box's edge at that column is
    // read without the box, so nothing drawn in it (a numbered list, a
    // divider) passes for an option, a description or the rule.
    let beside: Option<usize> = raw.iter().rev().find_map(|l| {
        let chars: Vec<char> = l.chars().collect();
        let at = chars.iter().position(|c| BOX_EDGE.contains(c))?;
        let words_left = chars[..at].iter().any(|c| !c.is_whitespace());
        (['┌', '╭'].contains(&chars[at]) && words_left).then_some(at)
    });
    let edge_at = |l: &str, at: usize| l.chars().nth(at).is_some_and(|c| BOX_EDGE.contains(&c));
    let mapped: Vec<String> = raw
        .iter()
        .map(|l| match beside {
            Some(at) if edge_at(l, at) => l.chars().take(at).collect(),
            _ => l.to_string(),
        })
        .map(|l| l.replace(['│', '┃'], " ").trim_end().to_string())
        .collect();
    let kept: Vec<usize> = (0..mapped.len())
        .filter(|&i| !mapped[i].trim().is_empty() && (i == 0 || mapped[i] != mapped[i - 1]))
        .collect();
    let lines: Vec<&str> = kept.iter().map(|&i| mapped[i].as_str()).collect();
    struct Found {
        n: u32,
        label: String,
        checked: Option<bool>,
        line: usize,
        marked: bool,
        cut: bool,
    }
    let mut options: Vec<Found> = Vec::new();
    let mut first: Option<usize> = None;
    for i in (0..lines.len()).rev() {
        let Some(m) = OPTION.captures(lines[i]) else {
            continue;
        };
        let n: u32 = m[1].parse().unwrap_or(0);
        if !options.iter().any(|o| o.n == n) {
            let cut = LABEL_TAIL.is_match(&m[2]);
            let label = LABEL_TAIL.replace(&m[2], "").to_string();
            let (label, checked) = match CHECKBOX.captures(&label) {
                Some(b) => (b[2].to_string(), Some(&b[1] != " ")),
                None => (label, None),
            };
            let marked = lines[i].trim_start().starts_with(['❯', '›', '>']);
            options.push(Found {
                n,
                label,
                checked,
                line: i,
                marked,
                cut,
            });
        }
        first = Some(i);
        if n == 1 {
            break;
        }
    }
    options.sort_by_key(|o| o.n);
    if options.len() < 2 || options.iter().enumerate().any(|(i, o)| o.n != i as u32 + 1) {
        return None;
    }
    let first = first?;
    let last = options.last()?.line;
    // A line between an option and the next is more of it; under the last,
    // only a line set in past its number is, the screen's hints below
    // starting further out.
    let number_at = |l: &str| l.find(|c: char| c.is_ascii_digit()).unwrap_or(0);
    let indent = |l: &str| l.len() - l.trim_start().len();
    let under: Vec<Vec<&str>> = options
        .iter()
        .enumerate()
        .map(|(at, o)| match options.get(at + 1) {
            Some(next) => lines
                .get(o.line + 1..next.line)
                .unwrap_or_default()
                .iter()
                .filter(|l| !RULE.is_match(l) && !SUBMIT_ROW.is_match(l))
                .copied()
                .collect(),
            None => lines[o.line + 1..]
                .iter()
                .take_while(|l| indent(l) > number_at(lines[o.line]))
                .copied()
                .collect(),
        })
        .collect();
    let more: Vec<u32> = options
        .iter()
        .zip(&under)
        .filter(|(o, under)| o.cut || !under.is_empty())
        .map(|(o, _)| o.n)
        .collect();
    let preview = beside.is_some_and(|at| options.iter().any(|o| edge_at(raw[kept[o.line]], at)));
    let rule = (first + 1..last).find(|&i| RULE.is_match(lines[i]));
    let submit = (first + 1..last).find(|&i| SUBMIT_ROW.is_match(lines[i]));
    let free = rule.and_then(|r| options.iter().rfind(|o| o.line < r).map(|o| o.n));
    let multi = options.iter().any(|o| o.checked.is_some());
    let cursor = match submit {
        Some(s) if lines[s].contains('❯') => Some(Cursor::Submit),
        _ => options
            .iter()
            .find(|o| o.marked)
            .map(|o| Cursor::Option(o.n)),
    };
    // The question is the few lines just above the options, back to the
    // nearest rule or the names of the questions; under the names it is
    // every line back to them.
    let named = lines[first.saturating_sub(13)..first]
        .iter()
        .any(|l| QUESTION_TABS.is_match(l));
    // The line of names is the nearest above the options, back to the
    // prompt's rule, however many answers Submit's review lists.
    let names_at = (0..first)
        .rev()
        .take_while(|&i| !RULE.is_match(lines[i]))
        .find(|&i| QUESTION_TABS.is_match(lines[i]));
    let steps = names_at.map(|i| steps_in(lines[i])).unwrap_or_default();
    let review = match names_at {
        Some(i) if !steps.is_empty() => review_in(&lines[i + 1..first], &steps),
        _ => Vec::new(),
    };
    let most = if named { 12 } else { 3 };
    let mut above: Vec<&str> = Vec::new();
    for i in (0..first).rev() {
        let l = lines[i];
        if above.len() >= most
            || RULE.is_match(l)
            || QUESTION_TABS.is_match(l)
            || OPTION.is_match(l)
        {
            break;
        }
        above.insert(0, l.trim());
    }
    Some(Prompt {
        question: above.join(if named { "\n" } else { " " }),
        options: options
            .into_iter()
            .zip(under)
            .map(|(o, under)| PromptOption {
                n: o.n,
                label: o.label,
                checked: o.checked,
                detail: under.iter().map(|l| l.trim()).collect::<Vec<_>>().join(" "),
            })
            .collect(),
        multi,
        preview,
        free,
        cursor: free.and(cursor),
        more,
        steps,
        shown: None,
        review,
    })
}

/// Your reply to a prompt, as what to press.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// An option's number.
    Choose(PromptOption),
    /// Options to tick or untick, in a question that takes several.
    Toggle(Vec<PromptOption>),
    /// Your own answer, in the free option.
    Free(String),
    /// Submit, in a question that takes several.
    Submit,
}

/// Your answer as one of the options: its number, or the start of its text.
pub fn chosen_option(options: &[PromptOption], text: &str) -> Option<PromptOption> {
    if !text.is_empty() && text.len() <= 2 && text.chars().all(|c| c.is_ascii_digit()) {
        let n: u32 = text.parse().ok()?;
        return options.iter().find(|o| o.n == n).cloned();
    }
    if text.chars().count() < 2 {
        return None;
    }
    let lower = text.to_lowercase();
    options
        .iter()
        .find(|o| o.label.to_lowercase().starts_with(&lower))
        .cloned()
}

/// What a reply presses: an option's number, options to tick, Submit, or
/// your own answer; `None` when it is none of those.
pub fn answer_for(found: &Prompt, text: &str) -> Option<Answer> {
    static SUBMIT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^(submit|done|next)$").expect("a regex"));
    static NUMBERS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\d{1,2}(?:[\s,]+\d{1,2})+$").expect("a regex"));
    static SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\s,]+").expect("a regex"));
    let reply = text.trim();
    let fixed: Vec<PromptOption> = found
        .options
        .iter()
        .filter(|o| Some(o.n) != found.free)
        .cloned()
        .collect();
    if found.multi && SUBMIT.is_match(reply) {
        return Some(Answer::Submit);
    }
    if found.multi && NUMBERS.is_match(reply) {
        let mut seen: Vec<&str> = Vec::new();
        for n in SPLIT.split(reply) {
            if !seen.contains(&n) {
                seen.push(n);
            }
        }
        let options: Option<Vec<PromptOption>> =
            seen.iter().map(|n| chosen_option(&fixed, n)).collect();
        if let Some(options) = options {
            return Some(Answer::Toggle(options));
        }
    }
    if let Some(option) = chosen_option(&fixed, reply) {
        return Some(if found.multi {
            Answer::Toggle(vec![option])
        } else {
            Answer::Choose(option)
        });
    }
    if found.free.is_some() && !reply.is_empty() {
        return Some(Answer::Free(reply.to_string()));
    }
    None
}

/// What an answer did, in words.
pub fn answer_said(answer: &Answer) -> String {
    match answer {
        Answer::Choose(o) => format!("Chose {}. {}", o.n, o.label),
        Answer::Toggle(options) => options
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let word = match (o.checked == Some(true), i) {
                    (true, 0) => "Unticked",
                    (true, _) => "unticked",
                    (false, 0) => "Ticked",
                    (false, _) => "ticked",
                };
                format!("{word} {}. {}", o.n, o.label)
            })
            .collect::<Vec<_>>()
            .join(", "),
        Answer::Free(text) => format!("Answered \"{text}\""),
        Answer::Submit => "Submitted your choices".into(),
    }
}

/// One thing pressed in a pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Press {
    /// A key, encoded for the pane's modes.
    Key(Key),
    /// Text typed as it is.
    Text(String),
    /// Text pasted (bracketed, when the program asks for it).
    Paste(String),
    /// A pause for a paste to land before what follows.
    Settle,
}

/// What pressing an answer takes, in a pane showing `found`: a number picks
/// or ticks its option; Submit and a free answer in a question that takes
/// several are reached with the arrow keys from wherever the ❯ is, since a
/// number only ticks; in one that takes one answer, the free option's
/// number opens it for typing, and Enter sends what was typed. Beside a
/// preview a number only moves the ❯ to its option, so Enter follows it.
pub fn presses(found: &Prompt, answer: &Answer) -> Vec<Press> {
    let rows = found.free.unwrap_or(found.options.len() as u32);
    let at = match found.cursor {
        Some(Cursor::Submit) => rows + 1,
        Some(Cursor::Option(n)) => n,
        None => 1,
    };
    let move_to = |row: u32| -> Vec<Press> {
        let code = if row > at { KeyCode::Down } else { KeyCode::Up };
        (0..row.abs_diff(at))
            .map(|_| Press::Key(Key::new(code, Mods::NONE)))
            .collect()
    };
    let enter = Press::Key(Key::new(KeyCode::Enter, Mods::NONE));
    match answer {
        Answer::Submit => {
            let mut out = move_to(rows + 1);
            out.push(enter);
            out
        }
        Answer::Free(text) if found.multi => {
            let mut out = move_to(found.free.unwrap_or(rows));
            out.push(Press::Paste(text.clone()));
            out.push(Press::Settle);
            out
        }
        Answer::Free(text) => vec![
            Press::Text(found.free.unwrap_or(rows).to_string()),
            Press::Paste(text.clone()),
            Press::Settle,
            enter,
        ],
        Answer::Choose(o) if found.preview => {
            vec![Press::Text(o.n.to_string()), Press::Settle, enter]
        }
        Answer::Choose(o) => vec![Press::Text(o.n.to_string())],
        Answer::Toggle(options) => options
            .iter()
            .map(|o| Press::Text(o.n.to_string()))
            .collect(),
    }
}

/// A message typed into a tab showing no prompt: pasted, then Enter once it
/// has landed.
pub fn message_presses(text: &str) -> Vec<Press> {
    vec![
        Press::Paste(text.to_string()),
        Press::Settle,
        Press::Key(Key::new(KeyCode::Enter, Mods::NONE)),
    ]
}

/// Whether Claude Code's prompt, on this screen, is in vim's Normal mode:
/// with Claude Code's vim mode on, its mode line at the screen's foot reads
/// `-- NORMAL --` there (claude-pane.sh's `claude_type`, tmux.js's
/// `typeIntoClaude`). The last three lines with anything on them are read.
pub fn normal_mode(screen: &str) -> bool {
    screen
        .lines()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .take(3)
        .any(|l| l.contains("-- NORMAL --"))
}

/// What typing `presses` into Claude Code's prompt takes on this screen:
/// `i` first in vim's Normal mode, where the first letters typed would be
/// taken as vim commands until one started Insert mode, cutting off the
/// front of what is typed.
pub fn into_prompt(screen: &str, presses: Vec<Press>) -> Vec<Press> {
    if normal_mode(screen) {
        std::iter::once(Press::Text("i".into()))
            .chain(presses)
            .collect()
    } else {
        presses
    }
}

// ── The box ─────────────────────────────────────────────────────────────────

/// What the box gives back as its outcome's value: an option's number,
/// `submit`, or the text typed.
pub struct AnswerBox {
    title: String,
    found: Option<Prompt>,
    field: Field,
    /// The box's lines that hold the field's rows.
    field_lines: std::ops::Range<usize>,
    typing: bool,
    note: Option<String>,
    /// The search `/` opens over the options.
    filter: Filter,
    /// The line the search was last drawn on, while it is open.
    search_line: Option<usize>,
    /// Where Tab has put the highlight: one of the options shown, or past
    /// the last the close button.
    focus: Option<usize>,
    close_focus: bool,
    /// The option under the mouse.
    lit: Option<u32>,
    drawn: Option<Drawn>,
    /// Which of the box's lines holds which option.
    option_rows: Vec<(usize, u32)>,
}

impl AnswerBox {
    pub fn new(title: String, found: Option<Prompt>, note: Option<String>) -> AnswerBox {
        AnswerBox {
            title,
            found,
            field: Field::new().multi_line(),
            field_lines: 0..0,
            typing: false,
            note,
            filter: Filter::default(),
            search_line: None,
            focus: None,
            close_focus: false,
            lit: None,
            drawn: None,
            option_rows: Vec::new(),
        }
    }

    /// The prompt it shows.
    pub fn found(&self) -> Option<&Prompt> {
        self.found.as_ref()
    }

    fn give(reply: impl Into<String>) -> Step {
        Step::Close(Outcome::Picked(Picked {
            index: 0,
            value: reply.into(),
            query: String::new(),
        }))
    }

    /// The options shown, by their place: every one while the search is
    /// closed.
    fn shown(&self) -> Vec<usize> {
        let labels: Vec<&str> = self
            .found
            .iter()
            .flat_map(|f| f.options.iter().map(|o| o.label.as_str()))
            .collect();
        self.filter.shown(&labels)
    }

    fn typing_line(&self) -> bool {
        self.typing || self.found.is_none()
    }
}

impl Overlay for AnswerBox {
    fn drawn(&self) -> Option<&crate::ui::draw::Drawn> {
        self.drawn.as_ref()
    }

    fn kind(&self) -> &'static str {
        "answer"
    }

    fn stands_out(&self) -> Option<ratatui::layout::Rect> {
        self.drawn.as_ref().map(|d| d.outer)
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let inner = (buf.area.width as usize).saturating_sub(4).min(90);
        let mut lines: Vec<BoxLine> = Vec::new();
        self.option_rows.clear();
        let typing = self.typing_line();
        if let Some(found) = &self.found {
            let mut asked: Vec<String> = found
                .question
                .split('\n')
                .flat_map(|line| draw::wrap(line, inner.saturating_sub(2)))
                .collect();
            // While searching, a question too long for the screen gives up
            // its last rows, so the search and a match still fit in the box
            // (four rows of its own and two of keys).
            // While typing, it gives them up so the options, a blank and a
            // row of the field (and the note) still fit under it.
            let room = (buf.area.height as usize).saturating_sub(6);
            let most = if typing {
                let below = if self.note.is_some() { 2 } else { 0 };
                room.saturating_sub(found.options.len() + 3 + below).max(1)
            } else {
                room.saturating_sub(4).max(1)
            };
            if (self.filter.is_open() || typing) && asked.len() > most {
                asked.truncate(most - 1);
                asked.push("…".into());
            }
            for w in asked {
                lines.push(BoxLine::text(format!(" {w}"), look.bright()));
            }
            lines.push(BoxLine::blank());
            let shown = self.shown();
            self.search_line = self.filter.is_open().then_some(lines.len());
            if self.filter.is_open() {
                lines.push(self.filter.line(
                    inner.saturating_sub(2),
                    shown.len(),
                    found.options.len(),
                    look,
                ));
                lines.push(BoxLine::blank());
                if shown.is_empty() {
                    lines.push(BoxLine::text(NO_MATCH, look.dim()));
                }
            }
            // While searching, as many matches as the screen has rows for
            // under the question and the search, the chosen one among them.
            let (first, fits) = if self.filter.is_open() {
                let room = room.saturating_sub(lines.len()).max(1);
                let at = self.focus.unwrap_or(0);
                (at.saturating_sub(room - 1), room)
            } else {
                (0, shown.len())
            };
            for (at, o) in shown
                .iter()
                .map(|i| &found.options[*i])
                .enumerate()
                .skip(first)
                .take(fits)
            {
                let chosen = self.focus == Some(at) && !typing;
                let lit = !chosen && !typing && self.lit == Some(o.n);
                let reverse = |s: Style| {
                    if lit {
                        s.add_modifier(Modifier::REVERSED)
                    } else {
                        s
                    }
                };
                let boxed = match o.checked {
                    None => "",
                    Some(true) => "☑ ",
                    Some(false) => "☐ ",
                };
                let label = if chosen {
                    look.bold_bright()
                } else {
                    look.text()
                };
                self.option_rows.push((lines.len(), o.n));
                // While a custom answer is typed, the options take no click.
                lines.push(BoxLine {
                    clickable: !typing,
                    ..BoxLine::spans(vec![
                        Span::raw(" "),
                        Span::styled(
                            o.n.to_string(),
                            reverse(Style::new().fg(draw::YELLOW).add_modifier(Modifier::BOLD)),
                        ),
                        Span::raw("  "),
                        Span::styled(
                            format!("{boxed}{}", draw::cut(&o.label, inner.saturating_sub(8))),
                            reverse(label),
                        ),
                    ])
                    .fill(chosen.then_some(look.theme.highlight))
                });
            }
        } else {
            lines.push(BoxLine::text(
                " No question showing: what you type is sent to it as a message.",
                look.dim(),
            ));
        }
        if typing {
            lines.push(BoxLine::blank());
            let first = lines.len();
            // Up to half the screen, and never more than the box has rows
            // for under the question and the options (and the note after).
            let below = if self.note.is_some() { 2 } else { 0 };
            let left = (buf.area.height as usize).saturating_sub(6 + lines.len() + below);
            let most = (buf.area.height as usize / 2)
                .saturating_sub(4)
                .min(left)
                .max(1);
            let rows = self.field.view(
                inner.saturating_sub(4),
                most,
                look.bright(),
                look.dim(),
                true,
            );
            for (i, row) in rows.into_iter().enumerate() {
                let lead = if i == 0 { " › " } else { "   " };
                let mut spans = vec![Span::styled(lead, look.bright())];
                spans.extend(row);
                lines.push(BoxLine::spans(spans));
            }
            self.field_lines = first..lines.len();
        }
        if let Some(note) = &self.note {
            lines.push(BoxLine::blank());
            lines.push(BoxLine::text(format!(" {note}"), look.dim()));
        }
        let keys = match &self.found {
            Some(_) if self.filter.is_open() => SEARCH_KEYS.to_string(),
            Some(found) if !typing => format!(
                "1-{} or tab ↵ choose{}{}  esc back",
                found.options.len(),
                if found.multi { "  s submit" } else { "" },
                if found.free.is_some() {
                    "  t type your own"
                } else {
                    ""
                }
            ),
            _ => "type  ⇧↵ new line  ↵ send  esc back".to_string(),
        };
        let mut spec = BoxSpec::new(&self.title, &lines);
        spec.keys = Some(&keys);
        spec.width = inner as u16;
        let options = self.shown().len();
        spec.close_lit = self.close_focus || (!typing && self.focus == Some(options));
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn key(&mut self, key: Key, look: &Look) -> Step {
        let k = key.normalized();
        let plain = k.mods.is_empty();
        if !self.typing_line() {
            match self.filter.key(k) {
                Filtered::Changed | Filtered::Closed => {
                    self.focus = self.filter.is_open().then_some(0);
                    return Step::Redraw;
                }
                Filtered::Moved => return Step::Redraw,
                Filtered::Not => {}
            }
        }
        if self.filter.is_open() {
            let shown = self.shown();
            let last = shown.len().saturating_sub(1);
            let at = self.focus.unwrap_or(0);
            let to = match k.code {
                KeyCode::Enter => {
                    return match (shown.get(at), &self.found) {
                        (Some(i), Some(found)) => AnswerBox::give(found.options[*i].n.to_string()),
                        _ => Step::Same,
                    };
                }
                KeyCode::Down => wrap_step(at, shown.len(), true),
                KeyCode::Char('n') if k.mods == Mods::CTRL => wrap_step(at, shown.len(), true),
                KeyCode::Up => wrap_step(at, shown.len(), false),
                KeyCode::Char('p') if k.mods == Mods::CTRL => wrap_step(at, shown.len(), false),
                KeyCode::Tab => at + 1,
                KeyCode::BackTab => at.saturating_sub(1),
                KeyCode::Char('c') if k.mods == Mods::CTRL => {
                    return Step::Close(Outcome::Dismissed);
                }
                _ => return Step::Same,
            }
            .min(last);
            self.focus = Some(to);
            return Step::Redraw;
        }
        if k.code == KeyCode::Esc || (k.code == KeyCode::Char('c') && k.mods == Mods::CTRL) {
            return Step::Close(Outcome::Dismissed);
        }
        if self.typing_line() {
            // Tab puts focus on easy mode's close button, Enter pressing it;
            // typing takes it back to the line.
            if matches!(k.code, KeyCode::Tab | KeyCode::BackTab) && look.close {
                self.close_focus = !self.close_focus;
                return Step::Redraw;
            }
            if self.close_focus && matches!(k.code, KeyCode::Enter | KeyCode::Char(' ')) {
                return Step::Close(Outcome::Dismissed);
            }
            self.close_focus = false;
            return match self.field.key(k) {
                Edit::Changed | Edit::Moved => Step::Redraw,
                Edit::Not if k.code == KeyCode::Enter => {
                    AnswerBox::give(self.field.text().to_string())
                }
                _ => Step::Same,
            };
        }
        let Some(found) = &self.found else {
            return Step::Same;
        };
        let options = found.options.len();
        let stops = options + usize::from(look.close);
        let ctrl = k.mods == Mods::CTRL;
        let arrow_down = k.code == KeyCode::Down
            || (ctrl && k.code == KeyCode::Char('n'))
            || (plain && k.code == KeyCode::Char('j'));
        let arrow_up = k.code == KeyCode::Up
            || (ctrl && k.code == KeyCode::Char('p'))
            || (plain && k.code == KeyCode::Char('k'));
        if (arrow_down || arrow_up) && options > 0 {
            self.focus = Some(match self.focus {
                None if arrow_down => 0,
                None => options - 1,
                Some(at) => wrap_step(at, options, arrow_down),
            });
            return Step::Redraw;
        }
        match k.code {
            KeyCode::Tab | KeyCode::BackTab if stops > 0 => {
                let forward = k.code == KeyCode::Tab;
                self.focus = Some(match self.focus {
                    None => {
                        if forward {
                            0
                        } else {
                            stops - 1
                        }
                    }
                    Some(at) if at >= stops => 0,
                    Some(at) if forward => (at + 1) % stops,
                    Some(at) => (at + stops - 1) % stops,
                });
                Step::Redraw
            }
            KeyCode::Enter | KeyCode::Char(' ') if plain || k.code == KeyCode::Enter => {
                match self.focus {
                    Some(at) if at == options && look.close => Step::Close(Outcome::Dismissed),
                    Some(at) if at < options => AnswerBox::give(found.options[at].n.to_string()),
                    _ => Step::Same,
                }
            }
            KeyCode::Char(c @ '1'..='9') if plain => AnswerBox::give(c.to_string()),
            KeyCode::Char('s') if plain && found.multi => AnswerBox::give("submit"),
            KeyCode::Char('t') if plain && found.free.is_some() => {
                self.typing = true;
                Step::Redraw
            }
            _ => Step::Same,
        }
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        let Some(drawn) = &self.drawn else {
            return Step::Same;
        };
        let hit = drawn.hit(mouse.col, mouse.row);
        let option = match hit {
            Hit::Line(i, _) if !self.typing_line() => self
                .option_rows
                .iter()
                .find(|(line, _)| *line == i)
                .map(|(_, n)| *n),
            _ => None,
        };
        match draw::gesture(&mouse) {
            Gesture::Click if look.mouse => match (hit, option) {
                (_, Some(n)) => AnswerBox::give(n.to_string()),
                (Hit::Line(i, col), _) if Some(i) == self.search_line => {
                    self.filter.click(col as usize);
                    Step::Redraw
                }
                (Hit::Line(i, col), _) if self.typing_line() && self.field_lines.contains(&i) => {
                    self.close_focus = false;
                    self.field
                        .click(i - self.field_lines.start, (col as usize).saturating_sub(3));
                    Step::Redraw
                }
                (Hit::Close | Hit::Outside, _) => Step::Close(Outcome::Dismissed),
                _ => Step::Same,
            },
            Gesture::Move if look.hover => {
                let close = hit == Hit::Close;
                if option == self.lit && close == self.close_focus {
                    Step::Same
                } else {
                    self.lit = option;
                    self.close_focus = close;
                    Step::Redraw
                }
            }
            _ => Step::Same,
        }
    }

    fn paste(&mut self, text: &str, _look: &Look) -> Step {
        if self.filter.paste(text) {
            self.focus = Some(0);
            return Step::Redraw;
        }
        if !self.typing_line() {
            return Step::Same;
        }
        self.field.paste(text);
        Step::Redraw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PERMISSION: &str = "\
╭───────────────────────────────────────────╮
│ Bash command                              │
│                                           │
│   git push origin main                    │
│                                           │
│ Do you want to proceed?                   │
│ ❯ 1. Yes                                  │
│   2. Yes, and don't ask again for git push│
│   3. No, and tell Claude what to do       │
╰───────────────────────────────────────────╯";

    #[test]
    fn the_arrows_choose_an_answer_and_wrap_round() {
        let look = Look::default();
        let mut b = AnswerBox::new("Answer 2 fix".into(), options_in(PERMISSION), None);
        let options = b.found.as_ref().unwrap().options.len();
        b.key(Key::new(KeyCode::Up, Mods::NONE), &look);
        assert_eq!(b.focus, Some(options - 1), "Up first chooses the last");
        b.key(Key::new(KeyCode::Down, Mods::NONE), &look);
        assert_eq!(b.focus, Some(0), "Down on the last chooses the first");
        b.key(Key::new(KeyCode::Char('p'), Mods::CTRL), &look);
        assert_eq!(b.focus, Some(options - 1));
        b.key(Key::new(KeyCode::Char('j'), Mods::NONE), &look);
        assert_eq!(b.focus, Some(0), "j on the last chooses the first");
        b.key(Key::new(KeyCode::Char('k'), Mods::NONE), &look);
        assert_eq!(
            b.focus,
            Some(options - 1),
            "k on the first chooses the last"
        );
        assert!(matches!(
            b.key(Key::new(KeyCode::Enter, Mods::NONE), &look),
            Step::Close(Outcome::Picked(p)) if p.value == options.to_string()
        ));
    }

    #[test]
    fn a_permission_prompts_question_and_options_are_read_off_the_screen() {
        let found = options_in(PERMISSION).expect("a prompt");
        assert_eq!(
            found.question,
            "Bash command git push origin main Do you want to proceed?"
        );
        let labels: Vec<&str> = found.options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Yes",
                "Yes, and don't ask again for git push",
                "No, and tell Claude what to do"
            ]
        );
        assert!(!found.multi);
        assert_eq!(found.free, None);
        assert_eq!(
            found.cursor, None,
            "a permission prompt has no ❯ of its own to follow"
        );
        assert_eq!(options_in("$ ls\nfoo bar\n$ "), None);
        assert_eq!(options_in("1. only one"), None);
        assert!(found.more.is_empty(), "every label shown whole");
    }

    #[test]
    fn a_fullscreen_view_scrolled_back_over_a_permission_prompt_shows_none() {
        // Claude Code's fullscreen view, scrolled up while a Bash
        // permission prompt waits below: the screen holds a question
        // answered earlier, and the pill saying what is out of sight.
        let screen = "\
⏺ Which color?
  1. Red
  2. Blue
  3. Green

⏺ Ran 2 stop hooks

✻ Cogitated for 1s · done 11:23 AM

❯ Run this exact bash command with the Bash tool: node scripts/build.js
  ⎿  $ node scripts/build.js

⏺ line 1
  line 2
  line 3                                                      Waiting for your answer · fn+↓ to scroll
";
        assert!(scrolled_back(screen));
        assert_eq!(options_in(screen), None, "not the earlier question");
        let live = "\
⏺ Which color?
  1. Red
  2. Blue
  3. Green

 Bash command
 node scripts/build.js
 Do you want to proceed?
 ❯ 1. Yes
   2. Yes, and always allow access to scripts/ from this project
   3. No

 Esc to cancel · Tab to amend
";
        assert!(!scrolled_back(live));
        let found = options_in(live).expect("the permission prompt");
        assert_eq!(
            found.question,
            "Bash command node scripts/build.js Do you want to proceed?"
        );
        assert_eq!(found.options.len(), 3);
        assert_eq!(found.options[0].label, "Yes");
    }

    #[test]
    fn an_option_with_more_on_screen_than_its_label_is_marked() {
        let screen = "\
 Do you want to proceed?
 ❯ 1. Yes
   2. Yes, and don't ask again for cargo test
      commands in this project
   3. No  (esc)
      and tell Claude what to do
   4. Later

 Esc to cancel · Tab to amend";
        let found = options_in(screen).expect("a prompt");
        assert_eq!(
            found.more,
            vec![2, 3],
            "a line under 2, and 3 cut and under"
        );
        let last_wrapped = "\
 ❯ 1. Yes
   2. No, and tell Claude what
      to do differently";
        assert_eq!(options_in(last_wrapped).expect("a prompt").more, vec![2]);
    }

    #[test]
    fn each_choice_keeps_the_explainer_under_it_and_the_hints_belong_to_none() {
        let screen = "\
 Which database should the tests use?

 ❯ 1. Postgres
      The same database production runs
   2. SQLite
      Quicker to start, in a file of its own
   3. In memory
      Nothing written to disk
   4. Type something
 ──────────────────
   5. Chat about this

 Enter to select · ↑/↓ to navigate · Esc to cancel";
        let found = options_in(screen).expect("a prompt");
        let details: Vec<&str> = found.options.iter().map(|o| o.detail.as_str()).collect();
        assert_eq!(
            details,
            [
                "The same database production runs",
                "Quicker to start, in a file of its own",
                "Nothing written to disk",
                "",
                ""
            ],
            "the rule isn't 4's, and the hints aren't 5's"
        );
        assert_eq!(found.more, vec![1, 2, 3]);
    }

    #[test]
    fn a_question_of_several_answers_ticks_and_submits_and_its_free_option_is_found() {
        let screen = "\
 ☐ Colors  ☒ Size  ✔ Submit →

Which colors?

❯ 1. [ ] Red
  2. [✔] Blue
  3. [ ] Type something
──────────────────
  4. Chat about this
   Submit";
        let found = options_in(screen).expect("a prompt");
        assert!(found.multi);
        assert_eq!(found.free, Some(3));
        assert_eq!(found.cursor, Some(Cursor::Option(1)));
        assert_eq!(found.options[1].checked, Some(true));
        assert_eq!(found.options[1].label, "Blue");
        assert_eq!(found.question, "Which colors?");
        assert_eq!(answer_for(&found, "submit"), Some(Answer::Submit));
        let toggled = answer_for(&found, "1 2").unwrap();
        assert_eq!(answer_said(&toggled), "Ticked 1. Red, unticked 2. Blue");
        assert_eq!(
            answer_for(&found, "Green and teal"),
            Some(Answer::Free("Green and teal".into()))
        );
        assert_eq!(
            presses(&found, &Answer::Free("teal".into())),
            vec![
                Press::Key(Key::new(KeyCode::Down, Mods::NONE)),
                Press::Key(Key::new(KeyCode::Down, Mods::NONE)),
                Press::Paste("teal".into()),
                Press::Settle,
            ],
            "reached with the arrows from where the ❯ is, since a number only ticks"
        );
        assert_eq!(
            presses(&found, &Answer::Submit).len(),
            4,
            "three rows down to Submit, then Enter"
        );
    }

    const TWO_QUESTIONS: &str = "\
────────────────────────────────────────
 ← ☒ Colors  ☐ Size  ✔ Submit  →

Which colors should the picker offer first?

  1. Warm
❯ 2. Cool
  3. Both
  4. Type something
──────────────────
  5. Chat about this";

    /// The line of names as Claude Code draws it, the step at `lit` on a
    /// background of its own.
    fn names_row(lit: usize) -> Vec<Span<'static>> {
        let bg = Style::new().bg(ratatui::style::Color::Rgb(177, 185, 249));
        let mut row = vec![Span::raw(" ← ")];
        for (i, name) in [" ☒ Colors ", " ☐ Size ", " ✔ Submit "]
            .into_iter()
            .enumerate()
        {
            row.push(if i == lit {
                Span::styled(name, bg)
            } else {
                Span::raw(name)
            });
        }
        row.push(Span::raw(" →"));
        row
    }

    #[test]
    fn the_steps_of_several_questions_are_read_with_their_answered_marks() {
        let found = options_in(TWO_QUESTIONS).expect("a prompt");
        let steps: Vec<(&str, bool, bool)> = found
            .steps
            .iter()
            .map(|s| (s.name.as_str(), s.answered, s.submit))
            .collect();
        assert_eq!(
            steps,
            [
                ("Colors", true, false),
                ("Size", false, false),
                ("Submit", false, true)
            ]
        );
        assert_eq!(found.shown, None, "text alone can't tell which is shown");
        assert_eq!(found.cursor, Some(Cursor::Option(2)), "the answer given");
        assert_eq!(
            found.question,
            "Which colors should the picker offer first?"
        );
        assert!(found.review.is_empty());
        let one = "\
 ☐ Layout

Which layout?

❯ 1. Sidebar
  2. Top bar
  3. Type something
──────────────────
  4. Chat about this";
        assert!(
            options_in(one).expect("a prompt").steps.is_empty(),
            "a prompt of one question has no steps"
        );
    }

    #[test]
    fn the_step_shown_is_read_from_the_line_of_names_cells() {
        let mut found = options_in(TWO_QUESTIONS).expect("a prompt");
        let rows = vec![vec![Span::raw("─".repeat(40))], names_row(1)];
        read_shown(&mut found, &rows);
        assert_eq!(found.shown, Some(1));
        read_shown(&mut found, &[names_row(0)]);
        assert_eq!(found.shown, Some(0));
        let mut plain = options_in(TWO_QUESTIONS).expect("a prompt");
        read_shown(
            &mut plain,
            &[vec![Span::raw(" ← ☒ Colors  ☐ Size  ✔ Submit  →")]],
        );
        assert_eq!(plain.shown, None, "nothing drawn lit");
    }

    #[test]
    fn submits_step_reviews_each_answer_and_its_question() {
        let screen = "\
 ← ☒ Colors  ☒ Size  ✔ Submit  →

Review your answers

 ● Which colors should the picker offer first?
   → Cool
 ● Which size should it open at?
   → Roomy, with the sidebar
     open

❯ 1. Submit answers
  2. Cancel";
        let found = options_in(screen).expect("a prompt");
        assert_eq!(found.steps.len(), 3);
        assert_eq!(
            found.review,
            vec![
                Reviewed {
                    question: "Which colors should the picker offer first?".into(),
                    answer: "Cool".into(),
                    step: Some(0),
                },
                Reviewed {
                    question: "Which size should it open at?".into(),
                    answer: "Roomy, with the sidebar open".into(),
                    step: Some(1),
                },
            ]
        );
        let labels: Vec<&str> = found.options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, ["Submit answers", "Cancel"]);
    }

    #[test]
    fn stepping_presses_left_and_right_as_many_times_as_it_takes() {
        let left = Press::Key(Key::new(KeyCode::Left, Mods::NONE));
        let right = Press::Key(Key::new(KeyCode::Right, Mods::NONE));
        let mut found = options_in(TWO_QUESTIONS).expect("a prompt");
        read_shown(&mut found, &[names_row(0)]);
        assert!(
            step_presses(&found, &StepTo::Back).is_err(),
            "nothing before the first"
        );
        assert_eq!(
            step_presses(&found, &StepTo::Forward),
            Ok((vec![right.clone()], "Stepped to Size".into()))
        );
        assert_eq!(
            step_presses(&found, &StepTo::parse("Submit")).map(|(p, _)| p),
            Ok(vec![right.clone(), right.clone()])
        );
        read_shown(&mut found, &[names_row(2)]);
        assert!(
            step_presses(&found, &StepTo::Forward).is_err(),
            "nothing past Submit"
        );
        assert_eq!(
            step_presses(&found, &StepTo::parse("Colors")).map(|(p, _)| p),
            Ok(vec![left.clone(), left.clone()])
        );
        assert_eq!(
            step_presses(&found, &StepTo::parse("back")).map(|(p, _)| p),
            Ok(vec![left])
        );
        assert!(step_presses(&found, &StepTo::parse("Shape")).is_err());
        let mut typed = found.clone();
        typed.cursor = Some(Cursor::Option(4));
        assert_eq!(
            step_presses(&typed, &StepTo::Back).map(|(p, _)| p),
            Ok(vec![
                Press::Key(Key::new(KeyCode::Up, Mods::NONE)),
                Press::Key(Key::new(KeyCode::Left, Mods::NONE))
            ]),
            "out of the free option's field first"
        );
        assert!(
            step_presses(&found, &StepTo::parse("Submit")).is_err(),
            "already there"
        );
    }

    #[test]
    fn a_choice_beside_a_preview_is_its_number_then_enter() {
        let screen = "\
 ☐ Layout

Which layout?

❯ 1. Sidebar          ┌──────────────────────┐
  2. Top bar          │ ┌──────┐             │
  3. Tabs             │ │ nav  │  content    │
                      │ ──────────────────── │
                      │ 1. Install           │
                      │ 2. Run               │
                      │ └──────┘             │
                      └──────────────────────┘
  4. Type something
──────────────────
  5. Chat about this";
        let found = options_in(screen).expect("a prompt");
        assert!(found.preview);
        assert_eq!(found.free, Some(4), "a divider in the box is no rule");
        let labels: Vec<&str> = found.options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Sidebar",
                "Top bar",
                "Tabs",
                "Type something",
                "Chat about this"
            ],
            "nothing in the box passes for an option"
        );
        assert!(found.more.is_empty(), "nor for a description: {found:?}");
        let chose = answer_for(&found, "2").unwrap();
        assert_eq!(answer_said(&chose), "Chose 2. Top bar");
        assert_eq!(
            presses(&found, &chose),
            vec![
                Press::Text("2".into()),
                Press::Settle,
                Press::Key(Key::new(KeyCode::Enter, Mods::NONE)),
            ],
            "the number only moves the ❯ there, and Enter chooses"
        );
        let plain = "\
 ☐ Layout

Which layout?

❯ 1. Sidebar
  2. Top bar
  3. Type something
──────────────────
  4. Chat about this";
        let found = options_in(plain).expect("a prompt");
        assert!(!found.preview);
        let chose = answer_for(&found, "2").unwrap();
        assert_eq!(presses(&found, &chose), vec![Press::Text("2".into())]);
        let boxed = "\
│ Do you want to proceed?        │
│ ❯ 1. Yes                       │
│   2. No                        │";
        assert!(
            !options_in(boxed).expect("a prompt").preview,
            "a prompt in a box of its own has no preview"
        );
    }

    #[test]
    fn a_reply_is_an_options_number_or_the_start_of_its_label() {
        let found = options_in(PERMISSION).unwrap();
        let chose = answer_for(&found, "2").unwrap();
        assert_eq!(
            answer_said(&chose),
            "Chose 2. Yes, and don't ask again for git push"
        );
        assert_eq!(presses(&found, &chose), vec![Press::Text("2".into())]);
        assert!(matches!(answer_for(&found, "no"), Some(Answer::Choose(o)) if o.n == 3));
        assert_eq!(answer_for(&found, "9"), None);
        assert_eq!(answer_for(&found, "maybe"), None, "nothing to type it into");
    }

    #[test]
    fn the_box_answers_with_a_number_and_types_a_message_when_no_prompt_shows() {
        let look = Look::default();
        let mut b = AnswerBox::new("Answer 2 fix".into(), options_in(PERMISSION), None);
        let key = |c| Key::new(KeyCode::Char(c), Mods::NONE);
        assert!(matches!(
            b.key(key('2'), &look),
            Step::Close(Outcome::Picked(p)) if p.value == "2"
        ));
        let mut b = AnswerBox::new("Answer 2 fix".into(), options_in(PERMISSION), None);
        b.key(Key::new(KeyCode::Tab, Mods::NONE), &look);
        b.key(Key::new(KeyCode::Tab, Mods::NONE), &look);
        assert!(matches!(
            b.key(Key::new(KeyCode::Enter, Mods::NONE), &look),
            Step::Close(Outcome::Picked(p)) if p.value == "2"
        ));
        let mut b = AnswerBox::new("Answer 2 fix".into(), None, None);
        for c in "hi there".chars() {
            b.key(key(c), &look);
        }
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 20));
        b.draw(&mut buf, &look);
        let s = crate::ui::draw::tests::screen(&buf);
        assert!(s.iter().any(|l| l.contains("No question showing")));
        assert!(s.iter().any(|l| l.contains("› hi there")));
        assert!(matches!(
            b.key(Key::new(KeyCode::Enter, Mods::NONE), &look),
            Step::Close(Outcome::Picked(p)) if p.value == "hi there"
        ));
    }

    #[test]
    fn slash_searches_the_options_each_keeping_its_number() {
        let look = Look::default();
        let mut b = AnswerBox::new("Answer 2 fix".into(), options_in(PERMISSION), None);
        for c in "/tell".chars() {
            assert_eq!(
                b.key(Key::new(KeyCode::Char(c), Mods::NONE), &look),
                Step::Redraw
            );
        }
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 24));
        b.draw(&mut buf, &look);
        let s = crate::ui::draw::tests::screen(&buf);
        assert!(s.iter().any(|l| l.contains("› tell▏")), "{s:#?}");
        assert!(s.iter().any(|l| l.contains("3  No, and tell Claude")));
        assert!(!s.iter().any(|l| l.contains("don't ask again")));
        assert!(matches!(
            b.key(Key::new(KeyCode::Enter, Mods::NONE), &look),
            Step::Close(Outcome::Picked(p)) if p.value == "3"
        ));
        let mut b = AnswerBox::new("Answer 2 fix".into(), options_in(PERMISSION), None);
        b.key(Key::new(KeyCode::Char('/'), Mods::NONE), &look);
        b.key(Key::new(KeyCode::Down, Mods::NONE), &look);
        b.key(Key::new(KeyCode::Down, Mods::NONE), &look);
        let mut short = Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 11));
        b.draw(&mut short, &look);
        let s = crate::ui::draw::tests::screen(&short);
        assert!(
            s.iter().any(|l| l.contains("3  No, and tell Claude")),
            "the chosen match in view on a short screen: {s:#?}"
        );
        let long = Prompt {
            question: (0..20)
                .map(|i| format!("line {i}"))
                .collect::<Vec<_>>()
                .join("\n"),
            ..options_in(PERMISSION).unwrap()
        };
        let mut b = AnswerBox::new("Answer 2 fix".into(), Some(long), None);
        b.key(Key::new(KeyCode::Char('/'), Mods::NONE), &look);
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 24));
        b.draw(&mut buf, &look);
        let s = crate::ui::draw::tests::screen(&buf);
        assert!(
            s.iter().any(|l| l.contains("1  Yes")),
            "a long question leaves a match: {s:#?}"
        );
        let mut b = AnswerBox::new("Answer 2 fix".into(), options_in(PERMISSION), None);
        for c in "/yes".chars() {
            b.key(Key::new(KeyCode::Char(c), Mods::NONE), &look);
        }
        b.key(Key::new(KeyCode::Char('n'), Mods::CTRL), &look);
        assert!(
            matches!(
                b.key(Key::new(KeyCode::Enter, Mods::NONE), &look),
                Step::Close(Outcome::Picked(p)) if p.value == "2"
            ),
            "Ctrl-N moves to the second match"
        );
        let mut b = AnswerBox::new("Answer 2 fix".into(), options_in(PERMISSION), None);
        b.key(Key::new(KeyCode::Char('/'), Mods::NONE), &look);
        assert_eq!(
            b.key(Key::new(KeyCode::Esc, Mods::NONE), &look),
            Step::Redraw
        );
        assert_eq!(
            b.key(Key::new(KeyCode::Esc, Mods::NONE), &look),
            Step::Close(Outcome::Dismissed)
        );
    }

    #[test]
    fn typing_into_a_prompt_in_vim_s_normal_mode_presses_i_first() {
        let typed = vec![Press::Text("/ci-loop".into())];
        let normal = "╭───╮\n│ > │\n╰───╯\n  -- NORMAL --\n\n";
        assert!(normal_mode(normal));
        assert_eq!(
            into_prompt(normal, typed.clone()),
            vec![Press::Text("i".into()), Press::Text("/ci-loop".into())]
        );
        let insert = "╭───╮\n│ > │\n╰───╯\n  -- INSERT --\n";
        assert_eq!(into_prompt(insert, typed.clone()), typed);
        assert_eq!(into_prompt("> \n  ? for shortcuts", typed.clone()), typed);
        // Only at the screen's foot: Claude's reply quoting it is no mode line.
        let quoted = "Vim shows -- NORMAL -- there.\n\n> \n─────\n  ? for shortcuts\n  model";
        assert!(!normal_mode(quoted));
    }
}
