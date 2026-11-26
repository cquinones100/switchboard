//! The server's side of the phone app: `watch` (every Claude tab's state
//! and last turn, streamed as a line of JSON each time they change) and
//! `phone-pair`, the pictures a tab's Claude showed with `look` kept for
//! the phone, and, after the server takes over from another build, the
//! phone app started again in its tab on this one (`phone_after_upgrade`),
//! since a program a pane runs keeps the build it started with. What the
//! phone does to a tab (`prompt`, `answer`, `stop`, `remind`, `pin`,
//! `kill-tab -s`) is the remote front-end protocol's, src/remote/serve.rs.
//! `watch` lists Claude tabs alone.
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

use crate::mission::remind;
use crate::mux::proto::{self, ServerMsg};
use crate::mux::server::{Answer, Event, Reply, Server};
use crate::mux::{PaneId, TabId};
use crate::remote::serve::prompt_json;
use crate::sidebar::model::Rules;
use crate::{settings, util};

use crate::tabs::prs::TabPr;

use super::{cli, drawer, images, looks, tab_views};

/// How long after a change `watch` waits before it sends, so a burst of
/// changes goes as one.
const SETTLE: Duration = Duration::from_millis(250);

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

/// The tool call a working pane is on, as the phone's working line names
/// it: its tool, its argument's first line (a command, a path, a pattern)
/// and when it started, in seconds.
fn doing_json(doing: &crate::agent::Doing) -> Value {
    json!({
        "tool": doing.tool,
        "arg": doing.arg.lines().next().unwrap_or(""),
        "since": doing.since,
    })
}

impl Server {
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
    /// and branch (for the drawer's search), what it waits on, the tool
    /// call it is on (`doing_json`), its last turn, and the prompt it shows while waiting; and mission control's
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
                    t.state == crate::sidebar::TabState::Waiting || self.remote_asking(pane);
                let prompt = waiting
                    .then(|| self.remote_prompt_of(pane))
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
                    "doing": t.doing.as_ref().map(doing_json),
                    "prompt": prompt,
                    "stoppable": self.remote_stoppable(pane).is_ok(),
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
        self.remote_claude_pane(pane)?;
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
                    .filter(|t| t.source().is_on())
                    .ok_or("This tab has no ticket.")?;
                // What the Tickets view holds of it, else its view's last copy.
                let held = self.ticket_held(&ticket.short()).cloned();
                Ok(Box::new(move || {
                    let copy = held.or_else(|| ticket.last_read());
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
    fn a_working_panes_tool_call_reads_as_its_tool_first_line_and_start() {
        let doing = crate::agent::Doing {
            tool: "Bash".into(),
            arg: "cargo test phone\n  --all".into(),
            since: 1_700_000_000,
            call: "toolu_1".into(),
        };
        assert_eq!(
            doing_json(&doing),
            json!({"tool": "Bash", "arg": "cargo test phone", "since": 1_700_000_000})
        );
    }
}
