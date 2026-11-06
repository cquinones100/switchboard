//! The keys that start work, on the server: what tmux.conf's popups for
//! M-t, M-n, M-W, M-B and M-o and the palette's work commands ran
//! (tmux/new-tab.zsh, wt-open.zsh, wtb-open.zsh, wtpr-open.zsh,
//! wtn-open.zsh, shape-open.zsh, and the directory list they shared,
//! pick-dir.zsh), as overlays over the workspace rather than popups.

use crate::mux::TabId;
use crate::mux::server::Server;

impl Server {
    /// Moves a tab to the end of its session's tabs, so tabs opened one
    /// after another keep the order they were asked for in and no open tab
    /// is renumbered: M-1 to M-9 reach tabs by number.
    pub fn tab_to_end(&mut self, tab: TabId) {
        let Some(name) = self.session_of_tab(tab).map(|s| s.name.clone()) else {
            return;
        };
        if let Some(s) = self.session_mut(&name) {
            s.tabs.retain(|t| *t != tab);
            s.tabs.push(tab);
        }
        self.dirty_session(&name);
    }
}
