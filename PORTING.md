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
| diffview.nvim, `nvim/lua/diffstory.lua`, `diffask.lua` | `src/views/diff/` (syntect for highlighting) |
| the notes app (`notes/`, Neovim) | `src/views/notes/` (ropey) |
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
   prefix and ticket titles, looked up in the background); left: the second
   status line (tab-status.sh), the session badge and focused border in the
   tab's color (update-colors.sh), and reminders (remind.sh).
4. **Sidebar and mission control** — the boxes done (src/ui: an overlay stack
   per client, messages, tips, the question before M-w closes a busy pane,
   menus, pickers with nucleo-matcher, the M-p palette and its ranking), and
   the sidebar done (src/sidebar: drawn by the server left of the panes, the
   model with its groups and stages, Claude's transcripts read, M-s and its
   keys, the mouse, peeking, M-a's walk through Needs you, and the merged,
   loop and sidebar tips); left: mission control on the same model, its
   views, menus and keys, Settings, and the tips whose keys come with later
   parts (worktree, in-place, updates, github_off, linear_off, option-alt).
5. **Integrations**: GitHub, Linear, calendar, Slack look, meetings, standup and
   todo notes, habits, resources, usage, plugins (executables printing JSON),
   search, link view, PR and ticket views, the request audit. GitHub and
   Linear's lookups done (src/sources: every tab's PR facts and landing
   place, and the PRs, CI, Merge queues, Tickets and Inbox views' data,
   polled and guarded as the daemon did, paused while away).
6. **Worktrees and Claude programs**: `wt`/`wtn`/`wtr`/`wtb`, `claude-here`,
   `codex-review`, `shape`, `skill-path`, `land-lock`, `wait-deploy`,
   `watch-note`, the skills' scripts, the skills' text moved off tmux.
7. **The Slack relay**.
8. **The diff viewer**, with its story, pages and ask.
9. **The notes app**.
10. **`switchboard setup`, updates, releases**: Claude settings merge, skills,
    statusline, CI and release builds for the three targets, self-update.
11. **Docs and removal**: README, CLAUDE.md, user docs; the old files removed;
    the PR.

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
