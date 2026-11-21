//! Switchboard, as one library the `switchboard` binary runs and the tests
//! call into. Each part says at its top where it came from in the tmux and
//! Node version, so its history can be followed back. ARCHITECTURE.md has
//! the whole picture.
//!
//! Shared by every part:
//! - `paths`: where things are, through `~` and the clone.
//! - `util`: time, files, commands, JSON as JavaScript writes it.
//! - `settings`: the workspace's options (`assets/settings.json`).
//! - `assets`: what the binary carries in `assets/`, read from the clone
//!   the build came from while it is there; `skill_files`, the skills' walk
//!   it shares with build.rs.
//! - `requests`: the request audit for the Usage view.
//! - `platform`: what differs between macOS and Linux.
//! - `presence`: whether you're at the computer.
//! - `slots`: the queue for heavy work, so only so many builds and test
//!   runs go at once.
//! - `connector`: one call to a Claude connector.
//! - `worktree`: what the worktree commands share.
//! - `net`: HTTPS requests, each counted in the request audit.
//! - `integrations`: GitHub and Linear over their APIs.
//! - `trust`: whose code a directory holds: trusted-dirs, your own code,
//!   and someone else's PR checked out untrusted.
//! - `dev`: whether a directory, or the running build, is Switchboard's own
//!   clone, which alone gets what serves developing Switchboard.
//!
//! - `facts`: what GitHub and the landing queue say of each tab.
//! - `items`: what mission control's views list, in one shape for every source.
//! - `repos`: your settings for each repository, `repos.json`.
//! - `files`: your own files kept in a git repository, pulled and pushed
//!   as they change.
//! - `mux`: the multiplexer (in place of tmux).
//! - `agent`: what each Claude session is doing, from its hooks.
//! - `tabs`: what each tab is called and the color it wears.
//! - `history`: a tab's history (M-h), its Claude session on a timeline
//!   under the screen as it was.
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
//! - `photos`: people's photos from Slack, Linear and GitHub for their
//!   avatars, fetched once and kept.
//! - `views`: reading things beside your work: the PR description, the
//!   ticket, any link, a pane's links and images, search everything, and
//!   whether the PR can merge.
//! - `status`: the status line's button, health light and second line, and
//!   dark and light.
//! - `setup`: what `switchboard setup` puts in place for Claude Code:
//!   Switchboard's plugin (its hooks, skills and instructions, for the
//!   Claude in its panes) or the same in `~/.claude`, the statusline and
//!   permissions, and your CLAUDE.md.
//! - `diff`: the server's side of M-d's diff, which is a crate of its own
//!   (`switchboard-diff`, crates/diff) loaded here as a plugin: the diff in
//!   a pane's place, the workspace answering what it asks, the story told
//!   ahead; lazygit in a pane's place.
//! - `relay`: the Slack relay, a Slack app talking to you in its DM about
//!   your tabs while you are away.
//! - `phone`: the phone app, `switchboard phone`, a web app served on your
//!   tailnet alone for answering and messaging your Claude tabs.
//! - `update`: keeping Switchboard current: `switchboard update`, and the
//!   hourly look for a new release, offered in a box.
//! - `notes`: the server's side of Notes, whose app is a crate of its own
//!   (`switchboard-notes`, crates/notes, in place of the Neovim plugin)
//!   loaded here as a plugin: `switchboard notes`, the workspace answering
//!   what it asks, and Notes on the server (M-N, M-e, a tab's note).
//! - `extras`: the rest of tmux.conf's keys and M-p's commands: renaming the
//!   tab, mission control on a view, the habits report, a reply sent to
//!   another tab, setup checked, the loop tabs, a PR or ticket by number,
//!   the AWS login, M-0 and `C-w` between panes.
//! - `autopilot`: working a note's checklist continuously, a tab opened for
//!   each item as the last lands.
//! - `awake`: keeping the computer awake while tabs work or autopilot has
//!   tabs open, or until you say.
//! - `routines`: a prompt of yours run on a schedule, each run in a Claude
//!   tab of its own.
//! - `merge_queue`: Switchboard's own merge queue for a repository landing
//!   through PRs with no queue of GitHub's: PRs tested together in trains
//!   on a `ci/train-<id>` branch and merged in order once it passes.
//! - `worktree_sweep`: a worktree removed on its own once its work has
//!   landed and no tab has it open, its branch kept.
//!
//! - `demo`: `switchboard demo`, a workspace of made-up data reaching no
//!   service, with the strict stand-in services the tests share
//!   (`demo::services`).
//!
//! And for the unit tests alone, the integration tests' strict stand-ins
//! for the programs Switchboard runs (tests/common), so each kind is
//! written once: `stand_in`, and `fake_http`, the demo's services.

pub mod agent;
pub mod assets;
pub mod autopilot;
pub mod awake;
pub mod cmd;
pub mod connector;
pub mod day;
pub mod demo;
pub mod dev;
pub mod diff;
pub mod extras;
pub mod facts;
pub mod files;
pub mod history;
pub mod integrations;
pub mod items;
pub mod links;
pub mod merge_queue;
pub mod mission;
pub mod mux;
pub mod net;
pub mod notes;
pub mod paths;
pub mod phone;
pub mod photos;
pub mod platform;
pub mod presence;
pub mod relay;
pub mod repos;
pub mod requests;
pub mod routines;
pub mod settings;
pub mod setup;
pub mod sidebar;
pub mod skill_files;
pub mod slots;
pub mod sources;
pub mod status;
pub mod tabs;
pub mod trust;
pub mod ui;
pub mod update;
pub mod usage;
pub mod util;
pub mod views;
pub mod worktree;
pub mod worktree_sweep;

#[cfg(all(test, feature = "net"))]
pub(crate) use demo::services as fake_http;
#[cfg(test)]
#[path = "../tests/common/stand_in.rs"]
pub(crate) mod stand_in;
