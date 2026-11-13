//! The server's side of the extras: each command from its key or the
//! palette, and the keys that change what a key does before the bindings
//! see it (`extras_key`).
//!
//! What a popup of tmux.conf's asked for is a box over the workspace now
//! (src/ui): a line typed in an `Ask`, a list in a `Picker`, a choice in a
//! `Menu`, what a script printed in a `TextBox`. What reads more than a
//! small file, starts a process or waits on the network (the clipboard, a
//! transcript, git, setup) runs off the server's task (`off_task`) and
//! comes back with the rest of the work, as the keys that start work do
//! (src/cmd/work.rs).

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::{Command as Process, Stdio};

use serde_json::Value;

use super::text::{TextBox, Tone};
use super::{Command, OPTION_CHARS, PrTyped, TicketTyped};
use crate::mission::ask::Ask;
use crate::mission::{Mission, Opening, ViewId};
use crate::mux::input::{self, Key, KeyCode, Mods};
use crate::mux::keys::Action;
use crate::mux::server::{ClientId, Server};
use crate::mux::{Direction, PaneId, TabId};
use crate::ui::{Menu, MenuOption, Outcome, PickItem, Picker};
use crate::{paths, platform, settings, util, worktree};

/// What the server keeps for the extras: the clients that pressed `C-w`
/// and wait for h, j, k or l (tmux.conf's `window-nav` key table), and what
/// the server found as it started that the first client to attach is told.
#[derive(Default)]
pub struct Extras {
    nav: HashSet<ClientId>,
    on_attach: Vec<String>,
    /// keys.json's modification time when last read, so a change to it
    /// takes effect on the next tick, as a tmux.conf reload did.
    keys_seen: Option<std::time::SystemTime>,
}

/// When keys.json was last changed, if it is there.
fn keys_modified() -> Option<std::time::SystemTime> {
    std::fs::metadata(paths::config_dir().join("keys.json"))
        .and_then(|m| m.modified())
        .ok()
}

/// What a `~/.config/switchboard/tmux.conf` of your own is told: tmux.conf
/// sourced it last, so your bindings and options won; nothing reads it now,
/// and what took its place is said, rather than its lines being ignored
/// without a word.
const OLD_TMUX_CONF: &str = "~/.config/switchboard/tmux.conf isn't read any more: \
     Switchboard is no longer tmux. Your keys go in ~/.config/switchboard/keys.json \
     (the guide's \"Changing the keys\"), and the workspace's options are in Settings \
     (M-p, then Settings).";

/// What becomes of a key before the bindings see it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyFate {
    /// Done with here.
    Taken,
    /// To the pane as it is, past the bindings.
    ToPane,
    /// On to the bindings, as any key.
    Bindings,
}

/// How long the AWS login's pane is, as a share of the pane it is opened
/// beside.
const AWS_PERCENT: u16 = 50;

fn is_off(key: &str) -> bool {
    settings::cached(key) == Some(Value::Bool(false))
}

impl Server {
    /// A key before the bindings: `C-w`'s next key, Alt-arrows with
    /// pane_arrows off, and the characters Option types where it isn't
    /// Alt, which bring their tip once and go on to the pane.
    pub fn extras_key(&mut self, client: ClientId, key: Key) -> KeyFate {
        if self.extras.nav.remove(&client) {
            let dir = match key.code {
                KeyCode::Char('h') => Some(Direction::Left),
                KeyCode::Char('j') => Some(Direction::Down),
                KeyCode::Char('k') => Some(Direction::Up),
                KeyCode::Char('l') => Some(Direction::Right),
                _ => None,
            };
            if let Some(dir) = dir.filter(|_| key.mods.is_empty()) {
                self.act(client, Action::SelectPane(dir));
                return KeyFate::Taken;
            }
            // C-w twice: the pane gets one, which tmux.conf left no way to
            // send. Any other key goes on as if C-w hadn't been pressed, as
            // tmux tried a key its table lacked in the root table.
            if is_pane_nav(&key) {
                return KeyFate::ToPane;
            }
        }
        let arrow = matches!(
            key.code,
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down
        );
        // The diff and Notes take them too, moving between their own windows
        // first, as tmux.conf's is_vim passed them on to Neovim.
        if arrow
            && key.mods == Mods::ALT
            && matches!(self.bindings.root.get(&key), Some(Action::SelectPane(_)))
            && (is_off("pane_arrows")
                || self.pane_takes_keys(client, self.current_pane(client), false))
        {
            return KeyFate::ToPane;
        }
        // M-T showing the title (reopen_tab_key off) with the tab_title
        // setting off too reaches the pane as it is.
        let shows_title = self
            .bindings
            .root
            .get(&key)
            .is_some_and(|a| a.clone().pressed(!is_off("reopen_tab_key")) == Action::TabTitle);
        if shows_title && is_off("tab_title") {
            return KeyFate::ToPane;
        }
        if let KeyCode::Char(c) = key.code
            && key.mods.is_empty()
            && OPTION_CHARS.contains(&c)
            && !self.tip_was_seen("option-alt")
        {
            self.tip("option-alt", Some(client));
        }
        KeyFate::Bindings
    }

    /// What the server found as it started, to say to the first client
    /// that attaches: what keys.json has that couldn't be used, and a
    /// tmux.conf of your own it no longer reads.
    pub fn note_at_start(&mut self, keys_problems: &[String]) {
        self.extras.keys_seen = keys_modified();
        if !keys_problems.is_empty() {
            self.extras.on_attach.push(format!(
                "Some of ~/.config/switchboard/keys.json couldn't be used:\n\n{}",
                keys_problems.join("\n")
            ));
        }
        if paths::config_dir().join("tmux.conf").exists() {
            self.extras.on_attach.push(OLD_TMUX_CONF.to_string());
        }
    }

    /// Reads keys.json again when it has changed (or gone) since it was
    /// last read, and says what of it couldn't be used.
    pub fn reload_keys_if_changed(&mut self) {
        let seen = keys_modified();
        if seen == self.extras.keys_seen {
            return;
        }
        self.extras.keys_seen = seen;
        let (bindings, problems) = crate::mux::keys::Bindings::load();
        self.bindings = bindings;
        if !problems.is_empty() {
            self.say_for(
                None,
                &format!(
                    "Some of ~/.config/switchboard/keys.json couldn't be used:\n\n{}",
                    problems.join("\n")
                ),
                20,
            );
        }
    }

    /// A client attached: what the server had to say as it started, said.
    pub fn extras_attached(&mut self, client: ClientId) {
        let notes = std::mem::take(&mut self.extras.on_attach);
        if !notes.is_empty() {
            self.say_for(Some(client), &notes.join("\n\n"), 20);
        }
    }

    /// A command, from its key or the palette.
    pub fn extras_command(&mut self, client: ClientId, command: Command, pane: Option<PaneId>) {
        for (tag, service) in [("github", "github"), ("linear", "linear")] {
            if command.tags().contains(&tag) && !settings::is_on(service) {
                self.service_off(client, service);
                return;
            }
        }
        match command {
            Command::RenameTab => {
                if let Some(tab) = self.client_tab(client) {
                    self.rename_tab(client, tab);
                }
            }
            Command::GoToTab => self.ask_tab_number(client),
            Command::PaneNav => self.pane_nav(client, pane),
            Command::Mission(view) => self.mission_view(client, view),
            Command::Notifications => self.go_to_notifications(client),
            Command::Habits => {
                let lines = self.habits_report();
                self.open(
                    client,
                    Box::new(TextBox::new("How you use Switchboard", lines)),
                    None,
                );
            }
            Command::SendReply => self.send_reply(client),
            Command::UpdateMain => self.update_main(client, pane),
            Command::UpdateTarget => self.update_target(client, pane),
            Command::CheckSetup => self.run_setup(
                client,
                "Switchboard setup, checked",
                vec!["setup".into(), "--check".into()],
            ),
            Command::SetupPart => self.setup_part(client),
            Command::CalendarTab => self.loop_tab(client, "Calendar", "30m", "/today"),
            Command::SlackTab => self.loop_tab(client, "Slack", "5m", "/slack-updates"),
            Command::OpenPrNumber => self.open_pr_number(client, pane),
            Command::OpenTicketId => self.open_ticket_id_typed(client, pane),
            Command::AwsLogin => {
                if let Some(pane) = pane {
                    self.open_beside(pane, "@aws_login", vec!["aws-login".into()], AWS_PERCENT);
                }
            }
            // With the view off, M-/ is the pane's.
            Command::Keys if is_off("view_keys") => {
                self.key_back_to_pane(client, &Action::Extra(command));
            }
            Command::Keys => self.open_mission(client, Opening::View("keys".into())),
            Command::SidebarRepo | Command::SidebarAll if is_off("sidebar_repo_filter") => {
                self.say(
                    Some(client),
                    "The sidebar repository filter is off in Settings (sidebar_repo_filter).",
                );
            }
            Command::SidebarRepo => self.sidebar_pick_repo(client),
            Command::SidebarSelect if is_off("sidebar_select") => {
                self.say(
                    Some(client),
                    "Selecting several tabs is off in Settings (sidebar_select).",
                );
            }
            Command::SidebarSelect => self.sidebar_select_start(client),
            Command::TabTrail => {
                if let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) {
                    self.sidebar_trail_menu(client, &session);
                }
            }
            Command::MissionViews => self.sidebar_views_menu(client),
            Command::SidebarAll => {
                if let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) {
                    self.sidebar_set_repo(&session, None);
                    self.say(Some(client), "Sidebar: every repository");
                }
            }
        }
    }

    /// A key whose setting is off reaches the pane in front as it is, as
    /// tmux.conf's `send-keys` gave it back: the key pressed alone that is
    /// bound to `action`.
    pub fn key_back_to_pane(&mut self, client: ClientId, action: &Action) {
        let mut keys: Vec<Key> = self
            .bindings
            .root
            .iter()
            .filter(|(_, a)| *a == action)
            .map(|(k, _)| *k)
            .collect();
        keys.sort_by_key(Key::notation);
        if let (Some(key), Some(pane)) = (keys.first().copied(), self.current_pane(client)) {
            self.key_to_pane(pane, key);
        }
    }

    /// The tab in front in the client's session.
    fn client_tab(&self, client: ClientId) -> Option<TabId> {
        let session = &self.clients.get(&client)?.session;
        Some(self.session(session)?.current)
    }

    /// A pane's own directory (never its tab's `@tab_dir`, someone else's
    /// PR, since a tab may start in it), your home when it can't be read.
    fn dir_or_home(&self, pane: Option<PaneId>) -> PathBuf {
        pane.and_then(|p| self.pane_own_dir(p))
            .unwrap_or_else(paths::home)
    }

    /// Writes a key to a pane as the program there reads it.
    fn key_to_pane(&mut self, pane: PaneId, key: Key) {
        if let Some(slot) = self.panes.get_mut(&pane) {
            let bytes = input::encode_key(&key, &slot.pane.modes());
            let _ = slot.pane.write(&bytes);
        }
    }

    // ── M-0 and C-w ─────────────────────────────────────────────────────────

    /// M-0: a tab's number typed, and that tab (tab-goto.sh).
    fn ask_tab_number(&mut self, client: ClientId) {
        let ask = Ask::new("Go to tab", "Its number, for the tabs past 9.");
        self.open(
            client,
            Box::new(ask),
            Some(Box::new(|server: &mut Server, client, outcome| {
                let Outcome::Picked(p) = outcome else {
                    return;
                };
                let Some(n) = super::tab_number(&p.value) else {
                    // 0 is a number, of no tab, as tab-goto.sh said.
                    let said = if p.value.trim() == "0" {
                        "There is no tab 0.".to_string()
                    } else {
                        format!("Not a tab number: {}", p.value)
                    };
                    server.say(Some(client), &said);
                    return;
                };
                if server.tab_numbered(n).is_none() {
                    server.say(Some(client), &format!("There is no tab {n}."));
                    return;
                }
                server.act(client, Action::SelectTab(n));
            })),
        );
    }

    /// Whether the pane in front takes `C-w` itself (Vim, the diff, Notes),
    /// or with `vim` false the M-arrows (the diff, Notes), with the sidebar
    /// not holding the focus.
    fn pane_takes_keys(&self, client: ClientId, pane: Option<PaneId>, vim: bool) -> bool {
        let sidebar = self
            .clients
            .get(&client)
            .is_some_and(|c| self.sidebar.focused(&c.session));
        let Some(slot) = pane.and_then(|p| self.panes.get(&p)).filter(|_| !sidebar) else {
            return false;
        };
        if vim
            && slot
                .pane
                .foreground_command()
                .is_some_and(|c| super::is_vim(&c))
        {
            return true;
        }
        let args = slot.pane.foreground_args().unwrap_or_default();
        super::takes_pane_keys(&args)
    }

    /// C-w: the next key moves between panes, unless Vim, the diff or Notes
    /// is in front, which takes C-w for its own windows, or the
    /// ctrl_w_panes setting is off.
    fn pane_nav(&mut self, client: ClientId, pane: Option<PaneId>) {
        let sidebar = self
            .clients
            .get(&client)
            .is_some_and(|c| self.sidebar.focused(&c.session));
        let takes = self.pane_takes_keys(client, pane, true);
        if !sidebar && (takes || is_off("ctrl_w_panes")) {
            if let Some(pane) = pane {
                self.key_to_pane(pane, ctrl_w());
            }
            return;
        }
        self.extras.nav.insert(client);
    }

    // ── Mission control ─────────────────────────────────────────────────────

    /// Mission control on one of its views; said when that view is off.
    fn mission_view(&mut self, client: ClientId, view: ViewId) {
        if view == ViewId::Settings {
            self.open_mission(client, Opening::Settings);
            return;
        }
        // A plugin's view goes by the title its plugin gives it now.
        let slug = match &view {
            ViewId::Plugin(name) => self
                .plugin_views()
                .into_iter()
                .find(|p| p.name == *name)
                .map_or_else(|| view.slug(), |p| p.title.to_lowercase().replace(' ', "-")),
            _ => view.slug(),
        };
        self.open_mission(client, Opening::View(slug.clone()));
        let shown = self
            .clients
            .get(&client)
            .and_then(|c| c.overlays.find::<Mission>())
            .and_then(|m| m.current().map(|v| v.slug()));
        if shown.as_deref() != Some(slug.as_str()) {
            self.mission_note(
                client,
                format!(
                    "{} is off in Settings, so mission control has no view of it.",
                    view.title()
                ),
            );
        }
    }

    /// "Go to the notifications", as `sidebar.sh notices` did: the
    /// sidebar's cursor onto the first of them, which brings the
    /// Notifications window to the front with notice_list on.
    fn go_to_notifications(&mut self, client: ClientId) {
        self.sidebar_to_notices(client);
    }

    // ── Claude's last reply ─────────────────────────────────────────────────

    /// "Send Claude's last reply to another tab" (reply-send.sh): the reply
    /// read from the tab's transcript, a tab picked, and the reply pasted
    /// into that tab's Claude (its active pane when it runs none; brought
    /// beside a tool standing in its place) without Enter, as a bracketed paste so its line breaks stay in the text, and
    /// that tab brought to the front, to add to it before sending.
    fn send_reply(&mut self, client: ClientId) {
        let Some(here) = self.client_tab(client) else {
            return;
        };
        let tabs = self.sidebar_all_tabs();
        let Some(session) = tabs
            .iter()
            .find(|t| t.id == here)
            .and_then(|t| t.claude.as_ref())
            .map(|c| c.session_id.clone())
        else {
            self.say(Some(client), "This tab isn't running Claude.");
            return;
        };
        let others: Vec<PickItem> = tabs
            .iter()
            .filter(|t| t.id != here)
            .map(|t| {
                let label = match t.pr {
                    Some(pr) => format!("#{pr} {}", t.name),
                    None => t.name.clone(),
                };
                PickItem {
                    text: format!("{}  {label}", t.index),
                    tags: t.session.clone(),
                    value: t.id.0.to_string(),
                    ..PickItem::default()
                }
            })
            .collect();
        if others.is_empty() {
            self.say(Some(client), "There is no other tab to send it to.");
            return;
        }
        self.off_task(
            client,
            move || {
                crate::cmd::claude_reply::transcript_of(&session)
                    .and_then(|f| crate::cmd::claude_reply::last_said(&f))
            },
            move |server, client, reply| {
                let Some(reply) = reply.map(|r| r.trim().to_string()) else {
                    server.say(Some(client), "Claude hasn't replied in this tab yet.");
                    return;
                };
                let preview: String = worktree::one_line(&reply).chars().take(70).collect();
                let picker = Picker::new(format!("Send the last reply to: {preview}…"), others)
                    .named("send-reply")
                    .prompt("send to> ");
                server.pick(client, picker, move |server, client, picked| {
                    let Some(tab) = picked.and_then(|p| p.value.parse().ok()).map(TabId) else {
                        return;
                    };
                    server.paste_into_tab(client, tab, &reply);
                });
            },
        );
    }

    /// Text pasted into a tab's Claude (its active pane when it runs none)
    /// without Enter, and the tab brought to the front: Claude's last reply
    /// sent here, and a notification's "Send it to a tab".
    pub(crate) fn paste_into_tab(&mut self, client: ClientId, tab: TabId, reply: &str) {
        let model = self.sidebar_all_tabs().into_iter().find(|t| t.id == tab);
        let Some(model) = model else {
            self.say(Some(client), "That tab has closed.");
            return;
        };
        let Some(pane) = model.claude_pane.or(model.active) else {
            return;
        };
        // A Claude behind a tool in its pane's place (the diff, lazygit, a
        // view) comes back beside it first, as the history view brings it,
        // so the reply is pasted where you can see it and add to it.
        if self.parked(pane) {
            self.unpark_beside(pane);
        }
        if self.parked(pane) {
            self.say(
                Some(client),
                "That tab's Claude is behind a tool that couldn't make room for it, so nothing was pasted there.",
            );
            return;
        }
        let paste = vec![crate::mission::answer::Press::Paste(reply.to_string())];
        if model.claude_pane.is_some() {
            // Into Claude's prompt, `i` first in vim's Normal mode.
            self.type_into_claude(pane, paste);
        } else {
            self.press(pane, paste);
        }
        self.select_tab(&model.session, tab);
        self.focus_pane(pane);
        // A tab in another session brings this client there, as going to
        // a tab from mission control does.
        if self
            .clients
            .get(&client)
            .is_some_and(|c| c.session != model.session)
        {
            self.switch_client(client, &model.session);
        }
    }

    // ── Git and setup ───────────────────────────────────────────────────────

    /// "Update local main to origin's" (update-main.sh), in the pane's
    /// repository: said as it starts, since the fetch takes a while, and
    /// what it did in a box until a key.
    fn update_main(&mut self, client: ClientId, pane: Option<PaneId>) {
        let dir = self.dir_or_home(pane);
        self.say_for(Some(client), "Updating main from origin…", 60);
        self.off_task(
            client,
            move || crate::cmd::update_main::update(&dir),
            |server, client, done| {
                server.close_overlays(client, "message");
                let text = match done {
                    Ok(said) => said,
                    Err(why) => format!("update-main: {why}"),
                };
                server.open(client, Box::new(TextBox::plain("Update main", &text)), None);
            },
        );
    }

    /// "Bring a target branch up to date with the default branch": one of
    /// the pane's repository's local branches, read from git's files off the
    /// server's task, then
    /// the tab running `switchboard update-target` for it in the main clone
    /// (src/cmd/update_target.rs), or the one already running it brought to
    /// the front.
    fn update_target(&mut self, client: ClientId, pane: Option<PaneId>) {
        let dir = self.dir_or_home(pane);
        let Some(place) = crate::tabs::git::locate(&dir) else {
            self.say(Some(client), "This pane isn't in a git repository.");
            return;
        };
        let Some(root) = place.common_dir.parent().map(PathBuf::from) else {
            return;
        };
        // A repository's refs can be many files, and finding its default
        // branch may read them all: both off the task.
        let common = place.common_dir.clone();
        self.off_task(
            client,
            move || {
                let default = crate::tabs::git::default_branch(&common);
                (default, crate::tabs::git::local_branches(&common))
            },
            move |server, client, (default, branches)| {
                let Some(default) = default else {
                    server.say(
                        Some(client),
                        "The repository's default branch isn't known (git remote set-head origin -a).",
                    );
                    return;
                };
                server.pick_update_target(client, root, default, branches);
            },
        );
    }

    /// The branch to bring up to date, picked from `branches`, then its tab.
    fn pick_update_target(
        &mut self,
        client: ClientId,
        root: PathBuf,
        default: String,
        mut branches: Vec<String>,
    ) {
        branches.retain(|b| *b != default);
        branches.sort();
        if branches.is_empty() {
            self.say(
                Some(client),
                &format!("This repository has no local branch but {default}."),
            );
            return;
        }
        let items = branches
            .iter()
            .map(|b| PickItem::new(b.clone(), b.clone()))
            .collect();
        let picker = Picker::new(format!("Which branch takes {default}'s work?"), items)
            .named("update-target")
            .prompt("branch> ");
        self.pick(client, picker, move |server, client, picked| {
            let Some(branch) = picked.map(|p| p.value) else {
                return;
            };
            use crate::cmd::update_target::{TAB_OPTION, live, tab_command, tab_mark, tab_name};
            let mark = tab_mark(&root, &branch);
            let marked: Vec<TabId> = server
                .tabs
                .iter()
                .filter(|(_, t)| t.opts.get(TAB_OPTION) == Some(&mark))
                .map(|(id, _)| *id)
                .collect();
            let running = marked.iter().copied().find(|tab| {
                let commands: Vec<String> = server.tabs[tab]
                    .layout
                    .panes()
                    .into_iter()
                    .filter_map(|p| server.panes.get(&p)?.pane.foreground_command())
                    .collect();
                live(&root, &branch, &commands)
            });
            if let Some(tab) = running {
                server.front(client, &[tab]);
                return;
            }
            for tab in marked {
                if let Some(t) = server.tabs.get_mut(&tab) {
                    t.opts.remove(TAB_OPTION);
                }
            }
            let opened = crate::mux::server::Opened::Front;
            if let Some(tab) = server.work_tab(client, &root, tab_command(&branch), opened) {
                if let Some(t) = server.tabs.get_mut(&tab) {
                    t.opts.insert(TAB_OPTION.into(), mark);
                    t.opts
                        .insert("@tab_name".into(), tab_name(&branch, &default));
                }
                server.dirty_tab(tab);
                server.front(client, &[tab]);
            }
        });
    }

    /// `switchboard <args>` run off the server's task, what it printed shown
    /// in a box titled `title` (repo-update.sh's `run-setup-check` and
    /// `run-setup-part`). It asks nothing: a part named, or `--check`,
    /// never does, and its input is closed, which any question reads as no.
    fn run_setup(&mut self, client: ClientId, title: &str, args: Vec<String>) {
        let title = title.to_string();
        self.say_for(Some(client), "Running Switchboard's setup…", 60);
        self.off_task(
            client,
            move || {
                let exe = util::exe().map_err(|e| e.to_string())?;
                let out = Process::new(exe)
                    .args(&args)
                    .stdin(Stdio::null())
                    .output()
                    .map_err(|e| e.to_string())?;
                let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
                text.push_str(&String::from_utf8_lossy(&out.stderr));
                Ok::<String, String>(text)
            },
            move |server, client, done| {
                server.close_overlays(client, "message");
                let text = done.unwrap_or_else(|why| format!("Couldn't run the setup: {why}"));
                let mut lines = vec![vec![(String::new(), Tone::Plain)]];
                lines.extend(
                    text.trim_end()
                        .lines()
                        .map(|l| vec![(format!(" {l}"), Tone::Plain)]),
                );
                server.open(client, Box::new(TextBox::new(title, lines)), None);
            },
        );
    }

    /// "Run a part of Switchboard's setup": the parts `--parts` lists, then
    /// whether to run it or only check it.
    fn setup_part(&mut self, client: ClientId) {
        let mut rows = vec![PickItem::new("Every part", "")];
        rows.extend(crate::setup::PARTS.iter().map(|p| PickItem {
            text: p.name.to_string(),
            tags: p.about.to_string(),
            value: p.name.to_string(),
            ..PickItem::default()
        }));
        // Tab marks several, as repo-update.sh's fzf --multi did.
        let picker = Picker::new("A part of Switchboard's setup", rows)
            .named("setup-part")
            .prompt("part> ")
            .multi();
        self.pick_many(client, picker, |server, client, picked| {
            if picked.is_empty() {
                return;
            }
            let every = picked.iter().any(|p| p.value.is_empty());
            let parts: Vec<String> = if every {
                crate::setup::PARTS
                    .iter()
                    .map(|p| p.name.to_string())
                    .collect()
            } else {
                picked.iter().map(|p| p.value.clone()).collect()
            };
            let named = if every {
                "every part".to_string()
            } else {
                parts.join(", ")
            };
            let menu = Menu::new(
                format!("Switchboard setup: {named}"),
                vec![
                    MenuOption::new("Run it"),
                    MenuOption::new("Only check what it would change"),
                ],
            );
            server.choose(client, menu, move |server, client, chose| {
                let Some(chose) = chose else {
                    return;
                };
                let mut args = vec!["setup".to_string()];
                if chose == 1 {
                    args.push("--check".into());
                }
                args.extend(parts);
                let title = if chose == 1 {
                    format!("Switchboard setup: {named}, checked")
                } else {
                    format!("Switchboard setup: {named}")
                };
                server.run_setup(client, &title, args);
            });
        });
    }

    // ── The loop tabs ───────────────────────────────────────────────────────

    /// "Open a Calendar tab" and "Open a Slack tab": the tab the Calendar or
    /// Slack view opens in the background when it is out of date, opened in
    /// front (loop-tab.sh without `--background`).
    fn loop_tab(&mut self, client: ClientId, name: &str, every: &str, skill: &str) {
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        if let Err(e) = self.start_loop_tab(
            &session,
            name,
            every,
            skill,
            crate::mux::server::Opened::Front,
        ) {
            self.say(Some(client), &format!("Couldn't open a {name} tab: {e}"));
        }
    }

    // ── A PR or a ticket by number ──────────────────────────────────────────

    /// "Open a PR in the browser by number or URL" (pr-open.zsh): typed,
    /// the clipboard's when it holds one; empty is this tab's PR, as M-r
    /// opens it. A number is a PR of the pane's repository as `wtpr` takes
    /// a bare number (`worktree_from_pr::numbered_repo`, worked out off the
    /// server's task): `gh repo set-default`'s, else in a fork's clone the
    /// project its `upstream` remote names, else origin's. Then the browser,
    /// or a new tab working on it (`wtpr`).
    fn open_pr_number(&mut self, client: ClientId, pane: Option<PaneId>) {
        let dir = self.dir_or_home(pane);
        self.off_task(
            client,
            || {
                let clipboard = platform::paste();
                super::prefill(clipboard.as_deref(), |l| super::pr_typed(l).is_some())
            },
            move |server, client, prefill| {
                let ask = Ask::new(
                    "Open a PR",
                    "A PR number or URL; empty for this tab's PR, as M-r opens it.",
                )
                .text(prefill)
                .empty_ok();
                server.open(
                    client,
                    Box::new(ask),
                    Some(Box::new(move |server: &mut Server, client, outcome| {
                        let Outcome::Picked(p) = outcome else {
                            return;
                        };
                        server.pr_typed_in(client, &p.value, dir);
                    })),
                );
            },
        );
    }

    fn pr_typed_in(&mut self, client: ClientId, typed: &str, dir: PathBuf) {
        if typed.trim().is_empty() {
            self.view_command(client, crate::views::Command::OpenPr);
            return;
        }
        match super::pr_typed(typed) {
            Some(PrTyped::Url { url, number }) => self.pr_menu(client, url, number, dir),
            Some(PrTyped::Number(number)) => {
                let root = dir.clone();
                self.off_task(
                    client,
                    move || crate::cmd::worktree_from_pr::numbered_repo(&root),
                    move |server, client, repo| match repo {
                        Ok(repo) => {
                            let url = format!("https://github.com/{repo}/pull/{number}");
                            server.pr_menu(client, url, number, dir);
                        }
                        Err(why) => server.say(
                            Some(client),
                            &format!(
                                "PR {number} in {}: {why}. Paste the PR's URL instead.",
                                paths::tilde(&dir)
                            ),
                        ),
                    },
                );
            }
            None => self.say(
                Some(client),
                &format!("Not a PR number or GitHub PR URL: {}", typed.trim()),
            ),
        }
    }

    fn pr_menu(&mut self, client: ClientId, url: String, number: u64, dir: PathBuf) {
        let menu = Menu::new(
            format!("PR #{number}"),
            vec![
                MenuOption::new("Open it in the browser"),
                MenuOption::new("Work on it in a new tab (wtpr)"),
            ],
        );
        self.choose(client, menu, move |server, client, chose| match chose {
            Some(0) => server.open_link(client, url),
            Some(_) => {
                let command = worktree::tab_running("worktree-from-pr", "wtpr", &[url]);
                if let Some(tab) =
                    server.work_tab(client, &dir, command, crate::mux::server::Opened::Front)
                {
                    server.front(client, &[tab]);
                }
            }
            None => {}
        });
    }

    /// "Open a Linear ticket in the browser by ID, number or URL"
    /// (ticket-open.zsh): typed, the clipboard's when it holds one; empty is
    /// the tab's own ticket, as M-l opens it, and a bare number takes the
    /// team key of the ticket the branch names. Then the browser, or a new
    /// tab working on it (`wt`) in the pane's repository.
    fn open_ticket_id_typed(&mut self, client: ClientId, pane: Option<PaneId>) {
        let dir = self.dir_or_home(pane);
        let branch_ticket = pane
            .and_then(|p| self.panes.get(&p))
            .map(|s| s.tab)
            .and_then(|tab| self.ticket_here(tab, &dir));
        let known = branch_ticket.clone();
        self.off_task(
            client,
            move || {
                let clipboard = platform::paste();
                super::prefill(clipboard.as_deref(), |l| {
                    !l.chars().all(|c| c.is_ascii_digit() || c == '#')
                        && super::ticket_typed(l, known.as_deref()).is_ok()
                })
            },
            move |server, client, prefill| {
                let ask = Ask::new(
                    "Open a ticket",
                    "A ticket ID, number or Linear URL; empty for this tab's ticket, as M-l opens it.",
                )
                .text(prefill)
                .empty_ok();
                server.open(
                    client,
                    Box::new(ask),
                    Some(Box::new(move |server: &mut Server, client, outcome| {
                        let Outcome::Picked(p) = outcome else {
                            return;
                        };
                        server.ticket_typed_in(client, &p.value, dir, branch_ticket);
                    })),
                );
            },
        );
    }

    fn ticket_typed_in(
        &mut self,
        client: ClientId,
        typed: &str,
        dir: PathBuf,
        branch_ticket: Option<String>,
    ) {
        if typed.trim().is_empty() {
            self.view_command(client, crate::views::Command::OpenTicket);
            return;
        }
        let (id, url) = match super::ticket_typed(typed, branch_ticket.as_deref()) {
            Ok(TicketTyped::Url { url, id }) => (id, Some(url)),
            Ok(TicketTyped::Id(id)) => (id, None),
            Err(why) => {
                self.say(Some(client), &why);
                return;
            }
        };
        let menu = Menu::new(
            id.clone(),
            vec![
                MenuOption::new("Open it in the browser"),
                MenuOption::new("Work on it in a new tab (wt)"),
            ],
        );
        self.choose(client, menu, move |server, client, chose| match chose {
            Some(0) => match url {
                Some(url) => server.open_link(client, url),
                None => server.open_ticket_id(client, id),
            },
            Some(_) => {
                let Some(repo) = crate::cmd::work::main_clone(&dir) else {
                    server.say(
                        Some(client),
                        &format!(
                            "{} isn't in a repository, so there is nowhere to work on {id}.",
                            paths::tilde(&dir)
                        ),
                    );
                    return;
                };
                let command = worktree::tab_running("worktree-from-ticket", "wt", &[id]);
                if let Some(tab) =
                    server.work_tab(client, &repo, command, crate::mux::server::Opened::Front)
                {
                    server.front(client, &[tab]);
                }
            }
            None => {}
        });
    }
}

/// `C-w`, as a key.
fn ctrl_w() -> Key {
    Key::new(KeyCode::Char('w'), Mods::CTRL)
}

fn is_pane_nav(key: &Key) -> bool {
    *key == ctrl_w()
}
