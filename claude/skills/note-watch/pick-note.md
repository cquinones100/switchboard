# Settling which note

How `/note-watch`, `/queue-and-clean` and `/wtn` settle which of the user's notes (the notes app, `bin/notes`) a piece of work belongs to. Each reads this file, so they find a note the same way; a change to how belongs here.

Notes are in `$NOTES_DIR` or `~/Notes`, each note's file named after its title (`~/Notes/Editor Configs Notes.md`), folders as subfolders, deleted notes in `.trash/`.

1. **A note already named.** The skill's argument, if it names a note (by title or path), or else the note this conversation has been working from: one the user pointed you at, or whose items you have been working. Find its file.
2. **Confirm it.** Ask the user with `AskUserQuestion` whether that is the note ("Watch Editor Configs Notes?", "Tick this PR's item in Editor Configs Notes?"), even when it seems obvious, with the note as the first option and **Another note** as the second. The one exception is `/queue-and-clean` on a branch `/wtn` opened (`note/<slug>`, with `git config branch.<branch>.wtnItem` set), in a session that started with `/wtn`'s prompt: that prompt names the note, which counts as confirmed.
3. **Or pick one.** When nothing names a note, or the user wants another, list the notes by title, most recently changed first, leaving out `.trash/`, up to ten, titles only and never their contents, and ask which. A skill for which a note is optional, as `/queue-and-clean`'s is, adds **No note** as an option.
4. Read only the note the user confirms or picks. Until then, read no note's contents.
5. **Mark the tab.** Once a note is settled, and when `$TMUX_PANE` is set, record it on the tab, as its path within the notes directory, so `M-e` shows that note beside this tab (`tmux/notes-open.sh`):

   ```
   tmux set-option -w -t "$TMUX_PANE" @tab_note 'Editor Configs Notes.md'
   ```

   With **No note**, record nothing. If it fails, carry on.
