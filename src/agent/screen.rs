//! The one reading of Claude Code's screen, which everything that answers
//! or watches a Claude tab shares: mission control's answer box, the Slack
//! relay, the phone app and the server's own checks. It reads the question
//! or permission prompt a tab shows (`options_in`, the relay's
//! lib/slack-relay/tabs.js `optionsIn`) and what pressing an answer there
//! takes (`answer_for`, `answer_said`, `presses`, tabs.js's `answerFor`,
//! `answerSaid` and `answerPrompt`), a draft left unsent in the prompt box
//! (`draft_in`), a dialog Claude Code shows before its session starts
//! (`dialog_in`), and a prompt's id, the hash of its question and options a
//! button or card carries so a tap answers only the prompt it was made for.
//! Anything typed into Claude's prompt gets `i` first while the screen
//! shows the prompt in vim's Normal mode (`into_prompt`,
//! tmux/claude-pane.sh's `claude_type` and lib/sidebar/tmux.js's
//! `typeIntoClaude`). Plain functions of the screen's text, moved here from
//! mission control's answer box and the remote front-ends' own module.

use std::sync::LazyLock;

use ratatui::text::Span;
use regex::Regex;

use crate::mux::input::{Key, KeyCode, Mods};

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
    into_prompt(screen, clears.collect())
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
    use ratatui::style::Style;

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
