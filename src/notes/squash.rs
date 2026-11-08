//! Squashing notes, ported from notes/lua/notes/squash.lua: two or more
//! marked in the list (s marks, S squashes) become one, merged by a
//! headless Claude, after you say yes, since it sends their text to Claude.
//! It is the one place a note's text leaves the computer. Sonnet merges:
//! careful enough not to drop an item, which Haiku would risk, at a fraction
//! of Opus's cost. Where two notes clash and nothing says which is right,
//! the one changed last wins. The new note goes in the first one's folder,
//! the originals go to Recently Deleted, so nothing is lost, and every tab
//! linked to one of them links to the new one. No note's ID (id.rs) is sent:
//! the merged note gets one of its own, which autopilot's note,
//! its skipped items and its tabs take in place of any squashed note's.
//!
//! `ask` runs Claude, off the screen's thread, and gives its answer;
//! `finish` makes the note from it, on the app's, which first lets go of
//! any of the notes it has open.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::id;
use super::store::{self, Retitled, Store};
use crate::util;

/// The model that merges notes squashed in the list.
pub const MODEL: &str = "sonnet";

pub const PROMPT: &str = "Merge the notes below into one Markdown note. \
Its first line is '# ' and a title that covers them all. \
Keep every item, every checklist box as it is ([ ] or [x]), every link and every detail, and group what belongs together under headings; \
drop only what is said twice, keeping the fuller one. Leave the ticked items of a checklist under its unticked ones, a blank line between. \
Each note says when it was last changed. Where two of them disagree (an item ticked in one and not the other, a different date, number or decision) and nothing in them says which is right, keep the version from the note changed most recently. \
Write nothing but the note: no preamble, no code fence around it.";

/// What Claude is given on its stdin: each note with when it last changed,
/// in UTC so no clock change puts two out of order.
pub fn input(store: &Store, paths: &[PathBuf]) -> Result<String, String> {
    let mut parts = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        // Without its ID line: the merged note gets an ID of its own.
        let text = id::strip(
            &util::read_text(path)
                .ok_or_else(|| format!("couldn't read {}", store.relative(path)))?,
        );
        let changed = store::mtime(path)
            .and_then(|s| jiff::Timestamp::from_second(s).ok())
            .map(|t| t.strftime("%Y-%m-%dT%H:%M:%SZ").to_string())
            .unwrap_or_else(|| "unknown".into());
        let n = i + 1;
        parts.push(format!(
            "<note {n} changed=\"{changed}\">\n{text}\n</note {n}>"
        ));
    }
    Ok(parts.join("\n\n"))
}

/// The command that merges them: `claude -p` with no tools and no session
/// kept, nor Switchboard's plugin, the prompt as its argument and the notes
/// on its stdin.
pub fn command(program: &Path, model: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.args([
        "-p",
        "--model",
        model,
        "--tools",
        "",
        "--no-session-persistence",
        PROMPT,
    ]);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::setup::plugin::headless(&mut cmd);
    cmd
}

/// Runs a started merge to its end with `input` on its stdin, giving the
/// merged note or why not.
pub fn answer(mut child: std::process::Child, input: &str) -> Result<String, String> {
    use std::io::Write;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let text = stdout.trim();
    let text = text
        .strip_prefix("```")
        .map(|t| t.split_once('\n').map_or("", |(_, rest)| rest))
        .unwrap_or(text);
    let text = text.strip_suffix("\n```").unwrap_or(text).to_string();
    let titled = text
        .strip_prefix("# ")
        .is_some_and(|rest| rest.chars().next().is_some_and(|c| !c.is_whitespace()));
    if !out.status.success() || !titled {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let first = stderr.trim().lines().next().unwrap_or("").to_string();
        return Err(format!(
            "Claude didn't merge them{}",
            if first.is_empty() {
                String::new()
            } else {
                format!(": {first}")
            }
        ));
    }
    Ok(text)
}

/// Makes the merged note from Claude's answer: in the first note's folder,
/// the images it names copied there from the folders of the notes they came
/// from, the originals to Recently Deleted, and the tabs linked to them,
/// and those named after them, following it. Gives its path.
pub fn finish(store: &mut Store, paths: &[PathBuf], text: &str) -> Result<PathBuf, String> {
    let rel = store.relative(&paths[0]);
    let folder = rel.rfind('/').map_or("", |i| &rel[..i]).to_string();
    let title = store::title_of(text);
    let path = store.create(&folder, Some(&title));
    let new_id = id::new();
    std::fs::write(&path, id::with(text, &new_id)).map_err(|_| "couldn't write the new note")?;
    let folders: Vec<&Path> = paths.iter().filter_map(|p| p.parent()).collect();
    if let Some(here) = path.parent() {
        store::copy_attachments(text, &folders, here);
    }
    let mut rels = Vec::new();
    let mut titles = Vec::new();
    let mut renumber = std::collections::HashMap::new();
    for old in paths {
        if let Some(old_id) = id::of_file(old) {
            renumber.insert(old_id, new_id.clone());
        }
        rels.push(store.relative(old));
        titles.push(store::title_of(&util::read_text(old).unwrap_or_default()));
        store.delete(old);
    }
    let to = store.relative(&path);
    for (i, from) in titles.iter().enumerate() {
        let retitled = Retitled {
            path: to.clone(),
            from: from.clone(),
            to: title.clone(),
        };
        if i == 0 {
            store.relink(
                &|rel| rels.iter().any(|r| r == rel).then(|| to.clone()),
                Some(&retitled),
            );
        } else {
            store.relink(&|_| None, Some(&retitled));
        }
    }
    store.renumber(&renumber);
    Ok(path)
}

/// The question before a squash: the notes, where their text goes and where
/// the originals go.
pub fn question(titles: &[String]) -> (String, Vec<String>) {
    let mut list: Vec<String> = titles.iter().map(|t| format!("  · {t}")).collect();
    list.push(String::new());
    list.push(format!(
        "Their text goes to Claude ({MODEL}) to merge; the originals go to Recently Deleted."
    ));
    (
        format!("Squash these {} notes into one?", titles.len()),
        list,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::store::tests::{set_mtime, temp, write};

    /// A stand-in `claude -p`, as strict as Claude Code about how it is
    /// called, that keeps what it was given and answers.
    fn stand_in(dir: &Path, answer: &str) -> (PathBuf, PathBuf) {
        use crate::stand_in::{Claude, sh_quote};
        let asked = dir.join("asked");
        let file = sh_quote(&asked.to_string_lossy());
        let program = Claude::new()
            .answer(&format!(
                "printf '%s\\n' \"$@\" > {file}\ncat >> {file}\nprintf '%s' {}",
                sh_quote(answer)
            ))
            .install(dir);
        (program, asked)
    }

    fn run(store: &Store, program: &Path, paths: &[PathBuf]) -> Result<String, String> {
        let input = input(store, paths)?;
        let child = command(program, MODEL).spawn().map_err(|e| e.to_string())?;
        answer(child, &input)
    }

    #[test]
    fn squashing_merges_notes_with_claude_into_a_new_note_the_originals_to_recently_deleted() {
        let (dir, mut s) = temp("squash");
        let bin = dir.path.join(".bin");
        std::fs::create_dir_all(&bin).unwrap();
        let (program, asked) = stand_in(
            &bin,
            "# Groceries and errands\n\n- [ ] milk\n- [ ] post office\n",
        );
        let a = s.create("", Some("Groceries"));
        write(
            &a,
            "# Groceries\n\n- [ ] milk\n\n<!-- note-id: n-aaaaaaaaaaaa -->\n",
        );
        let b = s.create("", Some("Errands"));
        write(&b, "# Errands\n\n- [ ] post office\n");
        set_mtime(&a, 1_767_225_600);
        let text = run(&s, &program, &[a.clone(), b.clone()]).unwrap();
        let made = finish(&mut s, &[a.clone(), b.clone()], &text).unwrap();
        assert_eq!(s.relative(&made), "Groceries and errands.md");
        let merged = std::fs::read_to_string(&made).unwrap();
        let new_id = id::in_text(&merged).expect("an ID of its own");
        assert_eq!(
            merged,
            format!(
                "# Groceries and errands\n\n- [ ] milk\n- [ ] post office\n\n{}\n",
                id::line(&new_id)
            )
        );
        assert_ne!(new_id, "n-aaaaaaaaaaaa", "the merged note's ID is its own");
        assert!(!a.exists() && !b.exists(), "the originals leave the notes");
        assert!(
            s.root().join(".trash/Groceries.md").exists()
                && s.root().join(".trash/Errands.md").exists(),
            "to Recently Deleted"
        );
        let given = std::fs::read_to_string(&asked).unwrap();
        assert!(
            given.starts_with("-p\n--model\nsonnet\n--tools\n\n--no-session-persistence\n"),
            "{given}"
        );
        assert!(regex::Regex::new(r#"<note 1 changed="2026-01-01T00:00:00Z">\n# Groceries[\s\S]*<note 2 changed="\d{4}-[^"]+">\n# Errands"#).unwrap().is_match(&given), "each note with when it was last changed");
        assert!(given.contains("keep the version from the note changed most recently"));
        assert!(!given.contains("note-id"), "no note's ID is sent");
    }

    // test/notes.test.js's "tabs linked to squashed notes link to the new
    // note, and the IDs autopilot keeps for any of them become
    // the new note's", the mode's file here; its tabs' @autopilot_note are
    // set through the server, as their @tab_note are.
    #[test]
    fn the_ids_the_work_a_note_mode_keeps_for_squashed_notes_become_the_merged_notes() {
        let (dir, mut s) = temp("squash-ids");
        let bin = dir.path.join(".bin");
        std::fs::create_dir_all(&bin).unwrap();
        let (program, _) = stand_in(&bin, "# Both\n\n- one\n- two\n");
        let a = s.create("", Some("One"));
        let b = s.create("", Some("Two"));
        let (one, two) = (id::of_file(&a).unwrap(), id::of_file(&b).unwrap());
        std::fs::create_dir_all(&s.state).unwrap();
        let state = s.state.join("autopilot.json");
        std::fs::write(
            &state,
            serde_json::json!({
                "on": true, "note": "Two.md", "noteId": two,
                "skipped": [{ "note": one, "text": "skipped" }, { "note": "n-ffffffffffff", "text": "elsewhere" }],
            })
            .to_string(),
        )
        .unwrap();
        let text = run(&s, &program, &[a.clone(), b.clone()]).unwrap();
        let made = finish(&mut s, &[a, b], &text).unwrap();
        let new_id = id::of_file(&made).unwrap();
        let mode = util::read_json(&state).unwrap();
        assert_eq!(mode["note"], "Both.md");
        assert_eq!(
            mode["noteId"],
            new_id.as_str(),
            "the mode's note by the merged note's ID"
        );
        assert_eq!(
            mode["skipped"][0]["note"],
            new_id.as_str(),
            "its skipped item with it"
        );
        assert_eq!(
            mode["skipped"][1]["note"], "n-ffffffffffff",
            "another note's left alone"
        );
    }

    #[test]
    fn squashing_notes_from_two_folders_copies_the_images_the_merged_note_names() {
        let (dir, mut s) = temp("squash-images");
        let bin = dir.path.join(".bin");
        std::fs::create_dir_all(&bin).unwrap();
        let (program, _) = stand_in(
            &bin,
            "# Both\n\n- [ ] one\n- [ ] two\n  ![shot](.attachments/Two/shot.png)\n",
        );
        let a = s.create("A", Some("One"));
        write(&a, "# One\n\n- [ ] one\n");
        let b = s.create("B", Some("Two"));
        write(
            &b,
            "# Two\n\n- [ ] two\n  ![shot](.attachments/Two/shot.png)\n",
        );
        let shots = b.parent().unwrap().join(".attachments/Two");
        std::fs::create_dir_all(&shots).unwrap();
        std::fs::write(shots.join("shot.png"), b"\x89PNG").unwrap();
        let text = run(&s, &program, &[a.clone(), b.clone()]).unwrap();
        let made = finish(&mut s, &[a, b], &text).unwrap();
        assert_eq!(s.relative(&made), "A/Both.md");
        assert!(
            s.root().join("A/.attachments/Two/shot.png").exists(),
            "the image copied into the merged note's folder"
        );
    }

    #[test]
    fn an_answer_that_isnt_a_note_squashes_nothing() {
        let (dir, s) = temp("squash-fails");
        let bin = dir.path.join(".bin");
        std::fs::create_dir_all(&bin).unwrap();
        let (program, _) = stand_in(&bin, "Sorry, I can't.");
        let a = dir.path.join("a.md");
        write(&a, "# A\n");
        assert!(
            run(&s, &program, &[a.clone(), a])
                .unwrap_err()
                .starts_with("Claude didn't merge them")
        );
        let (q, list) = question(&["First".into(), "Second".into()]);
        assert_eq!(q, "Squash these 2 notes into one?");
        assert!(
            list.iter()
                .any(|l| l.contains("Claude (sonnet)") && l.contains("Recently Deleted"))
        );
    }
}
