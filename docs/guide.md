# Switchboard's guide

Every feature, key and command of Switchboard, as the `switchboard` binary has them. [README.md](../README.md) is the place to start: what Switchboard is, installing it and the first keys. `M` in a key is Alt (Option on a Mac), and a capital letter after it is with Shift: `M-N` is Alt+Shift+n.

## Contents

- [The workspace](#the-workspace)
- [Keys](#keys)
- [Tabs and the status line](#tabs-and-the-status-line)
- [The palette, messages, tips and menus](#the-palette-messages-tips-and-menus)
- [The sidebar](#the-sidebar)
- [Mission control](#mission-control)
- [Your day](#your-day)
- [How you use it](#how-you-use-it)
- [Reading beside your work](#reading-beside-your-work)
- [The diff](#the-diff)
- [The notes app](#the-notes-app)
- [Setting Claude Code up](#setting-claude-code-up)
- [Worktrees](#worktrees)
- [Landing and reviewing](#landing-and-reviewing)
- [The Slack relay](#the-slack-relay)
- [Staying up to date](#staying-up-to-date)
- [Your own files](#your-own-files)
- [Every setting](#every-setting)
- [Commands for scripts](#commands-for-scripts)

## The workspace

`switchboard` alone starts the workspace, or attaches to it when it is already running. It needs a terminal, and it refuses to run inside one of its own panes ("already inside Switchboard"), since a workspace drawn inside itself feeds on itself.

### The server

The first `switchboard` starts a server in the background, in a session of its own, and attaches your terminal to it. The server owns every pane: each pane's program runs on a terminal the server keeps, so closing the window, or losing an SSH connection, leaves every program running, Claude included. The next `switchboard` attaches to the same server and finds everything as it was. The server listens on `~/.local/state/switchboard/sock`, which only you can reach, and writes what it has to say to `~/.local/state/switchboard/server.log`, which is where to look when it doesn't come up. Only one server runs at a time.

`M-b d` detaches, leaving everything running. `switchboard detach` does the same from a script, for every attached terminal or, with `-s <name>`, those on one session. `switchboard kill-server` ends the server and every program in its panes, after saving the layout. A server also ends by itself once its last tab closes.

### Sessions and clients

A session is a set of tabs. `switchboard` attaches to the first session there is, or makes one named `main`; `switchboard -s <name>` attaches to the session by that name, making it, with one tab, when there is none. A new session's first tab runs your login shell in your home directory.

Several terminals can attach at once. Terminals on the same session show the same tab, and the tabs are sized to whichever of them you used last. `M-a`, the sidebar and mission control can take you to a tab in another session, and your terminal moves to that session. When a session's last tab closes, its terminals move to another session, or end when there is none.

### What a pane carries

A pane runs your login shell (`$SHELL`) unless something asked for another program. Its environment carries `SWITCHBOARD`, the server's socket, and `SWITCHBOARD_PANE`, the pane's id (`%12`), which is how `switchboard <command>` and Claude Code's hooks know which server and which pane they run in. `TERM` is `xterm-256color` and `COLORTERM` is `truecolor`. A program that asks its window's size in pixels is told your terminal's, as its cell's size times the pane's (where your terminal says its own; 8 by 16 pixels a cell where it doesn't), so pictures in Notes and the PR as an image are drawn at their own size. A pane's id stays the same for as long as the pane exists, across restarts too, so a pane named in a note or a script is the same pane after one.

A pane closes when its program ends. The colors a program asks for by name are your terminal's own, so your terminal's theme applies inside every pane; the panes without the focus are drawn on a slightly different background, and the lines around the focused pane take its tab's color.

### Saved and restored

The server saves the layout as it changes: every session with its tabs in order, each tab's name, splits, zoom and options (a reminder, a pin, a note), and each pane's directory, the program in front of it, and the Claude session it runs. It goes to `~/.local/state/switchboard/layout.json`, which only you can read, at most every ten seconds, and again when the server ends; the save before it is kept as `layout.json.1`. `switchboard save` writes it now and prints where.

When the server next starts with nothing open, as after a reboot, it brings the last save back: every session, tab and pane, with the same ids. Each pane starts your shell in its saved directory, `$HOME` when that is gone. A pane that ran Claude has `claude --resume <session>` typed into that shell, so the same conversation carries on and a shell is there when it ends. A pane that ran `nvim`, `vim`, `vi`, `less`, `man`, `htop` or `top` has that program typed again; any other comes back as its shell, since running a command a second time, a deploy or a migration, can do harm. Settings' "Restore on start" (`auto_restore`) turns this off. A save that can't be read is set aside as `layout.json.bad` and the server starts fresh.

With Settings' "Save what panes show" (`save_contents`) on, each save also keeps every pane's screen and the last 2,000 lines of its history, read again once a minute, and a restored pane shows them above its new prompt. What a pane showed can hold secrets: turn it off to keep only the layout.

### Scrollback, the mouse and the clipboard

Each pane keeps 10,000 lines of history. The wheel scrolls back through it, and scrolling down to the bottom ends it; `M-b [` is copy mode, which moves through it and selects with the keyboard, using Vim's keys as tmux's copy mode did. A cursor of its own comes up where the program's cursor is, and the view scrolls to keep it in sight:

| Key | What it does |
|---|---|
| `h` `j` `k` `l`, the arrows | The cursor a cell or a line |
| `w` `b` `e` (`W` `B` `E`) | The next word, the word before, the end of the word (words between blanks) |
| `0` `$` `^` | The start of the line, its end, its first character |
| `g` `G` | The top of the history, the bottom |
| `H` `M` `L` | The top, middle and bottom of the screen |
| `C-u` `C-d` | Half a screen up or down, the cursor with it |
| `C-b` `C-f`, `PageUp` `PageDown` | A screen up or down |
| `C-y` `C-e` | The view a line up or down, the cursor staying |
| `v` or `Space`, `V`, `C-v` | Select from the cursor: characters, whole lines, a rectangle; the same key again takes the selection away |
| `y` or `Enter` | Copy the selection, and out |
| `/` `?` | Search down or up, for text or a regular expression typed in a box (a capital makes case count) |
| `n` `N` | The next match the same way, the other way |
| `Esc` | Take the selection away; with none, out |
| `q` | Back to the bottom, and out |

While a pane is scrolled back, its top-right corner shows how many lines back it is (`[120]`), and other keys do nothing. A key in the wheel's scrolling brings up copy mode's cursor, and it then stays until `q`. A paste takes it back to the bottom and goes to the program.

A click focuses a pane. Dragging selects text and copies it when you let go; a double click selects a word, a triple click a line, and Alt-drag selects a rectangle. A program that asks for the mouse, such as an editor, gets the clicks, drags and wheel instead. In a full-screen program that doesn't take the mouse, the wheel sends it arrow keys. Dragging the line between two panes moves it, resizing them. A click on a tab in the status line goes to it, and the wheel over the status line goes to the tab before or after.

What you copy goes to the computer's clipboard and to your terminal's (OSC 52), which reaches the computer you are at even over SSH, as long as your terminal lets programs set its clipboard. A program in a pane can copy the same way. One that asks to read the clipboard gets nothing back: the clipboard is yours, not every program's.

A link a program in a pane writes as a hyperlink (OSC 8), as Claude Code, `ls --hyperlink` and the views do, reaches your terminal as one, so it opens as it would outside Switchboard: Cmd-click in most terminals on a Mac, Ctrl-click elsewhere, with Shift too where the pane's program takes the mouse. A box or mission control drawn over it isn't part of the link.

### Panes and tabs

A tab is split into panes with `M-H`, `M-J`, `M-K` and `M-L`, each opening a shell in the current pane's directory on that side. `M-S-Left`, `M-S-Down`, `M-S-Up` and `M-S-Right` grow the current pane toward that side, five cells at a time, and `M-=` zooms it to the whole tab and back. Resizing your terminal keeps each pane's share of the tab. `M-w` closes a pane, and the last pane closes its tab (see "Closing a busy pane" below).

### Claude in the tabs

Claude Code's hooks, which `switchboard setup` adds to your `~/.claude/settings.json`, run `switchboard hook` at each step of a session, and the workspace marks the pane accordingly. A pane is in one of three states, or none:

- **working**: from the moment you send a prompt, through every tool call, and while it compacts its context.
- **waiting**: on your permission for a tool, or on your answer to a question, its own or one a tool asks through it. A question is a wait from the moment it is asked.
- **done**: the turn ended and you haven't looked at the tab since. A turn that ends in the tab you are looking at is never marked done.

A tab shows the most urgent of its panes' states, waiting before done before working, as a badge in the status line and in the sidebar. A new or ended session clears the state, and a pane that no longer runs Claude loses its state the next time you move between tabs, so a session that died without saying so leaves nothing behind. A `claude -p` that a script or a loop starts in the tab doesn't mark it, and a conversation moved into the background still shows on the tab it came from.

"done" clears once the tab has been in front for a second, so passing over a tab, as moving the sidebar's cursor does, leaves the mark. Settings' "Finished tabs clear" (`done_clears`) can keep it instead until you next prompt that session ("on prompt"; "when seen" is the default).

`M-a` goes to the next tab that needs you: pinned tabs and reminders that have come due first, then the ones waiting on you, then the finished ones, and last the sidebar's notifications while there are any (Settings' "M-a goes to notifications", `needs_notices`). Pressed again, it goes on to the next, wrapping round, so pressing it repeatedly visits each one. It says "Nothing needs you." when nothing does. `M-c` goes to the pane running Claude in the current tab.

`M-f` pins the tab: it goes to the top of Needs you, ahead of everything else there, whatever Claude is doing in it, and stays there until you press `M-f` in it again, since visiting it doesn't clear it.

A tab holds one Claude. A session started in a tab that already runs one is moved to a tab of its own as it starts, and a box says so, since the sidebar, mission control, the Slack relay, `M-c` and `M-m` each take a tab's Claude to be one pane (Settings' "One Claude to a tab", `one_claude_per_tab`).

Each notification Claude Code sends (it waits on your permission or an answer, or has sat idle) also shows as a desktop notification, titled with the tab's number and name, `Claude Code: 3:app:fix-login`, so you know where to go: a banner on macOS, and the desktop's notifications on Linux. A reminder that comes due shows one too. Settings' "Push notifications" (`notifications`) turns them off.

## Keys

`M-` is Alt: Option on a Mac. A capital letter after `M-` is Alt with Shift (`M-H` is Alt+Shift+h). These keys work from any pane; a key not bound here goes to the program in the pane.

| Key | What it does |
|---|---|
| `M-Left` `M-Down` `M-Up` `M-Right` | Move to the pane on that side, coming round to the far side past an edge, as tmux did; `M-Left` from the leftmost pane, and `M-Right` from the rightmost, go into the sidebar, the leftmost of all. In the diff and Notes they move between the app's own windows first |
| `M-H` `M-J` `M-K` `M-L` | Split the pane to the left, below, above or to the right |
| `M-w` | Close the pane, asking first when a program runs in it |
| `M-=` | Zoom the pane in or out |
| `M-S-Left` `M-S-Down` `M-S-Up` `M-S-Right` | Grow the pane toward that side |
| `M-[` `M-]` | Previous tab, next tab |
| `M-Tab` | Go back to the tab you were on before |
| `M-1` … `M-9` | Go to the tab with that number |
| `M-0` | Go to a tab by its number, typed in a box, for the tabs past 9 |
| `C-w` then `h` `j` `k` `l` | Move to the pane on that side, as in Vim; in Vim, the diff and Notes `C-w` is theirs, and `C-w C-w` sends one `C-w` to the pane |
| `M-t`, `M-n` | New tab running Claude, in a directory you pick |
| `M-j` | Find a tab by typing its name, and go to it |
| `M-o` | Reopen a worktree no tab has open, resuming its Claude |
| `M-W` | Start work on Linear tickets, each in a new tab (`wt`) |
| `M-B` | Start work on a branch, new or existing, in a new tab (`wtb`) |
| `M-a` | Go to the next tab that needs you |
| `M-c` | Go to the pane running Claude in this tab |
| `M-f` | Pin or unpin the tab, keeping it at the top of Needs you |
| `M-p` | The palette: find any command by what it does, and run it |
| `M-Space` | Open or close mission control |
| `M-s` | Into the sidebar, or back out of it |
| `M-v` | The branch's PR description in a pane to the right, or close it |
| `M-i` | The branch's Linear ticket, with its comments, in a pane to the right, or close it |
| `M-d` | The branch's diff against its PR's base in place of the pane, or close it |
| `M-r` | Open the branch's PR in the browser |
| `M-l` | Open the branch's Linear ticket in the browser |
| `M-m` | Check whether the branch's PR is ready to merge |
| `M-F` | Search your notes, Linear, GitHub and Slack at once |
| `M-N` | Go to Notes, wherever it is open, or open it in a tab of its own |
| `M-e` | Notes in a pane to the right, or back in its own tab |
| `M-S` | The Slack relay in a tab of its own |

The rest follow the prefix, `M-b`. While it waits for the next key, a `^` shows at the right end of the status line.

| Key | What it does |
|---|---|
| `M-b d` | Detach, leaving everything running |
| `M-b [` | Copy mode: scroll back through the pane's history, select and copy with the keyboard |
| `M-b c` | New tab, a shell in the current pane's directory |
| `M-b x` | Close the pane |
| `M-b z` | Zoom the pane in or out |
| `M-b %` | Split the pane to the right |
| `M-b "` | Split the pane below |
| `M-b ?` | The list of keys |
| `M-b M-b` | Send `M-b` to the program in the pane |
| `M-b 1` … `M-b 9`, `M-b 0` | Go to that tab (`0` the tenth) |
| `M-b n`, `M-b p`, `M-b l` | The next tab, the previous one, the one you were on before |
| `M-b w` | Pick a tab from a list (as `M-j`) |
| `M-b ,` | Rename the tab |
| `M-b &` | Close the tab, after asking |
| `M-b o`, `M-b ;` | The next pane of the tab, going round; the pane you were in before |
| `M-b` and an arrow | The pane on that side |
| `M-b` and Ctrl with an arrow | Grow the pane a cell that way |

`M-b ?` shows every key bound now, yours included, in a box that `j` and `k` scroll and any other key closes. `switchboard list-keys` prints the same list, a key and what it does on each line.

`C-w` waits for the next key: `h`, `j`, `k` or `l` moves to the pane on that side (from the sidebar too), `C-w` again sends `C-w` itself to the pane (deleting a word in a shell or in Claude's prompt), and any other key does what it would have done. Settings' "C-w then h, j, k or l moves between panes" (`ctrl_w_panes`) off leaves `C-w` to the pane, and Settings' "Alt-arrows move between panes" (`pane_arrows`) off hands Alt-arrows to the pane.

Vim takes `C-w` for its own windows. The diff (`M-d`) and Notes take both `C-w` and the Alt-arrows: they move between their own windows (the file list and the diff's two sides; the notes list and the note) and, at their edge, on to the pane on that side, as Neovim with vim-tmux-navigator did; in Notes, `C-w` while typing deletes a word, as in Vim.

### Option as Alt on a Mac

On a Mac, every `M-` key needs the terminal to send Option as Alt; otherwise Option types a character (`˙`, `∆`, `∫`) into the pane and nothing happens. `switchboard setup` sets it in Ghostty, Terminal and iTerm2 after asking (its `terminal` part, [below](#option-as-alt-and-the-tmux-versions-leftovers)); by hand, set it in the terminal:

- Ghostty: `macos-option-as-alt = true` in its config (`left` keeps the right Option for typing accented characters).
- iTerm2: Settings, Profiles, Keys, then "Left Option key" (and the right one) set to "Esc+".
- Terminal: Settings, Profiles, Keyboard, then "Use Option as Meta key".
- kitty: `macos_option_as_alt yes`. WezTerm: `send_composed_key_when_left_alt_is_pressed = false`. Alacritty: `option_as_alt = "Both"` under `[window]`.

On Linux, terminals send Alt this way already. Where Option doesn't act as Alt, the first of `˙` `∆` `˚` `¬` (what Option+h, j, k and l type) brings a tip saying so, once; each still reaches the pane.

### Keys told apart

An old terminal sends some keys as the same bytes: Ctrl-Shift-P as Ctrl-P, Shift-Enter as Enter, Ctrl-I as Tab. So, as tmux's `extended-keys on` did, Switchboard asks your terminal to send such keys as keys of their own, through the kitty keyboard protocol or xterm's modifyOtherKeys, whichever it knows (Ghostty, kitty, WezTerm, Alacritty, foot, iTerm2 and xterm each know one), and gives back what it asked for when it lets go of the terminal. A terminal that knows neither sends the keys as before.

A program in a pane gets a key so only when it asked for keys told apart the same way, as Notes, Neovim, Helix and fish do: Ctrl-Shift-P reaches Notes as Ctrl-Shift-P, which opens its palette, while a shell, which asked for nothing, still gets Ctrl-P's byte. Settings' "Extended keys" (`extended_keys`, Help) turned off asks the terminal for nothing, and every program gets the keys a plain terminal sends; it takes effect when a terminal next attaches, and in panes opened after.

### Changing the keys

`~/.config/switchboard/keys.json` adds keys, changes them or takes them away, over the defaults above. It has up to three parts: `prefix-key`, the key that starts the prefix; `root`, keys pressed alone; and `prefix`, keys pressed after the prefix. Each key names an action, or `null` to unbind it:

```json
{
  "prefix-key": "C-a",
  "root": { "M-g": "new-tab", "M-Left": null, "M-Right": null },
  "prefix": { "y": "copy-mode" }
}
```

Keys are written as tmux wrote them: `M-` for Alt, `C-` for Ctrl, `S-` for Shift, then a character or one of `Enter`, `Escape`, `Tab`, `BTab` (Shift-Tab), `BSpace`, `Space`, `Up`, `Down`, `Left`, `Right`, `Home`, `End`, `PageUp`, `PageDown`, `Insert`, `Delete` and `F1` to `F12`. `M-F` and `M-S-f` are the same key. Unbinding `M-Left`, `M-Right`, `M-Up` and `M-Down`, as above, hands Alt-arrows to the programs in your panes.

The server reads the file as it starts and again within two seconds of a change, so a change takes effect without a restart; anything in it that can't be used is said in a box (to the first client to attach, at the start) and written to `server.log`. Settings' "Key bindings" row, under "Your files", opens it in an editor. A `~/.config/switchboard/tmux.conf` from the tmux version isn't read: the first client to attach is told so, and where its keys and options go now.

The actions, by the names the file uses:

- Panes: `select-pane-left`, `select-pane-right`, `select-pane-up`, `select-pane-down`, `next-pane`, `last-pane`, `pane-nav` (`C-w`, waiting for `h` `j` `k` `l`), `split-left`, `split-right`, `split-up`, `split-down`, `close-pane`, `zoom`, `resize-left`, `resize-right`, `resize-up`, `resize-down` (five cells; `resize-left-2` for two), `copy-mode`.
- Tabs: `prev-tab`, `next-tab`, `last-tab`, `select-tab-1` (any number from 1), `go-to-tab` (`M-0`), `new-tab`, `close-tab`, `new-claude-tab`, `find-tab`, `revive-worktree`, `toggle-pin`, `remind-tab`, `rename-tab`, `calendar-tab`, `slack-tab`.
- Claude and work: `next-needs-you`, `focus-claude`, `work-on-tickets`, `work-on-branch`, `work-on-pr`, `work-on-note`, `work-note-continuously`, `continue-shaping`, `shape-note`, `shape-new-ticket`, `send-reply`, `update-main`.
- The workspace: `palette`, `mission-control`, `mission-<view>` (the view's name in lower case with dashes for spaces: `mission-work-map`, `mission-calendar`, `mission-slack`, `mission-closed`, `mission-prs`, `mission-tickets`, `mission-inbox`, `mission-ci`, `mission-merge-queues`, `mission-sessions`, `mission-usage`, `mission-resources`), `notifications`, `settings`, `habits-report`, `focus-sidebar`, `toggle-sidebar`, `toggle-theme`, `reset-tips`, `list-keys`, `detach`, `send-prefix`, `slack-relay`, `stop-slack-relay`, `update-switchboard`, `run-setup`, `check-setup`, `setup-part`, `aws-login`.
- Reading: `pr-view`, `ticket-view`, `pr-image`, `ticket-image`, `open-pr`, `open-repo`, `open-ticket`, `open-pr-number`, `open-ticket-id`, `open-merge-queue`, `open-linear-project`, `copy-ticket-id`, `copy-ticket-url`, `copy-pr-number`, `copy-pr-url`, `pr-ready`, `search-everything`, `read-link`, `open-link`, `copy-link`, `open-media`, `open-link-being-read`.
- The diff: `diff`, `diff-working-tree`, `lazygit`, `tool-beside`, `tool-alone`, `copy-diff-story`, `ask-about-diff`.
- Notes: `notes`, `notes-beside`, `note-tab`, `choose-tab-note`.

These have no key by default and are in `M-p` alone: `work-on-pr`, `work-on-note`, `work-note-continuously`, `continue-shaping`, `shape-note`, `shape-new-ticket`, `pr-image`, `ticket-image`, `lazygit`, `diff-working-tree`, `settings`, `remind-tab`, `toggle-sidebar`, `toggle-theme`, `reset-tips`, `stop-slack-relay`, `open-repo`, `read-link`, `open-link`, `copy-link`, `open-media`, `open-link-being-read`, `tool-beside`, `tool-alone`, `copy-diff-story`, `ask-about-diff`, `note-tab`, `choose-tab-note`, `update-switchboard`, `run-setup`, and those of [More in the palette](#more-in-the-palette): `rename-tab`, the `mission-<view>` commands, `notifications`, `habits-report`, `send-reply`, `update-main`, `check-setup`, `setup-part`, `calendar-tab`, `slack-tab`, `open-pr-number`, `open-ticket-id` and `aws-login`. Give any of them a key in `keys.json` to have it on one, `open-merge-queue`, `open-linear-project`, `copy-ticket-id`, `copy-ticket-url`, `copy-pr-number`, `copy-pr-url`.

## Tabs and the status line

The status line along the bottom starts with the session's name on a badge in the current tab's color, then how many tabs wait on you (`!`, yellow), have finished unseen (`✓`, green) and are working (`⋯`, blue), each count shown only while it isn't zero. Then come the ⌘ Commands button, the health light when something is wrong, and the tabs, each with its number, its state's badge and its name, in its project's color, the current one bold. When the tabs don't all fit, the list scrolls to keep the current one in view, as near the middle as the ends allow, with a `<` or `>` at the end where more are hidden.

### What a tab is called

A tab is named after the directory of the pane you are in (the sidebar and a Notes pane beside your work don't count), read again every two seconds, so a `cd` renames it. A worktree gives its own name; any other directory gives its name with the branch after a colon (`app:fix-login`), unless the branch is `main` or `master`. When the branch names a Linear ticket that Linear confirms, the tab takes the ticket and its title instead (`ABC-123: Fix the login`). When the branch has a pull request, its number goes in front (`#123 ABC-123: Fix the login`), first because it is short and doesn't change, so the tabs can be scanned by number.

A name given to the tab wins over all of that, the PR number still in front: the `@tab_name` that "Rename the tab" (and "Rename it" in mission control's menu for the tab) sets, as a skill such as `/shape` does, so `/wt` clearing it hands the tab its ticket's name; else one set by `switchboard rename-tab <name>`. An empty name hands the tab back to its directory.

The PR number and the ticket's title are asked of GitHub and Linear in the background, so a tab never waits on the network: it shows what is known and takes the rest a moment later. A PR found, or a ticket's title, is kept a day, across a restart too (in `~/.cache/switchboard/tab-names.json`); a branch with no PR is asked about again a minute later, and a lookup that fails keeps what was shown and tries again in a minute. Settings' "PRs and tickets in tab names" (`tab_lookups`) turns both lookups off, and turning GitHub or Linear off under Integrations turns off its own.

Each project's tabs wear a color of their own, worked out from the directory: the worktree, or the directory and its branch. Claude Code's statusline, as `switchboard setup` sets it up, draws the same color, so a session and its tab match. The session's badge and the lines around the focused pane take the current tab's color. Settings' "Project colors" (`accent_colors`) turns them off, and every tab is gray.

### The ⌘ Commands button and the health light

The ⌘ Commands button opens the palette when clicked, as `M-p` does (Settings' "Commands button", `palette_button`).

The health light stays off while all is well, and shows a red `●` naming what has quietly stopped, so you see it before you miss what it stopped:

- `saves 7m old`: the layout changed and the save hasn't landed for over five minutes.
- `relay down`: the Slack relay crashed, rather than being stopped.
- `Slack not connected`, or another service: a Claude connector a feature needed wasn't connected when it last tried, in the last two hours.

Settings' "Health light" (`health_light`) turns it off.

### The second status line

Settings' "Second status line" (`second_status`, off by default) adds a line under the tabs about the current tab: its name in full, where the tab may cut it short, its PR and whether it has every commit made here ("↑2 to push" in yellow, the commits on the branch that its upstream, or origin's copy of it, hasn't got as last fetched or pushed; "pushed" when it has them all), the program in front of its pane, and in a Claude pane, what the session is doing ("waiting on you for 4m", "finished 2m ago", "working"), its name and your last prompt to it.

### Theme

Settings' "Theme" (`theme`) draws the status line, the lines between panes, the panes without the focus and every box dark or light; `system` follows your terminal's background, which the workspace asks each terminal for as it attaches, and stays dark when the terminal doesn't answer. `M-p`'s "Toggle light and dark theme" switches between the two. The programs in your panes keep your terminal's own colors either way. The diff and Notes follow the setting too, `system` by the background your terminal said.

The workspace also asks each terminal as it attaches for its text and background colors and the sixteen colors of its palette, and tells a program in a pane that asks for them your terminal's own, as tmux did: the diff mixes git's green and red over your real background, draws its code in your terminal's palette, and gives its buttons and faint text shades between your background and text.

## The palette, messages, tips and menus

### The palette

`M-p`, or a click on ⌘ Commands, opens the palette: every command the workspace has, by what it does, with its tags dimmed beside it and its key in a column of its own, Shift spelled out (`M-S-F`). It lists every action a key is bound to, yours included, and the ones that have no key. `M-1` to `M-9` are one row. Type to narrow the list, each word matched as you write it, and Enter runs the command as its key would, in the pane you were in. Esc closes it.

The commands you picked last come first, the newest at the top, then the rest by description. As you type, a command you picked before after typing the same words comes first, then one picked after longer words that start with what you typed, so `sha` finds what you picked after typing `shape`. What you typed before each pick is kept, the last 300, in `~/.local/state/switchboard/palette/`, readable only by you; Settings' "Learn from what you pick" (`palette_learn`) turns that off. After you pick a command that has a key of its own, a tip names the key, once for each key.

While GitHub or Linear is off in Settings, the commands tagged `#github` or `#linear` leave the palette, and every key bound to one (`M-v`, `M-r` and `M-m` for GitHub, `M-i`, `M-l` and `M-W` for Linear, and any you give one in `keys.json`) does nothing but show a tip saying the service is off, the first time, and nothing after. The diff's own commands, copying its story and asking Claude about it, are listed only while the pane has the diff open (asking only with Settings' "Ask Claude about the diff" on), and the two that show the pane a tool stands in for beside it, or the tool alone again, only while a tool stands in the pane's place.

`M-p`'s "Shape a note's checklist items as Linear tickets" picks a note (the one open in Notes first, then the newest), then its items (every one chosen to begin with; Enter on a row leaves it out or puts it back, and the first row shapes what is chosen), then the repository, and opens a background tab running `/shape` for each item, its details and numbered sub-items with it. "Shape a new Linear ticket from what you type, in a new tab" does the same for one idea typed in. Both need Linear (the `linear` setting). `M-p`'s "View the branch's PR description as an image" draws the description as GitHub does, in place of the pane.

### More in the palette

These are in `M-p` alone, with no key unless you give them one in `keys.json`:

- **Rename the tab**: a name typed in a box, starting from the one it has; empty gives the tab its own name back, as "Rename it" in mission control's menu does.
- **Mission control: \<view\>**, one for each view (Work map, Calendar, Slack, Closed, PRs, Tickets, Inbox, CI, Merge queues, Sessions, Usage, Resources): mission control opened on that view. A view turned off in Settings is said so at the bottom of mission control, which opens on the tabs.
- **Go to the notifications**: the Notifications window, every notification with its whole message and Enter opening its actions; with Settings' "Notifications window" (`notice_list`) off, the sidebar's cursor on the first notification instead, as `M-a` reaches them, or, in a window with no sidebar, mission control on the first one's view (its Slack, Inbox or Calendar view, Settings for a suggestion), else a box saying why there is nowhere to show them.
- **How you use Switchboard**: what [Suggestions from how you work](#suggestions-from-how-you-work) has counted, in a box: the keys you press most this week and month, the commands you pick in `M-p`, the views you open, and the suggestions now. `j` and `k` scroll it, and any other key closes it.
- **Send Claude's last reply to another tab**: the last thing this tab's Claude said in its last turn, from its transcript, pasted into the Claude of a tab you pick (its active pane when it runs none) without Enter, and that tab brought to the front, so you can add to it before sending. Its line breaks stay in the text.
- **Update local main to origin's, without switching branches**: `switchboard update-main` (below) for the pane's repository, what it did in a box.
- **Check Switchboard's setup**: `switchboard setup --check`, what it would change, in a box, changing nothing. **Run a part of Switchboard's setup, or only check it** lists the parts (`switchboard setup --parts`) and every part at once, Tab marking several, then runs the ones picked or only checks them.
- **Open a Calendar tab** and **Open a Slack tab**: the tabs the Calendar and Slack views open in the background when they are out of date (`/loop 30m /today`, `/loop 5m /slack-updates`, Claude in your home directory), opened in front.
- **Open a PR in the browser by number or URL**: a PR's number (a PR of the pane's repository) or its link, typed in a box that starts with the clipboard's when it holds one; empty is the tab's own PR, as `M-r` opens it. Then a menu: open it in the browser, or work on it in a new tab (`wtpr`). Needs GitHub.
- **Open a Linear ticket in the browser by ID, number or URL**: the same for a ticket, a bare number taking the team key of the ticket the branch names, and working on it a new tab running `wt` in the pane's repository. Needs Linear.
- **Open the repository's merge queue in the browser**: GitHub's merge queue for the branch's PR's base branch, or for the repository's default branch when the branch has no PR. Needs GitHub.
- **Open the branch's Linear project in the browser**: the project the ticket in the branch (or the tab's name) is in, as Linear says, or a box saying why there is none. Needs Linear.
- **Copy the branch's ticket ID**, **ticket URL**, **PR number** and **PR URL**: copied, and a box saying what was. The ticket's link is the workspace's in `linear.json` when it names one, else the one Linear gives. Each needs its service.
- **Log in to AWS (aws sso login) in your Chrome profile**: `switchboard aws-login` in a pane beside yours, which logs in to the one `[sso-session]` in `~/.aws/config` and opens AWS's login page in the Chrome profile `browser.json` gives `awsapps.com` (else its default profile, else the one Chrome picks itself, while Settings' `chrome_profiles` is on), whatever host the page is on. The pane stays until the login is done and a key is pressed, so its code and any error can be read. It needs the AWS CLI (`aws`), which Switchboard doesn't install.

### Messages

What a key has to tell you (nothing needs you, a link copied, no ticket in this branch) shows in a small box in the middle of the screen, as wide as its text, which any key or a click closes and which goes by itself after five seconds, rather than on the status line for a moment (Settings' "Messages in a box", `message_boxes`). The key that closes it goes no further. A new message takes the place of one still up. `switchboard say` shows one from a script.

### Tips

A tip is a small box in the top-right corner that teaches a key at the moment it would help, once each:

- A tab first waits on you, or first finishes, while you look elsewhere: `M-a`.
- You switch tabs with five or more open: `M-j` and `M-a`.
- You pin a tab for the first time: how it stays pinned.
- You first move into the sidebar: its keys.
- A tab's PR first merges: `/queue-and-clean`.
- A tab first loops in the background: where mission control shows it.
- A tool first takes a pane's place, as `M-d` does: how to get the pane back.
- You answer the first update offered: where to look for updates.
- You press a key of GitHub or Linear while that service is off: how to turn it back on.
- You pick a command from `M-p` that has a key of its own: the key.
- Claude first starts in a worktree (`.claude/worktrees/`), with GitHub on: `M-v`, `M-d` and `M-m` for its PR.
- Option types `˙` `∆` `˚` or `¬` where it doesn't act as Alt: how to set it (see [Option as Alt on a Mac](#option-as-alt-on-a-mac)).

Mission control's first opening says how to get around it in its own footer instead.

Any key closes a tip and then does what it would have done, so nothing you type is lost; a click or the wheel closes it and goes no further. Left alone, it goes after eight seconds. A tip that comes while another box is up (another tip, a message, a menu, mission control) waits for its next chance. Doing what a tip teaches (pressing `M-a`) counts as having seen it. The tips shown are listed in `~/.local/state/switchboard/tips`. `M-p`'s "Show the tips again" shows each once more, and Settings' "Tips" (`tips`) turns them off.

### Menus and questions

Every menu, question and list looks the same: a rounded blue edge with the title in its top, the chosen option highlighted with a yellow `›`, and the keys dim along the bottom. In a menu, `j` and `k` (the arrows, and the wheel with Settings' "Click in menus" on) move, `g` and `G` go to the first and last, Tab and Shift-Tab go round, Enter or Space takes the chosen option, a number takes that one, as does an option's own key where one has it (never one the menu moves or closes with), and Esc or `q` goes back. In a list to type in, such as the palette, the arrows (`C-p` and `C-n`, Tab and Shift-Tab) move, Enter picks, and Esc closes.

A question shows `[Y/n]` and takes Enter as yes; `y` says yes too, and `n`, Esc or any other key says no. Space answers only once Tab has moved between Yes and No, so a stray Space is still no.

Menus and questions take a click too: one click takes an option or answers Yes or No, a click on a list's row twice picks it, and a click outside the box closes it (Settings' "Click in menus", `menu_mouse`). With it on, Settings' "Highlight on hover" (`hover_highlight`) lights the option under the mouse, and Settings' "Easy mode" (`easy_keys`, off by default) puts a `×` in each box's top-right corner, which a click, or Tab to it and Enter, closes. A box's options always take Tab and Shift-Tab; Settings' "Tab moves focus" (`tab_focus`) does the same for mission control and the sidebar. A box follows these settings as they are when it opens.

### Closing a busy pane

`M-w` closes a pane at once when it is a shell at its prompt. When a program runs in it, it asks first, "Close pane?", with the program's command line under the question, so a running agent or build isn't lost to a stray key. Settings' "Ask before closing a pane" (`confirm_close`) turns the question off.

## The sidebar

The sidebar runs down the left of every tab: the session's tabs, most in need of you first, what each one's Claude is doing, under them the first items of each plugin that asks for a place there (see "Plugins"), and the notifications. It is 34 columns wide, and a window narrower than 90 columns has none (both are set in `sidebar.json`, below); a zoomed pane covers it. Settings' "Sidebar" (`sidebar`) turns it off and on, as does "Turn the sidebar on or off" in `M-p`.

At its top are the session's name, in a pill of the current tab's color, and its counts: failing builds first, in red (`✗ 2`), then the tabs waiting on you (`! 1`), finished (`✓ 3`) and working (`⋯ 4`). Below them are the tabs in their groups, each group under its heading with how many it holds.

### Groups

Each tab is in one group, and the groups come in this order:

| Group | What puts a tab there |
| --- | --- |
| Needs you | Its Claude waits on you or has finished there, you pinned it, or its reminder came due. Pinned tabs and reminders come first, then waiting tabs, the longest-waiting first, then finished ones, the oldest first. |
| Build failing | Its open pull request has a failing check and isn't in a merge queue. The heading is in red. |
| Working | Claude is working, or its turn ended with background tasks still running, which Claude carries on from when they end. |
| Monitoring | Claude's only background work is `Monitor` watches, each named ("monitoring · CI on #12"). |
| In merge queue | Its pull request is in the merge queue, in queue order. |
| Landing | It holds or waits for the landing lock of `/queue-and-clean`'s "Land on the default branch", in the landing queue's order. |
| Testing Deploy | Its pull request merged and the tab is still at work on it: `/deploy-test` waiting for the deploy or recording its test, or Claude working under `/deploy-test` or the `/queue-and-clean` that follows the merge. |
| Merged | Its pull request merged. |
| Idle | A Claude session with nothing to do. |
| Loops | A `/loop` between its ticks: a self-paced one while the wakeup its last tick scheduled is pending, and for five minutes after it is due; one on a timer (`/loop 5m …`, or a cron job Claude created) until Claude deletes the job, the session ends, or seven days pass. |
| Reminders | A reminder still to come, while nothing else places the tab. |
| Other | No Claude in it: Notes, the Slack relay, a shell. |

Needs you always wins: a tab whose Claude waits on you stays there even while its pull request is queued. After it, the landing queue and the pull request's place win over what Claude is doing, so a tab whose PR is queued stays under In merge queue while `/queue-and-clean` watches it; then a failing build; then the work running; then a reminder still to come. A tab's pull request, its checks and whether it is queued or merged come from one GitHub query for every tab's PR (see "How often mission control looks"), so a group can lag GitHub by that long. A tab whose worktree has been removed, as `/queue-and-clean` leaves it once its PR merges, is found by the worktree's branch and keeps its PR.

Settings' "Build failing" (`build_failing`) off leaves such tabs in Working or Idle, the failing count still in their words. Settings' "Show the landing queue" (`landing_queue`) off leaves landing tabs where their work puts them; the queue still orders the landings.

### What each tab shows

A tab's row starts with a bar in its project's color (thicker on the chosen tab), its badge, its number (for `M-1`…`M-9`) and its name as the tab bar shows it, wrapped to at most three lines. Under it is what it is doing and for how long, as its Claude's hooks record it: "waiting on you 4m · permission: Bash git push", "finished 2m ago", "working 2m · Run the test suite", "working in background · 2 tasks", "idle for 20m". A tab running a known loop names it from the moment you ask for it, the work between its rounds included: "review loop · round 2" while `codex-review` runs, two runs to a round; "CI loop · round 3" while `/ci-loop` watches the checks; "queue-and-clean · watching" while `/queue-and-clean` watches the merge queue, or the step it is at while it lands on the default branch ("queue-and-clean · waiting for the landing lock"). A loop between ticks says when the next one is and the loop's prompt ("next in 8m · /oncall"). The row adds the place of its pull request ("in merge queue at 3", "merged") or in the landing queue ("landing", "2nd in line to land"), the PR's number when the name doesn't already say it, and a failing check in bold red ("✗1 failing") whatever else the tab is doing. A tab with no Claude names the program in front.

| Badge | Means |
| --- | --- |
| `!` yellow | Claude is waiting on you |
| `✓` green | Claude finished and you haven't looked yet |
| spinner, blue | working |
| spinner, slate | working in the background |
| spinner, violet, teal or orange | a review loop, a CI loop, `/queue-and-clean` |
| spinner, magenta | testing the deploy |
| `◉` cyan | monitoring |
| `↻` blue | a loop between its ticks |
| `»` orange | idle, its PR in the merge queue |
| `◆` purple | its PR merged |
| `⇣` amber | landing, or in line to land |
| `✗` red | idle, its open PR has a failing check |
| `📌` yellow | pinned |
| `⏰` blue | a reminder still to come |
| `⏰` orange | a reminder come due |
| `○` faint | idle |

The states are the ones the tab bar's marks show, recorded by the hooks `switchboard setup` adds to Claude Code.

### Notifications

The notifications sit at the bottom of the sidebar, under a double yellow rule and their own heading, so they never scroll away and never read as a tab. Each kind is one line, saying how many there are, with a line of detail from the first under it, so the tabs keep their room however many come in; the Notifications window lists each one. The kinds, in this order:

- the next meeting, as it is about to start ("Standup in 3m · ↵ joins");
- new Slack messages for you ("3 new on Slack"), until you have seen them in the Slack view or dismissed them;
- changes to today's calendar since you last looked at it ("Calendar: Standup moved to 3pm", or "2 calendar changes");
- notes written for you ("New note: Standup 2026-11-02", or "3 new notes"), until you open them in Notes;
- unread notifications of your Linear Inbox ("2 in your Linear inbox"), while the Inbox view is on;
- what working a note continuously has to say: that it stopped for the day's spend, or the note has nothing left;
- suggestions from how you work.

"Your day", "How you use it" and "Working a note continuously", below, say more of each.

Moving past the last tab with `j`, `n` in the sidebar, or a click puts the cursor on them, and brings the Notifications window to the front, as moving onto a tab brings that tab: a tab of its own, named Notifications in the tab bar only while it is in front, listing every notification with its whole message (a Slack message as Slack draws it), the one the cursor is on chosen. `j` and `k` in the sidebar go through them one by one, the window following, and up from the first goes back to the last tab. Enter in the sidebar moves into the window, where `j` and `k` choose (the sidebar's cursor following), Enter, `l` or a click on the chosen one opens its menu over it, as in mission control's Notifications view, and `Esc`, `q` or `h`, or a click on "‹ tabs", go back to the tab it was brought in front of; with Settings' "Tab moves focus" on, Tab and Shift-Tab go round "‹ tabs" and the notifications. The window is no tab: the sidebar, mission control, `M-1`…`M-9`, the next and previous tab keys and `switchboard tabs` leave it out, it stays after every tab, and it isn't saved with the layout. Settings' "Notifications window" (`notice_list`) turned off leaves the cursor on them in the sidebar, a kind at a time, where Enter or a click does what that kind leads to: the meeting joins its call (or, with none, opens mission control's Calendar view), the newest note opens in Notes, calendar changes open the Calendar view, Slack messages and Inbox notifications open mission control's Slack and Inbox views, suggestions open Settings, and working a note continuously opens mission control on its line. Each one's menu, in the Notifications window or mission control's Notifications view, offers what its view's menu does and dismissing it, and whatever is picked there dismisses it, since it was dealt with; a suggestion's menu has its own choices. Settings' "Notifications" (`sidebar_notifications`) turns the block, and that view, off.

### Into the sidebar and its keys

`M-s` moves into the sidebar, on the tab you are in, and back out; `M-Left` from the leftmost pane moves in too, and `M-Right` back out. While it has the focus, its cursor moves through the tabs and each comes to the front as the cursor reaches it, with the list held still so tabs don't move about as you look at them. Leaving keeps the tab you are on. A key that acts on "the pane" (a split, `M-w`, zoom) gives the focus back to your pane first and acts there; `M-a`, `M-1`…`M-9`, `M-f` and `M-p` keep it, and pressed from the sidebar `M-a` and `M-1`…`M-9` leave the cursor on the tab they went to.

| Key | In the sidebar |
| --- | --- |
| `j` `k` (arrows, `C-n` `C-p`) | the next or previous tab, brought to the front |
| `g` `G` | the first or last tab |
| `C-d` `C-u` | the list half a screen down or up, the cursor onto the tab there, stopping at the first and last tab, as in Vim (Settings' "Half-page scrolling", `sidebar_half_page`; off, they run in the chosen tab) |
| `Enter` `l` | into the chosen tab's pane; on a notification, into the Notifications window (with it off, what the notification leads to) |
| `h` `q` `Esc` `C-c` | back out to your pane (`Esc` clears a kept filter first) |
| `1`–`9` | the cursor onto that tab, brought to the front |
| `/` | find a tab by name: typing narrows the list, the arrows move, Enter goes, Esc clears |
| `Tab` `S-Tab` | the next or previous row, tabs then notifications, wrapping round (Settings' "Tab moves focus", `tab_focus`; off, `Tab` opens mission control) |
| `Space` | peek at the chosen tab |
| `a` | answer the chosen tab's Claude, or send it a message, in mission control's box (see [Answering a tab](#answering-a-tab)) |
| `n` | to the notifications, or a box saying there are none |
| `t` | a new tab running Claude, as `M-t` opens one, from the chosen tab, so its directory is first in the list |
| any other Alt or Ctrl key | goes to the chosen tab and runs there |
| `?` | these keys |

A click on a tab goes there, a right click peeks at it, and the wheel moves the cursor, bringing the focus into the sidebar. A click anywhere else takes the focus back out. With Settings' "Highlight on hover" (`hover_highlight`) on, the row under the mouse is lit.

### Peeking

A peek shows a tab without going there, in a box over most of the screen: its last turn in full (your prompt, all of Claude's reply, and the tool call it waits on), or, for a tab with no Claude, its screen with 200 lines of what scrolled off above it. `s` switches between the turn and the screen, `r` reads the screen again, `j` `k` scroll (`C-d` `C-u` half the box, `g` `G` the ends), Enter goes to the tab, and `Esc`, `q`, `C-c` or `Space` close it, so `Space` in the sidebar opens and closes it; with easy mode's ×, Tab moves onto it and Enter presses it. `Space` in the sidebar, a right click on a tab, and "Peek at its last turn" in a tab's menu in mission control open it.

### What needs you, one tab at a time

`M-a` goes to the next of what needs you, across every session, in the sidebar's order: the tabs under Needs you, pinned ones and reminders come due first, then those waiting on you, then those that finished, and last, while there are any, the notifications (Settings' "M-a goes to notifications", `needs_notices`), where it puts the sidebar's cursor on the first one. Each press goes on to the stop after the one you are at, wrapping round, so pressing it again and again walks the whole list, though a tab stays waiting until you answer it. A finished tab leaves the list once seen, so pressed again from there the walk carries on where it was. From a tab that only finished, or one not on the list, a tab pinned or waiting on you comes first. With nothing to go to it says "Nothing needs you."

### Pinning and reminders

`M-f` pins the tab you are in: it goes to the top of Needs you with a yellow `📌`, whatever its Claude is doing, and stays until `M-f` unpins it, since going to it doesn't clear it. A tab's menu in mission control pins and unpins it too.

"Remind me about this tab later" in `M-p`, or "Remind me about it later" in a tab's menu in mission control, asks when, in a list (in 20 minutes, in 1 hour, in 3 hours, tomorrow at 9:00, next Monday at 9:00) where a choice's number and Enter take it, or a time typed in (`45m`, `2h`, `1d`, `3pm`, `15:30`, `tomorrow 9am`, a clock time already past today being tomorrow's), which wins over a row it only resembles, then what for, which may be left empty. Until then the tab waits under Reminders with a blue `⏰` and the time left ("remind in 1h 20m · check the deploy"), unless something else places it: working, it is under Working, and waiting on you or finished, under Needs you, each with its own badge and the reminder's words beside it. When the time comes it moves to the top of Needs you with an orange `⏰`, and a box on the screen used last names it, with a desktop notification while Settings' "Push notifications" (`notifications`) is on. It is done with once you are at the tab. Asked for again on a tab with a reminder, the menu offers "Clear the reminder" first. Reminders are kept with the tabs and survive a restart. Settings' "Reminders" (`reminders`) off neither sets nor shows them.

### The Landing group

`/queue-and-clean` in a repository that commits straight to its default branch lands each worktree's work one at a time, behind a landing lock, the others waiting in a landing queue. The tab holding the lock is first under Landing, saying "landing" or the step it is at; the tabs waiting follow in the queue's order ("2nd in line to land"). A tab counts only while its worktree is there and a Claude session runs in it, so a landing that ended without a word leaves the group. Enter on a waiting tab's card in mission control offers "Land it next", "Move it up in the landing queue" and "Move it down in the landing queue", which reorder the queue as `switchboard land-lock move` does; the landing tab can't be passed.

### sidebar.json

`~/.config/switchboard/sidebar.json` sets the sidebar's size, mission control's cards and the plugins. Every key is optional, and a file that isn't JSON leaves every key at its default. It is read again whenever it changes, and mission control's Settings view has a row for each key.

```json
{
  "width": 34,
  "minWindowWidth": 90,
  "cardWidth": 56,
  "cardHeight": 13,
  "plugins": { "github": { "query": "org:your-org" } }
}
```

`width` is the columns the sidebar takes, `minWindowWidth` the narrowest window that gets one, `cardWidth` and `cardHeight` the size of mission control's cards, and `plugins` the plugins that load (see "Plugins").

## Mission control

`M-Space` opens mission control over the whole screen, and closes it again. Its first view is every tab of every session as a card; the views after it (the Work map, pull requests, CI, tickets, your calendar and Slack, Settings and more) are named in the bar across the top, each with how many items it has, beside the counts of tabs waiting, finished and working. The keys, or what the last action did, are at the bottom. The first time it opens, the bottom line says how to get around it.

### The cards

The cards are grouped in stages, finer than the sidebar's groups: Needs you, Build failing, Working, Monitoring, Review loop, CI loop, Queue-and-clean, Loops, In merge queue, Landing, Testing Deploy, Merged, Idle, Reminders, then Other. A tab running a loop is in that loop's stage, from the moment you ask for it, while Claude works between its rounds too; a tab whose pull request is in the merge queue, landing or merged is in that stage even while it runs a loop; and a tab with a failing build that runs `/ci-loop` stays under CI loop while the loop is at work. Each stage is a box in the stage's color, titled on its top edge, packed side by side so the screen fills: a stage that doesn't fit in the room left on a row starts the next, and one too big for a row takes several rows of cards. While a note is worked continuously, a line above the cards says so ("Working a note continuously", under Worktrees).

A card's top edge carries the tab's badge, number and name. Inside are what it is doing, its pull request's number, checks, review and size ("#12 · ✓12 ✗1 ⏳3 · approved · +340 −80"), and the session's name; then your last prompt, the end of Claude's last reply, and, when it waits on a permission prompt or a question, what it is asking. A tab with no Claude shows its directory.

### Keys

Mission control's own keys only move about; everything that acts on what is chosen is in the menu Enter opens.

| Key | In mission control |
| --- | --- |
| `h` `j` `k` `l` (arrows) | move between cards or rows; `j` `k` move a row of cards keeping the column, on a board `h` `l` move between its columns, and in the Calendar between events side by side |
| `g` `G` | the first or last |
| `1`–`9` | choose that tab |
| `]` `[` | the next or previous view |
| `Tab` `S-Tab` | move the focus through what can be clicked, as on a webpage: each view's name, the filter, then each card or row, wrapping round; Enter or Space presses it. With Settings' "Tab moves focus" (`tab_focus`) off, they switch views |
| `v` | go to a view by typing part of its name, lit in the bar as you type; Enter goes there |
| `/` | filter: the cards by name, number, PR or directory, a view's rows by their words (not on the PR and ticket boards); Enter keeps the filter, Esc clears it |
| `Enter` | the menu of what can be done with the chosen card or row |
| `Space` | in Settings, changes the chosen setting |
| `r` | asks a plugin's view again |
| `?` | these keys |
| `Esc` | clears a filter, else closes mission control |
| `q` `C-c` `M-Space` | close it |
| `M-w` | close it, whatever is open over it |

A click on a view's name goes to that view, a click on the filter starts typing in it, and a click on a card or row chooses it, a second click doing what Enter does. The wheel moves through the cards or rows. With Settings' "Highlight on hover" (`hover_highlight`) on, what a click would press is lit under the mouse.

### Menus

Enter on a card or row opens a menu of what can be done with it, its usual action first, so Enter twice does it. `j` `k` move, Enter or Space takes the chosen option and a number takes that option; its options have no letters of their own, as the tmux version's had none, so a letter never acts ahead of moving. Esc or `q` goes back. With Settings' "Click in menus" (`menu_mouse`) on, a click takes an option and a click outside closes the menu. An option that takes a moment (opening a link, asking Linear, removing a worktree) shows a box with a spinner until it is done (a link until the browser comes to the front, up to eight seconds, or a moment on Linux, where that can't be told), and what it did is said at the bottom of the screen.

On a tab, the menu offers:

- go to the tab;
- answer it here, or send it a message (below);
- peek at its last turn, or at its screen for a tab with no Claude;
- its diff, as `M-d` opens it, or lazygit (below), in its pane's place, going to the tab;
- the `M-p` palette in that tab;
- rename it (empty names it after its directory again);
- pin or unpin it;
- set a reminder, or change or clear one;
- for a tab waiting in the landing queue, land it next or move it up or down;
- close the tab, after asking;
- on a merged tab, close every Merged tab whose Claude isn't waiting on you or finished, and remove their worktrees, after asking: a worktree with changes not committed is kept, and named.

### Answering a tab

"Answer it here, or send it a message" opens a box with the question or permission prompt the tab's Claude shows, read off its screen, and its options numbered. A number answers with that option, `s` submits a question that takes several answers, and `t` types an answer of your own where the question takes one. When the tab shows no prompt, what you type is sent to it as a message with Enter. After an answer the screen is read again, and the box comes back for the next question of several.

### The views

After the cards come Notifications, the Work map, Calendar, Slack, Closed, PRs, CI, Merge queues, Tickets, Inbox, Sessions, Resources, Usage and Settings, then each plugin's view, each only while its setting is on. The ones that read a service ask it again when you go to them.

**Notifications** is every notification the sidebar shows, one by one and whole. Enter's menu does what each leads to ("Open it in Notes" for a new note, "Open the Calendar view" for a calendar change, the Calendar view's menu for the next meeting, the Slack view's for a message and the Inbox's for an Inbox notification), and offers to dismiss it: a meeting, a Slack message (until its thread gets more), a calendar change (taking today's calendar as seen), a new note, or an Inbox notification (from the notifications alone: it stays unread in Linear and in the Inbox view). Whatever is picked dismisses it too; a suggestion gets its own choices instead. It is there while the sidebar's notifications are on.

**Work map** is every piece of work in the repositories your tabs are in, a row each: each worktree, and a main checkout a tab is in, with its ticket (from the branch, while Linear is on), the tab it is open in or "no tab", its pull request and where that stands ("no PR", "#12 open", "checks running", "✗2 failing", "in merge queue at 3", "merged"), and its directory. Worktrees no tab has open are there too, from every repository a Claude session worked in over the last two weeks, each with when it was last used and the last prompt given there. Every open tab has a row, one in no repository included. Rows needing you come first, then work going on, then tabs, then merged work, then work no tab is in, the most recently used first. Merged work no tab is in sits last under "Merged, can be removed", closed until Enter on that heading opens it; a worktree's pull request is asked for by its branch, so work with no tab shows where its PR stands too. On a row with a tab, Enter's menu goes to it, or opens its diff (`M-d`) or lazygit in its pane's place; on a worktree with no tab it offers to open a tab there and resume its Claude (as `switchboard claude-here` does), its diff or lazygit in a tab of their own there, which closes with them, or copying its directory, and on merged work, to remove the worktree after asking, which keeps one with changes and says why. The worktrees are read with git about once a minute while the view is in front. Settings' "Work map" (`view_work_map`); `M-j` and `M-o` open it whatever that says.

**Calendar** and **Slack** are in "Your day"; **Closed**, **Sessions**, **Resources** and **Usage** are in "How you use it".

**PRs** is a board of your pull requests in the repositories your tabs are in, in columns: Draft, In review, Approved, In merge queue, and Merged in the last 7 days, each with its checks, review and place in the queue, failing checks or changes requested marked as needing you. `h` `l` move between columns. Enter offers to go to the tab it is open in, work on it in a new tab (`wtpr`), read it here in a pane beside the one you were in, open it in the browser, or copy its link. Settings' "PR board" (`pr_board`).

**CI** is your open pull requests whose checks are failing, in the repositories your tabs are in, each with the checks that failed and the line of the first failing job's log that says why. Enter offers to start a CI loop on it (typing `/ci-loop` into the tab already on it when its Claude is idle or finished, else opening a new tab on its worktree with `wtpr`), go to its tab, read the failing job's log in a pane beside yours (the log of each failing job is kept in `ci-logs/` in the state directory once the view has read why it failed, and goes when the job no longer fails; it opens at its end, without the timestamps Actions puts on every line), open the failing check in the browser, read the pull request here in a pane beside the one you were in or open it in the browser, or copy its link. Settings' "CI" (`view_ci`).

**Merge queues** is every pull request in the merge queue of each repository your tabs are in, everyone's, in queue order: its place, number and title, the repository, where it stands, about how long until it merges as GitHub estimates it, its author, and the tab it is open in; one of your tabs' that can't merge is marked as needing you. A repository with no merge queue is left out. Enter offers to go to the tab, read the pull request here in a pane beside the one you were in, open it or the queue in the browser, or copy its link. Settings' "Merge queues" (`view_merge_queues`).

**Tickets** is a board of the Linear issues assigned to you, in columns: Todo (urgent first), In progress, In review (a started state whose name says review), and Done in the last 7 days, each with its state, priority, project and the tab working on it, an urgent one marked as needing you. `h` `l` move between columns. Enter offers to go to the tab it is open in, start work on it in a new tab (`wt`), shape it in a new tab (`shape`), close it in Linear (moving it to its team's first completed state, after asking), read it here in a pane beside the one you were in, open it in the browser, or copy its ID. A new tab starts in the directory of a tab working on a ticket of the same team, else your home directory. Settings' "Ticket board" (`ticket_board`).

**Inbox** is your unread Linear notifications that need you, newest first: a mention of you in a ticket or a comment, a new comment on a ticket of yours, a ticket newly assigned to you; status changes and the rest of Linear's inbox are left out. Each is in the sidebar's notifications too. Enter offers what the Tickets board offers for its ticket, and to mark it read in Linear. Settings' "Inbox" (`view_inbox`).

**Settings** is below, and each plugin's view comes last (see "Plugins").

Settings' "GitHub" (`github`) off takes out the PRs, CI and Merge queues views, and Settings' "Linear" (`linear`) off the Tickets and Inbox views.

### When a view isn't set up

A view that is on is always there to go to, and before what it reads from is set up it says what is missing and how to set it up. GitHub's views need a token and Linear's a key: "GitHub and Linear access", under "Reading beside your work", says where each is looked for and how to store one, and mission control picks a stored one up within two minutes. A view whose tabs are in no GitHub repository says to open a tab in one. A look that fails for another reason (the network, a rate limit) keeps what the view had, with the error above it.

### How often mission control looks

GitHub and Linear are asked every 15 seconds, and the PR board every 30, with Settings' "Look often" (`poll_fast`) on; off, every tab's pull request and the merge queues are asked every minute and the rest every two minutes. The merged column of the PR board is asked for only every ten minutes. A lookup still waiting on its answer when the next is due is skipped rather than started twice. Going to the Inbox, CI or Merge queues view asks again at once, and so does `M-m`, for the views that show pull requests, so they agree with what it says.

While you are away (no terminal attached, the screen locked, or the computer idle for 10 minutes; where the desktop can't say, 10 minutes since a key was pressed in Switchboard) the GitHub and Linear lookups, the calendar feed, the Slack look and the diff's stories told ahead pause, and all ask again when you are back. Settings' "Pause lookups while away" (`pause_when_away`) off keeps them going. Working a note continuously opens no tabs while you are away, whatever that setting says.

### Finding a tab or a worktree

`M-j` opens the Work map showing only the open tabs, and `M-o` only the worktrees no tab has open (and the merged ones under their heading), each with a filter already typing: the arrows, `C-n` `C-p` and Tab move, Tab going round past either end (stopping on easy mode's × on the way, which Enter closes), Enter opens the chosen row's menu, its usual action first, so Enter twice goes to the tab or opens one in the worktree and resumes its Claude, and Esc closes the menu, then the picker. The filter types on while a menu is open over it.

### Settings

"Settings: turn the workspace's options on and off" in `M-p` opens mission control on its Settings view: every option under its category, with its value and what it does. Enter or Space changes the chosen one (on and off, or the next of its choices), and the change takes effect at once. `/` filters by name, description, category, key or value, leaving out the categories with nothing that matches. What you change is kept in `~/.config/switchboard/settings.json`, only what differs from the defaults; a change made there by hand takes effect too, and the version before each change is kept beside it as `settings.json.bak`.

Below the options, each setting kept in a file of your own in `~/.config/switchboard` has a row under that file's name: where your Claude rules come from and the file in your rules repository that holds them (`claude.json`), your Linear workspace, team and labels (`linear.json`), the Chrome profile each site's links open in and the default profile (`browser.json`), the Slack view's channels (`slack-updates.json`), the Slack relay's owner (`slack-relay.json`), the on-call settings (`oncall.json`), and the sidebar's sizes (`sidebar.json`). Enter on one types a value in, starting from what it is (empty unsets it); a Chrome profile is picked from the profiles Chrome lists. "Your CLAUDE.md" offers the local file (`~/.config/switchboard/CLAUDE.md`), another file or a git repository, the last two typed in; it writes `claude.json` and runs `switchboard setup claude-md`, which clones a repository named and writes `~/.claude/CLAUDE.md` to import the rules, and says what it changed or why it couldn't. Saving "Rules file in the repo" runs that part too. Anything bigger (your CLAUDE.md rules, your key bindings in `keys.json`, your trusted directories, the plugins, the on-call channels and rules) opens the whole file in your editor (`$VISUAL`, else `$EDITOR`, else `vi`) in a tab of its own.

## Your day

Switchboard keeps your day beside your tabs: today's calendar, what is new for you on Slack, the next meeting, and notes it writes for you. The notes go in your notes directory (`$NOTES_DIR`, else `~/Notes`), in `Standups/`, `Todos/` and `Meetings/`. A note already there is never written over, so each is yours to edit once written.

### The Calendar view

Mission control's Calendar view is today, laid out as Google Calendar's Day view lays it out: the date and when it was read at the top, the all-day events as chips in a row of their own, then the hours down the left, each event a blue block from its start to its end with its title and times, events at the same time side by side, a red line where it is now, events already over faded, and one you haven't said yes to drawn in outline. Two meetings back to back in the same place take different shades, so they read as two. The day runs from 8 AM (or the first event, or now) to 6 PM (or the last event), as many rows to an hour as the screen has room for, up to eight; a day too long for the screen starts an hour before now. `j` and `k` go through the events (all-day ones first, then by start), the chosen one in a lighter blue, and a click chooses one. It reads your Google Calendar's iCal feed itself, every five minutes and when you go to the view, once its secret address is stored: in Google Calendar, open Settings, your calendar, Integrate calendar, and copy "Secret address in iCal format", then run

```sh
switchboard platform secret-set google-calendar-ical
```

paste it and press `Ctrl-D`. It is stored as the GitHub token is (see "When a view isn't set up").

Without the address, the view shows the day the `/today` skill last saved, which reads your calendar through Claude's Google Calendar connector (connected with `/mcp` in Claude Code); run `/today` once in any Claude tab to start. A day saved before today, or over 45 minutes ago, is marked out of date, and while no tab named Calendar runs Claude and none loops `/today`, mission control opens one in the background, in your home directory, running `/loop 30m /today`, at most every ten minutes. If the feed can't be read for 15 minutes (no network, Google down, the address revoked), the view falls back to `/today`'s day in the same way, and goes back to the feed once it reads again. Settings' "Calendar" (`view_calendar`).

`j` and `k` walk every event in turn, and `h` and `l` move between those side by side at the same time, or along the all-day ones. Enter on an event offers to join its call, open it in Google Calendar, read each ticket and pull request its description links to in a pane beside the one you were in, and answer an invitation: yes, maybe or no, through Claude's Google Calendar connector, as Google's own buttons answer it, so the organizer is told. Answering needs the feed, since only its events carry their Google ID (Settings' "RSVP from the Calendar", `calendar_rsvp`).

What changed on today's calendar since you last looked (an event added, moved, renamed or gone) is in the sidebar's notifications ("Calendar: Standup moved to 3pm", or "2 calendar changes", each one in the Notifications window). A key pressed with the Calendar view in front and unfiltered, or dismissing one, takes the day as seen and clears them. They need the Calendar view on (Settings' "Calendar changes", `calendar_changes`).

From five minutes before a meeting until ten minutes after it starts, the notifications show it with the tickets and pull requests it names, and Enter there joins its call (Settings' "Next meeting", `next_meeting`).

### The Slack view

Mission control's Slack view is what is new for you on Slack in the last day, newest first: direct messages and mentions, marked as needing you, then replies to threads you started and everything in the channels you list in `~/.config/switchboard/slack-updates.json`:

```json
{ "channels": ["C0123456789"] }
```

With Settings' "Slack without a tab" (`slack_background`) on, mission control looks itself every minute while you are at the computer, and when you go to the view: one search through Claude's Slack connector (connected with `/mcp` in Claude Code), run by a headless Haiku allowed that one tool, reading only and keeping no session. The first look goes back an hour. When the connector isn't connected, the view says so first. With it off, the `/slack-updates` skill looks instead, in a Slack tab looping it every 5 minutes, which going to the view opens when no tab keeps it, or asks to look at once when its last look is over a minute old; a look over 15 minutes old is marked out of date, and mission control opens the Slack tab in the background to catch up. The Slack relay's own messages are never in it. Settings' "Slack" (`view_slack`).

Slack's connector can't say what you have read, so the view counts as read anything before a message of your own in the same DM or thread, and remembers what you have seen in it: leaving the view marks what its look brought as seen, and picking anything from a message's menu marks that message seen. Seen messages sit below the new ones, faded. New messages are in the sidebar's notifications too, one each. Enter shows a message whole above its menu, which offers to open it in Slack, open a tab for its thread with Claude (in a directory you pick, as `M-t` asks, at the end of the tabs, Claude told to read the thread through the Slack connector and say what it asks of you, drafting no reply unless you ask), or dismiss it, or every message shown, until something newer comes in its thread.

### Meeting notes

With Settings' "Meeting notes" (`meeting_notes`) on, each meeting of yours that Fathom recorded becomes a note, `Meetings/<title> <date>.md`, with its summary, its action items (yours first) and its transcript, its timestamps linking back to the recording. Fathom is reached through Claude's Fathom connector, a headless Haiku allowed one call at a time: a look every 30 minutes from 7am to 9pm, and every five minutes for the half hour after a meeting on your calendar ends, then a summary and a transcript for each new meeting. A meeting is yours when you recorded it or were invited, by your Fathom email or your git name. One whose summary isn't ready yet is tried again on the next look.

### The standup note

With Settings' "Standup note" (`standup_note`) on, each weekday, once Switchboard runs after 6am, it writes `Standups/Standup <date>.md` from the last working day: the pull requests you merged, opened or worked on and the ones you reviewed, the Linear tickets assigned to you or made by you that changed state, and the commits you made in the repositories your tabs are in; then what is in progress today, the reviews asked of you, and an empty Blockers list for you to fill. GitHub's part needs GitHub on, and Linear's, Linear. With Settings' "Standup summary" (`standup_summary`) on, a few sentences over the list sum it up, written by Claude (Haiku, no tools) from the list alone; the note is written without them when Claude can't. A note that fails is tried again every 15 minutes.

### Today's todo note

With Settings' "Today's todo note" (`todo_note`) on, each weekday, once Switchboard runs after the hour Settings' "Todo note time" (`todo_note_time`, 7am to 11am, 9am by default) names, Claude (Sonnet, no tools, no session kept) writes `Todos/Today's Todo <date>.md` from what is already on this computer: the unticked top-level checklist items of your notes (a section named Later, Deferred, Parked, Someday or Backlog left out, and the standup and todo notes themselves), today's calendar, and, for each that is set up and on, what mission control last read of GitHub (your pull requests and their failing checks, and the reviews asked of you), Linear (your tickets and Inbox) and Slack (the messages that need you). It waits until each of those has answered today, or 30 minutes, naming one that didn't as left out, and waits for you to be at the computer. It is off by default because it sends your notes' unticked items to Claude.

### New notes in the notifications

Each note written for you (a meeting's, the standup, today's todo) is in the sidebar's notifications until Notes shows it, its file is gone, or its menu in the Notifications view dismisses it. Enter on one opens it in Notes, through the running app's socket (or as the note it opens with when it isn't running), and goes to Notes wherever it is, or opens it in a tab of its own (Settings' "New notes", `new_notes`).

## How you use it

### Suggestions from how you work

Switchboard sends nothing about how you use it anywhere. With Settings' "Suggestions from how you work" (`habits`) on, it counts on this computer the keys you press (the ones pressed alone, but `C-w`), the commands you pick in `M-p` (a command with a key counting as that key pressed) and the mission control views you bring to the front, written once a minute to a file a day in `~/.local/state/switchboard/habits/` and kept 60 days. From the counts it suggests, in the sidebar's notifications:

- a key, for a command you picked from `M-p` three times in 14 days;
- turning off a view you haven't opened in 30 days (once 30 days are counted), which for the views that ask GitHub or Linear also stops what they look up;
- turning on a feature that is off, when what you did this week would use it: the second status line after you looked up a tab's PR or ticket 10 times, today's todo note after you went to Notes 10 times.

A suggestion's menu in mission control's Notifications view makes the change ("Turn it on", "Turn it off", or "Got it" for a key), puts it off for 14 days ("Not now"), or stops suggesting it ("Don't suggest it again"). Turning the setting off removes what was counted. `M-p`'s "How you use Switchboard" shows the counts themselves, and the suggestions now, in a box.

### Resources

Mission control's Resources view is how much CPU and memory each tab uses, counting every process under its panes (Claude, a dev server, an editor and its language servers, a test run), as a table: a row per tab with its % CPU (100% is one core), resident memory and number of processes, and a row for all the tabs together at the foot. Tabs are ranked by how heavy they are overall, CPU and memory each counted against what marks a tab as heavy (a whole core, 4 GB) and added, so a pegged core ranks with 4 GB. The chosen row adds its three heaviest kinds of process, which `/` filters by too, and Enter offers to go to the tab. It is read from the kernel every three seconds while the view is in front, and never otherwise. Settings' "Resources" (`view_resources`).

### Sessions and Usage

The Sessions view lists each Claude tab's session with how full its context is, its model, how long it has run and how long since it last did anything, the fullest first, one at 80% or more marked as needing you: the ones to compact or close. Enter offers to go to the tab. Settings' "Sessions" (`view_sessions`).

The Usage view is what Claude has cost today, at API prices as Claude Code estimates it, across the interactive sessions that ran today (a headless `claude -p` isn't counted), and how much of each rate limit is used (the 5-hour and weekly limits, or a spend limit) with when it resets, one at 80% or more marked as needing you. Settings' "Usage" (`usage`).

Claude Code gives the context, the model, the cost and the limits only to its statusline, which `switchboard setup` makes `switchboard statusline`. It writes them to `~/.local/state/switchboard/context/` and `~/.local/state/switchboard/usage/` as each session draws, so a session shows in these views once its statusline has drawn, and the limits are as the last session to draw saw them.

### The request audit

Every request Switchboard makes to a service outside this computer is counted as it is made, a line each (when, the service, the protocol and what made it, tab-separated) in `~/.local/state/switchboard/requests/<day>.tsv`, a file a day, kept 14 days. Below its spend and limits, the Usage view lists today's: the total by protocol (GraphQL and REST over HTTPS, WebSocket, iCal, MCP, `claude -p`, git, plain HTTPS), then each service (GitHub, Linear, Slack, Google Calendar, Anthropic, Fathom and the rest) with its own protocols, busiest first. A Claude connector call counts twice, as the MCP call to its service and as the `claude -p` run that makes it. What Claude Code sessions reach themselves (their model and connectors) and skills' scripts aren't counted. Settings' "Requests in Usage" (`request_audit`) leaves them out of the view.

### Closed tabs

Mission control's Closed view is the tabs closed most recently, newest first, one per Claude conversation, so several closed in the same repository are each there, leaving out a conversation a tab has open again; a tab with no Claude is listed once per directory no tab is open in, and a tab that came and went within ten seconds isn't listed. Enter offers to reopen it where it was, with the name it had and its Claude resumed (`claude --resume`), or a shell when it had none; to remove it from the list, which brings it back only if that conversation is closed again; or to copy its directory. The newest 200 are kept in `~/.local/state/switchboard/closed.jsonl`, so a restart doesn't lose them. Settings' "Closed tabs" (`view_closed`).

### Plugins

Anything with an API can be a view of its own in mission control: a plugin fetches a list of items, and Enter on one opens its link. A plugin is an executable in any language (no longer a JavaScript module), `~/.config/switchboard/sidebar-plugins/<name>` (a name of lowercase letters, digits and dashes), and only the plugins named under `plugins` in `~/.config/switchboard/sidebar.json` load, each with the options given under its name (`true` for none, `false` to leave it out):

```json
{
  "plugins": {
    "deploys": { "url": "https://deploys.example.com/api", "intervalSeconds": 120 },
    "github": { "query": "org:your-org" }
  }
}
```

It is run every `intervalSeconds` (five minutes when nothing says), never through a shell, with its options as JSON on its input, and prints its items as JSON: a list, or an object with `items` and, if it likes, `title`, `intervalSeconds` and `sidebar`, which the config's own win over. `sidebar` is how many of its items the sidebar lists under the tabs (none when nothing says): its title in capitals with how many items it has and how many need you ("DEPLOYS 3 ! 1"), its error when its last run failed, and its first items, each marked by its state. Each item has a `title`, and may have an `id`, a `detail` line, a `state` (`attention` marks it as needing you, `active`, or `idle`), a `url` (an `http` or `https` link, which Enter opens) and a `time` in milliseconds:

```python
#!/usr/bin/env python3
import json, subprocess, sys, urllib.request

options = json.load(sys.stdin)
token = subprocess.run(["switchboard", "platform", "secret", "deploys-token"],
                       capture_output=True, text=True).stdout.strip()
request = urllib.request.Request(options["url"], headers={"Authorization": f"Bearer {token}"})
deploys = json.load(urllib.request.urlopen(request))
print(json.dumps({"title": "Deploys", "items": [
    {"id": d["id"], "title": d["service"], "detail": d["status"],
     "state": "attention" if d["status"] == "failed" else "active",
     "url": d["link"]} for d in deploys]}))
```

`switchboard platform secret <service>` prints a secret stored with `switchboard platform secret-set <service>`, so no token sits in the config. A run that takes over 30 seconds is killed, and one that fails keeps the items the last good one gave, with its error's first line above them. `r` in a plugin's view asks it again at once. The config is read again whenever it changes.

`github`, built in, lists your open pull requests and the ones waiting on your review, with the GitHub token mission control uses, marking as needing you a review requested of you, or a pull request of yours whose checks failed or that has changes requested; drafts are idle. Its `query` option adds search terms to both searches, such as `org:your-org`.

## Reading beside your work

The branch's pull request, its Linear ticket and any link can be read in a pane split off to the right of the one you are in, rather than in the browser; `M-v` and `M-i` close their view again from anywhere in the tab. Each view is a program of Switchboard's own (`switchboard pr-view`, `ticket-view`, `link-view`), drawn and paged the same way, so each can also be run by hand.

### The PR description

`M-v` shows the branch's pull request in a pane to the right, taking 45% of the width: its title and number, a line saying whose branch goes into which and whether it is open, a draft, merged or closed, then its description. The PR is found from the pane's branch (a fork's branch by the branch it tracks, a `wtpr` worktree by its number), and a merged or closed one still counts. The copy last read for the branch shows at once, and the PR is read again in the background; when it has changed, the view is laid out again where you were. `M-v` again closes it. With no PR to show, the view says why: not a git repository, no `origin` on GitHub, no GitHub token, or no PR for the branch yet ("Push it and open one, then M-v again").

`M-p`'s "View the branch's PR description as an image" draws the description as GitHub draws it (its HTML, GitHub's stylesheet, Mermaid diagrams drawn) and shows it a page at a time as pictures in place of the pane: `j`/`k` or Space between pages, "Page 2 of 3" and "↓ more" in the corner, `q` (or the command again) giving the pane back. "View the branch's Linear ticket as an image" does the same for the ticket. Both need Chrome or Chromium, which draws the page (the view says so when neither is installed), and a terminal that shows kitty's pictures (Ghostty or kitty).

### The Linear ticket

`M-i` does the same for the branch's Linear ticket. The ticket's ID comes from the pane's branch, the first thing in it shaped like a ticket ID in any case (`abc-123-fix-login` gives ABC-123); with none there, from the tab's name (a "Shaping ABC-123: ..." tab before there is a branch, or a name you gave it). The view shows the ticket's title, the facts from its side panel (state, priority, assignee, project, cycle, estimate, labels), who opened it and when, when it was last updated and its parent, then the description, sub-issues, related tickets and links, and the comments, oldest first, each reply under the comment it answers. A bare link to another ticket reads as that ticket's ID. As with the PR, the copy last read shows at once while the ticket is read again. With no ticket in the branch or the tab's name, `M-i` says so.

### In the pager

Every view draws its Markdown for the terminal: headings without their `#`s, tables, task lists and code blocks, a link as its text alone, an image as a line naming it, and the HTML a PR template leaves read for what it says. A click on a link, or an image's line, opens it in the browser. A line too wide for the pane is cut at its edge rather than wrapped, so a drawing keeps its shape; Left and Right scroll across. Resizing the pane lays the text out again at the new width, opened at the line that was at the top.

Mermaid diagrams are drawn as text: flowcharts (`flowchart` or `graph`, in any direction) and sequence diagrams. A left-to-right flowchart wider than the pane is drawn top to bottom instead, and one wider still is left whole, for Left and Right to scroll. Any other kind of diagram keeps its source, with a line saying where it is drawn properly (`M-r` on GitHub, `M-l` in Linear, the page in the browser).

| Key | What it does |
|---|---|
| `j` `k`, `↓` `↑`, `Enter`, `e` `y`, `Ctrl-n` `Ctrl-p` | A line down or up |
| `Space` `f` `b`, `PageDown` `PageUp`, `Ctrl-f` `Ctrl-b` | A screen down or up |
| `d` `u`, `Ctrl-d` `Ctrl-u` | Half a screen down or up |
| `g` `G`, `Home` `End`, `<` `>` | The top or the end ("(END)" shows at the bottom there) |
| `←` `→` | Half a screen across |
| `/` | Search, ignoring case unless what you type has a capital; `Enter` finds it, `Esc` cancels |
| `n` `N` | The next match, or the one before |
| `o` | The page being read, in the browser (the link view) |
| `q`, `Esc` | Close (`Ctrl-c` does nothing, as in less, so one meant for the pane beside it is lost harmlessly) |

The wheel scrolls three lines at a time.

### Opening the PR and the ticket in the browser

`M-r` opens the branch's pull request in the browser. A branch with no PR yet opens its repository instead, and a box says why. `M-p`'s "Open the repository in the browser" opens the repository whatever the branch. When the PR or the repository can't be opened, a box says why: the directory isn't in a git repository, the repository has no `origin` remote or one that isn't on GitHub, there is no GitHub token or GitHub refused it, or GitHub doesn't have the repository or won't show it to your token.

`M-l` opens the branch's Linear ticket, found as `M-i` finds it. With your workspace written in `linear.json` (below), the ticket's address is made from it at once; without, Linear is asked where the ticket is. A box says when there is no ticket in the branch or tab name, when Linear has no such ticket, or why Linear couldn't be asked.

With GitHub turned off in Settings (Integrations, `github`), `M-v`, `M-r`, `M-m` and "Open the repository in the browser" do nothing but show a tip saying so, the first time; with Linear off (`linear`), `M-i` and `M-l` do the same.

### Whether the PR can merge

`M-m` asks GitHub whether the branch's PR is ready to merge and answers in a box, "Reaching out to GitHub..." until it knows. A merged or closed PR says so and stops there, and so does one in the merge queue, with its place and where it stands (waiting on the queue's checks, passed and waiting its turn, merging now, or unable to merge and about to be removed). Otherwise the verdict is GitHub's own, so it agrees with the merge button: ready to merge (noting merge hooks, or optional checks not passing), or not ready because the branch is behind its base, a required review or check blocks it, it has conflicts, or it is still a draft. When GitHub hasn't worked out yet whether it can merge, it is asked again, up to three times a second apart. Under the verdict come the lines that explain it: the required checks, all checks counted by outcome, the review, conflicts, whether it is a draft, and auto-merge.

The box ends with what can be done about the answer. With a check failing, "See the failing checks" opens a pane beside yours listing each one with its description and link and, for a GitHub Actions job, the last 25 lines of its log. With conflicts, "Type /resolve-conflicts into this tab's Claude" types it into the tab's Claude session and moves you there to press Enter, since what is in its prompt already can't be seen from the box. `j` and `k` (or the arrows, Tab, the wheel) move between them, Enter, Space or a click takes one, and Esc or `q` closes the box; with nothing to do, any key closes it. Each `M-m` also has mission control's PR views look again, so they agree with what it said.

### Search everything

`M-F` (Alt-Shift-F) asks what to search for, then searches your notes, Linear, GitHub and Slack at once. Each source's results appear as it answers, up to 20 each, the title saying which are still looking, and the rows stay in the sources' order, so the list doesn't shift under the cursor when a slow one arrives. Typing narrows them. Enter reads the chosen one beside the pane you searched from, in the link view: a note as its Markdown, a pull request as `M-v` lays one out, a ticket as `M-i` does. A Slack message can't be read there, so Enter opens it in the browser. Ctrl-O opens the chosen one in the browser instead, whatever it is but a note.

- **Notes**: every Markdown note in `$NOTES_DIR`, else `~/Notes`, that holds every word you typed, newest first, with the line where the first word appears. Hidden folders, the trash among them, are skipped. The notes are read on the computer and sent nowhere.
- **Linear**: Linear's issue search.
- **GitHub**: issues and pull requests in the repositories your tabs are in.
- **Slack**: Slack's own search, through Claude's connector (below), arriving about 15 seconds after the rest.

A source that fails shows a row saying why, which reads nothing when chosen. GitHub and Linear are left out while their settings are off. Settings' "Slack in search everything" (`search_slack`) leaves Slack out, and "Search everything" (`search_everything`), both under Integrations, turns the key off.

### Reading a link

`M-p`'s "Read a link in a pane beside this one, typed or one this pane shows" lists the links the pane shows and has scrolled past, newest first, each once; a link a program wrote into its text (an OSC 8 hyperlink, as Claude Code writes them) shows that text before its address. In the notes pane the list is the links of the note it shows, top to bottom, since Notes shows a link's text alone. Type to narrow the list, or type or paste a link of your own; `M-Enter` takes what you typed even when a listed link matches it. Enter reads the link in a pane to the right, half the width, in place of any link read in that tab before, and `Ctrl-O` opens it in the browser instead. What the link is decides how it is read:

- a GitHub pull request, as `M-v` lays one out;
- a Linear ticket, as `M-i` lays one out;
- a Notion page, or a Google Doc or Slides deck, through Claude's connectors, as the connector gives it, in about 15 seconds. Settings' "Docs in the link view" (`link_view_docs`, Integrations) turns this off, and such a link is then read as any other page, which a private one won't allow;
- a note, a `file://` link that search everything gives, read only from the notes directory;
- a Slack message, which can't be read without Slack's connector, so its pane offers only to open it: `o` opens it, any other key closes;
- any other web page, fetched, its main text turned into Markdown under its title and address.

`o` in the link view opens the page in the browser, and `M-p`'s "Open the link being read beside this pane in the browser" does the same from any pane in the tab. `q` closes it. A page that can't be read says why: `o` then opens it in the browser and closes the pane, and any other key closes it.

The same list does two more things. "Open a link from this pane in the browser, or read it in a pane beside" opens the chosen link in the browser, `Ctrl-O` reading it beside instead; "Copy a link from this pane to the clipboard, or open it in the browser" copies it (to your terminal's clipboard and the computer's, as a selection is), `Ctrl-O` opening it. With no links in the pane, each says so.

"Open an image or video from this pane in its app, or show it in its folder" lists the pictures and videos the pane names, newest first: a file's path (absolute, from `~`, or from the pane's directory, listed only when the file is there; png, jpg, gif, webp, heic, svg, bmp, tiff, mp4, mov, webm, mkv, m4v and avi), then the links among the pane's whose address ends in one. Enter opens a file in the app the desktop opens it with (Preview or QuickTime on a Mac) and a link in the browser; `Ctrl-O` shows a file in Finder, or opens its folder on Linux.

Mission control's menus read a link beside your work too: "Read it here" on a pull request or ticket on the boards and in the Inbox, "Read the PR here" in the CI and Merge queues views, and an event's linked tickets and pull requests in the Calendar view, each in a pane beside the one you were in when mission control opened.

### The views by hand

```
switchboard view notes.md               # page a Markdown file, drawn as the views draw one
switchboard view --log ci.log           # page a CI job's log from its end
switchboard pr-view [dir]               # the branch's PR description
switchboard pr-view --checks [dir]      # the PR's failing checks, as M-m shows them
switchboard ticket-view [dir | ABC-123] # a ticket, by ID or from a directory's branch
switchboard pr-image [dir]              # the branch's PR description as GitHub draws it, as images
switchboard pr-image --ticket ABC-123   # a Linear ticket the same way
switchboard link-view <url>             # what a link holds
```

Each runs in the terminal it is started in, the directory defaulting to the one you are in, and takes the pager's keys. `pr-image` has Chrome draw the page and shows it a page at a time as pictures (`j`, `k`, Space and `b` page, `g` and `G` go to the ends, `r` draws it again, `q` quits), through kitty's graphics protocol, so it needs Chrome and a terminal that draws pictures (Ghostty, kitty); inside the workspace only a pane allowed to send pictures shows them, which is why `M-p`'s "View the branch's PR description as an image" and "View the branch's Linear ticket as an image" open it in the pane's place.

### Where links open

Every link Switchboard opens, from mission control, the views, `M-r` or `M-l`, goes to the Chrome profile `~/.config/switchboard/browser.json` gives its site, else in that file's default profile, else in your default browser. The file is yours and is never in a repository:

```json
{ "hosts": { "linear.app": "Profile 2", "slack.com": "Profile 2" }, "default": "Profile 1" }
```

A site in `hosts` matches itself and its subdomains, the longest match winning. A profile is written as the folder Chrome keeps it in, as `chrome://version` shows it ("Profile 2"), or as the name Chrome shows for it ("Work"). Settings lists `hosts` and `default` under "Browser (browser.json)", as Chrome profiles and Default Chrome profile, the default picked from Chrome's own profiles. On macOS the link opens with `open -na "Google Chrome"`; on Linux the first of `google-chrome` and Chromium that is installed is run, with the profile. Where neither is installed, the link opens in the default browser (through `xdg-open` on Linux). Settings' "Chrome profiles" (`chrome_profiles`, Integrations) turned off opens every link in the default browser.

`switchboard open-url <url>` opens a link the same way, for scripts and skills, and `switchboard open-url --as <site> <url>` in the profile `<site>` would get (as the AWS login opens AWS's pages in `awsapps.com`'s). Every pane's environment has `GH_BROWSER` set to `switchboard open-url`, so gh's `--web` (`gh pr view --web` and the rest) opens the same way from any pane; outside the workspace, set it yourself.

### GitHub and Linear access

Switchboard talks to GitHub and Linear over their own APIs, so nothing else has to be installed. Each finds its credential where you already keep one, the first that answers:

- **GitHub**: `GH_TOKEN`, `GITHUB_TOKEN`, the token of the `gh` command when it is installed and signed in, else the stored secret `github-token`.
- **Linear**: `LINEAR_API_KEY`, the stored secret `linear-api-key`, else `linearApiKey` in `~/.config/switchboard/linear.json`. Make a key in Linear under Settings, Security & access, Personal API keys.

A credential from `gh`, a secret or the file is kept for ten minutes and forgotten as soon as the service refuses it, so a new one is used on the next request. To store a secret, run the command and paste it, then press `Ctrl-d`:

```
switchboard platform secret-set github-token
switchboard platform secret-set linear-api-key
```

It reads the secret from its input, never from an argument, which other programs could see. On macOS it goes in the Keychain, as a generic password whose service is `github-token` or `linear-api-key` (one you added there yourself under that service is read too). On Linux it goes in the desktop's keyring through the Secret Service (GNOME Keyring, KWallet), as an item whose `service` attribute is that name; on a computer with no keyring, such as a server or WSL, it goes in `~/.config/switchboard/secrets.json`, which only you can read. `switchboard platform secret <name>` prints one back.

`~/.config/switchboard/linear.json` also holds what Switchboard and its skills need to know about your Linear, each listed in Settings under "Linear (linear.json)":

```json
{ "linearWorkspace": "your-workspace", "linearTeam": "ABC", "linearLabels": ["bug"] }
```

`linearWorkspace` is your workspace's name in its addresses (`linear.app/your-workspace`), which `M-l` makes the ticket's link from (`LINEAR_WORKSPACE` in the environment wins over it). `linearTeam` and `linearLabels` are the team key and the labels `/shape` and `/note-tickets` file new tickets with.

### Claude's connectors

What Switchboard can only reach through Claude's connectors (a Notion page or Google Doc in the link view, search everything's Slack) it asks of Claude Code: one headless Haiku, allowed only the one connector tool it needs, keeping no session, its answer taken from the tool's own result rather than from what Haiku writes. Claude Code must be installed and signed in, with the connector connected to your Claude account. A connector that isn't connected, or whose sign-in has lapsed, fails with a message saying what to do ("Notion isn't connected in Claude: run /mcp in Claude and sign in to Notion."), shown where the feature shows (the link view, search's Slack row), and the health light in the status bar names it ("Notion not connected") for two hours after, or until the connector answers again (Settings' "Health light", `health_light`). How each connector last answered is kept in `~/.local/state/switchboard/connectors.json`.

## The diff

`M-d` shows the branch's changes in place of the pane you are in, as GitHub's "Files changed" shows them: the changed files in reading order on the left, the chosen file side by side on the right. The rest of the tab stays as it is. The pane you were in waits, its program still running, and takes its place back when the diff closes: `M-d` again, `q`, or `M-w` on the diff's pane. The first time, a tip says so. Outside a git repository, `M-d` says so instead.

### What it compares

The diff is the branch's committed changes since it left its PR's base branch (`git diff <merge base> HEAD`); changes not yet committed aren't in it. The base is the PR's base branch, else `origin`'s default branch for a branch with no PR (or with GitHub off), else `main`. It is fetched from `origin` first, saying so on the diff's screen, so a branch rebased onto newer work isn't shown that work as its own; a branch still at the commit it was last compared at skips the fetch, since its merge base can't have moved. When the fetch fails, the diff compares with the base as last fetched and says so.

`M-p`'s "Open the changes not yet committed, staged and not, in place of this pane, or close them" shows the working tree's changes instead, in the same place and with the same keys: the changes not staged under "Changes", a file git doesn't know yet among them as added, and the staged ones under "Staged changes" below, as diffview listed them, a file staged and changed again since in both. A file not staged has the index on its left and the file on its right; a staged one has HEAD on its left and the index on its right. Its story reads the staged changes and then the rest, with no commit messages, and `yl` links to HEAD for a side GitHub can't have. `M-d` stays on the branch's commits, and closes either.

### The files and their changes

Each file in the list shows its status (A added, M modified, D deleted, R renamed), its name, its folder faintly, and its lines added and removed. The chosen file's old text is on the left and its new text on the right, each line beside the line it became and a line only one side has facing a gap. Lines are matched as a person reads them, so fewer look changed than a plain diff shows, and within a changed line only the words that changed are marked, in a stronger shade of git's green or red. The text has its language's syntax colours, in your terminal's own palette (comments its bright black, strings green, functions blue, keywords magenta, types yellow), which come a moment after a file opens. A long line wraps within its side, so a paragraph of Markdown on one line reads whole. Unchanged runs more than three lines from a change are folded into a "⋯ N unchanged lines" line. An added file is shown alone in a single pane, and so is a deleted one, as it was. Each file opens with the cursor on its first changed word. The bar over the new side names the file's path from the repository's root; the old side's names one only for a renamed file, which is where it was.

Settings' "Files in reading order" (`diff_order`, under Diff) lists the files flat in the order a reviewer reads them: documentation first (Markdown, `docs/`, READMEs, changelogs), then the database's changes (migrations, SQL, schema files), then the server's side of a web app (Next.js server actions, API routes, `*.server.ts`, `server/`), then the rest of the code, then tests, then generated files. Generated files are lockfiles, migration and test snapshots, minified files, source maps, and built or generated output; the diff opens on the first file that isn't one. When no story is told, Haiku orders the server's and the rest of the code's files from the most abstract to the most concrete, from their paths and line counts alone; until it answers, or if it can't, they are A to Z. Its answer is kept, so the same diff opens in that order at once. With the setting off, the list is A to Z.

### The story

With Settings' "Diff told as a story" (`diff_story`, which needs Files in reading order on) the list is told as a story. Haiku reads the whole diff, up to about 160,000 characters (files that fit kept whole, the rest sharing what is left, every change still named), and the commits' messages, and groups the changes into two to eight chapters, each one thing the change does, from its core to its edges, tests right after the code they test, generated files last whatever it says. This sends the diff's text and the commit messages to Claude. It needs Claude Code installed; a diff of one file, or of generated files only, has no story.

While Haiku reads, the diff waits under a cover, blank for its first moment so a story already known shows with no words flashing past. Then the cover shows how many files and lines it is reading, the time taken against about how long it should take, and each chapter as Haiku names it. `t` shows Haiku's answer itself as it comes, and `t` again the chapters; with Settings' "Diff story progress" (`diff_story_progress`) off, the cover shows only a spinner. `Esc` or `q` takes the cover down and shows the diff at once, in the kinds' order, with "telling the story" under the list's heading until the story comes; the list then takes the story's order and keeps the file you are on.

Each chapter heads its first file in the list with its number, title and a why of two or three sentences. The bar over the diff names the file's chapter before its path; in a list narrower than about 48 columns the heading keeps only the why's first sentence, and the whole why follows the path in that bar. A file sits in the chapter that holds most of its changed lines. With Settings' "Diff story notes on changes" (`diff_story_notes`) on, Haiku's note sits over each change worth one, saying what it does and what to check, and a change belonging to another chapter than its file's is headed with that chapter's number and title.

In the list, `]c` and `[c` go to the next chapter and the one before. `]p` opens the first chapter as a page, a diff of its own holding only the files that chapter touches, as if it were a commit, the list headed "Page 1 of N"; `]p` and `[p` turn the pages, and `[p` from the first goes back to the whole diff. On a page, the changes of other chapters in its files are dimmed. Settings' "Diff story a page at a time" (`diff_story_pages`) off makes `]p` and `[p` do nothing. With no story, `]c` in the list, `]p` and `[p` say why in a box: it is still being told, it is off in Settings, the diff changes only one file, or what went wrong when Haiku was asked. When Haiku's answer named changes the diff doesn't have, a box says the story may be thin.

`yS` copies the story as Markdown, to share: each chapter's number, title and why, its files, and each change's note under its file as `path:line`. `M-p`'s "Copy the diff's story (M-d) as Markdown, to share it" presses it, and is listed only while the diff is open.

A story is kept in Switchboard's cache (`~/.cache/switchboard/diff-story/`) under the diff's text, the commits' messages and the version of what Haiku is asked, so the same diff opens on its story at once and any change to it is a new one. Two diffs asked for the same story at once share one telling.

With Settings' "Diff story told ahead" (`diff_story_ahead`) on, when a tab's branch has a commit whose story isn't told yet, it is told in the background, so `M-d` opens on it at once; an `M-d` opened meanwhile follows that telling on its cover rather than asking again. It tells one branch at a time and each commit once, tries a failed one again after half an hour, and counts the default branch too, whose commits not yet pushed are its diff. It runs only with Files in reading order, Diff told as a story and GitHub on, and Claude Code installed, and sends each new commit's diff and messages to Haiku, a run taking up to five and a half minutes.

### The branch's commits one at a time

`c` goes through the branch's commits one at a time, oldest first, so the branch reads in the order it was written: each commit's own change against the commit before it, in the diff's place, the list headed with "Commit 2 of 5", its hash and its subject. `]p` goes to the next commit and `[p` to the one before; `c`, `q` or `Esc` in the list go back to the whole branch, where you were. A commit has no story of its own, and `ga` asks only about the whole branch. A working tree's changes have no commits to go through.

### Asking about the change

`ga`, in the list or the diff, asks Haiku a question about the change (Settings' "Ask Claude about the diff", `diff_ask`). A box opens in the middle of the diff to type it in: `Enter` asks, `Shift-Enter` or `Ctrl-j` starts a new line, a question pasted in keeps its line breaks, `←` `→`, `Home` `End` (`Ctrl-a` `Ctrl-e`), `Backspace` and `Delete` move and edit in it, `Ctrl-u` clears it, and `Esc`, `Ctrl-c`, Cancel or a click outside closes it. The answer streams into a window on the right half, titled with the question, each file Haiku opens named on a line of its own. Haiku is given the story, the diff as the story read it, where your cursor is (the file, the line and its change), and the question, and runs in the repository's top with only the tools to read, search and list files, so it can read the repository to answer and nothing outside it. It keeps no session: each question stands alone. This sends the diff and your question to Claude. `M-p`'s "Ask Claude about the diff (M-d), with its story, reading the repository as needed" presses `ga`, and is listed only while the diff is open.

| Key, in the answer | What it does |
|---|---|
| `j` `k`, `↓` `↑` | The line cursor down or up |
| `Ctrl-d` `Ctrl-u` | Half the window down or up |
| `gg` `G`, `Home` `End` | The first or last line |
| `gf` | The `path:line` on the cursor's line, in the diff, when the change has that file (else a box says it isn't changed, or isn't in the repository); a click on a `path:line` does the same |
| `ga` | A new question |
| `yy` | Copy the answer |
| `yS` | Copy the story |
| `q`, `Esc` | Close it, stopping Haiku if it is still answering |

Its Close, Ask again and Copy buttons do the same, the wheel scrolls it (the line cursor kept in sight), and a click outside it closes it.

### The pane beside its diff

`M-p`'s "Show the pane a tool stands in for (the diff, lazygit) beside it, on its left" brings the pane the diff stands in for back on the diff's left, half the width, so a Claude session and its diff sit side by side, and "Show a tool in place of a pane alone again, full width" puts it away again. Both are listed only where a tool stands in for a pane. `M-d` in either pane closes the diff, and the pane takes back its whole place. A restart saves the pane in the diff's place, so a Claude session waiting behind it is resumed.

### lazygit in a pane's place

`M-p`'s "Open lazygit in place of this pane, or quit it; quitting brings the pane back" runs [lazygit](https://github.com/jesseduffield/lazygit) in the pane's directory in its place, as `M-d` puts the diff there, the pane waiting behind it until lazygit quits. The command again, in lazygit or in the pane beside it, sends lazygit `Ctrl-c`, its own quit, rather than `q`, which would be typed into a commit message. lazygit is a program of its own, not part of Switchboard: where it isn't installed, the command says so. Mission control's menu on a tab offers it too.

### The file list

Settings' "Diff file list wide" (`diff_list_wide`) opens the list wide enough for its longest path, at most half the screen; `E` narrows it, and widens it again. From the list, `J` and `K` scroll the diff without leaving it. `yp` copies the cursor's `path:line` (on the side it is in, the new one from the list), and `yl` a GitHub link to that line at that side's commit, when `origin` is on GitHub. In a side of the diff, `v` or `V` starts selecting lines from the cursor, lit as the cursor moves; `yp` and `yl` then copy the lines selected (`path:12-20`, `#L12-L20`) and end the selection, as does `Esc` or `v` again. A click on a file shows it, a click in the diff puts the cursor there (opening a fold it lands on), and the wheel scrolls the list or the diff under it, the list past the file chosen, which a key brings back in sight.

With Settings' "Easy mode" (`easy_keys`, under Help) on, the diff has a row of buttons at the bottom, each pressing its keys in the list: ◀ Chapter, Chapter ▶, ◀ Page, Page ▶, Widen, Copy link, Ask (with Ask Claude about the diff on) and Close, followed, with "Key help line" (`key_help_line`) on, by a line naming the keys. With "Highlight on hover" (`hover_highlight`) on, the button under the mouse lights. Both take effect when the diff next opens.

### Keys in the diff

The keys depend on where the cursor is: in the file list, or in the diff on its right.

| Key | In the file list | In the diff |
|---|---|---|
| `j` `k`, `↓` `↑` | The next or previous file | The next or previous line |
| `Ctrl-d` `Ctrl-u` | Half a screen of files down or up | Half a screen down or up |
| `gg` `G`, `Home` `End` | The first or last file | The first or last line |
| `]c` `[c` | The next or previous chapter | The next or previous change |
| `Enter` | Into the diff | Open the fold under the cursor |
| `l`, `o`, `→` | Into the diff | `→`: from the old side to the new |
| `←` | | From the new side to the old, from the old to the list |
| `J` `K` | Scroll the diff five lines down or up | |
| `v` `V` | | Select lines from the cursor, for `yp` and `yl`; again to stop |
| `Esc` | Close the diff | End a selection, else back to the list |

These work wherever the cursor is:

| Key | What it does |
|---|---|
| `Ctrl-f` `Ctrl-b`, `PageDown` `PageUp` | The diff a screen down or up |
| `Ctrl-e` `Ctrl-y` | The diff scrolled a line down or up, the cursor kept in sight |
| `zo`, `za`, `zv` | Open the fold under the diff's cursor |
| `zR` `zM` | Open every fold of the file, or fold them again |
| `Tab`, `Shift-Tab` | Between the list and the diff |
| `Ctrl-w h` `j` `k` `l`, `M-Left` `M-Down` `M-Up` `M-Right` | The list or side of the diff that way, else the workspace's pane on that side (the sidebar past the list), as `M-Left` and the rest move between panes |
| `Ctrl-w w` | The next window of the diff |
| `]p` `[p` | The next or previous page of the story, or commit of the branch's |
| `c` | The branch's commits one at a time, and back to the whole branch |
| `E` | Widen or narrow the list |
| `yp` | Copy the cursor's `path:line`, or the lines selected (`path:12-20`) |
| `yl` | Copy a GitHub link to the cursor's line, or to the lines selected |
| `yS` | Copy the story as Markdown |
| `ga` | Ask Haiku about the change |
| `q`, `Ctrl-c` | Close the diff |
| A number | Repeats the move that follows (`5j`) |

Under the cover, `Esc` or `q` shows the diff and `t` switches between the chapters and Haiku's answer. While the change is still being read, `q` or `Esc` closes the diff, and when it can't be read, any key does. A message box in the diff closes on the next key, which does nothing else, or by itself after five seconds.

### The diff by hand

`M-d` runs `switchboard diff` in the pane's place, and it runs in any terminal too:

```
switchboard diff [dir]                  # the branch in dir (else here) against its PR's base, as M-d shows it
switchboard diff --base origin/develop  # against another branch or commit, used as given, nothing fetched
switchboard diff --working [dir]        # the changes not yet committed, staged and not
switchboard diff --ahead [dir]          # tell the story and keep it for M-d, drawing nothing
```

With `--ahead` it compares with the base `M-d` would, and exits 0 once the story is told (or there was none to tell), or 2 or 3 with why on its error output; a second run on the same commit does nothing.

## The notes app

The notes app keeps your notes as Markdown files and shows them in the terminal the way the macOS Notes app does, edited with Vim's keys.

```
switchboard notes
```

On the left is a list of folders, tags and notes; on the right, the note you had open when you last quit, or the newest when that one is gone. A note is a Markdown file, drawn the way Notes shows it: markup is hidden (it shows only on the line you are typing in), a title stands out from headings and subheadings, list markers show as • and –, checklist items as ☐ and ☑, a quote with a bar, a table with box lines, and pictures under their lines. The first run makes `~/Notes` with a welcome note showing everything a note can do.

Notes are kept in `$NOTES_DIR`, or `~/Notes`, one Markdown file each, in folders that are directories, nested as deep as you like. A note's title is its first line, and its file is named for it and renamed when the title changes. A note saves itself a moment after each change and whenever you leave it. Pinned notes and the note open last are listed in `.notes.json` there, and deleted notes go to `.trash/`, the Recently Deleted folder. Nothing else is stored, so any editor can open them.

One app runs on a notes directory at a time. While it runs it holds `.notes.lock` there, with its process ID: notes save themselves as you type, so a second app on the same notes would write over the first's. A second one says the notes are open already and closes with `q`. A lock left by an app that has gone is taken over.

A note changed from outside the app, by a skill, a script or another editor, shows the change while it is open, without a save or a keypress: a watch on the notes directory brings it in. If you have typed something not yet saved, the two are merged line by line, so changes to different lines, even next to each other, both stay; where both changed the same line, both versions are kept, yours first, and the app says so. An item ticked from outside moves under the ones left to do, as ticking it in the app does. A note deleted or moved outside the app while open isn't written back under its old name.

Nothing in a note is sent anywhere. Squashing notes, after you say yes, is the one exception in the app. The question of which note is a tab's ranks the notes on the computer, and a note's new tab tells Claude only the note's title and file. The `/note-tickets` and `/note-watch` skills read notes only when you start them, and only the notes you name or confirm.

### In the list

| Key | Does |
| --- | --- |
| Enter, or a click | open a note, or show a folder's or a tag's notes |
| `j` `k`, the arrows, `gg` `G`, Ctrl-D Ctrl-U | move, ten rows at a time with Ctrl-D and Ctrl-U |
| `n` / `N` | new note in the folder shown, its title ready to type / new folder |
| `dd` or `D` | delete, to Recently Deleted; from there, for good, after a question |
| `r` | restore, in Recently Deleted |
| `p` | pin or unpin |
| `m` | move to a folder |
| `R` | rename a note (its title, so its file, as typing a new title on its first line does) or a folder |
| `s` / `S` | mark notes with a ✓ / squash the marked ones into one |
| `t` | the note's tab (see below) |
| `/` | search every note, by every word you type; each note shows the line that matched |
| `x` | clear the search or tag |
| Ctrl-F | find a note (in a note too) |
| Ctrl-B | put the list away, for the note to have the room; Ctrl-B in a note shows it again |
| Ctrl-Shift-P | every command, in a palette (in a note too) |
| Tab, or Ctrl-W `l`, or `M-Right` | go to the note; Ctrl-W `h` or `M-Left` goes on to the pane on the list's left, Ctrl-W `j` `k` or `M-Down` `M-Up` to the pane below or above |
| `q` / `?` | quit / these keys |

The list has Folders (All Notes, Notes for the top folder, your folders, Recently Deleted, each with its count), then Tags, each with how many notes have it, then the notes in what is chosen: pinned first, then newest first, each with when it changed and how it starts, and in All Notes, which folder it is in. The wheel scrolls it.

**Squashing.** Mark two or more notes with `s`, then `S` merges them into one. After you say yes, a headless Claude (Sonnet, with no tools and no session kept) merges them, keeping every item, box and link, the note changed last winning where two disagree and nothing says which is right. A box shows it at work with the seconds so far; Esc in it cancels, leaving the notes as they were. The new note goes in the first one's folder, the originals go to Recently Deleted, and every tab linked to one of them links to the new one. It needs Claude Code.

**Renaming and moving.** A note renamed with `R`, its title typed over, moved with `m`, squashed, or in a folder renamed takes the tabs linked to it along, and a tab named after the note takes its new title.

### In a note

The note is edited with Vim's keys, with the settings a note had before: Space is the leader, which the app takes for its styles; indenting is two spaces; search ignores case unless you type a capital; and a yank or delete reaches the system clipboard, which `p` reads back.

| | Keys |
| --- | --- |
| Modes | `i` `a` `I` `A` `o` `O`, `v` `V`, `gv`, `gi`, Esc |
| Motions, each with a count | `h` `j` `k` `l` and the arrows, `w` `W` `b` `B` `e` `E` `ge` `gE`, `0` `^` `$` `g_`, Home, End, `gg` `G`, `f` `F` `t` `T` `;` `,`, `{` `}`, `%`, `H` `M` `L`, `+` `-` `_`, `gj` `gk`, Enter where there is nothing to tick, fold or follow, Shift-Left and Shift-Right by words |
| Operators, with a count, a motion or a text object | `d` `c` `y` `>` `<` `g~` `gu` `gU`, doubled for whole lines (`dd`, `cc`, `yy`, `>>`, `<<`, `g~~`, `guu`, `gUU`) |
| Text objects, after `i` or `a` | `w` `W`, `"` `'` `` ` ``, `(` `)` `b`, `[` `]`, `{` `}` `B`, `<` `>`, `p` |
| Changes | `x` `X` `s` `S` `C` `D` `Y` `p` `P` `J` `gJ` `r` `~`; `.` repeats the last change |
| Undo | `u`, Ctrl-R; each note keeps its history, and its folds, while the app runs, going to another note and back included, a change made to its file meanwhile one more step to undo |
| Search | `/` `?` `n` `N` `*` `#` |
| Scrolling | Ctrl-D Ctrl-U, PageDown PageUp, Ctrl-E Ctrl-Y, `zz` `zt` `zb` |
| Folds | `za` `zo` `zc` `zM` `zR` |
| Commands | `:<line>`, `:s/a/b/g` and `:%s/a/b/g`, `:w`, `:q`, `:wq` or `:x`, `:noh`; `ZZ`, `ZQ` |
| Insert mode | Ctrl-W, Ctrl-U, Ctrl-H, Backspace, Delete, the arrows, Home, End; Ctrl-V then a key types the key itself |
| Visual mode | the motions and text objects grow the selection; `d` `x` `c` `s` `y` `>` `<` `~` `u` `U` `J` `p` `r` `o` act on it, `v` and `V` switch between characters and lines, Ctrl-C copies it |
| Windows | Ctrl-W `h` or `M-Left` (typing too) goes to the list, saving the note and leaving Insert mode; Ctrl-W `l` or `M-Right` goes on to the pane on the note's right, and Ctrl-W `j` `k`, `M-Down` `M-Up` to the pane below or above, as the Alt-arrows move between panes |

Marks, macros and named registers aren't there: `"+y` and the like yank to the one register, which is the clipboard. A drag with the mouse selects; a click puts the cursor there; the wheel scrolls. Ctrl-F and Ctrl-B take the place of Vim's page down and page up in a note; PageDown and PageUp still page.

**The Format menu.** Everything a note can do is in the Format menu, Ctrl-P, which acts on the selection when opened from Visual mode. A key, Enter or a click picks an item; Esc or `Q` closes it. A style picked there leaves you typing in it, unless it was for a selection; opened while typing, the menu goes by where you were typing. The common styles also have a leader key (Space, then the key), which styles in place:

| Key | Style | Written as |
| --- | --- | --- |
| `1` `2` `3` `0` | Title, Heading, Subheading, Body | `# ` `## ` `### ` |
| `m` | Monospaced block | between ```` ``` ```` fences |
| `8` `-` `7` | Bulleted, dashed, numbered list | `* ` `- ` `1. ` |
| `l` | Checklist | `- [ ] `, `- [x] ` once ticked |
| `q` | Block quote | `> ` |
| `b` `i` `U` `x` `h` | Bold, italic, underline, strikethrough, highlight | `**b**` `*i*` `<u>u</u>` `~~x~~` `==h==` |
| `` ` `` | Monospaced text | `` `code` `` |
| `k` | Link | `[text](https://…)` |
| `t` | Table | rows of cells between bars |

The menu also has, under their own keys there, the checklist's Tick or untick (`c`), Move checked to bottom (`s`), Uncheck all (`u`) and Delete checked items (`D`); Paste image (`v`); and a table's Add row (`r`), Add column (`C`), Delete row (`R`) and Delete column (`X`).

A line style applied again turns the line back to body text, as in Notes. A character style works as it does in Notes too: on the selection; off again, for the whole run, in text that has it already; on the word under the cursor; and anywhere else (a space, an empty line, right after a word you typed) on what you type next, starting Insert mode between its markers, which go again if you type nothing. Underline and highlight, which Markdown has no syntax for, are written the way Markdown tools usually write them.

- **Checklists.** Click a ☐, or press Enter on its line, to tick it. A ticked item is dimmed and struck through with everything indented under it, and moves under the ones left to do, with its details and sub-items, a blank line between, so a list leads with what is left; sub-items sort the same way under their item.
- **Lists.** Enter in Insert mode carries a list on (a checklist item comes unticked, a number counts up) and ends it on an empty item; Tab and Shift-Tab indent and outdent an item.
- **Tables.** In Insert mode, Tab moves to the next cell, past the last one adding a row, and Shift-Tab back. A table lines up again whenever you leave Insert mode or change it, measured as it shows, so bold markup doesn't throw a column out.
- **Collapsible sections.** Enter on a title, heading or subheading, or a click on its ▾, folds what is under it, up to the next heading of the same or a higher level; a folded one shows ▸ and how many lines it hides. Vim's `z` commands work too.
- **Links.** Enter, or a click, opens a web link in the browser (in its site's Chrome profile, as every link Switchboard opens), a `[[Note title]]` link in the note with that title (made if there is none; drawn as missing until then), and a `#tag` in the list. Typing `[[` offers every note's title.
- **Tags.** Any `#word` after a space, not in code, is a tag.
- **Line numbers.** The note's lines are numbered relative to the cursor, the cursor's line showing its own, for jumps like `5j`.

**Pictures.** Ctrl-V while typing pastes the image on the clipboard into the note, as in Claude Code (Settings' "Ctrl-V pastes an image", `notes_ctrl_v_image`), and so does the Format menu's Paste image; with no image there, Ctrl-V does what it always does (the terminal's own paste key pastes only text). The image is saved beside the note in `.attachments/<note>/` in its folder, which the list leaves out, and written as `![name](path)` on a line of its own below the cursor's, indented under the item when the cursor is on a list or checklist item: the image goes with the item, and Claude, working the note's items, reads it with them. On Linux, pasting an image needs `wl-paste` (Wayland) or `xclip` (X). The picture is drawn in the note under its line in a terminal that speaks the kitty graphics protocol with Unicode placeholders, Ghostty or kitty (Settings' "Images drawn in a note", `notes_images`); elsewhere it shows as its name. The multiplexer places the picture like text, so it scrolls with the note and a box over it covers it, and passes the picture's graphics commands to your terminal from the notes app's pane alone, since one can have the terminal read any of your files. A picture other than a PNG is made one in Switchboard's cache first. Enter or a click on it opens it in Quick Look on macOS, or in the desktop's viewer on Linux.

**The palette.** Ctrl-Shift-P, in a note or the list, lists every command Notes has, each with its key, the ones picked last first: the list's (on the note open, from a note), the Format menu's, folding, and Switchboard's for Notes: moving Notes beside your work (`M-e`), the note's tab, choosing a tab's note, reading a link from the note beside it, searching notes, Linear, GitHub and Slack (`M-F`), and "Shape the note's checklist items as Linear tickets", which opens a tab running `/shape` for each unticked item. Settings' "Notes command palette" (`notes_palette`) turns it off.

**The finder.** Ctrl-F, in a note or the list, finds a note: every note but those in Recently Deleted, pinned first and then newest, each as its title, its folder and how it starts, any of which you can type to match, with the note shown beside the list. Enter opens it; Esc closes the finder.

**The toolbar.** Over each note is a row of buttons for the common styles (bold, italic, strikethrough, monospaced, title, heading, bulleted list, checklist, table, link, image) and `⋯` for the whole Format menu. A click runs one on the selection, or else on the word or line the cursor is on, and the button under the mouse lights up (Settings' "Highlight on hover", `hover_highlight`). In a narrow window the last buttons drop off, `⋯` always kept. Settings' "Toolbar" (`notes_toolbar`) hides it.

**Easy mode.** Settings' "Easy mode" (`easy_keys`, off at first) is for anyone who doesn't know Vim's keys. Shift with an arrow, Home or End selects, and so does a drag with the mouse; typing replaces the selection, and Backspace deletes it. Cmd-S saves, Cmd-Z undoes, Cmd-Shift-Z redoes and Cmd-X cuts, and Ctrl-C copies the selection. A line at the bottom names the keys that work in the mode you are in (Settings' "Key help line", `key_help_line`), and the Format menu, the palette, the finder and the app's questions get a × in their top-right corner that a click closes. Vim's keys keep working beside them, and the setting takes effect when Notes next opens.

The Cmd keys need your terminal to send them as Ctrl+Alt+Shift keys in the kitty keyboard form (`CSI <code>;8u`), since a terminal otherwise reads a Cmd key as its own or as Alt: Cmd-S as `CSI 115;8u`, Cmd-Z as `CSI 122;8u`, Cmd-Shift-Z as `CSI 121;8u` and Cmd-X as `CSI 120;8u`. In Ghostty, add to its config:

```
keybind = super+s=csi:115;8u
keybind = super+z=csi:122;8u
keybind = super+shift+z=csi:121;8u
keybind = super+x=csi:120;8u
```

The multiplexer passes those four keys only to the notes app's pane while it runs in easy mode, and drops them anywhere else, so no other program reads them as Ctrl keys.

### Notes in the workspace

There is only ever one Notes in the workspace, since two would save the same notes over each other.

`M-N` (Alt-Shift-N) goes to Notes wherever it is open, or opens it in a tab of its own, named Notes. `M-e` brings that same Notes into a pane to the right of the one you are in, or starts it there; pressed again in that tab, or `M-w` on Notes there, it goes back to a tab of its own rather than closing. While it is away, its own tab shows which tab has it ("Notes is beside tab 3"), so the tab stays in the sidebar: Enter there goes to that tab, and `M-e` brings Notes back. When the last of a tab's own panes closes, Notes goes back to its own tab rather than stay there alone. While Notes is beside a tab's pane, that tab says "✎ Notes here" in the sidebar and on its mission control card.

Beside another tab's pane, Notes' list is put away, so the note has the room (Ctrl-F still finds a note), and it comes back in Notes' own tab (Settings' "Hide the sidebar beside a tab", `notes_hide_sidebar`).

**A tab's note.** Beside a tab, Notes shows that tab's own note: the one its Claude works from (`/note-watch` and `/queue-and-clean` record it once they have settled on it), else the last one opened beside that tab (a note Notes only carried over from the tab before isn't taken as this one's). A tab with no note of its own is asked which it is, the likeliest first, so Enter takes it: notes that mention its ticket or PR number, then those whose titles share words with the tab's name, then the note last shown beside it, then those naming its repository, then the rest, the most recently changed first within each. "Keep whatever is open", last, stops it asking for that tab, and Esc leaves the question for next time. M-p's "Choose this tab's note, the one M-e shows beside it" asks again any time, and Settings' "Ask for a tab's note" (`ask_tab_note`) turns the question off.

**A note's tab.** `t` in the list, or M-p's "Open the tab for the note in Notes" (for the note Notes shows), goes to the tab linked to the note. With none, it asks which tab is the note's: one of the open tabs, linked to the note then, or a new one, in a directory you pick as `M-t`'s does, linked to the note and named after it. The new tab's Claude starts knowing which note it is for: its title and file are added to its system prompt, so no turn is spent on it, and the note itself is read only when you ask (Settings' "Tell a note's new tab its note", `note_tab_prompt`). Either way Notes moves beside the tab's Claude showing the note, as `M-e` puts it there, the cursor left in Claude's pane (Settings' "Notes beside a note's tab", `note_tab_beside`).

These need the workspace: in a plain terminal `switchboard notes` runs alone, and the palette's workspace commands say so.

The workspace reaches the running app through a socket, `~/.local/state/switchboard/notes.sock` (or `$NOTES_SOCKET`), which you can use from a shell too:

```
switchboard notes open <note>       show a note, by its path in the notes directory
                                    (with no app running, the note it opens with)
switchboard notes sidebar on|off    show or put away the list
switchboard notes shown             the note shown, as its path
```

## Setting Claude Code up

Switchboard learns what each Claude Code session is doing from Claude Code's own hooks, and gives it a statusline, a part of your `~/.claude/CLAUDE.md` and a set of skills. `switchboard setup` puts them in place. The install script runs it once the binary is in place, `switchboard update` runs it after installing a release, and `M-p`'s "Run Switchboard's setup" runs it in a tab of its own, to put in place what isn't or to answer a question you said no to.

```
switchboard setup                  the guided setup
switchboard setup --check          say what would change, and change nothing
switchboard setup claude skills    only those parts, asking nothing
switchboard setup --parts          list the parts
switchboard setup --refresh        bring the parts already set up up to date, quietly
```

Alone, it is the guided setup. When the tmux version of Switchboard left anything behind (below), it offers to tidy it first. It checks that `switchboard` is on your PATH, since Claude Code's hooks and statusline run it by name, that git is there for the worktree commands, the diff and the tabs' branches, and that Claude Code is installed; without Claude Code it says how to install it and goes on to the terminal. Then it takes each part in turn. The first time, it asks before the part changes anything (`[Y/n]`: Enter is yes, and a closed input is no, so nothing piped or unattended says yes); once a part is set up, it brings it up to date without asking. After Claude Code's parts it looks at the terminal (Option as Alt, below) and then says which optional parts are there, each for what it names, with how to set up the ones that aren't: the GitHub sign-in (`M-v`, the PR views, PR numbers in tab names), the Linear key (`M-i`, the Tickets and Inbox views, ticket titles) and the Linear team `/shape` files tickets in (`linearTeam` in `linear.json`), your calendar's address (the Calendar view), the Slack relay's tokens and owner (`M-S`), Chrome (the PR and ticket views as images, `/walkthrough`'s videos), ffmpeg and Node (`/walkthrough`, `/deploy-test`, `/preview-test`), git-delta, Node and Chrome (`/diff-screenshots`), and Codex (`/review-loop`'s reviewer; Claude reviews without it). GitHub and Linear are left out while Settings turns them off. It ends with the optional parts not there named on one line, then what is left to do, every ✗ again, or "Ready." Running it again changes only what isn't in place yet, and each part says what it changed or that it was up to date. With parts named, it does only those.

| Part | What it does |
|---|---|
| `claude` | Adds Switchboard's hooks, statusline and permissions to `~/.claude/settings.json` |
| `claude-md` | Writes `~/.claude/CLAUDE.md` to import your rules and Switchboard's part |
| `skills` | Writes the skills that ship with Switchboard into `~/.claude/skills` |
| `terminal` | Makes Option act as Alt in a Mac's terminal: Ghostty's config, Terminal's or iTerm2's default profile |
| `leftovers` | Removes the tmux version's links that lead nowhere, and runs its `~/.zshrc` lines through `switchboard` |

### Option as Alt, and the tmux version's leftovers

Every `M-` key needs a Mac's terminal to send Option as Alt ([Option as Alt on a Mac](#option-as-alt-on-a-mac)). The `terminal` part sets it where it can: a Ghostty config of yours (`~/.config/ghostty/config`, or the one in `~/Library/Application Support/com.mitchellh.ghostty/`) gets `macos-option-as-alt = true` unless it already sets `macos-option-as-alt`, and one is made when you run it in Ghostty with none; and when it runs in Terminal or iTerm2, that app's default profile is changed through macOS's own preferences (`defaults export` and `import`): "Use Option as Meta key" in Terminal, both Option keys sending Esc+ in iTerm2. Open a new window, or the app again, for it to take. The guided setup asks first, and for another terminal it says the line to add to its config. On Linux a terminal sends Alt as it is, so the part changes nothing there; under xterm the guided setup says to add `XTerm*metaSendsEscape: true` to `~/.Xresources`, and otherwise reminds you that a key your desktop takes for itself can be moved in `keys.json`.

A clone of the tmux version pulled past the rewrite leaves links into files it no longer has: the scripts setup.sh linked into `~/.local/bin` and `~/.config/tmux`, and `~/.claude/commands`, `~/.claude/hooks/*`, `~/.claude/statusline-command.sh` and `~/.claude/otel-headers.sh`. Its `~/.zshrc` lines, `eval "$(worktree-from-ticket init zsh)"` and the rest, then fail on every new shell. The `leftovers` part removes each such link that leads nowhere into a Switchboard clone (a link that still leads somewhere, or anywhere else, is left alone), and turns each of those lines in `~/.zshrc` or `~/.bashrc` whose script no longer runs into the same line through the binary (`eval "$(switchboard worktree-from-ticket init zsh)"`), so the functions stay; the file is changed through a link to it, with a copy as it was left beside it as `<name>.bak.<time>`. The guided setup offers it only when there is something to tidy.

### Claude Code's settings

The `claude` part merges Switchboard's settings into your own `~/.claude/settings.json`, which stays a file of yours, never a link: Claude Code writes `/model` and "always allow" there, and reads no settings file after it. What is already there is kept, and a copy of the file as it was is left beside it as `settings.json.bak.<time>`. A file that isn't a JSON object is left alone, and the part says so.

- **Hooks**: one for each Claude Code event the workspace follows (`SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `PermissionRequest`, `Notification`, `Stop`, `PreCompact`, `PostCompact` and `SessionEnd`), each running `switchboard hook <Event>`. They are how a tab knows its Claude is working, waiting on you or done; without them every tab shows idle and `M-a` has nothing to go to. A hook of yours running the same command is replaced, and so are the ones the tmux version of Switchboard added; your other hooks stay.
- **Statusline**: `switchboard statusline`, which shows the model in the project's accent color, the directory and branch when the session runs outside the workspace, and how full the context is. It also keeps each session's context use and spend for mission control's Sessions and Usage views. A statusline of your own is kept.
- **Permissions**, added to yours: what Switchboard's skills and loops need to run unattended (the `switchboard` commands they call, their scripts in `~/.claude/skills`, read-only `git` and `gh`, and the read-only tools of the Slack, Calendar and Linear connectors), and two denials, reading `.env` files and `git pull`. Anything that writes outside the computer is left to ask.

Once all of it is there, running the part again changes nothing. A permission or hook Switchboard stops shipping isn't taken out of your file.

### Your CLAUDE.md

The `claude-md` part writes Switchboard's part of your rules to `~/.local/share/switchboard/switchboard.md`: what every user's Claude needs to know about the workspace (the notes app, `switchboard tabs` for what the other tabs are doing, how to talk to another tab and name one, a repository's own skills). Your `~/.claude/CLAUDE.md` imports it with a line of its own, which Claude Code follows (`@path`), so a new version of the part takes effect with the next session.

Your own rules come from where `~/.config/switchboard/claude.json` says, a git repository or a file, which Settings' "Your CLAUDE.md" row writes for you (and then runs this part):

```json
{"repo": "https://github.com/you/claude-rules", "file": "CLAUDE.md"}
```

```json
{"path": "~/somewhere/CLAUDE.md"}
```

A `repo` is cloned into `~/.local/share/switchboard/claude-md` and pulled, fast forward only, each time the part runs. On GitHub or a gist, gh's sign-in is added as the clone's credential helper when gh is signed in, so a private repository works. Without `file`, your rules are the repository's `CLAUDE.md` at its root, else the one nested least deep; none, or several at that depth, is an error that says so. Changing `repo` moves the old clone aside. With neither `repo` nor `path`, `~/.config/switchboard/CLAUDE.md` is used when there is one.

With rules named, `~/.claude/CLAUDE.md` becomes a short file written by setup that imports your rules and then Switchboard's part; edit your rules where they live, not there. A CLAUDE.md of your own in its way is moved aside to `CLAUDE.md.bak.<time>` first. A rules file named that isn't there stops the part, naming the path. With no rules named, your own `~/.claude/CLAUDE.md` stays as it is and gets one line added, `@~/.local/share/switchboard/switchboard.md`; with none, one is made holding that line. A `~/.claude/CLAUDE.md` that linked into a clone of the tmux version has its rules copied to `~/.config/switchboard/CLAUDE.md`.

### Skills

The `skills` part writes each skill the binary carries into `~/.claude/skills/<name>`, where Claude Code finds a personal skill. A later run brings each one up to date, removes a file a newer version no longer ships, and leaves alone what a skill made for itself, such as `node_modules` beside its scripts. Each folder Switchboard wrote holds `.switchboard-skill`, the list of the files it wrote. A folder without that list, or a link to somewhere else, is a skill of your own by the same name: it is kept, and the guided setup lists the ones it kept. An edit to a skill Switchboard wrote is written over on the next run, so to change one, delete its `.switchboard-skill` and it is yours. Once any skill is installed, the guided setup writes a new one without asking. It also links `skill-path` and `codex-review` in `~/.local/bin` to the binary, which runs the command a link is named after, so a repository's own copy of a skill written for the tmux version, which runs them by those names, still finds them; a file of your own by either name is left alone, and a link leading nowhere (into a clone of the tmux version) is replaced.

A repository may have its own skill by one of these names, and Claude Code runs a personal skill over a project's, so each of Switchboard's starts by running `switchboard skill-path <name>` and follows the repository's copy when there is one (`.claude/skills/<name>/SKILL.md` in the worktree, else in its main clone). A skill that hands off to another reads the file `skill-path` prints in the same way.

The ones marked "you" run only when you type them; Claude may also pick up the others on its own when what you ask fits.

| Skill | What it does | Started by |
|---|---|---|
| `/ci-loop` | Watches the branch's PR checks in the background and fixes what fails, pushing each fix, until every check passes; stops and asks when a fix isn't the branch's to make | you |
| `/clean-worktree` | Removes a worktree and its local branch once nothing in them would be lost (no uncommitted changes, every commit on GitHub), then prunes deleted remote branches | you |
| `/deploy-test` | Waits for a merged PR to deploy, tests its feature on the deployed site in a real browser, records a captioned video of the test and asks you to approve it | you |
| `/diff-screenshots` | Renders a branch's or PR's diff with git delta as side-by-side PNG screenshots, one set per file, core code first and tests last, and opens them (in Preview on a Mac); it needs git-delta, Node and Chrome | you or Claude |
| `/note-tickets` | Reads the open checklist items in the notes you name, checks Linear for tickets that already cover each, and suggests tickets for the rest, creating only the ones you pick | you |
| `/note-watch` | Watches one note for new checklist items while the session runs, and works each as it appears, ticking it when done | you |
| `/oncall` | Watches your team's Slack alert channels while you're on call, and reaches you through the Slack relay only about what needs you | you or Claude |
| `/preview-test` | `/deploy-test` before the merge: tests an open PR's feature on its preview deploy and records it | you |
| `/project-status` | Says where the session's project stands, from Linear's tickets, milestones and status updates and what the team said on Slack | you or Claude |
| `/queue-and-clean` | Queues the branch's PR for merge, fixes what stops it merging, tests the deploy, closes out the ticket and the note's item, and removes the worktree; in a repository that commits straight to its default branch, lands the branch there instead | you |
| `/resolve-conflicts` | Rebases the branch onto the remote's main and resolves the conflicts, carrying a rebase, merge or cherry-pick through to the end | you or Claude |
| `/review-loop` | Reviews the branch over and over with Codex (Claude where Codex isn't there), fixing what each round finds, until two rounds in a row come back clean | you or Claude |
| `/shape` | Shapes a Linear ticket with you: reads the code, checks for tickets that already cover the work, drafts the ticket and writes it once you approve | you |
| `/slack-updates` | Gathers what is new for you on Slack since the last look and saves it for mission control's Slack view, meant to run as `/loop 5m /slack-updates` | you or Claude |
| `/today` | Today's calendar as a table, with times, length, where to join and your reply, and saves it for mission control's Calendar view | you or Claude |
| `/walkthrough` | Records a captioned video of a feature in a real browser, reached the way a person would reach it, and hands back an MP4 | you |
| `/wt` | Hands this tab over to `wt` for a ticket: the session ends, and the ticket's worktree opens in the same pane with a new Claude working on it | you |
| `/wtn` | The same for one checklist item of a note, through `wtn` | you |

Codex is used for one thing, reviews. `switchboard codex-review`, which `/review-loop` runs each round, has Codex review the branch when the `codex` command is installed and signed in (`codex login`); without it, Claude Code reviews in its place with the same prompt and the same JSON (see "Reviews" below).

## Worktrees

Each piece of work runs in a git worktree of its own, on a branch of its own, with Claude started in it: a Linear ticket, a checklist item of a note, a branch or a pull request. Five commands make or find one, and each prints a shell function for your shell's startup file:

| Function | Command | For |
|---|---|---|
| `wt` | `switchboard worktree-from-ticket` | a Linear ticket |
| `wtn` | `switchboard worktree-from-note` | a checklist item of a note |
| `wtb` | `switchboard worktree-from-branch` | any branch, new or existing |
| `wtpr` | `switchboard worktree-from-pr` | a GitHub pull request |
| `wtr` | `switchboard worktree-resume` | a worktree one of the others made, its Claude resumed |

### The shell functions

No program can change the directory of the shell that started it, so the commands alone make the worktree and start Claude in it but leave your shell where it was. A function runs its command, `cd`s into the worktree the command names, and starts Claude there itself, so your shell sits in the worktree for the whole session and after Claude exits, and a pane split from it opens there too. Add them to `~/.zshrc`:

```sh
eval "$(switchboard worktree-from-ticket init zsh)"
eval "$(switchboard worktree-from-note init zsh)"
eval "$(switchboard worktree-from-branch init zsh)"
eval "$(switchboard worktree-from-pr init zsh)"
eval "$(switchboard worktree-resume init zsh)"
```

For bash, put the same lines with `init bash` in `~/.bashrc`; zsh and bash are the shells `init` writes for. Setup doesn't add these lines. The workspace's own keys don't need them, since each tab they open defines its function from `init` before running it, and neither do mission control's menus ("Start work on it in a new tab (wt)", "Work on it in a new tab (wtpr)", "Shape it in a new tab", "Start a CI loop on it in a new tab"), which start the work the same way, or `/wt` and `/wtn`. You need them only to type the functions yourself.

### Where the worktrees go

Each worktree goes in its repository's `.claude/worktrees/`, under the main clone, so one made from inside another worktree goes beside it rather than inside it. Add `.claude/worktrees/` to the repository's `.gitignore`. A different parent directory can be given first, relative to the main clone (`wt .worktrees ABC-123`). The main clone's `.claude/settings.local.json`, which git doesn't copy since it is ignored, is linked into each new worktree, so your local permissions apply there too; a worktree with its own is left alone.

Run again on the same work, a command reopens it: the worktree is reused if it is there, and the branch if it is there. `wt` and `wtn` refuse while Claude is already running in the worktree, naming that session, its title and the tab it runs in, rather than start a second one editing the same files and pushing the same branch. `wtb` and `wtpr` stop when the branch is checked out in your main clone, since git allows a branch in one worktree at a time, and use the worktree that already has it when there is one.

`wt`, `wtpr`, `worktree-done`, `wt-jump`, `codex-review` and `wait-deploy` are programs you run, so they ask GitHub and Linear whatever the `github` and `linear` settings say, as the tmux version's scripts did; the settings turn off what the workspace does by itself (its lookups, views and keys). `wt` and `worktree-done` read Linear with the key the rest of Switchboard uses: `LINEAR_API_KEY`, else the stored secret `linear-api-key`, else `linearApiKey` in `~/.config/switchboard/linear.json`. Make a personal API key in Linear (Settings, Security & access) and store it with `switchboard platform secret-set linear-api-key`, which reads it from stdin and keeps it in the Keychain on a Mac, or the desktop's keyring on Linux (with no keyring, in `~/.config/switchboard/secrets.json`, which only you can read). The same `linear.json` holds `linearTeam` and `linearLabels`, the team and labels `/shape` files new tickets with. `wtpr` reads GitHub with your token: `GH_TOKEN` or `GITHUB_TOKEN`, `gh auth token` when gh is signed in, else the stored secret `github-token`.

### Starting work from the workspace

| Key | What it does |
|---|---|
| `M-t`, `M-n` | New tab, running Claude, in a directory you pick |
| `M-W` | Start work on tickets in new tabs (`wt`) |
| `M-B` | Start work on a branch, new or existing, in a new tab (`wtb`) |
| `M-p`, "Start work on a PR in a new tab (wtpr)" | A PR, in a new tab running `wtpr` |
| `M-p`, "Start work on a note's checklist items in new tabs (wtn)" | A note's items, a tab each running `wtn` |
| `M-p`, "Work the note continuously" | A note's items, a few tabs at a time, the next opened as each lands (below) |
| `M-p`, "Continue shaping Linear tickets, each in a new tab" | Tickets, a tab each running `shape --continue` |
| `M-o` | Reopen a worktree with no tab and resume its Claude |
| `M-j` | Search and jump to a tab |

Each of the first six asks which directory first, in a list: the one you are in, then every open tab's, then those Claude sessions ran in, most recent first, then your home, with worktrees left out but the one you are in. For the keys that make a worktree, each entry is its repository's main clone, listed once, and a directory in no repository is left out. Typing narrows the list; a path typed in is taken when nothing matches it, or at once with Alt-Enter (`~` is your home). Then:

- `M-t` opens the tab there running Claude, from an interactive shell of yours, which is left in the tab when Claude exits. Then a menu asks what to call it, which you may skip with Esc: `n` a name typed in, `t` its ticket and the ticket's title, or `s` "Shaping: " and a ticket, the form `/shape` gives a tab. Each ticket is the one the branch names, else asked for; `t` on a ticket's own branch leaves the tab to show its ticket and title by itself (Settings' "Name new tabs", `name_new_tabs`).
- `M-W` asks for the tickets, IDs or Linear URLs separated by commas or spaces, filled in from the clipboard when it holds only tickets. Every one is checked before any tab opens, so a typo leaves none half-started. The tabs go at the end of the tab bar in the order given, so no open tab is renumbered, and the first comes to the front.
- `M-B` lists the repository's branches, local and remote, the most recently committed first. Enter works on the one picked; a name no branch matches is a new branch, and Alt-Enter takes the name typed even when some branch matches it.
- "Start work on a PR" asks for its number or URL, filled in from the clipboard when it holds one. A URL names its repository, which `wtpr` finds a clone of, so it asks for no directory; a bare number asks which repository it is a PR of.
- "Start work on a note's checklist items" asks for the repository, then the note, the most recently changed first, then which of its unticked items to work on. Items under a section for parked work (`## Later`, `## Deferred`) and items a worktree already works on are left out, and one whose plan still needs a choice is marked so; with more than one, the first row takes them all, and Tab marks a few (a `+` before each, Shift-Tab marking going up), which Enter then takes together. The lower half of the list shows the item under the cursor whole, its full text and what is indented under it, since a row cuts it to one line (Settings' "Show a note's item whole in wtn's list", `wtn_item_preview`). Each item gets a tab, named after it and linked to the note.
- "Continue shaping Linear tickets" asks for the tickets, then the directory, which must be in a repository so `shape` has code to read.

`M-p`'s "Shape the note's checklist items" opens a tab running `/shape` for each unticked checklist item of a note you pick.

### Working a note continuously

`M-p`'s "Work the note continuously", asked from a pane in the note's repository (a worktree of it will do), keeps tabs working through a note's checklist unattended. It opens mission control on its tabs and asks which note, the one it last worked first, then the newest; how many of its tabs work at once (3); the most of them open at once, those waiting on you included (5); and a spend limit in dollars, empty for none. Each box starts with what it was last time, so Enter keeps it. From then on Switchboard opens the note's next item whenever fewer than that many of its tabs work and fewer than the most are open, one at a time and a few seconds apart, in the background, in the session you used last: a tab running `wtn`, named after the item and linked to the note, as "Start work on a note's checklist items" opens one, in the repository's main clone. The items are taken in this order: those whose `Plan: [[…]]` links a plan note with no "## Decision needed" first, then those with no plan, each in the note's order; never one whose plan needs a choice, one under a section for parked work, one a worktree already works on, or one you skipped.

Only the tabs it opened count, each marked `auto` in the sidebar. A tab waiting on you (a question, a turn ended) frees its slot but still counts toward the most open, as does one whose `wtn` failed and never reached its worktree; a tab `/queue-and-clean` has landed, back in the main clone, leaves the mode, and the next item opens in its place. What marks them is kept with the tabs, so it carries on after a restart. It opens nothing while you're away (whatever Settings' "Pause lookups while away" says), once today's Claude spend in the Usage view passes its limit, or while the note changed in the last minute, since an item being typed is saved as it is typed. It says so in the notifications when it stops for the spend, when the note has nothing left to take, or when the note is gone; its notification's menu goes to its line, or puts it away.

While it is on, its line leads mission control's Tabs view, above the cards: `AUTO`, the note, how many of its tabs work and are open against those counts, the item it takes next, and how many are skipped. `k` from the first row of cards, a click or Tab reaches it, and Enter there opens its menu: pause it (and resume), skip the next item (and take the skipped ones again), take only items with a settled plan (and those without one too), change its note, counts or spend limit, or stop it; the tabs open carry on either way. "Work the note continuously" opens the same menu while it is on. What it keeps is in `~/.local/state/switchboard/autopilot.json`, and Notes takes a renamed or moved note along. Settings' "Work a note continuously" (`autopilot`) off allows none of it.

### `wt`: a Linear ticket

```
wt ABC-123
wt https://linear.app/your-workspace/issue/ABC-123/some-title
wt ABC-123 --prompt "start with the migration"
wt .worktrees ABC-123
```

`wt` reads the ticket from Linear, makes the worktree on the branch Linear names for it (else `<id>-<title, slugged>`), and starts Claude with a prompt that carries the ticket's state. A ticket in a closed state (completed, canceled or duplicate, whatever the team calls it) is raised first, with the question whether to reopen it. Then Claude checks the ticket against the code (is it still relevant and needed, is its approach sound), builds what the code needs where the ticket gets a detail wrong and updates the ticket to match, and stops to ask only about a change of scope. It works on the ticket without running tests or checks, runs `/review-loop`, and pushes and opens a PR. Its last message holds only a ticket check ("Unchanged." or the edits it made), "PR pushed" or "PR not pushed" with why, a word when the review loop stopped at its round cap, and "Preview:" and "Prod:", a few steps each for trying the work. `--prompt` adds your own words at the end of the prompt.

`/wt` in a Claude session hands its tab to `wt`: the session ends, and in the same pane `wt` opens the ticket's worktree with a new session working on it. The ticket comes from the session (one it created or saved, the one it was about, else the branch's), or from `/wt ABC-123`; any other words go into the new prompt (`/wt start with the migration`). It first lists what the session still runs in the background and asks whether to stop it, and it clears a tab name `shape` or you set, so the tab takes the ticket's name.

### `wtn`: a checklist item of a note

```
wtn --note "Project notes" --item 12
wtn --note "Project notes" --item 12 --prompt "keep the old flag"
```

`--note` is the note's path within your notes directory (`$NOTES_DIR`, else `~/Notes`) or an absolute one, `.md` optional; `--item` is the line number of an unticked `- [ ]` item at the note's top level. A ticked item, a line that isn't one, a note outside the notes directory or in its trash, and an item worded the same as another are refused. The branch is `note/<the item's first line, slugged>` and the worktree `note-<slug>`, made from origin's default branch as it is now, fetched first. Which item a branch is for is kept in the repository's git config, so another item that slugs the same is refused rather than started in this one's worktree.

Claude is given the item as written, with what is indented under it. It is told to read the plan a `Plan: [[…]]` line links first and ask about any choice the plan still needs (with no plan, to read the code and ask before building what the item leaves open), to commit on the branch, and to land the work as `/queue-and-clean` does once it is done. `--prompt` adds to the prompt.

`/wtn` does for a note's item what `/wt` does for a ticket: it settles which note, lists its unticked items to pick from, names the tab after the item and hands the pane to `wtn`.

### `wtb`: any branch

```
wtb fix-login-redirect
wtb feature/login
```

A branch that exists here or on the remote is checked out as it is, tracking the remote's; a local one behind the remote's is fast-forwarded, and one that has diverged is left alone with a note. A branch that exists nowhere is made off the remote's default branch, fetched first, and gets an upstream on its first push; with no remote, it is made off `HEAD`. The worktree is named after the branch with its slashes made dashes (`feature-login`). Claude starts with no prompt. `wtb` is plain git: it needs no Linear key and no GitHub.

### `wtpr`: a pull request

```
wtpr 123
wtpr https://github.com/owner/repo/pull/123
```

`wtpr` makes the worktree on the PR's head branch, in `pr-<number>-<branch>`. Claude starts by reading the PR (its description, diff, unresolved review threads, failing checks and its Linear ticket), says where it stands and what it would do next, and changes nothing until you say; on someone else's PR it pushes nothing unless you ask. Settings' "wtpr asks where the PR stands" (`wtpr_prompt`) turned off starts Claude with no prompt. `WTPR_PROMPT` in the environment replaces the prompt, as mission control's "Start a CI loop on it in a new tab" does with `/ci-loop`.

A bare number is a PR of the repository you are in. A URL of another repository, or one given from outside any, is worked on in a clone of it found among every open tab's repository, every one a Claude session ran in lately and the directories right under your home; with none, it says so. Any PR works, whatever its state: one whose branch the remote has deleted opens from the commits GitHub keeps for it, on a local branch with no upstream. A fork's PR is set up as `gh pr checkout` sets one up: the fork becomes a remote named after its owner, and the branch, `<owner>-<branch>`, tracks the fork's, so a push updates the PR. It says so when the fork's owner hasn't let maintainers push.

### `wtr` and `claude-here`: back into a worktree

```
wtr ABC-123
wtr 123
wtr https://github.com/owner/repo/pull/123
wtr fix-login-redirect
```

`wtr` takes what the others take, finds the worktree they made for it in `git worktree list` (a ticket's ID or `pr-<number>` in its directory or branch, or a branch by its whole name), moves your shell there and resumes its Claude with `claude-here`. Nothing is made or fetched, so it needs no Linear key, no GitHub and no network. Several matches, such as a ticket's worktree and one made later from its PR, are listed by number, the most recently touched first, and Enter takes the first. A worktree git still tracks whose directory is gone is said to be so, with the `wt`, `wtpr` or `wtb` that makes it again.

`switchboard claude-here` resumes the Claude session of the repository you are in (its top level, or the directory you are in outside one), from the directory the session started in. With one session it resumes it at once. With several it lists them by when each was last active, with the last thing you typed in it, to pick with `j`/`k` or the arrows and Enter (Esc, `q` or `C-c` cancels), or by number when its input is a pipe. `--print` prints them instead, a line each with the session's ID, when it was last active and its transcript; any other argument is passed to `claude`.

### Finding a tab or a worktree again

`switchboard wt-jump` goes to the tab working on a ticket, a PR or a branch:

```
switchboard wt-jump ABC-123      the ticket's tab
switchboard wt-jump 123          the PR's tab
switchboard wt-jump theme-slot   any tab whose branch, directory or name contains that
switchboard wt-jump --list       print the tabs, and jump to none
```

Tabs are matched on each pane's directory and the branch checked out there, and on the tab's name, whose `#<number>` front is its PR; you land on the pane that matched. A ticket is matched whole, so `ABC-12` never finds `ABC-123`. A bare number is looked for as a `pr-<number>` worktree first, and only then is GitHub asked which branch the PR is on. One match jumps outright; several are listed by number, the tabs waiting on you first; none prints the `wt` or `wtpr` that would open one. A tab in another session is made the current tab there, and the terminal you used last moves to that session, as tmux's did. To pick a tab by looking, `M-j` opens mission control's Work map as a picker of the open tabs.

`switchboard wt-revive` lists the worktrees no tab has open, in the repository you are in (or `<dir>`'s), every open tab's and every one a Claude session ran in over the last two weeks: each one's repository and branch, how long since it was used and the last prompt Claude was given there, most recent first. `--json` prints a line of JSON for each. What each transcript says is cached in `~/.cache/wt-revive/index.json`, so it reads only the transcripts that changed. `M-o` shows these rows in mission control's Work map, and Enter on one offers "Open a tab there, resuming its Claude", which runs `claude-here` there as `wtr` does.

### Several at once

```
switchboard wt-batch ABC-123 ABC-124
switchboard wt-batch "ABC-123, https://linear.app/your-workspace/issue/ABC-124/some-title"
switchboard shape-batch "phase label is unclear" "results load slowly" ABC-123
switchboard shape-batch --continue "ABC-123, ABC-124"
switchboard shape-batch < ideas.txt
```

`wt-batch` opens a tab per ticket, running `wt` in each. Tickets are separated by commas, spaces or both, and every one is checked before any tab opens. `shape-batch` does the same with `shape`. Each of its arguments is one thing to shape, since an idea is free text, while an argument made only of ticket IDs or Linear URLs is split as `wt-batch` splits it; with no arguments, it reads ideas from stdin, one a line, and `--continue` is passed on to each `shape`. Both run only in a Switchboard pane. They open their tabs at the end of the tab bar in the order given while focus stays where it is, start each in the directory they were run from (so run them from the repository the work belongs to), and open anything given twice once. Each tab is left a shell when Claude exits.

### Shaping a ticket

```
switchboard shape "people can't tell which phase is active"
switchboard shape ABC-123
switchboard shape ABC-123 split out the API part
switchboard shape --continue ABC-123
```

`shape` is the step before `wt`. It starts Claude with `/shape` in the repository's main checkout, even when run from a worktree, since a ticket being shaped has no branch yet. It fetches first and warns when the local branch is behind its upstream, since Claude reads the files on disk, and it names the tab "Shaping: <first words>". The skill does the rest: it reads the code, checks Linear for a ticket that already covers the idea, drafts the ticket in the form the team's recent tickets use, writes it to Linear only once you approve, reviews what it wrote with the repository's `/review-loop` in ticket mode (Codex reading the ticket against the code, its edits saved to Linear only once you say so), and names the tab "Shaping <ID>: <title>" once there is a ticket. Words after a ticket are your direction for the shaping. Given a ticket, it asks whether to run the repository's review loop on it or keep shaping; `--continue` keeps shaping without asking. `/wt` afterwards hands the same tab to the ticket's worktree. `/shape` works the same in a session you already have.

### Cleaning up

`switchboard worktree-done` looks up the Linear ticket each worktree of the repository is named after (from its branch, else its directory, with a `pr-<number>-` front left out) and offers the ones whose ticket is finished, in Linear's completed or canceled state whatever the team calls it, to remove. On a terminal it lists them by number: type numbers or ranges (`1 3-5`), `a` for all, or Enter for none, and taking all of them asks again first. With no terminal it prints them instead. Removal is `git worktree remove` without `--force`, so a worktree holding uncommitted work refuses to go, and the branch always stays.

```
switchboard worktree-done            pick finished worktrees to remove
switchboard worktree-done --list     print them, removing nothing
switchboard worktree-done --all      print every worktree by its ticket's state
switchboard worktree-done --paths    print only the finished ones' paths
```

`switchboard update-main [<dir>]` brings the repository's local default branch up to date with origin's from wherever you are, a worktree on another branch included. It switches nothing and only ever fast-forwards, so a default branch with commits of its own is left as it is. Where the branch is checked out, it is fast-forwarded there, unless that checkout has uncommitted changes, which it says; checked out nowhere, the branch alone moves.

### Another tab's last reply

`switchboard claude-reply <tab>` prints the last reply Claude gave in a tab, the final text of its last turn, read from its session's transcript; `<tab>` is the tab's number in your session or its id (`@12`). `--pane <tab>` prints the pane to type into instead: the tab's Claude pane, or its active pane when it runs no Claude. It exits 1, saying why, when the tab has no Claude or no reply yet. `M-p`'s "Send Claude's last reply to another tab" does it in the workspace (see [More in the palette](#more-in-the-palette)).

## Landing and reviewing

### Reviews: `/review-loop` and `codex-review`

`/review-loop` runs the branch through rounds of skeptical, read-only review, fixing and committing the findings it agrees with, until two rounds in a row come back clean, five rounds at most. Before the first round it runs Claude Code's `/security-review` once, its findings fixed or asked about as the rounds' are (Settings' "Security review in the review loop", `review_security`). It keeps a notes file the reviewer reads every round, holding each finding you dispute and each gap you accept, with a reason for each. It pushes once, at the end, after asking. `/review-loop all` asks about low-severity findings too, and `/review-loop norules` leaves the rules out.

Each round is `switchboard codex-review`. It reviews committed changes only, and refuses a tree with uncommitted ones. The PR is looked for where gh looked: the repository `gh repo set-default` named, else the project an `upstream` remote names when origin is a fork, else origin's, and only a PR from where the branch is pushed counts, never a stranger's with the same branch name. The base is the PR's base on the remote that points at the PR's repository (upstream, in a fork's clone), else origin's default branch; on the default branch with no PR, working straight on `main`, it reviews the commits origin doesn't have yet, fetched first. It prints one JSON object: `summary`; `findings`, each with a severity (`critical`, `high`, `medium` or `low`), a category, a file and line, a title and the failure it found; then `reviewer`, `branch`, `base`, `reviewedCommit`, `mergeBase`, `patchId` and `warnings`, which lists the findings that mention a path outside the repository, to read before acting on them.

```
switchboard codex-review                       the branch against its PR's base (on main: the unpushed commits)
switchboard codex-review --base upstream/main  against another base, used as given when the ref exists
switchboard codex-review --notes notes.md      disputed findings and accepted gaps, each with a reason
switchboard codex-review --include-low         report low-severity findings too
switchboard codex-review --no-rules            leave your rules and the repository's out
switchboard codex-review --reviewer claude     have Claude review even with Codex there
switchboard codex-review --model <name>        the reviewer's model, the one option passed on to it
switchboard codex-review --dry-run             print the prompt and the reviewer's command, and run nothing
switchboard codex-review --patch-id A B        print the change from A to B as one hash, and review nothing
```

Codex runs in its read-only sandbox, the commands it runs with no network and no way to ask for more, without your `~/.codex/config.toml` or the repository's `AGENTS.md`, and with the commit messages and the PR description given to it as data, not instructions. Without Codex, Claude Code reviews with the same prompt and JSON, with only its Read, Grep and Glob tools, so it runs nothing, and with no settings, MCP servers, skills or CLAUDE.md. `CODEX_REVIEW_VERBOSE=1` shows Codex's progress on stderr.

The reviewer is given the rules you work under, and reports a change that breaks one (category `rules`): your `~/.claude/CLAUDE.md` with what it imports, and the repository's `CLAUDE.md` and `AGENTS.md` as the base has them, which win where the two disagree. Settings' "Rules in reviews" (`review_rules`) turned off leaves them out, so your rules aren't sent to the reviewer.

`patchId` is the reviewed change as one hash, context lines included, so the same change rebased onto a newer base has the same one. It is `git patch-id --verbatim`'s hash (whitespace kept), worked out by `codex-review` itself with git's diff options fixed, so a git older than 2.39, which hasn't `--verbatim`, or a git config of yours gives the same one; `switchboard codex-review --patch-id <from> <to>` prints it for any two commits, as the pre-push hook asks for it. Once a loop is clean, `/review-loop` records the reviewed commit, its base and that hash in the clone's `review-loop-clean` (`git rev-parse --git-path review-loop-clean`), for a pre-push hook that asks for a review. Switchboard's own repository has one for `main`, which Settings' "Review before pushing" (`review_before_push`) turns off.

### Landing: `/queue-and-clean`

`/queue-and-clean` takes a branch from finished to gone. With a PR, it queues it for merge (the merge queue, or auto-merge), watches it in the background, and hands a failing check, a conflict or a drop from the queue to `/ci-loop` to fix, three times at most. Once it merges, it waits for the deploy and tests it as `/deploy-test` does. Once you approve the video, it marks the Linear ticket done, ticks the item in your note, removes the worktree and its local branch, and starts the tab's pane again in the main clone with the same conversation resumed, keeping the tab's name. With no PR, it checks whether the default branch already holds the work, a squash included, and cleans up if so. If not, it offers to open a PR, or, in a repository whose `CLAUDE.md` says its work is committed straight to the default branch, to land it there.

Landing on the default branch makes the branch one commit on top of the default branch as it is now, runs the tests, runs `/review-loop` on it and asks before pushing, all in each worktree at once. Only the push waits its turn: the worktree takes the landing lock, rebases onto what landed meanwhile and runs the tests again, and pushes with no second review while its change keeps the review's `patchId`, reviewing and asking again when it doesn't. Then it fast-forwards the main clone, ticks the note's item and cleans up as above.

The lock, and the queue of worktrees waiting for it, live in the main clone's git directory, shared by every worktree of it. `switchboard land-lock` keeps them:

| Command | What it does |
|---|---|
| `land-lock take` | Takes the lock for this worktree: exits 0 when taken (or held here already), or 3 when another holds it or others wait ahead, printing which, and joins the queue |
| `land-lock wait` | Takes it, waiting in the queue for this worktree's turn |
| `land-lock release` | Lets it go when this worktree holds it, and leaves the queue |
| `land-lock leave` | Leaves the queue |
| `land-lock holder` | Prints which worktree holds it, nothing when none does |
| `land-lock queue` | Prints the holder and each waiter, a line each: place, path, since when |
| `land-lock move <path> up\|down\|first` | Moves a waiter in the queue |
| `land-lock phase [words]` | Shows the landing's step in the tab; no words clear it |

The lock never expires with time, since a landing may wait on you while it holds it. It is taken over only from a landing whose worktree is gone or has no Claude session running in it, and a waiter gone the same way leaves the queue. The sidebar and mission control show the landing tab and those waiting under Landing, in the queue's order ("landing", "2nd in line to land"), and Enter on a waiting tab offers "Land it next", "Move it up in the landing queue" and "Move it down in the landing queue" (Settings' "Show the landing queue", `landing_queue`; off, they show as Working, and the queue still orders the landings).

### Deploys and videos

`/deploy-test` waits for a merged PR to deploy, tests the feature it adds on the deployed site in a real browser, records a captioned video of the test, and asks you to approve it. `/queue-and-clean` runs it before removing a worktree, and you can run it on any merged PR. It changes nothing real on the site unless the environment is a test one or you say so. `/preview-test` does the same before the merge, on an open PR's preview deploy, found from the branch's GitHub deployments, a bot's comment or the repository's docs.

Both wait with `switchboard wait-deploy`, which prints one line once the deploy settles: `deployed <sha> <url>` (`deployed <sha> -` when it watched a deploy workflow, which has no address), `failed <url>` or `timeout`.

```
switchboard wait-deploy --repo owner/name --sha <commit> --environment production
switchboard wait-deploy --repo owner/name --sha <commit> --environment Preview --ref <branch> --timeout 20
switchboard wait-deploy --repo owner/name --sha <commit> --workflow Deploy --branch main
```

`--environment` watches GitHub deployments to that environment (what Vercel, Netlify, Render and most deploy actions report), and `--ref` only those of one branch; `--workflow` watches a deploy workflow's runs on `--branch`. A deploy counts when its commit is the one given or a later one, since a merge queue often deploys a commit that includes it. It looks every 30 seconds, for `--timeout` minutes, 45 by default.

Both record with `/walkthrough`'s recorder. `/walkthrough` records a feature as a video to keep or share: from the site's home page through its own navigation, each step captioned, each click shown with a cursor, and the page's address above it. You look it over and keep it, record it again or drop it; a video you keep goes in `~/Movies/Walkthroughs/`. The recorder is a Node program using Playwright, kept with the skill, so it needs Node, the system's Chrome and `ffmpeg`, and its packages installed in the skill's scripts folder, which the skill does itself when they are missing:

```
npm ci --prefix ~/.claude/skills/walkthrough/scripts
```

Each site keeps a Chrome profile of its own in `~/.cache/deploy-test/profiles/<host>`. When a recording lands on a sign-in page, the skill asks you to sign in and opens a window for it; previews keep one profile per repository, since their hosts change with every PR. Claude never types a password.

### Watching a note: `/note-watch`

`/note-watch` keeps an eye on one note while the session runs, and works each checklist item you add to it, ticking it when done. It confirms the note first and reads no other. It watches with `switchboard watch-note <note file>`, which prints `NEW: <the item's text>` for each item added after it starts, once the item's text has stayed the same for twenty seconds, and `GONE: …` if the note is moved or deleted.

### On call: `/oncall`

`/oncall` watches your team's Slack alert channels while you're on call, started as `/loop /oncall`. It reaches you only through the Slack relay (`switchboard slack-relay notify`, so Slack notifies you of the relay bot's message): an urgent alert at once, the rest only if they are still a problem after about 30 minutes with nobody on them. It reads Slack through the Slack connector and posts nothing there. It needs the relay running and `~/.config/switchboard/oncall.json`, which is yours and never in a repository; `config.example.json` beside the skill shows its shape:

- `channels`: each alert channel's `id` and `name`, with an optional `watch` (a sentence saying which posts in it count) and `urgent` (a sentence saying what in it is urgent);
- `rules`: optional, the path of a Markdown file of your team's own rules, followed over the general ones;
- `repo`: optional, the GitHub repository (`owner/name`) runbooks and code are looked up in;
- `from`: optional, the name shown above each notice (`oncall` by default);
- `addressed` and `taking`: optional, your team's own reaction names for an alert that is handled and one someone is on.

### What the skills run

`switchboard skill-path <name> [<dir>]` prints the `SKILL.md` to follow for a skill in the repository at `<dir>`, the current directory by default: the worktree's own `.claude/skills/<name>/SKILL.md`, else the main clone's (a branch cut before the repository added the skill has none of its own), else your personal `~/.claude/skills/<name>/SKILL.md`. It prints nothing and exits 1 when there is none.

`/today` and `/slack-updates` save what they read for mission control's Calendar and Slack views, which can't reach Claude's connectors themselves. `switchboard today-save` takes the day as JSON on stdin and keeps it in `~/.local/state/switchboard/calendar.json`. `switchboard slack-updates-save --since` prints when to look back to (ten minutes before the last look, or an hour ago the first time) as three fields: that time in ISO 8601, the same in seconds, and how far back to look for your own messages, in seconds, and `switchboard slack-updates-save` takes what the look found as JSON on stdin and adds it to `~/.local/state/switchboard/slack.json`. `switchboard setting <key>` prints one of the workspace's settings as Settings reads it (`on`, `off` or a choice's value), which is how `/review-loop` reads `review_security`.

## The Slack relay

The Slack relay connects a Slack app's DM to the Claude sessions in your tabs, for when you are away from the computer. Each Claude tab gets a thread in the DM. What you reply there is typed into that tab, and the relay posts back when the tab finishes, waits on you, may be stuck, commits or pushes. The app's Home tab lists every tab, and from it you can open a tab's screen to message, answer, stop, rename or close it.

It runs in a tab of its own, named Slack relay. `M-S` opens that tab, or goes to it when the relay is already running, and M-p's "Stop the Slack relay" stops it. From a shell:

```
switchboard slack-relay [start]   open the relay in its tab, or go to that tab
switchboard slack-relay stop      stop it
switchboard slack-relay status    whether it runs, its process and its tab
switchboard slack-relay notify [--from <name>] [<text>]
                                  post a notice in the DM, from <text> or stdin
```

It needs the workspace's server running, and works from anywhere that server can be reached (a Claude job, ssh): a new tab goes in the session you used last. The tab runs `switchboard slack-relay run`, which starts the relay again if it crashes, inside an interactive shell so your shell's setup puts `claude` on the PATH for its Haiku calls. When the relay stops, the tab is left at that shell, and `M-S` starts the relay there again. It also needs Claude Code's hooks from `switchboard setup`, since that is how it hears what each session does.

`switchboard slack-relay notify` is how something else on the computer reaches you through the relay, such as the `oncall` skill. The notice goes out as the app's bot, under a 🔔 and the `--from` name (`notice` when none is given; up to 40 letters, digits, spaces and `. : _ -`), so Slack notifies you, which it never does for anything posted as you. The running relay posts it within a second, whether or not you are at the computer, and `notify` waits up to 45 seconds for it: it exits 0 once it is posted and 1 if it wasn't (the relay isn't running, or Slack refused), so a caller knows whether you were reached. It only ever posts in the DM.

When it starts, the relay says so in the DM with buttons for what you would otherwise type: **Home** (a link to the app's Home tab, added once Slack has sent the relay an event, since that is the only way it learns the app's ID), Status, Threads, Progress, New tab, Help, and Stop relay, which asks first. Stopping on purpose edits that message to say the relay stopped and when, and starting again within a day edits it back, so restarts don't fill the DM; a first start, an older message or a crash posts a new one. You stop it on purpose with `stop` in the DM, Stop relay, `switchboard slack-relay stop`, M-p's "Stop the Slack relay", Ctrl-C in its tab, or closing its tab.

### Threads and notices

Each Claude tab's thread is started the first time there is something to say about it, and holds everything about that tab: its finished turns, its waits, its milestones, notes that it may be stuck, and the answers to what you sent. The thread starts with a header naming the tab, its state and since when (🔴 waiting on you, 🔵 working, ✅ finished, ⚪ idle), and Progress, Skill, Rename and Close buttons. The header is edited whenever the tab is renamed or its state changes, so Slack's list of threads shows how every tab stands. Once the tab closes, or starts another Claude session, the header says its session has ended and offers **Delete thread**. A thread belongs to its tab for as long as the tab is open, and a new tab gets a new one.

The main DM gets one-line notices for what needs you or went wrong: a tab waiting on you, a dialog, a tab that may be stuck, a failed commit, push or pull request. Each is posted in the tab's thread and also sent to the conversation, so tapping it opens the thread. A notice for a prompt carries the prompt's option buttons, so you can answer from the notice itself. A finished turn gets no notice, since it is already in the thread.

A tab can hold other panes beside Claude. The relay keeps each tab to its own Claude pane: the first one it sees in the tab (or the one `new:` started) is recorded on the tab and stays its pane while the tab is open, through `/clear` or Claude being started again in it. While that pane isn't running Claude, another Claude pane in the tab stands in.

What it posts by itself:

- **Finished turns and waits**, from the states Claude Code's hooks record. An alert carries the turn, and for a tab that is waiting, the end of its screen, since a permission prompt and its numbered options are only there. A turn that ends in the tab you are looking at isn't marked finished in the workspace, since you saw it, but its thread gets it all the same. A tab that loops a skill (a Calendar or Slack tab) finishes a turn on every tick, so it alerts only when it waits on you.
- **Dialogs before a session starts** ("trust this folder", "new MCP server found"), which no hook reports, read off the screen, with the dialog's options as buttons.
- **Stuck tabs.** A tab marked working whose transcript hasn't changed for ten minutes gets a note in its thread (what it did last, and its screen) with **Stop** and **Progress** buttons, and a line in the main DM. A long test run looks the same, and the screen usually tells the two apart. Each stall is noted once.
- **Milestones.** One line in the tab's thread when a session commits, pushes or opens a pull request, and in the main DM too when one of those fails. A push counts only when git says where it went and that is not a local folder. Test runs are not reported, failing or not, since one failing is routine while Claude works, and neither is CI, which GitHub already notifies you of.
- **A full context.** Each turn posted ends with how full the tab's context is and its model, as do the tab's line on the Home tab and its screen. Once a session passes 90%, its thread says so once, with a **Compact** button that opens the Skill form with `compact` picked; it says so again only after the context has dropped below 90% and passed it again.
- **Offline.** The relay can't tell Slack anything when the computer is off, asleep or offline, so it leaves the warning with Slack ahead of time: a message saying the relay is offline, scheduled in the DM about ten minutes ahead and renewed every two minutes. If the relay goes quiet, nothing pushes it back and Slack sends it, which notifies you. When the relay is back it says so, and for how long it was gone. A clean stop cancels it.
- **Crashes.** A relay that crashes is started again after a wait that grows each time (2 seconds, then 10, 30, 60 and 120), and posts that it restarted and why. After five crashes in ten minutes it posts that it is down and gives up.

Everything it posts is standard Markdown, the way Claude writes, so headings, **bold**, links, lists, tables and code show as they do in the terminal. An answer longer than one Slack message goes out as several.

### What you can send it

A message reaches a tab only as a reply in that tab's thread, or through a button or form made for it. A message never names a tab, so nothing at the top level starts work or reaches a tab by accident.

| You send | It does |
| --- | --- |
| a reply in a tab's thread | types it into that tab's Claude |
| `?` in a tab's thread | shows the tab's last turn: your last prompt and Claude's reply since, from its transcript (the screen if there is none) |
| `progress` in a tab's thread | what that tab is doing, without typing anything into it: its state, when the turn began, when it last did anything and how many tools have run, then a few bullets from a Haiku that read the turn's activity and the screen |
| `esc` in a tab's thread | presses Escape there, which stops Claude mid-turn or declines the prompt it waits on; a tab that isn't working or waiting is left alone, since a second Escape opens the rewind menu |
| `rename <name>` in a tab's thread | names the tab, as "Rename it" in its menu in mission control does; `rename` alone gives it back its own name |
| `status` or `?` | every Claude tab, its state, for how long, how full its context is, and a link to its thread |
| `threads` | a link to each Claude tab's thread; a tab with no thread yet says so |
| `progress` or `progress all` | the same as `progress`, for every tab at once: one line each, most urgent first |
| `new: <prompt>` | a new Claude tab with that prompt; its confirmation is its thread |
| `new <place>: <prompt>` | the same, in a place: a repository's name for its main checkout, a ticket ID or any part of a branch name for the worktree that matches, or a path starting `~` or `/` |
| `cleanup` | offers to delete the threads of closed tabs |
| `help` | the list of commands |
| `stop` | stops the relay |

`new <place>:` looks names up among the repositories your open tabs use and those a Claude session has run in over the last two weeks. A name that matches several places, or none, starts nothing and says so, and the reply names the directory it picked.

Anything else at the top level is not acted on, so a typo never starts work: the reply has a button per tab, and tapping one sends the message there, the conversation going on in that tab's thread.

When the relay types your message into a tab, it posts nothing. Your message's reaction says where it is: ⏳ while it waits behind a turn already running, 👀 once the tab's Claude takes it, ✅ once the answer is posted. Meanwhile a "Working on it" message with **Progress** and **Stop** sits where you asked, and is deleted once the answer is posted. If a tab that isn't busy hasn't taken your message after 30 seconds, the relay posts the tab's screen, since a dialog is usually what is in the way. It knows a tab has answered from the events Claude Code's hooks append while the relay runs, so it posts the turn within a couple of seconds; a session started before the hooks were set up is taken as done once its screen has stayed still for 20 seconds.

**Prompts and questions.** When a tab waits on a permission prompt or a question, the alert shows it and its numbered options, read from the screen, with a button for each. Tap one, or reply with its number or the start of an option. Only the number is pressed, with no Enter after it, since an Enter could accept a second prompt that follows. Anything else is refused while the choice is showing, rather than typed, since it would end with Enter and accept whichever option is selected; the refusal carries the option buttons. A button answers only the prompt it was made for, while that same wait is on, and is replaced by what was chosen once pressed. Answering a prompt anywhere (a button, a typed reply, or at the computer) clears the buttons from every other message showing it. A dialog before a session starts is answered the same way, the relay moving to your option with the arrow keys and pressing Enter.

A question Claude asks you itself (AskUserQuestion) works the same, with more to it. When it asks several at once, Claude Code shows them one at a time in the same wait, so after each answer the relay looks at the screen again and posts the next question, and then the review of your answers, each with its buttons. A question that takes several answers shows its options ticked ☑ or not ☐: a tap or a number ticks or unticks one, `1 3` does two, and **Submit choices** (or `submit`) moves on. Any other reply to a question Claude asked is typed in as its "Type something" option, your own answer.

**Quick replies.** A finished turn has a **Skill** button. When Claude ended by asking, the replies you would likely tap are added a few seconds later: **Yes, go ahead** when it asks whether to go on with what it proposed, and a button per option when it asks you to pick one of a few it named. A question mark alone can't tell those from a question only your own words answer, so a Haiku reads Claude's last message (only one with a question mark near its end), and anything else gets no buttons. A tap types the reply into the tab, and is refused once Claude has said more since.

**Skill.** Slack takes any message starting with `/` for one of its own commands and never sends it, so `/review-loop` can't be typed into a thread as it is at the keyboard. Skill opens a form instead: a menu of the skills that tab can run, and an Arguments field for what follows the name, such as a ticket ID. Picking a skill redraws the form to say what it takes, from the `argument-hint` in its file. The menu is the tab's own: the skills its session lists, plus those only you can invoke, from `~/.claude/skills` and `~/.claude/commands` and from `.claude` in the tab's directory and the ones above it. It is grouped frequently used (the eight you use most, from the counts Claude Code keeps, each use counting half as much every 30 days), yours, the project's, Claude Code's own commands that finish without asking anything (`compact`, `clear`, `context`, `cost`, `usage`), then plugins and built-in skills. A Slack menu holds 100 options, so a longer list is cut from the end, and a field takes any name the menu left out. A thread reply that starts with a space also gets past Slack.

### The Home tab and a tab's screen

The app's Home tab opens with when the computer last checked in, a time Slack itself renders and keeps counting ("checked in 47 minutes ago"); the relay publishes it every minute while it runs, so anything older than two means it is offline. **Refresh** beside it publishes at once, and **New tab** asks where and for the prompt, as `new <place>:` does: Where is a menu of every checkout a Claude session has run in that still exists, and every open tab's, newest first, grouped by repository, with a field beside it for anything else. **Delete N closed threads** appears while there are some.

Then every Claude tab, in groups so what needs you is at the top on a phone: **Needs you** (waiting on you, then finished and not yet looked at, the longest first), **Working** and **Idle**. Each has its state, for how long, its last activity, and one line from Haiku on what it is doing or, once it has stopped, where it left off, written again when the tab has done something or changed state, at most once a minute. A tab waiting on a prompt shows the prompt's options, which answer it from there. The relay looks every 10 seconds (every minute with Settings' "Look often", `poll_fast`, off) and publishes when anything changed, at least once a minute, and whenever you open the tab. The bottom says what the Haiku calls have cost today, from what `claude -p` reports for each. When the relay stops on purpose, the Home tab says so, since nothing in it is current any more.

**Open** on a tab shows its screen: its state, its last prompt, the end of its last reply, the options of a prompt it waits on as buttons, and these buttons:

| Button | Does |
| --- | --- |
| Message | a field whose text is typed into the tab, handled as a reply in its thread would be; the answer goes in the thread |
| Skill | the Skill form |
| Go to thread | the tab's thread, once it has one |
| Progress | `progress`, shown on the screen |
| Rename | a field holding its name; cleared, the tab gets its own name back |
| Stop | Escape, only while it works or waits |
| Close | closes the tab after Slack asks, ending its Claude session; `switchboard claude-here` in its directory resumes it |

An open screen is drawn again whenever its tab's state changes, so a prompt answered at the computer leaves it. Every button names the tab and its Claude's process, so a screen or a Close button left from a tab that has closed can't act on a tab that has taken its place.

Slack doesn't let you delete the app's messages in its DM, only your own, so the relay deletes a closed tab's thread for you: **Delete thread** on its header, or **Delete N closed threads** on the Home tab and under `cleanup`, each confirmed by Slack first. It deletes the header and every message the app posted in the thread, with the files it uploaded. Your own replies stay, since Slack doesn't let an app delete them. Only threads of tabs that have closed can be deleted, and nothing deleted can be brought back.

Settings' "Haiku summaries" (`haiku_summaries`) turns off every Haiku call the relay makes, each a paid one: the Home tab keeps the lines it has, `progress` says why it has no summary, and finished turns get no quick replies. Each Haiku runs with no tools, MCP servers or settings and no session kept, so nothing in a transcript can make it act.

### Images and videos

An image attached to a message you send, such as a screenshot from your phone, goes to the tab with your text: the relay downloads it to `~/.local/state/slack-relay/files/`, readable only by you, and adds `[Image: <path>]` to what it types, which Claude Code reads as the picture. Only PNG, JPEG, GIF and WebP up to 20 MB, only from Slack's own file host, and only on a message you typed; anything else is answered, not fetched. A video (MP4, MOV or WebM, up to 200 MB) is downloaded the same way, and since Claude can't watch one, the relay takes eight stills spread through it with `ffmpeg`, at most 1280 pixels wide, and types `[Video: <path>]` and then the stills. Without `ffmpeg`, or for a file it can't read, the tab gets only the path and you are told. Downloads are deleted after a week.

Images and videos a tab makes come back the same way. When a turn is posted, the files Claude's own messages in it name (a Playwright screenshot or recording, a chart it saved) are uploaded under it, but not ones only its tool calls name. Only files that really are PNG, JPEG, GIF, WebP, MP4, MOV or WebM by their contents, written during that turn, up to 20 MB for an image and 200 MB for a video, up to 20 a turn, ten to a message, and never one you sent. A PNG more than twice as tall as it is wide, such as a long diff's screenshot, is shared as slices about as tall as they are wide, in order, since Slack shrinks an image by its longest side until a tall one's text is unreadable.

### While you're at the computer

The tabs' threads always get everything at once, so each is a complete record whether you were at the computer or not. What notifies you is the one-line notice in the main DM, and those wait while you are at the computer, since the workspace already shows them, and go out in the order they happened once you have been away 30 seconds. You count as away once nothing has been typed or clicked for 30 seconds, or at once when the screen is locked; where the computer can't say (a Linux with no desktop session), you count as at it. Writing to the relay from Slack counts as being away for five minutes from your latest message, and sends what was held, unless the keyboard or mouse was used in the five seconds before it arrived, which means you wrote it in Slack on the computer itself. Replies to what you send, and the results of requests you made from Slack, never wait. Held notices are kept in memory, so stopping the relay drops them.

Slack pushes each message to the relay the moment you send it, so it answers in about a second and costs nothing while you are quiet. Messages sent while its connection to Slack was down are read back from the DM and answered. One sent more than a minute before the relay started is answered with a note that it was missed, so you can send it again if it still stands. The relay keeps the computer from sleeping while it runs, but not its displays. A MacBook with its lid closed and no display connected sleeps anyway.

A lock keeps it to one relay at a time. Its files are in `~/.local/state/slack-relay/`, readable only by you, where earlier versions kept them, so a relay carried over keeps its threads: `state.json`, the hooks' `events.jsonl` (removed when the relay stops on purpose, so turns that end while it restarts are not lost), `relay.lock`, the `outbox/` that `notify` writes to, the `files/` you sent, `haiku.json` with today's Haiku cost, and `relay.log`. The log keeps what the tab shows: where each post went and whether Slack notified you of it, when you count as at the computer or away and why, each notice held, and how long each message waited to be handled. The same failure again within ten minutes is counted, not written, and three old logs are kept beside it, `relay.log.1` to `.3`.

### The Slack app, and keeping it yours

The relay is a Slack app of your own, which you talk to in its Messages tab. Create it at api.slack.com/apps, from this manifest:

```yaml
display_information:
  name: Slack relay
features:
  app_home:
    home_tab_enabled: true
    messages_tab_enabled: true
    messages_tab_read_only_enabled: false
  bot_user:
    display_name: Slack relay
    always_online: false
oauth_config:
  scopes:
    bot:
      - chat:write
      - im:history
      - im:read
      - im:write
      - reactions:write
      - files:read
      - files:write
settings:
  event_subscriptions:
    bot_events:
      - app_home_opened
      - message.im
  interactivity:
    is_enabled: true
  socket_mode_enabled: true
```

That is Socket Mode on, so Slack connects through a connection the relay opens and nothing on the computer listens for connections; the bot scopes for posting, reading and opening its DM, reacting to your messages, and fetching and uploading files; only the `message.im` and `app_home_opened` events; Interactivity for the buttons; and the Home and Messages tabs, with messages allowed from the Messages tab. Install it to your workspace, then make an app-level token with the `connections:write` scope (Basic Information, App-Level Tokens).

Store both tokens as secrets, never in a file of your own. Each command reads the token from stdin, so it never appears on a command line: run it, paste the token, and press Ctrl-D.

```sh
switchboard platform secret-set slack-relay-bot-token    # xoxb-…
switchboard platform secret-set slack-relay-app-token    # xapp-…
```

On macOS they go in the Keychain, where the earlier relay's tokens are found as they are. On Linux they go in the desktop's keyring (GNOME Keyring, KWallet) when it is there and unlocked, else in `~/.config/switchboard/secrets.json`, readable only by you.

Anyone in the workspace can open a DM with an app, so the relay acts only on messages typed by one member, named in `~/.config/switchboard/slack-relay.json`:

```json
{ "owner": "<your Slack member ID: Profile → ⋯ → Copy member ID>" }
```

Without the owner or the tokens, the relay doesn't start, and its log says what to set up. Everything else is ignored and logged, with no reply, so the app doesn't even say it is listening: other members, other conversations, other workspaces, bots (the relay's own posts included), edits and deletions. A message has to have been typed, too: other apps can post as you (anything holding your Slack user token, the Slack connector your Claude sessions use included), so a message counts only if it carries the `client_msg_id` Slack's own apps give what you type, which the Web API doesn't let an app set, and no `app_id`. Otherwise a Claude session that read something malicious could post as you in a tab's thread and drive that tab. For the same reason reactions control nothing: Slack can't say whether you added one or an app did as you. Buttons are trusted the same way: a press counts only if it is yours, on a message in that DM, or on the Home tab or a tab's screen, which take only their own buttons, and Slack only reports a press a person made, so no app can press one as you. A form (Message, Skill, Rename, New tab) counts only if you sent it from one of the relay's own screens. The owner comes from the file, never from a message. Everything the relay writes is refused before it leaves the computer unless it is for the app's DM with you, or its Home tab as you see it, whatever asked for it. The app subscribes to no channel events and has no scopes to read channels, so adding it to one shows it nothing.

## Staying up to date

`switchboard update` downloads the newest release of Switchboard for your computer, checks it against the SHA-256 published beside it, runs it once to see that it starts, and only then puts it in place of the binary you ran (a link to the binary stays a link). Anything that fails before that leaves your binary as it was. Then it runs the guided `switchboard setup`, for what the new version brings. `switchboard update --check` says whether there is a newer release, and what it brings, and changes nothing.

The running server goes on with the old version until it starts again: `switchboard kill-server`, then `switchboard`, and your tabs come back as they were.

A release build of Switchboard looks for a new release by itself: each time a terminal attaches, and every hour. When there is one, a box lists the releases newer than yours, newest first, each with its age and notes, over two options. "Update now" opens a tab that runs the update and its setup and stays until you press Enter. "Not now", or Esc, puts it off: the hourly look doesn't ask about that release again, only about a newer one, while the look as a terminal attaches asks whenever something newer is out. A box left unanswered is asked about again the next hour, and a newer one takes its place. Settings' "Update check" (`update_check`) chooses when it looks: "open, hourly" (`hourly`, the default), "open only" (`open`), or "off" (`off`).

`M-p`'s "Update Switchboard" looks now, whatever that setting says, and says so when there is nothing newer or it can't look. `M-p`'s "Run Switchboard's setup" opens a tab running the guided `switchboard setup`, to put in place what isn't, or to say yes to a part you said no to before; "Check Switchboard's setup" and "Run a part of Switchboard's setup" are in [More in the palette](#more-in-the-palette).

When `~/.config/switchboard/claude.json` names a git repository for your Claude rules, each look the setting allows (as a terminal attaches, every hour, or asked for) also pulls that repository's clone, fast-forward only, with `switchboard setup claude-md`, which writes `~/.claude/CLAUDE.md` again if it must. This happens whether or not your build looks for releases, since your rules change apart from Switchboard. It never asks for a password: git's prompt and askpass are off, and so is ssh's askpass, so a pull that needs one fails quietly and is tried again at the next look.

A `switchboard` you built from source with cargo is kept current by git and cargo: its server never looks for releases, and `switchboard update` (and `M-p`'s "Update Switchboard", which says what is out and how to take it) leaves it alone unless you add `--force`. After each build, run `switchboard setup`: the skills, Claude Code's settings and Switchboard's part of CLAUDE.md are compiled into the binary and reach `~/.claude` only through the setup. The first time a new build attaches (`switchboard`), it does this by itself, quietly, for the parts you have set up, and records the build in `~/.local/share/switchboard/setup-build` so it does it once; a part you said no to is left for the guided setup to ask about. A run with a state directory of its own (`SWITCHBOARD_STATE_DIR`, as a try of a build is) leaves `~/.claude` as it is. `switchboard setup --refresh` does the same by hand.

### Trying a change

`switchboard try` tries a clone or worktree of Switchboard's source without touching the workspace you use:

```text
switchboard try              the worktree you are in, attached in this terminal
switchboard try <dir>        another worktree
switchboard try --print      set it up and print the environment for another
                             terminal: eval "$(switchboard try --print)", then
                             switchboard
switchboard try --detached   start its server with no client and print the
                             command that reaches it
switchboard try --clean      end its server and remove its home (name a
                             removed worktree by the path it had)
switchboard try --clean-all  the same for every try of yours
```

It builds the worktree with cargo (nothing to do when the build is up to date), then runs that build under a home of its own, `/tmp/sb-<hash>`: its own settings, state, server and socket, and its own `~/.claude` hooks, CLAUDE.md and skills, which the build's `switchboard setup claude claude-md skills` lays out there, so setup is tried too. Its `~/.local/bin/switchboard` is the build, first on its PATH, so Claude Code's hooks and statusline in its panes reach the try's server. Running it again sets the home up again and keeps what the try saved. Attaching from one of Switchboard's own panes is refused, since the workspace around it would take every key: use a terminal of its own, `--print`, or `--detached`, which is for a Claude working in the worktree: it prints the prefix to put before `send-keys`, `capture-pane` and `list` to press keys in the try and read its screen.

What isn't Switchboard's comes from your real home. Linked: `~/Library/Keychains` on a Mac (without it Claude Code isn't signed in and the Keychain's secrets aren't found), your dotfiles (`.ssh`, `.gitconfig`, shell history), your `~/.config` folders but Switchboard's (gh's sign-in among them), `~/.local/share` but Switchboard's, the programs in `~/.local/bin` that aren't Switchboard's, Claude's plugins and its sign-in on Linux, and your own Claude skills and hooks. Copied: your shell's startup files (each ending by moving any XDG, notes or Switchboard path it set in your home to the same place in the try's), `~/.claude.json`, `~/.claude/settings.json` and `settings.local.json`, `~/.config/switchboard` and the clone your CLAUDE.md's rules come from, so nothing the try does writes to yours. Fresh: `~/.local/state`, `~/.cache`, Claude's sessions and transcripts, and `~/Notes`. Its settings turn off what would duplicate your workspace or spend on its own: the Slack look and view, meeting, standup and todo notes, banners, the update check and restoring saved tabs; turn any back on in its Settings. Linked things are yours, so an edit made through them (a git config, gh signing in again) is made to the real one, and keys sent in a try act with your real credentials.

Since its build and setup run with those credentials, it tries only code of yours: a worktree whose `.git` leads back to it, whose commits beyond its origin's branches are on no other remote and were written and committed under your git email, and which isn't partway through a merge, rebase, cherry-pick or revert. Anything else, such as a fork's pull request checked out with `wtpr`, is refused, saying why, until you have read it and listed its path in `~/.config/switchboard/trusted-dirs`.

## Your own files

Everything of your own is in `~/.config/switchboard/`, which nothing writes over but Settings, so an update leaves it be. Each file is read after Switchboard's defaults, so what it says wins, and Settings lists each setting in these files under the file's name.

| File | What it holds | Where it is described |
|---|---|---|
| `settings.json` | What you changed in Settings, only the settings that differ from their defaults; you may edit it by hand | "Every setting", below |
| `keys.json` | Keys bound to other actions, or unbound | "Changing the keys" |
| `sidebar.json`, `sidebar-plugins/` | The sidebar's and mission control's sizes, and the plugins that load | "sidebar.json", "Plugins" |
| `browser.json` | The Chrome profile each site's links open in, and the default one | "Where links open" |
| `linear.json` | Your Linear workspace, and the team and labels `/shape` files tickets with | "GitHub and Linear access" |
| `CLAUDE.md`, `claude.json` | Your own Claude rules, or where they come from: another file or a git repository | "Your CLAUDE.md" |
| `slack-updates.json` | The Slack channels the Slack view shows in full | "The Slack view" |
| `slack-relay.json` | Your Slack member ID, the one person the relay listens to | "The Slack app, and keeping it yours" |
| `oncall.json`, `oncall-rules.md` | The channels `/oncall` watches, and your team's rules | "On call: `/oncall`" |
| `trusted-dirs` | The directories whose own code may run, one a line: `switchboard try` builds a worktree holding someone else's commits only once it is listed | "Trying a change" |
| `secrets.json` | On Linux with no keyring, the secrets `switchboard platform secret-set` stores, readable only by you | "GitHub and Linear access" |

What Switchboard keeps for itself is in `~/.local/state/switchboard/` (the socket, the saved layout, the tips shown, what it counted, the day's files) and its caches in `~/.cache/switchboard/` and `~/.cache/wt-revive/` (what each Claude transcript says of its worktree, for `wt-revive`); the Slack relay's state is in `~/.local/state/slack-relay/`, and `/oncall`'s in `~/.local/state/oncall/`. `~/.local/share/switchboard/` holds what setup writes for Claude Code: Switchboard's part of your CLAUDE.md (`switchboard.md`), the clone of your rules' repository (`claude-md/`) and the build whose parts were last put in place (`setup-build`). None of it is yours to edit, and any of it can be deleted while the server is stopped, at the cost of what it held.

## Every setting

Mission control's Settings view (`M-p`, then "Settings: turn the workspace's options on and off") lists every option below under its category; Enter or Space changes one, and `/` filters them. A new feature comes with a setting, on unless it sends something of yours somewhere, so each part can be turned off for a plain workspace. `switchboard setting <key>` prints one's value, `switchboard setting <key> <value>` changes it (`on` or `off`, or one of its choices; anything else is refused), and `--reset` gives it its default back.

### Integrations

| Setting | Default | What it does |
|---|---|---|
| GitHub (`github`) | on | PR commands in M-p, the PRs view, PR numbers and merge states in tabs and the Work map; off, the workspace asks GitHub nothing by itself (programs you run, such as codex-review and wtpr, still do) |
| Linear (`linear`) | on | Ticket commands in M-p, ticket titles in tab names and tickets in the Work map; off, the workspace asks Linear nothing by itself (programs you run, such as wt and worktree-done, still do) |
| Docs in the link view (`link_view_docs`) | on | The link view reads a Notion page or a Google Doc or Slides deck through Claude's connectors, a headless Haiku fetching it; off, they are read as any other page, which a private one won't let it |
| Chrome profiles (`chrome_profiles`) | on | Links Switchboard opens (mission control, the link view, M-r, M-l, switchboard open-url) open in the Chrome profile browser.json gives their site, else its default profile; off, every link opens in the default browser |
| Standup note (`standup_note`) | on | Each weekday from 6am, a note in Standups written from the last working day: the PRs you merged, opened, worked on and reviewed, your tickets that changed state and your commits, then what is in progress today and the reviews waiting on you |
| Standup summary (`standup_summary`) | on | Over the standup note's list of the last working day, two or three sentences summing it up, written by Claude (Haiku, no tools) from the PR, ticket and commit titles in that list |
| Today's todo note (`todo_note`) | off | Each weekday at the time below, a note Todos/Today's Todo <date> that Claude (Sonnet) writes from your notes' unticked items, today's calendar, and what mission control last read of GitHub, Linear and Slack; off by default, since it sends your notes' unticked items to Claude |
| Todo note time (`todo_note_time`) | 9am | When Today's todo note is written each weekday, or as soon as the computer is on after it |
| Meeting notes (`meeting_notes`) | on | Each meeting of yours Fathom recorded as a note in Meetings, with its summary, action items and transcript, through Claude's Fathom connector (a Haiku call every 30 minutes from 7am to 9pm, and two for each new meeting) |
| Search everything (`search_everything`) | on | M-S-f asks for a search and lists what your notes, Linear, GitHub and Slack hold for it, each read beside the pane with Enter |
| Slack in search everything (`search_slack`) | on | M-S-f searches Slack too, through Claude's connector, a headless Haiku making that one call, its results arriving a few seconds after the others |

### Notifications

| Setting | Default | What it does |
|---|---|---|
| Push notifications (`notifications`) | on | A desktop notification, named for the tab, when a Claude tab waits on you |
| Finished tabs clear (`done_clears`) | when seen | When a tab's finished mark goes: once you look at the tab, or only when you next prompt it |

### Sidebar

| Setting | Default | What it does |
|---|---|---|
| Sidebar (`sidebar`) | on | The sidebar on the left of every window; M-s still hides or shows it for now |
| Half-page scrolling (`sidebar_half_page`) | on | C-d and C-u in the sidebar scroll its list half a screen down or up, the cursor moving with it, as in Vim; off, they run in the chosen tab as other Ctrl keys do |
| Reminders (`reminders`) | on | M-p's Remind me about this tab: the tab waits under Reminders with the time left, then moves to Needs you with a banner when it comes due; off, reminders are neither set nor shown |
| Build failing (`build_failing`) | on | A Build failing group in the sidebar and mission control for the tabs whose open PR has a failing check, asked of GitHub every minute, an idle one wearing a red ✗; off, they stay in Working or Idle with the failing count in their words #github |
| Show the landing queue (`landing_queue`) | on | Tabs landing on the default branch with queue-and-clean, or waiting their turn, get a Landing group in the sidebar and mission control, in the landing queue's order, which a waiting tab's menu changes; off, they show as Working, and the queue still orders the landings |
| Notifications (`sidebar_notifications`) | on | The Notifications block at the bottom of the sidebar: new Slack messages and your Linear inbox, which a click opens in mission control |
| M-a goes to notifications (`needs_notices`) | on | M-a's walk of what needs you ends at the notifications while there are any, after the tabs, opening them as the sidebar's n does |
| Notifications window (`notice_list`) | on | Moving the sidebar's cursor onto a notification, or clicking one, brings the Notifications window to the front as moving onto a tab does: a tab of its own listing every one with its whole message, Enter opening its menu and Esc going back, as M-p's "Go to the notifications" does too; off, Enter or a click on one in the sidebar does what it leads to |
| New notes (`new_notes`) | on | In the sidebar's notifications, a note written for you (a meeting's from Fathom, the standup) until you read it, Enter offering to open it in Notes |
| Next meeting (`next_meeting`) | on | In the sidebar's notifications, from five minutes before a meeting until ten after it starts: its name, the tickets and PRs it mentions, and Enter or a click to join its call |
| Calendar changes (`calendar_changes`) | on | In the sidebar's notifications, what changed on today's calendar since you last looked at it: an event added, moved, renamed or gone; looking at the Calendar view clears them |

### Sessions

| Setting | Default | What it does |
|---|---|---|
| Work a note continuously (`autopilot`) | on | With it on, M-p's 'Work the note continuously' keeps tabs working through a note's checklist, opening the next item with wtn as each lands; the mode itself starts off until you turn it on there. Off, it opens no tabs, and its line in mission control and its notification go |
| wtpr asks where the PR stands (`wtpr_prompt`) | on | Claude, started in a PR's worktree by wtpr (or mission control's "work on it in a new tab"), first reads the PR, its review threads, failing checks and ticket and says where it stands and what it would do next, changing nothing until you say; off, it starts with no prompt |
| Save what panes show (`save_contents`) | on | Keep each pane's screen in the layout saves, to show again after a restart; it can hold secrets |
| Restore on start (`auto_restore`) | on | Bring back the last saved tabs and panes, each Claude resumed, when the server starts with none open |

### Diff

| Setting | Default | What it does |
|---|---|---|
| Files in reading order (`diff_order`) | on | M-d lists the changed files flat, documentation first and tests last, with Haiku ordering the code from most abstract to most concrete; off, flat, A to Z |
| Diff told as a story (`diff_story`) | on | M-d's file list grouped into a few chapters Haiku names from the whole diff and the commits' messages, each with a heading and two or three sentences on what it does and why it is built so, read from the change's core to its edges ([c and ]c move between them, yS copies the story as Markdown); off, the files keep the reading order by kind. Sends the diff's text and the commit messages, not only the paths, to Claude |
| Diff story progress (`diff_story_progress`) | on | While M-d's story is told, its cover shows what Haiku is reading, each chapter as Haiku names it and the time taken, t showing Haiku's answer itself as it comes; off, a spinner alone |
| Diff story told ahead (`diff_story_ahead`) | on | When a tab's branch has a new commit, its M-d story is told in the background, so M-d opens on it at once rather than waiting under its cover; sends that branch's diff and commit messages to Claude (Haiku) for each new commit, a run taking up to five and a half minutes |
| Diff file list wide (`diff_list_wide`) | on | M-d's file list opens wide enough for its longest path (at most half the screen), E narrowing it; off, it opens narrow and E widens it |
| Diff story a page at a time (`diff_story_pages`) | on | ]p and [p in M-d's diff open the story's chapters one at a time, each a diff of its own as if it were a commit, [p from the first going back to the whole diff; off, they do nothing |
| Ask Claude about the diff (`diff_ask`) | on | ga in M-d's diff, and M-p's command while the diff is open, ask Haiku a question about the change: it reads the story, the diff and where the cursor is, and may read the repository's files to answer, its answer streamed into a window beside the diff. Sends the diff and the question to Claude |
| Diff story notes on changes (`diff_story_notes`) | on | M-d's diff shows Haiku's note over each change worth one, what it does and what to check, and names the chapter of a change that belongs to another one than its file; off, only the list's chapters |

### Mission control

| Setting | Default | What it does |
|---|---|---|
| Click in menus (`menu_mouse`) | on | A click takes an option of mission control's menus, Yes or No of a question or a numbered answer, and a click outside the box closes it; and a click picks an option of M-m's, the update box's and M-w's menus; off, they take keys alone |
| Tab moves focus (`tab_focus`) | on | Tab and Shift-Tab move a highlight through what can be clicked, as on a webpage, and Enter presses it (Space too, but in the sidebar, where Space peeks): in mission control the views' names, the filter and the items, with [ ] switching views; in the sidebar each of its rows, M-Space opening mission control; and the options of every box and menu; off, Tab switches views in mission control and opens it from the sidebar |
| Highlight on hover (`hover_highlight`) | on | What a click would press lights under the mouse: in mission control the views' names, the filter, the cards and rows, a menu's options, Yes and No, an answer's options and easy mode's ×; the sidebar's tabs and notifications; the options of M-m's, the update box's and M-w's menus; and the buttons of Notes' toolbar, its boxes' × and M-d's diff; off, nothing follows the mouse. Takes effect at once in mission control and the sidebar, and when a menu, Notes or the diff next opens |
| Work map (`view_work_map`) | on | Mission control's Work map: every piece of work in your tabs' repositories (M-j and M-o open it whatever this says) |
| Calendar (`view_calendar`) | on | Mission control's Calendar view: today from the day /today last saved |
| Slack (`view_slack`) | on | Mission control's Slack view: your mentions, DMs, thread replies and chosen channels from the last day, as /slack-updates saved them |
| Closed tabs (`view_closed`) | on | Mission control's Closed view: the tabs closed last, to reopen |
| Sessions (`view_sessions`) | on | Mission control's Sessions view: each Claude session's context, model and age |
| Resources (`view_resources`) | on | Mission control's Resources view: a table of the CPU and memory each tab's processes use, heaviest overall first, read with ps every few seconds only while the view is in front |
| Requests in Usage (`request_audit`) | on | The Usage view lists today's requests to services outside this computer (GitHub, Linear, Slack, Anthropic and the rest), by protocol and by service, as Switchboard counts them; off, it leaves them out |
| Usage (`usage`) | on | Mission control's Usage view: today's Claude spend and how much of the rate limits is used |
| RSVP from the Calendar (`calendar_rsvp`) | on | Answer an invitation yes, maybe or no from its Enter menu in the Calendar view, through Claude's Google Calendar connector |
| Slack without a tab (`slack_background`) | on | Mission control looks at Slack itself every minute, through cheap headless Haiku calls, instead of a Slack tab looping /slack-updates |
| Pause lookups while away (`pause_when_away`) | on | Stop asking GitHub, Linear and the calendar for mission control's views, and telling the diff's stories ahead, while no terminal is attached, or the computer is locked or has been idle 10 minutes, and catch up on return |
| Look often (`poll_fast`) | on | Ask GitHub and Linear every 15 seconds (the PR board every 30) and look at the Slack Home tab every 10, in place of every one or two minutes |
| PR board (`pr_board`) | on | Mission control's PRs view, which asks GitHub about your pull requests every two minutes |
| Ticket board (`ticket_board`) | on | Mission control's Tickets view, which asks Linear about the issues assigned to you every two minutes |
| Inbox (`view_inbox`) | on | Mission control's Inbox view and the sidebar's count of it: your unread Linear mentions, comments and assignments, asked of Linear every two minutes #linear |
| CI (`view_ci`) | on | Mission control's CI view: your pull requests with failing checks and why, asked of GitHub every two minutes #github |
| Merge queues (`view_merge_queues`) | on | Mission control's Merge queues view: every pull request in the merge queue of each repository your tabs are in, in queue order, with the tab that has it open, asked of GitHub every minute #github |

### Tabs and status bar

| Setting | Default | What it does |
|---|---|---|
| One Claude to a tab (`one_claude_per_tab`) | on | A Claude session started in a tab already running one moves to a tab of its own, since the sidebar, the Slack relay, M-c and M-m each take a tab's Claude to be one pane |
| Name new tabs (`name_new_tabs`) | on | M-t and M-n ask what to call the tab they open: a name typed in, its ticket and the ticket's title, or shaping a ticket, each optional; off, the tab is named after its directory |
| Messages in a box (`message_boxes`) | on | What a key or script has to say shows in a small box in the middle of the screen, which any key closes, rather than on the status line for a moment |
| Second status line (`second_status`) | off | The line under the tabs with the tab's name, branch, PR and Claude session |
| PRs and tickets in tab names (`tab_lookups`) | on | Look up each tab's PR number on GitHub and its ticket's title on Linear, in the background |
| Project colors (`accent_colors`) | on | A color of its own for each project's tab, made from its directory; off, every tab is gray |
| Health light (`health_light`) | on | A red mark in the status bar when saves, the sidebar or the Slack relay quietly stop |

### Notes

| Setting | Default | What it does |
|---|---|---|
| Hide the sidebar beside a tab (`notes_hide_sidebar`) | on | When M-e brings Notes beside another tab's pane, put the notes app's sidebar away so the note has the room, and show it again in Notes' own tab |
| Notes command palette (`notes_palette`) | on | Ctrl-Shift-P in Notes lists every command Notes has, each with its key; off, Ctrl-Shift-P says the palette is off |
| Ctrl-V pastes an image (`notes_ctrl_v_image`) | on | Ctrl-V while typing in a note pastes the clipboard's image under the line, as in Claude Code, when the clipboard holds one; off, or with no image there, Ctrl-V types the next key as it is, as in Vim |
| Images drawn in a note (`notes_images`) | on | An image pasted into a note (Format menu, Paste image) drawn under the line that names it, in a terminal that speaks the kitty graphics protocol (Ghostty and kitty do); off, it is its name, which Enter opens in Quick Look or the desktop's image viewer |
| Toolbar (`notes_toolbar`) | on | A row of buttons over each note, one for each of the Format menu's styles (bold, italic, headings, lists, checklist, table, link, image) and ⋯ for the whole menu, each acting on the line or the selection; off, the Format menu is Ctrl-P alone |
| Tell a note's new tab its note (`note_tab_prompt`) | on | A new tab opened for a note from Notes starts Claude knowing the note's title and file (added to its system prompt, no turn spent), to read only when you ask; off, it starts as any other tab |
| Notes beside a note's tab (`note_tab_beside`) | on | Opening a note's tab from Notes (t, or M-p's Open the tab for the note) moves Notes beside the tab's Claude showing the note, as M-e does; off, it only goes to the tab |
| Show a note's item whole in wtn's list (`wtn_item_preview`) | on | M-p's "Start work on a note's checklist items" shows the item under the cursor whole in the lower half of its list, its full text and what is indented under it, since the list cuts each item to one line; off, the list has the whole box |
| Ask for a tab's note (`ask_tab_note`) | on | When M-e brings Notes beside a tab with no note of its own yet, ask which note is the tab's, the likely ones first, and show that note beside it from then on |

### Appearance

| Setting | Default | What it does |
|---|---|---|
| Theme (`theme`) | dark | The status line's, the pane borders' and the boxes' colors; system follows the terminal's own background, which it is asked for as it attaches |

### Safety

| Setting | Default | What it does |
|---|---|---|
| Rules in reviews (`review_rules`) | on | codex-review gives the reviewer the repository's CLAUDE.md and AGENTS.md as the base has them and your own ~/.claude/CLAUDE.md with what it imports, and reports a change that breaks one; off, it reviews without them, and your rules aren't sent to the reviewer |
| Review before pushing (`review_before_push`) | on | In this repository, a push to main needs a clean /review-loop first (.githooks/pre-push); off, it pushes without one |
| Security review in the review loop (`review_security`) | on | /review-loop runs Claude Code's /security-review on the branch once before its first round, its findings fixed or asked about as the rounds' are |
| Ask before closing a pane (`confirm_close`) | on | M-w asks first when a program runs in the pane (a shell at its prompt closes at once); off, it closes the pane at once whatever runs in it |

### Palette

| Setting | Default | What it does |
|---|---|---|
| Learn from what you pick (`palette_learn`) | on | M-p remembers what you typed before each pick, and puts that command first when you type the same again |
| Commands button (`palette_button`) | on | A ⌘ Commands button in the status bar, after the session name, that opens M-p when clicked |

### Help

| Setting | Default | What it does |
|---|---|---|
| Tips (`tips`) | on | A box in the top-right corner, once each, when a key would help (M-a, M-j, putting a pane back); any key closes it |
| Suggestions from how you work (`habits`) | on | Count the keys you press, the commands you pick in M-p and the mission control views you open, on this computer only and never sent anywhere, and suggest in the notifications a view to turn off that you never open, a key for a command you keep picking in M-p, or a feature that is off which what you do would use; off, what was counted is removed |
| Easy mode (`easy_keys`) | off | Keys for people who don't know Vim's, in Notes and M-d's diff: Shift-arrows and the mouse select, typing replaces a selection, Cmd-S saves, Cmd-Z undoes, Cmd-Shift-Z redoes and Cmd-X cuts in a note (where the terminal sends them as the guide says), and the diff gets a row of buttons for its keys; mission control's boxes, M-m's, the update box's and M-w's menus, messages, tips, and Notes' Format menu, palette and finder get a × in their top-right corner that a click or Tab and Enter closes, with Click in menus on (lists to pick from, such as M-p's, keep Esc alone); Vim's keys keep working. Takes effect when Notes or the diff next opens |
| Key help line (`key_help_line`) | on | With Easy mode on, a line at the bottom of Notes and M-d's diff naming the keys that work there |
| Alt-arrows move between panes (`pane_arrows`) | on | M-Left, M-Right, M-Up and M-Down move to the pane on that side, as C-w h, l, k and j do in Vim; off, they reach the pane |
| C-w then h, j, k or l moves between panes (`ctrl_w_panes`) | on | C-w followed by h, j, k or l moves to the pane on that side, as in Vim, which itself still gets C-w; C-w twice sends one to the pane. Off, C-w reaches the pane as it is, deleting a word in a shell or Claude's prompt |
| Extended keys (`extended_keys`) | on | Your terminal is asked to tell keys such as Ctrl-Shift-P from Ctrl-P and Shift-Enter from Enter, and a program that asks for them (Notes, Neovim) gets them so, while the rest get the keys as before; off, every key reaches every program as a plain terminal sends it. Takes effect when a terminal next attaches and for panes opened after |

### Updates

| Setting | Default | What it does |
|---|---|---|
| Update check (`update_check`) | open, hourly | Offer a new release of Switchboard: when a terminal attaches and every hour, only when one attaches, or never |

### Slack relay

| Setting | Default | What it does |
|---|---|---|
| Haiku summaries (`haiku_summaries`) | on | One line on what each tab is doing in the relay's Home tab and progress, each a paid Haiku call |

## Commands for scripts

Scripts, hooks and skills act on the running workspace through `switchboard <command>`, much as they once called tmux. These commands never start a server; with none running, they fail. Each acts on a target given as `-t %<pane>` or `-t @<tab>` (as `switchboard list` and `switchboard tabs` show them); without one, on the pane the command runs in (its `SWITCHBOARD_PANE`), else the pane in front on the terminal used last. A tab given where a pane is wanted means its active pane, and a pane given where a tab is wanted, or for a tab's option with `show` and `set` (`@tab_label`, `@tab_name` and the other `@tab_` ones, and `@agent`), means its tab. A directory given with `-c` is read from where the command runs, `~` included.

```text
switchboard list                         sessions, tabs and panes, as JSON
switchboard new-tab [-s session] [-c dir] [-n name] [-d] [-E] [-- command...]
                                         prints the new tab's and pane's ids;
                                         -d leaves the current tab in front,
                                         -E puts the tab at the end
switchboard split [-t pane] [-L|-R|-U|-D] [-c dir] [-p percent] [-d] [-- command...]
                                         a new pane on that side (right when
                                         none is given); prints its id
switchboard respawn-pane [-t pane] [-c dir] [-k] [-- command...]
                                         a new program in the same pane;
                                         -k ends one still running
switchboard send-keys [-t pane] [-l|-p] key...
                                         keys by name (Enter, C-c, M-x), a word
                                         that isn't one typed as text; -l types
                                         them all as text, -p pastes them
switchboard capture-pane [-t pane] [-S lines]
                                         the pane's screen, with that many lines
                                         of history above it
switchboard select-pane [-t pane] [-L|-R|-U|-D]
                                         focus a pane, or the one beside it on
                                         that side (the sidebar past the left
                                         edge), as the diff and Notes ask at
                                         their edges
switchboard select-tab -t tab            bring a tab to the front
switchboard switch-client -t tab         bring a tab to the front and show it
                                         in the terminal you used last, moving
                                         it to the tab's session
switchboard kill-pane [-t pane]          close a pane, without asking
switchboard kill-tab [-t tab]            close a tab, without asking
switchboard rename-tab [-t tab] name     name a tab; "" names it after its
                                         directory again
switchboard set [-t target] [-u] @option [value]
                                         set (or with -u, unset) a tab's or
                                         pane's option
switchboard show [-t target] @option     print an option's value
switchboard display [-t pane] pane|tab|session|cwd|command|title
                                         print one thing about a pane
switchboard say [-t seconds] text...     a message in a box
switchboard tip <name> | tip seen <name> | tip reset
                                         show a tip once, mark it shown, or
                                         show every tip again
switchboard press <action> [arg]         run an action as its key would, by its
                                         name in keys.json (only note-tab takes
                                         an arg, the note)
switchboard sources [refresh [view]]     mission control's GitHub and Linear
                                         data as JSON, or looked up again now
switchboard list-keys                    every key and what it does
switchboard save                         save the layout now; prints its file
switchboard detach [-s session]          detach every terminal, or a session's
switchboard kill-server                  save, then end the server and its panes
```

`set` and `show` without `-t` put an option whose name starts with `@tab_`, or is `@agent`, on the caller's tab, and any other on its pane. A few are the workspace's own: `@tab_name` names the tab; `show` gives `@tab_label`, the name the tab bar shows, PR number included, `@tab_path`, the directory it is named after, and `@agent`, the tab's most urgent state. On a pane, `@agent_state` and `@agent_since` are Claude's state there and since when (seconds), `@agent_activity` its tool call in a few words, `@agent_wait` what it waits on, `@agent_loop` the loop it runs (review, ci, queue or deploy) and `@agent_phase` the step a loop says it is at, which `land-lock` sets as it lands on the default branch.

```sh
# A tab in the background for a build, its pane's id kept for later
read tab pane < <(switchboard new-tab -d -n build -c ~/code/app)
switchboard send-keys -t "$pane" 'make test' Enter
switchboard capture-pane -t "$pane" -S 200
```

A pane closes when its program ends, so a command given to `new-tab` or `split` takes its pane with it when it finishes; type it into the shell, as above, to keep what it printed.

Four more commands are programs of their own:

- `switchboard tabs` prints a line per pane of every tab, tab-separated: the tab's number, the pane's place (`main:@3.%12`), the tab's name as the tab bar shows it, Claude's state there (blank when idle), the program in front, and the directory. Claude reads it to see what the other tabs are doing.
- `switchboard respawn [-t pane] [-c dir] <command> [args...]` replaces a pane's program with another in the same pane, which keeps its id, place and tab, and leaves a shell there when it ends. `wt`, `wtn`, `wtb` and `wtpr` run as the worktree commands' shell functions. The arguments are words of their own, never read by a shell. Skills use it to hand a tab over to the work it opens.
- `switchboard setting <key>` prints one of Settings' values as it stands, `on` or `off`, or the choice (`switchboard setting update_check` prints `hourly`); `switchboard setting <key> <value>` changes it as Settings would, and `switchboard setting <key> --reset` gives it its default back.
- `switchboard platform <what>` does what differs between macOS and Linux, for shell scripts: `name`, `secret <service> [-a account]`, `secret-set <service> [-a account]` (the secret read from stdin), `copy` (stdin to the clipboard), `paste`, `notify <title> <text>`, `presence` (idle milliseconds and whether the screen is locked), `open <file or link>`, `front-app` and `chrome-local-state`.

`switchboard help` lists every command, and `switchboard --version` prints the version and the commit it was built from.
