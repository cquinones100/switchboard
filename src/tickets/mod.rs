//! Where a repository's tickets come from, behind one interface: every part
//! that reads a ticket (`wt`, M-W, M-i and M-l, the tab's ticket and name,
//! the Tickets view's menu, `/shape`'s label, autopilot) asks it here
//! rather than naming a service itself.
//!
//! A source is a `Source`, one of the services repos.json's `tickets` can
//! name (`Kept`, which adds a note's checklist, `notes`, kept apart since
//! its items have no ID, state or link to read), and a ticket is a `Ticket`,
//! the service that keeps it given by `Ticket::source`. What differs by
//! source is a method of `Source` or `Ticket` that matches on it once, and
//! the source's own module (`github`, `linear`) holds what it does:
//! recognizing a ticket in text, a branch or a tab's name, listing the
//! tickets you'd choose a tab's from, and the link to open. Reading the
//! service itself goes through `integrations`, behind each source's setting
//! (`Source::key`, the same word as repos.json's `tickets`), so the request
//! audit and `integrations::run_by_you` apply as they always did.
//!
//! A new source is a variant of `Source` (its key, a setting of that name
//! in `assets/settings.json`, and its words, listed by hand in `Source::ALL`
//! and `Source::OFFERED`, which no compiler check covers), a module here with what it
//! recognizes and lists, its service under `integrations`, and the arms the
//! compiler then asks for: the views of its tickets, and how `wt` starts
//! work from one.

pub mod github;
pub mod linear;

use std::path::Path;

use crate::integrations::github::Repo;

/// A service a repository's tickets can be kept in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Github,
    Linear,
}

impl Source {
    pub const ALL: [Source; 2] = [Source::Github, Source::Linear];
    /// The order `wt` and M-W offer them in, where the repository's
    /// `tickets` isn't set.
    pub const OFFERED: [Source; 2] = [Source::Linear, Source::Github];

    /// Its key: the setting that turns it on and off, and what repos.json's
    /// `tickets` says for it.
    pub fn key(self) -> &'static str {
        match self {
            Source::Github => "github",
            Source::Linear => "linear",
        }
    }

    /// `key`'s inverse.
    pub fn parse(key: &str) -> Option<Source> {
        Source::ALL.into_iter().find(|s| s.key() == key)
    }

    /// How it is named to you.
    pub fn name(self) -> &'static str {
        match self {
            Source::Github => "GitHub",
            Source::Linear => "Linear",
        }
    }

    /// Whether its setting is on.
    pub fn is_on(self) -> bool {
        crate::settings::is_on(self.key())
    }

    /// What `wt` and M-W's question of where to start work from call it.
    pub fn offered_words(self) -> &'static str {
        match self {
            Source::Github => "A GitHub issue",
            Source::Linear => "A Linear ticket assigned to you",
        }
    }

    /// Whether work can start from its tickets in the repository at
    /// `root`: GitHub's only for a repository on GitHub. Files alone.
    pub fn offered_at(self, root: &Path) -> bool {
        match self {
            Source::Github => crate::sources::repo::origin_of(root).is_some(),
            Source::Linear => true,
        }
    }

    /// "Shape a new ticket…" naming it.
    pub fn shape_label(self) -> &'static str {
        match self {
            Source::Github => "Shape a new ticket as a GitHub issue…",
            Source::Linear => "Shape a new ticket in Linear…",
        }
    }

    /// What choosing a tab's ticket from its tickets says.
    pub fn choosing(self) -> Choosing {
        match self {
            Source::Github => Choosing {
                getting: "Getting the repository's open issues…",
                none: "GitHub has no open issues in this repository.",
                list: "the open issues",
                one: "issue",
            },
            Source::Linear => Choosing {
                getting: "Getting your tickets…",
                none: "Linear has no tickets or projects of yours.",
                list: "your tickets",
                one: "ticket or project",
            },
        }
    }

    /// The tickets the branch checked out in `dir` was started for, from
    /// git's own files, `branch` its name, in the order they count.
    fn of_branch(self, dir: &Path, common_dir: &Path, branch: &str) -> Vec<Ticket> {
        match self {
            Source::Github => github::of_branch(dir, common_dir, branch),
            Source::Linear => linear::in_branch(branch).into_iter().collect(),
        }
    }

    /// The ticket a branch's name alone gives, as a tab names it short, for
    /// the drawing path, which reads no file.
    pub fn short_in_branch(self, branch: &str) -> Option<String> {
        match self {
            Source::Github => github::issue_number_of_branch(branch).map(|n| format!("#{n}")),
            Source::Linear => linear::in_branch(branch).map(|t| t.short()),
        }
    }

    /// The ticket a branch's name leads with, as a worktree's row names it:
    /// an issue's branch's `#12`, a Linear ticket's ID at its start.
    pub fn leading_in_branch(self, branch: &str) -> Option<String> {
        match self {
            Source::Github => github::issue_number_of_branch(branch).map(|n| format!("#{n}")),
            Source::Linear => linear::leading_in_branch(branch),
        }
    }

    /// The ticket of its kind a name gives (a tab's label, "Shaping
    /// ABC-123: ..."), when its tickets are named by an ID a name can hold.
    pub fn in_name(self, name: &str) -> Option<Ticket> {
        match self {
            Source::Github => None,
            Source::Linear => linear::in_name(name),
        }
    }

    /// What M-i and "Choose this tab's ticket or project…" say each time
    /// in a repository whose tickets it keeps while it is off, when they
    /// say more than its tip: GitHub's, since the key still works for
    /// Linear's tickets elsewhere and would otherwise do nothing without a
    /// word.
    pub fn off_said(self) -> Option<&'static str> {
        match self {
            Source::Github => Some(
                "This repository keeps its tickets as GitHub issues, and GitHub is off in Settings.",
            ),
            Source::Linear => None,
        }
    }

    /// What the ticket view says finding none of its tickets in a branch.
    fn none_in_branch_words(self) -> &'static str {
        match self {
            Source::Github => "No GitHub issue for this branch.",
            Source::Linear => "No Linear ticket in this branch or tab name.",
        }
    }

    /// The tickets to choose a tab's from, for the repository at `dir`,
    /// none of a repository's own with none known. Asks the service, so
    /// off the server's task.
    pub async fn yours(self, dir: Option<&Path>) -> Result<Lists, String> {
        match (self, dir) {
            (Source::Github, Some(dir)) => github::open_issues_at(dir).await,
            (Source::Github, None) => Ok(Lists::default()),
            (Source::Linear, _) => linear::yours().await,
        }
    }
}

/// Where a repository's work is kept, its `tickets` in repos.json: one of
/// the sources, or a note's checklist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kept {
    In(Source),
    Notes,
}

impl Kept {
    /// Every value `tickets` takes: each source, then a note's checklist.
    pub fn all() -> impl Iterator<Item = Kept> {
        Source::ALL
            .into_iter()
            .map(Kept::In)
            .chain(std::iter::once(Kept::Notes))
    }

    /// As repos.json writes it.
    pub fn key(self) -> &'static str {
        match self {
            Kept::In(source) => source.key(),
            Kept::Notes => "notes",
        }
    }

    /// `key`'s inverse.
    pub fn parse(key: &str) -> Option<Kept> {
        Kept::all().find(|k| k.key() == key)
    }

    /// How Settings' Tickets row offers it.
    pub fn label(self) -> &'static str {
        match self {
            Kept::In(Source::Github) => "GitHub issues",
            Kept::In(Source::Linear) => "Linear",
            Kept::Notes => "A note's checklist, with a plan note each",
        }
    }

    /// The service it names, `None` for a note's checklist.
    pub fn source(self) -> Option<Source> {
        match self {
            Kept::In(source) => Some(source),
            Kept::Notes => None,
        }
    }

    /// "Shape a new ticket…" naming where it is filed.
    pub fn shape_label(kept: Option<Kept>) -> &'static str {
        match kept {
            Some(Kept::In(source)) => source.shape_label(),
            Some(Kept::Notes) => "Shape a new ticket as a note's item…",
            None => "Shape a new ticket…",
        }
    }
}

/// Where the work of the repository at `dir` is kept (`repos::tickets_at`).
/// Runs git, so off the server's task.
pub fn kept_at(dir: &Path) -> Result<Option<Kept>, String> {
    Ok(crate::repos::tickets_at(dir)?.and_then(|k| Kept::parse(&k)))
}

/// Where a repository's entry in repos.json says its work is kept.
pub fn kept_of(entry: &serde_json::Map<String, serde_json::Value>) -> Option<Kept> {
    crate::repos::tickets_of(entry).and_then(|k| Kept::parse(&k))
}

/// `kept_at` from the files alone (`repos::tickets_known`), for the
/// server's task.
pub fn kept_known(dir: &Path) -> Option<Kept> {
    crate::repos::tickets_known(dir).and_then(|k| Kept::parse(&k))
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
        repo: Repo,
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
        linear::project_arg(text)
            .or_else(|| github::issue_arg(text))
            .or_else(|| linear::id_of(text).map(Ticket::Linear))
    }

    /// How it is named to you: `ENG-42`, `P-ENG-4`, `owner/repo#12`.
    pub fn shown(&self) -> String {
        match self {
            Ticket::Project(id) => id.clone(),
            other => other.arg(),
        }
    }

    /// How a tab names it, short, in its label and boxes: `ENG-42`,
    /// `P-ENG-4`, an issue by its number, `#12`, its repository being the
    /// tab's own.
    pub fn short(&self) -> String {
        match self {
            Ticket::Issue { number, .. } => format!("#{number}"),
            other => other.shown(),
        }
    }

    /// The service that keeps it.
    pub fn source(&self) -> Source {
        match self {
            Ticket::Issue { .. } => Source::Github,
            Ticket::Linear(_) | Ticket::Project(_) => Source::Linear,
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

    /// Whether M-p's "Copy the branch's ticket ID" copies it: a ticket,
    /// never a project.
    pub fn copied(&self) -> bool {
        !matches!(self, Ticket::Project(_))
    }

    /// The copy of it its view last read, from the cache.
    pub fn last_read(&self) -> Option<serde_json::Value> {
        crate::util::read_json(match self {
            Ticket::Issue { repo, number } => crate::views::issue::cache_file(repo, *number),
            Ticket::Linear(id) => crate::views::ticket::cache_file(id),
            Ticket::Project(id) => crate::views::project::cache_file(id),
        })
    }

    /// Its link to open, when it is known without asking: an issue's, or a
    /// Linear ticket's at the workspace written down.
    pub fn link_now(&self) -> Option<String> {
        match self {
            Ticket::Linear(id) => linear::link_now(id),
            _ => self.url(),
        }
    }

    /// Its link, asked of its source where it isn't known without asking;
    /// `None` when the source has none for it.
    pub async fn link(&self, source: &str) -> Result<Option<String>, String> {
        if let Some(url) = self.link_now() {
            return Ok(Some(url));
        }
        match self {
            Ticket::Linear(id) => linear::ticket_url(id, source).await,
            Ticket::Project(id) => crate::views::project::url(id).await,
            Ticket::Issue { .. } => Ok(None),
        }
    }

    /// What is said when its source has no link for it.
    pub fn no_link_words(&self) -> String {
        match self {
            Ticket::Linear(id) => format!("Linear has no ticket {id}."),
            other => format!(
                "{} has no link for {}.",
                other.source().name(),
                other.shown()
            ),
        }
    }
}

/// The ticket a directory's branch is for, from git's own files, by the
/// repository's ticket source (repos.json's `tickets`): its GitHub issue
/// with `github`, its Linear ticket with `linear`, and none with `notes`
/// or none set, since nothing guesses where a repository keeps its work.
pub fn of_dir(dir: &Path, kept: Option<Kept>) -> Option<Ticket> {
    first_on(of_dir_all(dir, kept))
}

/// Every ticket `of_dir` chooses from, in its order.
pub fn of_dir_all(dir: &Path, kept: Option<Kept>) -> Vec<Ticket> {
    match kept.and_then(Kept::source) {
        Some(source) => of_branch_by(dir, source),
        None => Vec::new(),
    }
}

/// The tickets of `source` the branch checked out in `dir` was started for.
pub fn of_branch_by(dir: &Path, source: Source) -> Vec<Ticket> {
    match crate::tabs::git::locate(dir).filter(|p| p.on_branch()) {
        Some(place) => source.of_branch(dir, &place.common_dir, &place.branch),
        None => Vec::new(),
    }
}

/// The ticket of `source`'s a tab is titled after, from the branch checked
/// out in `dir`, `branch` its name, whatever the repository's `tickets`
/// says: a branch that only looks like a ticket is harmless, the source
/// having no such ticket. Projects name no tab.
pub fn titled_by_branch(
    dir: &Path,
    common_dir: &Path,
    branch: &str,
    source: Source,
) -> Option<Ticket> {
    match source {
        Source::Github => github::of_branch(dir, common_dir, branch)
            .into_iter()
            .next(),
        Source::Linear => linear::id_in_branch(branch).map(Ticket::Linear),
    }
}

/// What a reader of a tab's ticket in `dir` says finding none: that the
/// repository's ticket source isn't set (`repos::unset_words`) when it is
/// a repository with none set, else `otherwise`. Reads files alone.
pub fn none_words(dir: &Path, otherwise: &str) -> String {
    match crate::sources::repo::origin_of(dir) {
        Some(repo) if kept_known(dir).is_none() => crate::repos::unset_words(&repo.to_string()),
        _ => otherwise.to_string(),
    }
}

/// The source whose tickets M-i's picker offers, by where the repository
/// keeps its work: its own source, else Linear's tickets and projects.
pub fn picks_from(kept: Option<Kept>) -> Source {
    kept.and_then(Kept::source).unwrap_or(Source::Linear)
}

/// What the ticket view says finding no ticket for a branch of a
/// repository that keeps its work where `kept` says.
pub fn none_in_branch_words(kept: Option<Kept>) -> &'static str {
    match kept {
        Some(Kept::In(source)) => source.none_in_branch_words(),
        Some(Kept::Notes) => "No ticket: this repository keeps its work in a note's checklist.",
        // Before there were other sources, a branch's ticket was Linear's.
        None => Source::Linear.none_in_branch_words(),
    }
}

/// The first of `found` whose source is on in Settings, else the first,
/// so one of a source turned off gives way to the next and is said to be
/// off only when nothing else is found.
pub fn first_on(found: Vec<Ticket>) -> Option<Ticket> {
    let on = found.iter().position(|t| t.source().is_on());
    let mut found = found;
    match on {
        Some(at) => Some(found.swap_remove(at)),
        None => found.into_iter().next(),
    }
}

/// The ticket a name you gave a tab names, whatever the repository's
/// source, since you named it: the first any source finds.
pub fn in_name(name: &str) -> Option<Ticket> {
    Source::ALL.into_iter().find_map(|s| s.in_name(name))
}

/// The words of choosing a tab's ticket from a source's.
#[derive(Clone, Copy, Debug)]
pub struct Choosing {
    /// Said while they are read.
    pub getting: &'static str,
    /// Said when there are none.
    pub none: &'static str,
    /// The list, in "Couldn't get …".
    pub list: &'static str,
    /// One of them, in the picker's title.
    pub one: &'static str,
}

/// A ticket of yours, for the picker that chooses a tab's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Yours {
    pub ticket: Ticket,
    /// Its ID, title and state, as the picker lists it.
    pub row: String,
    created: String,
}

/// What the picker lists, and what of it the source couldn't give.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lists {
    pub found: Vec<Yours>,
    /// A line for each list not read, or read only in part.
    pub missing: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_reads_back_from_what_it_is_written_as() {
        let issue = Ticket::Issue {
            repo: Repo {
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
        assert_eq!(issue.source().key(), "github");
        assert_eq!(
            issue.url().as_deref(),
            Some("https://github.com/acme/widgets/issues/12")
        );
    }

    #[test]
    fn each_source_and_where_work_is_kept_read_back_from_their_keys() {
        for source in Source::ALL {
            assert_eq!(Source::parse(source.key()), Some(source));
        }
        for kept in Kept::all() {
            assert_eq!(Kept::parse(kept.key()), Some(kept));
        }
        assert_eq!(Kept::parse("jira"), None);
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
        let github = Some(Kept::In(Source::Github));
        let linear = Some(Kept::In(Source::Linear));
        let origin = "[remote \"origin\"]\n\turl = git@github.com:acme/widgets.git\n";
        let dir = checkout("issue/12-eng-7-fix", origin);
        let issue = Ticket::Issue {
            repo: Repo {
                owner: "acme".into(),
                name: "widgets".into(),
            },
            number: 12,
        };
        assert_eq!(of_dir(&dir.0, github), Some(issue));
        assert_eq!(of_dir(&dir.0, None), None, "no guess while it isn't set");
        assert_eq!(of_dir(&dir.0, Some(Kept::Notes)), None);
        assert_eq!(of_dir(&dir.0, linear), Some(Ticket::Linear("ENG-7".into())));

        // The issue its config names, whatever the branch is called.
        let named = format!("{origin}[branch \"fix-it\"]\n\twtnIssue = acme/other#40\n");
        let dir = checkout("fix-it", &named);
        assert_eq!(
            of_dir(&dir.0, github),
            Some(Ticket::Issue {
                repo: Repo {
                    owner: "acme".into(),
                    name: "other".into(),
                },
                number: 40,
            })
        );
        let dir = checkout("eng-9-thing", origin);
        assert_eq!(of_dir(&dir.0, github), None);
        assert_eq!(of_dir(&dir.0, None), None);
        assert_eq!(of_dir(&dir.0, linear), Some(Ticket::Linear("ENG-9".into())));
    }
}
