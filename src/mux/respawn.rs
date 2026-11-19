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
//! Notes, nor allowed what Notes was. The tab's `@tab_dir` goes too: the
//! tab is handed to other work, which is about the directory it starts in.

use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};

use super::PaneId;
use super::server::Server;

/// Pane options that say what the program in it is or may do, set by that
/// program or by whoever started it: the notes app (`@notes_pane`), the pane
/// standing in for it in its tab (`@notes_placeholder`), Notes in easy mode
/// taking the Cmd keys (`@notes_cmd_keys`), the note Notes shows
/// (`@notes_open`), a program allowed to pass
/// kitty's graphics commands to the terminal (`@allow_passthrough`), and the
/// mouse pointer's shape an app names over its pane (`@pointer`,
/// crates/ui/src/pointer.rs), and a ticket view's mark and the ticket it
/// reads (`@ticket_view`, `@ticket_view_id`, which M-l opens), and a link
/// view's mark and link (`@link_view`, `@link_url`). They go
/// when the program does: at a respawn, and when a restart brings the pane
/// back as a shell.
pub const PROGRAM_MARKS: &[&str] = &[
    "@notes_pane",
    "@notes_placeholder",
    "@notes_cmd_keys",
    "@notes_open",
    "@allow_passthrough",
    crate::ui::pointer::PANE_OPTION,
    crate::views::TICKET_ID,
    crate::views::TICKET_VIEW,
    crate::views::LINK_VIEW,
    crate::views::LINK_URL,
];

/// The tab options about the work a tab was doing, which go when it is
/// handed to other work: its directory, a sealed review's try, and the
/// ticket chosen for it.
pub const HANDED_OVER: &[&str] = &[
    super::server::TAB_DIR,
    crate::views::TAB_TICKET,
    crate::usage::serve::SEALED_REVIEW,
    crate::usage::serve::SEALED_REVIEW_FETCH,
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
        self.respawn(pane, cwd, command, kill, true)
    }

    /// `respawn_pane` for a view read again in its own place: the program
    /// replaced, the tab not handed to other work, so what it is about and
    /// the views beside it stay.
    pub fn replace_program(&mut self, pane: PaneId, command: Option<Vec<String>>) -> Result<()> {
        self.respawn(pane, None, command, true, false)
    }

    fn respawn(
        &mut self,
        pane: PaneId,
        cwd: Option<PathBuf>,
        command: Option<Vec<String>>,
        kill: bool,
        handed_over: bool,
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
        let cwd = cwd.or_else(|| self.pane_own_dir(pane));
        let fresh = self.start_program(pane, cwd, command, cols, rows)?;
        let slot = self.panes.get_mut(&pane).expect("checked above");
        let mut old = std::mem::replace(&mut slot.pane, fresh);
        old.kill();
        drop(old);
        slot.copy = None;
        slot.opts
            .retain(|name, _| !PROGRAM_MARKS.contains(&name.as_str()));
        self.forget_images(pane);
        // The tab is handed to other work (/wt, /wtn, queue-and-clean's
        // resume): the directory it was about goes with what it was doing.
        // A sealed review's try isn't the new work's either.
        if handed_over && let Some(t) = self.tabs.get_mut(&tab) {
            for name in HANDED_OVER {
                t.opts.remove(*name);
            }
        }
        // So does a ticket view beside it, showing the old work's ticket.
        if let Some(view) = self
            .marked_in(tab, crate::views::TICKET_VIEW)
            .filter(|_| handed_over)
            .filter(|v| *v != pane)
            // A live view, not a shell a restart brought back in its place.
            .filter(|v| {
                self.panes
                    .get(v)
                    .is_some_and(|s| s.opts.contains_key(crate::views::TICKET_ID))
            })
        {
            // The pane in front stays in front, though closing a view in
            // another's place gives that place, and the focus, back.
            // Set as the tab's own pane in front, never by focus_pane, which
            // would bring a tab in the background to the front.
            // Its zoom and its last pane stay too, the view's place in them
            // taken by the pane it stood in for.
            let before = self
                .tabs
                .get(&tab)
                .map(|t| (t.active, t.last_pane, t.zoomed));
            let restored = self.parked_of(view);
            self.kill_pane(view);
            if let Some((active, last, zoomed)) = before
                && active != view
                && self.panes.contains_key(&active)
                && let Some(t) = self.tabs.get_mut(&tab)
            {
                t.active = active;
                t.last_pane = last.and_then(|l| if l == view { restored } else { Some(l) });
                let rezoom = t.zoomed != zoomed;
                t.zoomed = zoomed;
                if rezoom {
                    self.relayout(tab);
                }
            }
        }
        self.agents.restarted(pane);
        self.sidebar.panes.remove(&pane);
        self.sidebar.stale = true;
        self.name_tab(tab);
        self.dirty_tab(tab);
        Ok(())
    }
}
