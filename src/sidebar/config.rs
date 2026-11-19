//! `~/.config/switchboard/sidebar.json` (lib/sidebar/config.js's
//! `readConfig`): how wide the sidebar is, the narrowest window that gets
//! one, mission control's card size, and which plugins load. Every key is
//! optional; one missing, or a file that isn't JSON, takes the default.

use std::path::PathBuf;

use serde_json::Value;

/// What sidebar.json says, with the defaults for what it doesn't.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Columns the sidebar takes.
    pub width: u16,
    /// The narrowest window that gets a sidebar.
    pub min_window_width: u16,
    /// Mission control's cards.
    pub card_width: u16,
    pub card_height: u16,
    /// Plugin name to its options; only plugins named here load.
    pub plugins: serde_json::Map<String, Value>,
}

/// The sidebar's width when sidebar.json names none.
pub const WIDTH: u16 = 34;

/// The narrowest a sidebar is dragged or resized to.
pub const LEAST: u16 = 24;

/// A width asked of a sidebar in a window `cols` wide, as it is given: no
/// narrower than `LEAST`, no wider than half the window.
pub fn resized(width: i32, cols: u16) -> u16 {
    let most = i32::from((cols / 2).max(LEAST));
    u16::try_from(width.clamp(i32::from(LEAST), most)).unwrap_or(LEAST)
}

impl Default for Config {
    fn default() -> Config {
        Config {
            width: WIDTH,
            min_window_width: 90,
            card_width: 56,
            card_height: 13,
            plugins: serde_json::Map::new(),
        }
    }
}

/// `~/.config/switchboard/sidebar.json`.
pub fn config_file() -> PathBuf {
    crate::paths::config_dir().join("sidebar.json")
}

impl Config {
    /// The config as a JSON object gives it.
    pub fn from_json(v: &Value) -> Config {
        let d = Config::default();
        let num = |key: &str, default: u16| {
            v.get(key)
                .and_then(Value::as_u64)
                .and_then(|n| u16::try_from(n).ok())
                .filter(|n| *n > 0)
                .unwrap_or(default)
        };
        Config {
            width: num("width", d.width),
            min_window_width: num("minWindowWidth", d.min_window_width),
            card_width: num("cardWidth", d.card_width),
            card_height: num("cardHeight", d.card_height),
            plugins: v
                .get("plugins")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
        }
    }

    /// The config from its file.
    pub fn read() -> Config {
        crate::util::read_json(config_file())
            .map(|v| Config::from_json(&v))
            .unwrap_or_default()
    }

    /// The columns a sidebar takes in a window `cols` wide, its line beside
    /// the panes included, or 0 when the window is too narrow for one.
    pub fn columns_in(&self, cols: u16) -> u16 {
        self.columns_at(cols, None)
    }

    /// The same for a sidebar given a width of its own (`kept`, from a drag
    /// of its edge or the pane resize keys), held between `LEAST` and half
    /// the window; with none, this file's.
    pub fn columns_at(&self, cols: u16, kept: Option<u16>) -> u16 {
        let width = kept.map_or(self.width, |w| resized(i32::from(w), cols));
        if cols < self.min_window_width || cols <= width + 1 {
            0
        } else {
            width + 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn its_keys_are_read_and_the_rest_default() {
        let c = Config::from_json(&json!({"width": 40, "plugins": {"weather": {}}}));
        assert_eq!(c.width, 40);
        assert_eq!(c.min_window_width, 90);
        assert!(c.plugins.contains_key("weather"));
        assert_eq!(Config::from_json(&json!({"width": "wide"})).width, 34);
        assert_eq!(Config::default().columns_in(100), 35);
        assert_eq!(Config::default().columns_in(89), 0);
    }

    #[test]
    fn a_width_of_its_own_is_held_between_the_least_and_half_the_window() {
        let c = Config::default();
        assert_eq!(c.columns_at(200, None), 35, "the file's with none");
        assert_eq!(c.columns_at(200, Some(50)), 51);
        assert_eq!(c.columns_at(200, Some(5)), LEAST + 1);
        assert_eq!(c.columns_at(120, Some(90)), 61, "half the window");
        assert_eq!(c.columns_at(89, Some(50)), 0, "too narrow for a sidebar");
        assert_eq!(resized(-3, 200), LEAST);
        assert_eq!(resized(150, 200), 100);
    }
}
