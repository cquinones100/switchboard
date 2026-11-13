//! A tab's panes as a tree of splits, each split side by side or one above
//! another, with a one-cell line between panes as tmux drew. Sizes are kept
//! as shares of their split, so a window resized keeps its proportions, and
//! a resize by cells moves the shares. Pure data, serialized for the layout
//! saved across restarts.
//!
//! It behaves as tmux's layouts did (tmux's `layout.c`), where Switchboard
//! relied on them:
//!
//! - A split's cells, less one for each line between its children, are shared
//!   out by largest remainder, so the children and lines fill it exactly and
//!   no cell is left over or drawn twice; an odd cell goes to the first child
//!   of equals, as tmux's spreading gave it to the first pane.
//! - Every child gets at least what its own panes need (a cell each way, and
//!   their lines) whenever the area has room for that, taken from the child
//!   with the most to spare. A window too small for it still lays out without
//!   overlapping or panicking: the children that don't fit get no cells.
//! - The tree is kept minimal: a split always has two children or more, and
//!   never holds a split running its own way. Otherwise the same row of panes
//!   could be two shapes, and a resize or a move to a neighbor would act on
//!   one differently from the other; minimal, the shape after any run of
//!   splits and closes is the one tmux would have made.
//! - tmux kept sizes in cells and spread a window's resize over them; shares
//!   keep the proportions instead, and a resize by cells writes the shares
//!   that give exactly those cells in the area it was asked in.

use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};

use super::{Direction, PaneId};

/// The widest a lone pane gets (Settings' `lone_pane_width`), in columns;
/// none when it is off, as it is unless someone chooses a width.
pub fn lone_width() -> Option<u16> {
    let value = crate::settings::cached("lone_pane_width")?;
    value
        .as_u64()
        .and_then(|n| u16::try_from(n).ok())
        .or_else(|| value.as_str()?.parse().ok())
}

/// Where a lone pane goes in `area` when it gets at most `cap` columns:
/// against its left edge (the sidebar), with a column left at its right for
/// the rule at its edge (UX 2.0 board 62), or the whole area when that
/// leaves no room beside it.
pub fn capped(area: Rect, cap: Option<u16>) -> Rect {
    match cap {
        Some(cap) if cap > 0 && area.width > cap => Rect { width: cap, ..area },
        _ => area,
    }
}

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

impl Placement {
    /// Where `pane` went, if it is in the layout.
    pub fn rect(&self, pane: PaneId) -> Option<Rect> {
        self.panes
            .iter()
            .find(|(id, _)| *id == pane)
            .map(|(_, rect)| *rect)
    }

    /// The line between panes at (`col`, `row`), for a drag that moves it,
    /// as tmux's MouseDrag1Border did: the panes before and after it there
    /// (left and right of a line down, above and below one across), and
    /// whether it runs down. Where two lines cross, the one running down.
    pub fn border_at(&self, col: u16, row: u16) -> Option<(PaneId, PaneId, bool)> {
        let on = |b: &Border| {
            if b.vertical {
                col == b.x && row >= b.y && row < b.y.saturating_add(b.len)
            } else {
                row == b.y && col >= b.x && col < b.x.saturating_add(b.len)
            }
        };
        let side_by_side = |vertical: bool| {
            let find = |before: bool| {
                self.panes.iter().find_map(|(id, r)| {
                    let (start, len, along, at) = if vertical {
                        (r.x, r.width, r.y..r.bottom(), row)
                    } else {
                        (r.y, r.height, r.x..r.right(), col)
                    };
                    let line = if vertical { col } else { row };
                    let touches = if before {
                        start.saturating_add(len) == line
                    } else {
                        start == line.saturating_add(1)
                    };
                    (touches && along.contains(&at)).then_some(*id)
                })
            };
            Some((find(true)?, find(false)?, vertical))
        };
        let mut lines: Vec<&Border> = self.borders.iter().filter(|b| on(b)).collect();
        lines.sort_by_key(|b| !b.vertical);
        lines.into_iter().find_map(|b| side_by_side(b.vertical))
    }
}

impl Layout {
    pub fn new(pane: PaneId) -> Layout {
        Layout::Pane(pane)
    }

    /// The panes, left to right and top to bottom.
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    pub fn contains(&self, pane: PaneId) -> bool {
        match self {
            Layout::Pane(id) => *id == pane,
            Layout::Split { children, .. } => children.iter().any(|c| c.contains(pane)),
        }
    }

    /// Puts `new` on the `side` of `target`, taking `percent` of target's
    /// space (half when `None`); false when `target` isn't here.
    ///
    /// When target's split already runs that way, `new` joins it beside
    /// target, so three panes split one after another side by side are one
    /// row of three rather than a row inside a row; otherwise target becomes
    /// a split of the two. The percent is kept between 1 and 99, so neither
    /// pane is left with nothing. False too when `new` is already here (or is
    /// `target`), since a pane can only be in one place.
    pub fn split(
        &mut self,
        target: PaneId,
        new: PaneId,
        side: Direction,
        percent: Option<u16>,
    ) -> bool {
        if target == new || !self.contains(target) || self.contains(new) {
            return false;
        }
        self.tidy();
        let Some(path) = self.path_to(target) else {
            return false;
        };
        let across = matches!(side, Direction::Left | Direction::Right);
        let before = matches!(side, Direction::Left | Direction::Up);
        let part = f32::from(percent.unwrap_or(50).clamp(1, 99)) / 100.0;

        if let Some((&at, up)) = path.split_last()
            && let Some(Layout::Split {
                across: runs,
                children,
                shares,
            }) = self.node_at_mut(up)
            && *runs == across
        {
            // Tidied above, so the shares line up with the children.
            let share = shares.get(at).copied().unwrap_or(0.0);
            if let Some(kept) = shares.get_mut(at) {
                *kept = share * (1.0 - part);
            }
            let to = if before { at } else { at + 1 };
            children.insert(to.min(children.len()), Layout::Pane(new));
            shares.insert(to.min(shares.len()), share * part);
        } else if let Some(node) = self.node_at_mut(&path) {
            let (children, shares) = if before {
                (
                    vec![Layout::Pane(new), Layout::Pane(target)],
                    vec![part, 1.0 - part],
                )
            } else {
                (
                    vec![Layout::Pane(target), Layout::Pane(new)],
                    vec![1.0 - part, part],
                )
            };
            *node = Layout::Split {
                across,
                children,
                shares,
            };
        }
        self.tidy();
        true
    }

    /// Takes a pane out, its space going to its neighbors; a split left
    /// with one child becomes that child. False when it isn't here, or is
    /// the last pane (the tab closes instead).
    ///
    /// The space goes to the child just before it in its split, or, for the
    /// first child, to the one after, as tmux gave it, so closing a pane
    /// split off to the right or below gives its space back to the pane it
    /// came from. A split left running the same way as the one holding it is
    /// merged into it, its children keeping their part of the space, so the
    /// tree stays minimal.
    pub fn remove(&mut self, pane: PaneId) -> bool {
        if !self.contains(pane) || self.count() <= 1 {
            return false;
        }
        self.tidy();
        let Some(path) = self.path_to(pane) else {
            return false;
        };
        let Some((&at, up)) = path.split_last() else {
            return false;
        };
        let Some(Layout::Split {
            children, shares, ..
        }) = self.node_at_mut(up)
        else {
            return false;
        };
        if at >= children.len() {
            return false;
        }
        children.remove(at);
        let share = if at < shares.len() {
            shares.remove(at)
        } else {
            0.0
        };
        if let Some(to) = shares.get_mut(at.saturating_sub(1)) {
            *to += share;
        }
        self.tidy();
        true
    }

    /// Puts `new` where `old` is. False when `old` isn't here, or when `new`
    /// already is somewhere else.
    pub fn replace(&mut self, old: PaneId, new: PaneId) -> bool {
        if old != new && self.contains(new) {
            return false;
        }
        let Some(path) = self.path_to(old) else {
            return false;
        };
        match self.node_at_mut(&path) {
            Some(node) => {
                *node = Layout::Pane(new);
                true
            }
            None => false,
        }
    }

    /// Swaps two panes' places. False when either isn't here.
    pub fn swap(&mut self, a: PaneId, b: PaneId) -> bool {
        let (Some(at_a), Some(at_b)) = (self.path_to(a), self.path_to(b)) else {
            return false;
        };
        if let Some(node) = self.node_at_mut(&at_a) {
            *node = Layout::Pane(b);
        }
        if let Some(node) = self.node_at_mut(&at_b) {
            *node = Layout::Pane(a);
        }
        true
    }

    /// Where each pane and each line between them goes in `area`; every
    /// pane at least one cell each way when there is room.
    ///
    /// The panes come in the order `panes` gives. A line runs the whole
    /// length of the split it divides, so where lines meet, the cell is the
    /// outer split's line's, and every cell of the area is a pane's or a
    /// line's, never both. Lines with no length (in an area with no height
    /// or width) aren't listed.
    pub fn place(&self, area: Rect) -> Placement {
        let mut out = Placement::default();
        self.place_in(clip(area), &mut out);
        out
    }

    /// The pane next to `from` in `dir`: of those touching that edge, the
    /// one beside the most of it (the topmost or leftmost of equals).
    ///
    /// A pane touches the edge when it starts just past the line beyond it.
    /// One beside none of the edge but reaching the line's ends counts after
    /// those beside some of it: a pane a single row tall, level with a line
    /// on the other side, would otherwise have no neighbor there at all.
    pub fn neighbor(&self, area: Rect, from: PaneId, dir: Direction) -> Option<PaneId> {
        let placed = self.place(area);
        let at = placed.rect(from)?;
        let right = u32::from(at.x) + u32::from(at.width);
        let bottom = u32::from(at.y) + u32::from(at.height);
        // Each candidate's start across the edge, and its length along it.
        let beyond = |r: &Rect| -> Option<(u16, u16)> {
            let touches = match dir {
                Direction::Right => u32::from(r.x) == right + 1,
                Direction::Left => u32::from(r.x) + u32::from(r.width) + 1 == u32::from(at.x),
                Direction::Down => u32::from(r.y) == bottom + 1,
                Direction::Up => u32::from(r.y) + u32::from(r.height) + 1 == u32::from(at.y),
            };
            let along = match dir {
                Direction::Left | Direction::Right => (r.y, r.height),
                Direction::Up | Direction::Down => (r.x, r.width),
            };
            (touches && r.width > 0 && r.height > 0).then_some(along)
        };
        let (start, len) = match dir {
            Direction::Left | Direction::Right => (at.y, at.height),
            Direction::Up | Direction::Down => (at.x, at.width),
        };
        let (start, end) = (i64::from(start), i64::from(start) + i64::from(len));
        placed
            .panes
            .iter()
            .filter(|(id, _)| *id != from)
            .filter_map(|(id, r)| {
                let (s, l) = beyond(r)?;
                let (s, e) = (i64::from(s), i64::from(s) + i64::from(l));
                let beside = overlap(start, end, s, e);
                let reaching = overlap(start - 1, end + 1, s, e);
                (reaching > 0).then_some((*id, beside, reaching, s))
            })
            .max_by_key(|&(_, beside, reaching, s)| (beside, reaching, std::cmp::Reverse(s)))
            .map(|(id, ..)| id)
    }

    /// Where moving past the edge toward `dir` from `from` comes round to,
    /// as tmux's select-pane wrapped: a pane on the far side of the tab
    /// level with `from`, the one beside most of it (the topmost or leftmost
    /// of equals); `None` when `from` is on the far side itself.
    pub fn wrap(&self, area: Rect, from: PaneId, dir: Direction) -> Option<PaneId> {
        let placed = self.place(area);
        let at = placed.rect(from)?;
        let live: Vec<&(PaneId, Rect)> = placed
            .panes
            .iter()
            .filter(|(_, r)| r.width > 0 && r.height > 0)
            .collect();
        // How far toward the far side each pane's edge there is: going
        // right, the far side is the left one.
        let edge = |r: &Rect| -> i64 {
            match dir {
                Direction::Right => -i64::from(r.x),
                Direction::Left => i64::from(r.right()),
                Direction::Down => -i64::from(r.y),
                Direction::Up => i64::from(r.bottom()),
            }
        };
        let far = live.iter().map(|(_, r)| edge(r)).max()?;
        let along = |r: &Rect| match dir {
            Direction::Left | Direction::Right => (i64::from(r.y), i64::from(r.bottom())),
            Direction::Up | Direction::Down => (i64::from(r.x), i64::from(r.right())),
        };
        let (start, end) = along(&at);
        live.into_iter()
            .filter(|(id, r)| *id != from && edge(r) == far)
            .filter_map(|(id, r)| {
                let (s, e) = along(r);
                let beside = overlap(start, end, s, e);
                let reaching = overlap(start - 1, end + 1, s, e);
                (reaching > 0).then_some((*id, beside, reaching, s))
            })
            .max_by_key(|&(_, beside, reaching, s)| (beside, reaching, std::cmp::Reverse(s)))
            .map(|(id, ..)| id)
    }

    /// Moves one of `pane`'s edges by `cells` toward `dir`, as tmux's
    /// `resize-pane` did: its edge on that side when it has one, growing it
    /// and taking from its neighbor there, else its edge on the other side,
    /// shrinking it and giving to its neighbor there; false when it has no
    /// edge running that way at all.
    ///
    /// The edge is the line of the nearest split running that way in which
    /// the pane's part has a sibling on that side; the cells come from the
    /// side that shrinks alone, which keeps what its own panes need, so a
    /// resize past that stops there rather than shrinking anything further
    /// on. The split's shares are written from its children's cells in
    /// `area`, so the edge moves by exactly the cells it could; true even
    /// when that side had nothing to give.
    pub fn resize(&mut self, area: Rect, pane: PaneId, dir: Direction, cells: u16) -> bool {
        let Some(path) = self.path_to(pane) else {
            return false;
        };
        let across = matches!(dir, Direction::Left | Direction::Right);
        let forward = matches!(dir, Direction::Right | Direction::Down);

        // The rect of each split down the path to the pane, from the root's.
        let mut rects = Vec::with_capacity(path.len());
        let mut rect = clip(area);
        let mut node: &Layout = self;
        for &i in &path {
            rects.push(rect);
            let Layout::Split {
                across: runs,
                children,
                shares,
            } = node
            else {
                return false;
            };
            let (placed, _) = lay_out(*runs, rect, children, shares);
            let (Some(child), Some(r)) = (children.get(i), placed.get(i)) else {
                return false;
            };
            rect = *r;
            node = child;
        }

        let edge_toward = |forward: bool| {
            (0..path.len()).rev().find_map(|depth| {
                let Layout::Split {
                    across: runs,
                    children,
                    ..
                } = self.node_at(&path[..depth])?
                else {
                    return None;
                };
                let i = path[depth];
                let j = if forward { i + 1 } else { i.checked_sub(1)? };
                (*runs == across && j < children.len()).then_some((depth, i, j))
            })
        };
        // The pane's part `i` and its sibling `j` across the edge: the one
        // that grows, then the one that shrinks.
        let Some((depth, grows, shrinks)) = edge_toward(forward)
            .or_else(|| edge_toward(!forward).map(|(depth, i, j)| (depth, j, i)))
        else {
            return false;
        };
        let Some(&rect) = rects.get(depth) else {
            return false;
        };
        let Some(Layout::Split {
            across: runs,
            children,
            shares,
        }) = self.node_at_mut(&path[..depth])
        else {
            return false;
        };
        let mut sizes = child_sizes(*runs, rect, children, shares);
        let (Some(&size_grows), Some(&size_shrinks), Some(shrinking)) =
            (sizes.get(grows), sizes.get(shrinks), children.get(shrinks))
        else {
            return false;
        };
        let moved = u32::from(cells).min(size_shrinks.saturating_sub(shrinking.min_extent(across)));
        if moved == 0 {
            return true;
        }
        sizes[grows] = size_grows + moved;
        sizes[shrinks] = size_shrinks - moved;
        // A child with no cells would get no share, and never come back; in
        // an area that small there is nothing worth moving.
        let total: u32 = sizes.iter().sum();
        if total == 0 || sizes.contains(&0) {
            return true;
        }
        *shares = sizes
            .iter()
            .map(|&s| (f64::from(s) / f64::from(total)) as f32)
            .collect();
        true
    }

    /// Every split's children made equal.
    pub fn equalize(&mut self) {
        if let Layout::Split {
            children, shares, ..
        } = self
        {
            let n = children.len();
            *shares = vec![1.0 / n as f32; n];
            for child in children {
                child.equalize();
            }
        }
    }

    /// The smallest area, as (width, height), in which every pane gets a
    /// cell each way: below it, `place` gives some panes no cells, so a
    /// split that would need more can be refused.
    pub fn min_size(&self) -> (u16, u16) {
        let fit = |n: u32| u16::try_from(n).unwrap_or(u16::MAX);
        (fit(self.min_extent(true)), fit(self.min_extent(false)))
    }

    /// Makes a layout read from a file sound: a share that is missing, not a
    /// number or not above zero becomes an even one, the shares are scaled
    /// to sum to 1, a split with one child becomes that child, and a split
    /// inside one running the same way is merged into it. Every change here
    /// does this after itself; `place` copes without it.
    pub fn tidy(&mut self) {
        let Layout::Split {
            across,
            children,
            shares,
        } = self
        else {
            return;
        };
        let across = *across;
        let n = children.len();
        let weights: Vec<f64> = (0..n).map(|i| weight(shares, i, n)).collect();
        for child in children.iter_mut() {
            child.tidy();
        }

        let mut kept = Vec::with_capacity(n);
        let mut kept_weights = Vec::with_capacity(n);
        for (child, w) in std::mem::take(children).into_iter().zip(weights) {
            match child {
                Layout::Split {
                    children: inner, ..
                } if inner.is_empty() => {}
                Layout::Split {
                    across: runs,
                    children: inner,
                    shares: inner_shares,
                } if runs == across => {
                    // Each grandchild keeps its part of this child's share,
                    // so merging moves nothing on the screen.
                    let m = inner.len();
                    let inner_weights: Vec<f64> =
                        (0..m).map(|k| weight(&inner_shares, k, m)).collect();
                    let sum: f64 = inner_weights.iter().sum();
                    for (grandchild, iw) in inner.into_iter().zip(inner_weights) {
                        kept.push(grandchild);
                        kept_weights.push(w * iw / sum);
                    }
                }
                other => {
                    kept.push(other);
                    kept_weights.push(w);
                }
            }
        }

        if kept.len() == 1
            && let Some(only) = kept.pop()
        {
            *self = only;
            return;
        }
        let sum: f64 = kept_weights.iter().sum();
        *children = kept;
        *shares = kept_weights
            .iter()
            .map(|w| (w / sum) as f32)
            .collect::<Vec<_>>();
    }

    fn count(&self) -> usize {
        match self {
            Layout::Pane(_) => 1,
            Layout::Split { children, .. } => children.iter().map(Layout::count).sum(),
        }
    }

    fn collect(&self, out: &mut Vec<PaneId>) {
        match self {
            Layout::Pane(id) => out.push(*id),
            Layout::Split { children, .. } => {
                for child in children {
                    child.collect(out);
                }
            }
        }
    }

    /// The children's indexes from here down to `pane`.
    fn path_to(&self, pane: PaneId) -> Option<Vec<usize>> {
        match self {
            Layout::Pane(id) => (*id == pane).then(Vec::new),
            Layout::Split { children, .. } => children.iter().enumerate().find_map(|(i, c)| {
                c.path_to(pane).map(|mut path| {
                    path.insert(0, i);
                    path
                })
            }),
        }
    }

    fn node_at(&self, path: &[usize]) -> Option<&Layout> {
        let mut node = self;
        for &i in path {
            node = match node {
                Layout::Split { children, .. } => children.get(i)?,
                Layout::Pane(_) => return None,
            };
        }
        Some(node)
    }

    fn node_at_mut(&mut self, path: &[usize]) -> Option<&mut Layout> {
        let mut node = self;
        for &i in path {
            node = match node {
                Layout::Split { children, .. } => children.get_mut(i)?,
                Layout::Pane(_) => return None,
            };
        }
        Some(node)
    }

    /// The fewest cells this needs across (`across`) or down to give every
    /// pane in it one, with the lines between them.
    fn min_extent(&self, across: bool) -> u32 {
        match self {
            Layout::Pane(_) => 1,
            Layout::Split {
                across: runs,
                children,
                ..
            } => {
                let mins = children.iter().map(|c| c.min_extent(across));
                if *runs == across {
                    let lines = u32::try_from(children.len().saturating_sub(1)).unwrap_or(u32::MAX);
                    mins.fold(lines, u32::saturating_add)
                } else {
                    mins.max().unwrap_or(0)
                }
            }
        }
    }

    fn place_in(&self, rect: Rect, out: &mut Placement) {
        match self {
            Layout::Pane(id) => out.panes.push((*id, rect)),
            Layout::Split {
                across,
                children,
                shares,
            } => {
                let (rects, borders) = lay_out(*across, rect, children, shares);
                out.borders.extend(borders);
                for (child, r) in children.iter().zip(rects) {
                    child.place_in(r, out);
                }
            }
        }
    }
}

/// The area cut to what a `Rect` can reach, so nothing below adds past
/// `u16::MAX`.
#[cfg(test)]
mod border_tests {
    use super::*;

    #[test]
    fn moving_past_an_edge_comes_round_to_the_far_side() {
        let (a, b, c) = (PaneId(1), PaneId(2), PaneId(3));
        // a | b, with c under b.
        let mut layout = Layout::new(a);
        layout.split(a, b, Direction::Right, None);
        layout.split(b, c, Direction::Down, None);
        let area = Rect::new(0, 0, 41, 21);
        assert_eq!(layout.wrap(area, b, Direction::Right), Some(a));
        assert_eq!(layout.wrap(area, c, Direction::Right), Some(a));
        assert_eq!(layout.wrap(area, a, Direction::Left), Some(b));
        assert_eq!(layout.wrap(area, c, Direction::Down), Some(b));
        assert_eq!(layout.wrap(area, b, Direction::Up), Some(c));
        assert_eq!(
            layout.wrap(area, a, Direction::Up),
            None,
            "a reaches top to bottom, so there is nothing to come round to"
        );
    }

    #[test]
    fn a_line_between_panes_is_found_with_the_panes_either_side() {
        let (a, b, c) = (PaneId(1), PaneId(2), PaneId(3));
        // a | b, with c under b.
        let mut layout = Layout::new(a);
        layout.split(a, b, Direction::Right, None);
        layout.split(b, c, Direction::Down, None);
        let placed = layout.place(Rect::new(0, 0, 41, 21));
        let ra = placed.rect(a).unwrap();
        let rb = placed.rect(b).unwrap();
        let line = ra.right();
        assert_eq!(placed.border_at(line, 2), Some((a, b, true)));
        let under = rb.bottom();
        assert_eq!(placed.border_at(line, under + 2), Some((a, c, true)));
        assert_eq!(placed.border_at(line + 5, under), Some((b, c, false)));
        assert_eq!(placed.border_at(1, 1), None, "inside a pane");
    }
}

fn clip(area: Rect) -> Rect {
    Rect {
        x: area.x,
        y: area.y,
        width: area.width.min(u16::MAX - area.x),
        height: area.height.min(u16::MAX - area.y),
    }
}

/// How many cells two spans have in common, the ends not included.
fn overlap(a_start: i64, a_end: i64, b_start: i64, b_end: i64) -> i64 {
    (a_end.min(b_end) - a_start.max(b_start)).max(0)
}

/// A child's share as a weight to share cells by: a share that is missing,
/// not a number or not above zero (a layout file edited by hand) counts as
/// an even one.
fn weight(shares: &[f32], i: usize, n: usize) -> f64 {
    match shares.get(i) {
        Some(&s) if s.is_finite() && s > 0.0 => f64::from(s),
        _ => 1.0 / n.max(1) as f64,
    }
}

/// Each child's cells along a split that runs `across` (or down) in `rect`:
/// the rect's cells that way, less a line between each two, shared out.
fn child_sizes(across: bool, rect: Rect, children: &[Layout], shares: &[f32]) -> Vec<u32> {
    let n = children.len();
    let extent = u32::from(if across { rect.width } else { rect.height });
    let lines = u32::try_from(n.saturating_sub(1)).unwrap_or(u32::MAX);
    let weights: Vec<f64> = (0..n).map(|i| weight(shares, i, n)).collect();
    let mins: Vec<u32> = children.iter().map(|c| c.min_extent(across)).collect();
    distribute(extent.saturating_sub(lines), &weights, &mins)
}

/// A split's children's rects in `rect`, and the lines between them. They
/// are laid one after another from the start, each line right after its
/// child, so they fill the rect exactly; in a rect too small for the lines
/// the children past its end get no cells and the lines past it aren't
/// drawn.
fn lay_out(
    across: bool,
    rect: Rect,
    children: &[Layout],
    shares: &[f32],
) -> (Vec<Rect>, Vec<Border>) {
    let sizes = child_sizes(across, rect, children, shares);
    let n = sizes.len();
    let (start, extent, cross) = if across {
        (rect.x, rect.width, rect.height)
    } else {
        (rect.y, rect.height, rect.width)
    };
    let end = u32::from(start) + u32::from(extent);
    let mut pos = u32::from(start);
    let mut rects = Vec::with_capacity(n);
    let mut borders = Vec::with_capacity(n.saturating_sub(1));
    for (i, &size) in sizes.iter().enumerate() {
        let len = size.min(end - pos);
        // `clip` keeps `end` within a u16, so these fit.
        let (at, len16) = (pos as u16, len as u16);
        rects.push(if across {
            Rect {
                x: at,
                y: rect.y,
                width: len16,
                height: rect.height,
            }
        } else {
            Rect {
                x: rect.x,
                y: at,
                width: rect.width,
                height: len16,
            }
        });
        pos += len;
        if i + 1 < n && pos < end {
            if cross > 0 {
                let line = pos as u16;
                borders.push(if across {
                    Border {
                        x: line,
                        y: rect.y,
                        len: cross,
                        vertical: true,
                    }
                } else {
                    Border {
                        x: rect.x,
                        y: line,
                        len: cross,
                        vertical: false,
                    }
                });
            }
            pos += 1;
        }
    }
    (rects, borders)
}

/// Shares `total` cells by weight: each its whole part, then the cells left
/// one each to the largest remainders (the first of equals), so they sum to
/// exactly `total`. Then a child below its floor takes cells, one at a time,
/// from the child with the most above its own. The floor is each child's
/// minimum when there is room for them all, else a cell each when there is
/// room for that, else nothing.
fn distribute(total: u32, weights: &[f64], mins: &[u32]) -> Vec<u32> {
    let n = weights.len();
    if n == 0 {
        return Vec::new();
    }
    let sum: f64 = weights.iter().sum();
    let ideal: Vec<f64> = weights
        .iter()
        .map(|w| {
            if sum > 0.0 {
                f64::from(total) * w / sum
            } else {
                f64::from(total) / n as f64
            }
        })
        .collect();
    // `as` saturates, so a wild weight can't wrap.
    let mut sizes: Vec<u32> = ideal
        .iter()
        .map(|x| (x.floor().max(0.0) as u32).min(total))
        .collect();
    let mut given: u64 = sizes.iter().map(|&s| u64::from(s)).sum();
    // Rounding can only fall short, but a float that came out a hair over
    // a whole number must not hand out more than there is.
    while given > u64::from(total) {
        let Some(most) = (0..n).max_by_key(|&i| sizes[i]) else {
            break;
        };
        sizes[most] -= 1;
        given -= 1;
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| ideal[b].fract().total_cmp(&ideal[a].fract()));
    for k in 0..(u64::from(total) - given) {
        sizes[order[(k % n as u64) as usize]] += 1;
    }

    let needed: u64 = mins.iter().map(|&m| u64::from(m)).sum();
    let floors: Vec<u32> = if u64::from(total) >= needed && mins.len() == n {
        mins.to_vec()
    } else if total as usize >= n {
        vec![1; n]
    } else {
        vec![0; n]
    };
    for i in 0..n {
        while sizes[i] < floors[i] {
            let spare = (0..n)
                .filter(|&j| sizes[j] > floors[j])
                .max_by_key(|&j| (sizes[j] - floors[j], std::cmp::Reverse(j)));
            let Some(j) = spare else {
                break;
            };
            sizes[j] -= 1;
            sizes[i] += 1;
        }
    }
    sizes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn p(n: u32) -> PaneId {
        PaneId(n)
    }

    fn rect(x: u16, y: u16, width: u16, height: u16) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn pane(n: u32) -> Layout {
        Layout::Pane(p(n))
    }

    fn split(across: bool, children: Vec<Layout>, shares: Vec<f32>) -> Layout {
        Layout::Split {
            across,
            children,
            shares,
        }
    }

    fn even(across: bool, children: Vec<Layout>) -> Layout {
        let n = children.len();
        split(across, children, vec![1.0 / n as f32; n])
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn shares_of(layout: &Layout) -> Vec<f32> {
        match layout {
            Layout::Split { shares, .. } => shares.clone(),
            Layout::Pane(_) => Vec::new(),
        }
    }

    /// Every cell of `area` is a pane's or a line's, exactly once, and
    /// nothing falls outside it.
    fn assert_covers(layout: &Layout, area: Rect) {
        let placed = layout.place(area);
        let (w, h) = (usize::from(area.width), usize::from(area.height));
        let mut seen = vec![0u8; w * h];
        // `None` for a line.
        let mut mark = |x: u16, y: u16, what: Option<PaneId>| {
            assert!(
                x >= area.x && y >= area.y && x - area.x < area.width && y - area.y < area.height,
                "{what:?} at ({x}, {y}) is outside {area:?} for {layout:?}"
            );
            seen[usize::from(y - area.y) * w + usize::from(x - area.x)] += 1;
        };
        for (id, r) in &placed.panes {
            for y in r.y..r.y + r.height {
                for x in r.x..r.x + r.width {
                    mark(x, y, Some(*id));
                }
            }
        }
        for b in &placed.borders {
            assert!(b.len > 0, "a line with no length is listed: {b:?}");
            for k in 0..b.len {
                if b.vertical {
                    mark(b.x, b.y + k, None);
                } else {
                    mark(b.x + k, b.y, None);
                }
            }
        }
        for (i, n) in seen.iter().enumerate() {
            assert_eq!(
                *n,
                1,
                "cell ({}, {}) is drawn {n} times in {area:?} for {layout:?}",
                i % w,
                i / w
            );
        }
        assert_eq!(
            placed.panes.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            layout.panes()
        );
    }

    /// The tree is minimal and its shares sound.
    fn assert_minimal(layout: &Layout, parent: Option<bool>) {
        if let Layout::Split {
            across,
            children,
            shares,
        } = layout
        {
            assert!(children.len() >= 2, "a split of one: {layout:?}");
            assert_ne!(parent, Some(*across), "a split in its own kind: {layout:?}");
            assert_eq!(shares.len(), children.len(), "{layout:?}");
            assert!(
                shares.iter().all(|s| s.is_finite() && *s > 0.0),
                "{shares:?}"
            );
            let sum: f32 = shares.iter().sum();
            assert!((sum - 1.0).abs() < 1e-3, "shares sum to {sum}: {shares:?}");
            for child in children {
                assert_minimal(child, Some(*across));
            }
        }
    }

    // ── Placement ───────────────────────────────────────────────────────────

    #[test]
    fn a_single_pane_fills_the_area() {
        let layout = Layout::new(p(1));
        let placed = layout.place(rect(3, 2, 80, 24));
        assert_eq!(placed.panes, vec![(p(1), rect(3, 2, 80, 24))]);
        assert!(placed.borders.is_empty());
        assert_eq!(layout.panes(), vec![p(1)]);
    }

    #[test]
    fn three_across_81_columns_share_79_with_the_odd_one_first() {
        let layout = even(true, vec![pane(1), pane(2), pane(3)]);
        let placed = layout.place(rect(0, 0, 81, 10));
        assert_eq!(
            placed.panes,
            vec![
                (p(1), rect(0, 0, 27, 10)),
                (p(2), rect(28, 0, 26, 10)),
                (p(3), rect(55, 0, 26, 10)),
            ]
        );
        assert_eq!(
            placed.borders,
            vec![
                Border {
                    x: 27,
                    y: 0,
                    len: 10,
                    vertical: true
                },
                Border {
                    x: 54,
                    y: 0,
                    len: 10,
                    vertical: true
                },
            ]
        );
        assert_covers(&layout, rect(0, 0, 81, 10));
    }

    #[test]
    fn largest_remainders_get_the_cells_left() {
        // 0.5, 0.3, 0.2 of 10 cells less two lines: 4, 2.4, 1.6, so the
        // last child's .6 takes the one cell left over.
        let layout = split(true, vec![pane(1), pane(2), pane(3)], vec![0.5, 0.3, 0.2]);
        let placed = layout.place(rect(0, 0, 10, 1));
        let widths: Vec<u16> = placed.panes.iter().map(|(_, r)| r.width).collect();
        assert_eq!(widths, vec![4, 2, 2]);
    }

    #[test]
    fn stacked_panes_have_lines_across_them() {
        let layout = split(false, vec![pane(1), pane(2)], vec![0.25, 0.75]);
        let placed = layout.place(rect(5, 1, 30, 21));
        assert_eq!(
            placed.panes,
            vec![(p(1), rect(5, 1, 30, 5)), (p(2), rect(5, 7, 30, 15))]
        );
        assert_eq!(
            placed.borders,
            vec![Border {
                x: 5,
                y: 6,
                len: 30,
                vertical: false
            }]
        );
        assert_covers(&layout, rect(5, 1, 30, 21));
    }

    #[test]
    fn nested_splits_place_inside_their_part() {
        let layout = even(true, vec![pane(1), even(false, vec![pane(2), pane(3)])]);
        let placed = layout.place(rect(0, 0, 80, 24));
        assert_eq!(
            placed.panes,
            vec![
                (p(1), rect(0, 0, 40, 24)),
                (p(2), rect(41, 0, 39, 12)),
                (p(3), rect(41, 13, 39, 11)),
            ]
        );
        assert_eq!(
            placed.borders,
            vec![
                Border {
                    x: 40,
                    y: 0,
                    len: 24,
                    vertical: true
                },
                Border {
                    x: 41,
                    y: 12,
                    len: 39,
                    vertical: false
                },
            ]
        );
        assert_covers(&layout, rect(0, 0, 80, 24));
    }

    #[test]
    fn a_small_share_still_gets_what_its_panes_need() {
        // The right part's three panes across need 5 columns; its share
        // alone would give it 1 of the 19.
        let right = even(
            false,
            vec![even(true, vec![pane(2), pane(3), pane(4)]), pane(5)],
        );
        let layout = split(true, vec![pane(1), right], vec![0.95, 0.05]);
        assert_eq!(layout.min_size(), (7, 3));
        let placed = layout.place(rect(0, 0, 20, 6));
        assert_eq!(placed.rect(p(1)), Some(rect(0, 0, 14, 6)));
        for (_, r) in &placed.panes {
            assert!(r.width >= 1 && r.height >= 1, "{placed:?}");
        }
        assert_covers(&layout, rect(0, 0, 20, 6));
    }

    #[test]
    fn a_tiny_area_degrades_without_overlap() {
        let layout = even(true, vec![pane(1), pane(2), pane(3)]);
        // Two columns: room for the two lines and nothing else.
        let placed = layout.place(rect(0, 0, 2, 3));
        assert!(placed.panes.iter().all(|(_, r)| r.width == 0));
        assert_eq!(placed.borders.len(), 2);
        assert_covers(&layout, rect(0, 0, 2, 3));
        // One column: the first line, and the rest past the end.
        assert_covers(&layout, rect(0, 0, 1, 3));
        // Room for the lines and two panes of three.
        let placed = layout.place(rect(0, 0, 4, 3));
        let widths: Vec<u16> = placed.panes.iter().map(|(_, r)| r.width).collect();
        assert_eq!(widths.iter().sum::<u16>(), 2);
        assert_covers(&layout, rect(0, 0, 4, 3));
        for size in [(0, 0), (0, 5), (5, 0), (1, 1)] {
            let placed = layout.place(rect(0, 0, size.0, size.1));
            assert_eq!(placed.panes.len(), 3);
            assert_covers(&layout, rect(0, 0, size.0, size.1));
        }
    }

    #[test]
    fn an_area_at_the_edge_of_the_coordinates_is_cut_to_fit() {
        let layout = even(true, vec![pane(1), pane(2)]);
        let placed = layout.place(rect(u16::MAX - 5, u16::MAX - 2, 100, 100));
        for (_, r) in &placed.panes {
            assert!(u32::from(r.x) + u32::from(r.width) <= u32::from(u16::MAX));
            assert!(u32::from(r.y) + u32::from(r.height) <= u32::from(u16::MAX));
        }
    }

    #[test]
    fn a_layout_edited_by_hand_still_places_and_is_tidied_by_a_change() {
        // One child too few shares, a negative one, a split of one, and a
        // split inside one running the same way.
        let json = r#"{"Split":{"across":true,"children":[
            {"Pane":1},
            {"Split":{"across":true,"children":[{"Pane":2},{"Pane":3}],"shares":[-1.0]}},
            {"Split":{"across":false,"children":[{"Pane":4}],"shares":[1.0]}}
        ],"shares":[0.5,0.5]}}"#;
        let mut layout: Layout = serde_json::from_str(json).unwrap();
        assert_eq!(layout.panes(), vec![p(1), p(2), p(3), p(4)]);
        for (w, h) in [(80, 24), (7, 1), (3, 2), (0, 0)] {
            assert_covers(&layout, rect(0, 0, w, h));
        }
        assert!(layout.split(p(4), p(5), Direction::Down, None));
        assert_minimal(&layout, None);
        assert_eq!(layout.panes(), vec![p(1), p(2), p(3), p(4), p(5)]);
        assert_covers(&layout, rect(0, 0, 80, 24));

        let mut empty = split(true, Vec::new(), Vec::new());
        assert!(empty.place(rect(0, 0, 10, 10)).panes.is_empty());
        assert!(!empty.remove(p(1)));
        assert!(!empty.split(p(1), p(2), Direction::Right, None));
        empty.equalize();
        empty.tidy();
        assert_eq!(empty.min_size(), (0, 0));
    }

    // ── Splitting and removing ──────────────────────────────────────────────

    #[test]
    fn splitting_a_lone_pane_makes_a_split_of_two() {
        let mut layout = Layout::new(p(1));
        assert!(layout.split(p(1), p(2), Direction::Right, None));
        assert_eq!(layout, even(true, vec![pane(1), pane(2)]));

        let mut layout = Layout::new(p(1));
        assert!(layout.split(p(1), p(2), Direction::Up, Some(30)));
        assert_eq!(layout.panes(), vec![p(2), p(1)]);
        let Layout::Split { across, shares, .. } = &layout else {
            panic!("not a split: {layout:?}");
        };
        assert!(!across);
        assert!(close(shares[0], 0.3) && close(shares[1], 0.7), "{shares:?}");
    }

    #[test]
    fn splitting_the_way_the_split_runs_joins_it() {
        let mut layout = Layout::new(p(1));
        assert!(layout.split(p(1), p(2), Direction::Right, None));
        assert!(layout.split(p(2), p(3), Direction::Right, Some(30)));
        assert_eq!(layout.panes(), vec![p(1), p(2), p(3)]);
        let shares = shares_of(&layout);
        assert_eq!(shares.len(), 3, "{layout:?}");
        assert!(close(shares[0], 0.5) && close(shares[1], 0.35) && close(shares[2], 0.15));

        assert!(layout.split(p(3), p(4), Direction::Left, None));
        assert_eq!(layout.panes(), vec![p(1), p(2), p(4), p(3)]);
        let shares = shares_of(&layout);
        assert!(
            close(shares[2], 0.075) && close(shares[3], 0.075),
            "{shares:?}"
        );
        assert_minimal(&layout, None);
    }

    #[test]
    fn splitting_across_the_split_makes_a_split_inside_it() {
        let mut layout = even(true, vec![pane(1), pane(2)]);
        assert!(layout.split(p(1), p(3), Direction::Down, None));
        assert_eq!(
            layout,
            even(true, vec![even(false, vec![pane(1), pane(3)]), pane(2)])
        );
        assert!(layout.split(p(3), p(4), Direction::Up, Some(25)));
        assert_eq!(layout.panes(), vec![p(1), p(4), p(3), p(2)]);
        let Layout::Split { children, .. } = &layout else {
            panic!("not a split");
        };
        let inner = shares_of(&children[0]);
        assert!(close(inner[0], 0.5) && close(inner[1], 0.125) && close(inner[2], 0.375));
        assert_minimal(&layout, None);
    }

    #[test]
    fn a_split_refuses_what_it_cant_do() {
        let mut layout = even(true, vec![pane(1), pane(2)]);
        let before = layout.clone();
        assert!(!layout.split(p(9), p(3), Direction::Right, None));
        assert!(!layout.split(p(1), p(2), Direction::Right, None));
        assert!(!layout.split(p(1), p(1), Direction::Right, None));
        assert_eq!(layout, before);
    }

    #[test]
    fn a_split_percent_is_kept_between_1_and_99() {
        let mut layout = Layout::new(p(1));
        assert!(layout.split(p(1), p(2), Direction::Right, Some(0)));
        assert!(close(shares_of(&layout)[1], 0.01));
        let mut layout = Layout::new(p(1));
        assert!(layout.split(p(1), p(2), Direction::Down, Some(150)));
        assert!(close(shares_of(&layout)[1], 0.99));
        // Even a pane with one percent gets a cell.
        let placed = layout.place(rect(0, 0, 10, 10));
        assert!(placed.rect(p(1)).unwrap().height >= 1);
    }

    #[test]
    fn a_removed_panes_share_goes_to_the_one_before_else_after() {
        let mut layout = split(true, vec![pane(1), pane(2), pane(3)], vec![0.2, 0.3, 0.5]);
        assert!(layout.remove(p(2)));
        let shares = shares_of(&layout);
        assert_eq!(layout.panes(), vec![p(1), p(3)]);
        assert!(close(shares[0], 0.5) && close(shares[1], 0.5), "{shares:?}");

        let mut layout = split(true, vec![pane(1), pane(2), pane(3)], vec![0.2, 0.3, 0.5]);
        assert!(layout.remove(p(1)));
        let shares = shares_of(&layout);
        assert!(close(shares[0], 0.5) && close(shares[1], 0.5), "{shares:?}");

        let mut layout = split(true, vec![pane(1), pane(2), pane(3)], vec![0.2, 0.3, 0.5]);
        assert!(layout.remove(p(3)));
        let shares = shares_of(&layout);
        assert!(close(shares[0], 0.2) && close(shares[1], 0.8), "{shares:?}");
    }

    #[test]
    fn removing_collapses_a_split_of_one() {
        let mut layout = Layout::new(p(1));
        assert!(layout.split(p(1), p(2), Direction::Right, None));
        assert!(layout.split(p(1), p(3), Direction::Down, None));
        assert!(layout.remove(p(3)));
        assert_eq!(layout, even(true, vec![pane(1), pane(2)]));
        assert!(layout.remove(p(1)));
        assert_eq!(layout, Layout::new(p(2)));
        assert!(!layout.remove(p(2)), "the last pane stays");
        assert!(!layout.remove(p(7)));
        assert_eq!(layout, Layout::new(p(2)));
    }

    #[test]
    fn removing_merges_a_split_into_one_running_the_same_way() {
        // [1 | (2 / [3 | 4])]: taking 2 out leaves [3 | 4] alone in the
        // column, which then belongs in the row with 1.
        let mut layout = split(
            true,
            vec![
                pane(1),
                split(
                    false,
                    vec![
                        pane(2),
                        split(true, vec![pane(3), pane(4)], vec![0.25, 0.75]),
                    ],
                    vec![0.5, 0.5],
                ),
            ],
            vec![0.6, 0.4],
        );
        let before = layout.place(rect(0, 0, 81, 20));
        assert!(layout.remove(p(2)));
        assert_minimal(&layout, None);
        assert_eq!(layout.panes(), vec![p(1), p(3), p(4)]);
        let shares = shares_of(&layout);
        assert!(close(shares[0], 0.6) && close(shares[1], 0.1) && close(shares[2], 0.3));
        // The panes that stayed keep their columns, but for the cell the
        // gone line gives back and a rounding that can fall one cell over.
        let after = layout.place(rect(0, 0, 81, 20));
        for id in [p(1), p(3), p(4)] {
            let (b, a) = (before.rect(id).unwrap(), after.rect(id).unwrap());
            assert!(
                b.x.abs_diff(a.x) <= 1 && b.width.abs_diff(a.width) <= 1,
                "{id} was {b:?}, is {a:?}"
            );
        }
    }

    #[test]
    fn a_run_of_splits_and_closes_stays_minimal() {
        let mut layout = Layout::new(p(1));
        let steps: [(u32, u32, Direction); 6] = [
            (1, 2, Direction::Right),
            (2, 3, Direction::Down),
            (3, 4, Direction::Right),
            (4, 5, Direction::Down),
            (1, 6, Direction::Left),
            (5, 7, Direction::Up),
        ];
        for (target, new, side) in steps {
            assert!(layout.split(p(target), p(new), side, None));
            assert_minimal(&layout, None);
            assert_covers(&layout, rect(0, 0, 120, 40));
        }
        for gone in [3, 6, 1, 7, 2, 5] {
            assert!(layout.remove(p(gone)));
            assert_minimal(&layout, None);
            assert_covers(&layout, rect(0, 0, 120, 40));
        }
        assert_eq!(layout, Layout::new(p(4)));
    }

    // ── Replacing, swapping, listing ────────────────────────────────────────

    #[test]
    fn panes_are_listed_depth_first() {
        let layout = even(
            true,
            vec![
                even(false, vec![pane(5), pane(2)]),
                pane(9),
                even(false, vec![pane(1), even(true, vec![pane(7), pane(3)])]),
            ],
        );
        assert_eq!(layout.panes(), vec![p(5), p(2), p(9), p(1), p(7), p(3)]);
        assert!(layout.contains(p(7)));
        assert!(!layout.contains(p(4)));
    }

    #[test]
    fn replace_puts_a_pane_in_anothers_place() {
        let mut layout = even(true, vec![pane(1), even(false, vec![pane(2), pane(3)])]);
        assert!(layout.replace(p(2), p(8)));
        assert_eq!(layout.panes(), vec![p(1), p(8), p(3)]);
        assert!(!layout.replace(p(2), p(9)), "2 is gone");
        assert!(!layout.replace(p(1), p(3)), "3 is already here");
        assert!(layout.replace(p(1), p(1)));
        assert_eq!(layout.panes(), vec![p(1), p(8), p(3)]);
    }

    #[test]
    fn swap_trades_two_panes_places() {
        let mut layout = even(true, vec![pane(1), even(false, vec![pane(2), pane(3)])]);
        let before = layout.place(rect(0, 0, 80, 24));
        assert!(layout.swap(p(1), p(3)));
        assert_eq!(layout.panes(), vec![p(3), p(2), p(1)]);
        let after = layout.place(rect(0, 0, 80, 24));
        assert_eq!(before.rect(p(1)), after.rect(p(3)));
        assert_eq!(before.rect(p(3)), after.rect(p(1)));
        assert!(!layout.swap(p(1), p(9)));
        assert!(layout.swap(p(2), p(2)));
        assert_eq!(layout.panes(), vec![p(3), p(2), p(1)]);
    }

    // ── Neighbors ───────────────────────────────────────────────────────────

    /// ```text
    /// 1 | 3
    /// --+---
    /// 2 | 4
    ///   |---
    ///   | 5
    /// ```
    fn grid() -> Layout {
        even(
            true,
            vec![
                even(false, vec![pane(1), pane(2)]),
                even(false, vec![pane(3), pane(4), pane(5)]),
            ],
        )
    }

    #[test]
    fn neighbors_in_every_direction() {
        let layout = grid();
        let area = rect(0, 0, 81, 31);
        let n = |from: u32, dir: Direction| layout.neighbor(area, p(from), dir);
        // 1 is rows 0-14; 3 is rows 0-9, 4 rows 11-20.
        assert_eq!(n(1, Direction::Right), Some(p(3)));
        assert_eq!(n(1, Direction::Down), Some(p(2)));
        assert_eq!(n(1, Direction::Left), None);
        assert_eq!(n(1, Direction::Up), None);
        // 2 is rows 16-30: beside 4 for rows 16-20 and 5 for 22-30.
        assert_eq!(n(2, Direction::Right), Some(p(5)));
        assert_eq!(n(2, Direction::Up), Some(p(1)));
        assert_eq!(n(3, Direction::Left), Some(p(1)));
        // 4 is rows 11-20: beside 1 for 11-14 and 2 for 16-20.
        assert_eq!(n(4, Direction::Left), Some(p(2)));
        assert_eq!(n(4, Direction::Up), Some(p(3)));
        assert_eq!(n(4, Direction::Down), Some(p(5)));
        assert_eq!(n(5, Direction::Down), None);
        assert_eq!(n(5, Direction::Right), None);
        assert_eq!(n(9, Direction::Right), None, "no such pane");
    }

    #[test]
    fn equal_neighbors_go_to_the_topmost_or_leftmost() {
        let layout = even(true, vec![pane(1), even(false, vec![pane(2), pane(3)])]);
        // 1 is rows 0-8; 2 rows 0-3 and 3 rows 5-8: four rows each.
        let area = rect(0, 0, 20, 9);
        assert_eq!(layout.neighbor(area, p(1), Direction::Right), Some(p(2)));

        let layout = even(false, vec![even(true, vec![pane(1), pane(2)]), pane(3)]);
        let area = rect(0, 0, 9, 20);
        assert_eq!(layout.neighbor(area, p(3), Direction::Up), Some(p(1)));
    }

    #[test]
    fn a_pane_level_with_a_line_still_has_a_neighbor() {
        // Left: rows 0-3 for 1, the line at 4, row 5 for 2, the line at 6,
        // rows 7-9 for 3. Right: rows 0-4 for 4, the line at 5, rows 6-9
        // for 5. Pane 2 sits level with the line between 4 and 5.
        let left = split(
            false,
            vec![pane(1), pane(2), pane(3)],
            vec![0.5, 0.125, 0.375],
        );
        let layout = even(true, vec![left, even(false, vec![pane(4), pane(5)])]);
        let area = rect(0, 0, 21, 10);
        let placed = layout.place(area);
        assert_eq!(placed.rect(p(2)), Some(rect(0, 5, 10, 1)));
        assert_eq!(placed.rect(p(4)), Some(rect(11, 0, 10, 5)));
        assert_eq!(layout.neighbor(area, p(2), Direction::Right), Some(p(4)));
    }

    // ── Resizing ────────────────────────────────────────────────────────────

    fn widths(layout: &Layout, area: Rect) -> Vec<u16> {
        layout
            .place(area)
            .panes
            .iter()
            .map(|(_, r)| r.width)
            .collect()
    }

    #[test]
    fn resize_moves_the_edge_by_exactly_the_cells() {
        let area = rect(0, 0, 81, 24);
        let mut layout = even(true, vec![pane(1), pane(2)]);
        assert_eq!(widths(&layout, area), vec![40, 40]);
        assert!(layout.resize(area, p(1), Direction::Right, 5));
        assert_eq!(widths(&layout, area), vec![45, 35]);
        assert!(layout.resize(area, p(2), Direction::Left, 7));
        assert_eq!(widths(&layout, area), vec![38, 42]);
        assert!(layout.resize(area, p(2), Direction::Left, 1));
        assert_eq!(widths(&layout, area), vec![37, 43]);
        let sum: f32 = shares_of(&layout).iter().sum();
        assert!(close(sum, 1.0));
        // A pane with no edge running that way, or not in the layout.
        assert!(!layout.resize(area, p(1), Direction::Up, 3));
        assert!(!layout.resize(area, p(9), Direction::Right, 3));
        assert_eq!(widths(&layout, area), vec![37, 43]);
        let mut alone = pane(1);
        assert!(!alone.resize(area, p(1), Direction::Left, 3));
    }

    #[test]
    fn resize_with_no_edge_that_way_moves_the_other_edge() {
        let area = rect(0, 0, 81, 24);
        let mut layout = even(true, vec![pane(1), pane(2)]);
        // The left pane's Left moves the line left, shrinking it.
        assert!(layout.resize(area, p(1), Direction::Left, 5));
        assert_eq!(widths(&layout, area), vec![35, 45]);
        assert!(layout.resize(area, p(1), Direction::Right, 5));
        assert_eq!(widths(&layout, area), vec![40, 40]);
        // The right pane's Right moves the line right, shrinking it.
        assert!(layout.resize(area, p(2), Direction::Right, 3));
        assert_eq!(widths(&layout, area), vec![43, 37]);

        let heights = |layout: &Layout| -> Vec<u16> {
            layout
                .place(area)
                .panes
                .iter()
                .map(|(_, r)| r.height)
                .collect()
        };
        let mut layout = even(false, vec![pane(1), pane(2)]);
        assert_eq!(heights(&layout), vec![12, 11]);
        assert!(layout.resize(area, p(1), Direction::Up, 4));
        assert_eq!(heights(&layout), vec![8, 15]);
        assert!(layout.resize(area, p(1), Direction::Down, 2));
        assert_eq!(heights(&layout), vec![10, 13]);
        assert!(layout.resize(area, p(2), Direction::Down, 3));
        assert_eq!(heights(&layout), vec![13, 10]);

        // In a row of three, the middle one's Left grows it; the last one's
        // Right shrinks it.
        let mut layout = even(true, vec![pane(1), pane(2), pane(3)]);
        let before = widths(&layout, area);
        assert!(layout.resize(area, p(2), Direction::Left, 4));
        assert_eq!(
            widths(&layout, area),
            vec![before[0] - 4, before[1] + 4, before[2]]
        );
        assert!(layout.resize(area, p(3), Direction::Right, 6));
        assert_eq!(
            widths(&layout, area),
            vec![before[0] - 4, before[1] + 10, before[2] - 6]
        );
        assert_covers(&layout, area);
    }

    #[test]
    fn resize_stops_at_a_single_cell() {
        let area = rect(0, 0, 81, 24);
        let mut layout = even(true, vec![pane(1), pane(2), pane(3)]);
        assert!(layout.resize(area, p(1), Direction::Right, 500));
        // Only 2 gives: it keeps one cell, and 3 is left as it was.
        assert_eq!(widths(&layout, area), vec![52, 1, 26]);
        assert!(
            layout.resize(area, p(1), Direction::Right, 1),
            "an edge, if a stuck one"
        );
        assert_eq!(widths(&layout, area), vec![52, 1, 26]);
        assert!(layout.resize(area, p(1), Direction::Right, 0));
        assert_eq!(widths(&layout, area), vec![52, 1, 26]);
    }

    #[test]
    fn resize_keeps_what_the_siblings_panes_need() {
        let area = rect(0, 0, 41, 10);
        let mut layout = even(
            true,
            vec![
                pane(1),
                even(
                    false,
                    vec![even(true, vec![pane(2), pane(3), pane(4)]), pane(5)],
                ),
            ],
        );
        assert!(layout.resize(area, p(1), Direction::Right, 100));
        let placed = layout.place(area);
        // Three panes and two lines need five columns.
        assert_eq!(placed.rect(p(1)).unwrap().width, 35);
        assert_eq!(placed.rect(p(5)).unwrap().width, 5);
        assert_covers(&layout, area);
    }

    #[test]
    fn resize_finds_the_edge_up_the_tree() {
        // 2 is in a column on the right: its left edge is the row's line.
        let area = rect(0, 0, 81, 24);
        let mut layout = even(true, vec![pane(1), even(false, vec![pane(2), pane(3)])]);
        assert!(layout.resize(area, p(2), Direction::Left, 10));
        let placed = layout.place(area);
        assert_eq!(placed.rect(p(1)).unwrap().width, 30);
        assert_eq!(placed.rect(p(2)).unwrap().width, 50);
        assert_eq!(placed.rect(p(3)).unwrap().width, 50);
        // Down from 2 is the column's own line; up from 2 there is none, so
        // Up moves that same line up.
        let before = placed.rect(p(2)).unwrap().height;
        assert!(layout.resize(area, p(2), Direction::Down, 4));
        assert_eq!(layout.place(area).rect(p(2)).unwrap().height, before + 4);
        assert!(layout.resize(area, p(2), Direction::Up, 1));
        assert_eq!(layout.place(area).rect(p(2)).unwrap().height, before + 3);
        assert!(layout.resize(area, p(3), Direction::Up, 1));
        assert_eq!(layout.place(area).rect(p(2)).unwrap().height, before + 2);
        assert_minimal(&layout, None);
    }

    #[test]
    fn resize_in_an_area_too_small_changes_nothing() {
        let mut layout = even(true, vec![pane(1), pane(2), pane(3)]);
        let before = layout.clone();
        assert!(layout.resize(rect(0, 0, 3, 3), p(1), Direction::Right, 1));
        assert_eq!(layout, before);
    }

    #[test]
    fn equalize_evens_every_split() {
        let mut layout = split(
            true,
            vec![
                pane(1),
                split(false, vec![pane(2), pane(3), pane(4)], vec![0.7, 0.2, 0.1]),
            ],
            vec![0.9, 0.1],
        );
        layout.equalize();
        assert_eq!(
            layout,
            even(
                true,
                vec![pane(1), even(false, vec![pane(2), pane(3), pane(4)])]
            )
        );
        let placed = layout.place(rect(0, 0, 81, 32));
        assert_eq!(placed.rect(p(1)).unwrap().width, 40);
        let heights: Vec<u16> = [p(2), p(3), p(4)]
            .iter()
            .map(|id| placed.rect(*id).unwrap().height)
            .collect();
        assert_eq!(heights, vec![10, 10, 10]);
    }

    // ── Saving ──────────────────────────────────────────────────────────────

    #[test]
    fn a_layout_survives_json() {
        let mut layout = Layout::new(p(1));
        layout.split(p(1), p(2), Direction::Right, Some(30));
        layout.split(p(2), p(3), Direction::Down, None);
        layout.split(p(1), p(4), Direction::Up, Some(40));
        layout.resize(rect(0, 0, 100, 30), p(3), Direction::Up, 3);
        let json = serde_json::to_string(&layout).unwrap();
        let back: Layout = serde_json::from_str(&json).unwrap();
        assert_eq!(back, layout);
        assert_eq!(
            back.place(rect(0, 0, 100, 30)),
            layout.place(rect(0, 0, 100, 30))
        );
    }

    // ── Any run of changes ──────────────────────────────────────────────────

    /// splitmix64: enough randomness to wander through layouts, and the
    /// same wander every run.
    struct Rng(u64);

    impl Rng {
        fn next_u64(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }

        fn below(&mut self, n: u64) -> u64 {
            if n == 0 { 0 } else { self.next_u64() % n }
        }

        fn pick(&mut self, panes: &[PaneId]) -> PaneId {
            panes[self.below(panes.len() as u64) as usize]
        }

        fn dir(&mut self) -> Direction {
            [
                Direction::Left,
                Direction::Right,
                Direction::Up,
                Direction::Down,
            ][self.below(4) as usize]
        }

        fn area(&mut self) -> Rect {
            rect(
                self.below(8) as u16,
                self.below(5) as u16,
                self.below(110) as u16,
                self.below(45) as u16,
            )
        }
    }

    fn assert_sound(layout: &Layout, expected: &HashSet<PaneId>, rng: &mut Rng) {
        assert_minimal(layout, None);
        let panes = layout.panes();
        assert_eq!(panes.len(), expected.len(), "{layout:?}");
        assert_eq!(panes.iter().copied().collect::<HashSet<_>>(), *expected);

        let (min_w, min_h) = layout.min_size();
        let roomy = rect(2, 1, min_w.max(100), min_h.max(40));
        for area in [rng.area(), rng.area(), roomy] {
            assert_covers(layout, area);
            let placed = layout.place(area);
            if area.width >= min_w && area.height >= min_h {
                for (id, r) in &placed.panes {
                    assert!(r.width >= 1 && r.height >= 1, "{id} gets {r:?} in {area:?}");
                }
            }
            // A neighbor is just past the line beyond the edge, and beside
            // it or at its ends.
            let from = rng.pick(&panes);
            let dir = rng.dir();
            if let Some(to) = layout.neighbor(area, from, dir) {
                let (a, b) = (placed.rect(from).unwrap(), placed.rect(to).unwrap());
                let (a_end_x, a_end_y) = (a.x + a.width, a.y + a.height);
                let (b_end_x, b_end_y) = (b.x + b.width, b.y + b.height);
                match dir {
                    Direction::Right => assert_eq!(b.x, a_end_x + 1),
                    Direction::Left => assert_eq!(b_end_x + 1, a.x),
                    Direction::Down => assert_eq!(b.y, a_end_y + 1),
                    Direction::Up => assert_eq!(b_end_y + 1, a.y),
                }
                match dir {
                    Direction::Left | Direction::Right => {
                        assert!(b.y <= a_end_y && b_end_y >= a.y)
                    }
                    Direction::Up | Direction::Down => {
                        assert!(b.x <= a_end_x && b_end_x >= a.x)
                    }
                }
            } else if area.width >= min_w && area.height >= min_h {
                // No neighbor only at the area's edge.
                let a = placed.rect(from).unwrap();
                let at_edge = match dir {
                    Direction::Right => a.x + a.width == area.x + area.width,
                    Direction::Left => a.x == area.x,
                    Direction::Down => a.y + a.height == area.y + area.height,
                    Direction::Up => a.y == area.y,
                };
                assert!(
                    at_edge,
                    "{from} has no neighbor {dir:?} in {area:?}: {layout:?}"
                );
            }
        }
    }

    #[test]
    fn any_run_of_changes_keeps_the_tree_minimal_and_the_area_covered() {
        for seed in 0..120 {
            let mut rng = Rng(seed);
            let mut layout = Layout::new(p(0));
            let mut expected: HashSet<PaneId> = [p(0)].into();
            let mut next_id = 1;
            for _ in 0..40 {
                let panes = layout.panes();
                match rng.below(10) {
                    0..=3 => {
                        let target = rng.pick(&panes);
                        let percent = match rng.below(3) {
                            0 => None,
                            _ => Some(rng.below(120) as u16),
                        };
                        assert!(layout.split(target, p(next_id), rng.dir(), percent));
                        expected.insert(p(next_id));
                        next_id += 1;
                    }
                    4 | 5 => {
                        let gone = rng.pick(&panes);
                        let removed = layout.remove(gone);
                        assert_eq!(removed, panes.len() > 1);
                        if removed {
                            expected.remove(&gone);
                        }
                    }
                    6 | 7 => {
                        let area = rng.area();
                        let pane = rng.pick(&panes);
                        let cells = rng.below(30) as u16;
                        layout.resize(area, pane, rng.dir(), cells);
                    }
                    8 => {
                        let (a, b) = (rng.pick(&panes), rng.pick(&panes));
                        assert!(layout.swap(a, b));
                    }
                    _ => {
                        if rng.below(2) == 0 {
                            layout.equalize();
                        } else {
                            let old = rng.pick(&panes);
                            assert!(layout.replace(old, p(next_id)));
                            expected.remove(&old);
                            expected.insert(p(next_id));
                            next_id += 1;
                        }
                    }
                }
                assert_sound(&layout, &expected, &mut rng);
            }
            let json = serde_json::to_string(&layout).unwrap();
            let back: Layout = serde_json::from_str(&json).unwrap();
            assert_eq!(back.panes(), layout.panes());
            assert_eq!(
                back.place(rect(0, 0, 97, 41)),
                layout.place(rect(0, 0, 97, 41))
            );
        }
    }

    #[test]
    fn a_lone_pane_is_capped_against_the_sidebar_with_room_for_its_rule() {
        let area = rect(35, 1, 330, 88);
        assert_eq!(capped(area, Some(165)), rect(35, 1, 165, 88));
        assert_eq!(capped(area, None), area, "off fills the width");
        assert_eq!(capped(rect(0, 0, 165, 10), Some(165)), rect(0, 0, 165, 10));
        assert_eq!(
            capped(rect(0, 0, 166, 10), Some(165)),
            rect(0, 0, 165, 10),
            "a column at its right for the rule"
        );
    }
}
