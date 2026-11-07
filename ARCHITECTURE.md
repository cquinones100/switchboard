# Architecture

How Switchboard is built: what the binary is, its parts, where each part keeps its state, how it is tested, and where it came from. [README.md](README.md) says what it is for someone using it, [docs/guide.md](docs/guide.md) what each feature does, and [CLAUDE.md](CLAUDE.md) the rules for working on it.

## What it is

- **One binary, `switchboard`.** `switchboard` alone starts the workspace or attaches to it; `switchboard <command>` runs everything else (`switchboard help` lists them). A link to the binary named after a command runs that command too. Claude Code and git are the only programs it expects; anything else (an editor, lazygit, a shell of your choice) runs in a pane like any program.
- **Its own terminal multiplexer, with a server.** A background server owns every pane's terminal, so closing the terminal never stops a running agent; clients attach and detach as with tmux. The layout is saved as it changes and restored after a reboot, each Claude pane resumed with `claude --resume`.
- **Everything built in**: the sidebar and mission control, the palette, menus, messages and tips, the status line, the Markdown views (PR description, ticket, link view), the diff viewer with its story and questions, the notes app, the Slack relay.
- **No dotfiles.** `switchboard setup` puts in place what Claude Code needs (its hooks, statusline, permissions, CLAUDE.md and skills) and nothing else.
- **Runs on** x86_64 Linux, x86_64 macOS and aarch64 macOS, built with the latest stable Rust, pinned in `rust-toolchain.toml`.
- **Crates are fine; system packages are not.** Pure-Rust crates (rustls, not OpenSSL; zbus, not libdbus), so a build needs rustup and a C compiler, and a user needs nothing. The `net` feature (on by default) holds the only C the build compiles, TLS's, so `cargo check --no-default-features --target aarch64-apple-darwin` type-checks every macOS path from Linux.

## The parts

```
switchboard (client) ──Unix socket── switchboard server ──PTYs── shells, claude, ...
switchboard hook <event>  ──┘            │
switchboard <command>     ──┘            ├─ background tasks: GitHub, Linear, calendar,
                                         │  Slack, transcripts, git, claude -p
                                         └─ ~/.local/state/switchboard (layout, state)
```

- **Server** (`src/mux/server.rs`): one task owns all state (`Server`): sessions, tabs, panes, clients, overlays. Everything else (client sockets, PTY readers, timers, background lookups, file watches) sends it `Event`s over a channel; it handles them in order and draws the clients that changed. So nothing locks, and a slow lookup never holds up drawing. It runs on a tokio runtime; blocking work goes to `spawn_blocking` or a thread of its own. One server runs per state directory, held by a lock (`src/mux/socket.rs`); a client that finds none starts it detached (`setsid`), its output in `server.log`.
- **Panes** (`src/mux/pane.rs`): a PTY (portable-pty) and a terminal emulator (alacritty_terminal) each, with a reader thread per pane. The emulator's replies to programs' queries go back to the PTY; OSC 52 goes to the clients' clipboard. Every pane's environment carries `SWITCHBOARD` (the socket) and `SWITCHBOARD_PANE` (its id), which the hooks and commands use as tmux's `TMUX` and `TMUX_PANE` were used, and `GH_BROWSER` (`switchboard open-url`), so gh's `--web` opens in the Chrome profile `browser.json` gives the site. `src/mux/passthrough.rs` lets the notes pane alone send kitty's graphics commands on to the terminal and take easy mode's Cmd keys.
- **Layout** (`src/mux/layout.rs`): each tab is a tree of splits. Overlays sit above it: the boxes of `src/ui/` and mission control (what tmux's popups were).
- **Drawing** (`src/mux/compose.rs`): per client, a ratatui `Buffer` is filled with the tab (sidebar, panes, borders, status line) and any overlay, and only the cells that changed are sent, each a pane's program made part of a hyperlink (OSC 8) inside it, kept beside the buffer since a cell can't carry one. The built-in screens draw into that buffer.
- **Input** (`src/mux/input.rs`, `src/mux/keys.rs`): the client sends raw bytes; the server parses them into keys, mouse and paste, each with its raw bytes, matches bindings (the defaults are what tmux.conf bound, under the same keys; `~/.config/switchboard/keys.json` changes them), and passes anything unbound to the focused pane, encoded again for it: mouse events for its position and mode, and keys in the form its program asked for. As tmux's `extended-keys on` did, the client asks the terminal for keys told apart (the kitty keyboard protocol's flag 1 and modifyOtherKeys level 2, `extended_keys` setting), which the parser reads in either form, and a key reaches a pane as CSI u only when its program turned on the kitty protocol (which alacritty_terminal keeps) or modifyOtherKeys (watched for in its output, `src/mux/extkeys.rs`), its old bytes otherwise: Ctrl-Shift-P reaches Notes as itself and a shell as Ctrl-P.
- **Client** (`src/mux/client.rs`): raw mode, the alternate screen, mouse and paste reporting on, keys told apart where the terminal can; stdin to the socket, the socket to stdout; resizes sent on SIGWINCH; the terminal put back however it ends. It refuses to attach inside a pane of its own server.
- **Apps in panes**: what ran in a pane under tmux (glow and less for a PR or ticket, diffview in Neovim, the notes app in Neovim) is a program of Switchboard's own run in a pane the same way: `switchboard pr-view`, `ticket-view`, `link-view`, `view`, `diff`, `notes`, full-screen programs drawn with ratatui and crossterm. The server stays a multiplexer, opening them in a split or in a pane's place (the pane parked until the program ends), and each app is built and tested on its own.
- **Commands** (`src/mux/commands.rs`): what scripts and skills used tmux for (list the tabs, open a tab, send keys to a pane, read its screen, rename a tab, set an option, say something) is `switchboard <command>` sent over the socket and run on the server's own state.
- **Agent state** (`src/agent/`): Claude Code's hooks run `switchboard hook <event>`, which sends the hook's JSON and `SWITCHBOARD_PANE` to the server, which marks the pane working, waiting or done and rolls that up into its tab.
- **Persistence** (`src/mux/persist.rs`): the layout (tabs, splits, each pane's directory and program, its Claude session) and tab options, written on change; restored when a server starts with no sessions (`auto_restore` setting; `save_contents` keeps what panes show).

Each part beyond the multiplexer has the same shape: modules that work on files and plain data, tested without a server, and a `serve.rs` whose `impl Server` puts them on the server (its keys, its tick, the answers of its background tasks as an `Event` of its own). Anything that reads the network, starts a process or reads more than a small file runs in a task and reports back, so nothing on the drawing path waits.

## The modules

Shared by every part:

| Module | What it is |
|---|---|
| `paths` | Where things are, all through `~` (`$HOME`), so a test or a try can give a run a home of its own |
| `util` | Time, files written whole (`write_atomic`), JSON as JavaScript wrote it (`js_number`), commands with their output, a runtime for short async work |
| `settings` | The workspace's options: the schema in `assets/settings.json`, what you changed in `~/.config/switchboard/settings.json` |
| `requests` | The request audit: every request to a service outside the computer counted as it is made |
| `net` | HTTPS through one client per runtime (reqwest on rustls), each request counted |
| `platform` | What differs between macOS and Linux: secrets, clipboard, notifications, presence, opening a file or link |
| `presence` | Whether you're at the computer |
| `connector` | One call to a Claude connector through a headless Haiku |
| `integrations` | GitHub and Linear over their APIs, refused while their setting is off |
| `worktree` | What the worktree commands share |
| `facts`, `items` | What GitHub and the landing queue say of each tab; the one shape every view's items take |

The workspace:

| Module | What it is |
|---|---|
| `mux` | The multiplexer: protocol, socket, client, input, panes, layout, keys, drawing, server, commands, persistence |
| `agent` | What each Claude session is doing, from its hooks; one Claude to a tab; `M-a`, `M-c`, `M-f` |
| `tabs` | What each tab is called and the color it wears, from git's files and lookups in the background |
| `status` | The status line's button, health light and second line, and dark and light |
| `ui` | The boxes over the workspace: messages, tips, questions, menus, pickers, the palette |
| `sidebar` | Every tab on the left, most in need of you first, and the model mission control shares |
| `mission` | Mission control: every tab as a card and every view beside them |
| `sources` | What GitHub and Linear say, looked up in the background on a schedule |
| `day` | Your day: the calendar, Slack, the notes written for you |
| `usage` | How the workspace is used, on this computer alone: habits, resources, sessions and spend, closed tabs, your own files' settings, plugins |
| `links` | Opening a link in its site's Chrome profile |
| `views` | Reading beside your work: the PR description, the ticket, a link, a pane's links and images, search everything, whether a PR can merge |
| `diff` | `M-d`'s diff in a pane's place, told as a story, with questions about it answered, a commit or the working tree at a time; lazygit in a pane's place |
| `notes` | The notes app, and Notes in the workspace |
| `autopilot` | Working a note's checklist continuously: a tab opened for each item as the last lands, its line in mission control |
| `relay` | The Slack relay, a program of its own reaching the tabs through the server |
| `setup` | `switchboard setup`: Claude Code's hooks, statusline, permissions, CLAUDE.md and skills |
| `update` | `switchboard update`, and the server's look for a new release (and the pull of your rules' repository beside it) |
| `extras` | The rest of tmux.conf's keys and `M-p`'s commands: `M-0`, `C-w` between panes, renaming the tab, mission control on a view, the habits report, a reply sent to another tab, setup checked, the loop tabs, a PR or ticket by number, the AWS login |
| `cmd` | The command table, and the commands that are programs of their own (the worktree commands, `codex-review`, `land-lock`, `wait-deploy` and the rest) |

`src/lib.rs` lists them with a line each, and every module opens with a comment saying what it is and why it works as it does.

## Where things live

| What | Where |
| --- | --- |
| Your settings and own files | `~/.config/switchboard/`: `settings.json`, `keys.json`, `sidebar.json` and `sidebar-plugins/`, `browser.json`, `linear.json`, `claude.json` and `CLAUDE.md`, `slack-updates.json`, `slack-relay.json`, `oncall.json`, `trusted-dirs`, and `secrets.json` on Linux without a keyring. Never written over by Switchboard but through Settings |
| State | `~/.local/state/switchboard/` (`SWITCHBOARD_STATE_DIR` moves it): the socket (`sock`), `server.lock` and `server.log`, the layout (`layout.json`), tips, habits, requests, connectors, the day's files, Notes' socket, the mode working a note (`autopilot.json`), and the statusline's `context/` and `usage/` |
| The Slack relay's state | `~/.local/state/slack-relay/`, where the Node relay kept it |
| Caches | `~/.cache/switchboard/`: the diff's order and stories, Notes' pictures, the PR and ticket views' last copies |
| Claude Code | `~/.claude/settings.json` (merged into, never linked), `~/.claude/CLAUDE.md`, `~/.claude/skills/` |
| The settings schema, Claude's settings, Switchboard's part of CLAUDE.md, the skills | `assets/`, compiled into the binary (`include_str!`, and `build.rs` for the skills), so the binary needs no clone |

## Tests

Unit tests sit beside the code. Integration tests are in `tests/`, each a binary of its own: `tests/common` starts a server of the test's own in a temporary home under `/tmp` (short, since a Unix socket's path can't be over 104 bytes on macOS), runs commands against it, and attaches a client that reads what the server draws through a terminal emulator, so a test looks at the screen as a person would. Services are stand-ins on 127.0.0.1, reached by moving their address (`SWITCHBOARD_GITHUB_API`, `SWITCHBOARD_GITHUB_GRAPHQL`, `SWITCHBOARD_LINEAR_API`); Slack, Claude and the desktop's tools are stand-ins on the test's PATH. No test reaches the real server, the real home or a real service.

Debug builds keep only line tables, and the crates no debug info, since every integration test links all of them. CI (`.github/workflows/ci.yml`) checks the formatting, runs clippy with warnings as errors (with the `net` feature and without it) and the tests on Linux and an Apple silicon Mac, and builds for Intel Macs.

## Releases

A tag `v<version>` matching `Cargo.toml`'s version builds the three targets (`.github/workflows/release.yml`) with `SWITCHBOARD_RELEASE` set, and publishes each as `switchboard-<version>-<target>.tar.gz` with its SHA-256. `install.sh` installs the newest into `~/.local/bin`; `src/update` finds a newer one, checks it, swaps the binary only once it runs, and runs `switchboard setup`. Only a release build looks by itself (`SWITCHBOARD_UPDATES` says otherwise), so a binary built from a clone is never offered a release in its place. `switchboard --version` names the commit it was built from.

## Conventions

- Each module opens with a comment saying what it is and why it works as it does; a part ported from a Node, shell or Lua file names that file, so its history can be followed.
- Behavior matches the tmux version unless that depended on tmux or macOS; where it changes, the comment says so.
- No tmux, no Node, no external program but Claude Code and git (and the platform's own tools in `src/platform/`). Desktop differences go through `src/platform/`.
- Files written whole go through `util::write_atomic`; JSON numbers through `util::js_number`, so files read as they did.
- Server state is touched only by the server task; anything slow runs elsewhere and reports back with an `Event`.
- Tests: unit tests beside the code; integration tests in `tests/` with a temporary home and a server of their own, never the real one.

## History

Switchboard was a tmux setup until the rewrite: tmux.conf and its shell scripts, a Node daemon for the sidebar and mission control, Node programs in `bin/`, a Neovim config with the diff viewer and the notes app as plugins, and `setup.sh` linking dotfiles for tmux, Neovim, lazygit, Ghostty and Karabiner. The rewrite ported each part in turn and then removed them; each module's opening comment names the files it came from, which git's history still has.

| Before | After |
| --- | --- |
| tmux, tmux.conf, `tmux/*.sh` | `src/mux/` and the parts below; the default keys are tmux.conf's |
| tmux user options (`@agent_state`, `@tab_label`, ...) | typed fields on panes and tabs, plus a string map for the rest |
| `display-popup`, `say.sh`, `tip.sh`, `menu.sh`, `confirm.sh` | overlays in `src/ui/` |
| fzf (palette, pickers) | `src/ui/picker.rs` with nucleo-matcher |
| glow, mermaid-ascii, less | `src/views/` (pulldown-cmark, a Mermaid flowchart and sequence drawer, a pager) |
| diffview.nvim, `diffstory.lua`, `diffask.lua`, `pane-in-place.sh`, `diff-base.sh`, `diff-story-ahead.sh` | `src/diff/` (syntect for highlighting, similar for the line and word diffs) |
| the notes app (`notes/`, Neovim), `bin/notes` | `src/notes/` (ropey for the text, notify for the watch, image for pictures other than PNGs) |
| the sidebar daemon (`bin/tmux-sidebar`, `lib/sidebar/`) | `src/sidebar/`, `src/mission/`, `src/sources/`, `src/day/`, `src/usage/` |
| `bin/slack-relay`, `lib/slack-relay/` | `src/relay/` |
| `gh` | GitHub's API over reqwest; the token from `GH_TOKEN` or `GITHUB_TOKEN`, `gh auth token` when gh is there, else a stored secret |
| `security`, `pbcopy`, `osascript`, `ioreg`, `open` | `src/platform/` |
| tmux-resurrect, tmux-continuum | `src/mux/persist.rs` |
| `setup.sh`, `.githooks`' setup runs, the dotfile links | `switchboard setup` (hooks, statusline, skills) and release downloads |
| the hourly update popup (git pull) | the same box, offering a new release |
| `lib/sidebar/autopilot.js`, `tmux/wtn-open.zsh`'s `queue` and `next` | `src/autopilot/`, `src/cmd/worktree_from_note.rs`'s `queue` |
| `bin/*` Node programs, `lib/` | `src/cmd/`, `src/integrations/`, `src/relay/`, ... |

### Not ported

What the tmux version did that the binary doesn't yet, so a part that brings one back knows where it stood:

- **Commands**: Rename's ticket and shaping shortcuts (tab-name.zsh's `t` and `s`): "Rename the tab" takes a typed name.
- **Mission control and the sidebar**: the Notifications window of its own (a view in mission control now).
- **The sidebar's tools**: its `d` and `L` on a tab (the tab's diff and lazygit in its pane's place), which mission control's menu on a tab offers.
- **Elsewhere**: `switchboard setup` adding the worktree commands' shell functions to your shell's startup file (nothing of the workspace's needs them: its keys and mission control's menus start work as the tab's own command, through the binary).

### Before this lands

Decisions left to the maintainer before the rewrite is merged:

- **The release repository.** `src/update/release.rs`'s `REPO`, `install.sh`'s `REPO` and the line in its opening comment, and README's install line and clone URL all name the repository the rewrite was made in. They change together, to the repository it lands in.
- **The modifier on Linux.** Every key is Alt, as on a Mac with Option as Alt. Whether Linux, where desktops and terminals take some Alt keys for themselves, gets a choice (a `modifier` setting, Alt by default, was proposed) is undecided.
- **Clones of the tmux version.** Their hourly update fast-forwards `main` and runs `./setup.sh`, which is gone, leaving their tmux, Neovim and `bin/` links pointing at removed files. Whether they get a last tmux-version commit saying to install the binary, or something else, is undecided. `switchboard setup` already turns a `~/.claude/settings.json` or `~/.claude/CLAUDE.md` linked into such a clone into files of your own, from the clone's history once the files are gone.
- **The review loop over the branch** (`/review-loop`, Claude reviewing where Codex isn't installed), offered before the PR.
