//! The design-docs project skill's renderer (`.claude/skills/design-docs/scripts/render.js`),
//! which writes the UX 2.0 canvas's boards as docs/design's web pages: its self-test checks a
//! grid board's page has no line breaks between its tags while a Slack board's keeps its spaces,
//! and its made-up-data check, then draws both with Chrome, checking each picture's size, that it
//! isn't blank, and that the grid board's meter is on its text's row, so a one-row shift fails.
//! It needs Node, so it is skipped without it, and the drawing needs Chrome (the script exits 77
//! when it finds none, having checked the rest). Its fixtures load nothing from the network, and
//! Chrome runs with a profile of its own under the test's temporary home. Its sync is checked
//! too, writing pages from the fixtures' sources. The skill's `render.js push` and
//! .githooks/pre-push are checked in a repository of their own against a bare stand-in for
//! origin: a change to docs/design/ alone reaches main with no review, anything else doesn't.

use crate::common;

use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

use common::TempDir;

#[test]
fn the_renderer_keeps_a_grid_boards_text_on_its_meters_and_finds_names_not_made_up() {
    let home = TempDir::new("design-docs");
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(".claude/skills/design-docs/scripts/render.js");
    let out = match Command::new("node")
        .arg(&script)
        .arg("--self-test")
        .env("HOME", home.path())
        .env("TMPDIR", home.path())
        .output()
    {
        Err(e) if e.kind() == ErrorKind::NotFound => return eprintln!("skipped: no node"),
        result => result.unwrap(),
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let drew = out.status.code() != Some(77);
    assert!(
        out.status.success() || !drew,
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("ok grid page: no line breaks between its tags"),
        "{stdout}"
    );
    assert!(
        stdout.contains("ok slack page: its spaces between tags kept"),
        "{stdout}"
    );
    assert!(
        stdout.contains("ok site page: the sidebar marks its own board"),
        "{stdout}"
    );
    assert!(
        stdout.contains("ok sync: pages from the sources"),
        "{stdout}"
    );
    assert!(
        stdout.contains("ok sync: a source no page draws is checked too"),
        "{stdout}"
    );
    if !drew {
        return eprintln!("{stdout}");
    }
    assert!(stdout.contains("ok grid.dc.html: 1280×800"), "{stdout}");
    assert!(
        stdout.contains("ok grid.dc.html: meter on its text's row"),
        "{stdout}"
    );
    assert!(
        stdout.contains("ok grid.dc.html in the site: meter on its text's row"),
        "{stdout}"
    );
    assert!(stdout.contains("ok slack.dc.html: 420×880"), "{stdout}");
}

#[test]
fn the_renderers_chrome_asks_macos_for_no_keychain() {
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(".claude/skills/design-docs/scripts/render.js");
    let out = match Command::new("node")
        .arg(&script)
        .arg("--print-chrome-args")
        .output()
    {
        Err(e) if e.kind() == ErrorKind::NotFound => return eprintln!("skipped: no node"),
        result => result.unwrap(),
    };
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let switches: Vec<&str> = stdout.lines().collect();
    for wanted in ["--use-mock-keychain", "--password-store=basic"] {
        assert!(switches.contains(&wanted), "{stdout}");
    }
}

/// A clone of a bare stand-in for origin, its main holding a design page and
/// some code, with this repository's pre-push hook, run with a home of its
/// own and the test's switchboard first on the PATH (for the hook's setting
/// and `render.js push`'s leak-check), nothing else of the environment.
struct DesignRepo {
    tmp: TempDir,
    work: std::path::PathBuf,
}

impl DesignRepo {
    fn new() -> DesignRepo {
        let tmp = TempDir::new("design-push");
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(common::BIN, bin.join("switchboard")).unwrap();
        let remote = tmp.path().join("origin.git");
        let work = tmp.path().join("work");
        let repo = DesignRepo { tmp, work };
        repo.ok(
            repo.tmp.path(),
            &["git", "init", "--bare", "-b", "main", "origin.git"],
        );
        repo.ok(repo.tmp.path(), &["git", "init", "-b", "main", "work"]);
        repo.ok(
            &repo.work,
            &["git", "remote", "add", "origin", remote.to_str().unwrap()],
        );
        repo.write("docs/design/a.html", "<p>a board</p>\n");
        repo.write("src/lib.rs", "pub fn f() {}\n");
        repo.commit("Start");
        repo.ok(&repo.work, &["git", "push", "-q", "origin", "main"]);
        repo.ok(&repo.work, &["git", "fetch", "-q", "origin"]);
        let hooks = Path::new(env!("CARGO_MANIFEST_DIR")).join(".githooks");
        repo.ok(
            &repo.work,
            &["git", "config", "core.hooksPath", hooks.to_str().unwrap()],
        );
        repo
    }

    fn command(&self, dir: &Path, args: &[&str]) -> std::process::Output {
        let path = format!(
            "{}:/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin",
            self.tmp.path().join("bin").display()
        );
        Command::new(args[0])
            .args(&args[1..])
            .current_dir(dir)
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.tmp.path())
            .env("TMPDIR", self.tmp.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@e")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@e")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    }

    fn ok(&self, dir: &Path, args: &[&str]) -> String {
        let out = self.command(dir, args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn write(&self, file: &str, text: &str) {
        let path = self.work.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn commit(&self, message: &str) {
        self.ok(&self.work, &["git", "add", "-A"]);
        self.ok(&self.work, &["git", "commit", "-q", "-m", message]);
    }

    /// Pushes HEAD to main as `git push` does, the hook judging it, and
    /// says whether it went, with what git printed.
    fn push(&self) -> (bool, String) {
        let out = self.command(&self.work, &["git", "push", "origin", "HEAD:main"]);
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    }

    fn origin_main(&self) -> String {
        self.ok(&self.work, &["git", "rev-parse", "origin/main"])
    }

    fn head(&self) -> String {
        self.ok(&self.work, &["git", "rev-parse", "HEAD"])
    }

    fn back_to_origin(&self) {
        self.ok(&self.work, &["git", "fetch", "-q", "origin"]);
        self.ok(&self.work, &["git", "reset", "-q", "--hard", "origin/main"]);
    }
}

#[test]
fn the_pre_push_hook_lets_a_change_to_docs_design_alone_reach_main_unreviewed() {
    let repo = DesignRepo::new();

    repo.write("docs/design/b.html", "<p>b board</p>\n");
    repo.write("docs/design/src/B.dc.html", "<p>b source</p>\n");
    repo.commit("Add board b");
    let (pushed, said) = repo.push();
    assert!(pushed, "{said}");

    // Anything outside docs/design/ in the push keeps the review, even beside a design commit.
    let refused = [
        ("docs/design/c.html", Some("src/lib.rs")),
        ("docs/design-other/x.html", None),
        ("README.md", None),
    ];
    for (design, code) in refused {
        repo.back_to_origin();
        repo.write(design, "<p>c</p>\n");
        repo.commit("A board");
        if let Some(code) = code {
            repo.write(code, "pub fn g() {}\n");
            repo.commit("Some code");
        }
        let (pushed, said) = repo.push();
        assert!(!pushed, "{design} {code:?} went unreviewed");
        assert!(said.contains("hasn't been through /review-loop"), "{said}");
    }

    // A rename out of docs/design/ touches a path outside it.
    repo.back_to_origin();
    repo.ok(
        &repo.work,
        &["git", "mv", "docs/design/a.html", "docs/a.html"],
    );
    repo.commit("Move a board out");
    let (pushed, said) = repo.push();
    assert!(
        !pushed,
        "a rename out of docs/design/ went unreviewed: {said}"
    );

    // A commit that changes code and one that takes it back out leave a design-only diff, but
    // the push still carries the code's commit.
    repo.back_to_origin();
    repo.write("src/lib.rs", "pub fn h() {}\n");
    repo.commit("Some code");
    repo.write("src/lib.rs", "pub fn f() {}\n");
    repo.write("docs/design/d.html", "<p>d</p>\n");
    repo.commit("Take it back, add a board");
    let (pushed, said) = repo.push();
    assert!(!pushed, "a code commit went unreviewed: {said}");
}

#[test]
fn render_js_push_pushes_a_design_only_change_and_refuses_anything_else() {
    let repo = DesignRepo::new();
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(".claude/skills/design-docs/scripts/render.js");
    let script = script.to_str().unwrap();
    // The test's PATH holds none of yours, so Node is linked into its own.
    let Some(node) = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("node"))
            .find(|node| node.is_file())
    }) else {
        return eprintln!("skipped: no node");
    };
    std::os::unix::fs::symlink(node, repo.tmp.path().join("bin/node")).unwrap();
    let push = || {
        let out = repo.command(&repo.work, &["node", script, "push"]);
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    };

    repo.write("docs/design/b.html", "<p>b board</p>\n");
    repo.commit("Add board b");
    repo.write("src/lib.rs", "pub fn g() {}\n");
    repo.commit("Some code");
    let before = repo.origin_main();
    let (code, said) = push();
    assert_eq!(code, Some(1), "{said}");
    assert!(
        said.contains("changes more than docs/design/") && said.contains("src/lib.rs"),
        "{said}"
    );
    repo.ok(&repo.work, &["git", "fetch", "-q", "origin"]);
    assert_eq!(repo.origin_main(), before);

    repo.back_to_origin();
    repo.write("docs/design/b.html", "<p>b board</p>\n");
    repo.commit("Add board b");
    let (code, said) = push();
    assert_eq!(code, Some(0), "{said}");
    repo.ok(&repo.work, &["git", "fetch", "-q", "origin"]);
    assert_eq!(repo.origin_main(), repo.head());
}
