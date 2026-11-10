//! Syntax colours for the diff's two sides, in place of the tree-sitter
//! highlighting nvim/diff.lua started for each file: syntect with bat's
//! grammars (the two-face crate), which hold syntect's own and the
//! languages those leave out (TypeScript, TSX, TOML, Dockerfile, Svelte,
//! Vue and more), picked by the file's extension (else its name, else its
//! first line), with syntect's pure-Rust regex engine, so nothing is
//! needed beyond the binary.
//!
//! The colours are the terminal's own: each kind of text takes one of the
//! sixteen ANSI colours, by the roles a base16 scheme gives them, as
//! nvim/colors/ghostty.lua laid Ghostty's palette out as base16 for
//! mini.base16 (comments the bright black, strings green, functions blue,
//! keywords magenta, types yellow, variables and tags red, constants bright
//! red for base16's orange, escapes and regexes cyan), and the rest is left
//! the terminal's text colour. So the code matches the terminal's theme,
//! dark or light, with nothing to choose. Only the text's colour is taken:
//! a background would read as more highlighting in a diff, which paints its
//! own (diffview.lua's `unpainted`).
//!
//! A file is coloured off the screen's thread, since a long file takes a
//! moment; it shows uncoloured until then. A file too long, or a line too
//! long, isn't coloured past that point, as a grammar can take seconds over
//! a minified line.

use std::ops::Range;
use std::str::FromStr;
use std::sync::LazyLock;

use ratatui::style::Color;
use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color as SynColor, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::SyntaxSet;

/// Past these, the rest of a file is left uncoloured.
const MAX_LINES: usize = 20_000;
const MAX_LINE_BYTES: usize = 4_000;

/// The scopes each ANSI colour is given, in base16's roles (base16's
/// TextMate scheme): later rules win where two match equally.
const RULES: &[(&str, u8)] = &[
    // base08: variables, tags, deletions.
    (
        "variable, entity.name.tag, markup.deleted, punctuation.definition.variable",
        1,
    ),
    // base09: constants, numbers, attributes.
    (
        "constant, constant.numeric, constant.language, constant.character, \
         entity.other.attribute-name, support.constant",
        9,
    ),
    // base0A: types and classes.
    (
        "entity.name.type, entity.name.class, support.class, support.type, \
         entity.other.inherited-class, markup.bold",
        3,
    ),
    // base0B: strings and insertions.
    (
        "string, constant.other.symbol, markup.inserted, markup.raw",
        2,
    ),
    // base0C: escapes, regexes, the language's own functions.
    (
        "constant.character.escape, string.regexp, support.function",
        6,
    ),
    // base0D: functions, headings, links.
    (
        "entity.name.function, meta.function-call entity.name, \
         variable.function, markup.heading, entity.name.section, \
         markup.underline.link",
        4,
    ),
    // base0E: keywords and storage.
    (
        "keyword, storage, storage.type, storage.modifier, meta.selector, \
         markup.italic, markup.changed",
        5,
    ),
    // base05 again: operators, the text's colour.
    ("keyword.operator", DEFAULT),
    // base03: comments, last so nothing inside one takes another colour.
    ("comment, punctuation.definition.comment", 8),
];

/// What a rule gives for the terminal's own text colour.
const DEFAULT: u8 = u8::MAX;

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);

/// A theme whose colours are the terminal's: an ANSI colour's number in
/// red with alpha 0, and the text's own as alpha 1, as bat's `ansi` theme
/// carries them through syntect.
static ANSI: LazyLock<Theme> = LazyLock::new(|| {
    let ansi = |n: u8| {
        if n == DEFAULT {
            TEXT
        } else {
            SynColor {
                r: n,
                g: 0,
                b: 0,
                a: 0,
            }
        }
    };
    Theme {
        settings: ThemeSettings {
            foreground: Some(TEXT),
            ..ThemeSettings::default()
        },
        scopes: RULES
            .iter()
            .filter_map(|(scopes, n)| {
                Some(ThemeItem {
                    scope: ScopeSelectors::from_str(scopes).ok()?,
                    style: StyleModifier {
                        foreground: Some(ansi(*n)),
                        ..StyleModifier::default()
                    },
                })
            })
            .collect(),
        ..Theme::default()
    }
});

/// The terminal's text colour, as `ANSI` carries it.
const TEXT: SynColor = SynColor {
    r: 0,
    g: 0,
    b: 0,
    a: 1,
};

/// A run of a line in one colour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hl {
    pub range: Range<usize>,
    pub color: Color,
}

/// Each line's coloured runs, for the text of `path` (as `sides::lines_of`
/// gives its lines), or `None` for a file no grammar knows.
pub fn highlight(path: &str, lines: &[String]) -> Option<Vec<Vec<Hl>>> {
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
    let mut h = HighlightLines::new(syntax, &ANSI);
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
            let c = style.foreground;
            // The text's own colour is left as it is, uncoloured.
            if end > at && c.a == 0 {
                runs.push(Hl {
                    range: at..end,
                    color: Color::Indexed(c.r),
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
    fn a_known_language_is_coloured_in_the_terminal_s_own_colours() {
        let lines = vec!["fn main() { let s = \"hi\"; } // done".to_string()];
        let runs = highlight("src/main.rs", &lines).expect("Rust is known");
        let colour_of = |text: &str| {
            let at = lines[0].find(text).unwrap();
            runs[0]
                .iter()
                .find(|h| h.range.contains(&at))
                .map(|h| h.color)
        };
        assert_eq!(colour_of("fn"), Some(Color::Indexed(5)), "{runs:?}");
        assert_eq!(colour_of("main"), Some(Color::Indexed(4)));
        assert_eq!(colour_of("\"hi\""), Some(Color::Indexed(2)));
        assert_eq!(colour_of("// done"), Some(Color::Indexed(8)));
        assert!(
            RULES
                .iter()
                .all(|(s, _)| ScopeSelectors::from_str(s).is_ok()),
            "every rule's scopes read"
        );
        assert!(highlight("notes.unknownext", &["hello".into()]).is_none());
    }

    /// The color of the run holding `text`'s first byte in a one-line file.
    fn color_in(path: &str, line: &str, text: &str) -> Option<Color> {
        let runs = highlight(path, &[line.to_string()])?;
        let at = line.find(text)?;
        runs[0]
            .iter()
            .find(|h| h.range.contains(&at))
            .map(|h| h.color)
    }

    #[test]
    fn typescript_and_tsx_are_colored() {
        let ts = "const n: number = f(\"a\"); // c";
        assert_eq!(color_in("src/a.ts", ts, "const"), Some(Color::Indexed(5)));
        assert_eq!(color_in("src/a.ts", ts, "number"), Some(Color::Indexed(3)));
        assert_eq!(color_in("src/a.ts", ts, "f("), Some(Color::Indexed(4)));
        assert_eq!(color_in("src/a.ts", ts, "\"a\""), Some(Color::Indexed(2)));
        assert_eq!(color_in("src/a.ts", ts, "// c"), Some(Color::Indexed(8)));

        let tsx = "const e = <div className=\"x\">{n}</div>;";
        assert_eq!(color_in("src/a.tsx", tsx, "const"), Some(Color::Indexed(5)));
        assert_eq!(color_in("src/a.tsx", tsx, "div"), Some(Color::Indexed(1)));
        assert_eq!(
            color_in("src/a.tsx", tsx, "className"),
            Some(Color::Indexed(9))
        );
        assert_eq!(color_in("src/a.tsx", tsx, "\"x\""), Some(Color::Indexed(2)));
    }

    #[test]
    fn the_languages_syntect_s_own_set_leaves_out_are_colored() {
        for (path, line, text, color) in [
            ("Cargo.toml", "name = \"x\" # c", "\"x\"", 2),
            ("docker/Dockerfile", "FROM rust:1 AS build", "FROM", 5),
            ("src/A.svelte", "<script>let a = 1;</script>", "let", 5),
            ("src/A.vue", "<template><div>a</div></template>", "div", 1),
            ("src/a.kt", "fun main() { val a = 1 }", "fun", 5),
            ("src/a.swift", "let a = 1", "let", 5),
            ("main.tf", "resource \"a\" \"b\" {}", "resource", 5),
            ("src/a.zig", "const a = 1;", "const", 5),
        ] {
            assert_eq!(
                color_in(path, line, text),
                Some(Color::Indexed(color)),
                "{path}"
            );
        }
    }
}
