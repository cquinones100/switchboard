# switchboard-diff

A branch's changes as GitHub's "Files changed" shows them, side by side in your terminal: the changed files in reading order on the left, the chosen file's two sides on the right, with syntax colors and the changed words marked. With Claude Code installed, the change is told as a story, in chapters of pages, each page with why it is there, and as a map of which page builds on which, read from the page the change exists for down.

It is [Switchboard](../../README.md)'s diff (`M-d` there) as an app of its own. It runs in any terminal and any git repository, with no Switchboard running. It is also the first plugin on Switchboard's plugin API ([`switchboard-plugin-api`](../plugin-api/src/lib.rs)), which is how the workspace loads it.

## Install

```
(dir=$(mktemp -d) && cd "$dir" &&
curl -fsSLO https://github.com/cquinones100/switchboard/releases/latest/download/install.sh &&
curl -fsSLO https://github.com/cquinones100/switchboard/releases/latest/download/install.sh.sig &&
echo 'switchboard-release namespaces="switchboard-installer" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHk/NIR8Ueq68RDwrJ3vWpLvcNX85BrMHDf0UBVnk05B' > allowed_signers &&
ssh-keygen -Y verify -f allowed_signers -I switchboard-release -n switchboard-installer -s install.sh.sig < install.sh &&
{ less install.sh || cat install.sh; } &&
sh install.sh --diff)
```

The installer is the newest Switchboard release's, signed with the release key ([Switchboard's README](../../README.md#install) says more); each line runs only when the one before it succeeded, in a new temporary folder and a subshell, in bash or zsh (in fish, run `bash` first). For one release, `diff-v<version>`, put `releases/download/diff-v<version>` in place of `releases/latest/download` in both `curl` lines, and `SWITCHBOARD_VERSION=diff-v<version> sh install.sh --diff)` as the last line: that release carries its own signed installer.

It has releases of its own, apart from Switchboard's, tagged `diff-v<version>` in Switchboard's repository. The line downloads the newest one's `switchboard-diff-<version>-<target>.tar.gz` for this computer (x86_64 Linux, or an Intel or Apple silicon Mac) over HTTPS, refuses it unless it matches the SHA-256 published beside it and is signed with Switchboard's release key, and puts `switchboard-diff` in `~/.local/bin`. It installs nothing else and sets nothing up. `switchboard-diff update` installs a newer release ([Updates](#updates)).

Or build it from a clone of Switchboard, with [rustup](https://rustup.rs) and a C compiler:

```
cargo build --release -p switchboard-diff
install -m 755 target/release/switchboard-diff ~/.local/bin/
```

## Requirements

- git.
- A terminal that draws 256 colors. One that reports the mouse lets you click, scroll and select text.
- Claude Code (`claude` on your PATH, or in `~/.local/bin`), only for the story and the reading order of the files. Without it the files are listed by kind and nothing of the change is sent to Claude; the once-a-day look for a newer release asks GitHub for its list of releases whatever is installed ([Updates](#updates)).

## Using it

```
switchboard-diff [dir]                  # the branch in dir (else here) against the default branch, or the branch it was cut from
switchboard-diff --base origin/develop  # against another branch or commit, used as given, nothing fetched
switchboard-diff --working [dir]        # the changes not yet committed, staged and not
```

The keys are Vim's, and the same as in Switchboard: [Keys in the diff](../../docs/guide.md#keys-in-the-diff) lists them. The cursor stays in the file it is in, stopping at its first and last lines: `]f` and `[f` go to the next file and the one before. `q` closes it. A drag of the mouse selects text in the part of the screen it starts in and copies it when the button is let go, a side of the diff without its line numbers or the other side; a double click takes a word and a triple click a line. With the keys, `v` selects lines and `y` copies their text.

A story told in chapters opens on what changes: Before and After in plain words and a line of why for each page, an outline of the chapters (the goal's first, then each part it can be read without, and the side work) and their pages at the left, a ✓ on each read. `Enter` in the outline reads from there down one column: each page with its claim, what it decided and instead of what, what to watch, and its changes inline, side by side where the column is wide enough, tests folded last. `]` and `[` move page to page, `Tab` between the outline and the column, and `Enter` on a change opens its menu (shown in the files, or its place copied). What runs, always over the column, lays the steps out in the order they happen, `h` and `l` in the column moving along them. `g s` shows the story view, `g m` the story's map (a box a page, an arrow to what each builds on, Enter reading a page in the column), `g f` the page being read in the file view, which lists the change by chapter, then page, then file, and `m` a menu of the three with Tell it again. A change of more than 20 files or 1,500 changed lines is told by Sonnet, any other by Haiku (`"diff_story_model": "haiku"` or `"sonnet"` in the settings picks one). `"diff_story_map": false` in the settings tells the story as one list instead.

With the story in chapters, it opens as a talk, read outside in (`"diff_story_talk": false` opens on what changes instead): the change in one sentence and one worked example taken from the change's own tests, its Before and After, then the flow the example takes, each step a box lit where it passes with its value over the arrow, then each step on a slide of its own, then the code. `-` and `+` zoom out and in through Gist, Flow, Steps and Code, `←` and `→` go between the slides, a row of dots at the foot showing which, and `Tab` reaches the zoom's switch and the dots. A slide shows its step's claim, the example going in and coming out, its hunk with notes in the margin, and its Decided, Instead and Watch (`n`); Code shows every hunk of the step and the example's test with its case lit. With no test giving an example, it says so rather than make one up. `g t` shows the talk from the other views. The talk's flow, What runs and the map scroll sideways where they are wider than the pane: `h` and `l` bring a step or a page out of sight into view, Shift with the wheel moves them, and a scroll bar at their foot shows where.

The first line of the screen is its header: its name, the repository and what the branch is compared with, and its version. Under it is a row of buttons, `←` and `→` first, back to where you were before a jump and forward again (`H` and `L`), then each pressing its keys in the file list, with Settings before Close. `m` in the file list, or a right click, opens a menu of the same.

## Settings

`M-m` (Alt-m), the Settings button, or "Settings" in the menu shows the diff's settings in its place: each option with its name, its key and whether it is on, the chosen one with its whole description and its default. `j` `k` or the arrows move, `Enter`, `Space` or a click on the chosen row turns it on or off, and `Esc`, `q` or `M-m` goes back to the diff.

| Option | Key | Default |
|---|---|---|
| Files in reading order | `diff_order` | on |
| Diff told as a story | `diff_story` | on |
| Diff story progress | `diff_story_progress` | on |
| Diff story a page at a time | `diff_story_pages` | on |
| Diff story notes on changes | `diff_story_notes` | on |
| Open the diff on what changes | `diff_story_map` | on |
| Open the diff as a talk | `diff_story_talk` | on |
| Uncommitted changes in M-d | `diff_uncommitted` | on |
| Diff file list wide | `diff_list_wide` | on |
| Check for a newer switchboard-diff | `diff_update_check` | on |

A change is kept in `~/.config/switchboard-diff/settings.json`, the diff's own file, apart from Switchboard's (`SWITCHBOARD_DIFF_SETTINGS` names another): only what differs from its default is written, the version before is kept as `settings.json.bak`, and a file that doesn't read as JSON is never written over. The diff follows a change at once. You can edit the file by hand too. In Switchboard the same options are rows of its own Settings, kept in its own file.

## Updates

Once a day (an hour after a run closed before GitHub answered), after it has drawn, `switchboard-diff` asks GitHub for the newest `diff-v` release and keeps the answer in `~/.cache/switchboard-diff/update.json`. The request is for the repository's list of releases; nothing of yours is sent. When a newer one is out, the header says so at its right end, `0.2.0 is out · switchboard-diff update`, and the settings view has a row that installs it. "Check for a newer switchboard-diff" turns the look off. Only a release looks: a binary built from a clone doesn't.

```
switchboard-diff update            # install the newest release
switchboard-diff update --check    # say whether there is one, changing nothing
switchboard-diff update --force    # install it in place of a build from a clone
```

`update` downloads the release's archive for this computer over HTTPS, refuses it unless it matches the SHA-256 published beside it and is signed with Switchboard's release key (compiled into the binary), runs the new binary once to see that it starts, and only then puts it in the old one's place. Whatever fails leaves the binary as it was.

## What it sends

With Claude Code installed, the diff and the commits' messages go to Claude (Haiku, or Sonnet for a large change, through `claude -p`) to order the files and tell the story. To keep the diff on your computer, turn off "Files in reading order" and "Diff told as a story" in its [settings](#settings). The story is asked for as a change opens, so to send Claude nothing from the first run, write them to `~/.config/switchboard-diff/settings.json` before opening a repository, with `diff_update_check` to ask GitHub for no releases either (comparing with a base it hasn't kept yet still fetches it from `origin`; `--base <ref>` or `--working` fetch nothing):

```json
{ "diff_order": false, "diff_story": false, "diff_update_check": false }
```

A `switchboard-diff` built from a clone before it had settings of its own read them from Switchboard's `~/.config/switchboard/settings.json`; it reads only its own file now, so write any of these you had set there into `~/.config/switchboard-diff/settings.json` before running it again.
 The look for a newer release sends GitHub a request for the list of releases and nothing else ([Updates](#updates)).

## Alone and in Switchboard

It is the same program in both. Alone, it answers for the workspace itself:

| | Alone | In Switchboard |
|---|---|---|
| Compared with | The branch it tracks when that is another branch than its own (one cut from a feature branch), else the repository's default branch on `origin`, fetched first (else `main` or `master`) | The base of the branch's pull request, then the same |
| The story | Told by `claude -p` directly | The same, counted in the request audit, with the tab's plan note as context when you turn that on |
| Settings | Its own settings view (`M-m`), kept in `~/.config/switchboard-diff/settings.json` | Mission control's Settings, under Diff; `M-m` is the workspace's |
| The header and the Settings button | Its own | The workspace's tab header; no Settings button |
| Updates | Its own look once a day, and `switchboard-diff update` | `switchboard update`, which covers the diff built into Switchboard |
| Its files | `~/.cache/switchboard-diff` and `~/.local/state/switchboard-diff` | Switchboard's own cache and state |
| Copying | Through the terminal (OSC 52) | The desktop's clipboard too |
| The mouse pointer (a hand over what a click presses, the text beam over text, with `hover_highlight`) | Set by the diff, in Ghostty and kitty | Shown by the workspace over the diff's pane |
| The pane beside it, a file opened below, a message in a box | Not there | The workspace's |
| Someone else's pull request | Read as your own code | A worktree opened untrusted is read as one: binary files as text, and no Claude started in it |

That last row matters: to read a pull request from someone you don't know, open it in Switchboard (`wtpr`, then `M-d`), not with `switchboard-diff` in its checkout.

## How it is built

Of Switchboard's crates it depends on three: `switchboard-plugin-api`, everything a plugin and the workspace say to each other, `switchboard-ui`, what everything draws with, and `switchboard-highlight`, the syntax colors it shares with Switchboard's code viewer. It never depends on `switchboard` itself, so the compiler keeps it out of the multiplexer. Everything it asks of the workspace is in [`src/host.rs`](src/host.rs). Its settings are declared in its manifest ([`src/lib.rs`](src/lib.rs)) with their names and descriptions, which its own settings view shows; Switchboard's Settings shows them from its own list, and a test there keeps the two the same. Its update checks a release with the plugin API's `release`, the code `switchboard update` runs; reaching GitHub is its `net` feature, on by default, which Switchboard's own dependency on the diff leaves off. A program it runs (git, `claude`, your editor) is given its options as it starts, by flags or environment; it never edits that program's own config files.
