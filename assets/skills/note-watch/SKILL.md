---
name: note-watch
description: Watch one of the user's notes for new checklist items while this session runs, and work each one as it appears, ticking it when done. Confirms the note before starting.
argument-hint: "[note title or path]"
disable-model-invocation: true
---

# Note watch

This is Switchboard's own copy, for repositories that have none. Before anything else, run `switchboard skill-path note-watch`: if it prints a path other than this file's, read that file and follow it instead, with the same arguments, and stop reading here.

The user keeps a running checklist in a note (Switchboard's notes app) and adds to it while you work. This skill keeps an eye on that note with the Monitor tool, so each item they add reaches you on its own, without them having to say "there are new items". The user starts it on purpose, and it reads only the note they confirm: the notes app keeps a note's text on the Mac, and starting this skill is the user's choice to send that one note to Claude.

Arguments: the note, by title (`Editor Configs Notes`) or path. With none, the note this conversation has been working from, if there is one.

## Ground rules

- Read only the confirmed note, never the rest of the notes directory, apart from listing titles in step 1.
- Change the note only to tick an item you have finished (step 4), editing that line and moving it under the unticked ones, as the user's CLAUDE.md says for notes. Never rewrite the note otherwise.
- An item phrased as a question is answered, not built. An item you can't tell how to do is asked about, not guessed at.

## Steps

1. **Settle the note**, as `~/.local/share/switchboard/claude/plugin/skills/note-watch/pick-note.md` says (the argument, or the note this conversation has been working from, confirmed; else a list of titles to pick from). Start nothing until the user has picked.

2. **Read it.** Read the note and say in a line how many items are still open. Ask whether to work those now, or only what is added from here.

3. **Start watching.** Run the Monitor tool with `switchboard watch-note "<the note's path>"`, described as `new items in <title>`. It prints `NEW: <text>` for each item added after it starts, once its text has stopped changing, and `GONE: …` if the note is moved or deleted. Tell the user it is watching, and how to stop it ("stop watching").

4. **Work each new item.** For each `NEW:` event, read the note again for the item's context, and any image in it or indented under it (`![name](path)`, its path from the note's folder, as Notes' Paste image writes it): read each with the Read tool, as part of the item, then do it the way this conversation has done its items: in the repository you are working in, following CLAUDE.md, a commit of its own when it changes code, asking first when it is unclear or would do something hard to undo. When it is done, tick it in the note (`- [x]`) and move it under the unticked items, with anything indented under it. Reply in a line or two: what you did, and the commit. Items that arrive while you work on one wait their turn, in order.

5. **Keep it running.** A Monitor watch expires (its start says when, usually after 30 minutes) and sends one notice when it does. Start it again with the same command when that notice comes, unless the user has said to stop. On `GONE:`, say the note has moved or gone, and ask which note to watch, as in step 1.

6. **Stop.** When the user says to stop, stop the watch with `TaskStop` and say so.
