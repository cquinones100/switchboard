//! The keys that start work, on the server: what tmux.conf's popups for
//! M-t and M-n (tmux/new-tab.zsh), M-W (wt-open.zsh), M-B (wtb-open.zsh)
//! and M-o (mission control's Work map of worktrees with no tab), and the
//! palette's "Start work on a PR" (wtpr-open.zsh), "Start work on a note's
//! checklist items" (wtn-open.zsh) and "Continue shaping Linear tickets"
//! (shape-open.zsh) ran, as boxes over the workspace rather than popups:
//! a list to pick a directory from, then what to work on, then a tab for
//! each. New in the Rust version, Notes' `gw` starts the one item under its
//! cursor the same way, the note and the item already known.
//! The palette's "Review a contributor's PR", listed only where the main
//! clone has a review-pr project skill, lists a repository's open PRs from
//! forks and starts `/review-pr <number>` in a tab in the main clone.
//!
//! The directory list is pick-dir.zsh's: the one you are in first, then
//! every open tab's, then every one a Claude session ran in, most recent
//! first, from wt-revive's cache (`~/.cache/wt-revive/index.json`), then
//! your home; only directories that are there, and no worktree but the one
//! you are in. For the commands that make a worktree, each is its
//! repository's main clone instead, since a worktree is made beside the
//! main clone whichever of its worktrees asks, and anything in no
//! repository is left out. A path typed in is taken when nothing matches it,
//! or with Alt-Enter, as fzf's `--print-query` took it. The main clone is
//! found from the files under `.git` alone, so the list starts no process.
//!
//! What has to read the disk, the clipboard or git is done off the server's
//! task and comes back as `Event::Work`, so a slow clipboard or a large
//! cache never holds up drawing. Each tab's work runs as its own command, a
//! shell of yours that defines `wt`, `wtb`, `wtpr` or `wtn` from the
//! command's `init` (`worktree::tab_running`), where the scripts typed it
//! into the new tab's shell; the tabs go at the end, in the order asked
//! for, and the first is brought to the front.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::cmd::{work_recent, worktree_from_note};
use crate::integrations::github;
use crate::mux::keys::Action;
use crate::mux::server::{ClientId, Event, Opened, Server};
use crate::mux::{PaneId, TabId};
use crate::ui::picker::TYPED;
use crate::ui::{Confirm, PickItem, Picker};
use crate::worktree;
use crate::{paths, platform, settings, util};

/// `wtn`'s arguments for a note's item: its line, and `--target` with the
/// branch it lands on, none for the default branch.
fn item_args(line: usize, target: &str) -> Vec<String> {
    let mut args = vec!["--item".to_string(), line.to_string()];
    if !target.is_empty() {
        args.extend(["--target".to_string(), target.to_string()]);
    }
    args
}

/// What a note is known by in the repositories its items were started in:
/// its ID, else its path within the notes folder.
fn note_key(text: &str, note: &str) -> String {
    crate::notes::id::in_text(text).unwrap_or_else(|| note.to_string())
}

/// Keeps `repo` as the one the note `key` names had its items started in,
/// off the server's task.
fn keep_note_repo(key: String, repo: &Path) {
    let repo = repo.to_path_buf();
    work_recent::in_background(move || work_recent::pick_note_repo(&key, &repo));
}

/// The rest of a key's work, run back on the server's task.
type Then = Box<dyn FnOnce(&mut Server, ClientId) + Send>;

/// What comes back from work done off the server's task: the rest of what
/// a key does, given the server and the client it was pressed in.
pub struct WorkEvent {
    client: ClientId,
    then: Then,
}

/// What a directory is wanted for: any directory (a new tab, shaping), or
/// a repository's main clone (the commands that make a worktree).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wanted {
    Any,
    Repo,
}

/// What a key found before its first box: the directories to offer, the
/// one it was pressed in, and the first line of the clipboard.
struct Found {
    here: PathBuf,
    dirs: Vec<PathBuf>,
    clipboard: String,
}

// ── Directories ─────────────────────────────────────────────────────────────

/// The main clone of the repository `dir` is in, when it has a review-pr
/// project skill (`.claude/skills/review-pr/SKILL.md`), where the palette's
/// "Review a contributor's PR" is listed. Files alone, no git process.
pub fn review_repo(dir: &Path) -> Option<PathBuf> {
    main_clone(dir).filter(|root| root.join(".claude/skills/review-pr/SKILL.md").is_file())
}

/// The main clone of the repository `dir` is in, read from the files under
/// `.git` with no git process: the checkout whose `.git` is a directory, or,
/// for a linked worktree, the one its git directory's `commondir` leads to.
/// A submodule, whose git directory has no `commondir`, is its own
/// checkout. `None` outside a repository.
pub fn main_clone(dir: &Path) -> Option<PathBuf> {
    for at in dir.ancestors() {
        let dot_git = at.join(".git");
        let Ok(meta) = std::fs::metadata(&dot_git) else {
            continue;
        };
        if meta.is_dir() {
            return at.canonicalize().ok();
        }
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let named = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
        let gitdir = at.join(named);
        return match std::fs::read_to_string(gitdir.join("commondir")) {
            Ok(common) => gitdir
                .join(common.trim())
                .canonicalize()
                .ok()?
                .parent()
                .map(Path::to_path_buf),
            Err(_) => at.canonicalize().ok(),
        };
    }
    None
}

/// The directories Claude sessions ran in, the most recent first, as
/// wt-revive's cache has them.
fn recent_session_dirs() -> Vec<PathBuf> {
    let Some(cache) = util::read_json(paths::home_join(".cache/wt-revive/index.json")) else {
        return Vec::new();
    };
    let mut found: Vec<(f64, PathBuf)> = cache["transcripts"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(_, t)| {
            let cwd = t["cwd"].as_str().filter(|c| !c.is_empty())?;
            Some((t["mtimeMs"].as_f64().unwrap_or(0.0), PathBuf::from(cwd)))
        })
        .collect();
    found.sort_by(|a, b| b.0.total_cmp(&a.0));
    found.into_iter().map(|(_, d)| d).collect()
}

/// The directory list for a key pressed in `here`.
fn offered_from(here: &Path, panes: &[PathBuf], wanted: Wanted) -> Vec<PathBuf> {
    let recent = work_recent::dirs();
    offered(here, panes, &recent, &recent_session_dirs(), wanted)
}

/// pick-dir.zsh's list, each once: the directories picked before
/// (`recent`, src/cmd/work_recent.rs), the most recent first, then
/// pick-dir.zsh's order (the one you are in, every tab's, the sessions',
/// home), every one put through the same checks. With nothing picked yet,
/// the one you are in leads, so Enter alone takes it.
fn offered(
    here: &Path,
    panes: &[PathBuf],
    recent: &[PathBuf],
    sessions: &[PathBuf],
    wanted: Wanted,
) -> Vec<PathBuf> {
    let home = paths::home();
    let candidates = recent
        .iter()
        .cloned()
        .chain(std::iter::once(here.to_path_buf()))
        .chain(panes.iter().cloned())
        .chain(sessions.iter().cloned())
        .chain(std::iter::once(home));
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for dir in candidates {
        if dir.as_os_str().is_empty() || !dir.is_dir() {
            continue;
        }
        // Never someone else's PR, checked out untrusted (src/trust.rs), for
        // a new Claude tab; its main clone is yours.
        if wanted == Wanted::Any && crate::trust::in_review_dir(&dir) {
            continue;
        }
        let dir = match wanted {
            Wanted::Repo => match main_clone(&dir) {
                Some(root) => root,
                None => continue,
            },
            // Worktrees are left out: there are dozens, and a new tab
            // belongs in a repository, where wt or wtpr makes its worktree.
            // The one you are in stays, since that is where you asked from.
            Wanted::Any if dir.to_string_lossy().contains("/.claude/worktrees/") && dir != here => {
                continue;
            }
            Wanted::Any => dir,
        };
        if seen.insert(dir.clone()) {
            out.push(dir);
        }
    }
    out
}

/// A directory typed in, `~` for your home.
fn expand(typed: &str) -> PathBuf {
    let typed = typed.trim();
    match typed.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            paths::home().join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(typed),
    }
}

// ── What is asked for ───────────────────────────────────────────────────────

/// Tickets separated by commas, spaces or both, each an ID or a Linear issue
/// URL, in capitals and each once; the first piece that isn't one, as the
/// error. Only an ID ever reaches a tab's command.
pub fn parse_tickets(text: &str) -> Result<Vec<String>, String> {
    let mut tickets = Vec::new();
    for token in text.split(|c: char| c == ',' || c.is_whitespace()) {
        if token.is_empty() {
            continue;
        }
        let ticket = worktree::ticket_from(token)
            .ok_or_else(|| token.to_string())?
            .to_uppercase();
        if !tickets.contains(&ticket) {
            tickets.push(ticket);
        }
    }
    Ok(tickets)
}

/// The clipboard's first line, when every piece of it is a ticket, so a
/// copied list works as well as a single link.
fn tickets_on(clipboard: &str) -> String {
    match parse_tickets(clipboard) {
        Ok(t) if !t.is_empty() => clipboard.trim().to_string(),
        _ => String::new(),
    }
}

static PR_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https://github\.com/([A-Za-z0-9._-]+)/([A-Za-z0-9._-]+)/pull/([0-9]+)([/?#].*)?$")
        .unwrap()
});
static PR_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#?([0-9]+)$").unwrap());

/// A PR as wtpr is given it: a URL rebuilt from its owner, name and number,
/// or a bare number, which is a PR of a repository still to pick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrAsked {
    Url(String, u64),
    Number(u64),
}

pub fn parse_pr(text: &str) -> Option<PrAsked> {
    let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if let Some(m) = PR_URL.captures(&text) {
        let number = m[3].parse().ok()?;
        return Some(PrAsked::Url(
            format!("https://github.com/{}/{}/pull/{number}", &m[1], &m[2]),
            number,
        ));
    }
    PR_NUMBER
        .captures(&text)
        .and_then(|m| m[1].parse().ok())
        .map(PrAsked::Number)
}

/// Whether git would take the name as a branch, and the shell would read
/// nothing in it specially (wtb-open.zsh's check, and `git check-ref-format
/// --branch`'s rules, without starting git).
pub fn branch_name_ok(name: &str) -> bool {
    static CHARS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._/-]+$").unwrap());
    CHARS.is_match(name)
        && !name.starts_with('-')
        && !name.starts_with('/')
        && !name.ends_with('/')
        && !name.ends_with('.')
        && !name.contains("..")
        && !name.contains("//")
        && name
            .split('/')
            .all(|part| !part.starts_with('.') && !part.ends_with(".lock"))
}

/// The repository's branches, local and remote, each once under its own
/// name, the most recently committed first, with how long ago: the refs
/// already here, with no fetch.
fn branches(repo: &Path) -> Vec<(String, String)> {
    let out = worktree::git_at(
        repo,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname)\t%(committerdate:relative)",
            "refs/heads",
            "refs/remotes",
        ],
    )
    .unwrap_or_default();
    let mut seen = HashSet::new();
    out.lines()
        .filter_map(|line| {
            let (refname, when) = line.split_once('\t')?;
            if refname.ends_with("/HEAD") {
                return None;
            }
            let name = match refname.strip_prefix("refs/heads/") {
                Some(n) => n.to_string(),
                None => refname
                    .strip_prefix("refs/remotes/")?
                    .split_once('/')?
                    .1
                    .to_string(),
            };
            seen.insert(name.clone()).then(|| (name, when.to_string()))
        })
        .collect()
}

// ── On the server ───────────────────────────────────────────────────────────

impl Server {
    /// Moves a tab to the end of its session's tabs, so tabs opened one
    /// after another keep the order they were asked for in and no open tab
    /// is renumbered: M-1 to M-9 reach tabs by number.
    pub fn tab_to_end(&mut self, tab: TabId) {
        let Some(name) = self.session_of_tab(tab).map(|s| s.name.clone()) else {
            return;
        };
        if let Some(s) = self.session_mut(&name) {
            s.tabs.retain(|t| *t != tab);
            s.tabs.push(tab);
        }
        self.notices_last(&name);
        self.dirty_session(&name);
    }

    /// The rest of a key's work, back from off the task.
    pub fn work_event(&mut self, event: WorkEvent) {
        if self.clients.contains_key(&event.client) {
            (event.then)(self, event.client);
        }
    }

    /// Runs `work` off the server's task and `then` with what it gives,
    /// back on it.
    pub(crate) fn off_task<T: Send + 'static>(
        &self,
        client: ClientId,
        work: impl FnOnce() -> T + Send + 'static,
        then: impl FnOnce(&mut Server, ClientId, T) + Send + 'static,
    ) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let found = work();
            let _ = tx.send(Event::Work(WorkEvent {
                client,
                then: Box::new(move |server, client| then(server, client, found)),
            }));
        });
    }

    /// One of the keys that start work, pressed in a client whose current
    /// pane is `pane`.
    pub fn work_action(&mut self, client: ClientId, action: Action, pane: Option<PaneId>) {
        let here = pane
            .and_then(|p| self.pane_own_dir(p))
            .unwrap_or_else(paths::home);
        let panes = self.pane_dirs();
        let wanted = match action {
            Action::NewClaudeTab | Action::ContinueShaping => Wanted::Any,
            _ => Wanted::Repo,
        };
        let clipboard = matches!(
            action,
            Action::WorkOnTickets | Action::WorkOnPr | Action::ContinueShaping
        );
        self.off_task(
            client,
            move || Found {
                dirs: offered_from(&here, &panes, wanted),
                here,
                clipboard: if clipboard {
                    platform::paste()
                        .and_then(|t| t.lines().next().map(str::to_string))
                        .unwrap_or_default()
                } else {
                    String::new()
                },
            },
            move |server, client, found| match action {
                Action::NewClaudeTab => {
                    server.ask_dir(client, "New tab", found, Wanted::Any, |s, c, dir, _| {
                        s.open_claude_tab(c, &dir)
                    })
                }
                Action::WorkOnTickets => server.ask_dir(
                    client,
                    "Work on tickets: the repository",
                    found,
                    Wanted::Repo,
                    |s, c, dir, clipboard| s.ask_tickets(c, dir, &clipboard),
                ),
                Action::WorkOnBranch => server.ask_dir(
                    client,
                    "Work on a branch: the repository",
                    found,
                    Wanted::Repo,
                    |s, c, dir, _| s.ask_branch(c, dir),
                ),
                Action::WorkOnPr => server.ask_pr(client, found),
                Action::WorkOnNote => server.ask_dir(
                    client,
                    "Work on a note's items: the repository",
                    found,
                    Wanted::Repo,
                    |s, c, dir, _| s.ask_note(c, dir),
                ),
                Action::ContinueShaping => server.ask_shaping(client, found),
                _ => {}
            },
        );
    }

    /// Every pane's own directory, for the directory list a tab starts in
    /// (never a tab's `@tab_dir`, someone else's PR).
    fn pane_dirs(&self) -> Vec<PathBuf> {
        self.panes
            .iter()
            .filter(|(_, s)| {
                s.opts
                    .get("@sidebar")
                    .is_none_or(|v| v.is_empty() || v == "0")
            })
            .filter_map(|(p, _)| self.pane_own_dir(*p))
            .collect()
    }

    /// Asks which directory, as M-t does, the client's current pane's
    /// first; `then` is given the one picked or typed. A note's new tab
    /// (src/notes/serve/tab.rs) asks this way, as new-tab.zsh did for it.
    pub(crate) fn pick_dir_for(
        &mut self,
        client: ClientId,
        title: &str,
        then: impl FnOnce(&mut Server, ClientId, PathBuf) + Send + 'static,
    ) {
        let here = self
            .current_pane(client)
            .and_then(|p| self.pane_own_dir(p))
            .unwrap_or_else(paths::home);
        let panes = self.pane_dirs();
        let title = title.to_string();
        self.off_task(
            client,
            move || Found {
                dirs: offered_from(&here, &panes, Wanted::Any),
                here,
                clipboard: String::new(),
            },
            move |server, client, found| {
                server.ask_dir(client, &title, found, Wanted::Any, |s, c, dir, _| {
                    then(s, c, dir)
                })
            },
        );
    }

    /// Asks which directory, the ones picked before first, else the one the
    /// key was pressed in; `then` is given the one picked or typed, and the
    /// clipboard's first line, and the pick is kept for next time.
    fn ask_dir(
        &mut self,
        client: ClientId,
        title: &str,
        found: Found,
        wanted: Wanted,
        then: impl FnOnce(&mut Server, ClientId, PathBuf, String) + Send + 'static,
    ) {
        let items: Vec<PickItem> = found
            .dirs
            .iter()
            .map(|d| PickItem::new(paths::tilde(d), d.to_string_lossy()))
            .collect();
        // Matches keep the list's order, as pick-dir.zsh's fzf --no-sort:
        // the ones picked before, here, the tabs' directories, the
        // sessions', then home.
        let picker = Picker::new(title, items)
            .named("pick-dir")
            .prompt("Open in > ")
            .exact()
            .free()
            .order(Box::new(|_, matched| matched.to_vec()));
        let clipboard = found.clipboard;
        let here = found.here;
        self.pick(client, picker, move |server, client, picked| {
            let Some(picked) = picked else {
                return;
            };
            let dir = if picked.index == TYPED {
                let typed = expand(&picked.value);
                if typed.is_relative() {
                    here.join(typed)
                } else {
                    typed
                }
            } else {
                PathBuf::from(&picked.value)
            };
            if !dir.is_dir() {
                server.say(
                    Some(client),
                    &format!("No such directory: {}", picked.value),
                );
                return;
            }
            let dir = match wanted {
                Wanted::Repo => match main_clone(&dir) {
                    Some(root) => root,
                    None => {
                        server.say(
                            Some(client),
                            &format!("Not in a repository: {}", picked.value),
                        );
                        return;
                    }
                },
                Wanted::Any => dir,
            };
            let kept = dir.clone();
            work_recent::in_background(move || work_recent::pick_dir(&kept));
            then(server, client, dir, clipboard);
        });
    }

    /// Opens a tab at the end of the client's session, in `dir`, running
    /// `command`, in front or behind as `opened` says. Says why when it can't.
    pub(crate) fn work_tab(
        &mut self,
        client: ClientId,
        dir: &Path,
        command: Vec<String>,
        opened: Opened,
    ) -> Option<TabId> {
        let session = self.clients.get(&client)?.session.clone();
        match self.new_tab(
            &session,
            Some(dir.to_path_buf()),
            Some(command),
            None,
            opened,
        ) {
            Ok((tab, _)) => {
                self.tab_to_end(tab);
                Some(tab)
            }
            Err(e) => {
                self.say(Some(client), &format!("Couldn't open a tab: {e}"));
                None
            }
        }
    }

    /// Brings the first of the tabs just opened to the front.
    pub(crate) fn front(&mut self, client: ClientId, tabs: &[TabId]) {
        let (Some(first), Some(session)) = (
            tabs.first(),
            self.clients.get(&client).map(|c| c.session.clone()),
        ) else {
            return;
        };
        self.select_tab(&session, *first);
    }

    /// M-t and M-n: a new tab in `dir` running a new Claude, from an
    /// interactive shell of yours for your PATH, left at that shell when
    /// Claude exits.
    fn open_claude_tab(&mut self, client: ClientId, dir: &Path) {
        // Never in a worktree of someone else's PR, whose project settings
        // and hooks Claude would load: known by its path alone, since this
        // runs on the server's task (src/trust.rs).
        if crate::trust::in_review_dir(dir) {
            let refusal = crate::trust::refusal(dir);
            self.say(Some(client), &refusal);
            return;
        }
        let command = worktree::tab_command("claude", "claude", &[]);
        if let Some(tab) = self.work_tab(client, dir, command, Opened::Front) {
            self.front(client, &[tab]);
            // Then what to call it, as new-tab.zsh ended in tab-name.zsh.
            if settings::is_on("name_new_tabs") {
                self.ask_tab_name(client, tab, dir.to_path_buf());
            }
        }
    }

    /// M-W, once the repository is picked: which tickets, the clipboard's
    /// when it holds only tickets. Every ticket is checked before any tab
    /// opens, so a typo in one leaves none half-started.
    fn ask_tickets(&mut self, client: ClientId, repo: PathBuf, clipboard: &str) {
        let picker = Picker::new(
            "Linear ticket IDs or URLs, separated by commas or spaces",
            Vec::new(),
        )
        .named("tickets")
        .prompt("Tickets: ")
        .free()
        .query(&tickets_on(clipboard));
        self.pick(client, picker, move |server, client, picked| {
            let Some(picked) = picked else {
                return;
            };
            let tickets = match parse_tickets(&picked.value) {
                Ok(t) => t,
                Err(bad) => {
                    server.say(
                        Some(client),
                        &format!("Not a ticket ID or Linear issue URL: {bad}"),
                    );
                    return;
                }
            };
            let tabs: Vec<TabId> = tickets
                .iter()
                .filter_map(|t| {
                    let command = worktree::tab_running(
                        "worktree-from-ticket",
                        "wt",
                        std::slice::from_ref(t),
                    );
                    server.work_tab(client, &repo, command, Opened::Background)
                })
                .collect();
            server.front(client, &tabs);
        });
    }

    /// M-B, once the repository is picked: its branches, the most recently
    /// committed first; a name no branch matches is a new branch, as is the
    /// name typed with Alt-Enter.
    fn ask_branch(&mut self, client: ClientId, repo: PathBuf) {
        let at = repo.clone();
        self.off_task(
            client,
            move || branches(&at),
            move |server, client, found| {
                let items = found
                    .iter()
                    .map(|(name, when)| PickItem {
                        text: name.clone(),
                        tags: when.clone(),
                        value: name.clone(),
                        ..PickItem::default()
                    })
                    .collect();
                let picker =
                    Picker::new("Work on a branch: a name no branch matches is made", items)
                        .named("branches")
                        .prompt("branch> ")
                        .exact()
                        .text_only()
                        .free();
                server.pick(client, picker, move |server, client, picked| {
                    let Some(picked) = picked else {
                        return;
                    };
                    let name: String = picked
                        .value
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .collect();
                    if !branch_name_ok(&name) {
                        server.say(
                            Some(client),
                            &format!("Not a branch name git accepts: {name}"),
                        );
                        return;
                    }
                    let command = worktree::tab_running("worktree-from-branch", "wtb", &[name]);
                    if let Some(tab) = server.work_tab(client, &repo, command, Opened::Front) {
                        server.front(client, &[tab]);
                    }
                });
            },
        );
    }

    /// The palette's "Start work on a PR": its number or URL, the
    /// clipboard's when it holds one. A URL names its own repository, which
    /// wtpr finds a clone of, so the tab starts where the key was pressed; a
    /// bare number then asks which repository it is a PR of.
    fn ask_pr(&mut self, client: ClientId, found: Found) {
        let prefill = if parse_pr(&found.clipboard).is_some() {
            found.clipboard.trim().to_string()
        } else {
            String::new()
        };
        let picker = Picker::new("A PR number or URL, to work on it in a new tab", Vec::new())
            .named("pr")
            .prompt("PR: ")
            .free()
            .query(&prefill);
        self.pick(client, picker, move |server, client, picked| {
            let Some(picked) = picked else {
                return;
            };
            match parse_pr(&picked.value) {
                Some(PrAsked::Url(url, _)) => {
                    let command = worktree::tab_running("worktree-from-pr", "wtpr", &[url]);
                    let here = found.here.clone();
                    if let Some(tab) = server.work_tab(client, &here, command, Opened::Front) {
                        server.front(client, &[tab]);
                    }
                }
                Some(PrAsked::Number(number)) => server.ask_dir(
                    client,
                    &format!("PR #{number} of which repository"),
                    found,
                    Wanted::Repo,
                    move |s, c, dir, _| {
                        let command = worktree::tab_running(
                            "worktree-from-pr",
                            "wtpr",
                            &[number.to_string()],
                        );
                        if let Some(tab) = s.work_tab(c, &dir, command, Opened::Front) {
                            s.front(c, &[tab]);
                        }
                    },
                ),
                None => server.say(
                    Some(client),
                    &format!("Not a PR number or GitHub PR URL: {}", picked.value.trim()),
                ),
            }
        });
    }

    /// The palette's "Review a contributor's PR", pressed in `pane`: the
    /// open PRs from forks of the repository the pane is in, the most
    /// recently updated first (a number typed taken too), then the PR's
    /// review tab (`review_tab`). Never in the PR's own worktree.
    pub(crate) fn review_pr(&mut self, client: ClientId, pane: Option<PaneId>) {
        let here = pane.and_then(|p| self.pane_dir(p));
        let Some(repo) = here.as_deref().and_then(review_repo) else {
            self.say(
                Some(client),
                "Review a contributor's PR works in a repository with a review-pr skill of its own (.claude/skills/review-pr)",
            );
            return;
        };
        let at = repo.clone();
        self.off_task(
            client,
            move || {
                tokio::runtime::Handle::current().block_on(async {
                    let repo = github::repo_of(&at).await?;
                    github::open_fork_prs(&repo, "M-p review a PR").await
                })
            },
            move |server, client, found| {
                let prs = match found {
                    Ok(prs) => prs,
                    Err(e) => {
                        server.say(Some(client), &format!("Couldn't list the open PRs: {e}"));
                        return;
                    }
                };
                // Its author wrote the title, which may hold what a
                // terminal acts on: shown without control characters.
                let items = prs
                    .iter()
                    .map(|p| PickItem {
                        text: format!(
                            "#{} {}",
                            p.pr.number,
                            p.pr.title
                                .chars()
                                .filter(|c| !c.is_control())
                                .collect::<String>()
                        ),
                        tags: format!("@{}", p.author),
                        value: p.pr.number.to_string(),
                        ..PickItem::default()
                    })
                    .collect();
                let title = if prs.is_empty() {
                    "No open PRs from forks: a PR number to review"
                } else {
                    "Review a contributor's PR"
                };
                let picker = Picker::new(title, items)
                    .named("review-pr")
                    .prompt("PR: ")
                    .free();
                server.pick(client, picker, move |server, client, picked| {
                    let Some(picked) = picked else {
                        return;
                    };
                    let Some(PrAsked::Number(number)) = parse_pr(&picked.value) else {
                        server.say(
                            Some(client),
                            &format!("Not a PR number: {}", picked.value.trim()),
                        );
                        return;
                    };
                    server.review_tab(client, repo, number);
                });
            },
        );
    }

    /// The review tab of PR `number`, once it is picked: the PR checked out
    /// untrusted first, off the server's task, then a tab running Claude
    /// with `/review-pr <number>` in the main clone `repo`, nothing of the
    /// worktree's loaded, while the tab is about the worktree (`TAB_DIR`):
    /// named, colored and given its PR from it, and M-v, M-m and M-d there
    /// show the PR's.
    fn review_tab(&mut self, client: ClientId, repo: PathBuf, number: u64) {
        self.say(
            Some(client),
            &format!("Checking out PR #{number} to review…"),
        );
        let root = repo.clone();
        self.off_task(
            client,
            move || {
                let handle = tokio::runtime::Handle::current();
                crate::cmd::worktree_from_pr::review_worktree(&handle, &root, number)
            },
            move |server, client, checked| {
                let worktree = match checked {
                    Ok(worktree) => worktree,
                    Err(e) => {
                        server.say(
                            Some(client),
                            &format!("Couldn't check out PR #{number}: {e}"),
                        );
                        return;
                    }
                };
                let command = worktree::tab_command(
                    "claude \"$@\"",
                    "claude",
                    &[format!("/review-pr {number}")],
                );
                if let Some(tab) = server.work_tab(client, &repo, command, Opened::Front)
                    && let Some(t) = server.tabs.get_mut(&tab)
                {
                    t.opts.insert("@tab_name".into(), "Review PR".to_string());
                    t.opts.insert(
                        crate::mux::server::TAB_DIR.into(),
                        worktree.to_string_lossy().into_owned(),
                    );
                    server.dirty_tab(tab);
                    server.name_tab(tab);
                }
            },
        );
    }

    /// The palette's "Start work on a note's checklist items", once the
    /// repository is picked: which note, the most recently changed first,
    /// then which of its unticked items no worktree has yet, each marked
    /// when its plan still needs a choice. fzf took several with Tab; here
    /// the first row takes every item listed, and any other row one.
    fn ask_note(&mut self, client: ClientId, repo: PathBuf) {
        let notes = worktree_from_note::notes_dir();
        if !notes.is_dir() {
            self.say(
                Some(client),
                &format!("No notes directory at {}.", paths::tilde(&notes)),
            );
            return;
        }
        let dir = notes.clone();
        self.off_task(
            client,
            move || worktree_from_note::notes_newest_first(&dir),
            move |server, client, names| {
                if names.is_empty() {
                    server.say(
                        Some(client),
                        &format!("No notes in {}.", paths::tilde(&notes)),
                    );
                    return;
                }
                let items = names
                    .iter()
                    .map(|n| PickItem::new(n.clone(), n.clone()))
                    .collect();
                let picker = Picker::new("Which note", items)
                    .named("notes")
                    .prompt("note> ");
                server.pick(client, picker, move |server, client, picked| {
                    let Some(picked) = picked else {
                        return;
                    };
                    server.ask_items(client, repo, notes, format!("{}.md", picked.value));
                });
            },
        );
    }

    fn ask_items(&mut self, client: ClientId, repo: PathBuf, notes: PathBuf, note: String) {
        let file = notes.join(&note);
        let (at, name) = (repo.clone(), note.clone());
        self.off_task(
            client,
            move || {
                let text = std::fs::read_to_string(&file).unwrap_or_default();
                let all = worktree_from_note::items(&text);
                let items = worktree_from_note::unclaimed(&at, &name, &text);
                // Each item whole, for the list's lower half, with the
                // wtn_item_preview setting (wtn-open.zsh's `item`).
                let wholes: Vec<String> = if settings::is_on("wtn_item_preview") {
                    items
                        .iter()
                        .map(|i| worktree_from_note::item_text(&text, i.line))
                        .collect()
                } else {
                    Vec::new()
                };
                (all.is_empty(), items, wholes, note_key(&text, &name))
            },
            move |server, client, (none, items, wholes, key)| {
                let title = note.strip_suffix(".md").unwrap_or(&note).to_string();
                if none {
                    server.say(Some(client), &format!("{title} has no unticked items."));
                    return;
                }
                if items.is_empty() {
                    server.say(
                        Some(client),
                        &format!("Every unticked item of {title} has a worktree already."),
                    );
                    return;
                }
                let mut rows = Vec::new();
                if items.len() > 1 {
                    rows.push(PickItem::new(
                        format!("Every item listed ({})", items.len()),
                        "all",
                    ));
                }
                // The item's text alone shown and matched, as wtn-open.zsh's
                // `--with-nth=2`; its line is what picking it gives.
                rows.extend(items.iter().map(|i| {
                    PickItem {
                        text: i.text.clone(),
                        tags: [
                            i.needs_choice.then(|| "(needs a choice)".to_string()),
                            i.waits_on.as_ref().map(|w| {
                                format!(
                                    "(waits on {})",
                                    crate::ui::draw::cut(w, worktree_from_note::WAITS_ON_SHOWN)
                                )
                            }),
                        ]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" "),
                        key: String::new(),
                        value: i.line.to_string(),
                    }
                }));
                let short = Path::new(&title)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let previews = if wholes.is_empty() {
                    Vec::new()
                } else if items.len() > 1 {
                    let every = items
                        .iter()
                        .map(|i| format!("- {}", i.text))
                        .collect::<Vec<_>>()
                        .join("\n");
                    std::iter::once(every).chain(wholes).collect()
                } else {
                    wholes
                };
                // Tab marks several, as wtn-open.zsh's fzf --multi did.
                let picker = Picker::new(format!("Work on {short}'s items"), rows)
                    .named("items")
                    .prompt(format!("{short}> "))
                    .previews(previews)
                    .multi();
                server.pick_many(client, picker, move |server, client, picked| {
                    let values: Vec<&str> = picked.iter().map(|p| p.value.as_str()).collect();
                    let chosen: Vec<&worktree_from_note::Item> = if values.contains(&"all") {
                        items.iter().collect()
                    } else {
                        items
                            .iter()
                            .filter(|i| values.contains(&i.line.to_string().as_str()))
                            .collect()
                    };
                    let chosen: Vec<worktree_from_note::Item> =
                        chosen.into_iter().cloned().collect();
                    if chosen.is_empty() {
                        return;
                    }
                    // "Land on:", as autopilot's setup asks it, the default
                    // branch first so Enter lands there as before.
                    let at = repo.clone();
                    server.autopilot_pick_target(client, at, move |server, client, target| {
                        keep_note_repo(key, &repo);
                        let tabs: Vec<TabId> = chosen
                            .iter()
                            .filter_map(|item| {
                                server.open_item_tab(
                                    client,
                                    &repo,
                                    &note,
                                    item,
                                    &target,
                                    Opened::Background,
                                )
                            })
                            .collect();
                        server.front(client, &tabs);
                    });
                });
            },
        );
    }

    /// Notes' `gw` (`press work-on-item <note> <line>`): the unticked
    /// top-level item on that line of the note, `note` its path within the
    /// notes folder. A tab already linked to it is offered instead; else
    /// which repository, the one this note's items were last started in
    /// first, then "Land on:", and a tab working on it, as M-I's picker opens
    /// one, brought to the front.
    pub fn work_on_item(&mut self, client: ClientId, note: String, line: usize) {
        let here = self
            .current_pane(client)
            .and_then(|p| self.pane_own_dir(p))
            .unwrap_or_else(paths::home);
        let panes = self.pane_dirs();
        let file = worktree_from_note::notes_dir().join(&note);
        let name = note.clone();
        // The tabs linked to the note, never Notes' own, with what names
        // their item: `@tab_title` (M-I's and these tabs), `@autopilot_item`,
        // else their directory's branch, read off the task (a tab /wtn
        // opened sets neither option).
        let linked: Vec<(TabId, Option<String>, Option<PathBuf>)> = self
            .sessions
            .iter()
            .flat_map(|s| s.tabs.iter().copied())
            .filter(|t| {
                self.tab_opt(*t, "@tab_note").as_deref() == Some(note.as_str())
                    && self.tab_opt(*t, "@notes").as_deref() != Some("1")
            })
            .map(|t| {
                let named = self
                    .tab_opt(t, "@tab_title")
                    .or_else(|| self.tab_opt(t, "@autopilot_item"))
                    .filter(|v| !v.is_empty());
                (t, named, self.tab_path(t).map(Path::to_path_buf))
            })
            .collect();
        self.off_task(
            client,
            move || {
                let text = std::fs::read_to_string(&file).ok()?;
                let key = note_key(&text, &name);
                let tabs: Vec<(TabId, String)> = linked
                    .into_iter()
                    .filter_map(|(tab, named, dir)| {
                        let item = named
                            .or_else(|| dir.and_then(|d| crate::tabs::title::wtn_item_at(&d)))?;
                        Some((tab, item.trim().to_string()))
                    })
                    .collect();
                let mut recent = work_recent::dirs();
                // A note started before it had an ID was kept by its path,
                // which wtn then gave one.
                let last = work_recent::note_repo(&key).or_else(|| work_recent::note_repo(&name));
                recent.splice(0..0, last);
                let found = Found {
                    dirs: offered(&here, &panes, &recent, &recent_session_dirs(), Wanted::Repo),
                    here,
                    clipboard: String::new(),
                };
                Some((text, key, found, tabs))
            },
            move |server, client, read| {
                let title = note.strip_suffix(".md").unwrap_or(&note).to_string();
                let Some((text, key, found, tabs)) = read else {
                    server.say(Some(client), &format!("No note {title}."));
                    return;
                };
                let item = match worktree_from_note::item_at(&text, line) {
                    Ok(item) => item,
                    Err(why) => {
                        server.say(Some(client), &format!("Line {line} of {title} {why}."));
                        return;
                    }
                };
                let short = crate::tabs::title::short_name(&item.text);
                let linked = tabs
                    .iter()
                    .find(|(t, named)| *named == item.text && server.tabs.contains_key(t))
                    .map(|(t, _)| *t);
                if let Some(tab) = linked {
                    let question = Confirm::new(format!("“{short}” has a tab already. Go to it?"));
                    server.ask(client, question, move |server, client, yes| {
                        if yes {
                            server.go_to_tab(client, tab);
                        }
                    });
                    return;
                }
                let ask = format!("Work on “{short}”: the repository");
                server.ask_dir(
                    client,
                    &ask,
                    found,
                    Wanted::Repo,
                    move |server, client, repo, _| {
                        let (at, name) = (repo.clone(), note.clone());
                        server.off_task(
                            client,
                            move || {
                                worktree_from_note::unclaimed(&at, &name, &text)
                                    .iter()
                                    .any(|i| i.line == line)
                            },
                            move |server, client, free| {
                                if !free {
                                    server.say(
                                        Some(client),
                                        &format!(
                                            "“{short}” has a worktree in {} already.",
                                            paths::tilde(&repo)
                                        ),
                                    );
                                    return;
                                }
                                let at = repo.clone();
                                server.autopilot_pick_target(
                                    client,
                                    at,
                                    move |server, client, target| {
                                        keep_note_repo(key, &repo);
                                        if let Some(tab) = server.open_item_tab(
                                            client,
                                            &repo,
                                            &note,
                                            &item,
                                            &target,
                                            Opened::Front,
                                        ) {
                                            server.front(client, &[tab]);
                                        }
                                    },
                                );
                            },
                        );
                    },
                );
            },
        );
    }

    /// Which repository a note's work goes in, asked as `gw` asks it: the
    /// one this note's items were last started in first, then the
    /// directories of the panes and of recent work; `then` given the one
    /// picked, which is kept as the note's.
    fn ask_note_repo(
        &mut self,
        client: ClientId,
        note: String,
        ask: String,
        then: impl FnOnce(&mut Server, ClientId, PathBuf) + Send + 'static,
    ) {
        let here = self
            .current_pane(client)
            .and_then(|p| self.pane_own_dir(p))
            .unwrap_or_else(paths::home);
        let panes = self.pane_dirs();
        let file = worktree_from_note::notes_dir().join(&note);
        self.off_task(
            client,
            move || {
                let text = std::fs::read_to_string(&file).ok()?;
                let key = note_key(&text, &note);
                let mut recent = work_recent::dirs();
                let last = work_recent::note_repo(&key).or_else(|| work_recent::note_repo(&note));
                recent.splice(0..0, last);
                let found = Found {
                    dirs: offered(&here, &panes, &recent, &recent_session_dirs(), Wanted::Repo),
                    here,
                    clipboard: String::new(),
                };
                Some((key, found))
            },
            move |server, client, read| {
                let Some((key, found)) = read else {
                    server.say(Some(client), "No such note.");
                    return;
                };
                server.ask_dir(
                    client,
                    &ask,
                    found,
                    Wanted::Repo,
                    move |server, client, repo, _| {
                        keep_note_repo(key, &repo);
                        then(server, client, repo);
                    },
                );
            },
        );
    }

    /// Notes' M-Enter in "Add an item" (`press plan-item <note> <text>`):
    /// a tab in the note's repository, linked to the note and titled with
    /// the item's words, whose Claude
    /// runs `/plan-item <text>`, which adds the item to the note and writes
    /// its plan; the text its argument, never typed.
    pub fn plan_new_item(&mut self, client: ClientId, note: String, text: String) {
        let short = crate::tabs::title::short_name(&text);
        let ask = format!("Plan “{short}”: the repository");
        let name = note.clone();
        self.ask_note_repo(client, note, ask, move |server, client, repo| {
            if crate::trust::in_review_dir(&repo) {
                let refusal = crate::trust::refusal(&repo);
                server.say(Some(client), &refusal);
                return;
            }
            let command =
                worktree::tab_command("claude \"$@\"", "claude", &[format!("/plan-item {text}")]);
            let Some(tab) = server.work_tab(client, &repo, command, Opened::Front) else {
                return;
            };
            if let Some(t) = server.tabs.get_mut(&tab) {
                t.opts.insert("@tab_name".into(), format!("Plan: {short}"));
                // Its full title the item's words, by which Notes' o and
                // chips find the tab on the item once /plan-item adds it.
                t.opts.insert("@tab_title".into(), text.clone());
                t.opts.insert("@tab_note".into(), name);
            }
            server.dirty_tab(tab);
            server.front(client, &[tab]);
        });
    }

    /// M-p's "Autopilot: work “<note>”", listed while Notes shows the note
    /// in front: autopilot working that note. Already working it, its menu
    /// in mission control; working another, asked whether to switch to this
    /// one, its counts, spend limit and landing kept; off, in the
    /// repository its items are worked in, asked first.
    pub fn work_note(&mut self, client: ClientId, note: String) {
        if !settings::is_on("autopilot") {
            self.say(Some(client), "Autopilot is off in Settings (Autopilot)");
            return;
        }
        let mode = crate::autopilot::Mode::read(&paths::state_dir());
        let named = |note: &str| format!("“{}”", crate::autopilot::mode::note_title(note));
        if mode.on && mode.note == note {
            self.open_mission(client, crate::autopilot::opening());
            self.mission_note(
                client,
                format!("Autopilot is already working {}", named(&note)),
            );
            self.autopilot_menu(client);
            return;
        }
        if mode.on {
            let question = Confirm::new(format!(
                "Autopilot is working {}. Switch it to {}?",
                named(&mode.note),
                named(&note)
            ));
            self.ask(client, question, move |server, client, yes| {
                if yes {
                    server.autopilot_switch_note(client, note);
                }
            });
            return;
        }
        let title = note.strip_suffix(".md").unwrap_or(&note).to_string();
        let ask = format!("Work “{title}”: the repository");
        let name = note.clone();
        self.ask_note_repo(client, note, ask, move |server, client, repo| {
            server.autopilot_work_note(client, &repo, name);
        });
    }

    /// A tab for one of a note's items, in front or behind as `opened` says
    /// (autopilot's behind, one you start in front), in the note's
    /// repository: named after the item, cut short, its whole first line
    /// its full title, and linked to its note (`@tab_name`, `@tab_title`,
    /// `@tab_note`), running `wtn --item <line>` with the
    /// note's path in WTN_NOTE, so only the line number is a word of the
    /// command. What autopilot opens for each item it
    /// takes too (wtn-open.zsh's `open_tab`), with `--target` naming the
    /// branch its worktree is cut from and lands on (autopilot's target);
    /// empty for the default branch.
    pub fn open_item_tab(
        &mut self,
        client: ClientId,
        repo: &Path,
        note: &str,
        item: &worktree_from_note::Item,
        target: &str,
        opened: Opened,
    ) -> Option<TabId> {
        let mut command = vec!["env".to_string(), format!("WTN_NOTE={note}")];
        command.extend(worktree::tab_running(
            "worktree-from-note",
            "wtn",
            &item_args(item.line, target),
        ));
        let tab = self.work_tab(client, repo, command, opened)?;
        // Named with the item cut short, for the tab bar and the second
        // status line, its whole first line kept as the tab's full title,
        // which M-T, the sidebar and mission control show.
        if let Some(t) = self.tabs.get_mut(&tab) {
            t.opts.insert(
                "@tab_name".into(),
                crate::tabs::title::short_name(&item.text),
            );
            t.opts.insert("@tab_title".into(), item.text.clone());
            t.opts.insert("@tab_note".into(), note.to_string());
        }
        self.dirty_tab(tab);
        Some(tab)
    }

    /// The palette's "Continue shaping Linear tickets": which tickets, the
    /// clipboard's when it holds only tickets, then which directory to shape
    /// them in, which must be in a repository so `shape` has code to read; a
    /// tab each running `shape --continue <ID>`.
    fn ask_shaping(&mut self, client: ClientId, found: Found) {
        let picker = Picker::new(
            "Linear ticket IDs or URLs, to carry on shaping each in a new tab",
            Vec::new(),
        )
        .named("tickets")
        .prompt("Tickets: ")
        .free()
        .query(&tickets_on(&found.clipboard));
        self.pick(client, picker, move |server, client, picked| {
            let Some(picked) = picked else {
                return;
            };
            let tickets = match parse_tickets(&picked.value) {
                Ok(t) if !t.is_empty() => t,
                Ok(_) => return,
                Err(bad) => {
                    server.say(
                        Some(client),
                        &format!("Not a ticket ID or Linear issue URL: {bad}"),
                    );
                    return;
                }
            };
            let title = format!("Shape {} ticket(s) here", tickets.len());
            server.ask_dir(client, &title, found, Wanted::Any, move |s, c, dir, _| {
                if main_clone(&dir).is_none() {
                    s.say(
                        Some(c),
                        &format!(
                            "{} is not in a repository, so shape has no code to read. Pick the repo the tickets belong to.",
                            paths::tilde(&dir)
                        ),
                    );
                    return;
                }
                let script = format!("{} --continue \"$@\"", worktree::self_command("shape"));
                let tabs: Vec<TabId> = tickets
                    .iter()
                    .filter_map(|t| {
                        let command = worktree::tab_command(&script, "shape", std::slice::from_ref(t));
                        s.work_tab(c, &dir, command, Opened::Background)
                    })
                    .collect();
                s.front(c, &tabs);
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_items_tab_names_its_target_only_when_one_was_chosen() {
        assert_eq!(item_args(12, ""), ["--item", "12"]);
        assert_eq!(
            item_args(12, "team/next"),
            ["--item", "12", "--target", "team/next"]
        );
    }

    #[test]
    fn tickets_are_ids_or_urls_each_once_in_capitals() {
        assert_eq!(
            parse_tickets("abc-1, https://linear.app/x/issue/ABC-2/t ABC-1"),
            Ok(vec!["ABC-1".to_string(), "ABC-2".to_string()])
        );
        assert_eq!(parse_tickets("ABC-1 $(rm)"), Err("$(rm)".to_string()));
        assert_eq!(tickets_on("not a ticket"), "");
        assert_eq!(tickets_on(" ENG-4 "), "ENG-4");
    }

    #[test]
    fn prs_by_number_or_a_url_rebuilt_from_its_parts() {
        assert_eq!(parse_pr("#12"), Some(PrAsked::Number(12)));
        assert_eq!(
            parse_pr("https://github.com/o/r/pull/7/files?x=$(y)"),
            Some(PrAsked::Url("https://github.com/o/r/pull/7".into(), 7))
        );
        assert_eq!(parse_pr("https://example.com/o/r/pull/7"), None);
    }

    #[test]
    fn branch_names_git_would_take_and_the_shell_reads_plainly() {
        for ok in ["fix-login", "feature/login", "v1.2"] {
            assert!(branch_name_ok(ok), "{ok}");
        }
        for bad in ["-x", "a..b", "a b", "x.lock", "a/.b", "x/", "$(y)", "a//b"] {
            assert!(!branch_name_ok(bad), "{bad}");
        }
    }

    #[test]
    fn the_main_clone_is_read_from_the_files_under_git() {
        let dir = std::env::temp_dir().join(format!("sb-main-clone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let main = dir.join("main");
        std::fs::create_dir_all(main.join(".git/worktrees/w")).unwrap();
        std::fs::write(main.join(".git/worktrees/w/commondir"), "../..\n").unwrap();
        let linked = main.join(".claude/worktrees/w");
        std::fs::create_dir_all(linked.join("src")).unwrap();
        std::fs::write(
            linked.join(".git"),
            format!("gitdir: {}\n", main.join(".git/worktrees/w").display()),
        )
        .unwrap();
        let main_real = main.canonicalize().unwrap();
        assert_eq!(main_clone(&linked.join("src")), Some(main_real.clone()));
        assert_eq!(main_clone(&main), Some(main_real));
        assert_eq!(main_clone(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reviewing_a_pr_is_offered_where_the_main_clone_has_the_skill() {
        let dir = std::env::temp_dir().join(format!("sb-review-repo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let main = dir.join("main");
        std::fs::create_dir_all(main.join(".git/worktrees/w")).unwrap();
        std::fs::write(main.join(".git/worktrees/w/commondir"), "../..\n").unwrap();
        let linked = main.join(".claude/worktrees/w");
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::write(
            linked.join(".git"),
            format!("gitdir: {}\n", main.join(".git/worktrees/w").display()),
        )
        .unwrap();
        assert_eq!(review_repo(&main), None, "no skill, not listed");
        // A worktree's own copy doesn't count: the main clone's does.
        std::fs::create_dir_all(linked.join(".claude/skills/review-pr")).unwrap();
        std::fs::write(linked.join(".claude/skills/review-pr/SKILL.md"), "x").unwrap();
        assert_eq!(review_repo(&linked), None);
        std::fs::create_dir_all(main.join(".claude/skills/review-pr")).unwrap();
        std::fs::write(main.join(".claude/skills/review-pr/SKILL.md"), "x").unwrap();
        let main_real = main.canonicalize().unwrap();
        assert_eq!(review_repo(&main), Some(main_real.clone()));
        assert_eq!(review_repo(&linked), Some(main_real), "a worktree counts");
        assert_eq!(review_repo(&dir), None, "outside a repository");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn directories_picked_before_lead_the_list_else_the_one_you_are_in() {
        let dir = std::env::temp_dir().join(format!("sb-offered-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let [here, tab, picked, older, gone] =
            ["here", "tab", "picked", "older", "gone"].map(|n| dir.join(n));
        for d in [&here, &tab, &picked, &older] {
            std::fs::create_dir_all(d).unwrap();
        }
        let panes = [tab.clone()];
        let none = offered(&here, &panes, &[], &[], Wanted::Any);
        assert_eq!(none[..2], [here.clone(), tab.clone()]);
        let recent = [picked.clone(), gone, older.clone(), tab.clone()];
        let with = offered(&here, &panes, &recent, &[], Wanted::Any);
        assert_eq!(with[..4], [picked, older, tab, here]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
