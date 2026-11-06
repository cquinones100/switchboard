//! `worktree-from-note [path] --note <note> --item <line> [--prompt <text>]`:
//! a git worktree on a new branch named after a checklist item of one of
//! your notes (the notes app's Markdown files, in `$NOTES_DIR` or
//! `~/Notes`), with Claude opened in it, told to work on the item and its
//! plan and to land it with /queue-and-clean once the work is done: straight
//! on the default branch in a repository whose CLAUDE.md commits there with
//! no PR, as Switchboard's does, else through a PR. Ported from
//! `bin/worktree-from-note`.
//!
//! `<note>` is the note's path, absolute or within the notes directory,
//! ".md" optional; `<line>` is the item's line number in it, an unticked
//! `- [ ]` line of the note's top level. `init zsh` prints the `wtn`
//! function, which works as `wt` does (worktree_from_ticket.rs).
//!
//! What M-p's "Start work on a note's checklist items" listed, and what
//! working a note continuously takes next (tmux/wtn-open.zsh's `items`,
//! `item`, `unclaimed`, `plan_of` and `queue`), is here too, since it reads
//! the note and the repository the same way: `items`, `item_text`,
//! `unclaimed`, `plan_of` and `queue`, for the server's palette command and
//! the autopilot.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::LazyLock;

use regex::Regex;

use crate::paths;
use crate::worktree::{self, Given, Purpose};

/// A branch name short enough to read in a tab and a prompt.
const SLUG_LENGTH: usize = 50;

fn usage() -> ExitCode {
    eprintln!(
        "Usage: worktree-from-note [path] --note <note> --item <line> [--prompt <text>]\n       \
         worktree-from-note init <zsh|bash>\n\n\
         Examples:\n  \
         worktree-from-note --note \"Switchboard Notes\" --item 12\n\n\
         Path defaults to {} under the repo root.\n\
         --prompt adds to the prompt Claude starts with. The wtn function also takes\n\
         it from WT_PROMPT, and the note from WTN_NOTE, clearing both, so /wtn can\n\
         hand them over.",
        worktree::DEFAULT_WORKTREE_DIR
    );
    ExitCode::from(1)
}

/// The `wtn` function, as `wt`: runs the command with `--no-claude`, cd's
/// into the worktree and starts Claude there. The prompt is every line of
/// the cd-file after the first, since the item it carries can run to
/// several. WT_PROMPT and WTN_NOTE, which /wtn sets on the pane it respawns,
/// are passed on and cleared, so the note's path never has to be quoted into
/// a command.
pub fn shell_function() -> String {
    format!(
        r#"wtn() {{
  local cdfile rc dir prompt
  local -a extra
  extra=()
  [ -n "${{WT_PROMPT:-}}" ] && extra+=(--prompt "$WT_PROMPT")
  [ -n "${{WTN_NOTE:-}}" ] && extra+=(--note "$WTN_NOTE")
  unset WT_PROMPT WTN_NOTE
  cdfile=$(mktemp) || return 1
  {} --no-claude --cd-file "$cdfile" "${{extra[@]}}" "$@"
  rc=$?
  if [ $rc -ne 0 ]; then
    rm -f "$cdfile"
    return $rc
  fi
  dir=$(sed -n 1p "$cdfile")
  prompt=$(sed -n '2,$p' "$cdfile")
  rm -f "$cdfile"
  if [ ! -d "$dir" ]; then
    echo "wtn: expected a worktree directory, got: $dir" >&2
    return 1
  fi
  cd "$dir" || return 1
  claude "$prompt"
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
/// first line, and whether its line or those under it say its plan still
/// needs a choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub line: usize,
    pub text: String,
    pub needs_choice: bool,
}

impl Item {
    /// As wtn-open.zsh printed it: `<line>\t<text>`, with ` (needs a
    /// choice)` after.
    pub fn row(&self) -> String {
        format!(
            "{}\t{}{}",
            self.line,
            self.text,
            if self.needs_choice {
                " (needs a choice)"
            } else {
                ""
            }
        )
    }
}

static NEEDS_CHOICE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"needs (a )?choices?").unwrap());
static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(#+)\s").unwrap());
// The words a section of parked work is named with, as lib/sidebar/todo.js
// has them.
static PARKED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|[^a-z])(later|defer+ed|parked|someday|backlog)([^a-z]|$)").unwrap()
});

/// A note's unticked top-level items, in its order, leaving out any under a
/// section named for parked work (`## Defered`, `## Later`), a heading
/// under such a section still in it.
pub fn items(text: &str) -> Vec<Item> {
    let mut out = Vec::new();
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
                parked = if PARKED.is_match(&line.to_lowercase()) {
                    level
                } else {
                    0
                };
            }
            continue;
        }
        if line.starts_with("- [ ] ") && parked == 0 {
            out.extend(current.take());
            current = Some(Item {
                line: number,
                text: line[6..].trim().to_string(),
                needs_choice: NEEDS_CHOICE.is_match(line),
            });
            continue;
        }
        if line.starts_with(|c: char| !c.is_whitespace()) {
            out.extend(current.take());
            continue;
        }
        if let Some(item) = current.as_mut()
            && NEEDS_CHOICE.is_match(line)
        {
            item.needs_choice = true;
        }
    }
    out.extend(current);
    out
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
/// `<note>: <item's first line>`, the note's path within the notes
/// directory; an item counts as taken only while its branch is checked out
/// in a worktree, so one whose worktree was removed, its branch kept, shows
/// again. `repo` is any directory of the repository.
pub fn unclaimed(repo: &Path, note: &str, text: &str) -> Vec<Item> {
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
    items(text)
        .into_iter()
        .filter(|item| !claimed.contains(format!("{note}: {}", item.text).as_str()))
        .collect()
}

/// Whether an item's plan is settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Its `Plan: [[<name>]]` links a plan note with no "## Decision
    /// needed", the heading plans mark an open choice with.
    Settled,
    /// The plan note has an open choice.
    Choice,
    /// No plan, or its plan note isn't there.
    None,
}

static DECISION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?im)^#+[ \t]+decisions?[ \t]+needed").unwrap());

/// How settled an item's plan is, from the item whole.
pub fn plan_of(notes: &Path, item: &str) -> Plan {
    let Some(name) = PLAN_LINK.captures(item).map(|m| m[1].to_string()) else {
        return Plan::None;
    };
    let Some(file) = plan_file(notes, &name) else {
        return Plan::None;
    };
    match fs::read_to_string(file) {
        Ok(text) if DECISION.is_match(&text) => Plan::Choice,
        Ok(_) => Plan::Settled,
        Err(_) => Plan::None,
    }
}

/// The items working a note continuously would take, in its order: those
/// whose plan is settled first, then those with no plan (or a plan note
/// that isn't there), each in the note's order. An item whose plan needs a
/// choice is never taken, nor one under a parked section, nor one a
/// worktree has, nor an empty one, nor one whose first line is in `skip`
/// (the items skipped, and those opened a moment ago, before wtn has made
/// their branch). `ready_only` takes only the first kind. `note` is the
/// note's path within the notes directory; `repo` the note's repository.
pub fn queue(
    notes: &Path,
    repo: &Path,
    note: &str,
    skip: &HashSet<String>,
    ready_only: bool,
) -> Result<Vec<Item>, String> {
    let file = notes.join(note);
    let text = fs::read_to_string(&file).map_err(|_| format!("No note at {}", file.display()))?;
    let mut settled = Vec::new();
    let mut unplanned = Vec::new();
    for item in unclaimed(repo, note, &text) {
        if item.needs_choice || skip.contains(&item.text) || item.text.trim().is_empty() {
            continue;
        }
        match plan_of(notes, &item_text(&text, item.line)) {
            Plan::Settled => settled.push(item),
            Plan::Choice => {}
            Plan::None if !ready_only => unplanned.push(item),
            Plan::None => {}
        }
    }
    settled.extend(unplanned);
    Ok(settled)
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
fn start_point(root: &Path) -> Result<String, String> {
    if !worktree::remotes(root).iter().any(|r| r == "origin") {
        return worktree::git_at(root, &["rev-parse", "HEAD"]).map_err(|e| e.to_string());
    }
    let fetched = |base: &str| worktree::fetch_branch(root, "origin", base, "wtn: fetch the base");
    let named = worktree::git_at(
        root,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .ok()
    .map(|n| n.strip_prefix("origin/").unwrap_or(&n).to_string());
    let base = match &named {
        Some(name) => fetched(name).then(|| name.clone()),
        None => ["main", "master"]
            .into_iter()
            .find(|b| fetched(b))
            .map(str::to_string),
    };
    match base {
        Some(base) => Ok(format!("origin/{base}")),
        None => Err(format!(
            "couldn't fetch origin's {}, which the item's branch starts from",
            named.as_deref().unwrap_or("main or master")
        )),
    }
}

pub fn main(args: Vec<String>) -> ExitCode {
    let given = match Given::parse(&args, &["--prompt", "--note", "--item"]) {
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
    let plan_name = PLAN_LINK.captures(&block).map(|m| m[1].to_string());
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

    // Which item the branch is for, the note's path within the notes
    // directory and the item's first line, kept in its git config, so
    // another item whose first line slugs the same, in this note or another,
    // is refused rather than started inside this one's work.
    let relative = file
        .strip_prefix(&dir)
        .map(|r| r.to_string_lossy().into_owned())
        .unwrap_or_default();
    let identity = format!("{relative}: {first}");
    let key = format!("branch.{branch}.wtnItem");
    let recorded = worktree::git_at(&root, &["config", "--get", &key]).unwrap_or_default();
    if !recorded.is_empty() && recorded != identity {
        return Err(format!(
            "{branch} is for another item: \"{recorded}\".\n\
             Land or remove that worktree first, or, if the item was only reworded, tell git it is the same one:\n  \
             git config {key} '{}'",
            identity.replace('\'', r"'\''")
        ));
    }
    let start = start_point(&root)?;
    let purpose = Purpose {
        work: "item",
        command: "wtn",
        start: Some(&start),
    };
    worktree::ensure_worktree(&path, &branch, &purpose).map_err(|e| e.to_string())?;
    worktree::git_ok(&root, &["config", &key, &identity]);
    worktree::link_local_settings(&root, &path).map_err(|e| e.to_string())?;

    let extra = worktree::one_line(&given.all("--prompt").join(" "));
    let plan_words = match (&plan, &plan_name) {
        (Some(p), _) => format!(
            "Its plan is {}: read it first, and before building, ask me about any choice it still marks as needed. ",
            p.display()
        ),
        (None, Some(n)) => format!(
            "It links a plan, \"{n}\", that isn't in the notes directory: tell me before going on. "
        ),
        (None, None) => {
            "It has no plan: read the code the item touches, and ask me before building anything it leaves open. "
                .to_string()
        }
    };
    let mut prompt = format!(
        "Work on this checklist item from my note \"{title}\" ({}), line {item} there, as written:\n\n<item>\n{block}\n</item>\n\n{plan_words}\
         Commit the work on this worktree's branch, following the repository's CLAUDE.md, and don't push it or tick the item in the note. \
         When the work is done, say so in a line, then land it as /queue-and-clean does, which only I can start, so read the file `skill-path queue-and-clean` prints and follow it: it asks me before pushing, lands it and ticks the item, straight on the default branch, reviewed, where the repository's CLAUDE.md says its work is committed there with no PR, else through a PR it offers to open.",
        file.display()
    );
    if !extra.is_empty() {
        prompt.push_str(&format!(" Also, from me for this item: {extra}"));
    }
    // Read by the `wtn` function: line 1 the worktree, the rest the prompt.
    if let Some(cd_file) = &given.cd_file {
        worktree::write_cd_file(cd_file, &format!("{}\n{prompt}\n", path.display()))
            .map_err(|e| e.to_string())?;
    }
    if given.no_claude {
        return Ok(ExitCode::SUCCESS);
    }
    Ok(worktree::run_in("claude", &[prompt], &path))
}

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
