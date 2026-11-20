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
//! Each list holds the newest first, across the pane, what you sent its
//! Claude and the note (`seen`).
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
use super::search::{self, Found, Source};
use super::search_box::SearchBox;
use super::shown::{self, Shown};
use super::ticket::Ticket;
use super::{Command, ViewsEvent, page, seen, ticket};
use crate::integrations::{github, linear};
use crate::mux::server::{ClientId, Event, Server};
use crate::mux::{Direction, PaneId, TabId};
use crate::net::{self, ErrorKind};
use crate::ui::{Outcome, PickItem, Picker};
use crate::{links, paths, settings, util};

/// The options marking the panes the views open.
pub const PR_VIEW: &str = "@pr_view";
pub const TICKET_VIEW: &str = "@ticket_view";
/// The ticket a ticket view's pane shows (M-i's, or a link view reading a
/// Linear link), for telling it when the lookups see the ticket change
/// (`Server::views_follow`).
pub const TICKET_ID: &str = "@ticket_view_id";
/// The tab option holding the ticket or project chosen for it
/// (`Ticket::arg`), which comes before any found from its branch or name.
pub const TAB_TICKET: &str = "@tab_ticket";
/// The picker's row that lets a chosen ticket go.
const AUTOMATIC: &str = "automatic";
pub const LINK_VIEW: &str = "@link_view";
pub const LINK_URL: &str = "@link_url";
pub const CHECKS_VIEW: &str = "@checks_view";

/// The share of the pane a view takes, as tmux.conf's `split-window -l`.
/// The longest a link's opening message stays up, closed before then when
/// the open answers.
const OPENING_UP_TO: u64 = 120;
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
            util::read_json_object(crate::files::file("linear.json"))
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

/// How a list marks a link that comes from the note open in the tab, as
/// it marks one you gave Claude (`given::LABEL`).
const IN_THE_NOTE: &str = "in the note";

/// What the media list shows beside a file only the conversation named.
const CONVERSATION: &str = "from the conversation";

/// The rows of a list of what a pane shows, each giving its address.
fn rows(found: &[Shown]) -> Vec<PickItem> {
    found
        .iter()
        .map(|f| PickItem::new(f.row(), f.url.clone()))
        .collect()
}

impl Server {
    /// A view's command, from its key or the palette.
    pub fn view_command(&mut self, client: ClientId, command: Command) {
        if let Some(service) = crate::integrations::tagged_off(command.tags()) {
            self.service_off(client, service);
            return;
        }
        match command {
            Command::PrView => self.toggle_pr_view(client),
            Command::TicketView => self.toggle_ticket_view(client),
            Command::OpenPr => self.open_on_github(client, false),
            Command::OpenRepo => self.open_on_github(client, true),
            Command::OpenTicket => self.open_ticket(client),
            Command::ChooseTicket => {
                if let Some(tab) = self.here(client).map(|(_, tab, _)| tab) {
                    self.choose_ticket(client, tab, false);
                }
            }
            Command::PrReady => self.pr_ready(client),
            Command::Search => self.search_everything(client, ""),
            Command::SearchTabs => self.search_everything(client, Source::Tabs.prefix()),
            Command::SearchNotes => self.search_everything(client, Source::Notes.prefix()),
            Command::SearchLinear => self.search_everything(client, Source::Linear.prefix()),
            Command::SearchGitHub => self.search_everything(client, Source::GitHub.prefix()),
            Command::SearchSlack => self.search_everything(client, Source::Slack.prefix()),
            Command::ReadLink => self.ask_link(client),
            Command::OpenLinkRead => self.open_link_read(client),
            Command::OpenLink => self.pick_link(client, false),
            Command::CopyLink => self.pick_link(client, true),
            Command::GoToMentionedTab => self.pick_mentioned_tab(client, false),
            Command::CopyMentionedTab => self.pick_mentioned_tab(client, true),
            Command::OpenMedia => self.open_media(client),
            Command::OpenMergeQueue => self.open_merge_queue(client),
            Command::OpenProject => self.open_project(client),
            Command::CopyTicketId => self.copy_ticket(client, false),
            Command::CopyTicketUrl => self.copy_ticket(client, true),
            Command::CopyPrNumber => self.copy_pr(client, false),
            Command::CopyPrUrl => self.copy_pr(client, true),
            Command::ClearImageCache => self.clear_image_cache(client),
        }
    }

    /// What the views' work off the task found.
    pub fn views_event(&mut self, event: ViewsEvent) {
        match event {
            ViewsEvent::Say { client, text } => self.say(Some(client), &text),
            ViewsEvent::Open { client, url } => self.open_link(client, url),
            ViewsEvent::Opening { client, words } => {
                // Never on another client, where nothing would close it.
                if !self.clients.contains_key(&client) {
                    return;
                }
                // Up until the open answers (`Opened`), however long handing
                // the link over takes.
                self.say_for(Some(client), &words, OPENING_UP_TO);
            }
            ViewsEvent::Opened {
                client,
                words,
                shown,
            } => self.link_opened(client, &words, shown),
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
            ViewsEvent::Ask {
                client,
                search,
                query,
                sources,
                pane,
            } => self.search_ask(client, search, query, sources, pane),
            ViewsEvent::Chips { off } => {
                if let Err(e) = search::write_off(&off) {
                    self.say(None, &format!("Couldn't keep the search's sources: {e}"));
                }
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
        let dir = self.pane_dir(pane).unwrap_or_else(paths::home);
        Some((pane, slot.tab, dir))
    }

    /// The directory a tab's work is in, for its ticket: `dir`, the
    /// focused pane's, unless that pane is Notes beside the work, which runs
    /// in your home; then another pane's of the tab.
    pub(crate) fn work_dir(&self, tab: TabId, pane: PaneId, dir: PathBuf) -> PathBuf {
        let notes = |p: &PaneId| {
            self.panes
                .get(p)
                .is_some_and(|s| s.opts.contains_key("@notes_pane"))
        };
        if !notes(&pane) {
            return dir;
        }
        // The directory the tab is named from, its work pane's.
        if let Some(path) = self.tab_path(tab) {
            return path.to_path_buf();
        }
        self.panes
            .iter()
            .filter(|(p, s)| s.tab == tab && !notes(p))
            .find_map(|(p, _)| self.pane_dir(*p))
            .unwrap_or(dir)
    }

    /// The pane a view about the tab's work opens beside: `pane`, unless
    /// it is Notes beside the work, then the tab's first other pane.
    fn work_pane(&self, tab: TabId, pane: PaneId) -> PaneId {
        let notes = |p: &PaneId| {
            self.panes
                .get(p)
                .is_some_and(|s| s.opts.contains_key("@notes_pane"))
        };
        if !notes(&pane) {
            return pane;
        }
        self.tabs
            .get(&tab)
            .and_then(|t| t.layout.panes().into_iter().find(|p| !notes(p)))
            .unwrap_or(pane)
    }

    /// Closes a ticket view as M-w closes a pane: a shell in its place when
    /// it is the tab's last, and, in place of a pane no longer there,
    /// standing in for nothing, a shell rather than no tab.
    fn close_ticket_view(&mut self, view: PaneId) {
        if self.parked_of(view).is_none()
            && let Some(slot) = self.panes.get_mut(&view)
        {
            slot.opts.remove(crate::diff::in_place::IN_PLACE);
        }
        self.close_only_pane(view, true);
    }

    /// A ticket view read again in its own place, on `ticket`, or a shell
    /// with none: its program replaced, the tab not handed to other work.
    fn read_view_again(&mut self, view: PaneId, ticket: Option<Ticket>) {
        let arg = ticket.as_ref().map(Ticket::arg);
        let command = arg
            .clone()
            .map(|arg| vec![exe(), "ticket-view".into(), "--ticket".into(), arg]);
        if self.replace_program(view, command).is_ok()
            && let Some(arg) = arg
            && let Some(slot) = self.panes.get_mut(&view)
        {
            slot.opts.insert(TICKET_ID.into(), arg);
            slot.opts.insert(TICKET_VIEW.into(), "1".into());
        }
    }

    /// Every ticket view in a tab's layout, on screen.
    pub(crate) fn ticket_views_shown(&self, tab: TabId) -> Vec<PaneId> {
        let Some(t) = self.tabs.get(&tab) else {
            return Vec::new();
        };
        t.layout
            .panes()
            .into_iter()
            .filter(|p| {
                self.panes
                    .get(p)
                    .is_some_and(|s| s.opts.get(TICKET_VIEW).is_some_and(|v| v == "1"))
            })
            .collect()
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
        let tool = args.first().cloned().unwrap_or_default();
        command.extend(args);
        // UX 2.0 shows the PR and the ticket in Claude's place, as M-d
        // shows the diff (src/diff/in_place.rs), the key closing it giving
        // the place back; the header names what is shown.
        if crate::settings::is_on("ux2") && (mark == PR_VIEW || mark == TICKET_VIEW) {
            return match self.open_in_place(pane, &tool, command, None) {
                Ok(view) => {
                    if let Some(slot) = self.panes.get_mut(&view) {
                        slot.opts.insert(mark.to_string(), "1".into());
                    }
                    Some(view)
                }
                Err(e) => {
                    self.say(None, &format!("Couldn't open it in this pane's place: {e}"));
                    None
                }
            };
        }
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
            // A Linear link is read by the ticket view, which follows its
            // ticket here as M-i's does.
            if let Ok(page::Kind::Ticket(id)) = page::kind_of(url, std::path::Path::new("")) {
                slot.opts.insert(TICKET_ID.into(), id);
                self.views_follow(None);
            }
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
        // What the lookups know now is where the view starts following.
        self.views_follow(None);
    }

    /// The ticket a tab's work is on: one chosen for it (`@tab_ticket`),
    /// else its branch's (the GitHub issue it was started for or its
    /// Linear ticket, as repos.json's `tickets` says), else a ticket or
    /// project in a name you gave it, else in the label it shows. Reads
    /// git's and repos.json's files, so never on the drawing path.
    pub(crate) fn tab_ticket_at(&self, tab: TabId, dir: &std::path::Path) -> Option<Ticket> {
        // One chosen of a service turned off since stands aside, so it never
        // keeps the branch's issue from opening, and comes last, so the keys
        // say its service is off rather than that there is none.
        let chosen = self.chosen_ticket(tab);
        if let Some(on) = chosen.clone().filter(|c| settings::is_on(c.service())) {
            return Some(on);
        }
        let provider = crate::repos::tickets_known(dir);
        self.tab_ticket_by(tab, dir, provider.as_deref()).or(chosen)
    }

    /// The ticket or project chosen for a tab, kept as `@tab_ticket`.
    fn chosen_ticket(&self, tab: TabId) -> Option<Ticket> {
        Ticket::parse(self.chosen_ticket_kept(tab)?)
    }

    /// `@tab_ticket` as kept, read or not.
    fn chosen_ticket_kept(&self, tab: TabId) -> Option<&String> {
        self.tabs.get(&tab)?.opts.get(TAB_TICKET)
    }

    /// `tab_ticket_at` from the branch and names alone, by the provider
    /// given.
    fn tab_ticket_by(
        &self,
        tab: TabId,
        dir: &std::path::Path,
        provider: Option<&str>,
    ) -> Option<Ticket> {
        let t = self.tabs.get(&tab)?;
        let mut found = ticket::of_dir_all(dir, provider);
        found.extend(
            t.opts
                .get("@tab_name")
                .and_then(|n| ticket::in_name(n))
                .or_else(|| ticket::in_name(&t.name)),
        );
        // The label shown names the branch's Linear ticket too, which a
        // repository whose tickets are GitHub's never takes; any other it
        // names counts.
        let from_branch: Vec<Ticket> = ticket::id_for_dir(dir)
            .map(Ticket::Linear)
            .into_iter()
            .chain(crate::tabs::git::locate(dir).and_then(|p| ticket::linear_in_branch(&p.branch)))
            .collect();
        found.extend(
            ticket::in_name(&self.tab_label(tab))
                .filter(|t| provider != Some("github") || !from_branch.contains(t)),
        );
        ticket::first_on(found)
    }

    /// The Linear ticket a tab's work is on, for what reads Linear's
    /// tickets alone (the branch's ticket copied, its image): the one the
    /// branch names, whatever repos.json's `tickets` says, so a branch
    /// started for an issue still gives it; else one chosen for the tab;
    /// else one its names or label give.
    pub(crate) fn ticket_here(&self, tab: TabId, dir: &std::path::Path) -> Option<String> {
        let linear = |t: Ticket| match t {
            Ticket::Linear(id) => Some(id),
            _ => None,
        };
        let names = || {
            let t = self.tabs.get(&tab)?;
            t.opts
                .get("@tab_name")
                .and_then(|n| ticket::id_in_name(n))
                .or_else(|| ticket::id_in_name(&t.name))
                .or_else(|| ticket::id_in_name(&self.tab_label(tab)))
        };
        ticket::id_for_dir(dir)
            .or_else(|| self.chosen_ticket(tab).and_then(linear))
            .or_else(names)
    }

    /// Whether M-i has a ticket to open whose service is on, as
    /// `tab_ticket_at` finds one, from what the server holds alone: the
    /// branch tab naming last read and the tab's options and names. For
    /// the drawing path, which reads no file.
    pub(crate) fn ticket_known(&self, tab: TabId) -> bool {
        let Some(t) = self.tabs.get(&tab) else {
            return false;
        };
        // One chosen of a service turned off stands aside, as it does for M-i.
        if let Some(chosen) = t.opts.get(TAB_TICKET).and_then(|v| Ticket::parse(v))
            && settings::is_on(chosen.service())
        {
            return true;
        }
        let branch = self
            .tab_path(tab)
            .and_then(|d| self.tab_names.names.branch(d));
        if branch.and_then(ticket::issue_number_of_branch).is_some() && settings::is_on("github") {
            return true;
        }
        settings::is_on("linear")
            && (branch.and_then(ticket::linear_in_branch).is_some()
                || t.opts
                    .get("@tab_name")
                    .and_then(|n| ticket::in_name(n))
                    .or_else(|| ticket::in_name(&t.name))
                    .or_else(|| ticket::in_name(&self.tab_label(tab)))
                    .is_some())
    }

    /// M-i: the tab's ticket in a view to the right, or closed; with none
    /// found, yours to choose one from.
    fn toggle_ticket_view(&mut self, client: ClientId) {
        let Some((pane, tab, dir)) = self.here(client) else {
            return;
        };
        let dir = self.work_dir(tab, pane, dir);
        // Closed as M-w closes a pane: a shell in its place when it is the
        // tab's last, so the tab and the ticket chosen for it stay.
        if let Some(open) = self.marked_in(tab, TICKET_VIEW) {
            self.close_ticket_view(open);
            return;
        }
        // One of a service turned off counts as none when Linear can offer
        // a choice.
        let picks = settings::is_on("linear") && settings::is_on("ticket_picker");
        let found = self
            .tab_ticket_at(tab, &dir)
            .filter(|f| settings::is_on(f.service()) || !picks);
        let Some(found) = found else {
            if settings::is_on("linear") && settings::is_on("ticket_picker") {
                self.choose_ticket(client, tab, true);
            } else {
                self.say(Some(client), "No ticket or GitHub issue for this tab");
            }
            return;
        };
        if !settings::is_on(found.service()) {
            self.service_off(client, found.service());
            return;
        }
        // Beside the work, never in Notes' place.
        let anchor = self.work_pane(tab, pane);
        self.open_ticket_view(anchor, found);
    }

    /// The ticket view for `found` beside `pane`.
    fn open_ticket_view(&mut self, pane: PaneId, found: Ticket) {
        let arg = found.arg();
        if let Some(view) = self.open_beside(
            pane,
            TICKET_VIEW,
            vec!["ticket-view".into(), "--ticket".into(), arg.clone()],
            VIEW_PERCENT,
        ) && let Some(slot) = self.panes.get_mut(&view)
        {
            slot.opts.insert(TICKET_ID.into(), arg);
        }
        self.views_follow(None);
    }

    /// "Choose this tab's ticket or project…": your Linear tickets and
    /// projects, most recently created first, read off the task while it
    /// says so; the one chosen kept on the tab (`@tab_ticket`, saved with
    /// the layout), and with `open` its view opened. Esc changes nothing.
    pub(crate) fn choose_ticket(&mut self, client: ClientId, tab: TabId, open: bool) {
        if !self.tabs.contains_key(&tab) {
            return;
        }
        // The work the tab is on as it is asked: a tab handed to other work
        // meanwhile (its directory changed) takes no ticket chosen for the old.
        let work = self.tab_path(tab).map(std::path::Path::to_path_buf);
        let moved_on = move |server: &mut Server, client: ClientId| {
            let now = server.tab_path(tab).map(std::path::Path::to_path_buf);
            let moved = now != work;
            if moved {
                let n = server.tab_number(tab);
                server.say(
                    Some(client),
                    &format!("Tab {n} moved on to other work, so no ticket was chosen for it"),
                );
            }
            moved
        };
        let moved_on_again = moved_on.clone();
        // With Linear off, Linear isn't asked: only a choice made before
        // can be let go.
        let ask_linear = settings::is_on("linear");
        if ask_linear {
            self.say(Some(client), "Getting your tickets…");
        } else if self.chosen_ticket_kept(tab).is_none() {
            self.service_off(client, "linear");
            return;
        }
        self.off_task(
            client,
            move || {
                if ask_linear {
                    util::block_on(ticket::yours())
                } else {
                    Ok(ticket::Lists::default())
                }
            },
            move |server, client, found| {
                server.unsay(client);
                if moved_on(server, client) {
                    return;
                }
                // One chosen before can be let go, the branch and the names
                // deciding again, whatever Linear answered.
                let chosen_before = server
                    .tabs
                    .get(&tab)
                    .is_some_and(|t| t.opts.contains_key(TAB_TICKET));
                let mut why_empty = None;
                let rows = match found {
                    Ok(lists) if !lists.found.is_empty() || chosen_before => {
                        if !lists.missing.is_empty() {
                            why_empty = Some(lists.missing.join(" "));
                        }
                        lists.found
                    }
                    Ok(_) => {
                        server.say(Some(client), "Linear has no tickets or projects of yours.");
                        return;
                    }
                    // Said at the top of the picker, which still lets the
                    // chosen ticket go.
                    Err(e) if chosen_before => {
                        why_empty = Some(e);
                        Vec::new()
                    }
                    Err(e) => {
                        server.say(Some(client), &format!("Couldn't get your tickets.\n\n{e}"));
                        return;
                    }
                };
                let automatic = chosen_before
                    .then(|| PickItem::new("Automatic: from its branch or its name", AUTOMATIC));
                let items = automatic
                    .into_iter()
                    .chain(
                        rows.into_iter()
                            .map(|r| PickItem::new(r.row, r.ticket.arg())),
                    )
                    .collect();
                // Named by the tab's number, since Linear may answer after
                // you've gone to another tab.
                let title = format!("Tab {}'s ticket or project", server.tab_number(tab));
                let mut picker = Picker::new(title, items).keep_order();
                if let Some(why) = why_empty {
                    picker = picker.header(why);
                }
                // A box about the tab: beside a narrow lone pane where it fits.
                server.about_tab(client, tab, move |s| {
                    s.pick(client, picker, move |server, client, picked| {
                        let Some(picked) = picked else {
                            return;
                        };
                        if moved_on_again(server, client) {
                            return;
                        }
                        let chosen = Ticket::parse(&picked.value);
                        let automatic = picked.value == AUTOMATIC;
                        if chosen.is_none() && !automatic {
                            return;
                        }
                        // Its service turned off while the picker was open
                        // takes nothing from it.
                        if let Some(c) = chosen.as_ref().filter(|c| !settings::is_on(c.service())) {
                            server.service_off(client, c.service());
                            return;
                        }
                        // A view showing the ticket before closes, so none shows
                        // one M-l no longer opens; the tab's last pane is read
                        // again in place rather than closing the tab.
                        // A view parked under another tool counts too, so
                        // none comes back showing the old ticket.
                        let shown = server.ticket_views_shown(tab);
                        let was_open = shown.first().copied();
                        let parked: Vec<PaneId> = server
                            .panes
                            .iter()
                            .filter(|(p, s)| {
                                s.tab == tab
                                    && !shown.contains(p)
                                    && s.opts.get(TICKET_VIEW).is_some_and(|v| v == "1")
                            })
                            .map(|(p, _)| *p)
                            .collect();
                        for view in shown {
                            match &chosen {
                                Some(c) => server.read_view_again(view, Some(c.clone())),
                                // Let go, the view closes as M-w closes a pane,
                                // a shell in its place when it is the tab's last.
                                None => {
                                    server.close_ticket_view(view);
                                    if let Some(slot) = server.panes.get_mut(&view) {
                                        slot.opts.remove(TICKET_VIEW);
                                    }
                                }
                            }
                        }
                        if !parked.is_empty() {
                            let again = chosen.clone().or_else(|| {
                                let dir = server.tab_path(tab)?.to_path_buf();
                                let t = server.tabs.get_mut(&tab)?;
                                t.opts.remove(TAB_TICKET);
                                server
                                    .tab_ticket_at(tab, &dir)
                                    .filter(|t| settings::is_on(t.service()))
                            });
                            // With nothing to show, a parked view is left to
                            // its tool, never a shell standing in its marks.
                            if again.is_some() {
                                for view in parked {
                                    server.read_view_again(view, again.clone());
                                }
                            }
                        }
                        if automatic {
                            if let Some(t) = server.tabs.get_mut(&tab) {
                                t.opts.remove(TAB_TICKET);
                            }
                            server.dirty_tab(tab);
                            let n = server.tab_number(tab);
                            server.say(
                                Some(client),
                                &format!(
                                    "Tab {n}'s ticket comes from its branch or its name again"
                                ),
                            );
                            return;
                        }
                        let Some(chosen) = chosen else {
                            return;
                        };
                        let Some(t) = server.tabs.get_mut(&tab) else {
                            return;
                        };
                        t.opts.insert(TAB_TICKET.into(), chosen.arg());
                        server.dirty_tab(tab);
                        let pane = server
                            .tabs
                            .get(&tab)
                            .map(|t| server.work_pane(tab, t.active));
                        let replaced = was_open.is_some_and(|v| server.panes.contains_key(&v));
                        match pane {
                            Some(pane) if !replaced && (open || was_open.is_some()) => {
                                server.open_ticket_view(pane, chosen)
                            }
                            _ => server.say(
                                Some(client),
                                &format!(
                                    "Tab {}'s ticket is {}",
                                    server.tab_number(tab),
                                    chosen.shown()
                                ),
                            ),
                        }
                    })
                });
            },
        );
    }

    /// Says something on `client` from off the task.
    fn say_later(tx: &tokio::sync::mpsc::UnboundedSender<Event>, client: ClientId, text: String) {
        let _ = tx.send(Event::Views(ViewsEvent::Say { client, text }));
    }

    /// `open_link` from a task off the server's.
    fn open_link_later(
        tx: &tokio::sync::mpsc::UnboundedSender<Event>,
        client: ClientId,
        url: String,
    ) {
        let _ = tx.send(Event::Views(ViewsEvent::Open { client, url }));
    }

    /// The one way the server opens a link (src/links.rs): where it is going
    /// said on the client ("Opening github.com in Chrome · Work…") until the
    /// browser comes to the front, since a profile's Chrome can take seconds
    /// to start; then the message closes, or says the browser hasn't shown
    /// it yet, or why it couldn't open. With link_opening_message off, it
    /// opens with nothing said. Off a runtime (a unit test), nothing opens.
    pub(crate) fn open_link(&mut self, client: ClientId, url: String) {
        if !links::says_it() {
            links::open_in_background(url);
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let tx = self.tx.clone();
        runtime.spawn_blocking(move || {
            let words = links::opening_words(&url);
            let _ = tx.send(Event::Views(ViewsEvent::Opening {
                client,
                words: words.clone(),
            }));
            let shown = links::open_url_watched(&url).map_err(|e| e.to_string());
            let _ = tx.send(Event::Views(ViewsEvent::Opened {
                client,
                words,
                shown,
            }));
        });
    }

    /// A link has opened: its message closed while it is still the one
    /// shown, else left to what took its place.
    fn link_opened(&mut self, client: ClientId, words: &str, shown: Result<bool, String>) {
        if !self.clients.contains_key(&client) {
            return;
        }
        let shown_now = self.message_shown(client);
        // The status line shows it with its spaces run together.
        let one_line = |t: &str| t.split_whitespace().collect::<Vec<_>>().join(" ");
        let ours = shown_now.as_deref().map(one_line) == Some(one_line(words));
        let nothing_else = ours || shown_now.is_none();
        match shown {
            Ok(true) if ours => self.unsay(client),
            Ok(true) => {}
            Ok(false) if nothing_else => {
                let browser = if words.contains(" in Chrome") {
                    "Chrome"
                } else {
                    "The browser"
                };
                self.say_for(
                    Some(client),
                    &format!("{words}\n\n{browser} hasn't shown it yet."),
                    60,
                );
            }
            Ok(false) => {}
            Err(why) => self.say(Some(client), &format!("Couldn't open it.\n\n{why}")),
        }
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
                    // In gh-open.sh's words rather than GitHub's.
                    Err(e) if net::kind_of(&e) == Some(&ErrorKind::NotFound) => {
                        let at = github::repo_of(&dir)
                            .await
                            .map(|r| format!("github.com/{r}"))
                            .unwrap_or_else(|_| "the remote".into());
                        Err(format!(
                            "GitHub has no repository at {at},\nor you can't see it with the token Switchboard signs in with."
                        ))
                    }
                    Err(e) => Err(e.to_string()),
                }
            };
            match url {
                Ok(url) => Server::open_link_later(&tx, client, url),
                Err(why) => Server::say_later(&tx, client, format!("Can't open {what}.\n\n{why}")),
            }
        });
    }

    /// M-l: the tab's ticket in the browser: its GitHub issue at once, its
    /// Linear ticket or project where Linear says it is.
    fn open_ticket(&mut self, client: ClientId) {
        let Some((pane, tab, dir)) = self.here(client) else {
            return;
        };
        let dir = self.work_dir(tab, pane, dir);
        // The ticket a view shows, so M-l opens what is on the screen: the
        // focused pane's (a ticket view, or a link view reading a ticket),
        // else the tab's ticket view.
        let shown_in = |view: PaneId| {
            let arg = self.panes.get(&view)?.opts.get(TICKET_ID)?;
            Ticket::parse(arg).filter(|t| settings::is_on(t.service()))
        };
        // A link view reading a ticket opens the link it reads, in the
        // workspace it names.
        let link_view = self
            .panes
            .get(&pane)
            .is_some_and(|s| s.opts.get(LINK_VIEW).is_some_and(|v| v == "1"));
        if link_view
            && shown_in(pane).is_some()
            && let Some(url) = self
                .panes
                .get(&pane)
                .and_then(|s| s.opts.get(LINK_URL))
                .cloned()
        {
            self.open_link(client, url);
            return;
        }
        // The tab's ticket view counts only while it is on screen: a
        // zoomed tab shows its pane in front alone.
        let zoomed = self.tabs.get(&tab).is_some_and(|t| t.zoomed);
        let shown = shown_in(pane).or_else(|| {
            self.ticket_views_shown(tab)
                .into_iter()
                .filter(|_| !zoomed)
                .find_map(shown_in)
        });
        let Some(found) = shown.or_else(|| self.tab_ticket_at(tab, &dir)) else {
            self.say(Some(client), "No ticket or GitHub issue for this tab");
            return;
        };
        if !settings::is_on(found.service()) {
            self.service_off(client, found.service());
            return;
        }
        match found {
            Ticket::Linear(id) => self.open_ticket_id(client, id),
            Ticket::Issue { .. } => {
                if let Some(url) = found.url() {
                    self.open_link(client, url);
                }
            }
            Ticket::Project(id) => {
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    match super::project::url(&id).await {
                        Ok(Some(url)) => Server::open_link_later(&tx, client, url),
                        Ok(None) => {
                            Server::say_later(&tx, client, format!("Linear has no link for {id}."))
                        }
                        Err(e) => {
                            Server::say_later(&tx, client, format!("Can't open {id}.\n\n{e}"))
                        }
                    }
                });
            }
        }
    }

    /// A Linear ticket by its ID in the browser: at the workspace's link
    /// when one is written down, else where Linear says it is (M-l, and
    /// M-p's "Open a Linear ticket in the browser by ID").
    pub(crate) fn open_ticket_id(&mut self, client: ClientId, id: String) {
        if let Some(workspace) = linear_workspace() {
            self.open_link(client, format!("https://linear.app/{workspace}/issue/{id}"));
            return;
        }
        // No workspace written down: Linear says where the ticket is.
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match linear::ticket(&id, "M-l open ticket").await {
                Ok(Some(t)) if !t.url.is_empty() => Server::open_link_later(&tx, client, t.url),
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

    /// M-p's "Copy the branch's ticket ID" and, with `url`, its link: the
    /// tab's ticket as M-i finds it, a GitHub issue as `#12` and its link,
    /// a Linear ticket's link at the workspace written down, else where
    /// Linear says the ticket is.
    fn copy_ticket(&mut self, client: ClientId, url: bool) {
        let Some((_, tab, dir)) = self.here(client) else {
            return;
        };
        let id = match self.tab_ticket_at(tab, &dir) {
            Some(Ticket::Linear(id)) => id,
            Some(issue @ Ticket::Issue { .. }) => {
                let text = match issue.url() {
                    Some(link) if url => link,
                    _ => issue.short(),
                };
                self.copy_said(client, &text);
                return;
            }
            // A project, or none of the provider's: the Linear ticket the
            // branch or the tab's names give, as before there were issues.
            _ => match self.ticket_here(tab, &dir) {
                Some(id) => id,
                None => {
                    self.say(Some(client), "No ticket in this branch or tab name");
                    return;
                }
            },
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
                Ok(url) => Server::open_link_later(&tx, client, url),
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
                    Server::open_link_later(&tx, client, url);
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
        self.about_tab(client, tab, move |s| {
            s.open(
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
            )
        });
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
        // A review tab's Claude works from the main clone, and the PR is
        // its author's to rebase: nothing here is the branch to resolve.
        let reviewing = self
            .tabs
            .get(&tab)
            .and_then(|t| t.opts.get(crate::mux::server::TAB_DIR))
            .is_some_and(|d| crate::trust::in_review_dir(std::path::Path::new(d)));
        if reviewing {
            self.say(
                Some(client),
                "This tab reviews someone else's PR, whose branch is its author's: its conflicts are theirs to resolve, and its Claude works from your main clone.",
            );
            return;
        }
        let panes = self
            .tabs
            .get(&tab)
            .map(|t| t.layout.panes())
            .unwrap_or_default();
        match panes.into_iter().find(|p| self.runs_claude(*p)) {
            Some(pane) => {
                self.type_into_claude(
                    pane,
                    vec![crate::mission::answer::Press::Text(
                        "/resolve-conflicts".into(),
                    )],
                );
                self.focus_pane(pane);
            }
            None => self.say(Some(client), "No Claude in this tab to resolve them."),
        }
    }

    /// M-S-f: M-F's box (`SearchBox`), its chips as the last M-F left
    /// them; `typed` is a scope's prefix the palette's per-source commands
    /// start it with. Enter on a row reads it beside the pane the search
    /// started from, and Ctrl-O opens a web link in the browser instead,
    /// as search.sh's did. A tab chosen is gone to, Ctrl-O bringing up copy
    /// mode on its match.
    fn search_everything(&mut self, client: ClientId, typed: &str) {
        if !settings::is_on("search_everything") {
            self.say(
                Some(client),
                "Search everything is off in Settings (Integrations).",
            );
            return;
        }
        let pane = self.current_pane(client);
        let caps = self
            .clients
            .get(&client)
            .is_some_and(|c| crate::mux::graphics::round_ends(&c.term));
        let search_box = SearchBox::new(
            client,
            self.tx.clone(),
            pane,
            search::available(),
            &search::read_off(),
            typed,
            caps,
        )
        .with_arrows(settings::is_on("search_arrows"))
        .with_preview(settings::is_on("search_preview"), self.side_resize_keys());
        let then: crate::ui::Then = Box::new(move |server, client, outcome| {
            let (picked, browser) = match outcome {
                Outcome::Picked(p) => (p, false),
                Outcome::PickedAlso(p) => (p, true),
                _ => return,
            };
            let typed = picked.query.trim().to_lowercase();
            if let Some(at) = search::tab_of(&picked.value) {
                match (at.pane, at.said, browser) {
                    (_, Some(said), false) => server.history_at(client, at.tab, said),
                    (Some(pane), None, true) => {
                        server.mission_go_to_match(client, at.tab, pane, &typed)
                    }
                    _ => server.mission_go(client, at.tab),
                }
                return;
            }
            let web = picked.value.starts_with("https://") || picked.value.starts_with("http://");
            // What can't be read here (a Slack message) opens instead.
            if (browser && web)
                || page::kind_of(&picked.value, &page::notes_dir()) == Ok(page::Kind::Slack)
            {
                server.open_link(client, picked.value);
                return;
            }
            let pane = pane
                .filter(|p| server.panes.contains_key(p))
                .or_else(|| server.current_pane(client));
            if let Some(pane) = pane {
                server.read_beside(pane, &picked.value);
            }
        });
        self.open(client, Box::new(search_box), Some(then));
    }

    /// The keys bound to moving a pane's divider left or right, each with
    /// its columns (right positive), for a box with panes side by side.
    fn side_resize_keys(&self) -> Vec<(crate::mux::input::Key, isize)> {
        use crate::mux::Direction;
        use crate::mux::keys::Action;
        self.bindings
            .root
            .iter()
            .filter_map(|(key, action)| match action {
                Action::Resize(Direction::Left, n) => Some((*key, -(*n as isize))),
                Action::Resize(Direction::Right, n) => Some((*key, *n as isize)),
                _ => None,
            })
            .collect()
    }

    /// Asks each source M-F's box names for `query`, each answering with
    /// `ViewsEvent::Found` as it is done; a source whose setting is off is
    /// sent nothing, whatever the box asks.
    fn search_ask(
        &mut self,
        client: ClientId,
        id: u64,
        query: String,
        sources: Vec<Source>,
        pane: Option<PaneId>,
    ) {
        let on = search::available();
        // The repositories the tabs are in, for GitHub's.
        let mut dirs: Vec<PathBuf> = Vec::new();
        if sources.contains(&Source::GitHub) {
            for p in self.panes.keys() {
                if let Some(dir) = self.pane_dir(*p)
                    && !dirs.contains(&dir)
                {
                    dirs.push(dir);
                }
            }
        }
        for source in sources.into_iter().filter(|s| on.contains(s)) {
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
                    context: search::Context::default(),
                }]
            };
            match source {
                Source::Tabs => {
                    let (open, copies) = self.tabs_to_search(pane);
                    let text = q.trim().to_lowercase();
                    let files: Vec<PathBuf> = open
                        .iter()
                        .filter_map(|t| Some(t.claude.as_ref()?.1.clone()))
                        .collect();
                    tokio::task::spawn_blocking(move || {
                        let found = crate::mission::pane_search::search(&copies, &text);
                        let said = super::conversation::search(&files, &text);
                        send(search::tabs(&open, &text, &found, &said, util::now_secs()))
                    });
                }
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

    /// A tab's Claude pane and its session's transcript, as the hooks or
    /// the sidebar's last read named it, the tab's active pane first.
    fn transcript_in(&self, tab: TabId) -> Option<(PaneId, PathBuf)> {
        let active = self.tabs.get(&tab).map(|t| t.active);
        self.tab_panes(tab)
            .into_iter()
            .filter_map(|p| {
                let file = self
                    .history
                    .get(p)
                    .and_then(|r| r.transcript.clone())
                    .or_else(|| self.sidebar.panes.get(&p)?.transcript.clone())?;
                Some((p, file))
            })
            .max_by_key(|(p, _)| Some(*p) == active)
    }

    /// What search everything's Tabs source reads, copied on the server's
    /// task to be searched off it: the open tabs in the order they are
    /// listed, the one `from` is in marked, but not the Notifications
    /// window, with `tab_search_conversation` on each one's Claude
    /// transcript, and with `tab_search_panes` on their panes' screens and
    /// the last `tab_search_lines` of their scrollback, as M-j's picker
    /// reads them.
    fn tabs_to_search(
        &self,
        from: Option<PaneId>,
    ) -> (
        Vec<search::OpenTab>,
        Vec<crate::mission::pane_search::Copied>,
    ) {
        let here = from.and_then(|p| self.panes.get(&p)).map(|s| s.tab);
        let conversations = settings::is_on("tab_search_conversation");
        let open: Vec<search::OpenTab> = self
            .sessions
            .iter()
            .flat_map(|s| s.tabs.iter().copied())
            .filter(|t| self.tab_number(*t) != 0)
            .map(|tab| search::OpenTab {
                tab,
                number: self.tab_number(tab),
                label: self.tab_label(tab),
                here: Some(tab) == here,
                claude: conversations.then(|| self.transcript_in(tab)).flatten(),
            })
            .collect();
        if !settings::is_on("tab_search_panes") {
            return (open, Vec::new());
        }
        let lines = crate::mission::pane_search::lines_setting(
            settings::cached("tab_search_lines").as_ref(),
        );
        let copies = self
            .searchable_panes()
            .filter(|(_, slot)| open.iter().any(|t| t.tab == slot.tab))
            .map(|(pane, slot)| crate::mission::pane_search::Copied {
                tab: slot.tab,
                pane,
                lines: std::sync::Arc::new(
                    slot.pane.text(lines).lines().map(str::to_string).collect(),
                ),
            })
            .collect();
        (open, copies)
    }

    fn search_found(&mut self, client: ClientId, id: u64, source: Source, rows: Vec<Found>) {
        if let Some(c) = self.clients.get_mut(&client)
            && let Some(b) = c.overlays.find_mut::<SearchBox>()
            && b.answered(id, source, rows)
        {
            c.dirty = true;
        }
    }

    /// A hook from a pane's Claude: a prompt's links are kept for the link
    /// commands (src/views/given.rs), since Claude Code shows pasted text
    /// as "[Pasted text #1]", and a finished turn's are timed, so the
    /// commands list the newest first; a fresh session starts with none.
    pub fn links_hook(&mut self, pane: PaneId, hook: &crate::agent::Hook, payload: &Value) {
        if !hook.agent.is_empty() {
            return;
        }
        let text = |key: &str| payload.get(key).and_then(Value::as_str);
        match hook.event.as_str() {
            "SessionStart" if hook.source != "compact" => self.views.given.forget(pane),
            "UserPromptSubmit" if settings::is_on("prompt_links") => {
                if let Some(prompt) = text("prompt") {
                    self.views.given.took(pane, prompt, util::now_ms());
                }
            }
            "Stop" => {
                if let Some(reply) = text("last_assistant_message") {
                    self.views.given.replied(pane, reply, util::now_ms());
                }
            }
            _ => {}
        }
    }

    /// Whether a pane is the notes app's (`@notes_pane`).
    fn is_notes_pane(&self, pane: PaneId) -> bool {
        self.panes.get(&pane).is_some_and(|slot| {
            slot.opts
                .get("@notes_pane")
                .is_some_and(|v| !v.is_empty() && v != "0")
        })
    }

    /// Whether the pane commands read past the pane they run from: the
    /// note open in the tab from another pane, the tab's other panes from
    /// the notes pane (Settings' `pane_commands_read_note`).
    fn reads_tab(&self) -> bool {
        settings::is_on("pane_commands_read_note")
    }

    /// The tab's panes other than `pane` and the notes pane, while the
    /// pane commands read past their own pane.
    fn other_panes(&self, pane: PaneId, tab: TabId) -> Vec<PaneId> {
        if !self.reads_tab() {
            return Vec::new();
        }
        self.tab_panes(tab)
            .into_iter()
            .filter(|p| *p != pane && !self.is_notes_pane(*p))
            .collect()
    }

    /// The note open in a pane's tab: its name, its text and its folder
    /// (which a picture pasted into it is named from), when a pane of
    /// that tab is the notes pane and the notes app shows a note; nothing
    /// from a note closed or open in another tab, nor, but in the notes
    /// pane itself, with Settings' `pane_commands_read_note` off.
    fn tab_note_text(&self, pane: PaneId) -> Option<(String, String, PathBuf)> {
        if !self.is_notes_pane(pane) && !self.reads_tab() {
            return None;
        }
        let tab = self.panes.get(&pane)?.tab;
        let notes = self
            .tab_panes(tab)
            .into_iter()
            .find(|p| self.is_notes_pane(*p))?;
        let rel = self.shown_in(notes)?;
        let file = crate::notes::store::notes_dir().join(&rel);
        let text = util::read_text(&file)?;
        let folder = file.parent()?.to_path_buf();
        Some((rel.trim_end_matches(".md").to_string(), text, folder))
    }

    /// A pane's own links: its screen's and its scrollback's, newest
    /// first, then those in what you sent its Claude that it doesn't show
    /// (`prompt_links`).
    fn screen_links(&self, pane: PaneId) -> Vec<Shown> {
        let Some(slot) = self.panes.get(&pane) else {
            return Vec::new();
        };
        let (text, hyperlinks) = slot.pane.text_and_links(3000);
        let mut found = shown::links_in(&text, &hyperlinks);
        if settings::is_on("prompt_links") {
            for given in self.views.given.of(pane) {
                if !found.iter().any(|s| s.url == given.url) {
                    found.push(given);
                }
            }
        }
        found
    }

    /// The links a pane shows, newest first (src/views/seen.rs), by when
    /// the pane's Claude (or, from the notes pane, the tab's other panes')
    /// last saw each and when the note open in the tab first held it; the
    /// untimed after them, in the order `listed_links` gives.
    fn pane_links(&mut self, pane: PaneId) -> Result<Vec<Shown>, String> {
        let note = self.tab_note_text(pane);
        if let Some((name, text, _)) = &note {
            let file = crate::notes::store::notes_dir().join(format!("{name}.md"));
            let saved = std::fs::metadata(&file)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(i64::MAX, |d| d.as_millis() as i64)
                .min(util::now_ms());
            self.views.seen.note(name, &shown::note_links(text), saved);
        }
        let found = self.listed_links(pane, &note)?;
        // The panes whose links the list holds: the notes pane lists the
        // tab's others, any other pane its own alone.
        let mut claudes = vec![pane];
        if self.is_notes_pane(pane)
            && note.is_some()
            && let Some(slot) = self.panes.get(&pane)
        {
            claudes.extend(self.other_panes(pane, slot.tab));
        }
        let at = |url: &str| {
            let in_note = note
                .as_ref()
                .and_then(|(name, _, _)| self.views.seen.at(name, url));
            claudes
                .iter()
                .filter_map(|p| self.views.given.at(*p, url))
                .chain(in_note)
                .max()
        };
        Ok(seen::newest_first(found, at))
    }

    /// The links a pane shows: in the notes pane while it shows a note,
    /// the note's, top to bottom, then the tab's other panes'; in any
    /// other pane its own (`screen_links`), then those of the note open
    /// in its tab, marked as the note's. Each once, and why there are none,
    /// when there are none.
    fn listed_links(
        &self,
        pane: PaneId,
        note: &Option<(String, String, PathBuf)>,
    ) -> Result<Vec<Shown>, String> {
        let Some(slot) = self.panes.get(&pane) else {
            return Err("No pane here".into());
        };
        let mut found = Vec::new();
        let add = |found: &mut Vec<Shown>, more: Vec<Shown>| {
            for link in more {
                if !found.iter().any(|s: &Shown| s.url == link.url) {
                    found.push(link);
                }
            }
        };
        if self.is_notes_pane(pane)
            && let Some((name, text, _)) = &note
        {
            add(&mut found, shown::note_links(text));
            for other in self.other_panes(pane, slot.tab) {
                add(&mut found, self.screen_links(other));
            }
            if found.is_empty() {
                return Err(if self.reads_tab() {
                    format!("No links in {name} or this tab")
                } else {
                    format!("No links in {name}")
                });
            }
            return Ok(found);
        }
        add(&mut found, self.screen_links(pane));
        if let Some((_, text, _)) = &note {
            let marked = shown::note_links(text)
                .into_iter()
                .map(|link| Shown {
                    text: if link.text.is_empty() {
                        IN_THE_NOTE.into()
                    } else {
                        format!("{} · {IN_THE_NOTE}", link.text)
                    },
                    url: link.url,
                })
                .collect();
            add(&mut found, marked);
        }
        if found.is_empty() {
            return Err(if note.is_some() {
                "No links in this pane or its note".into()
            } else {
                "No links in this pane".into()
            });
        }
        Ok(found)
    }

    /// A link to read beside the pane: one typed, or one of those the pane
    /// shows (link-view.sh's `pick`); Ctrl-O opens it in the browser.
    fn ask_link(&mut self, client: ClientId) {
        let Some((pane, tab, _)) = self.here(client) else {
            return;
        };
        let shown = self.pane_links(pane).unwrap_or_default();
        let picker = Picker::new("Read a link beside this pane", rows(&shown))
            .named("links")
            .prompt("Read: ")
            .exact()
            .free()
            .also("read it beside", "open in browser");
        self.about_tab(client, tab, move |s| {
            s.pick_also(client, picker, move |server, client, picked| {
                let Some((picked, other)) = picked else {
                    return;
                };
                server.link_picked(client, pane, picked.value.trim(), other);
            })
        });
    }

    /// M-p's "Open a link from this pane in the browser" (`copy` false) and
    /// "Copy a link from this pane" (`copy`), each list's Ctrl-O doing what
    /// link-view.sh's did: reading it beside the pane, and opening it.
    fn pick_link(&mut self, client: ClientId, copy: bool) {
        let Some((pane, tab, _)) = self.here(client) else {
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
        self.about_tab(client, tab, move |s| {
            s.pick_also(client, picker, move |server, client, picked| {
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
            })
        });
    }

    /// The open tabs a pane's text names by number, newest first, each
    /// once, its own tab left out: the pane's screen and scrollback, then
    /// the note open in its tab; in the notes pane, the note first, then
    /// the tab's other panes'.
    fn mentioned_tabs(&self, pane: PaneId) -> Vec<TabId> {
        let Some(slot) = self.panes.get(&pane) else {
            return Vec::new();
        };
        let note = self.tab_note_text(pane).map(|(_, text, _)| text);
        let mut texts: Vec<String> = Vec::new();
        if self.is_notes_pane(pane)
            && let Some(note) = note
        {
            texts.push(note);
            for other in self.other_panes(pane, slot.tab) {
                if let Some(s) = self.panes.get(&other) {
                    texts.push(s.pane.text_and_links(3000).0);
                }
            }
        } else {
            texts.push(slot.pane.text_and_links(3000).0);
            texts.extend(note);
        }
        let mut tabs: Vec<TabId> = Vec::new();
        for text in texts {
            for tab in shown::tabs_in(&text)
                .into_iter()
                .filter_map(|n| self.tab_numbered(n))
            {
                if tab != slot.tab && !tabs.contains(&tab) {
                    tabs.push(tab);
                }
            }
        }
        tabs
    }

    /// M-p's "Go to a tab this pane mentions" (`copy` false) and "Copy the
    /// number of a tab this pane mentions" (`copy`), each list's Ctrl-O
    /// doing the other; with one tab named, going there needs no list.
    fn pick_mentioned_tab(&mut self, client: ClientId, copy: bool) {
        let Some((pane, tab, _)) = self.here(client) else {
            return;
        };
        let tabs = self.mentioned_tabs(pane);
        if tabs.is_empty() {
            return self.say(Some(client), "No tabs named in this pane");
        }
        if let [tab] = tabs[..]
            && !copy
        {
            let n = self.tab_number(tab);
            self.go_to_tab(client, tab);
            let label = self.tab_label(tab);
            return self.say(Some(client), &format!("Went to tab {n} {label}"));
        }
        let items: Vec<PickItem> = tabs
            .iter()
            .map(|t| PickItem {
                off: false,
                text: format!("{} {}", self.tab_number(*t), self.tab_label(*t)),
                tags: self.tab_agent(*t).map(|a| a.name()).unwrap_or("").into(),
                key: String::new(),
                // The tab itself, since a number freed while the list is
                // open is given to the next tab opened.
                value: t.to_string(),
            })
            .collect();
        let (title, prompt, enter, other) = if copy {
            (
                "Copy a tab's number",
                "Copy: ",
                "copy its number",
                "go to it",
            )
        } else {
            ("Go to a tab", "Go to: ", "go to it", "copy its number")
        };
        let picker = Picker::new(title, items)
            .named("mentioned-tabs")
            .prompt(prompt)
            .also(enter, other);
        self.about_tab(client, tab, move |s| {
            s.pick_also(client, picker, move |server, client, picked| {
                let Some((picked, other)) = picked else {
                    return;
                };
                let Some(tab) = picked
                    .value
                    .parse::<TabId>()
                    .ok()
                    .filter(|t| server.tabs.contains_key(t))
                else {
                    return server.say(Some(client), "That tab has closed");
                };
                if copy != other {
                    let n = server.tab_number(tab).to_string();
                    server.to_clipboard(Some(client), &n);
                    server.say(Some(client), &format!("Copied tab {n}"));
                } else {
                    server.go_to_tab(client, tab);
                }
            })
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
                self.open_link(client, url.to_string());
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
    /// After them, those the last turns of the tab's Claude named that the
    /// screen doesn't show, marked as the conversation's, from its
    /// transcript as the sidebar last found it, read off the server's task.
    fn open_media(&mut self, client: ClientId) {
        let Some((pane, tab, dir)) = self.here(client) else {
            return;
        };
        let links = self.pane_links(pane).unwrap_or_default();
        let Some(slot) = self.panes.get(&pane) else {
            return;
        };
        let home = paths::home();
        let note = self.tab_note_text(pane);
        // Where the files are named, each with the folder a relative path
        // starts from: the pane, then the note open in its tab; from the
        // notes pane, the note's Markdown, then the tab's other panes.
        let mut sources = Vec::new();
        match &note {
            Some((_, note, folder)) if self.is_notes_pane(pane) => {
                sources.push((note.clone(), folder.clone()));
                for other in self.other_panes(pane, slot.tab) {
                    if let Some(s) = self.panes.get(&other) {
                        let dir = self.pane_own_dir(other).unwrap_or_else(paths::home);
                        sources.push((s.pane.text(3000), dir));
                    }
                }
            }
            Some((_, note, folder)) => {
                sources.push((slot.pane.text(3000), dir));
                sources.push((note.clone(), folder.clone()));
            }
            None => sources.push((slot.pane.text(3000), dir)),
        }
        let mut found: Vec<Shown> = Vec::new();
        for (text, dir) in &sources {
            for file in shown::media_in(text, dir, &home, &[]) {
                if !found.iter().any(|s| s.url == file.url) {
                    found.push(file);
                }
            }
        }
        found.extend(links.into_iter().filter(|l| shown::is_media(&l.url)));
        let in_note = note.is_some() && self.reads_tab();
        // The tab's Claude pane, whose directory its relative paths are from.
        let claude = self
            .panes_with_parked(tab)
            .into_iter()
            .find(|p| self.runs_claude(*p));
        let transcript = claude
            .and_then(|p| self.sidebar.panes.get(&p))
            .and_then(|read| read.transcript.clone());
        let (true, Some(claude), Some(file)) =
            (settings::is_on("conversation_media"), claude, transcript)
        else {
            return self.pick_media(client, tab, pane, found, in_note);
        };
        let dir = self.pane_own_dir(claude).unwrap_or_else(paths::home);
        self.off_task(
            client,
            move || {
                let words = crate::sidebar::claude::last_words(&file).unwrap_or_default();
                shown::media_in(&words, &dir, &home, &[])
            },
            move |server, client, said| {
                let mut found = found;
                for file in said {
                    if !found.iter().any(|s| s.url == file.url) {
                        found.push(Shown {
                            text: CONVERSATION.into(),
                            url: file.url,
                        });
                    }
                }
                server.pick_media(client, tab, pane, found, in_note);
            },
        );
    }

    /// The media picker over what `open_media` found, or why it found
    /// nothing.
    fn pick_media(
        &mut self,
        client: ClientId,
        tab: TabId,
        pane: PaneId,
        found: Vec<Shown>,
        in_note: bool,
    ) {
        if found.is_empty() {
            let why = if in_note {
                "No image or video in this pane or its note"
            } else {
                "No image or video in this pane"
            };
            return self.say(Some(client), why);
        }
        let picker = Picker::new("Images and videos", rows(&found))
            .named("media")
            .prompt("Open: ")
            .exact()
            .also("open it", "show in its folder");
        self.about_tab(client, tab, move |s| {
            s.pick_also(client, picker, move |server, client, picked| {
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
            })
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
            Some(url) => self.open_link(client, url),
            None => self.say(Some(client), "No link is being read in this tab"),
        }
    }
}
