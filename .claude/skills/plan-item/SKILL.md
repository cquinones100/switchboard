---
name: plan-item
description: Add a checklist item to the tab's Switchboard note and write its plan note in the plans folder beside it, after checking the item isn't there already and looking into the code it touches. For work on Switchboard itself, in its repository only.
argument-hint: "<what the item is about, in your words>"
disable-model-invocation: true
---

# plan-item

Add one item to the tab's note, the way Switchboard's own work is planned: the item as a checklist line in the note, and a plan note beside it that a session started with `/wtn` can build from. The argument is what the item is about, in the user's words; with none, ask what it is.

This is a project skill of Switchboard's repository (`.claude/skills/`), for working on Switchboard itself. The skills Switchboard ships are in `assets/skills/`, compiled into the binary; this one isn't, so no Switchboard user gets it.

## Steps

1. **Only in Switchboard.** Run `git rev-parse --show-toplevel` and `git rev-parse --path-format=absolute --git-common-dir`. Go on only when the common directory's parent (the main clone) is the real path of `~/switchboard`, or the top level holds Switchboard's own files, this skill's `.claude/skills/plan-item/SKILL.md`, `assets/switchboard.md` and `src/cmd/skill_path.rs` all three (a clone elsewhere, or a fork). Anywhere else, say in a line that this skill is for Switchboard's own repository, and stop.

2. **The note.** Notes are in `$NOTES_DIR` or `~/Notes`.
   - The tab's own note first: `switchboard show @tab_note`, else `switchboard show @tab_note_last`, each a path within the notes directory (each prints nothing when the tab has none, or when `$SWITCHBOARD_PANE` isn't set, outside Switchboard's panes).
   - With neither, settle one as `assets/skills/note-watch/pick-note.md` in the repository says: read it and follow it. Its last step records the note on the tab.
   - With no note at all, offer to create `Switchboard Notes.md` (its first line `# Switchboard Notes`) and a `Switchboard Plans/` folder beside it. Create the file only when no file by that name exists, never over one, then record it on the tab as `pick-note.md`'s step 5 does.

   The plans folder is the one beside the note, named after the note's title with `Notes` replaced by `Plans` (`Switchboard Notes` → `Switchboard Plans`). Create it when it is missing.

3. **Already there?** Read the note, ticked and unticked items both, and the plan notes' `Item:` lines, for an item covering the same thing.
   - An open one: say which, and ask with `AskUserQuestion` whether to add to it, replace it, or add a new item anyway.
   - A ticked one did it: say so, with its `Done:` line when it has one, and ask whether this is something new.
   - Also look for a tab already working on it, with `switchboard tabs` (as `assets/switchboard.md` says under "My tabs"), and name it when one is.

4. **Look into it.** Read the code the item touches, to name what it does today with `file:line`, and the cause for a bug. When the user has evidence (a screen recording or screenshot on the Desktop, a log, `server.log` in `~/.local/state/switchboard/`), look at it, and note where it was seen. Settle whether there is a real choice for the user to make; most items have none.

5. **Where it goes.** Unless the request already says, ask with `AskUserQuestion`, the options in this order:
   - **Top of the list**: the first item in the note's top list.
   - **In order of difficulty**: the top list is kept easiest first; put the item before the first one clearly harder than it, and say in the reply which two items it sits between.
   - **Bottom of the list**: the last unticked item of the top list, before the blank line and the ticked items.

   An item always goes in the note's top list (the items before its first `## ` section), never in a section, which is where ticked items are moved.

6. **The plan note.** `<plans folder>/Plan - <name>.md`, `<name>` a few words in title case naming what the change does, and one no other `Plan - <name>.md` anywhere in the notes directory has, since `/wtn` and autopilot find a plan by its file name alone:

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
   - The steps are numbered and concrete, naming files and functions. The last ones are always the setting (one per new feature, on by default unless it sends something of yours somewhere, in `assets/settings.json` with its key, category, label, description and default, or "No setting" and why), the docs (`README.md`, `docs/guide.md`, `ARCHITECTURE.md`, `CLAUDE.md`, whichever the change reaches) and the tests (which file, unit tests beside the code or integration tests in `tests/`, and what each checks).
   - The plan follows the repository's `CLAUDE.md`: a new feature gets a setting, a new key an `Action` in `src/mux/keys.rs` or a part's `Command` with `about()` and its tags (so `M-p` lists it), anything that takes a choice a click and a Tab stop, what it says goes through `Server::say`, and nothing on the drawing path starts a process or waits on the network.
   - Write it as the user's rules say: changes as instructions, current behavior only under `## Today`, no rejected alternatives, no future notes, and no self-corrections or thinking aloud. Write each step as decided.

7. **The item**, only once its plan note is written, since autopilot can take an item as soon as it is in the note. One `- [ ]` line in the user's terms. Under it, indented two spaces, in this order: `- Cause: …` for a bug once found, `- Seen in …` for evidence, then `Plan: [[Plan - <name>]]`, followed by ` (needs a choice: <the options' letters>)` while the plan has an open decision. Edit the note file directly, as `assets/switchboard.md` says for notes; when moving items, move each whole, with its indented lines.

8. **A choice made later.** When the user answers a plan's decision ("A", "b"), tick that option, rename the heading `## Decision needed` to `## Decision` (autopilot, `plan_of` in `src/cmd/worktree_from_note.rs`, skips a plan while it has a "Decision needed" heading), rewrite the steps that depended on it and their heading, and change the item's `(needs a choice: …)` to `(chose <X>)`.

9. **The reply.** In a few lines: what was added and where it sits in the note, the cause in a line for a bug, and any choice needed as one-line options. Don't recap the plan's steps.
