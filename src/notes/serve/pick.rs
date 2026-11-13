//! Which note a tab's work belongs to, asked and recorded on the tab
//! (`@tab_note`, a path in the notes directory) for M-e to show beside it:
//! ported from tmux/note-pick.sh. The likely ones come first, from what the
//! tab is about: notes that mention its ticket or PR number, then those
//! whose title shares words with the tab's name, then the note last shown
//! beside it, then those that mention its repository's name, then the rest,
//! the most recently changed first within each. The notes are read here, on
//! the computer, and sent nowhere; off the server's task, since there may
//! be many.
//!
//! "Keep whatever is open" records that the tab was asked
//! (`@tab_note_asked`), so M-e doesn't ask again; Esc leaves it to ask next
//! time.

use std::sync::LazyLock;

use regex::Regex;

use crate::mux::TabId;
use crate::mux::server::{ClientId, Server};
use crate::notes::store::{self, NoteEntry, Store};
use crate::ui::{PickItem, Picker};

/// Words too common to say what a tab is about.
const STOP: &[&str] = &[
    "a", "an", "and", "the", "of", "for", "to", "in", "on", "with", "at", "by", "is", "it", "my",
    "our",
];

/// A ticket ID or a PR number in a tab's name.
static TERM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Z][A-Z0-9]*-[0-9]+|#[0-9]+").expect("a regex"));

/// The words of a text, lowercase, letters and digits.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// The notes ranked for a tab named `label`, whose last note shown beside
/// it is `last` and whose repository is named `repo`: (what the list shows,
/// the note's path), the likeliest first.
pub fn rank(
    notes: &[NoteEntry],
    label: &str,
    last: Option<&str>,
    repo: Option<&str>,
) -> Vec<(String, String)> {
    let mut terms: Vec<String> = TERM
        .find_iter(label)
        .map(|m| m.as_str().to_lowercase())
        .collect();
    terms.sort();
    terms.dedup();
    let wanted: Vec<String> = words(label)
        .into_iter()
        .filter(|w| w.len() > 1 && !STOP.contains(&w.as_str()))
        .collect();
    let repo = repo
        .filter(|r| !r.is_empty() && *r != ".")
        .map(str::to_lowercase);

    let mut by_age: Vec<&NoteEntry> = notes.iter().collect();
    by_age.sort_by_key(|n| std::cmp::Reverse(n.mtime));
    // (rank, order by age, path): lower first.
    let mut ranked: Vec<(f32, usize, &str)> = Vec::new();
    for (at, note) in by_age.iter().enumerate() {
        let text = note.text.to_lowercase();
        if terms.iter().any(|t| text.contains(t.as_str())) {
            ranked.push((0.0, at, &note.rel));
        }
        let title = note
            .rel
            .rsplit('/')
            .next()
            .unwrap_or(&note.rel)
            .trim_end_matches(".md");
        let has = words(title);
        let shared = wanted.iter().filter(|w| has.contains(w)).count().min(9);
        if shared > 0 {
            ranked.push((2.0 - shared as f32 / 10.0, at, &note.rel));
        }
        if last == Some(note.rel.as_str()) {
            ranked.push((2.0, at, &note.rel));
        }
        if repo.as_ref().is_some_and(|r| text.contains(r.as_str())) {
            ranked.push((3.0, at, &note.rel));
        }
        ranked.push((4.0, at, &note.rel));
    }
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut seen = std::collections::HashSet::new();
    ranked
        .into_iter()
        .filter(|(_, _, rel)| seen.insert(*rel))
        .map(|(_, _, rel)| {
            (
                rel.strip_suffix(".md").unwrap_or(rel).to_string(),
                rel.to_string(),
            )
        })
        .collect()
}

impl Server {
    /// Asks on `client` which note is `tab`'s, the likeliest first so Enter
    /// takes it, keeping what is open last.
    pub(super) fn ask_tab_note(&mut self, tab: TabId, client: Option<ClientId>) {
        let Some(client) = client.filter(|c| self.clients.contains_key(c)) else {
            return;
        };
        let label = self.tab_label(tab);
        let last = self.tab_opt(tab, "@tab_note_last");
        let dir = self
            .tabs
            .get(&tab)
            .and_then(|t| self.dir_of(t.active))
            .or_else(|| self.tab_path(tab).map(std::path::Path::to_path_buf));
        let named = label.clone();
        self.notes_off_task(
            move || {
                let repo = dir
                    .as_deref()
                    .and_then(crate::cmd::work::main_clone)
                    .and_then(|root| root.file_name().map(|n| n.to_string_lossy().into_owned()));
                let notes = Store::at(store::notes_dir()).notes(None);
                rank(&notes, &named, last.as_deref(), repo.as_deref())
            },
            move |server, ranked| {
                if !server.clients.contains_key(&client) || !server.tabs.contains_key(&tab) {
                    return;
                }
                let mut items: Vec<PickItem> = ranked
                    .into_iter()
                    .map(|(text, rel)| PickItem::new(text, rel))
                    .collect();
                items.push(PickItem::new("Keep whatever is open", ""));
                let picker = Picker::new(
                    format!("Which note is this tab's? {label} · Esc asks again next time"),
                    items,
                )
                .named("tab-note")
                .prompt("Note: ");
                server.pick(client, picker, move |server, _, picked| {
                    let Some(picked) = picked else {
                        return;
                    };
                    if picked.value.is_empty() {
                        server.set_tab_opt(tab, "@tab_note_asked", Some("1"));
                    } else {
                        server.set_tab_opt(tab, "@tab_note", Some(&picked.value));
                        server.set_tab_opt(tab, super::BY_YOU, Some(&picked.value));
                        // Notes beside the tab already shows it now.
                        server.notes_show(tab);
                    }
                });
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn note(rel: &str, mtime: i64, text: &str) -> NoteEntry {
        NoteEntry {
            path: PathBuf::from(rel),
            rel: rel.to_string(),
            folder: String::new(),
            title: String::new(),
            preview: String::new(),
            mtime,
            text: text.to_string(),
            pinned: false,
            matched: None,
        }
    }

    #[test]
    fn the_likely_notes_come_first() {
        let notes = vec![
            note("Newest.md", 900, "nothing here"),
            note("Work/Checkout flow.md", 100, "the cart"),
            note("Mentions ticket.md", 50, "see abc-12 for this"),
            note("Repo notes.md", 300, "things about Switchboard"),
            note("Last one.md", 200, "x"),
            note("Checkout.md", 400, "y"),
        ];
        let ranked = rank(
            &notes,
            "#88 ABC-12: Fix the checkout flow",
            Some("Last one.md"),
            Some("switchboard"),
        );
        let order: Vec<&str> = ranked.iter().map(|(_, rel)| rel.as_str()).collect();
        assert_eq!(
            order,
            [
                // The ticket, wherever in its text, whatever its case.
                "Mentions ticket.md",
                // Two words of the name in its title, then one.
                "Work/Checkout flow.md",
                "Checkout.md",
                // The note shown beside it last, then the repository's.
                "Last one.md",
                "Repo notes.md",
                // Then the rest, newest first.
                "Newest.md",
            ]
        );
        assert_eq!(ranked[1].0, "Work/Checkout flow", "shown without .md");
    }

    #[test]
    fn a_tab_with_nothing_to_go_on_lists_the_newest_first() {
        let notes = vec![note("Old.md", 1, ""), note("New.md", 2, "")];
        let ranked = rank(&notes, "zsh", None, None);
        assert_eq!(
            ranked,
            vec![
                ("New".to_string(), "New.md".to_string()),
                ("Old".to_string(), "Old.md".to_string())
            ]
        );
    }
}
