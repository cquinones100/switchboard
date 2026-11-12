//! Where notes live, and everything done to them on disk, ported from
//! notes/lua/notes/store.lua.
//!
//! A note is a Markdown file. Its title is its first line, without the `# `
//! a title is written with, and its file is named after the title when it is
//! made, then renamed when the title changes (`rename_to_title`), so the
//! folder reads like the notes in it. Folders are directories, nested as
//! deep as you like. The notes directory is `$NOTES_DIR`, or `~/Notes`.
//!
//! Two things in it aren't notes: `.trash/`, where a deleted note goes (the
//! "Recently Deleted" folder; deleting it from there removes it for good),
//! and `.notes.json`, which records the pinned notes and the note open last,
//! by their paths in the notes directory. A note's last line holds its
//! stable ID (id.rs), which the app writes and never shows. Nothing else is
//! kept anywhere, so the notes stay plain files any editor can open.
//!
//! The directory is the `Store`'s, given when it is made, rather than read
//! from the environment each time as the plugin did, so the tests each give
//! one of their own while running side by side.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

use super::id;
use crate::util;

pub const TRASH: &str = ".trash";
pub const META: &str = ".notes.json";
pub const LOCK: &str = ".notes.lock";

/// The notes directory, and where the rest of the workspace keeps what
/// follows a note (the tabs linked to it, the mode working it).
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
    /// Switchboard's state directory, where the mode that works a note
    /// continuously keeps `autopilot.json`.
    pub state: PathBuf,
    /// Whether tabs linked to a note are told when it moves: only in a pane
    /// of a running Switchboard, as the plugin reached tmux only inside it.
    pub server: bool,
}

/// `$NOTES_DIR`, else `~/Notes`.
pub fn notes_dir() -> PathBuf {
    match std::env::var_os("NOTES_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => crate::paths::home_join("Notes"),
    }
}

/// One note, as the list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct NoteEntry {
    pub path: PathBuf,
    pub rel: String,
    /// Its folder in the notes directory, "" for the top.
    pub folder: String,
    pub title: String,
    pub preview: String,
    pub mtime: i64,
    pub text: String,
    pub pinned: bool,
    /// For a search, the first line that matched.
    pub matched: Option<String>,
}

/// A folder, as the list shows it: its path, its name, how deep it is and
/// how many notes it holds directly.
#[derive(Clone, Debug, PartialEq)]
pub struct Folder {
    pub rel: String,
    pub name: String,
    pub depth: usize,
    pub count: usize,
}

/// A note's title before and after it changed, for the tabs named after it.
#[derive(Clone, Debug)]
pub struct Retitled {
    /// The note's path in the notes directory now.
    pub path: String,
    pub from: String,
    pub to: String,
}

/// A tab's links to notes, as the server lists them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabLinks {
    pub id: String,
    pub note: String,
    pub last: String,
    pub arrived: String,
    pub name: String,
    /// The ID of the note autopilot opened it for
    /// (`@autopilot_note`).
    pub pilot_note: String,
}

fn dirname(rel: &str) -> &str {
    rel.rfind('/').map_or("", |i| &rel[..i])
}

fn basename(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A file's modified time in seconds.
pub fn mtime(path: &Path) -> Option<i64> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let d = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(d.as_secs() as i64)
}

impl Store {
    /// The notes directory `dir`, as its real path, so it matches the paths
    /// given for notes in it: on macOS /var and /tmp are links into
    /// /private, for one.
    pub fn at(dir: impl Into<PathBuf>) -> Store {
        let dir = dir.into();
        let root = fs::canonicalize(&dir).unwrap_or(dir);
        Store {
            root,
            state: crate::paths::state_dir(),
            server: false,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The directory and Recently Deleted made, and the root read as its
    /// real path once it is there.
    pub fn ensure(&mut self) {
        let _ = fs::create_dir_all(self.root.join(TRASH));
        if let Ok(real) = fs::canonicalize(&self.root) {
            self.root = real;
        }
    }

    /// A path relative to the notes directory, for the metadata and the
    /// list; its folder read as its real path, so a link to it still counts.
    pub fn relative(&self, path: &Path) -> String {
        let mut path = path.to_path_buf();
        if let (Some(parent), Some(name)) = (path.parent(), path.file_name())
            && let Ok(real) = fs::canonicalize(parent)
        {
            path = real.join(name);
        }
        match path.strip_prefix(&self.root) {
            Ok(rel) => rel.to_string_lossy().into_owned(),
            Err(_) => path.to_string_lossy().into_owned(),
        }
    }

    pub fn absolute(&self, rel: &str) -> PathBuf {
        if rel.is_empty() {
            self.root.clone()
        } else {
            self.root.join(rel)
        }
    }

    // ── Metadata ──────────────────────────────────────────────────────────

    /// `.notes.json`, with `pinned` always a list.
    pub fn meta(&self) -> Map<String, Value> {
        let mut data = util::read_json_object(self.absolute(META));
        if !data.get("pinned").is_some_and(Value::is_array) {
            data.insert("pinned".into(), Value::Array(Vec::new()));
        }
        data
    }

    /// Written to a file beside it, then moved into place, so a crash or a
    /// full disk mid-write leaves the old pins rather than half a file.
    pub fn save_meta(&self, data: &Map<String, Value>) {
        let _ = util::write_atomic(self.absolute(META), util::json_compact(data));
    }

    fn pinned_list(data: &Map<String, Value>) -> Vec<String> {
        data.get("pinned")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn pinned(&self) -> Vec<String> {
        Self::pinned_list(&self.meta())
    }

    pub fn is_pinned(&self, rel: &str) -> bool {
        self.pinned().iter().any(|p| p == rel)
    }

    pub fn set_pinned(&self, rel: &str, on: bool) {
        let mut data = self.meta();
        let mut list: Vec<String> = Self::pinned_list(&data)
            .into_iter()
            .filter(|p| p != rel)
            .collect();
        if on {
            list.push(rel.to_string());
        }
        data.insert("pinned".into(), list.into());
        self.save_meta(&data);
    }

    /// A note moved or renamed keeps its pin, and stays the note open last.
    fn repin(&self, from: &str, to: &str) {
        let mut data = self.meta();
        let list: Vec<String> = Self::pinned_list(&data)
            .into_iter()
            .map(|p| if p == from { to.to_string() } else { p })
            .collect();
        data.insert("pinned".into(), list.into());
        if data.get("last").and_then(Value::as_str) == Some(from) {
            data.insert("last".into(), to.into());
        }
        self.save_meta(&data);
    }

    /// The note open last, which the app opens again when it starts.
    pub fn set_last(&self, rel: Option<&str>) {
        let mut data = self.meta();
        if data.get("last").and_then(Value::as_str) == rel {
            return;
        }
        match rel {
            Some(rel) => {
                data.insert("last".into(), rel.into());
            }
            None => {
                data.shift_remove("last");
            }
        }
        self.save_meta(&data);
    }

    /// The note open last, while it is still a note and not in Recently
    /// Deleted.
    pub fn last(&self) -> Option<PathBuf> {
        let rel = self.meta().get("last")?.as_str()?.to_string();
        if rel.starts_with(&format!("{TRASH}/")) {
            return None;
        }
        let path = self.absolute(&rel);
        path.is_file().then_some(path)
    }

    // ── One app at a time ─────────────────────────────────────────────────

    /// Takes `.notes.lock`, or gives the process ID holding it. Notes save
    /// themselves as you type, so two apps open on the same notes would
    /// write over each other: the one running holds the lock, with its
    /// process ID, and another finding it held by a live process doesn't
    /// start. A lock left by a process that has gone is taken over.
    pub fn lock(&mut self) -> Result<(), i64> {
        self.ensure();
        let path = self.absolute(LOCK);
        let me = std::process::id() as i64;
        let holder = util::read_text(&path).and_then(|t| t.trim().parse::<i64>().ok());
        if let Some(holder) = holder
            && holder != me
            && util::is_alive(holder)
        {
            return Err(holder);
        }
        let _ = fs::write(&path, me.to_string());
        Ok(())
    }

    /// Lets go of the lock, if this process holds it.
    pub fn unlock(&self) {
        let path = self.absolute(LOCK);
        let me = std::process::id() as i64;
        if util::read_text(&path).and_then(|t| t.trim().parse::<i64>().ok()) == Some(me) {
            let _ = fs::remove_file(path);
        }
    }

    // ── Notes ─────────────────────────────────────────────────────────────

    /// A file name for a title in `folder` that no other file has
    /// (`except`, the note's own when it is being renamed, may be the one).
    pub fn file_for(&self, folder: &str, title: &str, except: Option<&Path>) -> PathBuf {
        let base = base_for(title);
        let dir = self.absolute(folder);
        let mut candidate = dir.join(format!("{base}.md"));
        let mut n = 2;
        while candidate.exists() && Some(candidate.as_path()) != except {
            candidate = dir.join(format!("{base} {n}.md"));
            n += 1;
        }
        candidate
    }

    fn entry(&self, path: &Path) -> Option<NoteEntry> {
        let meta = fs::symlink_metadata(path).ok()?;
        if !meta.is_file() {
            return None;
        }
        // Without its ID line, which no list, search or tag should find.
        let text = id::strip(
            &fs::read(path)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default(),
        );
        let rel = self.relative(path);
        Some(NoteEntry {
            path: path.to_path_buf(),
            folder: dirname(&rel).to_string(),
            rel,
            title: title_of(&text),
            preview: preview_of(&text),
            mtime: mtime(path).unwrap_or(0),
            text,
            pinned: false,
            matched: None,
        })
    }

    /// Every note in the notes directory, or in `folder` and the folders
    /// under it, or in Recently Deleted when `folder` is `TRASH`. Pinned
    /// notes first, then newest first.
    pub fn notes(&self, folder: Option<&str>) -> Vec<NoteEntry> {
        let start = match folder {
            Some(f) if !f.is_empty() => self.absolute(f),
            _ => self.root.clone(),
        };
        let in_trash = folder == Some(TRASH);
        let mut found = Vec::new();
        for (rel, is_dir) in walk(&start) {
            // Hidden files and folders (Recently Deleted among them) aren't
            // notes, except when Recently Deleted is what was asked for.
            let hidden = rel.starts_with('.') || rel.contains("/.");
            if !is_dir
                && rel.ends_with(".md")
                && (in_trash || !hidden)
                && let Some(entry) = self.entry(&start.join(&rel))
            {
                found.push(entry);
            }
        }
        let pinned = self.pinned();
        for note in &mut found {
            note.pinned = pinned.contains(&note.rel);
        }
        sort_notes(&mut found);
        found
    }

    /// The folders, in tree order, each with how deep it is and how many
    /// notes it holds directly.
    pub fn folders(&self) -> Vec<Folder> {
        let mut list: Vec<String> = walk(&self.root)
            .into_iter()
            .filter(|(rel, is_dir)| *is_dir && !rel.starts_with('.') && !rel.contains("/."))
            .map(|(rel, _)| rel)
            .collect();
        list.sort_by_key(|a| a.to_lowercase());
        list.into_iter()
            .map(|rel| {
                let count = fs::read_dir(self.absolute(&rel))
                    .map(|dir| {
                        dir.flatten()
                            .filter(|e| {
                                e.file_type().is_ok_and(|t| t.is_file())
                                    && e.file_name().to_string_lossy().ends_with(".md")
                            })
                            .count()
                    })
                    .unwrap_or(0);
                Folder {
                    name: rel.rsplit('/').next().unwrap_or(&rel).to_string(),
                    depth: rel.matches('/').count(),
                    count,
                    rel,
                }
            })
            .collect()
    }

    /// A new note in `folder` ("" for the notes directory itself), with
    /// `title` as its first line, written as a title, and an ID of its own.
    /// Without one, the first line is an empty title to type, and the file
    /// is "New Note" until there is one.
    pub fn create(&mut self, folder: &str, title: Option<&str>) -> PathBuf {
        self.ensure();
        let title = title.map(str::trim).unwrap_or("");
        let path = self.file_for(
            folder,
            if title.is_empty() { "New Note" } else { title },
            None,
        );
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(&path, id::with(&format!("# {title}\n\n"), &id::new()));
        path
    }

    /// Renames a folder in place, to `name` in the same parent, keeping its
    /// notes' pins and the note open last. A name has no slash, doesn't
    /// start with a dot and isn't .., so a rename can't move a folder out of
    /// the notes directory, into another folder, or out of sight. Gives its
    /// new path, or why not.
    pub fn rename_folder(&self, from: &str, name: &str) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() || name.starts_with('.') || name.contains('/') || name.contains('\\') {
            return Err(
                "a folder's name can't be empty, start with a dot, or have a slash in it".into(),
            );
        }
        let parent = dirname(from);
        let to = if parent.is_empty() {
            name.to_string()
        } else {
            format!("{parent}/{name}")
        };
        if self.absolute(&to).exists() {
            return Err(format!("{to} is there already"));
        }
        fs::rename(self.absolute(from), self.absolute(&to)).map_err(|e| e.to_string())?;
        let moved = |p: &str| -> Option<String> {
            p.strip_prefix(&format!("{from}/"))
                .map(|rest| format!("{to}/{rest}"))
        };
        let mut data = self.meta();
        let list: Vec<String> = Self::pinned_list(&data)
            .into_iter()
            .map(|p| moved(&p).unwrap_or(p))
            .collect();
        data.insert("pinned".into(), list.into());
        if let Some(last) = data.get("last").and_then(Value::as_str).and_then(moved) {
            data.insert("last".into(), last.into());
        }
        self.save_meta(&data);
        self.relink(&moved, None);
        Ok(to)
    }

    pub fn create_folder(&self, rel: &str) -> Option<String> {
        let rel = rel.trim().trim_matches('/');
        if rel.is_empty() || rel.starts_with('.') || rel.contains("/.") {
            return None;
        }
        fs::create_dir_all(self.absolute(rel)).ok()?;
        Some(rel.to_string())
    }

    /// The tabs linked to a note (`@tab_note`, `@tab_note_last` and
    /// `@tab_note_arrived`: paths in the notes directory) follow it when it
    /// is renamed, moved or squashed into another: `follow` gives a linked
    /// path's new one, or `None` to leave it. With `title`, a tab linked to
    /// that note and named after it, as a tab opened for a note is, takes
    /// the new title as its name. The note the mode that works a note
    /// continuously is on follows too.
    pub fn relink(&self, follow: &dyn Fn(&str) -> Option<String>, title: Option<&Retitled>) {
        self.relink_mode(follow);
        if !self.server {
            return;
        }
        let Ok(listed) = crate::mux::socket::ask_ok(&["list"]) else {
            return;
        };
        let tabs = tab_links(&listed);
        for (tab, option, value) in relink_plan(&tabs, follow, title) {
            let _ = crate::mux::socket::ask(&["set", "-t", &tab, &option, &value]);
        }
    }

    /// `autopilot.json`'s note, followed to its new path, and the words
    /// saying it couldn't be found let go.
    fn relink_mode(&self, follow: &dyn Fn(&str) -> Option<String>) {
        let file = self.state.join("autopilot.json");
        let Some(Value::Object(mut mode)) = util::read_json(&file) else {
            return;
        };
        let Some(to) = mode
            .get("note")
            .and_then(Value::as_str)
            .filter(|n| !n.is_empty())
            .and_then(follow)
        else {
            return;
        };
        mode.insert("note".into(), to.into());
        if mode
            .get("said")
            .and_then(Value::as_str)
            .is_some_and(|s| s.starts_with("Can't find the note"))
        {
            mode.insert("said".into(), "".into());
        }
        let _ = util::write_atomic(&file, util::json_pretty(&mode));
    }

    /// The IDs autopilot keeps for notes squashed into one
    /// (`renumber`, each squashed note's ID to the merged note's) follow
    /// too: its note's, its skipped items', its landings' and its tabs'
    /// (`@autopilot_note`),
    /// whichever of the squashed notes they were.
    pub fn renumber(&self, renumber: &std::collections::HashMap<String, String>) {
        if renumber.is_empty() {
            return;
        }
        let file = self.state.join("autopilot.json");
        if let Some(Value::Object(mut mode)) = util::read_json(&file) {
            let mut changed = false;
            if let Some(to) = mode
                .get("noteId")
                .and_then(Value::as_str)
                .and_then(|id| renumber.get(id))
            {
                mode.insert("noteId".into(), to.clone().into());
                changed = true;
            }
            for key in ["skipped", "landed"] {
                let Some(Value::Array(items)) = mode.get_mut(key) else {
                    continue;
                };
                for item in items.iter_mut() {
                    let Some(to) = item
                        .get("note")
                        .and_then(Value::as_str)
                        .and_then(|id| renumber.get(id))
                        .cloned()
                    else {
                        continue;
                    };
                    item["note"] = to.into();
                    changed = true;
                }
            }
            if changed {
                let _ = util::write_atomic(&file, util::json_pretty(&mode));
            }
        }
        if !self.server {
            return;
        }
        let Ok(listed) = crate::mux::socket::ask_ok(&["list"]) else {
            return;
        };
        for tab in tab_links(&listed) {
            if let Some(to) = renumber.get(&tab.pilot_note) {
                let _ = crate::mux::socket::ask(&["set", "-t", &tab.id, "@autopilot_note", to]);
            }
        }
    }

    /// Moves a note's file, keeping its pin and the tabs linked to it, and
    /// gives its new path. A note going to or coming back from Recently
    /// Deleted leaves its tabs alone: a deleted note is no tab's note.
    fn move_file(&self, path: &Path, to: &Path) -> PathBuf {
        if path == to {
            return path.to_path_buf();
        }
        let from_rel = self.relative(path);
        if let Some(dir) = to.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if fs::rename(path, to).is_err() {
            return path.to_path_buf();
        }
        carry_attachments(path, to);
        let to_rel = self.relative(to);
        self.repin(&from_rel, &to_rel);
        if !is_trash(&from_rel) && !is_trash(&to_rel) {
            self.relink(&|rel| (rel == from_rel).then(|| to_rel.clone()), None);
        }
        to.to_path_buf()
    }

    /// A note renamed or moved outside the app, found again by its ID: its
    /// pin, the last note open and the tabs linked to it follow, as they do
    /// for a move made here, and with `title`, the tabs named after it take
    /// its new title. The images it names are copied along to a new
    /// folder, as a move here copies them.
    pub fn moved_outside(&self, from: &str, to: &str, title: Option<&Retitled>) {
        carry_attachments(&self.absolute(from), &self.absolute(to));
        self.repin(from, to);
        self.set_last(Some(to));
        self.relink(&|rel| (rel == from).then(|| to.to_string()), title);
    }

    /// A note whose title changed gets a file named for the new one. Gives
    /// its path, which is the old one when the name already fits.
    pub fn rename_to_title(&self, path: &Path) -> PathBuf {
        let Some(text) = util::read_text(path) else {
            return path.to_path_buf();
        };
        let folder = dirname(&self.relative(path)).to_string();
        let wanted = self.file_for(&folder, &title_of(&text), Some(path));
        let base = |p: &Path| NUMBERED.replace(&basename(p), ".md").into_owned();
        if base(&wanted) == base(path) {
            return path.to_path_buf();
        }
        self.move_file(path, &wanted)
    }

    /// Moves a note to another folder.
    pub fn move_note(&self, path: &Path, folder: &str) -> PathBuf {
        let text = util::read_text(path).unwrap_or_default();
        self.move_file(path, &self.file_for(folder, &title_of(&text), None))
    }

    /// Deleting moves a note to Recently Deleted, keeping the folder it was
    /// in, so restoring puts it back; deleting from there removes it for
    /// good, and gives `None`.
    pub fn delete(&self, path: &Path) -> Option<PathBuf> {
        let rel = self.relative(path);
        if self.meta().get("last").and_then(Value::as_str) == Some(rel.as_str()) {
            self.set_last(None);
        }
        if is_trash(&rel) {
            let _ = fs::remove_file(path);
            self.set_pinned(&rel, false);
            return None;
        }
        self.set_pinned(&rel, false);
        let folder = match dirname(&rel) {
            "" => TRASH.to_string(),
            f => format!("{TRASH}/{f}"),
        };
        let text = util::read_text(path).unwrap_or_default();
        Some(self.move_file(path, &self.file_for(&folder, &title_of(&text), None)))
    }

    /// Puts a note from Recently Deleted back in the folder it came from.
    pub fn restore(&self, path: &Path) -> PathBuf {
        let rel = self.relative(path);
        let folder = dirname(&rel)
            .strip_prefix(TRASH)
            .map(|f| f.trim_start_matches('/'))
            .unwrap_or("")
            .to_string();
        let text = util::read_text(path).unwrap_or_default();
        self.move_file(path, &self.file_for(&folder, &title_of(&text), None))
    }

    pub fn in_trash(&self, path: &Path) -> bool {
        is_trash(&self.relative(path))
    }

    // ── Search, tags and links ────────────────────────────────────────────

    /// Notes whose title or text holds every word of `query`, ignoring
    /// case, each with the first line that matches.
    pub fn search(&self, query: &str) -> Vec<NoteEntry> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if words.is_empty() {
            return Vec::new();
        }
        self.notes(None)
            .into_iter()
            .filter_map(|mut note| {
                let hay = note.text.to_lowercase();
                if !words.iter().all(|w| hay.contains(w.as_str())) {
                    return None;
                }
                note.matched = note
                    .text
                    .split('\n')
                    .filter(|l| !l.is_empty())
                    .find(|l| l.to_lowercase().contains(words[0].as_str()))
                    .map(|l| l.trim().to_string());
                Some(note)
            })
            .collect()
    }

    /// Every tag in every note, with how many notes have it, by name.
    pub fn tags(&self) -> Vec<(String, usize)> {
        let mut counts: Vec<(String, usize)> = Vec::new();
        for note in self.notes(None) {
            for tag in tags_in(&note.text) {
                match counts
                    .iter_mut()
                    .find(|(name, _)| name.to_lowercase() == tag.to_lowercase())
                {
                    Some((_, n)) => *n += 1,
                    None => counts.push((tag, 1)),
                }
            }
        }
        counts.sort_by_key(|a| a.0.to_lowercase());
        counts
    }

    pub fn tagged(&self, tag: &str) -> Vec<NoteEntry> {
        let tag = tag.to_lowercase();
        self.notes(None)
            .into_iter()
            .filter(|n| tags_in(&n.text).iter().any(|t| t.to_lowercase() == tag))
            .collect()
    }

    /// The note a [[link]] names, by its title, ignoring case: the newest
    /// when several have that title.
    pub fn find_by_title(&self, title: &str) -> Option<NoteEntry> {
        let wanted = title.trim().to_lowercase();
        self.notes(None)
            .into_iter()
            .find(|n| n.title.to_lowercase() == wanted)
    }

    pub fn titles(&self) -> Vec<String> {
        self.notes(None).into_iter().map(|n| n.title).collect()
    }

    /// A note opened while Notes sits beside a tab, other than the one it
    /// came with (`@tab_note_arrived`), becomes the tab's last note
    /// (`@tab_note_last`) at once, so quitting Notes there keeps it, and the
    /// marker is dropped, so going back to the one it came with counts too.
    /// Never in Notes' own tab (`@notes`). In the background, and nothing
    /// outside a Switchboard pane.
    pub fn opened_beside(&self, rel: &str) {
        if !self.server {
            return;
        }
        let Ok(pane) = std::env::var("SWITCHBOARD_PANE") else {
            return;
        };
        let rel = rel.to_string();
        std::thread::spawn(move || {
            use crate::mux::socket::{ask, ask_ok};
            let Ok(tab) = ask_ok(&["display", "-t", &pane, "tab"]) else {
                return;
            };
            let tab = tab.trim().to_string();
            let show = |option: &str| {
                ask_ok(&["show", "-t", &tab, option])
                    .map(|v| v.trim().to_string())
                    .unwrap_or_default()
            };
            if show("@notes") == "1" || show("@tab_note_arrived") == rel {
                return;
            }
            let _ = ask(&["set", "-t", &tab, "@tab_note_last", &rel]);
            let _ = ask(&["set", "-t", &tab, "-u", "@tab_note_arrived"]);
        });
    }
}

fn is_trash(rel: &str) -> bool {
    rel.starts_with(&format!("{TRASH}/"))
}

static NUMBERED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" \d+\.md$").unwrap());

/// Every file and folder under `start`, as paths relative to it, twenty
/// deep at most; links are left alone, as Neovim's `vim.fs.dir` left them.
fn walk(start: &Path) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    fn go(dir: &Path, prefix: &str, depth: usize, out: &mut Vec<(String, bool)>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if kind.is_dir() {
                out.push((rel.clone(), true));
                if depth < 20 {
                    go(&entry.path(), &rel, depth + 1, out);
                }
            } else if kind.is_file() {
                out.push((rel, false));
            }
        }
    }
    go(start, "", 1, &mut out);
    out
}

/// Pinned first, then newest, then by title.
pub fn sort_notes(notes: &mut [NoteEntry]) {
    notes.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then(b.mtime.cmp(&a.mtime))
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
}

/// The images a note names copied along when it moves to another folder
/// (`![name](.attachments/…)`, a path from the note's folder), so its links
/// still find them there; copied, not moved, since another note (a squash's
/// original, restored) may still name them where they were. One already at
/// the new place is left as it is.
fn carry_attachments(from: &Path, to: &Path) {
    let (Some(old), Some(new)) = (from.parent(), to.parent()) else {
        return;
    };
    if old == new {
        return;
    }
    let text = util::read_text(to).unwrap_or_default();
    copy_attachments(&text, &[old], new);
}

static ATTACHMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!\[[^\]]*\]\((\.attachments/[^)\s]+)\)").unwrap());

/// Each `.attachments/` image `text` names, copied into `here` from the
/// first of `folders` that has it, unless `here` has it already.
pub fn copy_attachments(text: &str, folders: &[&Path], here: &Path) {
    for cap in ATTACHMENT.captures_iter(text) {
        let rel = &cap[1];
        if rel.contains("..") || here.join(rel).exists() {
            continue;
        }
        if let Some(src) = folders.iter().map(|f| f.join(rel)).find(|p| p.exists()) {
            let dst = here.join(rel);
            if let Some(dir) = dst.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::copy(src, dst);
        }
    }
}

/// The tabs' links to notes from the server's `list`.
pub fn tab_links(listed: &str) -> Vec<TabLinks> {
    let Ok(value) = serde_json::from_str::<Value>(listed) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for session in value["sessions"].as_array().into_iter().flatten() {
        for tab in session["tabs"].as_array().into_iter().flatten() {
            let opt = |name: &str| {
                tab["options"][name]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            };
            out.push(TabLinks {
                id: tab["id"].as_str().unwrap_or_default().to_string(),
                note: opt("@tab_note"),
                last: opt("@tab_note_last"),
                arrived: opt("@tab_note_arrived"),
                name: opt("@tab_name"),
                pilot_note: opt("@autopilot_note"),
            });
        }
    }
    out
}

/// What relinking sets on each tab: (tab, option, value).
pub fn relink_plan(
    tabs: &[TabLinks],
    follow: &dyn Fn(&str) -> Option<String>,
    title: Option<&Retitled>,
) -> Vec<(String, String, String)> {
    let mut plan = Vec::new();
    for tab in tabs {
        let followed = |rel: &str| (!rel.is_empty()).then(|| follow(rel)).flatten();
        let to_note = followed(&tab.note);
        let to_last = followed(&tab.last);
        let to_arrived = followed(&tab.arrived);
        for (option, to) in [
            ("@tab_note_arrived", &to_arrived),
            ("@tab_note", &to_note),
            ("@tab_note_last", &to_last),
        ] {
            if let Some(to) = to {
                plan.push((tab.id.clone(), option.to_string(), to.clone()));
            }
        }
        if let Some(t) = title {
            let linked =
                to_note.is_some() || to_last.is_some() || tab.note == t.path || tab.last == t.path;
            if linked && tab.name.trim() == t.from.trim() && tab.name != t.to {
                plan.push((tab.id.clone(), "@tab_name".into(), t.to.clone()));
            }
        }
    }
    plan
}

// ── Titles, previews, tags ──────────────────────────────────────────────────

static HEADING_MARKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*#+\s*").unwrap());

/// The title from a note's text: its first line that isn't blank, without
/// Markdown's heading marks, or "New Note" for an empty one.
pub fn title_of(text: &str) -> String {
    id::strip(text)
        .split('\n')
        .map(|line| HEADING_MARKS.replace(line, "").trim().to_string())
        .find(|t| !t.is_empty())
        .unwrap_or_else(|| "New Note".into())
}

static PREVIEW_MARKER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*[#>*\-]+\s*").unwrap());
static PREVIEW_BOX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[[ xX]\]\s*").unwrap());
static PREVIEW_MARKUP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[*_~=`|]").unwrap());

/// The text under the title, flattened to one line, for the list.
pub fn preview_of(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut seen_title = false;
    for line in id::strip(text).split('\n') {
        if !seen_title {
            if !line.trim().is_empty() {
                seen_title = true;
            }
            continue;
        }
        let plain = PREVIEW_MARKER.replace(line, "");
        let plain = PREVIEW_BOX.replace_all(&plain, "");
        let plain = PREVIEW_MARKUP.replace_all(&plain, "").trim().to_string();
        if !plain.is_empty() {
            lines.push(plain);
        }
        if lines.join(" ").len() > 80 {
            break;
        }
    }
    lines.join(" ")
}

/// A file name from a title: what a file name can hold, cut to a sensible
/// length.
fn base_for(title: &str) -> String {
    let base: String = title
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c == ':' || c.is_ascii_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    let base = base.trim_start_matches('.');
    let base = base.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut cut = base.trim().to_string();
    if cut.len() > 80 {
        let mut end = 80;
        while !cut.is_char_boundary(end) {
            end -= 1;
        }
        cut.truncate(end);
    }
    if cut.is_empty() {
        "New Note".into()
    } else {
        cut
    }
}

static CODE_SPAN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`[^`]*`").unwrap());
static TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s#([A-Za-z0-9][A-Za-z0-9_\-/]*)").unwrap());

/// The #tags in a text: a # right after the start of a line or a space,
/// then letters, digits, - or _, not in code. Headings ("# Title") have a
/// space after the #, so they aren't tags.
pub fn tags_in(text: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let mut seen = HashSet::new();
    let mut in_code = false;
    for line in text.split('\n') {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        let plain = format!(" {}", CODE_SPAN.replace_all(line, ""));
        for cap in TAG.captures_iter(&plain) {
            let tag = cap[1].to_string();
            if seen.insert(tag.to_lowercase()) {
                tags.push(tag);
            }
        }
    }
    tags
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A notes directory of its own for a test, removed when it ends.
    pub struct Dir {
        pub path: PathBuf,
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    pub fn temp(name: &str) -> (Dir, Store) {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("notes-test-{}-{name}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        let mut store = Store::at(&path);
        store.state = path.join(".state-for-test");
        (
            Dir {
                path: store.root().to_path_buf(),
            },
            store,
        )
    }

    pub fn write(path: &Path, text: &str) {
        fs::write(path, text).unwrap();
    }

    pub fn set_mtime(path: &Path, secs: i64) {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs as u64);
        let f = fs::File::options().write(true).open(path).unwrap();
        f.set_modified(t).unwrap();
    }

    #[test]
    fn a_notes_title_is_its_first_line_and_its_file_is_named_for_it() {
        let (_dir, mut s) = temp("titles");
        let a = s.create("", Some("Grocery list"));
        let b = s.create("", Some("Grocery list"));
        let blank = s.create("", None);
        write(&a, "# Weekend plans\n\nhike\n");
        let renamed = s.rename_to_title(&a);
        let again = s.rename_to_title(&renamed);
        assert_eq!(s.relative(&a), "Grocery list.md");
        assert_eq!(
            s.relative(&b),
            "Grocery list 2.md",
            "a second note with that title doesn't overwrite the first"
        );
        assert_eq!(s.relative(&blank), "New Note.md");
        assert_eq!(
            title_of(&fs::read_to_string(&blank).unwrap()),
            "New Note",
            "an empty title reads as New Note"
        );
        assert_eq!(s.relative(&renamed), "Weekend plans.md");
        assert_eq!(
            s.relative(&again),
            "Weekend plans.md",
            "a name that fits is left alone"
        );
        assert_eq!(
            title_of("\n\nJust words\nmore"),
            "Just words",
            "a first line without # is the title too"
        );
        assert_eq!(
            preview_of("# Title\n\n- [ ] eggs\n**milk** and ==bread==\n"),
            "eggs milk and bread"
        );
        assert!(s.root().join("Weekend plans.md").exists());
        assert!(!s.root().join("Grocery list.md").exists());
    }

    #[test]
    fn notes_list_pinned_first_then_newest_and_recently_deleted_keeps_a_note() {
        let (_dir, mut s) = temp("list");
        let old = s.create("", Some("Old"));
        let new = s.create("Work/Projects", Some("Launch"));
        let pinned = s.create("Work", Some("Pinned one"));
        let now = util::now_secs();
        set_mtime(&old, now - 500);
        set_mtime(&pinned, now - 900);
        s.set_pinned(&s.relative(&pinned), true);
        let titles = |list: Vec<NoteEntry>| list.into_iter().map(|n| n.title).collect::<Vec<_>>();
        assert_eq!(titles(s.notes(None)), ["Pinned one", "Launch", "Old"]);
        assert_eq!(
            titles(s.notes(Some("Work"))),
            ["Pinned one", "Launch"],
            "a folder shows the notes in the folders under it too"
        );
        let folders: Vec<(String, usize, usize)> = s
            .folders()
            .into_iter()
            .map(|f| (f.rel, f.depth, f.count))
            .collect();
        assert_eq!(
            folders,
            [
                ("Work".to_string(), 0, 1),
                ("Work/Projects".to_string(), 1, 1)
            ]
        );
        // Moved, a pinned note stays pinned.
        let moved = s.move_note(&pinned, "");
        assert!(s.is_pinned(&s.relative(&moved)));
        let trashed = s.delete(&new).unwrap();
        assert_eq!(
            s.relative(&trashed),
            ".trash/Work/Projects/Launch.md",
            "it remembers the folder it came from"
        );
        assert_eq!(s.notes(None).len(), 2, "and is out of every other list");
        let in_trash: Vec<String> = s.notes(Some(TRASH)).into_iter().map(|n| n.rel).collect();
        assert_eq!(in_trash, [".trash/Work/Projects/Launch.md"]);
        let restored = s.restore(&trashed);
        assert_eq!(s.relative(&restored), "Work/Projects/Launch.md");
        let gone = s.delete(&s.delete(&old).unwrap());
        assert!(gone.is_none());
        assert_eq!(
            s.notes(Some(TRASH)).len(),
            0,
            "deleted from Recently Deleted, it is gone for good"
        );
        assert!(!s.root().join(".trash/Old.md").exists());
    }

    #[test]
    fn a_folder_is_renamed_in_place_and_never_out_of_the_notes_or_out_of_sight() {
        let (_dir, mut s) = temp("folders");
        s.create("Work", Some("Plan"));
        s.set_pinned("Work/Plan.md", true);
        s.set_last(Some("Work/Plan.md"));
        s.create_folder("Home");
        for name in ["../Out", ".hidden", "a/b", "..", "  ", "Home"] {
            assert!(s.rename_folder("Work", name).is_err(), "{name:?} refused");
        }
        assert_eq!(s.rename_folder("Work", " Projects ").unwrap(), "Projects");
        assert_eq!(s.pinned(), ["Projects/Plan.md"]);
        assert_eq!(s.meta()["last"], "Projects/Plan.md");
        assert!(s.root().join("Projects/Plan.md").exists());
        assert!(
            !s.root().parent().unwrap().join("Out").exists(),
            "nothing moved out"
        );
    }

    #[test]
    fn search_finds_every_word_and_tags_come_from_words_outside_code() {
        let (_dir, mut s) = temp("search");
        let a = s.create("", Some("Trip"));
        write(
            &a,
            "# Trip\n\nPack the #travel bag and #Camping gear\n\n`#notatag`\n```\n#nope\n```\nissue#3 is not a tag\n",
        );
        let b = s.create("Work", Some("Standup"));
        write(&b, "# Standup\n\nTalked about the #travel budget\n");
        let titles = |list: Vec<NoteEntry>| list.into_iter().map(|n| n.title).collect::<Vec<_>>();
        assert_eq!(titles(s.search("TRAVEL bag")), ["Trip"]);
        assert_eq!(
            s.search("budget")[0].matched.as_deref(),
            Some("Talked about the #travel budget")
        );
        assert!(s.search("submarine").is_empty());
        assert_eq!(
            s.tags(),
            [("Camping".to_string(), 1), ("travel".to_string(), 2)]
        );
        assert_eq!(
            titles(s.tagged("camping")),
            ["Trip"],
            "tags match without regard to case"
        );
        assert_eq!(
            s.find_by_title("  standup ").unwrap().rel,
            "Work/Standup.md"
        );
        assert!(s.find_by_title("Nope").is_none());
    }

    #[test]
    fn only_one_app_holds_the_lock_and_one_left_by_a_process_gone_is_taken_over() {
        let (_dir, mut s) = temp("lock");
        // Held by a live process: this test's parent, which is alive.
        let parent = unsafe { libc::getppid() } as i64;
        fs::write(s.absolute(LOCK), parent.to_string()).unwrap();
        assert_eq!(s.lock(), Err(parent));
        assert_eq!(
            fs::read_to_string(s.absolute(LOCK)).unwrap(),
            parent.to_string(),
            "the lock is left to its holder"
        );
        fs::write(s.absolute(LOCK), "999999999").unwrap();
        assert_eq!(s.lock(), Ok(()));
        s.unlock();
        assert!(!s.absolute(LOCK).exists(), "gone once it lets go");
        s.set_pinned("x.md", true);
        assert!(
            !fs::read_dir(s.root())
                .unwrap()
                .flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".tmp")),
            "the pins were written whole"
        );
    }

    #[test]
    fn tabs_linked_to_a_note_follow_it_and_a_tab_named_for_it_takes_its_title() {
        let tabs = vec![
            TabLinks {
                id: "@1".into(),
                note: "Plans.md".into(),
                name: "Plans".into(),
                ..Default::default()
            },
            TabLinks {
                id: "@2".into(),
                note: "Plans.md".into(),
                name: "Release work".into(),
                ..Default::default()
            },
            TabLinks {
                id: "@3".into(),
                last: "Work/Ideas.md".into(),
                arrived: "Work/Ideas.md".into(),
                ..Default::default()
            },
        ];
        let follow = |rel: &str| (rel == "Plans.md").then(|| "Weekend plans.md".to_string());
        let title = Retitled {
            path: "Weekend plans.md".into(),
            from: "Plans".into(),
            to: "Weekend plans".into(),
        };
        let plan = relink_plan(&tabs, &follow, Some(&title));
        assert_eq!(
            plan,
            [
                (
                    "@1".to_string(),
                    "@tab_note".to_string(),
                    "Weekend plans.md".to_string()
                ),
                (
                    "@1".to_string(),
                    "@tab_name".to_string(),
                    "Weekend plans".to_string()
                ),
                (
                    "@2".to_string(),
                    "@tab_note".to_string(),
                    "Weekend plans.md".to_string()
                ),
            ],
            "a tab with a name of its own keeps it"
        );
        let folder = |rel: &str| rel.strip_prefix("Work/").map(|r| format!("Home/{r}"));
        let plan = relink_plan(&tabs, &folder, None);
        assert_eq!(
            plan,
            [
                (
                    "@3".to_string(),
                    "@tab_note_arrived".to_string(),
                    "Home/Ideas.md".to_string()
                ),
                (
                    "@3".to_string(),
                    "@tab_note_last".to_string(),
                    "Home/Ideas.md".to_string()
                ),
            ],
            "a renamed folder takes its notes' links along"
        );
        let listed = r#"{"sessions":[{"tabs":[{"id":"@4","options":{"@tab_note":"A.md","@tab_name":"A"}}]}]}"#;
        assert_eq!(
            tab_links(listed),
            [TabLinks {
                id: "@4".into(),
                note: "A.md".into(),
                name: "A".into(),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn a_note_renamed_takes_the_mode_working_it_along() {
        let (_dir, s) = temp("autopilot");
        fs::create_dir_all(&s.state).unwrap();
        let file = s.state.join("autopilot.json");
        fs::write(&file, r#"{"on":true,"note":"Plans.md","repo":"/r","slots":3,"cap":5,"skipped":[],"said":"Can't find the note Plans: set the mode up again from M-p"}"#).unwrap();
        s.relink(
            &|rel| (rel == "Plans.md").then(|| "Weekend plans.md".to_string()),
            None,
        );
        let mode = util::read_json(&file).unwrap();
        assert_eq!(mode["note"], "Weekend plans.md");
        assert_eq!(mode["on"], true);
        assert_eq!(mode["repo"], "/r");
        assert_eq!(mode["slots"], 3);
        assert_eq!(mode["said"], "", "which keeps its other state");
    }

    #[test]
    fn notes_squashed_into_one_take_the_modes_ids_along() {
        let (_dir, s) = temp("renumber");
        fs::create_dir_all(&s.state).unwrap();
        let file = s.state.join("autopilot.json");
        fs::write(
            &file,
            r#"{"noteId":"n-a","skipped":[{"note":"n-a","text":"x"}],"landed":[{"id":"3@1","note":"n-a"},{"id":"4@1","note":"n-c"}]}"#,
        )
        .unwrap();
        s.renumber(&[("n-a".to_string(), "n-b".to_string())].into());
        let mode = util::read_json(&file).unwrap();
        assert_eq!(mode["noteId"], "n-b");
        assert_eq!(mode["skipped"][0]["note"], "n-b");
        assert_eq!(mode["landed"][0]["note"], "n-b");
        assert_eq!(mode["landed"][1]["note"], "n-c", "another note's kept");
    }
}
