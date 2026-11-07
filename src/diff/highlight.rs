//! Syntax colours for the diff's two sides, in place of the tree-sitter
//! highlighting nvim/diff.lua started for each file: syntect's grammars,
//! picked by the file's extension (else its first line), with its
//! pure-Rust regex engine, so nothing is needed beyond the binary.
//!
//! Only the text's colour is taken: a theme's background would read as more
//! highlighting in a diff, which paints its own (diffview.lua's
//! `unpainted`). A dark theme on a dark terminal and a light one on a light,
//! as the `theme` setting says.
//!
//! A file is coloured off the screen's thread, since a long file takes a
//! moment; it shows uncoloured until then. A file too long, or a line too
//! long, isn't coloured past that point, as a grammar can take seconds over
//! a minified line.

use std::ops::Range;
use std::sync::LazyLock;

use ratatui::style::Color;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

/// Past these, the rest of a file is left uncoloured.
const MAX_LINES: usize = 20_000;
const MAX_LINE_BYTES: usize = 4_000;

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEMES: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

fn theme(light: bool) -> &'static Theme {
    let name = if light {
        "base16-ocean.light"
    } else {
        "base16-ocean.dark"
    };
    &THEMES.themes[name]
}

/// A run of a line in one colour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hl {
    pub range: Range<usize>,
    pub color: Color,
}

/// Each line's coloured runs, for the text of `path` (as `sides::lines_of`
/// gives its lines), or `None` for a file no grammar knows.
pub fn highlight(path: &str, lines: &[String], light: bool) -> Option<Vec<Vec<Hl>>> {
    let ext = path.rsplit('/').next()?.rsplit_once('.').map(|(_, e)| e);
    let syntax = ext
        .and_then(|e| SYNTAXES.find_syntax_by_extension(e))
        .or_else(|| {
            let name = path.rsplit('/').next()?;
            SYNTAXES.find_syntax_by_extension(name)
        })
        .or_else(|| SYNTAXES.find_syntax_by_first_line(lines.first()?))?;
    if syntax.name == "Plain Text" {
        return None;
    }
    let mut h = HighlightLines::new(syntax, theme(light));
    let mut out = Vec::with_capacity(lines.len().min(MAX_LINES));
    for line in lines.iter().take(MAX_LINES) {
        if line.len() > MAX_LINE_BYTES {
            break;
        }
        let with_end = format!("{line}\n");
        let mut runs = Vec::new();
        let Ok(pieces) = h.highlight_line(&with_end, &SYNTAXES) else {
            break;
        };
        let mut at = 0;
        for (style, text) in pieces {
            let end = (at + text.len()).min(line.len());
            if end > at {
                let c = style.foreground;
                runs.push(Hl {
                    range: at..end,
                    color: Color::Rgb(c.r, c.g, c.b),
                });
            }
            at += text.len();
        }
        out.push(runs);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_language_is_coloured_and_plain_text_is_not() {
        let lines = vec!["fn main() {}".to_string()];
        let runs = highlight("src/main.rs", &lines, false).expect("Rust is known");
        assert!(runs[0].len() > 1, "{:?}", runs[0]);
        assert_eq!(runs[0].last().unwrap().range.end, lines[0].len());
        assert!(highlight("notes.unknownext", &["hello".into()], false).is_none());
    }
}
