//! The server's side of the phone app: the three commands `switchboard
//! phone` asks of it over the socket, `prompt` (a Claude pane's waiting
//! prompt as JSON), `answer` (an option pressed, or a message typed, in a
//! Claude pane) and `watch` (every Claude tab's state and last turn,
//! streamed as a line of JSON each time they change), and the server's
//! log's line for what your phone did, with no notification, since the tab
//! itself shows it; and, after the server takes over from another build,
//! the phone app started again in its tab on this one
//! (`phone_after_upgrade`), since a program a pane runs keeps the build it
//! started with.
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

use crate::mission::answer::{self, Prompt};
use crate::mux::proto::{self, ServerMsg};
use crate::mux::server::{Answer, Event, Reply, Server};
use crate::mux::{PaneId, TabId};
use crate::sidebar::model::{Rules, by_need, needs_tab};
use crate::{settings, util};

use super::cli;

/// How long after a change `watch` waits before it sends, so a burst of
/// changes goes as one.
const SETTLE: Duration = Duration::from_millis(250);
/// The longest a message's words run in the log's line.
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
    /// The app an older build ran has exited: its tab's pane starts it
    /// again on this one.
    OldExited { pane: PaneId, old: i64 },
    /// Whether the app started again on this build still runs at `until`:
    /// its lock is written before it fetches its certificate and binds its
    /// port, so one that stops once it was `seen` failed to start too.
    Restarting {
        old: i64,
        seen: bool,
        until: std::time::Instant,
    },
}

/// How long the app an older build ran has to stop after it is asked
/// before it is killed, and how long the one started in its place has to
/// be running (not stopped after writing its lock) before it counts as
/// started; the screen says so when it isn't.
const STOPPING: Duration = Duration::from_secs(10);
const STARTING: Duration = Duration::from_secs(15);

/// A prompt as the phone reads it.
pub fn prompt_json(found: &Prompt) -> Value {
    json!({
        "id": crate::relay::tabs::hash_of(found),
        "question": found.question,
        "options": found
            .options
            .iter()
            .map(|o| json!({"n": o.n, "label": o.label, "checked": o.checked, "detail": o.detail}))
            .collect::<Vec<_>>(),
        "multi": found.multi,
        "free": found.free,
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

    /// Whether a pane's Claude waits on you: marked waiting, or, in auto
    /// mode, asked a permission its Notification hook hasn't yet marked
    /// (`asked`), while the prompt is already on screen.
    fn phone_asking(&self, pane: PaneId) -> bool {
        self.agents.state(pane) == Some(crate::agent::State::Waiting)
            || self.agents.get(pane).is_some_and(|a| a.asked.is_some())
    }

    fn phone_dialog(&self, pane: PaneId) -> bool {
        self.panes
            .get(&pane)
            .is_some_and(|slot| crate::relay::tabs::dialog_in(&slot.pane.text(0)).is_some())
    }

    fn phone_draft(&self, pane: PaneId) -> bool {
        self.panes.get(&pane).is_some_and(|slot| {
            crate::relay::tabs::draft_in(&slot.pane.text(0), &slot.pane.typed_text(0)).is_some()
        })
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
    /// never answers the next question. `from` names who asked, for the log.
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
        // A numbered list in a finished reply reads as options too, so a
        // message waits on a prompt only while its Claude is waiting, as
        // the drawer shows one and the relay reads one.
        let waiting = self.phone_asking(pane);
        let said = match (message, found) {
            (true, Some(_)) if waiting => {
                return Err("its Claude is showing a prompt: answer that first".into());
            }
            (true, _) if reply.trim().is_empty() => return Err("nothing to send".into()),
            // A dialog before its session starts (trust this folder, a new
            // MCP server), which Enter would confirm unseen, and a draft in
            // its prompt box, which would go with the words, are left for
            // the computer, as mission control's answer box leaves them.
            (true, _) if self.phone_dialog(pane) => {
                return Err(
                    "its Claude is showing a dialog before it starts: answer it at the computer"
                        .into(),
                );
            }
            (true, _) if self.phone_draft(pane) => {
                return Err(
                    "its Claude has a draft in its prompt box, which would go with this: send it or clear it at the computer"
                        .into(),
                );
            }
            (true, _) => {
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
    /// server's log alone: no notification, since the tab shows what was
    /// done there.
    pub(crate) fn phone_did(&self, from: &str, tab: TabId, what: &str) {
        let n = self.tab_number(tab);
        let name = self
            .tabs
            .get(&tab)
            .map(|t| t.display_name().to_string())
            .unwrap_or_default();
        eprintln!("switchboard server: from your {from}: tab {n} ({name}): {what}");
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

    /// M-p's "Open the phone app in its own tab", "Stop the phone app" and
    /// "Send a test notification to my phone": `switchboard phone start`,
    /// `stop` or `test`, which say what came of it in a box, run off the
    /// server's task as the Slack relay's are.
    pub fn phone_app(&mut self, command: &str) {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let spawned = tokio::process::Command::new(exe)
            .args(["phone", command, "--say"])
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

    /// After taking over from another build (`upgrade::adopt`): the phone
    /// app running in its own tab from a binary other than this server's
    /// is asked to stop, and once it has exited (killed if it hasn't within
    /// `STOPPING`, so its port and lock are free), its pane runs it again
    /// from this one. One outside its tab, or with Settings' "Phone app"
    /// off, is left as it is.
    pub(crate) fn phone_after_upgrade(&mut self) {
        if !settings::is_on("phone") {
            return;
        }
        let Some(app) = cli::running_app() else {
            return;
        };
        if app.build == cli::build_of_self() {
            return;
        }
        let Some(pane) = self.phone_app_pane(app.pid) else {
            return;
        };
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let tx = self.tx.clone();
        let old = app.pid;
        tokio::spawn(async move {
            let pid = old as libc::pid_t;
            // SAFETY: a signal to the process the lock names, alive as
            // just checked.
            unsafe { libc::kill(pid, libc::SIGTERM) };
            let deadline = tokio::time::Instant::now() + STOPPING;
            while util::is_alive(old) {
                if tokio::time::Instant::now() >= deadline {
                    // SAFETY: as above; it didn't stop when asked.
                    unsafe { libc::kill(pid, libc::SIGKILL) };
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let _ = tx.send(Event::Phone(PhoneEvent::OldExited { pane, old }));
        });
    }

    /// The pane of a tab marked as the phone app's whose program, or one in
    /// front in it, is `pid`.
    fn phone_app_pane(&self, pid: i64) -> Option<PaneId> {
        let pid = u32::try_from(pid).ok()?;
        self.panes
            .iter()
            .filter(|(_, slot)| slot.opts.get(cli::MARK).is_some_and(|v| v == "1"))
            .filter(|(_, slot)| self.tabs.contains_key(&slot.tab))
            .find(|(_, slot)| {
                slot.pane.pid() == Some(pid) || slot.pane.foreground_pids().contains(&pid)
            })
            .map(|(id, _)| *id)
    }

    pub(crate) fn phone_event(&mut self, event: PhoneEvent) {
        match event {
            PhoneEvent::OldExited { pane, old } => {
                match self.replace_program(pane, Some(cli::tab_command())) {
                    Ok(()) => {
                        if let Some(slot) = self.panes.get_mut(&pane) {
                            slot.opts.insert(cli::MARK.into(), "1".into());
                        }
                        eprintln!(
                            "switchboard server: the phone app was restarted on the new build in pane {pane}"
                        );
                        self.phone_event(PhoneEvent::Restarting {
                            old,
                            seen: false,
                            until: std::time::Instant::now() + STARTING,
                        });
                    }
                    Err(e) => {
                        let said = format!(
                            "Phone app: couldn't start it again on the new build ({e:#}). {}",
                            cli::status_words()
                        );
                        eprintln!("switchboard server: {said}");
                        self.say(None, &said);
                    }
                }
            }
            PhoneEvent::Restarting { old, seen, until } => {
                let runs = cli::running_app().is_some_and(|app| app.pid != old);
                let stopped_after_starting = seen && !runs;
                let waited = std::time::Instant::now() >= until;
                if waited && runs {
                    return;
                }
                if !waited && !stopped_after_starting {
                    let seen = seen || runs;
                    self.send_later(
                        Duration::from_millis(250),
                        Event::Phone(PhoneEvent::Restarting { old, seen, until }),
                    );
                    return;
                }
                let said = format!(
                    "Phone app: it didn't start again on the new build. {} Its tab and {} say why.",
                    cli::status_words(),
                    super::log::file().display()
                );
                eprintln!("switchboard server: {said}");
                self.say(None, &said);
            }
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

    /// Every Claude tab, those that need you first: its repository and
    /// branch (for the drawer's search), its state, what it waits on, its
    /// last turn, and the prompt it shows while waiting.
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
                let waiting =
                    t.state == crate::sidebar::TabState::Waiting || self.phone_asking(pane);
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
                let branch = self
                    .tab_names
                    .shown
                    .get(&t.id)
                    .and_then(|shown| shown.branch.clone());
                Some(json!({
                    "tab": t.id.to_string(),
                    "index": t.index,
                    "name": t.name,
                    "repo": t.repo_dir.as_ref().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()),
                    "branch": branch,
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
        assert_eq!(v["options"][1]["detail"], "", "no explainer under it");
        let explained = answer::options_in(
            "Which way?\n❯ 1. Fast\n     Skips the checks\n  2. Slow\n     Runs every check\n",
        )
        .unwrap();
        let v = prompt_json(&explained);
        assert_eq!(v["options"][0]["detail"], "Skips the checks");
        assert_eq!(v["options"][1]["detail"], "Runs every check");
    }
}
