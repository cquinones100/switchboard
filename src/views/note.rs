//! A note's preview (`switchboard view --note <file>`): the note read-only,
//! drawn as the PR view draws Markdown, which M-p's "Preview a note…" and
//! Notes' "Preview this note" open in a pane's place
//! (`notes::serve::preview`). Notes is an editor, its markup a keypress
//! away and every key an edit; this is the note as it reads on GitHub.
//!
//! Its stable ID line is left out (`id::strip`), and no picture is fetched
//! from the web (`pager::run_local`), since nothing of a note leaves the
//! computer. It follows the note as it changes: the file's modified time
//! and length are looked at every `EVERY`, and a change reads it again,
//! the pager keeping the reader's place as the PR view does on a reread
//! (`Pager::set_content`). New with the preview, with no tmux-version file
//! behind it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant, SystemTime};

use super::pager::{self, Content};
use crate::notes::id;

/// How often the note's file is looked at for a change.
const EVERY: Duration = Duration::from_millis(300);

/// How long a note whose file is gone (trashed, deleted) waits between
/// searches of the notes directory for its ID, after the first.
const SEARCH_AGAIN: Duration = Duration::from_secs(5);

/// What a note's diagram that can't be drawn here points to.
const PLACE: &str = "the note in Notes";

/// The note's text as the preview shows it.
pub fn content_of(text: &str) -> Content {
    Content::Markdown {
        text: id::strip(text),
        place: PLACE.into(),
    }
}

/// The note at `file`, read now.
fn read(file: &Path) -> Content {
    match std::fs::read_to_string(file) {
        Ok(text) => content_of(&text),
        Err(e) => Content::Failed(format!("Couldn't read the note: {e}")),
    }
}

/// What says the file changed: its modified time and length.
fn stamp(file: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(file).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// The file in the notes directory `notes` whose note has the ID `id`.
fn found_by_id(notes: &Path, id: &str) -> Option<PathBuf> {
    crate::notes::store::Store::at(notes.to_path_buf())
        .notes(None)
        .into_iter()
        .map(|note| note.path)
        .find(|path| {
            std::fs::read_to_string(path)
                .ok()
                .and_then(|text| id::in_text(&text))
                .as_deref()
                == Some(id)
        })
}

/// Reads the note again each time its file changes, until the preview is
/// gone. Notes renames a note's file as its title changes, so a file
/// gone is looked for again by the note's stable ID, in the notes
/// directory `notes`, and followed where it went: once as it goes, then
/// every `SEARCH_AGAIN` while it stays gone, since a search reads every
/// note.
fn follow(mut file: PathBuf, notes: PathBuf, tx: Sender<Content>) {
    let id = std::fs::read_to_string(&file)
        .ok()
        .and_then(|text| id::in_text(&text));
    let mut last = stamp(&file);
    let mut searched: Option<Instant> = None;
    loop {
        std::thread::sleep(EVERY);
        let mut now = stamp(&file);
        let search_due = searched.is_none_or(|at| at.elapsed() >= SEARCH_AGAIN);
        if now.is_none()
            && search_due
            && let Some(id) = id.as_deref()
        {
            searched = Some(Instant::now());
            if let Some(moved) = found_by_id(&notes, id) {
                file = moved;
                now = stamp(&file);
            }
        }
        if now.is_some() {
            searched = None;
        }
        if now != last {
            last = now;
            if tx.send(read(&file)).is_err() {
                return;
            }
        }
    }
}

/// The name the preview goes by: the note's path in the notes directory,
/// without `.md`.
fn name_of(file: &Path) -> String {
    let notes = crate::notes::store::notes_dir();
    let rel = file
        .strip_prefix(&notes)
        .unwrap_or(file)
        .to_string_lossy()
        .into_owned();
    rel.strip_suffix(".md").unwrap_or(&rel).to_string()
}

/// `switchboard view --note <file>`.
pub fn main(file: &str) -> ExitCode {
    let file = PathBuf::from(file);
    let (tx, rx) = channel();
    let watched = file.clone();
    let notes = crate::notes::store::notes_dir();
    std::thread::spawn(move || follow(watched, notes, tx));
    pager::run_local_named(read(&file), rx, format!("Preview of {}", name_of(&file)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::pager::Pager;

    const NOTE: &str = "# Weekend Plans\n\n- one\n- two\n\n| Day | What |\n| --- | --- |\n| Sat | Hike |\n\n<!-- note-id: n-0123456789ab -->\n";

    #[test]
    fn a_note_is_drawn_as_the_views_draw_markdown_without_its_id_line() {
        let pager = Pager::new(content_of(NOTE), 60, 20);
        let screen = pager.screen().join("\n");
        assert!(screen.contains("Weekend Plans"), "{screen}");
        assert!(!screen.contains("# Weekend"), "the heading's markup hidden");
        assert!(
            screen.contains("• one") || screen.contains("· one"),
            "{screen}"
        );
        assert!(
            screen.contains("Sat") && screen.contains("Hike"),
            "{screen}"
        );
        assert!(screen.contains('│'), "the table drawn with rules: {screen}");
        assert!(!screen.contains("note-id"), "{screen}");
        assert_eq!(
            pager.screen(),
            Pager::new(
                Content::Markdown {
                    text: NOTE.replace("<!-- note-id: n-0123456789ab -->\n", ""),
                    place: PLACE.into()
                },
                60,
                20
            )
            .screen(),
            "as the views draw the same Markdown"
        );
    }

    #[test]
    fn a_change_to_the_note_is_drawn_where_the_reader_was() {
        let long: String = (1..=80).map(|n| format!("- item {n}\n")).collect();
        let mut pager = Pager::new(content_of(&format!("# Long\n\n{long}")), 40, 10);
        for _ in 0..30 {
            pager.key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('j'),
                crossterm::event::KeyModifiers::NONE,
            ));
        }
        let top = pager.screen()[0].clone();
        assert!(top.contains("item"), "{top:?}");
        let changed = long.replace("- item 79\n", "- item 79, changed\n");
        pager.set_content(content_of(&format!("# Long\n\n{changed}")));
        assert_eq!(pager.screen()[0], top, "the place kept");
        pager.key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('G'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(
            pager.screen().join("\n").contains("item 79, changed"),
            "the change drawn"
        );
    }

    #[test]
    fn the_watch_reads_the_note_again_when_its_file_changes() {
        let dir = std::env::temp_dir().join(format!("note-preview-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("Plans.md");
        std::fs::write(&file, "# Plans\n").unwrap();
        let (tx, rx) = channel();
        let watched = file.clone();
        let notes = dir.clone();
        std::thread::spawn(move || follow(watched, notes, tx));
        let wait = Duration::from_secs(5) * crate::demo::tour::patience();
        std::thread::sleep(EVERY * 2);
        std::fs::write(&file, "# Plans\n\n- more of them\n").unwrap();
        let got = rx.recv_timeout(wait).expect("read again");
        assert_eq!(got, content_of("# Plans\n\n- more of them\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_note_renamed_as_its_title_changes_is_followed_by_its_id() {
        let dir = std::env::temp_dir().join(format!("note-renamed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Plans.md");
        let line = "<!-- note-id: n-0123456789ab -->";
        std::fs::write(&file, format!("# Plans\n\n{line}\n")).unwrap();
        let (tx, rx) = channel();
        let watched = file.clone();
        let notes = dir.clone();
        std::thread::spawn(move || follow(watched, notes, tx));
        let wait = Duration::from_secs(5) * crate::demo::tour::patience();
        std::thread::sleep(EVERY * 2);
        let renamed = format!("# Trips\n\n- the first\n\n{line}\n");
        std::fs::write(dir.join("Trips.md"), &renamed).unwrap();
        std::fs::remove_file(&file).unwrap();
        let got = rx.recv_timeout(wait).expect("found again");
        assert_eq!(got, content_of(&renamed));
        std::fs::write(
            dir.join("Trips.md"),
            format!("# Trips\n\n- the second\n\n{line}\n"),
        )
        .unwrap();
        let got = rx.recv_timeout(wait).expect("followed where it went");
        assert!(matches!(got, Content::Markdown { text, .. } if text.contains("the second")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
