//! A tab's panes as a tree of splits, each split side by side or one above
//! another, with a one-cell line between panes as tmux drew. Sizes are kept
//! as shares of their split, so a window resized keeps its proportions, and
//! a resize by cells moves the shares. Pure data, serialized for the layout
//! saved across restarts.
//!
//! STUB: the signatures are the contract; step 2 fills them in.

use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};

use super::{Direction, PaneId};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Layout {
    Pane(PaneId),
    Split {
        /// Children side by side (true) or one above another (false).
        across: bool,
        children: Vec<Layout>,
        /// Each child's share of the split, summing to 1.
        shares: Vec<f32>,
    },
}

/// A line between panes, to be drawn: from (`x`, `y`), `len` cells long.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Border {
    pub x: u16,
    pub y: u16,
    pub len: u16,
    pub vertical: bool,
}

/// Where everything goes in an area.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Placement {
    pub panes: Vec<(PaneId, Rect)>,
    pub borders: Vec<Border>,
}

impl Layout {
    pub fn new(pane: PaneId) -> Layout {
        Layout::Pane(pane)
    }

    /// The panes, left to right and top to bottom.
    pub fn panes(&self) -> Vec<PaneId> {
        todo!("layout::Layout::panes")
    }

    pub fn contains(&self, pane: PaneId) -> bool {
        let _ = pane;
        todo!("layout::Layout::contains")
    }

    /// Puts `new` on the `side` of `target`, taking `percent` of target's
    /// space (half when `None`); false when `target` isn't here.
    pub fn split(
        &mut self,
        target: PaneId,
        new: PaneId,
        side: Direction,
        percent: Option<u16>,
    ) -> bool {
        let _ = (target, new, side, percent);
        todo!("layout::Layout::split")
    }

    /// Takes a pane out, its space going to its neighbors; a split left
    /// with one child becomes that child. False when it isn't here, or is
    /// the last pane (the tab closes instead).
    pub fn remove(&mut self, pane: PaneId) -> bool {
        let _ = pane;
        todo!("layout::Layout::remove")
    }

    /// Puts `new` where `old` is.
    pub fn replace(&mut self, old: PaneId, new: PaneId) -> bool {
        let _ = (old, new);
        todo!("layout::Layout::replace")
    }

    /// Swaps two panes' places.
    pub fn swap(&mut self, a: PaneId, b: PaneId) -> bool {
        let _ = (a, b);
        todo!("layout::Layout::swap")
    }

    /// Where each pane and each line between them goes in `area`; every
    /// pane at least one cell each way when there is room.
    pub fn place(&self, area: Rect) -> Placement {
        let _ = area;
        todo!("layout::Layout::place")
    }

    /// The pane next to `from` in `dir`: of those touching that edge, the
    /// one beside the most of it (the topmost or leftmost of equals).
    pub fn neighbor(&self, area: Rect, from: PaneId, dir: Direction) -> Option<PaneId> {
        let _ = (area, from, dir);
        todo!("layout::Layout::neighbor")
    }

    /// Grows `pane` by `cells` toward `dir`, taking from its neighbor there;
    /// false when it has no edge that way.
    pub fn resize(&mut self, area: Rect, pane: PaneId, dir: Direction, cells: u16) -> bool {
        let _ = (area, pane, dir, cells);
        todo!("layout::Layout::resize")
    }

    /// Every split's children made equal.
    pub fn equalize(&mut self) {
        todo!("layout::Layout::equalize")
    }
}
