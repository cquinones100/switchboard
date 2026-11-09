//! What a link holds, read in a pane beside your work rather than in the
//! browser (tmux/link-view.sh's `show`, lib/link-view.js): a GitHub pull
//! request as the PR view lays it out, a Linear ticket as the ticket view
//! does, a Notion page or Google Doc or Slides deck as Claude's connector
//! gives it, a note (a `file://` link search found) from the notes
//! directory and nowhere else, and any other page as its main text in
//! Markdown. A Slack message can't be read without Claude's connector, so
//! its pane only offers to open it. o opens the link in the browser, in
//! its site's Chrome profile.
//!
//! A web page is fetched (redirects followed, the request counted under
//! its host), its main part kept (`<main>`, else `<article>`, else
//! `<body>`), what is no reading (scripts, styles, navigation, headers and
//! footers, forms) dropped, and the rest turned into Markdown, as turndown
//! did, under the page's title and its address. Text and Markdown pages
//! are kept as they are.
//!
//! A Notion page or Google Doc can't be fetched that way, being private, so
//! it is asked of Claude's connectors (`connector::call_connector`):
//! notion-fetch, or Drive's read_file_content, the page shown as the
//! connector gave it, unless the link_view_docs setting is off.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::markdown::decode_entities;
use super::pager::{self, Content};
use crate::integrations::github;
use crate::net::{self, Audit};
use crate::{connector, settings, util};

/// What is read of a page at most.
const MAX_BYTES: usize = 5 * 1024 * 1024;
/// What is dropped from a page before it is read.
const DROP: &[&str] = &[
    "script", "style", "noscript", "nav", "header", "footer", "aside", "form", "svg", "iframe",
    "button", "template",
];

static LINEAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https://linear\.app/[^/]+/issue/([A-Za-z][A-Za-z0-9]*-[0-9]+)").unwrap()
});
static SLACK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^https://[^/]*slack\.com/").unwrap());
static PULL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https://github\.com/([A-Za-z0-9._-]+)/([A-Za-z0-9._-]+)/pull/([0-9]+)").unwrap()
});
static NOTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^https://([a-z0-9-]+\.)?notion\.(so|site|com)/").unwrap());
static GOOGLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https://docs\.google\.com/(document|presentation)/d/([A-Za-z0-9_-]+)").unwrap()
});

/// What a link is, to the link view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Ticket(String),
    Slack,
    Note(PathBuf),
    Pr {
        repo: github::Repo,
        number: u64,
    },
    /// A Notion page or Google Doc, through the connector.
    Doc,
    Page,
}

/// What a link is, or why it can't be read here.
pub fn kind_of(url: &str, notes: &Path) -> Result<Kind, String> {
    if let Some(c) = LINEAR.captures(url) {
        return Ok(Kind::Ticket(c[1].to_ascii_uppercase()));
    }
    if SLACK.is_match(url) {
        return Ok(Kind::Slack);
    }
    if url.starts_with("file://") {
        return note_path(url, notes).map(Kind::Note);
    }
    if let Some(c) = PULL.captures(url) {
        return Ok(Kind::Pr {
            repo: github::Repo {
                owner: c[1].to_string(),
                name: c[2].to_string(),
            },
            number: c[3]
                .parse()
                .map_err(|_| "That PR's number is too big.".to_string())?,
        });
    }
    if (NOTION.is_match(url) || GOOGLE.is_match(url)) && settings::is_on("link_view_docs") {
        return Ok(Kind::Doc);
    }
    if crate::links::host_of(url).is_some() {
        return Ok(Kind::Page);
    }
    Err("That isn't a link that can be read here.".into())
}

/// `%XX` escapes decoded, as a link's path writes them.
pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let hex = |b: u8| (b as char).to_digit(16);
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((high * 16 + low) as u8);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The note a `file://` link names, only when it is a Markdown file in the
/// notes directory, not hidden, and there.
pub fn note_path(url: &str, notes: &Path) -> Result<PathBuf, String> {
    let refuse = || "Only a note can be read from a file.".to_string();
    let file = PathBuf::from(percent_decode(
        url.strip_prefix("file://").ok_or_else(refuse)?,
    ));
    let real = file.canonicalize().map_err(|_| refuse())?;
    let notes = notes.canonicalize().map_err(|_| refuse())?;
    let hidden = real
        .strip_prefix(&notes)
        .map_err(|_| refuse())?
        .components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with('.'));
    let markdown = real.extension().is_some_and(|e| e == "md");
    if hidden || !markdown || !real.is_file() {
        return Err(refuse());
    }
    Ok(real)
}

/// The notes directory: `$NOTES_DIR`, else `~/Notes`.
pub fn notes_dir() -> PathBuf {
    match std::env::var_os("NOTES_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => crate::paths::home_join("Notes"),
    }
}

static TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap());
static MAIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<main[\s>].*</main>").unwrap());
static ARTICLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<article[\s>].*</article>").unwrap());
static BODY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<body[\s>].*</body>").unwrap());
static BLANKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());

/// A web page's HTML as Markdown, under its title and its address.
pub fn html_markdown(url: &str, html: &str) -> String {
    let title = TITLE
        .captures(html)
        .map(|c| c[1].split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|t| !t.is_empty())
        .map(|t| decode_entities(&t))
        .unwrap_or_else(|| url.to_string());
    let main = MAIN
        .find(html)
        .or_else(|| ARTICLE.find(html))
        .or_else(|| BODY.find(html))
        .map_or(html, |m| m.as_str());
    let converter = htmd::HtmlToMarkdown::builder()
        .skip_tags(DROP.to_vec())
        .build();
    let markdown = converter.convert(main).unwrap_or_default();
    let markdown = BLANKS.replace_all(&markdown, "\n\n");
    format!("# {title}\n\n{url}\n\n---\n\n{}\n", markdown.trim())
}

/// A web page as Markdown, or why it couldn't be read.
pub async fn page_markdown(url: &str) -> Result<String, String> {
    let host = crate::links::host_of(url).unwrap_or_default();
    let protocol = if url.starts_with("http://") {
        "HTTP"
    } else {
        "HTTPS"
    };
    let audit = Audit {
        service: &host,
        protocol,
        source: "link view",
    };
    let accept = [(
        "Accept",
        "text/html,text/markdown,text/plain;q=0.9,*/*;q=0.5",
    )];
    let response = net::request_json("GET", url, &accept, None, audit)
        .await
        .map_err(|e| e.to_string())?;
    if !response.ok() {
        return Err(format!("the site answered {}", response.status));
    }
    let kind = response
        .header("content-type")
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut body = match &response.body {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        json => format!("```json\n{}\n```", util::json_pretty(json)),
    };
    if body.len() > MAX_BYTES {
        let mut cut = MAX_BYTES;
        while !body.is_char_boundary(cut) {
            cut -= 1;
        }
        body.truncate(cut);
    }
    if kind.contains("html") {
        return Ok(html_markdown(url, &body));
    }
    if ["text", "markdown", "json"]
        .iter()
        .any(|k| kind.contains(k))
    {
        return Ok(format!("{url}\n\n---\n\n{body}"));
    }
    Err(format!(
        "it is {}, which can't be read here",
        if kind.is_empty() { "not text" } else { &kind }
    ))
}

static CONTENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<content>(.*)</content>").unwrap());
static TABLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<table[^>]*>(.*?)</table>").unwrap());
static ROW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<tr[^>]*>(.*?)</tr>").unwrap());
static CELL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<t[dh][^>]*>(.*?)</t[dh]>").unwrap());

/// Notion's HTML for what Markdown lacks, made Markdown: its tables
/// Markdown tables, bold and italics Markdown's, its other tags (callouts,
/// columns, mentions) their text (link-view.js's `notionMarkdown`).
pub fn notion_markdown(text: &str) -> String {
    static RULES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
        [
            (r"(?s)<b>(.*?)</b>", "**$1**"),
            (r"(?s)<strong>(.*?)</strong>", "**$1**"),
            (r"(?s)<i>(.*?)</i>", "_${1}_"),
            (r"(?s)<em>(.*?)</em>", "_${1}_"),
            (r"(?s)<code>(.*?)</code>", "`$1`"),
            (r#"(?s)<a href="([^"]*)"[^>]*>(.*?)</a>"#, "[$2]($1)"),
            (r#"<mention-[a-z]+ [^>]*title="([^"]*)"[^>]*/>"#, "$1"),
            (r"</?[a-z][a-z-]*(\s[^>]*)?/?>", ""),
        ]
        .into_iter()
        .map(|(r, to)| (Regex::new(r).expect("a rule"), to))
        .collect()
    });
    let inline = |html: &str| {
        let mut out = html.to_string();
        for (rule, to) in RULES.iter() {
            out = rule.replace_all(&out, *to).into_owned();
        }
        decode_entities(&out)
    };
    let content = CONTENT
        .captures(text)
        .map_or(text, |c| c.get(1).map_or(text, |m| m.as_str()));
    let tables = TABLE.replace_all(content, |c: &regex::Captures| {
        let rows: Vec<Vec<String>> = ROW
            .captures_iter(&c[1])
            .map(|row| {
                CELL.captures_iter(&row[1])
                    .map(|cell| {
                        inline(&cell[1])
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ")
                            .replace('|', "\\|")
                    })
                    .collect()
            })
            .collect();
        let Some(head) = rows.first() else {
            return String::new();
        };
        let line = |cells: &[String]| format!("| {} |", cells.join(" | "));
        let mut out = vec![line(head), line(&vec!["---".to_string(); head.len()])];
        out.extend(rows[1..].iter().map(|r| line(r)));
        out.join("\n")
    });
    BLANKS
        .replace_all(&inline(&tables), "\n\n")
        .trim()
        .to_string()
}

/// A Notion page or Google Doc through Claude's connector, as Markdown.
pub async fn doc_markdown(url: &str) -> Result<String, String> {
    let notion = NOTION.is_match(url);
    let (tool, input) = if notion {
        (
            "mcp__claude_ai_Notion__notion-fetch",
            connector::input([("id", Value::from(url))]),
        )
    } else {
        let id = GOOGLE
            .captures(url)
            .map(|c| c[2].to_string())
            .ok_or("it isn't a Notion page or a Google Doc or Slides deck")?;
        (
            "mcp__claude_ai_Google_Drive__read_file_content",
            connector::input([("fileId", Value::from(id))]),
        )
    };
    let text = connector::call_connector(tool, &input, None)
        .await
        .map_err(|e| e.message)?;
    let body: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
    let field = |k: &str| {
        body.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let markdown = if notion {
        notion_markdown(&field("text"))
    } else {
        field("fileContent").trim().to_string()
    };
    let title = body
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or(url)
        .to_string();
    Ok(format!("# {title}\n\n{url}\n\n---\n\n{markdown}\n"))
}

/// The link view: what the link is decides how it is read.
pub fn main(url: &str) -> ExitCode {
    let url = url.trim().to_string();
    let kind = match kind_of(&url, &notes_dir()) {
        Ok(kind) => kind,
        Err(why) => return pager::run(Content::Failed(why), None, None),
    };
    let browser = url.starts_with("http").then(|| url.clone());
    let place = "the page in the browser (o)".to_string();
    let read = |content: Content| pager::run(content, None, browser.clone());
    match kind {
        Kind::Ticket(id) => super::ticket::main(Some(id)),
        Kind::Slack => read(Content::Offer {
            text: "A Slack message can't be read here, only in Slack.".into(),
            url: url.clone(),
        }),
        Kind::Note(file) => pager::run_local(match std::fs::read_to_string(&file) {
            Ok(text) => Content::Markdown { text, place },
            Err(e) => Content::Failed(format!("Couldn't read the note: {e}")),
        }),
        kind => {
            let (loading, fetch): (String, _) = match kind {
                Kind::Pr { .. } => ("Reading the PR...".into(), kind),
                Kind::Doc => ("Reading it through Claude's connector...".into(), kind),
                _ => (format!("Reading {url}..."), kind),
            };
            let (tx, rx) = std::sync::mpsc::channel();
            let at = url.clone();
            std::thread::spawn(move || {
                let got = util::block_on(async {
                    match fetch {
                        Kind::Pr { repo, number } => {
                            super::pr::by_number(&repo, number, "link view: PR")
                                .await
                                .map(|pr| super::pr::markdown(&pr))
                                .map_err(|e| e.to_string())
                        }
                        Kind::Doc => doc_markdown(&at).await,
                        _ => page_markdown(&at).await,
                    }
                });
                let _ = tx.send(match got {
                    Ok(text) => Content::Markdown {
                        text,
                        place: "the page in the browser (o)".into(),
                    },
                    // The pager says o opens it in the browser, under this.
                    Err(why) => Content::Failed(format!("Couldn't read it: {why}.")),
                });
            });
            pager::run(Content::Loading(loading), Some(rx), browser)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_told_apart() {
        let notes = std::env::temp_dir();
        assert_eq!(
            kind_of("https://linear.app/team/issue/abc-12/title", &notes),
            Ok(Kind::Ticket("ABC-12".into()))
        );
        assert_eq!(
            kind_of("https://acme.slack.com/archives/C1/p2", &notes),
            Ok(Kind::Slack)
        );
        assert!(matches!(
            kind_of("https://github.com/o/r/pull/12345/files", &notes),
            Ok(Kind::Pr { number: 12345, .. })
        ));
        assert_eq!(kind_of("https://example.com/a", &notes), Ok(Kind::Page));
        assert!(kind_of("ftp://example.com", &notes).is_err());
        assert!(kind_of("file:///etc/passwd", &notes).is_err());
    }

    #[test]
    fn a_note_is_read_from_the_notes_directory_only() {
        let dir = std::env::temp_dir().join(format!("sb-notes-{}", std::process::id()));
        let notes = dir.join("Notes");
        std::fs::create_dir_all(notes.join("Work")).unwrap();
        std::fs::create_dir_all(notes.join(".trash")).unwrap();
        std::fs::write(notes.join("Work/Carl's (draft).md"), "# Carl").unwrap();
        std::fs::write(notes.join(".trash/Gone.md"), "# Gone").unwrap();
        std::fs::write(dir.join("Outside.md"), "# Out").unwrap();
        let url = format!("file://{}/Work/Carl%27s%20%28draft%29.md", notes.display());
        assert_eq!(
            note_path(&url, &notes).unwrap(),
            notes.join("Work/Carl's (draft).md").canonicalize().unwrap()
        );
        let trash = format!("file://{}/.trash/Gone.md", notes.display());
        assert!(note_path(&trash, &notes).is_err());
        let out = format!("file://{}/Notes/../Outside.md", dir.display());
        assert!(note_path(&out, &notes).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_page_is_its_main_part_in_markdown_under_its_title() {
        let html = "<html><head><title>The  &amp; page</title><script>x()</script></head>\
            <body><nav>Home | About</nav><main><h2>Hello</h2><p>Some <b>bold</b> words, \
            <a href=\"https://example.com/more\">more</a>.</p></main><footer>(c)</footer></body></html>";
        let md = html_markdown("https://example.com/a", html);
        assert!(
            md.starts_with("# The & page\n\nhttps://example.com/a\n\n---\n\n"),
            "{md}"
        );
        assert!(md.contains("## Hello"), "{md}");
        assert!(md.contains("**bold**"), "{md}");
        assert!(md.contains("[more](https://example.com/more)"), "{md}");
        assert!(!md.contains("Home"), "the navigation dropped: {md}");
        assert!(!md.contains("(c)"), "{md}");
    }

    #[test]
    fn notions_html_becomes_markdown() {
        let text = "<page><content>Intro <b>bold</b> and <mention-user url=\"u\" title=\"Jo\"/>.\n\n\
            <table><tr><td>A</td><td>B</td></tr><tr><td>1</td><td>x | y</td></tr></table></content></page>";
        assert_eq!(
            notion_markdown(text),
            "Intro **bold** and Jo.\n\n| A | B |\n| --- | --- |\n| 1 | x \\| y |"
        );
    }
}
