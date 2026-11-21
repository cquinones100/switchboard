//! The keys that start work (src/cmd/work.rs), pressed in a client of a
//! test server: M-t's directory list and the Claude tab it opens, M-W's
//! tickets each in a tab of its own running `wt`, M-B's branch made in a
//! worktree by `wtb`, and M-o's past sessions resumed. A stand-in `claude`
//! in the test home's `bin` says where it ran, and Linear and GitHub are an
//! address nothing answers on, so nothing leaves the machine; `wt` alone
//! asks in its own terminal, a pane's.

use crate::common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use common::TestServer;
use common::stand_in::Claude;
use serde_json::Value;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@e")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@e")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A server whose home has a repository with one commit and a stand-in
/// Claude, and whose current tab is in the repository.
fn world() -> (TestServer, PathBuf, common::TestClient) {
    let server = TestServer::start_with_env(&[
        ("LINEAR_API_KEY", "lin_test"),
        ("SWITCHBOARD_LINEAR_API", "http://127.0.0.1:9"),
        ("SWITCHBOARD_GITHUB_API", "http://127.0.0.1:9"),
        ("SWITCHBOARD_GITHUB_GRAPHQL", "http://127.0.0.1:9"),
    ]);
    Claude::new()
        .answer("echo \"$PWD $*\" >> \"$HOME/claude-ran\"")
        .install(&server.home.join("bin"));
    let repo = server.home.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    let repo = repo.canonicalize().unwrap();
    let mut client = server.attach(120, 40);
    client.wait_text("$");
    server.ok(&["new-tab", "-c", repo.to_str().unwrap()]);
    (server, repo, client)
}

fn tabs(server: &TestServer) -> Vec<Value> {
    server.list()["sessions"][0]["tabs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn wait(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = common::deadline(std::time::Duration::from_secs(15));
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn m_t_lists_the_directory_youre_in_first_and_opens_claude_in_the_one_picked() {
    let (server, repo, mut client) = world();
    let before = tabs(&server).len();
    client.send(b"\x1bt");
    let screen = client.wait_text("Open in >");
    let first = screen
        .iter()
        .position(|l| l.contains("│ › ~/repo") || l.contains("│   ~/repo"))
        .expect("the repository is listed");
    assert!(
        screen[first].contains("› ~/repo") && screen[first + 1].contains("   ~ "),
        "the directory you are in first, chosen: {screen:#?}"
    );
    client.send(b"\r");
    wait("Claude to run in the new tab", || {
        std::fs::read_to_string(server.home.join("claude-ran"))
            .is_ok_and(|t| t.starts_with(repo.to_str().unwrap()))
    });
    let all = tabs(&server);
    assert_eq!(all.len(), before + 1);
    assert_eq!(all.last().unwrap()["current"], true, "at the end, in front");
    // Then what to call it: s, shaping a ticket, asks which, since the
    // repository's branch names none.
    client.wait_text("Name the tab");
    client.type_text("s");
    client.wait_text("Its ID, like ABC-123");
    client.type_text("abc-7\r");
    wait("the tab named", || {
        tabs(&server).last().unwrap()["options"]["@tab_name"] == "Shaping: ABC-7"
    });
}

#[test]
fn m_shift_w_opens_a_tab_for_each_ticket_typed_running_wt_and_refuses_anything_else() {
    let (server, _repo, mut client) = world();
    let before = tabs(&server).len();
    client.send(b"\x1bW");
    client.wait_text("Open in >");
    client.send(b"\r");
    client.wait_text("Tickets:");
    client.type_text("abc-1, ABC-2 abc-1");
    client.send(b"\r");
    wait("a tab for each ticket", || {
        tabs(&server).len() == before + 2
    });
    let all = tabs(&server);
    let opened: Vec<String> = all[before..]
        .iter()
        .map(|t| t["panes"][0]["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        all[before]["current"], true,
        "the first brought to the front"
    );
    // wt ran in each, defined from worktree-from-ticket's init, and asked
    // Linear, which isn't there.
    for pane in &opened {
        wait("wt to fail on Linear", || {
            server.ok(&["capture-pane", "-t", pane]).contains("Error:")
        });
    }

    client.send(b"\x1bW");
    client.wait_text("Open in >");
    client.send(b"\r");
    client.wait_text("Tickets:");
    client.type_text("ABC-3 $(touch x)");
    client.send(b"\r");
    client.wait_text("Not a ticket ID or Linear issue URL: $(touch");
    assert_eq!(tabs(&server).len(), before + 2, "no tab half-started");
}

#[test]
fn m_shift_b_makes_a_branch_no_branch_matches_in_a_worktree_and_starts_claude_there() {
    let (server, repo, mut client) = world();
    client.send(b"\x1bB");
    client.wait_text("Open in >");
    client.send(b"\r");
    client.wait_text("branch>");
    client.type_text("fix-the-login");
    client.send(b"\r");
    let worktree = repo.join(".claude/worktrees/fix-the-login");
    wait("Claude in the branch's worktree", || {
        std::fs::read_to_string(server.home.join("claude-ran"))
            .is_ok_and(|t| t.contains(worktree.to_str().unwrap()))
    });
}

#[test]
fn m_o_lists_a_past_session_and_resumes_it_in_a_new_tab() {
    let (server, repo, mut client) = world();
    // A Claude session of today in the repository that no tab runs.
    let encoded: String = repo
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let project = server.home.join(".claude/projects").join(encoded);
    std::fs::create_dir_all(&project).unwrap();
    let line = serde_json::json!({
        "type": "user",
        "cwd": repo,
        "timestamp": jiff::Timestamp::now().to_string(),
        "message": { "content": "make the lonely thing work again" },
    });
    std::fs::write(project.join("sess-1.jsonl"), line.to_string()).unwrap();
    // M-o opens mission control's Reopen view as a picker, a row each.
    client.send(b"\x1bo");
    client.wait_for("the session's row", |s| {
        s.iter()
            .any(|l| l.contains("make the lonely thing work again"))
    });
    // Enter opens its menu, resuming it first.
    client.send(b"\r");
    client.wait_text("Resume it in a new tab");
    client.send(b"\r");
    wait("Claude resumed in a new tab", || {
        std::fs::read_to_string(server.home.join("claude-ran"))
            .is_ok_and(|t| t.contains(&format!("{} --resume sess-1", repo.display())))
    });
}

#[test]
fn a_key_of_a_service_turned_off_shows_its_tip_once_and_does_nothing() {
    let (server, _repo, mut client) = world();
    common::write_settings(&server.home, serde_json::json!({"github": false}));
    // The server reads the file again on its next tick.
    std::thread::sleep(Duration::from_millis(2500) * common::patience());
    client.send(b"\x1bv");
    client.wait_text("GitHub is off in Settings.");
    // Any key closes the tip and goes on; the key again does nothing at all.
    client.send(b"\x1bv");
    client.wait_for("the tip to close", |s| {
        !s.iter().any(|l| l.contains("GitHub is off"))
    });
    std::thread::sleep(Duration::from_millis(500));
    let screen = client.screen();
    assert!(
        !screen.iter().any(|l| l.contains("GitHub is off")),
        "{screen:#?}"
    );
}

#[test]
fn tab_marks_a_few_of_a_note_s_items_and_each_marked_gets_a_tab() {
    let (server, _repo, mut client) = world();
    let notes = server.home.join("Notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("Plan.md"),
        "# Plan\n\n- [ ] Fix the login\n- [ ] Speed up the build\n- [ ] Write the docs\n",
    )
    .unwrap();
    let before = tabs(&server).len();
    server.ok(&["press", "work-on-note"]);
    client.wait_text("Open in >");
    client.send(b"\r");
    client.wait_text("note>");
    client.send(b"\r");
    client.wait_text("Every item listed (3)");
    client.wait_text("tab mark");
    // Down past "Every item listed", then the first and third marked.
    client.send(b"\x1b[B");
    client.send(b"\t");
    client.send(b"\x1b[B");
    client.send(b"\t");
    client.wait_text("(2 marked)");
    client.send(b"\r");
    // Where they land: the default branch, first, as Enter takes it.
    client.wait_text("Land on");
    client.send(b"\r");
    wait("a tab for each item marked", || {
        tabs(&server).len() == before + 2
    });
    let names: Vec<String> = tabs(&server)[before..]
        .iter()
        .map(|t| t["options"]["@tab_name"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(names, ["Fix the login", "Write the docs"]);
    let titles: Vec<String> = tabs(&server)[before..]
        .iter()
        .map(|t| {
            t["options"]["@tab_title"]
                .as_str()
                .unwrap_or("")
                .to_string()
        })
        .collect();
    assert_eq!(
        titles,
        ["Fix the login", "Write the docs"],
        "each item's whole first line kept as its tab's full title"
    );
}

#[test]
fn land_on_lists_the_branch_picked_last_time_first() {
    let (server, repo, mut client) = world();
    lands(&server, &repo, "direct");
    git(&repo, &["branch", "team/dev"]);
    git(&repo, &["branch", "a-first-by-name"]);
    let notes = server.home.join("Notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("Plan.md"),
        "# Plan\n\n- [ ] Fix the login\n- [ ] Write the docs\n",
    )
    .unwrap();
    let before = tabs(&server).len();
    server.ok(&["press", "work-on-note"]);
    client.wait_text("Open in >");
    client.send(b"\r");
    client.wait_text("note>");
    client.send(b"\r");
    client.wait_text("Every item listed (2)");
    // The first item alone.
    client.send(b"\x1b[B");
    client.send(b"\r");
    let screen = client.wait_text("Land on");
    assert!(
        screen
            .iter()
            .any(|l| l.contains("› main (the default branch)")),
        "nothing picked yet, the default branch first: {screen:#?}"
    );
    client.type_text("team/dev");
    client.wait_text("branch> team/dev");
    client.send(b"\r");
    wait("the item's tab", || tabs(&server).len() == before + 1);
    wait("the pick kept", || {
        std::fs::read_to_string(server.state.join("work-recent.json"))
            .is_ok_and(|t| t.contains("team/dev"))
    });

    server.ok(&["press", "work-on-note"]);
    client.wait_text("Open in >");
    client.send(b"\r");
    client.wait_text("note>");
    client.send(b"\r");
    client.wait_text("item");
    client.send(b"\r");
    let screen = client.wait_text("Land on");
    assert!(
        screen.iter().any(|l| l.contains("› team/dev")),
        "the branch picked last time first: {screen:#?}"
    );
    client.send(b"\x1b");
}

/// Sets how the repository at `repo` lands in the test's repos.json.
fn lands(server: &TestServer, repo: &Path, way: &str) {
    let config = server.home.join(".config/switchboard");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("repos.json"),
        serde_json::json!({ repo.join(".git").to_str().unwrap(): { "landing": way } }).to_string(),
    )
    .unwrap();
}

/// Waits for the tab just opened to be in front, its row `row` in the
/// sidebar, and for the server to read its pane's directory as `dir`,
/// which the palette reads as it opens.
#[cfg(feature = "net")]
fn in_front_at(server: &TestServer, client: &mut common::TestClient, row: &str, dir: &Path) {
    client.wait_for("the tab in front", |s| s.iter().any(|l| l.contains(row)));
    let dir = dir.canonicalize().unwrap();
    wait("the pane's directory", || {
        tabs(server)
            .iter()
            .filter(|t| t["current"] == true)
            .flat_map(|t| t["panes"].as_array().cloned().unwrap_or_default())
            .any(|p| {
                p["cwd"]
                    .as_str()
                    .and_then(|c| Path::new(c).canonicalize().ok())
                    .is_some_and(|c| c == dir)
            })
    });
}

/// The palette's "Review a contributor's PR": listed only in a repository
/// whose main clone has a review-pr project skill, it lists the open PRs
/// from forks (GitHub a stand-in, as strict as the real one), checks the
/// one picked out untrusted, and opens a tab in the main clone running
/// Claude with `/review-pr <number>`, the tab about the PR's worktree: its
/// PR on its label, the worktree its pane's directory, lazygit refused.
#[cfg(feature = "net")]
#[test]
fn reviewing_a_contributors_pr_lists_forks_prs_and_starts_review_pr_in_the_main_clone() {
    use common::http::{Fake, Reply};
    use serde_json::json;

    let github = Fake::github("test-token", |request| {
        let details = request.path() == "/graphql"
            && request
                .text()
                .contains("isCrossRepository maintainerCanModify");
        if details {
            return Some(Reply::json(
                200,
                json!({"data": {"repository": {"pullRequest": {
                    "number": 7, "title": "Add a theme", "body": "", "state": "OPEN",
                    "url": "https://github.com/acme/widgets/pull/7",
                    "author": {"login": "someone", "createdAt": "2015-01-01T00:00:00Z"},
                    "baseRefName": "main", "headRefName": "topic",
                    "headRepository": {"name": "widgets"}, "headRepositoryOwner": {"login": "someone"},
                    "isCrossRepository": true, "maintainerCanModify": false,
                    "changedFiles": 1, "additions": 1, "deletions": 0
                }}}}),
            ));
        }
        // An older draft, not in the list, asked about by its number.
        let older_draft = request.path() == "/graphql"
            && request.text().contains("pullRequest(number:")
            && request.text().contains("\"number\":11");
        if older_draft {
            return Some(Reply::json(
                200,
                json!({"data": {"repository": {"pullRequest": {
                    "number": 11, "url": "https://github.com/acme/widgets/pull/11",
                    "title": "Old work", "state": "OPEN", "isDraft": true,
                    "baseRefName": "main", "headRefName": "old"
                }}}}),
            ));
        }
        (request.path() == "/graphql" && request.text().contains("states: OPEN")).then(|| {
            let pr = |number: u64, title: &str, cross: bool, author: &str| {
                json!({
                    "number": number, "url": format!("https://github.com/acme/widgets/pull/{number}"),
                    "title": title, "state": "OPEN", "isDraft": number == 9,
                    "baseRefName": "main", "headRefName": "topic",
                    "isCrossRepository": cross, "author": {"login": author}
                })
            };
            Reply::json(
                200,
                json!({"data": {"repository": {"pullRequests": {"nodes": [
                    pr(9, "Half a feature", true, "another"),
                    // Its author wrote the title: a control character in
                    // it shows nowhere, the picker or the tab's name.
                    pr(7, "Add a\u{7} theme", true, "someone"),
                    pr(8, "Our own branch", false, "acme"),
                ]}}}}),
            )
        })
    });
    let server = TestServer::start_with_env(&[
        ("LINEAR_API_KEY", "lin_test"),
        ("SWITCHBOARD_LINEAR_API", "http://127.0.0.1:9"),
        ("GH_TOKEN", "test-token"),
        ("SWITCHBOARD_GITHUB_GRAPHQL", &github.graphql_url()),
        ("SWITCHBOARD_GITHUB_API", &github.url),
    ]);
    Claude::new()
        .answer("echo \"$PWD $*\" >> \"$HOME/claude-ran\"")
        .install(&server.home.join("bin"));
    let repo = server.home.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/widgets.git",
        ],
    );
    // GitHub's repository is a bare one here, reached through insteadOf,
    // keeping the fork's commit under refs/pull/7/head as GitHub does; the
    // repository named as gh repo set-default names it, since insteadOf
    // rewrites what origin's URL reads as.
    git(
        &repo,
        &["config", "remote.origin.gh-resolved", "acme/widgets"],
    );
    let origin = server.home.join("widgets.git");
    git(
        &server.home,
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    std::fs::write(
        server.home.join(".gitconfig"),
        format!(
            "[url \"{}\"]\n\tinsteadOf = https://github.com/acme/widgets.git\n",
            origin.display()
        ),
    )
    .unwrap();
    git(&repo, &["push", "-q", origin.to_str().unwrap(), "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "a theme"]);
    git(
        &repo,
        &[
            "push",
            "-q",
            origin.to_str().unwrap(),
            "HEAD:refs/pull/7/head",
        ],
    );
    git(&repo, &["reset", "-q", "--hard", "HEAD~1"]);
    let repo = repo.canonicalize().unwrap();
    let mut client = server.attach(120, 40);
    client.wait_text("$");
    server.ok(&["new-tab", "-c", repo.to_str().unwrap()]);

    // With no skill, it isn't listed.
    in_front_at(&server, &mut client, "▌○ 2 repo", &repo);
    client.type_text("\x1bp");
    client.wait_text("╭─ Actions");
    client.type_text("a PR in a new tab");
    let screen = client.wait_text("Start work on a PR");
    assert!(
        !screen.iter().any(|l| l.contains("Review a contributor")),
        "{screen:#?}"
    );
    client.type_text("\x1b");
    client.wait_for("the palette to close", |s| {
        !s.iter().any(|l| l.contains("╭─ Actions"))
    });

    // In a worktree of a repository whose main clone has it, it is.
    std::fs::create_dir_all(repo.join(".claude/skills/review-pr")).unwrap();
    std::fs::write(repo.join(".claude/skills/review-pr/SKILL.md"), "x").unwrap();
    let worktree = repo.join(".claude/worktrees/w");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "w",
            worktree.to_str().unwrap(),
        ],
    );
    server.ok(&["new-tab", "-c", worktree.to_str().unwrap()]);
    in_front_at(&server, &mut client, "▌○ 3 w", &worktree);
    client.type_text("\x1bp");
    client.wait_text("╭─ Actions");
    client.type_text("a PR in a new tab");
    client.wait_text("› Review a contributor's PR");
    client.type_text("\x1b");
    client.wait_for("the palette to close", |s| {
        !s.iter().any(|l| l.contains("╭─ Actions"))
    });

    let before = tabs(&server).len();
    let w_tab = tabs(&server)
        .into_iter()
        .find(|t| t["current"] == true)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let w_dir = worktree.clone();
    server.ok(&["press", "review-pr"]);
    let screen = client.wait_text("#7 Add a theme");
    assert!(screen.iter().any(|l| l.contains("@someone")), "{screen:#?}");
    assert!(
        !screen.iter().any(|l| l.contains("Our own branch")),
        "only PRs from forks: {screen:#?}"
    );
    // A draft is listed after the ready PRs, marked, and Enter on it
    // takes nothing.
    let row = |text: &str| screen.iter().position(|l| l.contains(text));
    assert!(
        row("#9 Half a feature · draft") > row("#7 Add a theme"),
        "{screen:#?}"
    );
    client.type_text("\x1b[B");
    client.type_text("\r");
    client.type_text("\x1b[A");
    client.type_text("\r");
    // The workspace's own background `claude -p` calls (its connectors)
    // write to the same file, so the tab's is one line among them.
    let ran = format!("{} /review-pr 7", repo.display());
    wait("Claude to run in the main clone", || {
        std::fs::read_to_string(server.home.join("claude-ran"))
            .is_ok_and(|t| t.lines().any(|l| l == ran))
    });
    let all = tabs(&server);
    assert_eq!(all.len(), before + 1);
    let tab = all.last().unwrap();
    assert_eq!(tab["options"]["@tab_name"], "Review PR Add a theme");
    assert_eq!(tab["current"], true, "in front, as a tab you asked for");

    // The tab is about the PR's worktree, checked out untrusted.
    let worktree = server
        .home
        .join(".cache/switchboard/pr-review/acme-widgets/7")
        .canonicalize()
        .unwrap();
    assert_eq!(
        tab["options"]["@tab_dir"],
        worktree.to_string_lossy().as_ref()
    );
    let id = tab["id"].as_str().unwrap().to_string();
    wait("the tab to carry its PR", || {
        tabs(&server)
            .iter()
            .any(|t| t["id"] == id.as_str() && t["label"] == "#7 Review PR Add a theme")
    });
    let tab = tabs(&server)
        .into_iter()
        .find(|t| t["id"] == id.as_str())
        .unwrap();
    assert_eq!(tab["panes"][0]["cwd"], worktree.to_string_lossy().as_ref());

    // lazygit, which runs the repository's own commands, isn't opened there.
    let bin = server.home.join("bin/lazygit");
    std::fs::write(&bin, "#!/bin/sh\nsleep 30\n").unwrap();
    std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    server.ok(&["select-tab", "-t", &id]);
    client.wait_for("the review tab in front", |s| {
        s.iter().any(|l| l.contains("#7 Review PR Add a theme"))
    });
    server.ok(&["press", "lazygit"]);
    client.wait_text("so lazygit isn't opened there");

    // A draft's number typed opens nothing.
    server.ok(&["select-tab", "-t", &w_tab]);
    in_front_at(&server, &mut client, "3 w", &w_dir);
    let before = tabs(&server).len();
    server.ok(&["press", "review-pr"]);
    client.wait_text("#9 Half a feature · draft");
    client.type_text("9");
    client.type_text("\x1b\r");
    client.wait_text("#9 is a draft: it can be reviewed once it is ready");
    assert_eq!(tabs(&server).len(), before);

    // So does one the list doesn't hold, asked about by its number.
    client.type_text("\r");
    client.wait_for("the message to close", |s| {
        !s.iter().any(|l| l.contains("#9 is a draft"))
    });
    server.ok(&["press", "review-pr"]);
    client.wait_text("#9 Half a feature · draft");
    client.type_text("11");
    client.type_text("\x1b\r");
    client.wait_text("#11 is a draft: it can be reviewed once it is ready");
    assert_eq!(tabs(&server).len(), before);
}

/// With UX 2.0, Notes' `gw` opens the "Work on this item" box (board 36):
/// the item with its plan's chip, the repository, how it lands and what
/// Start opens; while autopilot works the note, its "Next in autopilot's
/// queue" puts the item first in the mode's `next`; Enter on "▶ Start it"
/// opens the item's tab; and with a tab on the item, the box offers to go
/// there.
#[test]
fn gw_opens_the_work_on_this_item_box() {
    let (server, repo, mut client) = world();
    common::write_settings(&server.home, serde_json::json!({ "tips": false }));
    let notes = server.home.join("Notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("Plan.md"),
        "# Plan\n\n- [ ] Fix the login\n- [ ] Speed up the build\n",
    )
    .unwrap();
    std::fs::write(
        server.state.join("autopilot.json"),
        serde_json::json!({
            "on": true, "paused": true, "note": "Plan.md",
            "repo": repo.to_str().unwrap(), "slots": 1, "cap": 1,
            "skipped": ["Speed up the build"],
        })
        .to_string(),
    )
    .unwrap();
    // The repository keeps its work in notes, so autopilot works the note.
    let config = server.home.join(".config/switchboard");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("repos.json"),
        serde_json::json!({ repo.join(".git").to_str().unwrap(): { "tickets": "notes" } })
            .to_string(),
    )
    .unwrap();
    let before = tabs(&server).len();

    // Autopilot read and the setting taken in, the box offers the queue;
    // before then, the chain of boxes it replaces, put away.
    let deadline = common::deadline(Duration::from_secs(15));
    loop {
        server.ok(&["press", "work-on-item", "Plan.md", "4"]);
        let shown = client.wait_for("a box for the item", |s| {
            s.iter()
                .any(|l| l.contains("Work on this item") || l.contains("the repository"))
        });
        if shown
            .iter()
            .any(|l| l.contains("Next in autopilot's queue"))
        {
            break;
        }
        client.send(b"\x1b");
        client.wait_for("the box put away", |s| {
            !s.iter()
                .any(|l| l.contains("Work on this item") || l.contains("the repository"))
        });
        assert!(Instant::now() < deadline, "no queue row: {shown:#?}");
        std::thread::sleep(Duration::from_millis(300) * common::patience());
    }
    let shown = client.wait_text("▶ Start it");
    let text = shown.join("\n");
    assert!(text.contains("☐ Speed up the build"), "{text}");
    assert!(text.contains("no plan"), "{text}");
    assert!(text.contains("REPOSITORY"), "{text}");
    assert!(text.contains("~/repo"), "{text}");
    assert!(text.contains("LANDS ON"), "{text}");
    assert!(text.contains("note-speed-up-the-build"), "{text}");

    // ↓ to the queue's row, Enter: the item first in the mode's next.
    client.send(b"\x1b[B");
    client.send(b"\r");
    client.wait_text("Next in autopilot's queue: Speed up the build");
    let mode: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(server.state.join("autopilot.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(mode["next"][0]["text"], "Speed up the build", "{mode:#}");
    assert_eq!(
        mode["skipped"],
        serde_json::json!([]),
        "put next, taken back: {mode:#}"
    );
    client.send(b"\r");

    // Enter on "▶ Start it": the item's tab.
    server.ok(&["press", "work-on-item", "Plan.md", "4"]);
    client.wait_text("▶ Start it");
    client.send(b"\r");
    wait("the item's tab", || tabs(&server).len() == before + 1);
    let tab = &tabs(&server)[before];
    assert_eq!(tab["options"]["@tab_name"], "Speed up the build");
    assert_eq!(tab["options"]["@tab_note"], "Plan.md");

    // Pressed again, the box says a tab has it and offers to go there.
    server.ok(&["press", "work-on-item", "Plan.md", "4"]);
    client.wait_text("A tab works on it already");
    client.wait_text("Go to it");
    client.send(b"\x1b");
    assert_eq!(tabs(&server).len(), before + 1);
}

/// The palette's "Start work on a GitHub issue": the repository, then its
/// open issues from GitHub (a stand-in, as strict as the real one), the
/// ones assigned to you first, each group the newest first; the one picked,
/// or a number typed, gets a tab running `wt --issue` on it.
#[cfg(feature = "net")]
#[test]
fn working_on_a_github_issue_lists_its_open_issues_yours_first_and_opens_the_picked_ones_tab() {
    use common::http::{Fake, Reply};
    use serde_json::json;

    let github = Fake::github("test-token", |request| {
        (request.path() == "/graphql" && request.text().contains("issues(states: OPEN")).then(
            || {
                let issue = |number: u64, title: &str, assignee: Option<&str>| {
                    json!({ "number": number, "title": title,
                            "assignees": { "nodes": assignee.map(|a| vec![json!({ "login": a })]).unwrap_or_default() } })
                };
                Reply::json(
                    200,
                    json!({ "data": { "viewer": { "login": "me" }, "repository": { "issues": { "nodes": [
                        issue(14, "Newest of all", None),
                        // Its author wrote the title: a control character
                        // in it shows nowhere.
                        issue(13, "Mine\u{7} to do", Some("me")),
                        issue(12, "Someone else's", Some("else")),
                    ] } } } }),
                )
            },
        )
    });
    let server = TestServer::start_with_env(&[
        ("GH_TOKEN", "test-token"),
        ("SWITCHBOARD_GITHUB_GRAPHQL", &github.graphql_url()),
        ("SWITCHBOARD_GITHUB_API", &github.url),
    ]);
    let repo = server.home.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/widgets.git",
        ],
    );
    let repo = repo.canonicalize().unwrap();
    let mut client = server.attach(120, 40);
    client.wait_text("$");
    server.ok(&["new-tab", "-c", repo.to_str().unwrap()]);
    in_front_at(&server, &mut client, "▌○ 2 repo", &repo);
    let repo_tab = tabs(&server)
        .into_iter()
        .find(|t| t["current"] == true)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Listed in the palette while GitHub is on.
    client.type_text("\x1bp");
    client.wait_text("╭─ Actions");
    client.type_text("GitHub issue in a new tab");
    client.wait_text("› Start work on a GitHub issue");
    client.type_text("\x1b");
    client.wait_for("the palette to close", |s| {
        !s.iter().any(|l| l.contains("╭─ Actions"))
    });

    let before = tabs(&server).len();
    server.ok(&["press", "work-on-issue"]);
    client.wait_text("Open in >");
    client.send(b"\r");
    let screen = client.wait_text("#12 Someone else's");
    let row = |text: &str| screen.iter().position(|l| l.contains(text));
    assert!(
        row("#13 Mine to do") < row("#14 Newest of all")
            && row("#14 Newest of all") < row("#12 Someone else's"),
        "yours first, then the newest first: {screen:#?}"
    );
    // The first row, yours, picked.
    client.send(b"\r");
    wait("the issue's tab", || tabs(&server).len() == before + 1);
    let tab = tabs(&server).last().unwrap().clone();
    // No name of its own: its branch names it once wt has made it.
    assert_eq!(tab["options"]["@tab_name"], Value::Null);
    assert_eq!(tab["current"], true, "in front, as a tab you asked for");

    // A number typed that isn't listed is taken too.
    server.ok(&["select-tab", "-t", &repo_tab]);
    in_front_at(&server, &mut client, "2 repo", &repo);
    server.ok(&["press", "work-on-issue"]);
    client.wait_text("Open in >");
    client.send(b"\r");
    client.wait_text("#12 Someone else's");
    client.type_text("#40");
    client.type_text("\x1b\r");
    wait("the typed issue's tab", || {
        tabs(&server).len() == before + 2
    });
}

/// The world's repository named `o/r` on GitHub by its origin, which git
/// reaches as a bare repository beside it (`insteadOf`), so nothing leaves
/// the machine, keeping its work in a note's checklist, and a note of two
/// items.
fn keeps_its_work_in_notes(server: &TestServer, repo: &Path) {
    let origin = server.home.join("origin.git");
    git(
        &server.home,
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    git(
        repo,
        &["remote", "add", "origin", "https://github.com/o/r.git"],
    );
    git(
        repo,
        &[
            "config",
            &format!("url.{}.insteadOf", origin.display()),
            "https://github.com/o/r.git",
        ],
    );
    git(repo, &["push", "-q", "origin", "main"]);
    let config = server.home.join(".config/switchboard");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("repos.json"),
        r#"{ "o/r": { "tickets": "notes", "landing": "direct" } }"#,
    )
    .unwrap();
    let notes = server.home.join("Notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("Plan.md"),
        "# Plan\n\n- [ ] Fix the login\n- [ ] Speed up the build\n",
    )
    .unwrap();
}

#[test]
fn m_shift_w_lists_the_work_from_where_the_repository_keeps_it() {
    let (server, repo, mut client) = world();
    keeps_its_work_in_notes(&server, &repo);
    client.send(b"\x1bW");
    client.wait_text("Open in >");
    client.send(b"\r");
    // Its tickets are a note's items: the note first, as M-I asks.
    client.wait_text("note>");
    client.send(b"\r");
    client.wait_text("Every item listed (2)");
    client.wait_text("Speed up the build");
}

#[test]
fn wt_alone_asks_in_its_own_terminal_for_the_work_and_starts_the_one_picked() {
    let (server, repo, _client) = world();
    keeps_its_work_in_notes(&server, &repo);
    let out = server.ok(&[
        "new-tab",
        "-d",
        "-c",
        repo.to_str().unwrap(),
        "--",
        common::BIN,
        "worktree-from-ticket",
        "--no-claude",
    ]);
    let pane = out.split_whitespace().nth(1).unwrap().to_string();
    let screen = |server: &TestServer| server.ok(&["capture-pane", "-t", &pane]);
    wait("the note asked", || screen(&server).contains("Which note"));
    server.ok(&["send-keys", "-t", &pane, "Enter"]);
    wait("its items listed", || {
        screen(&server).contains("Work on Plan's items")
    });
    server.ok(&["send-keys", "-t", &pane, "-l", "speed"]);
    wait("the item matched", || {
        screen(&server).contains("Speed up the build") && !screen(&server).contains("Fix the login")
    });
    server.ok(&["send-keys", "-t", &pane, "Enter"]);
    let worktree = repo.join(".claude/worktrees/note-speed-up-the-build");
    let deadline = common::deadline(Duration::from_secs(15));
    while !worktree.is_dir() {
        assert!(
            Instant::now() < deadline,
            "no worktree: {}",
            screen(&server)
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
