# Porting Switchboard to Rust

Working notes for the rewrite on the `rust-conversion` branch. When it is done,
this file becomes `ARCHITECTURE.md` (the parts that still describe the code) and
the rest goes; `CLAUDE.md`, `README.md` and the user docs are rewritten for the
new app before the PR.

## What it becomes

- **One binary, `switchboard`, that someone downloads and runs.** `switchboard`
  alone starts the workspace (or attaches to it); `switchboard <command>` runs
  everything else. No tmux, no Node, no Neovim, fzf, glow, lazygit or jq
  needed. Claude Code and git are the only programs it expects; anything else
  (lazygit, Neovim, a shell of your choice) runs in a pane like any program.
- **Its own terminal multiplexer, with a server.** A background server owns
  every pane's terminal, so closing the window never kills a running agent;
  clients attach and detach as with tmux. The layout is saved as it changes
  and restored after a reboot, each Claude pane resumed with `claude --resume`.
- **Everything built in**: the sidebar and mission control, the palette, menus,
  messages and tips, the status line, the Markdown views (PR description,
  ticket, link view), the diff viewer with its story and ask, and the notes
  app as an editor of its own.
- **The dotfiles go** (tmux.conf, the Neovim config, lazygit, Ghostty,
  Karabiner, pgcli), and so does `setup.sh`: `switchboard setup` adds the
  Claude Code hooks, statusline and skills, and nothing else.
- **Runs on** x86_64 Linux, x86_64 macOS and aarch64 macOS, built with the
  latest stable Rust (pinned in `rust-toolchain.toml`).
- **Crates are fine; system packages are not.** Pure-Rust crates (rustls, not
  OpenSSL; zbus, not libdbus), so a build needs rustup and a C compiler, and a
  user needs nothing.
- **Delivered as one PR** to cquinones100/switchboard once everything is done.

## Architecture

```
switchboard (client) ──Unix socket── switchboard server ──PTYs── shells, claude, ...
switchboard hook <event>  ──┘            │
switchboard <command>     ──┘            ├─ background tasks: GitHub, Linear, calendar,
                                         │  Slack, transcripts, git, claude -p
                                         └─ ~/.local/state/switchboard (layout, state)
```

- **Server** (`src/mux/server.rs`): one task owns all state (`Server`): sessions,
  tabs (windows), panes, clients, overlays. Everything else (client sockets,
  PTY readers, timers, background lookups, file watches) sends it `Event`s over
  a channel; it handles them in order and draws the clients that changed. So
  nothing locks, and a slow lookup never holds up drawing. Runs on a tokio
  runtime; blocking work goes to `spawn_blocking`.
- **Panes** (`src/mux/pane.rs`): a PTY (portable-pty) and a terminal emulator
  (alacritty_terminal) each, with a reader thread per pane. The emulator's
  replies to programs' queries go back to the PTY; OSC 52 goes to the clients'
  clipboard. Every pane's environment carries `SWITCHBOARD` (the socket) and
  `SWITCHBOARD_PANE` (its id), which the hooks and commands use the way tmux's
  `TMUX` and `TMUX_PANE` were used.
- **Layout** (`src/mux/layout.rs`): each tab is a tree of splits; floating panes
  and built-in screens are overlays above it (what tmux's popups were).
- **Drawing** (`src/mux/compose.rs`): per client, a ratatui `Buffer` is filled
  with the tab (sidebar, panes, borders, status line) and any overlay, and only
  the cells that changed are sent. The built-in screens (`src/ui/`) are ratatui
  widgets drawing into that buffer.
- **Input** (`src/mux/input.rs`, `src/mux/keys.rs`): the client sends raw bytes;
  the server parses them into keys, mouse and paste, each with its raw bytes,
  matches bindings (the default table is what tmux.conf bound, under the same
  keys; `~/.config/switchboard/keys.json` changes them), and passes anything
  unbound to the focused pane, mouse events re-encoded for its position and
  mode.
- **Client** (`src/mux/client.rs`): raw mode, alternate screen, mouse and paste
  reporting on; stdin to the socket, the socket to stdout; resizes sent on
  SIGWINCH; the terminal put back however it ends. Starts the server
  (detached, `setsid`) when none answers.
- **Apps in panes**: what ran in a pane under tmux (glow and less for a PR
  or ticket, diffview in Neovim, the notes app in Neovim, lazygit) is a
  program of Switchboard's own run in a pane the same way: `switchboard view`,
  `switchboard diff`, `switchboard notes`, full-screen programs drawn with
  ratatui and crossterm. The server stays a multiplexer, opening them in a
  split or in a pane's place as tmux did, and each app is built and tested
  on its own.
- **Commands** (`src/mux/commands.rs`): what scripts and skills used tmux for
  (rename a tab, set a tab's note, send keys to a pane, open a tab, say
  something) is `switchboard <command>` talking to the server.
- **Agent state** (`src/agent/`): Claude Code's hooks run `switchboard hook`,
  which sends the hook's JSON and `SWITCHBOARD_PANE` to the server: what
  `tmux/agent-state.sh` did, on the server's own model.
- **Persistence** (`src/mux/persist.rs`): the layout (tabs, splits, each pane's
  directory and program, its Claude session) and tab options written on change;
  restored when a server starts with no sessions (`auto_restore` setting;
  `save_contents` keeps what panes show).

### Where things live

| What | Where |
| --- | --- |
| Your settings and own files | `~/.config/switchboard/` (as before: `settings.json`, `sidebar.json`, `browser.json`, `trusted-dirs`, `claude.json`, `slack-relay.json`, `oncall.json`, keys, plugins) |
| State | `~/.local/state/switchboard/` (the socket, layout, tips, habits, requests, and what was `~/.local/state/tmux-sidebar/`) |
| Caches | `~/.cache/switchboard/` |
| The settings schema, skills, prompts, themes | `assets/`, compiled into the binary (`include_str!`), so the binary needs no clone |

### What replaces what

| Before | After |
| --- | --- |
| tmux, tmux.conf, `tmux/*.sh` | `src/mux/` and the parts below; key defaults from tmux.conf |
| tmux user options (`@agent_state`, `@tab_label`, ...) | typed fields on panes and tabs, plus a string map for plugins |
| `display-popup`, `say.sh`, `tip.sh`, `menu.sh`, `confirm.sh` | overlays in `src/ui/` |
| fzf (palette, pickers) | `src/ui/picker.rs` with nucleo-matcher |
| glow, mermaid-ascii | `src/views/markdown.rs` (pulldown-cmark), a Mermaid flowchart/sequence drawer |
| diffview.nvim, `nvim/lua/diffstory.lua`, `diffask.lua`, `pane-in-place.sh`, `diff-base.sh`, `diff-story-ahead.sh` | `src/diff/` (`switchboard diff`; syntect for highlighting, similar for the line and word diffs) |
| the notes app (`notes/`, Neovim), `bin/notes` | `src/notes/` (`switchboard notes`; ropey for the text, notify for the watch, image for pictures other than PNGs) |
| `gh` | GitHub's API over reqwest; the token from `GH_TOKEN`/`GITHUB_TOKEN`, `gh auth token` when gh is there, else a stored secret |
| `security`, `pbcopy`, `osascript`, `ioreg`, `open` | `src/platform/` |
| tmux-resurrect, tmux-continuum | `src/mux/persist.rs` |
| `setup.sh`, `install.sh`, `.githooks` (the dotfile links) | `switchboard setup` (hooks, statusline, skills) and release downloads |
| the hourly update popup (git pull) | the same popup, offering a new release (checked, then the binary replaced) |
| `bin/*` Node programs, `lib/` | `src/cmd/`, `src/integrations/`, `src/relay/`, ... |

## Plan

Each step is a commit (or a few) on the branch, with its tests.

1. **Foundation** — done: the crate, `paths`, `util`, `settings`, `requests`,
   `platform` (macOS and Linux), `presence`, `connector`, `worktree`, the
   command table.
2. **The multiplexer core** — done: IPC, client, server loop, panes, input,
   layout, drawing, bindings for panes and tabs, scrollback and copy mode,
   mouse, clipboard, detach/attach (tests/mux.rs runs a server in a temporary
   HOME and reads its screen through a test client), and the layout saved and
   restored (`persist`, tests/persist.rs restarts a server in the same home).
3. **Agent state and tabs** — agent state done (src/agent: `switchboard hook`,
   the states and their markers and counts in the status line, notifications,
   one Claude per tab, pinned tabs, M-a/M-c/M-f, and the messages and tips
   agent-state.sh showed) and tab names and accent colors done (src/tabs:
   tab-label.sh, accent-color.sh, window-names.sh, path-facts.sh, the PR
   prefix and ticket titles, looked up in the background), and the rest of
   the status line done (src/status: the second status line from
   tab-status.sh, the session badge and the focused pane's lines in the
   tab's color from update-colors.sh, the ⌘ Commands button, health.sh's
   light, and theme.sh's dark and light with `system` read from the
   terminal's answer to OSC 11); left: reminders (remind.sh).
4. **Sidebar and mission control** — the boxes done (src/ui: an overlay stack
   per client, messages, tips, the question before M-w closes a busy pane,
   menus, pickers with nucleo-matcher, the M-p palette and its ranking), and
   the sidebar done (src/sidebar: drawn by the server left of the panes, the
   model with its groups and stages, Claude's transcripts read, M-s and its
   keys, the mouse, peeking, M-a's walk through Needs you, and the merged,
   loop and sidebar tips), and mission control done (src/mission: an overlay
   over the whole screen on the same model, the cards by stage, every view
   with its menus, answering a tab, reminders, Settings, the Work map and
   M-j and M-o's pickers); left: the tips whose keys come with later parts
   (worktree, in-place, updates, github_off, linear_off, option-alt), and in
   mission control what waits on later parts (a link read in a pane beside,
   the diff and lazygit for a tab, the Calendar drawn as a day, the mode
   that works a note continuously).
5. **Integrations**: GitHub, Linear, calendar, Slack look, meetings, standup and
   todo notes, habits, resources, usage, plugins (executables printing JSON),
   search, link view, PR and ticket views, the request audit. GitHub and
   Linear's lookups done (src/sources: every tab's PR facts and landing
   place, and the PRs, CI, Merge queues, Tickets and Inbox views' data,
   polled and guarded as the daemon did, paused while away). Reading
   beside your work done (src/views: `switchboard view`, `pr-view`,
   `ticket-view` and `link-view`, glow's Markdown and mermaid-ascii's
   diagrams drawn and paged as less did, opened and closed by M-v and M-i
   in a pane to the right; M-r, M-l, M-m's box, search everything and the
   palette's link commands), and every link opened in its site's Chrome
   profile (src/links.rs, `switchboard open-url`). Left: the PR
   description as an image (pr-image.js), the media picker, and OSC 8
   links passed through the multiplexer to the terminal (a click in a view
   opens its link meanwhile).
6. **Worktrees and Claude programs**: `wt`/`wtn`/`wtr`/`wtb`, `claude-here`,
   `codex-review`, `shape`, `skill-path`, `land-lock`, `wait-deploy`,
   `watch-note`, the skills' scripts, the skills' text moved off tmux.
7. **The Slack relay** — done (src/relay: `switchboard slack-relay` and its
   commands, Socket Mode and the Web API with the owner-only rules, the
   tabs read and typed into through the server, follow-ups, alerts, stuck
   tabs, milestones, home threads, the Home tab and tab screens, buttons and
   forms, images both ways, notices, the offline note, the watchdog; M-S
   and the palette's stop).
8. **The diff viewer**, with its story, pages and ask — done (src/diff:
   `switchboard diff` in a pane's place, the pane parked behind it and
   given back when it ends, with M-p's side and full; M-d against the PR's
   base, fetched unless the branch is where it last compared; the files in
   reading order beside a side-by-side diff with its changed words, syntax
   colours and folds; the story with its cover, chapters, notes, pages and
   Markdown, cached and followed while another program tells it; told
   ahead for each tab's new commit; `ga`'s question; easy mode's buttons).
   Left: the branch's commits one at a time (diffview's `c`), visual-mode
   ranges for yp and yl, and a working tree's changes, which M-d never
   showed.
9. **The notes app** — done (src/notes: `switchboard notes`, the list of
   folders, tags and notes beside a note drawn as Notes shows it, with its
   tables, folds and pictures under their lines through kitty's Unicode
   placeholders; Vim's keys and easy mode's, the toolbar, the Format menu,
   the palette and the finder; the lock, `.notes.json`, renames, moves and
   folders; every save merging in what changed outside, and a watch on the
   directory; squashing through Claude after a yes; a socket the workspace
   opens a tab's note through, `open <note>`, `sidebar on|off`, `shown`).
   Left, for the server: M-N and M-e (Notes in a tab of its own or beside
   one, the list put away there), note tabs and picking a tab's note
   (`note-tab.sh`, `note-pick.sh`), the palette's Switchboard commands
   (the app asks `switchboard press <name>`), passing the Cmd keys to a pane
   marked `@notes_cmd_keys`, and passing kitty's graphics commands and the
   placeholders' marks through to the terminal.
10. **`switchboard setup`, updates, releases**: Claude settings merge, skills,
    statusline, CI and release builds for the three targets, self-update.
    `switchboard setup` done (src/setup: the `claude`, `claude-md` and
    `skills` parts and the guided run, `assets/claude-settings.json` with
    hooks running `switchboard hook <Event>`, the skills compiled in from
    `assets/skills` by build.rs and moved off tmux onto `switchboard`'s
    commands, `respawn`, `tabs` and `setting` among them), and `switchboard
    statusline` done (src/cmd/statusline.rs), and releases and updates
    done (`.github/workflows/ci.yml` and `release.yml`: a `v<version>` tag
    matching Cargo.toml's version builds the three targets and publishes
    each archive with its SHA-256; install.sh installs the newest into
    `~/.local/bin`; src/update: `switchboard update`, the hourly look and
    the box on attaching, the binary swapped only once its checksum
    matches and it runs; `switchboard --version` names its commit).
11. **Docs and removal**: README, CLAUDE.md, user docs; the old files removed;
    the PR. Before the PR:
    - point `src/update/release.rs`'s `REPO` and install.sh at
      cquinones100/switchboard, and README's install line at the new
      install.sh;
    - settle which modifier the keys use on Linux (Alt with a `modifier`
      setting was proposed);
    - a server-side `respawn` (a pane's program started again in place),
      replacing the workarounds in `switchboard respawn` (split and close)
      and the Slack relay (typing into the leftover shell);
    - CI's clippy with `--no-default-features` once tests/views.rs builds
      without dead-code warnings there;
    - the review loop (`/review-loop`, Claude reviewing where Codex isn't
      installed) over the branch, offered before the PR.

## Conventions

- Each module opens with a comment saying what it is and why it works as it
  does, in the voice of the original code's comments; a part ported from a
  Node, shell or Lua file names that file, so its history can be followed.
- Behavior matches the original unless the original depended on tmux or
  macOS; where it changes, the comment says so.
- No tmux, no Node, no external program but Claude Code and git (and the
  platform's own tools in `src/platform/`). Desktop differences go through
  `src/platform/`.
- Files written whole go through `util::write_atomic`; JSON numbers through
  `util::js_number`, so files read as they did.
- Server state is touched only by the server task; anything slow runs
  elsewhere and reports back with an `Event`.
- Tests: unit tests beside the code; integration tests in `tests/` with a
  temporary HOME and a server of their own, never the real one.
