//! The server's side of your files' repository (src/files): what `follow`
//! found said through `Server::say`, leak-check's finds asked about before
//! a push, and M-p's "Sync your Switchboard files now". The question is a
//! yes/no box on the client used last, the finds listed under it; it waits
//! for a terminal to be attached, and an answer is kept for the commits it
//! was asked about, so the same finds aren't asked about twice.

use super::FilesEvent;
use crate::mux::server::{ClientId, Server};
use crate::ui::Confirm;

/// What the server keeps for your files' repository.
#[derive(Default)]
pub struct Files {
    /// leak-check's finds waiting to be asked about, with their tip.
    waiting: Option<(String, Vec<String>)>,
}

/// The commands in M-p.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    /// "Sync your Switchboard files now": pull, then push what waits.
    SyncNow,
}

impl Command {
    pub const ALL: [Command; 1] = [Command::SyncNow];

    /// Its name in keys.json.
    pub fn name(self) -> &'static str {
        match self {
            Command::SyncNow => "sync-your-files",
        }
    }

    pub fn parse(name: &str) -> Option<Command> {
        Command::ALL.into_iter().find(|c| c.name() == name)
    }

    pub fn about(self) -> &'static str {
        match self {
            Command::SyncNow => {
                "Sync your Switchboard files now: pull their git repository, then push what changed here"
            }
        }
    }

    /// More on what it does than `about`, for the palette's preview.
    pub fn detail(self) -> &'static str {
        match self {
            Command::SyncNow => {
                "With Settings' \"Your files from a git repository\" on and files.json naming one, your files are pulled every ten minutes and pushed as they change; this does both now and says how it went, after bringing the clone up to date by hand, say."
            }
        }
    }

    pub fn tags(self) -> &'static [&'static str] {
        match self {
            Command::SyncNow => &["switchboard", "settings", "git"],
        }
    }
}

impl Server {
    /// What a look at your files' repository found.
    pub fn files_event(&mut self, event: FilesEvent) {
        match event {
            FilesEvent::Said(text) => self.say(None, &text),
            FilesEvent::Found { tip, hits } => {
                self.files.waiting = Some((tip, hits));
                self.files_ask();
            }
        }
    }

    /// On the server's tick: a question still waiting for a terminal.
    pub fn files_tick(&mut self) {
        if self.files.waiting.is_some() {
            self.files_ask();
        }
    }

    /// Asks whether to push what leak-check found something in. No leaves
    /// the commits in the clone, unpushed.
    fn files_ask(&mut self) {
        let Some(client) = self.latest_client() else {
            return;
        };
        let Some((tip, hits)) = self.files.waiting.take() else {
            return;
        };
        let count = hits.len();
        let question = Confirm::new(format!(
            "Push your Switchboard files? leak-check found {count} {} in them",
            if count == 1 { "thing" } else { "things" }
        ))
        .list(hits);
        self.ask(client, question, move |server, client, yes| {
            super::answer(&tip, yes);
            if !yes {
                server.say(
                    Some(client),
                    &format!(
                        "Not pushed: the change stays committed in {}",
                        crate::paths::tilde(super::clone_dir())
                    ),
                );
            }
        });
    }

    pub fn files_command(&mut self, _client: ClientId, command: Command) {
        match command {
            Command::SyncNow => super::sync_now(),
        }
    }
}
