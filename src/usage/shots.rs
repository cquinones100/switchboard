//! The last screen of a tab that closed, for the Reopen view's "Last
//! screen" (board 18). New with the Reopen view, with no tmux-version file
//! behind it.
//!
//! As a tab closes, its Claude's pane (else its active one) is kept as a
//! `Shot`: each line as runs of a width and a color, what the picture atom
//! draws as rounded bars (`ui::images::raster_screen`), and the text of its
//! last few lines, its text form where the terminal draws no images. Each
//! is a file of `closed-screens/` in the state directory, written private,
//! named after its closing, and taken out with it when the closing leaves
//! the log (`closed::Closed::push`). It is never sent anywhere.

use std::path::{Path, PathBuf};

use ratatui::style::Color;
use ratatui::text::Span;
use serde::{Deserialize, Serialize};

use crate::util;

/// The lines of text kept for the text form.
pub const TEXT_LINES: usize = 8;
/// The default foreground's color in a shot, the canvas's text.
const TEXT: [u8; 3] = [0xc9, 0xcd, 0xd9];

/// A screen as kept: how wide it was, and its lines.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Shot {
    pub cols: u16,
    pub lines: Vec<ShotLine>,
}

/// A line of a shot: its runs, each a width in cells and a color (`None` a
/// gap), and its text when it is among the last `TEXT_LINES`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ShotLine {
    #[serde(default)]
    pub runs: Vec<(u16, Option<[u8; 3]>)>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
}

impl Shot {
    /// A pane's screen as `Pane::styled_lines` reads it: a run for each
    /// stretch of written cells of one color (a blank on a background of
    /// its own is written in that color), a gap for the blanks between.
    pub fn of(lines: &[Vec<Span<'static>>], cols: u16) -> Shot {
        let shown = lines.len().saturating_sub(TEXT_LINES);
        let lines = lines
            .iter()
            .enumerate()
            .map(|(at, spans)| {
                let mut runs: Vec<(u16, Option<[u8; 3]>)> = Vec::new();
                for span in spans {
                    for ch in span.content.chars() {
                        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
                        if w == 0 {
                            continue;
                        }
                        let color = if ch != ' ' {
                            Some(span.style.fg.map_or(TEXT, color_of))
                        } else {
                            span.style.bg.filter(|c| *c != Color::Reset).map(color_of)
                        };
                        match runs.last_mut() {
                            Some(last) if last.1 == color => last.0 += w,
                            _ => runs.push((w, color)),
                        }
                    }
                }
                if runs.last().is_some_and(|r| r.1.is_none()) {
                    runs.pop();
                }
                let text = if at >= shown {
                    spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                        .trim_end()
                        .to_string()
                } else {
                    String::new()
                };
                ShotLine { runs, text }
            })
            .collect();
        Shot { cols, lines }
    }

    /// A pane's screen as it is now; none when it shows nothing.
    pub fn of_pane(pane: &crate::mux::pane::Pane) -> Option<Shot> {
        let lines = pane.styled_lines();
        (!lines.is_empty()).then(|| Shot::of(&lines, pane.size().0))
    }

    /// Its last lines of text, for the picture's text form.
    pub fn last_text(&self, n: usize) -> Vec<&str> {
        let texts: Vec<&str> = self.lines.iter().map(|l| l.text.as_str()).collect();
        let from = texts.len().saturating_sub(n);
        texts[from..].to_vec()
    }
}

fn color_of(c: Color) -> [u8; 3] {
    match c {
        Color::Reset => TEXT,
        c => crate::ui::images::rgb(c),
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
    use ratatui::style::Style;

    #[test]
    fn a_screen_is_kept_as_runs_of_its_colors_and_its_last_lines() {
        let red = Style::new().fg(Color::Rgb(255, 0, 0));
        let lines = vec![
            vec![Span::styled("ab", red), Span::raw("  "), Span::raw("cd")],
            vec![Span::styled("  ", Style::new().bg(Color::Rgb(0, 0, 255)))],
        ];
        let shot = Shot::of(&lines, 80);
        assert_eq!(
            shot.lines[0].runs,
            vec![(2, Some([255, 0, 0])), (2, None), (2, Some(TEXT))]
        );
        assert_eq!(shot.lines[1].runs, vec![(2, Some([0, 0, 255]))]);
        assert_eq!(shot.last_text(1), vec![""]);
        assert_eq!(shot.last_text(2), vec!["ab  cd", ""]);
    }

    #[test]
    fn a_shot_is_written_read_and_removed_by_its_name_alone() {
        let state = std::env::temp_dir().join(format!("sb-shots-{}", std::process::id()));
        let shot = Shot {
            cols: 10,
            lines: vec![ShotLine {
                runs: vec![(3, Some([1, 2, 3]))],
                text: "abc".into(),
            }],
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
