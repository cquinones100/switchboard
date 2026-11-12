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
/// written into, the newest text a link was given winning.
pub fn links_in(text: &str, hyperlinks: &[(Range<usize>, String)]) -> Vec<Shown> {
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

/// The images and videos a pane's text names: the files first, newest
/// first, each a path absolute, from `~`, or from the pane's directory
/// (`dir`), listed only when the file is there; then the links to them
/// among the pane's links (`links`, as `links_in` gives them), each known
/// by its path's extension.
pub fn media_in(text: &str, dir: &Path, home: &Path, links: &[Shown]) -> Vec<Shown> {
    let mut files: Vec<String> = Vec::new();
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
        let full = if let Some(rest) = path.strip_prefix("~/") {
            home.join(rest)
        } else if path.starts_with('/') {
            Path::new(path).to_path_buf()
        } else {
            dir.join(path.strip_prefix("./").unwrap_or(path))
        };
        if full.is_file() {
            files.push(full.to_string_lossy().into_owned());
        }
    }
    let home = home.to_string_lossy();
    let mut out: Vec<Shown> = Vec::new();
    for file in files.into_iter().rev() {
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
    fn only_a_urls_characters_make_a_safe_link() {
        assert!(safe("https://example.com/a?b=c&d=(e)"));
        assert!(safe("file:///home/me/Notes/a.md"));
        assert!(!safe("https://example.com/a b"));
        assert!(!safe("javascript:alert(1)"));
    }
}
