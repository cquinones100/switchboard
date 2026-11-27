//! Ctrl-F: find a note, ported from notes/lua/notes/finder.lua, which ran
//! fzf in a floating window. Every note but those in Recently Deleted is
//! listed as the list lists them (pinned first, then newest), each as its
//! title, its folder and how it starts, any of which you can type to match,
//! with the note itself shown beside the list. Enter opens it; Escape closes
//! the finder. It is `ui::picker`'s list, its previews beside the rows.

use super::store::NoteEntry;
use switchboard_ui::draw;
use switchboard_ui::{PickItem, Picker};

/// The finder over `notes`, titled "Find a note", or "Recent notes" for
/// the notes opened lately: picking gives the note's path as the value.
pub fn finder(notes: Vec<NoteEntry>, title: &'static str) -> Picker {
    let previews = notes.iter().map(|n| n.text.clone()).collect();
    let items = notes
        .iter()
        .map(|n| PickItem::new(row(n), n.path.to_string_lossy()))
        .collect();
    Picker::new(title, items)
        .prompt(format!("{title}: "))
        .keep_order()
        .beside(previews)
        .named("finder")
}

/// A note's row, which is what is matched too: its title (a dot before a
/// pinned one), its folder and how it starts, each padded to its column
/// but never cut, so all of each is matched.
fn row(n: &NoteEntry) -> String {
    let folder = if n.folder.is_empty() {
        "Notes"
    } else {
        &n.folder
    };
    let mark = if n.pinned { "● " } else { "  " };
    let pad = |text: &str, cols: usize| {
        format!(
            "{text}{}",
            " ".repeat(cols.saturating_sub(draw::width(text)))
        )
    };
    format!(
        "{} {}  {}",
        pad(&format!("{mark}{}", n.title), 32),
        pad(folder, 14),
        n.preview
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::{set_mtime, temp};
    use switchboard_ui::input::{Key, KeyCode, Mods};
    use switchboard_ui::{Look, Outcome, Overlay, Step};

    fn titles(f: &Picker) -> Vec<String> {
        f.shown()
            .iter()
            .map(|r| r.split_whitespace().next().unwrap_or("").to_string())
            .collect()
    }

    #[test]
    fn the_finder_lists_every_note_newest_first_and_matches_what_is_typed() {
        let (_dir, mut s) = temp("finder");
        let standup = s.create("Work", Some("Standup"));
        set_mtime(&standup, crate::util::now_secs() - 60);
        s.create("", Some("Groceries"));
        let gone = s.create("", Some("Gone"));
        s.delete(&gone);
        let look = Look::default();
        let mut f = finder(s.notes(None), "Find a note");
        assert_eq!(
            titles(&f),
            ["Groceries", "Standup"],
            "Recently Deleted isn't listed"
        );
        for c in "work".chars() {
            f.key(Key::new(KeyCode::Char(c), Mods::NONE), &look);
        }
        assert_eq!(titles(&f), ["Standup"], "its folder matches too");
        match f.key(Key::new(KeyCode::Enter, Mods::NONE), &look) {
            Step::Close(Outcome::Picked(p)) => assert!(p.value.ends_with("Work/Standup.md")),
            _ => panic!("Enter opens the one picked"),
        }
    }
}
