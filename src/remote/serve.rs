//! The server's side of the remote front-end protocol (src/remote/protocol.rs):
//! the commands the phone app and the Slack relay ask over the socket,
//! `prompt` (a Claude pane's waiting prompt as JSON), `answer` (an option
//! pressed, a dialog's option chosen, or a message typed, in a Claude
//! pane), `clear-draft` (a draft left in its prompt box cleared),
//! `kill-tab -s` (a Claude tab closed, which the phone asks about first),
//! `remind` (a Claude tab's reminder set or cleared, as the workspace's
//! reminder box does), `pin` (a Claude tab pinned or unpinned, as M-f
//! does) and `stop` (a working Claude's turn stopped with Esc), and the
//! server's log's line for what was done from elsewhere, with no
//! notification, since you know what you did and the tab shows it. Moved
//! from the phone's src/phone/serve.rs, where `watch`, which streams the
//! phone's tabs, stays.
//!
//! A prompt is read and answered with mission control's own reader and
//! keys (`mission::answer`), so a front-end presses exactly what the
//! answer box would. Nothing here reaches a pane that isn't running
//! Claude: each command refuses one.

use serde_json::{Value, json};

use super::protocol::How;
use super::screen::{self, Dialog};
use crate::mission::answer::{self, Prompt};
use crate::mission::remind;
use crate::mux::input::{Key, KeyCode, Mods};
use crate::mux::server::{Reply, Server};
use crate::mux::{PaneId, TabId};
use crate::{settings, util};

/// The longest a message's words run in the log's line.
const SAID_WIDTH: usize = 80;

/// A prompt as the phone reads it.
pub fn prompt_json(found: &Prompt) -> Value {
    json!({
        "id": screen::hash_of(found),
        "question": found.question,
        "options": found
            .options
            .iter()
            .map(|o| json!({"n": o.n, "label": o.label, "checked": o.checked, "detail": o.detail}))
            .collect::<Vec<_>>(),
        "multi": found.multi,
        "free": found.free,
        "cursor": match found.cursor {
            Some(answer::Cursor::Option(n)) => json!(n),
            Some(answer::Cursor::Submit) => json!("submit"),
            None => Value::Null,
        },
        "steps": found
            .steps
            .iter()
            .map(|s| json!({"name": s.name, "answered": s.answered, "submit": s.submit}))
            .collect::<Vec<_>>(),
        "shown": found.shown,
        "review": found
            .review
            .iter()
            .map(|r| json!({"question": r.question, "answer": r.answer, "step": r.step}))
            .collect::<Vec<_>>(),
    })
}

/// What a message said, cut to fit the log's line.
fn short(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= SAID_WIDTH {
        return line;
    }
    let cut: String = line.chars().take(SAID_WIDTH - 1).collect();
    format!("{cut}…")
}

impl Server {
    /// Whether a pane runs Claude, as the sidebar last read it.
    fn remote_runs_claude(&self, pane: PaneId) -> bool {
        self.sidebar.panes.get(&pane).is_some_and(|r| r.claude)
    }

    pub(crate) fn remote_claude_pane(&self, pane: PaneId) -> Result<(), String> {
        if self.remote_runs_claude(pane) {
            Ok(())
        } else {
            Err(format!("pane {pane} isn't running Claude"))
        }
    }

    /// The prompt a Claude pane shows, read off its screen.
    pub(crate) fn remote_prompt_of(&self, pane: PaneId) -> Option<Prompt> {
        let slot = self.panes.get(&pane)?;
        let mut found = answer::options_in(&slot.pane.text(60))?;
        if !found.steps.is_empty() {
            answer::read_shown(&mut found, &slot.pane.styled_lines());
        }
        Some(found)
    }

    /// Whether a pane's Claude waits on you: marked waiting, or, in auto
    /// mode, asked a permission its Notification hook hasn't yet marked
    /// (`asked`), while the prompt is already on screen.
    pub(crate) fn remote_asking(&self, pane: PaneId) -> bool {
        self.agents.state(pane) == Some(crate::agent::State::Waiting)
            || self.agents.get(pane).is_some_and(|a| a.asked.is_some())
    }

    /// The dialog a Claude pane shows before its session starts.
    fn remote_dialog(&self, pane: PaneId) -> Option<Dialog> {
        screen::dialog_in(&self.panes.get(&pane)?.pane.text(0))
    }

    /// Whether Claude's fullscreen view is scrolled back, so what it waits
    /// on is out of sight and a message's keys would reach it unseen.
    fn remote_scrolled_back(&self, pane: PaneId) -> bool {
        self.panes
            .get(&pane)
            .is_some_and(|slot| answer::scrolled_back(&slot.pane.text(0)))
    }

    /// What is typed and unsent in a Claude pane's prompt box.
    fn remote_draft(&self, pane: PaneId) -> Option<String> {
        let slot = self.panes.get(&pane)?;
        screen::draft_in(&slot.pane.text(0), &slot.pane.typed_text(0))
    }

    /// `prompt`: the prompt the pane's Claude waits on as JSON, `null` with
    /// none.
    pub(crate) fn remote_prompt(&self, pane: PaneId) -> Result<Reply, String> {
        self.remote_claude_pane(pane)?;
        let found = self.remote_prompt_of(pane);
        Ok(Reply::ok(format!(
            "{}\n",
            found.as_ref().map(prompt_json).unwrap_or(Value::Null)
        )))
    }

    /// `answer`: `reply` pressed in the prompt the pane's Claude shows (an
    /// option's number or the start of its label, `submit`, or your own
    /// words where it takes them), or, with `message`, typed as a message to
    /// a Claude that shows none. `how.expect`, the prompt's id as `prompt`
    /// gave it (the hash of its question and options), refuses an answer
    /// meant for a prompt no longer showing, so a tap on a card a moment old
    /// never answers the next question. `how.step`, in place of a reply, steps
    /// over a prompt of several questions (`back`, `forward` or a step's
    /// name), pressing ← or → as Claude Code takes them. With `how.dialog` a dialog before
    /// the session starts is answered too, and with `how.after_draft` a
    /// message goes after a draft in the prompt box, as one prompt: the
    /// relay asks you about both first. `how.from` names who asked, for the
    /// log.
    pub(crate) fn remote_answer(
        &mut self,
        pane: PaneId,
        reply: &str,
        message: bool,
        how: How,
    ) -> Result<Reply, String> {
        if how.step.is_some() && (message || !reply.trim().is_empty()) {
            return Err("a step takes no reply or message".into());
        }
        self.remote_claude_pane(pane)?;
        let found = self.remote_prompt_of(pane);
        // A numbered list in a finished reply reads as options too, so a
        // message waits on a prompt only while its Claude is waiting, as
        // the drawer shows one and the relay reads one.
        let waiting = self.remote_asking(pane);
        let said = match (message, found) {
            (true, Some(_)) if waiting => {
                return Err("its Claude is showing a prompt: answer that first".into());
            }
            (true, None) if waiting && self.remote_scrolled_back(pane) => {
                return Err(
                    "its Claude's view is scrolled up past what it waits on: scroll it down at the computer"
                        .into(),
                );
            }
            (true, _) if reply.trim().is_empty() => return Err("nothing to send".into()),
            // A dialog before its session starts (trust this folder, a new
            // MCP server), which Enter would confirm unseen, and a draft in
            // its prompt box, which would go with the words, are left for
            // the computer, as mission control's answer box leaves them.
            (true, _) if self.remote_dialog(pane).is_some() => {
                return Err(
                    "its Claude is showing a dialog before it starts: answer it at the computer"
                        .into(),
                );
            }
            (true, _) if !how.after_draft && self.remote_draft(pane).is_some() => {
                return Err(
                    "its Claude has a draft in its prompt box, which would go with this: send it or clear it at the computer"
                        .into(),
                );
            }
            (true, _) => {
                self.type_into_claude(pane, answer::message_presses(reply));
                format!("Sent \"{}\"", short(reply))
            }
            (false, None) => {
                let dialog = self
                    .remote_dialog(pane)
                    .filter(|_| how.dialog)
                    .ok_or("its Claude isn't showing a prompt")?;
                if how.expect.is_some_and(|id| id != dialog.hash()) {
                    return Err("its dialog changed since: look at it again".into());
                }
                let picked = answer::chosen_option(&dialog.options, reply)
                    .ok_or_else(|| format!("\"{reply}\" isn't one of its options"))?;
                self.press(pane, dialog.presses(&picked));
                format!("Chose {}. {}", picked.n, picked.label)
            }
            (false, Some(found)) if how.expect.is_some_and(|id| id != screen::hash_of(&found)) => {
                return Err("its prompt changed since: look at it again".into());
            }
            (false, Some(found)) if let Some(step) = how.step => {
                let (presses, said) = answer::step_presses(&found, &answer::StepTo::parse(step))?;
                self.press(pane, presses);
                said
            }
            (false, Some(found)) => {
                let a = answer::answer_for(&found, reply)
                    .ok_or_else(|| format!("\"{reply}\" isn't one of its options"))?;
                self.press(pane, answer::presses(&found, &a));
                answer::answer_said(&a)
            }
        };
        if let Some(from) = how.from
            && let Some(tab) = self.panes.get(&pane).map(|slot| slot.tab)
        {
            self.remote_did(from, tab, &short(&said));
        }
        Ok(Reply::ok(format!("{said}\n")))
    }

    /// `kill-tab -s from`: the tab of a pane running Claude closed for
    /// `from` (the phone, which asks first), written in the server's log
    /// as answering and messaging are; refused for a pane not running
    /// Claude, as they are.
    pub(crate) fn remote_close(&mut self, pane: PaneId, from: &str) -> Result<Reply, String> {
        if !crate::settings::is_on("phone_close_tabs") {
            return Err("closing tabs from the phone is off in Settings".into());
        }
        self.remote_claude_pane(pane)?;
        let tab = self.panes[&pane].tab;
        let n = self.tab_number(tab);
        let name = self
            .tabs
            .get(&tab)
            .map(|t| t.display_name().to_string())
            .unwrap_or_default();
        self.remote_did(from, tab, "closed it");
        self.kill_tab(tab);
        Ok(Reply::ok(format!("Closed tab {n} {name}\n")))
    }

    /// Whether Esc in a Claude pane would stop its turn, else why not: the
    /// one test `stop` refuses by and `watch` tells the phone (`stoppable`),
    /// so the phone offers ■ exactly when the server takes it.
    pub(crate) fn remote_stoppable(&self, pane: PaneId) -> Result<(), String> {
        if self.agents.state(pane) != Some(crate::agent::State::Working) {
            return Err("its Claude isn't working".into());
        }
        // A permission auto mode asks while still working, or a dialog,
        // which Esc would refuse or close rather than stop the turn. A
        // prompt counts as `watch` sends the phone one, while the pane is
        // marked asked, so a numbered list in Claude's own words, with
        // nothing asked, leaves it allowed.
        let prompt_shown = self.remote_asking(pane) && self.remote_prompt_of(pane).is_some();
        if prompt_shown || self.remote_dialog(pane).is_some() {
            return Err("its Claude is asking something: answer it first".into());
        }
        // Scrolled back, Esc would scroll the view down, or reach a prompt
        // hidden below it, rather than stop the turn.
        if self.remote_scrolled_back(pane) {
            return Err(
                "its Claude's view is scrolled back: scroll it down at the computer first".into(),
            );
        }
        Ok(())
    }

    /// `stop`: Esc pressed in a pane's Claude, which stops its turn as Esc
    /// at the computer does; refused for a pane not running Claude, for one
    /// whose Claude isn't working and for one asking a permission or showing
    /// a dialog, which Esc would answer instead. With `force`, the relay's
    /// Escape, pressed whenever its Claude is working or waiting, where it
    /// answers no to what it asks. `from` names who asked, for the log.
    pub(crate) fn remote_stop(
        &mut self,
        pane: PaneId,
        from: Option<&str>,
        force: bool,
    ) -> Result<Reply, String> {
        self.remote_claude_pane(pane)?;
        if !force {
            self.remote_stoppable(pane)?;
        } else if !matches!(
            self.agents.state(pane),
            Some(crate::agent::State::Working | crate::agent::State::Waiting)
        ) {
            return Err("its Claude isn't in the middle of anything".into());
        }
        let tab = self.panes[&pane].tab;
        self.press(
            pane,
            vec![answer::Press::Key(Key::new(KeyCode::Esc, Mods::NONE))],
        );
        if let Some(from) = from {
            self.remote_did(from, tab, "Stopped");
        }
        let n = self.tab_number(tab);
        let name = self
            .tabs
            .get(&tab)
            .map(|t| t.display_name().to_string())
            .unwrap_or_default();
        Ok(Reply::ok(format!("Stopped tab {n} {name}\n")))
    }

    /// `clear-draft`: what is typed and unsent in a Claude pane's prompt box
    /// cleared, which the relay does before typing a message in its place.
    pub(crate) fn remote_clear_draft(&mut self, pane: PaneId) -> Result<Reply, String> {
        self.remote_claude_pane(pane)?;
        if let Some(draft) = self.remote_draft(pane) {
            let shown = self.panes[&pane].pane.text(0);
            self.press(pane, screen::clearing(&shown, &draft));
        }
        Ok(Reply::ok(""))
    }

    /// `remind`: the reminder of a pane's tab set for `when` (one of the
    /// box's times, or one typed, read as the box reads it) with `note`,
    /// or, with `None`, cleared, as the workspace's box sets and clears
    /// one; refused with Settings' "Reminders" off, for a pane not running
    /// Claude, and for a time that can't be read, in the box's words.
    /// `from` names who asked, for the log.
    pub(crate) fn remote_remind(
        &mut self,
        pane: PaneId,
        when: Option<&str>,
        note: &str,
        from: Option<&str>,
    ) -> Result<Reply, String> {
        if !settings::is_on("reminders") {
            return Err("reminders are off in Settings".into());
        }
        self.remote_claude_pane(pane)?;
        let tab = self.panes[&pane].tab;
        let name = format!(
            "tab {} {}",
            self.tab_number(tab),
            self.tabs
                .get(&tab)
                .map(|t| t.display_name().to_string())
                .unwrap_or_default()
        );
        let (said, what) = match when {
            None => {
                self.write_reminder(tab, None);
                (
                    format!("Cleared the reminder for {name}"),
                    "Cleared its reminder".to_string(),
                )
            }
            Some(text) => {
                let text = text.trim();
                let at = remind::when(text, &util::local_now())
                    .ok_or_else(|| format!("\"{text}\" {}", remind::NOT_A_TIME))?;
                self.write_reminder(tab, Some((at, note.trim())));
                let at = remind::said(at);
                (
                    format!("Reminder for {name} at {at}"),
                    format!("Reminder at {at}"),
                )
            }
        };
        if let Some(from) = from {
            self.remote_did(from, tab, &what);
        }
        Ok(Reply::ok(format!("{said}\n")))
    }

    /// `pin`: the tab of a pane running Claude pinned, or with `pin` false
    /// unpinned, as M-f does; refused for a pane not running Claude.
    /// `from` names who asked, for the log.
    pub(crate) fn remote_pin(
        &mut self,
        pane: PaneId,
        pin: bool,
        from: Option<&str>,
    ) -> Result<Reply, String> {
        self.remote_claude_pane(pane)?;
        let tab = self.panes[&pane].tab;
        self.set_pin(tab, pin);
        let (did, what) = if pin {
            ("Pinned", "Pinned it")
        } else {
            ("Unpinned", "Unpinned it")
        };
        if let Some(from) = from {
            self.remote_did(from, tab, what);
        }
        let n = self.tab_number(tab);
        let name = self
            .tabs
            .get(&tab)
            .map(|t| t.display_name().to_string())
            .unwrap_or_default();
        Ok(Reply::ok(format!("{did} tab {n} {name}\n")))
    }

    /// Something done from elsewhere (`from`, the phone), written in the
    /// server's log alone: no notification, since the tab shows what was
    /// done there.
    pub(crate) fn remote_did(&self, from: &str, tab: TabId, what: &str) {
        let n = self.tab_number(tab);
        let name = self
            .tabs
            .get(&tab)
            .map(|t| t.display_name().to_string())
            .unwrap_or_default();
        eprintln!("switchboard server: from your {from}: tab {n} ({name}): {what}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_message_is_cut_to_a_line() {
        assert_eq!(short("one\ntwo  three"), "one two three");
        let long = "x".repeat(200);
        let cut = short(&long);
        assert_eq!(cut.chars().count(), SAID_WIDTH);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn a_prompt_reads_as_its_options() {
        let found = answer::options_in("Do you want to proceed?\n❯ 1. Yes\n  2. No\n").unwrap();
        let v = prompt_json(&found);
        assert_eq!(v["question"], "Do you want to proceed?");
        assert_eq!(v["options"][1]["label"], "No");
        assert_eq!(v["multi"], false);
        assert_eq!(v["options"][1]["detail"], "", "no explainer under it");
        let explained = answer::options_in(
            "Which way?\n❯ 1. Fast\n     Skips the checks\n  2. Slow\n     Runs every check\n",
        )
        .unwrap();
        let v = prompt_json(&explained);
        assert_eq!(v["options"][0]["detail"], "Skips the checks");
        assert_eq!(v["options"][1]["detail"], "Runs every check");
    }

    #[test]
    fn a_prompt_of_several_questions_carries_its_steps_and_the_step_shown() {
        let screen = "\
 ← ☒ Colors  ☐ Size  ✔ Submit  →

Which size should it open at?

❯ 1. Compact
  2. Roomy
  3. Type something
──────────────────
  4. Chat about this";
        let mut found = answer::options_in(screen).unwrap();
        let plain = prompt_json(&found);
        assert_eq!(
            plain["steps"][0],
            json!({"name": "Colors", "answered": true, "submit": false})
        );
        assert_eq!(plain["steps"][2]["submit"], true);
        assert_eq!(plain["shown"], Value::Null);
        assert_eq!(plain["cursor"], 1);
        assert_eq!(plain["review"], json!([]));
        let lit = ratatui::style::Style::new().bg(ratatui::style::Color::Indexed(4));
        let row = vec![
            ratatui::text::Span::raw(" ←  ☒ Colors "),
            ratatui::text::Span::styled(" ☐ Size ", lit),
            ratatui::text::Span::raw(" ✔ Submit  →"),
        ];
        answer::read_shown(&mut found, &[row]);
        let v = prompt_json(&found);
        assert_eq!(v["shown"], 1);
        assert_ne!(v["id"], plain["id"], "the step shown is part of its id");
    }
}
