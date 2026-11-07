//! Switchboard, as one library the `switchboard` binary runs and the tests
//! call into. Each part says at its top where it came from in the tmux and
//! Node version, so its history can be followed back. ARCHITECTURE.md has
//! the whole picture.
//!
//! Shared by every part:
//! - `paths`: where things are, through `~` and the clone.
//! - `util`: time, files, commands, JSON as JavaScript writes it.
//! - `settings`: the workspace's options (`assets/settings.json`).
//! - `requests`: the request audit for the Usage view.
//! - `platform`: what differs between macOS and Linux.
//! - `presence`: whether you're at the computer.
//! - `connector`: one call to a Claude connector.
//! - `worktree`: what the worktree commands share.
//! - `net`: HTTPS requests, each counted in the request audit.
//! - `integrations`: GitHub and Linear over their APIs.
//!
//! - `facts`: what GitHub and the landing queue say of each tab.
//! - `items`: what mission control's views list, in one shape for every source.
//! - `mux`: the multiplexer (in place of tmux).
//! - `agent`: what each Claude session is doing, from its hooks.
//! - `tabs`: what each tab is called and the color it wears.
//! - `sources`: what the sidebar and mission control know from GitHub and
//!   Linear, looked up in the background.
//! - `day`: your day: the calendar, Slack, the notes written for you.
//! - `sidebar`: every tab on the left of the screen, most in need of you
//!   first, and the model mission control shares.
//! - `ui`: the boxes over the workspace: messages, tips, questions, menus,
//!   pickers and the palette.
//! - `mission`: mission control (M-Space), every tab as a card and every
//!   view beside them, over the whole screen.
//! - `usage`: how the workspace is used, on this computer alone: habits and
//!   their suggestions, each tab's CPU and memory, the Claude sessions and
//!   the day's spend, closed tabs, your own files' settings, plugins.
//! - `links`: opening a link in the browser, in its site's Chrome profile.
//! - `views`: reading things beside your work: the PR description, the
//!   ticket, any link, search everything, and whether the PR can merge.
//! - `status`: the status line's button, health light and second line, and
//!   dark and light.
//! - `setup`: what `switchboard setup` puts in place for Claude Code: its
//!   hooks, statusline and permissions, your CLAUDE.md, and the skills.
//! - `diff`: M-d's diff in a pane's place, told as a story, with questions
//!   about it answered.
//! - `relay`: the Slack relay, a Slack app talking to you in its DM about
//!   your tabs while you are away.
//! - `update`: keeping Switchboard current: `switchboard update`, and the
//!   hourly look for a new release, offered in a box.
//! - `notes`: the notes app, `switchboard notes`, in place of the Neovim
//!   plugin.
//! - `extras`: the rest of tmux.conf's keys and M-p's commands: renaming the
//!   tab, mission control on a view, the habits report, a reply sent to
//!   another tab, setup checked, the loop tabs, a PR or ticket by number,
//!   the AWS login, M-0 and `C-w` between panes.
//! - `autopilot`: working a note's checklist continuously, a tab opened for
//!   each item as the last lands.

pub mod agent;
pub mod autopilot;
pub mod cmd;
pub mod connector;
pub mod day;
pub mod diff;
pub mod extras;
pub mod facts;
pub mod integrations;
pub mod items;
pub mod links;
pub mod mission;
pub mod mux;
pub mod net;
pub mod notes;
pub mod paths;
pub mod platform;
pub mod presence;
pub mod relay;
pub mod requests;
pub mod settings;
pub mod setup;
pub mod sidebar;
pub mod sources;
pub mod status;
pub mod tabs;
pub mod ui;
pub mod update;
pub mod usage;
pub mod util;
pub mod views;
pub mod worktree;
