//! Notes: the macOS Notes app's notes, in the terminal, edited with Vim's
//! keys. Ported from the Neovim plugin in notes/ that bin/notes opened, and
//! taken out of the `switchboard` crate (src/notes) to be an app of its own
//! on Switchboard's plugin API, as the diff was before it.
//!
//! It runs two ways, the same program in both:
//!
//! - alone, as `switchboard-notes` (src/main.rs), in any terminal, with no
//!   server: it answers for the workspace itself (`host::Alone`), and what
//!   it can't do without one (go to a note's tab, start work on an item,
//!   open a link, show a picture) is left out;
//! - in the workspace, as `switchboard notes`, which the server runs beside
//!   a pane on M-e: `switchboard` loads it in its own process through the
//!   API's `Plugin` (`Notes`) and answers what it asks (`Host`).
//!
//! It reaches the workspace only through that API (`host`): its settings,
//! its folders, the server's commands (the tab's note, going to a tab, a
//! pane's options), the desktop's clipboard and viewer, a link opened by
//! the workspace's rules, how a picture reaches the terminal, the request
//! audit, and what the workspace knows of a checklist item (whether work
//! can be started on it, which tab works on it, which it works next). Of
//! the workspace's crates it depends on `switchboard-plugin-api` and
//! `switchboard-ui` alone, so the compiler holds that.
//!
//! Notes are Markdown files in `$NOTES_DIR`, or `~/Notes`, in folders that
//! are directories, never in this repository. On the left is a list of
//! folders, tags and notes; on the right, the note open last, drawn the way
//! Notes shows it: markup hidden (it shows on the line being typed in), a
//! title standing out from headings, list markers as • and –, checklist
//! items as ☐ and ☑, a quote with a bar, a table with box lines, pictures
//! under their lines. A note saves itself a moment after each change, and
//! its file is renamed when its title changes; one app at a time holds a
//! notes directory. Nothing in a note is sent anywhere but by squashing
//! notes, after a yes.
//!
//! One module per part, as the plugin had one Lua file per part:
//! - `store`: the files on disk (store.lua).
//! - `id`: a note's stable ID, its last line, which wtn's claims and the
//!   autopilot follow a note by (id.lua, lib/note-id.js).
//! - `text`: a note's lines, in a rope.
//! - `render`: a note drawn as Notes shows it (render.lua).
//! - `format`: the Format menu's commands and the typing it does
//!   (format.lua); `table` (table.lua), `fold` (fold.lua).
//! - `marks`: a note's marks; `jumps`: the jump list across notes, both
//!   Neovim's, new here.
//! - `editor`: Vim's keys.
//! - `sync`: notes changed from outside (sync.lua).
//! - `images`: pictures pasted and drawn (images.lua); `diagram`: mermaid
//!   blocks drawn as their diagrams, new here.
//! - `sidebar`: the list (sidebar.lua).
//! - `menu`: the Format menu and the boxes the app asks in.
//! - `palette`: Ctrl-Shift-P (palette.lua); `finder`: Ctrl-F (finder.lua).
//! - `easy`: the toolbar and easy mode (easy.lua).
//! - `squash`: notes merged by Claude (squash.lua).
//! - `plan`: an item's plan note, shown beside its note.
//! - `socket`: the socket the workspace shows a tab's note through.
//! - `app`: the program on the screen (init.lua).
//! - `host`: who answers for the workspace.
//!
//! Notes in the workspace, on the server (M-N, M-e, a note's tab, a tab's
//! note, the palette's commands), stays `switchboard`'s (its src/notes).

use std::process::ExitCode;
use std::sync::Arc;

use switchboard_plugin_api::{Event, Host, Manifest, PROTOCOL_VERSION, Plugin, Setting};

pub mod app;
pub mod diagram;
pub mod easy;
pub mod editor;
pub mod finder;
pub mod fold;
pub mod format;
pub mod host;
pub mod id;
pub mod images;
pub mod jumps;
pub mod marks;
pub mod menu;
pub mod palette;
pub mod plan;
pub mod render;
pub mod sidebar;
pub mod socket;
pub mod squash;
pub mod store;
pub mod sync;
pub mod table;
pub mod text;

use store::Store;
use switchboard_plugin_api::sdk as util;

/// Claude Code and the other programs' stand-ins, shared with
/// `switchboard`'s tests (tests/common/stand_in.rs).
#[cfg(test)]
#[path = "../../../tests/common/stand_in.rs"]
pub(crate) mod stand_in;

/// What Notes presses to start work on a checklist item (`gw`), with the
/// note and the item's line.
pub const WORK_ON_ITEM: &str = "work-on-item";

/// What Notes presses for M-Enter in "Add an item": a tab whose Claude adds
/// the item to the note and writes its plan, with `/shape --notes`.
pub const SHAPE_ITEM: &str = "shape-item";

/// Marks the note you linked to a tab yourself (the picker, Notes' `t`,
/// M-p's "Choose the tab's note"), set with `@tab_note`; what the workspace
/// links leaves it alone.
pub const BY_YOU: &str = "@tab_note_by_you";

/// The plan note of an autopilot tab's item, a path in the notes directory,
/// found once off the server's task; empty while it is looked for, or when
/// the item links none.
pub const PLAN_NOTE: &str = "@tab_plan_note";

/// Marks a tab with the note "Attach this note to a tab" took from it for
/// another, while a Notes beside it may still show that note: Notes
/// leaving the tab doesn't make it the tab's last note again (the
/// workspace's `remember_note`). The mark stays until the note is attached
/// to the tab again, since a close asked about and canceled leaves Notes
/// there.
pub const LEFT: &str = "@tab_note_left";

/// Notes as a plugin, which the workspace loads in its own process.
pub struct Notes;

/// What Notes says of itself: every setting of its own the app reads, with
/// its default (the same as `switchboard`'s `assets/settings.json` lists it
/// with, which a test there keeps so).
pub fn manifest() -> Manifest {
    Manifest {
        protocol: PROTOCOL_VERSION,
        name: "notes".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        settings: vec![
            Setting::new("notes_toolbar", true),
            Setting::new("notes_palette", true),
            Setting::new("notes_preview", true),
            Setting::new("notes_ctrl_v_image", true),
            Setting::new("notes_images", true),
            Setting::new("notes_mermaid", true),
            Setting::new("notes_item_chips", true),
            Setting::new("notes_recent", true),
            Setting::new("notes_relative_numbers", false),
            Setting::new("notes_per_tab", true),
            Setting::new("picture_full_size", true),
            Setting::new("key_help_line", true),
            Setting::new("palette_button", true),
            Setting::new("pane_arrows", true),
            Setting::new("notes_resize", true),
        ],
    }
}

impl Plugin for Notes {
    fn manifest(&self) -> Manifest {
        manifest()
    }

    /// `<program>`: the app; `<program> open <note>`, `sidebar on|off` and
    /// `shown` ask the running one, as tmux/notes-open.sh did.
    fn run(&self, program: &str, args: Vec<String>, host: Arc<dyn Host>) -> ExitCode {
        host::install(host);
        host::share_with_ui();
        let usage = || {
            eprintln!("Usage: {program} [open <note> | sidebar on|off | shown]");
            ExitCode::from(2)
        };
        let mut args = args.into_iter();
        match args.next().as_deref() {
            None => app::run(program, Store::at(store::notes_dir())),
            Some("open") => match args.next() {
                Some(rel) => open(program, &rel),
                None => usage(),
            },
            Some("sidebar") => match args.next().as_deref() {
                Some(on @ ("on" | "off")) => ask(program, &format!("sidebar {on}")),
                _ => usage(),
            },
            Some("shown") => ask(program, "shown"),
            Some("-h" | "--help") => {
                usage();
                ExitCode::SUCCESS
            }
            Some(_) => usage(),
        }
    }

    /// Notes is told of nothing yet.
    fn on(&self, _event: Event, host: Arc<dyn Host>) -> Result<(), String> {
        host::install(host);
        Ok(())
    }
}

fn ask(program: &str, request: &str) -> ExitCode {
    match socket::send(&socket::path(), request) {
        Ok(reply) => {
            if !reply.is_empty() {
                println!("{reply}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{program}: {e}");
            ExitCode::FAILURE
        }
    }
}

/// A note shown: in the running app, or, with none running, the note the
/// app opens with (`.notes.json`'s last).
fn open(program: &str, rel: &str) -> ExitCode {
    if rel.split('/').any(|p| p == "..") || rel.starts_with('/') {
        eprintln!("{program}: {rel} isn't a note in the notes directory");
        return ExitCode::from(2);
    }
    if socket::send(&socket::path(), &format!("open {rel}")).is_ok() {
        return ExitCode::SUCCESS;
    }
    let store = Store::at(store::notes_dir());
    if !store.absolute(rel).is_file() {
        eprintln!("{program}: no note {rel}");
        return ExitCode::FAILURE;
    }
    store.set_last(Some(rel));
    ExitCode::SUCCESS
}
