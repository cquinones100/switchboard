//! The server's side of updates, as repo-update.sh's `check`, `periodic`,
//! `now` and `prompt` were: a look as a terminal attaches and every hour on
//! the server's tick, run off the server's task, and the box offering what
//! it found on the client used last. The box is a menu, "Update now" first
//! so Enter alone updates, then "Not now"; Esc, q or a click outside it is
//! "Not now" too. Either answer is recorded (`update-offered`), so the
//! hourly look doesn't ask about that release again; a box closed by the
//! server for a newer one isn't an answer, so coming back after hours shows
//! what is new now, as repo-update.sh closed a prompt left open for a newer
//! look.
//!
//! "Update now" opens a tab running `switchboard update --tag <tag>
//! --hold`, which downloads, checks and swaps the binary and then runs the
//! guided setup with that tab's terminal, as the popup ran setup.sh; the
//! server itself goes on with the code it started with until it starts
//! again, which the tab says. M-p's "Update Switchboard" looks at once
//! whatever the setting says and says when there is nothing new or GitHub
//! can't be reached; its "Run Switchboard's setup" opens the guided setup
//! in a tab the same way.

use std::time::Instant;

use ratatui::buffer::Buffer;

use super::{Decision, Release, Version, Why};
use crate::mux::input::{Key, Mouse};
use crate::mux::server::{ClientId, Event, Server};
use crate::ui::{Look, Menu, MenuOption, Outcome, Overlay, Step};
use crate::{paths, settings, util};

/// What the server keeps for updates.
#[derive(Default)]
pub struct Updates {
    /// A look is under way, so another isn't started beside it.
    checking: bool,
    /// The box up now, if one is.
    prompt: Option<Prompt>,
    serial: u64,
}

/// A box up on a client, offering a release.
struct Prompt {
    client: ClientId,
    tag: String,
    serial: u64,
}

/// What a look off the server's task found.
#[derive(Debug)]
pub enum UpdateEvent {
    Checked {
        why: Why,
        /// The client it is for, when one asked or attached.
        client: Option<ClientId>,
        /// The releases, or what went wrong, in words.
        found: Result<Vec<Release>, String>,
    },
}

/// The commands in M-p.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    /// "Update Switchboard": look now, and offer what is new.
    UpdateNow,
    /// "Run Switchboard's setup": the guided setup in a tab.
    Setup,
}

impl Command {
    pub const ALL: [Command; 2] = [Command::UpdateNow, Command::Setup];

    /// Its name in keys.json.
    pub fn name(self) -> &'static str {
        match self {
            Command::UpdateNow => "update-switchboard",
            Command::Setup => "run-setup",
        }
    }

    pub fn parse(name: &str) -> Option<Command> {
        Command::ALL.into_iter().find(|c| c.name() == name)
    }

    /// What it does, as tmux.conf described its binding.
    pub fn about(self) -> &'static str {
        match self {
            Command::UpdateNow => {
                "Update Switchboard: look for a newer release now, and install it with its setup"
            }
            Command::Setup => {
                "Run Switchboard's setup, to put in place what isn't or answer a question said no to"
            }
        }
    }

    /// tmux.conf's tags.
    pub fn tags(self) -> &'static [&'static str] {
        match self {
            Command::UpdateNow => &["switchboard", "update", "release"],
            Command::Setup => &["switchboard", "setup", "claude"],
        }
    }
}

/// The box: a menu, under a kind of its own so a newer look can find it and
/// close it without touching any other menu.
struct UpdateBox(Menu);

impl Overlay for UpdateBox {
    fn kind(&self) -> &'static str {
        "update"
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        self.0.draw(buf, look);
    }

    fn key(&mut self, key: Key, look: &Look) -> Step {
        self.0.key(key, look)
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        self.0.mouse(mouse, look)
    }

    fn deadline(&self) -> Option<Instant> {
        Overlay::deadline(&self.0)
    }
}

const UPDATE_NOW: usize = 0;
const NOT_NOW: usize = 1;

impl Server {
    /// On the server's tick: the hourly look, once the hour since the last
    /// one is up.
    pub fn update_tick(&mut self) {
        if self.update.checking || !super::looks_by_itself() {
            return;
        }
        if !super::allowed(Why::Hourly, &update_setting(false)) {
            return;
        }
        if super::hour_up(super::last_checked(), util::now_secs()) {
            self.update_look(Why::Hourly, None);
        }
    }

    /// A terminal attached: a look, offering anything newer on it.
    pub fn update_attached(&mut self, client: ClientId) {
        if super::looks_by_itself() && super::allowed(Why::Attach, &update_setting(true)) {
            self.update_look(Why::Attach, Some(client));
        }
    }

    /// M-p's commands.
    pub fn update_command(&mut self, client: ClientId, command: Command) {
        match command {
            Command::UpdateNow => self.update_look(Why::Asked, Some(client)),
            Command::Setup => self.update_tab(client, &["update", "--setup", "--hold"]),
        }
    }

    /// Looks for releases off the server's task. The time is recorded as it
    /// starts, as repo-update.sh did, so a look that fails waits its hour
    /// too rather than being tried on every tick.
    fn update_look(&mut self, why: Why, client: Option<ClientId>) {
        if self.update.checking {
            if why == Why::Asked {
                self.say(client, "Switchboard is already looking for updates.");
            }
            return;
        }
        self.update.checking = true;
        super::mark_checked(util::now_secs());
        if why == Why::Asked {
            self.say(client, "Looking for Switchboard updates…");
        }
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let found = super::release::list("Switchboard update check")
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(Event::Update(UpdateEvent::Checked { why, client, found }));
        });
    }

    pub fn update_event(&mut self, event: UpdateEvent) {
        let UpdateEvent::Checked { why, client, found } = event;
        self.update.checking = false;
        let client = client
            .filter(|c| self.clients.contains_key(c))
            .or_else(|| self.latest_client());
        let releases = match found {
            Ok(releases) => releases,
            Err(why_not) => {
                if why == Why::Asked {
                    self.say(
                        client,
                        &format!("Couldn't look for Switchboard updates: {why_not}"),
                    );
                }
                return;
            }
        };
        let current = Version::current();
        let offered = super::last_offered();
        match super::decide(
            why,
            &releases,
            &current,
            super::release::this_target(),
            offered.as_deref(),
        ) {
            Decision::Quiet => {}
            Decision::UpToDate => self.say(
                client,
                &format!(
                    "Switchboard {current} is up to date ({}).",
                    env!("SWITCHBOARD_COMMIT")
                ),
            ),
            Decision::Offer(newer) => {
                if let Some(client) = client {
                    self.offer(client, &newer, &current);
                }
            }
        }
    }

    /// The box, on `client`. One still up for the same release on the same
    /// client is left as it is; any other is closed first, unanswered.
    fn offer(&mut self, client: ClientId, newer: &[Release], current: &Version) {
        let newest = &newer[0];
        if let Some(open) = &self.update.prompt {
            if open.tag == newest.tag && open.client == client {
                return;
            }
            let old = open.client;
            self.update.prompt = None;
            self.close_overlays(old, "update");
        }
        self.update.serial += 1;
        let serial = self.update.serial;
        self.update.prompt = Some(Prompt {
            client,
            tag: newest.tag.clone(),
            serial,
        });
        let menu = Menu::new(
            "Switchboard",
            vec![
                MenuOption::new(format!("Update now, to {}", newest.version)),
                MenuOption::new("Not now"),
            ],
        )
        .body(super::offer_text(newer, current, util::now_secs()));
        let tag = newest.tag.clone();
        let then = Box::new(move |s: &mut Server, c: ClientId, outcome: Outcome| {
            // Closed by a newer look: no answer, so it asks again.
            if s.update.prompt.as_ref().map(|p| p.serial) != Some(serial) {
                return;
            }
            s.update.prompt = None;
            super::mark_offered(&tag);
            if outcome == Outcome::Chose(UPDATE_NOW) {
                s.update_tab(c, &["update", "--tag", &tag, "--hold"]);
            } else {
                debug_assert!(matches!(
                    outcome,
                    Outcome::Chose(NOT_NOW) | Outcome::Dismissed
                ));
                // How updates also arrive, once (tip.sh's `updates`).
                s.tip("updates", Some(c));
            }
        });
        self.open(client, Box::new(UpdateBox(menu)), Some(then));
    }

    /// A tab in the client's session running this binary with `args`.
    fn update_tab(&mut self, client: ClientId, args: &[&str]) {
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        let exe = match util::exe() {
            Ok(exe) => exe,
            Err(e) => {
                self.say(Some(client), &format!("Couldn't find switchboard: {e}"));
                return;
            }
        };
        let mut command = vec![exe.to_string_lossy().into_owned()];
        command.extend(args.iter().map(|a| a.to_string()));
        let name = if args.contains(&"--setup") {
            "Switchboard setup"
        } else {
            "Switchboard update"
        };
        if let Err(e) = self.new_tab(
            &session,
            Some(paths::home()),
            Some(command),
            Some(name.to_string()),
            false,
        ) {
            self.say(Some(client), &format!("Couldn't open a tab for it: {e}"));
        }
    }
}

/// The update_check setting's value: "hourly", "open" or "off"; read from
/// the file when `fresh` (a terminal attaching, which is rare), else as the
/// server last read it (every tick).
fn update_setting(fresh: bool) -> String {
    if fresh {
        settings::setting("update_check")
    } else {
        settings::cached("update_check")
    }
    .and_then(|v| v.as_str().map(str::to_string))
    .unwrap_or_else(|| "hourly".to_string())
}
