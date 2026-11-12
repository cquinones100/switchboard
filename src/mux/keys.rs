//! Key bindings: which key does what. The defaults are the multiplexer keys
//! tmux.conf bound (moving between panes and tabs, splitting, closing,
//! zooming, resizing), its Claude keys (M-a to what needs you, M-A to the
//! next idle tab, M-c to the tab's Claude, M-y to copy its latest reply,
//! M-f to pin a tab), M-h for the tab's history (new with UX 2.0,
//! src/history), M-s into the sidebar and the views'
//! (M-v, M-i, M-r, M-l, M-m, M-S-f, src/views), M-d (src/diff) and Notes'
//! M-N and M-e (src/notes/serve.rs), M-0 and C-w (src/extras), under the
//! same keys, with the prefix `M-b` for the rest;
//! `~/.config/switchboard/keys.json` adds to or changes them:
//!
//! ```json
//! { "root": { "M-g": "new-tab", "M-w": null }, "prefix": { "y": "copy-mode" } }
//! ```
//!
//! A binding names an action, as docs/guide.md lists them, or `null` to
//! unbind. Each action is one of `Action`'s, written as it is parsed by
//! `Action::parse`.
//!
//! Each action has a description and tags (`Action::note`), as tmux.conf
//! gave every binding one with `bind -N "Split pane to the left #panes
//! #split"`: the M-p palette (src/ui/palette.rs) lists every action by it,
//! with its keys, and the actions no key is bound to by default
//! (`Action::palette_only`) with none.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::Direction;
use super::input::Key;

/// What a key does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Move to the pane in a direction.
    SelectPane(Direction),
    /// The previous or next tab, wrapping round.
    PrevTab,
    NextTab,
    /// The tab you were on before.
    LastTab,
    /// The tab with this number (1-based, as the status line shows).
    SelectTab(usize),
    /// A new tab, its shell in the current pane's directory.
    NewTab,
    /// A new pane on that side of the current one, in its directory.
    Split(Direction),
    /// Close the current pane (its tab, when it is the last).
    ClosePane,
    /// Close the current tab, after asking (tmux's prefix `&`).
    CloseTab,
    /// The next pane of the tab, going round (tmux's prefix `o`).
    NextPane,
    /// The pane you were in before, in this tab (tmux's prefix `;`).
    LastPane,
    /// Zoom the current pane to the whole tab, or back.
    Zoom,
    /// Grow the current pane toward a direction by cells.
    Resize(Direction, u16),
    /// Leave the workspace running and end this client.
    Detach,
    /// Scroll back through the pane's history, with vi keys.
    CopyMode,
    /// The prefix key itself, sent to the pane.
    SendPrefix,
    /// Every binding, in a list.
    ListKeys,
    /// The next tab that needs you: pinned, then waiting, then done.
    NextNeedsYou,
    /// The next idle tab, in the sidebar's Idle order (M-A).
    NextIdle,
    /// Into the pane running Claude in this tab.
    FocusClaude,
    /// The latest reply of this tab's Claude, copied (M-y).
    CopyReply,
    /// The tab's history: a timeline of its Claude session under its
    /// screen as it was at the moment chosen (M-h, src/history).
    History,
    /// Pin the tab, keeping it at the top of what needs you, or unpin it.
    TogglePin,
    /// The palette: every command, to find and run one (M-p).
    Palette,
    /// Show every tip again, each once more.
    ResetTips,
    /// Switch between the light and dark theme (the theme setting;
    /// src/status/theme.rs).
    ToggleTheme,
    /// Into the sidebar, or back out of it (M-s).
    FocusSidebar,
    /// Turn the sidebar on or off (the sidebar setting).
    ToggleSidebar,
    /// A new tab running Claude, in a directory picked from the ones you
    /// work in (M-t, M-n; src/cmd/work.rs).
    NewClaudeTab,
    /// Work on Linear tickets, each in a new tab running `wt` (M-W).
    WorkOnTickets,
    /// Work on a branch, new or existing, in a new tab running `wtb` (M-B).
    WorkOnBranch,
    /// Work on a pull request in a new tab running `wtpr`.
    WorkOnPr,
    /// Review a contributor's open PR in a new tab in the main clone running
    /// `/review-pr <number>`, listed only in a repository with that project
    /// skill (src/cmd/work.rs).
    ReviewPr,
    /// Work on a note's checklist items, each in a new tab running `wtn` (M-I).
    WorkOnNote,
    /// Carry on shaping Linear tickets, each in a new tab running `shape`.
    ContinueShaping,
    /// Shape a note's checklist items as Linear tickets, each in a new tab
    /// (note-shape.sh; src/cmd/shape_note.rs).
    ShapeNote,
    /// Shape a new Linear ticket from an idea typed in (note-shape.sh's
    /// `prompt`).
    ShapeNewTicket,
    /// The branch's PR description as GitHub draws it, as images in place
    /// of the pane (pr-image.js; src/views/image).
    PrImage,
    /// The branch's Linear ticket drawn the same way.
    TicketImage,
    /// Mission control, or closed when it is open (M-Space; src/mission).
    MissionControl,
    /// The Work map as a picker of the open tabs (M-j).
    FindTab,
    /// The Work map as a picker of the work no tab has open (M-o).
    ReviveWorktree,
    /// Mission control's Settings view.
    Settings,
    /// A reminder on the current tab (remind.sh).
    RemindTab,
    /// The tab's whole title in a box, which the sidebar and the status
    /// line cut (M-T; tab-title.sh; src/tabs/title.rs).
    TabTitle,
    /// What the views do: M-v, M-i, M-r, M-l, M-m, M-S-f and the link
    /// commands (src/views).
    View(crate::views::Command),
    /// What the diff does: M-d, and the palette's commands for a tool in a
    /// pane's place and for the diff open in it (src/diff).
    Diff(crate::diff::Command),
    /// The Slack relay in a tab of its own, or its tab (M-S; src/relay).
    SlackRelay,
    /// Stop the Slack relay.
    StopSlackRelay,
    /// M-p's "Update Switchboard" and "Run Switchboard's setup" (src/update).
    Update(crate::update::Command),
    /// Build the checkout a build from a clone came from, then restart the
    /// server on it when it is new, keeping every pane, tab and terminal
    /// attached (src/mux/latest.rs, src/mux/upgrade.rs).
    Upgrade,
    /// The computer kept awake whatever runs, until pressed again
    /// (src/awake).
    KeepAwake,
    /// Notes in the workspace: M-N, M-e, a note's tab and a tab's note
    /// (src/notes/serve.rs).
    Notes(crate::notes::serve::Command),
    /// The rest of tmux.conf's keys and M-p's commands: M-0, C-w, renaming
    /// the tab, mission control on a view and the rest (src/extras).
    Extra(crate::extras::Command),
    /// Autopilot (src/autopilot).
    Autopilot(crate::autopilot::Command),
}

impl Action {
    /// An action from its name in keys.json: `select-pane-left`,
    /// `prev-tab`, `next-tab`, `last-tab`, `select-tab-3`, `new-tab`,
    /// `split-right`, `close-pane`, `zoom`, `resize-left-5`, `detach`,
    /// `copy-mode`, `send-prefix`, `list-keys`, `next-needs-you`,
    /// `next-idle`, `focus-claude`, `copy-reply`, `toggle-pin`, `palette`,
    /// `reset-tips`, `toggle-theme`, `focus-sidebar`, `toggle-sidebar`,
    /// `new-claude-tab`,
    /// `work-on-tickets`, `work-on-branch`, `work-on-pr`, `work-on-note`,
    /// `continue-shaping`, `shape-note`, `shape-new-ticket`, `pr-image`,
    /// `ticket-image`, `mission-control`, `find-tab`,
    /// `revive-worktree`, `settings`, `remind-tab`, `tab-title`, and the views' (`pr-view`,
    /// `ticket-view`, `open-pr`, `open-repo`, `open-ticket`, `pr-ready`,
    /// `search-everything`, `read-link`, `open-link-being-read`,
    /// `open-merge-queue`, `open-linear-project`, `copy-ticket-id`,
    /// `copy-ticket-url`, `copy-pr-number`, `copy-pr-url`,
    /// `clear-image-cache`), updates' (`update-switchboard`, `run-setup`),
    /// `upgrade`,
    /// Notes' (`notes`, `notes-beside`, `note-tab`, `choose-tab-note`), and
    /// the extras'
    /// (`rename-tab`, `go-to-tab`, `pane-nav`, `mission-<view>`,
    /// `notifications`, `habits-report`, `send-reply`, `update-main`,
    /// `check-setup`, `setup-part`, `calendar-tab`, `slack-tab`,
    /// `open-pr-number`, `open-ticket-id`, `aws-login`), and
    /// `work-note-continuously`.
    pub fn parse(name: &str) -> Option<Action> {
        let dir = |d: &str| match d {
            "left" => Some(Direction::Left),
            "right" => Some(Direction::Right),
            "up" => Some(Direction::Up),
            "down" => Some(Direction::Down),
            _ => None,
        };
        if let Some(command) = crate::views::Command::parse(name) {
            return Some(Action::View(command));
        }
        if let Some(command) = crate::update::Command::parse(name) {
            return Some(Action::Update(command));
        }
        if let Some(command) = crate::diff::Command::parse(name) {
            return Some(Action::Diff(command));
        }
        if let Some(command) = crate::notes::serve::Command::parse(name) {
            return Some(Action::Notes(command));
        }
        if let Some(command) = crate::extras::Command::parse(name) {
            return Some(Action::Extra(command));
        }
        if let Some(command) = crate::autopilot::Command::parse(name) {
            return Some(Action::Autopilot(command));
        }
        Some(match name {
            "prev-tab" => Action::PrevTab,
            "next-tab" => Action::NextTab,
            "last-tab" => Action::LastTab,
            "new-tab" => Action::NewTab,
            "close-pane" => Action::ClosePane,
            "close-tab" => Action::CloseTab,
            "next-pane" => Action::NextPane,
            "last-pane" => Action::LastPane,
            "zoom" => Action::Zoom,
            "detach" => Action::Detach,
            "copy-mode" => Action::CopyMode,
            "send-prefix" => Action::SendPrefix,
            "list-keys" => Action::ListKeys,
            "next-needs-you" => Action::NextNeedsYou,
            "next-idle" => Action::NextIdle,
            "focus-claude" => Action::FocusClaude,
            "copy-reply" => Action::CopyReply,
            "history" => Action::History,
            "toggle-pin" => Action::TogglePin,
            "palette" => Action::Palette,
            "reset-tips" => Action::ResetTips,
            "toggle-theme" => Action::ToggleTheme,
            "slack-relay" => Action::SlackRelay,
            "stop-slack-relay" => Action::StopSlackRelay,
            "focus-sidebar" => Action::FocusSidebar,
            "toggle-sidebar" => Action::ToggleSidebar,
            "new-claude-tab" => Action::NewClaudeTab,
            "work-on-tickets" => Action::WorkOnTickets,
            "work-on-branch" => Action::WorkOnBranch,
            "work-on-pr" => Action::WorkOnPr,
            "review-pr" => Action::ReviewPr,
            "work-on-note" => Action::WorkOnNote,
            "continue-shaping" => Action::ContinueShaping,
            "shape-note" => Action::ShapeNote,
            "shape-new-ticket" => Action::ShapeNewTicket,
            "pr-image" => Action::PrImage,
            "ticket-image" => Action::TicketImage,
            "mission-control" => Action::MissionControl,
            "find-tab" => Action::FindTab,
            "revive-worktree" => Action::ReviveWorktree,
            "settings" => Action::Settings,
            "remind-tab" => Action::RemindTab,
            "tab-title" => Action::TabTitle,
            "upgrade" => Action::Upgrade,
            "keep-awake" => Action::KeepAwake,
            _ => {
                if let Some(d) = name.strip_prefix("select-pane-") {
                    Action::SelectPane(dir(d)?)
                } else if let Some(d) = name.strip_prefix("split-") {
                    Action::Split(dir(d)?)
                } else if let Some(n) = name.strip_prefix("select-tab-") {
                    Action::SelectTab(n.parse().ok().filter(|n| *n > 0)?)
                } else {
                    let rest = name.strip_prefix("resize-")?;
                    let (d, cells) = rest.split_once('-').unwrap_or((rest, "5"));
                    Action::Resize(dir(d)?, cells.parse().ok()?)
                }
            }
        })
    }

    /// The action's name, as `parse` reads it.
    pub fn name(&self) -> String {
        let dir = |d: &Direction| match d {
            Direction::Left => "left",
            Direction::Right => "right",
            Direction::Up => "up",
            Direction::Down => "down",
        };
        match self {
            Action::SelectPane(d) => format!("select-pane-{}", dir(d)),
            Action::PrevTab => "prev-tab".into(),
            Action::NextTab => "next-tab".into(),
            Action::LastTab => "last-tab".into(),
            Action::SelectTab(n) => format!("select-tab-{n}"),
            Action::NewTab => "new-tab".into(),
            Action::Split(d) => format!("split-{}", dir(d)),
            Action::ClosePane => "close-pane".into(),
            Action::CloseTab => "close-tab".into(),
            Action::NextPane => "next-pane".into(),
            Action::LastPane => "last-pane".into(),
            Action::Zoom => "zoom".into(),
            Action::Resize(d, n) => format!("resize-{}-{n}", dir(d)),
            Action::Detach => "detach".into(),
            Action::CopyMode => "copy-mode".into(),
            Action::SendPrefix => "send-prefix".into(),
            Action::ListKeys => "list-keys".into(),
            Action::NextNeedsYou => "next-needs-you".into(),
            Action::NextIdle => "next-idle".into(),
            Action::FocusClaude => "focus-claude".into(),
            Action::CopyReply => "copy-reply".into(),
            Action::History => "history".into(),
            Action::TogglePin => "toggle-pin".into(),
            Action::Palette => "palette".into(),
            Action::ResetTips => "reset-tips".into(),
            Action::ToggleTheme => "toggle-theme".into(),
            Action::FocusSidebar => "focus-sidebar".into(),
            Action::ToggleSidebar => "toggle-sidebar".into(),
            Action::NewClaudeTab => "new-claude-tab".into(),
            Action::WorkOnTickets => "work-on-tickets".into(),
            Action::WorkOnBranch => "work-on-branch".into(),
            Action::WorkOnPr => "work-on-pr".into(),
            Action::ReviewPr => "review-pr".into(),
            Action::WorkOnNote => "work-on-note".into(),
            Action::ContinueShaping => "continue-shaping".into(),
            Action::ShapeNote => "shape-note".into(),
            Action::ShapeNewTicket => "shape-new-ticket".into(),
            Action::PrImage => "pr-image".into(),
            Action::TicketImage => "ticket-image".into(),
            Action::MissionControl => "mission-control".into(),
            Action::FindTab => "find-tab".into(),
            Action::ReviveWorktree => "revive-worktree".into(),
            Action::Settings => "settings".into(),
            Action::RemindTab => "remind-tab".into(),
            Action::TabTitle => "tab-title".into(),
            Action::View(command) => command.name().into(),
            Action::Diff(command) => command.name().into(),
            Action::SlackRelay => "slack-relay".into(),
            Action::StopSlackRelay => "stop-slack-relay".into(),
            Action::Update(command) => command.name().into(),
            Action::Upgrade => "upgrade".into(),
            Action::KeepAwake => "keep-awake".into(),
            Action::Notes(command) => command.name().into(),
            Action::Extra(command) => command.name(),
            Action::Autopilot(command) => command.name().into(),
        }
    }

    /// What it does, in words, for the list of keys: tmux.conf's
    /// descriptions without their tags.
    pub fn about(&self) -> String {
        let side = |d: &Direction| match d {
            Direction::Left => "to the left",
            Direction::Right => "to the right",
            Direction::Up => "above",
            Direction::Down => "below",
        };
        let toward = |d: &Direction| match d {
            Direction::Left => "left",
            Direction::Right => "right",
            Direction::Up => "up",
            Direction::Down => "down",
        };
        match self {
            Action::SelectPane(Direction::Left) => "Move to the pane on the left (as C-w h)".into(),
            Action::SelectPane(Direction::Right) => {
                "Move to the pane on the right (as C-w l)".into()
            }
            Action::SelectPane(Direction::Up) => "Move to the pane above (as C-w k)".into(),
            Action::SelectPane(Direction::Down) => "Move to the pane below (as C-w j)".into(),
            Action::PrevTab => "Previous tab".into(),
            Action::NextTab => "Next tab".into(),
            Action::LastTab => "Go back to the previous tab".into(),
            Action::SelectTab(n) => format!("Go to tab {n}"),
            Action::NewTab => "New tab".into(),
            Action::Split(d) => format!("Split pane {}", side(d)),
            Action::ClosePane => "Close the pane".into(),
            Action::CloseTab => "Close the tab, after asking".into(),
            Action::NextPane => "Move to the next pane in the tab".into(),
            Action::LastPane => "Go back to the pane you were in before".into(),
            Action::Zoom => "Zoom the pane in or out".into(),
            Action::Resize(d, 5) => format!("Grow pane {}", toward(d)),
            Action::Resize(d, n) => format!("Grow pane {} by {n}", toward(d)),
            Action::Detach => "Detach, leaving everything running".into(),
            Action::CopyMode => "Copy mode: scroll back through the pane's history, select and copy with the keyboard".into(),
            Action::SendPrefix => "Send the prefix key to the pane".into(),
            Action::ListKeys => "List the keys".into(),
            Action::NextNeedsYou => "Go to the next tab that needs you".into(),
            Action::NextIdle => "Go to the next idle tab".into(),
            Action::FocusClaude => "Jump to the pane running Claude in this tab".into(),
            Action::CopyReply => "Copy the latest reply of this tab's Claude".into(),
            Action::History => {
                "Show this tab's history: its Claude session on a timeline, and the screen as it was"
                    .into()
            }
            Action::TogglePin => "Pin or unpin this tab, keeping it at the top of Needs you".into(),
            Action::Palette => "Find a command and run it (the palette)".into(),
            Action::ResetTips => "Show the tips again, each once more".into(),
            Action::ToggleTheme => "Toggle light and dark theme".into(),
            Action::FocusSidebar => "Jump into the sidebar, or back out of it".into(),
            Action::ToggleSidebar => "Turn the sidebar on or off".into(),
            Action::NewClaudeTab => "New tab, running Claude, in a directory you pick".into(),
            Action::WorkOnTickets => "Start work on tickets in new tabs (wt)".into(),
            Action::WorkOnBranch => {
                "Start work on a branch, new or existing, in a new tab (wtb)".into()
            }
            Action::WorkOnPr => "Start work on a PR in a new tab (wtpr)".into(),
            Action::ReviewPr => "Review a contributor's PR in a new tab (/review-pr)".into(),
            Action::WorkOnNote => "Start work on a note's checklist items in new tabs (wtn)".into(),
            Action::ContinueShaping => "Continue shaping Linear tickets, each in a new tab".into(),
            Action::ShapeNote => "Shape a note's checklist items as Linear tickets".into(),
            Action::ShapeNewTicket => {
                "Shape a new Linear ticket from what you type, in a new tab".into()
            }
            Action::PrImage => {
                "View the branch's PR description as an image, as GitHub draws it, in place of this pane".into()
            }
            Action::TicketImage => {
                "View the branch's Linear ticket as an image, as the PR image shows a PR, in place of this pane".into()
            }
            Action::MissionControl => "Open or close mission control".into(),
            Action::FindTab => "Search and jump to a tab".into(),
            Action::ReviveWorktree => {
                "Reopen a worktree with no tab and resume its Claude".into()
            }
            Action::Settings => "Settings: turn the workspace's options on and off".into(),
            Action::RemindTab => {
                "Remind me about this tab later, as Slack does: it waits under Reminders with the time left, then needs you".into()
            }
            Action::TabTitle => "Show this tab's full title".into(),
            Action::View(command) => command.about().into(),
            Action::Diff(command) => command.about().into(),
            Action::SlackRelay => "Open the Slack relay in its own tab".into(),
            Action::StopSlackRelay => "Stop the Slack relay".into(),
            Action::Update(command) => command.about().into(),
            Action::Upgrade => "Restart Switchboard on the latest build on disk".into(),
            Action::KeepAwake => {
                "Keep the computer awake until I say, or back to only while tabs work".into()
            }
            Action::Notes(command) => command.about().into(),
            Action::Extra(command) => command.about().into(),
            Action::Autopilot(command) => command.about().into(),
        }
    }

    /// More on what it does than `about`, for the palette's preview of the
    /// chosen command.
    pub fn detail(&self) -> String {
        match self {
            Action::SelectPane(_) => {
                "Past the edge it comes round to the far side; from the leftmost pane it goes into the sidebar. In the diff and Notes it moves between the app's own windows first.".into()
            }
            Action::PrevTab | Action::NextTab => {
                "Tabs go in the order the tab bar shows them, coming round past either end.".into()
            }
            Action::LastTab => {
                "Pressed again, it goes back the other way, so two tabs can be swapped between with one key.".into()
            }
            Action::SelectTab(_) => {
                "M-1 goes to the first tab and M-9 to the ninth, by the numbers the sidebar shows; M-0 asks for a number, for the tabs past 9.".into()
            }
            Action::NewTab => {
                "The tab runs your shell in the current pane's directory and goes right after the current one.".into()
            }
            Action::Split(_) => {
                "The new pane runs your shell in the current pane's directory and takes the focus.".into()
            }
            Action::ClosePane => {
                "It asks first when a program other than a shell at its prompt runs in it (Settings' confirm_close). The last pane closes its tab.".into()
            }
            Action::CloseTab => {
                "The question names the programs running in its panes. Closing it ends them.".into()
            }
            Action::NextPane => {
                "Past the last pane it comes round to the first.".into()
            }
            Action::LastPane => {
                "Pressed again, it goes back, so two panes can be swapped between with one key.".into()
            }
            Action::Zoom => {
                "The pane covers the tab, the sidebar too, until zoomed out again; the others keep running behind it.".into()
            }
            Action::Resize(..) => {
                "The pane's edge moves that way, the panes beside giving up the room. Resizing the terminal keeps each pane's share.".into()
            }
            Action::Detach => {
                "This terminal leaves the workspace, and every pane's program goes on running. Run switchboard again to attach.".into()
            }
            Action::CopyMode => {
                "Vim's keys move through the 10,000 lines each pane keeps, v selects and y copies to the clipboard; q leaves it.".into()
            }
            Action::SendPrefix => {
                "The program in the pane gets M-b itself, for one that has a use for it.".into()
            }
            Action::ListKeys => {
                "Every key bound now, yours from keys.json included, in a box j and k scroll. M-/ shows them with every part's keys in mission control.".into()
            }
            Action::NextNeedsYou => {
                "Pinned tabs and reminders come due go first, then those waiting on you, then the finished, then the notifications. Pressed again, it walks on, coming round.".into()
            }
            Action::NextIdle => {
                "Pressed again, it walks on through the idle tabs of every session, coming round. A tab with no Claude isn't on the list.".into()
            }
            Action::FocusClaude => {
                "From any pane of the tab, or the sidebar. It says so when the tab has no Claude.".into()
            }
            Action::CopyReply => {
                "The last message Claude wrote text in, from its session's transcript, onto the clipboard; a box says how many lines, or that there is no reply yet.".into()
            }
            Action::History => {
                "A timeline under the screen of the session's state, your turns, commits, review rounds and context; h/l move through time and the screen shows the pane as it was, [ ] go by turns, c to a commit, w to a wait, Esc back to now.".into()
            }
            Action::TogglePin => {
                "A pinned tab stays at the top of Needs you whatever its Claude is doing, until unpinned; going to it doesn't clear it.".into()
            }
            Action::Palette => {
                "Type to narrow by what a command does or its tags; Enter runs it in the pane you were in.".into()
            }
            Action::ResetTips => {
                "Each tip comes back once more, in the top-right corner, the next time what it teaches would help.".into()
            }
            Action::ToggleTheme => {
                "Sets the theme setting to the other one. The programs in your panes keep your terminal's own colors.".into()
            }
            Action::FocusSidebar => {
                "In the sidebar, the arrows or j and k move between tabs and Enter goes to one; pressed again, the focus goes back to the pane.".into()
            }
            Action::ToggleSidebar => {
                "Flips Settings' sidebar setting, for every tab. A window narrower than 90 columns has none either way.".into()
            }
            Action::NewClaudeTab => {
                "The directories picked before come first, then the one you're in. Claude starts from your shell, which stays in the tab when it exits; then you may name the tab.".into()
            }
            Action::WorkOnTickets => {
                "Asks for the tickets, IDs or Linear links, then the repository; each gets a worktree on its branch and a tab running Claude on it. Every ticket is checked before any tab opens.".into()
            }
            Action::WorkOnBranch => {
                "Lists the repository's branches, the most recently committed first; a name no branch matches makes a new one. The tab runs Claude in a worktree on it.".into()
            }
            Action::WorkOnPr => {
                "Asks for the PR's number or link. Claude reads the PR and says where it stands, changing nothing until you say; a fork's PR opens untrusted, running none of its code.".into()
            }
            Action::ReviewPr => {
                "Lists the repository's open PRs from forks; the tab runs Claude in the main clone, reviewing without running the PR's code outside the sealed try.".into()
            }
            Action::WorkOnNote => {
                "Asks for the repository, the note and its unticked items, then where to land; parked sections and items already under way are left out.".into()
            }
            Action::ContinueShaping => {
                "Asks for the tickets, then a directory in a repository, and runs shape --continue in a tab for each.".into()
            }
            Action::ShapeNote => {
                "Picks a note, its items and a repository, then opens a background tab running /shape for each item. The items go to Claude only once you choose them.".into()
            }
            Action::ShapeNewTicket => {
                "The idea typed goes to a background tab running /shape in the repository you pick.".into()
            }
            Action::PrImage => {
                "Chrome draws its HTML with GitHub's stylesheet and Mermaid diagrams, shown a page at a time; j, k and Space page, q closes it.".into()
            }
            Action::TicketImage => {
                "Drawn as the PR image draws a PR, a page at a time; j, k and Space page, q closes it.".into()
            }
            Action::MissionControl => {
                "Every tab of every session as a card, then the views (Work map, PRs, CI, tickets, your day, Settings) named in the bar across the top.".into()
            }
            Action::FindTab => {
                "The Work map with the open tabs alone, a filter already typing; Enter opens the chosen tab's menu, so Enter twice goes to it.".into()
            }
            Action::ReviveWorktree => {
                "The Work map with the worktrees no tab has open; Enter twice opens a tab in the chosen one and resumes its Claude. Untrusted worktrees are left out.".into()
            }
            Action::Settings => {
                "Mission control's Settings view: every option by category, then your own files' settings. What you change is kept in ~/.config/switchboard/settings.json.".into()
            }
            Action::RemindTab => {
                "Asks when (20 minutes to next Monday, or a time typed like 45m or 3pm) and what for. Until then the tab waits under Reminders.".into()
            }
            Action::TabTitle => {
                "The title wrapped, its PR number in front, then its branch, directory, PR and ticket as far as the workspace knows them; y copies the title.".into()
            }
            Action::View(command) => command.detail().into(),
            Action::Diff(command) => command.detail().into(),
            Action::SlackRelay => {
                "Each Claude tab gets a thread in the Slack app's DM: what you reply there is typed into the tab, and the relay posts back when it finishes or waits on you.".into()
            }
            Action::StopSlackRelay => {
                "Runs switchboard slack-relay stop, which says when it has stopped; Slack hears nothing more from the tabs until it starts again.".into()
            }
            Action::Update(command) => command.detail().into(),
            Action::Upgrade => {
                "Builds the checkout the running binary came from, then the server runs the new build in its own process, handing it every pane, so nothing in a pane ends.".into()
            }
            Action::KeepAwake => {
                "Held whatever the tabs do, through a restart on a new build too, until chosen again, which goes back to Settings' \"Keep the computer awake while tabs work\" (keep_awake). A ☕ shows while it holds; the display still sleeps.".into()
            }
            Action::Notes(command) => command.detail().into(),
            Action::Extra(command) => command.detail().into(),
            Action::Autopilot(command) => command.detail().into(),
        }
    }

    /// What it is about, as tmux.conf's `#tags`: matched in the palette as
    /// any other word, and shown dimmed.
    pub fn tags(&self) -> &'static [&'static str] {
        match self {
            Action::SelectPane(_) => &["panes", "navigate"],
            Action::PrevTab | Action::NextTab | Action::LastTab | Action::SelectTab(_) => {
                &["tabs", "navigate"]
            }
            Action::NewTab => &["tabs", "new"],
            Action::Split(_) => &["panes", "split"],
            Action::ClosePane => &["panes", "close"],
            Action::CloseTab => &["tabs", "close"],
            Action::NextPane | Action::LastPane => &["panes", "navigate"],
            Action::Zoom => &["panes", "layout"],
            Action::Resize(..) => &["panes", "resize"],
            Action::Detach => &["session", "detach"],
            Action::CopyMode => &["panes", "scroll", "history"],
            Action::SendPrefix => &["keys"],
            Action::ListKeys => &["help", "keys"],
            Action::NextNeedsYou | Action::NextIdle | Action::FocusClaude => {
                &["claude", "tabs", "navigate"]
            }
            Action::CopyReply => &["claude", "copy"],
            Action::History => &["claude", "tabs", "history"],
            Action::TogglePin => &["tabs", "pin"],
            Action::Palette => &["palette"],
            Action::ResetTips => &["help", "tips"],
            Action::ToggleTheme => &["theme", "colors"],
            Action::FocusSidebar => &["sidebar", "tabs", "navigate"],
            Action::ToggleSidebar => &["sidebar"],
            Action::NewClaudeTab => &["tabs", "claude", "new"],
            Action::WorkOnTickets => &["linear", "ticket", "worktree", "new"],
            Action::WorkOnBranch => &["git", "branch", "worktree", "new"],
            Action::WorkOnPr => &["github", "pr", "worktree", "new"],
            Action::ReviewPr => &["github", "pr", "review"],
            Action::WorkOnNote => &["notes", "worktree", "new"],
            Action::ContinueShaping => &["linear", "ticket", "shape"],
            Action::ShapeNote => &["notes", "linear", "shape"],
            Action::ShapeNewTicket => &["linear", "ticket", "shape"],
            Action::PrImage => &["github", "pr", "panes"],
            Action::TicketImage => &["linear", "ticket", "panes"],
            Action::MissionControl => &["sidebar", "overview", "tabs"],
            Action::FindTab => &["tabs", "navigate", "find"],
            Action::ReviveWorktree => &["worktree", "claude", "tabs", "reopen"],
            Action::Settings => &["settings", "config", "preferences"],
            Action::RemindTab => &["tabs", "remind", "todo"],
            Action::TabTitle => &["tabs"],
            Action::View(command) => command.tags(),
            Action::Diff(command) => command.tags(),
            Action::SlackRelay | Action::StopSlackRelay => &["slack", "relay", "remote"],
            Action::Update(command) => command.tags(),
            Action::Upgrade => &["switchboard", "update", "restart", "upgrade"],
            Action::KeepAwake => &["system", "sleep", "awake"],
            Action::Notes(command) => command.tags(),
            Action::Extra(command) => command.tags(),
            Action::Autopilot(command) => command.tags(),
        }
    }

    /// Its description as tmux.conf wrote one: the words, then the tags
    /// (`Split pane to the left #panes #split`).
    pub fn note(&self) -> String {
        let mut note = self.about();
        for tag in self.tags() {
            note.push_str(" #");
            note.push_str(tag);
        }
        note
    }

    /// The words and tags the palette lists it by, or `None` to leave it
    /// out: of M-1 to M-9, only the first, for all nine, as tmux.conf gave
    /// only M-1 a description.
    pub fn palette_note(&self) -> Option<(String, String)> {
        let words = match self {
            Action::SelectTab(1) => "Go to a tab by number (M-1 to M-9)".to_string(),
            Action::SelectTab(_) => return None,
            Action::Extra(command) if !command.in_palette() => return None,
            _ => self.about(),
        };
        let tags = self
            .tags()
            .iter()
            .map(|t| format!("#{t}"))
            .collect::<Vec<_>>()
            .join(" ");
        Some((words, tags))
    }

    /// Whether it opens or changes a mission control view, so a palette
    /// opened over mission control runs it there rather than closing it
    /// first.
    pub fn in_mission(&self) -> bool {
        use crate::extras::Command as Extra;
        match self {
            // Off, the Keys view's key goes on to the pane instead.
            Action::Extra(Extra::Keys) => crate::settings::is_on("view_keys"),
            _ => matches!(
                self,
                Action::MissionControl
                    | Action::Settings
                    | Action::FindTab
                    | Action::ReviveWorktree
                    | Action::Extra(Extra::Mission(_))
                    | Action::Autopilot(_)
            ),
        }
    }

    /// The commands no key is bound to by default, which the palette lists
    /// with none (tmux.conf's `palette` key table).
    pub fn palette_only() -> Vec<Action> {
        let views = crate::views::Command::ALL
            .into_iter()
            .filter(|c| c.key().is_none())
            .map(Action::View);
        let diff = crate::diff::Command::ALL
            .into_iter()
            .filter(|c| c.key().is_none())
            .map(Action::Diff);
        let notes = crate::notes::serve::Command::ALL
            .into_iter()
            .filter(|c| c.key().is_none())
            .map(Action::Notes);
        let extras = crate::extras::Command::ALL
            .into_iter()
            .filter(|c| c.key().is_none())
            .map(Action::Extra);
        [
            Action::ResetTips,
            Action::ToggleTheme,
            Action::ToggleSidebar,
            Action::WorkOnPr,
            Action::ReviewPr,
            Action::ContinueShaping,
            Action::ShapeNote,
            Action::ShapeNewTicket,
            Action::PrImage,
            Action::TicketImage,
            Action::Settings,
            Action::RemindTab,
            Action::StopSlackRelay,
            Action::Upgrade,
            Action::KeepAwake,
        ]
        .into_iter()
        .chain(views)
        .chain(diff)
        .chain(notes)
        .chain(extras)
        .chain(crate::update::Command::ALL.map(Action::Update))
        .chain(crate::autopilot::Command::ALL.map(Action::Autopilot))
        .collect()
    }
}

/// The two tables of bindings: keys pressed alone (`root`), and keys pressed
/// after the prefix.
#[derive(Clone, Debug)]
pub struct Bindings {
    pub prefix_key: Key,
    pub root: HashMap<Key, Action>,
    pub prefix: HashMap<Key, Action>,
    /// The keys pressed alone, and after the prefix, that `keys.json`
    /// binds or binds to something else, for the Keys view's "yours".
    pub yours: HashSet<Key>,
    pub yours_prefix: HashSet<Key>,
}

fn key(notation: &str) -> Key {
    Key::parse(notation)
        .unwrap_or_else(|| panic!("a default binding's key, {notation}, reads as a key"))
}

impl Default for Bindings {
    fn default() -> Bindings {
        use Action::*;
        use Direction::*;
        let mut root = HashMap::new();
        let mut bind = |k: &str, a: Action| {
            root.insert(key(k), a);
        };
        bind("M-Left", SelectPane(Left));
        bind("M-Right", SelectPane(Right));
        bind("M-Up", SelectPane(Up));
        bind("M-Down", SelectPane(Down));
        bind("M-[", PrevTab);
        bind("M-]", NextTab);
        bind("M-Tab", LastTab);
        for n in 1..=9 {
            bind(&format!("M-{n}"), SelectTab(n));
        }
        bind("M-H", Split(Left));
        bind("M-J", Split(Down));
        bind("M-K", Split(Up));
        bind("M-L", Split(Right));
        bind("M-w", ClosePane);
        bind("M-=", Zoom);
        bind("M-S-Left", Resize(Left, 5));
        bind("M-S-Right", Resize(Right, 5));
        bind("M-S-Up", Resize(Up, 5));
        bind("M-S-Down", Resize(Down, 5));
        bind("M-t", NewClaudeTab);
        bind("M-n", NewClaudeTab);
        bind("M-W", WorkOnTickets);
        bind("M-B", WorkOnBranch);
        bind("M-I", WorkOnNote);
        bind("M-a", NextNeedsYou);
        bind("M-A", NextIdle);
        bind("M-c", FocusClaude);
        bind("M-y", CopyReply);
        bind("M-h", History);
        bind("M-f", TogglePin);
        bind("M-p", Palette);
        bind("M-s", FocusSidebar);
        bind("M-Space", MissionControl);
        bind("M-S", SlackRelay);
        bind("M-j", FindTab);
        bind("M-o", ReviveWorktree);
        bind("M-T", TabTitle);
        for command in crate::views::Command::ALL {
            if let Some(k) = command.key() {
                bind(k, View(command));
            }
        }
        for command in crate::diff::Command::ALL {
            if let Some(k) = command.key() {
                bind(k, Diff(command));
            }
        }
        for command in crate::notes::serve::Command::ALL {
            if let Some(k) = command.key() {
                bind(k, Notes(command));
            }
        }
        for command in crate::extras::Command::ALL {
            if let Some(k) = command.key() {
                bind(k, Extra(command));
            }
        }
        let mut prefix = HashMap::new();
        let mut pbind = |k: &str, a: Action| {
            prefix.insert(key(k), a);
        };
        pbind("d", Detach);
        pbind("[", CopyMode);
        pbind("c", NewTab);
        pbind("x", ClosePane);
        pbind("z", Zoom);
        pbind("?", ListKeys);
        pbind("M-b", SendPrefix);
        pbind("%", Split(Right));
        pbind("\"", Split(Down));
        // The rest of tmux's own prefix keys that have a place here: tabs
        // by number (0 the tenth), next and previous, the last one, one
        // picked from the open tabs (choose-tree's place), renamed, or
        // closed after asking; panes in turn, the last one, by arrow, and
        // grown a cell at a time with Ctrl.
        for n in 1..=9 {
            pbind(&n.to_string(), SelectTab(n));
        }
        pbind("0", SelectTab(10));
        pbind("n", NextTab);
        pbind("p", PrevTab);
        pbind("l", LastTab);
        pbind("w", FindTab);
        pbind(",", Extra(crate::extras::Command::RenameTab));
        pbind("&", CloseTab);
        pbind("o", NextPane);
        pbind(";", LastPane);
        pbind("Left", SelectPane(Left));
        pbind("Right", SelectPane(Right));
        pbind("Up", SelectPane(Up));
        pbind("Down", SelectPane(Down));
        pbind("C-Left", Resize(Left, 1));
        pbind("C-Right", Resize(Right, 1));
        pbind("C-Up", Resize(Up, 1));
        pbind("C-Down", Resize(Down, 1));
        Bindings {
            prefix_key: key("M-b"),
            root,
            prefix,
            yours: HashSet::new(),
            yours_prefix: HashSet::new(),
        }
    }
}

impl Bindings {
    /// The defaults with `keys.json`'s changes, and the changes it couldn't
    /// read, in words.
    pub fn load() -> (Bindings, Vec<String>) {
        let mut bindings = Bindings::default();
        let file = crate::paths::config_dir().join("keys.json");
        let Some(text) = crate::util::read_text(&file) else {
            return (bindings, Vec::new());
        };
        let problems = match serde_json::from_str::<Value>(&text) {
            Ok(config) => bindings.apply(&config),
            Err(e) => vec![format!("{} isn't JSON: {e}", crate::paths::tilde(&file))],
        };
        (bindings, problems)
    }

    /// Applies a keys.json object; gives what it couldn't use.
    pub fn apply(&mut self, config: &Value) -> Vec<String> {
        let mut problems = Vec::new();
        if let Some(p) = config.get("prefix-key").and_then(Value::as_str) {
            match Key::parse(p) {
                Some(k) => self.prefix_key = k,
                None => problems.push(format!("prefix-key: {p} isn't a key")),
            }
        }
        for (table_name, table, yours) in [
            ("root", &mut self.root, &mut self.yours),
            ("prefix", &mut self.prefix, &mut self.yours_prefix),
        ] {
            let Some(entries) = config.get(table_name).and_then(Value::as_object) else {
                continue;
            };
            for (notation, action) in entries {
                let Some(k) = Key::parse(notation) else {
                    problems.push(format!("{table_name}: {notation} isn't a key"));
                    continue;
                };
                match action {
                    Value::Null => {
                        table.remove(&k);
                        yours.remove(&k);
                    }
                    Value::String(name) => match Action::parse(name) {
                        Some(a) => {
                            if table.get(&k) != Some(&a) {
                                yours.insert(k);
                            }
                            table.insert(k, a);
                        }
                        None => problems.push(format!("{table_name}: {name} isn't an action")),
                    },
                    other => problems.push(format!(
                        "{table_name}: {notation}: {other} isn't an action name"
                    )),
                }
            }
        }
        problems
    }

    /// Every binding as (keys, what it does), the prefix's after the root's,
    /// each table in key order.
    pub fn list(&self) -> Vec<(String, String)> {
        let mut root: Vec<_> = self
            .root
            .iter()
            .map(|(k, a)| (k.notation(), a.about()))
            .collect();
        root.sort();
        let mut prefix: Vec<_> = self
            .prefix
            .iter()
            .map(|(k, a)| {
                (
                    format!("{} {}", self.prefix_key.notation(), k.notation()),
                    a.about(),
                )
            })
            .collect();
        prefix.sort();
        root.into_iter().chain(prefix).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_names_round_trip() {
        for a in [
            Action::SelectPane(Direction::Up),
            Action::SelectTab(7),
            Action::Resize(Direction::Left, 3),
            Action::Split(Direction::Down),
            Action::Detach,
            Action::CloseTab,
            Action::NextPane,
            Action::LastPane,
            Action::NextNeedsYou,
            Action::NextIdle,
            Action::FocusClaude,
            Action::CopyReply,
            Action::History,
            Action::TogglePin,
            Action::Palette,
            Action::ResetTips,
            Action::ToggleTheme,
            Action::FocusSidebar,
            Action::ToggleSidebar,
            Action::MissionControl,
            Action::FindTab,
            Action::ReviveWorktree,
            Action::Settings,
            Action::RemindTab,
            Action::TabTitle,
            Action::Upgrade,
            Action::KeepAwake,
            Action::ShapeNote,
            Action::ShapeNewTicket,
            Action::PrImage,
            Action::TicketImage,
            Action::View(crate::views::Command::PrView),
            Action::View(crate::views::Command::OpenLinkRead),
            Action::Diff(crate::diff::Command::Diff),
            Action::Diff(crate::diff::Command::Ask),
            Action::Notes(crate::notes::serve::Command::Beside),
            Action::Notes(crate::notes::serve::Command::NoteTab),
            Action::Extra(crate::extras::Command::GoToTab),
            Action::Extra(crate::extras::Command::Mission(
                crate::extras::MissionView::Landing,
            )),
            Action::Autopilot(crate::autopilot::Command::WorkNote),
        ] {
            assert_eq!(Action::parse(&a.name()), Some(a));
        }
        assert_eq!(
            Action::parse("resize-up"),
            Some(Action::Resize(Direction::Up, 5))
        );
        assert_eq!(Action::parse("select-tab-0"), None);
        assert_eq!(Action::parse("fly"), None);
    }

    #[test]
    fn the_keys_keys_json_binds_are_yours_and_the_defaults_are_not() {
        let mut b = Bindings::default();
        let problems = b.apply(&serde_json::json!({
            "root": { "M-g": "new-tab", "M-j": "find-tab", "M-w": null },
            "prefix": { "y": "copy-mode" },
        }));
        assert!(problems.is_empty(), "{problems:?}");
        let k = |n: &str| Key::parse(n).unwrap();
        assert!(b.yours.contains(&k("M-g")));
        assert!(
            !b.yours.contains(&k("M-j")),
            "bound to what it already does"
        );
        assert!(!b.yours.contains(&k("M-w")), "unbound");
        assert!(!b.yours.contains(&k("M-t")));
        assert!(b.yours_prefix.contains(&k("y")));
        assert!(Bindings::default().yours.is_empty());
    }

    #[test]
    fn notes_read_as_tmux_confs() {
        assert_eq!(
            Action::Split(Direction::Left).note(),
            "Split pane to the left #panes #split"
        );
        assert_eq!(
            Action::Resize(Direction::Down, 5).note(),
            "Grow pane down #panes #resize"
        );
        assert_eq!(
            Action::Resize(Direction::Up, 2).about(),
            "Grow pane up by 2"
        );
        assert_eq!(
            Action::SelectTab(1).palette_note().unwrap().0,
            "Go to a tab by number (M-1 to M-9)"
        );
        assert_eq!(Action::SelectTab(4).palette_note(), None);
    }
}
