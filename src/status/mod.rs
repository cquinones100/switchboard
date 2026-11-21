//! Dark and light (tmux/theme.sh) and the server's tick for the tab
//! headers' ages.
//!
//! - `theme`: the palettes for dark and light, and which is in use.
//! - `serve`: the server's side: the tick, the terminal's answer about its
//!   background, M-p's "Toggle light and dark theme".

mod serve;

pub use switchboard_ui::theme;
pub use theme::Palette;
