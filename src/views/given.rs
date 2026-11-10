//! The links in what you sent each pane's Claude, for M-p's link commands.
//! Claude Code folds a long paste into `[Pasted text #1 +12 lines]`, in its
//! prompt and in the conversation, so a link inside one is never on the
//! screen, where `shown::links_in` finds the rest. Its UserPromptSubmit
//! hook carries the prompt whole, pasted text and all, so the server keeps
//! the links of each as it comes: newest first, each once, the newest
//! `MOST`, until the pane's Claude starts a fresh session or the pane
//! closes. Only the links are kept, never the prompt, and nothing here
//! reads a file, starts a process or sends anything. New with the Rust
//! version: tmux/link-view.sh read `capture-pane` alone.

use std::collections::{HashMap, VecDeque};

use super::shown::{self, Shown};
use crate::mux::PaneId;

/// How many links a pane keeps.
pub const MOST: usize = 50;

/// What a list shows beside a link that came from a prompt.
pub const LABEL: &str = "you gave it";

/// Every pane's links from its prompts, newest first.
#[derive(Debug, Default)]
pub struct Given {
    panes: HashMap<PaneId, VecDeque<String>>,
}

impl Given {
    /// A prompt sent in a pane: its links go to the front, the last in the
    /// prompt first, one kept before moving up.
    pub fn took(&mut self, pane: PaneId, prompt: &str) {
        let found = shown::links_in(prompt, &[]);
        if found.is_empty() {
            return;
        }
        let kept = self.panes.entry(pane).or_default();
        for link in found.into_iter().rev() {
            kept.retain(|url| *url != link.url);
            kept.push_front(link.url);
        }
        kept.truncate(MOST);
    }

    /// A fresh session in the pane, or the pane closing.
    pub fn forget(&mut self, pane: PaneId) {
        self.panes.remove(&pane);
    }

    /// The pane's links, newest first, as a list shows them.
    pub fn of(&self, pane: PaneId) -> Vec<Shown> {
        self.panes
            .get(&pane)
            .into_iter()
            .flatten()
            .map(|url| Shown {
                text: LABEL.into(),
                url: url.clone(),
            })
            .collect()
    }

    /// Every pane's links, for handing them to a new build.
    pub fn records(&self) -> Vec<(PaneId, Vec<String>)> {
        let mut all: Vec<(PaneId, Vec<String>)> = self
            .panes
            .iter()
            .map(|(pane, kept)| (*pane, kept.iter().cloned().collect()))
            .collect();
        all.sort_by_key(|(pane, _)| *pane);
        all
    }

    /// A pane's links as the build before kept them.
    pub fn put(&mut self, pane: PaneId, links: Vec<String>) {
        let kept: VecDeque<String> = links.into_iter().take(MOST).collect();
        if !kept.is_empty() {
            self.panes.insert(pane, kept);
        }
    }
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
        given.took(P, "read https://a.example/1 and https://b.example/2.");
        given.took(P, "no link here");
        given.took(P, "again https://a.example/1, then https://c.example/3");
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
    }

    #[test]
    fn a_pane_keeps_only_the_newest_links() {
        let mut given = Given::default();
        for n in 0..MOST + 5 {
            given.took(P, &format!("see https://n.example/{n}"));
        }
        let kept = urls(&given, P);
        assert_eq!(kept.len(), MOST);
        assert_eq!(kept[0], format!("https://n.example/{}", MOST + 4));
        assert_eq!(kept[MOST - 1], "https://n.example/5");
    }

    #[test]
    fn a_fresh_session_forgets_them_and_a_new_build_takes_them_up() {
        let mut given = Given::default();
        given.took(P, "https://a.example/1 https://b.example/2");
        let handed = given.records();
        assert_eq!(
            handed,
            vec![(
                P,
                vec![
                    "https://b.example/2".to_string(),
                    "https://a.example/1".to_string()
                ]
            )]
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
    }
}
