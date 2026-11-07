//! The list on the left, as in Notes, ported from
//! notes/lua/notes/sidebar.lua: folders, tags, then the notes in what is
//! chosen, pinned ones first, then newest first, each as its title and a
//! line of when it changed and how it starts. A click or Enter on a folder
//! or a tag shows its notes; on a note, opens it.
//!
//! ```text
//!   n  new note here          N  new folder        dd  delete (to Recently Deleted)
//!   p  pin or unpin           m  move to a folder  r   restore, in Recently Deleted
//!   /  search all notes       x  clear the search or tag   Ctrl-F  find a note
//!   R  rename a note or folder  q  quit            ?   these keys
//!   s  mark to squash         S  squash the marked ones into one
//!   t  the note's tab         Ctrl-B  put the list away (in a note, show it again)
//!   Ctrl-Shift-P  every command, in a palette
//! ```
//!
//! This is the list's model: what it shows, built from the store; the app
//! draws it and acts on its rows.

use std::path::{Path, PathBuf};

use jiff::civil::Weekday;

use super::store::{self, NoteEntry, Store, TRASH};
use crate::ui::draw::{cut, width};
use crate::util;

/// Its full width, and the narrowest it gets beside a note.
pub const WIDTH: u16 = 34;
pub const MIN_WIDTH: u16 = 24;

/// What a row of the list does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// A folder: `None` for All Notes, "" for the top folder, `TRASH` for
    /// Recently Deleted.
    Folder(Option<String>),
    Tag(String),
    Note(PathBuf),
}

/// How a row is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Plain,
    Header,
    Section,
    /// The folder or tag shown.
    Current,
    /// The note open.
    Open,
    Title,
    Detail,
    Tag,
    Rule,
    Faint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub text: String,
    pub look: Look,
    pub item: Option<Item>,
}

/// What is shown: a folder, or the notes with a tag, or those matching a
/// search; and the notes marked with s for squashing, in the order marked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub folder: Option<String>,
    pub tag: Option<String>,
    pub query: Option<String>,
    pub marked: Vec<PathBuf>,
}

/// When a note changed, as Notes says it: the time today, Yesterday, the
/// day this week, else the date.
pub fn when(mtime: i64, now: &jiff::Zoned) -> String {
    let at = util::local_at_ms(mtime * 1000);
    let today = now.date();
    if at.date() == today {
        let h = at.hour();
        let (h12, half) = match h {
            0 => (12, "AM"),
            1..=11 => (h, "AM"),
            12 => (12, "PM"),
            _ => (h - 12, "PM"),
        };
        return format!("{h12}:{:02} {half}", at.minute());
    }
    if today.yesterday().ok() == Some(at.date()) {
        return "Yesterday".into();
    }
    if now.timestamp().as_second() - mtime < 6 * 86_400 {
        return match at.weekday() {
            Weekday::Monday => "Monday",
            Weekday::Tuesday => "Tuesday",
            Weekday::Wednesday => "Wednesday",
            Weekday::Thursday => "Thursday",
            Weekday::Friday => "Friday",
            Weekday::Saturday => "Saturday",
            Weekday::Sunday => "Sunday",
        }
        .into();
    }
    format!("{}/{}/{:02}", at.month(), at.day(), at.year() % 100)
}

fn fit(text: &str, cols: usize) -> String {
    cut(&text.replace(['\n', '\t'], " "), cols)
}

/// A row's name on the left and a number on the right.
fn counted(name: &str, count: usize, cols: usize) -> String {
    let number = count.to_string();
    let room = cols.saturating_sub(number.len() + 1);
    let shown = fit(name, room);
    format!(
        "{shown}{}{number}",
        " ".repeat(room.saturating_sub(width(&shown)) + 1)
    )
}

impl State {
    pub fn title(&self) -> String {
        if let Some(q) = &self.query {
            return format!("Search: {q}");
        }
        if let Some(t) = &self.tag {
            return format!("#{t}");
        }
        match self.folder.as_deref() {
            None => "All Notes".into(),
            Some("") => "Notes".into(),
            Some(TRASH) => "Recently Deleted".into(),
            Some(f) => f.rsplit('/').next().unwrap_or(f).to_string(),
        }
    }

    /// The notes shown.
    pub fn notes(&self, store: &Store) -> Vec<NoteEntry> {
        if let Some(q) = &self.query {
            return store.search(q);
        }
        if let Some(t) = &self.tag {
            return store.tagged(t);
        }
        match self.folder.as_deref() {
            Some("") => store
                .notes(None)
                .into_iter()
                .filter(|n| n.folder.is_empty())
                .collect(),
            f => store.notes(f),
        }
    }

    /// Shows a folder, a tag or a search.
    pub fn show(&mut self, folder: Option<String>, tag: Option<String>, query: Option<String>) {
        self.folder = folder;
        self.tag = tag;
        self.query = query;
    }

    /// The folder a new note goes in: the one shown, or the top folder for
    /// All Notes, a tag, a search or Recently Deleted.
    pub fn here(&self) -> String {
        if self.tag.is_some() || self.query.is_some() {
            return String::new();
        }
        match self.folder.as_deref() {
            None | Some(TRASH) => String::new(),
            Some(f) => f.to_string(),
        }
    }

    /// The list's rows for a list `cols` wide, the note open (`current`)
    /// marked.
    pub fn build(&self, store: &Store, current: Option<&Path>, cols: u16) -> Vec<Row> {
        let w = cols.saturating_sub(2) as usize;
        let mut rows = Vec::new();
        let add = |rows: &mut Vec<Row>, text: String, look: Look, item: Option<Item>| {
            rows.push(Row {
                text: format!(" {text}"),
                look,
                item,
            });
        };
        let all = store.notes(None);
        let plain = self.tag.is_none() && self.query.is_none();
        let current_if = |on: bool| if on { Look::Current } else { Look::Plain };
        add(&mut rows, "Folders".into(), Look::Header, None);
        add(
            &mut rows,
            counted("  All Notes", all.len(), w),
            current_if(plain && self.folder.is_none()),
            Some(Item::Folder(None)),
        );
        let top = all.iter().filter(|n| n.folder.is_empty()).count();
        add(
            &mut rows,
            counted("  Notes", top, w),
            current_if(plain && self.folder.as_deref() == Some("")),
            Some(Item::Folder(Some(String::new()))),
        );
        for folder in store.folders() {
            let on = plain && self.folder.as_deref() == Some(folder.rel.as_str());
            add(
                &mut rows,
                counted(
                    &format!("{}{}", "  ".repeat(folder.depth + 2), folder.name),
                    folder.count,
                    w,
                ),
                current_if(on),
                Some(Item::Folder(Some(folder.rel))),
            );
        }
        add(
            &mut rows,
            counted("  Recently Deleted", store.notes(Some(TRASH)).len(), w),
            current_if(plain && self.folder.as_deref() == Some(TRASH)),
            Some(Item::Folder(Some(TRASH.into()))),
        );
        let tags = store.tags();
        if !tags.is_empty() {
            add(&mut rows, String::new(), Look::Plain, None);
            add(&mut rows, "Tags".into(), Look::Header, None);
            for (name, count) in tags {
                let on = self
                    .tag
                    .as_deref()
                    .is_some_and(|t| t.to_lowercase() == name.to_lowercase());
                add(
                    &mut rows,
                    counted(&format!("  #{name}"), count, w),
                    if on { Look::Current } else { Look::Tag },
                    Some(Item::Tag(name)),
                );
            }
        }
        add(&mut rows, String::new(), Look::Plain, None);
        add(&mut rows, "─".repeat(w), Look::Rule, None);
        let notes = self.notes(store);
        let count = format!(
            "{} {}",
            notes.len(),
            if notes.len() == 1 { "note" } else { "notes" }
        );
        let title = fit(&self.title(), w.saturating_sub(8));
        add(
            &mut rows,
            format!(
                "{title}{}{count:>8}",
                " ".repeat(w.saturating_sub(8).saturating_sub(width(&title)))
            ),
            Look::Header,
            None,
        );
        if notes.is_empty() {
            add(&mut rows, String::new(), Look::Plain, None);
            add(
                &mut rows,
                if self.query.is_some() {
                    "  No notes match.".into()
                } else {
                    "  No notes. Press n to make one.".into()
                },
                Look::Faint,
                None,
            );
        }
        let now = util::local_now();
        let mut section: Option<&str> = None;
        for note in &notes {
            let this = if note.pinned { "Pinned" } else { "Notes" };
            if section != Some(this) && (note.pinned || section == Some("Pinned")) {
                add(&mut rows, String::new(), Look::Plain, None);
                add(&mut rows, this.into(), Look::Section, None);
            } else if section.is_none() {
                add(&mut rows, String::new(), Look::Plain, None);
            }
            section = Some(this);
            let item = Some(Item::Note(note.path.clone()));
            let open = current.is_some_and(|c| c == note.path);
            let marked = self.marked.contains(&note.path);
            let mark = if marked {
                "✓ "
            } else if note.pinned {
                "● "
            } else {
                "  "
            };
            add(
                &mut rows,
                fit(&format!("{mark}{}", note.title), w),
                if open { Look::Open } else { Look::Title },
                item.clone(),
            );
            let mut detail = match (&self.query, &note.matched) {
                (Some(_), Some(m)) => m.clone(),
                _ => note.preview.clone(),
            };
            if plain && self.folder.is_none() && !note.folder.is_empty() {
                detail = format!("▸ {}  {detail}", note.folder);
            }
            add(
                &mut rows,
                fit(&format!("  {}  {detail}", when(note.mtime, &now)), w),
                Look::Detail,
                item,
            );
            add(&mut rows, String::new(), Look::Plain, None);
        }
        rows
    }
}

/// The note shown after `path` goes from the list: the next one listed.
pub fn next_note(notes: &[NoteEntry], path: &Path) -> Option<PathBuf> {
    notes
        .iter()
        .find(|n| n.path != path)
        .map(|n| n.path.clone())
}

/// The list's keys, for `?`.
pub const HELP: &[&str] = &[
    "Enter or click   open a note, folder or tag",
    "n                new note in this folder",
    "N                new folder",
    "dd               delete (to Recently Deleted)",
    "r                restore from Recently Deleted",
    "p                pin or unpin",
    "t                its tab: go there, or open one",
    "s  S             mark notes, squash them into one",
    "m                move to a folder",
    "R                rename a note or folder",
    "/                search every note",
    "Ctrl-F           find a note",
    "Ctrl-B           put the list away (Ctrl-B again",
    "                 in a note shows it)",
    "Ctrl-Shift-P     every command, in a palette",
    "x                clear the search or tag",
    "Tab              go to the note",
    "q                quit",
    "",
    "In a note, Ctrl-P opens the Format menu.",
];

/// A note's title, read from its file.
pub fn title_at(path: &Path) -> String {
    store::title_of(&util::read_text(path).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::store::tests::temp;

    #[test]
    fn the_list_shows_folders_tags_and_the_notes_in_what_is_chosen() {
        let (_dir, mut s) = temp("sidebar");
        let a = s.create("", Some("Groceries"));
        std::fs::write(&a, "# Groceries\n\n- [ ] eggs #food\n").unwrap();
        s.create("Work", Some("Plan"));
        let mut state = State::default();
        let rows = state.build(&s, Some(&a), WIDTH);
        let text: Vec<&str> = rows.iter().map(|r| r.text.as_str()).collect();
        assert!(
            text.iter()
                .any(|l| l.starts_with("   All Notes") && l.ends_with(" 2")),
            "{text:?}"
        );
        assert!(text.iter().any(|l| l.contains("Work") && l.ends_with(" 1")));
        assert!(text.iter().any(|l| l.contains("#food")));
        let open = rows.iter().find(|r| r.look == Look::Open).unwrap();
        assert!(open.text.contains("Groceries"));
        let detail = rows.iter().find(|r| r.text.contains("▸ Work")).unwrap();
        assert_eq!(
            detail.look,
            Look::Detail,
            "in All Notes, which folder a note is in"
        );
        state.show(None, Some("food".into()), None);
        let rows = state.build(&s, None, WIDTH);
        assert_eq!(
            rows.iter().filter(|r| r.look == Look::Title).count(),
            1,
            "a tag shows the notes that have it"
        );
        state.show(None, None, Some("nothing like it".into()));
        let rows = state.build(&s, None, WIDTH);
        assert!(rows.iter().any(|r| r.text.contains("No notes match.")));
        assert_eq!(state.here(), "");
        state.show(Some("Work".into()), None, None);
        assert_eq!(state.here(), "Work");
    }

    #[test]
    fn a_notes_time_reads_as_notes_says_it() {
        let now = util::local_now();
        let secs = now.timestamp().as_second();
        assert!(when(secs, &now).contains(':'));
        assert_eq!(when(secs - 86_400 * 30, &now).matches('/').count(), 2);
    }
}
