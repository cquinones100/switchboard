//! `watch-note`: Watch a note for new items. Ported from `claude/skills/note-watch/scripts/watch-note.js`.

use std::process::ExitCode;

pub fn main(_args: Vec<String>) -> ExitCode {
    super::not_yet("watch-note")
}
