//! What a pane shows that can be read or opened, for M-p's link commands:
//! its links, newest first, each once, a hyperlink's text before its
//! address; the links of the note the notes pane shows, top to bottom,
//! since Notes draws a link as its text alone; and the images and videos it
//! shows, files and links; and the tabs it names by number ("tab 15"),
//! which a tab keeps while it is open. Ported from tmux/link-view.sh's `links_of` and
//! `media_of`, which read `capture-pane`'s text (with `-e` for the
//! hyperlinks) through perl; here the pane gives its text and its
//! hyperlinks' places in it (`Pane::text_and_links`).

use std::ops::Range;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::ui::draw::cut;

/// One link or file found: what the list shows of it besides the address
/// (a hyperlink's or a Markdown link's text, a path shortened with `~`),
/// empty when that is the address itself; and the address or the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub text: String,
    pub url: String,
}

impl Shown {
    /// The row a list shows: the text, then the address, or the address
    /// alone.
    pub fn row(&self) -> String {
        if self.text.is_empty() {
            self.url.clone()
        } else {
            format!("{}  {}", self.text, self.url)
        }
    }
}

/// A link of nothing but a URL's characters, as link-view.sh's `safe`
/// allowed (a `file://` link is a note search everything found).
static SAFE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(https?|file)://[A-Za-z0-9._~:/?#@!$&()*+,;=%-]+$").expect("a regex")
});

/// Whether a link can be read or opened from here.
pub fn safe(url: &str) -> bool {
    SAFE.is_match(url)
}

static LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://[A-Za-z0-9._~:/?#\[\]@!$&'()*+,;=%-]+").expect("a regex")
});

/// A link's text as a list shows it: one line, its spaces single, cut to
/// 60 columns; nothing when it is the address itself.
fn label(text: &str, url: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text == url {
        return String::new();
    }
    cut(&text, 60)
}

/// The punctuation a sentence puts after a link taken off its end, and a
/// closing bracket the link didn't open.
fn trimmed(url: &str) -> &str {
    let mut url = url;
    while let Some(last) = url.chars().last() {
        let unbalanced = last == ')' && url.matches('(').count() < url.matches(')').count();
        if ".,;:!?'".contains(last) || unbalanced {
            url = &url[..url.len() - 1];
        } else {
            break;
        }
    }
    url
}

/// The links in a pane's text, newest (lowest) first, each once: the plain
/// ones without the punctuation that follows a link in a sentence, and the
/// hyperlinks (`hyperlinks`, by their text's bytes) with the text they were
/// written into, the newest text a link was given winning. A link a
/// program broke across lines is listed put back together too
/// (`wrapped_link`), before the half the line shows, which stays listed
/// since a join can't be told from a link with a path under it for sure.
pub fn links_in(text: &str, hyperlinks: &[(Range<usize>, String)]) -> Vec<Shown> {
    found_links(text, hyperlinks, true)
}

/// As `links_in`, for text no screen wrapped (what you sent Claude, its
/// replies): no link is joined across lines.
pub fn links_in_unwrapped(text: &str) -> Vec<Shown> {
    found_links(text, &[], false)
}

fn found_links(text: &str, hyperlinks: &[(Range<usize>, String)], wrapped: bool) -> Vec<Shown> {
    let mut found: Vec<(usize, String, String)> = Vec::new();
    for (range, url) in hyperlinks {
        let url = url.as_str();
        if url.starts_with("file://") || !safe(url) {
            continue;
        }
        let words = text.get(range.clone()).unwrap_or("");
        found.push((range.start, url.to_string(), label(words, url)));
    }
    for m in LINK.find_iter(text) {
        let inside = hyperlinks
            .iter()
            .any(|(r, _)| m.start() < r.end && r.start < m.end());
        if !inside {
            found.push((m.start(), trimmed(m.as_str()).to_string(), String::new()));
            if let Some(joined) = wrapped
                .then(|| wrapped_link(text, m.as_str(), m.end()))
                .flatten()
            {
                found.push((m.start(), joined, String::new()));
            }
        }
    }
    found.sort_by_key(|(at, _, _)| *at);
    let mut out: Vec<Shown> = Vec::new();
    for (_, url, text) in found.into_iter().rev() {
        match out.iter_mut().find(|s| s.url == url) {
            Some(seen) if seen.text.is_empty() => seen.text = text,
            Some(_) => {}
            None => out.push(Shown { text, url }),
        }
    }
    out
}

/// The characters that close a sentence, a bracket or a quote after a path
/// or a link.
const CLOSERS: &[char] = &[
    '.', ',', ';', ':', '!', '?', ')', ']', '}', '"', '\'', '`', '>',
];

/// The lines after the one ending at `end` (a byte of `text`), each from
/// its first character past its indentation, at most `most` of them: where
/// a path or a link a program broke across lines goes on.
fn lines_after(text: &str, end: usize, most: usize) -> Vec<&str> {
    let rest = &text[end..];
    let rest = rest.trim_start_matches([' ', '\t']);
    let Some(rest) = rest.strip_prefix('\n') else {
        return Vec::new();
    };
    rest.split('\n').take(most).map(str::trim).collect()
}

/// A link that runs to its line's end and that a program broke there, put
/// back together (up to 3 lines): when the next line, past its indentation,
/// holds one word of nothing but a link's characters (and not a link of
/// its own), since a link's rest followed by words can't be told from prose,
/// the half ends in no punctuation a sentence puts after a link, and the
/// next piece holds what a link holds and a word doesn't (`/`, `?`, `=`,
/// `&`, `#`, `%`, `_`), so a whole link followed by a line of prose
/// stays as it is, the joined link, kept
/// when its host is the half's. Nothing when it isn't broken.
fn wrapped_link(text: &str, half: &str, end: usize) -> Option<String> {
    if trimmed(half) != half {
        return None;
    }
    let host = crate::links::host_of(half)?;
    let mut joined = half.to_string();
    for line in lines_after(text, end, 2) {
        let piece = line;
        let breaks = !piece.contains(char::is_whitespace)
            && piece.contains(['/', '?', '=', '&', '#', '%', '_']);
        let all_link = !piece.is_empty()
            && LINK.find(piece).is_none()
            && piece
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._~:/?#[]@!$&'()*+,;=%-".contains(c));
        if !all_link || !breaks {
            break;
        }
        joined.push_str(piece);
    }
    let joined = trimmed(&joined).to_string();
    let whole = joined.len() > trimmed(half).len()
        && crate::links::host_of(&joined).as_deref() == Some(host.as_str())
        && safe(&joined);
    whole.then_some(joined)
}

static TAB_NAMED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\btabs?\s+#?\d+\b(?:(?:\s*,\s*(?:and\s+|&\s*)?|\s+(?:and|&)\s+)#?\d+\b)*")
        .expect("a regex")
});

static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").expect("a regex"));

/// The tab numbers a pane's text names ("tab 15", "Tab #3", "tabs 3, 5
/// and 7"), any case and as whole words, newest (lowest) first, each once.
/// Whether a tab has that number now is the server's to say.
pub fn tabs_in(text: &str) -> Vec<usize> {
    let mut found: Vec<usize> = Vec::new();
    for m in TAB_NAMED.find_iter(text) {
        for n in NUMBER.find_iter(m.as_str()) {
            if let Ok(n) = n.as_str().parse() {
                found.push(n);
            }
        }
    }
    let mut out: Vec<usize> = Vec::new();
    for n in found.into_iter().rev() {
        if !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

static NOTE_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[([^\]\n]*)\]\((https?://[^)\s]+)\)|(https?://[A-Za-z0-9._~:/?#@!$&*+,;=%-]+)")
        .expect("a regex")
});

/// The links in a note's Markdown, top to bottom, each once: a Markdown
/// link with its text, and a bare one, which isn't inside a Markdown
/// link's brackets.
pub fn note_links(markdown: &str) -> Vec<Shown> {
    let mut out: Vec<Shown> = Vec::new();
    for c in NOTE_LINK.captures_iter(markdown) {
        let found = if let (Some(text), Some(url)) = (c.get(1), c.get(2)) {
            Shown {
                text: label(text.as_str(), url.as_str()),
                url: url.as_str().to_string(),
            }
        } else {
            let bare = c.get(3).expect("one of the two");
            let before = markdown[..bare.start()].chars().last();
            if matches!(before, Some('(' | '[')) {
                continue;
            }
            Shown {
                text: String::new(),
                url: bare
                    .as_str()
                    .trim_end_matches(['.', ',', ';', ':', '!', '?'])
                    .to_string(),
            }
        };
        if !out.iter().any(|s| s.url == found.url) {
            out.push(found);
        }
    }
    out
}

/// The extensions of the images and videos the media picker lists.
const MEDIA: &str = "png|jpe?g|gif|webp|heic|svg|bmp|tiff?|mp4|mov|webm|mkv|m4v|avi";

static MEDIA_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)(?:~|\.{{1,2}})?/?[\w.~@+%/'-]*[\w-]\.(?:{MEDIA})"
    ))
    .expect("a regex")
});

/// A whole path to an image or a video, as `MEDIA_PATH` finds one.
static MEDIA_WHOLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)^(?:~|\.{{1,2}})?/?[\w.~@+%/'-]*[\w-]\.(?:{MEDIA})$"
    ))
    .expect("a regex")
});

static MEDIA_END: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"(?i)\.(?:{MEDIA})$")).expect("a regex"));

/// Whether a link's path names an image or a video, by its extension.
pub fn is_media(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let path = match path.split_once("://") {
        Some((_, rest)) => rest.find('/').map_or("", |at| &rest[at..]),
        None => path,
    };
    MEDIA_END.is_match(path)
}

/// Whether a path can start after `before`: at a line's start, after a
/// space or an opening bracket or `=`.
fn opens(before: Option<char>) -> bool {
    before.is_none_or(|c| c.is_whitespace() || "([{<=".contains(c))
}

/// The paths a program broke across lines, put back together: a line's
/// last word holding a `/` (past a quote or bracket that opens it), joined
/// with the first word of the next line past its indentation, and of the
/// line after when the next held nothing else, each kept by where it starts
/// when it names an image or a video whose file is there (`full_of`). A
/// join that names no file leaves the halves as they were.
fn wrapped_media(text: &str, full_of: &dyn Fn(&str) -> std::path::PathBuf) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    let mut at = 0;
    for line in text.split('\n') {
        let start = at;
        at += line.len() + 1;
        let kept = line.trim_end();
        let word_at = kept
            .char_indices()
            .rfind(|(_, c)| c.is_whitespace())
            .map_or(0, |(i, c)| i + c.len_utf8());
        let word = &kept[word_at..];
        let head = word.trim_start_matches(['"', '\'', '`', '(', '[', '{', '<']);
        let head = head.rsplit_once('=').map_or(head, |(_, path)| path);
        if !head.contains('/') {
            continue;
        }
        let mut joined = head.to_string();
        for next in lines_after(text, start + kept.len(), 2) {
            let Some(piece) = next.split_whitespace().next() else {
                break;
            };
            joined.push_str(piece);
            let path = joined.trim_end_matches(CLOSERS);
            if MEDIA_WHOLE.is_match(path) {
                let full = full_of(path);
                if full.is_file() {
                    found.push((start + word_at, full.to_string_lossy().into_owned()));
                }
                break;
            }
            if piece != next {
                break;
            }
        }
    }
    found
}

/// The images and videos a pane's text names: the files first, newest
/// first, each a path absolute, from `~`, or from the pane's directory
/// (`dir`), listed only when the file is there; then the links to them
/// among the pane's links (`links`, as `links_in` gives them), each known
/// by its path's extension.
pub fn media_in(text: &str, dir: &Path, home: &Path, links: &[Shown]) -> Vec<Shown> {
    let full_of = |path: &str| {
        if let Some(rest) = path.strip_prefix("~/") {
            home.join(rest)
        } else if path.starts_with('/') {
            Path::new(path).to_path_buf()
        } else {
            dir.join(path.strip_prefix("./").unwrap_or(path))
        }
    };
    let mut files: Vec<(usize, String)> = Vec::new();
    for m in MEDIA_PATH.find_iter(text) {
        let mut start = m.start();
        let mut path = m.as_str();
        // A path in quotes: the quote opens it, after what may open one.
        while let Some(rest) = path.strip_prefix('\'') {
            path = rest;
            start += 1;
        }
        let mut before = text[..start].chars().rev();
        let first = before.next();
        let ok = match first {
            Some('"' | '\'' | '`') => opens(before.next()),
            other => opens(other),
        };
        let mut after = text[m.end()..].chars();
        let next = after.next();
        let ends = match next {
            None => true,
            Some('.') => after.next().is_none_or(char::is_whitespace),
            Some(c) => c.is_whitespace() || ",;:!?)]}\"'`>".contains(c),
        };
        if !ok || !ends || path.is_empty() {
            continue;
        }
        let full = full_of(path);
        if full.is_file() {
            files.push((start, full.to_string_lossy().into_owned()));
        }
    }
    files.extend(wrapped_media(text, &full_of));
    files.sort_by_key(|(at, _)| *at);
    let home = home.to_string_lossy();
    let mut out: Vec<Shown> = Vec::new();
    for (_, file) in files.into_iter().rev() {
        if out.iter().any(|s| s.url == file) {
            continue;
        }
        let text = match file.strip_prefix(&format!("{home}/")) {
            Some(rest) => format!("~/{rest}"),
            None => file.clone(),
        };
        out.push(Shown {
            text: if text == file { String::new() } else { text },
            url: file,
        });
    }
    out.extend(links.iter().filter(|l| is_media(&l.url)).cloned());
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn tabs_in_finds_the_tabs_named_newest_first_each_once() {
        let text =
            "see tab 15\nTab #3 (design) and tabs 3, 5 and 7\ntab 12's table 4 stab 9 tab 15x\n";
        assert_eq!(tabs_in(text), vec![12, 7, 5, 3, 15]);
        assert_eq!(tabs_in("tab 3 and tab 9"), vec![9, 3]);
        assert_eq!(tabs_in("tabs 2 & 4"), vec![4, 2]);
        assert!(tabs_in("the table 4, stab 9, tabby 2").is_empty());
    }

    use super::*;

    fn urls(found: &[Shown]) -> Vec<&str> {
        found.iter().map(|s| s.url.as_str()).collect()
    }

    #[test]
    fn a_panes_links_are_listed_newest_first_each_once() {
        let text = "See https://plain.example/one.\nThe PR (https://github.com/o/r/pull/2) is up\n\
                    again https://plain.example/one and https://same.example/x";
        assert_eq!(
            urls(&links_in(text, &[])),
            vec![
                "https://same.example/x",
                "https://plain.example/one",
                "https://github.com/o/r/pull/2",
            ]
        );
    }

    #[test]
    fn a_hyperlink_is_listed_with_the_text_it_was_written_into() {
        let text = "Read the docs here and https://plain.example/a\nthe docs again";
        let at = |words: &str| {
            let start = text.find(words).unwrap();
            start..start + words.len()
        };
        let docs = "https://docs.example/guide".to_string();
        let mut later = at("the docs again");
        later.start = text.rfind("the docs").unwrap();
        let found = links_in(
            text,
            &[
                (at("the docs"), docs.clone()),
                (later, docs.clone()),
                (at("here"), "javascript:alert(1)".into()),
            ],
        );
        assert_eq!(urls(&found), vec![docs.as_str(), "https://plain.example/a"]);
        assert_eq!(found[0].text, "the docs again", "the newest text");
        assert_eq!(found[0].row(), "the docs again  https://docs.example/guide");
        assert_eq!(found[1].row(), "https://plain.example/a");
    }

    #[test]
    fn a_notes_links_are_its_markdown_links_and_bare_ones_top_to_bottom() {
        let note = "- [ ] Read [the RFC](https://rfc.example/1) first\n\
                    - see https://bare.example/x.\n\
                    - again (https://rfc.example/1)\n";
        let found = note_links(note);
        assert_eq!(
            urls(&found),
            vec!["https://rfc.example/1", "https://bare.example/x"]
        );
        assert_eq!(found[0].text, "the RFC");
        assert!(note_links("nothing here").is_empty());
    }

    #[test]
    fn images_and_videos_are_files_that_are_there_then_links_to_them() {
        let root = std::env::temp_dir().join(format!("sb-media-{}", std::process::id()));
        let home = root.join("home");
        let dir = root.join("work");
        std::fs::create_dir_all(home.join("Pictures")).unwrap();
        std::fs::create_dir_all(dir.join("shots")).unwrap();
        std::fs::write(home.join("Pictures/cat.PNG"), b"x").unwrap();
        std::fs::write(dir.join("shots/a.mov"), b"x").unwrap();
        std::fs::write(dir.join("b.gif"), b"x").unwrap();
        let text = "Saved ~/Pictures/cat.PNG and ./shots/a.mov.\n\
                    missing/c.png here, path=b.gif, word:b.gif no\n\
                    \"shots/a.mov\" again";
        let links = links_in(
            "https://x.example/v/clip.mp4?t=3 https://x.example/page",
            &[],
        );
        let found = media_in(text, &dir, &home, &links);
        assert_eq!(
            urls(&found),
            vec![
                dir.join("shots/a.mov").to_str().unwrap(),
                dir.join("b.gif").to_str().unwrap(),
                home.join("Pictures/cat.PNG").to_str().unwrap(),
                "https://x.example/v/clip.mp4?t=3",
            ]
        );
        assert_eq!(found[2].text, "~/Pictures/cat.PNG");
        assert!(is_media("https://x.example/a.webm#t"));
        assert!(!is_media("https://x.example/png"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_path_a_program_broke_across_lines_is_found_when_its_file_is_there() {
        let root = std::env::temp_dir().join(format!("sb-wrapped-{}", std::process::id()));
        let session = root.join("scratchpad/6152");
        std::fs::create_dir_all(session.join("walkthrough")).unwrap();
        let video = session.join("walkthrough/Force-the-variants.mp4");
        std::fs::write(&video, b"x").unwrap();
        let first = format!("  │ The video is at {}/", session.display());
        let text = format!("{first}\n    walkthrough/Force-the-variants.mp4.\n  Keep it?");
        let found = media_in(&text, &root, &root, &[]);
        assert_eq!(urls(&found), vec![video.to_str().unwrap()]);
        // Over three lines, the middle one holding nothing else.
        let text = format!(
            "at {}/\n  walkthrough/\n  Force-the-variants.mp4 done",
            session.display()
        );
        assert_eq!(
            urls(&media_in(&text, &root, &root, &[])),
            vec![video.to_str().unwrap()]
        );
        // A join that names no file leaves the halves.
        let text = format!("at {}/\n    walkthrough/gone.mp4", session.display());
        assert!(media_in(&text, &root, &root, &[]).is_empty());
        // A space wider than a byte before the last word.
        assert!(media_in("hello\u{a0}world\nmore", &root, &root, &[]).is_empty());
        // A path ending a line before a new sentence stays as it is.
        let text = format!("saved {}\nThen it plays.mp4", video.display());
        assert_eq!(
            urls(&media_in(&text, &root, &root, &[])),
            vec![video.to_str().unwrap()]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_link_a_program_broke_across_lines_is_joined() {
        let text = "see https://github.com/acme/widgets/\n    pull/12345/files\n  for it";
        assert_eq!(
            urls(&links_in(text, &[])),
            vec![
                "https://github.com/acme/widgets/pull/12345/files",
                "https://github.com/acme/widgets/",
            ]
        );
        assert_eq!(
            urls(&links_in_unwrapped(text)),
            vec!["https://github.com/acme/widgets/"]
        );
        let text = "read https://x.example/docs?page=\n  2&tab=api.";
        assert_eq!(
            urls(&links_in(text, &[]))[0],
            "https://x.example/docs?page=2&tab=api"
        );
        // A link ending a sentence, then a new line of words, stays whole.
        let text = "see https://x.example/a.\nThen more\nhttps://x.example/b/\nRun the walkthrough\nhttps://x.example/c/\nCI/CD instructions follow";
        assert_eq!(
            urls(&links_in(text, &[])),
            vec![
                "https://x.example/c/",
                "https://x.example/b/",
                "https://x.example/a"
            ]
        );
    }

    #[test]
    fn only_a_urls_characters_make_a_safe_link() {
        assert!(safe("https://example.com/a?b=c&d=(e)"));
        assert!(safe("file:///home/me/Notes/a.md"));
        assert!(!safe("https://example.com/a b"));
        assert!(!safe("javascript:alert(1)"));
    }
}
