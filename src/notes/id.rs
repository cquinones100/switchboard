//! A note's stable ID, which survives the note being renamed or moved, in
//! the app or outside it (`mv`, Finder, git, a Claude editing the file): a
//! last line `<!-- note-id: n-<12 hex> -->`. Ported from
//! notes/lua/notes/id.lua, the app's side, and lib/note-id.js, the side of
//! everything else that names a note: wtn's branch claims
//! (src/cmd/worktree_from_note.rs), and autopilot's note and the
//! items it skips or holds (src/autopilot), which name a note by its ID
//! rather than by its path, so a rename doesn't lose them.
//!
//! The app (app.rs) gives a note one when it makes it or first opens it
//! without one, keeps it last on every save, puts it back when typing took
//! it out, and never shows it: the ID line, and the blank line written
//! before it, are kept out of the text it edits (`body_of`), and written
//! back last (`file_of`), so a note reads back as it was written. Titles,
//! previews and searches leave it out (`strip`), and a squash sends none to
//! Claude.
//!
//! Outside the app, wtn and the mode give a note its ID (`ensure_id`), or
//! give it back to a note rewritten without it (`give_id`), with its
//! modified time kept, so the note doesn't move up the list or read as just
//! changed; the app takes the line in as it takes in any change made from
//! outside. Several `wtn` started at once give one note its ID, or bring one
//! repository's claims to IDs, one process at a time, each reading the note
//! again under a lock, so all of them claim by the one ID the note keeps.
//!
//! A copy of a note's file carries the original's ID until it is given one
//! of its own: of two notes holding one ID, the one changed later (or,
//! changed at the same moment, the one whose path sorts later) is the copy
//! (`holding`).
//!
//! wtn's claims (`branch.<branch>.wtnItem` in the repository's git config)
//! from before notes had IDs named the note by its path; `migrate_claims`
//! brings them to IDs once per repository.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime};

use regex::Regex;

use crate::{util, worktree};

static LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*<!-- note-id: (n-[0-9a-f]{12}) -->\s*$").unwrap());
/// An ID line formatting wrapped in other text (a checklist box, bold).
static EMBEDDED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<!-- note-id: n-[0-9a-fA-F]+ -->").unwrap());
static CLAIM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(n-[0-9a-f]{12}): ((?s).*)$").unwrap());
static ITEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^- \[[ xX]\] (.*\S.*)$").unwrap());

/// How much of a note's end is read for its ID first: the line, and the
/// blank lines and spaces an editor may leave after it. A note with lines
/// added below its ID since (by a script, before the app moved the ID last
/// again) is read whole.
const TAIL: u64 = 300;

/// A new ID: `n-` and twelve hex digits.
pub fn new() -> String {
    let mut bytes = [0u8; 6];
    let read = fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes));
    if read.is_err() {
        use std::hash::{BuildHasher, Hasher};
        let mut hasher = std::hash::RandomState::new().build_hasher();
        hasher.write_u128(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        hasher.write_u32(std::process::id());
        bytes.copy_from_slice(&hasher.finish().to_le_bytes()[..6]);
    }
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("n-{hex}")
}

/// The line holding `id`.
pub fn line(id: &str) -> String {
    format!("<!-- note-id: {id} -->")
}

/// The ID a line holds, the line being an ID line and nothing else.
pub fn of_line(line: &str) -> Option<&str> {
    LINE.captures(line)
        .and_then(|m| m.get(1))
        .map(|m| m.as_str())
}

/// Whether a line is an ID line.
pub fn is_line(line: &str) -> bool {
    LINE.is_match(line)
}

/// The ID in a note's lines, from the last line holding one, and its row.
pub fn in_lines<S: AsRef<str>>(lines: &[S]) -> Option<(String, usize)> {
    lines
        .iter()
        .enumerate()
        .rev()
        .find_map(|(i, l)| of_line(l.as_ref()).map(|id| (id.to_string(), i)))
}

/// The ID in a note's text.
pub fn in_text(text: &str) -> Option<String> {
    text.split('\n')
        .rev()
        .find_map(|l| of_line(l).map(str::to_string))
}

/// The text without its ID line, for whatever shows, searches or sends a
/// note's words.
pub fn strip(text: &str) -> String {
    if !text.contains("<!-- note-id: ") {
        return text.to_string();
    }
    text.split('\n')
        .filter(|l| !is_line(l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A file's lines, as the app reads them: the newline that ends the file
/// isn't a line of its own.
pub fn lines_of(text: &str) -> Vec<&str> {
    text.strip_suffix('\n')
        .unwrap_or(text)
        .split('\n')
        .collect()
}

/// A note's text as the app edits it: without its ID line and the blank
/// line written before it, nor any other ID line, so `file_of` gives the
/// file back as it was.
pub fn body_of(text: &str) -> String {
    let mut lines = lines_of(text);
    if lines.last().is_some_and(|l| is_line(l)) {
        lines.pop();
        if lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.pop();
        }
    }
    let kept: Vec<&str> = lines.into_iter().filter(|l| !is_line(l)).collect();
    if kept.is_empty() {
        String::new()
    } else {
        format!("{}\n", kept.join("\n"))
    }
}

/// A note's file from the lines the app edits: those lines, without any
/// ID, then a blank line and `id`'s. An ID line formatting wrapped in other
/// text (a checklist box, bold) loses the ID, and the line goes when
/// nothing else is left on it.
pub fn file_of<S: AsRef<str>>(lines: &[S], id: &str) -> String {
    let mut out = without_ids(lines);
    out.push(String::new());
    out.push(line(id));
    format!("{}\n", out.join("\n"))
}

/// A note's text with `id` as its last line, after a blank one, and no
/// other ID line.
pub fn with(text: &str, id: &str) -> String {
    file_of(&lines_of(&body_of(text)), id)
}

/// A note's lines without any ID, its own line or wrapped in other text,
/// every other line left as it is.
pub fn without_ids<S: AsRef<str>>(lines: &[S]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 2);
    for line in lines {
        let line = line.as_ref();
        if is_line(line) {
            continue;
        }
        if EMBEDDED.is_match(line) {
            let rest = EMBEDDED.replace_all(line, "");
            if rest.chars().any(char::is_alphanumeric) {
                out.push(rest.into_owned());
            }
            continue;
        }
        out.push(line.to_string());
    }
    out
}

/// Whether any line holds an ID, as its own line or wrapped in other text.
pub fn any_in<S: AsRef<str>>(lines: &[S]) -> bool {
    lines.iter().any(|l| EMBEDDED.is_match(l.as_ref()))
}

/// The ID of the note at `path`, read from its last bytes, or from the
/// whole note when lines were added below its ID since.
pub fn of_file(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let from = size.saturating_sub(TAIL);
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    if let Some(id) = in_text(&String::from_utf8_lossy(&tail)) {
        return Some(id);
    }
    if from == 0 {
        return None;
    }
    in_text(&util::read_text(path)?)
}

/// Every note of the notes directory but its deleted ones and other
/// dot-folders.
pub fn note_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                if depth < 20 {
                    walk(&path, depth + 1, out);
                }
            } else if kind.is_file() && path.extension().is_some_and(|x| x == "md") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, 1, &mut out);
    out
}

fn real(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The notes holding `id`, the one made first first: the copy of a note's
/// file is the one changed later, or, changed at the same moment, the one
/// whose path sorts later.
pub fn holding(id: &str, dir: &Path) -> Vec<PathBuf> {
    if id.is_empty() {
        return Vec::new();
    }
    let mut found: Vec<(SystemTime, PathBuf)> = note_files(dir)
        .into_iter()
        .filter(|f| of_file(f).as_deref() == Some(id))
        .map(|f| {
            let at = fs::metadata(&f)
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            (at, real(&f))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, f)| f).collect()
}

/// The note holding `id`, the original when a copy holds it too.
pub fn find_by_id(id: &str, dir: &Path) -> Option<PathBuf> {
    holding(id, dir).into_iter().next()
}

/// The note another note holding the same ID was copied from, when `path`
/// is that copy.
pub fn copy_of(path: &Path, id: &str, dir: &Path) -> Option<PathBuf> {
    let first = find_by_id(id, dir)?;
    (first != real(path)).then_some(first)
}

/// Writes `text` to the note with `id` as its last line, its modified time
/// kept, so a note given an ID doesn't move up the notes list or read as
/// just changed (autopilot waits a minute after a change).
pub fn write_id(file: &Path, text: &str, id: &str) -> io::Result<()> {
    let meta = fs::metadata(file)?;
    util::write_atomic(file, with(text, id))?;
    keep_times(file, &meta)
}

/// A file's times put back as they were before it was written.
pub fn keep_times(file: &Path, was: &fs::Metadata) -> io::Result<()> {
    let mut times = fs::FileTimes::new();
    if let Ok(at) = was.modified() {
        times = times.set_modified(at);
    }
    if let Ok(at) = was.accessed() {
        times = times.set_accessed(at);
    }
    fs::File::options().write(true).open(file)?.set_times(times)
}

/// A note given back an ID it lost, as when something outside the app
/// rewrote it without its last line.
pub fn give_id(file: &Path, id: &str) -> io::Result<()> {
    let text = fs::read_to_string(file)?;
    write_id(file, &text, id)
}

/// The note's ID, given one when it has none, or a new one when it is a
/// copy of another note holding the same. `dir` is the notes directory.
pub fn ensure_id(file: &Path, dir: &Path) -> io::Result<String> {
    let settled = || -> io::Result<(String, Option<String>)> {
        let text = fs::read_to_string(file)?;
        let id = in_text(&text).filter(|id| copy_of(file, id, dir).is_none());
        Ok((text, id))
    };
    if let (_, Some(id)) = settled()? {
        return Ok(id);
    }
    locked(file, || {
        let (text, id) = settled()?;
        if let Some(id) = id {
            return Ok(id);
        }
        let fresh = new();
        write_id(file, &text, &fresh)?;
        Ok(fresh)
    })
}

/// A lock older than this is a process that died holding it.
const LOCK_STALE: Duration = Duration::from_secs(10);
/// Past this, the work goes on without one.
const LOCK_WAIT: Duration = Duration::from_secs(5);

/// Runs `f` holding a lock on `key` (a note, or a repository), in the
/// temporary directory, named for the key's real path, so every process
/// giving the same note its ID takes the same lock.
fn locked<T>(key: &Path, f: impl FnOnce() -> T) -> T {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(real(key).to_string_lossy().as_bytes());
    let name: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    let lock = std::env::temp_dir().join(format!("note-id-{name}.lock"));
    let start = Instant::now();
    let mut held = false;
    while !held && start.elapsed() < LOCK_WAIT {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
        {
            Ok(mut file) => {
                use std::io::Write;
                let _ = write!(file, "{}", std::process::id());
                held = true;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let stale = fs::metadata(&lock)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|at| at.elapsed().ok())
                    .is_some_and(|age| age > LOCK_STALE);
                if stale {
                    let _ = fs::remove_file(&lock);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break,
        }
    }
    let out = f();
    if held {
        let _ = fs::remove_file(&lock);
    }
    out
}

/// A note's title as wtn names it: its first line that isn't blank or its
/// ID, without Markdown's heading marks; `None` when it has none.
pub fn title_of(text: &str) -> Option<String> {
    text.split('\n')
        .filter(|l| !is_line(l))
        .map(|l| l.trim_start().trim_start_matches('#').trim().to_string())
        .find(|t| !t.is_empty())
}

/// A wtn claim by a note's ID: the ID and the item's first line.
pub fn claim_of(value: &str) -> Option<(&str, &str)> {
    let m = CLAIM.captures(value)?;
    Some((m.get(1)?.as_str(), m.get(2)?.as_str()))
}

/// Marks a repository whose claims have been brought to IDs.
const MIGRATED: &str = "switchboard.wtnClaimsById";

/// wtn's claims (`branch.<branch>.wtnItem`) from before notes had IDs read
/// `<note's path in the notes directory>: <item's first line>`. Once per
/// repository (`switchboard.wtnClaimsById` in its git config marks it
/// done), each is rewritten as `<note's ID>: <item's first line>`, the note
/// found at that path, else the one note titled as the path's name, else
/// the one note holding the item, each only when it holds the item, ticked
/// or not, so another note made under a renamed note's old name or title
/// isn't taken for it. Gives the claims it couldn't place, left as they
/// were, which the picker and the mode still read by path. Runs git, so it
/// is never called on the server's task.
pub fn migrate_claims(root: &Path, dir: &Path) -> Vec<String> {
    let done =
        || worktree::git_at(root, &["config", "--get", MIGRATED]).is_ok_and(|v| v.trim() == "true");
    if done() {
        return Vec::new();
    }
    // One at a time per repository, several being started at once, each
    // looking at the marker again once it has the lock.
    locked(root, || {
        if done() {
            return Vec::new();
        }
        let mut unplaced = Vec::new();
        let claims = worktree::git_at(root, &["config", "--get-regexp", r"^branch\..*\.wtnitem$"])
            .unwrap_or_default();
        struct Note {
            file: PathBuf,
            title: String,
            items: std::collections::HashSet<String>,
        }
        let mut notes: Option<Vec<Note>> = None;
        for row in claims.lines().filter(|l| !l.is_empty()) {
            let Some((key, value)) = row.split_once(' ') else {
                continue;
            };
            if claim_of(value).is_some() {
                continue;
            }
            let Some((note, text)) = value
                .split_once(".md: ")
                .map(|(n, t)| (format!("{n}.md"), t))
            else {
                unplaced.push(value.to_string());
                continue;
            };
            let notes = notes.get_or_insert_with(|| {
                note_files(dir)
                    .into_iter()
                    .map(|file| {
                        let text = util::read_text(&file).unwrap_or_default();
                        Note {
                            title: title_of(&text).unwrap_or_default(),
                            items: text
                                .split('\n')
                                .filter_map(|l| ITEM.captures(l).map(|m| m[1].trim().to_string()))
                                .collect(),
                            file: real(&file),
                        }
                    })
                    .collect()
            });
            let holders: Vec<&Note> = notes.iter().filter(|n| n.items.contains(text)).collect();
            let only = |list: Vec<&Note>| (list.len() == 1).then(|| list[0].file.clone());
            let at = dir.join(&note);
            let named = Path::new(&note)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let file = holders
                .iter()
                .find(|n| at.exists() && n.file == real(&at))
                .map(|n| n.file.clone())
                .or_else(|| {
                    only(
                        holders
                            .iter()
                            .copied()
                            .filter(|n| n.title == named)
                            .collect(),
                    )
                })
                .or_else(|| only(holders.clone()));
            let Some(file) = file else {
                unplaced.push(value.to_string());
                continue;
            };
            let branch = key
                .strip_prefix("branch.")
                .and_then(|b| b.strip_suffix(".wtnitem"))
                .unwrap_or(key);
            let Ok(id) = ensure_id(&file, dir) else {
                unplaced.push(value.to_string());
                continue;
            };
            let _ = worktree::git_ok(
                root,
                &[
                    "config",
                    &format!("branch.{branch}.wtnItem"),
                    &format!("{id}: {text}"),
                ],
            );
        }
        let _ = worktree::git_ok(root, &["config", MIGRATED, "true"]);
        unplaced
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sb-note-id-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn set_mtime(path: &Path, secs: u64) {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(at)
            .unwrap();
    }

    #[test]
    fn an_id_line_is_read_last_first_and_kept_last_with_nothing_else_moved() {
        let id = new();
        assert!(
            Regex::new(r"^n-[0-9a-f]{12}$").unwrap().is_match(&id),
            "{id}"
        );
        assert_ne!(new(), new());
        let text = format!("# T\n\nwords\n\n{}  \n", line("n-0123456789ab"));
        assert_eq!(in_text(&text).as_deref(), Some("n-0123456789ab"));
        assert_eq!(strip(&text), "# T\n\nwords\n\n");
        assert_eq!(in_text("# T\n<!-- note-id: n-XYZ -->\n"), None);
        let id = "n-0123456789ab";
        assert_eq!(
            with("# T\n\nwords\n", id),
            "# T\n\nwords\n\n<!-- note-id: n-0123456789ab -->\n"
        );
        assert_eq!(
            with("# T\n\nwords\n\n<!-- note-id: n-bbbbbbbbbbbb -->\n", id),
            "# T\n\nwords\n\n<!-- note-id: n-0123456789ab -->\n",
            "another ID's place taken"
        );
        assert_eq!(
            file_of(
                &["# T", "", "one", "<!-- note-id: n-bbbbbbbbbbbb -->", "two"],
                id
            ),
            "# T\n\none\ntwo\n\n<!-- note-id: n-0123456789ab -->\n",
            "another ID dropped, a line typed below moved above it"
        );
        assert_eq!(
            file_of(
                &[
                    "# T",
                    "- [ ] <!-- note-id: n-0123456789ab -->",
                    "**<!-- note-id: n-0123456789ab -->** kept"
                ],
                id
            ),
            "# T\n**** kept\n\n<!-- note-id: n-0123456789ab -->\n",
            "formatting wrapped round it loses the ID, and a line with nothing else left goes"
        );
        // What the app edits reads back as it was written, a blank last
        // line and all.
        for lines in [
            vec!["# T", "words"],
            vec!["# T", ""],
            vec![""],
            vec!["a", "", ""],
        ] {
            let file = file_of(&lines, id);
            assert_eq!(lines_of(&body_of(&file)), lines, "{file:?}");
            assert_eq!(in_text(&file).as_deref(), Some(id));
        }
        assert_eq!(
            body_of("# T\n\n<!-- note-id: n-0123456789ab -->\n- [ ] added below\n"),
            "# T\n\n- [ ] added below\n",
            "lines added below it kept"
        );
        assert_eq!(title_of("\n<!-- note-id: n-0123456789ab -->\n"), None);
        assert_eq!(title_of("## Plans\n").as_deref(), Some("Plans"));
        assert_eq!(
            claim_of("n-0123456789ab: Add search"),
            Some(("n-0123456789ab", "Add search"))
        );
        assert_eq!(claim_of("Work.md: Add search"), None);
    }

    #[test]
    fn a_note_is_given_an_id_once_its_time_kept_and_a_copy_gets_one_of_its_own() {
        let dir = temp("ensure");
        let note = dir.join("Work.md");
        fs::write(&note, "# Work\n\n- [ ] one\n").unwrap();
        set_mtime(&note, 1_767_225_600);
        let id = ensure_id(&note, &dir).unwrap();
        assert_eq!(
            fs::read_to_string(&note).unwrap(),
            format!("# Work\n\n- [ ] one\n\n{}\n", line(&id))
        );
        assert_eq!(
            fs::metadata(&note).unwrap().modified().unwrap(),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_767_225_600),
            "its modified time kept"
        );
        assert_eq!(ensure_id(&note, &dir).unwrap(), id, "given once");
        assert_eq!(of_file(&note).as_deref(), Some(id.as_str()));

        // A copy, changed later, holds the same ID until it gets its own.
        fs::create_dir_all(dir.join("Projects")).unwrap();
        let copy = dir.join("Projects/Work copy.md");
        fs::copy(&note, &copy).unwrap();
        set_mtime(&copy, 1_767_225_700);
        assert_eq!(find_by_id(&id, &dir).as_deref(), Some(note.as_path()));
        assert_eq!(copy_of(&copy, &id, &dir).as_deref(), Some(note.as_path()));
        let own = ensure_id(&copy, &dir).unwrap();
        assert_ne!(own, id);
        assert_eq!(
            ensure_id(&note, &dir).unwrap(),
            id,
            "the original keeps its own"
        );

        // Lines appended below the ID, more than its last bytes.
        let mut text = fs::read_to_string(&note).unwrap();
        text.push_str(&"- [ ] appended below the ID line by a script\n".repeat(10));
        fs::write(&note, &text).unwrap();
        assert_eq!(of_file(&note).as_deref(), Some(id.as_str()), "read whole");
        fs::rename(&note, dir.join("Projects/Renamed.md")).unwrap();
        assert_eq!(
            find_by_id(&id, &dir),
            Some(dir.join("Projects/Renamed.md")),
            "found wherever it went"
        );
        fs::create_dir_all(dir.join(".trash")).unwrap();
        fs::rename(
            dir.join("Projects/Renamed.md"),
            dir.join(".trash/Renamed.md"),
        )
        .unwrap();
        assert_eq!(find_by_id(&id, &dir), None, "never in Recently Deleted");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn several_giving_one_note_its_id_at_once_all_take_the_one_it_keeps() {
        let dir = temp("race");
        let note = dir.join("Race.md");
        fs::write(&note, "# Race\n\n- [ ] a\n").unwrap();
        let ids: Vec<String> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..6)
                .map(|_| s.spawn(|| ensure_id(&note, &dir).unwrap()))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let kept = of_file(&note).unwrap();
        assert!(ids.iter().all(|id| *id == kept), "{ids:?} {kept}");
        let _ = fs::remove_dir_all(&dir);
    }
}
