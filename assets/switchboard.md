# Switchboard

What every Switchboard user's Claude Code needs to know about the workspace. Switchboard's plugin gives it to each Claude session in Switchboard's panes as it starts (or, with the plugin off, your own ~/.claude/CLAUDE.md imports it, as `switchboard setup` writes it), so an update brings changes to it.

# Notes

- "Notes" or "my note" means my terminal notes app (the Notes tab in Switchboard), never macOS Notes. Notes are Markdown files in `~/Notes/`, named after their title (e.g. `~/Notes/Weekend Plans.md`); strike through with `~~text~~`. A note's first line is its title, `# Title`. Checklists are `- [ ]` and `- [x]`; ticking an item moves it under the unticked ones in its list, with what is indented under it, as the app does, so a list leads with what is left, one blank line between the unticked items and the ticked ones. Folders are subfolders of `~/Notes`; `.trash/` holds deleted notes, so leave it out of searches. Edit the file directly: the open app takes in changes without saving over them.
- Working through a note's checklist, take the next unticked item from the top. In a note grouped into `## <feature>` sections, new items sit ungrouped at the top: when ticking one, move it, with what is indented under it, into the section it belongs to (or a new `## ` section when none fits), under that section's unticked items. Skip any section named for parked work, such as `## Defered` or `## Later`.

# My tabs

My work runs in Switchboard's tabs, most of them a Claude session in a worktree of their own, so much of what is in flight is in a tab rather than in a repository or Linear. When an answer could depend on work that may be under way in another tab, look before answering, with this one command (it asks the workspace's server only, so it costs nothing to run):

```
switchboard tabs
```

One line per pane, tab-separated: the tab's number, the pane's place (`<session>:@<tab>.%<pane>`), its name (with the PR number and the Linear ticket and title when it has them), Claude's state there (`waiting` on me, `done` and not yet seen, `working`, blank for idle), the program (a version number is Claude Code), and its directory, a worktree's naming its branch. Some cases where it matters, as examples of the kind of question, not a list of the only ones:

- The status of a project or ticket: which of its tickets are being worked on in a tab right now, and which of those wait on me.
- Starting work on a ticket, branch or PR: whether a tab already has it, so it isn't started twice.
- What to do next, or what is waiting on me: what is in flight and where.

Don't look on turns that can't depend on it. Reading another tab's screen or typing into it is a different thing: only when I ask.

## Talking to another tab

To reach another tab's Claude, use `ListAgents` and `SendMessage`, never `switchboard send-keys` into its pane. Find the tab's session by matching its `ListAgents` row's directory to the last column of the command above, which gives the tab's number. Send only when I ask, or a skill says to, as with reading its screen. Write the message so it stands alone, since that session sees nothing of this conversation, and report that it was delivered, not that it was done: an answer comes back as a message of its own.

## Naming a tab

To name a tab, set its `@tab_name`; never `switchboard rename-tab`, whose name stays when `/wt` clears `@tab_name` to hand the tab its ticket's. Leave out the PR number, which the tab adds itself:

```
switchboard set @tab_name 'Review loop: settings page layout'
```

# Skills

- When a skill exists both in the repository (`.claude/skills/<name>/`, in the worktree or its main clone) and as my personal one or Switchboard's (`/switchboard:<name>`), use the repository's: read its SKILL.md and follow it instead. `switchboard skill-path <name>` prints which copy to follow, the repository's or the personal one, and a skill handing off to another reads the file it prints.
