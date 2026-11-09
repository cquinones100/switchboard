//! A pane's program ended and another started in the same pane, as tmux's
//! `respawn-pane` did: the pane keeps its id, its place in its tab, its size
//! and its options, so whatever names it (a hook's `SWITCHBOARD_PANE`, a
//! tab's `@claude_pane`, a note) still names it. The skills handed a tab
//! over this way (/wt and /wtn to the worktree command that opens the work,
//! queue-and-clean to `claude --resume` in the main clone), and the Slack
//! relay starts itself again in the tab it left at a shell.
//!
//! As with tmux, a pane whose program still runs is refused unless `-k`
//! says to end it. The old program is hung up as closing its pane would and
//! let go of; it goes on ending in the background, and what it writes
//! meanwhile, and its end, carry its own generation (`Pane::generation`),
//! which the server drops, so its end never closes the pane the new one now
//! runs in.
//!
//! What the server knew of the old program goes with it: Claude's state in
//! the pane (a new session says its own), what the sidebar read of it, the
//! pictures it gave the terminal (src/mux/sixel), and the options that
//! belong to the program rather than the pane
//! (`PROGRAM_MARKS`), so a shell started where Notes ran isn't taken for
//! Notes, nor allowed what Notes was.

use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};

use super::PaneId;
use super::server::Server;

/// Pane options that say what the program in it is or may do, set by that
/// program or by whoever started it: the notes app (`@notes_pane`), the pane
/// standing in for it in its tab (`@notes_placeholder`), Notes in easy mode
/// taking the Cmd keys (`@notes_cmd_keys`), Notes with UX 2.0 taking M-p
/// (`@notes_work_key`), and a program allowed to pass
/// kitty's graphics commands to the terminal (`@allow_passthrough`). They go
/// when the program does: at a respawn, and when a restart brings the pane
/// back as a shell.
pub const PROGRAM_MARKS: &[&str] = &[
    "@notes_pane",
    "@notes_placeholder",
    "@notes_cmd_keys",
    "@notes_work_key",
    "@allow_passthrough",
];

impl Server {
    /// Ends `pane`'s program (with `kill`; without, one still running is
    /// refused) and starts `command` in its place (the login shell when
    /// `None`), in `cwd` (the pane's directory when `None`), at the pane's
    /// size.
    pub fn respawn_pane(
        &mut self,
        pane: PaneId,
        cwd: Option<PathBuf>,
        command: Option<Vec<String>>,
        kill: bool,
    ) -> Result<()> {
        let slot = self
            .panes
            .get(&pane)
            .ok_or_else(|| anyhow!("no pane {pane}"))?;
        if !kill && slot.pane.running() {
            bail!("pane {pane} is still running its program (-k ends it)");
        }
        let tab = slot.tab;
        let (cols, rows) = slot.pane.size();
        let cwd = cwd.or_else(|| slot.pane.cwd());
        let fresh = self.start_program(pane, cwd, command, cols, rows)?;
        let slot = self.panes.get_mut(&pane).expect("checked above");
        let mut old = std::mem::replace(&mut slot.pane, fresh);
        old.kill();
        drop(old);
        slot.copy = None;
        slot.opts
            .retain(|name, _| !PROGRAM_MARKS.contains(&name.as_str()));
        self.forget_images(pane);
        self.agents.retain(|p| p != pane);
        self.sidebar.panes.remove(&pane);
        self.sidebar.stale = true;
        self.name_tab(tab);
        self.dirty_tab(tab);
        Ok(())
    }
}
