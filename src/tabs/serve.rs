//! Naming the tabs on the server: what tmux/window-names.sh did for every
//! window on every pane focus, done in the server task from `Names`, on a
//! focus, a tab switch and every tick. Each tab's label and color are kept
//! for the status line, `switchboard list` and later parts (the sidebar, the
//! Slack relay) to read, as the window's `@tab_label` was, and the lookups a
//! directory needs run in background tasks that answer with
//! `Event::TabLookup`, so neither a focus nor a draw ever waits on the
//! network or starts a process.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::{paths, util};

use super::color::{self, Rgb};
use super::label::{self, PaneKind};
use super::names::{Answer, Ask};
use super::{git, title};
use crate::mux::TabId;
use crate::mux::server::{ClientId, Event, Server};
use crate::ui::{Menu, MenuOption};

/// Where the PRs and titles found are kept for the next server, as
/// window-pr.sh and ticket-title.sh kept theirs under `~/.cache`.
fn names_file() -> PathBuf {
    paths::cache_dir().join("tab-names.json")
}

/// What a tab shows, as last worked out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    /// The directory it is named from (the window's `@tab_path`).
    pub path: Option<PathBuf>,
    /// Its label with no name of its own and no PR.
    pub label: String,
    pub pr: Option<u64>,
    /// The color it wears: its project's, or gray with the `accent_colors`
    /// setting off.
    pub color: Option<Rgb>,
}

impl Server {
    /// Works out what a tab shows from the directory of the pane it is named
    /// after, starting any lookup that directory needs, and redraws the
    /// session's status line when it changed.
    pub fn name_tab(&mut self, tab_id: TabId) {
        // The PRs and titles the last server found, before anything is
        // asked for again.
        if !self.tab_names.restored {
            self.tab_names.restored = true;
            if let Some(saved) = util::read_json(names_file()) {
                let names = &mut self.tab_names.names;
                names.restore(&saved, Instant::now(), util::now_secs());
            }
        }
        let Some(tab) = self.tabs.get(&tab_id) else {
            return;
        };
        let panes = tab.layout.panes();
        let kinds: Vec<(PaneKind, bool)> = panes
            .iter()
            .map(|p| {
                let opts = self.panes.get(p).map(|s| &s.opts);
                let marked = |name: &str| {
                    opts.and_then(|o| o.get(name))
                        .is_some_and(|v| !v.is_empty() && v != "0")
                };
                let kind = if marked("@sidebar") {
                    PaneKind::Sidebar
                } else if marked("@notes_pane") {
                    PaneKind::Notes
                } else {
                    PaneKind::Work
                };
                (kind, *p == tab.active)
            })
            .collect();
        let dir = label::naming_pane(&kinds)
            .and_then(|i| self.panes.get(&panes[i]))
            .and_then(|slot| slot.pane.cwd());
        // A directory that can't be read (the pane's program just ended)
        // leaves the tab as it was.
        let Some(dir) = dir else {
            return;
        };
        let (facts, asks) = self.tab_names.names.facts(&dir, Instant::now());
        for ask in asks {
            self.start_lookup(ask);
        }
        let shown = Shown {
            path: Some(dir),
            label: facts.label,
            pr: facts.pr,
            color: Some(if self.tab_names.names.last_switches().colors {
                facts.color
            } else {
                color::GRAY
            }),
        };
        let tab = self.tabs.get_mut(&tab_id).expect("checked above");
        tab.auto_name = shown.label.clone();
        if self.tab_names.shown.get(&tab_id) != Some(&shown) {
            self.tab_names.shown.insert(tab_id, shown);
            if let Some(name) = self.session_of_tab(tab_id).map(|s| s.name.clone()) {
                self.dirty_session(&name);
            }
        }
    }

    /// Names every tab again, and forgets what was kept for tabs and
    /// directories that are gone.
    pub fn name_all_tabs(&mut self) {
        let ids: Vec<TabId> = self.tabs.keys().copied().collect();
        for id in ids {
            self.name_tab(id);
        }
        let tabs = &self.tabs;
        self.tab_names.shown.retain(|id, _| tabs.contains_key(id));
        self.tab_names.names.prune(Instant::now());
    }

    /// A lookup's answer, from `Event::TabLookup`: the tabs are named again
    /// when it changed what they show.
    pub fn tab_lookup(&mut self, answer: Answer) {
        let now = Instant::now();
        if self.tab_names.names.answer(answer, now) {
            let saved = self.tab_names.names.saved(now, util::now_secs());
            let _ = util::write_atomic(names_file(), saved.to_string());
            self.name_all_tabs();
        }
    }

    // Runs a lookup off the server task, answering with `Event::TabLookup`.
    fn start_lookup(&self, ask: Ask) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let answer = ask.run().await;
            let _ = tx.send(Event::TabLookup(answer));
        });
    }

    /// The tab's own name: its `@tab_name` option (what M-p's "Rename the
    /// tab", `/shape` and `/wtn` set, and `/wt` clears), first, as
    /// tab-label.sh read it, else the one given with `rename-tab`.
    pub fn tab_own_name(&self, tab: TabId) -> Option<&str> {
        let tab = self.tabs.get(&tab)?;
        tab.opts
            .get("@tab_name")
            .map(String::as_str)
            .filter(|n| !n.is_empty())
            .or_else(|| (!tab.name.is_empty()).then_some(tab.name.as_str()))
    }

    /// The tab's label (what tmux kept in `@tab_label`): its own name, else
    /// its ticket and title, else its directory's name, with `#<PR> ` in
    /// front when its branch has one. Stored as it is: nothing here reads a
    /// `#` as a format, so none is doubled.
    pub fn tab_label(&self, tab: TabId) -> String {
        let shown = self.tab_names.shown.get(&tab);
        let facts = shown
            .map(|s| s.label.as_str())
            .or_else(|| self.tabs.get(&tab).map(|t| t.auto_name.as_str()))
            .unwrap_or("");
        label::label(self.tab_own_name(tab), facts, shown.and_then(|s| s.pr))
    }

    /// The color the tab wears: its project's, or gray with the
    /// `accent_colors` setting off; `None` before its directory is known.
    pub fn tab_color(&self, tab: TabId) -> Option<Rgb> {
        self.tab_names.shown.get(&tab)?.color
    }

    /// The directory the tab's name was worked out from (`@tab_path`).
    pub fn tab_path(&self, tab: TabId) -> Option<&Path> {
        self.tab_names.shown.get(&tab)?.path.as_deref()
    }

    /// The tab's PR number, when its branch has one.
    pub fn tab_pr(&self, tab: TabId) -> Option<u64> {
        self.tab_names.shown.get(&tab)?.pr
    }

    /// M-T's box for a tab (src/tabs/title.rs): its whole title, and its
    /// branch, directory, PR and ticket, with Copy the title and Close; with
    /// the tab_title setting off, it says so.
    pub fn show_tab_title(&mut self, client: ClientId, tab: TabId) {
        if !crate::settings::is_on("tab_title") {
            self.say(
                Some(client),
                "Showing a tab's full title is off in Settings (Show a tab's full title)",
            );
            return;
        }
        let Some(index) = self
            .session_of_tab(tab)
            .and_then(|s| s.tabs.iter().position(|t| *t == tab))
        else {
            return;
        };
        let full = self.tab_label(tab);
        let path = self.tab_path(tab).map(Path::to_path_buf);
        let branch = path
            .as_deref()
            .and_then(git::locate)
            .map(|place| place.branch);
        let shown = self.tab_names.shown.get(&tab);
        let ticket = shown
            .and_then(|s| title::ticket_of(&s.label))
            .map(str::to_string);
        let facts = title::facts(
            branch.as_deref(),
            path.as_deref().map(paths::tilde).as_deref(),
            self.tab_pr(tab),
            ticket.as_deref(),
        );
        let menu = Menu::new(
            format!("Tab {}", index + 1),
            vec![
                MenuOption::new("Copy the title").key('y'),
                MenuOption::new("Close"),
            ],
        )
        .body(title::body(&full, &facts));
        self.choose(client, menu, move |server, client, chose| {
            if chose == Some(0) {
                server.to_clipboard(Some(client), &full);
                server.say(Some(client), "Copied the tab's title");
            }
        });
    }
}
