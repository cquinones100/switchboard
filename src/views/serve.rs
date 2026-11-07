//! The server's side of the views: what M-v, M-i, M-r, M-l, M-m and M-S-f
//! do, and the palette's link commands, as tmux.conf bound them to
//! pr-view.sh, ticket-view.sh, gh-open.sh, linear-ticket.sh, pr-ready.sh,
//! search.sh and link-view.sh.
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
//! settings.sh's binding did, and says so after.

use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::ready::{self, Choice, ReadyBox};
use super::search::{self, Found, Running, Source};
use super::{Command, ViewsEvent, page, ticket};
use crate::integrations::{github, linear};
use crate::mux::server::{ClientId, Event, Server};
use crate::mux::{Direction, PaneId, TabId};
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

static LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https?://[A-Za-z0-9._~:/?#\[\]@!$&'()*+,;=%-]+").unwrap());

/// The links in a pane's text, newest (lowest) first, each once, without
/// the punctuation that follows a link in a sentence (link-view.sh's
/// `links_of`).
pub fn links_in(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for m in LINK.find_iter(text) {
        let mut url = m.as_str();
        while let Some(last) = url.chars().last() {
            let unbalanced = last == ')' && url.matches('(').count() < url.matches(')').count();
            if ".,;:!?'".contains(last) || unbalanced {
                url = &url[..url.len() - 1];
            } else {
                break;
            }
        }
        found.push(url.to_string());
    }
    let mut out: Vec<String> = Vec::new();
    for url in found.into_iter().rev() {
        if !out.contains(&url) {
            out.push(url);
        }
    }
    out
}

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
    url.starts_with("file://") || links::host_of(url).is_some()
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
        }
    }

    /// What the views' work off the task found.
    pub fn views_event(&mut self, event: ViewsEvent) {
        match event {
            ViewsEvent::Say { client, text } => self.say(Some(client), &text),
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

    fn service_off(&mut self, client: ClientId, service: &str) {
        if !self.tip(&format!("{service}_off"), Some(client)) {
            let name = if service == "github" {
                "GitHub"
            } else {
                "Linear"
            };
            self.say(
                Some(client),
                &format!("{name} is off in Settings, so its keys do nothing until it is on again."),
            );
        }
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
    fn ticket_here(&self, tab: TabId, dir: &std::path::Path) -> Option<String> {
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
    /// the one chosen beside the pane the search started from.
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
            .prompt("Narrow: ");
        self.pick(client, picker, move |server, client, picked| {
            let running = server.views.found.remove(&client);
            let Some(picked) = picked.filter(|p| !p.value.is_empty()) else {
                return;
            };
            // What can't be read here (a Slack message) opens instead.
            if page::kind_of(&picked.value, &page::notes_dir()) == Ok(page::Kind::Slack) {
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

    /// A link to read beside the pane: one typed, or one of those the pane
    /// shows, newest first (link-view.sh's `pick`).
    fn ask_link(&mut self, client: ClientId) {
        let Some((pane, _, _)) = self.here(client) else {
            return;
        };
        let shown = self
            .panes
            .get(&pane)
            .map(|s| links_in(&s.pane.text(3000)))
            .unwrap_or_default();
        let items: Vec<PickItem> = shown
            .iter()
            .map(|l| PickItem::new(l.clone(), l.clone()))
            .collect();
        let picker = Picker::new("Read a link beside this pane", items)
            .named("links")
            .prompt("Link: ")
            .exact()
            .free();
        self.pick(client, picker, move |server, client, picked| {
            let Some(picked) = picked else {
                return;
            };
            let url = picked.value.trim().to_string();
            if url.is_empty() {
                return;
            }
            if !is_readable(&url) {
                server.say(Some(client), "That isn't a link that can be read here");
                return;
            }
            let pane = Some(pane)
                .filter(|p| server.panes.contains_key(p))
                .or_else(|| server.current_pane(client));
            if let Some(pane) = pane {
                server.read_beside(pane, &url);
            }
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
    fn a_panes_links_are_listed_newest_first_each_once() {
        let text = "See https://plain.example/one.\nThe PR (https://github.com/o/r/pull/2) is up\n\
                    again https://plain.example/one and https://same.example/x";
        assert_eq!(
            links_in(text),
            vec![
                "https://same.example/x",
                "https://plain.example/one",
                "https://github.com/o/r/pull/2",
            ]
        );
    }

    #[test]
    fn a_search_says_which_sources_are_still_looking() {
        assert_eq!(
            search_title("deploy", &[Source::GitHub, Source::Slack]),
            "Search: \"deploy\" · GitHub, Slack still looking"
        );
        assert_eq!(search_title("x", &[]), "Search: \"x\"");
    }
}
