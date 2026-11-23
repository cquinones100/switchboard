//! The server's side of the code viewer: M-p's "Open a file from this pane
//! in the side panel" (`views::Command::OpenCode`, behind Settings'
//! `code_view`). The paths are read from what the server already holds of
//! the pane's screen (`find::paths_in`), with nothing started or waited on
//! on its task; which of them are files, and the repository's files that
//! typing searches, are asked off it. The file picked opens in
//! `switchboard code` as the PR view opens: in the side panel beside a
//! narrow lone pane, in place of what is there (`clear_beside`), else in the
//! pane's place, marked `@code_view`, so a second file read replaces the
//! first. A file in an untrusted worktree opens too: reading and coloring
//! it runs nothing of it.

use std::path::PathBuf;

use super::find::{self, Found};
use crate::mux::server::{ClientId, Server};
use crate::mux::{PaneId, TabId};
use crate::ui::{PickItem, Picker};
use crate::{paths, settings};

/// The most of the repository's files typing searches: the picker filters
/// them on the server's task at each key, so a list beyond this would slow
/// every tab while you type; the box says when it was cut.
const MOST_FILES: usize = 50_000;

/// The longest the box waits on git to list them, the screen's files
/// shown with as many as it listed by then.
const LIST_WITHIN: std::time::Duration = std::time::Duration::from_secs(2);

/// The option marking the viewer's pane.
pub const CODE_VIEW: &str = "@code_view";

/// What `open_code` found off the server's task: the files the screen
/// names with where each is, the repository's files by their path from its
/// top, and whether that list was cut at `MOST_FILES`.
struct Listed {
    files: Vec<(Found, PathBuf)>,
    repo: Vec<String>,
    top: PathBuf,
    cut: bool,
}

impl Server {
    /// M-p's "Open a file from this pane in the side panel": the files the
    /// pane names, newest first, then the repository's, found by typing.
    pub(crate) fn open_code(&mut self, client: ClientId) {
        if !settings::is_on("code_view") {
            return self.say(
                Some(client),
                "Opening code files from a pane is off in Settings (code_view)",
            );
        }
        let Some((pane, tab, dir)) = self.here(client) else {
            return;
        };
        let Some(slot) = self.panes.get(&pane) else {
            return;
        };
        let named = find::paths_in(&slot.pane.text(3000));
        let home = paths::home();
        self.off_task(
            client,
            move || {
                let files: Vec<(Found, PathBuf)> = named
                    .into_iter()
                    .filter_map(|f| Some((find::resolve(&f.path, &dir, &home)?, f)))
                    .map(|(file, f)| (f, file))
                    .collect();
                let listed = find::repo_files(&dir, MOST_FILES, LIST_WITHIN);
                let (top, repo, cut) = match listed {
                    Some(l) => (l.top, l.files, l.cut),
                    None => (dir, Vec::new(), false),
                };
                Listed {
                    files,
                    repo,
                    top,
                    cut,
                }
            },
            move |server, client, listed| server.pick_code(client, tab, pane, listed),
        );
    }

    /// The list `open_code` found, or why there is none.
    fn pick_code(&mut self, client: ClientId, tab: TabId, pane: PaneId, listed: Listed) {
        let Listed {
            files,
            repo,
            top,
            cut,
        } = listed;
        if files.is_empty() && repo.is_empty() {
            return self.say(Some(client), "No file in this pane");
        }
        let on_screen = files.len();
        let mut items: Vec<PickItem> = files
            .iter()
            .map(|(found, file)| PickItem::new(found.row(), found.arg(file)))
            .collect();
        // A file the screen names is listed once, at the line it names;
        // a file's value always carries a line, so a name ending in
        // `:<digits>` isn't read as one.
        let shown: Vec<&PathBuf> = files.iter().map(|(_, file)| file).collect();
        items.extend(
            repo.iter()
                .map(|name| (name, top.join(name)))
                .filter(|(_, file)| !shown.contains(&file))
                .map(|(name, file)| PickItem::new(name.clone(), format!("{}:1", file.display()))),
        );
        let mut picker = Picker::new("Open a file from this pane", items)
            .named("code")
            .prompt("Open: ")
            .keep_order()
            .searched_from(on_screen);
        if on_screen == 0 {
            picker = picker.header("No file on the screen: type to search the repository's");
        }
        if cut {
            picker = picker.header(format!(
                "Typing searches the first {MOST_FILES} of the repository's files, or what git listed in {}s",
                LIST_WITHIN.as_secs()
            ));
        }
        self.about_tab(client, tab, move |s| {
            s.pick(client, picker, move |server, client, picked| {
                if let Some(picked) = picked {
                    server.open_code_view(client, tab, pane, picked.value);
                }
            })
        });
    }

    /// `switchboard code <arg>` beside `pane`, in place of a file read
    /// there before.
    pub(crate) fn open_code_view(
        &mut self,
        client: ClientId,
        tab: TabId,
        pane: PaneId,
        arg: String,
    ) {
        let pane = self.work_pane(tab, pane);
        let command = vec![crate::views::exe(), "code".into(), arg];
        // A file read before is replaced in its own pane, beside the pane or
        // in its place, so viewers never stack and the pane it stands in
        // for stays where it is.
        if let Some(old) = self.marked_in(tab, CODE_VIEW) {
            match self.replace_program(old, Some(command)) {
                Ok(()) => {
                    if let Some(slot) = self.panes.get_mut(&old) {
                        slot.opts.insert(CODE_VIEW.into(), "1".into());
                    }
                    self.focus_pane(old);
                }
                Err(e) => self.say(Some(client), &format!("Couldn't open the file: {e}")),
            }
            return;
        }
        let opened = match self.clear_beside(tab) {
            Some((lone, width)) => {
                self.open_in_place_keeping(lone, "code", command, None, Some(width))
            }
            None => self.open_in_place(pane, "code", command, None),
        };
        match opened {
            Ok(view) => {
                if let Some(slot) = self.panes.get_mut(&view) {
                    slot.opts.insert(CODE_VIEW.into(), "1".into());
                }
            }
            Err(e) => self.say(Some(client), &format!("Couldn't open the file: {e}")),
        }
    }
}
