---
name: wt
description: Replace this Claude session with `wt` in the same Switchboard pane, so a worktree opens in this tab for one piece of work, a Linear ticket, a GitHub issue or a note's checklist item, with a fresh Claude session working on it. For the end of a shaping conversation, or to start a note's item; /queue-and-clean lands it as `switchboard landing` says for the repository.
argument-hint: "[ticket ID, #issue or link | note and item] [→ branch to land on] [what to add to its prompt]"
disable-model-invocation: true
allowed-tools:
  - Bash(switchboard skill-path *)
  - Bash(switchboard set *)
  - Bash(switchboard landing)
  - Bash(switchboard landing *)
  - Bash(switchboard respawn *)
  - Bash(git status)
  - Bash(git status *)
---

# wt

This is Switchboard's own copy, for repositories that have none. Before anything else, run `switchboard skill-path wt`: if it prints a path other than this file's, read that file and follow it instead, with the same arguments, and stop reading here.

Hand this tab over to `wt` for one piece of work: a Linear ticket, a GitHub issue, or a checklist item of a note. This session ends, and in its place `wt` (`switchboard worktree-from-ticket`) makes the work's worktree (a ticket's branch, `issue/<number>-<slug>` for an issue, `note/<slug>` for an item), moves the pane's shell into it, and starts a new Claude session there, told the work (an item with its plan) and, for an issue or an item, to land it with `/queue-and-clean` when done, as `switchboard landing` says for the repository: straight on the default branch, or through a PR. Because the pane itself now runs from the worktree, the workspace sees that directory: splits, new tabs, the tab name, and its header all follow it, which a `cd` from inside this session could never do.

The conversation does not carry over. The work is the hand-off, and the new session starts by reading it.

## Steps

1. **The work.** Split the arguments. The first word decides what they name:

   - **A Linear ticket**, its ID (`ABC-123`) or a Linear URL (take the ID from its `/issue/<ID>/` part): the ticket; everything after it is the text to add.
   - **A GitHub issue**, `#12` or an issue's URL (`https://github.com/<owner>/<repo>/issues/12`): the issue; everything after it is the text to add.
   - **A note's item**, the note (its title or path) and then the item (a few of its words, or its line number in the note): go to "A note's item" below.
   - **Anything else, or nothing**: all of it is the text to add, and the work is found in this order, without asking for an ID:
     1. A ticket or issue this session created or saved (through Linear or `gh`). If it saved more than one, the most recent.
     2. The one ticket ID (`ABC-123`) or issue (`#12`) the conversation centered on, or the one checklist item of a note it worked on, which is "A note's item" below with the note and the item already named.
     3. The current branch, from the first line of `git status --porcelain --branch` (`## <branch>`, then `...<upstream>` if it has one; a detached HEAD has no branch), when it starts with a ticket ID such as `abc-123-…` (uppercase it) or is `issue/<number>-…` (the issue `#<number>`).

     If the answer is a guess between several (more than one saved, or a conversation about two or three), name the most likely one and ask the user to confirm it before going on. When none of the three gives anything, ask which work outright.

   A branch to land on is written `→ <branch>` after the work, or `--target <branch>` anywhere: the worktree is cut from it and `/queue-and-clean` lands it there, as autopilot's target does. It takes an item or an issue, not a Linear ticket. With none, it lands on the default branch.

   **How it lands.** Run `switchboard landing` here and keep the way its first line prints, the one way the work lands, which `/queue-and-clean` follows: `direct` lands straight on the default branch (or the branch to land on), with no PR, under the landing lock with the repository's tests and review; `switchboard-queue`, `github-queue`, `auto-merge` and `service` land through a PR against the default branch (or against the branch to land on, which GitHub's auto-merge merges as it does any PR whose base isn't the default branch, but under a merge service, which merges it as it merges the rest), merged only by that way, never by a `gh pr merge` or a push to the default branch of the session's own. When it fails, the repository has no way set yet: go on, since the new session's landing stops there until one is chosen in Settings' Repositories, and say so in step 6.

   Check the work's shape: a ticket ID `ABC-123`, an issue's number (digits), an item's line number (digits). Anything else: say so and stop. It is placed into a shell command below, so nothing else may go in.

2. **A note's item** (only for one).
   - **The note.** Settle it as `~/.local/share/switchboard/claude/plugin/skills/note-watch/pick-note.md` says (read it and follow it), a note named in the arguments counting as already named. Its step 5 marks the tab with the note.
   - **The item.** Read the note. Its items are its unticked top-level checklist lines (`- [ ] …` at the start of a line), each with the lines indented under it, leaving out any under a section named for parked work (`## Defered`, `## Later`). Named in the arguments, by line number or by words only one item's first line holds: use it; words that match several: ask which, as below, with those alone. Not named: ask with `AskUserQuestion`, the items in the note's order, four to a question with **More…** as an option moving to the next four, each option the item's first line, shortened, and its description `needs a choice` when its `Plan:` line or plan note still marks a choice as needed, and `waits on <item>` when an `After: <first line>` line under it names an item still unticked in the note (by its first line, or a part of exactly one open item's), which `wt` asks about before opening it.

   Note the item's line number: everything after this goes by it. Whatever else the arguments hold, the branch to land on aside, is the text to add.

3. **In a Switchboard pane.** `$SWITCHBOARD_PANE` must be set. If it is not, there is no pane to hand over. Tell the user to run it themselves and stop: `switchboard worktree-from-ticket <ID>` for a ticket, `switchboard worktree-from-ticket '#<number>'` for an issue, `switchboard worktree-from-ticket --note '<note path>' --item <line>` for an item, with `--prompt '<text>'` for text to add and `--target '<branch>'` for a branch to land on.

4. **Work still running.** Handing the tab over ends this session, and everything it started ends with it: background shell commands, subagents, workflows, Monitor watches and scheduled loops (`CronList`). If any is still running, list each in a line and ask with `AskUserQuestion` whether to stop them and hand the tab over, or wait (recommended first when one is close to done). On wait, stop here and say `/wt` can be run again once they finish. On stop, stop each with `TaskStop` (a scheduled loop with `CronDelete`) and go on.

5. **Name the tab.**
   - For a ticket or an issue, clear the tab's name, whether `shape` set it ("Shaping: …") or the user did with "Rename the tab" in M-p:

     ```
     switchboard set -u @tab_name
     ```

     A tab working on a ticket or an issue is named for it: with no name set, the workspace names this one from the worktree's branch once it opens, the same name every other such tab has.
   - For an item, name it for the item, its first line cut to 50 characters, so the tab says what it is working on (a worktree on a `note/` branch has no ticket to name it), single-quoted, each `'` in it written `'\''`:

     ```
     switchboard set @tab_name '<item, shortened>'
     ```

6. Say, in one line, that this session is handing the tab to `wt` for the work, and how it will land (the way from step 1, or that none is set). That line is the last thing the user will see from this session, so put nothing after it.

7. Run this as your final action, on its own, with the work filled in:

   ```
   switchboard respawn -c "$PWD" wt ABC-123
   switchboard respawn -c "$PWD" wt '#12'
   switchboard respawn -c "$PWD" wt --item 12 --note 'Switchboard Notes.md'
   ```

   The first for a ticket, the second for an issue (or its URL, single-quoted), the third for an item, with its line number (digits only) and the note's path within the notes directory, single-quoted with each `'` written `'\''`. With a branch to land on, `--target '<branch>'` after them, and with text to add, `--prompt '<text>'`, each quoted the same way, all on one line:

   ```
   switchboard respawn -c "$PWD" wt ABC-123 --prompt 'start with the migration'
   ```

   The `wt` function adds the text to the work's prompt.

   `respawn` stops this session and starts `wt` in its place in the same pane, in a shell of the user's that defines `wt` and reads their rc for the PATH. `-c "$PWD"` starts it in this directory, so `wt` finds the same repository. When the new Claude session exits, or if `wt` refuses (a ticked item, a closed ticket it asks about, or a worktree another session is already working in), a shell is left in the pane: in the worktree if `wt` got that far, here otherwise.

Run each command above as a separate, plain command, not chained or wrapped in shell logic. Switchboard's settings allow exactly these by name (`switchboard set`, `switchboard respawn`), and a compound command would not match those rules and would be stopped for approval.

Do not run anything after the respawn. It ends this session, so there is nothing to report and no one to report it to.
