---
paths:
  - "src/notes/**"
  - "crates/notes/**"
  - "src/autopilot/**"
  - "src/agent/hands_free.rs"
  - "src/cmd/item_done.rs"
  - "src/cmd/worktree_from_note.rs"
  - "assets/skills/plan-item/**"
---

# Notes

**Notes stay on the computer.** Notes live in `$NOTES_DIR` or `~/Notes`, never in this repository, which is public. Nothing in a note is sent anywhere, with five exceptions, each one you start or turn on: squashing notes, after a yes; the `note-tickets`, `note-watch` and `plan-item` skills, which read only the notes you name or confirm (`plan-item` the tab's own note too), the first two `disable-model-invocation` and `plan-item` started by Claude only when you ask for an item; the Today's todo note routine, paused unless you turn it on; the tab's own note, which `switchboard claude-context` points each interactive session in the tab at by its path for Claude to read (`tab_note_context`, off by default), never passing its words itself; and the tab's plan note, which M-d's story gives Haiku as its context (`diff_story_plan`, off by default). Anything new that reads a note keeps it that way. Each note is open for editing in one notes app at a time: with an app in each tab (`notes_per_tab`), the app with a note open holds its lock (`.locks/<note id>`, `Store::lock_note`) and another shows it read-only, and with the setting off one app runs on a notes directory at a time (`.notes.lock`); `.notes.json` is written through `Store::update_meta`, a read-merge-write under a lock of a moment. Nothing writes a note but through the app's save, which merges in changes made from outside, with three exceptions: a note's stable ID, its last line (`crates/notes/src/id.rs`), which the app writes as it opens a note that has none, and which `wtn` and autopilot give a note that has none, or give back to one rewritten without it, through `id::ensure_id` and `id::give_id` alone, one process at a time under the ID's lock and with the note's modified time kept, a change the app takes in as it takes in any made from outside; the server's `Decided hands-free` lines for a hands-free tab's question box, at the end of its item's plan note or under the item in its note, written off its task through `id::rewrite` alone (`src/agent/hands_free.rs`), under the same lock and with the modified time kept; and `switchboard item-done`, which ticks the item a landed worktree's branch was started for and writes its `Done:` line under it (`format::toggle_check` on the file's text), under the same lock, its modified time changing as any edit's does so autopilot sees the tick, a change the app takes in the same way. Whatever shows, searches or sends a note's words leaves its ID out (`id::strip`).
