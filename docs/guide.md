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

`M-b d` detaches, leaving everything running. `switchboard detach` does the same from a script, for every attached terminal or, with `-s <name>`, those on one session. `switchboard kill-server` ends the server and every program in its panes, after saving the layout. A server also ends by itself once its last tab closes. `switchboard upgrade` restarts the server on the build installed now without ending anything in it ([Restarting on a new build](#restarting-on-a-new-build)).

### Sessions and clients

A session is a set of tabs. `switchboard` attaches to the first session there is, or makes one named `main`; `switchboard -s <name>` attaches to the session by that name, making it, with one tab, when there is none. A new session's first tab runs your login shell in your home directory.

Several terminals can attach at once. Terminals on the same session show the same tab, and the tabs are sized to whichever of them you used last. `M-a`, the sidebar and mission control can take you to a tab in another session, and your terminal moves to that session. When a session's last tab closes, its terminals move to another session, or end when there is none.

### What a pane carries

A pane runs your login shell (`$SHELL`) unless something asked for another program. Its environment carries `SWITCHBOARD`, the server's socket, and `SWITCHBOARD_PANE`, the pane's id (`%12`), which is how `switchboard <command>` and Claude Code's hooks know which server and which pane they run in. `CLAUDE_CODE_PLUGIN_DIRS` names Switchboard's plugin (`~/.local/share/switchboard/plugin`, after any directories the server was started with) while Settings' "Load Switchboard as a plugin" is on, so Claude Code run there loads its hooks, skills and instructions ([Setting Claude Code up](#setting-claude-code-up)). `TERM` is `xterm-256color` and `COLORTERM` is `truecolor`. A program that asks its window's size in pixels is told your terminal's, as its cell's size times the pane's (where your terminal says its own; 8 by 16 pixels a cell where it doesn't), so pictures in Notes and the PR as an image are drawn at their own size. A pane's id stays the same for as long as the pane exists, across restarts too, so a pane named in a note or a script is the same pane after one.

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

A tab something you did opens (a key, a palette command, a menu's choice, a card's action, `switchboard new-tab` without `-d`) comes to the front, so it is what you see; when one choice opens several, the first comes to the front. A tab the workspace opens on its own stays behind the one in front: autopilot's tabs, the Calendar and Slack loop tabs a view opens when it is out of date, the Notifications window, and the Slack relay's.

### Claude in the tabs

Switchboard's hooks for Claude Code, which its plugin gives the Claude in its panes (or, with the plugin off, `switchboard setup` adds to your `~/.claude/settings.json`), run `switchboard hook` at each step of a session, and the workspace marks the pane accordingly. A pane is in one of three states, or none:

- **working**: from the moment you send a prompt, through every tool call and a compaction in the middle of a turn, and while a `/compact` you typed runs. A compaction Claude Code makes on its own between turns (while idle, before the prompt cache expires, or a summary it writes ahead of time in the background) leaves the tab as it was, since nothing says when a summary written ahead ends.
- **waiting**: on your permission for a tool, or on your answer to a question, its own or one a tool asks through it. A question is a wait from the moment it is asked. A wait belongs to whoever asked, the conversation or one of its subagents, so a background agent's tool calls while Claude waits on you leave the tab waiting, and to its call, so a background agent finishing, or a monitor firing, while a question is up leaves it waiting too, under Needs you rather than its loop's stage, until the question itself is answered; a question still unanswered in the conversation whose hook never arrived is read from the transcript and marks the pane waiting too. Denying a permission request, or pressing Esc on it, ends the turn without running any hook, so the workspace reads the interruption from the conversation and hands the pane back as idle, a Claude parked behind the diff or lazygit included (its request can be denied from the Slack relay), unless it has moved on since with a new prompt or request. Pressing Esc while Claude works, on a command auto mode let run without asking, ends the turn without a hook the same way, and the pane goes idle unless a prompt came after it. A `/compact` you typed that fails or that you cancel runs no hook either, and the pane goes idle once Claude Code says its session is idle again.
- **done**: the turn ended and you haven't looked at the tab since. A turn that ends in the tab you are looking at is never marked done.

A tab shows the most urgent of its panes' states, waiting before done before working, as a badge in the status line and in the sidebar. A new or ended session clears the state, and a pane that no longer runs Claude loses its state the next time you move between tabs, so a session that died without saying so leaves nothing behind, even when the shell back at its prompt got the dead Claude's process number. A `claude -p` that a script or a loop starts in the tab doesn't mark it, and a conversation moved into the background still shows on the tab it came from.

"done" clears once the tab has been in front for a second, so passing over a tab, as moving the sidebar's cursor does, leaves the mark. Settings' "Finished tabs clear" (`done_clears`) can keep it instead until you next prompt that session ("on prompt"; "when seen" is the default).

Whatever the workspace types into a tab's Claude (`M-m`'s `/resolve-conflicts`, the Slack tab's `/slack-updates`, a CI loop started from mission control, a message answering a tab from mission control, the sidebar or the Slack relay, a reply sent from another tab) presses `i` first when Claude Code's vim mode has left the prompt in Normal mode (its mode line reading `-- NORMAL --`), where the first letters typed would otherwise be taken as vim commands and what is typed would arrive cut off.

`M-a` goes to the next tab that needs you: pinned tabs and reminders that have come due first, then the ones waiting on you, then the finished ones, and last the sidebar's notifications while there are any (Settings' "M-a goes to notifications", `needs_notices`). Pressed again, it goes on to the next, wrapping round, so pressing it repeatedly visits each one. It says "Nothing needs you." when nothing does. `M-A` walks the idle tabs the same way (see "What needs you, one tab at a time"). `M-c` goes to the pane running Claude in the current tab.

`M-y` copies the latest reply of the tab's Claude to the clipboard, from any pane of the tab or its sidebar: the last message Claude wrote text in, read from its session's transcript, its parts joined as written, reaching back past a prompt not answered yet and passing over subagents, API errors and Claude Code's own stand-in messages. A Claude parked behind the diff or lazygit in its pane's place counts. A box says how many lines were copied, and that Claude is still working when it is, or that the tab has no Claude or no reply yet. Settings' "Copy Claude's reply" (`copy_reply`) off hands `M-y` to the pane.

### Keeping the computer awake

While a tab's Claude is working, the workspace keeps the computer from sleeping when it is left idle, so autopilot's tabs, a review loop or a long test run carry on while you're away. A tab counts while its Claude works in the foreground, and while background work or watches it started run (a review loop, a CI watch, a Monitor), between turns or while it waits on you; a `/loop` waiting for its next run doesn't, since it may wait for hours. While autopilot is on and not paused, each of its tabs open and not yet landed counts too, waiting on you or not. Once nothing counts, the workspace lets go two minutes later, so the moment between one tool call and the next doesn't let go and take it again.

On macOS it runs `caffeinate -is` (watching the server, so it ends with it), on Linux it asks logind for a sleep inhibitor; nothing has to be installed. The display still sleeps, and a laptop closed with no display on it sleeps anyway. While it holds, a ☕ shows after the health light on the status line, or with UX 2.0 after the tab count at the top of the sidebar, and the first time work holds it a tip says so. `switchboard list` says the same in its `awake`: whether it holds (`holding`), what holds it (`why`: `3 tabs working`, `autopilot has 2 tabs open`, `until you say`; none in the two minutes after everything went idle) and whether by hand (`by_hand`).

M-p's "Keep the computer awake until I say" holds it whatever the tabs do, through a restart on a new build too, until you choose it again, which goes back to the rule above. Settings' "Keep the computer awake while tabs work" (`keep_awake`) off leaves the computer to sleep on its own schedule unless held from M-p.

### A tab's history

`M-h` (or the header's `M-h History` key with UX 2.0) shows the history of the tab's Claude session: a timeline under the tab's screen, from when the session started to now, the span filling the timeline's width, so a column is a little under a minute for an hour and a half on a wide screen. Its lanes are the session's state (working blue, waiting on you yellow, finished green, idle grey), your turns (`▸`), commits (`•` and the commit's short hash), review rounds (`◆ round 2`, `round 3 clean`) and the context it used, as a chart with each compaction marked by a dashed line and what it left (`compacted 66% → 18%`); the state lane and the context chart are images at pixel resolution where the terminal draws them (Ghostty, kitty), and blocks of text elsewhere. A line through the lanes marks the moment shown, and the screen above shows the pane as it was then, with a chip under it while it waited on you saying for how long and what you chose (`waited 6m for you · you chose 1. Fix it`); the header says which moment it is (`viewing 13:41, 51 minutes ago`), as the keys row does without UX 2.0. While the diff, a PR or ticket view or lazygit stands in Claude's place, `M-h` first brings Claude back beside it, as `M-p`'s "side" does, so what you type when the history closes reaches Claude.

`h` and `l` (or Left and Right, or the wheel) move a column back and on, `[` and `]` to the turn before and after, `c` to the next commit and `w` to the next wait, each from the first once past the last; a click on the lanes shows that moment (with menu_mouse on), and easy mode's × at the keys row's end, or a click on the header, closes it. Esc comes back to now, and at now closes it, as `M-h` does; any other key closes it and goes on to the tab, so typing reaches Claude as before.

The state and the screens come from what the server saw: each change of state as the hooks say it, and a snapshot of the screen at most every 15 seconds while it changes and as its state changes, kept in the server's memory alone, never written to disk, the oldest going after about six hours of change, and gone when the server ends (a moment before the first shows "No screen was kept from then"). The rest is read from the session's transcript as the history opens and on while it is open, so those lanes reach back to the session's start. A new session in the pane (started, `/clear` or resumed, not a compaction) starts its history again. A tab whose Claude has run no hook since the server started (one restarted on a new build) opens from the session Switchboard reads for its pane from Claude Code's session files, its lanes read from the transcript and its screen kept from then on; a tab with no Claude session says it has no history yet. Settings' "Tab history" (`tab_history`) off keeps nothing, and `M-h` says it is off.

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
| `M-I` | Start work on a note's checklist items, each in a new tab (`wtn`) |
| `M-a` | Go to the next tab that needs you |
| `M-A` | Go to the next idle tab, in the sidebar's Idle order |
| `M-c` | Go to the pane running Claude in this tab |
| `M-y` | Copy the latest reply of this tab's Claude |
| `M-h` | This tab's history: its Claude session on a timeline under its screen as it was then (see "A tab's history") |
| `M-f` | Pin or unpin the tab, keeping it at the top of Needs you |
| `M-T` | This tab's whole title in a box, which the sidebar cuts at three lines and the status line at its width: the title as the tab bar has it, its PR number in front, wrapped, then its branch, directory, PR and Linear ticket as far as they are known, read from what the workspace holds alone, never the network; Copy the title (or `y`) copies it, and Close, Esc or a click off the options closes it (Settings' "Show a tab's full title", `tab_title`; off, the key reaches the pane) |
| `M-p` | The palette: find any command by what it does, and run it |
| `M-/` | Every key, in mission control's Keys view (see "The Keys view") |
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

`M-b ?` shows every key bound now, yours included, in a box that `j` and `k` scroll and any other key closes. `switchboard list-keys` prints the same list, a key and what it does on each line. `M-/` shows them all with the keys of the sidebar, mission control, the diff and Notes, in mission control's Keys view.

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

A key the file binds, or binds to something else, carries a "yours" chip in mission control's Keys view and in "Find a key"; the rest say "default".

The server reads the file as it starts and again within two seconds of a change, so a change takes effect without a restart; anything in it that can't be used is said in a box (to the first client to attach, at the start) and written to `server.log`. Settings' "Key bindings" row, under "Your files", opens it in an editor. A `~/.config/switchboard/tmux.conf` from the tmux version isn't read: the first client to attach is told so, and where its keys and options go now.

The actions, by the names the file uses:

- Panes: `select-pane-left`, `select-pane-right`, `select-pane-up`, `select-pane-down`, `next-pane`, `last-pane`, `pane-nav` (`C-w`, waiting for `h` `j` `k` `l`), `split-left`, `split-right`, `split-up`, `split-down`, `close-pane`, `zoom`, `resize-left`, `resize-right`, `resize-up`, `resize-down` (five cells; `resize-left-2` for two), `copy-mode`.
- Tabs: `prev-tab`, `next-tab`, `last-tab`, `select-tab-1` (any number from 1), `go-to-tab` (`M-0`), `new-tab`, `close-tab`, `new-claude-tab`, `find-tab`, `revive-worktree`, `toggle-pin`, `remind-tab`, `tab-title` (`M-T`), `rename-tab`, `calendar-tab`, `slack-tab`.
- Claude and work: `next-needs-you`, `next-idle`, `focus-claude`, `copy-reply`, `work-on-tickets`, `work-on-branch`, `work-on-pr`, `work-on-note`, `work-note-continuously`, `continue-shaping`, `shape-note`, `shape-new-ticket`, `send-reply`, `update-main`.
- The workspace: `palette`, `mission-control`, `mission-<view>` (the view's name in lower case with dashes for spaces: `mission-work-map`, `mission-calendar`, `mission-slack`, `mission-closed`, `mission-prs`, `mission-tickets`, `mission-inbox`, `mission-ci`, `mission-landing` (`mission-merge-queues`, its name before, still works), `mission-sessions`, `mission-usage`, `mission-resources`), `keys-view` (`M-/`), `notifications`, `settings`, `habits-report`, `focus-sidebar`, `toggle-sidebar`, `toggle-theme`, `reset-tips`, `list-keys`, `detach`, `send-prefix`, `slack-relay`, `stop-slack-relay`, `update-switchboard`, `run-setup`, `upgrade`, `keep-awake`, `check-setup`, `setup-part`, `aws-login`.
- Reading: `pr-view`, `ticket-view`, `pr-image`, `ticket-image`, `clear-image-cache`, `open-pr`, `open-repo`, `open-ticket`, `open-pr-number`, `open-ticket-id`, `open-merge-queue`, `open-linear-project`, `copy-ticket-id`, `copy-ticket-url`, `copy-pr-number`, `copy-pr-url`, `pr-ready`, `search-everything`, `read-link`, `open-link`, `copy-link`, `open-media`, `open-link-being-read`.
- The diff: `diff`, `diff-working-tree`, `lazygit`, `tool-beside`, `tool-alone`, `copy-diff-story`, `ask-about-diff`, `clear-diff-cache`.
- Notes: `notes`, `notes-beside`, `note-tab`, `choose-tab-note`.

These have no key by default and are in `M-p` alone: `work-on-pr`, `work-on-note`, `work-note-continuously`, `continue-shaping`, `shape-note`, `shape-new-ticket`, `pr-image`, `ticket-image`, `clear-image-cache`, `lazygit`, `diff-working-tree`, `settings`, `remind-tab`, `toggle-sidebar`, `toggle-theme`, `reset-tips`, `stop-slack-relay`, `open-repo`, `read-link`, `open-link`, `copy-link`, `open-media`, `open-link-being-read`, `tool-beside`, `tool-alone`, `copy-diff-story`, `ask-about-diff`, `clear-diff-cache`, `note-tab`, `choose-tab-note`, `update-switchboard`, `run-setup`, `upgrade`, `keep-awake`, and those of [More in the palette](#more-in-the-palette): `rename-tab`, the `mission-<view>` commands, `notifications`, `habits-report`, `send-reply`, `update-main`, `check-setup`, `setup-part`, `calendar-tab`, `slack-tab`, `open-pr-number`, `open-ticket-id` and `aws-login`. Give any of them a key in `keys.json` to have it on one, `open-merge-queue`, `open-linear-project`, `copy-ticket-id`, `copy-ticket-url`, `copy-pr-number`, `copy-pr-url`.

## Tabs and the status line

The status line along the bottom starts with the session's name on a badge in the current tab's color, then how many tabs wait on you (`!`, yellow), have finished unseen (`✓`, green) and are working (`⋯`, blue), each count shown only while it isn't zero. Then come the ⌘ Commands button, the health light when something is wrong, and the tabs, each with its number, its state's badge and its name, in its project's color, the current one bold. When the tabs don't all fit, the list scrolls to keep the current one in view, as near the middle as the ends allow, with a `<` or `>` at the end where more are hidden.

### What a tab is called

A tab is named after the directory of the pane you are in (the sidebar and a Notes pane beside your work don't count), read again every two seconds, so a `cd` renames it. In a pane running Claude, that is the directory Claude works in now, as its hooks report it (its Bash tool's `cd` moves it while Claude's own process stays where it started), so a tab whose worktree `/queue-and-clean` removed follows Claude back to the main clone. A directory removed under a pane is never shown: its nearest parent still there takes its place in `switchboard tabs` and the directory a new split or tab opens in, while the tab keeps the name, color and PR the removed directory gave it until its pane is in a directory that exists again. `/queue-and-clean` pins the tab's name (`@tab_name`) as it starts, when it has none of its own, so the tab keeps its title in the main clone after the cleanup. A worktree gives its own name; any other directory gives its name with the branch after a colon (`app:fix-login`), unless the branch is `main` or `master`. When the branch names a Linear ticket that Linear confirms, the tab takes the ticket and its title instead (`ABC-123: Fix the login`). When the branch has a pull request, its number goes in front (`#123 ABC-123: Fix the login`), first because it is short and doesn't change, so the tabs can be scanned by number. In a fork's clone, where `origin` is your fork and an `upstream` remote names the project, the branch's PR is looked for in both, so one you opened from your fork against the project counts; one in your fork itself wins when both have one, and another fork's branch of the same name never counts.

A name given to the tab wins over all of that, the PR number still in front: the `@tab_name` that "Rename the tab" (and "Rename it" in mission control's menu for the tab) sets, as a skill such as `/shape` does, so `/wt` clearing it hands the tab its ticket's name; else one set by `switchboard rename-tab <name>`. An empty name hands the tab back to its directory.

The PR number and the ticket's title are asked of GitHub and Linear in the background, so a tab never waits on the network: it shows what is known and takes the rest a moment later. A PR found, or a ticket's title, is kept a day, across a restart too (in `~/.cache/switchboard/tab-names.json`); a branch with no PR is asked about again a minute later, and a lookup that fails keeps what was shown and tries again in a minute. Settings' "PRs and tickets in tab names" (`tab_lookups`) turns both lookups off, and turning GitHub or Linear off under Integrations turns off its own.

Each project's tabs wear a color of their own, worked out from the directory: the worktree, or the directory and its branch. It is never a red, which means an error here (a failing build, a failed stage). Claude Code's statusline, as `switchboard setup` sets it up, draws the same color, so a session and its tab match. The session's badge and the lines around the focused pane take the current tab's color. Settings' "Project colors" (`accent_colors`) turns them off, and every tab is gray.

### The ⌘ Commands button and the health light

The ⌘ Commands button opens the palette when clicked, as `M-p` does (Settings' "Commands button", `palette_button`). With UX 2.0 it is at the right end of the tab header's keys instead, lit while the palette is open, and a click on it then closes the palette; a header too narrow for the keys and the button (two tabs side by side) leaves the button out.

The health light stays off while all is well, and shows a red `●` naming what has quietly stopped, so you see it before you miss what it stopped:

- `saves 7m old`: the layout changed and the save hasn't landed for over five minutes.
- `relay down`: the Slack relay crashed, rather than being stopped.
- `Slack not connected`, or another service: a Claude connector a feature needed wasn't connected when it last tried, in the last two hours.

Settings' "Health light" (`health_light`) turns it off.

After it, a ☕ shows while the workspace keeps the computer awake ([Keeping the computer awake](#keeping-the-computer-awake)).

### The second status line

Settings' "Second status line" (`second_status`, off by default) adds a line under the tabs about the current tab: its name in full, where the tab may cut it short, its PR and whether it has every commit made here ("↑2 to push" in yellow, the commits on the branch that its upstream, or origin's copy of it, hasn't got as last fetched or pushed; "pushed" when it has them all), the program in front of its pane, and in a Claude pane, what the session is doing ("waiting on you for 4m", "finished 2m ago", "working"), its name and your last prompt to it.

### UX 2.0

Settings' "UX 2.0" (`ux2`, off by default) is a new look being built a part at a time. Today it:

- takes the status line away;
- draws the sidebar, the tab header and mission control with one kit of pieces, so they look alike: one mark and one color for each state wherever it shows (● needs you, ⠹ working, ▲ landing or queued, ✓ done, ✕ failed, ◌ pending, ◷ a reminder, ○ idle), section labels in capitals in their state's color with the count at the right, chips and stage cells on a tint of their color, cards and boxes with rounded corners, and the keys on every footer and bar in the accent color with what they do dim; a chip, a stage cell and a key end in half circles on a terminal that draws them itself (Ghostty) and in spaces elsewhere, the same width either way;
- gives each tab a header over its panes, its first line filled with the tab's own color (the one its number has in the sidebar), and on it, dark: its state, its name in bold, and where it works and what Claude is doing, with how long (`waiting 4m`), so after `M-a` you see how long the tab has waited on you; under it, the keys of what the tab can show in Claude's place (`M-c` Claude, `M-d` Diff, `M-v` PR, `M-i` Ticket, `M-e` Note), the one it shows lit as one pill, then `M-h` History, lit while the tab's history is up, and at the row's right end the ⌘ Commands button;
- says in the header the branch after where the tab works (`~/code/app · main`), and at the right of its first line the tab `M-a` goes to next (`M-a next: 3 billing`) while another needs you; a click on a key in the header does what the key does, and with hover_highlight on the key under the mouse is lit;
- shows the PR (`M-v`) and the ticket (`M-i`) in Claude's place, as `M-d` shows the diff, rather than beside it, laid out as their pages are with a panel beside the description: the PR's checks, reviewers, open review threads and files, the ticket's facts, pull requests and related tickets, and people's photos by what they wrote; the same key, or `M-c`, putting Claude back;
- starts the sidebar with how many tabs there are, a ☕ after the count while the computer is kept awake, and `/ filter`, in place of the session's name and the counts, and its notifications under that, a line each, gone when there are none, in place of their section at the bottom, so `k` or Up from the first tab moves up onto them and `j` or Down from the last of them back onto the first tab, while `j` from the last tab stays there;
- ends each notification line with how long ago it came, dim at the right edge (`1m`, `12m`, `1h`), its title cut so the time always fits: the newest of its kind for Slack, the Linear inbox, notes written for you and landed tabs; a calendar change, the next meeting, the mode's word and a suggestion keep no time and show none;
- draws each group's heading in its color, bold, its count at the right edge, and each tab in two lines: its state's mark with no pill, its number in its project's color and its name on one line, then what it is doing; the cursor's tab shaded with a bar at its left, no rule between tabs, and the keys that start, find and walk tabs (`M-t`, `M-j`, `M-a`) on the sidebar's last row;
- draws the pinned, reminder and pending-checks marks as symbols of your terminal's font (▲ ◷ ◌) in place of emoji (📌 ⏰ ⏳), one column each;
- lays mission control's PRs and Tickets views out as one family of boards: each column's name in capitals in its state's color with its count, each PR or ticket a rounded card with its number or ID in the accent color on its top edge and at the edge's right end the PR's checks' mark (✓ ✕ ◌) or the ticket's priority (`!!!` urgent, `!!` high, `!` medium), its title on two lines, then a dim line about it, the chosen card's edge in the accent color; a done ticket's card faint, its title struck through, saying how long ago it was done;
- puts a PR with changes asked for in a Changes asked column of its own between In review and Approved, and ends each card with the tab working on it in its PR's repository (`tab 3` and its state's mark), or else a PR's reviewers, requested and latest, or a ticket's project and estimate; under the columns, a faint row of the same cards for the PRs merged and the tickets canceled in the last 7 days ("Merged this week", "Closed this week"), each saying when, and who merged the PR or how the ticket was closed, which `j` and `l` reach and `h` and `l` move along; and on a screen 140 columns wide or more the chosen PR or ticket beside the board: its number or ID and title bold, a dim line of a PR's author, commits, lines added and removed and checks, or a ticket's state, priority, project and estimate, then its description through the PR and ticket views' Markdown, all from each board's one query, a Mermaid diagram in it drawn as a picture as those views draw it, as wide as the pane at most and cut at the pane's edges as the text is, `J` and `K` (or the wheel over the pane) scrolling what is below its bottom into sight, where the terminal draws images (Settings' `view_images` and `server_images`, both on), and as text boxes until it is drawn and anywhere else;
- in Notes, draws chips on a line of their own under an unticked checklist item: `plan` when an indented `Plan: [[...]]` line is under it, and `needs a choice` in yellow when the item or that line says so; a ticked item gets none;
- draws each worktree in mission control's Landing view as a row of stage cells under the stages' names (tests, review, leak check, lock, push, applied), its name bold with where it is dim under it: done green with ✓, the step its phase names blue, its place in the landing queue amber, a failure red with ✕ (the phase `tests failed` or `review blocked` queue-and-clean sets, with the words after it), the rest dim and waiting; done cells say "passed" and "clean", since nothing the view reads holds a test count or the review's rounds; and each PR in GitHub's merge queue as the same row under its own stages (CI, approved, merge queue, merge, deploy test): CI and approval done, since GitHub queues a PR only once they are, its place in the queue, its checks running, ✕ when it can't merge, and merging;
- in Notes, gives unticked checklist items keys of their own, each Vim's where it has nothing to do: Enter on an item with a plan opens its plan note (else ticks it as before), `o` on an item a tab works on or waits in goes to that tab, `ga` (or a click) types into the "Add an item" input at the bottom, Enter adding the item after the last unticked one of the note's top list and M-Enter opening a tab in the note's repository, linked to the note, whose Claude runs `/plan-item` with it (which puts the item in the note and writes its plan; the repository is asked as `gw` asks it), and `M-p`, which the workspace hands Notes instead of opening the palette, has autopilot work the open note, asking for the repository and then as "Autopilot: work a note" does after its note, or opening mission control on it while it is on; a section for parked work (`## Defered`, `## Later`, the ones wtn and autopilot leave out) opens folded to one line with its count of items (`▸ Defered   7 items, parked`), and the footer shows these keys;
- in Notes, puts the tab on an unticked item first among its chips, from a look at the workspace's tabs every few seconds off the drawing path: `⠹ tab 3` in blue with its loop's phase while it works, `● tab 2 waits on you` in yellow with what it waits on, the item a tab is for read from its full title, autopilot's item or its branch's `wtnItem`, and shown only on the note that item is in, known from the note's ID in its branch's `wtnItem`, else the note the tab is linked to (a tab whose note is neither shows on any note's item of its words); and right-aligns a green `landed <hash>` on a ticked item whose indented lines name a commit;
- draws each worktree in mission control's Landing view as a row of stage cells under the stages' names (tests, review, leak check, lock, push, applied), its name bold with where it is dim under it: done green with ✓, the step its phase names blue, its place in the landing queue amber, the rest dim and waiting; the merge queue's rows stay one line each;
- gives autopilot a mission control view of its own, Autopilot, after Landing, reached with `[ ]` while autopilot works a note or while it is in front (Settings' "Autopilot view", `view_autopilot`): its note bold with its tabs working and open against its slots and cap (`1 of 2 working · 3 of 5 open`), whether its tabs land without asking, only items with a settled plan, today's Claude spend against its spend limit (`$5.00 of $20 today`, with a meter at the right of the heading, amber from four fifths and red once passed), and why it is paused (by you, while you're away, or what it last said, such as the spend limit passed); then a board of every item it works, each a rounded card in the column where it stands: Next up (the items it will open next, in that order, each saying its place in line), Working, Waiting on you, Landing (in the landing queue or holding the lock), Landed (every item of its note landed today, its tab open or closed, a closed one's card saying `closed` and the commit it landed, with no tab to go to) and Skipped (the items you skipped, each card its item dim and `skipped by you`, with no meter), decided from the tab's marks, its Claude's state, loop and phase, and its worktree's place in the landing queue. Within a column the cards go from the least far along to the furthest, so landed ones come last. A tab's card shows its tab's number, its item, where it is dim and a meter of its eight stages done (opened, working, tests, review, leak check, lock, landed, closed) in its column's color, red with ✕ once a stage failed; the meter is a kitty image where the terminal draws them (Settings' `server_images`), a line of ━ elsewhere. `h` and `l` move between columns, `j` and `k` along the cards, and the chosen card's edge is in the accent color. On a wide screen the chosen item sits beside the board in full, with its stages one a line in Landing's colors and its plan under them (the plan note its `Plan: [[…]]` line links, a Mermaid diagram in it as a picture), as a ticket's description sits beside the Tickets board, `J` and `K` scrolling it; an item that links no plan says "No plan" there. Before a tab joins the landing queue, its tests and review show only what its loop or phase says and its leak check waits, and closing waits while the tab is open. On a screen too narrow for the board the view is a plain list in the same order. Enter on a card or row goes to its tab, skips its item, takes a skipped one back (or all of the note's skipped items), or pauses or resumes autopilot, has it land without asking or ask again, sets its tabs working at once, most tabs open or spend limit, or stops it; Enter on its heading, `m` anywhere in the view, or a click on the heading's `m ☰ Menu` button (lit under the mouse, and while Tab's focus is on the heading) opens the menu the Tabs view's line opened, every choice of it. With the view, the Tabs view has no autopilot line and its cursor starts on the first card, and "Autopilot: work a note", autopilot's notifications and their "Open mission control's Autopilot" open mission control on this view instead of the Tabs view; with the view off, the line above the Tabs view stays as it is;
- groups mission control's views by feature in a bar of two rows over a rule: the groups on the first, Work (Tabs, Landing, Autopilot, Map, Closed, Sessions), Code (PRs, Tickets, CI), Day (Calendar, Slack, Inbox, Notifications) and System (Usage, Resources, Settings, Keys, then each plugin's view), the one in front lit on the accent, beside the counts of tabs; the views of that group on the second, the one in front lit and underlined, with `{ } groups  [ ] views` at its right. `}` and `{` go to the next or previous group's first view, `]` and `[` round the views of the group in front, and Tab, with Settings' "Tab moves focus" on, moves through the groups' names and then the names of the views in front; a group with no view turned on is left out;
- opens mission control (`M-Space`) right of the sidebar, from the top row down, so the sidebar stays in sight beside it, and draws each tab in its Tabs view as a rounded card: its state mark, number and name in bold on the top edge and its state at the edge's right end, the last lines of its screen inside, and its branch dimmed on the card's last line;
- shows the screen inside each mission control card in the pane's own colors, taken from its emulator in memory and cropped to the card, and after the card's branch its PR's checks as marks (✕ red, ✓ green, ◌ pending, with their counts);
- draws mission control's Calendar view as a timeline: the hours down the left, three rows an hour, each labelled dim and right-aligned with a dashed rule across; each event a block the height of its length with a bar in its color, its title bold and its time and where to join dim, one already over dimmed and struck through; now as a red line with its time; and, on a screen 100 columns wide or more, the event under the cursor beside the day, with your reply, the link to join and the tickets and PRs it names;
- gives the Calendar timeline's chosen event the board's detail: joining and each answer its menu offers as buttons labelled with the menu's keys (j Join, y Going, n Not going, m Maybe, in Enter's menu), who is going by first name with +n for the rest, and the first lines of its description as its agenda, both read from the feed's ATTENDEE and DESCRIPTION lines; and says how soon the next event starts ("in 15m") on its second line;
- draws `M-d`'s diff as its board does: the list on a panel, headed STORY with the number of chapters Haiku told, each chapter its number in the accent color and its title, bold for the shown file's, the files under it with a yellow ● on those not committed yet and their counts at the right, the cursor's row shaded with a bar at its left; the files changed only since the last commit under a NOT COMMITTED YET label in yellow with their count, still the last stop for `]p` and `]c`, and generated files last under GENERATED, LAST; over the diff, the shown file's chapter (`Chapter 1 of 3` in the accent color and its title, or Not committed yet in yellow) with the base it compares with at the right, its why under it in at most three lines and a rule, the why no longer in the list; one bar over both sides, the path with its folders dim and its name bold, its counts and at the right the two sides compared (`origin/main  ⟷  working tree`); each hunk's note in a rounded card under the hunk, across both sides, titled ✎ Note, or with the chapter's number and title for a hunk of another chapter; and the files as one scroll, each following the last under a rule and its bar, so `j`, `k`, the wheel and `]c` `[c` in the diff go on from one file into the next and back, the list following, while `G` and `gg` go to the file's own ends and the bar of the file at the top stays over the diff;
- with Settings' "Context in Claude's status line" (`statusline_context`) off, has Claude's statusline under its prompt show the model alone inside the workspace, how full Claude's context is being in mission control's Sessions view; with the setting on, as it is by default, with that view off (`view_sessions`), and outside the workspace, it shows the context as before.

Images: where your terminal speaks the kitty graphics protocol (Ghostty, kitty), the workspace draws its own images: a person's photo as a round avatar, a picture inside a rounded border, a meter with its tick, a series of values as a chart or, one row tall, a sparkline, a setting's on/off switch, the keyboard as a heatmap of the keys you pressed, and a soft shadow under a box. Each is placed in the screen's cells, so a box over it covers it, and replaced in place when it changes; in another terminal, or with Settings' "Images in the workspace" (`server_images`) off, each is drawn with characters: the initial on the person's color, the picture's name, `━` for a meter, `▁`…`█` for a series, ` ━●` or `○━ ` for a switch, each key's letter in the accent at three shades for the keyboard, half blocks for the shadow. An avatar with no photo is the initial in every terminal. The parts that show them come as they are built.

Photos: an avatar shows the person's own photo from Slack, Linear or GitHub, fetched in the background the first time something draws them, kept in `~/.cache/switchboard/photos/` and fetched again only after 30 days, each request counted in the Usage view's request audit. Slack's comes through the Slack relay's app, so it needs the relay set up and the `users:read` scope in its manifest (below); someone who kept Slack's own picture shows their initial. Linear's and GitHub's follow the `linear` and `github` settings. With Settings' "People's photos" (`photos`) off, nothing is fetched and every avatar is the initial.

A pane zoomed with `M-=` covers the header, as it covers the sidebar. With `ux2` off, the status line, the sidebar and those keys are as before.

### Theme

Settings' "Theme" (`theme`) draws the status line, the lines between panes, the panes without the focus and every box dark or light; `system` follows your terminal's background, which the workspace asks each terminal for as it attaches, and stays dark when the terminal doesn't answer. `M-p`'s "Toggle light and dark theme" switches between the two. The programs in your panes keep your terminal's own colors either way. The diff and Notes follow the setting too, `system` by the background your terminal said.

The workspace also asks each terminal as it attaches for its text and background colors and the sixteen colors of its palette, and tells a program in a pane that asks for them your terminal's own, as tmux did: the diff mixes git's green and red over your real background, draws its code in your terminal's palette, and gives its buttons and faint text shades between your background and text.

### Pictures in Windows Terminal

A note's pictures and the PR and ticket image views are drawn with the kitty graphics protocol, which Ghostty and kitty speak. Windows Terminal (1.22 and later, WSL included) doesn't, but draws sixel: set Settings' "Image protocol" (`image_protocol`) to `sixel`, then open Notes or the image view again, and the workspace draws each picture itself as sixel. The apps still give it their pictures as kitty's, by the PNG's path, and write the cells the picture covers; the server keeps the commands rather than passing them on, and draws each picture over its cells in the frame it sends, so a picture scrolls with its note, is cut off where its cells are, and is covered by a box over it, as it would be in Ghostty. Until a picture is read and drawn, a moment after it first shows, its cells are blank. The PR and ticket views (`M-v`, `M-i`) send their pictures inline rather than by path, so while sixel is chosen they show each picture and diagram as its text.

While sixel is chosen the workspace asks each terminal as it attaches for its cell's size in pixels (`CSI 16 t`) and draws pictures to fill their cells at that size. Windows Terminal answers with the 10 by 20 pixels a cell its sixel works in, whatever its font, and scales each cell's share of a picture to the font; where a terminal doesn't answer, the cell its window's size in pixels makes is used, else the same 10 by 20. Other terminals that draw sixel and not kitty's pictures (foot, WezTerm, xterm with sixel on, mlterm) take the setting too. A picture takes up to 256 colors, its own when it has that few, as a screenshot of text often does. A change to the setting reaches the terminals attached at once, each asked for its cell then; a workspace whose server started from a build without the setting takes it after `switchboard upgrade` (or `M-p`'s "Restart Switchboard on the latest build on disk").

With UX 2.0 (`ux2`), a picture is drawn under the tab's header, never over it: one scrolled up past the top of its pane is cut there, its rows still in the pane drawn from the pane's first. With the status line gone, a pane reaches the screen's last row, and a sixel there would scroll the whole screen, so the part of a picture on that row is left blank; Notes and the image views keep their own last row for their messages and footer, so their pictures never reach it. Mission control's cards, which show each tab's last lines, and a peek at a tab's screen copy its text, not its pictures: the cells a picture covers show blank there, and a picture is drawn only where its own pane shows it.

## The palette, messages, tips and menus

### The palette

`M-p`, or a click on ⌘ Commands, opens the palette: every command the workspace has, by what it does, with its tags dimmed beside it and its key in a column of its own, Shift spelled out (`M-S-F`). It lists every action a key is bound to, yours included, and the ones that have no key. `M-1` to `M-9` are one row. Type to narrow the list, each word matched as you write it, and Enter runs the command as its key would, in the pane you were in. Esc closes it. With UX 2.0 (`ux2`), what you type is in a rounded field with the count matched at its right, and under the list the chosen command is previewed: what it does, a sentence or two more on how it works, and its key, or that it has none and can be given one in `keys.json`.

The commands you picked last come first, the newest at the top, then the rest by description. As you type, a command you picked before after typing the same words comes first, then one picked after longer words that start with what you typed, so `sha` finds what you picked after typing `shape`. What you typed before each pick is kept, the last 300, in `~/.local/state/switchboard/palette/`, readable only by you; Settings' "Learn from what you pick" (`palette_learn`) turns that off. After you pick a command that has a key of its own, a tip names the key, once for each key.

While GitHub or Linear is off in Settings, the commands tagged `#github` or `#linear` leave the palette, and every key bound to one (`M-v`, `M-r` and `M-m` for GitHub, `M-i`, `M-l` and `M-W` for Linear, and any you give one in `keys.json`) does nothing but show a tip saying the service is off, the first time, and nothing after. The diff's own commands, copying its story and asking Claude about it, are listed only while the pane has the diff open (asking only with Settings' "Ask Claude about the diff" on), and the two that show the pane a tool stands in for beside it, or the tool alone again, only while a tool stands in the pane's place.

`M-p`'s "Shape a note's checklist items as Linear tickets" picks a note (the one open in Notes first, then the newest), then its items (every one chosen to begin with; Enter on a row leaves it out or puts it back, and the first row shapes what is chosen), then the repository, and opens a tab running `/shape` for each item at the end of the tab bar, the first in front, its details and numbered sub-items with it. "Shape a new Linear ticket from what you type, in a new tab" does the same for one idea typed in. Both need Linear (the `linear` setting). `M-p`'s "View the branch's PR description as an image" draws the description as GitHub does, in place of the pane.

### More in the palette

These are in `M-p` alone, with no key unless you give them one in `keys.json`:

- **Rename the tab**: a name typed in a box, starting from the one it has; empty gives the tab its own name back, as "Rename it" in mission control's menu does.
- **Mission control: \<view\>**, one for each view (Work map, Calendar, Slack, Closed, PRs, Tickets, Inbox, CI, Landing, Sessions, Usage, Resources): mission control opened on that view. A view turned off in Settings is said so at the bottom of mission control, which opens on the tabs.
- **Go to the notifications**: the Notifications window, every notification with its whole message and Enter opening its actions; with Settings' "Notifications window" (`notice_list`) off, the sidebar's cursor on the first notification instead, as `M-a` reaches them, or, in a window with no sidebar, mission control on the first one's view (its Slack, Inbox or Calendar view, Settings for a suggestion), else a box saying why there is nowhere to show them.
- **How you use Switchboard**: what [Suggestions from how you work](#suggestions-from-how-you-work) has counted, in a box: the keys you press most this week and month, the commands you pick in `M-p`, the views you open, and the suggestions now. `j` and `k` scroll it, and any other key closes it.
- **Send Claude's last reply to another tab**: the last thing this tab's Claude said in its last turn, from its transcript, pasted into the Claude of a tab you pick (its active pane when it runs none) without Enter, and that tab brought to the front, so you can add to it before sending. A Claude behind the diff, lazygit or a view in its pane's place comes back beside it first. Its line breaks stay in the text.
- **Update local main to origin's, without switching branches**: `switchboard update-main` (below) for the pane's repository, what it did in a box.
- **Check Switchboard's setup**: `switchboard setup --check`, what it would change, in a box, changing nothing. **Run a part of Switchboard's setup, or only check it** lists the parts (`switchboard setup --parts`) and every part at once, Tab marking several, then runs the ones picked or only checks them.
- **Open a Calendar tab** and **Open a Slack tab**: the tabs the Calendar and Slack views open in the background when they are out of date (`/loop 30m /today`, `/loop 5m /slack-updates`, Claude in your home directory), opened in front.
- **Open a PR in the browser by number or URL**: a PR's number (a PR of the pane's repository as `gh` and `wtpr` read a bare number: the one `gh repo set-default` names, else in a fork's clone the project its `upstream` remote names, else origin's) or its link, typed in a box that starts with the clipboard's when it holds one; empty is the tab's own PR, as `M-r` opens it. Then a menu: open it in the browser, or work on it in a new tab (`wtpr`). Needs GitHub.
- **Open a Linear ticket in the browser by ID, number or URL**: the same for a ticket, a bare number taking the team key of the ticket the branch names, and working on it a new tab running `wt` in the pane's repository. Needs Linear.
- **Open the repository's merge queue in the browser**: GitHub's merge queue for the branch's PR's base branch, or for the repository's default branch when the branch has no PR. Needs GitHub.
- **Open the branch's Linear project in the browser**: the project the ticket in the branch (or the tab's name) is in, as Linear says, or a box saying why there is none. Needs Linear.
- **Copy the branch's ticket ID**, **ticket URL**, **PR number** and **PR URL**: copied, and a box saying what was. The ticket's link is the workspace's in `linear.json` when it names one, else the one Linear gives. Each needs its service.
- **Keep the computer awake until I say, or back to only while tabs work**: the computer held awake whatever the tabs do, a restart on a new build included, until you choose it again, which goes back to Settings' "Keep the computer awake while tabs work" ([Keeping the computer awake](#keeping-the-computer-awake)). A box says which it is now.
- **Filter the sidebar to a repository** and **Show every repository's tabs in the sidebar**: the sidebar's tabs of one repository in their groups, every other one's under Other repos, and the filter cleared ([Filtered to a repository](#filtered-to-a-repository)).
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

Every menu, question and list looks the same: a rounded blue edge with the title in its top, the chosen option highlighted with a yellow `›`, and the keys dim along the bottom. A box that waits on you (a menu, a question, a list, the palette, a message) stands out the same way too: everything behind it dims, the sidebar and status line included, the box sits one shade lighter than the background, and a soft shadow falls under it, drawn as an image where the terminal can show one and as half blocks (`▌` `▀`, meeting in a `▘` at the corner) along its right and bottom where it can't. A tip in the corner and the sidebar's peek leave the screen as it is. In a menu, `j` and `k` (the arrows, and the wheel with Settings' "Click in menus" on) move, `g` and `G` go to the first and last, Tab and Shift-Tab go round, Enter or Space takes the chosen option, a number takes that one, as does an option's own key where one has it (never one the menu moves or closes with), and Esc or `q` goes back. In a list to type in, such as the palette, the arrows (`C-p` and `C-n`, Tab and Shift-Tab) move, Enter picks, and Esc closes.

A question shows `[Y/n]` and takes Enter as yes; `y` says yes too, and `n`, Esc or any other key says no. Space answers only once Tab has moved between Yes and No, so a stray Space is still no.

Menus and questions take a click too: one click takes an option or answers Yes or No, a click on a list's row twice picks it, and a click outside the box closes it (Settings' "Click in menus", `menu_mouse`). With it on, Settings' "Highlight on hover" (`hover_highlight`) lights the option under the mouse, and Settings' "Easy mode" (`easy_keys`, off by default) puts a `×` in each box's top-right corner, which a click, or Tab to it and Enter, closes. A box's options always take Tab and Shift-Tab; Settings' "Tab moves focus" (`tab_focus`) does the same for mission control and the sidebar. A box follows these settings as they are when it opens.

### Closing a busy pane

`M-w` closes a pane at once when it is a shell at its prompt. When a program runs in it, it asks first, "Close pane?", with the program's command line under the question, so a running agent or build isn't lost to a stray key. Settings' "Ask before closing a pane" (`confirm_close`) turns the question off.

## The sidebar

The sidebar runs down the left of every tab: the session's tabs, most in need of you first, what each one's Claude is doing, under them the first items of each plugin that asks for a place there (see "Plugins"), and the notifications. It is 34 columns wide, and a window narrower than 90 columns has none (both are set in `sidebar.json`, below); a zoomed pane covers it. Settings' "Sidebar" (`sidebar`) turns it off and on, as does "Turn the sidebar on or off" in `M-p`.

At its top are the session's name, in a pill of the current tab's color, and its counts: failing builds first, in red (`✗ 2`), then the tabs waiting on you (`! 1`), finished (`✓ 3`) and working (`⋯ 4`). Below them are the tabs in their groups, each group under its heading with how many it holds.

On a screen too short for every tab, the session's line at the top and the notifications at the bottom stay put and only the groups and tabs scroll: the line under the session's says how many tabs are above (`▲ 3 more`) while no filter is typed there, and the list's last row how many are cut below (`▼ 2 more`). The list holds still as you go from tab to tab: it stays where it was while the tab in front is in sight there, else at the top while the tab is in sight there, else it scrolls to the heading of the tab's group.

### Filtered to a repository

`M-p`'s "Filter the sidebar to a repository" picks one of the repositories the session's tabs are in, each with how many tabs it has, "Every repository" first and the one filtered to now marked `now`; a worktree counts as its main clone, so every worktree of one repository is in it. Every other repository's tabs go under Other repos at the bottom, whatever they are doing, and a box says "Sidebar: switchboard only". Other repos is folded to one line of its tabs' marks, so one waiting on you still shows, their count and their repositories by name; `Enter`, `l` or a click on that line unfolds it, listing its tabs ordered by the group each would otherwise be in, and a click on its heading folds it again. `M-a` and `M-A` pass those tabs by, and their notifications are as before. The top line shows the filter as a chip, "switchboard ×", and how many of the tabs are its ("7 of 9"); a click on it clears the filter, as "Show every repository's tabs in the sidebar" in `M-p` does. The filter and whether Other repos is unfolded are kept with the layout, through a restart and an upgrade, until cleared. `switchboard sidebar-repo <dir>` filters to the repository a directory is in from a script, and `--all` clears it. Settings' "Sidebar repository filter" (`sidebar_repo_filter`) off takes the commands away and puts every tab in its own group.

### Groups

Each tab is in one group, the same as its stage in mission control's Tabs view, and the groups come in this order:

| Group | What puts a tab there |
| --- | --- |
| Needs you | Its Claude waits on you or has finished there (a turn that ends while only watches run, too, the watches named: "finished 2m ago · monitoring · CI on #12"), you pinned it, or its reminder came due. Pinned tabs and reminders come first, then waiting tabs, the longest-waiting first, then finished ones, the oldest first. |
| Build failing | Its open pull request has a failing check and isn't in a merge queue, and no `/ci-loop` is at work on it. The heading is in red. |
| Working | Claude is working, or its turn ended with background tasks still running, which Claude carries on from when they end, with none of the loops below. |
| Monitoring | Claude's only background work is watches, each named ("monitoring · CI on #12"): `Monitor` watches, and background shell commands that only wait (a `while` or `until` loop that sleeps, `gh run watch`, `tail -f`) and run nothing but a sleep and commands that look (`grep`, `kill -0`, `gh run view`, `git log` and the like), named by their description; one that builds or tests between its looks stays working. A turn that ends while the watches run shows under Needs you first, then here once seen. |
| Review loop | Its Claude works under `/review-loop`, from the moment you ask for it and between its rounds, or the loop runs in the background. |
| CI loop | The same for `/ci-loop`, a failing build's included. |
| Queue-and-clean | The same for `/queue-and-clean`, until its pull request is queued, landing or merged. |
| Loops | A `/loop` between its ticks: a self-paced one while the wakeup its last tick scheduled is pending, and for five minutes after it is due; one on a timer (`/loop 5m …`, or a cron job Claude created) until Claude deletes the job, the session ends, or seven days pass. |
| In merge queue | Its pull request is in the merge queue, in queue order, with Settings' "Show the landing queue" off. |
| Landing | It holds or waits for the landing lock of `/queue-and-clean`'s "Land on the default branch", in the landing queue's order, or its pull request is in the merge queue ("merging" at its head, else "2nd in line to merge"). |
| Testing Deploy | Its pull request merged and the tab is still at work on it: `/deploy-test` waiting for the deploy or recording its test, or Claude working under `/deploy-test` or the `/queue-and-clean` that follows the merge. |
| Merged | Its work landed: its pull request merged, or `/queue-and-clean` landed it straight on a branch ("landed on main · 1a2b3c4", "landed on main without asking"), while the tab is in that repository's main clone on the branch it had, where the landing leaves it, until a prompt there starts new work. |
| Idle | A Claude session with nothing to do. |
| Reminders | A reminder still to come, ahead of Build failing, while Claude isn't working there. |
| Other | No Claude in it: Notes, the Slack relay, a shell. A Claude parked behind the diff, lazygit or a view in its pane's place keeps its tab in its own stage. |
| Other repos | While the sidebar is filtered to a repository, a tab of any other, whatever it is doing (see [Filtered to a repository](#filtered-to-a-repository)). |

Needs you always wins: a tab whose Claude waits on you stays there even while its pull request is queued. After it, the landing queue and the pull request's place win over what Claude is doing, so a tab whose PR is queued stays under Landing while `/queue-and-clean` watches it; then a reminder still to come, while Claude isn't working there, so a tab you set one on waits whatever its checks say (its row still shows the failing checks in red); then a failing build, unless `/ci-loop` is at work on it; then the loop running; then the work running. A tab's pull request, its checks and whether it is queued or merged come from one GitHub query for every tab's PR (see "How often mission control looks"), so a group can lag GitHub by that long. A tab whose worktree has been removed, as `/queue-and-clean` leaves it once its PR merges, is found by the worktree's branch and keeps its PR (in a fork's clone, a PR from your fork's branch in the project `upstream` names counts, one in your fork winning).

Settings' "Build failing" (`build_failing`) off leaves such tabs in Working or Idle, the failing count still in their words. Settings' "Show the landing queue" (`landing_queue`) off leaves landing tabs where their work puts them, and a tab whose PR is queued under In merge queue; the queue still orders the landings.

### What each tab shows

A tab's row starts with a bar in its project's color (thicker on the chosen tab), its badge, its number (for `M-1`…`M-9`) and its name as the tab bar shows it, wrapped to at most three lines. Under it is what it is doing and for how long, as its Claude's hooks record it: "waiting on you 4m · permission: Bash git push", "finished 2m ago", "working 2m · Run the test suite", "working in background · 2 tasks", "idle for 20m". In auto mode a permission request counts as waiting only once Claude Code shows it to you: a call auto mode allows by itself stays work. A request whose hook names no tool takes the call from the transcript ("permission: Bash: sleep 30"). A tab running a known loop names it from the moment you ask for it (by its bare name or Switchboard's plugin's, `/switchboard:review-loop`), the work between its rounds included: "review loop · round 2" while `codex-review` runs, two runs to a round; "CI loop · round 3" while `/ci-loop` watches the checks; "queue-and-clean · watching" while `/queue-and-clean` watches the merge queue, or the step it is at while it lands on the default branch ("queue-and-clean · waiting for the landing lock"). A loop between ticks says when the next one is and the loop's prompt ("next in 8m · /oncall"). The row adds the place of its pull request ("in merge queue at 3", "merged") or in the landing queue ("landing", "2nd in line to land"), the PR's number when the name doesn't already say it, and a failing check in bold red ("✗1 failing") whatever else the tab is doing. A tab with no Claude names the program in front.

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

The states are the ones the tab bar's marks show, recorded by Switchboard's hooks for Claude Code.

### Notifications

The notifications sit at the bottom of the sidebar, under a double yellow rule and their own heading, so they never scroll away and never read as a tab. Each kind is one line, saying how many there are, with a line of detail from the first under it, so the tabs keep their room however many come in; the Notifications window lists each one. The kinds, in this order:

- the next meeting, as it is about to start ("Standup in 3m · ↵ joins");
- new Slack messages for you ("3 new on Slack"), until you have seen them in the Slack view or dismissed them;
- changes to today's calendar since you last looked at it ("Calendar: Standup moved to 3pm", or "2 calendar changes");
- notes written for you ("New note: Standup 2026-11-02", or "3 new notes"), until you open them in Notes;
- unread notifications of your Linear Inbox ("2 in your Linear inbox"), while the Inbox view is on;
- your PRs' news ("PR 212 merged, web-app", "PR 2019 approved, web-app", "Checks failed on PR 2027, web-app", or "3 PR updates"), found by comparing each answer of the PRs board's lookups with the one before, so nothing that happened before the workspace started is said; each until you dismiss it, the newest 20 kept (Settings' "PR notifications", `pr_notices`);
- a default branch whose CI fails ("main failing at abc1234: Tests"), while autopilot works on its repository or a tab waits to land on it and its landings test in CI, once for each failing commit, until you dismiss it or a later commit passes;
- each landing autopilot made without asking ("Landed <short hash>: <item>", or "Landed 2 changes without asking"), until you dismiss it or go to its tab;
- what autopilot has to say: that it stopped for the day's spend, or the note has nothing left and none of its tabs is open;
- suggestions from how you work.

"Your day", "How you use it" and "Autopilot", below, say more of each.

Moving past the last tab with `j`, `n` in the sidebar, or a click puts the cursor on them, and brings the Notifications window to the front, as moving onto a tab brings that tab: a tab of its own, named Notifications in the tab bar only while it is in front, listing every notification with its whole message (a Slack message as Slack draws it), the one the cursor is on chosen. `j` and `k` in the sidebar go through them one by one, the window following, and up from the first goes back to the last tab. Enter in the sidebar moves into the window, where `j` and `k` choose (the sidebar's cursor following), Enter, `l` or a click on the chosen one opens its menu over it, as in mission control's Notifications view, and `Esc`, `q` or `h`, or a click on "‹ tabs", go back to the tab it was brought in front of; with Settings' "Tab moves focus" on, Tab and Shift-Tab go round "‹ tabs" and the notifications. The window is no tab: the sidebar, mission control, `M-1`…`M-9`, the next and previous tab keys and `switchboard tabs` leave it out, it stays after every tab, and it isn't saved with the layout. Settings' "Notifications window" (`notice_list`) turned off leaves the cursor on them in the sidebar, a kind at a time, where Enter or a click does what that kind leads to: the meeting joins its call (or, with none, opens mission control's Calendar view), the newest note opens in Notes, calendar changes open the Calendar view, Slack messages and Inbox notifications open mission control's Slack and Inbox views, suggestions open Settings, autopilot opens mission control on its line, a landing it made without asking goes to its tab while it is open, and the landed tabs it closed unseen are dismissed. Each one's menu, the same box in the Notifications window and mission control's Notifications view, is its item's menu in the view it came from with dismissing added (a Slack message's is the Slack view's menu, the message whole above it and dismissed until its thread gets a reply), and whatever is picked there dismisses it, since it was dealt with; a suggestion's menu has its own choices. Settings' "Notifications" (`sidebar_notifications`) turns the block, and that view, off.

### Into the sidebar and its keys

`M-s` moves into the sidebar, on the tab you are in, and back out; `M-Left` from the leftmost pane moves in too, and `M-Right` back out. While it has the focus, its cursor moves through the tabs and each comes to the front as the cursor reaches it, with the list held still so tabs don't move about as you look at them. Leaving keeps the tab you are on. A key that acts on "the pane" (a split, `M-w`, zoom) gives the focus back to your pane first and acts there; `M-a`, `M-A`, `M-1`…`M-9`, `M-f`, `M-y` and `M-p` keep it, and pressed from the sidebar `M-a`, `M-A` and `M-1`…`M-9` leave the cursor on the tab they went to.

| Key | In the sidebar |
| --- | --- |
| `j` `k` (arrows, `C-n` `C-p`) | the next or previous tab, brought to the front |
| `g` `G` | the first or last tab |
| `C-d` `C-u` | the list half a screen down or up, the cursor onto the tab there, stopping at the first and last tab, as in Vim (Settings' "Half-page scrolling", `sidebar_half_page`; off, they run in the chosen tab) |
| `Enter` `l` | into the chosen tab's pane; on a folded Other repos, unfolds it; on a notification, into the Notifications window (with it off, what the notification leads to) |
| `h` `q` `Esc` `C-c` | back out to your pane (`Esc` clears a kept filter first) |
| `1`–`9` | the cursor onto that tab, brought to the front |
| `/` | find a tab by name: typing narrows the list, the arrows move, Enter goes, Esc clears |
| `Tab` `S-Tab` | the next or previous row, tabs then notifications, wrapping round (Settings' "Tab moves focus", `tab_focus`; off, `Tab` opens mission control) |
| `Space` | peek at the chosen tab |
| `a` | answer the chosen tab's Claude, or send it a message, in mission control's box (see [Answering a tab](#answering-a-tab)) |
| `n` | to the notifications, or a box saying there are none |
| `t` | a new tab running Claude, as `M-t` opens one, from the chosen tab, so its directory is first in the list |
| `i` | the chosen tab's whole title in a box, as `M-T` shows it |
| `d` `L` | the chosen tab gone to, its diff (as `M-d`) or lazygit in its pane's place |
| any other Alt or Ctrl key | goes to the chosen tab and runs there |
| `?` | these keys |

A click on a tab goes there, a right click peeks at it, and the wheel moves the cursor, bringing the focus into the sidebar. A click on the `▲ more` or `▼ more` row scrolls half a screen, as `C-u` and `C-d` do, whether or not a filter is being typed (with `sidebar_half_page` on). A click anywhere else takes the focus back out. With Settings' "Highlight on hover" (`hover_highlight`) on, the row under the mouse is lit.

### Peeking

A peek shows a tab without going there, in a box over most of the screen: its last turn in full (your prompt, all of Claude's reply, and the tool call it waits on), or, for a tab with no Claude, its screen with 200 lines of what scrolled off above it. `s` switches between the turn and the screen, `r` reads the screen again, `j` `k` scroll (`C-d` `C-u` half the box, `g` `G` the ends), Enter goes to the tab, and `Esc`, `q`, `C-c` or `Space` close it, so `Space` in the sidebar opens and closes it; with easy mode's ×, Tab moves onto it and Enter presses it. `Space` in the sidebar, a right click on a tab, and "Peek at its last turn" in a tab's menu in mission control open it.

### What needs you, one tab at a time

`M-a` goes to the next of what needs you, across every session, in the sidebar's order: the tabs under Needs you, pinned ones and reminders come due first, then those waiting on you, then those that finished, and last, while there are any, the notifications (Settings' "M-a goes to notifications", `needs_notices`), where it puts the sidebar's cursor on the first one. Each press goes on to the stop after the one you are at, wrapping round, so pressing it again and again walks the whole list, though a tab stays waiting until you answer it. A finished tab leaves the list once seen, so pressed again from there the walk carries on where it was. From a tab that only finished, or one not on the list, a tab pinned or waiting on you comes first. With nothing to go to it says "Nothing needs you."

`M-A` goes to the next idle tab the same way: the tabs under Idle, in the sidebar's order, across every session, the one after the tab you are at, wrapping at the end, or the first from a tab that isn't idle. Only the sidebar's grouping can tell an idle Claude from one working in the background or looping, so those aren't on it, nor a tab with no Claude. It says "No idle tabs." when there are none, and "This is the only idle tab." from the only one. Settings' "M-A cycles idle tabs" (`idle_walk`) off hands `M-A` to the pane.

### Pinning and reminders

`M-f` pins the tab you are in: it goes to the top of Needs you with a yellow `📌`, whatever its Claude is doing, and stays until `M-f` unpins it, since going to it doesn't clear it. A tab's menu in mission control pins and unpins it too.

"Remind me about this tab later" in `M-p`, or "Remind me about it later" in a tab's menu in mission control, asks when, in a list (in 20 minutes, in 1 hour, in 3 hours, tomorrow at 9:00, next Monday at 9:00) where a choice's number and Enter take it, or a time typed in (`45m`, `2h`, `1d`, `3pm`, `15:30`, `tomorrow 9am`, a clock time already past today being tomorrow's), which wins over a row it only resembles, then what for, which may be left empty. Until then the tab waits under Reminders with a blue `⏰` and the time left ("remind in 1h 20m · check the deploy"), even while its build fails, unless something else places it: working, it is under Working (or Build failing), waiting on you or finished, under Needs you, and its PR landing, queued or merged, there, each with its own badge and the reminder's words beside it. When the time comes it moves to the top of Needs you with an orange `⏰`, and a box on the screen used last names it, with a desktop notification while Settings' "Push notifications" (`notifications`) is on. It is done with once you are at the tab. Asked for again on a tab with a reminder, the menu offers "Clear the reminder" first. Reminders are kept with the tabs and survive a restart. Settings' "Reminders" (`reminders`) off neither sets nor shows them.

### The Landing group

`/queue-and-clean` in a repository that lands straight on its default branch lands each worktree's work one at a time, behind a landing lock, the others waiting in a landing queue. The tab holding the lock is first under Landing, saying "landing" or the step it is at; the tabs waiting follow in the queue's order ("2nd in line to land"). A tab counts only while its worktree is there and the Claude session that took the lock or joined the queue still runs, wherever it started (a session started in the main clone that went into the worktree to land it included), or any Claude session runs in the worktree, so a landing that ended without a word leaves the group. The tab shown is the one whose pane took the lock or joined the queue, else the one whose Claude runs that session, so a landing run from a directory other tabs share files only its own tab; a lock taken by an older build, which names neither, goes to the tab in its worktree, and to none when several tabs are there. Enter on a waiting tab's card in mission control offers "Land it next", "Move it up in the landing queue" and "Move it down in the landing queue", which reorder the queue as `switchboard land-lock move` does; the landing tab can't be passed. A tab whose pull request is in the merge queue joins the group too, "merging" at the queue's head, else "2nd in line to merge".

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

The cards are grouped in stages, the sidebar's groups: Needs you, Build failing, Working, Monitoring, Review loop, CI loop, Queue-and-clean, Loops, In merge queue, Landing, Testing Deploy, Merged, Idle, Reminders, then Other. A tab running a loop is in that loop's stage, from the moment you ask for it, while Claude works between its rounds too; a tab whose pull request is in the merge queue, landing or merged is in that stage even while it runs a loop; and a tab with a failing build that runs `/ci-loop` stays under CI loop while the loop is at work. Each stage is a box in the stage's color, titled on its top edge, packed side by side so the screen fills: a stage that doesn't fit in the room left on a row starts the next, and one too big for a row takes several rows of cards. While a note is worked continuously, a line above the cards says so ("Autopilot", under Worktrees).

A card's top edge carries the tab's badge, number and name. Inside are what it is doing, its pull request's number, checks, review and size ("#12 · ✓12 ✗1 ⏳3 · approved · +340 −80"), and the session's name; then your last prompt, the end of Claude's last reply, and, when it waits on a permission prompt or a question, what it is asking. A tab with no Claude shows its directory.

### Keys

Mission control's own keys only move about; everything that acts on what is chosen is in the menu Enter opens.

| Key | In mission control |
| --- | --- |
| `h` `j` `k` `l` (arrows) | move between cards or rows; `j` `k` move a row of cards keeping the column, on a board `h` `l` move between its columns, and in the Calendar between events side by side |
| `g` `G` | the first or last |
| `J` `K` | with UX 2.0, scroll the pane beside a board (a PR's or a ticket's description, the Autopilot plan) down or up a line; the wheel over the pane scrolls it three; it shows another item from its top |
| `1`–`9` | choose that tab |
| `]` `[` | the next or previous view; with UX 2.0, round the views of the group in front |
| `}` `{` | with UX 2.0, the first view of the next or previous group |
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

A click on a view's name goes to that view, on a group's name (with UX 2.0) to its first view, a click on the filter starts typing in it, and a click on a card or row chooses it, a second click doing what Enter does. The wheel moves through the cards or rows. With Settings' "Highlight on hover" (`hover_highlight`) on, what a click would press is lit under the mouse.

### Menus

Enter on a card or row opens a menu of what can be done with it, its usual action first, so Enter twice does it. `j` `k` move, Enter or Space takes the chosen option and a number takes that option; its options have no letters of their own, as the tmux version's had none, so a letter never acts ahead of moving. Esc or `q` goes back. With Settings' "Click in menus" (`menu_mouse`) on, a click takes an option and a click outside closes the menu. An option that takes a moment (opening a link, asking Linear, removing a worktree) shows a box with a spinner until it is done (a link until the browser comes to the front, up to eight seconds, or a moment on Linux, where that can't be told), and what it did is said at the bottom of the screen.

On a tab, the menu offers:

- go to the tab;
- answer it here, or send it a message (below);
- peek at its last turn, or at its screen for a tab with no Claude;
- show its full title, as `M-T` does, with Copy the title;
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

After the cards come Notifications, the Work map, Calendar, Slack, Closed, PRs, CI, Landing (while something lands), Tickets, Inbox, Sessions, Resources, Usage, Keys and Settings, then each plugin's view, each only while its setting is on. The ones that read a service ask it again when you go to them.

**Notifications** is every notification of the last two weeks, kept by day, newest first: under "Today", "Yesterday", then each weekday, a row each with its time, its kind's mark, its title, what it led to and whether the sidebar still shows it (`shown`, `dismissed`, `read`, or `older` for one its source let go by itself, such as a Slack message seen in Slack). What it led to is recorded as it happens: the choice picked from its menu ("you went to tab 5", "a tab opened for it", "you chose yes", "marked read in Linear" once Linear answers, "opened in the browser", "you opened it at 9:02"), and for a landed tab autopilot closed, "tab 2 closed"; one with nothing recorded leaves it empty. With UX 2.0, a row of keycaps along its top counts the week's notifications of each kind ("1 All 23", "2 Landed 6", "3 PRs 5", "4 Slack 4", "5 Inbox 3", "6 Calendar 3", "7 Notes 2"), the one shown lit, with "23 this week · 5 in the sidebar" at its right; the column heads sit over a dashed rule, a title is bright while the sidebar shows it and dim after, and its state at the right is in the accent while shown. Without it, the rows say what each led to and its state after its title. `1`-`7` (or a click on a keycap, or Tab to one and Enter) show one kind, `1` all of them; suggestions are under All alone. The days before yesterday that start past the first screenful fold into a row each ("▸ WEDNESDAY   7 more, folded · Enter shows them"), which Enter opens. Enter on a notification goes to it, doing its menu's first choice (a Slack message opens in Slack, a PR's news goes to its tab or opens a tab for it), `m` opens its whole menu, and `x` puts it out of the sidebar, or says "It's already out of the sidebar." for one that is; a notification no longer in the sidebar is neither dismissed after what is picked nor offered dismissing. The history is kept on this computer only, a file a day in `~/.local/state/switchboard/notifications/`, the days past two weeks removed; Settings' "Notifications' history" (`notification_history`) off keeps nothing, and the view lists only what the sidebar shows, one by one and whole, Enter opening each one's menu, as below.

A notification's menu is its item's menu in the view it came from, with dismissing added: a Slack message's is the Slack view's, the message whole above it as there, an Inbox notification's the Inbox's, and the next meeting's its event's in the Calendar view, so a choice a view's menu has is in its notifications' too. A kind no view lists leads where it leads ("Open it in Notes" for a new note, "Open the Calendar view" for a calendar change, autopilot's view for what it said, its tab and its plan for a landing made without asking) and is shown whole above its choices, here as in the Notifications window. Dismissing is the view's own where its menu has one (a Slack message's "Dismiss it, until its thread gets a reply"; dismissing every message shown stays that view's), else "Dismiss it": the meeting until another, a calendar change by taking today's calendar as seen, a new note, an Inbox notification from the notifications alone (it stays unread in Linear and in the Inbox view). Whatever is picked dismisses it too; a suggestion gets its own choices instead. It is there while the sidebar's notifications are on.

**Work map** is every piece of work in the repositories your tabs are in, a row each, under a heading for each repository: each worktree, and a main checkout a tab is in, with its ticket (from the branch, while Linear is on), the tab it is open in or "no tab", its pull request and where that stands ("no PR", "#12 open", "checks running", "✗2 failing", "in merge queue at 3", "merged"), and its directory. Worktrees no tab has open are there too, from every repository a Claude session worked in over the last two weeks, each with when it was last used and the last prompt given there. Every open tab has a row, one in no repository under "Other tabs" last. Under each repository its main checkout comes first, then rows needing you, work going on, tabs, merged work, and work no tab is in, the most recently used first; the repository with the most pressing row comes first. Each repository's merged work no tab is in sits last under a line of its own ("▸ 2 merged worktrees, folded"), closed until Enter on that line opens it, and Enter on a repository's heading folds the repository or opens it again; a typed filter opens them all; a worktree's pull request is asked for by its branch (in a fork's clone, in the project `upstream` names too, from your fork's branch alone), so work with no tab shows where its PR stands too. On a row with a tab, Enter's menu goes to it, or opens its diff (`M-d`) or lazygit in its pane's place; on a worktree with no tab it offers to open a tab there and resume its Claude (as `switchboard claude-here` does), its diff or lazygit in a tab of their own there, which closes with them, or copying its directory, and on merged work, to remove the worktree after asking, which keeps one with changes and says why. The worktrees are read with git about once a minute while the view is in front. Settings' "Work map" (`view_work_map`); `M-j` and `M-o` open it whatever that says.

With UX 2.0 on, the Work map is a table, its columns' heads once at the top: the worktree (its branch, or its directory's name), the ticket, the tab (its state's mark, its number in its color and its name, or "no tab"), the PR, where it stands ("In review", "Approved", "Changes asked", "2nd in the merge queue", "3rd in the landing queue", "Landed 1a2b3c4", "✕ tests failed", "2 commits to push", "no PR"), its checks ("✓ passed", "◌ 3 of 5", "✕ 1 failing") and when it was last used ("now" while its tab works). Each repository's heading says where it is and how it lands ("lands straight on main" or "lands through a PR", from repos.json or what was last worked out, while GitHub is on), with "N worktrees · M open" at the right. On a screen 30 rows high or more, the chosen row opens a pane under the map: its path and the tab it is open in, its last prompt, and its branch, with how many commits it is ahead of the default branch and what it has to push, counted with git in the background for that row, once for each commit it is at. Its keys: Enter goes to the row's tab, or opens a tab in its worktree and resumes its Claude, with no menu; `o` shows only the rows with a tab and `n` only those without, its key saying "show all" until pressed again; `z` folds or opens the chosen row's repository; `/` filters; `m` opens the row's menu. Each key on the footer takes a click too.

**Calendar** and **Slack** are in "Your day"; **Closed**, **Sessions**, **Resources** and **Usage** are in "How you use it".

**PRs** is a board of your pull requests in the repositories your tabs are in, a fork's clone counting as in both its `origin` and the project its `upstream` remote names (where the pull requests from your fork are), in columns: Draft, In review, Approved, In merge queue, and Merged in the last 7 days, each with its checks, review and place in the queue, failing checks or changes requested marked as needing you. `h` `l` move between columns. Enter offers to go to the tab it is open in, work on it in a new tab (`wtpr`), read it here in a pane beside the one you were in, open it in the browser, or copy its link. Settings' "PR board" (`pr_board`).

**CI** is your open pull requests in the repositories your tabs are in (a fork's upstream among them, as on the PR board), under a heading each with their counts: Failing (a canceled check counts, since it never passed), each with the checks that failed and the line of the first failing job's log that says why; Running, with how many checks passed so far ("4 of 5 passed, 1 running"); and Passing ("9/9"). With UX 2.0 on, each failing PR is its mark, number, title and the tab it is open in ("tab 6", or "no tab"), a dim line of its repository, branch and how many checks failed ("2 of 9 failed"), then a line for each failed check with the line of its log that says why; a running or passing PR is one line. `j` and `k` move through the PRs and their failed checks' lines, and a click chooses one. The chosen check (a PR's first, on its own row) is a pane beside the list, or under it on a screen narrower than 140 columns: its name, the PR, run and how long it ran, then only the step of its log that failed (the one holding its last error), its lines numbered as in the whole log, the line that says why tinted red with `▶` and opened in sight, `J` and `K` scrolling the rest; under it each other failed check of the PR with its own line that says why. Each failed check's log is read once and kept, so the step costs nothing more. `l` reads the chosen check's whole log in a pane beside yours, from its failing step, `o` goes to the PR's tab (saying when no tab has it), and `M-r` opens the check in the browser; the footer's keys take a click too. Enter offers to start a CI loop on a failing one (typing `/ci-loop` into the tab already on it when its Claude is idle or finished, else opening a new tab on its worktree with `wtpr`), go to its tab, read the failing job's log in a pane beside yours (the log of each failing job is kept in `ci-logs/` in the state directory once the view has read why it failed, and goes when the job no longer fails; it opens at its failing step, else at its end, without the timestamps Actions puts on every line), open the failing check in the browser, read the pull request here in a pane beside the one you were in or open it in the browser, or copy its link. Above them, while one fails, is each default branch Switchboard watches (Settings' "Watch the default branch's CI", `main_ci_watch`: a repository whose landings test in CI, while autopilot works on it or a tab waits to land on it), "main failing at <short hash>: <job>" with the line of its log that says why, asked every two minutes; its menu reads the failing log or opens the failing run, and it stays while a later commit's CI still runs, going once one passes. Settings' "CI" (`view_ci`).

**Landing** (`M-p`, "Mission control: Landing") is there only while something lands in a repository a tab is in, or while it is in front, saying "Nothing is landing now" once the last landing ends until you leave it; a client stays on the view it was on when Landing comes or goes. It has a section per repository and branch landed on, headed "<repo> · straight to <branch>" for the default branch's landing queue, "<repo> → <branch>" for a target branch's (autopilot's "Land on:"), or "<repo> · merge queue", its rows in queue order. A landing-queue row is the tab landing or waiting, the one whose pane took the lock or joined the queue (the worktree's folder when no tab has it), with its step ("landing · pushing") or "waiting", how long since it took the lock or joined, and its tab; Enter offers to go to the tab, Land it next, move it up or down, take it out of the queue after asking (`switchboard land-lock leave`, which stops its wait), or copy its directory. A merge-queue row is every pull request in it, everyone's: its place, number and title, the repository, where it stands, about how long until it merges as GitHub estimates it, its author, and the tab it is open in; one of your tabs' that can't merge is marked as needing you. Enter offers to go to the tab, read the pull request here in a pane beside the one you were in, open it or the queue in the browser, or copy its link. The merge queues are asked of GitHub with the `github` setting on; the landing queues are read from files. Settings' "Landing view" (`view_landing`); a "Merge queues" turned off before is read as it turned off.

**Tickets** is a board of the Linear issues assigned to you, in columns: Todo (urgent first), In progress, In review (a started state whose name says review), and Done in the last 7 days, each with its state, priority, project and the tab working on it, an urgent one marked as needing you. `h` `l` move between columns. Enter offers to go to the tab it is open in, start work on it in a new tab (`wt`), shape it in a new tab (`shape`), close it in Linear (moving it to its team's first completed state, after asking), read it here in a pane beside the one you were in, open it in the browser, or copy its ID. A new tab starts in the directory of a tab working on a ticket of the same team, else your home directory. Settings' "Ticket board" (`ticket_board`).

**Inbox** is your unread Linear notifications that need you, newest first: a mention of you in a ticket or a comment, a new comment on a ticket of yours, a ticket newly assigned to you; status changes and the rest of Linear's inbox are left out. Each is in the sidebar's notifications too. Enter offers what the Tickets board offers for its ticket, and to mark it read in Linear. Settings' "Inbox" (`view_inbox`).

**Keys** is below, as are **Settings**; each plugin's view comes last (see "Plugins").

Settings' "GitHub" (`github`) off takes out the PRs and CI views and the Landing view's merge queues, and Settings' "Linear" (`linear`) off the Tickets and Inbox views.

### When a view isn't set up

A view that is on is always there to go to, and before what it reads from is set up it says what is missing and how to set it up. GitHub's views need a token and Linear's a key: "GitHub and Linear access", under "Reading beside your work", says where each is looked for and how to store one, and mission control picks a stored one up within two minutes. A view whose tabs are in no GitHub repository says to open a tab in one. A look that fails for another reason (the network, a rate limit) keeps what the view had, with the error above it.

### How often mission control looks

GitHub and Linear are asked every 15 seconds, and the PR board every 30, with Settings' "Look often" (`poll_fast`) on; off, every tab's pull request and the merge queues are asked every minute and the rest every two minutes. The merged column of the PR board is asked for only every ten minutes. A lookup still waiting on its answer when the next is due is skipped rather than started twice. Going to the Inbox, CI or Landing view asks again at once, and so does `M-m`, for the views that show pull requests, so they agree with what it says.

While you are away (no terminal attached, the screen locked, or 10 minutes with neither the computer's keyboard and mouse nor a key in Switchboard used; a key pressed in Switchboard from another computer, over SSH, counts even with the screen locked) the GitHub and Linear lookups, the calendar feed, the Slack look and the diff's stories told ahead pause, and all ask again when you are back. Settings' "Pause lookups while away" (`pause_when_away`) off keeps them going. Autopilot opens no tabs while you are away, whatever that setting says.

### Finding a tab or a worktree

`M-j` opens the Work map showing only the open tabs, and `M-o` only the worktrees no tab has open (and the merged ones under their repository's line), with no repository's heading, each with a filter already typing: the arrows, `C-n` `C-p` and Tab move, Tab going round past either end (stopping on easy mode's × on the way, which Enter closes), Enter opens the chosen row's menu, its usual action first, so Enter twice goes to the tab or opens one in the worktree and resumes its Claude, and Esc closes the menu, then the picker. The filter types on while a menu is open over it.

### The Keys view

`M-/` opens mission control on its Keys view: every key Switchboard binds, in one list. First the workspace's, the keys above and yours from `keys.json`, as `M-p` lists them, each under the first of its tags (panes, tabs, git and the rest), the biggest group first, a command with no key of its own shown as `M-p`, which reaches it; then the sidebar's and mission control's own (what their `?` shows), then the diff's (`M-d`: its own keys, the cover while its story is told, and, with Settings' "Ask Claude about the diff" (`diff_ask`) on, the box `ga` opens and the answer it shows), then Notes' (a note's, its leader keys and, with Easy mode on, easy mode's Cmd keys, then the list's). Last come the commands with no key of their own, under "Through M-p", which reaches them. Each row is the key, what it does and its tags, with a "yours" chip on a key `keys.json` binds. `/` filters by a key, its words or any of its tags, leaving out the groups with nothing that matches, and Enter on one of the workspace's keys presses it at once in the pane mission control was opened over, as an `M-p` pick is; the diff's commands, which only the diff takes, and the other parts' keys are there to read, and Enter on one says which app takes it. Settings' "Keys" (`view_keys`), under Mission control, takes the view away and hands `M-/` back to the pane.

With UX 2.0 and mission control at least 100 columns wide and tall enough for its panel of groups, the view is a board, laid out from the screen's width: the table's column of what a key does and the chosen key's description take what the rest leave. Narrower, the table's tags column gives way first, then the chosen key beside the keyboard. At the left, its groups: the workspace's tags with how many keys each has, then each app's own keys (the sidebar, mission control, the diff, Notes), then those only through `M-p`, the group in front on the highlight. At the top right, the keyboard as a heatmap: each letter and digit a key, filled in the accent by how often you pressed it with Alt this week (`M-j` and `M-J` both count for J), the keys the group in front binds outlined, a line under it saying which group that is. The counts are Settings' "Suggestions from how you work" (`habits`), kept on this computer only and read again about once a minute while the view is in front; with it off, the keyboard's place says to turn it on. Beside the keyboard, the chosen key: its keycap and what it does, a longer description, whether it is the default or yours, and how often it was pressed this week ("212 this week · your most used"). Under a rule, the group's keys as a table: the key, what it does, its tags, and "default" or a "yours" chip. `h` and `l` move between the groups, `j` and `k` between the group's keys, and a click on a group, a key of the keyboard or a row chooses it. Narrower or shorter, or with UX 2.0 off, it is the list.

`?` on the Keys view opens "Find a key": press any key, the way you would anywhere, and it comes up as a large keycap with what it does, its tags, whether it is the default or yours, how often it was pressed this week and its longer description, nothing pressed. A key an app takes for itself (`j`, the sidebar's) is named with its app; the prefix waits for the key after it; a key nothing is bound to says `keys.json` can bind it. Under a dashed rule is the key you looked up before it. Enter, or a click on its keycap, closes the box with the key chosen in the list, the filter cleared, where Enter presses it; Esc closes it. Elsewhere in mission control `?` still lists its own keys.

### Settings

"Settings: turn the workspace's options on and off" in `M-p` opens mission control on its Settings view: every option under its category, with its value and what it does. Enter or Space changes the chosen one (on and off, or the next of its choices), and the change takes effect at once. `/` filters by name, description, category, key or value, leaving out the categories with nothing that matches. What you change is kept in `~/.config/switchboard/settings.json`, only what differs from the defaults; a change made there by hand takes effect too, and the version before each change is kept beside it as `settings.json.bak`.

With UX 2.0 on and mission control at least 136 columns wide, Settings is a board in two panes. At the left, a panel lists the categories, each with how many options it has and, when some of them differ from their defaults, a chip with how many, then under "Your files" each file of yours with its settings. Beside it, the category in front: its name, how many options it has and how many changed, and a row per option, its name bold, a "changed" chip when it differs from its default, its key dim at the right, and a switch at the far right (green with its knob at the right when on, a kitty image where the terminal draws them, ` ━●` or `○━ ` elsewhere), or the value's words for an option of choices. The chosen option opens to its whole description, its default and what Enter does ("default on · Enter turns it back on"). `h` and `l` move to the category before or after, `j` and `k` between options, Enter and Space change the chosen one, a click on a switch flips it and a click on a category goes there. `/` searches every category, the pane listing the matches under each category's name and the panel dimming the categories with none. `e` opens the file in front in your editor in a tab of its own: `settings.json` for a category, the file itself for one of yours, `repos.json` for a repository's landing. With UX 2.0 on, the "Tabs and status bar" category is named "Tabs and header", since the status line is gone; `/` finds it by either name. Narrower, the view is the list above, with the "changed" chip on an option that differs from its default.

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

Each note written for you (a meeting's, the standup, today's todo) is in the sidebar's notifications until Notes shows it, its file is gone, or its menu in the Notifications view dismisses it. Enter on one opens it in Notes, through the running app's socket (or as the note it opens with when it isn't running), and goes to Notes in its own tab, or opens it in one (Settings' "New notes", `new_notes`). Notes beside a tab's pane goes home first, as `M-e` pressed again there takes it, so that tab keeps its work and its own note.

## How you use it

### Suggestions from how you work

Switchboard sends nothing about how you use it anywhere. With Settings' "Suggestions from how you work" (`habits`) on, it counts on this computer the keys you press (the ones pressed alone, but `C-w`), the commands you pick in `M-p` (a command with a key counting as that key pressed) and the mission control views you bring to the front, written once a minute to a file a day in `~/.local/state/switchboard/habits/` and kept 60 days. From the counts it suggests, in the sidebar's notifications:

- a key, for a command you picked from `M-p` three times in 14 days;
- turning off a view you haven't opened in 30 days (once 30 days are counted), which for the views that ask GitHub or Linear also stops what they look up;
- turning on a feature that is off, when what you did this week would use it: the second status line after you looked up a tab's PR or ticket 10 times, today's todo note after you went to Notes 10 times.

A suggestion's menu in mission control's Notifications view makes the change ("Turn it on", "Turn it off", or "Got it" for a key), puts it off for 14 days ("Not now"), or stops suggesting it ("Don't suggest it again"). Turning the setting off removes what was counted. `M-p`'s "How you use Switchboard" shows the counts themselves, and the suggestions now, in a box.

### Resources

Mission control's Resources view is how much CPU and memory each tab uses, counting every process under its panes (Claude, a dev server, an editor and its language servers, a test run), as a table: a row per tab with its % CPU (100% is one core), resident memory and number of processes, and a row for all the tabs together at the foot. Tabs are sorted by CPU, and `s` sorts them by memory instead, and back; the sorted column's name is lit. The chosen row adds its three heaviest kinds of process, which `/` filters by too. Enter offers to go to the tab, and to end each of its processes using more than 1% CPU or 100 MB, up to five, heaviest first: asked first ("End node in tab 3? [Y/n]"), and sent SIGTERM only once it is checked to have the same name and still be under the tab's panes, so a pid handed to another process is never signaled. It is read from the kernel every three seconds while the view is in front, and never otherwise. Settings' "Resources" (`view_resources`).

With UX 2.0 and a screen 136 columns wide, it is a board. At the top is the whole computer: its CPU as a share of all its cores with a sparkline of the last two minutes, its memory used of what is installed with a meter (on macOS app memory, wired and compressed, as Activity Monitor's "Memory Used" counts it), and Switchboard itself, its server and the lookups it runs outside your panes. Under a rule each tab is two lines: its number in its color and its name, its CPU, memory and processes, each red past a core or 4 GB, and two sparklines of its last two minutes at the right, CPU (to 160%, red past 100%) and memory (to 6 GB, red past 4 GB); on the line under, when it is heavy, a red ✕ chip saying why ("a core pegged", "over 4 GB"), then its heaviest processes ("node 96% · lua-language-server 12%"). The chosen row opens to its four heaviest processes as a tree, indented as they were started, each with its CPU and memory, then how many more and their names. A note at the foot says memory is resident, so memory two processes share counts in both, and 100% CPU is one core, as `top` counts it. The history starts over when the view has been away, and is drawn as text where the terminal takes no images.

### Sessions and Usage

The Sessions view lists each Claude tab's session with how full its context is, its model, how long it has run and how long since it last did anything, the fullest first, one at 80% or more marked as needing you: the ones to compact or close. Enter offers to go to the tab. Settings' "Sessions" (`view_sessions`).

The Usage view is what Claude has cost today, at API prices as Claude Code estimates it, across the interactive sessions that ran today (a headless `claude -p` isn't counted; a session resumed in a new Claude process, by the handoff, a restart or `claude --resume`, adds only what it costs from then on), and how much of each rate limit is used (the 5-hour and weekly limits, or a spend limit) with when it resets, one at 80% or more marked as needing you, and a row for each tab that spent today or this week. Settings' "Usage" (`usage`).

With UX 2.0 on and mission control at least 136 columns wide, the Usage view is a board of charts. Its heading is today's spend. Under it, the spend by quarter hour since the hour of the day's first spend (four hours of the day at least), each bar stacked by the tabs that spent in it, each in its tab's color, with the amounts at its left, the hours under it and a red line at "now"; beside it, each tab's number in its color, its name, its spend and its share of the day, and under them the chosen tab's rate since its first spend ("tab 7 since 11:00 AM: $1.21 an hour"). A tab that has closed keeps its number, name and color; spend no tab is known for is "sessions outside a tab". Then the limits, a meter each, with how much is used, when it resets and, from 15 minutes into a window, where it ends up at the pace it has filled so far ("at this pace, 58% by then"), the 5-hour meter marking that pace with a tick; then the request audit (below), with each service's requests by hour as a sparkline beside a meter of its share of the busiest service, and under the chosen one what made them (the PR board, the tab names, CI, ...). j and k move through the tabs, then the services, and the chart outlines the chosen tab's part of each bar; Enter on a tab goes to it, or says it has closed; `w` shows this week by day in place of today, and again today; `/` filters the tabs and the services; a click on a tab, a service or a bar chooses it. Narrower, or with UX 2.0 off, it is the list.

Claude Code gives the context, the model, the cost and the limits only to its statusline, which `switchboard setup` makes `switchboard statusline`. It writes them to `~/.local/state/switchboard/context/` and `~/.local/state/switchboard/usage/` as each session draws, so a session shows in these views once its statusline has drawn, and the limits are as the last request answered left them: a session that hasn't spent since it last wrote them leaves them alone, so an idle session redrawing never brings back a week the account has moved past. The statusline also appends what each draw adds to the day's spend, with the time and the session, to `usage/spent-<day>.tsv`, which places the spend in time on the board, and the workspace keeps which tab ran each session in `usage/sessions-tabs.tsv`, so a tab closed since still has its name and color there; both are kept 8 days and stay on this computer.

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

`M-v` shows the branch's pull request in a pane to the right, taking 45% of the width: its title and number, a line saying whose branch goes into which and whether it is open, a draft, merged or closed, then its description. The PR is found from the pane's branch as the tab's name finds it (in origin and, in a fork's clone, the project `upstream` names; a branch checked out from someone's fork by the branch it tracks; a `wtpr` worktree by its number), and a merged or closed one still counts. The copy last read for the branch shows at once, and the PR is read again in the background; when it has changed, the view is laid out again where you were. `M-v` again closes it. With no PR to show, the view says why: not a git repository, no `origin` on GitHub, no GitHub token, or no PR for the branch yet ("Push it and open one, then M-v again").

With [UX 2.0](#ux-20) on, the PR shows in Claude's place, laid out as GitHub lays out its page: a chip saying whether it is open, a draft, merged or closed, its number and title, a line saying who wants to merge how many commits from which branch into which and the lines added and removed, then the description and the PR's comments, each with its author's photo. Beside them, a panel lists the checks (the failing ones first, then those still running, with how many have passed), the reviewers and what each said or that they were asked, the review threads still open (who started each, on which file and line, what they said, and how many replies with the last one), and the files changed with their lines added and removed, a long list ending in "N more". The panel stays put while the description scrolls; in a pane under 100 columns wide it follows the description instead, and so does any of its sections that doesn't fit beside it in the pane's height. The footer names the keys that work from there. A photo is drawn where the view draws pictures, with Settings' `photos` on, and is the person's initial on a color of their own until then and elsewhere; one that failed to download is tried again every 15 minutes while the view stays open.

`M-p`'s "View the branch's PR description as an image" draws the description as GitHub draws it (its HTML, GitHub's stylesheet, Mermaid diagrams drawn) and shows it a page at a time as pictures in place of the pane: `j`/`k` or Space between pages, "Page 2 of 3" and "↓ more" in the corner, `q` (or the command again) giving the pane back. "View the branch's Linear ticket as an image" does the same for the ticket. Both need Chrome or Chromium, which draws the page (the view says so when neither is installed), and a terminal that shows kitty's pictures (Ghostty or kitty), or one that draws sixel, as Windows Terminal does, with Settings' "Image protocol" on `sixel` ([Pictures in Windows Terminal](#pictures-in-windows-terminal)).

Both keep what they draw on this computer (Settings' "Cache PR and ticket images", `image_cache`), so one opened again shows at once. What the page loads from the web (GitHub's stylesheet, Mermaid and the Markdown scripts, and the PR's or ticket's pictures) is fetched once and kept in `~/.cache/switchboard/assets/`, under its link without the query for GitHub's private pictures and Linear's uploads, whose links carry a new signature on every read; a pinned file is used only while its hash is the one the page pins. The pages drawn are kept in `~/.cache/switchboard/pr-image/pages/`, under the text (those signatures left out) and the pane's size, so the same PR or ticket at the same size shows from there without starting Chrome, which runs only once something needs drawing: a size not drawn before, or a description that changed when it was read again. Pages in which something failed to load are shown but not kept, and are drawn again once the PR or ticket has been read again. The two together are held to 300 MB, the least recently used going first, and pages not opened for 30 days are dropped. `M-p`'s "Clear the PR and ticket image cache" removes it all, the text last read included, after a yes. With the setting off, everything is fetched and drawn each time.

### The Linear ticket

`M-i` does the same for the branch's Linear ticket. The ticket's ID comes from the pane's branch, the first thing in it shaped like a ticket ID in any case (`abc-123-fix-login` gives ABC-123); with none there, from the tab's name (a "Shaping ABC-123: ..." tab before there is a branch, or a name you gave it). The view shows the ticket's title, the facts from its side panel (state, priority, assignee, project, cycle, estimate, labels), who opened it and when, when it was last updated and its parent, then the description, sub-issues, related tickets and links, and the comments, oldest first, each reply under the comment it answers. A bare link to another ticket reads as that ticket's ID. As with the PR, the copy last read shows at once while the ticket is read again. With no ticket in the branch or the tab's name, `M-i` says so.

With [UX 2.0](#ux-20) on, the ticket shows in Claude's place, as its page in Linear: its ID and title, who opened it and when with their photo, the description, the sub-issues each with its state's mark and how many are done, and the comment threads, each reply under the comment it answers, with each author's photo. Beside them, as the PR's, a panel holds its facts (its state as a chip, priority, assignee, project, cycle, estimate, labels and parent), the pull requests linked to it with their state where Linear keeps it, the related tickets, and its other links.

### In the pager

Every view draws its Markdown for the terminal: headings without their `#`s, tables, task lists and code blocks, a link as its text alone, an image as a line naming it, and the HTML a PR template leaves read for what it says. A click on a link, or an image's line, opens it in the browser. A line too wide for the pane is cut at its edge rather than wrapped, so a drawing keeps its shape; Left and Right scroll across. Resizing the pane lays the text out again at the new width, opened at the line that was at the top.

Mermaid diagrams are drawn as text: flowcharts (`flowchart` or `graph`, in any direction) and sequence diagrams. A left-to-right flowchart wider than the pane is drawn top to bottom instead, and one wider still is left whole, for Left and Right to scroll. Any other kind of diagram keeps its source, with a line saying where it is drawn properly (`M-r` on GitHub, `M-l` in Linear, the page in the browser).

In a terminal that shows kitty's pictures (Ghostty or kitty), with Settings' "Pictures and diagrams in the views" (`view_images`) on, a picture on a line of its own and every Mermaid diagram, of any kind, are drawn as pictures instead, inside a rounded border, as large as they are but no wider than the text and no taller than the pane. Each shows as text until it is ready, and stays text when it can't be had. A picture is fetched from its link, a GitHub attachment with your GitHub token and a Linear upload with your Linear key, each sent to that host alone, and none of GitHub's or Linear's pictures is fetched while that integration is off; a note or a file of yours (`switchboard view`) has its diagrams drawn but no picture fetched, since a link in it is your words; a diagram is drawn by Mermaid in Chrome or Chromium, as the image view draws one, and stays text without either. Both are kept in the image views' cache while `image_cache` is on, so a view opened again shows them at once. The view's pane passes on the graphics commands that draw them while it runs, and the view leaves out any control character the text holds, so a PR or ticket can't send your terminal commands of its own.

The Markdown, and a board's column (the PR and the ticket with UX 2.0, its panel staying put), has a cursor, a yellow `›` in the margin of the line it is on, with Settings' "A cursor in the views" (`view_cursor`) on. The keys below move it rather than the screen, which scrolls only to keep it in sight; it passes over blank lines, and over a picture or diagram in one step, its `›` beside every row of it. `Enter` or a click on a picture or diagram shows it full size over the view, drawn by the workspace as a note's picture is, `Esc`, `q` or `Enter` going back; the prompt line says so while the cursor is on one. Where the workspace won't draw it (outside Switchboard, or with Settings' "Images in the workspace", `server_images`, or "Enter shows a picture full size", `picture_full_size`, off), it opens in Quick Look on macOS, or in the desktop's viewer on Linux. In a terminal that doesn't draw the views' pictures, a picture stays its line of text, and a click on it opens its link in the browser. A click elsewhere moves the cursor to the line, and opens a link under it. With the setting off, and for a log or a plain text file, the keys scroll as less's do and `Enter` is a line down.

| Key | What it does |
|---|---|
| `j` `k`, `↓` `↑`, `Enter`, `e` `y`, `Ctrl-n` `Ctrl-p` | A line down or up (`Enter` on a picture or diagram shows it full size) |
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

`M-p`'s "Read a link in a pane beside this one, typed or one this pane shows" lists the links the pane shows and has scrolled past, newest first, each once; a link a program wrote into its text (an OSC 8 hyperlink, as Claude Code writes them) shows that text before its address. After them come the links in what you sent the pane's Claude that the pane doesn't show, marked "you gave it", newest first: Claude Code shows a long paste as "[Pasted text #1 +12 lines]", so a link inside one is never on the screen. The newest 50 are kept, on this computer alone, until the pane's Claude starts a new session or the pane closes, and through a restart that keeps every pane (Settings' "Links you gave Claude", `prompt_links`; off, the commands list only what the pane shows). In the notes pane the list is the links of the note it shows, top to bottom, since Notes shows a link's text alone. Type to narrow the list, or type or paste a link of your own; `M-Enter` takes what you typed even when a listed link matches it. Enter reads the link in a pane to the right, half the width, in place of any link read in that tab before, and `Ctrl-O` opens it in the browser instead. What the link is decides how it is read:

- a GitHub pull request, as `M-v` lays one out;
- a Linear ticket, as `M-i` lays one out;
- a Notion page, or a Google Doc or Slides deck, through Claude's connectors, as the connector gives it, in about 15 seconds. Settings' "Docs in the link view" (`link_view_docs`, Integrations) turns this off, and such a link is then read as any other page, which a private one won't allow;
- a note, a `file://` link that search everything gives, read only from the notes directory;
- a Slack message, which can't be read without Slack's connector, so its pane offers only to open it: `o` opens it, any other key closes;
- any other web page, fetched, its main text turned into Markdown under its title and address.

`o` in the link view opens the page in the browser, and `M-p`'s "Open the link being read beside this pane in the browser" does the same from any pane in the tab. `q` closes it. A page that can't be read says why: `o` then opens it in the browser and closes the pane, and any other key closes it.

The same list does two more things. "Open a link from this pane in the browser, or read it in a pane beside" opens the chosen link in the browser, `Ctrl-O` reading it beside instead; "Copy a link from this pane to the clipboard, or open it in the browser" copies it (to your terminal's clipboard and the computer's, as a selection is), `Ctrl-O` opening it. With no links in the pane, each says so.

"Open an image or video from this pane in its app, or show it in its folder" lists the pictures and videos the pane names, newest first: a file's path (absolute, from `~`, or from the pane's directory, listed only when the file is there; png, jpg, gif, webp, heic, svg, bmp, tiff, mp4, mov, webm, mkv, m4v and avi), then the links among the pane's whose address ends in one, those you gave its Claude included (a file's path in a prompt is not listed). Enter opens a file in the app the desktop opens it with (Preview or QuickTime on a Mac) and a link in the browser; `Ctrl-O` shows a file in Finder, or opens its folder on Linux.

Mission control's menus read a link beside your work too: "Read it here" on a pull request or ticket on the boards and in the Inbox, "Read the PR here" in the CI and Landing views, and an event's linked tickets and pull requests in the Calendar view, each in a pane beside the one you were in when mission control opened.

### The views by hand

```
switchboard view notes.md               # page a Markdown file, drawn as the views draw one
switchboard view --log ci.log           # page a CI job's log from its end
switchboard view --log +40 ci.log       # page it from line 40
switchboard view +42 src/main.rs        # page any file as plain text from line 42
switchboard pr-view [dir]               # the branch's PR description
switchboard pr-view --checks [dir]      # the PR's failing checks, as M-m shows them
switchboard ticket-view [dir | ABC-123] # a ticket, by ID or from a directory's branch
switchboard pr-image [dir]              # the branch's PR description as GitHub draws it, as images
switchboard pr-image --ticket ABC-123   # a Linear ticket the same way
switchboard link-view <url>             # what a link holds
```

Each runs in the terminal it is started in, the directory defaulting to the one you are in, and takes the pager's keys. `pr-image` has Chrome draw the page and shows it a page at a time as pictures (`j`, `k`, Space and `b` page, `g` and `G` go to the ends, `r` draws it again, `q` quits), through kitty's graphics protocol, so it needs Chrome and a terminal that draws pictures (Ghostty, kitty, or inside the workspace one that draws sixel with Settings' "Image protocol" on `sixel`); inside the workspace only a pane allowed to send pictures shows them, which is why `M-p`'s "View the branch's PR description as an image" and "View the branch's Linear ticket as an image" open it in the pane's place.

### Where links open

Every link Switchboard opens, from mission control, the views, `M-r` or `M-l`, goes to the Chrome profile `~/.config/switchboard/browser.json` gives its site, else in that file's default profile, else in your default browser. The file is yours and is never in a repository:

```json
{ "hosts": { "linear.app": "Profile 2", "slack.com": "Profile 2" }, "default": "Profile 1" }
```

A site in `hosts` matches itself and its subdomains, the longest match winning. A profile is written as the folder Chrome keeps it in, as `chrome://version` shows it ("Profile 2"), or as the name Chrome shows for it ("Work"). Settings lists `hosts` and `default` under "Browser (browser.json)", as Chrome profiles and Default Chrome profile, the default picked from Chrome's own profiles. On macOS the link opens with `open -na "Google Chrome"`; on Linux the first of `google-chrome` and Chromium that is installed is run, with the profile. Where neither is installed, the link opens in the default browser (through `xdg-open` on Linux). Settings' "Chrome profiles" (`chrome_profiles`, Integrations) turned off opens every link in the default browser.

While a link opens, a box says where it is going ("Opening github.com in Chrome · Work…", or "in your browser") until the browser comes to the front; then it closes by itself, says the browser hasn't shown it yet when it hasn't after 8 seconds, or says why it couldn't open. A link in a chosen profile can take seconds: naming a profile starts a new Chrome process, which hands the link to the Chrome already running, and macOS (and any security agent) checks that process before it runs, slowest while something else keeps it busy. A link with no profile goes to the running browser at once. Mission control says the same in its busy box, and the notes app and the PR and ticket views on their message lines. Settings' "Opening-link message" (`link_opening_message`) turned off opens links with nothing said.

`switchboard open-url <url>` opens a link the same way, for scripts and skills, and `switchboard open-url --as <site> <url>` in the profile `<site>` would get (as the AWS login opens AWS's pages in `awsapps.com`'s). Every pane's environment has `GH_BROWSER` set to `switchboard open-url`, so gh's `--web` (`gh pr view --web` and the rest) opens the same way from any pane; outside the workspace, set it yourself.

### GitHub and Linear access

Switchboard talks to GitHub and Linear over their own APIs, so nothing else has to be installed. Each finds its credential where you already keep one, the first that answers:

- **GitHub**: `GH_TOKEN`, `GITHUB_TOKEN`, the token of the `gh` command when it is installed and signed in, else the stored secret `github-token`.
- **Linear**: `LINEAR_API_KEY`, the stored secret `linear-api-key`, else `linearApiKey` in `~/.config/switchboard/linear.json`. Make a key in Linear under Settings, Security & access, Personal API keys.

A credential from `gh`, a secret or the file is kept for ten minutes and forgotten as soon as the service refuses it, so a new one is used on the next request. GitHub's is looked for once at a time, and a look that finds nothing (a `gh` signed out, or too slow on a busy computer) is made again only after a minute, the token found before still used meanwhile. To store a secret, run the command and paste it, then press `Ctrl-d`:

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

What Switchboard can only reach through Claude's connectors (a Notion page or Google Doc in the link view, search everything's Slack) it asks of Claude Code: one headless Haiku, allowed only the one connector tool it needs, keeping no session, its answer taken from the tool's own result rather than from what Haiku writes. Claude Code must be installed and signed in, with the connector connected to your Claude account. A connector that isn't connected, or whose sign-in has lapsed, fails with a message saying what to do ("Notion isn't connected in Claude: run /mcp in Claude and sign in to Notion."), shown where the feature shows (the link view, search's Slack row), and the health light in the status bar names it ("Notion not connected") for two hours after, or until the connector answers again (Settings' "Health light", `health_light`). How each connector last answered is kept in `~/.local/state/switchboard/connectors.json`. While one is down, the looks nobody is waiting on (the Slack view's look every minute, the meeting notes) don't run a Haiku to find it still isn't there: every 15 minutes they ask Claude Code whether it is connected now (`claude mcp get`, which checks that one connector with no model, in about a second), and look again once it is. A search, a link or anything else you start tries at once, so it is how to see a connector you have just connected.

## The diff

`M-d` shows the branch's changes, those not yet committed too, in place of the pane you are in, as GitHub's "Files changed" shows them: the changed files in reading order on the left, the chosen file side by side on the right. The rest of the tab stays as it is. The pane you were in waits, its program still running, and takes its place back when the diff closes: `M-d` again, `q`, or `M-w` on the diff's pane. The first time, a tip says so. Outside a git repository, `M-d` says so instead.

### What it compares

The diff runs from where the branch left its PR's base branch (the merge base) to the working tree, clean or not: the branch's commits and its changes not yet committed together, staged, unstaged and files git doesn't know yet, those listed as added. Each file with changes not yet committed is marked with a yellow ● in the list and "● uncommitted" in its bar (see "The story" for where it goes in the list). `R` reads the change again, so edits made while the diff is open come in, and the file you are on stays chosen. With Settings' "Uncommitted changes in M-d" (`diff_uncommitted`, under Diff) off, the diff is the branch's committed changes alone (`git diff <merge base> HEAD`), as GitHub's "Files changed" shows them.

The base is the PR's base branch, else `origin`'s default branch for a branch with no PR (or with GitHub off), else `main`. It is fetched from `origin` first, saying so on the diff's screen, so a branch rebased onto newer work isn't shown that work as its own; a branch still at the commit it was last compared at skips the fetch, since its merge base can't have moved. When the fetch fails, the diff compares with the base as last fetched and says so. A branch with no commits past its base and nothing uncommitted (with the setting off, no commits past its base) has nothing to show: a box says so ("Nothing to show: no commits past origin/main and no uncommitted changes") and the pane stays as it was.

`M-p`'s "Open the changes not yet committed, staged and not, in place of this pane, or close them" shows the working tree's changes instead, in the same place and with the same keys: the changes not staged under "Changes", a file git doesn't know yet among them as added, and the staged ones under "Staged changes" below, as diffview listed them, a file staged and changed again since in both. A file not staged has the index on its left and the file on its right; a staged one has HEAD on its left and the index on its right. Its story reads the staged changes and then the rest, with no commit messages, and `yl` links to HEAD for a side GitHub can't have. `M-d` stays on the branch's diff, and closes either.

### The files and their changes

Each file in the list shows its status (A added, M modified, D deleted, R renamed), its name, its folder faintly, and its lines added and removed. The chosen file's old text is on the left and its new text on the right, each line beside the line it became and a line only one side has facing a gap. Lines are matched as a person reads them, so fewer look changed than a plain diff shows, and within a changed line only the words that changed are marked, in a stronger shade of git's green or red. The text has its language's syntax colours, in your terminal's own palette (comments its bright black, strings green, functions blue, keywords magenta, types yellow), which come a moment after a file opens. A long line wraps within its side, so a paragraph of Markdown on one line reads whole. Unchanged runs more than three lines from a change are folded into a "⋯ N unchanged lines" line. An added file is shown alone in a single pane, and so is a deleted one, as it was. Each file opens with the cursor on its first changed word. The bar over the new side names the file's path from the repository's root; the old side's names one only for a renamed file, which is where it was.

Settings' "Files in reading order" (`diff_order`, under Diff) lists the files flat in the order a reviewer reads them: documentation first (Markdown, `docs/`, READMEs, changelogs), then the database's changes (migrations, SQL, schema files), then the server's side of a web app (Next.js server actions, API routes, `*.server.ts`, `server/`), then the rest of the code, then tests, then generated files. Generated files are lockfiles, migration and test snapshots, minified files, source maps, and built or generated output; the diff opens on the first file that isn't one. When no story is told, Haiku orders the server's and the rest of the code's files from the most abstract to the most concrete, from their paths and line counts alone; until it answers, or if it can't, they are A to Z. Its answer is kept, so the same diff opens in that order at once. With the setting off, the list is A to Z.

### The story

With Settings' "Diff told as a story" (`diff_story`, which needs Files in reading order on) the list is told as a story. Haiku reads the whole diff, up to about 160,000 characters (files that fit kept whole, the rest sharing what is left, every change still named), and the commits' messages, and groups the changes into two to eight chapters, each one thing the change does, from its core to its edges, tests right after the code they test, generated files last whatever it says. This sends the diff's text and the commit messages to Claude. It needs Claude Code installed; a diff of one file, or of generated files only, has no story.

The story is the commits' own, the one told ahead for the branch's commit (below): what isn't committed yet is never sent to Haiku, so editing tells no new story. The files changed only since the last commit go in a last chapter, "Not committed yet", with no why; a file changed both in a commit and since stays in its committed chapter, the last chapter's page showing it too, and its notes are left out, being for the lines as committed. When the commits have no story (one file, Claude Code not installed, Haiku failing), they go first in a "Committed" chapter, so the uncommitted one stays last; with nothing committed yet, the "Not committed yet" chapter stands alone, with no cover and nothing asked of Haiku. `R` puts what is uncommitted now in its chapter, keeping the story; after a new commit, the new commits' story is told, with no cover over the diff already open.

While Haiku reads, the diff waits under a cover, blank for its first moment so a story already known shows with no words flashing past. Then the cover shows how many files and lines it is reading, the time taken against about how long it should take, and each chapter as Haiku names it. `t` shows Haiku's answer itself as it comes, and `t` again the chapters; with Settings' "Diff story progress" (`diff_story_progress`) off, the cover shows only a spinner. `Esc` or `q` takes the cover down and shows the diff at once, in the kinds' order, with "telling the story" under the list's heading until the story comes; the list then takes the story's order and keeps the file you are on.

Each chapter heads its first file in the list with its number, title and a why of two or three sentences. The bar over the diff names the file's chapter before its path; in a list narrower than about 48 columns the heading keeps only the why's first sentence, and the whole why follows the path in that bar. A file sits in the chapter that holds most of its changed lines. With Settings' "Diff story notes on changes" (`diff_story_notes`) on, Haiku's note sits over each change worth one, saying what it does and what to check, and a change belonging to another chapter than its file's is headed with that chapter's number and title.

In the list, `]c` and `[c` go to the next chapter and the one before. `]p` opens the first chapter as a page, a diff of its own holding only the files that chapter touches, as if it were a commit, the list headed "Page 1 of N"; `]p` and `[p` turn the pages, and `[p` from the first goes back to the whole diff. On a page, the changes of other chapters in its files are dimmed. Settings' "Diff story a page at a time" (`diff_story_pages`) off makes `]p` and `[p` do nothing. With no story, `]c` in the list, `]p` and `[p` say why in a box: it is still being told, it is off in Settings, the diff changes only one file, a merge conflict is unresolved, or what went wrong when Haiku was asked. When Haiku's answer named changes the diff doesn't have, a box says the story may be thin.

`yS` copies the story as Markdown, to share: each chapter's number, title and why, its files, and each change's note under its file as `path:line`. `M-p`'s "Copy the diff's story (M-d) as Markdown, to share it" presses it, and is listed only while the diff is open.

A story is kept in Switchboard's cache (`~/.cache/switchboard/diff-story/`) under the diff's text, the commits' messages and the version of what Haiku is asked, so the same diff opens on its story at once and any change to it is a new one. Two diffs asked for the same story at once share one telling.

With Settings' "Diff story told ahead" (`diff_story_ahead`) on, when a tab's branch has a commit whose story isn't told yet, it is told in the background, so `M-d` opens on it at once; an `M-d` opened meanwhile follows that telling on its cover rather than asking again. It tells one branch at a time and each commit once, tries a failed one again after half an hour, and counts the default branch too, whose commits not yet pushed are its diff. It runs only with Files in reading order, Diff told as a story and GitHub on, and Claude Code installed, and sends each new commit's diff and messages to Haiku, a run taking up to five and a half minutes.

`M-p`'s "Clear this tab's diff cache (M-d): the story, file order and PR base kept for its branch" removes, after a yes, what the diff keeps of the tab's branch: its story, Haiku's order of its files (of its commits, and of the diff to the working tree as it is now) and the PR base kept for the worktree, so the next `M-d` asks again. It works from any pane of the tab, the diff open or not, in the directory the tab is named from, so Notes beside it clears the tab's work, and a box says how many files went. Nothing is told again until `M-d` opens. A story being told right now is kept, since its telling would keep it again, and the box says so; the base is found as `M-d` finds it, never asked of GitHub with GitHub off, and when the change can't be read (no base, a diff git won't give) the box says so and why rather than reporting the story and order cleared.

### The branch's commits one at a time

`c` goes through the branch's commits one at a time, oldest first, so the branch reads in the order it was written: each commit's own change against the commit before it, in the diff's place, the list headed with "Commit 2 of 5", its hash and its subject. `]p` goes to the next commit and `[p` to the one before; `c`, `q` or `Esc` in the list go back to the whole branch, where you were. A commit has no story of its own, and `ga` asks only about the whole branch (in a commit it says so). A working tree's changes have no commits to go through.

### Asking about the change

`ga`, in the list or the diff, asks Haiku a question about the change (Settings' "Ask Claude about the diff", `diff_ask`). A box opens in the middle of the diff to type it in: `Enter` asks, `Shift-Enter` or `Ctrl-j` starts a new line, a question pasted in keeps its line breaks, `←` `→`, `Home` `End` (`Ctrl-a` `Ctrl-e`), `Backspace` and `Delete` move and edit in it, `Ctrl-u` clears it, and `Esc`, `Ctrl-c`, Cancel or a click outside closes it. The answer streams into a window on the right half, titled with the question, each file Haiku opens named on a line of its own. Haiku is given the story, the diff as the story read it, where your cursor is (the file, the line and its change), and the question, and runs in the repository's top with only the tools to read, search and list files, so it can read the repository to answer and nothing outside it. It keeps no session: each question stands alone. This sends the diff and your question to Claude. `M-p`'s "Ask Claude about the diff (M-d), with its story, reading the repository as needed" presses `ga`, and is listed only while the diff is open.

| Key, in the answer | What it does |
|---|---|
| `j` `k`, `↓` `↑` | The line cursor down or up |
| `Ctrl-d` `Ctrl-u` | Half the window down or up |
| `gg` `G`, `Home` `End` | The first or last line |
| `gf` | The `path:line` on the cursor's line, in the diff, when the change has that file; a file of the repository the diff doesn't change opens in a pane below the diff at that line, in your `$VISUAL` or `$EDITOR` (given `+<line>`), else as plain text (`switchboard view`); a click on a `path:line` does the same |
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

Settings' "Diff file list wide" (`diff_list_wide`) opens the list wide enough for its longest path, at most half the screen; `E` narrows it, and widens it again. From the list, `J` and `K` scroll the diff without leaving it. `yp` copies the cursor's `path:line` (on the side it is in, the new one from the list), and `yl` a GitHub link to that line at that side's commit, when `origin` is on GitHub; a file marked ● has no link from its new side, the file on disk, whose lines GitHub doesn't have yet, and `yl` says so. In a side of the diff, `v` or `V` starts selecting lines from the cursor, lit as the cursor moves; `yp` and `yl` then copy the lines selected (`path:12-20`, `#L12-L20`) and end the selection, as does `Esc` or `v` again. A click on a file shows it, a click in the diff puts the cursor there (opening a fold it lands on), and the wheel scrolls the list or the diff under it, the list past the file chosen, which a key brings back in sight.

With Settings' "Easy mode" (`easy_keys`, under Help) on, the diff has a row of buttons at the bottom, each pressing its keys in the list: ◀ Chapter, Chapter ▶, ◀ Page, Page ▶, Widen, Copy link, Refresh, Ask (with Ask Claude about the diff on) and Close, followed, with "Key help line" (`key_help_line`) on, by a line naming the keys. With "Highlight on hover" (`hover_highlight`) on, the button under the mouse lights. Both take effect when the diff next opens.

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
| `R` | Read the change again, edits made since it opened included |
| `E` | Widen or narrow the list |
| `yp` | Copy the cursor's `path:line`, or the lines selected (`path:12-20`) |
| `yl` | Copy a GitHub link to the cursor's line, or to the lines selected |
| `yS` | Copy the story as Markdown |
| `ga` | Ask Haiku about the change |
| `q`, `Ctrl-c` | Close the diff |
| A number | Repeats the move that follows (`5j`) |

Under the cover, `Esc` or `q` shows the diff and `t` switches between the chapters and Haiku's answer. While the change is still being read, `q` or `Esc` closes the diff, and when it can't be read, any key does. A message box in the diff ("Copied …") closes on the next key or click, which goes on to do what it does (`Esc` only closes it), or by itself after five seconds.

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

Notes are kept in `$NOTES_DIR`, or `~/Notes`, one Markdown file each, in folders that are directories, nested as deep as you like. A note's title is its first line, and its file is named for it and renamed when the title changes. A title emptied or deleted, in Notes or by a write from outside, is put back as the note saves (a title being typed over is left until you leave Insert mode), so a note is never written without its title nor renamed after its first item; a title retyped, as a heading or as plain words, is the new title. A note saves itself a moment after each change and whenever you leave it. Pinned notes and the note open last are listed in `.notes.json` there, and deleted notes go to `.trash/`, the Recently Deleted folder. Nothing else is stored, so any editor can open them.

Each note's last line is its ID, `<!-- note-id: n-… -->`, which the app never shows, keeps last and puts back if it goes; it gives a note one when it makes it or first opens it, without changing when the note was last changed, and a copy of a note's file, which carries the original's, one of its own, saying so once (of two notes holding one ID, the one changed later is the copy). The ID survives a rename or move anywhere (`mv`, Finder, git, a Claude editing the note), so what keeps a note by it, `wtn`'s worktrees and autopilot, still finds it; `wtn` and autopilot give a note one too when it has none, and another editor shows it as an HTML comment. Titles, previews and searches leave it out.

One app runs on a notes directory at a time. While it runs it holds `.notes.lock` there, with its process ID: notes save themselves as you type, so a second app on the same notes would write over the first's. A second one says the notes are open already and closes with `q`. A lock left by an app that has gone is taken over.

A note changed from outside the app, by a skill, a script or another editor, shows the change while it is open, without a save or a keypress: a watch on the notes directory brings it in. If you have typed something not yet saved, the two are merged line by line, so changes to different lines, even next to each other, both stay; where both changed the same line, both versions are kept, yours first, and the app says so. An item ticked from outside moves under the ones left to do, as ticking it in the app does. A note deleted or moved outside the app while open isn't written back under its old name.

Nothing in a note is sent anywhere. Squashing notes, after you say yes, is the one exception in the app, and it sends no note's ID. The question of which note is a tab's ranks the notes on the computer, and a note's new tab tells Claude only the note's title and file. The `/note-tickets` and `/note-watch` skills read notes only when you start them, and only the notes you name or confirm.

### In the list

| Key | Does |
| --- | --- |
| Enter, or a click | open a note, or show a folder's or a tag's notes |
| `j` `k`, the arrows, `gg` `G`, Ctrl-D Ctrl-U | move, half the list at a time with Ctrl-D and Ctrl-U |
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

**Squashing.** Mark two or more notes with `s`, then `S` merges them into one. After you say yes, a headless Claude (Sonnet, with no tools and no session kept) merges them, keeping every item, box and link, the note changed last winning where two disagree and nothing says which is right. A box shows it at work with the seconds so far; Esc in it cancels, leaving the notes as they were. The new note goes in the first one's folder, with an ID of its own, the originals go to Recently Deleted, and every tab linked to one of them links to the new one; autopilot, on one of them, its skipped items and its tabs take the new note's ID in place of theirs. It needs Claude Code.

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
| Work | `gw` on an unticked checklist item at the top of its list starts work on it in a new tab, as `M-I` does (see [Starting work from the workspace](#starting-work-from-the-workspace)); with UX 2.0, Enter opens an item's plan, `o` goes to its tab, `ga` adds an item and `M-p` has autopilot work the note (see [UX 2.0](#ux-20)) |
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

The menu also has, under their own keys there, the checklist's Tick or untick (`c`), Move checked to bottom (`s`), Uncheck all (`u`), Delete checked items (`D`) and Start work on the item in a new tab (`w`, as `gw`); Paste image (`v`); and a table's Add row (`r`), Add column (`C`), Delete row (`R`) and Delete column (`X`).

A line style applied again turns the line back to body text, as in Notes. A character style works as it does in Notes too: on the selection; off again, for the whole run, in text that has it already; on the word under the cursor; and anywhere else (a space, an empty line, right after a word you typed) on what you type next, starting Insert mode between its markers, which go again if you type nothing. Underline and highlight, which Markdown has no syntax for, are written the way Markdown tools usually write them.

- **Checklists.** Click a ☐, or press Enter on its line, to tick it. A ticked item is dimmed and struck through with everything indented under it, and moves under the ones left to do, with its details and sub-items, a blank line between, so a list leads with what is left; sub-items sort the same way under their item.
- **Lists.** Enter in Insert mode carries a list on (a checklist item comes unticked, a number counts up) and ends it on an empty item; Tab and Shift-Tab indent and outdent an item.
- **Tables.** In Insert mode, Tab moves to the next cell, past the last one adding a row, and Shift-Tab back. A table lines up again whenever you leave Insert mode or change it, measured as it shows, so bold markup doesn't throw a column out.
- **Collapsible sections.** Enter on a title, heading or subheading, or a click on its ▾, folds what is under it, up to the next heading of the same or a higher level; a folded one shows ▸ and how many lines it hides. Vim's `z` commands work too.
- **Links.** Enter, or a click, opens a web link in the browser (in its site's Chrome profile, as every link Switchboard opens), a `[[Note title]]` link in the note with that title (made if there is none; drawn as missing until then), and a `#tag` in the list. Typing `[[` offers every note's title.
- **Tags.** Any `#word` after a space, not in code, is a tag.
- **Line numbers.** The note's lines are numbered relative to the cursor, the cursor's line showing its own, for jumps like `5j`.

**Pictures.** Ctrl-V while typing pastes the image on the clipboard into the note, as in Claude Code (Settings' "Ctrl-V pastes an image", `notes_ctrl_v_image`), and so does the Format menu's Paste image; with no image there, Ctrl-V does what it always does (the terminal's own paste key pastes only text). The image is saved beside the note in `.attachments/<note>/` in its folder, which the list leaves out, and written as `![name](path)` on a line of its own below the cursor's, indented under the item when the cursor is on a list or checklist item: the image goes with the item, and Claude, working the note's items, reads it with them. On Linux, pasting an image needs `wl-paste` (Wayland) or `xclip` (X); under WSL it takes the picture on Windows' clipboard (a screenshot, an image copied in a browser, or a picture file copied in Explorer, read from the file) through Windows PowerShell. Windows Terminal keeps Ctrl+V as its own paste, which pastes text (a copied file's path), so there use the Format menu's Paste image. The picture is drawn in the note under its line (Settings' "Images drawn in a note", `notes_images`) in a terminal that speaks the kitty graphics protocol with Unicode placeholders, Ghostty or kitty, or in one that draws sixel, Windows Terminal among them, with Settings' "Image protocol" on `sixel` ([Pictures in Windows Terminal](#pictures-in-windows-terminal)); elsewhere it shows as its name. The multiplexer places the picture like text, so it scrolls with the note and a box over it covers it, and passes the picture's graphics commands to your terminal (or, with sixel, reads them and draws the picture itself) from the notes app's pane alone, since one can have the terminal, or the workspace, read any of your files. A picture other than a PNG is made one in Switchboard's cache first. Enter or a click on it shows it full size over the notes pane, drawn by the workspace with kitty graphics, Esc, q or Enter going back (Settings' "Enter shows a picture full size", `picture_full_size`); where the terminal doesn't draw the workspace's images ("Images in the workspace", `server_images`), or with the setting off, it opens in Quick Look on macOS, or in the desktop's viewer on Linux.

**The palette.** Ctrl-Shift-P, in a note or the list, lists every command Notes has, each with its key, the ones picked last first: the list's (on the note open, from a note), the Format menu's, folding, and Switchboard's for Notes: moving Notes beside your work (`M-e`), the note's tab, starting work on the checklist item under the cursor in a new tab (`gw`), with UX 2.0 the item keys (its plan, its tab, Add an item, autopilot working the note), choosing a tab's note, reading a link from the note beside it, searching notes, Linear, GitHub and Slack (`M-F`), and "Shape the note's checklist items as Linear tickets", which opens a tab running `/shape` for each unticked item. Settings' "Notes command palette" (`notes_palette`) turns it off.

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

`M-N` (Alt-Shift-N) goes to Notes wherever it is open, or opens it in a tab of its own, named Notes. `M-e` brings that same Notes into a pane to the right of the one you are in, or starts it there; pressed again in that tab, or `M-w` on Notes there, it goes back to a tab of its own rather than closing. While it is away, its own tab shows which tab has it ("Notes is beside tab 3"), so the tab stays in the sidebar: Enter there goes to that tab, and `M-e` brings Notes back. When the last of a tab's own panes closes, Notes goes back to its own tab rather than stay there alone. A note opened from a notification, or from anywhere but `M-e`, opens in Notes' own tab, Notes going home first when it was beside a tab. While Notes is beside a tab's pane, that tab says "✎ Notes here" in the sidebar and on its mission control card.

Beside another tab's pane, Notes' list is put away, so the note has the room (Ctrl-F still finds a note), and it comes back in Notes' own tab (Settings' "Hide the sidebar beside a tab", `notes_hide_sidebar`).

**A tab's note.** Beside a tab, Notes shows that tab's own note: the one its Claude works from (`/note-watch` and `/queue-and-clean` record it once they have settled on it), else the last one opened beside that tab (a note Notes only carried over from the tab before isn't taken as this one's). A tab with no note of its own is asked which it is, the likeliest first, so Enter takes it: notes that mention its ticket or PR number, then those whose titles share words with the tab's name, then the note last shown beside it, then those naming its repository, then the rest, the most recently changed first within each. "Keep whatever is open", last, stops it asking for that tab, and Esc leaves the question for next time. M-p's "Choose this tab's note, the one M-e shows beside it" asks again any time, and Settings' "Ask for a tab's note" (`ask_tab_note`) turns the question off. Each Claude session that starts in a tab with a note of its own is told the note's path and to read it before its first reply (by the plugin's `switchboard claude-context`, below, so only while Switchboard loads as a plugin), so a standing rule kept in the note reaches every session in the tab, whatever repository it works in; Settings' "Point Claude at the tab's note" (`tab_note_context`) turns that off.

**A note's tab.** `t` in the list, or M-p's "Open the tab for the note in Notes" (for the note Notes shows), goes to the tab linked to the note. With none, it asks which tab is the note's: one of the open tabs, linked to the note then, or a new one, in a directory you pick as `M-t`'s does, linked to the note and named after it. The new tab's Claude starts knowing which note it is for: its title and file are added to its system prompt, so no turn is spent on it, and the note itself is read when you ask, or before the first reply while "Point Claude at the tab's note" is on (Settings' "Tell a note's new tab its note", `note_tab_prompt`). Either way Notes moves beside the tab's Claude showing the note, as `M-e` puts it there, the cursor left in Claude's pane (Settings' "Notes beside a note's tab", `note_tab_beside`).

These need the workspace: in a plain terminal `switchboard notes` runs alone, and the palette's workspace commands say so.

The workspace reaches the running app through a socket, `~/.local/state/switchboard/notes.sock` (or `$NOTES_SOCKET`), which you can use from a shell too:

```
switchboard notes open <note>       show a note, by its path in the notes directory
                                    (with no app running, the note it opens with)
switchboard notes sidebar on|off    show or put away the list
switchboard notes shown             the note shown, as its path
```

## Setting Claude Code up

Switchboard learns what each Claude Code session is doing from Claude Code's own hooks, and gives it a statusline, its own instructions (how to read the tabs, the notes app) and a set of skills. `switchboard setup` puts them in place. The install script runs it once the binary is in place, `switchboard update` runs it after installing a release, and `M-p`'s "Run Switchboard's setup" runs it in a tab of its own, to put in place what isn't or to answer a question you said no to.

```
switchboard setup                  the guided setup
switchboard setup --check          say what would change, and change nothing
switchboard setup claude skills    only those parts, asking nothing
switchboard setup --parts          list the parts
switchboard setup --refresh        bring the parts already set up up to date, quietly
switchboard setup --move           move Switchboard into its plugin or back into ~/.claude, as the setting says
```

### As a plugin, or in ~/.claude

Switchboard's hooks, instructions and skills are a Claude Code plugin, `~/.local/share/switchboard/plugin`, which only the Claude in Switchboard's panes loads: every pane's environment names it in `CLAUDE_CODE_PLUGIN_DIRS`, which Claude Code 2.1.280 and later read, so Claude started anywhere else has your own hooks, rules and skills and none of Switchboard's. The plugin holds every skill below, a `hooks/hooks.json` with the hooks under [Claude Code's settings](#claude-codes-settings), and a hook as each session starts, resumes, clears or compacts that runs `switchboard claude-context`, which gives an interactive session Switchboard's instructions and the path of its tab's note to read (and a headless `claude -p` nothing, by Claude Code's `CLAUDE_CODE_ENTRYPOINT` and the session's file). The server writes the plugin as it starts (but one with a state directory of its own, `SWITCHBOARD_STATE_DIR`, which shares your home) and `switchboard setup` in every mode, each file only when it differs, so a new build's arrives with its first start. Switchboard's skills are then `/switchboard:<name>`, and `/<name>` too while no skill of yours or the project's has that name. A plugin can't carry a statusline or permission rules, so those are still merged into your `~/.claude/settings.json`. The headless `claude -p` runs Switchboard starts for itself from a pane (the diff's story and questions, the connectors, Notes' squash, today's todo note, the Slack relay's summaries, `codex-review`'s Claude reviewer) leave the plugin out of the variable, keeping any other directories you set, so its hooks don't speak for the tab.

Settings' "Load Switchboard as a plugin" (`claude_plugin`) is on for a new install. Off, setup puts Switchboard in `~/.claude` as earlier versions did, for every Claude of yours: the hooks in your settings, an import of its instructions in your CLAUDE.md, and its skills in `~/.claude/skills`, and the panes name no plugin, which would run the hooks twice. Changing the setting in Settings (or with `switchboard setting claude_plugin on`) runs `switchboard setup --move`, which moves them: into the plugin, it takes out of `~/.claude` exactly what Switchboard put there, each part only when there is something of it to take out, and back, it puts all three in. A pane started after that follows the setting; a Claude already running keeps what it loaded. A tab already open keeps the environment its shell started with, so a `claude` started again there has none of Switchboard's hooks, skills or instructions after a move into the plugin, and runs every hook twice after a move back, until the tab is reopened or the workspace restarts (`switchboard kill-server`, then `switchboard`); what setup says after a move ends by saying so. Editing the setting by hand moves nothing until setup runs.

An install an earlier version made has Switchboard's hooks, import or skills in `~/.claude` and no choice in the setting. The guided setup asks it once, "Load Switchboard as a plugin in its panes?": yes moves it over, and no keeps it there, recording the setting off. The first run of a new build (below) never moves anything by itself: it turns the setting off for such an install, so the panes don't load the plugin on top of the hooks still in your settings, and notes that you haven't been asked (`~/.local/state/switchboard/claude-plugin-unasked`), so the guided setup still asks, once Claude Code can load the plugin; your answer there, or changing the setting yourself, takes the note away. Claude Code older than 2.1.280 can't load the plugin, so with one setup keeps Switchboard in `~/.claude` whatever the setting says, turns the setting off and says why; once Claude Code is updated (`claude update`), turn it on in Settings. It does the same where Claude Code's managed settings, which an organization puts on the computer (`/etc/claude-code/managed-settings.json` on Linux, `/Library/Application Support/ClaudeCode/managed-settings.json` on a Mac), set `disableSideloadFlags`: with that, Claude refuses to start when its environment names a plugin directory, as every pane's would; if the policy is lifted, turn the setting on in Settings. Mission control's suggestions may offer it too, once you use the Claude tabs a lot.

As a plugin, a Claude session outside Switchboard runs none of Switchboard's hooks, so the Slack relay hears only of sessions in the workspace, not of one in a terminal of its own; with the setting off, every session's hooks tell it, as before.

### The guided setup

Alone, it is the guided setup. When the tmux version of Switchboard left anything behind (below), it offers to tidy it first. It checks that `switchboard` is on your PATH, since Claude Code's hooks and statusline run it by name, and that git is there for the worktree commands, the diff and the tabs' branches, writes the plugin, which is Switchboard's own and never asked about, and checks that Claude Code is installed; without Claude Code it says how to install it and goes on to the terminal. With it, it says whether Claude Code is too old for the plugin or its managed settings keep one from loading, asks an earlier install whether to load Switchboard as a plugin, and takes each part of that way in turn. The first time, it asks before the part changes anything of yours (`[Y/n]`: Enter is yes, and a closed input is no, so nothing piped or unattended says yes); once a part is set up, it brings it up to date without asking, and taking out what an earlier version put in `~/.claude` isn't asked about either. After Claude Code's parts it looks at the terminal (Option as Alt, below) and then says which optional parts are there, each for what it names, with how to set up the ones that aren't: the GitHub sign-in (`M-v`, the PR views, PR numbers in tab names), the Linear key (`M-i`, the Tickets and Inbox views, ticket titles) and the Linear team `/shape` files tickets in (`linearTeam` in `linear.json`), your calendar's address (the Calendar view), the Slack relay's tokens and owner (`M-S`), Chrome (the PR and ticket views as images, `/walkthrough`'s videos), ffmpeg and Node (`/walkthrough`, `/deploy-test`, `/preview-test`), git-delta, Node and Chrome (`/diff-screenshots`), and Codex (`/review-loop`'s reviewer; Claude reviews without it). GitHub and Linear are left out while Settings turns them off. It ends with the optional parts not there named on one line, then what is left to do, every ✗ again, or "Ready." Running it again changes only what isn't in place yet, and each part says what it changed or that it was up to date. With parts named, it does only those, asking nothing, the way the setting says.

| Part | What it does |
|---|---|
| `plugin` | Writes Switchboard's plugin, which the Claude in its panes loads, to `~/.local/share/switchboard/plugin` |
| `claude` | Adds Switchboard's statusline and permissions to `~/.claude/settings.json`, and its hooks with the plugin off; with it on, takes out the hooks an earlier version added |
| `claude-md` | Writes `~/.claude/CLAUDE.md` to import your rules, and Switchboard's part with the plugin off; with it on, takes out the import an earlier version added |
| `skills` | Writes Switchboard's skills into `~/.claude/skills` with the plugin off; with it on, takes out the ones an earlier version wrote. Either way, links `skill-path` and `codex-review` into `~/.local/bin` |
| `terminal` | Makes Option act as Alt in a Mac's terminal: Ghostty's config, Terminal's or iTerm2's default profile |
| `leftovers` | Removes the tmux version's links that lead nowhere, putting back from its clone's history the files Claude Code still uses and the configs setup.sh linked, takes out its worktree lines in `~/.zshrc`, and takes out a `switchboard` function or alias that starts tmux |

### Option as Alt, and the tmux version's leftovers

Every `M-` key needs a Mac's terminal to send Option as Alt ([Option as Alt on a Mac](#option-as-alt-on-a-mac)). The `terminal` part sets it where it can: a Ghostty config of yours (`~/.config/ghostty/config`, or the one in `~/Library/Application Support/com.mitchellh.ghostty/`) gets `macos-option-as-alt = true` unless it already sets `macos-option-as-alt`, and one is made when you run it in Ghostty with none; and when it runs in Terminal or iTerm2, that app's default profile is changed through macOS's own preferences (`defaults export` and `import`): "Use Option as Meta key" in Terminal, both Option keys sending Esc+ in iTerm2. Open a new window, or the app again, for it to take. The guided setup asks first, and for another terminal it says the line to add to its config. On Linux a terminal sends Alt as it is, so the part changes nothing there; under xterm the guided setup says to add `XTerm*metaSendsEscape: true` to `~/.Xresources`, and otherwise reminds you that a key your desktop takes for itself can be moved in `keys.json`.

A clone of the tmux version pulled past the rewrite leaves links into files it no longer has: the scripts setup.sh linked into `~/.local/bin` and `~/.config/tmux`, and `~/.claude/commands`, `~/.claude/hooks/*`, `~/.claude/statusline-command.sh` and `~/.claude/otel-headers.sh`. Its `~/.zshrc` lines, `eval "$(worktree-from-ticket init zsh)"` and the rest, are no longer needed, and fail on every new shell once its scripts are gone. The `leftovers` part removes each such link that leads nowhere into a Switchboard clone (a link that still leads somewhere, or anywhere else, is left alone). One Claude Code still uses, a link `~/.claude/settings.json` names (a hook's command, `otelHeadersHelper`; not the statusline, which the `claude` part moves onto `switchboard statusline`) or `~/.claude/commands`, which Claude reads by itself, is replaced with a copy of what it led to from the clone's git history instead, and the part says so; one the history doesn't have is left alone, and the part says that too. The configs setup.sh linked into the clone, which the rewrite removed (`~/.config/ghostty/config`, `~/.config/karabiner/karabiner.json`, `~/.pgclirc`, lazygit's `config.yml`, the `~/.config/nvim` folder), and any other link leading nowhere into a clone directly in `~`, in `~/.config` or `~/Library/Application Support` or a folder in either, or directly in `~/.claude`, are each replaced with what they last held in the clone's history, a folder as it last was, and the part names each; Ghostty's `command`, which attached to tmux, starts the `switchboard` that ran setup instead, named by its full path. A link into the clone's older name, `~/editor-configs`, is looked up in the history of the `switchboard` clone beside it, and removed when that history doesn't have it. It takes each of those lines out of `~/.zshrc` or `~/.bashrc`, with the comment right above it that names the function it defined, since every tab, menu and skill that runs `wt`, `wtn` and the rest defines the function itself; a line through the binary (`eval "$(switchboard worktree-from-ticket init zsh)"`) is yours, and left alone. The tmux version's `tmux_server` setting ran it on a server of its own, opened with `tmux -L switchboard new-session -A -s main`; a `switchboard` function or alias on one line that runs tmux, as that command was wrapped, would run in the binary's place in every new shell, so the part takes it out, with a comment right above it that names Switchboard and tmux (one that runs anything else is yours, and left alone). The file is changed through a link to it, with a copy as it was left beside it as `<name>.bak.<time>`. The guided setup offers it only when there is something to tidy.

### Claude Code's settings

The `claude` part merges Switchboard's settings into your own `~/.claude/settings.json`, which stays a file of yours, never a link: Claude Code writes `/model` and "always allow" there, and reads no settings file after it. What is already there is kept, and a copy of the file as it was is left beside it as `settings.json.bak.<time>`. A file that isn't a JSON object is left alone, and the part says so.

- **Hooks**: one for each Claude Code event the workspace follows (`SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `PermissionRequest`, `Notification`, `Stop`, `PreCompact`, `PostCompact` and `SessionEnd`), each running `switchboard hook <Event>`. They are how a tab knows its Claude is working, waiting on you or done; without them every tab shows idle and `M-a` has nothing to go to. As a plugin they are the plugin's, and the part takes out of your file any hook running one of their commands, or one the tmux version of Switchboard added, keeping your other hooks; with the plugin off it adds them, replacing a hook of yours running the same command and the tmux version's.
- **Statusline**: `switchboard statusline`, which shows the model in the project's accent color, the directory and branch when the session runs outside the workspace, and how full the context is, inside the workspace as outside it. With Settings' "Context in Claude's status line" (`statusline_context`) off, UX 2.0 on and mission control's Sessions view on, only that view shows the context inside the workspace. It also keeps each session's context use and spend for mission control's Sessions and Usage views. A statusline of your own is kept, `~/.claude/statusline-command.sh` included, the script Claude Code's `/statusline` writes; only that name as the tmux version's link into its clone gives way to Switchboard's. Keeping yours leaves the Sessions and Usage views without what the statusline records.
- **Permissions**, added to yours: what Switchboard's skills and loops need to run unattended (the `switchboard` commands they call, their scripts in the plugin's skills, read-only `git` and `gh`, and the read-only tools of the Slack, Calendar and Linear connectors), and two denials, reading `.env` files and `git pull`. Anything that writes outside the computer is left to ask.

Once all of it is there, running the part again changes nothing. A permission or hook Switchboard stops shipping isn't taken out of your file.

### Your CLAUDE.md

Switchboard's part of Claude's instructions is what every user's Claude needs to know about the workspace (the notes app and which note is the tab's own, `switchboard tabs` for what the other tabs are doing, how to talk to another tab and name one, a repository's own skills). As a plugin, the plugin's start-of-session hook gives it to the Claude in Switchboard's panes, and your CLAUDE.md doesn't import it. With the plugin off, the `claude-md` part writes it to `~/.local/share/switchboard/switchboard.md` and your `~/.claude/CLAUDE.md` imports it with a line of its own, which Claude Code follows (`@path`). Either way a new version takes effect with the next session.

Your own rules come from where `~/.config/switchboard/claude.json` says, a git repository or a file, which Settings' "Your CLAUDE.md" row writes for you (and then runs this part):

```json
{"repo": "https://github.com/you/claude-rules", "file": "CLAUDE.md"}
```

```json
{"path": "~/somewhere/CLAUDE.md"}
```

A `repo` is cloned into `~/.local/share/switchboard/claude-md` and pulled, fast forward only, each time the part runs. On GitHub or a gist, gh's sign-in is added as the clone's credential helper when gh is signed in, so a private repository works. Without `file`, your rules are the repository's `CLAUDE.md` at its root, else the one nested least deep; none, or several at that depth, is an error that says so. Changing `repo` moves the old clone aside. With neither `repo` nor `path`, `~/.config/switchboard/CLAUDE.md` is used when there is one.

With rules named, `~/.claude/CLAUDE.md` becomes a short file written by setup that imports your rules (and then, with the plugin off, Switchboard's part); edit your rules where they live, not there. A CLAUDE.md of your own in its way is moved aside to `CLAUDE.md.bak.<time>` first, but never by the move that follows a change of "Load Switchboard as a plugin", which asks nothing: that only adds Switchboard's import line to a CLAUDE.md of yours, or takes it out, and leaves a link of yours as it is. A rules file named that isn't there stops the part, naming the path. With no rules named, your own `~/.claude/CLAUDE.md` stays as it is: with the plugin off it gets one line added, `@~/.local/share/switchboard/switchboard.md`, and with none, one is made holding that line. A `~/.claude/CLAUDE.md` that linked into a clone of the tmux version has its rules copied to `~/.config/switchboard/CLAUDE.md`.

Moving into the plugin, the part takes the import line out of your CLAUDE.md, with the blank line setup put before it when it added it to the end of yours, and nothing else; a CLAUDE.md setup made to hold only that line would be left empty, so it is removed, as it was before setup made it. Then `~/.local/share/switchboard/switchboard.md` goes too, unless your CLAUDE.md is a link to a file that still imports it.

### Skills

As a plugin, the skills are the plugin's `skills/<name>`, which only the Claude in Switchboard's panes loads, as `/switchboard:<name>`, and `/<name>` while no skill of yours or the project's has that name. The `skills` part takes out of `~/.claude/skills` each skill an earlier version wrote there (in a folder holding `.switchboard-skill`, the files that list names, the list and the packages the skill installed beside them, such as `node_modules`; or a link into a Switchboard clone), so it doesn't take the bare name everywhere. A folder still holding a file you added there is left with it, which the part says: without its `SKILL.md` it is no skill, and with the plugin off Switchboard's is written into it again beside your files. The part keeps a skill of your own by the same name; the guided setup lists those, since they take the bare name over Switchboard's.

With the plugin off, the `skills` part writes each skill the binary carries into `~/.claude/skills/<name>`, where Claude Code finds a personal skill. A later run brings each one up to date, removes a file a newer version no longer ships, and leaves alone what a skill made for itself, such as `node_modules` beside its scripts. Each folder Switchboard wrote holds `.switchboard-skill`, the list of the files it wrote. A folder without that list, or a link to somewhere else, is a skill of your own by the same name: it is kept, and the guided setup lists the ones it kept. An edit to a skill Switchboard wrote is written over on the next run, so to change one, delete its `.switchboard-skill` and it is yours. Once any skill is installed, the guided setup writes a new one without asking.

Either way, the skills' scripts and the files they share are run from the plugin, which is always there, and the part links `skill-path` and `codex-review` in `~/.local/bin` to the binary, which runs the command a link is named after, so a repository's own copy of a skill written for the tmux version, which runs them by those names, still finds them; a file of your own by either name is left alone, and a link leading nowhere (into a clone of the tmux version) is replaced.

A repository may have its own skill by one of these names, and Claude Code may run Switchboard's over it (a personal skill over a project's, the plugin's when it is named), so each of Switchboard's starts by running `switchboard skill-path <name>` and follows the repository's copy when there is one (`.claude/skills/<name>/SKILL.md` in the worktree, else in its main clone). A skill that hands off to another reads the file `skill-path` prints in the same way. Without a repository's copy, `skill-path` prints Switchboard's own, never a skill of yours that only shares its name, so `/switchboard:<name>` and a hand-off reach Switchboard's while yours keeps the bare name; a copy of Switchboard's you made yours, which still opens by running `skill-path`, counts as Switchboard's.

The ones marked "you" run only when you type them; Claude may also pick up the others on its own when what you ask fits.

| Skill | What it does | Started by |
|---|---|---|
| `/ci-loop` | Watches the branch's PR checks in the background and fixes what fails, pushing each fix, until every check passes; stops and asks when a fix isn't the branch's to make | you |
| `/clean-worktree` | Removes a worktree and its local branch once nothing in them would be lost (no uncommitted changes, every commit on GitHub), then prunes deleted remote branches | you |
| `/deploy-test` | Waits for a merged PR to deploy, tests its feature on the deployed site in a real browser, records a captioned video of the test and asks you to approve it | you |
| `/diff-screenshots` | Renders a branch's or PR's diff with git delta as side-by-side PNG screenshots, one set per file, core code first and tests last, and opens them (in Preview on a Mac); it needs git-delta, Node and Chrome | you or Claude |
| `/note-tickets` | Reads the open checklist items in the notes you name, checks Linear for tickets that already cover each, and suggests tickets for the rest, creating only the ones you pick | you |
| `/note-watch` | Watches one note for new checklist items while the session runs, and works each as it appears, ticking it when done | you |
| `/plan-item` | Adds a checklist item to the tab's note, its first line a title under 90 characters, `BUG: ` and what is broken or `FEAT: ` and what to build, and writes its plan note, which holds the detail, in the plans folder beside it, after checking the item isn't there already and reading the code it touches, in the shape `/wtn` and autopilot build from; Notes' M-Enter in "Add an item" starts it in a new tab | you, or Claude when you ask for an item |
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

No program can change the directory of the shell that started it, so the commands alone make the worktree and start Claude in it but leave your shell where it was. A function runs its command, `cd`s into the worktree the command names, and starts Claude there itself, so your shell sits in the worktree for the whole session and after Claude exits, and a pane split from it opens there too. Nothing needs them in your shell: the workspace's keys, mission control's menus, autopilot, `/wt` and `/wtn` each define the function in the tab they open. Only to type them at a prompt yourself, add them to `~/.zshrc`:

```sh
eval "$(switchboard worktree-from-ticket init zsh)"
eval "$(switchboard worktree-from-note init zsh)"
eval "$(switchboard worktree-from-branch init zsh)"
eval "$(switchboard worktree-from-pr init zsh)"
eval "$(switchboard worktree-resume init zsh)"
```

For bash, put the same lines with `init bash` in `~/.bashrc`; zsh and bash are the shells `init` writes for. `worktree-from-ticket`'s `init` also defines `claude`, which in one of Switchboard's panes starts Claude with Switchboard's plugin as Settings' "Load Switchboard as a plugin" says now (`switchboard claude-plugin-dirs`), so a Claude typed in a shell opened before that setting changed loads it, or doesn't, as a new tab would; outside the workspace it is plain `claude`. Setup doesn't add these lines.

### Where the worktrees go

Each worktree goes in its repository's `.claude/worktrees/`, under the main clone, so one made from inside another worktree goes beside it rather than inside it. Add `.claude/worktrees/` to the repository's `.gitignore`. A different parent directory can be given first, relative to the main clone (`wt .worktrees ABC-123`). The main clone's `.claude/settings.local.json`, which git doesn't copy since it is ignored, is linked into each new worktree, so your local permissions apply there too; a worktree with its own is left alone.

Run again on the same work, a command reopens it: the worktree is reused if it is there, and the branch if it is there. `wt` and `wtn` refuse while Claude is already running in the worktree, naming that session, its title and the tab it runs in, rather than start a second one editing the same files and pushing the same branch. `wtb` and `wtpr` stop when the branch is checked out in your main clone, since git allows a branch in one worktree at a time, and use the worktree that already has it when there is one.

`wt`, `wtpr`, `worktree-done`, `wt-jump`, `codex-review`, `wait-ci` and `wait-deploy` are programs you run, so they ask GitHub and Linear whatever the `github` and `linear` settings say, as the tmux version's scripts did; the settings turn off what the workspace does by itself (its lookups, views and keys). `wt` and `worktree-done` read Linear with the key the rest of Switchboard uses: `LINEAR_API_KEY`, else the stored secret `linear-api-key`, else `linearApiKey` in `~/.config/switchboard/linear.json`. Make a personal API key in Linear (Settings, Security & access) and store it with `switchboard platform secret-set linear-api-key`, which reads it from stdin and keeps it in the Keychain on a Mac, or the desktop's keyring on Linux (with no keyring, in `~/.config/switchboard/secrets.json`, which only you can read). The same `linear.json` holds `linearTeam` and `linearLabels`, the team and labels `/shape` files new tickets with. `wtpr` reads GitHub with your token: `GH_TOKEN` or `GITHUB_TOKEN`, `gh auth token` when gh is signed in, else the stored secret `github-token`.

### Starting work from the workspace

| Key | What it does |
|---|---|
| `M-t`, `M-n` | New tab, running Claude, in a directory you pick |
| `M-W` | Start work on tickets in new tabs (`wt`) |
| `M-B` | Start work on a branch, new or existing, in a new tab (`wtb`) |
| `M-p`, "Start work on a PR in a new tab (wtpr)" | A PR, in a new tab running `wtpr` |
| `M-I` | A note's items, a tab each running `wtn` |
| `gw` in Notes, on a checklist item | That item, in a new tab running `wtn` |
| `M-p`, "Autopilot: work a note" | A note's items, a few tabs at a time, the next opened as each lands (below) |
| `M-p`, "Continue shaping Linear tickets, each in a new tab" | Tickets, a tab each running `shape --continue` |
| `M-p`, "Review a contributor's PR in a new tab (/review-pr)" | A fork's open PR, checked out untrusted, in a new tab about its worktree whose Claude runs `/review-pr` from the main clone; listed only in a repository with that skill |
| `M-o` | Reopen a worktree with no tab and resume its Claude |
| `M-j` | Search and jump to a tab |

Each of the first six asks which directory first, in a list: the directories picked in any of them before, the most recent first (the last 50, kept in `~/.local/state/switchboard/work-recent.json`), then the one you are in, so with nothing picked yet Enter takes it, then every open tab's, then those Claude sessions ran in, most recent first, then your home, with worktrees left out but the one you are in. For the keys that make a worktree, each entry is its repository's main clone, listed once, and a directory in no repository is left out. Typing narrows the list; a path typed in is taken when nothing matches it, or at once with Alt-Enter (`~` is your home). Then:

- `M-t` opens the tab there running Claude, from an interactive shell of yours, which is left in the tab when Claude exits. Then a menu asks what to call it, which you may skip with Esc: `n` a name typed in, `t` its ticket and the ticket's title, or `s` "Shaping: " and a ticket, the form `/shape` gives a tab. Each ticket is the one the branch names, else asked for; `t` on a ticket's own branch leaves the tab to show its ticket and title by itself (Settings' "Name new tabs", `name_new_tabs`).
- `M-W` asks for the tickets, IDs or Linear URLs separated by commas or spaces, filled in from the clipboard when it holds only tickets. Every one is checked before any tab opens, so a typo leaves none half-started. The tabs go at the end of the tab bar in the order given, so no open tab is renumbered, and the first comes to the front.
- `M-B` lists the repository's branches, local and remote, the most recently committed first. Enter works on the one picked; a name no branch matches is a new branch, and Alt-Enter takes the name typed even when some branch matches it.
- "Start work on a PR" asks for its number or URL, filled in from the clipboard when it holds one. A URL names its repository, which `wtpr` finds a clone of, so it asks for no directory; a bare number asks which repository it is a PR of.
- `M-I` asks for the repository, then the note, the most recently changed first, then which of its unticked items to work on. Items under a section for parked work (`## Later`, `## Deferred`) and items a worktree already works on are left out, and one whose plan still needs a choice is marked so; with more than one, the first row takes them all, and Tab marks a few (a `+` before each, Shift-Tab marking going up), which Enter then takes together, then which branch to land on ("Land on:", the same question autopilot asks, sharing its order: the branches picked there before in this repository, the most recent first, the default branch among them by when it was picked, then the default branch if it wasn't, so with nothing picked yet Enter lands there, then the repository's other local branches by name, then "Another branch…", a branch typed there counting as picked; a branch other than the default one runs `wtn --target <branch>`, as autopilot's tabs do). The lower half of the list shows the item under the cursor whole, its full text and what is indented under it, since a row cuts it to one line (Settings' "Show a note's item whole in wtn's list", `wtn_item_preview`). Each item gets a tab, named after it and linked to the note: the tab bar shows the item's first 50 characters, cut at a word and ending in …, and `M-T`, the sidebar and mission control its whole first line, while the tab keeps that name; renamed since, it shows its new name.
- `gw` in Notes, on an unticked checklist item at the top of its list (the Format menu's "Start work on the item in a new tab" and Notes' palette have it too), starts that item the same way without asking for the note or the item: Notes saves the note, so the line it names is the one on screen, then the workspace asks for the repository, the one this note's items were last started in first (kept by the note's ID in `work-recent.json`, so Enter alone takes it), then "Land on:", and opens the item's tab in front. On a ticked item, one under another, one in a section for parked work or a line that is no item, Notes says why; an item a tab is already linked to asks whether to go to that tab, and one a worktree of the repository picked already works on is refused.
- "Continue shaping Linear tickets" asks for the tickets, then the directory, which must be in a repository so `shape` has code to read.
- "Review a contributor's PR" is listed only when the pane you press `M-p` in is in a repository whose main clone has a project skill named review-pr (`.claude/skills/review-pr/SKILL.md`, as Switchboard's own repository has), and asks for no directory: it lists that repository's open PRs from forks, the most recently updated first, each with its author, a number typed taken too, then checks the PR out untrusted, as `wtpr --untrusted` does (in `~/.cache/switchboard/pr-review/<owner>-<repo>/<number>`, again on a later review), and opens a tab in front in the main clone, named `#<number> Review PR`, running Claude with `/review-pr <number>`. Its Claude stays in the main clone, never loading the PR's `CLAUDE.md`, settings, hooks or skills, while the tab is about the PR's worktree (its `@tab_dir` option): named, colored and given its PR from it, so `M-v`, `M-m` and `M-d` there show the PR's, `switchboard tabs` names the worktree, and the diff reads its files as text whatever its `.gitattributes` say. lazygit, which runs the repository's own commands, isn't opened there, and the diff's `ga` asks no question there, since Claude would start in the worktree. A split or new tab from it still starts in the main clone, and handing the tab to other work (`/wt`, `/wtn`) forgets the worktree. With `github` off it is left out, as the other PR commands are.

`M-p`'s "Shape the note's checklist items" opens a tab running `/shape` for each unticked checklist item of a note you pick.

### Autopilot

`M-p`'s "Autopilot: work a note", asked from a pane in the note's repository (a worktree of it will do), keeps tabs working through a note's checklist unattended. It opens mission control on its tabs and asks which note, listing those holding an unticked item, the one it last worked first, then the newest, with the rest behind "More notes…" (all of them at once when none holds one); which branch to land on ("Land on:": the branches picked there before, here or in "Start work on a note's checklist items", the most recent first, then the repository's default branch if it isn't among them, so with nothing picked yet Enter lands there, then its other local branches by name, then "Another branch…" to type one, a branch only origin has made a local one tracking it); how many of its tabs work at once (3); the most of them open at once, those waiting on you included (5); and a spend limit in dollars, empty for none. Each box starts with what it was last time, so Enter keeps it. From then on Switchboard opens the note's next item whenever fewer than that many of its tabs work and fewer than the most are open, one at a time and a few seconds apart, in the background, in the session you used last: a tab running `wtn`, named after the item and linked to the note, as "Start work on a note's checklist items" opens one, in the repository's main clone, its worktree cut from that branch (`wtn --target <branch>`, which records it as `branch.<branch>.wtnTarget`), and `/queue-and-clean` lands it straight onto that branch, as `switchboard landing`'s `target` says: rebased onto it, fast-forwarded, and pushed only when it tracks a remote branch, a local-only one just moving. Each branch landed on has a landing lock and queue of its own (`land-lock` reads the branch from `wtnTarget`, or `--target <branch>`), so tabs landing on different branches never wait on each other. A repository that lands through a pull request (repos.json's `landing` `pr`, or GitHub's rules) is refused: autopilot lands only straight onto a branch for now. The items are taken in this order: those whose `Plan: [[…]]` links a settled plan note first (one with no "## Decision needed", or whose choice was made: a ticked option or a `Chose …` line in each such section, or the item saying `(chose …)`), then those with no plan, each in the note's order; never one whose plan needs a choice (hands-free, below, takes those after the settled ones), one under a section for parked work, one a worktree already works on, one `wtn` started in the last half hour (each item it starts is kept a day in `~/.local/state/switchboard/wtn-claims.tsv`, so one whose worktree is gone a moment before its item is ticked isn't taken again, however its tab was opened), one whose tab landed in the last ten minutes and the note doesn't show ticked yet, or one you skipped. When `wtn` refuses an item in one of its tabs (its branch claimed by another item), the item is skipped, its notification says why ("Skipped <item>: wtn refused it, <why>"), and the tab closes, so no slot waits at a shell; taking the skipped items back offers it again.

Only the tabs it opened count, each marked `auto` in the sidebar. A tab waiting on you (a question, a turn ended) frees its slot but still counts toward the most open, as does one whose `wtn` failed and never reached its worktree, and one out of its worktree but not back in the main clone, as while `/queue-and-clean` removes the worktree it is in, which keeps its item held; a tab `/queue-and-clean` has landed, back in the main clone, leaves the mode, and the next item opens in its place. Once its Claude's pane is back in a main clone, idle, it says `landed` in the sidebar instead of `auto` (`landed without asking` when it did, below), and closes as Settings' "Close a worked note's landed tabs" (`autopilot_close`) says: when seen (the default), once it has been in front for a second and you leave it, or half an hour after it landed if you never look; at once, as soon as it lands; after 5 minutes, or sooner once seen and left; or never, left open for you to close. Whatever the choice, it is never closed while in front or while a tool stands in for one of its panes, a prompt typed in it after it landed makes it yours (`@autopilot_yours`), out of the mode, so it stays open, a plain tab in the sidebar, and its item stays under the Autopilot view's Landed; with "never", a landed tab is yours as soon as it is marked landed, and with Notes moved beside it only its own panes close, so Notes goes back to its own tab. A tab closed without your having seen it leaves a notification, "Closed <item>: landed <short hash>", which Enter dismisses. A setting saved as on before reads as when seen, and off as never. What marks them is kept with the tabs, so it carries on after a restart. It opens nothing while you're away (whatever Settings' "Pause lookups while away" says), once today's Claude spend in the Usage view passes its limit, or while the note changed in the last minute, since an item being typed is saved as it is typed. It says so in the notifications when it stops for the spend, when the note has nothing left to take and none of its tabs is still open (while some are, the Autopilot view's heading says "nothing more to start" instead), or when the note is gone; its notification's menu goes to its line, or puts it away.

While it is on, its line leads mission control's Tabs view, above the cards: `AUTO`, the note, the branch it lands on when it isn't the default one ("→ team/next"), how many of its tabs work and are open against those counts, the item it takes next, and how many are skipped. `k` from the first row of cards, a click or Tab reaches it, and Enter or `m` there opens its menu: pause it (and resume), skip the next item (and take the skipped ones again), take only items with a settled plan (and those without one too), "Close landed tabs: <choice>", which cycles that setting, land without asking (and ask before landing again, below), hands-free (and ask me again, below), change the target branch (for the tabs it opens from then on; those open land where they were cut from), "Tabs working at once: <n>…", "Most tabs open: <n>…" and "Spend limit: <$n a day, or none>…", each naming its value and asking that one alone, written as you press Enter with the rest of the mode kept (slots past the most open raise it, and a most open typed below the slots is taken as the slots, which it says), "Change its note…", which asks the note and the branch to land on and keeps the counts, or stop it; the tabs open carry on either way. "Autopilot: work a note" while it is on opens mission control on the mode (the Autopilot view, with it on) and leaves the menu to `m` or the view's button, so the board shows first. The menu is a box at most 72 columns wide, or two thirds of the screen, an option longer than that (the skip option's item) cut with …. What it keeps is in `~/.local/state/switchboard/autopilot.json`: the note by its path and its ID (given one as the mode is set up, when it has none), its skipped items, today's landings for the Autopilot view, and the items it holds for its tabs, each by its note's ID and first line, so an item skipped in one note never hides one worded the same in another; each tab carries its note's ID in `@autopilot_note`, one opened before taking that of the note it was opened for. Notes takes a renamed or moved note along, and the mode follows one renamed or moved outside Notes too, by its ID, wherever it went, at most every 30 seconds while it looks, writing its new path back; a note at its path rewritten without its ID is given it back, and with the ID nowhere, or another note at its path, it takes nothing and says it can't find the note. Settings' "Autopilot" (`autopilot`) off allows none of it.

Its menu's "Land without asking", after a yes to a question saying what it skips and what it keeps, has its tabs land straight on the default branch without asking before the push: each of its tabs not landed yet is marked `@autopilot_land` (and unmarked by "Ask before landing", which asks nothing), which `/queue-and-clean` reads at its push question and takes as yes, and `/review-loop` reads to fix medium findings without asking, as it does high ones, leaving low ones as known gaps. The tests, two clean review rounds of this very change (the pre-push hook's check), the landing lock and `switchboard leak-check` all still run, and anything `leak-check` finds, an unclear conflict, a test failure the branch didn't cause or the review loop's round cap still asks, as does a repository whose landing runs no tests or no review loop; the landing reads the mark again just before it pushes, so "Ask before landing" chosen meanwhile asks after all. After each landing without asking, the notifications say "Landed <short hash>: <item>" (its menu, or Enter with the Notifications window off, goes to its tab while it is open), the Slack relay sends the same while it runs, the tab says `landed without asking` in the sidebar once landed, and the note's `Done:` line says so. While it is on, the mode's line in mission control says `lands on its own`. It covers landings straight on the default branch alone: a landing through a PR still waits for you to approve its test. In Claude Code's auto mode, which judges each command itself and may refuse a push to the default branch, the landing's push goes through because each of autopilot's tabs starts its Claude with that one command allowed for its session (`claude --settings`, with `Bash(git push origin HEAD:<branch>)`, or the remote branch its target tracks): the rule is in no file, no other session has it, and any other push (forced, another branch, a tag) is judged as before. In another permission mode the same rule means Claude Code's own prompt for that command doesn't come up in such a tab, the landing's question standing in for it. It is there whether or not "Land without asking" is on, since that is turned on and off while tabs work, and with it off the landing's question still asks. Settings' "Autopilot's push allowed in auto mode" (`autopilot_push_rule`) turns it off; a tab `wtn` or `/wtn` opens by hand, and a repository landing through a PR, never get it. The marks are kept with the tabs, so they carry on after a restart, and they are the tab's whatever tool stands in for Claude's pane.

Its menu's "Hands-free: decide for me", after a yes to a question saying what it decides and what still stops, has its tabs take the recommended option of anything they would ask you, and turns "Land without asking" on with it ("Ask me again: hands-free off" leaves that as it was, while "Ask before landing" turns hands-free off too). Each of its tabs not landed yet is marked `@autopilot_handsfree`, a new one as it opens, and the server answers its Claude's hooks: a question box (`AskUserQuestion`) is denied with a reason telling Claude to take the option marked (Recommended), else the first, and write `Decided hands-free: <question> → <choice>` under the item in its plan note (with no plan, indented under the item in the note), so the tab never waits in Needs you; a turn that ends on a question is sent back to carry on with what Claude recommends, at most three times a tab (`@autopilot_nudged`), after which it waits on you as usual, never once it has pushed (`@autopilot_pushed`, which the landing sets) or landed, and never when Claude says `[always ask]` in it, as it is told to for a question a rule or skill says must be asked; and the queue takes the items whose plan needs a choice too, after the settled ones, `wtn` telling Claude to pick the recommended option, tick it in the plan with a `Decided hands-free` line, and build it. Once the tab has pushed, it asks as usual again. What your rules or a skill say must be asked still asks: anything your or the repository's `CLAUDE.md` says to ask about, a question a skill says to stop and ask, which the shipped skills head `[always ask]` (the landing's push question when it doesn't land without asking, an unclear conflict, a test failure the branch didn't cause, `/review-loop`'s untracked files of someone else's, deploy-test's approval), auto mode's permission prompts, and the spend limit, which pauses autopilot as before. After a landing without asking, the notification counts the decisions ("Landed <short hash>: <item> (2 decided hands-free)") and its menu opens the plan in Notes, and the note's `Done:` line names them. An item a hands-free tab finds already done on the branch it lands on is ticked with a `Done:` line naming the commit that did it, and the tab cleans up and goes back to the main clone, where autopilot closes it, rather than stopping to tell you (one that no longer applies still stops); and a hands-free tab's cleanup removes its worktree and branch without asking when nothing would be lost, asking only about what would be. A hands-free tab doesn't ask after landing what the change needs to take effect ("Offer to apply a landed change"): it applies none of it, and a landing made without asking adds it to its notification ("Landed <short hash>: <item> (not applied: Build Switchboard and put it in place of the one running)"), and the note's `Done:` line says what it takes effect once. While it is on, the mode's line says `hands-free`. Settings' "Autopilot's hands-free mode" (`autopilot_handsfree`) off takes it out of the menu and every tab asks as usual.

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
wtn --note "Project notes" --item 12 --target team/next
```

`--note` is the note's path within your notes directory (`$NOTES_DIR`, else `~/Notes`) or an absolute one, `.md` optional; `--item` is the line number of an unticked `- [ ]` item at the note's top level. A ticked item, a line that isn't one, a note outside the notes directory or in its trash, and an item worded the same as another are refused. The branch is `note/<the item's first line, slugged>` and the worktree `note-<slug>`, made from origin's default branch as it is now, fetched first; the tab is named from that line too, which is why `/plan-item` keeps it short. With `--target <branch>`, it is made from that branch instead (one only origin has made a local branch tracking it): from the remote branch it tracks as it is now, fetched first, and refused while the local branch has commits that remote branch lacks, since the landing would never push them; a target tracking nothing is made from the local branch. The target is recorded as `branch.<branch>.wtnTarget`, which `/queue-and-clean` lands it on. Which item a branch is for is kept in the repository's git config (`branch.<branch>.wtnItem`, the note by its ID and the item by its first line), so another item that slugs the same is refused rather than started in this one's worktree, the other item named by its words and its note's title. The same first line in another note is the same item, copied there: `wtn` takes its worktree over, commits and all, moves the claim to this note and says "Taking over <branch>, started from <note>", adding "it is still open in <note> too" while the other note still has it unticked. While its worktree is there, "Start work on a note's checklist items" and autopilot leave the item out of both notes. A note renamed or moved, in Notes or outside it, keeps its items' worktrees: "Start work on a note's checklist items" and autopilot still leave them out, and `wtn` on one reuses its worktree. The note is given an ID first when it has none. Claims from before notes had IDs, by the note's path, are rewritten once in each repository, the note found at that path, else by its title, else as the one note holding the item, each only when it holds the item; one it can't place is listed once and still read by path, and taken as its item's once its note is back there.

Claude is given the item as written, with what is indented under it. It is told to read the plan a `Plan: [[…]]` line links first and ask about any choice the plan still needs (with no plan, to read the code and ask before building what the item leaves open), to compare the item and its plan with the code first and, when it is already done or no longer applies, to say so with the evidence and stop without ticking it (autopilot's tabs, given the same prompt, then wait on you), to build what the code needs where the plan gets a detail wrong and correct the plan to match, stopping only for a change of scope, to commit on the branch, and to land the work as `/queue-and-clean` does once it is done. `--prompt` adds to the prompt. In a tab autopilot opened, in a repository landing straight on a branch, Claude starts with the landing's one push allowed for its session, as "Land without asking" above says.

`/wtn` does for a note's item what `/wt` does for a ticket: it settles which note, lists its unticked items to pick from, names the tab after the item and hands the pane to `wtn`. A branch after the item, `/wtn <item> → <branch>` or `--target <branch>`, is passed on as `wtn --target`, so the item lands there; with none, on the default branch.

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
wtpr --untrusted 123       a worktree that runs none of its code (a fork's PR's default)
wtpr --trusted 123         a fork's PR as your own
```

`wtpr` makes the worktree on the PR's head branch, in `pr-<number>-<branch>`. Claude starts by reading the PR (its description, diff, unresolved review threads, failing checks and its Linear ticket), says where it stands and what it would do next, and changes nothing until you say; on someone else's PR it pushes nothing unless you ask. Settings' "wtpr asks where the PR stands" (`wtpr_prompt`) turned off starts Claude with no prompt. `WTPR_PROMPT` in the environment replaces the prompt, as mission control's "Start a CI loop on it in a new tab" does with `/ci-loop`.

A bare number is a PR of the repository you are in. A URL of another repository, or one given from outside any, is worked on in a clone of it found among every open tab's repository, every one a Claude session ran in lately and the directories right under your home; with none, it says so. Any PR works, whatever its state: one whose branch the remote has deleted opens from the commits GitHub keeps for it, on a local branch with no upstream.

A PR from a fork is someone else's code, so it opens **untrusted** unless you pass `--trusted` (any PR opens so with `--untrusted`): in `~/.cache/switchboard/pr-review/<owner>-<repo>/<number>`, outside the clone, on a local branch `pr-review/<number>` made from the commits GitHub keeps for it, with no remote added. That worktree's own git config, which no file in it can change, marks it (`switchboard.untrusted`) and turns off git's hooks, every filter your config names (a checkout runs the smudge filter its `.gitattributes` picks, git-lfs's say), the file-system monitor and submodule recursion (which would fetch from wherever its `.gitmodules` says), so no git command there runs the PR's hooks or yours. Nothing is linked into it, your shell stays where it is, and no Claude is started, since its `CLAUDE.md`, `.claude/settings.json`, hooks and skills are its author's. It prints the author and how long their account has been on GitHub, the PR's size and `switchboard pr-risk`'s list of what in it runs without being asked, then how to go on from your main clone: `/review-pr <number>` in Switchboard's own repository, or `switchboard pr-risk`, `switchboard codex-review -C` and `switchboard try --sealed` by hand (see [Someone else's PR](#someone-elses-pr)). Its title isn't printed, since its author wrote it and a terminal acts on escape sequences in it. Run again, it moves the worktree to the PR's newest commit.

A PR from a fork opened `--trusted` (a collaborator's you know) is set up as `gh pr checkout` sets one up: the fork becomes a remote named after its owner, and the branch, `<owner>-<branch>`, tracks the fork's, so a push updates the PR. It says so when the fork's owner hasn't let maintainers push.

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

`/review-loop` runs the branch through rounds of skeptical, read-only review, fixing and committing the findings it agrees with, until two rounds in a row come back clean, five rounds at most. Before the first round it runs Claude Code's `/security-review` once, its findings fixed or asked about as the rounds' are (Settings' "Security review in the review loop", `review_security`). It keeps a notes file the reviewer reads every round, holding each finding you dispute and each gap you accept, with a reason for each. It pushes once, at the end, after asking. `/review-loop all` asks about low-severity findings too, `/review-loop norules` leaves the rules out, and `/review-loop <worktree> [<base>]` reviews someone else's PR checked out untrusted from outside its worktree, reporting only ([Someone else's PR](#someone-elses-pr)).

Each round is `switchboard codex-review`. It reviews committed changes only, and refuses a tree with uncommitted ones. The PR is looked for as the tab's name finds it: in the repository `gh repo set-default` named, else in origin and the project an `upstream` remote names, origin's winning when both have one, and only a PR from where the branch is pushed counts, never a stranger's with the same branch name. The base is the PR's base on the remote that points at the PR's repository (upstream, in a fork's clone), else origin's default branch; on the default branch with no PR, working straight on `main`, it reviews the commits origin doesn't have yet, fetched first. It prints one JSON object: `summary`; `findings`, each with a severity (`critical`, `high`, `medium` or `low`), a category, a file and line, a title and the failure it found; then `reviewer`, `branch`, `base`, `reviewedCommit`, `mergeBase`, `patchId` and `warnings`, which lists the findings that mention a path outside the repository, to read before acting on them.

```
switchboard codex-review                       the branch against its PR's base (on main: the unpushed commits)
switchboard codex-review --base upstream/main  against another base, used as given when the ref exists
switchboard codex-review -C <dir> --pr <n> --base origin/main
                                               someone else's PR's untrusted worktree, from your main
                                               clone, with its description
switchboard codex-review --notes notes.md      disputed findings and accepted gaps, each with a reason
switchboard codex-review --include-low         report low-severity findings too
switchboard codex-review --no-rules            leave your rules and the repository's out
switchboard codex-review --reviewer claude     have Claude review even with Codex there
switchboard codex-review --model <name>        the reviewer's model, the one option passed on to it
switchboard codex-review --dry-run             print the prompt and the reviewer's command, and run nothing
switchboard codex-review --patch-id A B        print the change from A to B as one hash, and review nothing
```

Codex runs in its read-only sandbox, the commands it runs with no network and no way to ask for more, without your `~/.codex/config.toml` or the repository's `AGENTS.md`, and with the commit messages and the PR description given to it as data, not instructions. Without Codex, Claude Code reviews with the same prompt and JSON, with only its Read, Grep and Glob tools, so it runs nothing, and with no settings, MCP servers, skills or CLAUDE.md. The change is given as text (`git diff --text --no-textconv --no-ext-diff`), whatever the branch's `.gitattributes` say, since a `-diff` there would hide a file's lines. `-C <dir>` reviews the repository at `<dir>` rather than the one you are in, and `--pr <number|url>` names the PR whose description goes in the prompt when the branch's name doesn't lead to it, as with someone else's PR checked out untrusted; a PR named that isn't there stops the review. `CODEX_REVIEW_VERBOSE=1` shows Codex's progress on stderr.

The reviewer is given the rules you work under, and reports a change that breaks one (category `rules`): your `~/.claude/CLAUDE.md` with what it imports, and the repository's `CLAUDE.md` and `AGENTS.md` as the base has them, which win where the two disagree. Settings' "Rules in reviews" (`review_rules`) turned off leaves them out, so your rules aren't sent to the reviewer.

`patchId` is the reviewed change as one hash, context lines included, so the same change rebased onto a newer base has the same one. It is `git patch-id --verbatim`'s hash (whitespace kept), worked out by `codex-review` itself with git's diff options fixed, so a git older than 2.39, which hasn't `--verbatim`, or a git config of yours gives the same one; `switchboard codex-review --patch-id <from> <to>` prints it for any two commits, as the pre-push hook asks for it. Once a loop is clean, `/review-loop` records the reviewed commit, its base and that hash in the clone's `review-loop-clean` (`git rev-parse --git-path review-loop-clean`), for a pre-push hook that asks for a review. Switchboard's own repository has one for `main`, which Settings' "Review before pushing" (`review_before_push`) turns off.

### Someone else's PR

A pull request from someone you don't know is reviewed before any of its code runs on your computer. `wtpr` opens a fork's PR untrusted (above, [`wtpr`: a pull request](#wtpr-a-pull-request)): in a worktree under `~/.cache/switchboard/pr-review/` where no git hook, filter or Claude session of its own runs. From your main clone, then:

```
switchboard pr-risk <worktree> <base>                              what in it runs without being asked
switchboard codex-review -C <worktree> --pr <number> --base <base>  a review round, from outside it
/review-loop <worktree> <base>                                     the review loop, reporting only
switchboard try --sealed <worktree>                                its code, in a sandbox (macOS)
switchboard try --sealed <worktree> --run cargo test               its tests, in the sandbox
```

`/review-loop` given the worktree's path reviews it from the session you are in, never one started there, with `codex-review -C` every round, and fixes, commits and pushes nothing: each finding it agrees with goes in its notes file and in a report worded for the PR's author. Its code runs only in a sealed `switchboard try` ([Sealed](#sealed)), which Linux hasn't. In Switchboard's own repository, `/review-pr <number>`, a skill of the repository's own that no one else is given, runs all of it and drafts the review for you to post; `M-p`'s "Review a contributor's PR" picks the PR and starts it in a tab of its own. `switchboard pr-risk` is the first look at it: what it changes that runs without being asked, each file with its added lines, read from git alone (no hook, external diff, textconv or file-system monitor, and as text whatever its `.gitattributes` say), so nothing of the PR's runs.

```
switchboard pr-risk <worktree> <base>          the report
switchboard pr-risk --files <worktree> <base>  the files alone, a line each
switchboard pr-risk --json <worktree> <base>   all of it as JSON
```

It lists, under a heading each: what git runs or reads in the worktree (`.githooks/`, `.husky/`, hook managers' files, `.gitattributes`, which picks among the filters and diff drivers your config names, and `.gitmodules`); what runs on install and setup (`install.sh`, `src/setup/`); what a Claude session loads or runs (`.claude/`, `CLAUDE.md` and `AGENTS.md` anywhere, `.mcp.json`, and the plugin's settings, skills and instructions in `assets/`); what runs as it builds (`build.rs`, `Cargo.toml` and `Cargo.lock`, `.cargo/` and `rust-toolchain.toml`, anywhere: a build script or a proc macro runs as cargo builds, checks or tests, and as an editor's rust-analyzer opens the folder); CI (`.github/workflows/`, `.github/actions/`); and packages (manifests, lockfiles and package managers' settings anywhere). Then every added line, in any file, that fetches, evaluates, runs a shell, decodes, runs AppleScript, asks the Keychain, reaches for credentials or SSH keys, opens a socket or changes what git runs (its hooks path among them), and the hosts the PR names that the base names nowhere.

A worktree of someone else's PR checked out untrusted, in `~/.cache/switchboard/pr-review/` or marked `switchboard.untrusted` in its own git config, is left alone, since its `CLAUDE.md`, `.claude/settings.json`, hooks and skills are its author's: `claude-here`, `wtr` and `wtb` refuse it; `M-t` opens no Claude tab there; `wt-revive`, the Work map and the Slack relay's places never list it, and the relay refuses one named; `switchboard skill-path` passes over its skills for its main clone's, so no PR can rewrite the review loop that reviews it; and `switchboard apply install` never builds it, whatever `trusted-dirs` says.

### Landing: `/queue-and-clean`

`/queue-and-clean` takes a branch from finished to gone. With a PR, it queues it for merge (the merge queue, or auto-merge), watches it in the background, and hands a failing check, a conflict or a drop from the queue to `/ci-loop` to fix, three times at most. Once it merges, it waits for the deploy and tests it as `/deploy-test` does. Once you approve the video, it marks the Linear ticket done, ticks the item in your note, removes the worktree and its local branch, and starts the tab's pane again in the main clone with the same conversation resumed, keeping the tab's name. With no PR, it checks whether the default branch already holds the work, a squash included, and cleans up if so. If not, it offers to open a PR, or, in a repository that lands straight on its default branch, as `switchboard landing` says (below), to land it there. A branch `/wtn` opened asks neither: its prompt names the note, and, in such a repository, settles that it lands on the default branch (the branch's `wtnItem` in git's config marks it).

`switchboard landing [--json] [--refresh] [<dir>]` says how the repository at `<dir>` (the current directory by default) lands its work: `pr` (through a pull request) or `direct` (straight on its default branch) on the first line, then a direct landing's steps as JSON, `test`, `testIn`, `squash`, `review` and `beforePush`, with where the answer came from (`source`) and why; `--json` prints both as one object, and it fails when `repos.json` can't be read. The answer is the repository's entry in `~/.config/switchboard/repos.json`, by its origin's `owner/name`:

```json
{ "owner/name": { "landing": "direct", "test": "npm test", "testIn": "here", "squash": true, "review": "review-loop", "beforePush": "" } }
```

`landing` is `pr` or `direct`; the rest is a direct landing's steps: `test` runs before the review and again after the last rebase (empty, none; not set, the tests the repository's `CLAUDE.md` names), `testIn` is where they run, `here` (in the worktree, the default) or `ci` (in the repository's CI, below), `squash` lands the branch as one commit (off, its commits land as they are), `review` is `review-loop` or `none`, `beforePush` must pass before the push (empty, none), and `afterRebase` runs in place of the tests after the last rebase when that rebase had no conflict and the default branch's CI isn't failing (empty or not set, the tests run again; `cargo clippy --locked --all-targets -- -D warnings` for a Rust repository, say). With no `landing` set, it works one out: a pull request when GitHub says the default branch needs one (a ruleset, branch protection, or a merge queue), else straight on it when the repository's `CLAUDE.md` says its work goes there with no PR, as a headless Haiku reads that file alone (never one that links out of the repository), else a pull request. That answer is kept a day in `~/.local/state/switchboard/repos-landing.json`, `--refresh` asking again; off the network, or with Settings' "GitHub" off, the last answer stands, and with none it is a pull request. Settings lists each repository a tab is in under "Landing (repos.json)", with what was worked out for it (going to Settings works out those not known yet, in the background), and Enter changes its entry: how it lands (automatic, through a PR, straight on the default branch), and, landing straight, its tests, where they run, squash, review, command before the push and check after a clean rebase.

Landing on the default branch makes the branch one commit on top of the default branch as it is now, runs the tests, runs `/review-loop` on it, runs `switchboard leak-check` on what it would push and asks before pushing, showing what `leak-check` found, all in each worktree at once, each step only as the repository's steps say (a tab of autopilot with "Land without asking" on takes the question as yes when `leak-check` finds nothing, the tests ran and the review loop ended on two clean rounds of this very change, above). A `beforePush` command must pass before the push. Only the push waits its turn: the worktree takes the landing lock, rebases onto what landed meanwhile and, when anything had, runs the tests again (with `afterRebase` set, a rebase with no conflict onto a default branch whose CI isn't failing runs that check instead, since the tests have passed on each side, which keeps the lock held for about a minute rather than a second CI run), and pushes with no second review while its change keeps the review's `patchId`, reviewing and asking again when it doesn't. Then it fast-forwards the main clone, ticks the note's item and cleans up as above.

With `testIn` `ci`, a landing's tests run in the repository's CI rather than on your computer, both times. The landing runs `switchboard leak-check` on its commit, asking before anything leaves when it finds something, pushes the commit to a branch of its own, `ci/land-<id>` (the id a hash of the worktree's branch name, so the name itself, which may hold a note item's words, isn't published), forced since each fix rewrites it, and waits for the CI that runs on it with `switchboard wait-ci`; a failure is read from the run's log and fixed as a local one is, unless the default branch fails the same tests at its tip (`wait-ci --now`), when the landing lets the lock go, waits for a tip of the default branch that passes and starts again from the rebase, fixing nothing, and the branch is deleted once the landing ends, whichever way. The setting is your standing answer for that push, so it isn't asked about; the push to the default branch still is. A repository with no CI, or whose CI doesn't run on such a branch, has its tests run in the worktree, as `here` does. `ci` takes whatever CI runs on a pushed branch to be the repository's tests, so set it only where CI runs them on every branch.

`switchboard wait-ci` prints one line once the commit's check runs and commit statuses settle: `passed <sha>` when every one has finished with none failing and one at least succeeding, `failed <sha> <run id> <job names>` as soon as one fails (the run's id for `gh run view <run id> --log-failed`, `-` when the check isn't GitHub Actions'), `none` when no check run that ran anything has appeared within `--grace` minutes (3 by default; check runs all skipped count as none), or `timeout` after `--timeout` minutes (45 by default). With `--now` it asks once and says what that look sees, without waiting: `passed <sha>`, `failed <sha> <run id> <job names>`, `running <sha>` or `none`; it exits 1 when GitHub can't be asked.

```
switchboard wait-ci --repo owner/name --sha <commit>
switchboard wait-ci --repo owner/name --sha <commit> --timeout 60 --grace 5
switchboard wait-ci --now --repo owner/name --sha "$(git rev-parse origin/main)"
```

Once the main clone has the landed change (after a PR's merge too, which fast-forwards it first when it has the base branch checked out), it asks which of what the change needs to take effect to do, each named with the file that calls for it, and does the ones you pick; what only you can do it names in the question, and the note's item's `Done:` line says what was applied or when it takes effect. The list comes from `switchboard apply-actions <repo> <from> <to>`, which prints one action a line, `<key>`, `<label>`, `<why>` and `<command>` tab-separated, an empty command being yours to do, and each listed once however many files call for it:

- In your own clone of Switchboard, `~/switchboard` by its real path, a change to its code (`src/`, `assets/`, `build.rs`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`) is built and put in place of the binary running (`switchboard apply install <clone>`: `cargo build --release`, the build written beside the binary, run once and renamed over it, as `switchboard update` puts a release, then its `setup --refresh`, so Claude started from then on loads its skills and hooks), the Slack relay restarted while it runs and its code changed (`switchboard apply relay`, which waits for it to stop before starting it again in its tab), and the workspace restarted, which is yours to do, since the server, Notes, `M-d`'s diff and the views run the build they started with: `switchboard upgrade` restarts the server on it keeping every tab and what runs in it ([Restarting on a new build](#restarting-on-a-new-build)), and Notes, the diff and the views take it when next opened. A release build lists none of it, since releases keep it current.
- In another repository, the `apply` list of its entry in `~/.config/switchboard/repos.json`, by its origin's `owner/name`, each `{ "paths": ["db/**"], "label": "Reload the dev server", "command": "make reload" }` run in the clone when one of its `paths` (`*` within a folder, `**` any number of them) changed. Without one: a changed lockfile installs with its package manager (pnpm, npm, Yarn, Bun, Bundler, Poetry, uv) where packages are installed, Yarn's Plug'n'Play included; new migration files run the package's migrate script (`db:migrate`, `migrate`, …); and a changed `.env.example` asks you to copy what's new. An install, a migrate script or a command of yours runs the clone's own code, so each is offered only in a clone listed in `~/.config/switchboard/trusted-dirs`, and named for you to run elsewhere.

A tab in autopilot's hands-free mode asks nothing here and applies nothing, leaving it to you: the note's `Done:` line says what wasn't applied, and so does the landing's notification when it landed without asking. Settings' "Offer to apply a landed change" (`apply_after_landing`) turns the offer off.

`switchboard leak-check <range>` looks for anything identifying you or your employer in what a range of commits adds, before it goes to a public repository: each line any of its commits adds (a line a later commit removes, a merged side branch's and a file the repository marks binary included, read without its textconv drivers) and each commit message. It finds the words in `~/.config/switchboard/leak-check.txt` (your organization's and company's names, usernames, internal hosts, one a line, `#` lines left out; Settings' "Identifying words" opens it), as whole words in any case; Linear-style ticket IDs and PR numbers written with a `#`, outside tests, whose IDs are made up (and `ABC-…`, `XYZ-…`, `#123` and `#12345` anywhere, the made-up ones the docs use, and standards such as `UTF-8`); home directory paths; email addresses but noreply and example ones; and the shapes of GitHub, OpenAI-style and Slack tokens. A hit in a file is left out when the range's base (the merge base of its two ends) already has the same text in its copy of that file, so a line a change moves or rewrites isn't reported again, while one brought into another file, or a new one, still is; a commit message's hits always count. It prints each hit as `<file>:<line>: <kind>: <text>` or `commit <hash>: <kind>: <text>`, then on stderr how many it left out (`--all` prints those too, marked `(on <base>)`), and exits 1 on any hit kept, 0 on none and 2 when it can't look, a word list there but unreadable included, so a landing asks rather than pushing unchecked.

The lock, and the queue of worktrees waiting for it, live in the main clone's git directory, shared by every worktree of it, one of each per branch landed on: the default branch's `land.lock` and `land.queue`, and a target branch's `land.lock.<branch>` and `land.queue.<branch>` (its `/` written `%2F`). Each command takes the branch from the worktree's `branch.<branch>.wtnTarget`, or from `--target <branch>` given before it, else the default branch. `switchboard land-lock` keeps them:

| Command | What it does |
|---|---|
| `land-lock take` | Takes the lock for this worktree: exits 0 when taken (or held here already), or 3 when another holds it or others wait ahead, printing which, and joins the queue |
| `land-lock wait` | Takes it, waiting in the queue for this worktree's turn; exits 4 when taken out of the queue meanwhile (mission control's Landing view) |
| `land-lock release` | Lets it go when this worktree holds it, and leaves the queue |
| `land-lock leave` | Leaves the queue |
| `land-lock holder` | Prints which worktree holds it, nothing when none does |
| `land-lock queue` | Prints the holder and each waiter, a line each: place, path, since when |
| `land-lock move <path> up\|down\|first` | Moves a waiter in the queue |
| `land-lock phase [words]` | Shows the landing's step in the tab; no words clear it |
| `land-lock landed <short hash> <branch>` | Says the tab's work landed straight on `<branch>`: marks it `@tab_landed`, which files it under Merged as "landed on <branch>" while the tab is in the repository's main clone on the branch it had |
| `land-lock autolanded <short hash> [--plan <file> \| --decided <n>] [--not-applied <labels>] [item]` | Says the tab landed without asking: marks it `@autopilot_autoland` with the hash and adds "Landed <short hash>: <item>" to the notifications (`~/.local/state/switchboard/autolanded.json`), with the count of the plan's `Decided hands-free` lines (the count `--decided` gives for an item with no plan), what a hands-free tab left for you to apply (`--not-applied`) and a way to open it; said again for the same hash, it updates that notification |

The lock never expires with time, since a landing may wait on you while it holds it. It is taken over only from a landing whose worktree is gone, or whose Claude session has ended with none other running in the worktree, and a waiter gone the same way leaves the queue. The lock and each place in the queue name the Claude session that took it (Claude Code's `CLAUDE_CODE_SESSION_ID`) and its pane, so a landing run by a session started in the main clone keeps its lock, and a `take` from the same worktree by another session names that one instead; one taken by a build from before names no session and is kept by a session in its worktree alone. The sidebar and mission control show the landing tab and those waiting under Landing, in the queue's order ("landing", "2nd in line to land"), and Enter on a waiting tab offers "Land it next", "Move it up in the landing queue" and "Move it down in the landing queue" (Settings' "Show the landing queue", `landing_queue`; off, they show as Working, and the queue still orders the landings).

### Deploys and videos

`/deploy-test` waits for a merged PR to deploy, tests the feature it adds on the deployed site in a real browser, records a captioned video of the test, and asks you to approve it. `/queue-and-clean` runs it before removing a worktree, and you can run it on any merged PR. It changes nothing real on the site unless the environment is a test one or you say so. `/preview-test` does the same before the merge, on an open PR's preview deploy, found from the branch's GitHub deployments, a bot's comment or the repository's docs.

Both wait with `switchboard wait-deploy`, which prints one line once the deploy settles: `deployed <sha> <url>` (`deployed <sha> -` when it watched a deploy workflow, which has no address), `failed <url>` or `timeout`.

```
switchboard wait-deploy --repo owner/name --sha <commit> --environment production
switchboard wait-deploy --repo owner/name --sha <commit> --environment Preview --ref <branch> --timeout 20
switchboard wait-deploy --repo owner/name --sha <commit> --workflow Deploy --branch main
```

`--environment` watches GitHub deployments to that environment (what Vercel, Netlify, Render and most deploy actions report), and `--ref` only those of one branch; `--workflow` watches a deploy workflow's runs on `--branch`. A deploy counts when its commit is the one given or a later one, since a merge queue often deploys a commit that includes it. It looks every 30 seconds, for `--timeout` minutes, 45 by default.

Both record with `/walkthrough`'s recorder. `/walkthrough` records a feature as a video to keep or share: from the site's home page through its own navigation, each step captioned, each click shown with a cursor, and the page's address above it. You look it over and keep it, record it again or drop it; a video you keep goes in `~/Movies/Walkthroughs/`. The recorder is a Node program using Playwright, kept with the skill, so it needs Node, the system's Chrome and `ffmpeg`, and its packages installed in the skill's scripts folder in Switchboard's plugin, which the skill does itself when they are missing:

```
npm ci --prefix ~/.local/share/switchboard/plugin/skills/walkthrough/scripts
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

`switchboard skill-path <name> [<dir>]` prints the `SKILL.md` to follow for a skill in the repository at `<dir>`, the current directory by default: the worktree's own `.claude/skills/<name>/SKILL.md`, else the main clone's (a branch cut before the repository added the skill has none of its own), else Switchboard's own: your personal `~/.claude/skills/<name>/SKILL.md` when it is Switchboard's (written there with the plugin off, or a copy of it you made your own by deleting its `.switchboard-skill`, which still opens by running `skill-path`), else the plugin's, `~/.local/share/switchboard/plugin/skills/<name>/SKILL.md`. A skill of yours that only shares a name with one of Switchboard's is never printed, so Switchboard's, run as `/switchboard:<name>` or handed off to by another, follows its own text. It prints nothing and exits 1 when there is none.

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

It needs the workspace's server running, and works from anywhere that server can be reached (a Claude job, ssh): a new tab goes in the session you used last. The tab runs `switchboard slack-relay run`, which starts the relay again if it crashes, inside an interactive shell so your shell's setup puts `claude` on the PATH for its Haiku calls. When the relay stops, the tab is left at that shell, and `M-S` starts the relay there again. It also needs Switchboard's hooks for Claude Code, since that is how it hears what each session does: as a plugin, only the sessions in the workspace's panes run them, so the relay hears of those alone; with Settings' "Load Switchboard as a plugin" off, every session runs them from your `~/.claude/settings.json`, one in a terminal of its own too.

`switchboard slack-relay notify` is how something else on the computer reaches you through the relay, such as the `oncall` skill. The notice goes out as the app's bot, under a 🔔 and the `--from` name (`notice` when none is given; up to 40 letters, digits, spaces and `. : _ -`), so Slack notifies you, which it never does for anything posted as you. The running relay posts it within a second, whether or not you are at the computer, and `notify` waits up to 45 seconds for it: it exits 0 once it is posted and 1 if it wasn't (the relay isn't running, or Slack refused), so a caller knows whether you were reached. It only ever posts in the DM.

When it starts, the relay says so in the DM with buttons for what you would otherwise type: **Home** (a link to the app's Home tab, added once Slack has sent the relay an event, since that is the only way it learns the app's ID), Status, Threads, Progress, New tab, Help, and Stop relay, which asks first. Stopping on purpose edits that message to say the relay stopped and when, and starting again within a day edits it back, so restarts don't fill the DM; a first start, an older message or a crash posts a new one. You stop it on purpose with `stop` in the DM, Stop relay, `switchboard slack-relay stop`, M-p's "Stop the Slack relay", Ctrl-C in its tab, or closing its tab.

### Threads and notices

Each Claude tab's thread is started the first time there is something to say about it, or when you open its screen from the Home tab, and holds everything about that tab: its finished turns, its waits, its milestones, notes that it may be stuck, and the answers to what you sent. The thread starts with a header naming the tab, its state and since when (🔴 waiting on you, 🔵 working, ✅ finished, ⚪ idle), and Progress, Skill, Rename and Close buttons. The header is edited whenever the tab is renamed or its state changes, so Slack's list of threads shows how every tab stands. Once the tab closes, or starts another Claude session, the header says its session has ended and offers **Delete thread**. A thread belongs to its tab for as long as the tab is open, and a new tab gets a new one.

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

**An unsent draft.** Text typed in a tab's prompt box and not sent would go with your message as one prompt, so before typing a message the relay reads the prompt box, and when it holds a draft, it types nothing and asks in the thread, quoting the draft, with three buttons: **Replace it** clears the draft (Ctrl-U) and sends your message, **Send after it** types your message where the prompt's cursor is, after the draft unless you moved the cursor back into it, and sends both, and **Keep it** leaves the draft and drops your message. Only what is typed counts: Claude Code's dimmed placeholder and suggested next prompt don't. A second message for the same tab while one waits takes its place and asks again, and the buttons of the first then do nothing. A button pressed once the draft has gone (sent or cleared at the computer) sends your message as it is and says so. The message waits only while the relay runs. Settings' "Ask before typing over a draft" (`relay_draft_check`) off types the message after whatever is there, as before.

**Prompts and questions.** When a tab waits on a permission prompt or a question, the alert shows it and its numbered options, read from the screen, with a button for each. Tap one, or reply with its number or the start of an option. Only the number is pressed, with no Enter after it, since an Enter could accept a second prompt that follows. Anything else is refused while the choice is showing, rather than typed, since it would end with Enter and accept whichever option is selected; the refusal carries the option buttons. A button answers only the prompt it was made for, while that same wait is on, and is replaced by what was chosen once pressed. Answering a prompt anywhere (a button, a typed reply, or at the computer) clears the buttons from every other message showing it. A dialog before a session starts is answered the same way, the relay moving to your option with the arrow keys and pressing Enter.

A question Claude asks you itself (AskUserQuestion) works the same, with more to it. When it asks several at once, Claude Code shows them one at a time in the same wait, so after each answer the relay looks at the screen again and posts the next question, and then the review of your answers, each with its buttons. A question that takes several answers shows its options ticked ☑ or not ☐: a tap or a number ticks or unticks one, `1 3` does two, and **Submit choices** (or `submit`) moves on. Any other reply to a question Claude asked is typed in as its "Type something" option, your own answer.

**Quick replies.** A finished turn has a **Skill** button. When Claude ended by asking, the replies you would likely tap are added a few seconds later: **Yes, go ahead** when it asks whether to go on with what it proposed, and a button per option when it asks you to pick one of a few it named. A question mark alone can't tell those from a question only your own words answer, so a Haiku reads Claude's last message (only one with a question mark near its end), and anything else gets no buttons. A tap types the reply into the tab, and is refused once Claude has said more since.

**Skill.** Slack takes any message starting with `/` for one of its own commands and never sends it, so `/review-loop` can't be typed into a thread as it is at the keyboard. Skill opens a form instead: a menu of the skills that tab can run, and an Arguments field for what follows the name, such as a ticket ID. Picking a skill redraws the form to say what it takes, from the `argument-hint` in its file. The menu is the tab's own: the skills its session lists, plus those only you can invoke, from `~/.claude/skills` and `~/.claude/commands` and from `.claude` in the tab's directory and the ones above it, and Switchboard's from its plugin while Settings loads it as one (by their bare names, or `switchboard:<name>` where one of yours or the project's has the bare one). It is grouped frequently used (the eight you use most, from the counts Claude Code keeps, each use counting half as much every 30 days), yours, Switchboard's, the project's, Claude Code's own commands that finish without asking anything (`compact`, `clear`, `context`, `cost`, `usage`), then plugins and built-in skills. A Slack menu holds 100 options, so a longer list is cut from the end, and a field takes any name the menu left out. A thread reply that starts with a space also gets past Slack.

### The Home tab and a tab's screen

The app's Home tab opens with when the computer last checked in, a time Slack itself renders and keeps counting ("checked in 47 minutes ago"); the relay publishes it every minute while it runs, so anything older than two means it is offline. **Refresh** beside it publishes at once, and **New tab** asks where and for the prompt, as `new <place>:` does: Where is a menu of every checkout a Claude session has run in that still exists, and every open tab's, newest first, grouped by repository, with a field beside it for anything else. **Delete N closed threads** appears while there are some.

Then every Claude tab, in the sidebar's groups and order (**Needs you**, **Review loop**, **Merged**, **Idle**, **Reminders** and the rest), so the Home and the sidebar never place a tab apart; a tab under Reminders says when ("remind in 2h"). A server too old to say a tab's stage gets the Home's own three groups instead: **Needs you** (waiting on you, then finished and not yet looked at, the longest first), **Working** and **Idle**. Each has its state, for how long, its last activity, and one line from Haiku on what it is doing or, once it has stopped, where it left off, written again when the tab has done something or changed state, at most once a minute. A tab waiting on a prompt shows the prompt's options, which answer it from there. The relay looks every 10 seconds (every minute with Settings' "Look often", `poll_fast`, off) and publishes when anything changed, at least once a minute, and whenever you open the tab. The bottom says what the Haiku calls have cost today, from what `claude -p` reports for each. When the relay stops on purpose, the Home tab says so, since nothing in it is current any more.

**Open** on a tab shows its screen: its state, its last prompt, the end of its last reply, the options of a prompt it waits on as buttons, and these buttons:

| Button | Does |
| --- | --- |
| Message | a field whose text is typed into the tab, handled as a reply in its thread would be (asking first when its prompt holds an unsent draft); the answer goes in the thread |
| Skill | the Skill form |
| Go to thread | the tab's thread, started when the screen opens if the tab has none yet |
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

The tabs' threads always get everything at once, so each is a complete record whether you were at the computer or not. What notifies you is the one-line notice in the main DM, and those wait while you are at the computer, since the workspace already shows them, and go out in the order they happened once you have been away 30 seconds. You count as away once nothing has been typed or clicked for 30 seconds, at the computer or in Switchboard from another one over SSH, or at once when the screen is locked, unless a key is then pressed in Switchboard from another computer; where the computer can't say (a Linux with no desktop session), the keys pressed in Switchboard alone count. Writing to the relay from Slack counts as being away for five minutes from your latest message, and sends what was held, unless the keyboard or mouse was used, or a key pressed in Switchboard, in the five seconds before it arrived, which means you wrote it in Slack where you work, not on your phone. Replies to what you send, and the results of requests you made from Slack, never wait. Held notices are kept in memory, so stopping the relay drops them.

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
      - users:read
settings:
  event_subscriptions:
    bot_events:
      - app_home_opened
      - message.im
  interactivity:
    is_enabled: true
  socket_mode_enabled: true
```

That is Socket Mode on, so Slack connects through a connection the relay opens and nothing on the computer listens for connections; the bot scopes for posting, reading and opening its DM, reacting to your messages, fetching and uploading files, and reading people's profiles for their photos (`users:read`; an app made before it needs the scope added and the app reinstalled); only the `message.im` and `app_home_opened` events; Interactivity for the buttons; and the Home and Messages tabs, with messages allowed from the Messages tab. Install it to your workspace, then make an app-level token with the `connections:write` scope (Basic Information, App-Level Tokens).

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

The running server goes on with the old version until it restarts on the new one. Once the release is in place, `switchboard update` asks whether to do that now, keeping every tab and what runs in it ([Restarting on a new build](#restarting-on-a-new-build)); Enter is yes, and with no terminal to answer (piped, or run by a script) it only says how. Settings' "Offer to restart after an update" (`upgrade_after_update`, on) off, it never asks, and says how.

A release build of Switchboard looks for a new release by itself: each time a terminal attaches, and every hour. When there is one, a box lists the releases newer than yours, newest first, each with its age and notes, over two options. "Update now" opens a tab that runs the update and its setup, then offers the restart, and stays until you press Enter. "Not now", or Esc, puts it off: the hourly look doesn't ask about that release again, only about a newer one, while the look as a terminal attaches asks whenever something newer is out. A box left unanswered is asked about again the next hour, and a newer one takes its place. Settings' "Update check" (`update_check`) chooses when it looks: "open, hourly" (`hourly`, the default), "open only" (`open`), or "off" (`off`).

`M-p`'s "Update Switchboard" looks now, whatever that setting says, and says so when there is nothing newer or it can't look. `M-p`'s "Run Switchboard's setup" opens a tab running the guided `switchboard setup`, to put in place what isn't, or to say yes to a part you said no to before; "Check Switchboard's setup" and "Run a part of Switchboard's setup" are in [More in the palette](#more-in-the-palette).

When `~/.config/switchboard/claude.json` names a git repository for your Claude rules, each look the setting allows (as a terminal attaches, every hour, or asked for) also pulls that repository's clone, fast-forward only, with `switchboard setup claude-md`, which writes `~/.claude/CLAUDE.md` again if it must. This happens whether or not your build looks for releases, since your rules change apart from Switchboard. It never asks for a password: git's prompt and askpass are off, and so is ssh's askpass, so a pull that needs one fails quietly and is tried again at the next look.

A `switchboard` you built from source with cargo is kept current by git and cargo: its server never looks for releases, and `switchboard update` (and `M-p`'s "Update Switchboard", which says what is out and how to take it) leaves it alone unless you add `--force`. After each build, run `switchboard setup`: the skills, hooks, Claude Code's settings and Switchboard's part of CLAUDE.md are compiled into the binary and reach the plugin and `~/.claude` only through the setup (the plugin also as the build's server starts). The first time a new build attaches (`switchboard`), it does this by itself, quietly, for the parts you have set up, and records the build in `~/.local/share/switchboard/setup-build` so it does it once; a part you said no to is left for the guided setup to ask about, and it moves nothing between the plugin and `~/.claude` (an install an earlier version made in `~/.claude` stays there, the setting recorded off, until the guided setup asks). A run with a state directory of its own (`SWITCHBOARD_STATE_DIR`) leaves `~/.claude` and the plugin as they are, its server too. `switchboard setup --refresh` does the same by hand. With the new build in place of the one the server started from, `switchboard upgrade` restarts the server on it.

### Restarting on a new build

`switchboard upgrade`, or `M-p`'s "Restart Switchboard on the latest build on disk", restarts the server on the binary at the path it was started from, as that file is now, without ending anything: every pane's program goes on running, a Claude session mid-turn, a build or a shell alike, and every terminal attached stays attached. The server replaces itself with the new build in the same process, so its programs stay its own and never see their terminal close, and hands it every pane's terminal, every attached terminal's connection, its socket and what each pane showed; the new build takes them up as they are, shows each pane as it was (its history, colours, the screen of a full-screen program, where its cursor was), tells each program its size again, so a full-screen one draws itself again, and redraws every screen. Tabs, splits, names, options, pane ids and what each Claude was doing are the same after; what was open on top (a menu, mission control, copy mode) is closed. `switchboard upgrade` prints how it went, and the key says so in a box.

The key builds first. A `switchboard` built from a clone knows the main clone of the checkout it was built in, and builds that rather than the checkout, since a worktree is removed once its work lands. It brings the main clone up to date first: when it isn't on its default branch (`origin/HEAD`, else `main`), has uncommitted changes, or has commits origin doesn't, the key says so and builds nothing; otherwise it fetches origin (while Settings' GitHub is on) and fast-forwards to it, and when the fetch fails (offline) or GitHub is off builds the clone as it is, checked against what it last fetched, and says it didn't fetch after the restart's message. Then it runs `cargo build --release --locked` there (cargo rebuilds only what changed, so with nothing new it takes a second) and, when the server runs a copy installed elsewhere (`~/.local/bin/switchboard`), puts the new build in its place. Cargo runs the checkout's own code, so, as with `switchboard try`, it builds only a checkout of yours or one listed in `~/.config/switchboard/trusted-dirs`, never a PR `wtpr --untrusted` set up. A release build has nothing to build, so the key takes the binary on disk as it is; `switchboard update` is how a new release gets there. In a `switchboard try` the key builds the worktree under test, as it is. A binary already put in place of the server's since it started (a build of another worktree installed by hand) is the latest, so the key restarts on it without building. Otherwise it restarts only when the binary at the server's path is no longer the one it started from, and otherwise says "Already on the latest build". A server running from a cargo target directory (`target/debug`, or one `CARGO_TARGET_DIR` shares between worktrees, whose builds and tests write over it) never restarts on that path: the key puts the build in `~/.local/bin/switchboard` when there is one, else in a copy at `~/.cache/switchboard/build/switchboard`, and restarts on that, or, when a build was installed in `~/.local/bin` since the server started, restarts on it as it is; a build that fails says why, and the server goes on as it was. Settings' "Build before restarting" (`build_before_restart`, on) off, the key builds nothing. `switchboard upgrade` never builds and always restarts.

Before it stops anything it asks the new binary whether it can take over (`switchboard server --handover-version`), and only one that can is started: a build from before this was possible, or one that hands over differently, is refused, saying why, and the server goes on as it was, as it does when anything else stops the restart. Notes, `M-d`'s diff and the views are programs in their panes, so they keep the build they started with until next opened; and a tab's shell keeps the environment it started with.

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
switchboard try --sealed [<dir>]
                             someone else's code, in a sandbox that holds
                             nothing of yours (macOS only)
switchboard try --sealed <dir> --run cargo test
                             a command in it, the PR's tests here
```

It builds the worktree with cargo (nothing to do when the build is up to date), then runs that build under a home of its own, `/tmp/sb-<hash>`: its own settings, state, server and socket, and its own Switchboard plugin, Claude Code statusline and permissions, which the build's `switchboard setup plugin claude claude-md skills` lays out there, so setup is tried too (with "Load Switchboard as a plugin" off in the settings it copies, its own `~/.claude` hooks, CLAUDE.md and skills instead). Its `~/.local/bin/switchboard` is the build, first on its PATH, so Claude Code's hooks and statusline in its panes reach the try's server. Running it again sets the home up again and keeps what the try saved. Attaching from one of Switchboard's own panes is refused, since the workspace around it would take every key: use a terminal of its own, `--print`, or `--detached`, which is for a Claude working in the worktree: it prints the prefix to put before `send-keys`, `capture-pane` and `list` to press keys in the try and read its screen.

What isn't Switchboard's comes from your real home. Linked: `~/Library/Keychains` on a Mac (without it Claude Code isn't signed in and the Keychain's secrets aren't found), your dotfiles (`.ssh`, `.gitconfig`, shell history), your `~/.config` folders but Switchboard's (gh's sign-in among them), `~/.local/share` but Switchboard's, the programs in `~/.local/bin` that aren't Switchboard's, Claude's plugins and its sign-in on Linux, and your own Claude skills and hooks. Copied: your shell's startup files (each ending by moving any XDG, notes or Switchboard path it set in your home to the same place in the try's), `~/.claude.json`, `~/.claude/settings.json` and `settings.local.json`, `~/.config/switchboard` and the clone your CLAUDE.md's rules come from, so nothing the try does writes to yours. Fresh: `~/.local/state`, `~/.cache`, Claude's sessions and transcripts, and `~/Notes`. Its settings turn off what would duplicate your workspace or spend on its own: the Slack look and view, meeting, standup and todo notes, banners, the update check and restoring saved tabs; turn any back on in its Settings. Linked things are yours, so an edit made through them (a git config, gh signing in again) is made to the real one, and keys sent in a try act with your real credentials.

Since its build and setup run with those credentials, it tries only code of yours: a worktree whose `.git` leads back to it, whose commits beyond its origin's branches are on no other remote and were written and committed under your git email, and which isn't partway through a merge, rebase, cherry-pick or revert. Anything else, such as a fork's pull request checked out with `wtpr --trusted`, is refused, saying why, until you have read it and listed its path in `~/.config/switchboard/trusted-dirs`. A PR checked out untrusted (`wtpr`'s default for a fork's) is tried only sealed, whatever `trusted-dirs` says.

#### Sealed

`--sealed` tries someone else's code, a PR `wtpr --untrusted` checked out, without any of it running outside a sandbox: macOS's own (`sandbox-exec`), which every process of the try runs in, the build, the build's own setup, its server and everything the server starts, and every client of it, since a client is the PR's binary too. Linux has no sandbox without installing another program, so there `--sealed` refuses, saying so; read the PR with `switchboard pr-risk` and review it with `switchboard codex-review -C`, and try it on a Mac. Inside the sandbox nothing can read your home but the worktree, its git directory and the toolchain (nor see `~/.cargo`, whose settings cargo would otherwise fail to read), or your per-user temporary and cache folders; write anywhere but the try's home and `/tmp` (not the worktree, its git directory, Claude's or tmux's folders of yours there, nor another try's home, nor a `.git`, `.cargo`, `CLAUDE.md`, `package.json` or the like in `/tmp` itself, which a program of yours run anywhere under `/tmp` would find and take up); reach the network or listen on it; connect to any socket but its own (your Switchboard's, ssh-agent's, the Notes app's and every Neovim's, which take commands from whoever connects); signal any process of yours outside it; send an Apple Event; or ask the Keychain, the clipboard, Launch Services (so no `open`) or the window server. Its rules are written by `switchboard try` into `~/.cache/switchboard/sealed/`, where nothing in the try can write, and given to `sandbox-exec` on each run.

Nothing of yours is in its home: a fresh `~/.config/switchboard` whose settings turn off every feature that reaches a service or Claude, an empty `~/.claude` (Claude isn't signed in there), `~/Notes` with a few made-up notes, and `~/code` with two made-up repositories, one with a branch to see in `M-d`. Its environment is made from nothing, so no token or `SSH_AUTH_SOCK` comes along. Since the try's code can change anything in its home, nothing outside the sandbox writes there once any of it has run: a try still running is only attached to, and any other is stopped, removed and made again from nothing, laid out beside where it goes and moved into place whole.

Building runs the PR's code (its `build.rs` and proc macros), so cargo builds inside the sandbox, offline and locked, into the try's home, with your main clone's toolchain, run directly so the PR's `rust-toolchain.toml` picks nothing. The crates are fetched first, outside the sandbox (`cargo fetch` runs none of their code), into `~/.cache/switchboard/sealed/cargo`, only from crates.io and each with its checksum in `Cargo.lock`: a git, path or other registry's dependency, a `[patch]` or `[replace]`, or a `Cargo.lock` not as cargo writes it is refused, since cargo would fetch from wherever the PR says. When its `Cargo.lock` differs from your main clone's, only `--fetch` fetches it, once you have read the change. The build's output is shown with its control characters written out, as `pr-risk` shows the PR's lines; what the try draws once you attach, and what `--run`'s command prints, reach your terminal as any program's do. A try is built again each time it is made again, so a sealed try's first start takes a full build.

`--print` prints the command that attaches to it, sealed, for another terminal, and `--detached` starts its server with no client (a session made in it by Switchboard's own binary in a sandbox that reaches that socket alone) and prints the prefix that drives it, the try's binary in the sandbox. `--run <command>...` runs a command there from the worktree and exits with its status, the PR's tests for one (`--run cargo test`), with localhost (any service listening there, yours included) and the sockets in a folder under `/tmp` named as the tests name theirs allowed too. A `--run` again on the same worktree takes up the home the last one made, its crates and build kept, while its `Cargo.lock` is the same; otherwise the home is made again from nothing. `--clean` stops its server from the same small sandbox, removes its home (a link the try put in its place removed as a link) and its rules. It runs a worktree `wtpr --untrusted` made, one of your own, or one listed in `trusted-dirs`.

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
| `repos.json` | Each repository's own settings by its origin's `owner/name`: how its work lands, through a PR or straight on its default branch, a direct landing's tests and where they run (here or in CI), squash, review and command before the push, and the `apply` list of what a change landed there needs to take effect | "Landing: `/queue-and-clean`" |
| `leak-check.txt` | Your organization's and company's names, usernames and internal hosts, one a line (blank lines and `#` lines left out), which `switchboard leak-check` finds, as whole words in any case, in what a landing would push to a public repository | "Landing: `/queue-and-clean`" |
| `trusted-dirs` | The directories whose own code may run, one a line: `switchboard try` builds a worktree holding someone else's commits only once it is listed, and `/queue-and-clean` offers to run a landed repository's install or migrate script only in one listed; a PR checked out untrusted stays untrusted when listed | "Trying a change" |
| `secrets.json` | On Linux with no keyring, the secrets `switchboard platform secret-set` stores, readable only by you | "GitHub and Linear access" |

What Switchboard keeps for itself is in `~/.local/state/switchboard/` (the socket, the saved layout, the tips shown, what it counted, the day's files) and its caches in `~/.cache/switchboard/` and `~/.cache/wt-revive/` (what each Claude transcript says of its worktree, for `wt-revive`); the Slack relay's state is in `~/.local/state/slack-relay/`, and `/oncall`'s in `~/.local/state/oncall/`. `~/.local/share/switchboard/` holds what setup writes for Claude Code: Switchboard's plugin (`plugin/`, with `.switchboard-plugin` listing what was written, and what a skill installed for itself beside its scripts), Switchboard's part of your CLAUDE.md when that imports it (`switchboard.md`), the clone of your rules' repository (`claude-md/`) and the build whose parts were last put in place (`setup-build`). None of it is yours to edit, and any of it can be deleted while the server is stopped, at the cost of what it held.

## Every setting

Mission control's Settings view (`M-p`, then "Settings: turn the workspace's options on and off") lists every option below under its category; Enter or Space changes one, and `/` filters them. A new feature comes with a setting, on unless it sends something of yours somewhere, so each part can be turned off for a plain workspace. `switchboard setting <key>` prints one's value, `switchboard setting <key> <value>` changes it (`on` or `off`, or one of its choices; anything else is refused), and `--reset` gives it its default back.

### Integrations

| Setting | Default | What it does |
|---|---|---|
| GitHub (`github`) | on | PR commands in M-p, the PRs view, PR numbers and merge states in tabs and the Work map; off, the workspace asks GitHub nothing by itself (programs you run, such as codex-review and wtpr, still do) |
| Linear (`linear`) | on | Ticket commands in M-p, ticket titles in tab names and tickets in the Work map; off, the workspace asks Linear nothing by itself (programs you run, such as wt and worktree-done, still do) |
| Cache PR and ticket images (`image_cache`) | on | Keeps the pictures, stylesheet, scripts and drawn pages of the PR and ticket image views on this computer, up to 300 MB, so one opened again shows at once; off, everything is fetched and drawn each time |
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
| Sidebar repository filter (`sidebar_repo_filter`) | on | M-p's Filter the sidebar to a repository moves every other repository's tabs to an Other repos group at the bottom, whatever they are doing, until Show every repository's tabs clears it; the filter is kept across restarts. Off, the commands are gone and every tab is in its own group |
| Reminders (`reminders`) | on | M-p's Remind me about this tab: the tab waits under Reminders with the time left, then moves to Needs you with a banner when it comes due; off, reminders are neither set nor shown |
| Build failing (`build_failing`) | on | A Build failing group in the sidebar and mission control for the tabs whose open PR has a failing check, asked of GitHub every minute, an idle one wearing a red ✗; off, they stay in Working or Idle with the failing count in their words #github |
| Show the landing queue (`landing_queue`) | on | Tabs landing on the default branch with queue-and-clean, or waiting their turn, get a Landing group in the sidebar and mission control, in the landing queue's order, which a waiting tab's menu changes; off, they show as Working, and the queue still orders the landings |
| Notifications (`sidebar_notifications`) | on | The Notifications block at the bottom of the sidebar: new Slack messages and your Linear inbox, which a click opens in mission control |
| Notifications' history (`notification_history`) | on | Mission control's Notifications view keeps every notification of the last two weeks by day, with what each led to and whether the sidebar still shows it, in ~/.local/state/switchboard/notifications on this computer; off, the view shows only the sidebar's notifications, as they are now, and nothing is kept |
| PR notifications (`pr_notices`) | on | The sidebar tells you when one of your PRs merges, is approved or its checks start failing, from the PRs board's lookups; off, it doesn't |
| M-a goes to notifications (`needs_notices`) | on | M-a's walk of what needs you ends at the notifications while there are any, after the tabs, opening them as the sidebar's n does |
| Notifications window (`notice_list`) | on | Moving the sidebar's cursor onto a notification, or clicking one, brings the Notifications window to the front as moving onto a tab does: a tab of its own listing every one with its whole message, Enter opening its menu and Esc going back, as M-p's "Go to the notifications" does too; off, Enter or a click on one in the sidebar does what it leads to |
| New notes (`new_notes`) | on | In the sidebar's notifications, a note written for you (a meeting's from Fathom, the standup) until you read it, Enter offering to open it in Notes |
| Next meeting (`next_meeting`) | on | In the sidebar's notifications, from five minutes before a meeting until ten after it starts: its name, the tickets and PRs it mentions, and Enter or a click to join its call |
| Calendar changes (`calendar_changes`) | on | In the sidebar's notifications, what changed on today's calendar since you last looked at it: an event added, moved, renamed or gone; looking at the Calendar view clears them |

### Sessions

| Setting | Default | What it does |
|---|---|---|
| Autopilot (`autopilot`) | on | With it on, M-p's 'Autopilot: work a note' keeps tabs working through a note's checklist, opening the next item with wtn as each lands; the mode itself starts off until you turn it on there. Off, it opens no tabs, and its line in mission control and its notification go |
| Autopilot's push allowed in auto mode (`autopilot_push_rule`) | on | A tab autopilot opens in a repository that lands straight on a branch starts its Claude with the landing's one push, git push origin HEAD:<branch>, allowed for that session, so Claude Code's auto mode doesn't refuse it; every other push and tab still asks. Off, the push is judged as any command is, and a tab landing without asking may stop at it |
| Watch the default branch's CI (`main_ci_watch`) | on | While autopilot works on a repository, or a tab waits to land on it, and its landings test in CI, Switchboard looks at its default branch's CI every two minutes and tells you when it fails, naming the commit and the line of the log that says why. Off, nothing looks, and you hear of a failing default branch only from the next landing |
| Autopilot's hands-free mode (`autopilot_handsfree`) | on | Lets autopilot's menu offer 'Hands-free', which has its tabs take the recommended option of what they would ask you (a question box, a question that ends the turn, an item whose plan needs a choice), write each decision into the item's plan as a 'Decided hands-free' line, and land without asking; what your rules or a skill say must be asked still asks. The mode itself starts off. Off, the menu doesn't offer it and every tab asks as usual |
| Close a worked note's landed tabs (`autopilot_close`) | when seen | When a tab autopilot opened closes once queue-and-clean has landed it and moved it back to the main clone: when seen, as soon as you leave it after seeing it, or 30 minutes after landing if you never look; at once, as soon as it lands; after 5 minutes, or sooner once seen and left; never, left open for you to close. Whatever the choice, a tab in front stays until you leave it and a prompt typed in it after landing keeps it open; one closed without being seen leaves a notification |
| Keep the computer awake while tabs work (`keep_awake`) | on | While a tab's Claude works (background work and watches too, not a /loop between its runs) or autopilot, on and not paused, has tabs open, the computer doesn't sleep when idle (caffeinate on macOS, a logind inhibitor on Linux), shown by a ☕ on the status line, or with UX 2.0 at the top of the sidebar; it lets go two minutes after everything is idle, the display still sleeps, and a laptop closed with no display on it sleeps anyway. M-p's 'Keep the computer awake until I say' holds it whatever runs. Off, the computer sleeps on its own schedule unless held from M-p |
| wtpr asks where the PR stands (`wtpr_prompt`) | on | Claude, started in a PR's worktree by wtpr (or mission control's "work on it in a new tab"), first reads the PR, its review threads, failing checks and ticket and says where it stands and what it would do next, changing nothing until you say; off, it starts with no prompt |
| Copy Claude's reply (`copy_reply`) | on | M-y copies the latest reply of the tab's Claude to the clipboard, read from its session transcript; off, the key reaches the pane |
| Log Claude's hooks (`hook_log`) | off | Write a line to the server's log (server.log in Switchboard's state directory) for each hook Claude Code runs in a pane: the event, the tool, the pane, the call's id, the subagent and a notification's kind, never what was asked or run; for finding out why a tab showed the wrong state. Off, nothing is logged |
| Offer to apply a landed change (`apply_after_landing`) | on | After queue-and-clean lands work, it lists what the change needs to take effect (Switchboard built and put in place, the Slack relay restarted, an install, a migration) and offers to do it; a tab in autopilot's hands-free mode applies none of it and says so in its landing's notification |
| Save what panes show (`save_contents`) | on | Keep each pane's screen in the layout saves, to show again after a restart; it can hold secrets |
| Restore on start (`auto_restore`) | on | Bring back the last saved tabs and panes, each Claude resumed, when the server starts with none open |
| Load Switchboard as a plugin (`claude_plugin`) | on | Claude in Switchboard's panes loads its hooks (the tabs' Claude states), skills and instructions as a plugin, and setup keeps them out of ~/.claude, so Claude elsewhere has only your own; it needs Claude Code 2.1.280 or later, and managed settings that don't set disableSideloadFlags. Off, setup puts them in ~/.claude as before, for every Claude of yours. Changing it moves them over |
| Context in Claude's status line (`statusline_context`) | on | Claude Code's status line under its prompt says how full the context is, beside the model, inside the workspace as outside it. Off, with UX 2.0 and mission control's Sessions view on, the status line inside the workspace shows the model alone and the Sessions view shows the context; without both of those it shows the context anyway |

### Diff

| Setting | Default | What it does |
|---|---|---|
| Files in reading order (`diff_order`) | on | M-d lists the changed files flat, documentation first and tests last, with Haiku ordering the code from most abstract to most concrete; off, flat, A to Z |
| Diff told as a story (`diff_story`) | on | M-d's file list grouped into a few chapters Haiku names from the whole diff and the commits' messages, each with a heading and two or three sentences on what it does and why it is built so, read from the change's core to its edges ([c and ]c move between them, yS copies the story as Markdown); off, the files keep the reading order by kind. Sends the diff's text and the commit messages, not only the paths, to Claude |
| Diff story progress (`diff_story_progress`) | on | While M-d's story is told, its cover shows what Haiku is reading, each chapter as Haiku names it and the time taken, t showing Haiku's answer itself as it comes; off, a spinner alone |
| Diff story told ahead (`diff_story_ahead`) | on | When a tab's branch has a new commit, its M-d story is told in the background, so M-d opens on it at once rather than waiting under its cover; sends that branch's diff and commit messages to Claude (Haiku) for each new commit, a run taking up to five and a half minutes |
| Uncommitted changes in M-d (`diff_uncommitted`) | on | M-d's diff runs from the PR's base to the working tree, marking files with changes not yet committed and listing them in a last chapter; off, it shows the branch's commits only |
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
| Resources (`view_resources`) | on | Mission control's Resources view: the computer's CPU and memory, and a table of the CPU and memory each tab's processes use, sorted by CPU or memory, read from the kernel every few seconds only while the view is in front; with UX 2.0 a board of sparklines and each tab's process tree, whose menu can end a process. Off, the view and its readings are gone |
| Requests in Usage (`request_audit`) | on | The Usage view lists today's requests to services outside this computer (GitHub, Linear, Slack, Anthropic and the rest), by protocol and by service, as Switchboard counts them; off, it leaves them out |
| Usage (`usage`) | on | Mission control's Usage view: today's Claude spend and how much of the rate limits is used |
| Keys (`view_keys`) | on | Mission control's Keys view, which M-/ opens: every key Switchboard binds, the workspace's grouped by their first tag, then the sidebar's, mission control's, M-d's and Notes', then the commands reached only through M-p, with / to filter, Enter to press a workspace key and ? to find a key by pressing it; with UX 2.0, a board with a heatmap of the Alt keys you pressed this week (counted with habits on); off, M-/ reaches the pane |
| RSVP from the Calendar (`calendar_rsvp`) | on | Answer an invitation yes, maybe or no from its Enter menu in the Calendar view, through Claude's Google Calendar connector |
| Slack without a tab (`slack_background`) | on | Mission control looks at Slack itself every minute, through cheap headless Haiku calls, instead of a Slack tab looping /slack-updates |
| Pause lookups while away (`pause_when_away`) | on | Stop asking GitHub, Linear and the calendar for mission control's views, and telling the diff's stories ahead, while no terminal is attached, or the computer has been locked, or idle 10 minutes, with no key pressed in Switchboard since (from another computer over SSH too), and catch up on return |
| Look often (`poll_fast`) | on | Ask GitHub and Linear every 15 seconds (the PR board every 30) and look at the Slack Home tab every 10, in place of every one or two minutes |
| PR board (`pr_board`) | on | Mission control's PRs view, which asks GitHub about your pull requests every two minutes |
| Ticket board (`ticket_board`) | on | Mission control's Tickets view, which asks Linear about the issues assigned to you every two minutes |
| Inbox (`view_inbox`) | on | Mission control's Inbox view and the sidebar's count of it: your unread Linear mentions, comments and assignments, asked of Linear every two minutes #linear |
| CI (`view_ci`) | on | Mission control's CI view: your pull requests failing, running and passing, each failed check with the line of its log that says why and, with UX 2.0, its failing step beside the list, asked of GitHub every two minutes #github |
| Autopilot view (`view_autopilot`) | on | With UX 2.0 on, while autopilot works a note, mission control shows a view of it: the note, its tabs working and open against its slots and cap, today's spend against its limit, and a board of every item it works, a card in the column where it stands (Next up, Working, Waiting on you, Landing, Landed, Skipped). Off, its line above the tabs stays and the view goes; on, the view takes the line's place |
| Landing view (`view_landing`) | on | While a tab is landing or waiting to land, mission control shows each repository's landing queue or merge queue (everyone's PRs in it, asked of GitHub every 15 seconds), in queue order, with the tab that has each; a waiting one's menu moves it or takes it out |

### Tabs and status bar

With UX 2.0 on, this category is named "Tabs and header".

| Setting | Default | What it does |
|---|---|---|
| M-A cycles idle tabs (`idle_walk`) | on | M-A goes to the next idle tab, in the sidebar's Idle order, wrapping at the end, as M-a walks the tabs that need you; off, M-A reaches the pane |
| Tab history (`tab_history`) | on | M-h shows a tab's history: its Claude session on a timeline (its state, your turns, commits, review rounds and context) under its screen as it was at the moment you move to, the screens kept in the server's memory only, never on disk, and the rest read from the session's transcript; off, nothing is kept, M-h says it is off and the header has no History key |
| One Claude to a tab (`one_claude_per_tab`) | on | A Claude session started in a tab already running one moves to a tab of its own, since the sidebar, the Slack relay, M-c and M-m each take a tab's Claude to be one pane |
| Name new tabs (`name_new_tabs`) | on | M-t and M-n ask what to call the tab they open: a name typed in, its ticket and the ticket's title, or shaping a ticket, each optional; off, the tab is named after its directory |
| Show a tab's full title (`tab_title`) | on | M-T, the sidebar's i and a tab's menu in mission control show the tab's whole title in a box, with its branch, directory, PR and ticket, which the sidebar and the status line cut; off, the key reaches the pane |
| Messages in a box (`message_boxes`) | on | What a key or script has to say shows in a small box in the middle of the screen, which any key closes, rather than on the status line for a moment |
| Opening-link message (`link_opening_message`) | on | Opening a link says where it is going (the site, Chrome and its profile) in a box until the browser comes to the front, since a profile's Chrome can take seconds to start. Off, links open with nothing said |
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
| Images drawn in a note (`notes_images`) | on | An image pasted into a note (Format menu, Paste image) drawn under the line that names it, in a terminal that speaks the kitty graphics protocol (Ghostty and kitty do), or one that draws sixel with Image protocol on sixel (Windows Terminal); off, it is its name, which Enter opens as "Enter shows a picture full size" says |
| Enter shows a picture full size (`picture_full_size`) | on | Enter or a click on a note's picture shows it full size over the notes pane, as it does a picture or diagram under the cursor in the PR, ticket, link and file views, drawn by the workspace in a terminal that speaks the kitty graphics protocol (Ghostty, kitty) with "Images in the workspace" on, Esc going back; off, or where the terminal can't, it opens in Quick Look or the desktop's image viewer |
| Toolbar (`notes_toolbar`) | on | A row of buttons over each note, one for each of the Format menu's styles (bold, italic, headings, lists, checklist, table, link, image) and ⋯ for the whole menu, each acting on the line or the selection; off, the Format menu is Ctrl-P alone |
| Tell a note's new tab its note (`note_tab_prompt`) | on | A new tab opened for a note from Notes starts Claude knowing the note's title and file (added to its system prompt, no turn spent), to read when you ask, or first while "Point Claude at the tab's note" is on; off, it starts as any other tab |
| Notes beside a note's tab (`note_tab_beside`) | on | Opening a note's tab from Notes (t, or M-p's Open the tab for the note) moves Notes beside the tab's Claude showing the note, as M-e does; off, it only goes to the tab |
| Show a note's item whole in wtn's list (`wtn_item_preview`) | on | M-p's "Start work on a note's checklist items" shows the item under the cursor whole in the lower half of its list, its full text and what is indented under it, since the list cuts each item to one line; off, the list has the whole box |
| Ask for a tab's note (`ask_tab_note`) | on | When M-e brings Notes beside a tab with no note of its own yet, ask which note is the tab's, the likely ones first, and show that note beside it from then on |
| Point Claude at the tab's note (`tab_note_context`) | off | As a Claude session starts in a tab with a note of its own, tell it the note's path and to read it before its first reply, so the note's standing rules reach every session in the tab, in any repository, while Switchboard loads as a plugin (claude_plugin), whose session-start hook does it; off, a session reads the note only when asked to |

### Appearance

| Setting | Default | What it does |
|---|---|---|
| Images in the workspace (`server_images`) | on | Draws avatars, pictures, meters, charts and the shadow under boxes as images where the terminal can show them (Ghostty, kitty). Off, they are drawn with characters |
| People's photos (`photos`) | on | Avatars show people's photos from Slack (through the Slack relay's app, with the users:read scope), Linear and GitHub, each fetched once and kept in ~/.cache/switchboard/photos for 30 days. Off, nothing is fetched and an avatar is the person's initial |
| A cursor in the views (`view_cursor`) | on | Gives the PR, ticket, link and file views a cursor, a yellow › that j and k move from line to line, a picture or diagram one step, the screen scrolling to keep it in sight; Enter or a click on a picture or diagram shows it full size over the view, as a note's does, Esc going back. Off, the views scroll as less does, and a click on a picture opens its link |
| Pictures and diagrams in the views (`view_images`) | on | Draws the pictures on lines of their own and the Mermaid diagrams in the PR, ticket, link and file views as images where the terminal can show them (Ghostty, kitty), fetching the pictures and drawing the diagrams with Mermaid in Chrome, and draws the diagrams of the description beside mission control's PR and ticket boards the same way (with "Images in the workspace" on too). Off, a picture is its name and a diagram is drawn with characters |
| UX 2.0 (`ux2`) | off | No status line: each tab has a header naming it, with the keys of what it can show in Claude's place (M-c Claude, M-d Diff, M-v PR, M-i Ticket, M-e Note); the sidebar starts with its notifications; M-v and M-i show the PR and the ticket in Claude's place rather than beside it, laid out as their pages are with a panel of their facts beside the description; and M-d's diff reads as one scroll of files under its story's chapters, the chapter's why over the diff and each hunk's note in a card under it ([UX 2.0](#ux-20)). Off, the status line, the sidebar and those keys are as before |
| Theme (`theme`) | dark | The status line's, the pane borders' and the boxes' colors; system follows the terminal's own background, which it is asked for as it attaches |
| Image protocol (`image_protocol`) | kitty | How pictures (a note's images, the PR and ticket image views) reach your terminal: kitty passes kitty's graphics commands on, for Ghostty and kitty; sixel has the workspace draw each picture itself as sixel where it shows, for a terminal that draws sixel but not kitty's pictures, as Windows Terminal (1.22 and later) does ([Pictures in Windows Terminal](#pictures-in-windows-terminal)); Notes and the image view opened after a change draw by it |

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
| Links you gave Claude (`prompt_links`) | on | M-p's link commands also list the links in what you sent the pane's Claude, pasted text included, which Claude Code doesn't show. Off, they list only what the pane shows |
| Commands button (`palette_button`) | on | A ⌘ Commands button that opens M-p when clicked: in the status bar after the session name, or with UX 2.0 at the right end of the tab header's keys, where a click while the palette is open closes it |

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
| Offer to restart after an update (`upgrade_after_update`) | on | Once switchboard update has put a new release in place, ask whether to restart the workspace on it now, keeping every tab and what runs in it; off, it only says how (switchboard upgrade) |
| Build before restarting (`build_before_restart`) | on | M-p's "Restart Switchboard on the latest build on disk" first brings the main clone of the checkout a build from a clone came from up to date with origin, on its default branch with nothing uncommitted, and builds it (cargo build --release --locked; a try builds its worktree) and puts it in place of the binary the server runs, restarting only when that is new (from a cargo target directory, which other worktrees write over, the build goes to ~/.local/bin/switchboard, else a copy in ~/.cache/switchboard/build, and the restart runs from there); off, it restarts on the binary on disk as it is, when that is new |

### Slack relay

| Setting | Default | What it does |
|---|---|---|
| Haiku summaries (`haiku_summaries`) | on | One line on what each tab is doing in the relay's Home tab and progress, each a paid Haiku call |

## Commands for scripts

Scripts, hooks and skills act on the running workspace through `switchboard <command>`, much as they once called tmux. These commands never start a server; with none running, they fail. Each acts on a target given as `-t %<pane>` or `-t @<tab>` (as `switchboard list` and `switchboard tabs` show them); without one, on the pane the command runs in (its `SWITCHBOARD_PANE`), else the pane in front on the terminal used last. A tab given where a pane is wanted means its active pane, and a pane given where a tab is wanted, or for a tab's option with `show` and `set` (`@tab_label`, `@tab_name` and the other `@tab_` ones, autopilot's `@autopilot` ones, and `@agent`), means its tab. A directory given with `-c` is read from where the command runs, `~` included.

```text
switchboard list                         sessions, tabs and panes, as JSON;
                                         each tab's `stage` is the sidebar's
                                         group for it (key, title, the
                                         group's rank, its place in it)
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
switchboard capture-pane [-t pane] [-S lines] [-D]
                                         the pane's screen, with that many lines
                                         of history above it; -D leaves what is
                                         drawn dimmed blank, as hints are
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
switchboard picture [-t pane] <file>     show a picture full size over the
                                         pane, Esc going back; fails where
                                         the terminal draws no images
switchboard tip <name> | tip seen <name> | tip reset
                                         show a tip once, mark it shown, or
                                         show every tip again
switchboard press <action> [arg]         run an action as its key would, by its
                                         name in keys.json (only note-tab takes
                                         an arg, the note; work-on-item, what
                                         Notes' gw presses, takes the note and
                                         the item's line; work-note, Notes'
                                         M-p, the note; plan-item, M-Enter in
                                         Notes' Add an item, the note and the
                                         item)
switchboard sidebar-repo [-s session] <dir> | --all
                                         filter the sidebar to the repository
                                         dir is in, or clear the filter
switchboard sources [refresh [view]]     mission control's GitHub and Linear
                                         data as JSON, or looked up again now
switchboard list-keys                    every key and what it does
switchboard save                         save the layout now; prints its file
switchboard detach [-s session]          detach every terminal, or a session's
switchboard kill-server                  save, then end the server and its panes
switchboard upgrade                      restart the server on the build installed
                                         now, keeping every pane, its program and
                                         the terminals attached
```

`set` and `show` without `-t` put an option whose name starts with `@tab_` or `@autopilot`, or is `@agent`, on the caller's tab, and any other on its pane. A few are the workspace's own: `@tab_name` names the tab; `show` gives `@tab_label`, the name the tab bar shows, PR number included, `@tab_path`, the directory it is named after, and `@agent`, the tab's most urgent state. On a pane, `@agent_state` and `@agent_since` are Claude's state there and since when (seconds), `@agent_activity` its tool call in a few words, `@agent_wait` what it waits on, `@agent_loop` the loop it runs (review, ci, queue or deploy) and `@agent_phase` the step a loop says it is at, which `land-lock` sets as it lands on the default branch.

```sh
# A tab in the background for a build, its pane's id kept for later
read tab pane < <(switchboard new-tab -d -n build -c ~/code/app)
switchboard send-keys -t "$pane" 'make test' Enter
switchboard capture-pane -t "$pane" -S 200
```

A pane closes when its program ends, so a command given to `new-tab` or `split` takes its pane with it when it finishes; type it into the shell, as above, to keep what it printed. Each of these says how it is used with `--help` after its name.

Five more commands are programs of their own:

- `switchboard tabs` prints a line per pane of every tab, tab-separated: the tab's number, the pane's place (`main:@3.%12`), the tab's name as the tab bar shows it, Claude's state there (blank when idle), the program in front, and the directory; then, where the pane runs Claude, its session ID, its process ID, the name its row in Claude Code's `ListAgents` shows, and its address (`uds:` and its messaging socket), which `SendMessage` takes as its `to`, blank where it runs none. Claude reads it to see what the other tabs are doing and to reach another tab's Claude. `--json` prints the same as an array of objects (`tab`, `pane`, `name`, `state`, `program`, `dir`, `session`, `pid`, `agent`, `address`, a blank one null), for scripts.
- `switchboard respawn [-t pane] [-c dir] <command> [args...]` replaces a pane's program with another in the same pane, which keeps its id, place and tab, and leaves a shell there when it ends. `wt`, `wtn`, `wtb` and `wtpr` run as the worktree commands' shell functions. The arguments are words of their own, never read by a shell. Skills use it to hand a tab over to the work it opens.
- `switchboard landing [<dir>]` says how a repository lands its work, through a PR or straight on its default branch, and `switchboard apply-actions <repo> <from> <to>` prints what a landed change needs to take effect, and `switchboard apply install <repo>` or `switchboard apply relay` does Switchboard's part of it ("Landing: `/queue-and-clean`").
- `switchboard setting <key>` prints one of Settings' values as it stands, `on` or `off`, or the choice (`switchboard setting update_check` prints `hourly`); `switchboard setting <key> <value>` changes it as Settings would, and `switchboard setting <key> --reset` gives it its default back.
- `switchboard platform <what>` does what differs between macOS and Linux, for shell scripts: `name`, `secret <service> [-a account]`, `secret-set <service> [-a account]` (the secret read from stdin), `copy` (stdin to the clipboard), `paste`, `notify <title> <text>`, `presence` (idle milliseconds and whether the screen is locked), `open <file or link>`, `front-app` and `chrome-local-state`.

`switchboard help` lists every command, and `switchboard --version` prints the version and the commit it was built from.
