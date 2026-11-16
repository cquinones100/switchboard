//! A tool in a pane's place, leaving the rest of the tab, sidebar
//! included, as it is; the tool ending puts the pane back. Ported from
//! tmux/pane-in-place.sh, which M-d's diff and M-p's lazygit open through
//! (and the PR and ticket images, which can open through `open_in_place`
//! too).
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
//!
//! A lone pane narrower than its spread (Settings' `lone_pane_width`, or the
//! tab's own width) has spare columns at its right: with Settings'
//! `views_beside_narrow_pane` on and at least `BESIDE_LEAST` of them, the
//! tool opens there instead, as "side" leaves it, the pane keeping its
//! width and never parked, so the same key, `q` or M-w closes the tool and
//! the tab is one capped pane again. The diff, with its file list and two
//! sides, takes Settings' `diff_beside_share` of the spread (three quarters
//! by default) when the spare columns give it less, the pane narrowed while
//! it is open and capped again once it closes. New with the lone pane's
//! cap, with no tmux-version file behind it.

use anyhow::{Result, anyhow};
use ratatui::layout::Rect;

use crate::mux::layout::Layout;
use crate::mux::server::{ClientId, Server, Tab};
use crate::mux::{Direction, PaneId, TabId};

/// The tool's pane names the pane it stands in for, and itself.
pub const PARKED_PANE: &str = "@parked_pane";
pub const IN_PLACE: &str = "@in_place";
/// A pane waiting while a tool stands in for it.
pub const PANE_PARKED: &str = "@pane_parked";
/// The fewest spare columns beside a narrow lone pane a tool opens in
/// rather than in the pane's place, as a lone pane gets no fewer.
pub const BESIDE_LEAST: u16 = crate::mux::layout::LONE_LEAST;

/// The columns a tool opening beside a narrow lone pane takes, of a spread
/// `spread` wide with `spare` columns right of the pane's rule: the spare
/// columns, or for the diff Settings' `diff_beside_share` of the spread
/// when that is more, always leaving the pane a column and the rule one.
fn beside_width(tool: &str, spread: u16, spare: u16) -> u16 {
    if tool != crate::diff::serve::TOOL {
        return spare;
    }
    let setting = crate::settings::cached("diff_beside_share");
    share_width(setting.as_ref().and_then(share_percent), spread, spare)
}

/// The columns the diff takes beside a narrow lone pane given its share,
/// `percent` of the spread (none for the spare columns alone).
fn share_width(percent: Option<u32>, spread: u16, spare: u16) -> u16 {
    let Some(percent) = percent else {
        return spare;
    };
    let share = u32::from(spread) * percent / 100;
    let share = u16::try_from(share)
        .unwrap_or(u16::MAX)
        .min(spread.saturating_sub(2));
    spare.max(share)
}

/// The percent of the spread `diff_beside_share` gives the diff: none for
/// `spare`, the spare columns alone.
fn share_percent(value: &serde_json::Value) -> Option<u32> {
    let percent = value.as_u64().or_else(|| value.as_str()?.parse().ok())?;
    u32::try_from(percent.min(100)).ok()
}

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
        // The pane's own directory, never its tab's `@tab_dir` (someone
        // else's PR): a tool is given the directory it works on, and a
        // split or new tab from it starts where this one does.
        let cwd = self.pane_own_dir(pane);
        if !self
            .tabs
            .get(&tab_id)
            .is_some_and(|t| t.layout.contains(pane))
        {
            return Err(anyhow!("{pane} is waiting already"));
        }
        let beside = self.spare_beside(tab_id);
        let width = beside.map_or(cols, |(spread, spare)| {
            beside_width(tool, spread.width, spare.width)
        });
        let made = self.spawn_pane(tab_id, cwd, Some(command), width, rows)?;
        let tab = self.tabs.get_mut(&tab_id).expect("checked above");
        match beside {
            Some((spread, _)) => {
                tab.layout.split(pane, made, Direction::Right, None);
                // The tool takes its columns at the right (the spare ones,
                // so the pane keeps its width, or the diff's share), the
                // rule at the pane's edge the line between.
                let keep = spread.width.saturating_sub(width + 1);
                let now = tab.layout.place(spread).panes;
                if let Some((_, now)) = now.iter().find(|(p, _)| *p == pane) {
                    let (dir, cells) = if keep > now.width {
                        (Direction::Right, keep - now.width)
                    } else {
                        (Direction::Left, now.width - keep)
                    };
                    tab.layout.resize(spread, pane, dir, cells);
                }
            }
            None => {
                tab.layout.replace(pane, made);
            }
        }
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
        if beside.is_none()
            && let Some(slot) = self.panes.get_mut(&pane)
        {
            slot.opts.insert(PANE_PARKED.into(), "1".into());
        }
        self.relayout(tab_id);
        self.focus_pane(made);
        self.dirty_tab(tab_id);
        if beside.is_none() {
            // How to get the pane back, the first time.
            self.tip("in-place", client);
        }
        Ok(made)
    }

    /// The tab's spread and the spare columns in it, when a tool opens
    /// there rather than in the pane's place: Settings'
    /// `views_beside_narrow_pane` on, the tab one pane (nothing beside it,
    /// not zoomed) capped narrower than its spread, with at least
    /// `BESIDE_LEAST` columns right of its rule.
    fn spare_beside(&self, tab_id: TabId) -> Option<(Rect, Rect)> {
        if !crate::settings::is_on("views_beside_narrow_pane") {
            return None;
        }
        let tab = self.tabs.get(&tab_id)?;
        if tab.zoomed || !matches!(tab.layout, Layout::Pane(_)) {
            return None;
        }
        let session = self.session_of_tab(tab_id)?;
        let spread = session.spread_for(tab);
        let spare = crate::mux::beside::spare_of(spread, session.area_for(tab))?;
        (spare.width >= BESIDE_LEAST).then_some((spread, spare))
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
    pub(crate) fn parked(&self, pane: PaneId) -> bool {
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
        // A tool can stand in for a tool (the diff opened over lazygit or a
        // view), so each parked pane is asked for its own in turn.
        let mut next = 0;
        while next < out.len() {
            if let Some(parked) = self.parked_of(out[next])
                && !out.contains(&parked)
            {
                out.push(parked);
            }
            next += 1;
        }
        out
    }

    /// The pane drawn where a parked pane was: the tool standing in for
    /// it, or, when that tool is parked behind another, the one in front.
    pub(crate) fn in_front_of(&self, parked: PaneId) -> Option<PaneId> {
        let tab_id = self.panes.get(&parked)?.tab;
        let tab = self.tabs.get(&tab_id)?;
        let mut behind = parked;
        for _ in 0..self.panes.len() {
            let tool = self
                .panes
                .iter()
                .find(|(id, slot)| slot.tab == tab_id && self.parked_of(**id) == Some(behind))
                .map(|(id, _)| *id)?;
            if tab.layout.contains(tool) {
                return Some(tool);
            }
            behind = tool;
        }
        None
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
        self.place_beside(tool, parked, beside);
    }

    /// The parked pane `parked` brought beside the tool standing in for it,
    /// whichever pane has the focus. False when no tool stands in for it.
    pub fn unpark_beside(&mut self, parked: PaneId) -> bool {
        let tool = self
            .panes
            .keys()
            .copied()
            .find(|t| self.parked_of(*t) == Some(parked));
        match tool {
            Some(tool) => {
                self.place_beside(tool, parked, true);
                true
            }
            None => false,
        }
    }

    fn place_beside(&mut self, tool: PaneId, parked: PaneId, beside: bool) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_diff_takes_its_share_of_the_spread_or_the_spare_columns_when_more() {
        // A 210-column spread, the pane capped at 80: 129 spare.
        assert_eq!(share_width(Some(75), 210, 129), 157);
        assert_eq!(share_width(Some(50), 210, 129), 129);
        assert_eq!(share_width(None, 210, 129), 129);
        // Always a column for the pane and one for the rule.
        assert_eq!(share_width(Some(100), 210, 129), 208);
    }

    #[test]
    fn the_share_reads_a_percent_and_spare_as_none() {
        assert_eq!(share_percent(&json!("75")), Some(75));
        assert_eq!(share_percent(&json!(66)), Some(66));
        assert_eq!(share_percent(&json!("spare")), None);
    }

    #[test]
    fn only_the_diff_takes_a_share() {
        assert_eq!(beside_width(crate::diff::serve::LAZYGIT, 210, 129), 129);
        assert_eq!(beside_width("pr-view", 210, 129), 129);
    }
}
