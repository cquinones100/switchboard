---
name: plan-item
description: Add a checklist item to the tab's note and write its plan note in the plans folder beside it, after checking the item isn't there already and looking into the code it touches, in the shape `/wtn` and autopilot build from, its first line a bug ("BUG:") or a feature ("FEAT:"). Use when the user asks to add something to their note's checklist, plan an item, or log a bug or a feature for later work; not for a Linear ticket (`/shape`) or for working an item now (`/wtn`).
argument-hint: "<what the item is about, in your words>"
---

# plan-item

This is Switchboard's own copy, for repositories that have none. Before anything else, run `switchboard skill-path plan-item`: if it prints a path other than this file's, read that file and follow it instead, with the same arguments, and stop reading here.

Add one item to the tab's note: the item as a checklist line in the note, and a plan note beside it that a session started with `/wtn` can build from. The argument is what the item is about, in the user's words; with none, ask what it is.

## Steps

1. **The note.** Notes are in `$NOTES_DIR` or `~/Notes`.
   - The tab's own note first: `switchboard show @tab_note`, else `switchboard show @tab_note_last`, each a path within the notes directory (each prints nothing when the tab has none, or when `$SWITCHBOARD_PANE` isn't set, outside Switchboard's panes).
   - With neither, settle one as `~/.local/share/switchboard/plugin/skills/note-watch/pick-note.md` says: read it and follow it. Its last step records the note on the tab.
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

5. **The plan note.** `<plans folder>/Plan - <name>.md`, `<name>` a few words in title case naming what the change does, and one no other `Plan - <name>.md` anywhere in the notes directory has, since `/wtn` and autopilot find a plan by its file name alone:

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
   - The steps are numbered and concrete, naming files and functions. The last ones are always the docs and the tests the change needs (which files, and what each test checks), and whatever else the repository's `CLAUDE.md` asks every change to have.
   - The plan follows the repository's `CLAUDE.md` and the user's. In Switchboard's own repository that means: a new feature gets a setting (in `assets/settings.json`, on by default unless it sends something of yours somewhere, or "No setting" and why), a new key an `Action` in `src/mux/keys.rs` or a part's `Command` with `about()` and its tags (so `M-p` lists it), anything that takes a choice a click and a Tab stop, what it says goes through `Server::say`, nothing on the drawing path starts a process or waits on the network, and the docs are `README.md`, `docs/guide.md`, `ARCHITECTURE.md` and `CLAUDE.md`, whichever the change reaches.
   - Write it as the user's rules say: changes as instructions, current behavior only under `## Today`, no rejected alternatives, no future notes, and no self-corrections or thinking aloud. Write each step as decided.

6. **The item**, only once its plan note is written, since autopilot can take an item as soon as it is in the note. One `- [ ]` line, its first line the item's title, a bug or a feature, in one sentence under 90 characters, in the user's words where they fit:
   - A bug starts `BUG: ` and says what is broken now, in the present tense: `BUG: A tab shows Working when it is idle`.
   - A feature starts `FEAT: ` and says what to build, as a task starting with a verb: `FEAT: Add a view to mission control that lists the day's landings`. Only a bug is in the present tense.
   - After its prefix, it leads with the words that tell it from the note's other items, since `/wtn` names the branch and the worktree from it and the tab's name and the board show about its first 50 characters. Its first six words differ from every other item's in the note, ticked ones included: the first line is the item's key, and `/wtn` refuses one that slugs the same as another.
   - A leading tag is kept as given, after the prefix (`FEAT: board 18: …`), and keys and commands go in backticks.
   - No file paths, line numbers, dates or screenshots, no `[` or `]` (a `[[…]]` there would be read as the item's plan link), and no period at the end.
   - The cause, the evidence and the pointers go under it and in the plan's `## Today`, and the request's longer wording nowhere else: the plan's `Item:` quotes the first line as written.

   Under it, indented two spaces, in this order: `- Cause: …` for a bug once found, `- Seen in …` for evidence, `After: <first line>` for each open item of the note it builds on (a change that needs the other's code first), that item's first line as the note has it without its `- [ ] `, which holds the item in `/wtn` and autopilot until that one is ticked, said in the reply, then `Plan: [[Plan - <name>]]`, followed by ` (needs a choice: <the options' letters>)` while the plan has an open decision. Edit the note file directly, as `assets/switchboard.md` says for notes; when moving items, move each whole, with its indented lines.

7. **A choice made later.** When the user answers a plan's decision ("A", "b"), tick that option, rename the heading `## Decision needed` to `## Decision` (autopilot skips a plan while it has a "Decision needed" heading), rewrite the steps that depended on it and their heading, and change the item's `(needs a choice: …)` to `(chose <X>)`.

8. **The reply.** In a few lines: the item's first line quoted as written, so one the user wants worded otherwise shows at once, where it sits in the note, the cause in a line for a bug, and any choice needed as one-line options. Don't recap the plan's steps.
