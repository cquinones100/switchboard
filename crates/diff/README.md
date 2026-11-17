# switchboard-diff

A branch's changes as GitHub's "Files changed" shows them, side by side in your terminal: the changed files in reading order on the left, the chosen file's two sides on the right, with syntax colors and the changed words marked. With Claude Code installed, the change is told as a story, in chapters, each with why it is there.

It is [Switchboard](../../README.md)'s diff (`M-d` there) as an app of its own. It runs in any terminal and any git repository, with no Switchboard running. It is also the first plugin on Switchboard's plugin API ([`switchboard-plugin-api`](../plugin-api/src/lib.rs)), which is how the workspace loads it.

## Install

```
curl -fsSL https://raw.githubusercontent.com/cquinones100/switchboard/main/install.sh | sh -s -- --diff
```

It has releases of its own, apart from Switchboard's, tagged `diff-v<version>` in Switchboard's repository. The line downloads the newest one's `switchboard-diff-<version>-<target>.tar.gz` for this computer (x86_64 Linux, or an Intel or Apple silicon Mac) over HTTPS, refuses it unless it matches the SHA-256 published beside it and is signed with Switchboard's release key, and puts `switchboard-diff` in `~/.local/bin`. It installs nothing else and sets nothing up. Run it again for a newer release.

Or build it from a clone of Switchboard, with [rustup](https://rustup.rs) and a C compiler:

```
cargo build --release -p switchboard-diff
install -m 755 target/release/switchboard-diff ~/.local/bin/
```

## Requirements

- git.
- A terminal that draws 256 colors. One that reports the mouse lets you click, scroll and select text.
- Claude Code (`claude` on your PATH, or in `~/.local/bin`), only for the story, the reading order of the files and questions about the change. Without it the files are listed by kind and nothing is sent anywhere.

## Using it

```
switchboard-diff [dir]                  # the branch in dir (else here) against the default branch, or the branch it was cut from
switchboard-diff --base origin/develop  # against another branch or commit, used as given, nothing fetched
switchboard-diff --working [dir]        # the changes not yet committed, staged and not
```

The keys are Vim's, and the same as in Switchboard: [Keys in the diff](../../docs/guide.md#keys-in-the-diff) lists them. The cursor stays in the file it is in, stopping at its first and last lines: `]f` and `[f` go to the next file and the one before. `q` closes it. A drag of the mouse selects text in the part of the screen it starts in and copies it when the button is let go, a side of the diff without its line numbers or the other side; a double click takes a word and a triple click a line. With the keys, `v` selects lines and `y` copies their text.

## What it sends

With Claude Code installed, the diff and the commits' messages go to Claude (Haiku, through `claude -p`) to order the files and tell the story, and a question you ask with `ga` goes with the diff. To keep the diff on your computer, turn those off in `~/.config/switchboard/settings.json`:

```json
{ "diff_order": false, "diff_story": false, "diff_ask": false }
```

## Alone and in Switchboard

It is the same program in both. Alone, it answers for the workspace itself:

| | Alone | In Switchboard |
|---|---|---|
| Compared with | The branch it tracks when that is another branch than its own (one cut from a feature branch), else the repository's default branch on `origin`, fetched first (else `main` or `master`) | The base of the branch's pull request, then the same |
| The story | Told by `claude -p` directly | The same, counted in the request audit, with the tab's plan note as context when you turn that on |
| Settings | Its defaults, and what you set in `~/.config/switchboard/settings.json` when you have one | Mission control's Settings |
| Its files | `~/.cache/switchboard-diff` and `~/.local/state/switchboard-diff` | Switchboard's own cache and state |
| Copying | Through the terminal (OSC 52) | The desktop's clipboard too |
| The pane beside it, a file opened below, a message in a box | Not there | The workspace's |
| Someone else's pull request | Read as your own code | A worktree opened untrusted is read as one: binary files as text, and no Claude started in it |

That last row matters: to read a pull request from someone you don't know, open it in Switchboard (`wtpr`, then `M-d`), not with `switchboard-diff` in its checkout.

## How it is built

Of Switchboard's crates it depends on two: `switchboard-plugin-api`, everything a plugin and the workspace say to each other, and `switchboard-ui`, what everything draws with. It never depends on `switchboard` itself, so the compiler keeps it out of the multiplexer. Everything it asks of the workspace is in [`src/host.rs`](src/host.rs). A program it runs (git, `claude`, your editor) is given its options as it starts, by flags or environment; it never edits that program's own config files.
