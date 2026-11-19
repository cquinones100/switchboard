//! A Linear ticket as the ticket view shows it (tmux/ticket-view.sh, and
//! pr-view.js's `ticket`, which laid it out as its page in Linear does):
//! its title, the facts from its side panel, who opened it and when, the
//! description, sub-issues, related tickets and links, then the comments,
//! oldest first, each reply under the comment it answers.
//!
//! The ticket is the one the pane's branch names, else the one the tab's
//! name gives (a "Shaping ABC-123: ..." tab before there is a branch), which
//! the server works out before it opens the pane. As the PR view does, the
//! copy last read is shown at once (`~/.cache/switchboard/ticket-view/`)
//! and the ticket read again in the background, through Linear's API with
//! the key `integrations::linear` finds.
//!
//! With UX 2.0 on, the ticket is laid out as board 14 draws it (`board`,
//! through `super::board`): the description, sub-issues and comment
//! threads, with its facts, pull requests, related tickets and links beside
//! them.

use std::path::Path;
use std::process::ExitCode;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use super::board::{self, Board, Part, Piece};
use super::follow;
use super::pager::{self, Content};
use crate::integrations::{github, linear};
use crate::photos::{Service, Who};
use crate::ui::kit::{self, Mark, Tone};
use crate::{paths, settings, util};

const SOURCE: &str = "M-i ticket view";
/// Where a diagram that can't be drawn here is shown properly.
pub const PLACE: &str = "M-l in Linear";

const QUERY: &str = "query($id: String!) { issue(id: $id) { \
    identifier title url description createdAt updatedAt priorityLabel estimate \
    state { name type } assignee { name } creator { id name } project { name } cycle { number name } \
    labels { nodes { name } } parent { identifier title } \
    children { nodes { identifier title url state { name type } } } \
    relations { nodes { type relatedIssue { identifier title url state { name type } } } } \
    attachments { nodes { title url metadata } } \
    comments(first: 100) { nodes { id body createdAt parent { id } user { id name } botActor { name } } } } }";

static ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9]*-[0-9]+$").unwrap());
static IN_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Z][A-Z0-9]*-[0-9]+").unwrap());
/// A Linear project's ID in a branch, in any case.
static PROJECT_IN_BRANCH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^A-Za-z0-9])(P-[A-Z][A-Z0-9]*-[0-9]+)(?:$|[^A-Za-z0-9])").unwrap()
});
/// A ticket's ID in a branch, in any case, as `linear::ticket_in` finds it.
static TICKET_IN_BRANCH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)[a-z][a-z0-9]*-[0-9]+").unwrap());
/// A Linear project's ID, `P-<TEAM>-<n>`.
static PROJECT_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?i)P-[A-Z][A-Z0-9]*-[0-9]+$").unwrap());
static PROJECT_IN_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^A-Za-z0-9])([Pp]-[A-Z][A-Z0-9]*-[0-9]+)(?:$|[^A-Za-z0-9])").unwrap()
});
static ISSUE_BRANCH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^issue/([0-9]+)(?:-|$)").unwrap());
static LINEAR_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|[^(<])<?(https://linear\.app/[^/\s]+/issue/([A-Z][A-Z0-9]*-\d+)[^\s)>]*)>?")
        .unwrap()
});

/// A ticket's identifier, in capitals, when `text` is one (`eng-42`).
pub fn id_of(text: &str) -> Option<String> {
    ID.is_match(text.trim())
        .then(|| text.trim().to_ascii_uppercase())
}

/// The ticket a directory's branch names (linear-ticket.sh's match).
pub fn id_for_dir(dir: &Path) -> Option<String> {
    let place = crate::tabs::git::locate(dir).filter(|p| p.on_branch())?;
    id_in_branch(&place.branch)
}

/// The Linear ticket a branch names, in capitals, never the end of a
/// project's ID: what the tab's name and M-i read from a branch alike.
pub fn id_in_branch(branch: &str) -> Option<String> {
    branch_ticket(branch).map(|(_, id)| id)
}

/// The first ticket ID a branch names, in capitals, and where: as
/// `linear::ticket_in` finds one, but never the end of a project's ID
/// (`p-abc-4-eng-7` gives ENG-7).
fn branch_ticket(branch: &str) -> Option<(usize, String)> {
    TICKET_IN_BRANCH
        .find_iter(branch)
        .find(|m| {
            let before = &branch[..m.start()];
            let alnum = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
            let whole = !alnum(before.chars().last()) && !alnum(branch[m.end()..].chars().next());
            let project_end = (before.ends_with("p-") || before.ends_with("P-"))
                && !before[..before.len() - 2]
                    .chars()
                    .last()
                    .is_some_and(|c| c.is_ascii_alphanumeric());
            whole && !project_end
        })
        .map(|m| (m.start(), m.as_str().to_ascii_uppercase()))
}

/// The ticket a tab's name gives, in capitals as Linear writes them
/// ("Shaping ABC-123: ..."): a whole word, never the end of a project's ID
/// (`P-ABC-4` names no ticket `ABC-4`).
pub fn id_in_name(name: &str) -> Option<String> {
    ticket_word(name).map(|(_, id)| id)
}

/// Where the first ticket ID in `name` is, and the ID: a word of its own
/// (`feat-ABC-42-fix` gives ABC-42), never part of a longer word or the
/// end of a project's ID (`P-ABC-4`).
fn ticket_word(name: &str) -> Option<(usize, String)> {
    let alnum = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    IN_NAME
        .find_iter(name)
        .find(|m| {
            let before = &name[..m.start()];
            let project_end = before
                .strip_suffix("P-")
                .or_else(|| before.strip_suffix("p-"))
                .is_some_and(|b| !alnum(b.chars().last()));
            !alnum(before.chars().last()) && !alnum(name[m.end()..].chars().next()) && !project_end
        })
        .map(|m| (m.start(), m.as_str().to_string()))
}

/// What a branch names in Linear: a project's ID (`p-abc-4-review`) or a
/// ticket's, whichever comes first.
pub fn linear_in_branch(branch: &str) -> Option<Ticket> {
    let project = PROJECT_IN_BRANCH
        .captures(branch)
        .and_then(|c| c.get(1))
        .map(|m| (m.start(), Ticket::Project(m.as_str().to_ascii_uppercase())));
    let ticket = branch_ticket(branch).map(|(at, id)| (at, Ticket::Linear(id)));
    match (project, ticket) {
        (Some(p), Some(t)) => Some(if p.0 <= t.0 { p.1 } else { t.1 }),
        (p, t) => p.or(t).map(|(_, found)| found),
    }
}

/// The Linear project a name gives (`Shaping P-ABC-4: ...`), in capitals.
pub fn project_in_name(name: &str) -> Option<String> {
    PROJECT_IN_NAME
        .captures(name)
        .map(|c| c[1].to_ascii_uppercase())
}

/// What a tab's work is on, by the service that keeps it: a Linear ticket,
/// a Linear project, or one of the repository's GitHub issues.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ticket {
    Linear(String),
    /// A project, by what Linear's `project(id:)` takes: its `P-<TEAM>-<n>`
    /// identifier, or its slug.
    Project(String),
    Issue {
        repo: github::Repo,
        number: u64,
    },
}

impl Ticket {
    /// What `switchboard ticket-view` and `@tab_ticket` take:
    /// `ENG-42`, `project:P-ENG-4`, `owner/repo#12`.
    pub fn arg(&self) -> String {
        match self {
            Ticket::Linear(id) => id.clone(),
            Ticket::Project(id) => format!("project:{id}"),
            Ticket::Issue { repo, number } => format!("{repo}#{number}"),
        }
    }

    /// `arg`'s inverse; a bare `P-<TEAM>-<n>` is a project too.
    pub fn parse(text: &str) -> Option<Ticket> {
        Ticket::parse_any(text).filter(Ticket::plain)
    }

    /// Whether an issue's repository is names alone, never `.` or `..`,
    /// which would reach past its cache folder or GitHub path.
    fn plain(&self) -> bool {
        match self {
            Ticket::Issue { repo, .. } => [&repo.owner, &repo.name]
                .iter()
                .all(|part| !part.is_empty() && part.chars().any(|c| c != '.')),
            _ => true,
        }
    }

    fn parse_any(text: &str) -> Option<Ticket> {
        let text = text.trim();
        if let Some(id) = text.strip_prefix("project:").filter(|id| !id.is_empty()) {
            return Some(Ticket::Project(id.to_string()));
        }
        if PROJECT_ID.is_match(text) {
            return Some(Ticket::Project(text.to_ascii_uppercase()));
        }
        if let Some((repo, number)) = crate::autopilot::issues::parse_reference(text) {
            return Some(Ticket::Issue { repo, number });
        }
        id_of(text).map(Ticket::Linear)
    }

    /// How it is named to you: `ENG-42`, `P-ENG-4`, `owner/repo#12`.
    pub fn shown(&self) -> String {
        match self {
            Ticket::Project(id) => id.clone(),
            other => other.arg(),
        }
    }

    /// The setting of the service that keeps it.
    pub fn service(&self) -> &'static str {
        match self {
            Ticket::Issue { .. } => "github",
            _ => "linear",
        }
    }

    /// Its link, when it is known without asking: an issue's.
    pub fn url(&self) -> Option<String> {
        match self {
            Ticket::Issue { repo, number } => {
                Some(format!("https://github.com/{repo}/issues/{number}"))
            }
            _ => None,
        }
    }
}

/// The GitHub issue a branch was started for (`wtn --issue`), from the
/// text of its repository's git config and its origin: its
/// `branch.<branch>.wtnIssue`, else an `issue/<number>-<slug>` branch's
/// number in origin's repository.
pub fn issue_of_branch(
    config: &str,
    branch: &str,
    origin: Option<&github::Repo>,
) -> Option<Ticket> {
    if let Some(issue) = crate::tabs::title::branch_value(config, branch, "wtnIssue")
        .and_then(|r| Ticket::parse(&r))
        .filter(|t| matches!(t, Ticket::Issue { .. }))
    {
        return Some(issue);
    }
    let number = issue_number_of_branch(branch)?;
    Some(Ticket::Issue {
        repo: origin?.clone(),
        number,
    })
    .filter(Ticket::plain)
}

/// The number an `issue/<number>-<slug>` branch was named for.
pub fn issue_number_of_branch(branch: &str) -> Option<u64> {
    ISSUE_BRANCH.captures(branch)?[1].parse().ok()
}

/// The ticket a directory's branch is for, from git's own files, by the
/// repository's ticket provider (repos.json's `tickets`): its GitHub issue
/// with `github`, its Linear ticket with `linear`, and with none set the
/// issue, else the Linear ticket; of these, the first whose service is on.
pub fn of_dir(dir: &Path, provider: Option<&str>) -> Option<Ticket> {
    first_on(of_dir_all(dir, provider))
}

/// Every ticket `of_dir` chooses from, in its order.
pub fn of_dir_all(dir: &Path, provider: Option<&str>) -> Vec<Ticket> {
    let Some(place) = crate::tabs::git::locate(dir).filter(|p| p.on_branch()) else {
        return Vec::new();
    };
    let issue = || {
        let config = std::fs::read_to_string(place.common_dir.join("config")).unwrap_or_default();
        let origin = crate::sources::repo::origin_of(dir);
        issue_of_branch(&config, &place.branch, origin.as_ref())
    };
    let linear = || linear_in_branch(&place.branch);
    match provider {
        Some("github") => issue().into_iter().collect(),
        Some("linear") => linear().into_iter().collect(),
        _ => issue().into_iter().chain(linear()).collect(),
    }
}

/// The first of `found` whose service is on in Settings, else the first,
/// so one of a service turned off gives way to the next and is said to be
/// off only when nothing else is found.
pub fn first_on(found: Vec<Ticket>) -> Option<Ticket> {
    let on = found
        .iter()
        .position(|t| crate::settings::is_on(t.service()));
    let mut found = found;
    match on {
        Some(at) => Some(found.swap_remove(at)),
        None => found.into_iter().next(),
    }
}

/// The ticket or project a name you gave a tab names, whatever the
/// repository's provider, since you named it.
pub fn in_name(name: &str) -> Option<Ticket> {
    // Whichever the name gives first.
    let project = PROJECT_IN_NAME
        .captures(name)
        .and_then(|c| c.get(1))
        .map(|m| (m.start(), Ticket::Project(m.as_str().to_ascii_uppercase())));
    let ticket = ticket_word(name).map(|(at, id)| (at, Ticket::Linear(id)));
    match (project, ticket) {
        (Some(p), Some(t)) => Some(if p.0 < t.0 { p.1 } else { t.1 }),
        (p, t) => p.or(t).map(|(_, found)| found),
    }
}

/// Every ticket ID `text` names, each once, in order.
pub fn ids_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in IN_NAME.find_iter(text) {
        if !out.iter().any(|id| id == m.as_str()) {
            out.push(m.as_str().to_string());
        }
    }
    out
}

/// A bare link to a Linear ticket as a link named by its identifier, which
/// then reads as the identifier, clickable (pr-view.js's `tidy`).
pub fn tidy(text: &str) -> String {
    LINEAR_URL
        .replace_all(text, "${1}[${3}](${2})")
        .into_owned()
}

fn day(iso: &Value) -> String {
    iso.as_str().unwrap_or("").chars().take(10).collect()
}

fn name_at(v: &Value, path: &str) -> Option<String> {
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

/// The ticket as Markdown, as pr-view.js's `ticket` laid it out.
pub fn markdown(issue: &Value) -> String {
    let text = |path: &str| name_at(issue, path).unwrap_or_default();
    let mut facts: Vec<String> = Vec::new();
    if let Some(state) = name_at(issue, "/state/name") {
        facts.push(state);
    }
    if let Some(priority) = name_at(issue, "/priorityLabel").filter(|p| p != "No priority") {
        facts.push(priority);
    }
    facts.push(match name_at(issue, "/assignee/name") {
        Some(name) => format!("assigned to {name}"),
        None => "unassigned".into(),
    });
    if let Some(project) = name_at(issue, "/project/name") {
        facts.push(format!("project {project}"));
    }
    if let Some(cycle) = issue.get("cycle").filter(|c| c.is_object()) {
        let named = name_at(cycle, "/name").or_else(|| {
            cycle
                .get("number")
                .filter(|n| !n.is_null())
                .map(util::js_string)
        });
        facts.push(format!("cycle {}", named.unwrap_or_default()));
    }
    if let Some(estimate) = issue.get("estimate").filter(|e| !e.is_null()) {
        facts.push(format!("estimate {}", util::js_string(estimate)));
    }
    let labels: Vec<String> = nodes(issue, "/labels/nodes")
        .iter()
        .filter_map(|l| name_at(l, "/name"))
        .collect();
    if !labels.is_empty() {
        facts.push(labels.join(", "));
    }
    let list = |title: &str, rows: Vec<String>| {
        if rows.is_empty() {
            String::new()
        } else {
            format!("\n\n## {title}\n\n{}", rows.join("\n"))
        }
    };
    let link = |i: &Value| {
        let state = name_at(i, "/state/name")
            .map(|s| format!(" _({s})_"))
            .unwrap_or_default();
        format!(
            "**{}** {}{state}",
            name_at(i, "/identifier").unwrap_or_default(),
            name_at(i, "/title").unwrap_or_default()
        )
    };
    let relation = |kind: &str| match kind {
        "blocks" => "Blocks".to_string(),
        "duplicate" => "Duplicates".to_string(),
        "related" => "Related to".to_string(),
        "similar" => "Similar to".to_string(),
        other => other.to_string(),
    };
    let mut comments = nodes(issue, "/comments/nodes");
    comments.sort_by(|a, b| {
        let at = |c: &Value| {
            c.get("createdAt")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        at(a).cmp(&at(b))
    });
    let comment = |c: &Value, reply: bool| {
        let who = name_at(c, "/user/name")
            .or_else(|| name_at(c, "/botActor/name"))
            .unwrap_or_else(|| "Someone".into());
        let head = format!(
            "{}**{who}** · {}",
            if reply { "↳ " } else { "" },
            day(c.get("createdAt").unwrap_or(&Value::Null))
        );
        let body = tidy(c.get("body").and_then(Value::as_str).unwrap_or(""));
        if reply {
            let quoted: Vec<String> = body.split('\n').map(|l| format!("> {l}")).collect();
            format!("> {head}\n>\n{}", quoted.join("\n"))
        } else {
            format!("{head}\n\n{body}")
        }
    };
    let parent_of = |c: &Value| name_at(c, "/parent/id");
    let threads: Vec<String> = comments
        .iter()
        .filter(|c| parent_of(c).is_none())
        .map(|c| {
            let id = name_at(c, "/id");
            let mut parts = vec![comment(c, false)];
            parts.extend(
                comments
                    .iter()
                    .filter(|r| parent_of(r).is_some() && parent_of(r) == id)
                    .map(|r| comment(r, true)),
            );
            parts.join("\n\n")
        })
        .collect();
    let parent = match (
        name_at(issue, "/parent/identifier"),
        name_at(issue, "/parent/title"),
    ) {
        (Some(id), title) => format!(" · sub-issue of {id} {}", title.unwrap_or_default()),
        _ => String::new(),
    };
    let description = tidy(&text("/description"));
    let mut out = String::new();
    out.push_str(&format!("# {}: {}", text("/identifier"), text("/title")));
    out.push_str(&format!("\n\n_{}_", facts.join(" · ")));
    out.push_str(&format!(
        "\n\n_Opened by {} on {}, updated {}{parent}_",
        name_at(issue, "/creator/name").unwrap_or_else(|| "someone".into()),
        day(issue.get("createdAt").unwrap_or(&Value::Null)),
        day(issue.get("updatedAt").unwrap_or(&Value::Null)),
    ));
    out.push_str("\n\n---\n\n");
    out.push_str(if description.trim().is_empty() {
        "_No description._"
    } else {
        &description
    });
    out.push_str(&list(
        "Sub-issues",
        nodes(issue, "/children/nodes")
            .iter()
            .map(|c| format!("- {}", link(c)))
            .collect(),
    ));
    out.push_str(&list(
        "Related",
        nodes(issue, "/relations/nodes")
            .iter()
            .map(|r| {
                let kind = r.get("type").and_then(Value::as_str).unwrap_or("");
                format!(
                    "- {} {}",
                    relation(kind),
                    link(r.get("relatedIssue").unwrap_or(&Value::Null))
                )
            })
            .collect(),
    ));
    out.push_str(&list(
        "Links",
        nodes(issue, "/attachments/nodes")
            .iter()
            .map(|a| {
                format!(
                    "- [{}]({})",
                    name_at(a, "/title").unwrap_or_default(),
                    name_at(a, "/url").unwrap_or_default()
                )
            })
            .collect(),
    ));
    if !threads.is_empty() {
        out.push_str(&format!(
            "\n\n## Comments\n\n{}",
            threads.join("\n\n---\n\n")
        ));
    }
    out
}

/// A Linear user, for their avatar.
fn who_of(user: &Value) -> Option<Who> {
    name_at(user, "/id").map(|id| Who::new(Service::Linear, &id))
}

/// A ticket's state as the kit marks it, by the kind Linear gives it.
fn state_mark(issue: &Value) -> Mark {
    match name_at(issue, "/state/type").as_deref() {
        Some("completed") => Mark::Passed,
        Some("started") => Mark::Pending,
        Some("canceled") => Mark::Failed,
        _ => Mark::Idle,
    }
}

static PR_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^https://github\.com/[^/]+/[^/]+/pull/(\d+)").unwrap());

/// The ticket as board 14 draws it: its title and who opened it, the
/// description through Markdown, its sub-issues and the comment threads,
/// with its facts, pull requests, related tickets and links beside them.
pub fn board(issue: &Value, now: jiff::Timestamp) -> Board {
    let text = |path: &str| name_at(issue, path).unwrap_or_default();
    let creator = name_at(issue, "/creator/name").unwrap_or_else(|| "Someone".into());
    let mut main = vec![
        Part::Row(vec![
            Piece::Name(text("/identifier"), Tone::Accent),
            Piece::Name(format!("  {}", text("/title")), Tone::Text),
        ]),
        Part::Said {
            who: who_of(issue.get("creator").unwrap_or(&Value::Null)),
            name: creator.clone(),
            head: vec![Piece::Text(
                format!(
                    "{creator} opened it {}",
                    board::when(&text("/createdAt"), now)
                ),
                Tone::Dim,
            )],
            body: String::new(),
            indent: 0,
        },
        Part::Rule,
        Part::Blank,
    ];
    let description = tidy(&text("/description"));
    main.push(Part::Text {
        markdown: if description.trim().is_empty() {
            "_No description._".into()
        } else {
            description
        },
        indent: 0,
    });
    let children = nodes(issue, "/children/nodes");
    if !children.is_empty() {
        let done = children
            .iter()
            .filter(|c| state_mark(c) == Mark::Passed)
            .count();
        main.extend([
            Part::Blank,
            Part::Label(
                "Sub-issues".into(),
                Some(format!("{done} of {} done", children.len())),
            ),
        ]);
        for c in children {
            main.push(Part::Row(vec![
                Piece::Mark(state_mark(c)),
                Piece::Text(" ".into(), Tone::Text),
                Piece::Link(
                    name_at(c, "/identifier").unwrap_or_default(),
                    Tone::Dim,
                    name_at(c, "/url").unwrap_or_default(),
                ),
                Piece::Text(
                    format!("  {}", name_at(c, "/title").unwrap_or_default()),
                    Tone::Text,
                ),
            ]));
        }
    }
    let mut comments = nodes(issue, "/comments/nodes");
    comments.sort_by_key(|c| name_at(c, "/createdAt").unwrap_or_default());
    let parent_of = |c: &Value| name_at(c, "/parent/id");
    if !comments.is_empty() {
        main.extend([
            Part::Blank,
            Part::Label("Comments".into(), Some(comments.len().to_string())),
        ]);
    }
    let comment = |c: &Value, indent: usize| {
        let name = name_at(c, "/user/name")
            .or_else(|| name_at(c, "/botActor/name"))
            .unwrap_or_else(|| "Someone".into());
        Part::Said {
            who: who_of(c.get("user").unwrap_or(&Value::Null)),
            name: name.clone(),
            head: vec![
                Piece::Name(name, Tone::Text),
                Piece::Text(
                    format!(
                        "  {}",
                        board::when(&name_at(c, "/createdAt").unwrap_or_default(), now)
                    ),
                    Tone::Dim,
                ),
            ],
            body: tidy(c.get("body").and_then(Value::as_str).unwrap_or("")),
            indent,
        }
    };
    for c in comments.iter().filter(|c| parent_of(c).is_none()) {
        main.push(Part::Blank);
        main.push(comment(c, 0));
        let id = name_at(c, "/id");
        for r in comments
            .iter()
            .filter(|r| parent_of(r).is_some() && parent_of(r) == id)
        {
            main.push(Part::Blank);
            // A reply's avatar at its parent's words.
            main.push(comment(r, kit::SAID_INDENT));
        }
    }

    let mut panel = Vec::new();
    let fact = |key: &str, value: Piece| {
        Part::Row(vec![Piece::Text(format!("{key:<11}"), Tone::Dim), value])
    };
    if let Some(state) = name_at(issue, "/state/name") {
        let tone = match name_at(issue, "/state/type").as_deref() {
            Some("completed") => Tone::Done,
            Some("started") => Tone::Needs,
            Some("canceled") => Tone::Failed,
            _ => Tone::Dim,
        };
        panel.push(fact("Status", Piece::Chip(state, tone)));
    }
    // A space where a chip has its left end, so every value starts in the
    // same column as the status's.
    let plain = |v: String| Piece::Text(format!(" {v}"), Tone::Text);
    if let Some(p) = name_at(issue, "/priorityLabel").filter(|p| p != "No priority") {
        panel.push(fact("Priority", plain(p)));
    }
    panel.push(fact(
        "Assignee",
        plain(name_at(issue, "/assignee/name").unwrap_or_else(|| "unassigned".into())),
    ));
    if let Some(project) = name_at(issue, "/project/name") {
        panel.push(fact("Project", plain(project)));
    }
    if let Some(cycle) = issue.get("cycle").filter(|c| c.is_object()) {
        let named = name_at(cycle, "/name").or_else(|| {
            cycle
                .get("number")
                .filter(|n| !n.is_null())
                .map(|n| format!("Cycle {}", util::js_string(n)))
        });
        panel.push(fact("Cycle", plain(named.unwrap_or_default())));
    }
    if let Some(estimate) = issue.get("estimate").filter(|e| !e.is_null()) {
        panel.push(fact("Estimate", plain(util::js_string(estimate))));
    }
    let labels: Vec<String> = nodes(issue, "/labels/nodes")
        .iter()
        .filter_map(|l| name_at(l, "/name"))
        .collect();
    if !labels.is_empty() {
        panel.push(fact("Labels", plain(labels.join(" · "))));
    }
    if let Some(parent) = name_at(issue, "/parent/identifier") {
        panel.push(fact("Parent", plain(parent)));
    }
    let attachments = nodes(issue, "/attachments/nodes");
    let (prs, links): (Vec<&Value>, Vec<&Value>) = attachments
        .iter()
        .partition(|a| PR_LINK.is_match(&name_at(a, "/url").unwrap_or_default()));
    if !prs.is_empty() {
        panel.extend([
            Part::Blank,
            Part::Label("Pull requests".into(), Some(prs.len().to_string())),
        ]);
        for a in prs {
            let url = name_at(a, "/url").unwrap_or_default();
            let number = PR_LINK
                .captures(&url)
                .map(|c| c[1].to_string())
                .unwrap_or_default();
            let mut row = vec![
                Piece::Link(format!("#{number}"), Tone::Accent, url.clone()),
                Piece::Text(
                    format!(" {}", name_at(a, "/title").unwrap_or_default()),
                    Tone::Text,
                ),
            ];
            // Linear's GitHub integration keeps the PR's state with it.
            let (mark, said) = match name_at(a, "/metadata/status").as_deref() {
                Some("merged") => (Some(Mark::Passed), "merged"),
                Some("closed") => (Some(Mark::Failed), "closed"),
                Some("draft") => (Some(Mark::Idle), "draft"),
                Some("open" | "inReview" | "approved") => (Some(Mark::Pending), "open"),
                _ => (None, ""),
            };
            if let Some(m) = mark {
                row.extend([
                    Piece::Text("  ".into(), Tone::Text),
                    Piece::Mark(m),
                    Piece::Text(format!(" {said}"), Tone::Dim),
                ]);
            }
            panel.push(Part::Row(row));
        }
    }
    let relations = nodes(issue, "/relations/nodes");
    if !relations.is_empty() {
        panel.extend([
            Part::Blank,
            Part::Label("Related".into(), Some(relations.len().to_string())),
        ]);
        for r in relations {
            let other = r.get("relatedIssue").unwrap_or(&Value::Null);
            let kind = match r.get("type").and_then(Value::as_str) {
                Some("blocks") => "blocks ",
                Some("duplicate") => "duplicates ",
                _ => "",
            };
            let mut row = vec![
                Piece::Mark(state_mark(other)),
                Piece::Text(format!(" {kind}"), Tone::Dim),
                Piece::Link(
                    name_at(other, "/identifier").unwrap_or_default(),
                    Tone::Dim,
                    name_at(other, "/url").unwrap_or_default(),
                ),
                Piece::Text(
                    format!("  {}", name_at(other, "/title").unwrap_or_default()),
                    Tone::Text,
                ),
            ];
            if let Some(state) = name_at(other, "/state/name") {
                row.push(Piece::Text(format!(" · {state}"), Tone::Dim));
            }
            panel.push(Part::Row(row));
        }
    }
    if !links.is_empty() {
        panel.extend([Part::Blank, Part::Label("Links".into(), None)]);
        for a in links {
            let url = name_at(a, "/url").unwrap_or_default();
            let title = name_at(a, "/title").unwrap_or_else(|| url.clone());
            panel.push(Part::Row(vec![Piece::Link(title, Tone::Text, url)]));
        }
    }
    Board {
        main,
        panel,
        place: PLACE.into(),
        keys: [
            ("j/k", "scroll"),
            ("/", "search"),
            ("r", "read again"),
            ("M-l", "in Linear"),
            ("M-v", "its PR"),
            ("M-c", "back to Claude"),
        ]
        .map(|(k, w)| (k.to_string(), w.to_string()))
        .to_vec(),
    }
}

/// What the view shows of a ticket: board 14 with UX 2.0 on, else its
/// Markdown, as pr-view.js laid it out.
fn content(issue: &Value) -> Content {
    if settings::is_on("ux2") {
        Content::Board(board(issue, jiff::Timestamp::now()))
    } else {
        Content::Markdown {
            text: markdown(issue),
            place: PLACE.into(),
        }
    }
}

/// The ticket from Linear, or why it couldn't be read.
pub async fn fetch(id: &str) -> Result<Value, String> {
    let data = linear::graphql(QUERY, json!({ "id": id }), SOURCE)
        .await
        .map_err(|e| format!("Couldn't read {id} from Linear: {e}"))?;
    data.get("issue")
        .filter(|i| i.is_object())
        .cloned()
        .ok_or_else(|| format!("Linear has no ticket {id}."))
}

/// A ticket or project of yours, for the picker that chooses a tab's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Yours {
    pub ticket: Ticket,
    /// Its ID, title and state, as the picker lists it.
    pub row: String,
    created: String,
}

const YOUR_TICKETS: &str = "query($after: String) { viewer { \
    assignedIssues(first: 50, after: $after, orderBy: createdAt) { \
    nodes { identifier title createdAt state { name } } pageInfo { hasNextPage endCursor } } } }";
const YOUR_PROJECTS: &str = "query($after: String) { projects(first: 50, after: $after, orderBy: createdAt, \
    filter: { members: { some: { isMe: { eq: true } } } }) { \
    nodes { id name createdAt status { name } } pageInfo { hasNextPage endCursor } } }";

/// Every page of a list `query` asks for, its connection at `at`, as one
/// answer of the first page's shape: up to twenty pages (a thousand), the
/// picker filtering them on the server's task. A page after the first
/// that fails, or one past the twentieth, ends the list there, and says so.
async fn every_page(
    query: &str,
    at: &str,
    source: &str,
) -> anyhow::Result<(Value, Option<String>)> {
    let mut whole = linear::graphql(query, json!({}), source).await?;
    let mut all: Vec<Value> = nodes(&whole, &format!("{at}/nodes"))
        .into_iter()
        .cloned()
        .collect();
    let mut info = whole.pointer(&format!("{at}/pageInfo")).cloned();
    let mut stopped = None;
    for _ in 1..20 {
        let more = info.as_ref().and_then(|i| {
            (i.get("hasNextPage") == Some(&json!(true)))
                .then(|| i.get("endCursor").cloned())
                .flatten()
        });
        let Some(after) = more else {
            break;
        };
        match linear::graphql(query, json!({ "after": after }), source).await {
            Ok(page) => {
                all.extend(nodes(&page, &format!("{at}/nodes")).into_iter().cloned());
                info = page.pointer(&format!("{at}/pageInfo")).cloned();
            }
            Err(e) => {
                stopped = Some(e.to_string());
                break;
            }
        }
    }
    let more = info
        .as_ref()
        .is_some_and(|i| i.get("hasNextPage") == Some(&json!(true)));
    if stopped.is_none() && more {
        stopped = Some(format!("the first {} are listed", all.len()));
    }
    if let Some(list) = whole.pointer_mut(&format!("{at}/nodes")) {
        *list = Value::Array(all);
    }
    Ok((whole, stopped))
}

/// What the picker lists, and what of it Linear couldn't give.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lists {
    pub found: Vec<Yours>,
    /// A line for each list not read, or read only in part.
    pub missing: Vec<String>,
}

/// The tickets assigned to you and the projects you're a member of, most
/// recently created first, with what couldn't be read; an error when
/// nothing could.
pub async fn yours() -> Result<Lists, String> {
    const SOURCE: &str = "choose a tab's ticket";
    let tickets = every_page(YOUR_TICKETS, "/viewer/assignedIssues", SOURCE).await;
    let projects = every_page(YOUR_PROJECTS, "/projects", SOURCE).await;
    let found = yours_in(
        tickets.as_ref().map(|(v, _)| v).unwrap_or(&Value::Null),
        projects.as_ref().map(|(v, _)| v).unwrap_or(&Value::Null),
    );
    let missing: Vec<String> = [("tickets", &tickets), ("projects", &projects)]
        .into_iter()
        .filter_map(|(what, read)| match read {
            Err(e) => Some(format!("Couldn't list your {what}: {e}")),
            Ok((_, Some(e))) => Some(format!("Only some of your {what} are listed: {e}")),
            Ok((_, None)) => None,
        })
        .collect();
    if found.is_empty() && !missing.is_empty() {
        return Err(missing.join("\n"));
    }
    Ok(Lists { found, missing })
}

/// `yours` from Linear's answers.
pub fn yours_in(tickets: &Value, projects: &Value) -> Vec<Yours> {
    let text = |v: &Value, path: &str| name_at(v, path).unwrap_or_default();
    let mut out: Vec<Yours> = nodes(tickets, "/viewer/assignedIssues/nodes")
        .into_iter()
        .filter_map(|t| {
            let id = name_at(t, "/identifier")?;
            Some(Yours {
                row: format!("{id}  {} · {}", text(t, "/title"), text(t, "/state/name")),
                ticket: Ticket::Linear(id),
                created: text(t, "/createdAt"),
            })
        })
        .collect();
    out.extend(
        nodes(projects, "/projects/nodes")
            .into_iter()
            .filter_map(|p| {
                let id = name_at(p, "/id")?;
                Some(Yours {
                    row: format!(
                        "Project  {} · {}",
                        text(p, "/name"),
                        text(p, "/status/name")
                    ),
                    ticket: Ticket::Project(id),
                    created: text(p, "/createdAt"),
                })
            }),
    );
    out.sort_by(|a, b| b.created.cmp(&a.created));
    out
}

pub(crate) fn cache_file(id: &str) -> std::path::PathBuf {
    paths::cache_dir()
        .join("ticket-view")
        .join(format!("{id}.json"))
}

/// The ticket view: the last copy at once, then the ticket read again, and
/// again as it changes (`follow`).
pub fn main(id: Option<String>) -> ExitCode {
    let Some(id) = id else {
        return pager::run(
            Content::Failed("No Linear ticket in this branch or tab name.".into()),
            None,
            None,
        );
    };
    let cache = cache_file(&id);
    let cached = util::read_json(&cache);
    let initial = match &cached {
        Some(issue) => content(issue),
        None => Content::Loading(format!("Reading {id}...")),
    };
    let read = move || util::block_on(fetch(&id));
    let file = move || Some(cache.clone());
    pager::run_following(initial, follow::start(cached, file, read, content))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue() -> Value {
        json!({
            "identifier": "AB-1", "title": "Make it faster", "url": "https://linear.app/x/issue/AB-1",
            "description": "It is slow. See https://linear.app/x/issue/AB-2/slow-thing for more.",
            "createdAt": "2026-10-01T09:00:00Z", "updatedAt": "2026-10-03T10:00:00Z",
            "priorityLabel": "High", "estimate": 3,
            "state": { "name": "In Progress" }, "assignee": { "name": "Jo" }, "creator": { "name": "Sam" },
            "project": { "name": "Speed" }, "cycle": { "number": 7, "name": null },
            "labels": { "nodes": [{ "name": "perf" }, { "name": "backend" }] },
            "parent": { "identifier": "AB-0", "title": "Speed it all up" },
            "children": { "nodes": [{ "identifier": "AB-3", "title": "Cache it", "state": { "name": "Todo" } }] },
            "relations": { "nodes": [{ "type": "blocks", "relatedIssue": { "identifier": "AB-4", "title": "Ship", "state": null } }] },
            "attachments": { "nodes": [{ "title": "PR #12", "url": "https://github.com/o/r/pull/12" }] },
            "comments": { "nodes": [
                { "id": "c2", "body": "Agreed.", "createdAt": "2026-10-02T00:00:00Z", "parent": { "id": "c1" }, "user": { "name": "Jo" } },
                { "id": "c1", "body": "Where is it slow?", "createdAt": "2026-10-01T12:00:00Z", "parent": null, "user": null, "botActor": { "name": "Bot" } },
                { "id": "c3", "body": "Found it.", "createdAt": "2026-10-03T00:00:00Z", "parent": null, "user": { "name": "Sam" } }
            ] }
        })
    }

    #[test]
    fn a_ticket_is_laid_out_as_its_page_in_linear() {
        let md = markdown(&issue());
        let expected = "# AB-1: Make it faster\n\n\
            _In Progress · High · assigned to Jo · project Speed · cycle 7 · estimate 3 · perf, backend_\n\n\
            _Opened by Sam on 2026-10-01, updated 2026-10-03 · sub-issue of AB-0 Speed it all up_\n\n\
            ---\n\n\
            It is slow. See [AB-2](https://linear.app/x/issue/AB-2/slow-thing) for more.\n\n\
            ## Sub-issues\n\n- **AB-3** Cache it _(Todo)_\n\n\
            ## Related\n\n- Blocks **AB-4** Ship\n\n\
            ## Links\n\n- [PR #12](https://github.com/o/r/pull/12)\n\n\
            ## Comments\n\n\
            **Bot** · 2026-10-01\n\nWhere is it slow?\n\n\
            > ↳ **Jo** · 2026-10-02\n>\n> Agreed.\n\n---\n\n\
            **Sam** · 2026-10-03\n\nFound it.";
        assert_eq!(md, expected);
    }

    #[test]
    fn the_board_has_sub_issues_threads_and_the_facts_beside() {
        let mut issue = issue();
        issue["state"] = json!({ "name": "In Review", "type": "started" });
        issue["creator"] = json!({ "id": "u-sam", "name": "Sam" });
        issue["children"] = json!({ "nodes": [
            { "identifier": "AB-3", "title": "Cache it", "url": "https://linear.app/x/issue/AB-3", "state": { "name": "Done", "type": "completed" } },
            { "identifier": "AB-5", "title": "Warm it", "state": { "name": "Todo", "type": "unstarted" } }
        ] });
        issue["relations"] = json!({ "nodes": [
            { "type": "blocks", "relatedIssue": { "identifier": "AB-4", "title": "Ship", "state": null } },
            { "type": "related", "relatedIssue": { "identifier": "AB-6", "title": "Tidy", "state": { "name": "Done", "type": "completed" } } }
        ] });
        issue["attachments"] = json!({ "nodes": [
            { "title": "Cache the login", "url": "https://github.com/o/r/pull/12", "metadata": { "status": "merged" } },
            { "title": "Plan", "url": "https://example.com/plan" }
        ] });
        let now = "2026-10-04T12:00:00Z".parse().unwrap();
        let b = board(&issue, now);
        assert!(
            b.wanted(&b.place)
                .contains(&super::super::markdown::Wanted::Photo(Who::new(
                    Service::Linear,
                    "u-sam"
                )))
        );
        let pager = pager::Pager::new(Content::Board(b), 130, 40);
        let screen = pager.screen().join("\n");
        for want in [
            "AB-1  Make it faster",
            "S    Sam opened it 3 days ago",
            "It is slow. See AB-2 for more.",
            "SUB-ISSUES",
            "1 of 2 done",
            "✓ AB-3  Cache it",
            "○ AB-5  Warm it",
            "COMMENTS",
            "  B    Bot  3 days ago",
            "       Where is it slow?",
            "       J    Jo  2 days ago",
            "            Agreed.",
            "  S    Sam  yesterday",
            "       Found it.",
            "Status      In Review",
            "Priority    High",
            "Assignee    Jo",
            "Cycle       Cycle 7",
            "Labels      perf · backend",
            "Parent      AB-0",
            "PULL REQUESTS",
            "#12 Cache the login  ✓ merged",
            "RELATED",
            "○ blocks AB-4  Ship",
            "✓ AB-6  Tidy · Done",
            "LINKS",
            "Plan",
        ] {
            assert!(screen.contains(want), "{want:?} in:\n{screen}");
        }
        assert!(pager.frame().contains("M-l"), "its keys on the footer");
    }

    #[test]
    fn a_bare_ticket_with_nothing_else_still_reads() {
        let md =
            markdown(&json!({ "identifier": "AB-9", "title": "T", "labels": { "nodes": [] } }));
        assert!(md.starts_with("# AB-9: T\n\n_unassigned_\n\n_Opened by someone on , updated _"));
        assert!(md.ends_with("---\n\n_No description._"));
    }

    #[test]
    fn identifiers_are_found_in_arguments_and_tab_names() {
        assert_eq!(id_of("eng-42"), Some("ENG-42".into()));
        assert_eq!(id_of("/tmp/eng-42"), None);
        assert_eq!(
            id_in_name("Shaping ABC-123: a thing"),
            Some("ABC-123".into())
        );
        assert_eq!(id_in_name("my tab"), None);
        assert_eq!(
            tidy("<https://linear.app/t/issue/AB-1>"),
            "[AB-1](https://linear.app/t/issue/AB-1)"
        );
        assert_eq!(
            tidy("[x](https://linear.app/t/issue/AB-1)"),
            "[x](https://linear.app/t/issue/AB-1)",
            "a link already named stays"
        );
    }

    #[test]
    fn a_name_gives_a_ticket_as_a_whole_word_and_a_project_by_its_id() {
        assert_eq!(id_in_name("Shaping P-ABC-4: x"), None);
        assert_eq!(
            in_name("Shaping P-ABC-4: x"),
            Some(Ticket::Project("P-ABC-4".into()))
        );
        assert_eq!(in_name("ABC-12: x"), Some(Ticket::Linear("ABC-12".into())));
        assert_eq!(
            in_name("P-ABC-4-follow-up"),
            Some(Ticket::Project("P-ABC-4".into()))
        );
        assert_eq!(
            in_name("Shaping ABC-42: follow up on P-ABC-4"),
            Some(Ticket::Linear("ABC-42".into())),
            "the first the name gives"
        );
        assert_eq!(id_in_name("XABC-12x"), None);
        assert_eq!(id_in_name("ABC-42-fix-login"), Some("ABC-42".into()));
        assert_eq!(id_in_name("feat-ABC-42-fix-login"), Some("ABC-42".into()));
        assert_eq!(id_in_name("plan-P-ABC-4-review"), None);
        assert_eq!(
            in_name("Shaping p-ENG-4"),
            Some(Ticket::Project("P-ENG-4".into()))
        );
        assert_eq!(
            in_name("plan-P-ABC-4-review"),
            Some(Ticket::Project("P-ABC-4".into()))
        );
        assert_eq!(
            branch_ticket("p-abc-4-eng-7-fix"),
            Some((8, "ENG-7".into()))
        );
        assert_eq!(branch_ticket("eng-42x-fix"), None);
        assert_eq!(branch_ticket("feature/123eng-42-fix"), None);
        assert_eq!(branch_ticket("me/eng-42"), Some((3, "ENG-42".into())));
        assert_eq!(
            linear_in_branch("p-abc-4-review-cost"),
            Some(Ticket::Project("P-ABC-4".into()))
        );
        assert_eq!(
            linear_in_branch("eng-42-p-abc-4"),
            Some(Ticket::Linear("ENG-42".into()))
        );
    }

    #[test]
    fn a_ticket_reads_back_from_what_it_is_written_as() {
        let issue = Ticket::Issue {
            repo: github::Repo {
                owner: "acme".into(),
                name: "widgets".into(),
            },
            number: 12,
        };
        for t in [
            Ticket::Linear("ENG-4".into()),
            Ticket::Project("P-ENG-4".into()),
            Ticket::Project("1f2e-uuid".into()),
            issue.clone(),
        ] {
            assert_eq!(Ticket::parse(&t.arg()), Some(t));
        }
        assert_eq!(
            Ticket::parse("p-eng-4"),
            Some(Ticket::Project("P-ENG-4".into()))
        );
        assert_eq!(issue.service(), "github");
        assert_eq!(
            issue.url().as_deref(),
            Some("https://github.com/acme/widgets/issues/12")
        );
    }

    /// A checkout made of git's files alone, on `branch`, its config `config`.
    fn checkout(branch: &str, config: &str) -> crate::tabs::git::tests::Scratch {
        let dir =
            crate::tabs::git::tests::Scratch::new(&format!("ticket-{}", branch.replace('/', "-")));
        let git = dir.0.join(".git");
        let head = git.join("refs/heads").join(branch);
        std::fs::create_dir_all(head.parent().unwrap()).unwrap();
        std::fs::write(&head, "0123456789012345678901234567890123456789\n").unwrap();
        std::fs::write(git.join("HEAD"), format!("ref: refs/heads/{branch}\n")).unwrap();
        std::fs::write(git.join("config"), config).unwrap();
        dir
    }

    #[test]
    fn a_branch_s_ticket_is_its_issue_or_its_linear_ticket_as_the_provider_says() {
        let origin = "[remote \"origin\"]\n\turl = git@github.com:acme/widgets.git\n";
        let dir = checkout("issue/12-eng-7-fix", origin);
        let issue = Ticket::Issue {
            repo: github::Repo {
                owner: "acme".into(),
                name: "widgets".into(),
            },
            number: 12,
        };
        assert_eq!(of_dir(&dir.0, Some("github")), Some(issue.clone()));
        assert_eq!(of_dir(&dir.0, None), Some(issue));
        assert_eq!(
            of_dir(&dir.0, Some("linear")),
            Some(Ticket::Linear("ENG-7".into()))
        );

        // The issue its config names, whatever the branch is called.
        let named = format!("{origin}[branch \"fix-it\"]\n\twtnIssue = acme/other#40\n");
        let dir = checkout("fix-it", &named);
        assert_eq!(
            of_dir(&dir.0, None),
            Some(Ticket::Issue {
                repo: github::Repo {
                    owner: "acme".into(),
                    name: "other".into(),
                },
                number: 40,
            })
        );
        let dir = checkout("eng-9-thing", origin);
        assert_eq!(of_dir(&dir.0, Some("github")), None);
        assert_eq!(of_dir(&dir.0, None), Some(Ticket::Linear("ENG-9".into())));
    }

    #[test]
    fn your_tickets_and_projects_list_newest_first() {
        let tickets = json!({ "viewer": { "assignedIssues": { "nodes": [
            { "identifier": "AB-1", "title": "Old", "createdAt": "2026-01-01T00:00:00Z", "state": { "name": "Todo" } },
            { "identifier": "AB-2", "title": "New", "createdAt": "2026-03-01T00:00:00Z", "state": { "name": "Done" } }
        ] } } });
        let projects = json!({ "projects": { "nodes": [
            { "id": "p1", "name": "Speed", "createdAt": "2026-02-01T00:00:00Z", "status": { "name": "Backlog" } }
        ] } });
        let rows: Vec<String> = yours_in(&tickets, &projects)
            .into_iter()
            .map(|y| format!("{} = {}", y.row, y.ticket.arg()))
            .collect();
        assert_eq!(
            rows,
            [
                "AB-2  New · Done = AB-2",
                "Project  Speed · Backlog = project:p1",
                "AB-1  Old · Todo = AB-1"
            ]
        );
    }
}
