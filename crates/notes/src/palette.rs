//! Ctrl-Shift-P: the notes app's command palette, ported from
//! notes/lua/notes/palette.lua. Every command Notes has, each with its key:
//! the list's (on the note open, from a note), the Format menu's (from a
//! note), folding, the app's, and Switchboard's commands for Notes, which
//! the workspace runs as their keys would. The ones picked last come first,
//! kept in Switchboard's state directory. The notes_palette setting turns
//! it off.

use std::path::Path;

use super::menu;
use crate::util;
use switchboard_ui::PickItem;

/// Picks kept, newest first, so the palette lists them first.
pub const RECENT_MAX: usize = 8;

/// What a command does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    ToggleList,
    /// The list back to the width the screen gives it.
    DefaultWidth,
    Find,
    Recent,
    Search,
    ClearSearch,
    /// To the note, from the list; to the list, from a note.
    OtherWindow,
    ListHelp,
    OpenListItem,
    NewNote,
    NewFolder,
    Rename,
    Move,
    Pin,
    Delete,
    Restore,
    Mark,
    Squash,
    NoteTab,
    /// Which tab the note is attached to, asked whatever tab it has.
    AttachNote,
    /// Start work on the checklist item under the cursor in a new tab.
    WorkOnItem,
    /// UX 2.0: the item's plan note (Enter).
    OpenPlan,
    /// UX 2.0: the tab working on the item (o).
    ItemTab,
    /// UX 2.0: the "Add an item" input (ga).
    AddItem,
    /// UX 2.0: on a narrow screen, the note shown in the place of the plan
    /// beside it, or the plan again.
    NoteOrPlan,
    /// A Switchboard command for Notes, by its name in keys.json, which the
    /// workspace runs (`switchboard press <name>`).
    Press(&'static str),
    Quit,
    Act,
    Indent,
    Outdent,
    FoldHere,
    FoldAll,
    UnfoldAll,
    Format(menu::Act),
}

impl Act {
    /// Whether it is the workspace's to do (a tab gone to or opened, a
    /// command of its server's), so the palette leaves it out where there
    /// is none: Notes alone, or outside a pane.
    pub fn needs_workspace(self) -> bool {
        matches!(
            self,
            Act::Press(_) | Act::NoteTab | Act::AttachNote | Act::WorkOnItem | Act::ItemTab
        )
    }

    /// Whether it acts on the checklist item under the cursor.
    pub fn on_item(self) -> bool {
        matches!(
            self,
            Act::WorkOnItem | Act::OpenPlan | Act::ItemTab | Act::AddItem | Act::NoteOrPlan
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub name: String,
    pub key: String,
    pub tag: &'static str,
    pub act: Act,
}

fn command(
    name: impl Into<String>,
    key: impl Into<String>,
    tag: &'static str,
    act: Act,
) -> Command {
    Command {
        name: name.into(),
        key: key.into(),
        tag,
        act,
    }
}

/// The commands for where the palette was opened: the list, or a note;
/// with UX 2.0 on (`ux2`), its item keys too.
pub fn commands(from_list: bool, in_note: bool, ux2: bool) -> Vec<Command> {
    let mut list = vec![
        command(
            "Show or put away the sidebar",
            "Ctrl-B",
            "view",
            Act::ToggleList,
        ),
        command("Find a note", "Ctrl-F", "view", Act::Find),
        command("Open a recent note", "", "view", Act::Recent),
        command("Search every note", "/", "view", Act::Search),
        command("Clear the search or tag", "x", "view", Act::ClearSearch),
        command(
            if from_list {
                "Go to the note"
            } else {
                "Go to the sidebar"
            },
            "Tab",
            "view",
            Act::OtherWindow,
        ),
        command("Show the sidebar's keys", "?", "view", Act::ListHelp),
        command(
            "Open what the sidebar's cursor is on",
            "Enter",
            "view",
            Act::OpenListItem,
        ),
        command("New note", "n", "note", Act::NewNote),
        command("New folder", "N", "note", Act::NewFolder),
        command("Rename the note", "R", "note", Act::Rename),
        command("Move the note to a folder", "m", "note", Act::Move),
        command("Pin or unpin the note", "p", "note", Act::Pin),
        command("Delete the note", "dd", "note", Act::Delete),
        command(
            "Restore the note from Recently Deleted",
            "r",
            "note",
            Act::Restore,
        ),
        command("Mark the note to squash", "s", "note", Act::Mark),
        command("Squash the marked notes into one", "S", "note", Act::Squash),
        command("Open the note's tab", "t", "tabs", Act::NoteTab),
        command("Attach this note to a tab", "", "tabs", Act::AttachNote),
        command(
            "Move Notes beside your work, or back to its own tab",
            "M-e",
            "tabs",
            Act::Press("notes-beside"),
        ),
        command(
            "Choose a tab's note",
            "",
            "tabs",
            Act::Press("choose-tab-note"),
        ),
        command(
            "Shape the note's checklist items as Linear tickets",
            "",
            "tabs",
            Act::Press("shape-note"),
        ),
        command(
            "Read a link from the note beside it",
            "",
            "links",
            Act::Press("read-link"),
        ),
        command(
            "Search notes, Linear, GitHub and Slack",
            "M-S-f",
            "search",
            Act::Press("search-everything"),
        ),
        command("Quit Notes", "q", "app", Act::Quit),
    ];
    if in_note {
        list.push(command(
            "Tick, fold or follow what the cursor is on",
            "Enter",
            "note",
            Act::Act,
        ));
        list.push(command(
            "Indent the item, or go to the next cell",
            "Tab",
            "note",
            Act::Indent,
        ));
        list.push(command(
            "Outdent the item, or go to the previous cell",
            "Shift-Tab",
            "note",
            Act::Outdent,
        ));
        list.push(command(
            "Fold or unfold this heading",
            "Enter",
            "fold",
            Act::FoldHere,
        ));
        list.push(command(
            "Start work on this item in a new tab (wtn)",
            "gw",
            "tabs",
            Act::WorkOnItem,
        ));
        if ux2 {
            list.push(command(
                "Open this item's plan",
                "Enter",
                "note",
                Act::OpenPlan,
            ));
            list.push(command(
                "Go to the tab working on this item",
                "o",
                "tabs",
                Act::ItemTab,
            ));
            list.push(command("Add an item", "ga", "note", Act::AddItem));
        }
        list.push(command("Fold every heading", "zM", "fold", Act::FoldAll));
        list.push(command(
            "Unfold every heading",
            "zR",
            "fold",
            Act::UnfoldAll,
        ));
        // The Format menu's own ways to start work and to attach the note
        // are the commands above.
        for (heading, item) in menu::items()
            .into_iter()
            .filter(|(_, i)| !matches!(i.act, menu::Act::WorkOnItem | menu::Act::AttachNote))
        {
            list.push(command(
                format!("{heading}: {}", item.label),
                if item.key.is_empty() {
                    String::new()
                } else {
                    format!("Ctrl-P {}", item.key)
                },
                "format",
                Act::Format(item.act),
            ));
        }
    }
    list
}

/// UX 2.0's command while an item's plan shows beside the note, which
/// has no key of its own.
pub fn note_or_plan() -> Command {
    command("Show the note beside its plan", "", "view", Act::NoteOrPlan)
}

/// The command while the list has a width of its own (a drag
/// of its edge, the pane resize keys), which has no key.
pub fn default_width() -> Command {
    command("Use the default width", "", "view", Act::DefaultWidth)
}

/// The commands picked last, newest first.
pub fn recent(file: &Path) -> Vec<String> {
    util::read_text(file)
        .map(|t| t.lines().map(String::from).collect())
        .unwrap_or_default()
}

/// A command picked, first in the list kept.
pub fn remember(file: &Path, name: &str) {
    let mut list = vec![name.to_string()];
    for other in recent(file) {
        if other != name && list.len() < RECENT_MAX {
            list.push(other);
        }
    }
    let _ = util::write_atomic(file, format!("{}\n", list.join("\n")));
}

/// The commands as the picker lists them, the ones picked last first, then
/// the rest in their order; each item's value is its place in `commands`.
pub fn items(commands: &[Command], recent: &[String]) -> Vec<PickItem> {
    let mut order: Vec<usize> = (0..commands.len()).collect();
    order.sort_by_key(|&i| {
        (
            recent
                .iter()
                .position(|r| *r == commands[i].name)
                .unwrap_or(usize::MAX),
            i,
        )
    });
    order
        .into_iter()
        .map(|i| {
            let c = &commands[i];
            PickItem {
                off: false,
                text: c.name.clone(),
                tags: format!("#{}", c.tag),
                key: c.key.clone(),
                value: i.to_string(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palette_lists_every_command_with_its_key_the_last_picked_first() {
        let dir = std::env::temp_dir().join(format!("notes-palette-{}", std::process::id()));
        let file = dir.join("recent");
        let _ = std::fs::remove_dir_all(&dir);
        let from_note = commands(false, true, false);
        assert!(
            from_note
                .iter()
                .any(|c| c.name == "Paragraph: Heading" && c.key == "Ctrl-P 2")
        );
        assert!(
            from_note
                .iter()
                .any(|c| c.name.starts_with("Move Notes beside your work") && c.key == "M-e"),
            "Switchboard's commands for Notes"
        );
        assert_eq!(
            from_note
                .iter()
                .filter(|c| c.name.ends_with("Attach this note to a tab"))
                .map(|c| (c.act, c.key.as_str()))
                .collect::<Vec<_>>(),
            [(Act::AttachNote, "")],
            "once, with no key, the Format menu's left out"
        );
        assert!(!from_note.iter().any(|c| c.act == Act::AddItem));
        assert!(
            commands(false, true, true)
                .iter()
                .any(|c| c.act == Act::AddItem && c.key == "ga"),
            "UX 2.0's item keys"
        );
        let from_list = commands(true, false, false);
        assert!(
            !from_list
                .iter()
                .any(|c| c.name.starts_with("Paragraph: Title")),
            "no Format menu from the sidebar"
        );
        remember(&file, "Find a note");
        remember(&file, "Show or put away the sidebar");
        let shown = items(&from_note, &recent(&file));
        assert_eq!(
            shown[0].text, "Show or put away the sidebar",
            "the one picked last first"
        );
        assert_eq!(
            (shown[0].key.as_str(), shown[0].tags.as_str()),
            ("Ctrl-B", "#view"),
            "with its key and tag"
        );
        assert_eq!(shown[1].text, "Find a note");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
