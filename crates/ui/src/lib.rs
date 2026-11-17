//! What Switchboard draws with, shared by the workspace and by the apps
//! that run in its panes or alone in a terminal (the diff, Notes): taken from the
//! `switchboard` crate (src/ui, src/mux/input.rs, src/sidebar/text.rs,
//! src/status/theme.rs, src/views/markdown.rs and mermaid.rs, and the part
//! of src/mux/terminal.rs and src/mux/compose.rs an app uses) when the diff
//! became a crate of its own, so that an app depends on this and on
//! `switchboard-plugin-api`, never on the workspace, and one state has one
//! mark and one color in every program. `switchboard` gives each module at
//! the path it had (`ui::kit`, `mux::input`, `sidebar::text`,
//! `status::theme`, `views::markdown`).
//!
//! Nothing here knows the workspace: no server, no socket, no file of
//! yours. The few settings the drawing follows are asked of whoever runs
//! (`settings`), which answers their defaults until a program says where
//! its settings are.
//!
//! - `text`: text measured in columns, cut and wrapped; the badges.
//! - `draw`: the box every menu, question and list is drawn in.
//! - `kit`: UX 2.0's atoms (tones, marks, chips, cards, keys, columns).
//! - `field`: the text field every box that takes typing shares.
//! - `theme`: dark and light, the palettes, and which is in use.
//! - `look`: the settings a box follows (`Look`).
//! - `input`: the bytes a terminal sends, as keys, mouse and paste.
//! - `terminal`: a terminal asked for the colours it draws in.
//! - `screen`: a frame sent to a terminal as what changed since the last.
//! - `overlay`: a box over what a program draws, and what comes of a key
//!   given to it; `message`, `confirm`, `menu` and `picker` are the boxes,
//!   `rank` the order what is typed gives a list, `pointer` the mouse
//!   pointer's shape over one.
//! - `markdown`, `mermaid`: Markdown laid out in columns, its diagrams as text.
//! - `placeholder`: the cells a kitty image is placed by, the ids of the
//!   workspace's own, and a box's shadow as text.
//! - `who`: who someone is to a service, which an avatar's photo is known by.
//! - `settings`: the settings the drawing follows, asked of the program.

pub mod confirm;
pub mod draw;
pub mod field;
pub mod input;
pub mod kit;
pub mod look;
pub mod markdown;
pub mod menu;
pub mod mermaid;
pub mod message;
pub mod overlay;
pub mod picker;
pub mod placeholder;
pub mod pointer;
pub mod rank;
pub mod screen;
pub mod settings;
pub mod terminal;
pub mod text;
pub mod theme;
pub mod who;

pub use confirm::Confirm;
pub use look::Look;
pub use menu::{Menu, MenuOption};
pub use message::Message;
pub use overlay::{AsAny, Canvas, Outcome, Overlay, Step, dim_behind};
pub use picker::{PickItem, Picked, Picker};
pub use theme::Theme;
