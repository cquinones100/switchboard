//! The page the image view draws, ported from tmux/pr-image.js's
//! `pageFor`, `prPage` and `ticketPage`: a heading (the title, then its
//! number or ID dimmed), a line of facts, and the body, laid out in an HTML
//! page with GitHub's dark stylesheet, for headless Chrome to draw.
//!
//! A PR's body is GitHub's own HTML (the pull request's `body_html`, read
//! with a GET), so links, mentions and checklists are GitHub's, and it is
//! already made safe by GitHub; each Mermaid diagram in it is turned back
//! into its source for Mermaid to draw. A ticket's description and comments
//! are Markdown, from Linear, which nothing has made safe, so they go into
//! the page as JSON, never as HTML, and are drawn in the page by marked and
//! cleaned by DOMPurify before they become part of it. Everything the page
//! loads from the web is pinned by version and by its integrity hash, so a
//! changed file on the CDN is refused rather than run. When those scripts
//! can't be loaded at all, the Markdown is shown as its text and diagrams as
//! their source, rather than nothing.
//!
//! With the image cache on (`super::cache`), the page names the copies kept
//! on this computer in place of the links (`Local`): a pinned file's copy,
//! whose hash was checked as it was kept and again as it is used, in place
//! of the CDN's, a PR's pictures in its HTML, and a ticket's in the page's
//! script, once DOMPurify has cleaned the Markdown, so a copy is never a
//! way past it.
//!
//! The page measures itself once drawn, when everything it loads has
//! loaded or failed and the diagrams are drawn: where the body's blocks
//! start and end, which the pages are cut between (`super::render`), and
//! whether anything failed to load, written as JSON into a hidden element
//! that Chrome's `--dump-dom` prints.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

/// A file the page loads from the web, at a pinned version, with the hash
/// the browser checks it against.
#[derive(Clone, Copy, Debug)]
pub struct Pin {
    pub url: &'static str,
    pub integrity: &'static str,
}

/// GitHub's stylesheet for Markdown, dark.
pub const STYLESHEET: Pin = Pin {
    url: "https://cdn.jsdelivr.net/npm/github-markdown-css@5.8.1/github-markdown-dark.css",
    integrity: "sha384-zK5tmCaB05WYuRfCHaMrOJAgIErORoIojJtB/QOoQIRe2Q+W6X6J0hAgE9ajtQT4",
};
pub const MERMAID: Pin = Pin {
    url: "https://cdn.jsdelivr.net/npm/mermaid@11.4.1/dist/mermaid.min.js",
    integrity: "sha384-rbtjAdnIQE/aQJGEgXrVUlMibdfTSa4PQju4HDhN3sR2PmaKFzhEafuePsl9H/9I",
};
pub const MARKED: Pin = Pin {
    url: "https://cdn.jsdelivr.net/npm/marked@12.0.2/marked.min.js",
    integrity: "sha384-/TQbtLCAerC3jgaim+N78RZSDYV7ryeoBCVqTuzRrFec2akfBkHS7ACQ3PQhvMVi",
};
pub const PURIFY: Pin = Pin {
    url: "https://cdn.jsdelivr.net/npm/dompurify@3.1.6/dist/purify.min.js",
    integrity: "sha384-+VfUPEb0PdtChMwmBcBmykRMDd+v6D/oFmB3rZM/puCMDYcIvF968OimRh4KQY9a",
};
/// Every file the page loads, for the request audit.
pub const PINS: [Pin; 4] = [STYLESHEET, MARKED, PURIFY, MERMAID];

/// The page's background, which the pages' strips are filled with too.
pub const BACKGROUND: [u8; 3] = [0x0d, 0x11, 0x17];

/// What the page shows: the heading's title and number, the line of facts
/// (HTML, its text escaped), the body (HTML), and the Markdown sources the
/// body's `data-md` elements name, by index.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parts {
    pub title: String,
    pub number: String,
    pub meta: String,
    pub html: String,
    #[serde(default)]
    pub markdown: Vec<String>,
}

/// Text made safe to put in HTML.
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// GitHub wraps a diagram's source in a section its own viewer fills in.
static GITHUB_DIAGRAM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<section[^>]*data-type="mermaid"[^>]*>.*?<pre lang="mermaid"[^>]*>(.*?)</pre>.*?</section>"#)
        .unwrap()
});

/// GitHub's diagram sections as blocks for Mermaid to draw.
pub fn diagrams(html: &str) -> String {
    GITHUB_DIAGRAM
        .replace_all(html, r#"<pre class="mermaid">$1</pre>"#)
        .into_owned()
}

/// The copies on this computer of what the page loads, by the link the
/// page names: the pinned files' and the pictures' (`super::cache`).
pub type Local = BTreeMap<String, PathBuf>;

/// A file's `file://` link, every byte but a path's own characters
/// percent-encoded, so it reads the same in an attribute, in JSON and to
/// Chrome whatever the path holds.
pub fn file_url(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut url = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            url.push(b as char);
        } else {
            url.push_str(&format!("%{b:02X}"));
        }
    }
    url
}

/// A script tag for a pinned file: its copy here when there is one, which
/// was checked against the pin's hash as it was taken (the browser can't
/// check a `file://` script, which it reads as another origin's), else the
/// CDN's, checked by the browser.
fn script(pin: Pin, local: &Local) -> String {
    match local.get(pin.url) {
        Some(copy) => format!(r#"<script src="{}"></script>"#, file_url(copy)),
        None => format!(
            r#"<script src="{}" integrity="{}" crossorigin="anonymous"></script>"#,
            pin.url, pin.integrity
        ),
    }
}

/// The stylesheet's tag, its copy here or the CDN's, as `script`.
fn stylesheet(local: &Local) -> String {
    match local.get(STYLESHEET.url) {
        Some(copy) => format!(r#"<link rel="stylesheet" href="{}">"#, file_url(copy)),
        None => format!(
            r#"<link rel="stylesheet" href="{}" integrity="{}" crossorigin="anonymous">"#,
            STYLESHEET.url, STYLESHEET.integrity
        ),
    }
}

/// HTML's escapes in an attribute's value undone, as the browser reads it.
fn unescape(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// A picture in HTML: the tag up to its `src`, and the link.
static IMG_SRC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)(<img\b[^>]*?\ssrc=")([^"]*)""#).unwrap());
/// A picture in Markdown, `![words](link)`, or as HTML in it.
static MD_IMAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"!\[[^\]]*\]\(\s*<?(https://[^\s)>]+)|(?i:<img\b[^>]*?\ssrc=["']?(https://[^"'\s>]+))"#,
    )
    .unwrap()
});

/// The pictures the page shows, by their links as the browser reads them,
/// each once: a PR's from its HTML, a ticket's from its Markdown. Only
/// `https://` ones, which are all the image cache keeps.
pub fn pictures(parts: &Parts) -> Vec<String> {
    let mut found: Vec<String> = IMG_SRC
        .captures_iter(&parts.html)
        .map(|c| unescape(&c[2]))
        .collect();
    for text in &parts.markdown {
        found.extend(
            MD_IMAGE
                .captures_iter(text)
                .filter_map(|c| c.get(1).or_else(|| c.get(2)))
                .map(|m| m.as_str().to_string()),
        );
    }
    let mut seen = std::collections::HashSet::new();
    found.retain(|url| url.starts_with("https://") && seen.insert(url.clone()));
    found
}

/// A PR's HTML with each picture kept here named by its copy.
pub fn with_local_pictures(html: &str, local: &Local) -> String {
    if local.is_empty() {
        return html.to_string();
    }
    IMG_SRC
        .replace_all(html, |c: &regex::Captures| {
            match local.get(&unescape(&c[2])) {
                Some(copy) => format!(r#"{}{}""#, &c[1], file_url(copy)),
                None => c[0].to_string(),
            }
        })
        .into_owned()
}

/// What runs once the scripts have loaded: the Markdown drawn and cleaned
/// in place of the elements that name it (so its paragraphs are the page's
/// own blocks, which pages break between), its pictures kept here named by
/// their copies before they are part of the page (and so before they
/// load), each diagram drawn, and then, once everything has loaded or
/// failed, the page measured.
const RUN: &str = r#"
const sources = JSON.parse(document.getElementById("markdown").textContent);
const local = JSON.parse(document.getElementById("local").textContent);
const safe = typeof marked !== "undefined" && typeof DOMPurify !== "undefined";
document.querySelectorAll("[data-md]").forEach((el) => {
  const source = sources[Number(el.dataset.md)] ?? "";
  if (safe) {
    const t = document.createElement("template");
    t.innerHTML = DOMPurify.sanitize(marked.parse(source));
    t.content.querySelectorAll("img[src]").forEach((img) => {
      const copy = local[img.getAttribute("src")];
      if (copy) img.setAttribute("src", copy);
    });
    el.replaceWith(t.content);
  } else {
    const pre = document.createElement("pre");
    pre.textContent = source;
    el.replaceWith(pre);
  }
});
document.querySelectorAll("pre > code.language-mermaid").forEach((code) => {
  const pre = document.createElement("pre");
  pre.className = "mermaid";
  pre.textContent = code.textContent;
  code.parentElement.replaceWith(pre);
});
const measure = () => {
  const body = document.querySelector(".markdown-body");
  const box = body.getBoundingClientRect();
  const blocks = [...body.children].map((el) => {
    const r = el.getBoundingClientRect();
    return [r.top - box.top, r.bottom - box.top];
  });
  const sheet = document.querySelector('link[rel="stylesheet"]');
  document.getElementById("measured").textContent = JSON.stringify({
    top: box.top + window.scrollY,
    height: box.height,
    blocks,
    loaded: safe && typeof mermaid !== "undefined",
    complete: !!sheet?.sheet && [...document.images].every((img) => img.complete && img.naturalWidth > 0),
  });
  document.body.dataset.done = "1";
};
const settled = new Promise((done) => {
  if (document.readyState === "complete") done();
  else window.addEventListener("load", done);
});
let drawn = Promise.resolve();
if (typeof mermaid !== "undefined") {
  mermaid.initialize({ startOnLoad: false, theme: "dark" });
  drawn = mermaid.run().catch(() => {});
}
Promise.all([settled, drawn]).then(measure);
"#;

/// The strips that say which page is which, drawn below the body where
/// nothing of it is, one under the other, to be laid over each page's top
/// (`super::render::compose`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bands {
    /// Where the first starts, in CSS pixels from the page's top.
    pub top: f64,
    /// Each one's words.
    pub labels: Vec<String>,
}

/// A page's strip's words: "Page 2 of 3", and "↓ more" while more follow.
pub fn band_label(page: usize, total: usize) -> String {
    format!(
        "Page {page} of {total}{}",
        if page < total {
            "\u{a0}\u{a0}↓ more"
        } else {
            ""
        }
    )
}

/// The page, `width` CSS pixels wide, a diagram no taller than `fit`, with
/// the strips when there are several pages, naming the copies in `local`
/// in place of their links.
pub fn html(
    parts: &Parts,
    width: u32,
    fit: u32,
    band: u32,
    bands: &Bands,
    local: &Local,
) -> String {
    let sources = serde_json::to_string(&parts.markdown)
        .unwrap_or_else(|_| "[]".into())
        .replace('<', "\\u003c");
    let copies: BTreeMap<&str, String> = local
        .iter()
        .map(|(url, copy)| (url.as_str(), file_url(copy)))
        .collect();
    let copies = serde_json::to_string(&copies)
        .unwrap_or_else(|_| "{}".into())
        .replace('<', "\\u003c");
    let strips: String = bands
        .labels
        .iter()
        .enumerate()
        .map(|(i, label)| {
            format!(
                r#"<div class="band" style="top:{}px"><span>{}</span></div>"#,
                bands.top + (i as f64) * band as f64,
                escape(label)
            )
        })
        .collect();
    let [r, g, b] = BACKGROUND;
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8">
{css}
<style>
html {{ overflow: hidden; }}
body {{ background: #{r:02x}{g:02x}{b:02x}; margin: 0; }}
.markdown-body {{ box-sizing: border-box; width: {width}px; padding: 24px; font-size: 14px; }}
pre.mermaid {{ background: transparent; }}
pre.mermaid svg {{ max-height: {fit}px; max-width: 100%; height: auto; }}
.band {{ position: absolute; left: 0; width: {width}px; height: {band}px; display: flex; justify-content: flex-end; align-items: center; padding: 0 12px; box-sizing: border-box; }}
.band span {{ padding: 3px 10px; border-radius: 12px; font: 600 12px -apple-system, system-ui, sans-serif; color: #fff; background: rgba(31, 111, 235, 0.92); }}
</style>
</head><body><article class="markdown-body">
<h1>{title} <span style="color:#7d8590">{number}</span></h1>
<p><em>{meta}</em></p>
{body}
</article>
{strips}<pre id="measured" hidden></pre>
<script type="application/json" id="markdown">{sources}</script>
<script type="application/json" id="local">{copies}</script>
{marked}
{purify}
{mermaid}
<script>{run}</script>
</body></html>"#,
        css = stylesheet(local),
        title = escape(&parts.title),
        number = escape(&parts.number),
        meta = parts.meta,
        body = with_local_pictures(&diagrams(&parts.html), local),
        marked = script(MARKED, local),
        purify = script(PURIFY, local),
        mermaid = script(MERMAID, local),
        run = RUN,
    )
}

// ── What a PR and a ticket are drawn from ───────────────────────────────────

/// A PR as the page's parts, from GitHub's REST answer with its rendered
/// HTML: its state as `gh pr view` named it, its author and branches.
pub fn pr_parts(pr: &Value) -> Parts {
    let text = |path: &str| pr.pointer(path).and_then(Value::as_str).unwrap_or("");
    let state = if pr.get("draft").and_then(Value::as_bool) == Some(true) {
        "Draft".to_string()
    } else if pr.get("merged_at").is_some_and(|m| !m.is_null()) {
        "MERGED".to_string()
    } else {
        text("/state").to_uppercase()
    };
    let number = pr.get("number").and_then(Value::as_u64).unwrap_or(0);
    Parts {
        title: text("/title").to_string(),
        number: format!("#{number}"),
        meta: format!(
            "{} · {} wants to merge <code>{}</code> into <code>{}</code>",
            escape(&state),
            escape(text("/user/login")),
            escape(text("/head/ref")),
            escape(text("/base/ref"))
        ),
        html: text("/body_html").to_string(),
        markdown: Vec::new(),
    }
}

/// The ticket's fields the page shows, as pr-image.js asked for them.
pub const TICKET_QUERY: &str = "query($id: String!) { issue(id: $id) { \
    identifier title description priorityLabel \
    state { name } assignee { name } project { name } labels { nodes { name } } \
    children { nodes { identifier title state { name } } } \
    comments(first: 100) { nodes { body createdAt user { name } botActor { name } } } } }";

fn name(v: &Value, path: &str) -> Option<String> {
    v.pointer(path)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn nodes<'a>(v: &'a Value, path: &str) -> Vec<&'a Value> {
    v.pointer(path)
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default()
}

/// When a comment was made, as `toLocaleString("en-US", { month: "short",
/// day: "numeric", hour: "numeric", minute: "2-digit" })` wrote it: "Nov 6,
/// 3:04 PM", in the local time zone.
fn when(iso: &str) -> String {
    match iso.parse::<jiff::Timestamp>() {
        Ok(at) => crate::util::local_at_ms(at.as_millisecond())
            .strftime("%b %-d, %-I:%M %p")
            .to_string(),
        Err(_) => String::new(),
    }
}

/// A ticket as the page's parts: its facts, its description, sub-issues
/// and comments, oldest first, the Markdown left for the page to draw.
pub fn ticket_parts(issue: &Value) -> Parts {
    let mut markdown =
        vec![name(issue, "/description").unwrap_or_else(|| "_No description._".into())];
    let mut facts: Vec<String> = Vec::new();
    facts.extend(name(issue, "/state/name"));
    facts.push(name(issue, "/assignee/name").unwrap_or_else(|| "Unassigned".into()));
    facts.extend(name(issue, "/priorityLabel"));
    facts.extend(name(issue, "/project/name"));
    facts.extend(
        nodes(issue, "/labels/nodes")
            .iter()
            .filter_map(|l| name(l, "/name")),
    );
    let children = nodes(issue, "/children/nodes");
    let children = if children.is_empty() {
        String::new()
    } else {
        let rows: String = children
            .iter()
            .map(|c| {
                format!(
                    r#"<li><code>{}</code> {} <span style="color:#7d8590">{}</span></li>"#,
                    escape(&name(c, "/identifier").unwrap_or_default()),
                    escape(&name(c, "/title").unwrap_or_default()),
                    escape(&name(c, "/state/name").unwrap_or_default())
                )
            })
            .collect();
        format!("<h2>Sub-issues</h2><ul>{rows}</ul>")
    };
    let mut comments = nodes(issue, "/comments/nodes");
    comments.sort_by_key(|c| {
        c.get("createdAt")
            .and_then(Value::as_str)
            .and_then(|t| t.parse::<jiff::Timestamp>().ok())
            .map(|t| t.as_millisecond())
            .unwrap_or(0)
    });
    let thread = if comments.is_empty() {
        String::new()
    } else {
        let mut out = String::from("<h2>Comments</h2>");
        for c in comments {
            markdown.push(name(c, "/body").unwrap_or_default());
            let who = name(c, "/user/name")
                .or_else(|| name(c, "/botActor/name"))
                .unwrap_or_else(|| "Someone".into());
            let at = when(c.get("createdAt").and_then(Value::as_str).unwrap_or(""));
            out.push_str(&format!(
                r#"<h4>{} <span style="color:#7d8590">{}</span></h4><div data-md="{}"></div>"#,
                escape(&who),
                escape(&at),
                markdown.len() - 1
            ));
        }
        out
    };
    Parts {
        title: name(issue, "/title").unwrap_or_default(),
        number: name(issue, "/identifier").unwrap_or_default(),
        meta: facts
            .iter()
            .map(|f| escape(f))
            .collect::<Vec<_>>()
            .join(" · "),
        html: format!(r#"<div data-md="0"></div>{children}{thread}"#),
        markdown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_page_loads_only_pinned_files_each_checked_by_its_hash() {
        let page = html(
            &Parts::default(),
            600,
            400,
            30,
            &Bands::default(),
            &Local::new(),
        );
        for pin in PINS {
            assert!(pin.url.contains('@'), "pinned to a version: {}", pin.url);
            assert!(pin.integrity.starts_with("sha384-"));
            assert!(
                page.contains(&format!(
                    r#"="{}" integrity="{}" crossorigin="anonymous""#,
                    pin.url, pin.integrity
                )),
                "{} loaded with its hash",
                pin.url
            );
        }
        let loads = Regex::new(r#"(?:src|href)="https?://[^"]+""#).unwrap();
        assert_eq!(
            loads.find_iter(&page).count(),
            PINS.len(),
            "nothing else is loaded from the web"
        );
    }

    #[test]
    fn the_page_names_the_copies_kept_here_in_place_of_their_links() {
        let mut local = Local::new();
        for pin in PINS {
            let name = pin.url.rsplit('/').next().unwrap();
            local.insert(pin.url.into(), PathBuf::from(format!("/c/my cache/{name}")));
        }
        let picture = "https://private-user-images.githubusercontent.com/1/a.png?jwt=x&x=1";
        local.insert(picture.into(), PathBuf::from("/c/a.png"));
        let parts = Parts {
            html: format!(
                r#"<p><img src="{}" alt="a"> <img src="https://example.com/b.png"></p>"#,
                picture.replace('&', "&amp;")
            ),
            ..Parts::default()
        };
        let page = html(&parts, 600, 400, 30, &Bands::default(), &local);
        assert!(page.contains(
            r#"<link rel="stylesheet" href="file:///c/my%20cache/github-markdown-dark.css">"#
        ));
        assert!(page.contains(r#"<script src="file:///c/my%20cache/mermaid.min.js"></script>"#));
        assert!(
            !page.contains("integrity="),
            "a copy was checked as it was kept, and the browser can't check a file's"
        );
        assert!(page.contains(r#"<img src="file:///c/a.png" alt="a">"#));
        assert!(
            page.contains(r#"<img src="https://example.com/b.png">"#),
            "a picture with no copy is loaded as before"
        );
        // A ticket's pictures are named by their copies in the page's script,
        // after DOMPurify has cleaned the Markdown.
        assert!(page.contains(&format!(r#""{picture}":"file:///c/a.png""#)));
        assert!(page.contains("const copy = local[img.getAttribute(\"src\")]"));
        let cleaned = page.find("DOMPurify.sanitize(").unwrap();
        assert!(page.find("img.setAttribute(\"src\", copy)").unwrap() > cleaned);
    }

    #[test]
    fn the_pictures_are_found_in_a_prs_html_and_a_tickets_markdown() {
        let parts = Parts {
            html: concat!(
                r#"<a href="https://x.example/p"><img src="https://camo.githubusercontent.com/a/b" style="max-width:100%"></a>"#,
                r#"<IMG alt="q" SRC="https://img.example/q.png?a=1&amp;b=2"><img src="http://plain.example/p.png">"#,
                r#"<img src="https://camo.githubusercontent.com/a/b">"#
            )
            .into(),
            markdown: vec![
                "See ![shot](https://uploads.linear.app/x/y.png?signature=s) and [a link](https://x.example)".into(),
                r#"<img width="20" src='https://img.example/r.gif'> ![](<https://img.example/s.png>)"#.into(),
            ],
            ..Parts::default()
        };
        assert_eq!(
            pictures(&parts),
            [
                "https://camo.githubusercontent.com/a/b",
                "https://img.example/q.png?a=1&b=2",
                "https://uploads.linear.app/x/y.png?signature=s",
                "https://img.example/r.gif",
                "https://img.example/s.png",
            ]
        );
        assert_eq!(
            file_url(Path::new("/a b/c#d?.png")),
            "file:///a%20b/c%23d%3F.png"
        );
    }

    #[test]
    fn a_tickets_markdown_is_cleaned_before_it_is_part_of_the_page() {
        let issue = json!({
            "identifier": "ENG-4", "title": "<b>Bold</b> & co",
            "description": "Hi </script><script>alert(1)</script> <img src=x onerror=alert(2)>",
            "state": { "name": "Todo" }, "assignee": null, "priorityLabel": "High",
            "project": null, "labels": { "nodes": [{ "name": "bug" }] },
            "children": { "nodes": [{ "identifier": "ENG-5", "title": "<i>x</i>", "state": { "name": "Done" } }] },
            "comments": { "nodes": [
                { "body": "second", "createdAt": "2026-11-06T15:04:00Z", "user": { "name": "Jo" } },
                { "body": "first", "createdAt": "2026-11-05T09:00:00Z", "user": null, "botActor": { "name": "Bot" } }
            ] }
        });
        let parts = ticket_parts(&issue);
        assert_eq!(
            parts.markdown[1..],
            ["first".to_string(), "second".to_string()]
        );
        assert_eq!(parts.meta, "Todo · Unassigned · High · bug");
        let page = html(&parts, 600, 400, 30, &Bands::default(), &Local::new());
        assert!(
            page.contains("DOMPurify.sanitize(marked.parse(source))"),
            "Markdown becomes HTML only through DOMPurify"
        );
        assert!(
            !page.contains("<script>alert(1)"),
            "the description is JSON, its tags unable to close the script that holds it"
        );
        let lt = "\\u003c";
        assert!(page.contains(&format!("{lt}/script>{lt}script>alert(1)")));
        assert!(!page.contains("<img src=x"));
        assert!(page.contains("&lt;b&gt;Bold&lt;/b&gt; &amp; co"));
        assert!(page.contains("<code>ENG-5</code> &lt;i&gt;x&lt;/i&gt;"));
        assert!(page.contains(r#"<h4>Bot "#) && page.contains(r#"<div data-md="1"></div>"#));
    }

    #[test]
    fn a_prs_html_is_githubs_with_its_diagrams_given_to_mermaid() {
        let pr = json!({
            "number": 12, "title": "Fix it", "state": "open", "draft": false, "merged_at": null,
            "user": { "login": "jo" }, "head": { "ref": "eng-4" }, "base": { "ref": "main" },
            "body_html": "<p>Hi</p><section class=\"js-render-needs-enrichment\" data-type=\"mermaid\"><div><pre lang=\"mermaid\" aria-label=\"Raw mermaid code\">graph TD; A--&gt;B</pre></div></section>"
        });
        let parts = pr_parts(&pr);
        assert_eq!(parts.number, "#12");
        assert_eq!(
            parts.meta,
            "OPEN · jo wants to merge <code>eng-4</code> into <code>main</code>"
        );
        assert_eq!(
            diagrams(&parts.html),
            "<p>Hi</p><pre class=\"mermaid\">graph TD; A--&gt;B</pre>"
        );
        let merged = json!({ "number": 1, "state": "closed", "merged_at": "2026-01-01T00:00:00Z" });
        assert!(pr_parts(&merged).meta.starts_with("MERGED · "));
    }

    #[test]
    fn several_pages_each_say_which_they_are() {
        assert_eq!(band_label(1, 3), "Page 1 of 3\u{a0}\u{a0}↓ more");
        assert_eq!(band_label(3, 3), "Page 3 of 3");
        let bands = Bands {
            top: 500.0,
            labels: vec![band_label(1, 2), band_label(2, 2)],
        };
        let page = html(&Parts::default(), 600, 400, 30, &bands, &Local::new());
        assert!(page.contains(r#"<div class="band" style="top:500px"><span>Page 1 of 2"#));
        assert!(page.contains(r#"<div class="band" style="top:530px"><span>Page 2 of 2</span>"#));
    }
}
