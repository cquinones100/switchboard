//! `watch-note <note file>`: for the note-watch skill's Monitor, a line for
//! each checklist item added to a note while it runs, `NEW: <the item's
//! text>`, and nothing else unless the note goes away (`GONE: …`). Ported
//! from claude/skills/note-watch/scripts/watch-note.js. The items already
//! there when it starts are known, so only what is added after counts.
//!
//! An item counts once its text has stayed the same for the settle time, so
//! one still being typed is not reported half-written, and an empty `- [ ]`
//! never is. An item ticked or deleted is forgotten. Editing an item's text
//! reads as a new item. The note is read every two seconds and an item
//! settles after twenty, shorter in the tests through `NOTE_WATCH_POLL_MS`
//! and `NOTE_WATCH_SETTLE_MS`.

use std::collections::HashMap;
use std::path::Path;
use std::process::ExitCode;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use indexmap::IndexSet;
use regex::Regex;

const POLL_MS: u64 = 2_000;
const SETTLE_MS: u64 = 20_000;

static ITEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*[-*+]\s+\[ \]\s+(\S.*?)\s*$").unwrap());

/// A duration in milliseconds from the environment, else the default; zero
/// or anything not a number is the default, as `Number(x) || default` was.
fn ms_from_env(var: &str, default: u64) -> Duration {
    let ms = std::env::var(var)
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .map(|v| v as u64)
        .unwrap_or(default);
    Duration::from_millis(ms.max(1))
}

/// The note's unticked items, by their text, in the note's order, or `None`
/// when it can't be read.
pub fn open_items(file: &Path) -> Option<IndexSet<String>> {
    let text = std::fs::read_to_string(file).ok()?;
    Some(
        text.split('\n')
            .filter_map(|line| ITEM.captures(line).map(|m| m[1].to_string()))
            .collect(),
    )
}

pub fn main(args: Vec<String>) -> ExitCode {
    let Some(file) = args.first() else {
        eprintln!("Usage: watch-note <note file>");
        return ExitCode::from(1);
    };
    let path = Path::new(file);
    let poll = ms_from_env("NOTE_WATCH_POLL_MS", POLL_MS);
    let settle = ms_from_env("NOTE_WATCH_SETTLE_MS", SETTLE_MS);
    let Some(mut known) = open_items(path) else {
        println!("GONE: {file} can't be read");
        return ExitCode::from(1);
    };
    // Text first seen, and when, until it has settled.
    let mut pending: HashMap<String, Instant> = HashMap::new();
    loop {
        std::thread::sleep(poll);
        let now = Instant::now();
        let Some(items) = open_items(path) else {
            println!("GONE: {file} was moved or deleted");
            return ExitCode::SUCCESS;
        };
        let mut settled = Vec::new();
        for text in &items {
            if known.contains(text) {
                continue;
            }
            match pending.get(text) {
                None => {
                    pending.insert(text.clone(), now);
                }
                Some(first) if now.duration_since(*first) >= settle => settled.push(text.clone()),
                Some(_) => {}
            }
        }
        for text in settled {
            println!("NEW: {text}");
            pending.remove(&text);
            known.insert(text);
        }
        // Ticked, deleted, or typed further: no longer waiting to settle.
        pending.retain(|text, _| items.contains(text));
        known.retain(|text| items.contains(text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_unticked_items_with_words_count() {
        let dir = std::env::temp_dir().join(format!("watch-note-unit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let note = dir.join("n.md");
        std::fs::write(
            &note,
            "# N\n- [ ] one  \n* [ ] two\n  + [ ] nested\n- [x] done\n- [ ] \n- [ ]\ntext\n",
        )
        .unwrap();
        let items: Vec<_> = open_items(&note).unwrap().into_iter().collect();
        assert_eq!(items, ["one", "two", "nested"]);
        assert!(open_items(&dir.join("gone.md")).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
