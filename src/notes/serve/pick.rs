//! Which note a tab's work belongs to, asked and recorded on the tab
//! (`@tab_note`, a path in the notes directory) for M-e to show beside it:
//! ported from tmux/note-pick.sh. Every note is listed by when it was last
//! changed, the newest first, each with its age at the right, and what is
//! typed keeps the matches in that order rather than by how well they
//! match, so the note changed today stays above an old one with a tighter
//! match (the notes holding the words typed as written, then those matched
//! only as scattered letters, newest first in each). Above them, under
//! "Suggested" (the list itself under "Every note", with how many there
//! are: board 126, the labels the kit's), are at most three
//! notes the tab is likely about, so Enter takes the first: the note last shown beside it,
//! then notes that mention its ticket or PR number, those whose title
//! shares words with the tab's name, and those that mention its
//! repository's name; each is in its place below too, and typing leaves the
//! suggestions out. The notes are read here, on the computer, and sent
//! nowhere; off the server's task, since there may be many.
//!
//! "Keep whatever is open" records that the tab was asked
//! (`@tab_note_asked`), so M-e doesn't ask again; Esc leaves it to ask next
//! time.

use std::sync::LazyLock;

use regex::Regex;

use crate::mux::TabId;
use crate::mux::server::{ClientId, Server};
use crate::notes::store::{self, NoteEntry, Store};
use crate::ui::when::{self, Said};
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

/// The most notes suggested for a tab, above the list.
const SUGGESTED: usize = 3;

/// A note as the list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// Its path without `.md`.
    pub text: String,
    /// Its path in the notes directory.
    pub rel: String,
    /// How long ago it was changed (`2h`, `3d`).
    pub age: String,
}

/// The notes listed for a tab.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listed {
    /// The few the tab is likely about, the likeliest first.
    pub suggested: Vec<Row>,
    /// Every note, the most recently changed first.
    pub all: Vec<Row>,
}

/// The notes listed for a tab named `label`, whose last note shown beside
/// it is `last` and whose repository is named `repo`, their ages as of
/// `now` (seconds).
pub fn list(
    notes: &[NoteEntry],
    label: &str,
    last: Option<&str>,
    repo: Option<&str>,
    now: i64,
) -> Listed {
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
    by_age.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.rel.cmp(&b.rel)));
    let row = |note: &NoteEntry| Row {
        text: note
            .rel
            .strip_suffix(".md")
            .unwrap_or(&note.rel)
            .to_string(),
        rel: note.rel.clone(),
        age: when::ago(Some(note.mtime), now, Said::Short),
    };
    // (rank, order by age): lower first.
    let mut ranked: Vec<(f32, usize)> = Vec::new();
    for (at, note) in by_age.iter().enumerate() {
        if last == Some(note.rel.as_str()) {
            ranked.push((0.0, at));
        }
        let text = note.text.to_lowercase();
        if terms.iter().any(|t| text.contains(t.as_str())) {
            ranked.push((1.0, at));
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
            ranked.push((3.0 - shared as f32 / 10.0, at));
        }
        if repo.as_ref().is_some_and(|r| text.contains(r.as_str())) {
            ranked.push((4.0, at));
        }
    }
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut seen = std::collections::HashSet::new();
    Listed {
        suggested: ranked
            .into_iter()
            .filter(|(_, at)| seen.insert(*at))
            .take(SUGGESTED)
            .map(|(_, at)| row(by_age[at]))
            .collect(),
        all: by_age.into_iter().map(row).collect(),
    }
}

/// The list asking which note is the tab named `label`'s: the suggestions
/// under a dim "Suggested" line, then every note, newest first, each with
/// its age, then keeping what is open. What is typed matches the notes'
/// names alone, kept newest first, the suggestions left out.
///
/// `kit`: drawn with UX 2.0's atoms, as board 126, and whether the terminal
/// draws the half circles.
fn picker(label: &str, listed: Listed, kit: Option<bool>) -> Picker {
    let item = |row: Row| PickItem {
        key: row.age,
        ..PickItem::new(row.text, row.rel)
    };
    let off = |text: &str| PickItem {
        off: true,
        ..PickItem::new(text, "")
    };
    let mut items: Vec<PickItem> = Vec::new();
    if !listed.suggested.is_empty() {
        items.push(off("Suggested"));
        items.extend(listed.suggested.into_iter().map(item));
        items.push(off(""));
        items.push(PickItem {
            key: listed.all.len().to_string(),
            ..off("Every note")
        });
    }
    let suggestions = items.len();
    items.extend(listed.all.into_iter().map(item));
    items.push(PickItem::new("Keep whatever is open", ""));
    let picker = Picker::new(
        format!("Which note is this tab's? {label} · Esc asks again next time"),
        items,
    );
    let picker = match kit {
        Some(caps) => picker.kit(caps),
        None => picker,
    };
    picker
        .named("tab-note")
        .prompt("Note: ")
        .text_only()
        .headings()
        .keep_order()
        .order(Box::new(move |query, matched| {
            let typing = !query.trim().is_empty();
            matched
                .iter()
                .copied()
                .filter(|i| !typing || *i >= suggestions)
                .collect()
        }))
}

impl Server {
    /// Asks on `client` which note is `tab`'s, the few likely ones first so
    /// Enter takes one, then every note newest first, keeping what is open
    /// last.
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
                list(
                    &notes,
                    &named,
                    last.as_deref(),
                    repo.as_deref(),
                    crate::util::now_secs(),
                )
            },
            move |server, listed| {
                if !server.clients.contains_key(&client) || !server.tabs.contains_key(&tab) {
                    return;
                }
                let kit = Some(
                    server
                        .clients
                        .get(&client)
                        .is_some_and(|c| crate::mux::graphics::round_ends(&c.term)),
                );
                let picker = picker(&label, listed, kit);
                server.about_tab(client, tab, move |s| {
                    s.pick(client, picker, move |server, _, picked| {
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
                    })
                });
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::Overlay;
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

    fn rels(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.rel.as_str()).collect()
    }

    #[test]
    fn every_note_is_listed_newest_first_under_at_most_three_suggestions() {
        let notes = vec![
            note("Newest.md", 900, "nothing here"),
            note("Work/Checkout flow.md", 100, "the cart"),
            note("Mentions ticket.md", 50, "see abc-12 for this"),
            note("Repo notes.md", 300, "things about Switchboard"),
            note("Last one.md", 200, "x"),
            note("Checkout.md", 400, "y"),
        ];
        let listed = list(
            &notes,
            "#88 ABC-12: Fix the checkout flow",
            Some("Last one.md"),
            Some("switchboard"),
            1000,
        );
        assert_eq!(
            rels(&listed.all),
            [
                "Newest.md",
                "Checkout.md",
                "Repo notes.md",
                "Last one.md",
                "Work/Checkout flow.md",
                "Mentions ticket.md",
            ]
        );
        assert_eq!(
            rels(&listed.suggested),
            [
                // The note shown beside it last.
                "Last one.md",
                // The ticket, wherever in its text, whatever its case.
                "Mentions ticket.md",
                // Two words of the name in its title; the note with one, and
                // the repository's, are past the three.
                "Work/Checkout flow.md",
            ]
        );
        assert_eq!(
            listed.all[4].text, "Work/Checkout flow",
            "shown without .md"
        );
        assert_eq!(listed.all[0].age, "1m");
    }

    #[test]
    fn a_tab_with_nothing_to_go_on_has_no_suggestions() {
        let notes = vec![note("Old.md", 1, ""), note("New.md", 2, "")];
        let listed = list(&notes, "zsh", None, None, 10);
        assert!(listed.suggested.is_empty());
        assert_eq!(rels(&listed.all), ["New.md", "Old.md"]);
        let p = picker("zsh", listed, None);
        assert_eq!(p.shown(), ["New", "Old", "Keep whatever is open"]);
    }

    #[test]
    fn the_list_is_drawn_as_board_126() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        let notes = vec![
            note("Standup.md", 100, ""),
            note("Other.md", 500, ""),
            note("Newest.md", 940, ""),
        ];
        let listed = list(&notes, "Standup", Some("Other.md"), None, 1000);
        let mut p = picker("Standup", listed, Some(false));
        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 30));
        p.draw(&mut buf, &crate::ui::Look::default());
        let s = crate::ui::draw::tests::screen(&buf);
        let row = |text: &str| {
            s.iter()
                .position(|l| l.contains(text))
                .unwrap_or_else(|| panic!("no {text}: {s:#?}"))
        };
        // The labels in capitals, the list's with how many notes it has.
        let (suggested, every) = (row("SUGGESTED"), row("EVERY NOTE"));
        assert!(s[every].contains(" 3"), "{s:#?}");
        // The first suggestion chosen under its label, its age at the right.
        assert!(s[suggested + 1].contains("› Other"), "{s:#?}");
        assert!(s[suggested + 1].contains("8m"), "{s:#?}");
        // A blank row, then every note, newest first.
        assert!(s[every - 1].replace('│', "").trim().is_empty(), "{s:#?}");
        assert!(s[every + 1].contains("Newest") && s[every + 1].contains("1m"));
        assert!(s[every + 2].contains("Other"));
        assert!(s[every + 3].contains("Standup"));
    }

    #[test]
    fn typing_keeps_the_matches_newest_first_and_leaves_the_suggestions_out() {
        let notes = vec![
            note("Standup.md", 100, ""),
            note("Standup notes from the spring.md", 900, ""),
            note("Other.md", 500, ""),
            note("Sprint tasks and dev updates.md", 950, ""),
        ];
        let listed = list(&notes, "Standup", Some("Other.md"), None, 1000);
        let p = picker("Standup", listed.clone(), None);
        assert_eq!(
            p.shown(),
            [
                "Suggested",
                "Other",
                "Standup notes from the spring",
                "Standup",
                "",
                "Every note",
                "Sprint tasks and dev updates",
                "Standup notes from the spring",
                "Other",
                "Standup",
                "Keep whatever is open",
            ]
        );
        assert_eq!(
            p.chosen().unwrap().value,
            "Other.md",
            "Enter takes the first"
        );
        // Up from there goes round to keeping what is open, never onto the
        // "Suggested" line.
        let mut p = p;
        p.key(
            crate::mux::input::Key::new(
                crate::mux::input::KeyCode::Up,
                crate::mux::input::Mods::NONE,
            ),
            &crate::ui::Look::default(),
        );
        assert_eq!(p.chosen().unwrap().text, "Keep whatever is open");
        // The old note's name is the tighter match, and the new one leads.
        let p = picker("Standup", listed.clone(), None).query("standup");
        // The newest holds its letters only scattered, so it comes last.
        assert_eq!(
            p.shown(),
            [
                "Standup notes from the spring",
                "Standup",
                "Sprint tasks and dev updates",
            ]
        );
        // A note's age isn't what is typed.
        assert!(
            picker("Standup", listed, Some(false))
                .query("1m")
                .shown()
                .is_empty()
        );
    }
}
