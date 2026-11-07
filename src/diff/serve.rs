//! The server's side of the diff: what M-d and the palette's diff commands
//! do, as tmux.conf bound them (M-d to pane-in-place.sh's `open diffview`,
//! the palette's to `side`, `full`, and the keys pressed in the diff for
//! its story and a question), and the story told ahead for each tab's new
//! commit, looked for on the server's tick.
//!
//! M-d opens `switchboard diff` in the pane's place, in the pane's
//! directory; the program works out the base itself (fetching it while it
//! says so on its own screen), so the key never waits on the network.
//! Pressed again in the diff, or in the pane the diff stands in for beside
//! it, it closes the diff, which gives the pane its place back.

use std::path::PathBuf;

use super::in_place::IN_PLACE;
use super::{Command, ahead};
use crate::mux::PaneId;
use crate::mux::keys::Action;
use crate::mux::server::{ClientId, Server};
use crate::{paths, settings};

/// The diff's name in `@in_place`.
pub const TOOL: &str = "diff";

/// The program to run in the diff's pane: this binary.
fn exe() -> String {
    std::env::current_exe()
        .ok()
        .filter(|p| p.exists())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "switchboard".into())
}

impl Server {
    /// A diff command, from its key or the palette.
    pub fn diff_command(&mut self, client: ClientId, command: Command) {
        match command {
            Command::Diff => self.toggle_diff(client),
            Command::Side => self.tool_beside(client, true),
            Command::Full => self.tool_beside(client, false),
            // Pressed in the diff after C-\ C-n, which takes it back to its
            // own keys from a box being typed in, as tmux.conf's palette
            // entries pressed them in Neovim.
            Command::CopyStory => self.press_in_diff(client, b"\x1c\x0eyS"),
            Command::Ask => self.press_in_diff(client, b"\x1c\x0ega"),
        }
    }

    /// The diff's pane for the pane a client is in: itself, or the diff
    /// standing in for it beside it.
    fn diff_here(&self, client: ClientId) -> Option<PaneId> {
        let pane = self.current_pane(client)?;
        let tool = self.tool_for(pane)?;
        (self.panes.get(&tool)?.opts.get(IN_PLACE)? == TOOL).then_some(tool)
    }

    /// Which of the diff's commands the palette lists for a client: those
    /// for the diff (`#diffview`) only while its pane has the diff in it,
    /// as palette.sh's filter listed them, asking only with diff_ask on,
    /// and "side" and "full" only where a tool stands in for a pane. Any
    /// other action is listed.
    pub fn diff_palette(&self, client: ClientId) -> impl Fn(&Action) -> bool + use<> {
        let diff = self.diff_here(client).is_some();
        let tool = self
            .current_pane(client)
            .and_then(|p| self.tool_for(p))
            .is_some();
        let ask = settings::is_on("diff_ask");
        move |action| match action {
            Action::Diff(Command::CopyStory) => diff,
            Action::Diff(Command::Ask) => diff && ask,
            Action::Diff(Command::Side | Command::Full) => tool,
            _ => true,
        }
    }

    fn press_in_diff(&mut self, client: ClientId, keys: &[u8]) {
        match self.diff_here(client) {
            Some(pane) => {
                if let Some(slot) = self.panes.get_mut(&pane) {
                    let _ = slot.pane.write(keys);
                }
            }
            None => self.say(Some(client), "No diff in this pane: M-d opens one"),
        }
    }

    /// M-d: the branch's diff in place of the pane, or closed.
    fn toggle_diff(&mut self, client: ClientId) {
        if let Some(diff) = self.diff_here(client) {
            self.kill_pane(diff);
            return;
        }
        let Some(pane) = self.current_pane(client) else {
            return;
        };
        let dir = self
            .panes
            .get(&pane)
            .and_then(|s| s.pane.cwd())
            .unwrap_or_else(paths::home);
        if crate::tabs::git::locate(&dir).is_none() {
            self.say(
                Some(client),
                &format!("Not in a git repository: {}", paths::tilde(&dir)),
            );
            return;
        }
        let command = vec![exe(), "diff".into(), dir.to_string_lossy().into_owned()];
        if let Err(e) = self.open_in_place(pane, TOOL, command, Some(client)) {
            self.say(Some(client), &format!("Couldn't open the diff: {e}"));
        }
    }

    /// On the server's tick: the story of a tab's new commit told ahead,
    /// when the settings want it.
    pub fn diff_tick(&mut self) {
        if !ahead::due() || !ahead::wanted() || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let mut dirs: Vec<PathBuf> = Vec::new();
        for s in &self.sessions {
            for tab in &s.tabs {
                for pane in self.panes_with_parked(*tab) {
                    if let Some(dir) = self.panes.get(&pane).and_then(|p| p.pane.cwd())
                        && !dirs.contains(&dir)
                    {
                        dirs.push(dir);
                    }
                }
            }
        }
        let exe = exe();
        tokio::task::spawn_blocking(move || ahead::story_ahead(&dirs, &exe));
    }
}
