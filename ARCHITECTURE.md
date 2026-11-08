# Architecture

How Switchboard is built: what the binary is, its parts, where each part keeps its state, how it is tested, and where it came from. [README.md](README.md) says what it is for someone using it, [docs/guide.md](docs/guide.md) what each feature does, and [CLAUDE.md](CLAUDE.md) the rules for working on it.

## What it is

- **One binary, `switchboard`.** `switchboard` alone starts the workspace or attaches to it; `switchboard <command>` runs everything else (`switchboard help` lists them). A link to the binary named after a command runs that command too. Claude Code and git are the only programs it expects; anything else (an editor, lazygit, a shell of your choice) runs in a pane like any program.
- **Its own terminal multiplexer, with a server.** A background server owns every pane's terminal, so closing the terminal never stops a running agent; clients attach and detach as with tmux. The layout is saved as it changes and restored after a reboot, each Claude pane resumed with `claude --resume`.
- **Everything built in**: the sidebar and mission control, the palette, menus, messages and tips, the status line, the Markdown views (PR description, ticket, link view), the diff viewer with its story and questions, the notes app, the Slack relay.
- **No dotfiles.** `switchboard setup` puts in place what Claude Code needs and nothing else: Switchboard's hooks, instructions and skills as a Claude Code plugin that only the Claude in its panes loads (or, with Settings' "Load Switchboard as a plugin" off, in `~/.claude` for every Claude), and a statusline and permissions merged into your Claude settings.
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
- **Upgrading in place** (`src/mux/upgrade.rs`): `switchboard upgrade` (and `M-p`'s action, and `switchboard update` once a release is in place) restarts the server on the binary at the path it started from, without ending anything. `M-p`'s action first builds the checkout a build from a clone was built in (`src/mux/latest.rs`, from build.rs's `SWITCHBOARD_CHECKOUT` and `SWITCHBOARD_TARGET_DIR`, empty in a release build), puts the result in place of the server's binary when that is a copy, and restarts only when the file at that path is no longer the one the server started from (its device, inode, size and modified time). The new binary is first asked `server --handover-version`, and only one answering the handover's version is exec'd. The server stops taking connections (they wait in the socket's queue), has every connection stop reading, pauses every pane between two reads (`src/mux/pty.rs`), answers the commands under way, has each connection write what is queued for it and give its socket back, then writes `handover.json` in the state directory (the saved layout's shape, plus each pane's master descriptor, pid, size, unwritten input and screen as the bytes that draw it again, `src/mux/replay.rs`, a frame the program was drawing included and what the emulator held of a sequence not yet whole; each terminal's connection by descriptor with its session, size, the frame bytes in flight and the keys its parser held (a paste not yet ended); the listening socket and the lock; each pane's Claude's state; the connection of the `switchboard upgrade` that asked) and `execv`s `switchboard server --handover <file>` with those descriptors left open. The process and its pid stay the same, so the panes' programs stay its children and never see their terminal close. The new build takes each pane up (`Pane::adopt`) instead of starting one, rebuilds the sessions and tabs, tells each program its size again (SIGWINCH), serves each connection again from the frame it was at and redraws its screen whole, and answers the asker. Anything failing before the exec, or the exec itself, resumes everything as it was. What isn't carried: the overlays open (menus, mission control, copy mode, a prefix key pressed), a pane's scroll region, tab stops and character sets (a full-screen program redraws on the SIGWINCH), what background lookups were under way (asked again), and the processes they had started, left unreaped until the server ends.
- **Panes** (`src/mux/pane.rs`): a PTY and a terminal emulator (alacritty_terminal) each. portable-pty opens the PTY and starts the program; from then on the pane holds the master's descriptor and the program's pid (`src/mux/pty.rs`), with a thread reading it and one writing what is typed, both waiting with `select` beside a pipe that pauses them between two reads or writes, so a pane can also be taken up from a descriptor and a pid. The emulator's replies to programs' queries go back to the PTY, with what the attached terminal says of itself (`src/mux/terminal.rs`): its cell's size in pixels, which the client sends with its size and each PTY is given, and its colors, asked as it attaches (OSC 10, 11 and 4) and told to a program that asks the same; OSC 52 goes to the clients' clipboard. Every pane's environment carries `SWITCHBOARD` (the socket) and `SWITCHBOARD_PANE` (its id), which the hooks and commands use as tmux's `TMUX` and `TMUX_PANE` were used, `GH_BROWSER` (`switchboard open-url`), so gh's `--web` opens in the Chrome profile `browser.json` gives the site, and, while `claude_plugin` is on, `CLAUDE_CODE_PLUGIN_DIRS` naming Switchboard's plugin (`src/setup/plugin.rs`), so Claude run there loads its hooks, skills and instructions; a headless `claude -p` Switchboard starts for itself leaves it out (`plugin::headless`). `src/mux/passthrough.rs` lets the notes pane alone send kitty's graphics commands on to the terminal and take easy mode's Cmd keys.
- **Layout** (`src/mux/layout.rs`): each tab is a tree of splits. Overlays sit above it: the boxes of `src/ui/` and mission control (what tmux's popups were).
- **Drawing** (`src/mux/compose.rs`): per client, a ratatui `Buffer` is filled with the tab (sidebar, panes, borders, status line, or with UX 2.0's `ux2` setting the tab's header from `src/mux/header.rs` in its place) and any overlay, and only the cells that changed are sent, each a pane's program made part of a hyperlink (OSC 8) inside it, kept beside the buffer since a cell can't carry one. The built-in screens draw into that buffer.
- **Input** (`src/mux/input.rs`, `src/mux/keys.rs`): the client sends raw bytes; the server parses them into keys, mouse and paste, each with its raw bytes, matches bindings (the defaults are what tmux.conf bound, under the same keys; `~/.config/switchboard/keys.json` changes them), and passes anything unbound to the focused pane, encoded again for it: mouse events for its position and mode, and keys in the form its program asked for. As tmux's `extended-keys on` did, the client asks the terminal for keys told apart (the kitty keyboard protocol's flag 1 and modifyOtherKeys level 2, `extended_keys` setting), which the parser reads in either form, and a key reaches a pane as CSI u only when its program turned on the kitty protocol (which alacritty_terminal keeps) or modifyOtherKeys (watched for in its output, `src/mux/extkeys.rs`), its old bytes otherwise: Ctrl-Shift-P reaches Notes as itself and a shell as Ctrl-P.
- **Client** (`src/mux/client.rs`): raw mode, the alternate screen, mouse and paste reporting on, keys told apart where the terminal can; stdin to the socket, the socket to stdout; resizes sent on SIGWINCH, with the window's size in pixels; the terminal put back however it ends. It refuses to attach inside a pane of its own server. On the server's side each attached connection is one task reading into a buffer of its own and writing from another (`serve_client`), so it can stop between two frames and be handed to a new build; the client never knows.
- **Apps in panes**: what ran in a pane under tmux (glow and less for a PR or ticket, diffview in Neovim, the notes app in Neovim) is a program of Switchboard's own run in a pane the same way: `switchboard pr-view`, `ticket-view`, `link-view`, `view`, `diff`, `notes`, full-screen programs drawn with ratatui and crossterm. The server stays a multiplexer, opening them in a split or in a pane's place (the pane parked until the program ends), and each app is built and tested on its own.
- **Commands** (`src/mux/commands.rs`): what scripts and skills used tmux for (list the tabs, open a tab, send keys to a pane, read its screen, rename a tab, set an option, say something) is `switchboard <command>` sent over the socket and run on the server's own state.
- **Agent state** (`src/agent/`): Claude Code's hooks, from Switchboard's plugin (or `~/.claude/settings.json` the old way), run `switchboard hook <event>`, which sends the hook's JSON and `SWITCHBOARD_PANE` to the server, which marks the pane working, waiting or done and rolls that up into its tab.
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
| `platform` | What differs between macOS and Linux: secrets, clipboard, notifications, presence, opening a file or link, and the sandbox a sealed try runs in (`sandbox`: its rules, as macOS's `sandbox-exec` reads them; Linux has none) |
| `presence` | Whether you're at the computer |
| `connector` | One call to a Claude connector through a headless Haiku |
| `integrations` | GitHub and Linear over their APIs, refused while their setting is off but in a program you run yourself (`codex-review`, `wt`, `wtpr`, ...); a branch's PR looked for in the repository `gh repo set-default` names, else in origin and, in a fork's clone, the project its `upstream` remote names, in one query, origin's winning |
| `worktree` | What the worktree commands share |
| `trust` | Whose code a directory holds, the one place that decides: the directories listed in `trusted-dirs`, a worktree whose code is your own (what `switchboard try` builds), and someone else's PR checked out untrusted, which nothing starts Claude in, lists to reopen or builds |
| `facts`, `items` | What GitHub and the landing queue say of each tab; the one shape every view's items take |
| `repos` | Your settings for each repository, `repos.json`, by its origin's `owner/name`: how its work lands (through a PR or straight on its default branch, worked out from GitHub's rules and its CLAUDE.md when not set) and what a landed change needs |

The workspace:

| Module | What it is |
|---|---|
| `mux` | The multiplexer: protocol, socket, client, input, panes, layout, keys, drawing, server, commands, persistence |
| `agent` | What each Claude session is doing, from its hooks; one Claude to a tab; `M-a`, `M-A`, `M-c`, `M-y`, `M-f` |
| `tabs` | What each tab is called and the color it wears, from git's files and lookups in the background |
| `status` | The status line's button, health light and second line, and dark and light; the rows the status line and UX 2.0's tab header take (`lines`, `top`) |
| `ui` | The boxes over the workspace: messages, tips, questions, menus, pickers, the palette |
| `sidebar` | Every tab on the left, most in need of you first, the Notifications window, and the model mission control shares |
| `mission` | Mission control: every tab as a card and every view beside them |
| `sources` | What GitHub and Linear say, looked up in the background on a schedule |
| `day` | Your day: the calendar, Slack, the notes written for you |
| `usage` | How the workspace is used, on this computer alone: habits, resources, sessions and spend, closed tabs, your own files' settings, plugins |
| `links` | Opening a link in its site's Chrome profile |
| `views` | Reading beside your work: the PR description, the ticket, a link, a pane's links and images, search everything, whether a PR can merge, and the branch's merge queue, Linear project, ticket and PR opened or copied |
| `diff` | `M-d`'s diff in a pane's place, from the merge base to the working tree, told as a story of its commits with what isn't committed in a last chapter, with questions about it answered, a commit or the working tree at a time; lazygit in a pane's place |
| `notes` | The notes app, and Notes in the workspace |
| `autopilot` | Working a note's checklist continuously: a tab opened for each item as the last lands, its line in mission control |
| `relay` | The Slack relay, a program of its own reaching the tabs through the server |
| `setup` | `switchboard setup`: Switchboard's plugin for Claude Code (its hooks, skills and instructions), or the same in `~/.claude` the old way, and which way (`way`); Claude Code's statusline and permissions; your CLAUDE.md; Option as Alt in a Mac's terminal; the tmux version's leftovers tidied; the optional parts reported; a new build's parts brought up to date |
| `update` | `switchboard update`, and the server's look for a new release (and the pull of your rules' repository beside it) |
| `extras` | The rest of tmux.conf's keys and `M-p`'s commands: `M-0`, `C-w` between panes, renaming the tab and naming a new one, mission control on a view, the habits report, a reply sent to another tab, setup checked, the loop tabs, a PR or ticket by number, the AWS login |
| `cmd` | The command table, and the commands that are programs of their own (the worktree commands, `codex-review`, `pr-risk`, `land-lock`, `apply-actions`, `wait-deploy` and the rest) |

`src/lib.rs` lists them with a line each, and every module opens with a comment saying what it is and why it works as it does.

## Where things live

| What | Where |
| --- | --- |
| Your settings and own files | `~/.config/switchboard/`: `settings.json`, `keys.json`, `sidebar.json` and `sidebar-plugins/`, `browser.json`, `linear.json`, `claude.json` and `CLAUDE.md`, `slack-updates.json`, `slack-relay.json`, `oncall.json` and `oncall-rules.md`, `repos.json`, `leak-check.txt`, `trusted-dirs`, and `secrets.json` on Linux without a keyring. Never written over by Switchboard but through Settings |
| State | `~/.local/state/switchboard/` (`SWITCHBOARD_STATE_DIR` moves it): the socket (`sock`), `server.lock` and `server.log`, the layout (`layout.json`), the handover to a new build during `switchboard upgrade` (`handover.json`, removed once read), tips, habits, requests, connectors, the day's files, Notes' socket, autopilot (`autopilot.json`), how each repository lands as last worked out (`repos-landing.json`), the items `wtn` started lately (`wtn-claims.tsv`), the directories and "Land on:" branches the work pickers last had picked, and the repository each note's items were last started in (`work-recent.json`), setup's note that an earlier install kept in `~/.claude` hasn't been asked about the plugin yet (`claude-plugin-unasked`), and the statusline's `context/` and `usage/` |
| The Slack relay's state | `~/.local/state/slack-relay/`, where the Node relay kept it |
| Caches | `~/.cache/switchboard/`: the diff's order and stories, Notes' pictures, the PR and ticket views' last copies, what the PR and ticket image views load and draw (`assets/`, `pr-image/pages/`, held to 300 MB together), the PRs and ticket titles the tabs show (`tab-names.json`), the worktrees of someone else's PRs `wtpr --untrusted` checks out (`pr-review/<owner>-<repo>/<number>`), a sealed try's sandbox rules, which also mark its home as sealed, and the crates sealed tries fetch (`sealed/`); `~/.cache/wt-revive/index.json`, what each Claude transcript says of its worktree |
| What setup writes for Claude Code | `~/.local/share/switchboard/`: `plugin/` (Switchboard's plugin: `.claude-plugin/plugin.json`, `hooks/hooks.json` made from `assets/claude-settings.json`'s hooks, `skills/`; written by setup and by the server as it starts, unless it has a state directory of its own), `switchboard.md` (Switchboard's part of CLAUDE.md, imported from `~/.claude/CLAUDE.md` the old way), `claude-md/` (the clone of your rules' repository), `setup-build` (the build whose parts were last put in place) |
| `/oncall`'s state | `~/.local/state/oncall/state.md`, kept by the skill |
| Claude Code | `~/.claude/settings.json` (merged into, never linked: the statusline and permissions, and the hooks the old way), `~/.claude/CLAUDE.md` (your rules, and Switchboard's part the old way), `~/.claude/skills/` (Switchboard's the old way); the panes' `CLAUDE_CODE_PLUGIN_DIRS` names the plugin as a plugin |
| The settings schema, Claude's settings, Switchboard's part of CLAUDE.md, the skills | `assets/`, compiled into the binary (`include_str!`, and `build.rs` for the skills), so the binary needs no clone |

## Tests

Unit tests sit beside the code. Integration tests are in `tests/`, each a binary of its own: `tests/common` starts a server of the test's own in a temporary home under `/tmp` (short, since a Unix socket's path can't be over 104 bytes on macOS), runs commands against it, and attaches a client that reads what the server draws through a terminal emulator, so a test looks at the screen as a person would. Services are stand-ins on 127.0.0.1, reached by moving their address (`SWITCHBOARD_GITHUB_API`, `SWITCHBOARD_GITHUB_GRAPHQL`, `SWITCHBOARD_LINEAR_API`); Slack, Claude and the desktop's tools are stand-ins on the test's PATH. No test reaches the real server, the real home or a real service. Each stand-in is as strict as what it stands in for, so a test passes only when the code calls it rightly: `tests/common/stand_in.rs` writes the programs (Claude Code with its options as commander reads them and its prompt on stdin, `gh`, a release of `switchboard` that answers only to that name, the clipboard and opener tools, `security`, `uname` and `sysctl`, `cargo`, lazygit), each refusing a call the real one refuses with its words and exit status before it runs the test's answer, and `tests/common/http.rs` the services (GitHub's REST and GraphQL, Linear, Slack), each refusing the wrong method, an unknown path or a missing or wrong token as the real one answers it. Each writes down what it refused, and a test whose stand-ins refused anything fails, however quietly the code took the failure. A test server starts with stand-ins for what its test gave none of (a Claude Code and a gh that aren't signed in, the desktop's clipboard, openers and Chrome), so no test reaches the computer's own. The library's unit tests include both (`crate::stand_in`, `crate::fake_http`), and `tests/stand_ins.rs` shows each refusing a wrong call.

Debug builds keep only line tables, and the crates no debug info, since every integration test links all of them. CI (`.github/workflows/ci.yml`) checks the formatting, runs clippy with warnings as errors (with the `net` feature and without it) and the tests on Linux and an Apple silicon Mac, and builds for Intel Macs.

## Releases

A tag `v<version>` matching `Cargo.toml`'s version builds the three targets (`.github/workflows/release.yml`) with `SWITCHBOARD_RELEASE` set, and publishes each as `switchboard-<version>-<target>.tar.gz` with its SHA-256. `install.sh` installs the newest into `~/.local/bin`; `src/update` finds a newer one, checks it, swaps the binary only once it runs, runs `switchboard setup`, and offers a running workspace a restart on it in place (`upgrade_after_update`). Only a release build looks by itself (`SWITCHBOARD_UPDATES` says otherwise), so a binary built from a clone is never offered a release in its place, nor given one by `M-p`'s "Update Switchboard" without `--force`. What the setup writes is compiled in, so the first run of a new build (`switchboard` attaching, `setup::refresh`) writes the plugin (which the server also writes as it starts) and brings the parts already set up up to date, once per build (`~/.local/share/switchboard/setup-build`), moving nothing between the plugin and `~/.claude`, where `.githooks`' post-merge once ran setup.sh. `switchboard --version` names the commit it was built from.

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
| a note's stable ID (`notes/lua/notes/id.lua`, `lib/note-id.js`) | `src/notes/id.rs`, which the app, `wtn` and autopilot share |
| the sidebar daemon (`bin/tmux-sidebar`, `lib/sidebar/`) | `src/sidebar/`, `src/mission/`, `src/sources/`, `src/day/`, `src/usage/` |
| `bin/slack-relay`, `lib/slack-relay/` | `src/relay/` |
| `gh` | GitHub's API over reqwest; the token from `GH_TOKEN` or `GITHUB_TOKEN`, `gh auth token` when gh is there, else a stored secret |
| `security`, `pbcopy`, `osascript`, `ioreg`, `open` | `src/platform/` |
| tmux-resurrect, tmux-continuum | `src/mux/persist.rs` |
| `setup.sh`, `.githooks`' setup runs, the dotfile links | `switchboard setup` (the plugin, or hooks and skills in `~/.claude`; the statusline, the terminal, the leftovers, the optional report), a new build's first run, and release downloads |
| the hourly update popup (git pull) | the same box, offering a new release |
| `lib/sidebar/autopilot.js`, `tmux/wtn-open.zsh`'s `queue` and `next` | `src/autopilot/`, `src/cmd/worktree_from_note.rs`'s `queue` |
| `bin/switchboard-try` (a tmux server under `/tmp/sb-<hash>`; `--sealed`'s under macOS's sandbox) | `switchboard try` (`src/cmd/try_.rs`: the build under that home, its server with no client for `--detached`; `src/cmd/try_sealed.rs`: `--sealed`, the build, server and clients in the sandbox) |
| `bin/*` Node programs, `lib/` | `src/cmd/`, `src/integrations/`, `src/relay/`, ... |

### Not ported

What the tmux version did that the binary doesn't yet, so a part that brings one back knows where it stood:

- **The diff**: diffview read the change again by itself when Neovim got the focus back and on a save, as well as on `R`; `M-d`'s diff reads it again on `R` alone (`src/diff/app.rs`'s `refresh`).
- **Commands**: Rename's ticket and shaping shortcuts (tab-name.zsh's `t` and `s`): "Rename the tab" takes a typed name, and the shortcuts are in the name asked for after `M-t` alone (`src/extras/naming.rs`).
- **Sealed tries**: on Linux, which has no sandbox without installing another program (bubblewrap, firejail), `--sealed` refuses. On macOS, the try attaches in the terminal it was run from, or `--print`'s command in another, where the script opened a Ghostty window; `--npm-ci` for lib's packages is `--fetch` for the crates.
- **Elsewhere**: `switchboard setup` adding the worktree commands' shell functions to your shell's startup file (nothing of the workspace's needs them: its keys and mission control's menus start work as the tab's own command, through the binary).

### Before this lands

Decisions left to the maintainer before the rewrite is merged:

- **The first release here.** `src/update/release.rs`'s `REPO`, `install.sh` and the README name this repository, which has no release of the Rust version yet, so the installer and `switchboard update` find nothing until a `v*` tag publishes one.
- **The review loop over the branch** (`/review-loop`, Claude reviewing where Codex isn't installed), offered before the PR.
