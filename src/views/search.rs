//! Search everything (M-S-f; tmux/search.sh and lib/search.js): one query
//! asked of your notes, Linear, GitHub and Slack at once, each source's
//! results shown as soon as it answers, so the quick ones are there while
//! Slack is still looking. Enter reads the chosen one beside the pane, in
//! the link view, a note as its Markdown; a Slack message, which can't be
//! read here, opens in the browser.
//!
//! Notes: every Markdown note in `$NOTES_DIR` or `~/Notes` (nothing hidden,
//! so not `.trash`) holding every word, newest first, read here and sent
//! nowhere, each as a `file://` link the link view reads. Linear: its issue
//! search, with the key `integrations::linear` finds. GitHub: issues and
//! pull requests in the repositories the tabs are in, through its search
//! API rather than `gh search issues`. Slack: Slack's own search through
//! Claude's connector (`connector::call_connector`), taking about 15
//! seconds. GitHub and Linear refuse while their settings are off; Slack is
//! asked only with search_slack on.
//!
//! The rows are kept in the sources' order, whatever order they answer in,
//! each link once (`merge`), so the list doesn't reshuffle under the cursor
//! as a slow source arrives the way fzf's appended rows could.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use crate::integrations::{github, linear};
use crate::mux::PaneId;
use crate::{connector, settings};

/// Results each source gives at most.
pub const LIMIT: usize = 20;
const SOURCE: &str = "search";

/// Where a result comes from, in the order they are listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    Notes,
    Linear,
    GitHub,
    Slack,
}

impl Source {
    pub const ALL: [Source; 4] = [Source::Notes, Source::Linear, Source::GitHub, Source::Slack];

    /// Its name in the list's last column.
    pub fn name(self) -> &'static str {
        match self {
            Source::Notes => "Note",
            Source::Linear => "Linear",
            Source::GitHub => "GitHub",
            Source::Slack => "Slack",
        }
    }
}

/// One result: what it is, a word about it, and the link it reads (empty
/// for a row that only says something, as Slack's connector missing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub source: Source,
    pub text: String,
    pub detail: String,
    pub link: String,
}

/// A search under way on a client: its number, what was asked, the rows so
/// far, the sources still looking, and the pane its results are read
/// beside.
#[derive(Clone, Debug, Default)]
pub struct Running {
    pub id: u64,
    pub query: String,
    pub rows: Vec<Found>,
    pub waiting: Vec<Source>,
    pub pane: Option<PaneId>,
}

/// A source's answer taken into the rows: in the sources' order, each
/// source's rows in its own order, a link found already left out, and a
/// source answering again taking the place of its last answer.
pub fn merge(rows: &mut Vec<Found>, source: Source, found: Vec<Found>) {
    rows.retain(|r| r.source != source);
    for f in found {
        if f.link.is_empty() || !rows.iter().any(|r| r.link == f.link) {
            rows.push(Found { source, ..f });
        }
    }
    // A stable sort, so each source's own order stays.
    rows.sort_by_key(|r| r.source);
}

/// Text on one line: tabs and line breaks as spaces, runs of space as one.
pub fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Everything but letters, digits and `-_.~` written `%XX`, as
/// `encodeURIComponent` with `'()*!` escaped too.
pub fn encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A file as the `file://` link the link view reads, each part of its path
/// encoded.
pub fn file_url(path: &Path) -> String {
    let parts: Vec<String> = path.to_string_lossy().split('/').map(encode).collect();
    format!("file://{}", parts.join("/"))
}

fn markdown_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => markdown_files(&path, out),
            Ok(t) if t.is_file() && path.extension().is_some_and(|e| e == "md") => out.push(path),
            _ => {}
        }
    }
}

static ITEM_MARK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*[-*]\s*(\[[ xX]\]\s*)?").unwrap());

/// The notes in `dir` holding every word of `query`, newest first: each
/// note's title (its first line, else its name) and the first line after
/// it holding the first word.
pub fn notes(dir: &Path, query: &str) -> Vec<Found> {
    let words: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut files = Vec::new();
    markdown_files(dir, &mut files);
    let mut hits: Vec<(std::time::SystemTime, PathBuf, String)> = files
        .into_iter()
        .filter_map(|file| {
            // Without the note's ID line (src/notes/id.rs), which every
            // note has.
            let text = crate::notes::id::strip(&std::fs::read_to_string(&file).ok()?);
            let lower = text.to_lowercase();
            if !words.iter().all(|w| lower.contains(w.as_str())) {
                return None;
            }
            let time = std::fs::metadata(&file).and_then(|m| m.modified()).ok()?;
            Some((time, file, text))
        })
        .collect();
    hits.sort_by_key(|h| std::cmp::Reverse(h.0));
    hits.into_iter()
        .take(LIMIT)
        .map(|(_, file, text)| {
            let mut lines = text.lines();
            let first = lines.next().unwrap_or("");
            let title = first.strip_prefix('#').unwrap_or(first).trim().to_string();
            let title = if title.is_empty() {
                file.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            } else {
                title
            };
            let hit = lines
                .find(|l| l.to_lowercase().contains(&words[0]))
                .map(|l| {
                    ITEM_MARK
                        .replace(l, "")
                        .chars()
                        .take(120)
                        .collect::<String>()
                })
                .unwrap_or_default();
            Found {
                source: Source::Notes,
                text: clean(&title),
                detail: clean(&hit),
                link: file_url(&file),
            }
        })
        .collect()
}

/// Linear's issue search.
pub async fn linear(query: &str) -> Result<Vec<Found>, String> {
    let data = linear::graphql(
        &format!(
            "query($q: String!) {{ searchIssues(term: $q, first: {LIMIT}) {{ nodes {{ identifier title url state {{ name }} }} }} }}"
        ),
        json!({ "q": query }),
        SOURCE,
    )
    .await
    .map_err(|e| e.to_string())?;
    let nodes = data
        .pointer("/searchIssues/nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(nodes
        .iter()
        .map(|t| {
            let text = |p: &str| {
                t.pointer(p)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            };
            Found {
                source: Source::Linear,
                text: clean(&format!("{} {}", text("/identifier"), text("/title"))),
                detail: text("/state/name"),
                link: text("/url"),
            }
        })
        .collect())
}

/// Issues and pull requests in the repositories of `dirs`, through
/// GitHub's search.
pub async fn github(query: &str, dirs: Vec<PathBuf>) -> Result<Vec<Found>, String> {
    let mut repos: Vec<String> = Vec::new();
    for dir in dirs {
        if let Ok(repo) = github::repo_of(&dir).await {
            let name = repo.to_string();
            if !repos.contains(&name) {
                repos.push(name);
            }
        }
    }
    if repos.is_empty() {
        return Ok(Vec::new());
    }
    let q = format!(
        "{query} {}",
        repos
            .iter()
            .map(|r| format!("repo:{r}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let answer = github::rest(
        "GET",
        &format!("/search/issues?q={}&per_page={LIMIT}", encode(&q)),
        None,
        SOURCE,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(github_results(&answer))
}

/// GitHub's search answer as rows.
pub fn github_results(answer: &Value) -> Vec<Found> {
    let items = answer
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    items
        .iter()
        .map(|i| {
            let text = |k: &str| i.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let repo = text("repository_url")
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_string();
            let number = i
                .get("number")
                .map(crate::util::js_string)
                .unwrap_or_default();
            let kind = if i.get("pull_request").is_some_and(|p| !p.is_null()) {
                "PR"
            } else {
                "issue"
            };
            Found {
                source: Source::GitHub,
                text: clean(&format!("{repo}#{number} {}", text("title"))),
                detail: format!("{kind}, {}", text("state")),
                link: text("html_url"),
            }
        })
        .collect()
}

/// Slack's search through Claude's connector. A connector that isn't
/// connected is a row saying so, so it isn't taken for no results.
pub async fn slack(query: &str) -> Result<Vec<Found>, String> {
    let input = json!({ "query": query, "limit": LIMIT });
    match connector::call_connector(
        "mcp__claude_ai_Slack__slack_search_public_and_private",
        &input,
        None,
    )
    .await
    {
        Ok(text) => {
            let results = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v.get("results").cloned())
                .map(|r| match r {
                    Value::String(s) => s,
                    other => other.to_string(),
                })
                .unwrap_or(text);
            Ok(slack_results(&results))
        }
        Err(e) if e.not_connected => Ok(vec![Found {
            source: Source::Slack,
            text: e.message,
            detail: String::new(),
            link: String::new(),
        }]),
        Err(e) => Err(e.message),
    }
}

static RESULT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^### Result \d+ of \d+$").unwrap());
static PERMALINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^Permalink: \[[^\]]*\]\(([^)]+)\)").unwrap());
static MENTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<@\w+\|([^>]+)>").unwrap());
static NAMED_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<(https?:[^|>]+)\|([^>]+)>").unwrap());
static BARE_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<(https?:[^>]+)>").unwrap());
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" \(ID: \w+\).*$").unwrap());
static EMAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" <[^>]*>").unwrap());

/// The connector's text as rows: each result's message, who wrote it,
/// where and when, and its permalink; one with no permalink left out.
pub fn slack_results(text: &str) -> Vec<Found> {
    RESULT
        .split(text)
        .skip(1)
        .filter_map(|block| {
            let field = |name: &str| {
                block
                    .lines()
                    .find_map(|l| l.strip_prefix(&format!("{name}: ")))
                    .unwrap_or("")
                    .to_string()
            };
            let link = PERMALINK.captures(block)?[1].replace("\\/", "/");
            // The message: after `Text:` up to its context or the end.
            let mut body = Vec::new();
            let mut lines = block.lines().skip_while(|l| !l.starts_with("Text:"));
            if let Some(first) = lines.next() {
                let rest = first.trim_start_matches("Text:").trim();
                if !rest.is_empty() {
                    body.push(rest.to_string());
                }
                for l in lines {
                    if l.starts_with("Context before:")
                        || l.starts_with("Context after:")
                        || l.starts_with("---")
                    {
                        break;
                    }
                    body.push(l.to_string());
                }
            }
            let body = body.join("\n");
            let body = MENTION.replace_all(&body, "@$1");
            let body = NAMED_LINK.replace_all(&body, "$2");
            let body = BARE_LINK.replace_all(&body, "$1");
            let channel = ID.replace(&field("Channel"), "").into_owned();
            let who = ID
                .replace(&EMAIL.replace(&field("From"), ""), "")
                .into_owned();
            let time: String = field("Time").chars().take(16).collect();
            Some(Found {
                source: Source::Slack,
                text: clean(&body).chars().take(140).collect(),
                detail: clean(&format!("{who} in {channel}, {time}")),
                link,
            })
        })
        .collect()
}

/// Whether search everything asks Slack (the search_slack setting).
pub fn asks_slack() -> bool {
    settings::is_on("search_slack")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(source: Source, link: &str) -> Found {
        Found {
            source,
            text: link.to_string(),
            detail: String::new(),
            link: link.to_string(),
        }
    }

    #[test]
    fn answers_merge_in_the_sources_order_each_link_once() {
        let mut rows = Vec::new();
        merge(&mut rows, Source::Slack, vec![row(Source::Slack, "s1")]);
        merge(
            &mut rows,
            Source::GitHub,
            vec![row(Source::GitHub, "g1"), row(Source::GitHub, "g2")],
        );
        merge(&mut rows, Source::Notes, vec![row(Source::Notes, "n1")]);
        merge(
            &mut rows,
            Source::Linear,
            vec![row(Source::Linear, "g1"), row(Source::Linear, "l1")],
        );
        let links: Vec<&str> = rows.iter().map(|r| r.link.as_str()).collect();
        assert_eq!(
            links,
            ["n1", "l1", "g1", "g2", "s1"],
            "a link GitHub gave first isn't Linear's again"
        );
        merge(&mut rows, Source::GitHub, vec![row(Source::GitHub, "g3")]);
        let links: Vec<&str> = rows.iter().map(|r| r.link.as_str()).collect();
        assert_eq!(
            links,
            ["n1", "l1", "g3", "s1"],
            "a source answering again replaces its rows"
        );
        let mut said = Vec::new();
        merge(
            &mut said,
            Source::Slack,
            vec![row(Source::Slack, ""), row(Source::Slack, "")],
        );
        assert_eq!(
            said.len(),
            2,
            "rows that only say something aren't links to dedupe"
        );
    }

    #[test]
    fn notes_holding_every_word_come_newest_first_as_links_and_nothing_hidden() {
        let dir = std::env::temp_dir().join(format!("sb-search-{}", std::process::id()));
        let notes_dir = dir.join("Notes");
        std::fs::create_dir_all(notes_dir.join("Work")).unwrap();
        std::fs::create_dir_all(notes_dir.join(".trash")).unwrap();
        std::fs::write(
            notes_dir.join("Old plan.md"),
            "# Old plan\n\n- [ ] deploy the freeze fix\n",
        )
        .unwrap();
        std::fs::write(
            notes_dir.join("Work/Carl's (draft).md"),
            "# Carl's (draft)\n\nThe Deploy freeze is on\n",
        )
        .unwrap();
        std::fs::write(notes_dir.join("Other.md"), "# Other\n\ndeploy only\n").unwrap();
        std::fs::write(
            notes_dir.join(".trash/Gone.md"),
            "# Gone\n\ndeploy freeze\n",
        )
        .unwrap();
        let old = std::fs::File::options()
            .write(true)
            .open(notes_dir.join("Old plan.md"))
            .unwrap();
        old.set_modified(
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_577_836_800),
        )
        .unwrap();
        let found = notes(&notes_dir, "deploy FREEZE");
        let rows: Vec<(String, String, String)> = found
            .iter()
            .map(|f| (f.text.clone(), f.detail.clone(), f.link.clone()))
            .collect();
        let base = notes_dir.display().to_string();
        let encoded = |rest: &str| format!("{}/{rest}", file_url(Path::new(&base)));
        assert_eq!(
            rows,
            vec![
                (
                    "Carl's (draft)".into(),
                    "The Deploy freeze is on".into(),
                    encoded("Work/Carl%27s%20%28draft%29.md")
                ),
                (
                    "Old plan".into(),
                    "deploy the freeze fix".into(),
                    encoded("Old%20plan.md")
                ),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn slacks_answer_is_read_into_rows() {
        let text = "Found 2.\n### Result 1 of 2\nChannel: #deploys (ID: C1)\nFrom: Jo <jo@x.com> (ID: U1)\n\
            Time: 2026-11-05 10:30:12 EST\nPermalink: [link](https:\\/\\/acme.slack.com\\/archives\\/C1\\/p1)\n\
            Text:\nThe <@U2|sam> freeze is <https://x.com/a|on>\nsee <https://x.com/b>\nContext after:\nignored\n\
            ### Result 2 of 2\nChannel: #x\nFrom: Bot\nText: no link here\n";
        let rows = slack_results(text);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text, "The @sam freeze is on see https://x.com/b");
        assert_eq!(rows[0].detail, "Jo in #deploys, 2026-11-05 10:30");
        assert_eq!(rows[0].link, "https://acme.slack.com/archives/C1/p1");
    }

    #[test]
    fn githubs_answer_is_read_into_rows() {
        let answer = json!({ "items": [
            { "repository_url": "https://api.github.com/repos/acme/widgets", "number": 12,
              "title": "Fix the\nlogin", "html_url": "https://github.com/acme/widgets/pull/12",
              "state": "open", "pull_request": { "url": "x" } },
            { "repository_url": "https://api.github.com/repos/acme/widgets", "number": 3,
              "title": "Slow", "html_url": "https://github.com/acme/widgets/issues/3", "state": "closed" }
        ] });
        let rows = github_results(&answer);
        assert_eq!(rows[0].text, "widgets#12 Fix the login");
        assert_eq!(rows[0].detail, "PR, open");
        assert_eq!(rows[1].detail, "issue, closed");
    }
}
