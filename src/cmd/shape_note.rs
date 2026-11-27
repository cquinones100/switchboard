//! Shapes a note's to-dos as Linear tickets, for M-p's "Shape a note's
//! checklist items" and the notes app's palette (`press shape-note`): pick
//! a note, the checklist items to shape (every unticked one, to start
//! with), and the repository they belong to, and each item gets a tab of
//! its own running `shape`, as shape-batch opens them. /shape looks for a
//! ticket that already covers an item first, and offers to resume shaping
//! that one instead. M-p's "Shape a new ticket" is the same with one idea
//! typed in place of a note's items, in an ask box drawn as a text box
//! (`Ask::text_box`) titled after where the pane's repository keeps its
//! tickets, and filed where the chosen repository's `tickets` says (Linear
//! ticket IDs alone each shaped further as a ticket of their own): while
//! that is unset, its source is off, or the directory chosen isn't in a
//! repository, the box comes back with what was typed and why. Ported from
//! tmux/note-shape.sh.
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
use crate::mission::ask::Ask;
use crate::mux::TabId;
use crate::mux::server::{ClientId, Server};
use crate::tickets::Kept;
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

/// The tab command that runs `shape` with one item, as shape-batch makes it,
/// filed in `source` (`linear`, `github` or `notes`) whatever the
/// repository keeps its work in, or, with none, where its `tickets` says.
fn shape_tab(item: &str, source: Option<&str>) -> Vec<String> {
    let flag = source.map(|s| format!(" --{s}")).unwrap_or_default();
    let script = format!("{}{flag} \"$@\"", worktree::self_command("shape"));
    worktree::tab_command(&script, "shape", &[item.to_string()])
}

/// The Shape a new ticket box, titled after `kept`, `text` typed in it.
fn shape_box(kept: Option<Kept>, text: &str) -> Ask {
    Ask::new(Kept::shape_title(kept), "What should it be about?")
        .multi_line()
        .text(text)
        .text_box("Shape it")
}

impl Server {
    /// M-p's "Shape a note's checklist items" (`typed` false) and "Shape a
    /// new ticket" (`typed` true).
    pub fn shape_note(&mut self, client: ClientId, typed: bool) {
        if typed {
            let kept = self
                .current_pane(client)
                .and_then(|p| self.pane_own_dir(p))
                .and_then(|dir| crate::tickets::kept_known(&dir));
            self.ask_new_ticket(client, shape_box(kept, ""));
            return;
        }
        if !settings::is_on("linear") {
            self.service_off(client, "linear");
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
                    .keep_order()
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

    /// The Tickets view's "Shape a new ticket…": an idea typed in, shaped by
    /// `/shape` in a tab of its own in the repository at `dir` (the front
    /// pane's main clone), filed in that repository's ticket source; while
    /// none is set, what sets it is said instead, as `/shape` would stop.
    pub fn shape_from_tickets(&mut self, client: ClientId, dir: PathBuf, source: Option<String>) {
        let Some(source) = source else {
            let name = crate::repos::name_known(&dir);
            self.say(Some(client), &crate::repos::unset_words(&name));
            return;
        };
        let ask = shape_box(Kept::parse(&source), "");
        self.open(
            client,
            Box::new(ask),
            Some(Box::new(move |server: &mut Server, client, outcome| {
                let crate::ui::Outcome::Picked(p) = outcome else {
                    return;
                };
                let Some(session) = server.clients.get(&client).map(|c| c.session.clone()) else {
                    return;
                };
                let command = shape_tab(&p.value, Some(&source));
                match server.new_tab(
                    &session,
                    Some(dir.clone()),
                    Some(command),
                    None,
                    crate::mux::server::Opened::Background,
                ) {
                    Ok((tab, _)) => {
                        server.tab_to_end(tab);
                        server.front(client, &[tab]);
                    }
                    Err(e) => server.say(Some(client), &format!("Couldn't open a tab: {e}")),
                }
            })),
        );
    }

    /// The Shape a new ticket box: what is typed is shaped in the
    /// repository asked for next.
    fn ask_new_ticket(&mut self, client: ClientId, ask: Ask) {
        self.open(
            client,
            Box::new(ask),
            Some(Box::new(|server: &mut Server, client, outcome| {
                if let crate::ui::Outcome::Picked(p) = outcome {
                    server.shape_typed(client, p.value);
                }
            })),
        );
    }

    /// Asks which repository `text` is a ticket for, the pane's own first,
    /// and opens a tab there running `shape`, an idea filed where the
    /// repository's `tickets` says, Linear ticket IDs alone a tab each;
    /// where it can't be, the box comes back with `text` and why.
    fn shape_typed(&mut self, client: ClientId, text: String) {
        let kept_here = self
            .current_pane(client)
            .and_then(|p| self.pane_own_dir(p))
            .and_then(|dir| crate::tickets::kept_known(&dir));
        let back_text = text.clone();
        self.pick_dir_or_back(
            client,
            "The repo this ticket is for",
            move |server, client, dir| {
                let again = |server: &mut Server, kept: Option<Kept>, why: String| {
                    server.ask_new_ticket(client, shape_box(kept, &text).failed(why));
                };
                let Some(repo) = work::main_clone(&dir) else {
                    let why = format!(
                        "{} is not in a repository, so shape has no code to read. Pick the repo the ticket belongs to.",
                        paths::tilde(&dir)
                    );
                    return again(server, None, why);
                };
                let kept = crate::tickets::kept_known(&repo);
                // Ticket IDs alone are a tab each, as shape-batch takes them,
                // each a Linear ticket to shape further whatever the
                // repository keeps, so they need Linear alone; anything else
                // is an idea, filed in the repository's own source, named so
                // a leading word shaped like an ID stays part of it.
                let items = shape_batch::items_of(std::slice::from_ref(&text));
                // As `items_from` splits them: every word an ID or a link.
                let words: Vec<&str> = text
                    .split(|c: char| c.is_whitespace() || c == ',')
                    .filter(|w| !w.is_empty())
                    .collect();
                let only_ids =
                    !words.is_empty() && words.iter().all(|w| worktree::looks_like_ticket(w));
                let source = if only_ids {
                    if !settings::is_on("linear") {
                        let why = "Linear is off in Settings, so the Linear tickets typed can't be shaped.".to_string();
                        return again(server, kept, why);
                    }
                    None
                } else {
                    let Some(kept) = kept else {
                        let name = crate::repos::name_known(&repo);
                        return again(server, None, crate::repos::unset_words(&name));
                    };
                    if let Some(source) = kept.source().filter(|s| !s.is_on()) {
                        let why = format!(
                            "{} keeps its tickets in {}, which is off in Settings.",
                            crate::repos::name_known(&repo),
                            source.name()
                        );
                        return again(server, Some(kept), why);
                    }
                    Some(kept.key())
                };
                let mut opened: Vec<TabId> = Vec::new();
                for item in items {
                    match server.tab_at_end(
                        client,
                        &repo,
                        shape_tab(&item, source),
                        crate::mux::server::Opened::Background,
                    ) {
                        Some(tab) => opened.push(tab),
                        None => break,
                    }
                }
                if opened.is_empty() {
                    return again(server, kept, "Couldn't open a tab for it.".to_string());
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
            // Esc, or a directory that isn't there: back to the box, with
            // what was typed.
            move |server, client, why| {
                let ask = shape_box(kept_here, &back_text);
                let ask = match why {
                    Some(why) => ask.failed(why),
                    None => ask,
                };
                server.ask_new_ticket(client, ask);
            },
        );
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
                    match server.new_tab(&session, Some(repo.clone()), Some(shape_tab(item, Some("linear"))), None, crate::mux::server::Opened::Background)
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
