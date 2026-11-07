# Switchboard

A terminal workspace for working with coding agents, built around Claude Code. Every tab says what its agent is doing and whether it is waiting on you, one key takes you to the next one that needs you, and a ticket goes from idea to merged PR without leaving the keyboard.

Switchboard is its own terminal multiplexer: a server in the background owns every pane, so closing the terminal never stops an agent, and `switchboard` attaches to it again, as tmux did. The sidebar, mission control, the command palette, the PR and ticket views, a diff viewer that tells a change as a story, a notes app and a Slack relay for when you are away are built in. It is one program, `switchboard`.

It is opinionated: the defaults are how it is meant to be used, and Settings (`M-p`, then Settings) turns off the parts you don't want.

## Requirements

- Linux on x86_64 (another architecture builds from source), or macOS on Apple silicon or Intel. On Linux it uses the desktop's keyring, notifications and idle time where there are any, and does without them where there aren't.
- [Claude Code](https://claude.com/claude-code), which the tabs run, and git.
- A terminal with 24-bit color. On a Mac, Option has to act as Alt, since every `M-` key is Alt: in Ghostty, `macos-option-as-alt = true`; in iTerm2, Settings, Profiles, Keys, "Left Option key: Esc+"; in Terminal, Settings, Profiles, Keyboard, "Use Option as Meta key". `switchboard setup` offers to set it in Ghostty, Terminal and iTerm2. Pictures in notes need a terminal that draws kitty's graphics protocol, such as Ghostty or kitty.

Nothing else has to be installed. GitHub, Linear, the calendar, Slack and Codex each turn on a part of it when you have them; the [guide](docs/guide.md) says what each needs.

## Install

```
curl -fsSL https://raw.githubusercontent.com/cooljacob204/switchboard/main/install.sh | sh
```

It downloads the newest release for this computer, refuses it unless it matches the SHA-256 published beside it, puts `switchboard` in `~/.local/bin` (saying how to add that to your PATH when it isn't there), and runs `switchboard setup`, which asks before it changes anything.

Or download a release yourself: each one on the repository's Releases page has `switchboard-<version>-<target>.tar.gz` for `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin` and `x86_64-apple-darwin`, with its `.sha256`. Unpack it, put `switchboard` on your PATH, and run `switchboard setup`.

Or build it from a clone, with [rustup](https://rustup.rs) and a C compiler (the Rust it needs is pinned in `rust-toolchain.toml`, which rustup installs):

```
git clone https://github.com/cooljacob204/switchboard.git
cd switchboard
cargo build --release
install -m 755 target/release/switchboard ~/.local/bin/
switchboard setup
```

Then start it:

```
switchboard
```

`switchboard setup` puts in place what Claude Code needs for the workspace: the hooks that tell each tab what its Claude is doing, the statusline, the permissions its skills run with, a `~/.claude/CLAUDE.md` that imports Switchboard's part beside your own rules, and the skills that ship with it (`/wt`, `/shape`, `/review-loop` and the rest). It merges into your own `~/.claude/settings.json` rather than replacing it, keeps a copy of the file before, and keeps a skill of your own by the same name. It also offers to make Option act as Alt in a Mac's terminal, to tidy the links and `~/.zshrc` lines a tmux version of Switchboard left, and says which optional parts are missing (the GitHub and Linear sign-ins, Chrome, ffmpeg, Codex and the rest), each with what it is for. Run again, it changes only what isn't in place; `switchboard setup --check` only says what would change.

## First keys

`M` is Alt (Option on a Mac), and a capital letter is with Shift: `M-N` is Alt+Shift+n.

| Key | What it does |
|---|---|
| `M-p` | Find any command by what it does, Settings included; the ⌘ Commands button in the status line opens it too |
| `M-a` | Go to the next tab that needs you: pinned, then waiting on you, then finished, then the notifications |
| `M-Space` | Mission control: every tab as a card, the Work map, PRs, CI, tickets, your day, Settings |
| `M-t` | New tab running Claude, in a directory you pick |
| `M-j` | Find a tab by typing its name |
| `M-F` | Search your notes, Linear, GitHub and Slack at once |
| `M-s` | Into the sidebar and back out |
| `M-c` | Into the pane running Claude in this tab |
| `M-f` | Pin the tab to the top of Needs you |
| `M-Left` `M-Down` `M-Up` `M-Right` | To the pane on that side |
| `M-H` `M-J` `M-K` `M-L` | Split the pane to that side |
| `M-[` `M-]`, `M-1` … `M-9` | The previous or next tab, or a tab by number |
| `M-w` | Close the pane, asking first when something runs in it |
| `M-v` / `M-i` | The branch's PR description / its Linear ticket, in a pane to the right |
| `M-d` | The branch's diff against its PR's base, in place of the pane, its changes told as a story |
| `M-N` / `M-e` | Notes in its own tab / beside the pane you are in |
| `M-b d` | Detach, leaving everything running; `switchboard` attaches again |

`M-b` is the prefix for the rest, and `M-b ?` lists every key. Tips in the top-right corner teach the others the first time each would help. Menus and questions take a click too, and a click outside a box closes it.

Vim's keys are the default in Notes and the diff, and you don't need to know them: Notes has a toolbar over each note, and Settings' Easy mode lets Shift-arrows and the mouse select, typing replace a selection, and gives the diff a row of buttons for its keys.

## Staying up to date

A release keeps itself current: each time you attach, and hourly after that, the server looks for a newer release and offers it in a box, which downloads it, checks it, puts it in place of the binary and runs `switchboard setup` for what it brings. `switchboard update` does the same from a shell, and `M-p`'s "Update Switchboard" any time. A binary you built is left to you: `git pull`, `cargo build --release` and `switchboard setup` again. The skills, hooks and Switchboard's part of CLAUDE.md are compiled into the binary, so the setup is what takes a new build's to `~/.claude`; the first time a new build attaches, it brings the parts you have set up up to date by itself, and `switchboard setup` says what it did.

## Making it yours

Everything of your own is in `~/.config/switchboard/`, which nothing here writes over, so an update leaves it be:

| File | What it holds |
|---|---|
| `settings.json` | What you changed in Settings, only the settings that differ from their defaults; you may edit it by hand |
| `keys.json` | Keys bound to other actions, or unbound |
| `CLAUDE.md` | Your own Claude rules, imported with Switchboard's part into `~/.claude/CLAUDE.md` |
| `claude.json` | Where your rules come from instead: another file, or a git repository of yours |
| `browser.json` | The Chrome profile each site's links open in, and the default one |
| `linear.json` | Your Linear workspace, and the team and labels `/shape` files tickets with |
| `slack-updates.json` | The Slack channels the Slack view shows in full |
| `slack-relay.json` | Your Slack member ID, the one person the relay listens to |
| `oncall.json`, `oncall-rules.md` | The channels `/oncall` watches, and your team's rules |
| `sidebar.json`, `sidebar-plugins/` | The sidebar's and mission control's sizes, and the plugins that add views |
| `trusted-dirs` | The directories whose own scripts may run |

Settings lists each setting in these files under the file's name, and Enter changes it there. What Switchboard keeps for itself (the socket, the saved layout, tips shown, what it counted) is in `~/.local/state/switchboard/`, and its caches in `~/.cache/switchboard/`.

## More

[docs/guide.md](docs/guide.md) describes every feature, key and command. [ARCHITECTURE.md](ARCHITECTURE.md) describes the code, and [CLAUDE.md](CLAUDE.md) how it is worked on.
