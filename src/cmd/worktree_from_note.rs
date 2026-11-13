//! `worktree-from-note [path] --note <note> --item <line> [--prompt <text>]`:
//! a git worktree on a new branch named after a checklist item of one of
//! your notes (the notes app's Markdown files, in `$NOTES_DIR` or
//! `~/Notes`), with Claude opened in it, told to work on the item and its
//! plan and to land it with /queue-and-clean once the work is done, as
//! `switchboard landing` says for the repository (src/repos.rs): straight
//! on the default branch, or through a PR. Ported from
//! `bin/worktree-from-note`.
//!
//! `<note>` is the note's path, absolute or within the notes directory,
//! ".md" optional; `<line>` is the item's line number in it, an unticked
//! `- [ ]` line of the note's top level. `init zsh` prints the `wtn`
//! function, which works as `wt` does (worktree_from_ticket.rs).
//!
//! What M-p's "Start work on a note's checklist items" listed, and what
//! autopilot takes next (tmux/wtn-open.zsh's `items`,
//! `item`, `unclaimed`, `plan_of` and `queue`), is here too, since it reads
//! the note and the repository the same way: `items`, `item_text`,
//! `unclaimed`, `plan_of` and `queue`, for the server's palette command and
//! the autopilot.
//!
//! A branch's item is kept by its note's stable ID (src/notes/id.rs, the
//! note given one here when it has none), so a note renamed or moved, in
//! Notes or outside it, keeps its items' worktrees: the picker and the mode
//! still leave them out, and wtn on one reuses its worktree. Claims from
//! before notes had IDs, by the note's path, are brought to IDs once per
//! repository (`id::migrate_claims`); one it can't place is listed, and
//! still read by path.
//!
//! `--target <branch>` cuts the item's branch from that branch and records
//! it as the one queue-and-clean lands on, so it is refused in a repository
//! that lands through PRs, whose PRs go to the default branch: the item
//! would land there, carrying the target's commits.
//!
//! A tab of autopilot's in a repository landing straight on a branch starts
//! its Claude with one permission for the session, the landing's own push
//! (`push_rule`), since Claude Code's auto mode otherwise judges that push
//! itself and may refuse it. It is passed on the command line
//! (`claude --settings`), so it is in no file and no other session has it;
//! the `wtn` function, which starts Claude itself, reads it back from
//! `--settings-file`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::LazyLock;

use regex::Regex;

use crate::notes::id;
use crate::paths;
use crate::worktree::{self, Given, Purpose};

/// A branch name short enough to read in a tab and a prompt.
const SLUG_LENGTH: usize = 50;

/// Each item wtn starts, kept a day, `<seconds>\t<note's ID>\t<item's
/// first line>` (the note's path in a row from before notes had IDs): the
/// autopilot's queue takes an item claimed in the last
/// `CLAIMED_S` as taken even once its worktree is gone, since
/// queue-and-clean may remove it a moment before ticking the item.
pub fn claims_file() -> PathBuf {
    claims_in(&paths::state_dir())
}

/// The claims file in a state directory.
pub fn claims_in(state: &Path) -> PathBuf {
    state.join("wtn-claims.tsv")
}
/// An item claimed this recently is taken, whatever its worktree.
pub const CLAIMED_S: i64 = 30 * 60;
const DAY_S: i64 = 24 * 60 * 60;

/// Appends an item's claim, as M-p's popup starts several wtn at once and
/// a line appended is written whole; the file is written again without the
/// rows over a day old only when its first row is that old.
fn claim(note: &str, item: &str) {
    let file = claims_file();
    let now = crate::util::now_secs();
    if let Some(dir) = file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    crate::util::append_quietly(&file, format!("{now}\t{note}\t{item}\n"));
    let Ok(text) = fs::read_to_string(&file) else {
        return;
    };
    let first = text
        .split('\t')
        .next()
        .and_then(|t| t.trim().parse::<i64>().ok())
        .unwrap_or(now);
    if now - first < DAY_S {
        return;
    }
    let kept: String = text
        .lines()
        .filter(|row| {
            row.split('\t')
                .next()
                .and_then(|t| t.parse::<i64>().ok())
                .is_some_and(|at| now - at < DAY_S)
        })
        .map(|row| format!("{row}\n"))
        .collect();
    let _ = crate::util::write_atomic(&file, kept);
}

/// The items of a note wtn started in the last `CLAIMED_S` before `now`
/// (seconds), by their first line: the note's by its ID (`None` while it
/// has none), or, in a row from before notes had IDs, by its path within
/// the notes directory.
pub fn claimed_lately(text: &str, note_id: Option<&str>, note: &str, now: i64) -> HashSet<String> {
    text.lines()
        .filter_map(|row| {
            let mut fields = row.splitn(3, '\t');
            let at: i64 = fields.next()?.trim().parse().ok()?;
            let whose = fields.next()?;
            let ours = whose == note || note_id.is_some_and(|id| id == whose);
            (ours && now - at < CLAIMED_S).then(|| fields.next().map(str::to_string))?
        })
        .collect()
}

fn usage() -> ExitCode {
    eprintln!(
        "Usage: worktree-from-note [path] --note <note> --item <line> [--prompt <text>] [--target <branch>]\n       \
         worktree-from-note init <zsh|bash>\n\n\
         Examples:\n  \
         worktree-from-note --note \"Switchboard Notes\" --item 12\n\n\
         Path defaults to {} under the repo root.\n\
         --prompt adds to the prompt Claude starts with. The wtn function also takes\n\
         it from WT_PROMPT, and the note from WTN_NOTE, clearing both, so /wtn can\n\
         hand them over.\n\
         --target cuts the item's branch from that local branch rather than the default\n\
         branch, and records it as branch.<branch>.wtnTarget, which queue-and-clean\n\
         lands it on (autopilot's target, or one picked by hand). It is refused in a\n\
         repository that lands through PRs, which go to the default branch.",
        worktree::DEFAULT_WORKTREE_DIR
    );
    ExitCode::from(1)
}

/// The `wtn` function, as `wt`: runs the command with `--no-claude`, cd's
/// into the worktree and starts Claude there. The prompt is every line of
/// the cd-file after the first, since the item it carries can run to
/// several. WT_PROMPT and WTN_NOTE, which /wtn sets on the pane it respawns,
/// are passed on and cleared, so the note's path never has to be quoted into
/// a command. The settings Claude starts with (`push_rule`), when the
/// command writes any, come back in a file of their own, so a function
/// defined by an older build, which passes no `--settings-file`, reads the
/// cd-file as it always did.
pub fn shell_function() -> String {
    format!(
        r#"wtn() {{
  local cdfile settingsfile settings rc dir prompt
  local -a extra
  extra=()
  [ -n "${{WT_PROMPT:-}}" ] && extra+=(--prompt "$WT_PROMPT")
  [ -n "${{WTN_NOTE:-}}" ] && extra+=(--note "$WTN_NOTE")
  unset WT_PROMPT WTN_NOTE
  cdfile=$(mktemp) || return 1
  settingsfile=$(mktemp) || {{ rm -f "$cdfile"; return 1; }}
  {} --no-claude --cd-file "$cdfile" --settings-file "$settingsfile" "${{extra[@]}}" "$@"
  rc=$?
  if [ $rc -ne 0 ]; then
    rm -f "$cdfile" "$settingsfile"
    return $rc
  fi
  dir=$(sed -n 1p "$cdfile")
  prompt=$(sed -n '2,$p' "$cdfile")
  settings=$(cat "$settingsfile")
  rm -f "$cdfile" "$settingsfile"
  if [ ! -d "$dir" ]; then
    echo "wtn: expected a worktree directory, got: $dir" >&2
    return 1
  fi
  cd "$dir" || return 1
  if [ -n "$settings" ]; then
    claude --settings "$settings" "$prompt"
  else
    claude "$prompt"
  fi
}}"#,
        worktree::self_command("worktree-from-note")
    )
}

/// The notes directory as given: `$NOTES_DIR`, else `~/Notes`.
pub fn notes_dir() -> PathBuf {
    match std::env::var_os("NOTES_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => paths::home_join("Notes"),
    }
}

static UNTICKED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^- \[ \] (.*\S.*)$").unwrap());
static ITEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^- \[[ xX]\] (.*\S.*)$").unwrap());
static SUB_ITEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s+[-*+]\s+\[[ xX]\]\s").unwrap());
static TICKED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^- \[[xX]\] ").unwrap());
static PLAN_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Plan: \[\[([^\]]+)\]\]").unwrap());

/// The note's file: a path, absolute or within the notes directory, ".md"
/// added when left off, and only ever a note of the notes directory, not a
/// deleted one.
fn note_file(dir: &Path, note: &str) -> Result<PathBuf, String> {
    let given = worktree::normalize(&dir.join(note));
    let mut candidates = vec![given.clone()];
    if !note.ends_with(".md") {
        candidates.push(PathBuf::from(format!("{}.md", given.display())));
    }
    let found = candidates
        .into_iter()
        .find(|f| f.is_file())
        .ok_or_else(|| format!("no note at {}", given.display()))?;
    let real = found.canonicalize().map_err(|e| e.to_string())?;
    let inside = real
        .strip_prefix(dir)
        .ok()
        .filter(|rel| !rel.as_os_str().is_empty());
    match inside {
        Some(rel)
            if rel
                .components()
                .next()
                .is_some_and(|c| c.as_os_str() != ".trash") =>
        {
            Ok(real)
        }
        _ => Err(format!(
            "{} is not a note of {}",
            found.display(),
            dir.display()
        )),
    }
}

/// A Plan note linked from an item (`Plan: [[Plan - <title>]]`), found by
/// its file name anywhere in the notes directory but its deleted notes and
/// other dot-folders.
pub fn plan_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let wanted = format!("{name}.md");
    let mut entries: Vec<_> = fs::read_dir(dir).ok()?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in &entries {
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if file_name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = plan_file(&path, name) {
                return Some(found);
            }
        } else if file_name == wanted && path.is_file() {
            return Some(path);
        }
    }
    None
}

/// The plan note an item links, for the pane beside the Autopilot board:
/// its file's name, and its words (`plan_body`), none when no note has the
/// name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlanNote {
    pub name: String,
    pub text: Option<String>,
}

/// A plan note's words for a pane its item already heads: without its ID,
/// its `# Plan - …` heading and its `Item:` line.
pub fn plan_body(text: &str) -> String {
    let text = id::strip(text);
    let mut lines = text.lines().skip_while(|l| l.trim().is_empty()).peekable();
    if lines.peek().is_some_and(|l| l.starts_with("# ")) {
        lines.next();
    }
    let mut lines = lines.skip_while(|l| l.trim().is_empty()).peekable();
    if lines.peek().is_some_and(|l| l.starts_with("Item:")) {
        lines.next();
    }
    lines.collect::<Vec<_>>().join("\n").trim().to_string()
}

/// Every note under `dir` by its name without `.md`, in the order
/// `plan_file` looks through them, so the first of a name is the one it
/// finds.
fn note_files(dir: &Path, out: &mut HashMap<String, PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in &entries {
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if file_name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            note_files(&path, out);
        } else if let Some(name) = file_name.strip_suffix(".md")
            && path.is_file()
        {
            out.entry(name.to_string()).or_insert(path);
        }
    }
}

/// The note a line under an item names as its plan, read as Notes reads
/// it (`notes::render::plan_of`): the line, after an optional bullet,
/// starts with `Plan:` and holds a `[[…]]`, its name without the spaces
/// around it.
fn plan_line(line: &str) -> Option<String> {
    let text = line.trim_start();
    let text = text.strip_prefix("- ").unwrap_or(text);
    let (_, after) = text.strip_prefix("Plan:")?.split_once("[[")?;
    let (name, _) = after.split_once("]]")?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// The plan an item's block links as its own: the first `Plan:` line under
/// it that isn't a sub-item's, since a plan linked under a sub-item is the
/// sub-item's, as Notes reads it; else a `Plan: [[…]]` on its own first
/// line.
fn own_plan(block: &str) -> Option<String> {
    let indent_of = |line: &str| line.len() - line.trim_start().len();
    let mut lines = block.split('\n');
    let first = lines.next()?;
    // The indent of the sub-item the lines are under, if any.
    let mut sub: Option<usize> = None;
    for line in lines {
        if line.trim().is_empty() || sub.is_some_and(|s| indent_of(line) > s) {
            continue;
        }
        sub = SUB_ITEM.is_match(line).then(|| indent_of(line));
        if sub.is_none()
            && let Some(name) = plan_line(line)
        {
            return Some(name);
        }
    }
    PLAN_LINK
        .captures(first)
        .map(|m| m[1].trim().to_string())
        .filter(|name| !name.is_empty())
}

/// The plan each top-level item of a note links (`Plan: [[…]]` on its line
/// or under it), ticked or not, by the item's first line: the notes
/// directory walked once and each plan read once, off the server's task,
/// for the Autopilot board to show beside the item.
pub fn plan_notes(notes: &Path, text: &str) -> HashMap<String, PlanNote> {
    let lines: Vec<&str> = text.split('\n').collect();
    let linked: Vec<(String, String)> = lines
        .iter()
        .enumerate()
        .filter_map(|(at, line)| {
            let first = ITEM.captures(line)?[1].trim().to_string();
            let name = own_plan(&item_block(&lines, at))?;
            Some((first, name))
        })
        .collect();
    if linked.is_empty() {
        return HashMap::new();
    }
    let mut files = HashMap::new();
    note_files(notes, &mut files);
    linked
        .into_iter()
        .map(|(first, name)| {
            let text = files
                .get(&name)
                .and_then(|file| fs::read_to_string(file).ok())
                .map(|text| plan_body(&text));
            (first, PlanNote { name, text })
        })
        .collect()
}

/// The plan note the item whose first line is `item` links in the note
/// `text`, ticked or not (`Plan: [[…]]`, as `plan_notes` reads it): its
/// file, found under `notes`; none when the item or the plan isn't there.
/// For M-e on an autopilot tab, off the server's task.
pub fn item_plan_file(notes: &Path, text: &str, item: &str) -> Option<PathBuf> {
    let lines: Vec<&str> = text.split('\n').collect();
    let at = lines
        .iter()
        .position(|line| ITEM.captures(line).is_some_and(|m| m[1].trim() == item))?;
    let name = own_plan(&item_block(&lines, at))?;
    plan_file(notes, &name)
}

/// The item's first line, when the line is an unticked item.
fn unticked(line: &str) -> Option<String> {
    UNTICKED
        .captures(line)
        .map(|m| m[1].trim().to_string())
        .filter(|t| !t.is_empty())
}

/// The item at `at` (0-based) and what is indented under it, blank lines
/// between included, up to the next line of the note's top level, trailing
/// blank lines left off.
fn item_block(lines: &[&str], at: usize) -> String {
    let mut rest: Vec<&str> = lines[at + 1..]
        .iter()
        .take_while(|l| !l.starts_with(|c: char| !c.is_whitespace()))
        .copied()
        .collect();
    while rest.last().is_some_and(|l| l.trim().is_empty()) {
        rest.pop();
    }
    std::iter::once(lines[at])
        .chain(rest)
        .collect::<Vec<_>>()
        .join("\n")
}

// ── What the palette's popup and the autopilot list ─────────────────────────

/// One unticked top-level item of a note: its line number (1-based), its
/// first line, whether its line or those under it say its plan still needs
/// a choice, the items its `After: <first line>` lines say it builds on,
/// and the first of those still open in the note (`waits_on`), which holds
/// it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub line: usize,
    pub text: String,
    pub needs_choice: bool,
    pub after: Vec<String>,
    pub waits_on: Option<String>,
}

/// How much of the item an item waits on its row and the view show.
pub const WAITS_ON_SHOWN: usize = 40;

impl Item {
    /// As wtn-open.zsh printed it: `<line>\t<text>`, with ` (needs a
    /// choice)` after, and ` (waits on <item>)` while it is held.
    pub fn row(&self) -> String {
        format!(
            "{}\t{}{}{}",
            self.line,
            self.text,
            if self.needs_choice {
                " (needs a choice)"
            } else {
                ""
            },
            self.waits_on
                .as_ref()
                .map(|w| format!(" (waits on {})", crate::ui::draw::cut(w, WAITS_ON_SHOWN)))
                .unwrap_or_default()
        )
    }
}

/// The open item an item's `After:` lines name, if any: matched by its
/// first line exactly, or, when no item of the note has that first line,
/// open or ticked, by being a part of exactly one open item's first line,
/// ignoring case. `open` is the note's unticked top-level items' first
/// lines, parked ones too, the item's own left out; `ticked` its ticked
/// ones, so a line naming a ticked item exactly holds nothing.
fn waits_on(after: &[String], open: &[&str], ticked: &[&str]) -> Option<String> {
    after.iter().find_map(|a| {
        if let Some(exact) = open.iter().find(|o| **o == a) {
            return Some(exact.to_string());
        }
        if ticked.contains(&a.as_str()) {
            return None;
        }
        let lower = a.to_lowercase();
        let mut parts = open.iter().filter(|o| o.to_lowercase().contains(&lower));
        match (parts.next(), parts.next()) {
            (Some(only), None) => Some(only.to_string()),
            _ => None,
        }
    })
}

static AFTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s+After:\s*(\S.*?)\s*$").unwrap());
static NEEDS_CHOICE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"needs (a )?choices?").unwrap());
static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(#+)\s").unwrap());
// The words a section of parked work is named with, as lib/sidebar/todo.js
// has them.
static PARKED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|[^a-z])(later|defer+ed|parked|someday|backlog)([^a-z]|$)").unwrap()
});

/// Whether a heading names a section of parked work (`## Defered`,
/// `## Later`), which `items` leaves out and Notes shows folded.
pub fn parked_heading(line: &str) -> bool {
    PARKED.is_match(&line.to_lowercase())
}

/// The open item the item on a note's line (1-based) waits on, by its
/// `After:` lines, as `items` reads them, whether the item is parked or not.
pub fn held_by(text: &str, line: usize) -> Option<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let first = lines
        .get(line.checked_sub(1)?)?
        .strip_prefix("- [ ] ")?
        .trim();
    let after: Vec<String> = item_block(&lines, line - 1)
        .lines()
        .skip(1)
        .filter_map(|l| AFTER.captures(l).map(|m| m[1].to_string()))
        .collect();
    let open: Vec<&str> = lines
        .iter()
        .filter_map(|l| l.strip_prefix("- [ ] "))
        .map(str::trim)
        .filter(|o| *o != first)
        .collect();
    let ticked: Vec<&str> = lines
        .iter()
        .filter(|l| TICKED.is_match(l))
        .map(|l| l[6..].trim())
        .collect();
    waits_on(&after, &open, &ticked)
}

/// A note's unticked top-level items, in its order, leaving out any under a
/// section named for parked work (`## Defered`, `## Later`), a heading
/// under such a section still in it. An item is held (`waits_on`) while
/// one of its `After:` lines names an item still unticked, parked or not.
pub fn items(text: &str) -> Vec<Item> {
    let mut out: Vec<Item> = Vec::new();
    let mut current: Option<Item> = None;
    let mut parked = 0usize;
    for (i, line) in text.split('\n').enumerate() {
        let number = i + 1;
        if number > 1
            && let Some(m) = HEADING.captures(line)
        {
            out.extend(current.take());
            let level = m[1].len();
            if parked == 0 || level <= parked {
                parked = if parked_heading(line) { level } else { 0 };
            }
            continue;
        }
        if line.starts_with("- [ ] ") && parked == 0 {
            out.extend(current.take());
            current = Some(Item {
                line: number,
                text: line[6..].trim().to_string(),
                needs_choice: NEEDS_CHOICE.is_match(line),
                after: Vec::new(),
                waits_on: None,
            });
            continue;
        }
        if line.starts_with(|c: char| !c.is_whitespace()) {
            out.extend(current.take());
            continue;
        }
        let Some(item) = current.as_mut() else {
            continue;
        };
        // An After: line names another item, whose words say nothing of this
        // one's plan.
        if let Some(m) = AFTER.captures(line) {
            item.after.push(m[1].to_string());
        } else if NEEDS_CHOICE.is_match(line) {
            item.needs_choice = true;
        }
    }
    out.extend(current);
    for item in &mut out {
        item.waits_on = held_by(text, item.line);
    }
    out
}

/// The item on a note's line (1-based), for starting work on it from
/// Notes; else why that line can't be started, to say after "That line ".
pub fn item_at(text: &str, line: usize) -> Result<Item, &'static str> {
    let Some(at) = line.checked_sub(1).and_then(|i| text.split('\n').nth(i)) else {
        return Err("isn't in the note");
    };
    let trimmed = at.trim_start();
    let checklist = |rest: &str| {
        ["- [ ] ", "- [x] ", "- [X] "]
            .iter()
            .any(|m| rest.starts_with(m))
    };
    if !checklist(trimmed) {
        return Err("isn't a checklist item");
    }
    if trimmed.len() != at.len() {
        return Err("is under another item: start work on the item at the top of the list");
    }
    if !at.starts_with("- [ ] ") {
        return Err("is ticked");
    }
    items(text)
        .into_iter()
        .find(|i| i.line == line)
        .ok_or("is in a section of parked work")
}

/// One item whole, by its line number (1-based): its line and what is
/// indented under it, as wtn hands it to Claude, each line ending in a
/// newline; empty when the note has no such line.
pub fn item_text(text: &str, line: usize) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    if line == 0 || line > lines.len() {
        return String::new();
    }
    let block = item_block(&lines, line - 1);
    format!("{block}\n")
}

/// The items no worktree is working on yet. wtn keeps the item a branch is
/// for in the repository's git config, `branch.<branch>.wtnItem`, as
/// `<note's ID>: <item's first line>`, the ID read from the note's text, so
/// a note renamed or moved keeps its claims; a claim from before notes had
/// IDs that wtn couldn't place still reads `<note's path within the notes
/// directory>: <item's first line>`. An item worded the same as one claimed
/// from another note is the same item, copied there, and taken too. An item counts as taken only while its
/// branch is checked out in a worktree, so one whose worktree was removed,
/// its branch kept, shows again. `repo` is any directory of the repository.
pub fn unclaimed(repo: &Path, note: &str, text: &str) -> Vec<Item> {
    let note_id = id::in_text(text);
    let checked_out: HashSet<String> = worktree::worktrees(repo)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|w| w.branch)
        .collect();
    let config = worktree::git_at(repo, &["config", "--get-regexp", r"^branch\..*\.wtnitem$"])
        .unwrap_or_default();
    let claimed: HashSet<&str> = config
        .lines()
        .filter_map(|l| l.split_once(' '))
        .filter_map(|(key, value)| {
            let branch = key.strip_prefix("branch.")?.strip_suffix(".wtnitem")?;
            checked_out.contains(branch).then_some(value)
        })
        .collect();
    // The same first line claimed from another note is the same item, which
    // wtn would take over (`run`), so it is taken here too.
    let words: HashSet<&str> = claimed
        .iter()
        .filter_map(|c| id::claim_of(c).map(|(_, text)| text))
        .collect();
    items(text)
        .into_iter()
        .filter(|item| {
            let by_id = note_id
                .as_ref()
                .is_some_and(|id| claimed.contains(format!("{id}: {}", item.text).as_str()));
            !by_id
                && !words.contains(item.text.as_str())
                && !claimed.contains(format!("{note}: {}", item.text).as_str())
        })
        .collect()
}

/// Whether an item's plan is settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Its `Plan: [[<name>]]` links a plan note with no "## Decision
    /// needed", the heading plans mark an open choice with, or one whose
    /// choice was made: each such section has a ticked option or a
    /// `Chose …` line, or the item says `(chose …)`.
    Settled,
    /// The plan note has an open choice.
    Choice,
    /// No plan, or its plan note isn't there.
    None,
}

static DECISION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^#+[ \t]+decisions?[ \t]+needed").unwrap());
// A choice made: a ticked option at the section's top level (a ticked task
// nested under an option doesn't choose it), or the plans' own "Chose A." line.
static MADE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:[-*+]\s+\[x\]|\s*(?:[-*+]\s+)?chose\b)").unwrap());
// An item that says what was chosen: "(chose B)".
static CHOSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\(chose\b").unwrap());

/// Whether a plan note leaves a choice open: a "## Decision needed"
/// section (up to the next heading of its level or above, its subheadings
/// in it) with no ticked option and no `Chose …` line. The heading is often
/// left in place once the choice is made, so the heading alone doesn't say
/// it is open.
fn open_choice(plan: &str) -> bool {
    let mut open = false;
    // The level of the decision section the lines are in, if any.
    let mut decision: Option<usize> = None;
    for line in plan.lines() {
        if let Some(m) = HEADING.captures(line) {
            let level = m[1].len();
            if decision.is_some_and(|d| level > d) {
                continue;
            }
            if open {
                return true;
            }
            decision = DECISION.is_match(line).then_some(level);
            open = decision.is_some();
        } else if decision.is_some() && MADE.is_match(line) {
            open = false;
        }
    }
    open
}

/// An item's block without its `After:` lines, which name other items, so
/// their words say nothing of this one's plan.
fn without_after(block: &str) -> String {
    block
        .split('\n')
        .filter(|l| !AFTER.is_match(l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// How settled an item's plan is, from the item whole.
pub fn plan_of(notes: &Path, item: &str) -> Plan {
    let item = without_after(item);
    let item = item.as_str();
    let Some(name) = PLAN_LINK.captures(item).map(|m| m[1].to_string()) else {
        return Plan::None;
    };
    let Some(file) = plan_file(notes, &name) else {
        return Plan::None;
    };
    match fs::read_to_string(file) {
        Ok(text) if open_choice(&text) && !CHOSE.is_match(item) => Plan::Choice,
        Ok(_) => Plan::Settled,
        Err(_) => Plan::None,
    }
}

/// The items autopilot would take, in its order: those
/// whose plan is settled first, then those with no plan (or a plan note
/// that isn't there), each in the note's order. An item whose plan needs a
/// choice is taken only `with_choices` (autopilot hands-free, whose tab
/// takes the recommended option), after the settled ones and before those
/// with no plan; never taken is nor one under a parked section, nor one a
/// worktree has or wtn started in the last half hour (its claims file),
/// nor one held by an open item its `After:` line names, nor an empty one, nor one whose first line is in `skip` (the items
/// skipped, and those opened a moment ago, before wtn has made their
/// branch). `ready_only` takes only the planned kinds. `note` is the
/// note's path within the notes directory; `repo` the note's repository;
/// `claims` wtn's claims file (`claims_file`).
pub fn queue(
    notes: &Path,
    repo: &Path,
    note: &str,
    skip: &HashSet<String>,
    ready_only: bool,
    with_choices: bool,
    claims: &Path,
) -> Result<Vec<Item>, String> {
    queue_and_held(notes, repo, note, skip, ready_only, with_choices, claims)
        .map(|(queue, _)| queue)
}

/// `queue`'s items, and apart from them those it would take but for the
/// open item each waits on (`Item::waits_on`), in the note's order.
pub fn queue_and_held(
    notes: &Path,
    repo: &Path,
    note: &str,
    skip: &HashSet<String>,
    ready_only: bool,
    with_choices: bool,
    claims: &Path,
) -> Result<(Vec<Item>, Vec<Item>), String> {
    let file = notes.join(note);
    let text = fs::read_to_string(&file).map_err(|_| format!("No note at {}", file.display()))?;
    let mut settled = Vec::new();
    let mut choices = Vec::new();
    let mut unplanned = Vec::new();
    let mut held = Vec::new();
    let lately = claimed_lately(
        &fs::read_to_string(claims).unwrap_or_default(),
        id::in_text(&text).as_deref(),
        note,
        crate::util::now_secs(),
    );
    for item in unclaimed(repo, note, &text) {
        if (item.needs_choice && !with_choices)
            || skip.contains(&item.text)
            || lately.contains(&item.text)
            || item.text.trim().is_empty()
        {
            continue;
        }
        let bucket = match plan_of(notes, &item_text(&text, item.line)) {
            Plan::Choice if with_choices => &mut choices,
            Plan::Choice => continue,
            Plan::Settled if item.needs_choice => &mut choices,
            Plan::Settled => &mut settled,
            Plan::None if !ready_only => &mut unplanned,
            Plan::None => continue,
        };
        if item.waits_on.is_some() {
            held.push(item);
        } else {
            bucket.push(item);
        }
    }
    settled.extend(choices);
    settled.extend(unplanned);
    Ok((settled, held))
}

/// The notes, newest first, by their path within the notes directory
/// without ".md", leaving out `.trash` and every other dot-folder.
pub fn notes_newest_first(notes: &Path) -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<(std::time::SystemTime, PathBuf)>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|x| x == "md")
                && let Ok(meta) = entry.metadata()
                && meta.is_file()
            {
                out.push((meta.modified().unwrap_or(std::time::UNIX_EPOCH), path));
            }
        }
    }
    let mut found = Vec::new();
    walk(notes, &mut found);
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    found
        .into_iter()
        .filter_map(|(_, p)| {
            let rel = p.strip_prefix(notes).ok()?.to_string_lossy().into_owned();
            Some(rel.strip_suffix(".md").unwrap_or(&rel).to_string())
        })
        .collect()
}

// ── The command ─────────────────────────────────────────────────────────────

/// The commit a new item's branch starts from: origin's default branch as it
/// is now, fetched first (else the main clone's HEAD, with no remote), never
/// the worktree /wtn was run in, whose own commits would then land with this
/// item's. A fetch that fails stops it, since the main clone's unpushed
/// commits would otherwise land with the item.
fn start_point(root: &Path, target: Option<&str>) -> Result<String, String> {
    if let Some(target) = target {
        return target_start(root, target);
    }
    worktree::default_start(root, "wtn: fetch the base")
}

/// The commit an item's branch starts from when it lands on `target`, a
/// local branch (made from origin's when only origin has it): the remote
/// branch it tracks as it is now, fetched first, since the landing rebases
/// onto that and pushes there, so the work starts where it will land; the
/// local branch itself when it tracks none. A local branch with commits its
/// remote branch lacks is refused, since the landing would never push them
/// and couldn't fast-forward it. A fetch that fails is said and the local
/// branch used, since the landing fetches again.
fn target_start(root: &Path, target: &str) -> Result<String, String> {
    let local = format!("refs/heads/{target}");
    let exists = |r: &str| worktree::git_ok(root, &["rev-parse", "--verify", "--quiet", r]);
    // One on origin alone, typed for "Another branch…", made a local
    // branch tracking it.
    if !exists(&local)
        && worktree::remotes(root).iter().any(|r| r == "origin")
        && worktree::fetch_branch(root, "origin", target, "wtn: fetch the target")
        && exists(&format!("refs/remotes/origin/{target}"))
    {
        worktree::git_ok(
            root,
            &[
                "branch",
                "--quiet",
                "--track",
                target,
                &format!("origin/{target}"),
            ],
        );
    }
    if !exists(&local) {
        return Err(format!(
            "no branch {target} here or on origin, which autopilot lands this item on"
        ));
    }
    let Some((remote, branch)) = tracked(root, target) else {
        return Ok(local);
    };
    if !worktree::fetch_branch(root, &remote, &branch, "wtn: fetch the target") {
        eprintln!("Couldn't fetch {remote}/{branch}; starting from {target} as it is here");
        return Ok(local);
    }
    let fetched = format!("refs/remotes/{remote}/{branch}");
    let ahead = worktree::git_at(
        root,
        &["rev-list", "--count", &format!("{fetched}..{local}")],
    )
    .map_err(|e| e.to_string())?;
    if ahead != "0" {
        let commits = if ahead == "1" { "commit" } else { "commits" };
        let refspec = if branch == target {
            target.to_string()
        } else {
            format!("{target}:{branch}")
        };
        return Err(format!(
            "{target} has {ahead} {commits} {remote}/{branch} doesn't, which the landing wouldn't push; \
             push them (`git push {remote} {refspec}`) or move {target} back, then start the item again"
        ));
    }
    Ok(fetched)
}

/// The remote branch a local branch tracks, by its remote and its name
/// there, from git's config; `None` for a branch that tracks none.
pub fn tracked(root: &Path, branch: &str) -> Option<(String, String)> {
    let get = |key: &str| {
        worktree::git_at(
            root,
            &["config", "--get", &format!("branch.{branch}.{key}")],
        )
        .ok()
        .filter(|v| !v.is_empty())
    };
    let remote = get("remote").filter(|r| r != ".")?;
    let merge = get("merge")?;
    Some((
        remote,
        merge
            .strip_prefix("refs/heads/")
            .unwrap_or(&merge)
            .to_string(),
    ))
}

pub fn main(args: Vec<String>) -> ExitCode {
    let given = match Given::parse(
        &args,
        &[
            "--prompt",
            "--note",
            "--item",
            "--target",
            "--settings-file",
        ],
    ) {
        Ok(g) => g,
        Err(flag) if flag == "--cd-file" => return worktree::fail("--cd-file requires a path"),
        Err(flag) => return worktree::fail(&format!("{flag} requires a value")),
    };
    match worktree::init_shell(&given.positional) {
        Some(true) => {
            println!("{}", shell_function());
            return ExitCode::SUCCESS;
        }
        Some(false) => {
            return worktree::fail("init requires a shell: worktree-from-note init <zsh|bash>");
        }
        None => {}
    }
    if given.help {
        return usage();
    }
    let (Some(note), Some(item)) = (given.last("--note"), given.last("--item")) else {
        return usage();
    };
    if given.positional.len() > 1 {
        return usage();
    }
    if item.is_empty() || !item.bytes().all(|b| b.is_ascii_digit()) {
        return worktree::fail(&format!(
            "--item takes the item's line number, not \"{item}\""
        ));
    }
    match run(&given, note, item) {
        Ok(code) => code,
        Err(e) => worktree::fail(&e),
    }
}

fn run(given: &Given, note: &str, item: &str) -> Result<ExitCode, String> {
    let target = given
        .positional
        .first()
        .map(String::as_str)
        .unwrap_or(worktree::DEFAULT_WORKTREE_DIR);
    let given_dir = notes_dir();
    let dir = given_dir
        .canonicalize()
        .map_err(|_| format!("no notes directory at {}", given_dir.display()))?;
    let file = note_file(&dir, note)?;
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let text = fs::read_to_string(&file).map_err(|e| e.to_string())?;
    let lines: Vec<&str> = text.split('\n').collect();
    let number: usize = item
        .parse()
        .map_err(|_| format!("{name} has no line {item}"))?;
    let at = number.checked_sub(1).filter(|at| *at < lines.len());
    let Some(at) = at else {
        return Err(format!("{name} has no line {item}"));
    };
    let line = lines[at];
    if TICKED.is_match(line) {
        return Err(format!("line {item} of {name} is ticked already: {line}"));
    }
    let Some(first) = unticked(line) else {
        return Err(format!(
            "line {item} of {name} is not an unticked checklist item: {line}"
        ));
    };
    // Two items worded alike can't be told apart once one has a worktree.
    if lines
        .iter()
        .filter(|l| unticked(l).as_deref() == Some(&first))
        .count()
        > 1
    {
        return Err(format!(
            "{name} has more than one item reading \"{first}\": reword one first"
        ));
    }
    // An item whose After: line names one still open waits for it: asked
    // about, a closed input being no. Autopilot's queue leaves held items
    // out, so its tabs never come here with one.
    if let Some(other) = held_by(&text, number)
        && !open_anyway(&format!(
            "{first} waits on {other}, still open. Open it anyway? [Y/n]"
        ))
    {
        return Err(format!("{first} waits on {other}, still open"));
    }
    let block = item_block(&lines, at);
    let slug: String = worktree::slugify(&first)
        .chars()
        .take(SLUG_LENGTH)
        .collect();
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() {
        return Err(format!(
            "the item's first line has nothing to name a branch after: {first}"
        ));
    }
    let branch = format!("note/{slug}");
    let root = worktree::repo_root().map_err(|e| e.to_string())?;
    // note-<slug>, the branch's name made flat, as wtb makes a branch's, so
    // a worktree of another branch named like the item is never this one's.
    let path = worktree::normalize(&root.join(target).join(format!("note-{slug}")));
    let title = lines[0]
        .strip_prefix('#')
        .unwrap_or(lines[0])
        .trim()
        .to_string();
    let title = if title.is_empty() {
        name.strip_suffix(".md").unwrap_or(&name).to_string()
    } else {
        title
    };
    let plan_name = PLAN_LINK
        .captures(&without_after(&block))
        .map(|m| m[1].to_string());
    let plan = plan_name.as_deref().and_then(|n| plan_file(&dir, n));

    eprintln!("Note:     {title}");
    eprintln!("Item:     {first}");
    if let Some(plan_name) = &plan_name {
        match &plan {
            Some(p) => eprintln!("Plan:     {}", p.display()),
            None => eprintln!("Plan:     {plan_name} (not found)"),
        }
    }
    eprintln!("Branch:   {branch}");
    eprintln!("Worktree: {}", path.display());

    // Which item the branch is for, the note's ID (given one here when it
    // has none) and the item's first line, kept in its git config, so
    // another item whose first line slugs the same, in this note or another,
    // is refused rather than started inside this one's work, and the note
    // renamed or moved since is still the same note. The same first line in
    // another note is the same item, copied there: its worktree is taken
    // over and the claim moved to this note. Claims from before
    // notes had IDs, by the note's path, are brought to IDs once first.
    let unplaced = id::migrate_claims(&root, &dir);
    if !unplaced.is_empty() {
        eprintln!(
            "Couldn't find the note of these older claims, left as they were:\n{}",
            unplaced
                .iter()
                .map(|c| format!("  {c}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
    let note_id =
        id::ensure_id(&file, &dir).map_err(|e| format!("couldn't give {name} its ID: {e}"))?;
    let relative = file
        .strip_prefix(&dir)
        .map(|r| r.to_string_lossy().into_owned())
        .unwrap_or_default();
    let identity = format!("{note_id}: {first}");
    let key = format!("branch.{branch}.wtnItem");
    let recorded = worktree::git_at(&root, &["config", "--get", &key]).unwrap_or_default();
    // A claim from before IDs that the migration couldn't place, its note
    // back at that path since, is this item's too, and is rewritten below.
    let by_path = recorded == format!("{relative}: {first}");
    let other_note = id::claim_of(&recorded).filter(|_| recorded != identity && !by_path);
    // The other note, found by its ID wherever it is now, its title and
    // whether it still has an unticked item reading `first`.
    let other_title = other_note.map(|(other_id, other_text)| {
        let found = id::find_by_id(other_id, &dir).map(|f| {
            let text = fs::read_to_string(&f).unwrap_or_default();
            let title = id::title_of(&text).unwrap_or_else(|| {
                f.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            let open = items(&text).iter().any(|i| i.text == other_text);
            (title, open)
        });
        (other_text, found)
    });
    if let Some((other_text, found)) = &other_title
        && *other_text == first
    {
        let (from, open) = match found {
            Some((title, open)) => (title.as_str(), *open),
            None => ("a note no longer there", false),
        };
        let still = if open {
            format!("; it is still open in {from} too")
        } else {
            String::new()
        };
        eprintln!("Taking over {branch}, started from {from}{still}.");
    } else if !recorded.is_empty() && recorded != identity && !by_path {
        let other = match &other_title {
            Some((other_text, found)) => format!(
                "\"{other_text}\" in {}",
                found
                    .as_ref()
                    .map(|(title, _)| title.as_str())
                    .unwrap_or("a note no longer there")
            ),
            None => format!("\"{recorded}\""),
        };
        let refusal = format!(
            "{branch} is for another item: {other}.\n\
             Land or remove that worktree first, or, if the item was only reworded, tell git it is the same one:\n  \
             git config {key} '{}'",
            identity.replace('\'', r"'\''")
        );
        refused_in_autopilot(&format!("{branch} is for another item: {other}."));
        return Err(refusal);
    }
    let target = match given.last("--target").filter(|t| !t.is_empty()) {
        Some(target) if crate::repos::landing_for(&root, false)?.landing != "direct" => {
            // Where PRs land, the default branch named is the default
            // branch, cut from origin's as it is now with no target
            // recorded, since that is where its PR goes; any other is
            // refused.
            let default = crate::tabs::git::locate(&root)
                .and_then(|p| crate::tabs::git::default_branch(&p.common_dir));
            if default.as_deref() != Some(target) {
                return Err(format!(
                    "{} lands through PRs, so an item cut from {target} would land on the default branch through a PR, carrying {target}'s commits. \
                     Start it without --target, or set \"landing\": \"direct\" for the repository in ~/.config/switchboard/repos.json",
                    paths::tilde(&root)
                ));
            }
            None
        }
        target => target,
    };
    let start = start_point(&root, target)?;
    let purpose = Purpose {
        work: "item",
        command: "wtn",
        start: Some(&start),
    };
    worktree::ensure_worktree(&path, &branch, &purpose).map_err(|e| e.to_string())?;
    // Without it the item looks unclaimed, and would be offered again.
    if !worktree::git_ok(&root, &["config", &key, &identity]) {
        return Err(format!("could not record the item in git config ({key})"));
    }
    claim(&note_id, &first);
    // When the item started, for the time its Done line says it took
    // (`switchboard landing`'s `took`): kept when a rerun reuses it.
    let started_key = format!("branch.{branch}.wtnStarted");
    if worktree::git_at(&root, &["config", "--get", &started_key])
        .unwrap_or_default()
        .is_empty()
    {
        worktree::git_ok(
            &root,
            &["config", &started_key, &crate::util::now_secs().to_string()],
        );
    }
    // The branch it lands on, read by `switchboard landing` for
    // queue-and-clean; none, the default branch.
    let target_key = format!("branch.{branch}.wtnTarget");
    let recorded_target = match target {
        Some(t) => worktree::git_ok(&root, &["config", &target_key, t]),
        None => {
            worktree::git_ok(&root, &["config", "--unset", &target_key]);
            true
        }
    };
    if !recorded_target {
        return Err(format!(
            "could not record the target in git config ({target_key})"
        ));
    }
    worktree::link_local_settings(&root, &path).map_err(|e| e.to_string())?;

    let extra = worktree::one_line(&given.all("--prompt").join(" "));
    let hands_free = in_hands_free_tab();
    let plan_words = match (&plan, &plan_name) {
        (Some(p), _) if hands_free => format!(
            "Its plan is {}: read it first. This tab is hands-free: where the plan or the item still marks a choice as needed, take the recommended option (else the plan's first), tick it in the plan with a `Decided hands-free: <choice>` line, then build it, as long as `switchboard show @autopilot_handsfree` still prints 1 when you decide (otherwise hands-free was turned off: ask me as usual). ",
            p.display()
        ),
        (Some(p), _) => format!(
            "Its plan is {}: read it first, and before building, ask me about any choice it still marks as needed. ",
            p.display()
        ),
        (None, Some(n)) => format!(
            "It links a plan, \"{n}\", that isn't in the notes directory: tell me before going on. "
        ),
        (None, None) if hands_free => {
            "It has no plan: read the code the item touches. This tab is hands-free: decide what it leaves open with the option you recommend, writing each decision as an indented `Decided hands-free: <question> → <choice>` line under the item in the note, then build it, as long as `switchboard show @autopilot_handsfree` still prints 1 when you decide (otherwise hands-free was turned off: ask me before building anything it leaves open). "
                .to_string()
        }
        (None, None) => {
            "It has no plan: read the code the item touches, and ask me before building anything it leaves open. "
                .to_string()
        }
    };
    let onto = match target {
        Some(t) => format!(
            "{t}, the branch it was cut from (its target, which `switchboard landing` names)"
        ),
        None => "the default branch".to_string(),
    };
    let mut prompt = format!(
        "Work on this checklist item from my note \"{title}\" ({}), line {item} there, as written:\n\n<item>\n{block}\n</item>\n\n{plan_words}{}\
         Commit the work on this worktree's branch, following the repository's CLAUDE.md, and don't push it or tick the item in the note. \
         When the work is done, say so in a line, then land it as /queue-and-clean does, which only I can start, so read the file `switchboard skill-path queue-and-clean` prints and follow it: it asks me before pushing, lands it and ticks the item, as `switchboard landing` says for the repository: straight on {onto}, with the steps it gives, or else through a PR it offers to open.",
        file.display(),
        check_first(hands_free),
    );
    if !extra.is_empty() {
        prompt.push_str(&format!(" Also, from me for this item: {extra}"));
    }
    let settings = push_rule(&path, &root, &start, target);
    // Read by the `wtn` function: line 1 the worktree, the rest the prompt.
    if let Some(cd_file) = &given.cd_file {
        worktree::write_cd_file(cd_file, &format!("{}\n{prompt}\n", path.display()))
            .map_err(|e| e.to_string())?;
    }
    // Read by it too: the settings its Claude starts with, empty for none.
    if let Some(settings_file) = given.last("--settings-file") {
        fs::write(settings_file, settings.as_deref().unwrap_or_default())
            .map_err(|e| format!("couldn't write {settings_file}: {e}"))?;
    }
    if given.no_claude {
        return Ok(ExitCode::SUCCESS);
    }
    let args = match settings {
        Some(settings) => vec!["--settings".to_string(), settings, prompt],
        None => vec![prompt],
    };
    Ok(worktree::run_in("claude", &args, &path))
}

/// What the tab this runs in has an option set to, read from the server;
/// `None` outside a pane, or with it unset.
fn tab_option(option: &str) -> Option<String> {
    if !crate::mux::socket::in_pane() {
        return None;
    }
    let pane = std::env::var("SWITCHBOARD_PANE").ok();
    let (code, out, _) =
        crate::mux::socket::ask_quiet(vec!["show".into(), option.into()], pane, None)?;
    let value = String::from_utf8_lossy(&out).trim().to_string();
    (code == 0 && !value.is_empty()).then_some(value)
}

/// A yes/no question on the terminal, Enter being yes; no terminal, or one
/// closed, is no.
fn open_anyway(question: &str) -> bool {
    use std::io::{BufRead, Write};
    let Ok(tty) = fs::File::open("/dev/tty") else {
        return false;
    };
    eprint!("{question} ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    match std::io::BufReader::new(tty).read_line(&mut line) {
        Ok(0) | Err(_) => false,
        Ok(_) => matches!(line.trim().to_lowercase().as_str(), "" | "y" | "yes"),
    }
}

/// Whether the tab this runs in is one of autopilot's, hands-free
/// (`@autopilot_handsfree`, which autopilot sets as it opens the tab).
fn in_hands_free_tab() -> bool {
    tab_option(crate::autopilot::HANDS_FREE).as_deref() == Some("1")
}

/// In a tab autopilot opened (`@autopilot`), why wtn refused its item, set
/// as the tab's `@autopilot_refused`, so autopilot skips the item with that
/// reason and closes the tab rather than leave a slot waiting at a shell.
fn refused_in_autopilot(why: &str) {
    if tab_option(crate::autopilot::OPENED).is_some() {
        let _ = crate::mux::socket::ask(&["set", crate::autopilot::REFUSED, why]);
    }
}

/// The settings a tab of autopilot's starts its Claude with, for that
/// session alone: the one push queue-and-clean's landing makes, allowed, so
/// Claude Code's auto mode, which takes an allow rule before its own
/// judgment, doesn't refuse a push the landing's checks already passed.
/// Only in a tab autopilot opened (`@autopilot`), in a repository landing
/// straight on a branch that is on a remote, with `autopilot_push_rule` on:
/// every other tab and push is judged as before. Whether the tab lands
/// without asking isn't read, since that is turned on and off while it
/// works and the landing reads it just before the push.
fn push_rule(path: &Path, root: &Path, start: &str, target: Option<&str>) -> Option<String> {
    if !crate::settings::is_on("autopilot_push_rule") {
        return None;
    }
    tab_option(crate::autopilot::OPENED)?;
    // The default branch on origin, which the item's branch starts from; a
    // target, the remote branch it tracks. With no remote the landing moves
    // a local branch, which needs no rule.
    let (remote, branch) = match target {
        Some(target) => tracked(root, target)?,
        None => (
            "origin".to_string(),
            start.strip_prefix("refs/remotes/origin/")?.to_string(),
        ),
    };
    let settings = push_settings(&remote, &branch)?;
    let lands_direct = crate::repos::landing_for(path, false).is_ok_and(|l| l.landing == "direct");
    lands_direct.then_some(settings)
}

/// Claude Code's settings allowing `git push <remote> HEAD:<branch>`, the
/// command queue-and-clean's step 11 runs, and nothing else; none for a
/// name a permission rule could read as more than itself (a parenthesis, a
/// space), which then asks as any push does.
fn push_settings(remote: &str, branch: &str) -> Option<String> {
    let plain = |name: &str| {
        !name.is_empty()
            && !name.starts_with('-')
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c))
    };
    (plain(remote) && plain(branch)).then(|| {
        serde_json::json!({
            "permissions": { "allow": [format!("Bash(git push {remote} HEAD:{branch})")] }
        })
        .to_string()
    })
}

/// What Claude does before building, as `wt`'s prompt has it for a ticket:
/// an item can sit in a note while the code moves under it. A stale one stops
/// short of landing, so autopilot's tab, which runs `wtn` too, waits on you
/// rather than building it; in a hands-free tab, one already done on the
/// target is ticked and its tab freed instead, since nothing is left to
/// decide, and starting it there asked for its worktree's removal once done.
fn check_first(hands_free: bool) -> String {
    let done = if hands_free {
        "If it is already done on the branch it lands on, and `switchboard show @autopilot_handsfree` still prints 1 when you find it so (otherwise hands-free was turned off: tell me with the evidence and stop, as usual), tick it in the note, as the note's rules say, with an indented `Done: <short hash of the commit that did it>, already on <that branch>. <what it does now>` line under it, \
         then land it as below, which finds the work already there and cleans up, without asking me; what you built beyond what that branch has (a test, a doc) lands as the item's change instead. \
         If it no longer applies, tell me so with the evidence (the code, or the commit that did it) and stop there, without building, landing or ticking it, so I decide. \
         Starting this item hands-free is my ask to remove its worktree and branch once its work has landed: don't ask about that cleanup when nothing would be lost. "
    } else {
        "If it is already done or no longer applies, tell me so with the evidence (the code, or the commit that did it) and stop there, \
         without building, landing or ticking it, so I decide. "
    };
    format!("{CHECK_FIRST}{done}{THEN}")
}

const CHECK_FIRST: &str = "Before building, compare the item and its plan with the code as it is now: is it still needed, already done, or worth doing another way? ";

const THEN: &str = "Where the plan gets a detail wrong (a wrong type, the wrong file, a function that turns out to be unused), \
     build what the code actually needs rather than stopping to ask, then correct the plan so it matches what you built. \
     Stop and wait for my answer only for a change of scope: doing it another way, widening the work to another file, deleting a caller, changing an API. ";

/// What `skip` holds from `WTN_SKIP`, one first line a line, as the daemon
/// handed it to wtn-open.zsh.
pub fn skip_from(text: &str) -> HashSet<String> {
    text.lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hands_free_tab_ticks_an_item_already_done_and_frees_itself_where_another_stops() {
        let hands_free = check_first(true);
        assert!(hands_free.contains("If it is already done on the branch it lands on, and `switchboard show @autopilot_handsfree` still prints 1"));
        assert!(hands_free.contains("don't ask about that cleanup when nothing would be lost"));
        assert!(!hands_free.contains("If it is already done or no longer applies"));
        let asking = check_first(false);
        assert!(
            asking.contains(
                "If it is already done or no longer applies, tell me so with the evidence"
            )
        );
        assert!(!asking.contains("tick it in the note"));
        for text in [hands_free, asking] {
            assert!(text.starts_with("Before building, compare"));
            assert!(text.ends_with("deleting a caller, changing an API. "));
        }
    }

    #[test]
    fn the_settings_allow_the_landings_one_push_and_none_for_a_name_a_rule_could_widen() {
        assert_eq!(
            push_settings("origin", "main").as_deref(),
            Some(r#"{"permissions":{"allow":["Bash(git push origin HEAD:main)"]}}"#)
        );
        assert_eq!(
            push_settings("team", "release/2.x").as_deref(),
            Some(r#"{"permissions":{"allow":["Bash(git push team HEAD:release/2.x)"]}}"#)
        );
        for (remote, branch) in [
            ("origin", "fix(ui)"),
            ("origin", "--force"),
            ("origin", ""),
            ("my remote", "main"),
        ] {
            assert_eq!(push_settings(remote, branch), None, "{remote} {branch}");
        }
    }

    #[test]
    fn the_item_on_a_line_is_an_unticked_top_level_one_or_says_why_not() {
        let text = "# Plan\n\n- [ ] Fix the login\n  - [ ] A step\n- [x] Done\nProse\n\n## Later\n\n- [ ] Someday\n";
        let item = item_at(text, 3).unwrap();
        assert_eq!((item.line, item.text.as_str()), (3, "Fix the login"));
        assert!(
            item_at(text, 4)
                .unwrap_err()
                .starts_with("is under another")
        );
        assert_eq!(item_at(text, 5), Err("is ticked"));
        assert_eq!(item_at(text, 6), Err("isn't a checklist item"));
        assert_eq!(item_at(text, 10), Err("is in a section of parked work"));
        assert_eq!(item_at(text, 0), Err("isn't in the note"));
        assert_eq!(item_at(text, 99), Err("isn't in the note"));
    }

    #[test]
    fn each_item_carries_the_plan_it_links_without_the_plans_heading_and_item_line() {
        let dir = std::env::temp_dir().join(format!("sb-plan-notes-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Plans")).unwrap();
        fs::create_dir_all(dir.join(".trash")).unwrap();
        fs::write(
            dir.join("Plans/Plan - Stable IDs.md"),
            "# Plan - Stable IDs\n\nItem: \"Give notes stable IDs\"\n\n## Today\n\nNotes are found by name.\n\n<!-- note-id: n-0123456789ab -->\n",
        )
        .unwrap();
        fs::write(dir.join(".trash/Plan - Gone.md"), "# Plan - Gone\n").unwrap();
        let note = "# Work\n\n- [ ] Give notes stable IDs\n  Context: see Plan: [[Plan - Gone]]\n  - Plan:  [[ Plan - Stable IDs ]]\n- [ ] No plan here\n- [x] Landed already\n  - [ ] A step under it\n    Plan: [[Plan - Stable IDs]]\n  Plan: [[Plan - Gone]]\n- [ ] Only a step has one\n  -  [ ] Its step\n    Plan: [[Plan - Stable IDs]]\n";
        let plans = plan_notes(&dir, note);
        assert_eq!(
            plans.get("Give notes stable IDs"),
            Some(&PlanNote {
                name: "Plan - Stable IDs".into(),
                text: Some("## Today\n\nNotes are found by name.".into()),
            })
        );
        assert_eq!(plans.get("No plan here"), None);
        assert_eq!(
            plans.get("Landed already"),
            Some(&PlanNote {
                name: "Plan - Gone".into(),
                text: None,
            }),
            "a ticked item's own link, past its sub-item's, its note in the trash not found"
        );
        assert_eq!(
            plans.get("Only a step has one"),
            None,
            "a plan under a sub-item is the sub-item's"
        );
        assert_eq!(plans.len(), 2, "top-level items alone: {plans:?}");
        assert_eq!(
            item_plan_file(&dir, note, "Give notes stable IDs"),
            Some(dir.join("Plans/Plan - Stable IDs.md"))
        );
        assert_eq!(item_plan_file(&dir, note, "No plan here"), None);
        assert_eq!(
            item_plan_file(&dir, note, "Landed already"),
            None,
            "its plan is in the trash"
        );
        assert_eq!(item_plan_file(&dir, note, "Not an item"), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_decision_section_with_its_choice_made_counts_as_settled() {
        let dir = std::env::temp_dir().join(format!("sb-settled-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Plans")).unwrap();
        let plans = [
            (
                "Ticked",
                "## Decision needed\n\n- [ ] A\n- [x] B\n\n## Steps\n",
            ),
            ("Chose", "## Decisions needed\n\nA or B.\n\nChose B.\n"),
            (
                "Nested",
                "## Decision needed\n\n### Options\n\n- [ ] A\n- [x] B\n\n## Steps\n",
            ),
            (
                "Open",
                "## Decision needed\n\n- [ ] A\n  - [x] Check it builds\n- [ ] B\n\n## Steps\n\n- [x] done\n",
            ),
            ("Picked", "## Decision needed\n\n- [ ] A\n- [ ] B\n"),
            ("Asking", "## Decision needed\n\n- [x] A\n- [ ] B\n"),
            (
                "Second",
                "## Decision needed\n\n- [x] A\n\n## Decision needed\n\n- [ ] C\n",
            ),
        ];
        for (name, body) in plans {
            fs::write(
                dir.join(format!("Plans/Plan - {name}.md")),
                format!("# Plan - {name}\n\n{body}"),
            )
            .unwrap();
        }
        let item =
            |name: &str, after: &str| format!("- [ ] {name}\n  Plan: [[Plan - {name}]]{after}\n");
        assert_eq!(
            plan_of(&dir, &item("Ticked", "")),
            Plan::Settled,
            "a ticked option"
        );
        assert_eq!(
            plan_of(&dir, &item("Chose", "")),
            Plan::Settled,
            "a Chose line"
        );
        assert_eq!(
            plan_of(&dir, &item("Nested", "")),
            Plan::Settled,
            "a ticked option under a subheading of the section"
        );
        assert_eq!(
            plan_of(&dir, &item("Open", "")),
            Plan::Choice,
            "only unticked options, a ticked task under one or a ticked step elsewhere not counting"
        );
        assert_eq!(
            plan_of(&dir, &item("Picked", " (chose B)")),
            Plan::Settled,
            "the item saying what was chosen"
        );
        assert_eq!(
            plan_of(&dir, &item("Second", "")),
            Plan::Choice,
            "a second decision still open"
        );

        fs::write(
            dir.join("Work.md"),
            [
                "# Work",
                "",
                "- [ ] Open",
                "  Plan: [[Plan - Open]]",
                "- [ ] Asking",
                "  Plan: [[Plan - Asking]] (needs a choice: A or B)",
                "- [ ] Picked",
                "  Plan: [[Plan - Picked]] (chose B)",
                "- [ ] Ticked",
                "  Plan: [[Plan - Ticked]]",
                "",
            ]
            .join("\n"),
        )
        .unwrap();
        let taken: Vec<String> = queue(
            &dir,
            &dir,
            "Work.md",
            &HashSet::new(),
            true,
            false,
            &dir.join("claims.tsv"),
        )
        .unwrap()
        .into_iter()
        .map(|i| i.text)
        .collect();
        assert_eq!(
            taken,
            vec!["Picked", "Ticked"],
            "an item still saying it needs a choice never taken, whatever its plan says"
        );
        let hands_free: Vec<String> = queue(
            &dir,
            &dir,
            "Work.md",
            &HashSet::new(),
            true,
            true,
            &dir.join("claims.tsv"),
        )
        .unwrap()
        .into_iter()
        .map(|i| i.text)
        .collect();
        assert_eq!(
            hands_free,
            vec!["Picked", "Ticked", "Open", "Asking"],
            "hands-free takes the items needing a choice too, after the settled ones"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_item_wtn_started_in_the_last_half_hour_counts_as_taken() {
        let now = 1_700_000_000;
        let claims = format!(
            "{}\tWork.md\tFresh\n{}\tWork.md\tStale\n{}\tOther.md\tElsewhere\nbroken row\n",
            now - 60,
            now - CLAIMED_S - 1,
            now - 60
        );
        let lately = claimed_lately(&claims, None, "Work.md", now);
        assert_eq!(lately, HashSet::from(["Fresh".to_string()]));
        let by_id = format!(
            "{}\tn-0123456789ab\tBy its ID\n{}\tWork.md\tBy its path\n{}\tn-ffffffffffff\tAnother note's\n",
            now - 60,
            now - 60,
            now - 60
        );
        assert_eq!(
            claimed_lately(&by_id, Some("n-0123456789ab"), "Work.md", now),
            HashSet::from(["By its ID".to_string(), "By its path".to_string()]),
            "by the note's ID, or by its path in a row from before IDs"
        );
    }

    // tmux/wtn-open.zsh's `items`, from test/worktree-from-note.test.js.
    #[test]
    fn a_notes_unticked_top_level_items_marking_those_needing_a_choice_and_none_parked() {
        let note = [
            "# Work",
            "",
            "- [ ] First thing  ",
            "  - a detail",
            "",
            "  Plan: [[Plan - First]] (needs a choice: A or B)",
            "- [x] Done thing",
            "- [ ] Second thing",
            "  - [ ] a sub-item, not an item of its own",
            "",
            "## Feature",
            "- [ ] Third thing",
            "### Later",
            "- [ ] Postponed under a feature",
            "### Now",
            "- [ ] Fourth thing",
            "",
            "## Defered",
            "- [ ] Parked thing",
            "",
            "## Later work",
            "- [ ] Also parked",
            "### Its details",
            "- [ ] Still parked",
            "",
        ]
        .join("\n");
        let rows: Vec<String> = items(&note).iter().map(Item::row).collect();
        assert_eq!(
            rows,
            vec![
                "3\tFirst thing (needs a choice)",
                "8\tSecond thing",
                "12\tThird thing",
                "16\tFourth thing"
            ]
        );
    }

    #[test]
    fn an_items_after_lines_hold_it_while_the_item_they_name_is_open() {
        let note = [
            "# Work",
            "",
            "- [ ] FEAT: Make the tab header",
            "- [ ] FEAT: Right-click an item",
            "  After: FEAT: Make the tab header",
            "  After: BUG: Gone already",
            "  Plan: [[Plan - Right click]]",
            "- [ ] FEAT: By a part of a title",
            "  After: the tab HEADER",
            "- [ ] FEAT: Names nothing open",
            "  After: BUG: Gone already",
            "- [ ] FEAT: A part two titles hold",
            "  After: FEAT:",
            "- [ ] On the parked one",
            "  After: Parked base",
            "- [x] BUG: Gone already",
            "",
            "## Later",
            "- [ ] Parked base",
            "",
        ]
        .join("\n");
        let all = items(&note);
        let by = |t: &str| all.iter().find(|i| i.text == t).unwrap().clone();
        let right = by("FEAT: Right-click an item");
        assert_eq!(
            right.after,
            vec!["FEAT: Make the tab header", "BUG: Gone already"],
            "both After: lines read"
        );
        assert_eq!(
            right.waits_on.as_deref(),
            Some("FEAT: Make the tab header"),
            "held by its exact first line"
        );
        assert_eq!(
            by("FEAT: By a part of a title").waits_on.as_deref(),
            Some("FEAT: Make the tab header"),
            "a part of exactly one open item's title, ignoring case"
        );
        assert_eq!(
            by("FEAT: Names nothing open").waits_on,
            None,
            "a ticked item holds nothing"
        );
        assert_eq!(
            by("FEAT: A part two titles hold").waits_on,
            None,
            "a part several titles hold names none"
        );
        assert_eq!(
            by("On the parked one").waits_on.as_deref(),
            Some("Parked base"),
            "a parked item is still open"
        );
        assert_eq!(
            right.row(),
            "4\tFEAT: Right-click an item (waits on FEAT: Make the tab header)"
        );
        assert_eq!(
            by("FEAT: Make the tab header").waits_on,
            None,
            "an item never holds itself"
        );
        let done = items(
            "- [ ] FEAT: Base follow-up\n- [ ] Builds on it\n  After: FEAT: Base\n- [x] FEAT: Base\n",
        );
        assert_eq!(
            done[1].waits_on, None,
            "a ticked item named exactly holds nothing, though an open title holds its words"
        );
        let named = items(
            "- [ ] Base (needs a choice)\n- [ ] Builds on it\n  After: Base (needs a choice)\n",
        );
        assert!(
            !named[1].needs_choice,
            "the words of the item an After: line names say nothing of this one's plan"
        );
        let parked = format!("{note}- [ ] Parked on it\n  After: FEAT: Make the tab header\n");
        let at = parked
            .split('\n')
            .position(|l| l == "- [ ] Parked on it")
            .unwrap()
            + 1;
        assert_eq!(
            held_by(&parked, at).as_deref(),
            Some("FEAT: Make the tab header"),
            "a parked item, which wtn takes by its line, held the same way"
        );
        assert_eq!(
            held_by(&parked, 3),
            None,
            "the item it waits on holds nothing"
        );
    }

    #[test]
    fn the_queue_leaves_a_held_item_out_until_what_it_waits_on_is_ticked() {
        let dir = std::env::temp_dir().join(format!("sb-held-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let note = |base: &str| {
            [
                "# Work",
                "",
                base,
                "- [ ] Builds on it",
                "  After: The base",
                "- [ ] Free",
                "",
            ]
            .join("\n")
        };
        let read = || {
            queue_and_held(
                &dir,
                &dir,
                "Work.md",
                &HashSet::new(),
                false,
                false,
                &dir.join("claims.tsv"),
            )
            .unwrap()
        };
        let texts = |items: Vec<Item>| items.into_iter().map(|i| i.text).collect::<Vec<_>>();
        fs::write(dir.join("Work.md"), note("- [ ] The base")).unwrap();
        let (taken, held) = read();
        assert_eq!(texts(taken), vec!["The base", "Free"]);
        assert_eq!(texts(held), vec!["Builds on it"]);
        fs::write(dir.join("Work.md"), note("- [x] The base")).unwrap();
        let (taken, held) = read();
        assert_eq!(
            texts(taken),
            vec!["Builds on it", "Free"],
            "taken once what it waits on is ticked"
        );
        assert!(held.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_item_whole_for_its_preview() {
        let note = [
            "# Work",
            "",
            "- [ ] First thing, a long line",
            "  - a detail",
            "",
            "  Plan: [[Plan - First]]",
            "",
            "- [ ] Second thing",
            "## Feature",
            "",
        ]
        .join("\n");
        assert_eq!(
            item_text(&note, 3),
            "- [ ] First thing, a long line\n  - a detail\n\n  Plan: [[Plan - First]]\n"
        );
        assert_eq!(item_text(&note, 8), "- [ ] Second thing\n");
    }
}
