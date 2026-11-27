//! Vim's keys over lines of text and a cursor, one implementation for
//! every text Switchboard shows: the notes app edits a note with it, and
//! the code viewer (`switchboard code`), the views' pager and the diff's
//! file view read with it, read-only (`Editor::read_only`). It started as the notes app's editor
//! (crates/notes/src/editor.rs), which keeps what a note adds on top (Enter
//! ticking a box, the Format menu, the leader keys, folds, tables, the
//! plan) and plugs its folds and its own state into the editor through
//! `Note`.
//!
//! It depends on `switchboard-ui` alone, for its keys, never on
//! `switchboard`, as the plugin boundary in CLAUDE.md asks.

pub mod doc;
pub mod editor;
pub mod marks;
pub mod places;
pub mod text;

pub use doc::{Block, Doc, Folding, Insert, Note, Selection};
pub use editor::{Editor, Effect, Mode, Register};
pub use text::Text;
