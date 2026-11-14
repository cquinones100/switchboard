//! The list on the left, as in Notes, ported from
//! notes/lua/notes/sidebar.lua: the notes opened lately (Recent, while
//! Settings' `notes_recent` is on), the folders, then the notes in what is
//! chosen, pinned ones first, then newest first, each as its title and a
//! line of when it changed and how it starts. A click or Enter on a folder
//! shows its notes; on a note, opens it. There is no All Notes and no list
//! of tags, which grew long and went unused: `/` searches every note, and a
//! `#tag` followed in a note (or typed into `/`) lists the notes that have
//! it. A click or Enter on Recent's heading ("all 20 ›"), or "Open a recent
//! note" in the palette, opens the whole history in the finder's box.
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
    /// A folder: "" for the top folder, `TRASH` for Recently Deleted.
    Folder(String),
    Note(PathBuf),
    /// Every note opened lately, in the finder's box (Recent's heading).
    Recent,
}

/// How a row is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Plain,
    Header,
    Section,
    /// The folder shown.
    Current,
    /// The note open.
    Open,
    Title,
    Detail,
    Rule,
    Faint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub text: String,
    pub look: Look,
    pub item: Option<Item>,
    /// Where the text turns dim, for what is at a row's right (a recent
    /// note's folder).
    pub dim_at: Option<usize>,
}

impl Row {
    /// The note a row of the notes shown is for: its title's row, never
    /// its detail line or its row under Recent, so the cursor put on a note
    /// lands where the view lists it.
    pub fn listed(&self) -> Option<&Path> {
        match &self.item {
            Some(Item::Note(path)) if matches!(self.look, Look::Title | Look::Open) => Some(path),
            _ => None,
        }
    }
}

/// How many of the notes opened lately the list shows under Recent.
pub const RECENT_ROWS: usize = 8;

/// What is shown: a folder ("" the top one, which the list opens on), or
/// the notes with a tag, or those matching a search, which keep the folder
/// to come back to; the notes marked with s for squashing, in the order
/// marked; and whether Recent is listed (`notes_recent`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub folder: String,
    pub tag: Option<String>,
    pub query: Option<String>,
    pub marked: Vec<PathBuf>,
    pub recent: bool,
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
        match self.folder.as_str() {
            "" => "Notes".into(),
            TRASH => "Recently Deleted".into(),
            f => f.rsplit('/').next().unwrap_or(f).to_string(),
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
        match self.folder.as_str() {
            "" => store
                .notes(None)
                .into_iter()
                .filter(|n| n.folder.is_empty())
                .collect(),
            f => store.notes(Some(f)),
        }
    }

    /// Shows a folder (`None` keeps the one shown), a tag or a search.
    pub fn show(&mut self, folder: Option<String>, tag: Option<String>, query: Option<String>) {
        if let Some(folder) = folder {
            self.folder = folder;
        }
        self.tag = tag;
        self.query = query;
    }

    /// The folder a new note goes in: the one shown, or the top folder for
    /// a tag, a search or Recently Deleted.
    pub fn here(&self) -> String {
        if self.tag.is_some() || self.query.is_some() || self.folder == TRASH {
            return String::new();
        }
        self.folder.clone()
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
                dim_at: None,
            });
        };
        let all = store.notes(None);
        let plain = self.tag.is_none() && self.query.is_none();
        let current_if = |on: bool| if on { Look::Current } else { Look::Plain };
        let recent = if self.recent {
            store.recent()
        } else {
            Vec::new()
        };
        if !recent.is_empty() {
            let all = format!("all {} ›", recent.len());
            add(
                &mut rows,
                format!(
                    "Recent{}",
                    " ".repeat(w.saturating_sub(6 + width(&all))) + &all
                ),
                Look::Header,
                Some(Item::Recent),
            );
            for note in recent.iter().take(RECENT_ROWS) {
                let folder = if note.folder.is_empty() {
                    "Notes"
                } else {
                    note.folder.rsplit('/').next().unwrap_or(&note.folder)
                };
                let marked = self.marked.contains(&note.path);
                let (text, dim_at) = beside(&note.title, folder, marked, w);
                rows.push(Row {
                    text: format!(" {text}"),
                    look: Look::Plain,
                    item: Some(Item::Note(note.path.clone())),
                    dim_at: dim_at.map(|at| at + 1),
                });
            }
            add(&mut rows, String::new(), Look::Plain, None);
        }
        add(&mut rows, "Folders".into(), Look::Header, None);
        let top = all.iter().filter(|n| n.folder.is_empty()).count();
        add(
            &mut rows,
            counted("  Notes", top, w),
            current_if(plain && self.folder.is_empty()),
            Some(Item::Folder(String::new())),
        );
        for folder in store.folders() {
            let on = plain && self.folder == folder.rel;
            add(
                &mut rows,
                counted(
                    &format!("{}{}", "  ".repeat(folder.depth + 2), folder.name),
                    folder.count,
                    w,
                ),
                current_if(on),
                Some(Item::Folder(folder.rel)),
            );
        }
        add(
            &mut rows,
            counted("  Recently Deleted", store.notes(Some(TRASH)).len(), w),
            current_if(plain && self.folder == TRASH),
            Some(Item::Folder(TRASH.into())),
        );
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
            if !plain && !note.folder.is_empty() {
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

/// A recent note's row, as board 69 draws it: its title (its first 20
/// columns) on the left and, dim at the right, its folder, cut to the room
/// left, a ✓ before it when it is marked for squashing. The text, and the
/// byte the dim part starts at.
fn beside(title: &str, folder: &str, marked: bool, cols: usize) -> (String, Option<usize>) {
    let mark = if marked { "✓ " } else { "  " };
    let shown = format!("{mark}{}", fit(title, 20));
    let room = cols.saturating_sub(width(&shown) + 2);
    let place = fit(folder, room);
    if place.is_empty() {
        return (shown, None);
    }
    let left = format!(
        "{shown}{}",
        " ".repeat(cols.saturating_sub(width(&shown) + width(&place)))
    );
    let at = left.len();
    (format!("{left}{place}"), Some(at))
}

/// The note shown after `path` goes from the list: the next one listed.
pub fn next_note(notes: &[NoteEntry], path: &Path) -> Option<PathBuf> {
    notes
        .iter()
        .find(|n| n.path != path)
        .map(|n| n.path.clone())
}

/// The list's keys, for `?` and mission control's Keys view.
pub const KEYS: &[(&str, &str)] = &[
    (
        "Enter or click",
        "open a note or folder, or Recent's history",
    ),
    ("n", "new note in this folder"),
    ("N", "new folder"),
    ("dd", "delete (to Recently Deleted)"),
    ("r", "restore from Recently Deleted"),
    ("p", "pin or unpin"),
    ("t", "its tab: go there, or open one"),
    ("s  S", "mark notes, squash them into one"),
    ("m", "move to a folder"),
    ("R", "rename a note or folder"),
    ("/", "search every note"),
    ("Ctrl-F", "find a note"),
    (
        "Ctrl-B",
        "put the list away (Ctrl-B again in a note shows it)",
    ),
    ("Ctrl-Shift-P", "every command, in a palette"),
    ("x", "clear the search or tag"),
    ("Tab", "go to the note"),
    ("q", "quit"),
];

/// The keys' column, and how wide what each does may run before it wraps.
const KEYS_WIDTH: usize = 17;
const DOES_WIDTH: usize = 31;

/// What `?` shows: the list's keys in two columns, what each does wrapped
/// under itself, then where the Format menu is.
pub fn help_text() -> String {
    let mut lines = Vec::new();
    for (keys, does) in KEYS {
        let mut line = format!("{keys:<KEYS_WIDTH$}");
        let mut used = 0;
        for word in does.split(' ') {
            let w = width(word);
            if used > 0 && used + 1 + w > DOES_WIDTH {
                lines.push(line);
                line = " ".repeat(KEYS_WIDTH);
                used = 0;
            }
            if used > 0 {
                line.push(' ');
                used += 1;
            }
            line.push_str(word);
            used += w;
        }
        lines.push(line);
    }
    lines.push(String::new());
    lines.push("In a note, Ctrl-P opens the Format menu.".into());
    lines.join("\n")
}

/// A note's title, read from its file.
pub fn title_at(path: &Path) -> String {
    store::title_of(&util::read_text(path).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::store::tests::temp;

    #[test]
    fn the_list_shows_folders_and_the_notes_in_what_is_chosen() {
        let (_dir, mut s) = temp("sidebar");
        let a = s.create("", Some("Groceries"));
        std::fs::write(&a, "# Groceries\n\n- [ ] eggs #food\n").unwrap();
        s.create("Work", Some("Plan"));
        let mut state = State::default();
        let rows = state.build(&s, Some(&a), WIDTH);
        let text: Vec<&str> = rows.iter().map(|r| r.text.as_str()).collect();
        assert!(
            !text.iter().any(|l| l.contains("All Notes")
                || l.contains("Tags")
                || l.trim_start().starts_with('#')),
            "no All Notes and no tags: {text:?}"
        );
        let top = rows
            .iter()
            .find(|r| r.item == Some(Item::Folder(String::new())))
            .unwrap();
        assert_eq!(top.look, Look::Current, "it opens on the top folder");
        assert!(top.text.ends_with(" 1"));
        assert!(text.iter().any(|l| l.contains("Work") && l.ends_with(" 1")));
        let open = rows.iter().find(|r| r.look == Look::Open).unwrap();
        assert!(open.text.contains("Groceries"));
        state.show(None, None, Some("plan".into()));
        let rows = state.build(&s, None, WIDTH);
        let detail = rows.iter().find(|r| r.text.contains("▸ Work")).unwrap();
        assert_eq!(
            detail.look,
            Look::Detail,
            "in a search, which folder a note is in"
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
        state.show(None, Some("food".into()), None);
        state.show(None, None, None);
        assert_eq!(
            state.folder, "Work",
            "clearing a tag goes back to the folder"
        );
    }

    #[test]
    fn recent_lists_the_notes_opened_lately_newest_first() {
        let (_dir, mut s) = temp("recent");
        let a = s.create("", Some("Groceries"));
        let b = s.create("Work", Some("Plan"));
        let c = s.create("", Some("Trip"));
        for path in [&a, &b, &c, &a] {
            s.opened(&s.relative(path));
        }
        let mut state = State {
            recent: true,
            ..State::default()
        };
        let recent = |state: &State, s: &Store| -> Vec<Row> {
            let rows = state.build(s, None, WIDTH);
            let Some(at) = rows
                .iter()
                .position(|r| r.look == Look::Header && r.text.trim().starts_with("Recent"))
            else {
                return Vec::new();
            };
            rows[at + 1..]
                .iter()
                .take_while(|r| r.item.is_some())
                .cloned()
                .collect()
        };
        let rows = recent(&state, &s);
        let titles: Vec<String> = rows
            .iter()
            .map(|r| r.text.split("  ").nth(1).unwrap_or("").trim().to_string())
            .collect();
        assert_eq!(
            titles,
            ["Groceries", "Trip", "Plan"],
            "newest first, each once"
        );
        assert!(rows[0].text.ends_with("Notes"), "the top folder named");
        let heading = state.build(&s, None, WIDTH);
        let heading = heading
            .iter()
            .find(|r| r.look == Look::Header && r.text.trim().starts_with("Recent"))
            .unwrap();
        assert_eq!(
            heading.item,
            Some(Item::Recent),
            "a click on it opens the history"
        );
        assert!(heading.text.ends_with("all 3 ›"));
        state.marked.push(b.clone());
        let rows = recent(&state, &s);
        assert!(rows[2].text.starts_with(" ✓ Plan"), "marked for squashing");
        state.marked.clear();
        s.set_pinned(&s.relative(&b), true);
        assert!(
            s.recent().iter().any(|n| n.path == b && n.pinned),
            "a pinned note keeps its pin in the history"
        );
        let plan = &rows[2];
        let at = plan.dim_at.expect("its folder at the right");
        assert_eq!(plan.text[at..].trim(), "Work");
        s.delete(&c);
        assert_eq!(recent(&state, &s).len(), 2, "a deleted note dropped");
        let trashed = s.absolute(".trash/Trip.md");
        for _ in 0..crate::notes::store::RECENT + 1 {
            s.opened(&s.relative(&trashed));
        }
        let stored = s.meta()["recent"].to_string();
        assert!(
            !stored.contains(".trash"),
            "nothing in Recently Deleted kept in the history: {stored}"
        );
        state.recent = false;
        assert!(recent(&state, &s).is_empty(), "notes_recent off: no Recent");
    }

    #[test]
    fn the_lists_keys_read_in_two_columns_a_long_one_wrapped_under_itself() {
        let text = help_text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Enter or click   open a note or folder, or");
        let at = lines.iter().position(|l| l.starts_with("Ctrl-B")).unwrap();
        assert_eq!(
            lines[at],
            "Ctrl-B           put the list away (Ctrl-B again"
        );
        assert_eq!(lines[at + 1], "                 in a note shows it)");
        assert_eq!(
            lines.last(),
            Some(&"In a note, Ctrl-P opens the Format menu.")
        );
    }

    #[test]
    fn a_notes_time_reads_as_notes_says_it() {
        let now = util::local_now();
        let secs = now.timestamp().as_second();
        assert!(when(secs, &now).contains(':'));
        assert_eq!(when(secs - 86_400 * 30, &now).matches('/').count(), 2);
    }
}
