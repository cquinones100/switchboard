---
name: wtn
description: Replace this Claude session with `wtn` in the same tmux pane, so a worktree opens in this tab for one checklist item of a note, with a fresh Claude session working on it and its plan. For work kept in a note in any repository; /queue-and-clean lands it, straight on the default branch where the repository commits there with no PR, as Switchboard does, else through a PR.
argument-hint: "[note] [item text or line number] [what to add to its prompt]"
disable-model-invocation: true
---

# wtn

This is Switchboard's personal copy, installed for repositories that have none. Before anything else, run `skill-path wtn`: if it prints a path other than this file's, read that file and follow it instead, with the same arguments, and stop reading here.

Hand this tab over to `wtn` for one checklist item of a note: `/wt`'s counterpart for work kept in a note rather than a Linear ticket. This session ends, and in its place `wtn` (`bin/worktree-from-note`) makes a worktree on a branch named after the item (`note/<slug>`), moves the pane's shell into it, and starts a new Claude session there, told the item, its plan, and to land it with `/queue-and-clean` when done: on the default branch in a repository whose `CLAUDE.md` commits straight there, else through a PR.

The conversation does not carry over. The item and its plan are the hand-off.

Arguments, all optional: the note (its title or path), the item (a few of its words, or its line number in the note), and text to add to the new session's prompt.

## Steps

1. **The note.** Settle it as `~/.claude/skills/note-watch/pick-note.md` says (read it and follow it), a note named in the arguments counting as already named. Its step 5 marks the tab with the note.

2. **The item.** Read the note. Its items are its unticked top-level checklist lines (`- [ ] …` at the start of a line), each with the lines indented under it, leaving out any under a section named for parked work (`## Defered`, `## Later`).
   - **Named in the arguments**, by line number or by words only one item's first line holds: use it. Words that match several: ask which, as below, with those alone.
   - **Not named**: ask with `AskUserQuestion`, the items in the note's order, four to a question with **More…** as an option moving to the next four, each option the item's first line, shortened, and its description `needs a choice` when its `Plan:` line or plan note still marks a choice as needed.

   Note the item's line number: everything after this goes by it. Whatever else the arguments hold is the text to add.

3. **In tmux.** `$TMUX_PANE` must be set. If it is not, tell the user to run `wtn --note '<note path>' --item <line>` themselves (`--prompt '<text>'` with text to add) and stop.

4. **Work still running.** As `/wt`'s step 3: handing the tab over ends this session and everything it started (background commands, subagents, workflows, Monitor watches, scheduled loops). If any is still running, list each in a line and ask whether to stop them and hand over, or wait; on wait, stop here.

5. **Name the tab** for the item, its first line cut to 50 characters, so the tab says what it is working on (a worktree on a `note/` branch has no ticket to name it):

   ```
   tmux set-option -w -t "$TMUX_PANE" @tab_name '<item, shortened>'
   ```

   Single-quoted, each `'` in it written `'\''`.

6. Say, in one line, that this session is handing the tab to `wtn` for the item. Put nothing after it.

7. Run this as your final action, on its own, with the item's line number (digits only) filled in, the note's path within the notes directory in `WTN_NOTE`, never in the command `zsh` runs, single-quoted with each `'` written `'\''`:

   ```
   tmux respawn-pane -k -t "$TMUX_PANE" -c "$PWD" -e 'WTN_NOTE=Switchboard Notes.md' "zsh -i -c 'wtn --item 12; exec zsh -i'"
   ```

   With text to add, a second `-e 'WT_PROMPT=<text>'` before the command, quoted the same way. The `wtn` function passes both on and clears them.

   `-k` stops this session, and `zsh -i` reads `~/.zshrc`, which defines `wtn` (`eval "$(worktree-from-note init zsh)"`, which `setup.sh` adds). `-c "$PWD"` starts it in this directory, so `wtn` finds the same repository. If `wtn` refuses (a ticked item, or a worktree another session is already working in), `exec zsh -i` leaves a shell in the pane.

Run each tmux command above as a separate, plain command, not chained or wrapped in shell logic, so it matches the settings that allow `tmux set-option` and `tmux respawn-pane` by name. Do not run anything after the respawn.
