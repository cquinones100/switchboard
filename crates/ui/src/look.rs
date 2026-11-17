//! The settings the boxes follow (`Look`), read as an overlay opens: taken
//! from src/ui/mod.rs when the drawing the apps share became a crate.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::settings;
use crate::theme::{self, Theme};

/// The settings the boxes follow, read as an overlay opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    /// A click takes an option, Yes or No, and closes a box from outside it
    /// (menu_mouse).
    pub mouse: bool,
    /// What a click would press lights under the mouse (hover_highlight,
    /// with menu_mouse).
    pub hover: bool,
    /// Easy mode's × in a box's top edge (easy_keys with menu_mouse), as
    /// render.js's `closable`.
    pub close: bool,
    /// Tab and Shift-Tab move focus as on a webpage (tab_focus). A box's
    /// own options take Tab whatever it says.
    pub tab_focus: bool,
    pub theme: Theme,
    /// A box whose list runs past it has a scroll bar on its right edge
    /// (scroll_bars).
    pub scroll_bars: bool,
    /// The empty columns beside a tab's narrow lone pane, where a box about
    /// the tab that goes in the middle is drawn as a pane instead when it
    /// fits there (boxes_beside_narrow_pane); set as the overlays are drawn
    /// for such a box alone (`Layer::about`), never under a view that
    /// covers the screen.
    pub middle: Option<Rect>,
}

impl Default for Look {
    /// The settings' defaults: clicks and hover on, easy mode off, dark.
    fn default() -> Look {
        Look {
            mouse: true,
            hover: true,
            close: false,
            tab_focus: true,
            theme: Theme::DARK,
            scroll_bars: true,
            middle: None,
        }
    }
}

impl Look {
    /// The look the settings give now. The theme is light when the theme
    /// setting says light, or says system on a terminal whose background is
    /// light (`theme`).
    /// The dark theme in the canvas's colors for a terminal that draws
    /// truecolor (`truecolor`), else as `read`.
    pub fn read_for(truecolor: bool) -> Look {
        let mut look = Look::read();
        if truecolor && look.theme == Theme::DARK {
            look.theme = Theme::DARK_TRUE;
        }
        look
    }

    pub fn read() -> Look {
        const KEYS: [&str; 6] = [
            "menu_mouse",
            "hover_highlight",
            "easy_keys",
            "tab_focus",
            "scroll_bars",
            "theme",
        ];
        let all = settings::fresh(&KEYS);
        let on = |key: &str| {
            KEYS.iter()
                .position(|k| *k == key)
                .is_none_or(|at| all[at] != Some(serde_json::Value::Bool(false)))
        };
        let light = all[5].as_ref().is_some_and(theme::light_for);
        let mouse = on("menu_mouse");
        Look {
            mouse,
            hover: mouse && on("hover_highlight"),
            close: mouse && on("easy_keys"),
            tab_focus: on("tab_focus"),
            theme: if light { Theme::LIGHT } else { Theme::DARK },
            scroll_bars: on("scroll_bars"),
            middle: None,
        }
    }

    pub fn text(&self) -> Style {
        Style::new().fg(self.theme.text)
    }

    pub fn dim(&self) -> Style {
        Style::new().fg(self.theme.dim)
    }

    pub fn faint(&self) -> Style {
        Style::new().fg(self.theme.faint)
    }

    pub fn bright(&self) -> Style {
        Style::new().fg(self.theme.bright)
    }

    pub fn bold_bright(&self) -> Style {
        self.bright().add_modifier(Modifier::BOLD)
    }

    /// The yellow bold `›` beside the chosen option.
    pub fn pointer(&self) -> Span<'static> {
        Span::styled(
            "›",
            Style::new()
                .fg(crate::draw::YELLOW)
                .add_modifier(Modifier::BOLD),
        )
    }
}
