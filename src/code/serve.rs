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

/// The option marking the viewer's pane.
pub const CODE_VIEW: &str = "@code_view";

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
                let (top, repo) = find::repo_files(&dir).unwrap_or((dir, Vec::new()));
                (files, repo, top)
            },
            move |server, client, (files, repo, top)| {
                server.pick_code(client, tab, pane, files, repo, top);
            },
        );
    }

    /// The list `open_code` found, or why there is none.
    fn pick_code(
        &mut self,
        client: ClientId,
        tab: TabId,
        pane: PaneId,
        files: Vec<(Found, PathBuf)>,
        repo: Vec<String>,
        top: PathBuf,
    ) {
        if files.is_empty() && repo.is_empty() {
            return self.say(Some(client), "No file in this pane");
        }
        let on_screen = files.len();
        let mut items: Vec<PickItem> = files
            .iter()
            .map(|(found, file)| PickItem::new(found.row(), found.arg(file)))
            .collect();
        items.extend(repo.iter().map(|name| {
            let mut item = PickItem::new(name.clone(), top.join(name).display().to_string());
            item.tags = "repository".into();
            item
        }));
        let mut picker = Picker::new("Open a file from this pane", items)
            .named("code")
            .prompt("Open: ")
            .keep_order()
            .searched_from(on_screen);
        if on_screen == 0 {
            picker = picker.header("No file on the screen: type to search the repository's");
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
