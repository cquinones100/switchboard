//! The links in what you sent each pane's Claude and in what it answered,
//! with when each was last seen, for M-p's link commands. Claude Code folds
//! a long paste into `[Pasted text #1 +12 lines]`, in its prompt and in the
//! conversation, so a link inside one is never on the screen, where
//! `shown::links_in` finds the rest. Its UserPromptSubmit hook carries the
//! prompt whole, pasted text and all, and its Stop hook the turn's last
//! message, so the server keeps the links of each as it comes, with the
//! moment: a prompt's to list (`of`), a reply's only to order the list
//! newest first (`at`), since the screen shows those. Newest first, each
//! once, the newest `MOST` of each, until the pane's Claude starts a fresh session
//! or the pane closes. Only the links are kept, never the prompt, and
//! nothing here reads a file, starts a process or sends anything. New with
//! the Rust version: tmux/link-view.sh read `capture-pane` alone.

use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};

use super::shown::{self, Shown};
use crate::mux::PaneId;

/// How many links a pane keeps of each kind: from its prompts, and from
/// its replies alone, so a run of replies never pushes out what you gave.
pub const MOST: usize = 500;

/// What a list shows beside a link that came from a prompt.
pub const LABEL: &str = "you gave it";

/// A link kept: its address, when it was last seen (milliseconds since the
/// epoch, none for one handed over by a build that kept no times), and
/// whether a prompt held it, which lists it, rather than a reply alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    pub url: String,
    #[serde(default)]
    pub at: Option<i64>,
    #[serde(default)]
    pub prompt: bool,
}

/// Every pane's links from its prompts and replies, newest first.
#[derive(Debug, Default)]
pub struct Given {
    panes: HashMap<PaneId, VecDeque<Kept>>,
}

impl Given {
    /// A prompt sent in a pane at `now`: its links go to the front, the
    /// last in the prompt first, one kept before moving up.
    pub fn took(&mut self, pane: PaneId, prompt: &str, now: i64) {
        self.keep(pane, prompt, now, true);
    }

    /// A turn of the pane's Claude ended at `now`, its last message
    /// `reply`: its links are timed, to order the list, and listed only
    /// when a prompt held them too.
    pub fn replied(&mut self, pane: PaneId, reply: &str, now: i64) {
        self.keep(pane, reply, now, false);
    }

    fn keep(&mut self, pane: PaneId, text: &str, now: i64, prompt: bool) {
        let found = shown::links_in(text, &[]);
        if found.is_empty() {
            return;
        }
        let kept = self.panes.entry(pane).or_default();
        for link in found.into_iter().rev() {
            let was = kept
                .iter()
                .position(|k| k.url == link.url)
                .and_then(|i| kept.remove(i));
            kept.push_front(Kept {
                url: link.url,
                at: Some(now),
                prompt: prompt || was.is_some_and(|k| k.prompt),
            });
        }
        limit(kept);
    }

    /// A fresh session in the pane, or the pane closing.
    pub fn forget(&mut self, pane: PaneId) {
        self.panes.remove(&pane);
    }

    /// The pane's links from its prompts, newest first, as a list shows
    /// them.
    pub fn of(&self, pane: PaneId) -> Vec<Shown> {
        self.panes
            .get(&pane)
            .into_iter()
            .flatten()
            .filter(|k| k.prompt)
            .map(|k| Shown {
                text: LABEL.into(),
                url: k.url.clone(),
            })
            .collect()
    }

    /// When the pane's Claude last saw `url`, in a prompt or a reply.
    pub fn at(&self, pane: PaneId, url: &str) -> Option<i64> {
        self.panes
            .get(&pane)?
            .iter()
            .find(|k| k.url == url)
            .and_then(|k| k.at)
    }

    /// Every pane's links, for handing them to a new build.
    pub fn records(&self) -> Vec<(PaneId, Vec<Kept>)> {
        let mut all: Vec<(PaneId, Vec<Kept>)> = self
            .panes
            .iter()
            .map(|(pane, kept)| (*pane, kept.iter().cloned().collect()))
            .collect();
        all.sort_by_key(|(pane, _)| *pane);
        all
    }

    /// A pane's links as the build before kept them.
    pub fn put(&mut self, pane: PaneId, links: Vec<Kept>) {
        let mut kept: VecDeque<Kept> = links.into_iter().collect();
        limit(&mut kept);
        if !kept.is_empty() {
            self.panes.insert(pane, kept);
        }
    }

    /// A pane's links as a build that kept no times handed them over: the
    /// prompts' alone, untimed.
    pub fn put_untimed(&mut self, pane: PaneId, links: Vec<String>) {
        let links = links
            .into_iter()
            .map(|url| Kept {
                url,
                at: None,
                prompt: true,
            })
            .collect();
        self.put(pane, links);
    }
}

/// A pane's links held to the newest `MOST` of each kind.
fn limit(kept: &mut VecDeque<Kept>) {
    let (mut prompts, mut replies) = (0, 0);
    kept.retain(|k| {
        let count = if k.prompt { &mut prompts } else { &mut replies };
        *count += 1;
        *count <= MOST
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: PaneId = PaneId(1);

    fn urls(given: &Given, pane: PaneId) -> Vec<String> {
        given.of(pane).into_iter().map(|s| s.url).collect()
    }

    #[test]
    fn a_prompts_links_are_kept_newest_first_each_once() {
        let mut given = Given::default();
        given.took(P, "read https://a.example/1 and https://b.example/2.", 1);
        given.took(P, "no link here", 2);
        given.took(P, "again https://a.example/1, then https://c.example/3", 3);
        assert_eq!(
            urls(&given, P),
            vec![
                "https://c.example/3",
                "https://a.example/1",
                "https://b.example/2",
            ]
        );
        assert!(given.of(P).iter().all(|s| s.text == LABEL));
        assert!(given.of(PaneId(2)).is_empty());
        assert_eq!(given.at(P, "https://a.example/1"), Some(3));
        assert_eq!(given.at(P, "https://b.example/2"), Some(1));
    }

    #[test]
    fn a_replys_links_are_timed_but_not_listed() {
        let mut given = Given::default();
        given.took(P, "see https://a.example/1", 1);
        given.replied(P, "made https://b.example/2 from https://a.example/1", 2);
        assert_eq!(urls(&given, P), vec!["https://a.example/1"]);
        assert_eq!(given.at(P, "https://a.example/1"), Some(2));
        assert_eq!(given.at(P, "https://b.example/2"), Some(2));
        assert_eq!(given.at(P, "https://c.example/3"), None);
    }

    #[test]
    fn a_pane_keeps_only_the_newest_links() {
        let mut given = Given::default();
        for n in 0..MOST + 5 {
            given.took(P, &format!("see https://n.example/{n}"), n as i64);
        }
        let kept = urls(&given, P);
        assert_eq!(kept.len(), MOST);
        assert_eq!(kept[0], format!("https://n.example/{}", MOST + 4));
        assert_eq!(kept[MOST - 1], "https://n.example/5");
        for n in 0..MOST + 5 {
            given.replied(P, &format!("made https://r.example/{n}"), 1000 + n as i64);
        }
        assert_eq!(urls(&given, P), kept, "replies push out no prompt");
        assert_eq!(given.at(P, "https://r.example/4"), None);
        assert_eq!(given.at(P, "https://r.example/5"), Some(1005));
        let mut next = Given::default();
        for (pane, links) in given.records() {
            next.put(pane, links);
        }
        assert_eq!(urls(&next, P), kept, "and so through an upgrade");
        assert_eq!(next.at(P, "https://r.example/5"), Some(1005));
    }

    #[test]
    fn a_fresh_session_forgets_them_and_a_new_build_takes_them_up() {
        let mut given = Given::default();
        given.took(P, "https://a.example/1 https://b.example/2", 7);
        let handed = given.records();
        assert_eq!(handed.len(), 1);
        assert_eq!(
            handed[0]
                .1
                .iter()
                .map(|k| k.url.as_str())
                .collect::<Vec<_>>(),
            vec!["https://b.example/2", "https://a.example/1"]
        );
        given.forget(P);
        assert!(given.of(P).is_empty());
        assert!(given.records().is_empty());
        let mut next = Given::default();
        for (pane, links) in handed {
            next.put(pane, links);
        }
        assert_eq!(
            urls(&next, P),
            vec!["https://b.example/2", "https://a.example/1"]
        );
        assert_eq!(next.at(P, "https://a.example/1"), Some(7));
        let mut older = Given::default();
        older.put_untimed(P, vec!["https://a.example/1".into()]);
        assert_eq!(urls(&older, P), vec!["https://a.example/1"]);
        assert_eq!(older.at(P, "https://a.example/1"), None);
    }
}
