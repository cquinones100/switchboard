//! The notes written for you without asking (the meeting notes from Fathom,
//! the standup note, today's to-do list), for the sidebar's notifications
//! until you read one. Ported from lib/sidebar/new-notes.js: each is kept in
//! `new-notes.json` in the state directory as `{ path, title, source, at }`,
//! its path within the notes directory, and is let go once Notes shows it
//! (`.notes.json`'s `last`), its file is gone, or its menu dismisses it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::items::{Item, State};
use crate::util;

const MAX: usize = 50;

/// A note written for you.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewNote {
    /// Within the notes directory.
    pub path: String,
    pub title: String,
    /// What wrote it ("Fathom", "the standup").
    pub source: String,
    pub at: i64,
}

fn file(state: &Path) -> PathBuf {
    state.join("new-notes.json")
}

fn read(state: &Path) -> Vec<NewNote> {
    util::read_json(file(state))
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

fn write(state: &Path, notes: &[NewNote]) {
    let skip = notes.len().saturating_sub(MAX);
    let _ = util::write_atomic(file(state), util::json_compact(&notes[skip..].to_vec()));
}

/// A note just written, by its path, and what wrote it.
pub fn note_written(state: &Path, notes_dir: &Path, note: &Path, source: &str) {
    let first = util::read_text(note)
        .and_then(|t| t.lines().next().map(str::to_string))
        .unwrap_or_default();
    let title = first.trim_start_matches('#').trim_start().to_string();
    let title = if title.is_empty() {
        note.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        title
    };
    let rel = note
        .strip_prefix(notes_dir)
        .unwrap_or(note)
        .to_string_lossy()
        .into_owned();
    let mut notes: Vec<NewNote> = read(state).into_iter().filter(|n| n.path != rel).collect();
    notes.push(NewNote {
        path: rel,
        title,
        source: source.to_string(),
        at: util::now_ms(),
    });
    write(state, &notes);
}

/// The ones not yet read, newest first.
pub fn new_notes(state: &Path, notes_dir: &Path) -> Vec<NewNote> {
    let shown = util::read_json(notes_dir.join(".notes.json"))
        .and_then(|v| v.get("last").and_then(|l| l.as_str()).map(str::to_string));
    let notes = read(state);
    let left: Vec<NewNote> = notes
        .iter()
        .filter(|n| Some(&n.path) != shown.as_ref() && notes_dir.join(&n.path).exists())
        .cloned()
        .collect();
    if left.len() != notes.len() {
        write(state, &left);
    }
    left.into_iter().rev().collect()
}

/// Lets one go, from its menu.
pub fn dismiss_note(state: &Path, rel: &str) {
    let notes: Vec<NewNote> = read(state).into_iter().filter(|n| n.path != rel).collect();
    write(state, &notes);
}

/// The notes as notifications, one each, their `data` naming the note.
pub fn note_notices(notes: &[NewNote]) -> Vec<Item> {
    notes
        .iter()
        .map(|n| {
            Item::new(format!("note:{}", n.path), &n.title)
                .detail(format!("Written from {} into {}", n.source, n.path))
                .state(State::Active)
                .time(n.at)
                .data(json!({ "kind": "note", "path": n.path, "source": n.source }))
        })
        .collect()
}
