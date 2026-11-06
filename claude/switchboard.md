# Switchboard

What every Switchboard user's Claude Code needs to know about the workspace. Your own ~/.claude/CLAUDE.md imports this file (setup.sh writes the import), so an update brings changes to it.

# Notes

- "Notes" or "my note" means my terminal notes app (the Notes window in tmux, nvim with `switchboard/notes`), never macOS Notes. Notes are Markdown files in `~/Notes/`, named after their title (e.g. `~/Notes/Weekend Plans.md`); strike through with `~~text~~`. A note's first line is its title, `# Title`. Checklists are `- [ ]` and `- [x]`; ticking an item moves it under the unticked ones in its list, with what is indented under it, as the app does, so a list leads with what is left, one blank line between the unticked items and the ticked ones. Folders are subfolders of `~/Notes`; `.trash/` holds deleted notes, so leave it out of searches. Edit the file directly: the open app takes in changes without saving over them.
- Working through a note's checklist, take the next unticked item from the top. In a note grouped into `## <feature>` sections, new items sit ungrouped at the top: when ticking one, move it, with what is indented under it, into the section it belongs to (or a new `## ` section when none fits), under that section's unticked items. Skip any section named for parked work, such as `## Defered` or `## Later`.

# My tabs

My work runs in tmux tabs, most of them a Claude session in a worktree of their own, so much of what is in flight is in a tab rather than in git or Linear. When an answer could depend on work that may be under way in another tab, look before answering, with this one command (it reads tmux only, so it costs nothing to run):

```
tmux list-panes -a -f '#{&&:#{!=:#{@sidebar},1},#{!=:#{@pane_parked},1}}' -F '#{window_index}	#{session_name}:#{window_id}.#{pane_id}	#{@tab_label}	#{@agent_state}	#{pane_current_command}	#{pane_current_path}'
```

One line per pane: the tab's number, the pane's place in tmux (the `main:@<window>.%<pane>` a Claude session's `ListAgents` row names), its name (with the PR number and the Linear ticket and title when it has them), Claude's state there (`waiting` on me, `done` and not yet seen, `working`, blank for idle), the program (a version number is Claude Code), and its directory, a worktree's naming its branch. Some cases where it matters, as examples of the kind of question, not a list of the only ones:

- The status of a project or ticket: which of its tickets are being worked on in a tab right now, and which of those wait on me.
- Starting work on a ticket, branch or PR: whether a tab already has it, so it isn't started twice.
- What to do next, or what is waiting on me: what is in flight and where.

Don't look on turns that can't depend on it. Reading another tab's screen or typing into it is a different thing: only when I ask.

## Talking to another tab

To reach another tab's Claude, use `ListAgents` and `SendMessage`, never `tmux send-keys` into its pane. Find the tab's session by matching the place its `ListAgents` row names (`tmux main:@<window>.%<pane>`) to that column of the command above, which gives the tab's number. Send only when I ask, or a skill says to, as with reading its screen. Write the message so it stands alone, since that session sees nothing of this conversation, and report that it was delivered, not that it was done: an answer comes back as a message of its own.

## Working in ~/switchboard

Several tabs may work in `~/switchboard`'s main clone at once, and every link `setup.sh` made points into it, so a change there reaches them all as soon as it is on disk. Before committing there, check `git status` for changes that aren't yours and leave them out. A new script in `tmux/` or `bin/` needs `./setup.sh tmux bin` before other tabs can use it.

To name a tab, set its `@tab_name` and have the tabs named again; never `tmux rename-window`, which the workspace names over on the next focus. Leave out the PR number, which the tab adds itself:

```
tmux set-option -w -t "$TMUX_PANE" @tab_name 'Review loop: settings page layout' &&
  ~/.config/tmux/window-names.sh "$(tmux display-message -p -t "$TMUX_PANE" '#{session_name}')"
```

# Skills

- When a skill exists both in the repository (`.claude/skills/<name>/`, in the worktree or its main clone) and as my personal one, use the repository's: read its SKILL.md and follow it instead of the personal one. `skill-path <name>` prints which copy to follow, the repository's or the personal one, and a skill handing off to another reads the file it prints.
