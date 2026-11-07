//! The notes app: the macOS Notes app's notes, in the terminal, edited with
//! Vim's keys, as a program of Switchboard's own, in place of the Neovim
//! plugin in notes/ that bin/notes opened.
//!
//! Notes are Markdown files in `$NOTES_DIR`, or `~/Notes`, in folders that
//! are directories, never in this repository. A note is drawn the way
//! Notes shows it: markup hidden (it shows on the line being typed in), a
//! title standing out from headings, list markers as • and –, checklist
//! items as ☐ and ☑, a quote with a bar, a table with box lines, pictures
//! under their lines. Nothing in a note is sent anywhere but by squashing
//! notes, after a yes.
//!
//! One module per part, as the plugin had one Lua file per part:
//! - `store`: the files on disk (store.lua).
//! - `text`: a note's lines, in a rope.
//! - `render`: a note drawn as Notes shows it (render.lua).
//! - `format`: the Format menu's commands and the typing it does
//!   (format.lua); `table` (table.lua), `fold` (fold.lua).
//! - `editor`: Vim's keys.
//! - `sync`: notes changed from outside (sync.lua).
//! - `images`: pictures pasted and drawn (images.lua).
//! - `sidebar`: the list (sidebar.lua).
//! - `menu`: the Format menu and the boxes the app asks in.
//! - `palette`: Ctrl-Shift-P (palette.lua); `finder`: Ctrl-F (finder.lua).
//! - `easy`: the toolbar and easy mode (easy.lua).
//! - `squash`: notes merged by Claude (squash.lua).

pub mod easy;
pub mod editor;
pub mod finder;
pub mod fold;
pub mod format;
pub mod images;
pub mod menu;
pub mod palette;
pub mod render;
pub mod sidebar;
pub mod squash;
pub mod store;
pub mod sync;
pub mod table;
pub mod text;
