//! Answering a tab from mission control (lib/sidebar/answer.js), as the
//! Slack relay answers one from a thread: the question or permission prompt
//! its Claude shows, read off its screen, and your answer pressed there. The
//! reading and pressing are the relay's own (lib/slack-relay/tabs.js's
//! `optionsIn`, `answerFor`, `answerSaid` and `answerPrompt`), kept here as
//! plain functions of the screen's text so the relay can share them when it
//! comes over. A tab showing no prompt takes what you type as a message,
//! pasted and sent with Enter.
//!
//! `AnswerBox` is the box over mission control (render.js's `answerModal`):
//! the question, its options numbered, and a line to type in. A number
//! answers with that option, `s` submits a question of several answers, `t`
//! types your own answer where the question takes one; Tab and Shift-Tab
//! move a highlight round the options (and easy mode's ×), Enter taking it.
//! It closes with what to press, which the server presses in the pane.

use std::sync::LazyLock;

use ratatui::buffer::Buffer;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use regex::Regex;

use crate::mux::input::{Key, KeyCode, Mods, Mouse};
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
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
static LABEL_TAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s{2,}.*$").expect("a regex"));

/// One numbered option of a prompt; `checked` for one that takes several
/// answers, ticked or not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptOption {
    pub n: u32,
    pub label: String,
    pub checked: Option<bool>,
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
    pub free: Option<u32>,
    pub cursor: Option<Cursor>,
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
/// time, under a line of their names marked ☐ or ☒. A question that takes
/// several answers (`multi`) shows each option as [ ] or [✔], and a Submit
/// row after the options moves on.
pub fn options_in(text: &str) -> Option<Prompt> {
    let mapped: Vec<String> = text
        .split('\n')
        .map(|l| l.replace(['│', '┃'], " ").trim_end().to_string())
        .collect();
    let lines: Vec<&str> = mapped
        .iter()
        .enumerate()
        .filter(|(i, l)| !l.trim().is_empty() && (*i == 0 || **l != mapped[i - 1]))
        .map(|(_, l)| l.as_str())
        .collect();
    struct Found {
        n: u32,
        label: String,
        checked: Option<bool>,
        line: usize,
        marked: bool,
    }
    let mut options: Vec<Found> = Vec::new();
    let mut first: Option<usize> = None;
    for i in (0..lines.len()).rev() {
        let Some(m) = OPTION.captures(lines[i]) else {
            continue;
        };
        let n: u32 = m[1].parse().unwrap_or(0);
        if !options.iter().any(|o| o.n == n) {
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
    let between = |re: &Regex| (first + 1..last).find(|&i| re.is_match(lines[i]));
    let rule = between(&RULE);
    let submit = between(&SUBMIT_ROW);
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
            .map(|o| PromptOption {
                n: o.n,
                label: o.label,
                checked: o.checked,
            })
            .collect(),
        multi,
        free,
        cursor: free.and(cursor),
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
fn chosen_option(options: &[PromptOption], text: &str) -> Option<PromptOption> {
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
/// number opens it for typing, and Enter sends what was typed.
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

// ── The box ─────────────────────────────────────────────────────────────────

/// What the box gives back as its outcome's value: an option's number,
/// `submit`, or the text typed.
pub struct AnswerBox {
    title: String,
    found: Option<Prompt>,
    text: String,
    typing: bool,
    note: Option<String>,
    /// Where Tab has put the highlight: an option, or past the last the
    /// close button.
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
            text: String::new(),
            typing: false,
            note,
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

    fn typing_line(&self) -> bool {
        self.typing || self.found.is_none()
    }
}

impl Overlay for AnswerBox {
    fn kind(&self) -> &'static str {
        "answer"
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let inner = (buf.area.width as usize).saturating_sub(4).min(90);
        let mut lines: Vec<BoxLine> = Vec::new();
        self.option_rows.clear();
        let typing = self.typing_line();
        if let Some(found) = &self.found {
            for line in found.question.split('\n') {
                for w in draw::wrap(line, inner.saturating_sub(2)) {
                    lines.push(BoxLine::text(format!(" {w}"), look.bright()));
                }
            }
            lines.push(BoxLine::blank());
            for (i, o) in found.options.iter().enumerate() {
                let chosen = self.focus == Some(i) && !typing;
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
                lines.push(
                    BoxLine::spans(vec![
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
                    .clickable(),
                );
            }
        } else {
            lines.push(BoxLine::text(
                " No question showing: what you type is sent to it as a message.",
                look.dim(),
            ));
        }
        if typing {
            lines.push(BoxLine::blank());
            lines.push(BoxLine::spans(vec![
                Span::styled(format!(" › {}", self.text), look.bright()),
                Span::styled("▏", look.dim()),
            ]));
        }
        if let Some(note) = &self.note {
            lines.push(BoxLine::blank());
            lines.push(BoxLine::text(format!(" {note}"), look.dim()));
        }
        let keys = match &self.found {
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
            _ => "type  ↵ send  esc back".to_string(),
        };
        let mut spec = BoxSpec::new(&self.title, &lines);
        spec.keys = Some(&keys);
        spec.width = inner as u16;
        let options = self.found.as_ref().map_or(0, |f| f.options.len());
        spec.close_lit = self.close_focus || (!typing && self.focus == Some(options));
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn key(&mut self, key: Key, look: &Look) -> Step {
        let k = key.normalized();
        let plain = k.mods.is_empty();
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
            match k.code {
                KeyCode::Enter => return AnswerBox::give(self.text.clone()),
                KeyCode::Backspace => {
                    self.text.pop();
                }
                KeyCode::Char('u') if k.mods == Mods::CTRL => self.text.clear(),
                KeyCode::Char(c) if !k.mods.ctrl && !k.mods.alt => self.text.push(c),
                _ => return Step::Same,
            }
            return Step::Redraw;
        }
        let Some(found) = &self.found else {
            return Step::Same;
        };
        let options = found.options.len();
        let stops = options + usize::from(look.close);
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
        if !self.typing_line() {
            return Step::Same;
        }
        self.text.push_str(text);
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
}
