//! The server's side of the phone app: the three commands `switchboard
//! phone` asks of it over the socket, `prompt` (a Claude pane's waiting
//! prompt as JSON), `answer` (an option pressed, or a message typed, in a
//! Claude pane) and `watch` (every Claude tab's state and last turn,
//! streamed as a line of JSON each time they change), and the
//! notifications that say what your phone did.
//!
//! A prompt is read and answered with mission control's own reader and
//! keys (`mission::answer`), the ones the Slack relay uses, so the phone
//! presses exactly what the answer box would. Nothing here reaches a pane
//! that isn't running Claude: `prompt` and `answer` refuse one, and
//! `watch` lists Claude tabs alone.
//!
//! `watch` keeps its connection: the server takes the stream back from
//! the command's task (`Answer::Keep`) and writes `Output` frames to it,
//! one JSON line each, until the phone hangs up. What it sends is worked
//! out a moment after anything happens (`SETTLE`), at most that often, and
//! sent only when it differs from the last, so a burst of hooks costs one.

use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};

use crate::items::{Item, Target};
use crate::mission::answer::{self, Prompt};
use crate::mux::proto::{self, ServerMsg};
use crate::mux::server::{Answer, Event, Reply, Server};
use crate::mux::{PaneId, TabId};
use crate::sidebar::model::{Rules, by_need, needs_tab};
use crate::util;

/// The notifications' source: what your phone did.
pub const SOURCE: &str = "phone";
/// How many of them are kept.
const KEEP: usize = 20;
/// How long after a change `watch` waits before it sends, so a burst of
/// changes goes as one.
const SETTLE: Duration = Duration::from_millis(250);
/// The longest a message's words run in a notification.
const SAID_WIDTH: usize = 80;

/// The phone's watchers and what they were last sent.
#[derive(Default)]
pub struct Phone {
    watchers: Vec<mpsc::UnboundedSender<String>>,
    last: String,
    due: bool,
}

#[derive(Debug)]
pub enum PhoneEvent {
    /// Send the watchers what changed.
    Publish,
}

/// A prompt as the phone reads it.
pub fn prompt_json(found: &Prompt) -> Value {
    json!({
        "id": crate::relay::tabs::hash_of(found),
        "question": found.question,
        "options": found
            .options
            .iter()
            .map(|o| json!({"n": o.n, "label": o.label, "checked": o.checked}))
            .collect::<Vec<_>>(),
        "multi": found.multi,
        "free": found.free,
    })
}

/// What a message said, cut to fit a notification.
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
    fn phone_runs_claude(&self, pane: PaneId) -> bool {
        self.sidebar.panes.get(&pane).is_some_and(|r| r.claude)
    }

    fn phone_claude_pane(&self, pane: PaneId) -> Result<(), String> {
        if self.phone_runs_claude(pane) {
            Ok(())
        } else {
            Err(format!("pane {pane} isn't running Claude"))
        }
    }

    /// The prompt a Claude pane shows, read off its screen.
    fn phone_prompt_of(&self, pane: PaneId) -> Option<Prompt> {
        let slot = self.panes.get(&pane)?;
        answer::options_in(&slot.pane.text(60))
    }

    /// `prompt`: the prompt the pane's Claude waits on as JSON, `null` with
    /// none.
    pub(crate) fn phone_prompt(&self, pane: PaneId) -> Result<Reply, String> {
        self.phone_claude_pane(pane)?;
        let found = self.phone_prompt_of(pane);
        Ok(Reply::ok(format!(
            "{}\n",
            found.as_ref().map(prompt_json).unwrap_or(Value::Null)
        )))
    }

    /// `answer`: `reply` pressed in the prompt the pane's Claude shows (an
    /// option's number or the start of its label, `submit`, or your own
    /// words where it takes them), or, with `message`, typed as a message to
    /// a Claude that shows none. `expect`, the prompt's id as `prompt` gave
    /// it (the relay's hash of its question and options), refuses an answer
    /// meant for a prompt no longer showing, so a tap on a card a moment old
    /// never answers the next question. `from` names who asked, for the log
    /// and the notifications.
    pub(crate) fn phone_answer(
        &mut self,
        pane: PaneId,
        reply: &str,
        message: bool,
        expect: Option<&str>,
        from: Option<&str>,
    ) -> Result<Reply, String> {
        self.phone_claude_pane(pane)?;
        let found = self.phone_prompt_of(pane);
        let said = match (message, found) {
            (true, Some(_)) => {
                return Err("its Claude is showing a prompt: answer that first".into());
            }
            (true, None) if reply.trim().is_empty() => return Err("nothing to send".into()),
            (true, None) => {
                self.type_into_claude(pane, answer::message_presses(reply));
                format!("Sent \"{}\"", short(reply))
            }
            (false, None) => return Err("its Claude isn't showing a prompt".into()),
            (false, Some(found))
                if expect.is_some_and(|id| id != crate::relay::tabs::hash_of(&found)) =>
            {
                return Err("its prompt changed since: look at it again".into());
            }
            (false, Some(found)) => {
                let a = answer::answer_for(&found, reply)
                    .ok_or_else(|| format!("\"{reply}\" isn't one of its options"))?;
                self.press(pane, answer::presses(&found, &a));
                answer::answer_said(&a)
            }
        };
        if let Some(from) = from
            && let Some(tab) = self.panes.get(&pane).map(|slot| slot.tab)
        {
            self.phone_did(from, tab, &short(&said));
        }
        Ok(Reply::ok(format!("{said}\n")))
    }

    /// Something done from elsewhere (`from`, the phone), written in the
    /// server's log and kept as a notification, newest first.
    pub(crate) fn phone_did(&mut self, from: &str, tab: TabId, what: &str) {
        let n = self.tab_number(tab);
        let name = self
            .tabs
            .get(&tab)
            .map(|t| t.display_name().to_string())
            .unwrap_or_default();
        eprintln!("switchboard server: from your {from}: tab {n} ({name}): {what}");
        let now = util::now_ms();
        let mut items = self.notices.get(SOURCE).cloned().unwrap_or_default();
        items.insert(
            0,
            Item {
                id: format!("phone:{now}:{tab}"),
                title: format!("Phone: {what}"),
                detail: format!("Tab {n} · {name}"),
                time: Some(now),
                target: Target::Tab(tab),
                ..Default::default()
            },
        );
        items.truncate(KEEP);
        self.set_notices(SOURCE, items);
    }

    /// `phone-pair`: a box asking you to type the code `device` shows
    /// (board 168), over the screen of the client used last. A typed code
    /// rather than a yes, so a stray Enter pairs nothing: Enter with the
    /// right code pairs it, Esc refuses it.
    pub(crate) fn phone_pair_ask(&mut self, device: &str) -> Result<Reply, String> {
        let client = self
            .latest_client()
            .ok_or("no workspace is attached to ask in; run switchboard phone pair <code>")?;
        let ask = crate::mission::ask::Ask::new(
            "Pair a phone?",
            format!(
                "{device}, signed in to Tailscale as you, wants to answer and message your tabs from the phone app. Type the code it shows (Esc refuses):"
            ),
        );
        self.open(
            client,
            Box::new(ask),
            Some(Box::new(move |server: &mut Server, client, outcome| {
                let said = match outcome {
                    crate::ui::Outcome::Picked(p) => {
                        crate::phone::pairing::approve(&p.value, util::now_ms()).words()
                    }
                    _ => {
                        crate::phone::pairing::cancel();
                        "Refused the phone.".into()
                    }
                };
                server.say(Some(client), &said);
            })),
        );
        Ok(Reply::ok(""))
    }

    /// M-p's "Open the phone app in its own tab" and "Stop the phone app":
    /// `switchboard phone start` or `stop`, which say what went wrong in a
    /// box, run off the server's task as the Slack relay's are.
    pub fn phone_app(&mut self, start: bool) {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let spawned = tokio::process::Command::new(exe)
            .args(["phone", if start { "start" } else { "stop" }, "--say"])
            .env("SWITCHBOARD", crate::mux::socket::socket_path())
            .env_remove("SWITCHBOARD_PANE")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if let Ok(mut child) = spawned {
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
        }
    }

    /// A phone notification put away.
    pub(crate) fn phone_dismiss(&mut self, item: &Item) {
        let mut items = self.notices.get(SOURCE).cloned().unwrap_or_default();
        items.retain(|i| i.id != item.id);
        self.set_notices(SOURCE, items);
    }

    /// `watch`: the connection kept, sent the Claude tabs now and each time
    /// they change.
    pub(crate) fn phone_watch(&mut self, reply: oneshot::Sender<Answer>) {
        let (give, given) = oneshot::channel();
        if reply.send(Answer::Keep(give)).is_err() {
            return;
        }
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let _ = tx.send(self.phone_snapshot().to_string());
        self.phone.watchers.push(tx);
        tokio::spawn(async move {
            let Ok(stream) = given.await else {
                return;
            };
            let (_read, mut write) = stream.into_split();
            while let Some(line) = rx.recv().await {
                let frame = proto::encode_server(&ServerMsg::Output(format!("{line}\n").into()));
                if write.write_all(&frame).await.is_err() {
                    break;
                }
            }
        });
    }

    /// Something happened: while anyone watches, what they see is worked
    /// out again once things settle.
    pub(crate) fn phone_changed(&mut self) {
        if self.phone.watchers.is_empty() || self.phone.due {
            return;
        }
        self.phone.due = true;
        self.send_later(SETTLE, Event::Phone(PhoneEvent::Publish));
    }

    pub(crate) fn phone_event(&mut self, event: PhoneEvent) {
        match event {
            PhoneEvent::Publish => {
                self.phone.due = false;
                let line = self.phone_snapshot().to_string();
                if line == self.phone.last {
                    return;
                }
                self.phone.watchers.retain(|w| w.send(line.clone()).is_ok());
                self.phone.last = line;
            }
        }
    }

    /// Every Claude tab, those that need you first: its state, what it waits
    /// on, its last turn, and the prompt it shows while waiting.
    pub(crate) fn phone_snapshot(&self) -> Value {
        let r = Rules::now();
        let mut tabs: Vec<_> = self
            .sidebar_all_tabs()
            .into_iter()
            .filter(|t| t.claude_pane.is_some())
            .collect();
        tabs.sort_by(|a, b| {
            needs_tab(b, &r)
                .cmp(&needs_tab(a, &r))
                .then_with(|| by_need(a, b))
        });
        let tabs: Vec<Value> = tabs
            .iter()
            .filter_map(|t| {
                let pane = t.claude_pane?;
                let waiting = t.state == crate::sidebar::TabState::Waiting;
                let prompt = waiting
                    .then(|| self.phone_prompt_of(pane))
                    .flatten()
                    .map(|p| prompt_json(&p));
                let transcript = self
                    .sidebar
                    .panes
                    .get(&pane)
                    .and_then(|r| r.transcript.as_ref())
                    .map(|p| p.to_string_lossy().into_owned());
                Some(json!({
                    "tab": t.id.to_string(),
                    "index": t.index,
                    "name": t.name,
                    "repo": t.repo_dir.as_ref().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()),
                    "state": t.state.name(),
                    "needs": needs_tab(t, &r),
                    "since": t.since,
                    "pane": pane.to_string(),
                    "wait": t.wait,
                    "transcript": transcript,
                    "turn": t.turn.as_ref().map(|turn| json!({
                        "prompt": turn.prompt,
                        "reply": turn.reply,
                        "at": turn.at,
                    })),
                    "prompt": prompt,
                }))
            })
            .collect();
        json!({ "tabs": tabs })
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
    }
}
