//! A note's own tab, ported from tmux/note-tab.sh and the note parts of
//! tmux/new-tab.zsh: Notes' `t` (for the note under the cursor, through
//! `press note-tab <note>`) and M-p's "Open the tab for the note in Notes"
//! (for the one Notes shows) go to the tab linked to the note (`@tab_note`),
//! or, with none, ask which tab is the note's: one of the open tabs, linked
//! to it then and gone to, or a new one in a directory you pick, as M-t's,
//! linked to the note and named after it, whose Claude starts knowing which
//! note it is for (added to its system prompt, so no turn is spent on it:
//! its title and file, the note itself read when asked or when the session's
//! start says to read it first; the note_tab_prompt setting). new-tab.zsh
//! passed that in the tab's environment as NEW_TAB_CONTEXT, out of reach of
//! the shell; here it is a word of the command, which no shell reads either.
//!
//! "Attach this note to a tab" (`attach_note`, the plan "Attach a note to a
//! tab") is the same link made whatever tab the note has, and without going
//! there: every open tab listed with the note it has now, a tab with none
//! taking the note at once, one with another note asked about first, and
//! the note leaving the tab it was attached to, since a note is one tab's.
//!
//! Whichever tab it goes to, Notes moves beside the tab's Claude showing the
//! note, as M-e puts it there, the cursor left in Claude's pane (the
//! note_tab_beside setting).

use std::path::Path;

use super::{in_notes, shown};
use crate::mux::server::{ClientId, Server};
use crate::mux::{PaneId, TabId};
use crate::notes::store;
use crate::ui::{Confirm, PickItem, Picker};
use crate::{paths, settings, worktree};

/// What a note's new tab tells its Claude, added to its system prompt
/// (new-tab.zsh's NEW_TAB_CONTEXT): the note's title (`name`, else its
/// file's name) and its file, to read when asked or when the session's
/// start says to read it first (src/cmd/claude_context.rs, by the
/// tab_note_context setting as it is when each session starts, which a
/// prompt fixed as the tab opens can't follow).
pub fn new_tab_context(note: &str, name: Option<&str>) -> String {
    let stem = Path::new(note)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| note.to_string());
    let name = name.filter(|n| !n.is_empty()).unwrap_or(&stem);
    let dir = paths::tilde(store::notes_dir());
    let dir = dir.trim_end_matches('/');
    format!(
        "This tab is for the note \"{name}\" in the user's notes app, the file {dir}/{note}, \
         which Notes shows beside you. It is the user's own and private: read it when they ask \
         you to, or when this session's start tells you to read it first. Renamed or \
         moved since, its file is the one the tab's @tab_note option names now, relative to \
         the notes directory (switchboard show @tab_note)."
    )
}

/// A note's title: its first line without the #s of a heading, else its
/// file's name.
fn title_of(note: &str) -> String {
    let text = std::fs::read_to_string(store::notes_dir().join(note)).unwrap_or_default();
    let first = text
        .lines()
        .next()
        .unwrap_or("")
        .trim_start_matches('#')
        .trim()
        .to_string();
    if first.is_empty() {
        note.trim_end_matches(".md").to_string()
    } else {
        first
    }
}

/// A note as a tab's row names it: its file's name, which is its title,
/// without reading it.
fn name_of(note: &str) -> String {
    Path::new(note)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| note.to_string())
}

impl Server {
    /// The note a command is about: the one named, else the one the Notes
    /// in the tab in front shows, else the one Notes shows; none when it is
    /// no file in the notes directory.
    fn note_asked(&mut self, client: ClientId, note: Option<String>) -> Option<String> {
        let dir = store::notes_dir();
        // The note of the Notes in the tab in front, of several apps.
        let here = self
            .current_pane(client)
            .and_then(|p| self.panes.get(&p))
            .map(|s| s.tab)
            .and_then(|tab| self.notes_in_tab(tab))
            .and_then(|p| self.shown_in(p));
        note.filter(|n| !n.is_empty())
            .or(here)
            .or_else(shown)
            .filter(|n| in_notes(n) && dir.join(n).is_file())
    }

    /// The open tabs a note can be a tab's, in the tab bar's order: all but
    /// Notes' own and the Notifications window, which is no tab
    /// (note-tab.sh's `choose`).
    fn tabs_for_a_note(&self) -> Vec<TabId> {
        self.sessions
            .iter()
            .flat_map(|s| s.tabs.iter().copied())
            .filter(|t| {
                self.tab_opt(*t, "@notes").as_deref() != Some("1") && !self.is_notices_tab(*t)
            })
            .collect()
    }

    /// The note a tab has, as the picker and a note's tab set it.
    fn note_of_tab(&self, tab: TabId) -> Option<String> {
        self.tab_opt(tab, "@tab_note").filter(|n| !n.is_empty())
    }

    /// "Attach this note to a tab", for `note` (the one Notes shows when
    /// `None`): every open tab, each with the note it has now, the one the
    /// note is attached to marked. Picking one attaches the note and stays
    /// where it is.
    pub(super) fn attach_note(&mut self, client: ClientId, note: Option<String>) {
        let Some(note) = self.note_asked(client, note) else {
            self.say(Some(client), "No note to attach to a tab");
            return;
        };
        let title = title_of(&note);
        let items: Vec<PickItem> = self
            .tabs_for_a_note()
            .into_iter()
            .map(|tab| PickItem {
                text: format!("{} {}", self.tab_number(tab), self.tab_label(tab)),
                tags: match self.note_of_tab(tab) {
                    Some(has) if has == note => "attached".to_string(),
                    // Labeled, since the picker drops a tags column
                    // under six wide, as a short name alone would be.
                    Some(has) => format!("note: {}", name_of(&has)),
                    None => String::new(),
                },
                value: tab.to_string(),
                ..PickItem::default()
            })
            .collect();
        if items.is_empty() {
            self.say(Some(client), &format!("No tab to attach {title} to"));
            return;
        }
        let picker = Picker::new(format!("Attach {title} to which tab?"), items)
            .named("attach-note")
            .prompt("Tab: ");
        self.pick(client, picker, move |server, client, picked| {
            let tab = picked
                .and_then(|p| p.value.parse::<TabId>().ok())
                .filter(|t| server.tabs.contains_key(t));
            if let Some(tab) = tab {
                server.attach_note_to(client, tab, note, title);
            }
        });
    }

    /// The note attached to the tab picked: at once when the tab has none,
    /// after a yes when it has another, and not at all when it has this one.
    fn attach_note_to(&mut self, client: ClientId, tab: TabId, note: String, title: String) {
        let number = self.tab_number(tab);
        let Some(has) = self.note_of_tab(tab) else {
            self.make_tabs_note(client, tab, &note, &title);
            return;
        };
        if has == note {
            self.say(
                Some(client),
                &format!("{title} is already attached to tab {number}"),
            );
            return;
        }
        let leaves: Vec<String> = self
            .tabs_for_a_note()
            .into_iter()
            .filter(|t| *t != tab && self.note_of_tab(*t).as_deref() == Some(note.as_str()))
            .map(|t| format!("{title} will no longer be tab {}'s", self.tab_number(t)))
            .collect();
        let question = Confirm::new(format!(
            "Tab {number} has {}. Attach {title} to it in its place?",
            name_of(&has)
        ))
        .list(leaves);
        self.ask(client, question, move |server, client, yes| {
            if yes {
                server.make_tabs_note(client, tab, &note, &title);
            }
        });
    }

    /// Makes `note` the tab's, linked by you, and no other tab's: a tab it
    /// was attached to is left with no note, the one shown there last
    /// included when it is this one, so M-e there asks rather than showing
    /// a note that has moved. A Notes beside that tab that shows the note
    /// goes on showing it, so the tab is marked with it (`LEFT`), and
    /// Notes leaving doesn't keep it as the tab's last.
    fn make_tabs_note(&mut self, client: ClientId, tab: TabId, note: &str, title: &str) {
        if !self.tabs.contains_key(&tab) {
            return;
        }
        let mut left = Vec::new();
        for other in self.tabs_for_a_note() {
            if other == tab || self.note_of_tab(other).as_deref() != Some(note) {
                continue;
            }
            for option in ["@tab_note", super::BY_YOU, "@tab_note_last"] {
                if self.tab_opt(other, option).as_deref() == Some(note) {
                    self.set_tab_opt(other, option, None);
                }
            }
            // Only the note a Notes beside that tab shows now, so a mark
            // for the one it shows isn't lost to another note leaving.
            let shown_there = self
                .notes_in_tab(other)
                .and_then(|pane| self.shown_in(pane));
            if shown_there.as_deref() == Some(note) {
                self.set_tab_opt(other, super::LEFT, Some(note));
            }
            left.push(format!("tab {}'s", self.tab_number(other)));
        }
        self.set_tab_opt(tab, "@tab_note", Some(note));
        self.set_tab_opt(tab, super::BY_YOU, Some(note));
        // Its own note back; a mark for another note stays while Notes may
        // still show that one.
        if self.tab_opt(tab, super::LEFT).as_deref() == Some(note) {
            self.set_tab_opt(tab, super::LEFT, None);
        }
        let mut said = format!("Attached {title} to tab {}", self.tab_number(tab));
        if !left.is_empty() {
            said.push_str(&format!(", no longer {}", left.join(" or ")));
        }
        self.say(Some(client), &said);
    }

    /// The tab for `note` (the one Notes shows when `None`): the one linked
    /// to it, else a question which.
    pub(super) fn note_tab(&mut self, client: ClientId, note: Option<String>) {
        let Some(note) = self.note_asked(client, note) else {
            self.say(Some(client), "No note to open a tab for");
            return;
        };
        // Its tab, if one is linked to it; never Notes' own.
        let linked = self
            .sessions
            .iter()
            .flat_map(|s| s.tabs.iter().copied())
            .find(|t| {
                self.tab_opt(*t, "@tab_note").as_deref() == Some(note.as_str())
                    && self.tab_opt(*t, "@notes").as_deref() != Some("1")
            });
        if let Some(tab) = linked {
            // Going to it from the note is linking it yourself.
            self.set_tab_opt(tab, super::BY_YOU, Some(&note));
            self.go_to_tab(client, tab);
            self.note_tab_beside(tab, client);
            return;
        }
        let title = title_of(&note);
        // The open tabs, as the tab bar names them.
        let mut items = vec![PickItem::new("+ New tab, in a directory you pick", "new")];
        for tab in self.tabs_for_a_note() {
            items.push(PickItem::new(
                format!("{} {}", self.tab_number(tab), self.tab_label(tab)),
                tab.to_string(),
            ));
        }
        let picker = Picker::new(
            format!("Which tab is {title}'s? Enter attaches it and goes there"),
            items,
        )
        .header(
            "As \"Attach this note to a tab\", which stays here and asks before replacing a note",
        )
        .named("note-tab")
        .prompt("Tab: ");
        self.pick(client, picker, move |server, client, picked| {
            let Some(picked) = picked else {
                return;
            };
            if picked.value == "new" {
                let asked = format!("A tab for {title}");
                server.pick_dir_for(client, &asked, move |server, client, dir| {
                    server.open_note_tab(client, &dir, &note, &title);
                });
                return;
            }
            let Some(tab) = picked
                .value
                .parse::<TabId>()
                .ok()
                .filter(|t| server.tabs.contains_key(t))
            else {
                return;
            };
            server.set_tab_opt(tab, "@tab_note", Some(&note));
            server.set_tab_opt(tab, super::BY_YOU, Some(&note));
            server.go_to_tab(client, tab);
            server.note_tab_beside(tab, client);
        });
    }

    /// A new tab in `dir` running Claude for `note`, linked to it and named
    /// after it, at the end of the tabs and in front.
    fn open_note_tab(&mut self, client: ClientId, dir: &Path, note: &str, title: &str) {
        let command = if settings::is_on("note_tab_prompt") {
            worktree::tab_command(
                "claude \"$@\"",
                "claude",
                &[
                    "--append-system-prompt".into(),
                    new_tab_context(note, Some(title)),
                ],
            )
        } else {
            worktree::tab_command("claude", "claude", &[])
        };
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        let tab = match self.new_tab(
            &session,
            Some(dir.to_path_buf()),
            Some(command),
            None,
            crate::mux::server::Opened::Front,
        ) {
            Ok((tab, _)) => tab,
            Err(e) => {
                self.say(Some(client), &format!("Couldn't open a tab: {e}"));
                return;
            }
        };
        self.tab_to_end(tab);
        self.set_tab_opt(tab, "@tab_note", Some(note));
        self.set_tab_opt(tab, super::BY_YOU, Some(note));
        let name = if title.is_empty() {
            note.trim_end_matches(".md")
        } else {
            title
        };
        self.set_tab_opt(tab, "@tab_name", Some(name));
        self.name_tab(tab);
        self.note_tab_beside(tab, client);
    }

    /// Notes beside the tab's Claude, showing the tab's note; there
    /// already, it is only shown it. Either way the focus goes to Claude's
    /// pane, else to the tab's first that isn't Notes, as in a tab just
    /// opened, whose Claude is still starting.
    pub(super) fn note_tab_beside(&mut self, tab: TabId, client: ClientId) {
        if !settings::is_on("note_tab_beside") {
            return;
        }
        let notes = self.notes_in_tab(tab);
        let panes: Vec<PaneId> = self
            .tabs
            .get(&tab)
            .map(|t| t.layout.panes())
            .unwrap_or_default()
            .into_iter()
            .filter(|p| Some(*p) != notes && !self.pane_marked(*p, "@notes_pane"))
            .collect();
        let Some(pane) = panes
            .iter()
            .copied()
            .find(|p| self.runs_claude(*p))
            .or_else(|| panes.first().copied())
        else {
            return;
        };
        if notes.is_some() {
            self.show_note(tab);
        } else {
            self.notes_beside(Some(client), pane);
        }
        self.focus_pane(pane);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_tab_is_told_its_note_and_where_to_find_it() {
        let context = new_tab_context("Work/Plan $(x).md", Some("Plan $(x)"));
        assert!(
            context.starts_with("This tab is for the note \"Plan $(x)\" in the user's notes app"),
            "{context}"
        );
        assert!(
            context.contains("/Work/Plan $(x).md, which Notes shows beside you"),
            "its file, as written: {context}"
        );
        // Read when asked, or first when the session's start says so, which
        // follows the setting as each session starts.
        assert!(context.contains("read it when they ask you to, or when this session's start"));
        assert!(
            context.contains("@tab_note"),
            "and where to find it once renamed"
        );
        // With no title, the file's name.
        assert!(new_tab_context("Work/Plan.md", None).contains("the note \"Plan\""));
    }
}
