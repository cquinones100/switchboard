//! What the phone's drawer shows of a tab (assets/phone/app.js's groups and
//! rows), from the sidebar's own model: its stage, its badge's mark and
//! whether it turns, and the sidebar's words for what it is doing. The
//! drawer only draws these, so it never files or marks a tab apart from the
//! sidebar. `phone::serve`'s snapshot and the preview's made-up tabs
//! (`demo::phone`) both go through it. New with the drawer's groups; no
//! tmux-version file.

use serde_json::{Map, Value, json};

use crate::sidebar::model::{self, Rules, Stage, TabModel};
use crate::ui::kit::{self, Mark, Tone};

/// A tone as app.css names it: `.mark.<tone>` and `.label.<tone>`.
fn tone_name(tone: Tone) -> &'static str {
    match tone {
        Tone::Needs => "needs",
        Tone::Working => "working",
        Tone::Landing => "landing",
        Tone::Done => "passed",
        Tone::Failed => "failed",
        _ => "idle",
    }
}

/// The tabs in the sidebar's groups and order, each with its stage, as no
/// held layout or repository filter places them.
pub fn staged<'a>(tabs: &'a [TabModel], r: &Rules) -> Vec<(Stage, &'a TabModel)> {
    model::grouped(tabs, None, r)
        .into_iter()
        .flat_map(|g| g.tabs.into_iter().map(move |t| (g.stage, t)))
        .collect()
}

/// A tab's fields in the snapshot that come from the sidebar's model: its
/// state, whether it needs you, its pin, since when, its group (`stage` by
/// key, `group` by heading, `group_tone`), its badge (`mark`, `tone`,
/// `spins`), its `words`, and its reminder while Settings' "Reminders" is
/// on (when, in seconds, what for, and whether it has come due).
pub fn fields(tab: &TabModel, stage: Stage, r: &Rules) -> Map<String, Value> {
    let badge = model::badge_state(tab, r);
    let (mark, tone) = Mark::of_badge(badge).look();
    let group_tone = Mark::of_badge(stage.color()).look().1;
    let v = json!({
        "state": tab.state.name(),
        "needs": stage == Stage::Needs,
        "pinned": tab.pinned,
        "since": tab.since,
        "stage": stage.key(),
        "group": stage.title(),
        "group_tone": tone_name(group_tone),
        "mark": mark,
        "tone": tone_name(tone),
        "spins": kit::spins(badge),
        "words": model::doing_words(tab, r),
        "remind": tab.remind_at.filter(|_| r.reminders).map(|at| json!({
            "at": at,
            "note": tab.remind_note.clone().unwrap_or_default(),
            "due": model::reminder_due(tab, r),
        })),
    });
    match v {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::PrState;
    use crate::sidebar::model::tests::{rules, tab};
    use crate::sidebar::model::{MergeState, TabState};

    #[test]
    fn each_tab_carries_its_stage_badge_and_words_as_the_sidebar_has_them() {
        let r = rules();
        let mut shell = tab(1, "tests", TabState::Background);
        shell.background = 1;
        shell.shells = 1;
        let mut queued = tab(2, "queued", TabState::Idle);
        queued.merge = Some(MergeState::Queued);
        queued.position = Some(2);
        queued.pr_state = Some(PrState::Open);
        let waiting = tab(3, "asks", TabState::Waiting);
        let tabs = vec![shell, queued, waiting];
        let staged = staged(&tabs, &r);
        let order: Vec<_> = staged.iter().map(|(s, t)| (s.key(), t.index)).collect();
        assert_eq!(
            order,
            [("needs", 3), ("monitoring", 1), ("queued", 2)],
            "the sidebar's groups, in its order"
        );
        for (stage, t) in &staged {
            let f = fields(t, *stage, &r);
            let badge = model::badge_state(t, &r);
            assert_eq!(f["stage"], stage.key());
            assert_eq!(f["group"], stage.title());
            assert_eq!(f["mark"], Mark::of_badge(badge).look().0);
            assert_eq!(f["spins"], kit::spins(badge));
            assert_eq!(f["words"], model::doing_words(t, &r));
        }
        let shell = fields(staged[1].1, Stage::Monitoring, &r);
        assert_eq!(shell["group"], "Monitoring");
        assert_eq!(shell["words"], "waiting on 1 shell");
        let queued = fields(staged[2].1, Stage::Queued, &r);
        assert_eq!(queued["words"], "in merge queue at 2");
        assert_eq!(queued["mark"], "▲");
        assert_eq!(queued["tone"], "landing");
        assert_eq!(queued["spins"], false);
        let waiting = fields(staged[0].1, Stage::Needs, &r);
        assert_eq!(waiting["needs"], true);
        assert_eq!(waiting["group_tone"], "needs");
    }
}
