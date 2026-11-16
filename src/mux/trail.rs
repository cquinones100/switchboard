//! The places a session has visited, for M-{ and M-} to go back and forward
//! through as a browser's buttons do, and M-Tab to go to the one before
//! (new, no tmux version had it). A place is a tab, or a mission control
//! view (by its name, with the tab it sat over). Every move to another
//! place puts the one left on `back` and clears `forward`, however it came;
//! a step back or forward moves the current place to the other side
//! instead. A tab that closes is taken out of both, a view whose setting is
//! off is passed over, and each keeps at most `CAP`.

use serde::{Deserialize, Serialize};

use super::TabId;

/// A place to go back or forward to. Untagged, so a tab is its number as
/// it was when the trail held only tabs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Place {
    Tab(TabId),
    /// A mission control view, by `View::slug` ("tabs" the Tabs view), and
    /// the tab mission control was open over.
    View {
        view: String,
        over: TabId,
    },
}

impl Place {
    /// The same place: the same tab, or the same view whatever tab it sat
    /// over, so a view visited twice in a row is one step.
    pub fn same(&self, other: &Place) -> bool {
        match (self, other) {
            (Place::Tab(a), Place::Tab(b)) => a == b,
            (Place::View { view: a, .. }, Place::View { view: b, .. }) => a == b,
            _ => false,
        }
    }
}

/// The places one way, nearest first, each with how many steps away it is.
pub type Steps = Vec<(Place, usize)>;

/// How many places each way are kept.
pub const CAP: usize = 50;

/// In a handover, `back` and `forward` hold the tabs alone, as a build
/// from before views joined the trail reads them, so restarting onto one
/// keeps every pane; the places, views included, go in fields of their
/// own, which such a build passes over and a newer one reads first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Saved", into = "Saved")]
pub struct Trail {
    /// The places before, the latest last.
    #[serde(default)]
    pub back: Vec<Place>,
    /// The places after, the nearest last.
    #[serde(default)]
    pub forward: Vec<Place>,
    /// The view mission control shows over the current tab, when the
    /// session is on one rather than on the tab; not kept over a restart,
    /// which mission control doesn't outlive.
    #[serde(skip)]
    pub view: Option<String>,
    /// The place before the current one, however it was left, for M-Tab.
    #[serde(skip)]
    pub last: Option<Place>,
}

/// A trail as a handover keeps it (see `Trail`).
#[derive(Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    back: Vec<TabId>,
    #[serde(default)]
    forward: Vec<TabId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    back_places: Option<Vec<Place>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    forward_places: Option<Vec<Place>>,
}

impl From<Trail> for Saved {
    fn from(t: Trail) -> Saved {
        let tabs = |list: &[Place]| {
            list.iter()
                .filter_map(|p| match p {
                    Place::Tab(t) => Some(*t),
                    Place::View { .. } => None,
                })
                .collect()
        };
        Saved {
            back: tabs(&t.back),
            forward: tabs(&t.forward),
            back_places: Some(t.back),
            forward_places: Some(t.forward),
        }
    }
}

impl From<Saved> for Trail {
    fn from(s: Saved) -> Trail {
        let places = |kept: Option<Vec<Place>>, tabs: Vec<TabId>| {
            kept.unwrap_or_else(|| tabs.into_iter().map(Place::Tab).collect())
        };
        Trail {
            back: places(s.back_places, s.back),
            forward: places(s.forward_places, s.forward),
            ..Trail::default()
        }
    }
}

impl Trail {
    pub fn is_empty(&self) -> bool {
        self.back.is_empty() && self.forward.is_empty()
    }

    /// Where the session is, its tab in front being `current`.
    pub fn here(&self, current: TabId) -> Place {
        match &self.view {
            Some(view) => Place::View {
                view: view.clone(),
                over: current,
            },
            None => Place::Tab(current),
        }
    }

    /// A move away from `left` that wasn't a step back or forward.
    pub fn visited(&mut self, left: Place) {
        push(&mut self.back, left.clone());
        self.forward.clear();
        self.last = Some(left);
    }

    /// The place to go back to from `current`, which goes on `forward`,
    /// passing over the places `usable` refuses; none when there is
    /// nothing before.
    pub fn back(&mut self, current: Place, usable: impl Fn(&Place) -> bool) -> Option<Place> {
        let to = pop_other(&mut self.back, &current, &usable)?;
        push(&mut self.forward, current.clone());
        self.last = Some(current);
        Some(to)
    }

    /// The place to go forward to from `current`, which goes on `back`.
    pub fn forward(&mut self, current: Place, usable: impl Fn(&Place) -> bool) -> Option<Place> {
        let to = pop_other(&mut self.forward, &current, &usable)?;
        push(&mut self.back, current.clone());
        self.last = Some(current);
        Some(to)
    }

    /// The place a step back from `current` would go to, without taking it.
    pub fn peek_back(&self, current: &Place, usable: impl Fn(&Place) -> bool) -> Option<Place> {
        peek(&self.back, current, &usable)
    }

    /// The place a step forward from `current` would go to.
    pub fn peek_forward(&self, current: &Place, usable: impl Fn(&Place) -> bool) -> Option<Place> {
        peek(&self.forward, current, &usable)
    }

    /// Every step each way from `current`, nearest first, with how many
    /// steps away it is, as stepping again and again would go: the back
    /// ones, then the forward ones.
    pub fn steps(&self, current: &Place, usable: impl Fn(&Place) -> bool) -> (Steps, Steps) {
        let way = |list: &[Place]| {
            let mut list = list.to_vec();
            let mut out: Steps = Vec::new();
            let mut at = current.clone();
            while let Some(p) = pop_other(&mut list, &at, &usable) {
                out.push((p.clone(), out.len() + 1));
                at = p;
            }
            out
        };
        (way(&self.back), way(&self.forward))
    }

    /// A tab closed, or not in the session any more. A view it sat over
    /// stays, to open over the tab in front.
    pub fn forget(&mut self, tab: TabId) {
        let gone = Place::Tab(tab);
        self.back.retain(|p| *p != gone);
        self.forward.retain(|p| *p != gone);
        if self.last.as_ref() == Some(&gone) {
            self.last = None;
        }
    }

    /// Only the tabs `keep` holds, for a trail read back from a handover;
    /// its views all stay.
    pub fn keeping(mut self, keep: &[TabId]) -> Trail {
        let kept = |p: &Place| match p {
            Place::Tab(t) => keep.contains(t),
            Place::View { .. } => true,
        };
        self.back.retain(kept);
        self.forward.retain(kept);
        self
    }
}

fn push(list: &mut Vec<Place>, place: Place) {
    if !list.last().is_some_and(|p| p.same(&place)) {
        list.push(place);
    }
    if list.len() > CAP {
        list.drain(..list.len() - CAP);
    }
}

fn peek(list: &[Place], current: &Place, usable: &dyn Fn(&Place) -> bool) -> Option<Place> {
    list.iter()
        .rev()
        .find(|p| !p.same(current) && usable(p))
        .cloned()
}

/// The latest place of `list` that isn't `current` and is usable, the ones
/// passed over dropped.
fn pop_other(
    list: &mut Vec<Place>,
    current: &Place,
    usable: &dyn Fn(&Place) -> bool,
) -> Option<Place> {
    while let Some(p) = list.pop() {
        if !p.same(current) && usable(&p) {
            return Some(p);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Place = Place::Tab(TabId(1));
    const B: Place = Place::Tab(TabId(2));
    const C: Place = Place::Tab(TabId(3));

    fn any(_: &Place) -> bool {
        true
    }

    fn view(name: &str) -> Place {
        Place::View {
            view: name.into(),
            over: TabId(1),
        }
    }

    #[test]
    fn back_and_forward_retrace_the_visits() {
        let mut t = Trail::default();
        t.visited(A);
        t.visited(B);
        assert_eq!(t.back(C, any), Some(B));
        assert_eq!(t.back(B, any), Some(A));
        assert_eq!(t.back(A, any), None);
        assert_eq!(t.forward(A, any), Some(B));
        assert_eq!(t.forward(B, any), Some(C));
        assert_eq!(t.forward(C, any), None);
    }

    #[test]
    fn a_new_visit_after_going_back_clears_forward() {
        let mut t = Trail::default();
        t.visited(A);
        t.visited(B);
        assert_eq!(t.back(C, any), Some(B));
        t.visited(B);
        assert_eq!(t.forward(A, any), None);
        assert_eq!(t.back(A, any), Some(B));
    }

    #[test]
    fn peeking_and_steps_take_nothing_off_the_trail() {
        let mut t = Trail::default();
        t.visited(A);
        t.visited(B);
        assert_eq!(t.back(C, any), Some(B));
        assert_eq!(t.peek_back(&B, any), Some(A));
        assert_eq!(t.peek_forward(&B, any), Some(C));
        assert_eq!(t.steps(&B, any), (vec![(A, 1)], vec![(C, 1)]));
        assert_eq!(t.back(B, any), Some(A));
        assert_eq!(t.steps(&A, any), (vec![], vec![(B, 1), (C, 2)]));
    }

    #[test]
    fn a_closed_tab_is_skipped() {
        let mut t = Trail::default();
        t.visited(A);
        t.visited(B);
        t.forget(TabId(2));
        assert_eq!(t.back(C, any), Some(A));
        assert_eq!(t.back(A, any), None);
    }

    #[test]
    fn views_are_places_and_one_turned_off_is_passed_over() {
        let mut t = Trail::default();
        t.visited(A);
        t.visited(view("reopen"));
        t.visited(view("reopen"));
        t.visited(view("prs"));
        assert_eq!(t.back.len(), 3, "the same view twice in a row is one");
        let no_prs = |p: &Place| *p != view("prs");
        assert_eq!(t.peek_back(&B, no_prs), Some(view("reopen")));
        assert_eq!(t.back(B, no_prs), Some(view("reopen")));
        assert_eq!(t.back(view("reopen"), no_prs), Some(A));
        assert_eq!(t.last, Some(view("reopen")));
    }

    #[test]
    fn a_trail_of_tabs_alone_reads_back_and_an_older_build_reads_its_tabs() {
        let t: Trail = serde_json::from_str(r#"{"back":[1,2],"forward":[3]}"#).unwrap();
        assert_eq!(t.back, vec![A, B]);
        assert_eq!(t.forward, vec![C]);
        let saved = serde_json::to_string(&Trail {
            back: vec![A, view("reopen")],
            ..Trail::default()
        })
        .unwrap();
        let again: Trail = serde_json::from_str(&saved).unwrap();
        assert_eq!(again.back, vec![A, view("reopen")]);
        // What a build from before views joined the trail reads.
        #[derive(Deserialize)]
        struct Older {
            back: Vec<TabId>,
        }
        let older: Older = serde_json::from_str(&saved).unwrap();
        assert_eq!(older.back, vec![TabId(1)]);
    }

    #[test]
    fn each_way_keeps_at_most_the_cap() {
        let mut t = Trail::default();
        for n in 0..(CAP as u32 + 10) {
            t.visited(Place::Tab(TabId(n)));
        }
        assert_eq!(t.back.len(), CAP);
        assert_eq!(t.back[0], Place::Tab(TabId(10)));
    }

    #[test]
    fn keeping_drops_tabs_gone() {
        let t = Trail {
            back: vec![A, B, view("reopen")],
            forward: vec![C],
            ..Trail::default()
        };
        assert_eq!(
            t.keeping(&[TabId(1), TabId(3)]),
            Trail {
                back: vec![A, view("reopen")],
                forward: vec![C],
                ..Trail::default()
            }
        );
    }
}
