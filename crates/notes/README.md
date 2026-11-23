# switchboard-notes

Notes as the macOS Notes app keeps them, in your terminal, edited with Vim's keys: a list of folders, tags and notes on the left, the note on the right, drawn as Notes shows it, with the markup hidden, checklists as boxes you tick, tables with box lines and mermaid blocks as diagrams.

It is [Switchboard](../../README.md)'s notes app (`M-e` there) as an app of its own. It runs in any terminal, with no Switchboard running. It is also a plugin on Switchboard's plugin API ([`switchboard-plugin-api`](../plugin-api/src/lib.rs)), which is how the workspace loads it, as it loads [the diff](../diff/README.md).

## Install

```
(dir=$(mktemp -d) && cd "$dir" &&
curl -fsSLO https://github.com/cquinones100/switchboard/releases/latest/download/install.sh &&
curl -fsSLO https://github.com/cquinones100/switchboard/releases/latest/download/install.sh.sig &&
echo 'switchboard-release namespaces="switchboard-installer" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHk/NIR8Ueq68RDwrJ3vWpLvcNX85BrMHDf0UBVnk05B' > allowed_signers &&
ssh-keygen -Y verify -f allowed_signers -I switchboard-release -n switchboard-installer -s install.sh.sig < install.sh &&
{ less install.sh || cat install.sh; } &&
sh install.sh --notes)
```

The installer is the newest Switchboard release's, signed with the release key ([Switchboard's README](../../README.md#install) says more); each line runs only when the one before it succeeded, in a new temporary folder and a subshell, in bash or zsh (in fish, run `bash` first). For one release, `notes-v<version>`, put `releases/download/notes-v<version>` in place of `releases/latest/download` in both `curl` lines, and `SWITCHBOARD_VERSION=notes-v<version> sh install.sh --notes)` as the last line: that release carries its own signed installer.

It has releases of its own, apart from Switchboard's, tagged `notes-v<version>` in Switchboard's repository. The line downloads the newest one's `switchboard-notes-<version>-<target>.tar.gz` for this computer (x86_64 Linux, or an Intel or Apple silicon Mac) over HTTPS, refuses it unless it matches the SHA-256 published beside it and is signed with Switchboard's release key, and puts `switchboard-notes` in `~/.local/bin`. It installs nothing else and sets nothing up. Run it again for a newer release.

Or build it from a clone of Switchboard, with [rustup](https://rustup.rs) and a C compiler:

```
cargo build --release -p switchboard-notes
install -m 755 target/release/switchboard-notes ~/.local/bin/
```

## Requirements

- A terminal that draws 256 colors. One that reports the mouse lets you click and scroll.
- Claude Code (`claude` on your PATH, or in `~/.local/bin`), only to squash notes into one. Without it nothing is sent anywhere.

## Using it

```
switchboard-notes                  # the app, on the note open last
switchboard-notes open <note>      # show a note in the running app, by its path in the notes directory
switchboard-notes sidebar on|off   # show or put away the running app's list
switchboard-notes shown            # print the note the running app shows
```

Notes are Markdown files in `$NOTES_DIR`, else `~/Notes`, one file a note, in folders that are directories, so any editor can open them. The keys are Vim's, and the same as in Switchboard: [The notes app](../../docs/guide.md#the-notes-app) lists them. `:q` closes it, and `Ctrl-Shift-P` lists every command.

## What it sends

Nothing in a note is sent anywhere, with one exception you start: squashing the notes you marked into one sends them to Claude (Sonnet, through `claude -p`), after a yes.

## Alone and in Switchboard

It is the same program in both. Alone, it answers for the workspace itself, and what it can't do without one is left out:

| | Alone | In Switchboard |
|---|---|---|
| A tab's note, going to a note's tab, starting work on a checklist item, the tabs working on an item | Not there, and left out of the command list | The workspace's |
| A link followed | Says it can't be opened | Opened in the browser the workspace gives its site |
| Pictures in a note | Shown as the lines that name them | Drawn in the note, and full size over it |
| Copying | Through the terminal (OSC 52) | The desktop's clipboard too |
| Putting the desktop clipboard's text or picture | Your terminal's own paste | `p`, and `Ctrl-V` for a picture |
| Squashing notes | `claude -p` directly | The same, counted in the request audit |
| Settings | Its defaults, and what you set in `~/.config/switchboard/settings.json` when you have one | Mission control's Settings |
| Its files | `~/.local/state/switchboard-notes` and `~/.cache/switchboard-notes` | Switchboard's own state and cache |

## How it is built

Of Switchboard's crates it depends on three: `switchboard-plugin-api`, everything a plugin and the workspace say to each other, `switchboard-ui`, what everything draws with, and `switchboard-vim`, the Vim it edits with, which Switchboard's views and code viewer read with too. It never depends on `switchboard` itself, so the compiler keeps it out of the multiplexer. Everything it asks of the workspace is in [`src/host.rs`](src/host.rs).
