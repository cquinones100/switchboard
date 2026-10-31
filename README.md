# Switchboard

A terminal workspace for working with coding agents: tmux, Neovim, lazygit and a set of scripts, built around Claude Code. Every tab says what its agent is doing and whether it is waiting on you, one key takes you to the next one that needs you, and a ticket goes from idea to merged PR without leaving the keyboard.

It is opinionated: the defaults are how it is meant to be used, and Settings (`M-p`, then Settings) turns off the parts you don't want.

## Requirements

- macOS, with zsh as your shell. Linux isn't supported yet: the scripts use pbcopy, osascript, open and the Keychain, and `setup.sh` stops at its first check there.
- [Homebrew](https://brew.sh)
- A terminal that passes Option through as Alt and shows 24-bit color: [Ghostty](https://ghostty.org) (`brew install --cask ghostty`; setup links its config) or iTerm2. `setup.sh` makes Option act as Alt in Ghostty (a config of your own too), iTerm2 and Terminal, after asking, and says what to add for WezTerm, kitty or Alacritty; Terminal.app still has no 24-bit color.
- [Claude Code](https://claude.com/claude-code), which every tab runs: `curl -fsSL https://claude.ai/install.sh | bash`

## Set up

```
curl -fsSL https://raw.githubusercontent.com/cquinones100/switchboard/main/install.sh | bash
```

It clones `main` into `~/switchboard`, the same branch updates come from, and runs the guided `setup.sh` there. Or do the same by hand:

```
git clone https://github.com/cquinones100/switchboard.git ~/switchboard
~/switchboard/setup.sh
```

`setup.sh` checks what the workspace needs and asks before it installs anything or changes `~/.zshrc`: Homebrew's `tmux jq fzf gh glow lazygit neovim git-delta ripgrep`, Node through nvm, the MesloLGS NF font, the links for the tmux, Neovim and lazygit configs and the scripts in `bin/`, the worktree shell functions, Ghostty's config, and Neovim's plugins. Then it asks about the optional parts (Claude Code's hooks for tab states, the skills that ship with Switchboard such as `/wt`, `/shape` and `/review-loop`, this repo's whole Claude Code setup, the git hooks) and lists what GitHub, Linear or Jira, the Slack relay, the PR image view (mermaid-cli), `/deploy-test` (ffmpeg) and `/review-loop` (Codex, else Claude reviews) need. It ends with a list of anything left to do; run it again until it says Ready. Running it again any time changes only what isn't in place, and `./setup.sh --check` only reports.

Then start tmux:

```
tmux new-session -A -s main
```

## First keys

| Key | What it does |
|---|---|
| `M-p` | Find any command by what it does, Settings included; what you picked after typing the same words comes first |
| `M-a` | Go to the next tab that needs you, one waiting on you before one that finished |
| `M-j` | Find a tab by typing its name |
| `M-S-f` | Search your notes, tickets, GitHub and Slack at once |
| `M-Space` | Mission control: every tab, the Work map, PRs, Settings |
| `M-t` | New tab running Claude, in a directory you pick |
| `M-s` | Into the sidebar and back out |
| `M-c` | Into the pane running Claude in this tab |
| `M-f` | Pin the tab to the top of Needs you |
| `M-v` / `M-d` | The branch's PR description / its diff, as GitHub shows them |

`M` is Option. Tips in the top-right corner teach the rest the first time each would help.

## Staying up to date

Each time tmux opens, and hourly after that, it checks GitHub for new commits and offers them in a popup that updates the clone and runs `setup.sh`. With the git hooks on, a `git pull` links whatever it added too. `M-p`'s "Update Switchboard" runs the setup even when there is nothing new, so a clone you pulled by hand gets set up too. To run the setup again yourself, say to set up something you turned down the first time, use `M-p`, "Run Switchboard's setup". "Check Switchboard's setup" says what isn't in place without changing anything (`./setup.sh --check`), and "Run a part of Switchboard's setup" runs or checks one part (`./setup.sh <part>`), picked from a list.

## Making it yours

Your own settings go in `~/.config/switchboard/`, which is never in this repository, so updates leave them be. Each file is read after Switchboard's own, so what it sets wins:

| File | Read by |
|---|---|
| `tmux.conf` | tmux, at the end of its config |
| `nvim.lua` | Neovim, at the end of its config |
| `ghostty` | Ghostty, after its config |
| `lazygit.yml` | lazygit, over its config, in panes tmux starts |
| `CLAUDE.md` | Claude Code, your own rules, imported with Switchboard's part into `~/.claude/CLAUDE.md` |
| `claude.json` | where your CLAUDE.md's rules come from, a file or a git repository |
| `browser.json` | the Chrome profile each site's links open in, `awsapps.com` for the AWS login |
| `linear.json` | your Linear workspace, the team and labels `/shape` files tickets with |
| `jira.json` | your Jira site, the email your API token belongs to, and the project and labels `/shape` files tickets with, with Jira as the tracker |
| `slack-updates.json` | the channels the Slack view shows in full |
| `slack-relay.json` | your Slack member ID, the one person the relay listens to |
| `oncall.json`, `oncall-rules.md` | the channels `/oncall` watches and your team's rules |
| `sidebar.json`, `sidebar-plugins/` | the sidebar's and mission control's sizes, and the plugins that load |
| `trusted-dirs` | the directories whose own scripts may run |

Claude Code reads no file after its own, so `setup.sh` adds what Switchboard needs (its hooks, statusline and the permissions its loops run with) to your `~/.claude/settings.json` and leaves the rest of it yours. Your `~/.claude/CLAUDE.md` imports Switchboard's part (`claude/switchboard.md`: the tabs, the notes app) and your own rules, from `~/.config/switchboard/CLAUDE.md`, or from where `~/.config/switchboard/claude.json` says: a file (`{"path": "~/somewhere/CLAUDE.md"}`) or a git repository of yours (`{"repo": "<url>", "file": "CLAUDE.md"}`, its CLAUDE.md at the root or in a folder found without `file`, cloned, signing in through `gh` when it is on GitHub or a gist, and pulled each hour with Switchboard's update check and on every setup run), so you can keep your rules in a repository of your own and share them across machines. Settings' "Your CLAUDE.md" sets it for you, and `./setup.sh claude_md` writes it again.

Settings (`M-p`) keeps what you change there in the same folder, as `settings.json`, and lists every setting in the files above too, under each file's name: Enter types a value in, or opens a whole file in Neovim. Setup moves these files here from where they were kept before (`~/.config/worktree-from-ticket`, `slack-relay`, `slack-updates`, `oncall`, `tmux-sidebar`, `aws-login`).

## Where things are

| Directory | What is in it |
|---|---|
| `tmux/` | `tmux.conf`, the tab states, sidebar launcher, palette, settings and tips |
| `lib/sidebar/`, `bin/tmux-sidebar` | The sidebar and mission control |
| `bin/` | The scripts: worktrees (`wt`, `wtpr`, `wtb`, `wtr`), the notes app, the Slack relay, and more; see [bin/README.md](bin/README.md) |
| `nvim/`, `notes/` | Neovim's config, and the notes app built on it |
| `claude/` | Claude Code's settings, hooks, statusline and skills |
| `lazygit/`, `ghostty/`, `karabiner/`, `pgcli/` | Those tools' configs |
| `test/` | `node --test test/*.test.js`, against a throwaway tmux and a temporary home |

[bin/README.md](bin/README.md) describes every script and feature in full.
