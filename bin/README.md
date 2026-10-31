# bin

`./setup.sh bin` symlinks every executable here into `~/.local/bin`. To install
just one, pass its name instead: `./setup.sh worktree-from-ticket`. Running
it again changes only what isn't in place yet, and says what it changed;
`--check` says what it would change. (`sync.sh`, the old name, still runs it.)

On a new Mac, `./setup.sh` sets up the whole tmux workspace these scripts
belong to: it checks for macOS, zsh, the repo at `~/switchboard`,
Homebrew's `tmux jq fzf gh glow lazygit neovim` (tmux 3.8 or newer), Node
from nvm and `~/.local/bin` on the PATH, links the tmux, Neovim and lazygit
configs and these scripts, adds the `wt`, `wtpr`, `wtr` and `wtb` functions
to `~/.zshrc`, and installs Neovim's pinned plugins, asking
before each install or change to `~/.zshrc`. Then come the optional parts, each asked for
and off unless you say yes: Claude Code's hooks for tab states and `M-a`
(only those hooks, added to your own settings, plus the statusline if you
have none), the skills that ship with Switchboard (`./setup.sh skills`:
`/wt`, `/shape`, `/review-loop`, `/ci-loop` and the rest, linked into
`~/.claude/skills`, a skill of your own by the same name kept, and new ones
linked on later runs), this repo's whole Claude Code setup in place of yours (`./setup.sh
claude`, its owner's settings and telemetry included), and this clone's git
hooks (`./setup.sh git`, for committing to it). It also lists GitHub, the
ticket tracker's key (Linear's or Jira's) and slack-relay, with the keys each
one is for. `./setup.sh
--check` only reports.

It ends by listing again whatever is left to do. Once `./setup.sh git` has
set this clone's git hooks, every `git pull` links what the pull added (the
`tmux`, `bin` and `git` parts, and `claude` if you use all of it) and prints
that list, when there is anything on it.

To turn parts of the workspace on or off (banners, the sidebar, the second
status line, PR and ticket lookups, project colors, the theme, saving pane
contents, the update check and more), open Settings from `M-p`: mission
control's Settings view lists every option by category, Enter or Space
changes one, and `/` searches them. Turning GitHub or the ticket tracker off under
Integrations takes its commands out of `M-p`, makes their keys (`M-v`, `M-m`, `M-r` for
GitHub; `M-i`, `M-l`, `M-W` for the tracker) do nothing, and takes its parts out of
mission control and the tab names, so nothing asks that service. Your
CLAUDE.md, under Claude Code, says where your own Claude rules come from, and
Enter changes it: the local file, another file, or a git repository, typed in;
it writes `~/.config/switchboard/claude.json` and runs setup's `claude_md`
step at once. Below it, each setting kept in a file of your own in
`~/.config/switchboard` (your Linear team or Jira site, the Chrome profile for each site,
the Slack channels, the relay's owner, the on-call config, the sidebar's
sizes, your overrides) is listed under the file's name: Enter types a value
in, starting from what it is (empty unsets it), or opens the whole file in
Neovim. The options are kept in
`~/.config/switchboard/settings.json`, only what differs from the
defaults in `tmux/settings.json`, and a change made by hand there takes
effect at once too.

New to it, you get a tip in the top-right corner the first time a key would
help: when a tab first waits on you or finishes (`M-a`), once you have five
tabs or more (`M-j`), when a tool first takes a pane's place, a PR first
merges, a tab first loops, you first pin a tab or focus the sidebar, Claude
first starts in a worktree, an update first arrives, a key of a service you
turned off is pressed, and after you pick a command from `M-p` that has a
key of its own. Any key closes
it and still does what it would have done. Tips are in Settings, and "Show
the tips again" in `M-p` brings them back.

Once set up, tmux keeps the clone up to date: each time it opens, and once an
hour after that, `tmux/repo-update.sh` checks GitHub, and when `main` there
has new commits it lists them in a popup and asks whether to update. Yes
moves `main` forward, runs `./setup.sh` and reloads tmux; a clone with
commits of its own is left for you to update by hand.

## worktree-from-ticket

Creates a git worktree on a branch named after a ticket, in Linear or Jira as
the tracker setting says (see "Linear or Jira" below), then opens claude in it
with a prompt that carries the ticket's state: if that state is a closed one (completed, cancelled, or duplicate, whatever the workspace
calls them) the prompt asks whether to reopen the ticket before anything else
happens. Then it checks the ticket against the current codebase (is it still
relevant and necessary, is the approach sound, how could it be improved). A
detail the ticket gets wrong it builds correctly without asking and then edits
into the ticket, leaving the PR description to describe the work rather than the
journey; only a change of scope stops and waits for an answer. Then it works on
the ticket without running tests, lint, or any other checks, runs the
`review-loop` skill, and pushes and creates a PR. Its final message has only a
ticket check ("Unchanged." or the edits made to the ticket), "PR pushed" or "PR
not pushed" with the reason, a note about the review loop only when it hit
its round cap, then "Preview:" and "Prod:", a few steps each to try the work
in the PR's preview environment and to check it in production once deployed
(kept out of the PR, which has no test plan).

```
wt ABC-123
```

That creates `.claude/worktrees/abc-123-the-ticket-title-slugified` off the repo
root, checks out a matching branch, moves your shell into it, and opens claude
there. The shell stays in the worktree for the whole session, so new tmux panes
and splits open in the worktree too — and you're still there after claude exits.

`wt ABC-123 --prompt "start with the migration"` adds your own words to the end
of the prompt. `/wt` in a Claude session hands its tab to `wt` the same way,
with anything after the ticket (`/wt ABC-123 start with the migration`, or
`/wt start with the migration` for the session's own ticket) passed in
`WT_PROMPT`, which the `wt` function adds and clears. Since the session ends,
`/wt` first lists anything it still has running in the background (commands,
agents, watches, loops) and asks whether to stop it or wait.

### Setup

1. **Install the script.** From a clone of this repo:

   ```sh
   ./setup.sh worktree-from-ticket
   ```

   That symlinks this one script into `~/.local/bin` and touches nothing else —
   no other scripts, no editor or shell config. Make sure `~/.local/bin` is on
   your `PATH`. The clone can live anywhere.

2. **Add your tracker's key.** For Linear, the default, create a personal key
   at <https://linear.app/settings/api>, then:

   ```sh
   security add-generic-password -U -a "$USER" -s linear-api-key -w   # asks for the key
   mkdir -p ~/.config/switchboard
   printf '{"linearWorkspace":"your-workspace"}' > ~/.config/switchboard/linear.json
   ```

   The key lives in the macOS Keychain, as the relay's Slack tokens do, rather
   than in a file any program you run can read. `LINEAR_API_KEY` in the
   environment also works and takes precedence; a `linearApiKey` in the config
   file is still read, after the Keychain, for a machine set up before.

   `linearWorkspace` is the slug in your Linear URLs
   (`linear.app/<workspace>/issue/...`). `worktree-from-ticket` does not need it,
   but the tmux `M-l` binding reads it from here to open the current branch's
   ticket, so no workspace or team name has to be committed to this repo.
   `LINEAR_WORKSPACE` overrides it.

   For Jira, see "Linear or Jira" below.

3. **Add one line to your shell config** (optional — see below):

   ```sh
   eval "$(worktree-from-ticket init zsh)"
   ```

   Use `bash` instead of `zsh` for bash. This defines the `wt` function.

### Linear or Jira

Everything that reads tickets — `wt`, `worktree-done`, the tab names, `M-i`,
`M-l`, mission control's Tickets view, search and the standup note — asks one
tracker, the one the **Ticket tracker** setting names (`M-p`, Settings):
Linear, the default, Jira, or off. One per machine. The code is in
`lib/tracker/`: `linear.js` and `jira.js` answer the same questions in the
same shape, and the shell scripts in `tmux/` reach them through
`tmux/tracker.sh`.

For Jira Cloud, make an API token at
<https://id.atlassian.com/manage-profile/security/api-tokens> (the plain
one, not "with scopes"), copy it, then:

```sh
printf 'add-generic-password -U -a "%s" -s jira-api-token -w "%s"\n' "$USER" "$(pbpaste)" | security -i
```

That hands the token to `security` on its stdin, so it is never on a command
line. Not `security add-generic-password ... -w` with nothing after it, as
for Linear's key: its hidden prompt keeps only the first 128 characters, and
Atlassian's tokens are longer, so Jira would refuse what it stored.

and add your site and the email of the account the token belongs to to
`~/.config/switchboard/jira.json` (or in Settings, under Jira):

```json
{ "jiraSite": "https://your-company.atlassian.net", "jiraEmail": "you@your-company.com" }
```

For the skills that file tickets (`/shape`, `/note-tickets`), add the project
they file in as `"jiraProject": "ABC"`, and labels for a new ticket as
`"jiraLabels": [...]`, if you want any.

`JIRA_API_TOKEN`, `JIRA_EMAIL` and `JIRA_SITE` in the environment take
precedence, and a `jiraApiToken` in the file is read after the Keychain.
Atlassian's tokens expire after a year at most; when Jira refuses one, the
error says to make a new one. Then pick Jira in Settings.

Where Jira differs:

- A ticket's state comes from its status category (To Do, In Progress,
  Done), and a Done one whose resolution says Won't Do, Duplicate or
  Declined counts as canceled.
- Linear's projects are Jira's epics: `M-p`'s "Open the branch's Linear
  project or Jira epic" opens the epic, and the Tickets view shows it.
- Descriptions and comments are Jira's rich text, shown as Markdown in `M-i`
  and the ticket image; an attachment shows as its name.
- Closing a ticket from mission control takes the first transition its
  workflow offers into Done.
- Branches are named `abc-123-the-title`, since Jira suggests none.
- There's no Inbox view: Jira has no inbox to read.
- The skills that read or write tickets (`/shape`, `/note-tickets`,
  `/queue-and-clean`, `/deploy-test`, `/project-status`) use Claude's
  Atlassian connector, which has to be
  connected in Claude (`/mcp`). They read with it, and write (create a ticket,
  link one, move one to Done) through its `executeWrite`, which Claude asks
  you about each time while the connector's write tools need approval.
  `/project-status` reads an epic's tickets and comments, since Jira has no
  project updates.

### Why step 3 exists

Running `worktree-from-ticket ABC-123` directly works fine and needs no shell
setup at all — it creates the worktree and opens claude in it. The only thing it
cannot do is move your shell: changing directory affects only the calling
process, so no child process can relocate the shell that invoked it. That
requires a shell function, which is what `init` prints.

Without it you still get a working claude session in the worktree, but your shell
stays where it was — so new tmux panes open in the original repo, not the
worktree.

The function body lives in the script rather than in your shell config, so it
stays one stable line that never needs re-editing when the tool changes.

### Notes

- Add `.claude/worktrees/` to the target repo's `.gitignore`.
- The main clone's `.claude/settings.local.json` is symlinked into the new
  worktree when one exists, so personal permissions and the auto mode
  environment profile apply there too. A worktree that already has its own file
  is left alone.
- Rerunning on the same ticket is safe: it reuses an existing worktree, and
  reuses the branch if the worktree directory was removed.
- Unless Claude is already running in that worktree. Then `wt` refuses, rather
  than start a second session editing the same files and pushing the same
  branch, and prints the running session's ID, its title (a `/rename` name, or
  the one Claude generated), and its tmux window. Running sessions are found
  through the files Claude Code keeps in `~/.claude/sessions`, ignoring any
  whose process has ended.
- Pass a different parent directory as a first argument:
  `worktree-from-ticket .worktrees ABC-123`. A full Linear or Jira issue URL
  works in place of the identifier.
- `worktree-done` removes the worktrees whose tickets are finished, and
  mission control's "Close every Merged tab" (Enter on a merged tab) the merged
  tabs' worktrees.
- `wt-batch` opens several tickets at once, one tmux window each, and
  `wt-jump` gets you back to any of those windows by ticket, PR, or branch.

## shape

The step before `wt`: shapes a ticket with Claude, in Linear or Jira as the
`tracker` setting says, from a rough idea or
an existing ticket that needs work.

```
shape "people can't tell which phase is active"
shape ABC-123
shape ABC-123 split out the API part
```

Anything after a ticket is your direction for the shaping, kept in mind
throughout; `/shape ABC-123 split out the API part` does the same. M-p's
**Shape a new ticket from what you type** (`note-shape.sh prompt`) asks
for the idea in a popup, then the repo, and opens a tab shaping it.

It starts Claude with the `/shape` skill, which holds the instructions, so
`/shape` works the same inside a session you already have. Claude reads the
code before asking anything, checks the tracker for tickets that already cover
the idea, drafts the ticket in the layout the team's recent tickets use, and
writes it to the tracker only once you approve the draft. It does not touch the code.
Given an existing ticket, it reads it first and asks whether to run the
repository's `/review-loop` on it in ticket mode (Codex checks the ticket
against the code), or keep shaping it; a review's findings can then be folded
into the ticket, again only once you approve. Afterwards `/wt <ID>` hands the same tab to `wt`, so one tab carries a ticket
from idea to PR.

- Runs in the repository's main checkout, even when called from a worktree,
  since a ticket being shaped has no branch yet.
- Fetches first and warns when the local main is behind its upstream, because
  Claude reads the files on disk. It never pulls.
- Inside tmux, names the tab "Shaping: <first words>" through the same
  `@tab_name` option `M-p`'s "Rename the tab" sets, and the skill names it again as
  "Shaping <ID>: <title>" once there is a ticket (read, or just created). The
  skill does this itself, so `/shape` typed into a session already running
  names its tab too. `/wt` clears that name, or one you set
  yourself, when it takes the tab over, so the tab is named for the ticket
  like every other ticket tab.
- The team to create tickets in comes from `linearTeam` in
  `~/.config/switchboard/linear.json`, next to the API key, with
  optional default labels in `linearLabels`; with Jira, the project from
  `jiraProject` in `jira.json`, with `jiraLabels`. Without one, Claude asks.
  Keeping them there keeps the team out of this repository.

```sh
./setup.sh shape
```

## shape-batch

Runs `shape` for several ideas or tickets at once, each in its own tmux window,
the way `wt-batch` does for `wt`.

```
shape-batch "phase label is unclear" "results load slowly" ABC-123
shape-batch "ABC-123, ABC-124"
shape-batch --continue "ABC-123, ABC-124"
shape-batch < ideas.txt
```

`--continue` carries straight on shaping each ticket, without `/shape` first
asking whether to run a review loop on it; `shape --continue ABC-123` does the
same for one. The M-p palette's "Continue shaping tickets"
(`tmux/shape-open.zsh`) asks for a list of ticket IDs or URLs, the clipboard
filled in when it holds them, and does the same.

An idea is free text full of spaces and commas, so unlike `wt-batch` each
argument is one thing to shape; quote it. An argument made only of ticket
identifiers or Linear URLs is still split on spaces and commas. With no
arguments, ideas are read from stdin, one per line, and blank lines are
skipped. Repeats open once.

The windows go at the end of the tab list, in the order given, so no open tab
is renumbered, and focus stays where it is. Each window starts `shape` with
the idea as its own argument, under an interactive zsh for your PATH, so
whatever the idea contains reaches `shape` as text and is never run, nothing
is typed into a shell that may not be ready, and the window becomes a shell
when Claude exits. Like
`wt-batch`, it must run inside tmux, from the repo the ideas belong to, and
needs `shape` installed.

```sh
./setup.sh shape-batch
```

## wt-batch

Runs `wt` for several tickets at once, each in its own tmux window. Tickets are
separated by commas, spaces, or both, and a full Linear or Jira issue URL works
in place of an identifier.

```
wt-batch ABC-123 ABC-124
wt-batch "ABC-123, ABC-124"
wt-batch ABC-123, https://linear.app/your-workspace/issue/ABC-124/some-title
```

The windows go at the end of the tab list, in the order given, so no open tab
is renumbered, each named after its ticket. Focus stays where it is, so the tabs fill in behind you
while you keep working. Repeated tickets open once, and every ticket is checked
before any window opens, so a typo in one leaves none of them half-started.

### Setup

1. **Install the script.**

   ```sh
   ./setup.sh wt-batch
   ```

2. **Have `wt` working**, meaning `worktree-from-ticket` installed with its
   tracker's key and the `eval "$(worktree-from-ticket init zsh)"` line in your
   shell config. `wt-batch` asks no tracker anything of its own; it only opens the
   windows and types `wt <ticket>` into each.

### Notes

- Must be run from inside tmux. The new windows start in the directory the
  command was run from, so run it from the repo the tickets belong to.
- `wt` is typed into each window's interactive shell rather than run as the
  window's command, for the reason `wt` is a shell function in the first place:
  the shell has to be what cd's, so it stays in the worktree after claude exits
  and panes split from it open there too. It also means the `wt` that runs is
  whatever your shell config defines, so the shell has to be zsh or bash with
  that line in place.

## update-main

Brings the repository's local default branch up to date with origin's, from
wherever you are, a worktree on another branch included, without switching
anything. Only a fast forward: a main with local commits is left as it is.
Where main is checked out, usually the main clone, it is fast-forwarded there,
which moves that checkout's files, so one with changes not committed is left
alone and said so; checked out nowhere, the branch alone is moved.

```
update-main          the repository of the current directory
update-main <dir>    the repository <dir> is in
```

The M-p palette's "Update local main to origin's" (`tmux/update-main.sh`) runs
it in a popup for the pane's repository.

## worktree-from-pr

The same idea for a GitHub pull request instead of a Linear ticket. Takes a PR
number or URL, creates a worktree on that PR's head branch, moves your shell
into it, and opens claude.

```
wtpr 123
wtpr https://github.com/owner/repo/pull/123
```

The M-p palette's "Start work on a PR in a new tab (wtpr)" (`tmux/wtpr-open.zsh`)
asks for a PR number or URL, the clipboard filled in when it holds one, and runs
`wtpr` with it in a new tab.

From anywhere, for a URL: when the PR is in another repository than the one
you are in, or you are in none, it finds a clone of that repository among the
ones you work in (every open tab's, every one a Claude session ran in lately,
and the directories right under your home), makes the worktree there and cd's
you into it; with no clone, it says so. A bare number is a PR of the
repository you are in.

Any PR, whatever its state. A merged or closed one whose branch the remote has
deleted, as GitHub does after a merge, opens from the commits GitHub keeps for
it (`refs/pull/<n>/head`), on a local branch of the PR's name with no upstream,
since there is nothing left to push to.

That creates `.claude/worktrees/pr-123-the-head-branch` off the repo root. The
directory is named after the PR rather than the branch because head branches
often contain slashes, which would nest worktrees inside shared parent
directories.

Unlike `wt`, claude opens with no prompt — you say what you want when you get
there.

A PR from a fork is set up as `gh pr checkout` sets one up: the fork is added
as a remote named after its owner (or a remote already pointing at it is
used), its branch is fetched from there onto a local branch named
`<owner>-<branch>`, since a fork's PR often comes from its `main`, and that
branch tracks the fork's, so `git push` updates the PR. When the fork's owner
hasn't let maintainers push to it, it says so. A fork or branch since deleted
is opened from the PR's last commit, as for any PR.

### Setup

1. **Install the script.**

   ```sh
   ./setup.sh worktree-from-pr
   ```

2. **Install and authenticate gh.** <https://cli.github.com>, then `gh auth
   login`. There is no API key to configure. `gh` is only used to turn a PR into
   a branch name; the fetch and the worktree are plain git.

3. **Add one line to your shell config** (optional):

   ```sh
   eval "$(worktree-from-pr init zsh)"
   ```

   This defines the `wtpr` function. Same reasoning as step 3 above: only a
   shell function can move your shell into the worktree.

### Notes

- The branch is set up to track its remote counterpart, so `git push` and `git
  pull` work with no arguments.
- The main clone's `.claude/settings.local.json` is symlinked into the new
  worktree when one exists, so personal permissions and the auto mode
  environment profile apply there too. A worktree that already has its own file
  is left alone.
- Rerunning on the same PR is safe: it reuses the worktree, and if the branch is
  already checked out in some other worktree it points you there instead of
  failing. A local branch left over from an earlier run gets fast-forwarded to
  the PR head; if it has diverged, it is left alone with a warning rather than
  losing your commits.
- If the PR's branch is checked out in your main clone, the run stops and says
  so. Git allows a branch in one worktree at a time, so there is nothing to
  create — switch the main clone to another branch and rerun.
- Closed and merged PRs work too — the state is printed when it isn't open.
- Works on shallow and `--single-branch` clones: the PR's branch is added to the
  remote's fetch refspec when the existing one doesn't cover it.

## worktree-from-branch

The same for any branch, with no ticket or PR behind it. Takes a branch name,
creates a worktree on it, moves your shell into it, and opens claude with no
prompt.

```
wtb fix-login-redirect
wtb feature/login
```

A branch that exists here or on the remote is checked out as it is, tracking
the remote's when there is one. A branch that exists nowhere is created off the
remote's default branch, fetched first, so it starts from what is merged now
rather than from wherever your checkout sits; it gets an upstream on its first
push. With no remote, it starts at `HEAD`.

`M-B` ("Start work on a branch, new or existing, in a new tab (wtb)" in `M-p`)
(`tmux/wtb-open.zsh`) lists the branches, local and remote, newest commit first, in
fzf: Enter works on the one picked, or creates the name typed when no branch
matches it, and ctrl-n creates the name typed even when some do. It runs `wtb`
with that in a new tab,
in the repository of the pane you were in. The tab is named by
`tmux/tab-label.sh` as for any directory, since there is no ticket to name it.

That creates `.claude/worktrees/<branch>` off the repo root, slashes made
dashes, so `feature/login` lands in `feature-login` rather than nesting.

### Setup

1. **Install the script.**

   ```sh
   ./setup.sh worktree-from-branch
   ```

2. **Add one line to your shell config** (`./setup.sh` offers to):

   ```sh
   eval "$(worktree-from-branch init zsh)"
   ```

   This defines the `wtb` function, which the palette's popup types into the
   new tab.

### Notes

- Plain git: no Linear key and no `gh`.
- Rerunning on the same branch reuses its worktree, or the worktree that already
  has the branch checked out. A local branch behind the remote's is
  fast-forwarded; one that has diverged is left alone with a warning.
- A branch checked out in your main clone stops the run, as with `wtpr`.
- `wtr <branch>` resumes it later, matching the branch name exactly.

## worktree-resume

The way back in. Takes the same ticket, PR or branch the other three take, finds the
worktree they already made for it, moves your shell there, and hands off to
`claude-here` to resume the session that was running in it. What you run after a
restart, when the worktrees survived but the terminal didn't.

```
wtr ABC-123
wtr 123
wtr https://github.com/owner/repo/pull/123
wtr fix-login-redirect
```

Nothing is created and nothing is fetched. The worktree is found by matching the
identifier against `git worktree list` — `wt` and `wtpr` bake it into the branch
name and the directory name, and a branch is matched by name, so it is already recorded on disk. That means no
Linear key, no `gh`, and no network: recovery works on a plane.

### Setup

1. **Install the script.**

   ```sh
   ./setup.sh worktree-resume
   ```

2. **Install `claude-here`** and make sure it is on your `PATH`. That is the
   piece that knows how to find and resume a directory's claude session; this
   script only gets you standing in the right directory.

3. **Add one line to your shell config** (optional):

   ```sh
   eval "$(worktree-resume init zsh)"
   ```

   This defines the `wtr` function. Same reasoning as the other two: only a
   shell function can move your shell into the worktree.

### Notes

- Run it from anywhere in the repo, including from inside another worktree.
- If both a `wt` worktree and a `wtpr` worktree carry the same ticket — you
  opened the ticket, then opened its PR — you get a short numbered list, most
  recently touched first, and Enter takes that one.
- Matching is bounded on both sides, so `ABC-12` never lands you in `ABC-123`.
- A worktree git still tracks but whose directory is gone is reported as such
  rather than as "not found", since rerunning `wt` or `wtpr` is the fix.
- Resuming is `claude-here`'s job, so its own session picker appears when the
  worktree has more than one session.
- For a worktree whose tmux window is still open, `wt-jump` is the shorter
  path: it selects that window instead of starting another session in it.

## wt-jump

The way back to a window you still have open. Takes the same ticket or PR the
other tools take, or any part of a branch name, finds the tmux window sitting in
that worktree, and selects it.

```
wt-jump ABC-123      the ticket's window
wt-jump 123          the PR's window
wt-jump theme-slot   any window whose branch or name contains that
wt-jump --list       print the windows, jump to none of them
```

Each row is the window's second tmux status line, from the same
`tmux/tab-status.sh` tmux runs for the bar: its label (custom name, ticket and
title, or directory), PR, program, and in a Claude pane the session title and
last prompt, coloured the same way. That is what the picker searches, so a
ticket title or something you said to Claude finds the window. It needs
`./setup.sh tmux` as well as `./setup.sh wt-jump`; without the tmux script a row
falls back to its repository, PR, and branch. Every window is rendered at once,
which takes about a second with a dozen open.

Windows whose Claude session is waiting on you come first, then ones that
finished while you were elsewhere, then the rest, from the `@agent` state
`tmux/agent-state.sh` keeps. The row says which ("waiting on you for 4m",
"finished 2m ago"), so typing "waiting" filters to them.

Windows are matched on what is on disk — the worktree directory and the branch
checked out in it — not on the window name. The name is rewritten by the
pane-focus-in hook and says nothing about pull requests, and a window opened by
hand never had a useful one to begin with.

A bare number is answered offline wherever it can be: from a `pr-<n>` worktree,
which is what `wtpr` creates, or from the `#<n>` the tmux tab already carries
once `window-pr.sh` has looked it up. Only when neither knows is `gh` asked
which branch the PR is on, so a ticket worktree that opened a PR minutes ago —
before anything cached the number — is still reachable by it. The difference is
about half a second.

### Setup

1. **Install the script.**

   ```sh
   ./setup.sh wt-jump
   ```

2. **Install fzf** if you want the no-argument picker. `brew install fzf`.
   Jumping by ticket, PR, or branch needs nothing but tmux and git; only the
   picker and the `M-j` binding below need fzf.

No shell function, so nothing to add to your shell config. Unlike `wt` and
`wtr`, this one never has to move your shell — tmux does the moving, and a
child process can ask tmux to do that.

### Notes

- To pick a window by looking, `M-j` opens mission control's Work map showing
  only open tabs, a filter typing (see tmux-sidebar). `wt-jump` itself has no
  picker: give it a ticket, PR or branch, or `--list` to print every window.
- Every pane is considered, not just each window's active one, and the pane
  that matched is the pane you land on — so a window holding claude beside a
  shell in the worktree puts you in the right half of it.
- Windows in other sessions are matched too, and the session is switched along
  with the window.
- One match jumps outright. Several — the same worktree open twice, or a branch
  fragment that is not unique — print a short numbered list, first one on Enter,
  the same prompt `worktree-resume` uses.
- Ticket matching is bounded on both sides, so `ABC-12` never lands you in
  `ABC-123`. A branch fragment is a plain substring, since that is what you are
  reaching for when you type one.
- Nothing is created. If no window is open for the identifier, the error says so
  and prints the `wt` or `wtpr` that would open one. `wtr` is the tool for a
  worktree that exists but has no window.
- `gh` is only consulted for a PR number that no `pr-<n>` worktree matches, and
  only against the repositories you have windows open in, so the offline cases
  stay offline.

## wt-revive

The way back to a worktree whose tab you closed: lists the worktrees no tmux
pane has open, for mission control's Work map, where `M-o` shows them and Enter
opens a tab in one, resuming its Claude with `claude-here`, as `wtr` does.

```
wt-revive          print the rows, from the current directory's repository
wt-revive <dir>    the same, starting from <dir>'s repository
wt-revive --json   each row as a line of JSON, for the Work map
```

The repositories are the one you are in, every open pane's, and every one a
Claude session ran in over the last two weeks, so a repository whose tabs are
all closed still counts. Each row is a worktree of one of them: its repository
and branch, how long since it was last used, and the last prompt Claude was
given there that says something ("yes" and "go ahead" don't), most recent
first. Worktrees whose directory is gone are left out.

What each transcript says, its last directory and last prompt, is kept in
`~/.cache/wt-revive/index.json` (readable only by you, since it holds prompts)
against the file's size and modified time, along with the repository each
directory belongs to. A press reads only the transcripts that changed since the
last one, so it takes about a quarter of a second rather than the three the
version that read the end of every transcript each time took.

### Setup

```sh
./setup.sh wt-revive
./setup.sh tmux
```

`M-o` in the tmux config opens mission control's Work map showing only these
worktrees, a filter typing, read from `wt-revive --json`, which prints each row as
a line of JSON; Enter there reopens one. `wt-revive` itself only prints the rows.

### Notes

- A worktree counts as open when any pane is in it or below it.
- The session shown is the newest one that ended in the worktree or a
  directory below it. A session that started elsewhere and moved into the
  worktree belongs to the worktree.
- A worktree with any tab still in it isn't listed, the main checkout
  included. A closed tab that shared its directory with other tabs is reopened
  by hand: a new tab there and `claude --resume` to pick the session.

## tmux-sidebar

The tabs as a sidebar down the left of every tmux window, ordered by what needs
you, and mission control, every tab at once with its latest Claude turn, full
screen. tmux stays the control surface: the sidebar is a pane, mission control
a popup, and every hotkey still works from both.

```
tmux-sidebar daemon   run the daemon (tmux starts it; a second one exits)
tmux-sidebar stop     stop it; the sidebars wait for the next one
tmux-sidebar status   whether it is running
```

The sidebar lists the session's tabs in groups: **Needs you** (a Claude
session waiting on an answer, the longest-waiting first, then one that finished
while you were elsewhere, the oldest first), **Working**, **Monitoring** (a
Claude whose only background work is `Monitor` watches, waiting for one to
report, each named, "monitoring · CI on #12"), **In merge queue**
(in queue order, each with its position), **Merged**, **Idle**, Claude sessions with nothing to do, by number, and
**Other**, the tabs with no Claude in them (the Slack relay, Notes, a shell).
A tab's PR comes from its label, and whether it is queued or merged from one
GitHub query for all the tabs' PRs a minute, through `gh`, so the sections can
lag GitHub by up to a minute. A tab whose worktree is gone, as `queue-and-clean`
leaves it once its PR merges, is found by the worktree's branch instead, and
shows that PR again. A Claude tab whose turn ended with background tasks still running, such as a review loop's Codex runs or a CI watch, is listed under Working, with a slate spinner and "working in background · 2 tasks", not as finished: Claude carries on when they end. The tasks are counted from its transcript, those started since its process started that no task notification has reported ended. When what runs is a known loop, the tab says so instead, with a spinner of its own: "review loop · round 2" (violet) while `codex-review` runs, two to a round since `/review-loop` last began; "CI loop · round 3" (teal) while `/ci-loop` watches the checks, one watch a round, a new one after each fix; and "queue-and-clean · watching" (orange) while `/queue-and-clean` watches the merge queue. A self-paced `/loop` (one with no interval, such as `/loop /oncall`) is looping between its ticks rather than idle: while the wakeup its last tick scheduled is pending, and for five minutes after it is due, the tab shows a steady `↻` (blue) with when the next tick is and the loop's prompt ("next in 8m · /oncall"), and both the sidebar (after Idle, before Other) and mission control put it under Loops; a tick that schedules nothing, or a wakeup with `stop`, ends it. A loop on a timer, a cron job Claude created (`/loop 5m …`, or a session asked to check something every few minutes), shows the same way, with the first sentence of its prompt, until Claude deletes the job, the session ends, or seven days pass. A tab can also be pinned by hand, with `M-f` (or `tmux set -w @pinned 1` in it): it goes under Needs you, ahead of everything else there, with a yellow `📌`, whatever Claude is doing there, and stays until it is unpinned the same way (`M-f` again, or `tmux set -wu @pinned`), since visiting it doesn't clear it. Needs you always wins: a tab whose Claude waits
on you stays there even while its PR is queued, and its details say so. An
idle tab whose PR is queued wears an orange `»` badge, and one whose PR
merged a purple `◆`. A merged PR's tab still at work on it, `deploy-test` waiting for the deploy or recording its test, or Claude working under `/deploy-test` or the `/queue-and-clean` that follows the merge, is under Testing Deploy, just before Merged, with a magenta spinner and "testing deploy". The same query brings each PR's checks, review and size, which a card shows as "✓12 ✗1 ⏳3 · approved · +340 −80", and a failing check is in the tab's line in the sidebar too ("✗1 failing"). Mission control splits them finer, into stages, packed side by side so the screen fills: each block of a stage's cards is boxed in a rounded border in the stage's color, titled on its top edge, a stage that doesn't fit in the room left on a row starts the next one, and one too big for a row is a single taller box with its cards in rows, and cards grow taller when the rows leave room. The stages are Needs you, Working, Monitoring, Review loop, CI loop, Queue-and-clean, Loops, In merge queue, Testing Deploy, Merged, Idle, then Other. A tab whose PR is in the merge queue or merged is in that stage even while it runs a loop, such as `queue-and-clean` watching it; otherwise a tab running a loop is in that loop's stage; `j`/`k` move a row, keeping the column. A working tab also says what Claude is doing ("working 2m · Run the test suite", "editing render.js"), and a waiting one what it waits on ("permission: Bash git push", "asks: Which layout?"), as its hooks record them. A tab running a loop is that loop's from the moment you ask for it: while Claude works between rounds, triaging or fixing, it keeps the loop's badge and stage and says "working 2m · review loop · Fix the lint error". Each tab shows its state as a colored badge (a yellow `!` waiting, a
green `✓` finished, a blue spinner turning while it works), its number for
`M-1`…`M-9` (and `M-0`, which asks for a number, for the tabs past 9), its name and PR as in the tab bar, and how long it has been in
that state. The tab you are in is highlighted, and the status line drops its
row of tabs to one line: the session, its counts, and the focused window in
full. The states are the ones `tmux/agent-state.sh` keeps, so they mean what
the tab badges meant.

Mission control's views, which `Tab` and `]` go through, are its tabs, then the built-in views, then the plugins'. In a built-in view, Enter on an item opens a menu of what can be done with it, its usual action first so Enter twice does what Enter alone used to, `j`/`k` moving to an action and Enter taking it (a menu takes no letters), the action named at once as it starts: the Work map's go to the tab or open one there, diff, lazygit, remove a merged worktree and copy its directory; Closed's reopen, remove and copy; Slack's open, dismiss and dismiss all; the Calendar's join the call and open the event; and the boards' own. `v` and part of a view's name ("cal") lights that view in the bar as you type, and Enter goes there. `M-p` has a "Mission control: <view>" command for each built-in view, which opens mission control on it.

`M-S-f` searches everything at once (`tmux/search.sh`, `lib/search.js`): it asks for a search, then lists what your notes (every word, in any note but the deleted ones, read on the Mac and sent nowhere), your ticket tracker (Linear's or Jira's issue search), GitHub (issues and pull requests in the repositories your tabs are in) and Slack (its own search, through Claude's connector, a headless Haiku allowed that one call, a few seconds after the rest) hold for it, each source's results as it answers; typing narrows them, Enter reads one beside the pane in the link view, a note as its Markdown, and Ctrl-O opens a link in the browser. GitHub and the ticket tracker are left out while their settings are off; `search_slack` leaves out Slack, and `search_everything` (both under Integrations) turns the key off. A link can be read in a pane beside the one you are in rather than in the browser (`tmux/link-view.sh`): `M-p`'s "Read a link from this pane" lists the links the pane shows, newest first, or, in the notes pane, the links of the note it shows, top to bottom, since Notes hides a link's address behind its text, a link written into text (a terminal hyperlink, as Claude Code and glow print them) with its text before it so it is easy to find, and Enter reads one beside it (Ctrl-O opens it in the browser instead), and "Open a link from this pane in the browser" lists the same links with Enter opening one in the browser (Ctrl-O reads it beside instead), "Copy a link from this pane" lists them with Enter copying one to the clipboard (Ctrl-O opens it), and mission control's menus offer "Read it here" for a pull request, a ticket or a meeting's links. A GitHub pull request is laid out as `M-v` lays out the branch's, a Linear ticket as `M-i` does, a Notion page or a Google Doc or Slides deck as Claude's connector gives it (a headless Haiku allowed that one call and nothing else, its answer taken from the call itself, in about 15 seconds; the `link_view_docs` setting, under Integrations, turns it off), and any other page as its main text in Markdown (`lib/link-view.js`, with turndown); a Slack message only offers to open, since nothing but Claude's connector reads Slack. `q` closes it, and `M-p`'s "Open the link being read in the browser" opens it there. Every link Switchboard opens goes through `tmux/open-url.sh`, in the Chrome profile `~/.config/switchboard/browser.json` gives its host (`{"hosts": {"linear.app": "Profile 2", "slack.com": "Profile 2"}}`, the profile's directory as `chrome://version` shows it), else the default browser, as GitHub's links do; `M-A`'s AWS login (`tmux/aws-login.sh`) opens in the profile it gives `awsapps.com`, AWS's access portal; the file is yours and never in this repository. A view that is on stays in mission control even before what it reads from is set up, and says what is missing and how to set it up: the Calendar its iCal address or a Calendar tab, Tickets and Inbox a Linear API key, PRs and CI GitHub's CLI signed in (or a tab in a GitHub repository), Slack its connector in Claude. Each built-in view but Settings can be turned off under Mission control in Settings; the Work map stays for `M-j` and `M-o`, which open it. The **Work map** is every piece of work in the repositories your tabs are in, a row each: each worktree, and a main checkout a tab is in, with its ticket (from the branch), the tab it is open in or "no tab", its PR and where that stands (open, failing checks, in the merge queue, merged), and its directory, the ones needing you first and those no tab is in last, so work in flight is never forgotten. Worktrees with no tab come from `wt-revive` too, so they cover every repository a Claude session used in the last two weeks, most recently used first, each with when it was last used and the last prompt given there. Enter goes to its tab, or opens one in a worktree that has none and resumes its Claude with `claude-here`. `/` filters it, as it does any list but the PR board, by title and detail. `M-j` opens it showing only open tabs and `M-o` only worktrees with no tab, a filter already typing: the arrows move, Enter goes, Esc closes. Its worktrees are read with git, and a worktree's PR looked up by its branch, in the same minute's query as the tabs'. Its rows line up in columns, the chosen row adding its directory and last prompt. Every open tab has a row, one in no repository too. Merged work no tab is in sits last under "Merged, can be removed", closed until Enter on it; Enter's menu on one removes its worktree after asking, and keeps one with changes, saying why. **Closed** is the tabs closed most recently, newest first, one per Claude conversation, so several closed in the same repository are each there, leaving out a conversation a tab has open again (a tab with no Claude, one per directory no tab is open in); Enter's menu removes one, which comes back only if that conversation is closed again: Enter reopens one there with its Claude session resumed (`claude --resume`), or a shell if it had none. Each refresh compares the tabs with how they were, and a tab gone since is recorded with its directory and session in `closed.jsonl` in the state directory, the newest 200 kept, so a restart doesn't lose them. **PRs** is a board of your pull requests in the repositories your tabs are in, in columns by where they stand: Draft, In review, Approved, In merge queue, and Merged in the last 7 days, each with its checks and review, failing checks or changes asked for marked as needing you. `h`/`l` move between columns, and Enter asks what to do with the PR: go to the tab it is open in, work on it in a new tab (`wtpr`), open it in the browser, or copy its link, each with `j`/`k` and Enter. It is fetched every two minutes through `gh`, the merged PRs only every ten, since they rarely change; only those repositories, so old pull requests elsewhere stay off it. **CI** is your open pull requests whose checks are failing, in the repositories your tabs are in, each with the checks that failed and the line of the first failing job's log that says why. Enter offers: start a CI loop on it (`/ci-loop` in the tab already on it when its Claude is idle, else in a new tab on its worktree through `wtpr`; Switchboard's `/ci-loop` follows the repository's own when it has one), go to its tab, read the failing log in a popup, open the failing check or the PR in the browser, or copy its link. It is asked of GitHub every two minutes and when you go to the view, while the `github` and `view_ci` settings are on. `M-m` asks for the PRs, CI and Work map views to look again as it checks a PR, so they agree with what it says. While you're away (no tmux client attached, or the Mac idle 10 minutes) the PR, CI, Inbox, Tickets and Calendar lookups pause, and all ask again when you're back; the `pause_when_away` setting (Mission control) keeps them going. **Inbox** is your unread Linear notifications that need you, newest first: a mention of you in a ticket or a comment, a new comment on a ticket of yours, a ticket newly assigned to you; status changes and the rest of Linear's inbox are left out. Enter offers what the Tickets board does with its ticket (go to its tab, `wt`, `shape`, open, copy) and to mark it read in Linear. Its count shows in the sidebar's notifications too, and a click there opens it. It is asked of Linear every two minutes and when you go to the view, while the `linear` and `view_inbox` settings are on. **Tickets** is a board of the Linear issues assigned to you, in columns by where they stand: Todo (urgent first), In progress, In review (a started state whose name says review), and Done in the last 7 days, each with its state, priority, project and the tab working on it, an urgent one marked as needing you. `h`/`l` move between columns, and Enter asks what to do with the ticket: go to the tab it is open in, start work on it (`wt`) or shape it (`shape`) in a new tab, close it (moving it to its team's first completed state, after asking), open it in the browser, or copy its ID. A new tab starts in a directory of the ticket's repository, one a tab with a ticket of the same team is in. It is fetched every two minutes from Linear with the key the ticket scripts use, and is gone while the `linear` or `ticket_board` setting is off. **Calendar** is today as Google Calendar's Day view shows it: all-day events across the top, then the hours, each event a colored block from its start to its end, side by side where they overlap, a red line at the time now, events already over faded and those you haven't said yes to in outline. `j`/`k` choose an event, `h`/`l` move between events side by side at the same time, and Enter offers to join its call, answer an invitation yes, maybe or no (through Claude's Google Calendar connector, as Google's own buttons do, so the organizer is told; from the feed only, and the `calendar_rsvp` setting turns it off), open it in Google Calendar, or open the tickets and pull requests its description links to. From five minutes before a meeting until ten minutes after it starts, the sidebar's notifications show it ("Standup in 3m", "Standup now") with the tickets and pull requests it names, and Enter or a click there joins its call, or opens the Calendar view when it has none; the `next_meeting` setting (Sidebar) turns that off. The tickets and pull requests come from the calendar feed, which `/today`'s saved day doesn't keep. With your calendar's secret iCal address in the Keychain, mission control reads the calendar itself every five minutes, and when you go to the view: in Google Calendar, Settings, your calendar, Integrate calendar, copy "Secret address in iCal format", then run `security add-generic-password -U -s google-calendar-ical -a "$USER" -w` and paste it at the prompt. Without it, it draws the day `/today` last saved; the Calendar tab (`M-p`, "Open a Calendar tab") runs `/today` every 30 minutes. A day saved before today, or over 45 minutes ago, is marked out of date, and when no tab is keeping the calendar, mission control opens a Calendar tab in the background to catch up. It appears once `/today` has saved a day. **Slack** is what is new for you on Slack in the last day, newest first: direct messages and mentions marked as needing you, then replies in threads you started and everything in the channels listed in `~/.config/switchboard/slack-updates.json` (`{"channels": ["C…"]}`). Enter shows a message whole, above what can be done with it, and opens it in the browser, in the Chrome profile `~/.config/switchboard/browser.json` gives Slack (see the link view). Slack's connector can't say what you have read, so the view counts as read anything before a message of your own in the same DM or thread (you answered it in Slack), and remembers what you have seen in it: leaving the view, or mission control, marks what was on screen as seen, picking anything from a message's menu (in the view or the Notifications view) marks that message seen, and seen messages sit below the new ones, faded. Enter shows the message whole above its menu, drawn as Slack draws it (its lines, bullets, numbered items, quotes, code blocks, bold, italic, strikethrough and code, links and mentions as their text), as the Notifications view does; the menu opens a tab for its thread with Claude (through `M-t`'s popup, so you pick its directory; Claude starts on a prompt with the thread's link and the message, to read the thread through the Slack connector and say what it asks of you, drafting no reply unless you ask), and the menu dismisses the chosen message, or every one shown: gone until something newer comes in its thread, a reply to it or to the thread it is in. New messages, ones not yet seen in the view nor dismissed, also show at the bottom of every sidebar, under a double yellow rule and a Notifications heading, so they read as no tab: "3 new on Slack" and the newest one's sender; moving onto them with `j` past the last tab, or a click, brings a Notifications view to the front as moving onto a tab does: every notification with its whole message (each new Slack message, each Linear inbox item, the next meeting, each note written for you: a meeting's from Fathom or the standup, until Notes shows it, its menu opening it in Notes or dismissing it, the `new_notes` setting), in a window of its own that is no tab and stays out of the tab bar, the sidebar's cursor still on the notification; `j`/`k` in the sidebar or the view move between them, Enter in the sidebar goes into the view, Enter in the view opens the chosen one's actions over it, as its mission control view offers them, with Dismiss it last, and whatever is picked dismisses it from the notifications (a Linear inbox item or a meeting only here, Linear not told), and Esc there goes back to the tab you came from; the `notice_list` setting (Sidebar, "Notifications view") keeps them single lines that open their view in mission control instead; the `sidebar_notifications` setting (Sidebar, Notifications) hides the block. The first look goes back an hour. Mission control gathers it itself every minute while you're at the Mac, through Claude's Slack connector, one search at a time in a headless Haiku that keeps no session, reading only, never the relay's own messages or DM; the `slack_background` setting (Mission control) turns that off, and then the `/slack-updates` skill does it in a Slack tab (`M-p`, "Open a Slack tab") every 5 minutes, and going to the view asks that tab to look again at once when its last look is over a minute old. Over 15 minutes old, it says so, and mission control opens a Slack tab in the background when none keeps it. It is there before the first look too, saying so, and going to it then opens a Slack tab. **Sessions** lists each Claude tab's session with how full its context is, its model, how long it has run and how long since it last did anything, fullest first, a session at 80% or more marked as needing you: the ones to compact or close. The context and model are what Claude Code gives the statusline, which writes them to `~/.local/state/tmux-sidebar/context/<session>`. **Usage** (`M-p`, "Mission control: Usage") is what Claude has cost today, at API prices as Claude Code estimates it, across the interactive sessions that ran today (a headless `claude -p` has no statusline, so it isn't counted), and how much of each rate limit your subscription has used (the 5-hour and weekly ones, or a gateway's spend limit) with when it resets, one at 80% or more marked as needing you; the statusline writes these to `~/.local/state/tmux-sidebar/usage/`, the limits as the last session to draw saw them. The `usage` setting (Mission control) turns the view off.

| Key | In the sidebar | In mission control |
| --- | --- | --- |
| `M-s` | jump in from any pane, on the tab you are in, and back out | |
| `M-c` | into the pane running Claude in the tab you are in, from the sidebar or a pane beside it | |
| `C-w h` / `C-w l` | move in, on the tab you are in, and back out, as between any panes | |
| (closing the tab in front) | the cursor lands in the sidebar of the tab that takes its place, to pick the next one | |
| `M-Space` | open mission control | close it |
| `M-w` | close the pane | close it, whatever is open over it |
| `j` `k` (arrows) | choose a tab, bringing it to the front (the list holds still until you leave the sidebar) | move down or up a row |
| `h` `l` | back out / go to the tab | move left or right |
| `Enter` | go into the chosen tab's pane | what can be done with the chosen card or row, in a menu: on a tab, go there (first, so Enter twice goes), answer it, peek at it, its diff or lazygit, the `M-p` palette there, and on a merged tab close every Merged tab; elsewhere the view's own actions |
| `1`–`9` | put the cursor on that tab, in its sidebar (Enter goes in) | choose that tab |
| `n` | to the notifications at the bottom, when there are any (the Notifications view, as moving onto them does), else it says there are none; `M-p`'s "Go to the notifications" does the same from any pane | |
| `t` | a new tab running Claude in the chosen tab's directory, as `M-t` opens one | |
| `d` `L` | the chosen tab's diff (diffview, as `M-d` there) or lazygit, in its place there, as `M-p` opens them | in Enter's menu |
| any bound Alt or Ctrl key | runs in the chosen tab | |
| `Tab` `S-Tab` | open mission control | the next or previous view |
| `r` | | fetch the plugin's view again |
| `Esc` `q` `C-c` | back out | close |
| `a` | answer the chosen tab, as in mission control, in a popup | in Enter's menu: answer the chosen tab's question or permission prompt there: an option's number, `s` to submit one that takes several, `t` to type your own answer; with no prompt showing, type a message and press Enter |
| `Space` | peek at the chosen tab, as in mission control, in a popup | in Enter's menu: peek at the chosen tab without going there: its whole last turn, or with `s` its screen (`r` captures it again); `jk` scroll, Enter goes there |
| `/` | find a tab by title: type, the arrows move, Enter goes, Esc clears; in every window's sidebar until you leave it | filter the cards by name, number, PR or directory; Enter keeps the filter, Esc clears it |
| `?` | these keys, in a popup over the window, until the next key | these keys, in a box over mission control, until the next key |
| the mouse | a click on a tab goes there, a right click peeks at it, the wheel moves | a click on a view's name switches to it; a click on a card or row chooses it, a second click goes there (Enter); the wheel moves |

A click on a tab in the sidebar goes there, and the wheel scrolls it. Each
mission control card shows a tab's state, PR and session name, your last
prompt, the end of Claude's last reply, and, when it is waiting on a
permission prompt or a question, what it is asking. The sidebar is turned off
and on with "Turn the sidebar on or off" in `M-p` (the `@sidebar_on` option).

### Setup

```sh
./setup.sh tmux-sidebar
./setup.sh tmux
```

then reload tmux. tmux starts the daemon through `tmux/sidebar.sh`, which puts
node on the PATH from nvm's default alias.

### Plugins

Anything with an API can be a view in mission control and, if it asks, a
short list in the sidebar under the tabs: a plugin fetches a list of items,
and Enter on one opens it. List the ones you want in
`~/.config/switchboard/sidebar.json`, with any options under each name:

```json
{
  "width": 34,
  "plugins": {
    "github": { "query": "org:my-company", "sidebar": 3 }
  }
}
```

`github`, kept in `lib/sidebar/plugins/`, shows your open pull requests and the
ones waiting on your review, through `gh`, marking the ones that need you:
a review requested, checks failing, changes requested. A plugin of your own
goes in `~/.config/switchboard/sidebar-plugins/<name>.js`:

```js
module.exports = {
  title: "Inbox",
  intervalSeconds: 120,   // how often fetch runs (default 300)
  sidebar: 3,             // items shown in the sidebar (default 0)
  async fetch({ options, run, keychain, log }) {
    const token = await keychain("inbox-token");   // from the macOS Keychain
    const response = await fetch(options.url, { headers: { authorization: `Bearer ${token}` } });
    return (await response.json()).map((m) => ({
      id: m.id, title: m.subject, detail: m.from,
      state: m.unread ? "attention" : "idle",   // or "active"
      url: m.link, time: Date.parse(m.date),
    }));
  },
  // optional; without it Enter opens item.url in the browser
  async open(item, { options, run }) {},
};
```

`run(command, args)` runs a program without a shell and returns its output;
`keychain(service)` reads a secret stored with
`security add-generic-password -s <service> -a "$USER" -w`, so no token sits in
the config. A fetch that fails keeps its last items and shows the error. The
config is read again whenever it changes.

### Notes

- One daemon draws everything. tmux has no sidebar of its own, so each window
  gets a narrow pane marked `@sidebar`, running `tmux/sidebar-pane.zsh`: a
  zsh client built from zsh's socket and select modules, so a sidebar is one
  small process that starts no others. It shows what the daemon sends over
  the Unix socket in `~/.local/state/tmux-sidebar/` (readable only by you) and
  sends back the keys pressed in it. A sidebar whose daemon goes away
  reconnects to the next one, starting it if need be.
- The daemon reads tmux with one `list-panes` when told something changed:
  `agent-state.sh` and `window-names.sh` append to
  `~/.local/state/tmux-sidebar/nudge` with a shell builtin, and a few tmux
  hooks do the same. Otherwise it looks every five seconds. Claude's sessions
  and transcripts are read from `~/.claude` as files; nothing on the drawing
  path starts a process or waits on the network.
- The sidebar keeps its width and its place on the left. A tab whose last other
  pane closes closes too, as it would have without the sidebar. Windows
  narrower than 90 columns and zoomed panes are left alone.
- Bindings that act on "the pane" (splits, close, zoom, search, resize, the
  lazygit, nvim and shell panes) start with `leave-sidebar`, which hands
  focus back to your pane first when the sidebar has it.
- tmux-resurrect saves the sidebars as panes; they come back as shells with
  the sidebar's title and are given their client again. While a restore runs
  (`@sidebar_hold`), no sidebar is added.
- `wt-jump` leaves sidebar panes out.
- The log is `~/.local/state/tmux-sidebar/daemon.log`.

## claude-reply

Prints the last reply Claude gave in a tmux tab, the final text of its last
turn, read from the session's transcript the way `slack-relay` reads it.

```
claude-reply 3            # tab 3's last reply
claude-reply --pane 3     # the pane to type into: its Claude pane
```

It is what the M-p action **Send Claude's last reply to another tab** runs
(`tmux/reply-send.sh`): from a tab where Claude has replied, pick another tab,
and the reply is pasted into its Claude's prompt, or its active pane when it
runs no Claude, without pressing Enter, and that tab comes to the front so
you can add to it before sending. The action has no hotkey.

## worktree-done

Reads every worktree in the repo, asks the ticket tracker (Linear or Jira)
what state each one's ticket is in, and offers up the ones that are finished — completed, merged, deployed,
canceled, whatever your workflow calls them — in an fzf multi-select that
removes what you pick. The answer to "which of these forty directories can I
delete", and the deleting.

```
worktree-done           pick finished worktrees to remove
worktree-done --list    print them instead, removing nothing
worktree-done --all     print every worktree grouped by ticket state
worktree-done --paths   print only the finished worktrees' paths, for pipes
```

Keys in the picker: Tab toggles, `C-a` takes
everything, Enter removes what is selected, Esc cancels. Taking the whole list
at once asks for a `y` first — every other selection was made row by row and
speaks for itself.

`C-a` selects what the current query matches rather than the entire list, so
typing `abc-6` and hitting it takes those and not all forty. It
costs fzf's default `C-a` (beginning-of-line) inside the query, which is a short
field here.

The worktrees this one will not touch, the ones with no ticket or whose ticket
is still open, are removed by hand with `git worktree remove <path>`.

### Setup

1. **Install the script.**

   ```sh
   ./setup.sh worktree-done
   ```

2. **Install fzf.** `brew install fzf`. Without it the command prints the list
   and says so, rather than removing nothing in silence.

3. **Add your tracker's key**, if you have not already. It reads the same key
   `worktree-from-ticket` reads, so if `wt` works this does too.

No shell function, so nothing to add to your shell config.

### Notes

- Finished means Linear's own `completed` or `canceled` state type (in Jira,
  the Done status category), so a custom state like "Shipped" or "Duplicate"
  counts without any configuration. The state's real name is what gets printed.
- The ticket is read from the branch first and the directory name second. Those
  usually agree, but when they don't the branch is the truth — a worktree named
  for one ticket sitting on another ticket's branch is worth seeing.
- A leading `pr-<number>-` is stripped before the identifier is matched, so a
  `wtpr` worktree is never read as ticket number `<number>` on a team called PR.
- Worktrees whose name yields no ticket are counted, not skipped, and `--all`
  lists them. They are usually PR worktrees for branches that never had one.
- One request per hundred tickets rather than one per worktree, so a repo with
  forty of them answers in a single round trip.
- The main clone is left out. Git will not remove it and its branch is normally
  the trunk, so it is never a cleanup candidate.
- Removal is `git worktree remove` with no `--force`, so a worktree holding
  uncommitted changes or untracked files refuses to go. The refusal is reported
  with the `--force` command to run if that is what you meant, and the rest of
  the selection still goes.
- Only the checkout is removed. The branch survives, so nothing you committed is
  lost even if the ticket turns out not to have been finished after all.

## slack-relay

Relays between a Slack app's DM and the Claude sessions in your tmux tabs, for
when you are away from the computer. It runs in its own tmux tab. The command
is `bin/slack-relay`; its code is in `lib/slack-relay/`, one file per part,
which `./setup.sh` doesn't need to install, since the command finds it from its
own location.

```
slack-relay          open it in a tab named "slack-relay", or go to that tab
                     (starting it again there if it has stopped)
slack-relay stop     stop it (or C-M-s, `stop` in the DM, or Ctrl-C in its tab)
slack-relay notify [--from <name>] [<text>]
                     post a notice in the DM, from <text> or stdin
```

`slack-relay notify` is how something else on this Mac reaches you through the relay, such as the `oncall` skill: the notice goes out as the app's bot, under a 🔔 and the `--from` name, so Slack notifies you, which it never does for anything posted as you, a self-DM or a channel of your own included. It is queued in `~/.local/state/slack-relay/outbox/`, posted by the running relay within a second whether or not you're at the computer, and `notify` waits for it: it exits 0 once posted and 1 if it wasn't (the relay isn't running, or Slack refused), so a caller knows you weren't reached. It only ever posts in the DM, as the relay does.

`slack-relay` works from outside tmux too (a Claude job, ssh): it uses the
running tmux server, putting a new tab in its most recently used session.

A tab can hold other panes beside Claude, such as a shell, a server or
another Claude. The relay keeps each tab to its own Claude pane, whichever
pane you use: the first Claude pane it sees in a window (or the pane a `new:`
tab starts in) is recorded on the window as the tmux option `@claude_pane`,
and stays the tab's pane while it is open, through `/clear` or Claude being
started again in it. While that pane isn't running Claude, another Claude
pane in the window stands in; once it has closed, that one is recorded
instead.

When it starts, the relay says so in the DM with buttons for what you'd
otherwise type: **Home** first (a link to the app's Home tab, added once
Slack has sent the relay an event, since that is the only way it learns the
app's ID), then Status, Threads, Progress, New tab, Help and Stop relay, which
asks first. They answer at the top level, as the typed commands would.
Stopping on purpose edits that message to say the relay stopped, and when,
and starting again within a day edits it back, so restarts don't fill the DM;
a first start, an older message or a crash posts a new one.

Each Claude tab gets its own thread in the DM, started the first time there's
something to say about it: its alerts with their full turns, answers,
milestone pings all go there, and whatever you reply there is
typed into that tab. The main DM only gets one-line notices for what needs you
or went wrong (waiting on you, a failure), posted in the tab's
thread and also sent to the conversation, so tapping one opens the thread; you
can answer a notice as you would the alert, and a notice for a prompt carries
the prompt's option buttons, so it can be answered from the notice itself. A finished turn gets no notice,
since it is already in the thread. A request you send from the main DM (`3:
…`) goes on in the tab's thread, and its answer is sent to the main DM too. A tab's thread is watched for
as long as the tab is open, and a new window gets a new one.

In the DM:

- a reply in a tab's thread is typed into that tab's Claude. A message never
  names a tab (there is no `3: …`): a tab is reached through its thread, its
  screen on the Home tab (Open, then Message), or the buttons. A message at
  the top level is only offered to the tabs, with a button for each; tapping
  one sends it, and the conversation goes on in that tab's thread. Every
  button and form acts on the tab it was made for, whatever the thread it
  sits in was later linked to
- `?` in a tab's thread shows its last turn: your last prompt and Claude's
  reply since, from its transcript (the screen if there is none)
- `status` lists every Claude tab, its state, how full its context is, and a
  link to its thread, which opens at the thread's newest message
- each turn posted ends with how full the tab's context is and its model, as
  do each tab's line on the Home tab and its screen, read from the file the
  status line writes for the sidebar, so as of its last redraw. Once a
  session passes 90%, its thread says so once, with a **Compact** button that
  opens the Skill form with `compact` picked; it says so again only after the
  context has dropped below and passed 90% again
- `threads` lists a link to each Claude tab's thread, to find a conversation
  again; a tab with no thread yet says so, and sending it a Message from its
  screen on the Home tab starts one
- a new tab stopped at a dialog Claude Code shows before its session starts
  ("trust this folder", "new MCP server found") gets an alert too, with the
  dialog's options as buttons; tap one, or reply with its number or the start
  of it, and the relay moves to it with the arrow keys and presses Enter
- when a tab is waiting on a permission prompt or a question, the alert shows
  the question and its numbered options, read from the screen, with a button
  for each; tap one, or reply with the number or the start of an option. A
  button only answers the prompt it was made for, while that same wait is on,
  and is replaced by what was chosen once pressed. Only the number is pressed, with no Enter
  after it, since an Enter could accept a second prompt that follows. Anything
  else is refused while the choice is showing, rather than typed: it would end
  with Enter, which accepts whichever option is selected. The refusal carries
  the option buttons. Answering a prompt anywhere (a button on the alert, the
  notice or the Home tab, or a typed reply) clears the buttons from every other
  message showing it, and a prompt answered at the computer clears them all,
  so no button is left that would only be refused
- a question Claude asks you itself (AskUserQuestion) works the same, with
  more to it. When it asks several at once, Claude Code shows them one at a
  time in the same wait, which no hook reports, so after each answer the relay
  looks at the screen again and posts the next question, and then the review
  of your answers, each with its buttons. A question that takes several
  answers shows its options ticked ☑ or not ☐: a tap or a number ticks or
  unticks one, `1 3` does two, and Submit choices (or `submit`) moves on,
  reached with the arrow keys since a number only ticks. Any other reply to a
  question Claude asked is typed in as its "Type something" option, your own
  answer, which has no button since it needs your text; a permission prompt
  has no such option, so there it is still refused
- `progress` in a tab's thread, or Progress on its screen or thread, tells
  you what that tab is
  doing without typing anything into it: its state, when the turn began, when
  it last did anything and how many tools have run, then a few bullets from a
  one-shot Haiku that reads the turn's activity and the screen. That Haiku has
  no tools, MCP servers or settings, so nothing in a transcript can make it act;
  it takes about five seconds
- `progress` at the top level does the same for every
  Claude tab at once: one line each, most urgent first (waiting on you, then
  stuck or failing, working, finished, idle)
- `rename <name>` in a tab's thread, or Rename on its screen or thread, names
  the tab as `M-p`'s "Rename the tab" does: the tab bar, the second status line and its thread
  all show it. `rename` alone clears it, so the tab goes back to its ticket's
  or directory's name
- **Close**, on a tab's screen on the Home tab and its thread, closes it.
  Slack asks to confirm first, and the button only closes the
  tab it was drawn for (same window and process), so it can't close a tab that
  has since taken its place. Closing ends the tab's Claude session;
  `claude-here` in its directory resumes it
- `esc` in a tab's thread, or Stop on its screen or a working note, presses
  Escape in that tab, which stops
  Claude mid-turn or declines the prompt it is waiting on; a tab that isn't
  working or waiting is left alone, since a second Escape opens the rewind menu
- `new <place>: <prompt>` starts it somewhere else: `new myapp: …` in that
  repository's main checkout, `new ABC-214: …` (or any part of a branch name) in
  the worktree that matches, `new ~/code/thing: …` in that directory. Names are
  looked up among the repositories your tabs and recent Claude sessions use,
  as `M-o` does; a name that matches several places, or none, starts nothing
  and says so, and the reply names the directory and branch it picked
- an image attached to a message (a screenshot from your phone, say) goes to
  the tab with your text: the relay downloads it to
  `~/.local/state/slack-relay/files/`, readable only by you, and adds
  `[Image: <path>]` to what it types, which Claude Code reads as the picture.
  Only PNG, JPEG, GIF and WebP up to 20 MB, only from Slack's own file host,
  and only on a message you typed; anything else is answered, not fetched.
  Downloads are deleted after a week
- a video attached the same way (a screen recording of a bug) is downloaded
  too, up to 200 MB as MP4, MOV or WebM. Claude can't watch a video, so the
  relay takes eight stills spread evenly through it with `ffmpeg`, at most
  1280 pixels wide, and types `[Video: <path>]` and then the stills as
  images, in order. Its sound doesn't reach Claude. Without `ffmpeg`, or for a
  file it can't read, the tab gets only the path and you are told
- images and videos a tab makes come back the same way: when a turn is
  posted, image and video files Claude names in its messages (a Playwright
  screenshot or recording, a chart it saved) are uploaded under it, but not
  ones only its tool calls name, such as scratch images made along the way, where Slack shows the
  images and plays the videos. Only files that really are PNG, JPEG, GIF,
  WebP, MP4, MOV or WebM by their contents, written during that turn, up to
  20 MB for an image and 200 MB for a video, up to 20 a turn, shared ten to a message, a PNG more than twice as tall as it is wide (a long diff's screenshot) as slices about as tall as they are wide, in order, since Slack shrinks an image by its longest side until a tall one's text is unreadable, and never one you
  sent, so an old file a turn only mentions stays on the Mac
- `new: <prompt>` starts a new Claude tab with that prompt
- a finished turn has a **Skill** button, and when Claude ended by asking,
  the replies you would likely tap, added a few seconds later: **Yes, go
  ahead** when it asks whether to go on with what it proposed, and a button
  per option when it asks you to pick one of a few it named. A question mark
  alone can't tell those from a question only your own words answer, so the
  same tool-less Haiku as `progress` reads Claude's last message (only one
  with a question mark near its end), and anything else gets no buttons. A
  tap types the reply into the tab, and is refused once Claude has said more
  since; a note that a tab may be stuck has
  **Stop** (`esc`) and **Progress**, since that tab is still running
- anything that names no tab and isn't a command is not acted on, so a typo
  never starts work; the reply has a button per tab, and tapping one sends the
  message there. `help` lists the commands

When the relay is offline (the Mac off, truly asleep, or without internet),
Slack can still tell you, because the relay leaves the warnings with Slack in
advance. The Home tab opens with when your Mac last checked in, a time Slack
itself renders and keeps counting ("checked in 47 minutes ago"); the relay
publishes it every minute, so anything older than two means it is offline.
Beside it, **Refresh** publishes it at once.
And a message saying the relay is offline is kept scheduled in the DM about
ten minutes ahead, renewed every two minutes: if the relay goes quiet, nothing
pushes it back and Slack sends it, which notifies you. When the relay is back
it says so, and for how long it was gone. A clean stop cancels it.

The app's Home tab shows every Claude tab: its state, for how long, its last
activity and one line from Haiku on what it is doing or, once it has
stopped, where it left off (written again only when the tab does something), with an **Open** button for its screen, and for a tab waiting on a prompt, the prompt's options, which answer it from there. The tabs come in groups so what needs you is at the top on a phone: **Needs you** (waiting on you, then finished and not yet looked at, the longest first, as `M-a` walks them), then **Working**, then **Idle**. The relay publishes it when anything has
changed, once a minute at most, and whenever you open it.

A tab's screen (**Open**) is a Slack modal with its state, its last prompt and
the end of its last reply, the options of a prompt it is waiting on as
buttons, and buttons for **Message** (a field whose text is typed into the
tab, said in its thread and handled as a reply there, so a showing choice
still refuses anything but its options, and the answer goes in the thread),
its thread, **Progress** (a Haiku summary, shown in the screen), **Rename** (a field holding its name, cleared for its
own name back), **Stop** (Escape, only while it works or waits) and **Close**.

**Skill** runs a skill or slash command in the tab. Slack takes any message
starting with `/` for one of its own commands and never sends it, so
`/review-loop` can't be typed into a thread as it is at the keyboard. Skill
opens a form instead: a menu of the skills that tab can run, and an
Arguments field for what follows the name, such as a ticket ID. Picking a
skill redraws the form to say what that skill takes, from the
`argument-hint` in its front matter, which only skill and command files
have: a plugin or built-in skill, known only from the session's list, says it
doesn't say. Run types `/<skill> <arguments>` into the tab as a reply in its
thread would be typed, and the answer goes there.
The menu is the tab's own: the skills its session lists in its transcript,
plus those only you can invoke (which that list leaves out), read from
`~/.claude/skills` and `~/.claude/commands` and from `.claude` in the tab's
directory and the ones above it. It is grouped frequently used, yours, the
project's, then Claude Code's own commands that finish without asking
anything (`compact`, `clear`, `context`, `cost`, `usage`; one that opens a
picker would wait on a key Slack can't press), which answer in the tab rather
than the thread, then plugins and built-in skills. Frequently used is the eight you
use most of those the tab can run, from the counts Claude Code keeps in
`~/.claude.json`, each use counting half as much every 30 days, so what you
use now outranks what you used a lot months ago; a Slack menu holds 100 options, so a longer list
is cut from the end, and a field takes any name the menu left out. The same
Skill button sits under every
finished turn in a tab's thread. A thread reply that starts with a space
also gets past Slack.

An open screen is drawn again whenever its tab's state changes, so a prompt
that has been answered at the computer leaves it. Like the Close button,
every one acts only on the tab it was drawn for, and only the relay's owner
can open a screen or use one: Home and modal presses, and the forms sent from
them, are taken from you alone, and only the ones these screens draw. **New
tab** at the top of the Home tab asks where and for the prompt, as
`new <place>: <prompt>` does. Where is a menu of every checkout a Claude
session has ever run in that still exists, at its top level (a session in a
subdirectory counts for its checkout), and every open tab's, newest first,
grouped by repository (a worktree marked ⎇ by its directory), with directories
outside a repository last, at most the 100 a Slack menu holds; a field beside
it takes anything else `new <place>:` would. The bottom of the Home tab says what the Haiku
summaries and `progress` have cost today, from what `claude -p` reports for
each (about $0.0002 a call), also logged a line a call. When the relay stops
on purpose it says so, since nothing in it is current any more. Each tab's
thread starts with a header naming the tab, with its state and since when
(🔴 waiting on you, 🔵 working, ✅ finished, ⚪ idle) and buttons for
Progress, Skill, Rename and Close. The header is edited whenever the
tab is renamed or its state changes, so Slack's list of threads shows how
every tab stands, and once the tab closes or its window runs another session
it says the session has ended and offers **Delete thread** instead.

Slack doesn't let you delete the app's messages in its DM, only your own, so
the relay deletes a closed tab's thread for you: Delete thread on its header,
or **Delete closed threads** on the Home tab and under `cleanup`, all
confirmed by Slack first. It deletes the header and every message the app
posted in the thread, with the files it uploaded there. Your own replies
stay, since Slack doesn't let an app delete them, and the relay says how many
were left. Only threads of tabs that have closed (recorded as they close)
can be deleted, never a running tab's, and nothing deleted can be brought back. Answers longer than one Slack message go out as
several.

Everything it posts is standard Markdown, the way Claude writes, sent as
Slack markdown blocks, so headings, **bold**, [links](…), lists, tables and
code render as they do in the terminal, rather than as Slack's older mrkdwn,
which shows most of that as raw symbols. A tab's screen is a modal, which renders only mrkdwn,
so its "Claude said" is converted: a table there becomes a bullet a row.

It posts an alert when a tab's Claude is waiting on you or has finished, from
the state `tmux/agent-state.sh` keeps. A turn that ends in the tab you're
looking at isn't marked finished in tmux, since you saw it; its thread gets it
all the same, from the hook's event and the final message it carries. A
Calendar or Slack tab (`tmux/loop-tab.sh`, marked `@loop_tab`) finishes a turn
on every tick, so it alerts only when it waits on you. You
count as away once nothing has been typed or clicked for 30 seconds (macOS's
own idle time), or at once when the screen is locked, read as the sidebar
reads it (`lib/presence.js`).

Each tab's thread always gets everything at once: finished turns, waits,
dialogs, stall notes and milestone pings, so it is a complete record whether
you were at the computer or not. What notifies you is the one-line notice in
the main DM (waiting on you, a dialog, a stall, a failing test), and those wait
while you're at the computer, since tmux already shows them, and go out in the
order they happened once you've been away 30 seconds. Replies to what you send
from Slack, and the results of requests you made there, never wait. Held
notices are kept in memory, so stopping the relay drops them. Writing to the relay from Slack counts as being away for five
minutes from your latest message, and sends what was held, since
you're likely on your phone; unless the Mac's keyboard or mouse was used in the
five seconds before it arrived, which means you wrote it in Slack on the Mac. An alert or answer for a tab that is
waiting also shows the end of its screen, since a permission prompt and its
numbered options are only there, not in the transcript; reply with the number. Tab states are read every second.

Slack pushes each message to the relay the moment you send it,
so it answers in about a second and costs nothing while you're quiet. Replies
under anything the relay posted in the last day (up to 30 threads), and under
each tab's thread for as long as the tab is open, are part of those
conversations. The log records how long each
message waited to be handled. It keeps the Mac awake while it runs, but not
its displays, which sleep as usual. On a MacBook with the lid closed, let the
external monitors sleep rather than switching them off: with no display
connected, a closed MacBook sleeps whatever keeps it awake (clamshell sleep).

When it types your message into a tab, it posts nothing: your message's
reaction says where it is, ⏳ while it waits behind a turn already running, 👀
once that tab's Claude takes it, and ✅ once the answer is posted. While
it runs, a "Working on it" message with **Progress** and **Stop** buttons sits
where you asked, and is deleted once the answer is posted. If a tab that isn't busy hasn't taken it after 30 seconds, it posts
the tab's screen, since a dialog is usually what's in the way.

A tab marked working whose transcript hasn't changed for ten minutes gets a
note in its thread (what it did last, and its screen) and a line in the main
DM saying it may be stuck. A long test run looks the same, and the screen
usually tells the two apart. Each stall is noted once.

It follows every Claude tab's transcript and posts one line in the tab's
latest thread (or at the top level) when a session commits, pushes, opens a
or opens a pull request. Test runs are not reported, failing or not: one
failing is routine while Claude works. A push counts only when git says where it went,
and that is not a local folder, so a push into a test's throwaway repository,
or a quiet `git push -q`, isn't reported. A command that
never ran (blocked by a Claude Code hook, declined, or one the shell couldn't
parse) is never a failure. It doesn't watch CI: GitHub already notifies about
checks.

It knows a tab has answered from the Claude Code hooks: while it runs,
`tmux/agent-state.sh` appends an event to `~/.local/state/slack-relay/events.jsonl`
whenever a session takes a prompt, finishes a turn or starts waiting, naming its
transcript.
The file is removed only when the relay is stopped on purpose, and how far it
has been read is saved, so turns that end while it restarts are not lost.
When that transcript's last prompt is the text the relay typed, the relay posts
the turn within a couple of seconds. Sessions started before the hooks fall back
to the tab's screen staying still for 20 seconds.

If the relay crashes, it is started again within seconds, after a longer wait
each time, and posts that it restarted and why; messages sent meanwhile are
read back from the DM and still answered, as are those sent while its
connection to Slack was down. After five crashes in ten minutes it gives up and posts that it
is down. Stopping it (`stop`, C-M-s, Ctrl-C, closing its tab) is never taken
for a crash.

A lock file keeps it to one relay at a time. State, the events file and a log
(`relay.log`, which keeps what the tab shows) live in
`~/.local/state/slack-relay/`, readable only by you. The log says where each post went
and whether Slack notified you of it (the main DM does, a thread reply only
when also sent there), when you count as at the Mac or away and why (idle,
screen locked, writing from Slack), and each notice held while you were here;
the same failure again within ten minutes is counted, not written. Three old
logs are kept beside it, `relay.log.1` to `.3`.

### The Slack app, and keeping it yours

The relay is a Slack app of its own, `Slack relay`, which you talk to in its
Messages tab. It needs, in the app's settings at api.slack.com/apps:

- Socket Mode on, and an app-level token with `connections:write`. Slack
  connects to the relay over a connection the relay opens, so nothing on the
  Mac listens for connections
- bot scopes `chat:write`, `im:history`, `im:read`, `im:write`,
  `reactions:write` (for the 👀 it adds to your messages), `files:read` and
  `files:write`
- the bot events `message.im` and `app_home_opened`, and nothing else
- Interactivity on, for the buttons (with Socket Mode, no Request URL)
- App Home's Home Tab on, and its Messages Tab on, with messages allowed from it

Both tokens live in the macOS Keychain, never in a file; each command asks for
the token rather than taking it on the command line:

```sh
security add-generic-password -U -a "$USER" -s slack-relay-bot-token -w   # xoxb-…
security add-generic-password -U -a "$USER" -s slack-relay-app-token -w   # xapp-…
```

Anyone in the workspace can open a DM with an app, so the relay only acts on
messages typed by one member, named in
`~/.config/switchboard/slack-relay.json` (outside this repository):

```json
{ "owner": "<your Slack member ID: Profile → ⋯ → Copy member ID>" }
```

Everything else is ignored and logged, with no reply, so the app doesn't even
say it's listening: other members, other conversations, other workspaces, bots
(the relay's own posts included), edits and deletions. A message has to have
been typed, too: other apps can post as you (anything holding your Slack user
token, the Slack connector your Claude sessions use included), so a message
counts only if it carries the `client_msg_id` Slack's own apps give what you
type, which the Web API doesn't let an app set, and no `app_id`. Otherwise a
Claude session that read something malicious could post as you, in a tab's
thread, and drive that tab. For the same reason reactions control nothing: Slack
can't say whether you added one or an app did as you. The owner comes from
that file, never from a message. Everything the relay writes is refused before
it leaves the Mac unless it is for the app's DM with you, whatever asked for
it. Buttons are trusted the same way: a press counts only if it is yours, on a
message in that DM, and Slack only reports a press a person made in Slack, so
no app can press one as you. The app subscribes to no channel events and has no scopes to read
channels, so adding it to one shows it nothing.

```sh
./setup.sh slack-relay
```

Its tests run against a tmux server of their own, a temporary home directory
and stand-ins for Slack, so they never type into your tabs or post anything:

```sh
node --test test/*.test.js
```

## notes

The macOS Notes app's notes, in the terminal, edited with vim.

```
notes
```

It opens Neovim with your own config, so your keys and colors work as they
always do, plus the notes plugin in `notes/`, added for that run only, so your
normal `nvim` is left as it is. On the left is a list of folders, tags and
notes; on the right, the note you had open when you last quit, or the newest
when that one is gone. A note is a Markdown file, drawn the way Notes
shows it: markup is hidden (it shows only on the line you are typing in), a
title stands out from headings and subheadings, list markers show as • and –,
checklist items as ☐ and ☑, a quote with a bar, a table with box lines.

Notes are kept in `$NOTES_DIR`, or `~/Notes`, one Markdown file each, in
folders that are directories. A note's title is its first line, and its file
is named for it and renamed when it changes. They save themselves a moment
after each change. Pinned notes are listed in `.notes.json` there, and deleted
notes go to `.trash/`, the Recently Deleted folder; deleting one from there
removes it for good. While it runs, the app holds `.notes.lock` there, with its
process ID: notes save themselves as you type, so a second app on the same
notes would write over the first's, and instead it says the notes are open
already and closes with `q`. A lock left by an app that has gone is taken over.
Nothing else is stored, so any editor can open them, and
they never leave the Mac: Copilot isn't loaded in the notes app at all, and
completion and the undo file are off in a note.

Each weekday from 6am, the sidebar daemon writes a standup draft into the
Standups folder, `Standup <date>.md` (`lib/sidebar/standup.js`): the last
working day (Friday's on a Monday), with the pull requests you merged, opened,
worked on and reviewed, the tickets assigned to or made by you that
changed state, from where each started to where it ended, and your commits in
the repositories your tabs are in, a squash merge left to its PR; then Today,
your tickets in progress and the reviews asked of you in the last two weeks;
and an empty Blockers list. It reads GitHub through `gh` and the ticket
tracker with the ticket scripts' key, while their settings are on, and tries again every 15
minutes while one can't be read. A note already there is never written over,
so edit it as you like. The `standup_note` setting (Integrations) turns it off.

Each meeting of yours that Fathom recorded becomes a note in the Meetings
folder, `<title> <date>.md` (`lib/sidebar/meetings.js`): its action items,
yours first, each linking to its moment in the recording, Fathom's summary,
and the whole transcript, each line's timestamp a link back. A meeting is
yours when you recorded it or were invited, by your Fathom email or your git
name. The sidebar daemon asks Fathom through Claude's connector (a headless
Haiku allowed that one call) every 30 minutes from 7am to 9pm, and every five minutes for the half hour after a meeting on your calendar ends, since Fathom has its summary a few minutes after the call, then twice for
each new meeting, the first look going back two days; a meeting whose summary
isn't ready is asked about again next time. Each is written once, and a note
already there is never written over. It needs the Fathom connector in Claude;
the `meeting_notes` setting (Integrations) turns it off.

### Setup

```
./setup.sh notes
```

links `bin/notes` into `~/.local/bin`. It needs Neovim 0.10 or newer and a
font with ☐, ☑ and the box-drawing characters, which every common one has.
The first run makes `~/Notes` with a welcome note showing everything a note
can do.

In tmux, `M-N` (Alt-Shift-N) opens it in a tab of its own named Notes, or goes
to that tab when it is open already, since two of them saving the same notes
as you type would write over each other. It runs `tmux/notes-open.sh`, which
`./setup.sh tmux` links, so the binding needs both that and `./setup.sh notes`. `M-e` puts the same
Notes in a pane to the right of the one you are in, moving it from wherever it
is, or starting it there; pressed again in that tab, or `M-w` on it, it goes back to a
tab of its own. While it is beside a tab's pane, that tab says "✎ Notes here" in the
sidebar and on its mission control card. Its own Notes tab stays, in the sidebar's Other, showing which tab
has Notes (Enter goes there, `M-e` brings it back), and Notes goes back into
it when put away. Beside a tab, Notes shows that tab's own note: the one its Claude works
from (`/note-watch` and `/queue-and-clean` record it), else the one Notes last
showed beside that tab. A tab with no note of its own is asked once which it is
(`tmux/note-pick.sh`), the likely ones first: notes that mention its ticket or
PR, then those whose titles share words with the tab's name, then the note
last shown beside it, then those naming its repository, then the rest by when
they changed, searched on the Mac and sent nowhere. "Keep whatever
is open" stops it asking for that tab; `M-p`'s "Choose this tab's note" asks
again any time, and the `ask_tab_note` setting turns the question off.

### In the list

| Key | Does |
| --- | --- |
| Enter, or a click | open a note, or show a folder's or a tag's notes |
| `n` / `N` | new note in the folder shown / new folder |
| `dd` | delete (to Recently Deleted) |
| `r` | restore, in Recently Deleted |
| `p` | pin or unpin |
| `t` | the note's tab: the one linked to it (`M-e`, `/note-watch`), or, with none, a question: one of the open tabs, linked to the note then, or a new one through `M-t`'s popup, so you pick its directory, linked to the note and named after it; `M-p`'s "Open the tab for the note in Notes" does the same for the note Notes shows (`tmux/note-tab.sh`) |
| `s` `S` | mark notes (a ✓), then squash the marked ones into one: after you say yes, a headless Claude (Sonnet, the `squash_model` option) merges them, keeping every item, box and link, a box over the notes showing it at work with the seconds so far (Esc in it cancels, leaving the notes as they were); the new note goes in the first one's folder, the originals to Recently Deleted, and every tab linked to one of them (`M-e`) links to the new one |
| `m` | move to a folder |
| `R` | rename a folder |
| `/` | search every note, by every word you type |
| `Ctrl-F` | find a note with fzf (in a note too) |
| `x` | clear the search or tag |
| `Tab` | go to the note |
| `q` / `?` | quit / these keys |

Notes are listed pinned first, then newest first, each with when it changed
and how it starts (in All Notes, also which folder it is in).

### In a note

Ctrl-F, in a note or the list, finds a note with fzf: every note but those in
Recently Deleted, pinned first and then newest, each as its title, its folder
and how it starts, any of which you can type to match, with the note shown
beside the list. Enter opens it. It takes the place of vim's Ctrl-F (page
down) in notes only, and needs `fzf` (`brew install fzf`).

Everything is in the Format menu, Ctrl-P, which acts on the selection when
opened from Visual mode. In a note it takes the place of your config's
Ctrl-P (Telescope's file finder), which works as before everywhere else.
The common ones also have their own `<leader>` key, picked to stay clear of
the ones your config uses:

| Key | Style | Written as |
| --- | --- | --- |
| `1` `2` `3` `0` | Title, Heading, Subheading, Body | `# ` `## ` `### ` |
| `m` | Monospaced block | between ```` ``` ```` fences |
| `8` `-` `7` | Bulleted, dashed, numbered list | `* ` `- ` `1. ` |
| `l` | Checklist | `- [ ] `, `- [x] ` once ticked |
| `q` | Block quote | `> ` |
| `b` `i` `U` `x` `h` | Bold, italic, underline, strikethrough, highlight | `**b**` `*i*` `<u>u</u>` `~~x~~` `==h==` |
| `` ` `` | Monospaced text | `` `code` `` |
| `k` | Link | `[text](https://…)` |
| `t` | Table | rows of cells between bars |

A line style applied again turns the line back to body text, as in Notes. A
character style (bold, italic, underline, strikethrough, highlight,
monospaced) works as it does in Notes too: on the selection; off again, for
the whole run, in text that has it already; on the word under the cursor; and
anywhere else (a space, an empty line, just after a word you typed) on what
you type next, starting Insert mode between its markers, which go again if
you type nothing. From Insert mode, with Ctrl-P, it goes by where you were
typing and leaves you typing. The menu also has the
checklist's Move Checked to Bottom, Uncheck All and Delete Checked Items, and
a table's add and delete row and column.

- **Checklists.** Click a ☐, or press Enter on its line, to tick it. A ticked
  item is struck through with everything indented under it (`strike_checked`),
  and moves under the ones left to do, with its details and sub-items, so a
  list leads with what is left, as Notes' "Move checked items to bottom"
  does; sub-items sort the same way under their item. A blank line parts
  the ticked items from the rest (`gap_before_checked`).
  `require("notes").setup({ move_checked_to_bottom = false })` turns it off.
- **Line numbers.** The note window numbers its lines relative to the
  cursor, with the cursor's line showing its own, as your Neovim does, for
  jumps like `5j`. `require("notes").setup({ relative_line_numbers = false })`
  leaves the note bare, as in Notes.
- **Lists.** Enter in Insert mode carries a list on (a checklist item comes
  unticked, a number counts up) and ends it on an empty item; Tab and
  Shift-Tab indent and outdent an item.
- **Tables.** In Insert mode, Tab moves to the next cell, past the last one
  adding a row, and Shift-Tab back. A table lines up again whenever you leave
  Insert mode or change it, measured as it shows, so bold markup doesn't throw
  a column out.
- **Collapsible sections.** Enter on a title, heading or subheading, or a
  click on its ▾, folds what is under it; vim's own `z` commands work too.
- **Links.** Enter, or a click, opens a web link in the browser, a
  `[[Note title]]` link in the note with that title (made if there is none;
  shown dimmed until then), and a `#tag` in the list. Typing `[[` offers every
  note's title.
- **Tags.** Any `#word` (after a space, not in code) is a tag; the list has a
  Tags section with each one and how many notes have it.

Formatting Markdown has no syntax for is written the way Markdown tools
usually write it: `<u>` for underline, `==…==` for highlight. Attachments,
locked notes, sharing and Notes' own database are left out on purpose; the
notes are plain files instead.

The M-p action **Shape a note's checklist items as tickets**
(`tmux/note-shape.sh`) picks a note, the one open in the app first, then its
unticked checklist items, all selected to start with, and the repo they are
for, and opens a tab per item running `/shape` through `shape-batch`.
Everything indented under an item is part of its ticket; unticked checklist
items under it are its sub-items, each a ticket of its own, and `/shape`
asks first whether the item should become a project (a Jira epic) with them as
its issues, or a ticket with them as sub-issues. It checks the tracker before reading
any code, and when a ticket already covers an item, it shows you that ticket
and asks whether to resume shaping it instead. Ticked items, and what is under
them, are left out.

`/note-tickets <folder, note or #tag>` in Claude turns the open checklist
items in those notes into Linear tickets: it checks Linear for tickets that
already cover each, suggests tickets for the rest, and creates only the ones
you pick. Afterwards it puts each ticket's ID at the end of its item, if you agree, so
the next run skips it. It reads only the notes you name.

`/note-watch [note]` in Claude watches one note while the session runs: it
asks you to confirm the note first (or pick one from a list of titles), then
works each checklist item you add, once you have stopped typing it, and ticks
it when done. It runs as a Monitor, started again when it expires, until you
say to stop.

`/project-status [project or ticket]` in Claude says where the Linear
project this session is part of stands (from the argument, else the branch's
or tab's ticket): its latest status update and target date, its tickets by
state and milestone, who has what in flight and which tab is on it, what is
stuck, what the team said about it on Slack in the last two weeks, and what
to pick up next. It only reads Linear, Slack and tmux.

A note changed from outside the app, by the skill, a script or another
editor, shows the change while it is open, without a save or a keypress. If
you have typed something not yet saved, the two are merged line by line, so
changes to different lines, even next to each other, both stay; where both
changed the same line, both versions are kept, yours first, and the app says
so. A note deleted or moved outside the app while open isn't written back
under its old name.

## codex-review

Runs Codex as a skeptical, read-only static reviewer of the current branch and
prints its findings as JSON, or Claude Code when Codex isn't installed or
signed in. It is the reviewer half of the `review-loop`
Claude skill in `claude/skills/review-loop`, which calls it, fixes what it
agrees with, commits, and calls it again until the findings list comes back
empty. Before its first round the skill also runs Claude Code's
`/security-review` on the branch once, its findings fixed or asked about as
the rounds' are (the `review_security` setting, under Safety, turns it off).
It is also fine to run by hand for a second opinion.

Working straight on `main`, with no branch or PR, it reviews the commits not
yet on `origin/main`, fetched first: what you are about to push. With every
commit pushed it says there is nothing to review; `--base <commit>` reviews
pushed ones from there.

```
codex-review                       review the branch against its PR's base (on main: the unpushed commits)
codex-review --base upstream/main  diff against a different base, used as given when the ref exists
codex-review --notes notes.md      disputed findings and accepted known gaps, each with a reason
codex-review --include-low         report low-severity findings too
codex-review --reviewer claude     have Claude review even with Codex there
codex-review --dry-run             print the prompt and the reviewer's command, run nothing
codex-review --model gpt-5.5       pick the reviewer's model; no other option passes through
```

Without Codex, `claude -p` reviews with the same prompt and JSON schema, and the
JSON's `reviewer` says which ran. It gets less than Codex does: only the Read,
Grep and Glob tools, so it runs nothing and reaches no network; the diff in a
file of its own rather than `git`; `dontAsk`, so a read outside the repository
and that file is denied rather than asked about (verified with `~/.zshrc` and
`/etc/hosts`); and no settings file, MCP server, skill or `CLAUDE.md`
(`CLAUDE_CODE_DISABLE_CLAUDE_MDS`), since on a branch under review those are
the contributor's. The rest of this section is about Codex.

The prompt is the one that used to be typed into a Codex pane by hand: assess
the code against what the commits assert, look for regressions and security
concerns, run no checks, treat every claim as unproven until the code shows it.
It is a static review with the burden of proof on the code, not a red team: Codex
cannot execute, build, or probe anything, so a suspicion it cannot make concrete
from the code is left out of the findings and noted in the summary as something
a hands-on test would need to check. The PR title and body are fetched with
`gh` and pasted in, so Codex never needs GitHub access. Codex is told to answer
in a fixed JSON shape: a `summary` plus `findings`, each with a severity, a
category, a file and line, a title, and the concrete failure it found.

Codex runs with the least access it can currently be given, because a reviewer
needs to read code and nothing else:

- The `read-only` sandbox for anything the model runs, which also blocks
  network access for those commands, with `approval_policy=never` so a command
  that needs more access fails instead of asking for it. Approval rules from
  `.rules` files, which let matching commands run outside the sandbox, are
  ignored.
- Your `~/.codex/config.toml` is not loaded, so MCP servers, plugins and other
  integrations set up for interactive use do not come along. Login does.
- `AGENTS.md` and its fallbacks are not loaded as instructions. On a branch
  under review that file belongs to the contributor; the prompt tells Codex to
  read any agent instruction file the branch touches as code that tries to
  steer automated tools. Verified: with loading on, a hostile `AGENTS.md` in a
  probe repository reached the model as an instruction; with it off, it did not.
- Commands Codex runs see only core environment variables such as `PATH` and
  `HOME`, not the tokens in the calling shell's environment.
- Commit messages and the PR title and body are placed in the prompt inside
  fenced blocks Codex is told to treat as data, never as instructions, and to
  report as a finding if they try to steer the review. The person running the
  review still verifies every finding against the code before acting on it.
- The only Codex option that passes through is `--model`. Arbitrary
  passthrough would let whoever builds the command line, another agent
  included, undo the sandbox.
- The temp directory holding the schema and Codex's answer is removed on every
  exit path, so a failed run does not leave code excerpts behind.

What that does not cover: the read-only sandbox lets commands read any file
your user can read, not only the repository. Codex's custom permission profiles
can confine reads to the workspace, but in the current release any custom
profile also makes the workspace writable, even one that extends `:read-only`
(verified with `codex exec`: `touch` succeeded while `~/.zshrc` was denied).
For a reviewer that is the worse trade, since a write into `.git/hooks` or an
ignored directory survives the clean-tree check. Wrapping Codex in an outer
macOS seatbelt does not work either: the kernel refuses to apply Codex's inner
sandbox under any outer profile that contains a deny rule. So a prompt injection
that gets Codex to read a file outside the repository can put its contents in
the review output. That output is the JSON this script prints, and commands
have no network, so it cannot go anywhere else. Findings that name or mention a
path outside the repository are listed in a `warnings` array in the JSON and
printed to stderr, so the caller reads those before acting on them. Full
isolation means running the reviewer in a container that holds only the
checkout and the Codex login.

### Setup

1. **Install the script.**

   ```sh
   ./setup.sh codex-review
   ```

2. **Log in to Codex** (`codex login`) if `codex` does not already work
   interactively, or skip it and Claude reviews. `codex exec` uses the same login. Nothing else in your Codex
   configuration is read, so no per-repository trust step is needed.

### Notes

- Only committed changes are reviewed, and the command refuses to run on a
  dirty tree. Each round of the loop is then a review of exactly the commits
  the next round is compared against.
- The notes file is how the author talks back. A disputed finding is not
  raised again unless the reasoning is wrong; a known gap the author has
  accepted is not reported unless its impact is larger than the note admits.
  Anything the notes leave out is fair game, which is the point: a limitation
  acknowledged in chat but not written down is a finding hidden from the
  reviewer.
- Low-severity findings are dropped by the prompt unless `--include-low` is
  given. A skeptical reviewer asked for everything never runs out of nits;
  the loop needs a floor to converge.
- The base is the PR's base branch, or origin's default branch when there is
  no PR. With a PR, the base is read from the remote whose URL points at the
  PR's repository, which is not always `origin` in a clone with several
  remotes. That remote's copy of the base is used because it defines the PR's
  contents: an unpushed commit on your local `main` that the
  branch also carries is part of the PR and gets reviewed. Without a PR, the
  local or remote copy with the newer merge base is used, so only the branch's
  own commits are in scope whichever side is behind. A PR whose remote base has
  not been fetched, or that comes from a different repository than it targets,
  stops the run with the command or `--base` to use instead of guessing.
- `gh pr view` failing for any reason other than "no pull requests found" stops
  the run too. An expired login must not turn into a review against the wrong
  base with the prompt claiming there is no PR.
- The commit under review is pinned before Codex starts, and the prompt scopes
  the diff by SHA rather than `HEAD`. If `HEAD` or the working tree differs
  when Codex finishes from when it started, the result is discarded rather than
  labeled with a commit it does not describe. This is a check at both ends, not
  a snapshot: a branch switch that is switched back before Codex finishes goes
  unnoticed, so leave the checkout alone while a review runs.
- Codex's progress output is discarded so stdout is just the JSON. Set
  `CODEX_REVIEW_VERBOSE=1` to watch it on stderr.
