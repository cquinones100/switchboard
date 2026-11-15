---
name: note-tickets
description: Read the open checklist items in the notes the user names, check Linear for tickets that already cover each, and suggest tickets for the rest, creating only the ones the user picks.
argument-hint: "[folder, note title or #tag]"
disable-model-invocation: true
allowed-tools:
  - Bash(switchboard skill-path *)
  - mcp__claude_ai_Linear__get_issue
  - mcp__claude_ai_Linear__list_issues
  - mcp__claude_ai_Linear__list_comments
  - mcp__claude_ai_Linear__get_team
  - mcp__claude_ai_Linear__list_teams
  - mcp__claude_ai_Linear__list_issue_statuses
  - mcp__claude_ai_Linear__get_user
---

# Note tickets

This is Switchboard's own copy, for repositories that have none. Before anything else, run `switchboard skill-path note-tickets`: if it prints a path other than this file's, read that file and follow it instead, with the same arguments, and stop reading here.

Turn the to-dos in the user's notes (from Switchboard's notes app) into Linear tickets, without duplicating work Linear already has. The user starts this on purpose, and it reads only the notes they name: the notes app is built to keep a note's text on the Mac, and running this skill is the user's choice to send some of it to Claude.

Arguments: which notes to read. Any of a folder (`Work`), a note (`Work/Planning.md`, or its title), or a tag (`#todo`), several allowed. With none, list the folders and tags (names only, not contents) and ask which, with `AskUserQuestion`. Never read the whole notes directory by default.

## Ground rules

- Read the notes with Read, Glob and Grep only, and nothing outside the ones named. Skip `.trash/`, `.notes.json` and `.notes.lock`.
- Nothing is written to Linear until the user picks which tickets to create.
- A note is changed only as step 7 says, never otherwise. Never write a note's whole file: change the lines step 7 names and nothing else.
- Keep team names, ticket prefixes and folder names out of this repository: it is public.

## Steps

1. **Find the notes.** The notes directory is `$NOTES_DIR`, or `~/Notes`. Resolve the arguments to notes: a folder means every `.md` under it, a tag every note whose text has it (a `#word` at the start of a line or after a space, outside code), a title the note whose first line is `# <title>`. If one matches nothing, say so; if a title matches more than one, ask.

2. **Collect the open items.** An item is an unticked checklist line, `- [ ] …`, at any indent; ticked ones (`- [x]`) are done. Skip any item that already names a ticket: an ID like `ABC-123`, or a Linear issue link. A nested item belongs to the one above it; keep it with its parent rather than as an item of its own. Keep each item's note and line number. An image in an item or under it (`![name](path)`, its path from the note's folder) is part of it: read it with the Read tool along with the item's text. Don't infer tasks from prose: only checklist items count. If the notes have none, say so and stop.

3. **Settle the team.** Read `linearTeam` and `linearLabels` from `~/.config/switchboard/linear.json`, as `/shape` does. If `linearTeam` is missing, ask, and suggest adding it to that file.

4. **Look each item up in Linear.** Search the team's open tickets, and those closed in the last 90 days, by the item's key words, and read the likely matches. Sort each item into one of:
   - **Covered**: a ticket does this work. Name it, and say how sure you are.
   - **Partly covered**: a ticket overlaps but leaves part of it out. Say which part.
   - **Not covered**: nothing found.

   A wrong "covered" hides work, so when unsure, say partly covered and show the candidate.

5. **Show the result.** One table: the item (as written), its note and line, what you found (the ticket ID and title, or nothing), and for anything not or partly covered, a proposed ticket title. Group the items that would make one ticket together, and say so. Then ask, with `AskUserQuestion` (`multiSelect`), which proposed tickets to create. Offer to shape one in full with `/shape` instead, for anything too rough to write from a checklist line.

6. **Write the tickets.** For each one picked, draft it the way `/shape` does: read three or four of the team's recent tickets and match their layout, read the code where the item names something in it, and keep it as short as the item allows; a one-line to-do makes a small ticket. Show the drafts together and wait for approval or edits. Then create them in the team, with `linearLabels` if set, linking any partly covering ticket as related. Reply with each ticket's ID and URL.

7. **Mark the notes.** So the next run skips them, each item that now has a ticket, or that the user agreed was covered, gets its ID at the end: `- [ ] Fix the card copy (ABC-123)`. Show the lines, and ask whether to add the IDs. If yes, change only those lines, with Edit, reading each note again first, since it may have changed since step 2. The notes app can be open: it brings a change made to a note's file into the open note, merged with anything you have typed and not yet saved, and never saves over it.
