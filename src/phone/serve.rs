//! The server's side of the phone app: the commands `switchboard phone`
//! asks of it over the socket, `prompt` (a Claude pane's waiting prompt as
//! JSON), `answer` (an option pressed, or a message typed, in a Claude
//! pane), `kill-tab -s` (a Claude tab closed, which the phone asks about
//! first), `remind` (a Claude tab's reminder set or cleared, as the
//! workspace's reminder box does), `pin` (a Claude tab pinned or unpinned,
//! as M-f does), `stop` (a working Claude's turn stopped with Esc, which
//! the phone asks about first) and `watch` (every Claude tab's state and last turn, streamed as
//! a line of JSON each time they change), and the server's log's line for
//! what your phone did, with no notification, since you know what you did
//! and the tab shows it; and, after the server takes over from another
//! build, the phone app started again in its tab on this one
//! (`phone_after_upgrade`), since a program a pane runs keeps the build it
//! started with.
//!
//! A prompt is read and answered with mission control's own reader and
//! keys (`mission::answer`), the ones the Slack relay uses, so the phone
//! presses exactly what the answer box would. Nothing here reaches a pane
//! that isn't running Claude: `prompt`, `answer`, `kill-tab -s`, `remind`,
//! `pin` and `stop` refuse one, and `watch` lists Claude tabs alone.
//!
//! `watch` keeps its connection: the server takes the stream back from
//! the command's task (`Answer::Keep`) and writes `Output` frames to it,
//! one JSON line each, until the phone hangs up. What it sends is worked
//! out a moment after anything happens (`SETTLE`), at most that often, and
//! sent only when it differs from the last, so a burst of hooks costs one.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};

use crate::mission::answer::{self, Prompt};
use crate::mission::remind;
use crate::mux::input::{Key, KeyCode, Mods};
use crate::mux::proto::{self, ServerMsg};
use crate::mux::server::{Answer, Event, Reply, Server};
use crate::mux::{PaneId, TabId};
use crate::sidebar::model::Rules;
use crate::{settings, util};

use crate::tabs::prs::TabPr;
use crate::views::ticket::Ticket;

use super::{cli, drawer, images, looks, tab_views};

/// How long after a change `watch` waits before it sends, so a burst of
/// changes goes as one.
const SETTLE: Duration = Duration::from_millis(250);
/// The longest a message's words run in the log's line.
const SAID_WIDTH: usize = 80;

/// The phone's watchers and what they were last sent, and the pictures
/// each Claude pane's Claude showed with `look` (`looks`), the newest
/// `looks::MOST`, handed over to a new build with the panes.
#[derive(Default)]
pub struct Phone {
    watchers: Vec<mpsc::UnboundedSender<String>>,
    last: String,
    due: bool,
    pub looks: HashMap<PaneId, Vec<looks::Kept>>,
    /// The views the stream last carried and when they were worked out
    /// (`phone_views`).
    pub(super) views: Option<(std::time::Instant, Value)>,
}

#[derive(Debug)]
pub enum PhoneEvent {
    /// Send the watchers what changed.
    Publish,
    /// A tab's Claude showed pictures with `look`, copied for the phone.
    Looked { pane: PaneId, kept: looks::Kept },
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
    /// An option the phone ran whose work off the task is done: the
    /// server finishes it and answers the phone (`phone_finished`).
    Finished(super::mission::Finished),
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

/// The reminder box's times with the clock time each means at `now`, as
/// the phone's reminder sheet lists them.
pub fn remind_times(now: &jiff::Zoned) -> Vec<Value> {
    remind::CHOICES
        .iter()
        .map(|label| {
            let clock = remind::when(label, now)
                .map(|at| remind::clock_of(at, now))
                .unwrap_or_default();
            json!({ "label": label, "clock": clock })
        })
        .collect()
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

    /// Whether Claude's fullscreen view is scrolled back, so what it waits
    /// on is out of sight and a message's keys would reach it unseen.
    fn phone_scrolled_back(&self, pane: PaneId) -> bool {
        self.panes
            .get(&pane)
            .is_some_and(|slot| answer::scrolled_back(&slot.pane.text(0)))
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
            (true, None) if waiting && self.phone_scrolled_back(pane) => {
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

    /// `kill-tab -s from`: the tab of a pane running Claude closed for
    /// `from` (the phone, which asks first), written in the server's log
    /// as answering and messaging are; refused for a pane not running
    /// Claude, as they are.
    pub(crate) fn phone_close(&mut self, pane: PaneId, from: &str) -> Result<Reply, String> {
        if !crate::settings::is_on("phone_close_tabs") {
            return Err("closing tabs from the phone is off in Settings".into());
        }
        self.phone_claude_pane(pane)?;
        let tab = self.panes[&pane].tab;
        let n = self.tab_number(tab);
        let name = self
            .tabs
            .get(&tab)
            .map(|t| t.display_name().to_string())
            .unwrap_or_default();
        self.phone_did(from, tab, "closed it");
        self.kill_tab(tab);
        Ok(Reply::ok(format!("Closed tab {n} {name}\n")))
    }

    /// Whether Esc in a Claude pane would stop its turn, else why not: the
    /// one test `stop` refuses by and `watch` tells the phone (`stoppable`),
    /// so the phone offers ■ exactly when the server takes it.
    fn phone_stoppable(&self, pane: PaneId) -> Result<(), String> {
        if self.agents.state(pane) != Some(crate::agent::State::Working) {
            return Err("its Claude isn't working".into());
        }
        // A permission auto mode asks while still working, or a dialog,
        // which Esc would refuse or close rather than stop the turn. A
        // prompt counts as `watch` sends the phone one, while the pane is
        // marked asked, so a numbered list in Claude's own words, with
        // nothing asked, leaves it allowed.
        let prompt_shown = self.phone_asking(pane) && self.phone_prompt_of(pane).is_some();
        if prompt_shown || self.phone_dialog(pane) {
            return Err("its Claude is asking something: answer it first".into());
        }
        // Scrolled back, Esc would scroll the view down, or reach a prompt
        // hidden below it, rather than stop the turn.
        if self.phone_scrolled_back(pane) {
            return Err(
                "its Claude's view is scrolled back: scroll it down at the computer first".into(),
            );
        }
        Ok(())
    }

    /// `stop`: Esc pressed in a pane's Claude, which stops its turn as Esc
    /// at the computer does; refused for a pane not running Claude, for one
    /// whose Claude isn't working and for one asking a permission or showing
    /// a dialog, which Esc would answer instead. `from` names who asked, for the log.
    pub(crate) fn phone_stop(&mut self, pane: PaneId, from: Option<&str>) -> Result<Reply, String> {
        self.phone_claude_pane(pane)?;
        self.phone_stoppable(pane)?;
        let tab = self.panes[&pane].tab;
        self.press(
            pane,
            vec![answer::Press::Key(Key::new(KeyCode::Esc, Mods::NONE))],
        );
        if let Some(from) = from {
            self.phone_did(from, tab, "Stopped");
        }
        let n = self.tab_number(tab);
        let name = self
            .tabs
            .get(&tab)
            .map(|t| t.display_name().to_string())
            .unwrap_or_default();
        Ok(Reply::ok(format!("Stopped tab {n} {name}\n")))
    }

    /// `remind`: the reminder of a pane's tab set for `when` (one of the
    /// box's times, or one typed, read as the box reads it) with `note`,
    /// or, with `None`, cleared, as the workspace's box sets and clears
    /// one; refused with Settings' "Reminders" off, for a pane not running
    /// Claude, and for a time that can't be read, in the box's words.
    /// `from` names who asked, for the log.
    pub(crate) fn phone_remind(
        &mut self,
        pane: PaneId,
        when: Option<&str>,
        note: &str,
        from: Option<&str>,
    ) -> Result<Reply, String> {
        if !settings::is_on("reminders") {
            return Err("reminders are off in Settings".into());
        }
        self.phone_claude_pane(pane)?;
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
            self.phone_did(from, tab, &what);
        }
        Ok(Reply::ok(format!("{said}\n")))
    }

    /// `pin`: the tab of a pane running Claude pinned, or with `pin` false
    /// unpinned, as M-f does; refused for a pane not running Claude.
    /// `from` names who asked, for the log.
    pub(crate) fn phone_pin(
        &mut self,
        pane: PaneId,
        pin: bool,
        from: Option<&str>,
    ) -> Result<Reply, String> {
        self.phone_claude_pane(pane)?;
        let tab = self.panes[&pane].tab;
        self.set_pin(tab, pin);
        let (did, what) = if pin {
            ("Pinned", "Pinned it")
        } else {
            ("Unpinned", "Unpinned it")
        };
        if let Some(from) = from {
            self.phone_did(from, tab, what);
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
                "{device}, signed in to Tailscale as you, wants to answer, message, set reminders on, pin, stop and close your Claude tabs from the phone app. Type the code it shows (Esc refuses):"
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

    /// `look`'s pictures for the phone (`looks`): with Settings' "Phone
    /// app" and `phone_look_pictures` on, in a tab whose Claude has a
    /// transcript, the pictures the phone can draw among `files` copied
    /// into its images folder off the server's task (none from a PR's
    /// untrusted worktree), and kept on the tab's Claude pane.
    pub(crate) fn phone_look(&self, tab: Option<TabId>, files: Vec<PathBuf>, before_after: bool) {
        if !settings::is_on("phone") || !settings::is_on("phone_look_pictures") {
            return;
        }
        let Some(pane) = tab.and_then(|t| self.tab_claude_pane(t)) else {
            return;
        };
        let Some(transcript) = self
            .sidebar
            .panes
            .get(&pane)
            .filter(|r| r.claude)
            .and_then(|r| r.transcript.clone())
        else {
            return;
        };
        let at = util::now_ms();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let files: Vec<PathBuf> = files
                .into_iter()
                .filter(|f| !f.parent().is_some_and(crate::trust::is_untrusted))
                .collect();
            if let Some(look) = looks::copy(&images::dir(), &files, before_after, at) {
                let kept = looks::Kept { transcript, look };
                let _ = tx.send(Event::Phone(PhoneEvent::Looked { pane, kept }));
            }
        });
    }

    /// `watch`: the connection kept, sent the Claude tabs now and each time
    /// they change.
    pub(crate) fn phone_watch(&mut self, reply: oneshot::Sender<Answer>) {
        let (give, given) = oneshot::channel();
        if reply.send(Answer::Keep(give)).is_err() {
            return;
        }
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let views = self.phone_views();
        let _ = tx.send(self.phone_snapshot(views).to_string());
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
            PhoneEvent::Looked { pane, kept } => {
                if !self.panes.contains_key(&pane) {
                    return;
                }
                let all = self.phone.looks.entry(pane).or_default();
                all.push(kept);
                let over = all.len().saturating_sub(looks::MOST);
                all.drain(..over);
                self.phone_changed();
            }
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
            PhoneEvent::Finished(done) => self.phone_finished(done),
            PhoneEvent::Publish => {
                self.phone.due = false;
                let views = self.phone_views();
                let line = self.phone_snapshot(views).to_string();
                if line == self.phone.last {
                    return;
                }
                self.phone.watchers.retain(|w| w.send(line.clone()).is_ok());
                self.phone.last = line;
            }
        }
    }

    /// Every Claude tab in the sidebar's groups and order: its group, badge
    /// and words as the sidebar has them (`drawer::fields`), its repository
    /// and branch (for the drawer's search), what it waits on, its last
    /// turn, and the prompt it shows while waiting; and mission control's
    /// views the phone's panel lists (`phone_views`), none while Settings'
    /// `phone_views` is off.
    pub(crate) fn phone_snapshot(&self, views: Option<Value>) -> Value {
        let r = Rules::now();
        let looks_on = settings::is_on("phone_look_pictures");
        let tabs: Vec<_> = self
            .sidebar_all_tabs()
            .into_iter()
            .filter(|t| t.claude_pane.is_some())
            .collect();
        let tabs: Vec<Value> = drawer::staged(&tabs, &r)
            .into_iter()
            .filter_map(|(stage, t)| {
                let pane = t.claude_pane?;
                let waiting =
                    t.state == crate::sidebar::TabState::Waiting || self.phone_asking(pane);
                let prompt = waiting
                    .then(|| self.phone_prompt_of(pane))
                    .flatten()
                    .map(|p| prompt_json(&p));
                let file = self
                    .sidebar
                    .panes
                    .get(&pane)
                    .and_then(|r| r.transcript.as_ref());
                let transcript = file.map(|p| p.to_string_lossy().into_owned());
                // What another tab's Claude reaches it by, `SendMessage`'s
                // address, which names a message it sent (`web::sender_name`).
                let address = self
                    .sidebar
                    .panes
                    .get(&pane)
                    .and_then(|r| r.session.as_ref()?.socket.as_ref())
                    .map(|s| format!("uds:{s}"));
                // The pictures its Claude showed in this conversation,
                // none while Settings turns them off.
                let shown: Vec<&looks::Look> = self
                    .phone
                    .looks
                    .get(&pane)
                    .into_iter()
                    .flatten()
                    .filter(|_| looks_on)
                    .filter(|k| Some(&k.transcript) == file)
                    .map(|k| &k.look)
                    .collect();
                let branch = self
                    .tab_names
                    .shown
                    .get(&t.id)
                    .and_then(|shown| shown.branch.clone());
                let mut tab = json!({
                    "tab": t.id.to_string(),
                    "index": t.index,
                    "name": t.name,
                    "repo": t.repo_dir.as_ref().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()),
                    "branch": branch,
                    "pane": pane.to_string(),
                    "wait": t.wait,
                    "transcript": transcript,
                    "address": address,
                    "looks": shown,
                    "turn": t.turn.as_ref().map(|turn| json!({
                        "prompt": turn.prompt,
                        "reply": turn.reply,
                        "at": turn.at,
                    })),
                    "prompt": prompt,
                    "stoppable": self.phone_stoppable(pane).is_ok(),
                    "tab_views": self.phone_tab_rows(t.id),
                });
                if let Some(tab) = tab.as_object_mut() {
                    tab.extend(drawer::fields(t, stage, &r));
                }
                Some(tab)
            })
            .collect();
        // Whether Close tab and Remind me are offered and the app buzzes, sent again as
        // Settings changes them, and the reminder box's times with the
        // clock time each means now, as the box shows them.
        json!({
            "tabs": tabs,
            "views": views,
            "close": crate::settings::is_on("phone_close_tabs"),
            "haptics": crate::settings::is_on("phone_haptics"),
            "reminders": r.reminders,
            "remind_times": remind_times(&util::local_now()),
        })
    }

    // ── A tab's own views (boards 189 and 190) ──────────────────────────────

    /// Whether the phone shows a tab's views: within mission control's on
    /// the phone, while both settings are on.
    fn phone_tab_views_on() -> bool {
        settings::is_on("phone_views") && settings::is_on("phone_tab_views")
    }

    /// What of its PR, its ticket and its note a tab has, as rows of the
    /// panel's list in that order, from what the server holds; `null`
    /// while the tab's views are off, so the app tells off from none. The
    /// note only while `phone_tab_note` is on.
    fn phone_tab_rows(&self, tab: TabId) -> Value {
        if !Self::phone_tab_views_on() {
            return Value::Null;
        }
        let mut rows = Vec::new();
        if let Some(pr) = self
            .tab_current_pr(tab)
            .filter(|_| settings::is_on("github"))
        {
            rows.push(tab_views::pr_row(pr.number, self.phone_pr_facts(tab, &pr)));
        }
        if let Some(short) = self.ticket_known_short(tab) {
            rows.push(tab_views::ticket_row(&short, self.ticket_held(&short)));
        }
        if let Some(note) = self.phone_tab_note(tab) {
            rows.push(tab_views::note_row(&note));
        }
        Value::Array(rows)
    }

    /// What the lookups last said of the tab's PR.
    fn phone_pr_facts(&self, tab: TabId, pr: &TabPr) -> Option<&crate::facts::PrFacts> {
        self.tab_pr_facts(pr).or_else(|| {
            self.facts
                .get(&tab)
                .and_then(|f| f.pr.as_ref())
                .filter(|f| f.number == pr.number)
        })
    }

    /// The tab's note as M-e finds it, while the phone may show it.
    fn phone_tab_note(&self, tab: TabId) -> Option<String> {
        settings::is_on("phone_tab_note")
            .then(|| self.note_for(tab))
            .flatten()
    }

    /// `phone-tab <pr|ticket|note> -t <pane>`: the tab's view of that kind
    /// as JSON for the phone, from what the server holds and the copy its
    /// view on the computer last read, the files read off this task;
    /// refused for a pane not running Claude, a kind the tab hasn't, and
    /// while the phone app, its tab's views or (for the note)
    /// `phone_tab_note` are off.
    pub(crate) fn phone_tab_asked(
        &mut self,
        kind: &str,
        pane: PaneId,
        reply: oneshot::Sender<Answer>,
    ) {
        let answer = |reply: oneshot::Sender<Answer>, r: Reply| {
            let _ = reply.send(Answer::Reply(r));
        };
        match self.phone_tab_read(kind, pane) {
            Err(why) => answer(reply, Reply::fail(why)),
            Ok(read) => {
                tokio::task::spawn_blocking(move || {
                    let r = match read() {
                        Ok(v) => Reply::ok(format!("{v}\n")),
                        Err(why) => Reply::fail(why),
                    };
                    answer(reply, r);
                });
            }
        }
    }

    /// What `phone_tab_asked` reads off the task, worked out on it.
    #[allow(clippy::type_complexity)]
    fn phone_tab_read(
        &self,
        kind: &str,
        pane: PaneId,
    ) -> Result<Box<dyn FnOnce() -> Result<Value, String> + Send>, String> {
        if !settings::is_on("phone") {
            return Err("The phone app is off.".into());
        }
        if !Self::phone_tab_views_on() {
            return Err("A tab's views on the phone are off.".into());
        }
        let kind = tab_views::Kind::of(kind)
            .ok_or_else(|| format!("phone-tab takes pr, ticket or note, not {kind}"))?;
        self.phone_claude_pane(pane)?;
        let tab = self
            .panes
            .get(&pane)
            .map(|slot| slot.tab)
            .ok_or_else(|| format!("no pane {pane}"))?;
        let dir = self
            .tab_path(tab)
            .map(std::path::Path::to_path_buf)
            .or_else(|| self.pane_dir(pane));
        match kind {
            tab_views::Kind::Pr => {
                let pr = self
                    .tab_current_pr(tab)
                    .filter(|_| settings::is_on("github"))
                    .ok_or("This tab has no PR.")?;
                let facts = self.phone_pr_facts(tab, &pr).cloned();
                let url = facts.as_ref().map(|f| f.url.clone()).unwrap_or_default();
                let body = self.pr_body_held(&url);
                // The PR view's copy: by its number when it isn't the
                // branch's, as M-v opens it, else the branch's.
                let elsewhere = self.tab_current_pr_elsewhere(tab);
                Ok(Box::new(move || {
                    let file = match (&elsewhere, &dir) {
                        (Some(pr), _) => pr.repo.as_ref().map(|repo| {
                            crate::paths::cache_dir()
                                .join("pr-view")
                                .join(format!("{}%{}#{}.json", repo.owner, repo.name, pr.number))
                        }),
                        (None, Some(dir)) => crate::views::pr::cache_file(dir),
                        (None, None) => None,
                    };
                    let copy = file.as_deref().and_then(util::read_json);
                    Ok(tab_views::pr_json(
                        pr.number,
                        facts.as_ref(),
                        copy.as_ref(),
                        body.as_deref(),
                    ))
                }))
            }
            tab_views::Kind::Ticket => {
                let ticket = dir
                    .as_deref()
                    .and_then(|d| self.tab_ticket_at(tab, d))
                    .filter(|t| settings::is_on(t.service()))
                    .ok_or("This tab has no ticket.")?;
                let held = match &ticket {
                    Ticket::Linear(id) => self.ticket_held(id).cloned(),
                    _ => None,
                };
                Ok(Box::new(move || {
                    let copy = held.or_else(|| match &ticket {
                        Ticket::Issue { repo, number } => {
                            util::read_json(crate::views::issue::cache_file(repo, *number))
                        }
                        Ticket::Linear(id) => util::read_json(crate::views::ticket::cache_file(id)),
                        Ticket::Project(_) => None,
                    });
                    Ok(tab_views::ticket_json(&ticket, copy.as_ref()))
                }))
            }
            tab_views::Kind::Note => {
                if !settings::is_on("phone_tab_note") {
                    return Err("A tab's note on the phone is off.".into());
                }
                let note = self.phone_tab_note(tab).ok_or("This tab has no note.")?;
                Ok(Box::new(move || {
                    let path = crate::notes::store::notes_dir().join(&note);
                    let text = std::fs::read_to_string(path)
                        .map_err(|_| "The tab's note couldn't be read.".to_string())?;
                    Ok(tab_views::note_json(&note, &text))
                }))
            }
        }
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
