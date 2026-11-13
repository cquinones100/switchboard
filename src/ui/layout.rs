//! The narrowest screen on which nothing stacks: every mission control view
//! that puts a pane beside its list does (`BESIDE_AT`, `SIDE_AT`,
//! `PILOT_SIDE_AT`, `PANE_AT`, `LANDING_OUTPUT_AT`), and every
//! box keeps its rows' extras (`WIDE`), with the sidebar at its default
//! width beside them. `switchboard headless` attaches at this size unless
//! told another, and the walkthrough's recorder never records under it, so
//! a video shows the views as a wide screen does. New here; the thresholds
//! are the views' own, named here so a new one raises it where it is
//! defined.

use crate::sidebar::config;

/// The sidebar's columns at its default width, its rule beside the panes
/// included: what mission control's width is short of the screen's.
const SIDEBAR: usize = config::WIDTH as usize + 1;

const fn most(list: &[usize]) -> usize {
    let mut i = 0;
    let mut out = 0;
    while i < list.len() {
        if list[i] > out {
            out = list[i];
        }
        i += 1;
    }
    out
}

/// The narrowest mission control on which every view lays its panes beside
/// each other.
const VIEWS_BESIDE: usize = most(&[
    crate::mission::reminders::BESIDE_AT,
    crate::mission::slack_board::BESIDE_AT,
    crate::routines::view::BESIDE_AT,
    crate::mission::render::SIDE_AT,
    crate::mission::render::PILOT_SIDE_AT,
    crate::mission::render::LANDING_OUTPUT_AT,
    crate::mission::inbox_board::PANE_AT,
]);

/// Rows enough for a view's list and the pane under or beside it.
const ROWS: u16 = 50;

/// The screen's `(cols, rows)` at which no view's panes stack and no box
/// drops its extras.
pub const SIDE_BY_SIDE: (u16, u16) = {
    let views = VIEWS_BESIDE + SIDEBAR;
    let boxes = crate::autopilot::settings_box::WIDE as usize;
    let cols = if views > boxes { views } else { boxes };
    (cols as u16, ROWS)
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_view_lays_its_panes_beside_each_other_at_that_size() {
        let mission = SIDE_BY_SIDE.0 as usize - SIDEBAR;
        assert_eq!(
            config::Config::default().columns_in(SIDE_BY_SIDE.0) as usize,
            SIDEBAR
        );
        assert!(mission >= crate::mission::reminders::BESIDE_AT);
        assert!(mission >= crate::mission::slack_board::BESIDE_AT);
        assert!(mission >= crate::routines::view::BESIDE_AT);
        assert!(mission >= crate::mission::render::SIDE_AT);
        assert!(mission >= crate::mission::render::PILOT_SIDE_AT);
        assert!(mission >= crate::mission::render::LANDING_OUTPUT_AT);
        assert!(mission >= crate::mission::inbox_board::PANE_AT);
        assert!(crate::mission::slack_board::beside(mission));
        assert!(SIDE_BY_SIDE.0 >= crate::autopilot::settings_box::WIDE);
    }
}
