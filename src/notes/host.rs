//! This program answering Notes for the workspace: the plugin API's `Host`,
//! for a plugin loaded in process. Whatever the notes app
//! (`switchboard-notes`) needs that isn't its own, it asks here, so each
//! rule the code keeps is kept in one place: a setting from `settings`, a
//! link opened by `links` in the browser its site is given, the desktop's
//! clipboard and viewer through `platform`, how a picture reaches the
//! terminal from `mux::graphics`, a request counted in `requests`, and
//! what a checklist item is to the workspace (whether work can be started
//! on it, which tab's branch was started for it, which autopilot works
//! next) from the modules that start that work.
//!
//! What Notes shares with the diff (a setting, the folders, a server
//! command, the clipboard written, the request audit, a headless Claude's
//! environment) is answered as the diff is (`diff::host::Core`). Its
//! folders are the workspace's own (`~/.local/state/switchboard` and
//! `~/.cache/switchboard`), not a `plugins/notes` under them, since its
//! files (`notes.sock`, the palette's picks, `notes-images`) were there
//! before it was a plugin, and the server finds its socket there.
//!
//! `switchboard notes` run outside a pane is answered here too: with no
//! server to ask, a server command has no answer.

use std::sync::Arc;

use serde_json::json;
use switchboard_plugin_api::{Action, Answer, Host};

use crate::mux::graphics;
use crate::{links, platform, settings};

/// The workspace, as Notes asks it.
pub struct Core;

/// The host every program of this binary gives Notes.
pub fn core() -> Arc<dyn Host> {
    Arc::new(Core)
}

/// Says Notes' modules, used here directly (a note's ID, the notes
/// directory, a note's pictures), who answers them: every program of the
/// binary does as it starts (`cmd::run`).
pub fn install() {
    switchboard_notes::host::install(core());
}

/// What the terminal an app draws in is given, as the API's words.
fn draws() -> String {
    let mut words = Vec::new();
    if graphics::apps_draw() {
        words.push("pictures");
    }
    if graphics::apps_round_ends() {
        words.push("round");
    }
    words.join(" ")
}

/// The items autopilot works next, of the note it works, while it is on.
fn queued(state: &std::path::Path) -> Option<serde_json::Value> {
    let mode = crate::autopilot::mode::Mode::read(state);
    (mode.on && settings::is_on("autopilot"))
        .then(|| json!({"note": mode.note_id, "items": mode.next_here()}))
}

impl Host for Core {
    fn ask(&self, action: Action) -> Answer {
        match action {
            Action::Settings { keys } => {
                let all = settings::read_settings();
                Answer::Values {
                    values: keys
                        .iter()
                        .map(|key| all.iter().find(|o| o.key == *key).map(|o| o.value.clone()))
                        .collect(),
                }
            }
            Action::Paste => match platform::paste() {
                Some(text) => Answer::Text { text },
                None => Answer::None,
            },
            Action::PasteImage { file } => Answer::Flag {
                on: platform::paste_image(&file),
            },
            Action::Preview { file } => match platform::preview(&file) {
                Ok(()) => Answer::Done,
                Err(_) => Answer::None,
            },
            Action::LinkWords { url } => {
                if links::says_it() {
                    Answer::Text {
                        text: links::opening_words(&url),
                    }
                } else {
                    Answer::None
                }
            }
            Action::OpenLink { url, watched } => {
                let opened = if watched {
                    links::open_url_watched(&url)
                } else {
                    links::open_url(&url).map(|_| false)
                };
                match opened {
                    Ok(on) => Answer::Flag { on },
                    Err(why) => Answer::Text {
                        text: why.to_string(),
                    },
                }
            }
            Action::Draws => Answer::Text { text: draws() },
            Action::Picture {
                id,
                cols,
                rows,
                png,
            } => Answer::Text {
                text: graphics::by_path(id, cols, rows, &png),
            },
            Action::PictureGone { id } => Answer::Text {
                text: graphics::take_back(id),
            },
            Action::ItemClaim { dir } => match crate::tabs::title::wt_claim_at(&dir) {
                Some((note, item)) => Answer::Value {
                    value: json!({"note": note, "item": item}),
                },
                None => Answer::None,
            },
            Action::Queued => match queued(&crate::paths::state_dir()) {
                Some(value) => Answer::Value { value },
                None => Answer::None,
            },
            Action::Item { text, line } => {
                match crate::cmd::worktree_from_note::item_at(&text, line) {
                    Ok(_) => Answer::Done,
                    Err(why) => Answer::Text { text: why.into() },
                }
            }
            shared => crate::diff::host::Core.ask(shared),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use switchboard_plugin_api::Manifest;

    #[test]
    fn notes_is_loaded_at_this_build_s_protocol_and_its_settings_are_the_schema_s() {
        let manifest: Manifest = crate::notes::plugin().manifest();
        assert_eq!(manifest.check(), Ok(()));
        assert_eq!(manifest.name, "notes");
        // Every setting it says it reads is listed, with that default.
        let schema = settings::schema();
        for declared in &manifest.settings {
            let listed = schema.iter().find(|o| o.key == declared.key);
            assert_eq!(
                listed.map(|o| &o.default),
                Some(&declared.default),
                "{}",
                declared.key
            );
        }
        // And every setting of the app's own (`notes_…`, but the one the
        // server reads to hide its list beside a pane) is one it says.
        for option in schema
            .iter()
            .filter(|o| o.key.starts_with("notes_") && o.category == "Notes")
            .filter(|o| o.key != "notes_hide_sidebar")
        {
            assert!(
                manifest.default_of(&option.key).is_some(),
                "{} isn't in Notes' manifest",
                option.key
            );
        }
    }

    #[test]
    fn the_workspace_answers_what_notes_asks() {
        let host = Core;
        assert_eq!(
            host.settings(&["notes_images", "notes_relative_numbers", "no such setting"]),
            vec![
                Some(serde_json::Value::Bool(true)),
                Some(serde_json::Value::Bool(false)),
                None
            ]
        );
        // What it shares with the diff is answered as the diff is.
        let folders = host.folders().unwrap();
        assert_eq!(folders.state, crate::paths::state_dir());
        assert_eq!(folders.cache, crate::paths::cache_dir());
        // Which line work can be started on is wt's to say.
        let note = "# Plan\n\n- [ ] one\n  under it\n- [x] done\n\n## Later\n\n- [ ] parked\n";
        assert_eq!(host.item_refused(note, 3), None);
        assert_eq!(host.item_refused(note, 5).as_deref(), Some("is ticked"));
        assert_eq!(
            host.item_refused(note, 9).as_deref(),
            Some("is in a section of parked work")
        );
        assert_eq!(
            host.item_refused(note, 1).as_deref(),
            Some("isn't a checklist item")
        );
        // A directory that is no branch's has no item, and autopilot never
        // turned on works none next.
        assert_eq!(host.item_claim(Path::new("/nonexistent/switchboard")), None);
        assert_eq!(queued(Path::new("/nonexistent/switchboard")), None);
        // A picture is placed by the one module that writes its command.
        let png = Path::new("/n/a.png");
        assert_eq!(
            host.picture(7, 10, 3, png),
            graphics::by_path(7, 10, 3, png)
        );
        assert_eq!(host.picture_gone(7), graphics::take_back(7));
    }
}
