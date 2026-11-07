//! The server's side of the views: what M-v, M-i, M-r, M-l, M-m and M-S-f
//! do, and the palette's link commands, as tmux.conf bound them to
//! pr-view.sh, ticket-view.sh, gh-open.sh, linear-ticket.sh, pr-ready.sh,
//! search.sh and link-view.sh.
//!
//! The link commands list what the pane shows (`shown`): read a link
//! beside it, open one in the browser, copy one, or open an image or video
//! in its app, each list's Ctrl-O doing the other thing link-view.sh's
//! `pick` and `media` offered. In the notes pane, the links are the note's,
//! since Notes shows a link's text alone.
//!
//! A view opens in a pane split off to the right of the pane you are in
//! (`open_beside`), running one of Switchboard's own programs, and is
//! marked with a pane option (`@pr_view`, `@ticket_view`, `@link_view`
//! with the link in `@link_url`, `@checks_view`), so the key that opened it
//! finds it in the tab and closes it, and the link view gives way to the
//! next link read. Anything that waits on GitHub, Linear or Claude runs off
//! the server's task and reports back with a `ViewsEvent`; what can't be
//! done is said in a box (`say`), as gh-open.sh's box said why, since a key
//! that does nothing reads as broken. A key of a service turned off in
//! Settings shows its tip (`github_off`, `linear_off`) the first time, as
//! settings.sh's binding did, and nothing after (`Server::act` checks
//! every action's tags first).

use std::path::PathBuf;

use serde_json::Value;

use super::ready::{self, Choice, ReadyBox};
use super::search::{self, Found, Running, Source};
use super::shown::{self, Shown};
use super::{Command, ViewsEvent, page, ticket};
use crate::integrations::{github, linear};
use crate::mux::server::{ClientId, Event, Server};
use crate::mux::{Direction, PaneId, TabId};
use crate::net::{self, ErrorKind};
use crate::ui::{Outcome, Overlay, PickItem, Picker};
use crate::{links, paths, settings, util};

/// The options marking the panes the views open.
pub const PR_VIEW: &str = "@pr_view";
pub const TICKET_VIEW: &str = "@ticket_view";
pub const LINK_VIEW: &str = "@link_view";
pub const LINK_URL: &str = "@link_url";
pub const CHECKS_VIEW: &str = "@checks_view";

/// The share of the pane a view takes, as tmux.conf's `split-window -l`.
const VIEW_PERCENT: u16 = 45;
const LINK_PERCENT: u16 = 50;

/// The program to run in a view's pane: this binary.
fn exe() -> String {
    crate::util::exe()
        .ok()
        .filter(|p| p.exists())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "switchboard".into())
}

/// Linear's workspace for a ticket's link: `LINEAR_WORKSPACE`, else
/// linear.json's `linearWorkspace`, as linear-ticket.sh read it.
fn linear_workspace() -> Option<String> {
    std::env::var("LINEAR_WORKSPACE")
        .ok()
        .filter(|w| !w.trim().is_empty())
        .or_else(|| {
            util::read_json_object(paths::config_dir().join("linear.json"))
                .get("linearWorkspace")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .map(|w| w.trim().to_string())
        .filter(|w| {
            !w.is_empty()
                && w.chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        })
}

fn is_readable(url: &str) -> bool {
    shown::safe(url) && (url.starts_with("file://") || links::host_of(url).is_some())
}

/// The rows of a list of what a pane shows, each giving its address.
fn rows(found: &[Shown]) -> Vec<PickItem> {
    found
        .iter()
        .map(|f| PickItem::new(f.row(), f.url.clone()))
        .collect()
}

fn search_title(query: &str, waiting: &[Source]) -> String {
    let mut title = format!("Search: \"{query}\"");
    if !waiting.is_empty() {
        let names: Vec<&str> = waiting.iter().map(|s| s.name()).collect();
        title.push_str(&format!(" · {} still looking", names.join(", ")));
    }
    title
}

fn item(found: &Found) -> PickItem {
    PickItem {
        text: found.text.clone(),
        tags: found.detail.clone(),
        key: found.source.name().to_string(),
        value: found.link.clone(),
    }
}

impl Server {
    /// A view's command, from its key or the palette.
    pub fn view_command(&mut self, client: ClientId, command: Command) {
        for (tag, service) in [("github", "github"), ("linear", "linear")] {
            if command.tags().contains(&tag) && !settings::is_on(service) {
                self.service_off(client, service);
                return;
            }
        }
        match command {
            Command::PrView => self.toggle_pr_view(client),
            Command::TicketView => self.toggle_ticket_view(client),
            Command::OpenPr => self.open_on_github(client, false),
            Command::OpenRepo => self.open_on_github(client, true),
            Command::OpenTicket => self.open_ticket(client),
            Command::PrReady => self.pr_ready(client),
            Command::Search => self.search_everything(client),
            Command::ReadLink => self.ask_link(client),
            Command::OpenLinkRead => self.open_link_read(client),
            Command::OpenLink => self.pick_link(client, false),
            Command::CopyLink => self.pick_link(client, true),
            Command::OpenMedia => self.open_media(client),
            Command::OpenMergeQueue => self.open_merge_queue(client),
            Command::OpenProject => self.open_project(client),
            Command::CopyTicketId => self.copy_ticket(client, false),
            Command::CopyTicketUrl => self.copy_ticket(client, true),
            Command::CopyPrNumber => self.copy_pr(client, false),
            Command::CopyPrUrl => self.copy_pr(client, true),
        }
    }

    /// What the views' work off the task found.
    pub fn views_event(&mut self, event: ViewsEvent) {
        match event {
            ViewsEvent::Say { client, text } => self.say(Some(client), &text),
            ViewsEvent::Copy { client, text } => self.copy_said(client, &text),
            ViewsEvent::Ready { client, answer } => {
                if let Some(c) = self.clients.get_mut(&client)
                    && let Some(b) = c.overlays.find_mut::<ReadyBox>()
                {
                    b.answered(answer);
                    c.dirty = true;
                }
                // As pr-ready.sh did: mission control's PR views ask GitHub
                // again too, so they agree with this answer.
                util::append_quietly(paths::state_dir().join("github"), "\n");
            }
            ViewsEvent::Found {
                client,
                search,
                source,
                rows,
            } => self.search_found(client, search, source, rows),
        }
    }

    /// A key of a service turned off in Settings: its tip the first time,
    /// and nothing after, as the bindings settings.sh gave such keys did.
    pub(crate) fn service_off(&mut self, client: ClientId, service: &str) {
        self.tip(&format!("{service}_off"), Some(client));
    }

    /// The pane a key acts on, its tab, and its directory.
    fn here(&self, client: ClientId) -> Option<(PaneId, TabId, PathBuf)> {
        let pane = self.current_pane(client)?;
        let slot = self.panes.get(&pane)?;
        let dir = slot.pane.cwd().unwrap_or_else(paths::home);
        Some((pane, slot.tab, dir))
    }

    /// The pane in a tab marked with `mark`.
    pub fn marked_in(&self, tab: TabId, mark: &str) -> Option<PaneId> {
        self.tabs.get(&tab)?.layout.panes().into_iter().find(|p| {
            self.panes
                .get(p)
                .is_some_and(|s| s.opts.get(mark).is_some_and(|v| v == "1"))
        })
    }

    /// Runs `switchboard <args>` in a pane to the right of `pane`, taking
    /// `percent` of its width, focused and marked with `mark`, as the views'
    /// `split-window -h` did. Gives the new pane, or says why there is none.
    pub fn open_beside(
        &mut self,
        pane: PaneId,
        mark: &str,
        args: Vec<String>,
        percent: u16,
    ) -> Option<PaneId> {
        let mut command = vec![exe()];
        command.extend(args);
        match self.split(
            pane,
            Direction::Right,
            None,
            Some(command),
            Some(percent),
            false,
        ) {
            Ok(view) => {
                if let Some(slot) = self.panes.get_mut(&view) {
                    slot.opts.insert(mark.to_string(), "1".into());
                }
                Some(view)
            }
            Err(e) => {
                self.say(None, &format!("Couldn't open a pane beside this one: {e}"));
                None
            }
        }
    }

    /// Reads a link in the pane beside `pane`, in place of a link read
    /// there before (link-view.sh's `open`).
    pub fn read_beside(&mut self, pane: PaneId, url: &str) {
        let Some(tab) = self.panes.get(&pane).map(|s| s.tab) else {
            return;
        };
        let mut target = pane;
        if let Some(old) = self.marked_in(tab, LINK_VIEW) {
            self.kill_pane(old);
            if old == pane {
                match self.tabs.get(&tab) {
                    Some(t) => target = t.active,
                    None => return,
                }
            }
        }
        if let Some(view) = self.open_beside(
            target,
            LINK_VIEW,
            vec!["link-view".into(), url.into()],
            LINK_PERCENT,
        ) && let Some(slot) = self.panes.get_mut(&view)
        {
            slot.opts.insert(LINK_URL.into(), url.to_string());
        }
    }

    /// M-v: the PR view to the right, or closed.
    fn toggle_pr_view(&mut self, client: ClientId) {
        let Some((pane, tab, dir)) = self.here(client) else {
            return;
        };
        if let Some(open) = self.marked_in(tab, PR_VIEW) {
            self.kill_pane(open);
            return;
        }
        let dir = dir.to_string_lossy().into_owned();
        self.open_beside(pane, PR_VIEW, vec!["pr-view".into(), dir], VIEW_PERCENT);
    }

    /// The ticket a tab's work is on: its branch's, else its name's.
    pub(crate) fn ticket_here(&self, tab: TabId, dir: &std::path::Path) -> Option<String> {
        ticket::id_for_dir(dir).or_else(|| {
            let t = self.tabs.get(&tab)?;
            t.opts
                .get("@tab_name")
                .and_then(|n| ticket::id_in_name(n))
                .or_else(|| ticket::id_in_name(&t.name))
                .or_else(|| ticket::id_in_name(&self.tab_label(tab)))
        })
    }

    /// M-i: the ticket view to the right, or closed.
    fn toggle_ticket_view(&mut self, client: ClientId) {
        let Some((pane, tab, dir)) = self.here(client) else {
            return;
        };
        if let Some(open) = self.marked_in(tab, TICKET_VIEW) {
            self.kill_pane(open);
            return;
        }
        let Some(id) = self.ticket_here(tab, &dir) else {
            self.say(Some(client), "No Linear ticket in this branch or tab name");
            return;
        };
        self.open_beside(
            pane,
            TICKET_VIEW,
            vec!["ticket-view".into(), id],
            VIEW_PERCENT,
        );
    }

    /// Says something on `client` from off the task.
    fn say_later(tx: &tokio::sync::mpsc::UnboundedSender<Event>, client: ClientId, text: String) {
        let _ = tx.send(Event::Views(ViewsEvent::Say { client, text }));
    }

    /// M-r: the branch's PR in the browser, else its repository when the
    /// branch has no PR yet; with `repo`, the repository (M-p's "Open the
    /// repository in the browser"). Why it can't, said.
    fn open_on_github(&mut self, client: ClientId, repo: bool) {
        let Some((_, _, dir)) = self.here(client) else {
            return;
        };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let what = if repo { "the repository" } else { "the PR" };
            let repo_url = |r: github::Repo| format!("https://github.com/{r}");
            let url = if repo {
                github::repo_of(&dir)
                    .await
                    .map(repo_url)
                    .map_err(|e| e.to_string())
            } else {
                match github::pr_for_branch(&dir, "open on GitHub").await {
                    Ok(Some(pr)) => Ok(pr.url),
                    // A branch with no PR yet: its repository, and a word
                    // saying why that and not the PR.
                    Ok(None) => {
                        let branch = crate::tabs::git::locate(&dir)
                            .map(|p| p.branch)
                            .unwrap_or_else(|| "(none)".into());
                        let opened = github::repo_of(&dir).await.map(repo_url);
                        if opened.is_ok() {
                            Server::say_later(
                                &tx,
                                client,
                                format!(
                                    "Branch {branch} has no PR yet, so its repository opened instead."
                                ),
                            );
                        }
                        opened.map_err(|e| e.to_string())
                    }
                    Err(e) => Err(e.to_string()),
                }
            };
            match url {
                Ok(url) => links::open_in_background(url),
                Err(why) => Server::say_later(&tx, client, format!("Can't open {what}.\n\n{why}")),
            }
        });
    }

    /// M-l: the branch's Linear ticket in the browser.
    fn open_ticket(&mut self, client: ClientId) {
        let Some((_, tab, dir)) = self.here(client) else {
            return;
        };
        let Some(id) = self.ticket_here(tab, &dir) else {
            self.say(Some(client), "No Linear ticket in this branch or tab name");
            return;
        };
        self.open_ticket_id(client, id);
    }

    /// A Linear ticket by its ID in the browser: at the workspace's link
    /// when one is written down, else where Linear says it is (M-l, and
    /// M-p's "Open a Linear ticket in the browser by ID").
    pub(crate) fn open_ticket_id(&mut self, client: ClientId, id: String) {
        if let Some(workspace) = linear_workspace() {
            links::open_in_background(format!("https://linear.app/{workspace}/issue/{id}"));
            return;
        }
        // No workspace written down: Linear says where the ticket is.
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match linear::ticket(&id, "M-l open ticket").await {
                Ok(Some(t)) if !t.url.is_empty() => links::open_in_background(t.url),
                Ok(_) => Server::say_later(&tx, client, format!("Linear has no ticket {id}.")),
                Err(e) => Server::say_later(&tx, client, format!("Can't open {id}.\n\n{e}")),
            }
        });
    }

    /// Copies `text` to the client's clipboard and says so, as
    /// branch-copy.sh did.
    fn copy_said(&mut self, client: ClientId, text: &str) {
        self.to_clipboard(Some(client), text);
        self.say(Some(client), &format!("Copied {text}"));
    }

    /// M-p's "Copy the branch's ticket ID" and, with `url`, its link: at
    /// the workspace written down, else where Linear says the ticket is.
    fn copy_ticket(&mut self, client: ClientId, url: bool) {
        let Some((_, tab, dir)) = self.here(client) else {
            return;
        };
        let Some(id) = self.ticket_here(tab, &dir) else {
            self.say(Some(client), "No Linear ticket in this branch or tab name");
            return;
        };
        if !url {
            self.copy_said(client, &id);
            return;
        }
        if let Some(workspace) = linear_workspace() {
            self.copy_said(
                client,
                &format!("https://linear.app/{workspace}/issue/{id}"),
            );
            return;
        }
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let said = match linear::ticket(&id, "copy the ticket's URL").await {
                Ok(Some(t)) if !t.url.is_empty() => {
                    let _ = tx.send(Event::Views(ViewsEvent::Copy {
                        client,
                        text: t.url,
                    }));
                    return;
                }
                Ok(_) => format!("Linear has no ticket {id}."),
                Err(e) => format!("Can't copy {id}'s URL.\n\n{e}"),
            };
            Server::say_later(&tx, client, said);
        });
    }

    /// M-p's "Copy the branch's PR number" and, with `url`, its link.
    fn copy_pr(&mut self, client: ClientId, url: bool) {
        let Some((_, _, dir)) = self.here(client) else {
            return;
        };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let said = match github::pr_for_branch(&dir, "copy the PR").await {
                Ok(Some(pr)) => {
                    let text = if url { pr.url } else { pr.number.to_string() };
                    let _ = tx.send(Event::Views(ViewsEvent::Copy { client, text }));
                    return;
                }
                Ok(None) => "This branch has no pull request.".to_string(),
                Err(e) => format!("Can't copy the PR.\n\n{e}"),
            };
            Server::say_later(&tx, client, said);
        });
    }

    /// M-p's "Open the repository's merge queue in the browser": the queue
    /// of the branch's PR's base, else of the repository's default branch.
    fn open_merge_queue(&mut self, client: ClientId) {
        let Some((_, _, dir)) = self.here(client) else {
            return;
        };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let source = "open the merge queue";
            let queue = match github::pr_for_branch(&dir, source).await {
                Ok(Some(pr)) => match pr.url.rsplit_once("/pull/") {
                    Some((repo, _)) if !pr.base_ref_name.is_empty() => {
                        Ok(format!("{repo}/queue/{}", pr.base_ref_name))
                    }
                    _ => Err("GitHub didn't say which branch's queue to open.".to_string()),
                },
                Ok(None) => match github::repo_of(&dir).await {
                    Ok(repo) => github::default_branch(&repo, source)
                        .await
                        .map(|base| format!("https://github.com/{repo}/queue/{base}"))
                        .map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                },
                Err(e) => Err(e.to_string()),
            };
            match queue {
                Ok(url) => links::open_in_background(url),
                Err(why) => {
                    Server::say_later(&tx, client, format!("Can't open the merge queue.\n\n{why}"))
                }
            }
        });
    }

    /// M-p's "Open the branch's Linear project in the browser": the project
    /// Linear says the branch's ticket is in, and why not when it can't.
    fn open_project(&mut self, client: ClientId) {
        let Some((_, tab, dir)) = self.here(client) else {
            return;
        };
        let Some(id) = self.ticket_here(tab, &dir) else {
            self.say(
                Some(client),
                "No Linear ticket in this branch or tab name, so there's no project to open.",
            );
            return;
        };
        self.say(
            Some(client),
            &format!("Looking up the Linear project for {id}..."),
        );
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let said = match linear::project_url(&id, "open the ticket's project").await {
                Ok(Some(url)) => {
                    links::open_in_background(url);
                    return;
                }
                Ok(None) => format!("{id} isn't in a Linear project."),
                Err(e) if net::kind_of(&e) == Some(&ErrorKind::NotFound) => {
                    format!("Linear has no ticket {id}, so there's no project to open.")
                }
                Err(e) => format!("Can't look up the Linear project for {id}.\n\n{e}"),
            };
            Server::say_later(&tx, client, said);
        });
    }

    /// M-m: whether the branch's PR can merge, in a box, and what can be
    /// done about it.
    fn pr_ready(&mut self, client: ClientId) {
        let Some((pane, tab, dir)) = self.here(client) else {
            return;
        };
        let checks_dir = dir.to_string_lossy().into_owned();
        self.open(
            client,
            Box::new(ReadyBox::new()),
            Some(Box::new(move |server: &mut Server, client, outcome| {
                let Outcome::Chose(i) = outcome else {
                    return;
                };
                match Choice::from_index(i) {
                    Choice::SeeFailing => {
                        let pane = Some(pane)
                            .filter(|p| server.panes.contains_key(p))
                            .or_else(|| server.current_pane(client));
                        if let Some(pane) = pane {
                            server.open_beside(
                                pane,
                                CHECKS_VIEW,
                                vec!["pr-view".into(), "--checks".into(), checks_dir],
                                LINK_PERCENT,
                            );
                        }
                    }
                    Choice::ResolveConflicts => server.resolve_in_tab(client, tab),
                    Choice::Close => {}
                }
            })),
        );
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let answer = ready::check(dir).await;
            let _ = tx.send(Event::Views(ViewsEvent::Ready { client, answer }));
        });
    }

    /// `/resolve-conflicts` typed into the tab's Claude, the focus put
    /// there for you to press Enter, since whatever is in its prompt
    /// already can't be seen from here (pr-ready.sh's `resolve_in_tab`).
    fn resolve_in_tab(&mut self, client: ClientId, tab: TabId) {
        let panes = self
            .tabs
            .get(&tab)
            .map(|t| t.layout.panes())
            .unwrap_or_default();
        match panes.into_iter().find(|p| self.runs_claude(*p)) {
            Some(pane) => {
                if let Some(slot) = self.panes.get_mut(&pane) {
                    let _ = slot.pane.write(b"/resolve-conflicts");
                }
                self.focus_pane(pane);
            }
            None => self.say(Some(client), "No Claude in this tab to resolve them."),
        }
    }

    /// M-S-f: asks what to search for, then searches.
    fn search_everything(&mut self, client: ClientId) {
        if !settings::is_on("search_everything") {
            self.say(
                Some(client),
                "Search everything is off in Settings (Integrations).",
            );
            return;
        }
        let pane = self.current_pane(client);
        let picker = Picker::new("Search everything", Vec::new())
            .named("search-query")
            .prompt("Search notes, Linear, GitHub and Slack: ")
            .free();
        self.pick(client, picker, move |server, client, picked| {
            let Some(picked) = picked else {
                return;
            };
            let query = picked.value.trim().to_string();
            if !query.is_empty() {
                server.start_search(client, pane, query);
            }
        });
    }

    /// Asks each source, the list filled in as they answer; Enter reads
    /// the one chosen beside the pane the search started from, and Ctrl-O
    /// opens a web link in the browser instead, as search.sh's did.
    fn start_search(&mut self, client: ClientId, pane: Option<PaneId>, query: String) {
        self.views.searches += 1;
        let id = self.views.searches;
        let mut waiting = vec![Source::Notes];
        if settings::is_on("linear") {
            waiting.push(Source::Linear);
        }
        if settings::is_on("github") {
            waiting.push(Source::GitHub);
        }
        if search::asks_slack() {
            waiting.push(Source::Slack);
        }
        self.views.found.insert(
            client,
            Running {
                id,
                query: query.clone(),
                rows: Vec::new(),
                waiting: waiting.clone(),
                pane,
            },
        );
        let picker = Picker::new(search_title(&query, &waiting), Vec::new())
            .named("search")
            .prompt("Narrow: ")
            .also("read it beside the pane", "open it in the browser");
        self.pick_also(client, picker, move |server, client, picked| {
            let running = server.views.found.remove(&client);
            let Some((picked, browser)) = picked.filter(|(p, _)| !p.value.is_empty()) else {
                return;
            };
            let web = picked.value.starts_with("https://") || picked.value.starts_with("http://");
            // What can't be read here (a Slack message) opens instead.
            if (browser && web)
                || page::kind_of(&picked.value, &page::notes_dir()) == Ok(page::Kind::Slack)
            {
                links::open_in_background(picked.value);
                return;
            }

            let pane = running
                .and_then(|r| r.pane)
                .filter(|p| server.panes.contains_key(p))
                .or_else(|| server.current_pane(client));
            if let Some(pane) = pane {
                server.read_beside(pane, &picked.value);
            }
        });
        // The repositories the tabs are in, for GitHub's.
        let mut dirs: Vec<PathBuf> = Vec::new();
        for slot in self.panes.values() {
            if let Some(dir) = slot.pane.cwd()
                && !dirs.contains(&dir)
            {
                dirs.push(dir);
            }
        }
        for source in waiting {
            let tx = self.tx.clone();
            let q = query.clone();
            let send = move |rows: Vec<Found>| {
                let _ = tx.send(Event::Views(ViewsEvent::Found {
                    client,
                    search: id,
                    source,
                    rows,
                }));
            };
            // A source that fails says why in a row of its own, which
            // reads nothing when chosen.
            let said = move |why: String| {
                vec![Found {
                    source,
                    text: why,
                    detail: String::new(),
                    link: String::new(),
                }]
            };
            match source {
                Source::Notes => {
                    let dir = page::notes_dir();
                    tokio::task::spawn_blocking(move || send(search::notes(&dir, &q)));
                }
                Source::Linear => {
                    tokio::spawn(
                        async move { send(search::linear(&q).await.unwrap_or_else(said)) },
                    );
                }
                Source::GitHub => {
                    let dirs = dirs.clone();
                    tokio::spawn(async move {
                        send(search::github(&q, dirs).await.unwrap_or_else(said))
                    });
                }
                Source::Slack => {
                    tokio::spawn(async move { send(search::slack(&q).await.unwrap_or_else(said)) });
                }
            }
        }
    }

    fn search_found(&mut self, client: ClientId, id: u64, source: Source, rows: Vec<Found>) {
        let Some(running) = self.views.found.get_mut(&client).filter(|r| r.id == id) else {
            return;
        };
        search::merge(&mut running.rows, source, rows);
        running.waiting.retain(|s| *s != source);
        let items: Vec<PickItem> = running.rows.iter().map(item).collect();
        let title = search_title(&running.query, &running.waiting);
        if let Some(c) = self.clients.get_mut(&client)
            && let Some(picker) = c.overlays.find_mut::<Picker>()
            && picker.kind() == "search"
        {
            picker.set_items(items);
            picker.set_title(title);
            c.dirty = true;
        }
    }

    /// The links a pane shows: the note's, top to bottom, in the notes
    /// pane while it shows one; else the screen's and its scrollback's,
    /// newest first. Why there are none, when there are none.
    fn pane_links(&self, pane: PaneId) -> Result<Vec<Shown>, String> {
        let Some(slot) = self.panes.get(&pane) else {
            return Err("No pane here".into());
        };
        let notes = slot
            .opts
            .get("@notes_pane")
            .is_some_and(|v| !v.is_empty() && v != "0");
        if notes && let Some(rel) = crate::notes::serve::shown() {
            let file = crate::notes::store::notes_dir().join(&rel);
            if let Some(text) = util::read_text(&file) {
                let found = shown::note_links(&text);
                if found.is_empty() {
                    return Err(format!("No links in {}", rel.trim_end_matches(".md")));
                }
                return Ok(found);
            }
        }
        let (text, hyperlinks) = slot.pane.text_and_links(3000);
        let found = shown::links_in(&text, &hyperlinks);
        if found.is_empty() {
            return Err("No links in this pane".into());
        }
        Ok(found)
    }

    /// A link to read beside the pane: one typed, or one of those the pane
    /// shows (link-view.sh's `pick`); Ctrl-O opens it in the browser.
    fn ask_link(&mut self, client: ClientId) {
        let Some((pane, _, _)) = self.here(client) else {
            return;
        };
        let shown = self.pane_links(pane).unwrap_or_default();
        let picker = Picker::new("Read a link beside this pane", rows(&shown))
            .named("links")
            .prompt("Read: ")
            .exact()
            .free()
            .also("read it beside", "open in browser");
        self.pick_also(client, picker, move |server, client, picked| {
            let Some((picked, other)) = picked else {
                return;
            };
            server.link_picked(client, pane, picked.value.trim(), other);
        });
    }

    /// M-p's "Open a link from this pane in the browser" (`copy` false) and
    /// "Copy a link from this pane" (`copy`), each list's Ctrl-O doing what
    /// link-view.sh's did: reading it beside the pane, and opening it.
    fn pick_link(&mut self, client: ClientId, copy: bool) {
        let Some((pane, _, _)) = self.here(client) else {
            return;
        };
        let shown = match self.pane_links(pane) {
            Ok(found) => found,
            Err(why) => return self.say(Some(client), &why),
        };
        let (title, prompt, enter, other) = if copy {
            ("Copy a link", "Copy: ", "copy it", "open in browser")
        } else {
            ("Open a link", "Open: ", "open in browser", "read it beside")
        };
        let picker = Picker::new(title, rows(&shown))
            .named("links")
            .prompt(prompt)
            .exact()
            .also(enter, other);
        self.pick_also(client, picker, move |server, client, picked| {
            let Some((picked, other)) = picked else {
                return;
            };
            let url = picked.value;
            match (copy, other) {
                (true, false) => {
                    server.to_clipboard(Some(client), &url);
                    server.say(Some(client), &format!("Copied {url}"));
                }
                (true, true) => server.link_picked(client, pane, &url, true),
                (false, other) => server.link_picked(client, pane, &url, !other),
            }
        });
    }

    /// A link picked from a list: opened in the browser (`browser`), else
    /// read beside the pane it was listed from.
    fn link_picked(&mut self, client: ClientId, pane: PaneId, url: &str, browser: bool) {
        if url.is_empty() {
            return;
        }
        if browser {
            if shown::safe(url) && links::host_of(url).is_some() {
                links::open_in_background(url.to_string());
            } else {
                self.say(Some(client), "That isn't a link that can be opened here");
            }
            return;
        }
        if !is_readable(url) {
            self.say(Some(client), "That isn't a link that can be read here");
            return;
        }
        let pane = Some(pane)
            .filter(|p| self.panes.contains_key(p))
            .or_else(|| self.current_pane(client));
        if let Some(pane) = pane {
            self.read_beside(pane, url);
        }
    }

    /// M-p's "Open an image or video from this pane": a file it names
    /// (from where the pane is, else `~` or absolute) or a link to one,
    /// newest first; Enter opens a file in its app and a link in the
    /// browser, Ctrl-O shows a file in its folder (link-view.sh's `media`).
    fn open_media(&mut self, client: ClientId) {
        let Some((pane, _, dir)) = self.here(client) else {
            return;
        };
        let Some(slot) = self.panes.get(&pane) else {
            return;
        };
        let text = slot.pane.text(3000);
        let links = self.pane_links(pane).unwrap_or_default();
        let found = shown::media_in(&text, &dir, &paths::home(), &links);
        if found.is_empty() {
            return self.say(Some(client), "No image or video in this pane");
        }
        let picker = Picker::new("Images and videos", rows(&found))
            .named("media")
            .prompt("Open: ")
            .exact()
            .also("open it", "show in its folder");
        self.pick_also(client, picker, move |server, client, picked| {
            let Some((picked, folder)) = picked else {
                return;
            };
            let target = picked.value;
            if target.contains("://") {
                return server.link_picked(client, pane, &target, true);
            }
            let file = PathBuf::from(&target);
            if !file.is_file() {
                return server.say(Some(client), &format!("{} is gone", paths::tilde(&file)));
            }
            let tx = server.tx.clone();
            tokio::task::spawn_blocking(move || {
                let opened = if folder {
                    crate::platform::reveal(&file)
                } else {
                    crate::platform::open(&target)
                };
                if let Err(e) = opened {
                    Server::say_later(&tx, client, format!("Couldn't open it: {e}"));
                }
            });
        });
    }

    /// The link read beside this pane, in the browser.
    fn open_link_read(&mut self, client: ClientId) {
        let Some((_, tab, _)) = self.here(client) else {
            return;
        };
        let url = self
            .marked_in(tab, LINK_VIEW)
            .and_then(|p| self.panes.get(&p))
            .and_then(|s| s.opts.get(LINK_URL).cloned())
            .filter(|u| links::host_of(u).is_some());
        match url {
            Some(url) => links::open_in_background(url),
            None => self.say(Some(client), "No link is being read in this tab"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_says_which_sources_are_still_looking() {
        assert_eq!(
            search_title("deploy", &[Source::GitHub, Source::Slack]),
            "Search: \"deploy\" · GitHub, Slack still looking"
        );
        assert_eq!(search_title("x", &[]), "Search: \"x\"");
    }
}
