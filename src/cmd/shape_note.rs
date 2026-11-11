//! Shapes a note's to-dos as Linear tickets, for M-p's "Shape a note's
//! checklist items" and the notes app's palette (`press shape-note`): pick
//! a note, the checklist items to shape (every unticked one, to start
//! with), and the repository they belong to, and each item gets a tab of
//! its own running `shape`, as shape-batch opens them. /shape looks for a
//! ticket that already covers an item first, and offers to resume shaping
//! that one instead. M-p's "Shape a new Linear ticket" is the same with one
//! idea typed in place of a note's items. Ported from tmux/note-shape.sh.
//!
//! The notes are the notes app's: Markdown in `$NOTES_DIR` or `~/Notes`,
//! the one open in the app first, then the rest, newest first. An item is
//! an unticked top-level `- [ ]` line, and everything indented under it is
//! part of its ticket: lines that aren't checklist items are its details,
//! and unticked checklist items at the first level under it are its
//! sub-items, each a ticket of its own, with anything indented under them
//! as their details. A ticked item, and all that is under it, is done and
//! left out, and so is one struck through (`~~...~~`). /shape gets the item
//! on one line, with the note's title, its details and its sub-items
//! labelled (`item_lines`), which the skill's "A checklist item from a
//! note" section reads, so a change to the format belongs in both.
//!
//! fzf took several items with Tab, all of them selected to start with;
//! here the list's first row shapes the ones selected, and picking any
//! other row selects or deselects it and shows the list again. The
//! repository is asked as M-t asks for a directory, the pane's own first,
//! and taken as its main clone, as note-shape.sh took each tab's. What has
//! to read the disk is done off the server's task, as the keys that start
//! work do (src/cmd/work.rs).

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::cmd::{shape_batch, work, worktree_from_note};
use crate::mux::TabId;
use crate::mux::server::{ClientId, Server};
use crate::ui::{PickItem, Picker};
use crate::{paths, settings, util, worktree};

/// One item of a note, as the list shows it and as /shape is handed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// The item, and how many sub-items it has: the list's row.
    pub shown: String,
    /// The line /shape gets.
    pub line: String,
}

static LEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*").unwrap());
static CHECKLIST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*- \[[ xX]\]").unwrap());
static TICKED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*- \[[xX]\]").unwrap());
static MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(- \[[ xX]\]|[-*+]|[0-9]+\.)?\s*").unwrap());
static TRAILING_DOTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.+$").unwrap());

/// A note's title, as note-shape.sh read it: its first line without the
/// `#`s of a heading, else its file's name.
pub fn title_of(text: &str, name: &str) -> String {
    let first = text.lines().next().unwrap_or("");
    let title = if first.starts_with('#') {
        first.trim_start_matches('#').trim_start()
    } else {
        first
    };
    if title.is_empty() {
        name.strip_suffix(".md").unwrap_or(name).to_string()
    } else {
        title.to_string()
    }
}

/// The item being read, until the next top-level line ends it.
#[derive(Default)]
struct Reading {
    item: String,
    details: String,
    subs: Vec<(String, String)>,
    child_depth: usize,
    skipping: bool,
}

impl Reading {
    fn line(&self, title: &str) -> Option<Item> {
        if self.item.is_empty() {
            return None;
        }
        let details = TRAILING_DOTS.replace(&self.details, "");
        let mut out = format!("{} (from my note \"{title}\").", self.item);
        if !details.is_empty() {
            out.push_str(&format!(" Details: {details}."));
        }
        let n = self.subs.len();
        if n > 0 {
            out.push_str(" Sub-items, each its own ticket:");
            for (i, (child, child_details)) in self.subs.iter().enumerate() {
                out.push_str(&format!(" ({}) {child}", i + 1));
                if !child_details.is_empty() {
                    out.push_str(&format!(" [{child_details}]"));
                }
                out.push(if i + 1 < n { ';' } else { '.' });
            }
        }
        let shown = if n > 0 {
            format!(
                "{}  ({n} sub-item{})",
                self.item,
                if n > 1 { "s" } else { "" }
            )
        } else {
            self.item.clone()
        };
        Some(Item { shown, line: out })
    }
}

/// Joins a line of details onto those before it, as ` / `.
fn add(list: &mut String, text: &str) {
    if !list.is_empty() {
        list.push_str(" / ");
    }
    list.push_str(text);
}

/// A note's unticked top-level items, each with what is under it, as
/// note-shape.sh's awk read them.
pub fn item_lines(text: &str, title: &str) -> Vec<Item> {
    let mut items = Vec::new();
    let mut r = Reading::default();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let depth = LEADING.find(line).map_or(0, |m| m.as_str().chars().count());
        let checklist = CHECKLIST.is_match(line);
        let rest = MARKER.replace(line, "").into_owned();
        let ticked = TICKED.is_match(line) || (checklist && rest.starts_with("~~"));
        // A top-level line ends the item before it; an unticked checklist
        // one starts the next.
        if depth == 0 {
            items.extend(r.line(title));
            r = Reading::default();
            if checklist && !ticked && !rest.is_empty() {
                r.item = rest;
            }
            continue;
        }
        if r.item.is_empty() {
            continue;
        }
        // Everything under a ticked sub-item is done with it.
        if r.skipping && depth > r.child_depth {
            continue;
        }
        r.skipping = false;
        // The first checklist level under the item holds its sub-items.
        if checklist && (r.child_depth == 0 || depth <= r.child_depth) {
            r.child_depth = depth;
            if ticked {
                r.skipping = true;
            } else if !rest.is_empty() {
                r.subs.push((rest, String::new()));
            }
            continue;
        }
        if depth > r.child_depth
            && let Some((_, child_details)) = r.subs.last_mut()
        {
            add(child_details, &rest);
            continue;
        }
        add(&mut r.details, &rest);
    }
    items.extend(r.line(title));
    items
}

/// The notes to offer: the one open in the app (`.notes.json`'s last)
/// first, then the rest, newest first, each by its path in the notes
/// directory without `.md`.
fn notes_offered(notes: &Path) -> Vec<String> {
    let mut names = worktree_from_note::notes_newest_first(notes);
    let open = util::read_json(notes.join(".notes.json"))
        .and_then(|v| v.get("last")?.as_str().map(str::to_string))
        .map(|last| last.strip_suffix(".md").unwrap_or(&last).to_string());
    if let Some(open) = open
        && let Some(at) = names.iter().position(|n| *n == open)
    {
        let name = names.remove(at);
        names.insert(0, name);
    }
    names
}

/// The tab command that runs `shape` with one item, as shape-batch makes it.
fn shape_tab(item: &str) -> Vec<String> {
    let script = format!("{} \"$@\"", worktree::self_command("shape"));
    worktree::tab_command(&script, "shape", &[item.to_string()])
}

impl Server {
    /// M-p's "Shape a note's checklist items" (`typed` false) and "Shape a
    /// new Linear ticket" (`typed` true).
    pub fn shape_note(&mut self, client: ClientId, typed: bool) {
        if !settings::is_on("linear") {
            self.service_off(client, "linear");
            return;
        }
        if typed {
            let picker = Picker::new(
                "Shape a new ticket: what should it be about? (Enter on nothing cancels)",
                Vec::new(),
            )
            .named("shape-idea")
            .prompt("> ")
            .free();
            self.pick(client, picker, |server, client, picked| {
                let idea = picked.map(|p| p.value.trim().to_string());
                if let Some(idea) = idea.filter(|i| !i.is_empty()) {
                    server.shape_in_repo(client, vec![idea]);
                }
            });
            return;
        }
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
            move || {
                let names = notes_offered(&dir);
                // Each note's unticked items, shown for the one chosen, as
                // note-shape.sh's fzf --preview grepped them.
                let previews: Vec<String> = names
                    .iter()
                    .map(|n| unticked(&dir.join(format!("{n}.md"))))
                    .collect();
                (names, previews)
            },
            move |server, client, (names, previews)| {
                if names.is_empty() {
                    server.say(
                        Some(client),
                        &format!("No notes in {}.", paths::tilde(&notes)),
                    );
                    return;
                }
                let rows = names
                    .iter()
                    .map(|n| PickItem::new(n.clone(), n.clone()))
                    .collect();
                let picker = Picker::new("The note whose checklist to shape", rows)
                    .named("notes")
                    .prompt("note> ")
                    .previews(previews);
                server.pick(client, picker, move |server, client, picked| {
                    if let Some(picked) = picked {
                        server.shape_items_of(client, notes, format!("{}.md", picked.value));
                    }
                });
            },
        );
    }
}

/// A note's unticked checklist lines, the first 40, as note-shape.sh's
/// preview showed them.
fn unticked(file: &std::path::Path) -> String {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.trim_start().starts_with("- [ ] "))
        .take(40)
        .collect::<Vec<_>>()
        .join("\n")
}

impl Server {
    /// The note's items read, then asked which to shape.
    fn shape_items_of(&mut self, client: ClientId, notes: PathBuf, note: String) {
        let file = notes.join(&note);
        let name = note.clone();
        self.off_task(
            client,
            move || {
                let text = std::fs::read_to_string(&file).unwrap_or_default();
                let title = title_of(&text, &name);
                let items = item_lines(&text, &title);
                (title, items)
            },
            |server, client, (title, items)| {
                if items.is_empty() {
                    server.say(
                        Some(client),
                        &format!("\"{title}\" has no unticked checklist items."),
                    );
                    return;
                }
                let selected = vec![true; items.len()];
                server.ask_which_items(client, title, items, selected, String::new());
            },
        );
    }

    /// Which items to shape: the first row shapes those selected, any other
    /// selects or deselects its item and asks again, what was typed kept.
    fn ask_which_items(
        &mut self,
        client: ClientId,
        title: String,
        items: Vec<Item>,
        selected: Vec<bool>,
        query: String,
    ) {
        let count = selected.iter().filter(|s| **s).count();
        let mut rows = vec![PickItem::new(
            match count {
                0 => "Nothing selected: pick an item to select it".to_string(),
                1 => "Shape the 1 selected, in a tab of its own".to_string(),
                n => format!("Shape the {n} selected, one tab each"),
            },
            "go",
        )];
        rows.extend(
            items
                .iter()
                .zip(&selected)
                .enumerate()
                .map(|(i, (item, on))| {
                    PickItem::new(
                        format!("{} {}", if *on { "●" } else { "○" }, item.shown),
                        i.to_string(),
                    )
                }),
        );
        let picker = Picker::new(
            format!("Shape {title}'s items: a pick selects or deselects one"),
            rows,
        )
        .named("items")
        .prompt("shape> ")
        .query(&query);
        self.pick(client, picker, move |server, client, picked| {
            let Some(picked) = picked else {
                return;
            };
            if picked.value == "go" {
                let chosen: Vec<String> = items
                    .iter()
                    .zip(&selected)
                    .filter(|(_, on)| **on)
                    .map(|(item, _)| item.line.clone())
                    .collect();
                if !chosen.is_empty() {
                    server.shape_in_repo(client, chosen);
                }
                return;
            }
            let mut selected = selected;
            if let Some(on) = picked
                .value
                .parse::<usize>()
                .ok()
                .and_then(|i| selected.get_mut(i))
            {
                *on = !*on;
            }
            server.ask_which_items(client, title, items, selected, picked.query);
        });
    }

    /// Asks which repository the tickets are for, the pane's own first, and
    /// opens a tab for each item there, at the end and in the background,
    /// as shape-batch did.
    fn shape_in_repo(&mut self, client: ClientId, items: Vec<String>) {
        let items = shape_batch::items_of(&items);
        self.pick_dir_for(
            client,
            "The repo these tickets are for",
            move |server, client, dir| {
                let Some(repo) = work::main_clone(&dir) else {
                    server.say(
                        Some(client),
                        &format!(
                            "{} is not in a repository, so shape has no code to read. Pick the repo the tickets belong to.",
                            paths::tilde(&dir)
                        ),
                    );
                    return;
                };
                let Some(session) = server.clients.get(&client).map(|c| c.session.clone())
                else {
                    return;
                };
                let mut opened: Vec<TabId> = Vec::new();
                for item in &items {
                    match server.new_tab(&session, Some(repo.clone()), Some(shape_tab(item)), None, crate::mux::server::Opened::Background)
                    {
                        Ok((tab, _)) => {
                            server.tab_to_end(tab);
                            opened.push(tab);
                        }
                        Err(e) => {
                            server.say(Some(client), &format!("Couldn't open a tab: {e}"));
                            return;
                        }
                    }
                }
                server.front(client, &opened);
                let n = opened.len();
                server.say(
                    Some(client),
                    &format!(
                        "Shaping {n} {} in {}, a tab each at the end, the first in front",
                        if n == 1 { "item" } else { "items" },
                        paths::tilde(&repo)
                    ),
                );
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "# Launch plan

Some words about it.

- [ ] Make the phase label clearer.
  It reads as a date
  - see the design doc
- [x] Already done
  - [ ] under a done one, so done too
- [ ] ~~Struck through~~
- [ ] Results load slowly
  - [ ] Cache the query
    measured at 3s
  - [x] Add an index
    - [ ] under a ticked sub-item
  - [ ] Show a spinner
  The page waits on all of it...
* not a checklist item
- [ ] Last one
";

    #[test]
    fn a_notes_items_are_one_line_each_with_details_and_sub_items_labelled() {
        let title = title_of(NOTE, "Launch.md");
        assert_eq!(title, "Launch plan");
        let items = item_lines(NOTE, &title);
        assert_eq!(
            items,
            vec![
                Item {
                    shown: "Make the phase label clearer.".into(),
                    line: "Make the phase label clearer. (from my note \"Launch plan\"). Details: It reads as a date / see the design doc.".into(),
                },
                Item {
                    shown: "Results load slowly  (2 sub-items)".into(),
                    line: "Results load slowly (from my note \"Launch plan\"). Details: The page waits on all of it. Sub-items, each its own ticket: (1) Cache the query [measured at 3s]; (2) Show a spinner.".into(),
                },
                Item {
                    shown: "Last one".into(),
                    line: "Last one (from my note \"Launch plan\").".into(),
                },
            ]
        );
    }

    #[test]
    fn a_note_with_no_heading_is_titled_by_its_name() {
        assert_eq!(title_of("", "Work/Ideas.md"), "Work/Ideas");
        assert_eq!(title_of("## Inbox\n", "x.md"), "Inbox");
        assert!(item_lines("- [x] done\n", "x").is_empty());
    }
}
