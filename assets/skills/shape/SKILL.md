---
name: shape
description: Shape new work with the user, filed where the repository keeps its work (`switchboard tickets`) - a Linear ticket, a GitHub issue, or a note's checklist item with its plan note. Reads the code first, checks for work that already covers it, drafts it in the usual form, and files it once the user approves. Given an existing Linear ticket, asks whether to run a review loop on it or keep shaping it. Hands off to /wt afterwards.
argument-hint: "[--notes | --github | --linear] [idea, or ticket ID or URL and what to do with it]"
disable-model-invocation: true
allowed-tools:
  - Bash(switchboard skill-path *)
  - Bash(switchboard set *)
  - Bash(switchboard tickets)
  - Bash(switchboard tabs)
  - Bash(switchboard show *)
  - Bash(gh issue view *)
  - mcp__claude_ai_Linear__get_issue
  - mcp__claude_ai_Linear__list_issues
  - mcp__claude_ai_Linear__list_comments
  - mcp__claude_ai_Linear__get_team
  - mcp__claude_ai_Linear__list_teams
  - mcp__claude_ai_Linear__list_issue_statuses
  - mcp__claude_ai_Linear__get_user
---

# Shape

This is Switchboard's own copy, for repositories that have none. Before anything else, run `switchboard skill-path shape`: if it prints a path other than this file's, read that file and follow it instead, with the same arguments, and stop reading here.

Turn a rough idea into work someone can pick up and build: clear on the problem, grounded in the code as it is, honest about scope, and filed where the repository keeps its work, so `/wt` and autopilot find it. The `switchboard shape` command starts a session with this skill; it also works in a session that is already running, and Notes' M-Enter in "Add an item" opens a tab running `/shape --notes <the item>`.

Arguments: a description of the problem or idea, or, with no source or `--linear`, a Linear ticket ID or URL (`ABC-123`, or a link with `/issue/ABC-123/` in it) to work on an existing ticket, after an optional source (`--notes`, `--github` or `--linear`, see "Where it goes"). Text after a ticket (and after `--continue`, when that is there) is the user's direction for this shaping, such as what to change or look into: keep it in mind throughout, and let it decide between a review loop and shaping where it says which. With neither, ask what the work is about. Given a ticket, read it first and then ask what to do with it (see "Given a ticket" below).

## Where it goes

Settle the source before anything else but naming the tab, and ask nothing about it:

- A source in the arguments wins: `--notes`, `--github` or `--linear`, taken off before the rest is read.
- Else run `switchboard tickets`, whose first line is where the repository keeps its work, repos.json's `tickets`: `github` (its issues), `linear`, or `notes` (a note's checklist, with a plan note each). It has no default: when the command fails (the key isn't set, or outside a repository), stop, telling the user what it printed on stderr, which says how to set it; nothing is filed anywhere.
- For a GitHub issue, `switchboard tickets` is run whichever way the source was settled, `--github` included: a second line `autopilot-label <label>` names the label autopilot takes this repository's issues by (repos.json's `issueLabel`); with no such line, or when the command fails after `--github`, autopilot doesn't take them.
- With no source flag, a Linear ticket ID or URL leading the arguments is Linear's, whatever the repository keeps, so it needs no `switchboard tickets` and isn't stopped by one that fails. After `--notes` or `--github`, everything is the description, a leading ticket ID included (`ABC-12 follow-up: retry the upload` is a note's item or an issue, not that ticket).

Then follow that source's section: "Linear" for `linear`, "GitHub issues" for `github`, "A note's item" for `notes`. "Ground rules" and "Name the tab" hold for all three.

## Ground rules

- Read only, but for what the source's last step files. Do not edit, create, or delete files in the repository, and do not commit, branch, or push. Shaping ends with filed work, not a change; `/wt` starts the work.
- Answer what the code can answer yourself. Ask the user about intent, priority, and scope, not about how the code works today.
- Nothing is written to Linear or GitHub until the user approves a draft.

## Name the tab

In Switchboard, the tab says what is being shaped, the way `/wt` names a ticket's tab, so it is easy to find in the tab bar, `M-j` and Slack. Name it as soon as the skill starts, from the arguments: `Shaping: <the first four or five words>` for a description, or `Shaping <ID>` for a ticket. Name it again once there is a ticket or an issue, from its ID (`#<number>` for an issue) and title: after reading the ticket you were given, and after creating one, as `Shaping <ID>: <title>`. `/wt` clears the name when it hands the tab over, and the tab then takes the ticket's own name. A note's item keeps the tab's name: Notes' M-Enter names its tab itself.

Only when `$SWITCHBOARD_PANE` is set, and with the name cut to 60 characters and kept to letters, digits, spaces and `. , : - ( ) / #` (drop anything else, quotes included, since it goes into a shell command):

```
switchboard set @tab_name 'Shaping ABC-123: Show which phase is active'
```

This sets the same `@tab_name` that renaming the tab in `M-p` and `switchboard shape` set, and the tab takes the name at once. If it fails, carry on: a name is no reason to stop.

## Linear

### Given a ticket

When the argument names an existing ticket, fetch it and its comments with the Linear connector before anything else. If it doesn't exist or can't be read, say so and stop. Name the tab for it (see "Name the tab"). When `--continue` follows the ticket (as `shape --continue`, `shape-batch --continue` and the palette's "Continue shaping Linear tickets" pass it), give the two-line summary below and carry straight on as "Continue shaping it", without asking. Otherwise give a two-line summary of where it stands (its title, state, and whether it already has a problem statement, an approach and acceptance criteria) and ask which of these to do, with `AskUserQuestion`:

- **Run a review loop on it.** Run `/review-loop ticket <ID>`, the review loop's ticket mode, which has Codex review the ticket's text against the code as it is (see that skill's `references/ticket-mode.md`). Use it when the ticket looks ready and you want it checked before anyone builds it. If this repository has a `review-loop` of its own with no ticket mode, say so and continue shaping instead. The loop ends asking whether to save its edits to the ticket; once it has, ask whether to keep shaping it by continuing below from step 6. Nothing is written to Linear without approval, as always.
- **Continue shaping it.** Carry on with the steps below, refining this ticket rather than drafting a new one. Use it when the ticket is still rough, or the user wants to change its scope.

Put the one the ticket's state suggests first, marked as recommended: a review loop for a ticket that already reads as complete, shaping for one that doesn't.

### A checklist item from a note

The notes action (M-p's, which splits a note's items as `switchboard shape-batch` does and opens a tab running `switchboard shape` for each) passes one item from a checklist in the user's notes, on one line: the item, `(from my note "<title>")`, then `Details:` with everything indented under it that isn't a checklist item (lines joined by ` / `), then `Sub-items, each its own ticket:` with the unticked checklist items indented under it, numbered, each with its own details in brackets. The details are part of the item's ticket. Each sub-item is a ticket of its own.

When there are sub-items, ask first, before anything else but naming the tab, with `AskUserQuestion`, how to organise them:

- **A project:** "<item>" becomes a Linear project, and each sub-item an issue in it.
- **A ticket with sub-issues:** "<item>" becomes a ticket, and each sub-item a sub-issue of it.

Then run the steps for the item and for each sub-item together: step 2 checks each one for a ticket that already covers it (and, for a project, for a project that already covers the item), and a sub-item one covers is offered as that sub-item rather than drafted again. Step 7 drafts all of them at once, the project or parent first, and step 8 creates the project or parent, then each sub-item in it or under it, linking the ones that already existed instead of copying them. An item that names a ticket ID is checked against that ticket first.

### Steps

1. **Settle the team.** Read `linearTeam` (a team key such as `ABC`) and `linearLabels` (an array of label names) from `~/.config/switchboard/linear.json`, the file `wt` already reads its Linear key from. If `linearTeam` is missing, take the team of the ticket being refined, or ask, and suggest adding it to that file. Keep team names and keys out of this repository: it is public.

2. **Check whether a ticket already covers it.** Given a description rather than a ticket, search Linear first, before reading any code: open tickets in the team, and ones closed in the last 90 days, by the description's key words. A ticket covers it when it is about the same change, not merely the same area. If one or more do, show each (its ID, title, state and link, and a line on why it matches) and ask with `AskUserQuestion` whether to resume shaping one of them, with an option per ticket, up to three, and **Shape a new ticket** last. Resuming one makes it the ticket being refined: fetch it and its comments, name the tab for it (see "Name the tab"), and carry on from step 3 as "Continue shaping it" would, without asking the "Given a ticket" question. If nothing covers it, say so in a line and go on. Shaping several items at once, as `shape-batch` and the notes action do, each session checks its own item.

3. **Learn the team's ticket shape.** Read three or four recent tickets from that team with the Linear connector, preferring ones that were built and closed. Note their headings and conventions (acceptance criteria, labels) and use them, but the user's own rules for tickets in `~/.claude/CLAUDE.md` ("PRs and Linear tickets") win where they differ: business logic first, a diagram for a pipeline, implementation details last and brief.

4. **Understand the problem in the code.** Find where the behaviour lives, how it works now, and what a change would touch. Name the files and functions you looked at, so the draft points at real places. When refining an existing ticket, read it and its comments first, and check whether anything it assumes has changed since it was written.

5. **Look for related tickets.** With the code understood, search Linear again for open and recently closed tickets that overlap with this or that it depends on, which step 2's search for the same work can't find. If one turns out to cover it after all, say so and ask whether to refine that one instead of opening another.

6. **Ask only what is left.** Usually a few questions about intent and scope: what the user wants to be true afterwards, and what is deliberately out. Keep them to what changes the ticket.

7. **Draft.** Show the full ticket, as "PRs and Linear tickets" in `~/.claude/CLAUDE.md` says: a title that says what changes, then the business logic (what changes and why, answering what a teammate would later ask), a diagram for a pipeline or multi-step flow, acceptance criteria, what is out of scope, and last the implementation in a few words, linking to the code on GitHub rather than describing it. Use step 3's headings and conventions where they don't conflict. Note related tickets as links rather than copying them in. Then wait for the user's approval or edits. Revise until they approve.

8. **Write it to Linear.** Once the user approves the draft, create the ticket in the team, with `linearLabels` if set, or update the ticket being refined. For a note item with sub-items, create the project (in the team) or the parent ticket first, then each sub-item as an issue in that project or a sub-issue of that ticket, as "A checklist item from a note" says. Link the related tickets from step 5. Name the tab for a ticket just created, or again if the title changed (see "Name the tab"). Reply with the ticket ID and URL.

9. **Review the ticket.** Right after writing it, run `/review-loop ticket <ID>` on it, the review loop's ticket mode, a ticket refined as much as one created, without asking: the user set this up in this skill, so it counts as asking for the loop by name. It reads the ticket from Linear and ends asking whether to save its edits there, as ticket mode always does. For a note item with sub-items, review the project or parent and each sub-item, one loop each. If the repository has a `review-loop` of its own with no ticket mode, say so and skip it. Then offer `/wt <ID>` to start the work in this tab.

## GitHub issues

The issue goes on `origin`'s repository, which is public for an open-source project, so nothing is posted before the user's yes. Read GitHub with `gh`, from the repository.

1. **Check whether an issue already covers it.** Before reading any code, search the repository's open issues, and those closed in the last 90 days, by the description's key words (`gh issue list --state open --search '<words>' --json number,title,state,url`, then `--state closed` with `closed:>=<the date 90 days ago>` added to the search). An issue covers it when it is about the same change, not merely the same area. If one or more do, show each (its number, title, state and link, and a line on why it matches) and ask with `AskUserQuestion` whether one of them is it, with an option per issue, up to three, and **Shape a new issue** last. Choosing one ends here: offer `/wt #<number>` to start it. If nothing covers it, say so in a line and go on.

2. **Learn the shape of its issues.** Read three or four of the repository's recent issues (`gh issue view <number>`), preferring ones that were built and closed. Note their headings and conventions and use them, but the user's own rules for tickets in `~/.claude/CLAUDE.md` ("PRs and Linear tickets") win where they differ.

3. **Understand the problem in the code**, **look for related issues**, and **ask only what is left**, as the Linear steps 4 to 6 say, with issues in place of tickets.

4. **Draft.** Show the full issue as the Linear step 7 drafts a ticket: its title, `BUG: ` and what is broken or `FEAT: ` and what to build when the repository's issues are titled so, and its body, which autopilot and `/wt` build from as its plan. Link the related issues by their URLs. Then wait for the user's approval or edits. Revise until they approve.

5. **Write it out.** Once approved, write the body to `issue.md` and the title to `title.txt` in your scratchpad directory.

6. **Create it.** Ask with `AskUserQuestion`, the issue's title in the question, **Create it on <owner/name>** or **Not yet**, since it is public. When `switchboard tickets` named autopilot's label, check first that the repository has it (`gh label list --search '<label>' --json name`, an exact name in the answer); when it hasn't, ask in the same question instead: **Create the label and the issue** (`gh label create '<label>'` first), **Create the issue without the label** (autopilot won't take it), or **Not yet**. On a yes, create it on `origin`, assigned to the user, with autopilot's label when it is there, so autopilot takes it:

   ```
   gh issue create --title '<title>' --body-file <issue.md> --assignee @me [--label '<label>']
   ```

   Name the tab for it (see "Name the tab") and reply with its number and URL.

7. **Review the issue.** As the Linear step 9 says, with the issue in place of the ticket: run `/review-loop ticket <number>`, which ends asking whether to save its edits to the issue. Then offer `/wt #<number>` to start the work in this tab.

## A note's item

The work is one item in the tab's note, as a checklist line, with a plan note beside it that a session started with `/wt` can build from. The item is written as soon as it is shaped, with no draft to approve: it stays on the computer.

1. **The note.** Notes are in `$NOTES_DIR` or `~/Notes`.
   - The tab's own note first: `switchboard show @tab_note`, else `switchboard show @tab_note_last`, each a path within the notes directory (each prints nothing when the tab has none, or when `$SWITCHBOARD_PANE` isn't set, outside Switchboard's panes).
   - With neither, settle one as `~/.local/share/switchboard/claude/plugin/skills/note-watch/pick-note.md` says: read it and follow it. Its last step records the note on the tab.
   - With no note at all, offer to create `<Name> Notes.md`, `<Name>` the repository's folder name with its first letter capitalized (its first line `# <Name> Notes`), and a `<Name> Plans/` folder beside it. Create the file only when no file by that name exists, never over one, then record it on the tab as `pick-note.md`'s step 5 does.

   The plans folder is the one beside the note, named after the note's title with `Notes` replaced by `Plans` (`Acme Notes` → `Acme Plans`). Create it when it is missing.

2. **Already there?** Read the note, ticked and unticked items both, and the plan notes' `Item:` lines, for an item covering the same thing.
   - An open one: say which, and ask with `AskUserQuestion` whether to add to it, replace it, or add a new item anyway.
   - A ticked one did it: say so, with its `Done:` line when it has one, and ask whether this is something new.
   - Also look for a tab already working on it, with `switchboard tabs` (as Switchboard's instructions say under "My tabs"), and name it when one is.

3. **Look into it.** Read the code the item touches, to name what it does today with `file:line`, and the cause for a bug. When the user has evidence (a screen recording or screenshot on the Desktop, a log, `server.log` in `~/.local/state/switchboard/` for Switchboard), look at it, and note where it was seen. Settle whether there is a real choice for the user to make; most items have none.

4. **Where it goes.** Unless the request already says, ask with `AskUserQuestion`, the options in this order:
   - **Top of the list**: the first item in the note's top list.
   - **In order of difficulty**: the top list is kept easiest first; put the item before the first one clearly harder than it, and say in the reply which two items it sits between.
   - **Bottom of the list**: the last unticked item of the top list, before the blank line and the ticked items.

   An item always goes in the note's top list (the items before its first `## ` section), never in a section, which is where ticked items are moved.

5. **The plan note.** `<plans folder>/Plan - <name>.md`, `<name>` a few words in title case naming what the change does, and one no other `Plan - <name>.md` anywhere in the notes directory has, since `/wt` and autopilot find a plan by its file name alone:

   ```
   # Plan - <name>

   Item: "<the item's first line, without its `- [ ] `>"

   ## Today

   - <what the code does now, with `file:line`; the cause for a bug>

   ## Decision needed

   - [ ] A (recommended): <option>
   - [ ] B: <option>

   ## Steps

   1. <step>
   ```

   - `## Decision needed` only when there is a real choice; with one, the steps are headed `## Steps (A)`, for the recommended option.
   - A choice that waits on the user's review of a design board names its boards by number in its option (`build it as boards 69 and 70 draw it, once they are reviewed`): autopilot holds such an item, hands-free or not, until the choice is ticked.
   - When the options differ in how something looks, indent under each a fenced block opened with ```` ```mock ````: a few lines of what it would look like drawn in text, which Notes draws inside the option's card beside the note.
   - The steps are numbered and concrete, naming files and functions. The last ones are always the docs and the tests the change needs (which files, and what each test checks), and whatever else the repository's `CLAUDE.md` asks every change to have.
   - The plan follows the repository's `CLAUDE.md` and the user's.
   - Write it as the user's rules say: changes as instructions, current behavior only under `## Today`, no rejected alternatives, no future notes, and no self-corrections or thinking aloud. Write each step as decided.

6. **The item**, only once its plan note is written, since autopilot can take an item as soon as it is in the note. One `- [ ]` line, its first line the item's title, a bug or a feature, in one sentence under 90 characters, in the user's words where they fit:
   - A bug starts `BUG: ` and says what is broken now, in the present tense: `BUG: A tab shows Working when it is idle`.
   - A feature starts `FEAT: ` and says what to build, as a task starting with a verb: `FEAT: Add a view to mission control that lists the day's landings`. Only a bug is in the present tense.
   - After its prefix, it leads with the words that tell it from the note's other items, since `/wt` names the branch and the worktree from it and the tab's name and the board show about its first 50 characters. Its first six words differ from every other item's in the note, ticked ones included: the first line is the item's key, and `/wt` refuses one that slugs the same as another.
   - A leading tag is kept as given, after the prefix (`FEAT: board 18: …`), and keys and commands go in backticks.
   - No file paths, line numbers, dates or screenshots, no `[` or `]` (a `[[…]]` there would be read as the item's plan link), and no period at the end.
   - The cause, the evidence and the pointers go under it and in the plan's `## Today`, and the request's longer wording nowhere else: the plan's `Item:` quotes the first line as written.

   Under it, indented two spaces, in this order: `- Cause: …` for a bug once found, `- Seen in …` for evidence, `After: <first line>` for each open item of the note it builds on (a change that needs the other's code first), that item's first line as the note has it without its `- [ ] `, which holds the item in `/wt` and autopilot until that one is ticked, said in the reply, then `Plan: [[Plan - <name>]]`, followed by ` (needs a choice: <the options' letters>)` while the plan has an open decision. Edit the note file directly, as `assets/switchboard.md` says for notes; when moving items, move each whole, with its indented lines.

7. **A choice made later.** When the user answers a plan's decision ("A", "b"), tick that option, rename the heading `## Decision needed` to `## Decision` (autopilot skips a plan while it has a "Decision needed" heading), rewrite the steps that depended on it and their heading, and change the item's `(needs a choice: …)` to `(chose <X>)`.

8. **The reply.** In a few lines: the item's first line quoted as written, so one the user wants worded otherwise shows at once, where it sits in the note, the cause in a line for a bug, and any choice needed as one-line options. Don't recap the plan's steps.
