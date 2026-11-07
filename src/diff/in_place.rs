//! A tool in a pane's place, leaving the rest of the tab, sidebar
//! included, as it is; the tool ending puts the pane back. Ported from
//! tmux/pane-in-place.sh, which M-d's diff opened through (and lazygit and
//! the PR and ticket images, which can open through `open_in_place` too).
//!
//! Under tmux the tool started in a window of its own and `swap-pane`
//! traded places, the pane waiting in that window. Here the pane is parked
//! instead: it stays in the server with its program running, in no tab's
//! layout, marked `@pane_parked`, and the tool's pane takes its place in
//! the layout, naming it in `@parked_pane` and itself in `@in_place`, which
//! the key uses to quit its own tool. When the tool's pane goes, however it
//! goes (it quit, or M-w closed it), the parked pane takes its place again
//! and has the focus (`give_place_back`, from `remove_pane`). A parked pane
//! whose program ends while parked just goes. The layout saved for a
//! restart has the parked pane in the tool's place (`saved_layout`), so a
//! Claude session waiting behind the diff is resumed after a reboot.
//!
//! M-p's "side" and "full" bring the parked pane beside the tool on its
//! left, half the width, and park it again, so a Claude session and its
//! diff sit side by side; quitting the tool either way brings the pane back
//! whole.

use anyhow::{Result, anyhow};

use crate::mux::layout::Layout;
use crate::mux::server::{ClientId, Server, Tab};
use crate::mux::{Direction, PaneId, TabId};

/// The tool's pane names the pane it stands in for, and itself.
pub const PARKED_PANE: &str = "@parked_pane";
pub const IN_PLACE: &str = "@in_place";
/// A pane waiting while a tool stands in for it.
pub const PANE_PARKED: &str = "@pane_parked";

impl Server {
    /// Runs `command` in `pane`'s place, as tool `tool`; gives the tool's
    /// pane. The pane waits, parked, until the tool's pane goes.
    pub fn open_in_place(
        &mut self,
        pane: PaneId,
        tool: &str,
        command: Vec<String>,
        client: Option<ClientId>,
    ) -> Result<PaneId> {
        let slot = self
            .panes
            .get(&pane)
            .ok_or_else(|| anyhow!("no pane {pane}"))?;
        let tab_id = slot.tab;
        let (cols, rows) = slot.pane.size();
        let cwd = slot.pane.cwd();
        if !self
            .tabs
            .get(&tab_id)
            .is_some_and(|t| t.layout.contains(pane))
        {
            return Err(anyhow!("{pane} is waiting already"));
        }
        let made = self.spawn_pane(tab_id, cwd, Some(command), cols, rows)?;
        let tab = self.tabs.get_mut(&tab_id).expect("checked above");
        tab.layout.replace(pane, made);
        tab.zoomed = false;
        if tab.active == pane {
            tab.active = made;
        }
        if tab.last_pane == Some(pane) {
            tab.last_pane = None;
        }
        if let Some(slot) = self.panes.get_mut(&made) {
            slot.opts.insert(PARKED_PANE.into(), pane.to_string());
            slot.opts.insert(IN_PLACE.into(), tool.into());
        }
        if let Some(slot) = self.panes.get_mut(&pane) {
            slot.opts.insert(PANE_PARKED.into(), "1".into());
        }
        self.relayout(tab_id);
        self.focus_pane(made);
        self.dirty_tab(tab_id);
        // How to get the pane back, the first time.
        self.tip("in-place", client);
        Ok(made)
    }

    /// The pane a tool's pane stands in for, while it is still there.
    pub fn parked_of(&self, tool: PaneId) -> Option<PaneId> {
        self.panes
            .get(&tool)?
            .opts
            .get(PARKED_PANE)?
            .parse()
            .ok()
            .filter(|p| self.panes.contains_key(p))
    }

    /// The tool's pane for `pane`: itself when it is one, else the one in
    /// its tab standing in for it (beside it).
    pub fn tool_for(&self, pane: PaneId) -> Option<PaneId> {
        let slot = self.panes.get(&pane)?;
        if slot.opts.contains_key(IN_PLACE) {
            return Some(pane);
        }
        let tab = self.tabs.get(&slot.tab)?;
        tab.layout
            .panes()
            .into_iter()
            .find(|p| self.parked_of(*p) == Some(pane))
    }

    /// Whether a pane is parked, out of every layout.
    fn parked(&self, pane: PaneId) -> bool {
        self.panes.get(&pane).is_some_and(|s| {
            s.opts.contains_key(PANE_PARKED)
                && !self
                    .tabs
                    .get(&s.tab)
                    .is_some_and(|t| t.layout.contains(pane))
        })
    }

    /// Before a pane is taken out: a tool's pane gives its place back to
    /// the pane it stood in for; a parked pane just goes. True when that
    /// was all there was to do.
    pub fn give_place_back(&mut self, pane: PaneId) -> bool {
        if self.parked(pane) {
            self.panes.remove(&pane);
            let id = pane.to_string();
            for slot in self.panes.values_mut() {
                if slot.opts.get(PARKED_PANE) == Some(&id) {
                    slot.opts.remove(PARKED_PANE);
                }
            }
            return true;
        }
        let Some(parked) = self.parked_of(pane) else {
            return false;
        };
        let Some(tab_id) = self.panes.get(&pane).map(|s| s.tab) else {
            return false;
        };
        if let Some(slot) = self.panes.get_mut(&parked) {
            slot.opts.remove(PANE_PARKED);
            slot.tab = tab_id;
        }
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return false;
        };
        if tab.layout.contains(parked) {
            // Beside it: the tool goes as any pane, the pane taking its room
            // and the focus.
            tab.last_pane = Some(parked);
            if tab.active == pane {
                tab.active = parked;
            }
            return false;
        }
        tab.layout.replace(pane, parked);
        if tab.active == pane {
            tab.active = parked;
        }
        if tab.last_pane == Some(pane) {
            tab.last_pane = None;
        }
        if let Some(mut slot) = self.panes.remove(&pane) {
            slot.pane.kill();
        }
        self.relayout(tab_id);
        self.focus_pane(parked);
        self.dirty_tab(tab_id);
        true
    }

    /// Every pane of a tab, the ones parked behind its tools included.
    pub fn panes_with_parked(&self, tab: TabId) -> Vec<PaneId> {
        let Some(t) = self.tabs.get(&tab) else {
            return Vec::new();
        };
        let mut out = t.layout.panes();
        let tools: Vec<PaneId> = out.clone();
        for tool in tools {
            if let Some(parked) = self.parked_of(tool)
                && !out.contains(&parked)
            {
                out.push(parked);
            }
        }
        out
    }

    /// A tab's layout as it is saved for a restart: each tool's place given
    /// to the pane it stands in for, the focus with it.
    pub fn saved_layout(&self, tab: &Tab) -> (Layout, PaneId, Option<PaneId>) {
        let mut layout = tab.layout.clone();
        let (mut active, mut last) = (tab.active, tab.last_pane);
        for tool in tab.layout.panes() {
            if let Some(parked) = self.parked_of(tool)
                && !layout.contains(parked)
            {
                layout.replace(tool, parked);
                if active == tool {
                    active = parked;
                }
                if last == Some(tool) {
                    last = None;
                }
            }
        }
        (layout, active, last)
    }

    /// M-p's "side" (`beside`) and "full": the pane the tool stands in for
    /// beside it on its left, half the width, or parked again.
    pub fn tool_beside(&mut self, client: ClientId, beside: bool) {
        let Some(current) = self.current_pane(client) else {
            return;
        };
        let tool = self.tool_for(current);
        let Some((tool, parked)) = tool.and_then(|t| Some((t, self.parked_of(t)?))) else {
            self.say(
                Some(client),
                "No tool here standing in for another pane (the diff, lazygit, a PR or ticket image)",
            );
            return;
        };
        let Some(tab_id) = self.panes.get(&tool).map(|s| s.tab) else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return;
        };
        let together = tab.layout.contains(parked);
        if beside {
            if together || !tab.layout.split(tool, parked, Direction::Left, Some(50)) {
                return;
            }
            tab.zoomed = false;
            if let Some(slot) = self.panes.get_mut(&parked) {
                slot.opts.remove(PANE_PARKED);
                slot.tab = tab_id;
            }
        } else {
            if !together || !tab.layout.remove(parked) {
                return;
            }
            if tab.active == parked {
                tab.active = tool;
            }
            if tab.last_pane == Some(parked) {
                tab.last_pane = None;
            }
            if let Some(slot) = self.panes.get_mut(&parked) {
                slot.opts.insert(PANE_PARKED.into(), "1".into());
            }
        }
        self.relayout(tab_id);
        if !beside {
            self.focus_pane(tool);
        }
        self.dirty_tab(tab_id);
    }
}
