//! The last screen of a tab that closed, for the Reopen view's "Last
//! screen" (board 18). New with the Reopen view, with no tmux-version file
//! behind it.
//!
//! As a tab closes, its Claude's pane (else its active one) is kept as a
//! `Shot`: the text of its last lines, which the view writes as text
//! (`ui::images::screen`), leaving out the lines that are only a frame, so
//! it reads as what was said and run. Each is a file of `closed-screens/`
//! in the state directory, written private, named after its closing, and
//! taken out with it when the closing leaves the log
//! (`closed::Closed::push`). It is never sent anywhere. A file an older
//! build wrote, with each line's runs of color and its width, reads the
//! same, those left out.

use std::path::{Path, PathBuf};

use ratatui::text::Span;
use serde::{Deserialize, Serialize};

use crate::util;

/// The lines of text kept, enough that a screen's frame left out still
/// leaves the box's rows.
pub const TEXT_LINES: usize = 16;

/// A screen as kept: its last lines.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Shot {
    pub lines: Vec<ShotLine>,
}

/// A line of a shot: its text, its trailing blanks left out.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ShotLine {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
}

/// Whether a character only draws a frame: a box's line or a block.
fn frame_char(c: char) -> bool {
    ('\u{2500}'..='\u{259f}').contains(&c)
}

impl Shot {
    /// A pane's screen as `Pane::styled_lines` reads it: its last
    /// `TEXT_LINES` lines.
    pub fn of(lines: &[Vec<Span<'static>>]) -> Shot {
        let from = lines.len().saturating_sub(TEXT_LINES);
        let lines = lines[from..]
            .iter()
            .map(|spans| ShotLine {
                text: spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
                    .trim_end()
                    .to_string(),
            })
            .collect();
        Shot { lines }
    }

    /// A pane's screen as it is now; none when it shows nothing.
    pub fn of_pane(pane: &crate::mux::pane::Pane) -> Option<Shot> {
        let lines = pane.styled_lines();
        (!lines.is_empty()).then(|| Shot::of(&lines))
    }

    /// Its last `n` lines with words, each without the frame at its edges
    /// (a box's sides, a rule); a line that is only frame or blank is left
    /// out.
    pub fn last_words(&self, n: usize) -> Vec<&str> {
        let words: Vec<&str> = self
            .lines
            .iter()
            .filter_map(|l| {
                let text = l
                    .text
                    .trim_end_matches(|c: char| c.is_whitespace() || frame_char(c));
                let edged = text.trim_start().starts_with(frame_char);
                let text = if edged {
                    text.trim_start_matches(|c: char| c.is_whitespace() || frame_char(c))
                } else {
                    text
                };
                (!text.trim().is_empty()).then_some(text)
            })
            .collect();
        let from = words.len().saturating_sub(n);
        words[from..].to_vec()
    }
}

/// Where the shots are kept.
pub fn dir(state: &Path) -> PathBuf {
    state.join("closed-screens")
}

/// A shot's file, by the name its closing keeps; never a path out of the
/// directory.
fn file(state: &Path, name: &str) -> Option<PathBuf> {
    let safe = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        && !name.starts_with('.');
    safe.then(|| dir(state).join(name))
}

/// Keeps a shot under `name`, private.
pub fn write(state: &Path, name: &str, shot: &Shot) {
    let Some(path) = file(state, name) else {
        return;
    };
    let _ = std::fs::create_dir_all(dir(state));
    let tmp = path.with_extension("tmp");
    if util::write_private(&tmp, util::json_compact(shot), false).is_ok()
        && std::fs::rename(&tmp, &path).is_err()
    {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// A shot kept under `name`, when it is there.
pub fn read(state: &Path, name: &str) -> Option<Shot> {
    let text = util::read_text(file(state, name)?)?;
    serde_json::from_str(&text).ok()
}

/// Takes a shot out.
pub fn remove(state: &Path, name: &str) {
    if let Some(path) = file(state, name) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_screen_keeps_its_last_lines_and_shows_their_words_without_the_frame() {
        let mut lines: Vec<Vec<Span<'static>>> = (0..20)
            .map(|n| vec![Span::raw(format!("line {n}"))])
            .collect();
        lines.extend([
            vec![Span::raw("─".repeat(30))],
            vec![Span::raw("│ > make it faster   │  ")],
            vec![Span::raw("╰──────────╯")],
            vec![Span::raw("   ")],
            vec![Span::raw("  ? for shortcuts")],
        ]);
        let shot = Shot::of(&lines);
        assert_eq!(shot.lines.len(), TEXT_LINES);
        assert_eq!(
            shot.last_words(3),
            vec!["line 19", "> make it faster", "  ? for shortcuts"]
        );
        let frame = Shot::of(&[vec![Span::raw("╭──╮")], vec![Span::raw(" ")]]);
        assert!(frame.last_words(4).is_empty());
    }

    #[test]
    fn a_shot_is_written_read_and_removed_by_its_name_alone() {
        let state = std::env::temp_dir().join(format!("sb-shots-{}", std::process::id()));
        let shot = Shot {
            lines: vec![ShotLine { text: "abc".into() }],
        };
        write(&state, "1-2.json", &shot);
        assert_eq!(read(&state, "1-2.json"), Some(shot));
        write(&state, "../out.json", &Shot::default());
        assert!(!state.join("out.json").exists());
        remove(&state, "1-2.json");
        assert_eq!(read(&state, "1-2.json"), None);
        let _ = std::fs::remove_dir_all(&state);
    }
}
