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
//! The page measures itself once drawn: where the body's blocks start and
//! end, which the pages are cut between (`super::render`), written as JSON
//! into a hidden element that Chrome's `--dump-dom` prints.

use serde::{Deserialize, Serialize};
use serde_json::Value;
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

/// A script or stylesheet tag for a pinned file.
fn script(pin: Pin) -> String {
    format!(
        r#"<script src="{}" integrity="{}" crossorigin="anonymous"></script>"#,
        pin.url, pin.integrity
    )
}

/// What runs once the scripts have loaded: the Markdown drawn and cleaned
/// in place of the elements that name it (so its paragraphs are the page's
/// own blocks, which pages break between), each diagram drawn, and then the
/// page measured.
const RUN: &str = r#"
const sources = JSON.parse(document.getElementById("markdown").textContent);
const safe = typeof marked !== "undefined" && typeof DOMPurify !== "undefined";
document.querySelectorAll("[data-md]").forEach((el) => {
  const source = sources[Number(el.dataset.md)] ?? "";
  if (safe) {
    const t = document.createElement("template");
    t.innerHTML = DOMPurify.sanitize(marked.parse(source));
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
  document.getElementById("measured").textContent = JSON.stringify({
    top: box.top + window.scrollY,
    height: box.height,
    blocks,
    loaded: safe && typeof mermaid !== "undefined",
  });
  document.body.dataset.done = "1";
};
if (typeof mermaid === "undefined") {
  measure();
} else {
  mermaid.initialize({ startOnLoad: false, theme: "dark" });
  mermaid.run().catch(() => {}).finally(measure);
}
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
/// the strips when there are several pages.
pub fn html(parts: &Parts, width: u32, fit: u32, band: u32, bands: &Bands) -> String {
    let sources = serde_json::to_string(&parts.markdown)
        .unwrap_or_else(|_| "[]".into())
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
<link rel="stylesheet" href="{css}" integrity="{css_integrity}" crossorigin="anonymous">
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
{marked}
{purify}
{mermaid}
<script>{run}</script>
</body></html>"#,
        css = STYLESHEET.url,
        css_integrity = STYLESHEET.integrity,
        title = escape(&parts.title),
        number = escape(&parts.number),
        meta = parts.meta,
        body = diagrams(&parts.html),
        marked = script(MARKED),
        purify = script(PURIFY),
        mermaid = script(MERMAID),
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
        let page = html(&Parts::default(), 600, 400, 30, &Bands::default());
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
        let page = html(&parts, 600, 400, 30, &Bands::default());
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
        let page = html(&Parts::default(), 600, 400, 30, &bands);
        assert!(page.contains(r#"<div class="band" style="top:500px"><span>Page 1 of 2"#));
        assert!(page.contains(r#"<div class="band" style="top:530px"><span>Page 2 of 2</span>"#));
    }
}
