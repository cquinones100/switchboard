//! A note's preview in a pane's place: M-p's "Preview a note…", which asks
//! which note, every note listed newest first as Notes' Find a note lists
//! them, and Notes' "Preview this note" (`press preview-note <note>`, from
//! its own pane), for the note it shows. The note is read by the views'
//! reader (`switchboard view --note`, src/views/note.rs), read-only and
//! drawn as the PR view draws Markdown, in the place of the pane it was
//! asked from, as M-d puts the diff there (src/diff/in_place.rs), so
//! quitting it with q or Esc, or the same command again, gives that pane
//! back where it was. Settings' notes_preview turns both commands off. New
//! with the preview, with no tmux-version file behind it.

use crate::mux::PaneId;
use crate::mux::server::{ClientId, Server};
use crate::notes::store::{self, Store};
use crate::settings;
use crate::ui::{PickItem, Picker};

use super::in_notes;

/// The command's name in keys.json and what Notes presses, and the
/// preview's tool name in its pane's `@in_place`.
pub const NAME: &str = "preview-note";

/// The question listing every note, the newest first with its age, as
/// Notes' Find a note lists them; `kit` as `pick::picker`'s.
fn picker(rows: Vec<super::pick::Row>, kit: Option<bool>) -> Picker {
    let items = rows
        .into_iter()
        .map(|row| PickItem {
            key: row.age,
            ..PickItem::new(row.text, row.rel)
        })
        .collect();
    let picker = Picker::new("Preview which note?", items);
    let picker = match kit {
        Some(caps) => picker.kit(caps),
        None => picker,
    };
    picker.named(NAME).prompt("Note: ").text_only().keep_order()
}

impl Server {
    /// Previews `note` (a path in the notes directory) in the place of
    /// `pane` (else the client's pane in front), or, with none named, asks
    /// which; closes the preview instead when it is that pane or stands
    /// for it (beside a narrow lone pane, `tool_for`).
    pub(super) fn preview_note(
        &mut self,
        client: ClientId,
        note: Option<String>,
        pane: Option<PaneId>,
    ) {
        if !settings::is_on("notes_preview") {
            self.say(
                Some(client),
                "Previewing a note is off (Settings' \"Preview a note read-only\").",
            );
            return;
        }
        let Some(pane) = pane.or_else(|| self.current_pane(client)) else {
            return;
        };
        let preview = self.tool_for(pane).filter(|tool| {
            self.panes
                .get(tool)
                .and_then(|s| s.opts.get(crate::diff::in_place::IN_PLACE))
                .is_some_and(|name| name == NAME)
        });
        if let Some(preview) = preview {
            self.kill_pane(preview);
            return;
        }
        match note {
            Some(note) => self.open_preview(client, pane, &note),
            None => self.ask_preview(client, pane),
        }
    }

    /// Asks on `client` which note to preview in `pane`'s place.
    fn ask_preview(&mut self, client: ClientId, pane: PaneId) {
        self.notes_off_task(
            || {
                let notes = Store::at(store::notes_dir()).notes(None);
                super::list(&notes, "", None, None, crate::util::now_secs()).all
            },
            move |server, rows| {
                if !server.clients.contains_key(&client) || !server.panes.contains_key(&pane) {
                    return;
                }
                if rows.is_empty() {
                    server.say(Some(client), "There is no note to preview yet.");
                    return;
                }
                let kit = settings::is_on("ux2").then(|| {
                    server
                        .clients
                        .get(&client)
                        .is_some_and(|c| crate::mux::graphics::round_ends(&c.term))
                });
                server.pick(client, picker(rows, kit), move |server, _, picked| {
                    if let Some(picked) = picked.filter(|p| !p.value.is_empty()) {
                        server.open_preview(client, pane, &picked.value);
                    }
                });
            },
        );
    }

    /// `note`'s preview in `pane`'s place.
    fn open_preview(&mut self, client: ClientId, pane: PaneId, note: &str) {
        if !in_notes(note) {
            self.say(Some(client), &format!("{note} isn't a note."));
            return;
        }
        let file = store::notes_dir().join(note);
        let command = vec![
            super::exe(),
            "view".into(),
            "--note".into(),
            file.to_string_lossy().into_owned(),
        ];
        if let Err(e) = self.open_in_place(pane, NAME, command, Some(client)) {
            self.say(Some(client), &format!("Couldn't preview the note: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_note_is_listed_newest_first_with_its_age() {
        let rows = vec![
            super::super::pick::Row {
                text: "Today".into(),
                rel: "Today.md".into(),
                age: "2h".into(),
            },
            super::super::pick::Row {
                text: "Old/Plans".into(),
                rel: "Old/Plans.md".into(),
                age: "3d".into(),
            },
        ];
        let p = picker(rows, None);
        assert_eq!(p.shown(), ["Today", "Old/Plans"], "in the list's order");
    }
}
