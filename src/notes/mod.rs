//! The notes app: the macOS Notes app's notes, in the terminal, edited with
//! Vim's keys, as a program of Switchboard's own, `switchboard notes`, in
//! place of the Neovim plugin in notes/ that bin/notes opened.
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
//! - `images`: pictures pasted and drawn (images.lua).
//! - `sidebar`: the list (sidebar.lua).
//! - `menu`: the Format menu and the boxes the app asks in.
//! - `palette`: Ctrl-Shift-P (palette.lua); `finder`: Ctrl-F (finder.lua).
//! - `easy`: the toolbar and easy mode (easy.lua).
//! - `squash`: notes merged by Claude (squash.lua).
//! - `socket`: the socket the workspace shows a tab's note through.
//! - `app`: the program on the screen (init.lua).
//! - `serve`: Notes in the workspace, on the server: M-N, M-e, a note's
//!   tab, a tab's note, the palette's commands (notes-open.sh and the rest),
//!   and `away`, what Notes' tab shows while it is beside another.

use std::process::ExitCode;

pub mod app;
pub mod easy;
pub mod editor;
pub mod finder;
pub mod fold;
pub mod format;
pub mod id;
pub mod images;
pub mod jumps;
pub mod marks;
pub mod menu;
pub mod palette;
pub mod render;
pub mod serve;
pub mod sidebar;
pub mod socket;
pub mod squash;
pub mod store;
pub mod sync;
pub mod table;
pub mod text;

use store::Store;

/// `switchboard notes`: the app; `switchboard notes open <note>`, `sidebar
/// on|off` and `shown` ask the running one, as tmux/notes-open.sh did;
/// `away` is what the server runs in Notes' tab while it is beside another
/// (serve/away.rs).
pub fn main(args: Vec<String>) -> ExitCode {
    let mut args = args.into_iter();
    match args.next().as_deref() {
        None => app::run(Store::at(store::notes_dir())),
        Some("open") => match args.next() {
            Some(rel) => open(&rel),
            None => usage(),
        },
        Some("sidebar") => match args.next().as_deref() {
            Some(on @ ("on" | "off")) => ask(&format!("sidebar {on}")),
            _ => usage(),
        },
        Some("shown") => ask("shown"),
        Some("away") => serve::away::main(),
        Some("-h" | "--help") => {
            usage();
            ExitCode::SUCCESS
        }
        Some(_) => usage(),
    }
}

fn ask(request: &str) -> ExitCode {
    match socket::send(&socket::path(), request) {
        Ok(reply) => {
            if !reply.is_empty() {
                println!("{reply}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("switchboard notes: {e}");
            ExitCode::FAILURE
        }
    }
}

/// A note shown: in the running app, or, with none running, the note the
/// app opens with (`.notes.json`'s last).
fn open(rel: &str) -> ExitCode {
    if rel.split('/').any(|p| p == "..") || rel.starts_with('/') {
        eprintln!("switchboard notes: {rel} isn't a note in the notes directory");
        return ExitCode::from(2);
    }
    if socket::send(&socket::path(), &format!("open {rel}")).is_ok() {
        return ExitCode::SUCCESS;
    }
    let store = Store::at(store::notes_dir());
    if !store.absolute(rel).is_file() {
        eprintln!("switchboard notes: no note {rel}");
        return ExitCode::FAILURE;
    }
    store.set_last(Some(rel));
    ExitCode::SUCCESS
}

fn usage() -> ExitCode {
    eprintln!("Usage: switchboard notes [open <note> | sidebar on|off | shown]");
    ExitCode::from(2)
}
