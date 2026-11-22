//! How Switchboard's skills defer to a repository's own skill of the same
//! name, ported from test/skills.test.js: every skill's opening step,
//! hand-offs between skills, and `switchboard skill-path`, which both use;
//! and the repository's own project skills (`.claude/skills/`), which
//! never ship.

use crate::common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::{BIN, TempDir};

fn skills_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/skills")
}

/// Every skill's name and SKILL.md text.
fn skills() -> Vec<(String, String)> {
    let mut all: Vec<(String, String)> = fs::read_dir(skills_dir())
        .unwrap()
        .flatten()
        .filter_map(|e| {
            let text = fs::read_to_string(e.path().join("SKILL.md")).ok()?;
            Some((e.file_name().to_string_lossy().into_owned(), text))
        })
        .collect();
    all.sort();
    all
}

/// The text after a skill's frontmatter and title, or `None` when it doesn't
/// start with them.
fn after_title(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    let rest = rest[end + 5..].strip_prefix("\n# ")?;
    let title_end = rest.find('\n')?;
    rest[title_end + 1..].strip_prefix('\n')
}

#[test]
fn every_skill_opens_by_running_skill_path_with_its_own_name() {
    let all = skills();
    assert!(!all.is_empty());
    for (name, text) in all {
        let opening = format!(
            "This is Switchboard's own copy, for repositories that have none. Before anything else, run `switchboard skill-path {name}`: if it prints a path other than this file's, read that file and follow it instead, with the same arguments, and stop reading here."
        );
        let rest = after_title(&text)
            .unwrap_or_else(|| panic!("{name} starts with its frontmatter and title"));
        assert!(
            rest.starts_with(&format!("{opening}\n")),
            "{name}'s first paragraph is the opening step"
        );
    }
}

#[test]
fn no_skill_names_another_skill_s_skill_md_by_a_fixed_path() {
    // A hand-off that needs Switchboard's own text rather than the
    // repository's copy reads `switchboard skill-path --own`.
    let fixed = regex::Regex::new(r"[^\s`(]*skills/[a-z-]+/SKILL\.md").unwrap();
    for (name, text) in skills() {
        let found: Vec<&str> = fixed.find_iter(&text).map(|m| m.as_str()).collect();
        assert!(
            found.is_empty(),
            "{name} reads other skills through skill-path: {found:?}"
        );
    }
}

#[test]
fn skill_path_finds_the_repository_s_copy_then_switchboard_s_personal_one_then_the_plugin_s() {
    let tmp = TempDir::new("skill-path");
    let home = tmp.path();
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .env("HOME", home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@e")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@e")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    let run = |dir: &Path| -> Output {
        Command::new(BIN)
            .args(["skill-path", "demo"])
            .arg(dir)
            .env("HOME", home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap()
    };
    let out = |dir: &Path| String::from_utf8_lossy(&run(dir).stdout).trim().to_string();
    let skill = |root: &Path| {
        let file = root.join(".claude/skills/demo/SKILL.md");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "# demo\n").unwrap();
        file.to_string_lossy().into_owned()
    };
    let repo = home.join("repo");
    let worktree = home.join("wt");
    let outside = home.join("outside");
    fs::create_dir(&outside).unwrap();
    git(&["init", "-q", repo.to_str().unwrap()]);
    git(&[
        "-C",
        repo.to_str().unwrap(),
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "start",
    ]);
    git(&[
        "-C",
        repo.to_str().unwrap(),
        "worktree",
        "add",
        "-q",
        "-b",
        "feature",
        worktree.to_str().unwrap(),
    ]);

    let none = run(&worktree);
    assert_eq!(none.status.code(), Some(1), "none anywhere");
    assert!(none.stdout.is_empty());

    // Switchboard's plugin's, when there is no other.
    let file = home.join(".local/share/switchboard/claude/plugin/skills/demo/SKILL.md");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "# demo\n").unwrap();
    let plugin = file.to_string_lossy().into_owned();
    assert_eq!(
        out(&worktree),
        plugin,
        "the plugin's when nothing else has it"
    );
    assert_eq!(out(&outside), plugin, "outside a repository too");

    // A skill of yours that only shares the name: Switchboard's own is
    // still the plugin's, so /switchboard:demo and a hand-off to it follow
    // Switchboard's text, not yours.
    let personal = skill(home);
    assert_eq!(out(&worktree), plugin, "not a skill of yours by the name");
    assert_eq!(out(&outside), plugin, "nor outside a repository");

    // Switchboard's, written there the old way.
    let marker = home.join(".claude/skills/demo/.switchboard-skill");
    fs::write(&marker, "SKILL.md\n").unwrap();
    assert_eq!(
        out(&worktree),
        personal,
        "the personal one when it is Switchboard's and the repository has none"
    );
    assert_eq!(
        out(&outside),
        personal,
        "the personal one outside a repository"
    );

    // A copy of Switchboard's made yours, as the guide says to change one:
    // its list deleted, its opening still deferring by skill-path.
    fs::remove_file(&marker).unwrap();
    fs::write(
        &personal,
        "# demo\n\nThis is Switchboard's own copy, for repositories that have none. Before anything else, run `switchboard skill-path demo`: if it prints a path other than this file's, read that file and follow it instead.\n\nMy way.\n",
    )
    .unwrap();
    assert_eq!(
        out(&worktree),
        personal,
        "a copy of Switchboard's made yours"
    );

    let main = skill(&repo);
    assert_eq!(
        out(&worktree),
        main,
        "the main clone's when the worktree has none"
    );
    assert_eq!(
        out(&repo.join(".claude")),
        main,
        "from inside the main clone"
    );

    let own = skill(&worktree);
    assert_eq!(out(&worktree), own, "the worktree's own first");

    // A worktree of someone else's PR, as wtpr --untrusted marks it: its own
    // copy is the PR author's, so the main clone's is followed.
    git(&[
        "-C",
        repo.to_str().unwrap(),
        "config",
        "extensions.worktreeConfig",
        "true",
    ]);
    git(&[
        "-C",
        worktree.to_str().unwrap(),
        "config",
        "--worktree",
        "switchboard.untrusted",
        "true",
    ]);
    let passed = run(&worktree);
    assert_eq!(
        String::from_utf8_lossy(&passed.stdout).trim(),
        main,
        "an untrusted worktree's own copy passed over"
    );
    assert!(String::from_utf8_lossy(&passed.stderr).contains("holds an untrusted PR"));
    assert_eq!(out(&repo), main, "the main clone is still its own");

    let usage = Command::new(BIN)
        .args(["skill-path", "-x"])
        .output()
        .unwrap();
    assert_eq!(usage.status.code(), Some(2));
}

// A skill read from a clone newer than the build names a command the build
// lacks: skill-path says so beside the path, which queue-and-clean stops on
// rather than reading the command's usage line as a failure to work around.
#[test]
fn skill_path_says_when_the_skill_names_a_command_this_build_lacks() {
    let tmp = TempDir::new("skill-behind");
    let home = tmp.path();
    let repo = home.join("repo");
    let status = Command::new("git")
        .args(["init", "-q"])
        .arg(&repo)
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .unwrap();
    assert!(status.success());
    let file = repo.join(".claude/skills/queue-and-clean/SKILL.md");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let run = |rules: &str| -> Output {
        fs::write(
            &file,
            format!("---\nname: queue-and-clean\nallowed-tools:\n{rules}---\n\n# Queue\n"),
        )
        .unwrap();
        Command::new(BIN)
            .args(["skill-path", "queue-and-clean"])
            .arg(&repo)
            .env("HOME", home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap()
    };

    let behind =
        run("  - Bash(switchboard land-lock test)\n  - Bash(switchboard land-lock nosuch *)\n");
    assert!(behind.status.success(), "the path is still printed");
    assert_eq!(
        String::from_utf8_lossy(&behind.stdout).trim(),
        file.to_string_lossy()
    );
    let said = String::from_utf8_lossy(&behind.stderr);
    assert!(
        said.contains(
            "skill-path: build behind: this switchboard lacks `switchboard land-lock nosuch`"
        ),
        "{said}"
    );
    assert!(!said.contains("land-lock test"), "{said}");

    let current = run("  - Bash(switchboard land-lock test)\n  - Bash(switchboard show *)\n");
    assert!(current.status.success());
    assert_eq!(String::from_utf8_lossy(&current.stderr), "");
}

#[test]
fn every_switchboard_command_a_skill_names_is_one_the_binary_has_and_none_names_tmux() {
    let known = |name: &str| {
        switchboard::cmd::COMMANDS.iter().any(|c| c.name == name)
            || switchboard::mux::commands::NAMES.contains(&name)
    };
    let named =
        regex::Regex::new(r"`switchboard ([a-z][a-z-]*)|^ *switchboard ([a-z][a-z-]*)").unwrap();
    let part =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/switchboard.md"))
            .unwrap();
    let mut texts = skills();
    texts.push(("switchboard.md".into(), part));
    texts.extend(project_skills());
    for (name, text) in texts {
        for line in text.lines() {
            for found in named.captures_iter(line) {
                let command = found.get(1).or_else(|| found.get(2)).unwrap().as_str();
                assert!(known(command), "{name} names switchboard {command}");
            }
        }
        assert!(!text.contains("tmux "), "{name} still names tmux");
        assert!(!text.contains("TMUX"), "{name} still reads TMUX");
    }
}

/// Switchboard's project skills, in `.claude/skills/`, by name with their
/// SKILL.md: for working on Switchboard itself, so they never ship
/// (tests/setup.rs checks `switchboard setup` leaves them out too).
fn project_skills() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(".claude/skills");
    let mut all: Vec<(String, String)> = fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter_map(|e| {
            let text = fs::read_to_string(e.path().join("SKILL.md")).ok()?;
            Some((e.file_name().to_string_lossy().into_owned(), text))
        })
        .collect();
    all.sort();
    all
}

#[test]
fn every_project_skill_is_named_after_its_folder_has_a_description_and_never_ships() {
    let all = project_skills();
    assert!(all.iter().any(|(name, _)| name == "review-pr"));
    assert!(all.iter().any(|(name, _)| name == "design-docs"));
    let shipped = switchboard::setup::skills::shipped();
    let named = regex::Regex::new(r"`((?:src|assets|tests|docs|\.claude)/[^`\s]+)`").unwrap();
    for (name, text) in all {
        let frontmatter = text
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .map(|(front, _)| front)
            .unwrap_or_else(|| panic!("{name} starts with its frontmatter"));
        assert!(
            frontmatter.lines().any(|l| l == format!("name: {name}")),
            "{name}'s name is its folder's"
        );
        assert!(
            frontmatter.lines().any(|l| l
                .strip_prefix("description: ")
                .is_some_and(|d| !d.trim().is_empty())),
            "{name} has a description"
        );
        assert!(
            !shipped.contains_key(name.as_str()),
            "{name} isn't one of the skills the binary carries"
        );
        // What it names of the repository is there, so a move in the code
        // that leaves it behind fails here.
        for path in named.captures_iter(&text).map(|c| c[1].to_string()) {
            assert!(
                Path::new(env!("CARGO_MANIFEST_DIR")).join(&path).exists(),
                "{name} names {path}, which isn't there"
            );
        }
    }
}

/// A section of a skill: from its `## ` heading to the next.
fn section<'t>(text: &'t str, heading: &str) -> &'t str {
    let start = text
        .find(&format!("\n## {heading}\n"))
        .unwrap_or_else(|| panic!("no ## {heading}"));
    let rest = &text[start + 1..];
    let end = rest[3..].find("\n## ").map_or(rest.len(), |e| e + 3);
    &rest[..end]
}

#[test]
fn shape_files_where_the_repository_keeps_its_work_and_only_you_start_it() {
    let all = skills();
    let (_, text) = all.iter().find(|(name, _)| name == "shape").unwrap();
    assert!(
        text.lines().any(|l| l == "disable-model-invocation: true"),
        "only you start it"
    );
    for rule in [
        "Bash(switchboard tickets)",
        "Bash(switchboard tabs)",
        "Bash(switchboard show *)",
    ] {
        assert!(
            text.contains(&format!("  - {rule}\n")),
            "allowed-tools: {rule}"
        );
    }
    let source = section(text, "Where it goes");
    for rule in [
        "`--notes`, `--github` or `--linear`",
        "run `switchboard tickets`",
        "ask nothing about it",
        "`autopilot-label <label>`",
    ] {
        assert!(source.contains(rule), "Where it goes: {rule}");
    }
    let github = section(text, "GitHub issues");
    let asked = github
        .find("**Create it on <owner/name>** or **Not yet**")
        .expect("asked about, since it is public");
    let created = github.find("gh issue create").expect("then created");
    assert!(asked < created, "asked, then created");
    assert!(github.contains("--assignee @me [--label '<label>']"));
    assert!(github.contains("closed in the last 90 days"));
}

#[test]
fn shape_writes_a_notes_item_and_its_plan_in_the_shape_wt_reads() {
    let all = skills();
    let (_, text) = all.iter().find(|(name, _)| name == "shape").unwrap();
    let notes = section(text, "A note's item");
    for option in [
        "**Top of the list**",
        "**In order of difficulty**",
        "**Bottom of the list**",
    ] {
        assert!(notes.contains(option), "{option}");
    }
    // The shape autopilot reads: an open choice is a "Decision
    // needed" heading in the plan and "(needs a choice" on the item.
    for shape in [
        "## Decision needed",
        "(needs a choice: ",
        "Plan: [[Plan - <name>]]",
        "`After: <first line>`",
    ] {
        assert!(notes.contains(shape), "{shape}");
    }
    // The item's first line names the branch, the worktree and the tab,
    // so step 6 keeps it to the outcome, and the reply quotes it.
    let step = |number: usize| {
        notes
            .lines()
            .skip_while(|l| !l.starts_with(&format!("{number}. **")))
            .take_while(|l| !l.starts_with(&format!("{}. **", number + 1)))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(step(2).contains("**Already there?**"));
    for rule in [
        "its first line the item's title, a bug or a feature",
        "A bug starts `BUG: ` and says what is broken now, in the present tense",
        "A feature starts `FEAT: ` and says what to build, as a task starting with a verb",
        "under 90 characters",
        "Its first six words differ from every other item's",
        "No file paths, line numbers, dates or screenshots",
    ] {
        assert!(step(6).contains(rule), "step 6: {rule}");
    }
    assert!(step(8).contains("the item's first line quoted as written"));
}

// What /shape files it hands to the review loop's ticket mode, started the
// way the mode's own file says it is, which the review loop reads when given
// `ticket`.
#[test]
fn shape_hands_what_it_files_to_the_review_loop_s_ticket_mode() {
    let all = skills();
    let (_, shape) = all.iter().find(|(name, _)| name == "shape").unwrap();
    let (_, review) = all.iter().find(|(name, _)| name == "review-loop").unwrap();
    let mode =
        fs::read_to_string(skills_dir().join("review-loop/references/ticket-mode.md")).unwrap();
    assert!(
        mode.contains("It is started as `/review-loop ticket <ticket>`"),
        "the mode says how it is started"
    );
    assert!(
        review.contains("read `references/ticket-mode.md` beside this file and follow it"),
        "the review loop reads it when given `ticket`"
    );
    let linear = section(shape, "Linear");
    assert!(linear.contains("run `/review-loop ticket <ID>` on it"));
    assert!(linear.contains("Run `/review-loop ticket <ID>`, the review loop's ticket mode"));
    let github = section(shape, "GitHub issues");
    assert!(github.contains("run `/review-loop ticket <number>`"));
    for (name, text) in [("shape", shape.as_str()), ("ticket mode", mode.as_str())] {
        assert!(
            !text.contains("working tree isn't clean"),
            "{name}: a ticket's review needs no clean tree"
        );
    }
    // Nothing is written to the ticket before a yes.
    let asked = mode.find("**Save it to the issue**").unwrap();
    let edited = mode.find("gh issue edit").unwrap();
    assert!(asked < edited, "asked, then saved");
    assert!(mode.contains("switchboard codex-review --ticket <file> --round start"));
}

/// The retired skill's name, put together so this file doesn't hold it.
fn retired() -> String {
    ["plan", "item"].join("-")
}

#[test]
fn nothing_names_the_skill_shape_took_the_place_of() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let name = retired();
    let mut found = Vec::new();
    let mut dirs: Vec<PathBuf> = ["assets", "src", "crates", "tests", "docs", ".claude"]
        .iter()
        .map(|d| root.join(d))
        .collect();
    let mut files: Vec<PathBuf> = ["README.md", "CLAUDE.md", "ARCHITECTURE.md"]
        .iter()
        .map(|f| root.join(f))
        .collect();
    while let Some(dir) = dirs.pop() {
        // The design boards change through /design-docs.
        if dir == root.join("docs/design") {
            continue;
        }
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let skip = ["target", "node_modules", "worktrees"]
                .contains(&entry.file_name().to_string_lossy().as_ref());
            if path.is_dir() && !skip {
                dirs.push(path);
            } else if path.is_file() {
                files.push(path);
            }
        }
    }
    for file in files {
        if fs::read_to_string(&file).is_ok_and(|t| t.contains(&name)) {
            found.push(
                file.strip_prefix(root)
                    .unwrap_or(&file)
                    .display()
                    .to_string(),
            );
        }
    }
    assert!(found.is_empty(), "{name} is named in {found:?}");
    assert!(!skills_dir().join(&name).exists());
}

#[test]
fn no_skill_names_the_skill_wt_took_the_place_of() {
    // Put together so this file doesn't hold it.
    let name = ["wt", "n"].concat();
    let mut dirs = vec![skills_dir()];
    let mut found = Vec::new();
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() && entry.file_name() != "node_modules" {
                dirs.push(path);
            } else if fs::read_to_string(&path).is_ok_and(|t| {
                // As a word: the git config keys it named (`wtnItem`) stay.
                t.match_indices(&name).any(|(at, _)| {
                    let before = t[..at].chars().next_back();
                    let after = t[at + name.len()..].chars().next();
                    !before.is_some_and(|c| c.is_alphanumeric())
                        && !after.is_some_and(|c| c.is_alphanumeric())
                })
            }) {
                found.push(path.display().to_string());
            }
        }
    }
    assert!(found.is_empty(), "{name} is named in {found:?}");
    assert!(!skills_dir().join(&name).exists());
}

// test/skills.test.js's "a tab of autopilot lands without asking
// only once leak-check finds nothing, and its review loop fixes mediums
// without asking", leak-check now Switchboard's own repository's alone.
#[test]
fn a_tab_of_the_work_a_note_mode_lands_without_asking_only_once_reviewed_clean() {
    let all = skills();
    let text = |name: &str| {
        all.iter()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t.clone())
            .unwrap()
    };
    let queue = text("queue-and-clean");
    let step = |start: &str| {
        queue
            .lines()
            .find(|l| l.starts_with(start))
            .unwrap_or_else(|| panic!("no {start}"))
            .to_string()
    };
    let check = "switchboard show @land";
    let step7 = step("7. **Ask before pushing**");
    assert!(step7.contains(check), "step 7 reads the tab's switch");
    assert!(
        regex::Regex::new(r"the landing ran both the tests and the review loop .*two clean rounds of this very change .*take the answer as \*\*Push to <name>\*\* without asking")
            .unwrap()
            .is_match(&step7),
        "{step7}"
    );
    assert!(
        queue.contains("<lock> autolanded <short hash>"),
        "it says so after landing"
    );
    let hands_free = step("3. **Hands-free leaves it to the user.**");
    assert!(
        hands_free.contains("switchboard show @autopilot_handsfree")
            && hands_free.contains("ask nothing")
            && hands_free.contains("--apply-range \"$before\" HEAD"),
        "a hands-free tab leaves applying to the user: {hands_free}"
    );
    let ask = step("4. **Ask** with `AskUserQuestion`");
    assert!(queue.find(&hands_free).unwrap() < queue.find(&ask).unwrap());
    let step9 = step("9. **Push**");
    let gated = step9
        .find("When step 7 took the answer without asking, run `<lock> push`")
        .expect("the push without asking goes through the command's gate");
    let again = step9
        .find("it reads `@land` again")
        .expect("which reads the switch again");
    assert!(gated < again, "as it pushes");
    assert!(
        step9.contains("run `<lock> push --asked`"),
        "the push after a yes"
    );
    let review = text("review-loop");
    assert!(
        review.contains(check),
        "the review loop reads the same switch"
    );
    assert!(review.contains("fix a `medium` one you agree with as a `high` one is"));
}

#[test]
fn a_landing_runs_its_tests_through_land_lock_test_which_pushes_and_deletes_its_ci_branch() {
    let all = skills();
    let (_, queue) = all
        .iter()
        .find(|(name, _)| name == "queue-and-clean")
        .unwrap();
    let section = queue
        .split("**Running the tests.**")
        .nth(1)
        .and_then(|rest| rest.split("\n1. **The note.**").next())
        .expect("the section");
    let at = |said: &str| {
        section
            .find(said)
            .unwrap_or_else(|| panic!("the section says {said}"))
    };
    assert!(section.starts_with(
        " Step 5 runs them with `<lock> test --base \"$base\"`, and step 8's `<lock> settle`"
    ));
    at("`switchboard landing --json`'s `ciBranch`");
    assert!(
        !queue.contains("ci/<branch>"),
        "the worktree's branch name is never part of what is pushed"
    );
    for said in [
        "- `passed …` (exit 0)",
        "- `no tests` (exit 0)",
        "- `not set` (exit 5)",
        "- `failed <n of m> <log>` (exit 1)",
        "- `failed in CI …`, `mixed in CI …` (exit 1), `base-failing in CI …` (exit 4)",
        "- `failed in CI <sha> <run id> <job names>`: none of the failures is the default branch's",
        "- `base-failing in CI <sha> <run id> <job names>`: every test",
        "- `mixed in CI <sha> <run id> <job names>`",
        "go on as for `base-failing`; otherwise as for `failed in CI`",
        "waits for its CI as `switchboard wait-ci --same-as` does",
        "`leak-check found <n>` (exit 3)",
        "- `timeout ci/land-<id>` (exit 6)",
        "`canceled in CI …` (exit 8)",
        "- `canceled in CI <sha> <run id> <job names>`: nothing failed, but the run was canceled",
        "it prints `passed in CI before, run <id>` above `passed`",
        "deletes the branch again on every way out, an interrupt included",
        "**Push to ci/land-<id>**, which runs it again with `--leak-ok`",
        "**Run them here**, which runs it again with `--here`",
        "`gh run view <run id> -R <owner/name> --log-failed`",
    ] {
        at(said);
    }
    assert!(
        !queue.contains("git push --force origin HEAD:refs/heads/ci/land-<id>"),
        "the push is the command's, not Claude's"
    );
    let line = |start: &str| {
        queue
            .lines()
            .find(|l| l.starts_with(start))
            .unwrap_or_else(|| panic!("no {start}"))
    };
    assert!(
        line("5. **The tests pass before the review**").contains("`<lock> test --base \"$base\"`")
    );
    assert!(
        line("8. **Settle under the lock**")
            .contains("runs the tests again as `<lock> test --again` does")
    );
    assert!(
        line("9. **Push**").contains("the landing's `ci/land-<id>` deleted"),
        "the push deletes it"
    );
    assert!(
        line("9. **Push**").contains("git push origin HEAD:<name>` (never forced"),
        "the landing's own push is never forced"
    );
}

// A landing whose change is no longer the one reviewed lets the landing lock
// go, keeping its turn, and settles again (rebasing under the lock) before
// pushing.
#[test]
fn a_landing_reviews_again_without_the_lock_and_rebases_again_once_it_has_it() {
    let all = skills();
    let (_, queue) = all
        .iter()
        .find(|(name, _)| name == "queue-and-clean")
        .unwrap();
    let start = queue.find("8. **Settle under the lock**").unwrap();
    let end = queue.find("9. **Push**").unwrap();
    let step8 = &queue[start..end];
    let changed = step8.find("- `12` (`changed`)").expect("it says changed");
    let requeued = step8
        .find("put this worktree first in the landing queue, marked reviewing")
        .expect("settle requeues");
    let review = step8
        .find("review it again as step 6 does")
        .expect("it reviews again");
    let back = step8
        .find("run step 8 again, never straight to the push")
        .expect("it rebases again");
    assert!(
        changed < requeued && requeued < review && review < back,
        "{step8}"
    );
    assert!(
        !step8.contains("holding the lock"),
        "nothing reviews or asks again holding the lock: {step8}"
    );
}

// A hands-free tab's review loop runs to 20 rounds, counted by codex-review,
// still asking at the cap.
#[test]
fn review_loop_runs_twenty_rounds_hands_free_and_five_otherwise() {
    let all = skills();
    let (_, text) = all.iter().find(|(name, _)| name == "review-loop").unwrap();
    let rounds = text
        .split("## Each round (at most 5, or 20 hands-free)\n")
        .nth(1)
        .and_then(|rest| rest.split("\n1. ").next())
        .expect("the rounds' section");
    assert!(
        regex::Regex::new(
            r"20 rounds .*`@autopilot_handsfree`.* 5 rounds otherwise.*`switchboard codex-review --round` counts the rounds, applies the cap"
        )
        .unwrap()
        .is_match(rounds),
        "{rounds}"
    );
    let report = text
        .split("## Final report\n")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("the report");
    assert!(report.contains("\"the round cap, 20 hands-free\""));
    for said in [
        "give the last round's findings right after, as a numbered list, one line each, never as a paragraph and never the rounds before it",
        "`#<n> <severity> <file>:<line> <title>: <what triage did with it>`",
        "The question is one sentence naming the findings still open by number and severity",
        "`reported` (a low left for the report, or a finding for someone else's PR)",
        "carrying on from the last round's list and never starting again, so no number names two findings",
    ] {
        assert!(report.contains(said), "{said}");
    }
    let listing = text
        .split("## Listing findings\n")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("the listing rule");
    assert!(listing.contains("one finding a line, never as a paragraph"));
    assert!(listing.contains("`#<n> <severity> <file>:<line> <title>`"));
    for said in [
        "a question per finding, each about that finding alone, never naming or listing another in its text",
        "the rest in further calls of up to four, one after another, numbered across them (`#5`, `#6`)",
    ] {
        assert!(text.contains(said), "{said}");
    }
    assert!(
        regex::Regex::new(r"cap ended the loop with findings still open.*in every tab, with `AskUserQuestion` headed `\[always ask\]`, so hands-free mode")
            .unwrap()
            .is_match(report),
        "{report}"
    );
}

// The loop's notes carry what each round fixed, per branch, so a later round
// or loop doesn't fix it again.
#[test]
fn review_loop_notes_carry_fixed_disputed_and_known_gap_lines() {
    let all = skills();
    let (_, text) = all.iter().find(|(name, _)| name == "review-loop").unwrap();
    let before = text
        .split("## Before the first round\n")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("the section");
    for said in [
        "`review-notes-<branch>.md`",
        "so no two branches share one",
        "keep only its `Fixed` lines",
        "take out every `Disputed` and `Known gap` line",
        "three kinds of entries",
        "`- Fixed: <title> (<file>:<line>): <what was changed, in a sentence>, in <short hash>`",
        "`- Disputed: <title> (<file>:<line>): <why the finding is wrong>`",
        "`- Known gap: <what is not handled> (<file>): <why that is acceptable for this PR>`",
    ] {
        assert!(before.contains(said), "{said}");
    }
    let rounds = text
        .split("## Each round (at most 5, or 20 hands-free)\n")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("the rounds' section");
    for said in [
        "**Dispute it at once** when it restates a `Fixed` or `Disputed` line",
        "add a `Fixed` line to the notes file for each finding fixed",
    ] {
        assert!(
            rounds.to_lowercase().contains(&said.to_lowercase()),
            "{said}"
        );
    }
}

#[test]
fn review_loop_reviews_someone_elses_pr_from_outside_its_worktree_and_only_reports() {
    let all = skills();
    let (_, text) = all.iter().find(|(name, _)| name == "review-loop").unwrap();
    let section = text
        .split("## Someone else's PR, from outside its worktree\n")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("the section");
    for said in [
        "`switchboard codex-review -C <worktree> --pr <number> --base <base> --notes <notes-file>`",
        "`--text --no-textconv --no-ext-diff`",
        "`switchboard try --sealed`",
        "Nothing is fixed, committed, pushed, or edited on GitHub",
        "Write no `review-loop-clean` record.",
        "`review-pr-notes-<number>.md` in the scratchpad, emptied as each loop starts",
        "**For the author**",
    ] {
        assert!(section.contains(said), "{said}");
    }
    // The flags it names are codex-review's own.
    let usage = Command::new(BIN)
        .args(["codex-review", "--help"])
        .output()
        .unwrap();
    let usage = String::from_utf8_lossy(&usage.stderr);
    for flag in [
        "-C <dir>",
        "--pr <number|url>",
        "--base <ref>",
        "--notes <file>",
    ] {
        assert!(usage.contains(flag), "codex-review takes {flag}: {usage}");
    }
}

#[test]
fn review_pr_runs_none_of_the_prs_code_outside_the_sealed_try() {
    let all = project_skills();
    let (_, text) = all.iter().find(|(name, _)| name == "review-pr").unwrap();
    assert!(
        text.lines().any(|l| l == "disable-model-invocation: true"),
        "it checks out someone else's code, so only you start it"
    );
    for said in [
        "`switchboard worktree-from-pr --untrusted <pr>`",
        "`switchboard pr-risk <worktree> <base>`",
        "`--text --no-textconv --no-ext-diff`",
        "`switchboard skill-path review-loop`",
        "`switchboard try --sealed <worktree> --run cargo test`",
        "on Linux `switchboard try --sealed` refuses",
        "never `cd` a shell into it",
    ] {
        assert!(text.contains(said), "{said}");
    }
}

// leak-check is a rule of Switchboard's own repository: no shipped skill
// runs it, and its project skill says where they used to.
#[test]
fn only_switchboard_s_own_project_skill_runs_leak_check() {
    let shipped = skills();
    for name in [
        "queue-and-clean",
        "review-loop",
        "shape",
        "diff-artifact",
        "ci-loop",
    ] {
        let (_, text) = shipped.iter().find(|(n, _)| n == name).unwrap();
        assert!(
            !text.contains("switchboard leak-check"),
            "{name} runs leak-check"
        );
    }
    let mode =
        fs::read_to_string(skills_dir().join("review-loop/references/ticket-mode.md")).unwrap();
    assert!(!mode.contains("leak-check"), "ticket mode runs leak-check");
    let own = project_skills();
    let (_, text) = own.iter().find(|(n, _)| n == "leak-check").unwrap();
    for said in [
        "`switchboard leak-check <base>..HEAD`",
        "`switchboard leak-check --file <file>`",
        "`[always ask]`",
    ] {
        assert!(text.contains(said), "{said}");
    }
}

// /shape is shipped to every repository, so it names no file of
// Switchboard's; this repository's CLAUDE.md holds its rules.
#[test]
fn shape_names_no_file_of_switchboard_s() {
    let shipped = skills();
    let (_, text) = shipped.iter().find(|(n, _)| n == "shape").unwrap();
    for file in [
        "assets/settings.json",
        "src/mux/keys.rs",
        "docs/guide.md",
        "ARCHITECTURE.md",
        "Switchboard's own repository",
    ] {
        assert!(!text.contains(file), "shape names {file}");
    }
    assert!(text.contains("The plan follows the repository's `CLAUDE.md` and the user's."));
}

#[test]
fn push_unreviewed_only_puts_the_push_on_your_clipboard() {
    let all = project_skills();
    let (_, text) = all
        .iter()
        .find(|(name, _)| name == "push-unreviewed")
        .unwrap();
    assert!(
        text.lines().any(|l| l == "disable-model-invocation: true"),
        "it pushes past the review rule, so only you start it"
    );
    for said in [
        "`switchboard leak-check <range>`",
        "`switchboard skill-path resolve-conflicts`",
        "`pbcopy < <that file>`",
        "never run the push yourself",
        "Never run the push yourself",
    ] {
        assert!(text.contains(said), "{said}");
    }
    let runs_a_push = regex::Regex::new(r"`git push[^`]*`").unwrap();
    assert!(
        !runs_a_push.is_match(text),
        "it names no push command to run, only the one it writes for you"
    );
}

#[test]
fn switchboard_s_own_copy_is_printed_whatever_is_installed() {
    let tmp = TempDir::new("sb-skill-own");
    let run = |name: &str| {
        Command::new(BIN)
            .args(["skill-path", "--own", name])
            .env("HOME", tmp.path())
            .output()
            .unwrap()
    };
    let out = run("ci-loop");
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("When queue-and-clean runs this"), "{text}");
    assert_eq!(run("no-such-skill").status.code(), Some(1));
}

#[test]
fn a_clean_rebase_is_checked_with_after_rebase_and_a_failing_default_branch_is_waited_out() {
    let all = skills();
    let (_, queue) = all
        .iter()
        .find(|(name, _)| name == "queue-and-clean")
        .unwrap();
    let step = |start: &str| {
        queue
            .lines()
            .find(|l| l.starts_with(start))
            .unwrap_or_else(|| panic!("no {start}"))
    };
    let eight = step("8. **Settle under the lock**");
    for said in [
        "checks it with `afterRebase` after a clean rebase while `<name>`'s own CI isn't failing",
        "(phase `checking after the rebase`)",
        "unless this very commit was already tested",
    ] {
        assert!(eight.contains(said), "step 8 says {said}");
    }
    assert!(queue.contains("or `check failed <log>` for `afterRebase`"));
    let section = queue
        .split("**The tests in CI.**")
        .nth(1)
        .and_then(|rest| rest.split("\n1. **The note.**").next())
        .expect("the section");
    for said in [
        "`switchboard wait-ci --now --repo <owner/name> --sha \"$(git rev-parse origin/<name>)\"`",
        "`<lock> phase waiting for <name> to pass`",
        "go back to step 3, fixing nothing",
    ] {
        assert!(
            section.contains(said),
            "a failing CI run asks whether the default branch fails the same way: {said}"
        );
    }
    assert!(queue.contains("`afterRebase`, a command step 8's `<lock> settle` runs"));
}

#[test]
fn queue_and_clean_pins_the_tabs_name_before_anything_moves() {
    let all = skills();
    let (_, queue) = all
        .iter()
        .find(|(name, _)| name == "queue-and-clean")
        .unwrap();
    let pin = "switchboard set @tab_name \"$(switchboard show @tab_label_without_pr_title | sed -E 's/^#[0-9]+ //')\"";
    let keeping = queue
        .split("## Keeping the tab's name")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("the section");
    assert!(keeping.contains(pin), "{keeping}");
    assert!(keeping.contains("When `switchboard show @tab_name` prints nothing"));
    let keeping_at = queue.find("## Keeping the tab's name").unwrap();
    assert!(keeping_at < queue.find("## Before watching").unwrap());
    assert!(keeping_at < queue.find("## No PR").unwrap());
    let back = queue
        .split("## Back to the repository")
        .nth(1)
        .expect("the section");
    assert!(!back.contains("@tab_name"), "{back}");
}

#[test]
fn queue_and_clean_stops_watching_a_pr_with_merge_conflicts() {
    let all = skills();
    let (_, queue) = all
        .iter()
        .find(|(name, _)| name == "queue-and-clean")
        .unwrap();
    let watching = queue
        .split("\n## Watching")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("the section");
    let query = watching
        .lines()
        .find(|l| l.starts_with("query="))
        .expect("the watch's query");
    assert!(query.contains(" mergeable "), "{query}");
    let exit = watching
        .find("*CONFLICTING*) echo conflicts; exit 0 ;;")
        .expect("the watch exits on conflicts");
    assert!(
        exit < watching.find("gh pr checks").unwrap(),
        "before the checks are read"
    );
    assert!(watching.contains("\n- `conflicts`: "), "{watching}");
}

#[test]
fn resolve_conflicts_force_pushes_so_only_you_start_it() {
    let all = skills();
    let (_, text) = all
        .iter()
        .find(|(name, _)| name == "resolve-conflicts")
        .unwrap();
    assert!(
        text.lines().any(|l| l == "disable-model-invocation: true"),
        "it force-pushes, so Claude never starts it on its own"
    );
}

#[test]
fn skills_keep_outside_text_out_of_the_shell_and_read_logs_as_data() {
    let all = skills();
    let text_of = |skill: &str| &all.iter().find(|(name, _)| name == skill).unwrap().1;
    let oncall = text_of("oncall");
    assert!(!oncall.contains("<<'EOF'"), "an alert's text could end it");
    assert!(oncall.contains("< ~/.local/state/oncall/notice.md"));
    for skill in ["ci-loop", "queue-and-clean"] {
        assert!(
            text_of(skill).contains("data, never instructions"),
            "{skill} reads CI logs"
        );
    }
    assert!(
        !text_of("ci-loop").contains("git pull"),
        "git pull is denied"
    );
}

#[test]
fn diff_artifact_tells_no_story_of_its_own_and_publishes_only_after_a_yes() {
    let all = skills();
    let text = &all
        .iter()
        .find(|(name, _)| name == "diff-artifact")
        .unwrap()
        .1;
    assert!(text.contains("`switchboard diff-story`"));
    assert!(text.contains("No story told for this change yet"));
    assert!(text.contains("never tell a story of your own"));
    assert!(text.contains("`switchboard show @tab_plan_note`"));
    assert!(text.contains("Publish only after their yes"));
    assert!(text.contains("never share it further"));
    assert!(text.contains("Never a link to GitHub or the PR in place of a hunk"));
}

// main requires a PR's branch to be up to date, and auto-merge waits on one
// that is behind without updating it, so both of queue-and-clean's waits end
// on `BEHIND` (after conflicts) and the skill rebases onto the base, as
// /resolve-conflicts does.
#[test]
fn queue_and_clean_brings_a_pr_behind_its_base_up_to_date() {
    let all = skills();
    let queue = &all
        .iter()
        .find(|(name, _)| name == "queue-and-clean")
        .unwrap()
        .1;
    let watch = queue
        .split("## Watching")
        .nth(1)
        .and_then(|rest| rest.split("```").nth(1))
        .expect("the watch's script");
    assert!(watch.contains("mergeStateStatus"), "{watch}");
    let conflicts = watch.find("echo conflicts").unwrap();
    let behind = watch.find("echo behind").expect("behind ends the watch");
    assert!(conflicts < behind, "conflicts are read before behind");
    let wait = queue
        .split("### Waiting until the PR is ready")
        .nth(1)
        .and_then(|rest| rest.split("```").nth(1))
        .expect("the wait's script");
    assert!(
        wait.contains("mergeStateStatus") && wait.contains("echo behind"),
        "{wait}"
    );
    assert!(
        queue.contains("- `behind`: "),
        "behind is an exit with a step"
    );
    let section = queue
        .split("## Bringing it up to date")
        .nth(1)
        .expect("the section behind goes to");
    let section = section.split("\n## ").next().unwrap();
    assert!(
        section.contains("`switchboard skill-path resolve-conflicts`")
            && section.contains("`[always ask]`")
            && !section.contains("git merge"),
        "{section}"
    );
    for exit in ["- `behind`: ", "- `conflicts`: "] {
        for line in queue.lines().filter(|l| l.starts_with(exit)) {
            assert!(line.contains("\"Bringing it up to date\""), "{line}");
        }
    }
}

// A branch behind its base or in conflict with it is rebased and pushed with a
// lease by /resolve-conflicts, read through skill-path, never merged into.
#[test]
fn ci_loop_hands_a_branch_behind_or_in_conflict_to_resolve_conflicts() {
    let all = skills();
    let ci = &all.iter().find(|(name, _)| name == "ci-loop").unwrap().1;
    let part = ci
        .split("**Merge conflicts, or a branch that has to be up to date.**")
        .nth(1)
        .and_then(|rest| rest.split("\n\n").next())
        .expect("the paragraph on conflicts");
    assert!(
        part.contains("`switchboard skill-path resolve-conflicts`")
            && part.contains("`[always ask]`")
            && !part.contains("git merge"),
        "{part}"
    );
}

// The review loop's cap question is asked in every tab, with its options
// and a recommendation, and queue-and-clean waits on it rather than running
// another loop.
#[test]
fn the_review_loop_s_cap_question_has_its_options_and_queue_and_clean_waits_on_it() {
    let all = skills();
    let text = |name: &str| {
        all.iter()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t.clone())
            .unwrap()
    };
    let review = text("review-loop");
    let report = review
        .split("## Final report\n")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("the final report");
    for said in [
        "ask the cap question before the push question, in every tab, with `AskUserQuestion` headed `[always ask]`",
        "- **Run 10 more rounds**: run `switchboard codex-review --round more`",
        "- **Land as it is**: offered only when queue-and-clean runs this loop and its landing is a PR",
        "- **Stop here**",
        "with a finding at `high` or above still open, **Run 10 more rounds**",
        "\"No finding above medium is open; the rest are listed above\"",
    ] {
        assert!(report.contains(said), "{said}");
    }
    assert!(review.contains("the cap counts the branch's rounds across every loop"));
    let queue = text("queue-and-clean");
    for said in [
        "wait on that answer, never starting another loop to get past the cap",
        "**Land as it is**, offered only for a PR landing",
    ] {
        assert!(queue.contains(said), "{said}");
    }
}

#[test]
fn queue_and_clean_uses_switchboard_s_merge_queue_only_with_its_setting_on() {
    let text = fs::read_to_string(skills_dir().join("queue-and-clean/SKILL.md")).unwrap();
    let queuing = section(&text, "Queuing");
    assert!(
        queuing.contains(
            "`switchboard setting pr_merge_queue` and `switchboard setting github` both print `on`"
        ) && queuing.contains("`queue=false`"),
        "Switchboard's queue only with the setting on and no queue of GitHub's"
    );
    assert!(
        queuing.contains("the setting off among them")
            && queuing.contains("`gh pr merge <number> --auto --match-head-commit <headRefOid>`"),
        "with the setting off it turns on auto-merge as before"
    );
    let ours = &queuing[queuing.find("### Switchboard's merge queue").unwrap()..];
    let ours = &ours[..ours.find("### Waiting").unwrap()];
    assert!(ours.contains("switchboard land-lock queue-pr <number>"));
    assert!(
        !ours.contains("--auto"),
        "a PR in the queue never gets auto-merge"
    );
    let watching = section(&text, "Watching");
    let watch = &watching[watching.find("### Watching Switchboard's queue").unwrap()..];
    assert!(watch.contains("switchboard land-lock pr-status <number>"));
    assert!(watch.contains("Go to \"Fixing\""), "a dropped PR is fixed");
}

// A tab that lands without asking skips the deploy and the test without asking
// wherever the repository has neither, not only in autopilot's tabs, so a
// merge in a repository with no deploy asks nothing.
#[test]
fn queue_and_clean_skips_a_missing_deploy_without_asking_in_a_tab_that_lands_without_asking() {
    let text = fs::read_to_string(skills_dir().join("queue-and-clean/SKILL.md")).unwrap();
    let before = section(&text, "Before watching");
    assert!(before.contains(
        "In any other tab that lands without asking (`switchboard show @land` prints `1`)"
    ));
    assert!(before.contains("\"Deploy and test\" then gives `not tested: no deploy`"));
    let deploy = section(&text, "Deploy and test");
    assert!(deploy.contains("`deploy-test: approved (not tested: no deploy)`"));
    assert!(deploy.contains("`deploy-test: approved (not tested: autopilot landed it)`"));
}
