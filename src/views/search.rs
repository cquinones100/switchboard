//! Search everything (M-S-f; tmux/search.sh and lib/search.js): one query
//! asked of the open tabs, your notes, Linear, GitHub and Slack at once,
//! each source's results shown as soon as it answers, so the quick ones are
//! there while Slack is still looking. Enter reads the chosen one beside
//! the pane, in the link view, a note as its Markdown; a Slack message,
//! which can't be read here, opens in the browser; a tab is gone to.
//!
//! Tabs: the open tabs, the one the search started from among them (last
//! of those found the same way, marked "this tab"), by their label, with
//! `tab_search_panes` on by what their panes show, matched and ranked by
//! M-j's picker's search (`mission::pane_search`) on copies the server made
//! of the panes, and with `tab_search_conversation` on by what was said in
//! their Claude's conversation (`conversation`), all searched off its task;
//! new with the Rust port. A tab found in its conversation is gone to with
//! its history (M-h) at the moment it was said.
//!
//! Notes: every Markdown note in `$NOTES_DIR` or `~/Notes` (nothing hidden,
//! so not `.trash`) holding every word, newest first, read here and sent
//! nowhere, each as a `file://` link the link view reads. Linear: its issue
//! search, with the key `integrations::linear` finds. GitHub: issues and
//! pull requests in the repositories the tabs are in, through its search
//! API rather than `gh search issues`. Slack: Slack's own search, through
//! your own token (`day::slack_api`) while one is stored and Settings'
//! `slack_token_reads` is on, else through Claude's connector
//! (`connector::call_connector`), taking about 15 seconds. GitHub and Linear refuse while their settings are off; Slack is
//! asked only with search_slack on.
//!
//! Which sources are asked is M-F's chips' (src/views/search_box.rs): a
//! chip per source Settings has on (`available`), those turned off kept
//! for the next M-F in `~/.local/state/switchboard/search.json`
//! (`read_off`, `write_off`). A query may open with the sources to ask,
//! each a word and a colon (`note: slack: deploy`, `scope`), which lights
//! those chips alone, so a note search doesn't wait beside Slack's.
//!
//! The rows are kept in the sources' order, whatever order they answer in,
//! each link once (`merge`), so the list doesn't reshuffle under the cursor
//! as a slow source arrives the way fzf's appended rows could.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use super::conversation;
use crate::integrations::{github, linear};
use crate::mission::pane_search;
use crate::mux::{PaneId, TabId};
use crate::{connector, settings};

/// Results each source gives at most.
pub const LIMIT: usize = 20;
const SOURCE: &str = "search";

/// Where a result comes from, in the order they are listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    Tabs,
    Notes,
    Linear,
    GitHub,
    Slack,
}

impl Source {
    pub const ALL: [Source; 5] = [
        Source::Tabs,
        Source::Notes,
        Source::Linear,
        Source::GitHub,
        Source::Slack,
    ];

    /// Its name in the list's last column.
    pub fn name(self) -> &'static str {
        match self {
            Source::Tabs => "Tab",
            Source::Notes => "Note",
            Source::Linear => "Linear",
            Source::GitHub => "GitHub",
            Source::Slack => "Slack",
        }
    }

    /// Its name in a search's title while a scope names it.
    pub fn scoped_name(self) -> &'static str {
        match self {
            Source::Tabs => "tabs",
            Source::Notes => "notes",
            other => other.name(),
        }
    }

    /// The prefix that scopes a search to it, as the palette types it.
    pub fn prefix(self) -> &'static str {
        match self {
            Source::Tabs => "tab: ",
            Source::Notes => "note: ",
            Source::Linear => "linear: ",
            Source::GitHub => "gh: ",
            Source::Slack => "slack: ",
        }
    }

    /// Its word in the file the chips are kept in.
    fn word(self) -> &'static str {
        self.prefix().trim_end().trim_end_matches(':')
    }

    /// The source a prefix's word names, whatever its case.
    fn named(word: &str) -> Option<Source> {
        match word.to_lowercase().as_str() {
            "tab" | "tabs" => Some(Source::Tabs),
            "note" | "notes" => Some(Source::Notes),
            "linear" => Some(Source::Linear),
            "gh" | "github" => Some(Source::GitHub),
            "slack" => Some(Source::Slack),
            _ => None,
        }
    }
}

/// Whether `text` is the start of a word naming a source (`sl`, `not`),
/// whatever its case, as a prefix is typed a letter at a time.
pub fn starts_a_source(text: &str) -> bool {
    let text = text.to_lowercase();
    [
        "tab", "tabs", "note", "notes", "linear", "gh", "github", "slack",
    ]
    .iter()
    .any(|w| w.starts_with(&text))
}

/// The sources a query's opening prefixes name (`note: slack: deploy`), in
/// the sources' order, and the query after them; no prefix names every
/// source. A word before a colon that names no source (`fix:`) stays part
/// of the query.
pub fn scope(query: &str) -> (Vec<Source>, &str) {
    let mut named = Vec::new();
    let mut rest = query.trim_start();
    while let Some((word, after)) = rest.split_once(':')
        && !word.contains(char::is_whitespace)
        && let Some(source) = Source::named(word)
    {
        if !named.contains(&source) {
            named.push(source);
        }
        rest = after.trim_start();
    }
    if named.is_empty() {
        return (Source::ALL.to_vec(), query.trim());
    }
    named.sort();
    (named, rest.trim())
}

/// One result: what it is, a word about it, the link it reads (empty for a
/// row that only says something, as Slack's connector missing), and where
/// it was found, for M-F's preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub source: Source,
    pub text: String,
    pub detail: String,
    pub link: String,
    pub context: Context,
}

/// Where a result was found, as M-F's preview shows it (board 91): its
/// place (a note's title and the heading above the match, a tab, a ticket,
/// a PR, a Slack channel), what it is, and the lines around the match,
/// filled by its source as it answers so the preview reads nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Context {
    pub place: String,
    pub about: String,
    /// The match's line number, for what has lines of its own to count (a
    /// note); 0 for what doesn't.
    pub line: usize,
    /// Up to `pane_search::AROUND` lines each side of the match and the
    /// match's own, each cut to `pane_search::AROUND_CHARS` characters.
    pub lines: Vec<String>,
    /// The match's line among `lines`.
    pub at: usize,
}

/// The qualifiers GitHub's, Linear's and Slack's searches take, as
/// `<key>:<value>`.
const QUALIFIERS: &[&str] = &[
    "after",
    "archived",
    "assignee",
    "author",
    "base",
    "before",
    "closed",
    "comments",
    "commenter",
    "created",
    "draft",
    "during",
    "from",
    "has",
    "head",
    "in",
    "involves",
    "is",
    "label",
    "language",
    "linked",
    "mentions",
    "merged",
    "milestone",
    "no",
    "on",
    "org",
    "project",
    "reason",
    "repo",
    "review",
    "sort",
    "state",
    "status",
    "team",
    "to",
    "type",
    "updated",
    "user",
    "with",
];

/// The words of `query` a result's text holds, lowercased, for finding and
/// marking the match: the services' qualifiers (`is:pr`, `in:#deploys`,
/// `label:"help wanted"`, a key of `QUALIFIERS`, its quoted value whole)
/// and exclusions (`-draft`) left out, and the quotes of a phrase taken
/// off; every word when that leaves none.
pub fn terms(query: &str) -> Vec<String> {
    let qualifier = |w: &str| {
        w.split_once(':').is_some_and(|(key, value)| {
            QUALIFIERS.contains(&key.to_lowercase().as_str()) && !value.is_empty()
        })
    };
    // Each word, with whether it is kept: inside a quoted phrase it is,
    // inside a qualifier's quoted value it isn't.
    let mut open: Option<bool> = None;
    let marked: Vec<(&str, bool)> = query
        .split_whitespace()
        .map(|w| {
            let odd = w.matches('"').count() % 2 == 1;
            let kept = match open {
                Some(kept) => {
                    if odd {
                        open = None;
                    }
                    kept
                }
                None => {
                    let kept = w.starts_with('"') || !(w.starts_with('-') || qualifier(w));
                    if odd {
                        open = Some(kept);
                    }
                    kept
                }
            };
            (w, kept)
        })
        .collect();
    let words = |all: bool| -> Vec<String> {
        marked
            .iter()
            .filter(|(_, kept)| all || *kept)
            .map(|(w, _)| {
                w.trim_matches(|c: char| c == '"' || c == '\'')
                    .to_lowercase()
            })
            .filter(|w| !w.is_empty())
            .collect()
    };
    let terms = words(false);
    if terms.is_empty() { words(true) } else { terms }
}

/// The first of `lines` holding the most of `terms`, at least one, so a
/// phrase or several words searched for are found together where they are.
fn first_holding<S: AsRef<str>>(lines: &[S], terms: &[String]) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    for (i, l) in lines.iter().enumerate() {
        let l = l.as_ref().to_lowercase();
        let held = terms.iter().filter(|t| l.contains(t.as_str())).count();
        if held > 0 && best.is_none_or(|(_, most)| held > most) {
            best = Some((i, held));
        }
    }
    best.map(|(i, _)| i)
}

impl Context {
    /// A result's context with nothing around it: its text alone.
    fn alone(place: &str, about: &str, text: &str) -> Context {
        let (lines, at) = pane_search::around(&[text], 0);
        Context {
            place: place.to_string(),
            about: about.to_string(),
            line: 0,
            lines,
            at,
        }
    }

    /// The lines of `body` around the first holding one of `query`'s
    /// `terms`, or its text alone when none does.
    fn in_body(place: &str, about: &str, body: &str, query: &str, text: &str) -> Context {
        let lines: Vec<&str> = body.lines().collect();
        match first_holding(&lines, &terms(query)) {
            Some(i) => {
                let (lines, at) = pane_search::around(&lines, i);
                Context {
                    place: place.to_string(),
                    about: about.to_string(),
                    line: 0,
                    lines,
                    at,
                }
            }
            None => Context::alone(place, about, text),
        }
    }
}

/// The sources Settings has on, a chip each in M-F's box: the tabs and the
/// notes always, Linear and GitHub with their settings, Slack with
/// search_slack.
pub fn available() -> Vec<Source> {
    Source::ALL
        .into_iter()
        .filter(|s| match s {
            Source::Tabs | Source::Notes => true,
            Source::Linear => settings::is_on("linear"),
            Source::GitHub => settings::is_on("github"),
            Source::Slack => asks_slack(),
        })
        .collect()
}

/// Where the chips turned off in the last M-F are kept.
fn chips_file() -> PathBuf {
    crate::paths::state_dir().join("search.json")
}

/// The sources whose chips were turned off in the last M-F; none when the
/// file is missing or can't be read.
pub fn read_off() -> Vec<Source> {
    let Ok(text) = std::fs::read_to_string(chips_file()) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    value
        .get("off")
        .and_then(Value::as_array)
        .map(|off| {
            off.iter()
                .filter_map(Value::as_str)
                .filter_map(Source::named)
                .collect()
        })
        .unwrap_or_default()
}

/// Keeps the sources whose chips are off for the next M-F.
pub fn write_off(off: &[Source]) -> std::io::Result<()> {
    let words: Vec<&str> = off.iter().map(|s| s.word()).collect();
    let file = chips_file();
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::util::write_atomic(
        file,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({ "off": words }))?
        ),
    )
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

/// An open tab as the Tabs source reads it: its id, its number, its label,
/// whether the search started from it, and its Claude's pane with that
/// session's transcript, in the order the tabs are listed.
#[derive(Clone, Debug)]
pub struct OpenTab {
    pub tab: TabId,
    pub number: usize,
    pub label: String,
    pub here: bool,
    pub claude: Option<(PaneId, PathBuf)>,
}

/// Where in a tab a result points: the pane its match is in (none for a
/// tab found by its name alone), and, for a match in its conversation,
/// when that was said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabAt {
    pub tab: TabId,
    pub pane: Option<PaneId>,
    pub said: Option<i64>,
}

/// A tab result's link.
pub fn tab_link(at: TabAt) -> String {
    let mut link = format!("{TAB_LINK}{}", at.tab.0);
    if let Some(p) = at.pane {
        link.push_str(&format!("/{}", p.0));
        if let Some(said) = at.said {
            link.push_str(&format!("@{said}"));
        }
    }
    link
}

/// Where a tab result's link points; `None` for any other link.
pub fn tab_of(link: &str) -> Option<TabAt> {
    let rest = link.strip_prefix(TAB_LINK)?;
    let (tab, pane) = match rest.split_once('/') {
        Some((t, p)) => (t, Some(p)),
        None => (rest, None),
    };
    let (pane, said) = match pane.map(|p| p.split_once('@').unwrap_or((p, ""))) {
        Some((p, "")) => (Some(PaneId(p.parse().ok()?)), None),
        Some((p, s)) => (Some(PaneId(p.parse().ok()?)), Some(s.parse().ok()?)),
        None => (None, None),
    };
    Some(TabAt {
        tab: TabId(tab.parse().ok()?),
        pane,
        said,
    })
}

const TAB_LINK: &str = "switchboard-tab:";

/// The open tabs whose label holds `query`, whose panes show it (`found`,
/// from `pane_search::search`) or whose Claude said it or was told it
/// (`said`, by transcript, from `conversation::search`), ranked as M-j's
/// picker ranks them: those found by name first, in the tabs' order, then
/// those found on their panes, the most matching lines first, then those
/// found only in their conversation, the latest first; in each, the tab
/// the search started from comes last, marked "this tab". Each shows its
/// number and label, and the last line holding the query (for one found
/// in its conversation, after "said" and how long ago, first so a narrow
/// column cuts the line rather than when), or "by name".
pub fn tabs(
    open: &[OpenTab],
    query: &str,
    found: &HashMap<TabId, pane_search::Found>,
    said: &HashMap<PathBuf, conversation::Hit>,
    now: i64,
) -> Vec<Found> {
    let text = query.trim().to_lowercase();
    if text.is_empty() {
        return Vec::new();
    }
    // (group, here, order within the group, row)
    let mut rows: Vec<(u8, bool, i64, Found)> = Vec::new();
    for (i, t) in open.iter().enumerate() {
        let named = t.label.to_lowercase().contains(&text);
        let on_screen = found.get(&t.tab);
        // A tab found by name is gone to as one, whatever its
        // conversation holds.
        let in_talk = t
            .claude
            .as_ref()
            .filter(|_| !named)
            .and_then(|(pane, file)| Some((*pane, said.get(file)?)));
        let mark = if t.here { " · this tab" } else { "" };
        let text = clean(&format!("{} {}{mark}", t.number, t.label));
        let (group, order, detail, at, context) = match (on_screen, in_talk) {
            (Some(f), _) => (
                1,
                -(f.count as i64),
                f.lines.last().map(|l| clean(l)).unwrap_or_default(),
                TabAt {
                    tab: t.tab,
                    pane: Some(f.pane),
                    said: None,
                },
                Context {
                    place: format!("{text} › pane {}", f.pane),
                    about: "What the pane shows".to_string(),
                    line: 0,
                    lines: f.around.clone(),
                    at: f.around_at,
                },
            ),
            (None, Some((pane, hit))) => {
                let ago = crate::ui::when::ago(Some(hit.at), now, crate::ui::when::Said::Ago);
                (
                    2,
                    -hit.at,
                    format!("said {ago}: {}", clean(&hit.line)),
                    TabAt {
                        tab: t.tab,
                        pane: Some(pane),
                        said: Some(hit.at),
                    },
                    Context {
                        place: format!("{text} › conversation"),
                        about: format!("Said in its Claude's conversation {ago}"),
                        line: 0,
                        lines: hit.around.clone(),
                        at: hit.around_at,
                    },
                )
            }
            (None, None) if named => (
                0,
                0,
                "by name".to_string(),
                TabAt {
                    tab: t.tab,
                    pane: None,
                    said: None,
                },
                Context::alone(&text, "A tab found by its name", &t.label),
            ),
            (None, None) => continue,
        };
        let (group, order) = if named { (0, i as i64) } else { (group, order) };
        rows.push((
            group,
            t.here,
            order,
            Found {
                source: Source::Tabs,
                text,
                detail,
                link: tab_link(at),
                context,
            },
        ));
    }
    rows.sort_by_key(|(group, here, order, _)| (*group, *here, *order));
    rows.into_iter().take(LIMIT).map(|(.., r)| r).collect()
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
            // Without the note's ID line (crates/notes/src/id.rs), which every
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
            let all: Vec<&str> = text.lines().collect();
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
            // The match's line: the first holding the first word, after the
            // title when it is past it.
            let at = (1..all.len())
                .chain([0])
                .find(|&i| all[i].to_lowercase().contains(&words[0]))
                .unwrap_or(0);
            let heading = all[1..=at.max(1).min(all.len().saturating_sub(1))]
                .iter()
                .rev()
                .find(|l| l.starts_with('#'))
                .map(|l| l.trim_start_matches('#').trim().to_string())
                .filter(|_| at > 0);
            let place = match heading {
                Some(h) if !h.is_empty() => format!("{title} › {h}"),
                _ => title.clone(),
            };
            let folder = file
                .parent()
                .and_then(|p| p.strip_prefix(dir).ok())
                .map(|p| p.to_string_lossy().into_owned())
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| "Notes".to_string());
            let (around, around_at) = pane_search::around(&all, at);
            Found {
                source: Source::Notes,
                text: clean(&title),
                detail: clean(&hit),
                link: file_url(&file),
                context: Context {
                    place: clean(&place),
                    about: format!("Note in {folder}"),
                    line: at + 1,
                    lines: around,
                    at: around_at,
                },
            }
        })
        .collect()
}

/// Linear's issue search.
pub async fn linear(query: &str) -> Result<Vec<Found>, String> {
    let data = linear::graphql(
        "query($q: String!, $first: Int!) { searchIssues(term: $q, first: $first) { nodes { identifier title description url state { name } } } }",
        json!({ "q": query, "first": LIMIT }),
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
            let title = clean(&format!("{} {}", text("/identifier"), text("/title")));
            let about = format!("Linear ticket, {}", text("/state/name"));
            Found {
                source: Source::Linear,
                context: Context::in_body(&title, &about, &text("/description"), query, &title),
                text: title,
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
    Ok(github_results(&answer, query))
}

/// GitHub's search answer as rows, each with the lines of its body around
/// `query`.
pub fn github_results(answer: &Value, query: &str) -> Vec<Found> {
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
            let title = clean(&format!("{repo}#{number} {}", text("title")));
            let detail = format!("{kind}, {}", text("state"));
            let about = format!("GitHub {detail}");
            Found {
                source: Source::GitHub,
                context: Context::in_body(&title, &about, &text("body"), query, &title),
                text: title,
                detail,
                link: text("html_url"),
            }
        })
        .collect()
}

/// Slack's search: through your own token (`day::slack_api`) while there
/// is one, else, or when that fails, through Claude's connector. A
/// connector that isn't connected is a row saying so, so it isn't taken for
/// no results.
pub async fn slack(query: &str) -> Result<Vec<Found>, String> {
    use crate::day::{slack_api, slack_user};
    let state = crate::paths::state_dir();
    let token = slack_api::client(&state).await;
    if let Some(client) = slack_api::usable(&state, token) {
        let me = slack_user::signed_in(&state)
            .map(|s| s.user)
            .unwrap_or_default();
        match slack_api::search(
            &client,
            query,
            &me,
            slack_api::How {
                count: LIMIT as u32,
                pages: 1,
                back_to: None,
                context: true,
            },
            "search: Slack",
        )
        .await
        {
            Ok(found) => {
                slack_api::worked(&state, slack_api::Part::Search);
                // A hit with no permalink can't be opened, as the
                // connector's reader leaves one out.
                return Ok(found
                    .iter()
                    .filter(|m| m.link.is_some())
                    .take(LIMIT)
                    .map(|m| slack_row(m, query))
                    .collect());
            }
            Err(why) => slack_api::failed(&state, slack_api::Part::Search, &why),
        }
    }
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
            Ok(slack_results(&results, query))
        }
        Err(e) if e.not_connected => Ok(vec![Found {
            source: Source::Slack,
            text: e.message,
            detail: String::new(),
            link: String::new(),
            context: Context::default(),
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
/// where and when, and its permalink, with the messages around it centered
/// on its first line holding one of `query`'s `terms`; one with no
/// permalink left out.
pub fn slack_results(text: &str, query: &str) -> Vec<Found> {
    let terms = terms(query);
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
            // The message after `Text:`, and the messages Slack gave before
            // and after it, each up to the next part or the end.
            let part = |name: &str| -> Vec<String> {
                let mut out = Vec::new();
                let mut lines = block.lines().skip_while(|l| !l.starts_with(name));
                if let Some(first) = lines.next() {
                    let rest = first.trim_start_matches(name).trim();
                    if !rest.is_empty() {
                        out.push(rest.to_string());
                    }
                    for l in lines {
                        // The message itself may hold a line starting
                        // "Text:"; only a context part ends at one.
                        if (name != "Text:" && l.starts_with("Text:"))
                            || l.starts_with("Context before:")
                            || l.starts_with("Context after:")
                            || l.starts_with("---")
                        {
                            break;
                        }
                        out.push(l.to_string());
                    }
                }
                out
            };
            let body = readable(&part("Text:").join("\n"));
            let before: Vec<String> = part("Context before:")
                .iter()
                .map(|l| readable(l))
                .collect();
            let after: Vec<String> = part("Context after:").iter().map(|l| readable(l)).collect();
            let mut around: Vec<String> = before.clone();
            around.extend(body.lines().map(str::to_string));
            around.extend(after);
            let body_lines: Vec<&str> = body.lines().collect();
            let matched = first_holding(&body_lines, &terms).unwrap_or(0);
            let (lines, at) = pane_search::around(&around, before.len() + matched);
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
                context: Context {
                    place: clean(&format!("{channel} › {who}")),
                    about: format!("Slack message, {time}"),
                    line: 0,
                    lines,
                    at,
                },
            })
        })
        .collect()
}

/// A message read through your own token as a row, as `slack_results`
/// makes one of the connector's.
pub fn slack_row(m: &crate::day::slack_look::Found, query: &str) -> Found {
    let body = readable(&m.text);
    let body_lines: Vec<&str> = body.lines().collect();
    let matched = first_holding(&body_lines, &terms(query)).unwrap_or(0);
    let mut lines: Vec<String> = m.before.iter().map(|l| readable(l)).collect();
    let before = lines.len();
    lines.extend(body_lines.iter().map(|l| l.to_string()));
    lines.extend(m.after.iter().map(|l| readable(l)));
    let (lines, at) = pane_search::around(&lines, before + matched);
    let time =
        m.ts.split('.')
            .next()
            .and_then(|s| s.parse::<i64>().ok())
            .and_then(|s| jiff::Timestamp::from_second(s).ok())
            .map(|t| {
                t.to_zoned(jiff::tz::TimeZone::system())
                    .strftime("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_default();
    let (who, place) = (&m.from, &m.r#where);
    Found {
        source: Source::Slack,
        text: clean(&body).chars().take(140).collect(),
        detail: clean(&format!("{who} in {place}, {time}")),
        link: m.link.clone().unwrap_or_default(),
        context: Context {
            place: clean(&format!("{place} › {who}")),
            about: format!("Slack message, {time}"),
            line: 0,
            lines,
            at,
        },
    }
}

/// A Slack message's text as read: `<@U1|ada>` as `@ada`, a link as its
/// words, a bare link as its address.
fn readable(text: &str) -> String {
    let text = MENTION.replace_all(text, "@$1");
    let text = NAMED_LINK.replace_all(&text, "$2");
    crate::day::slack_look::unescape(&BARE_LINK.replace_all(&text, "$1"))
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
            context: Context::default(),
        }
    }

    #[test]
    fn a_querys_prefixes_scope_it_to_their_sources() {
        assert_eq!(
            scope("deploy freeze"),
            (Source::ALL.to_vec(), "deploy freeze")
        );
        assert_eq!(scope("note: deploy"), (vec![Source::Notes], "deploy"));
        assert_eq!(
            scope("  Slack: NOTE:deploy  "),
            (vec![Source::Notes, Source::Slack], "deploy"),
            "two, in the sources' order, any case, no space needed"
        );
        assert_eq!(
            scope("gh: github: x"),
            (vec![Source::GitHub], "x"),
            "a source named twice is asked once"
        );
        assert_eq!(scope("tabs: linear: y").0, [Source::Tabs, Source::Linear]);
        assert_eq!(
            scope("fix: the build"),
            (Source::ALL.to_vec(), "fix: the build"),
            "an unknown word stays in the query"
        );
        assert_eq!(
            scope("note: fix: it"),
            (vec![Source::Notes], "fix: it"),
            "prefixes end at the first that names no source"
        );
        assert_eq!(scope("note:"), (vec![Source::Notes], ""));
        assert_eq!(
            scope("deploy note: x"),
            (Source::ALL.to_vec(), "deploy note: x"),
            "only at the start"
        );
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
    fn tabs_found_by_name_come_first_then_by_their_panes() {
        let open = |tab: u32, label: &str| OpenTab {
            tab: TabId(tab),
            number: tab as usize,
            label: label.to_string(),
            here: false,
            claude: None,
        };
        let tabs_open = [
            open(1, "alpha"),
            open(2, "Zebra work"),
            open(3, "gamma"),
            open(4, "delta"),
        ];
        let mut found = HashMap::new();
        for (tab, count) in [(1, 1), (3, 4)] {
            found.insert(
                TabId(tab),
                pane_search::Found {
                    pane: PaneId(tab + 10),
                    lines: vec!["first zebra".into(), format!("a  zebra {tab}")],
                    count,
                    around: Vec::new(),
                    around_at: 0,
                },
            );
        }
        let none = HashMap::new();
        let rows = tabs(&tabs_open, " ZEBRA ", &found, &none, 0);
        let shown: Vec<(&str, &str)> = rows
            .iter()
            .map(|r| (r.text.as_str(), r.detail.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                ("2 Zebra work", "by name"),
                ("3 gamma", "a zebra 3"),
                ("1 alpha", "a zebra 1")
            ]
        );
        let at = |tab, pane: Option<u32>, said| TabAt {
            tab: TabId(tab),
            pane: pane.map(PaneId),
            said,
        };
        assert_eq!(tab_of(&rows[0].link), Some(at(2, None, None)));
        assert_eq!(tab_of(&rows[1].link), Some(at(3, Some(13), None)));
        assert_eq!(tab_of("https://example.com"), None);
        assert!(tabs(&tabs_open, "  ", &found, &none, 0).is_empty());
    }

    #[test]
    fn tabs_found_in_their_conversation_come_after_and_this_tab_last() {
        let open = |tab: u32, here: bool| OpenTab {
            tab: TabId(tab),
            number: tab as usize,
            label: format!("tab {tab}"),
            here,
            claude: Some((PaneId(tab + 10), PathBuf::from(format!("/t{tab}.jsonl")))),
        };
        let tabs_open = [
            open(1, true),
            open(2, false),
            open(3, false),
            open(4, false),
        ];
        let mut found = HashMap::new();
        for tab in [1, 4] {
            found.insert(
                TabId(tab),
                pane_search::Found {
                    pane: PaneId(tab + 10),
                    lines: vec![format!("node on screen {tab}")],
                    count: 1,
                    around: vec![format!("node on screen {tab}")],
                    around_at: 0,
                },
            );
        }
        let mut said = HashMap::new();
        for (tab, at) in [(1, 900), (2, 500), (3, 800)] {
            said.insert(
                PathBuf::from(format!("/t{tab}.jsonl")),
                conversation::Hit {
                    at,
                    line: format!("asked about node in {tab}"),
                    count: 1,
                    around: vec![format!("asked about node in {tab}")],
                    around_at: 0,
                },
            );
        }
        let rows = tabs(&tabs_open, "node", &found, &said, 1000);
        let shown: Vec<(&str, &str)> = rows
            .iter()
            .map(|r| (r.text.as_str(), r.detail.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                ("4 tab 4", "node on screen 4"),
                ("1 tab 1 · this tab", "node on screen 1"),
                ("3 tab 3", "said 3m ago: asked about node in 3"),
                ("2 tab 2", "said 8m ago: asked about node in 2"),
            ]
        );
        assert_eq!(
            tab_of(&rows[2].link),
            Some(TabAt {
                tab: TabId(3),
                pane: Some(PaneId(13)),
                said: Some(800)
            })
        );
        // Found only in its conversation, this tab still comes last.
        found.clear();
        let rows = tabs(&tabs_open, "node", &found, &said, 1000);
        assert_eq!(rows.last().unwrap().text, "1 tab 1 · this tab");
        // Found by name, a tab is gone to as one, its conversation aside.
        let rows = tabs(&tabs_open, "tab 2", &found, &said, 1000);
        assert_eq!(rows[0].detail, "by name");
        assert_eq!(tab_of(&rows[0].link).unwrap().said, None);
    }

    #[test]
    fn slacks_answer_is_read_into_rows() {
        let text = "Found 2.\n### Result 1 of 2\nChannel: #deploys (ID: C1)\nFrom: Jo <jo@x.com> (ID: U1)\n\
            Time: 2026-11-05 10:30:12 EST\nPermalink: [link](https:\\/\\/acme.slack.com\\/archives\\/C1\\/p1)\n\
            Text:\nThe <@U2|sam> freeze is <https://x.com/a|on>\nsee <https://x.com/b>\nText: kept\nContext after:\nignored\n\
            ### Result 2 of 2\nChannel: #x\nFrom: Bot\nText: no link here\n";
        let rows = slack_results(text, "in:#deploys \"see\"");
        assert_eq!(
            rows[0].context.lines[rows[0].context.at], "see https://x.com/b",
            "the preview centered on the line holding the word"
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].text, "The @sam freeze is on see https://x.com/b Text: kept",
            "a line of the message starting Text: kept"
        );
        assert_eq!(rows[0].detail, "Jo in #deploys, 2026-11-05 10:30");
        assert_eq!(rows[0].link, "https://acme.slack.com/archives/C1/p1");
    }

    #[test]
    fn a_message_read_through_your_token_is_a_row_as_the_connectors_are() {
        let m = crate::day::slack_look::Found {
            channel: "C1".into(),
            r#where: "#deploys".into(),
            from: "Jo".into(),
            ts: "1793282136.000100".into(),
            link: Some("https://acme.slack.com/archives/C1/p1".into()),
            text: "The <@U2|sam> freeze is <https://x.com/a|on>\nsee <https://x.com/b>".into(),
            ..Default::default()
        };
        let row = slack_row(&m, "see");
        assert_eq!(row.text, "The @sam freeze is on see https://x.com/b");
        assert!(
            row.detail.starts_with("Jo in #deploys, 2026-"),
            "{}",
            row.detail
        );
        assert_eq!(row.link, "https://acme.slack.com/archives/C1/p1");
        assert_eq!(row.context.lines[row.context.at], "see https://x.com/b");
        assert_eq!(row.context.place, "#deploys › Jo");
        let m = crate::day::slack_look::Found {
            before: vec!["earlier".into()],
            after: vec!["later <@U3|cy>".into()],
            ..m
        };
        let row = slack_row(&m, "see");
        assert_eq!(
            row.context.lines.first().map(String::as_str),
            Some("earlier")
        );
        assert_eq!(
            row.context.lines.last().map(String::as_str),
            Some("later @cy")
        );
        assert_eq!(row.context.lines[row.context.at], "see https://x.com/b");
    }

    #[test]
    fn a_querys_terms_leave_out_qualifiers_exclusions_and_quotes() {
        assert_eq!(terms("is:pr \"Login\" -draft fix"), ["login", "fix"]);
        assert_eq!(terms("https://example.com/a"), ["https://example.com/a"]);
        assert_eq!(terms("\"note:x\""), ["note:x"], "a quoted word kept");
        assert_eq!(
            terms("\"error status:500\" -x"),
            ["error", "status:500"],
            "a qualifier-like word inside a phrase kept"
        );
        assert_eq!(
            terms("label:\"help wanted\" crash"),
            ["crash"],
            "a qualifier's quoted value left out"
        );
        assert_eq!(
            terms("localhost:3000 error"),
            ["localhost:3000", "error"],
            "an address kept"
        );
        assert_eq!(
            terms("is:open"),
            ["is:open"],
            "every word when none is left"
        );
        let ctx = Context::in_body("p", "a", "intro\nthe login form\nend", "is:pr login", "t");
        assert_eq!(ctx.lines[ctx.at], "the login form");
        let body = "Deployment guide\nintro\nthe deployment failed at 3";
        let ctx = Context::in_body("p", "a", body, "is:pr \"deployment failed\"", "t");
        assert_eq!(
            ctx.lines[ctx.at], "the deployment failed at 3",
            "the line holding both"
        );
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
        let rows = github_results(&answer, "login");
        assert_eq!(rows[0].text, "widgets#12 Fix the login");
        assert_eq!(rows[0].detail, "PR, open");
        assert_eq!(rows[1].detail, "issue, closed");
    }

    #[test]
    fn a_note_s_match_carries_its_line_and_five_lines_each_side() {
        let dir = std::env::temp_dir().join(format!("sb-search-context-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Plans")).unwrap();
        let mut text = vec!["# Long note".to_string()];
        text.extend((2..=20).map(|n| format!("line {n}")));
        text[11] = format!("## Steps\n{}", "x".repeat(pane_search::AROUND_CHARS + 100));
        // Line 12 the heading, 13 the long line, 14 the match.
        let text = text.join("\n").replace("line 13", "the zebra is here");
        std::fs::write(dir.join("Plans/Long note.md"), &text).unwrap();
        std::fs::write(dir.join("Short.md"), "# Short\nzebra first\nend\n").unwrap();
        let rows = notes(&dir, "zebra");
        let long = rows.iter().find(|r| r.text == "Long note").unwrap();
        let ctx = &long.context;
        assert_eq!(ctx.line, 14);
        assert_eq!(ctx.place, "Long note › Steps");
        assert_eq!(ctx.about, "Note in Plans");
        assert_eq!(ctx.lines.len(), 11);
        assert_eq!(ctx.lines[ctx.at], "the zebra is here");
        assert_eq!(ctx.lines[0], "line 9");
        assert_eq!(ctx.lines[10], "line 18");
        assert_eq!(
            ctx.lines[ctx.at - 1].chars().count(),
            pane_search::AROUND_CHARS,
            "a long line cut"
        );
        let short = rows.iter().find(|r| r.text == "Short").unwrap();
        assert_eq!(short.context.line, 2);
        assert_eq!(short.context.place, "Short");
        assert_eq!(short.context.about, "Note in Notes");
        assert_eq!(
            short.context.lines,
            ["# Short", "zebra first", "end"],
            "cut at the note's start and end"
        );
        assert_eq!(short.context.at, 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}
