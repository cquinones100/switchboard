# Switchboard

What every Switchboard user's Claude Code needs to know about the workspace. Switchboard's plugin gives it to each Claude session in Switchboard's panes as it starts (or, with the plugin off, your own ~/.claude/CLAUDE.md imports it, as `switchboard setup` writes it), so an update brings changes to it.

# Notes

- "Notes" or "my note" means my terminal notes app (the Notes tab in Switchboard), never macOS Notes. Notes are Markdown files in `~/Notes/`, named after their title (e.g. `~/Notes/Weekend Plans.md`); strike through with `~~text~~`. A note's first line is its title, `# Title`. Checklists are `- [ ]` and `- [x]`; ticking an item moves it under the unticked ones in its list, with what is indented under it, as the app does, so a list leads with what is left, one blank line between the unticked items and the ticked ones. Folders are subfolders of `~/Notes`; `.trash/` holds deleted notes, so leave it out of searches. Edit the file directly: the open app takes in changes without saving over them.
- Each tab has a note of its own, the one `M-e` opens: `switchboard show @tab_note` prints its file name in `~/Notes/`, else `switchboard show @tab_note_last` does. "Our note", "the associated note", "this tab's note" or "the note" with no name means that one. Before writing to a note I haven't named, run both and write to the one they print, rather than to a note named earlier in the conversation; ask when both print nothing.
- Working through a note's checklist, take the next unticked item from the top. In a note grouped into `## <feature>` sections, new items sit ungrouped at the top: when ticking one, move it, with what is indented under it, into the section it belongs to (or a new `## ` section when none fits), under that section's unticked items. Skip any section named for parked work, such as `## Defered` or `## Later`.

# My tabs

My work runs in Switchboard's tabs, most of them a Claude session in a worktree of their own, so much of what is in flight is in a tab rather than in a repository or Linear. When an answer could depend on work that may be under way in another tab, look before answering, with this one command (it asks the workspace's server only, so it costs nothing to run):

```
switchboard tabs
```

One line per pane, tab-separated: the tab's number (its own while the tab is open: closing another tab renumbers none), the pane's place (`<session>:@<tab>.%<pane>`), its name (with the PR number and the Linear ticket and title when it has them), Claude's state there (`waiting` on me, `done` and not yet seen, `working`, blank for idle), the program (a version number is Claude Code), its directory, a worktree's naming its branch, and, where the pane runs Claude, its session ID, its process ID, the name its `ListAgents` row shows and its address for `SendMessage` (blank where it runs none). Some cases where it matters, as examples of the kind of question, not a list of the only ones:

- The status of a project or ticket: which of its tickets are being worked on in a tab right now, and which of those wait on me.
- Starting work on a ticket, branch or PR: whether a tab already has it, so it isn't started twice.
- What to do next, or what is waiting on me: what is in flight and where.

Don't look on turns that can't depend on it. Reading another tab's screen or typing into it is a different thing: only when I ask.

## Talking to another tab

To reach another tab's Claude, use `ListAgents` and `SendMessage`, never `switchboard send-keys` into its pane. Find the tab's line in the command above and send to the address in its last column (`uds:<socket>`), which `SendMessage` takes as its `to`; a `ListAgents` row shows the name in the column before it, the same for every tab in one checkout until it is renamed, so never pick a row by its name or directory alone. A message from another tab names its sender's socket, which is that tab's address. Send only when I ask, or a skill says to, as with reading its screen. Write the message so it stands alone, since that session sees nothing of this conversation, and report that it was delivered, not that it was done: an answer comes back as a message of its own.

## Naming a tab

To name a tab, set its `@tab_name`; never `switchboard rename-tab`, whose name stays when `/wt` clears `@tab_name` to hand the tab its ticket's. Leave out the PR number, which the tab adds itself:

```
switchboard set @tab_name 'Review loop: settings page layout'
```

# What I see in the workspace

When I name one of these, I mean what Switchboard shows me:

- The diff (`M-d`, "the diff", "the diff view"): the tab's branch against its PR's base, side by side, in place of a pane, with its story ("the story"), Haiku's walk through the change in chapters. `switchboard diff-story` prints the story as Markdown (`--path` its file), from what the diff kept, never telling a new one.
- Mission control (`M-Space`): views of my day and work (Tabs, Work map, PRs, Tickets, Inbox, Slack, Calendar, Notifications, Usage, Settings), from what the workspace already holds.
- The sidebar: every tab, grouped by what its Claude is doing, as `switchboard tabs` lists them.
- The PR view (`M-v`) and the ticket view (`M-i`): the tab's PR description and its Linear ticket.
- Notes (`M-e`): see "# Notes".

# Skills

- When a skill exists both in the repository (`.claude/skills/<name>/`, in the worktree or its main clone) and as my personal one or Switchboard's (`/switchboard:<name>`), use the repository's: read its SKILL.md and follow it instead. `switchboard skill-path <name>` prints which copy to follow (the repository's, else Switchboard's own: my personal copy of it, else the plugin's, never a skill of mine that only shares its name), and a skill handing off to another reads the file it prints.
