//! `land-lock test`: runs a landing's tests and records them, queue-and-clean's
//! steps 5 and 9 in one command, where the skill had Claude run the tests,
//! read their counts and record them, or, with the tests in CI, run
//! `leak-check`, push the landing's branch, wait on CI and delete the branch
//! itself, each a step a session could miss. New with the Rust version.
//!
//! ```text
//! land-lock test [--leak-ok] [--command <cmd>] [--base <ref>] [--again] [--here]
//! ```
//!
//! `--command` and `--leak-ok` come before the other arguments, or it
//! refuses them: `assets/claude-settings.json` allows `land-lock test
//! --base *`, `--again *` and `--here *`, so Claude runs the tests
//! unasked, while a command of its own choosing or a push past leak-check
//! is asked about.
//!
//! The repository's steps (`switchboard landing`'s, from repos.json) say
//! where the tests run. With `testIn` `here`, it runs repos.json's `test` in
//! the worktree with `sh -c`, its output (stdout and stderr) in
//! `landing-test.log` in the worktree's own git directory, and reads the
//! counts it can from the output (cargo, nextest, jest, vitest, pytest).
//! `test` not set (`null`) runs nothing: it exits 5 for Claude to pass the
//! command the repository's `CLAUDE.md` names with `--command`, which is
//! used for that run alone, never written into repos.json, the user's file.
//!
//! With `testIn` `ci`, in Switchboard's clone alone (`dev::is_switchboard_clone`;
//! elsewhere nothing is scanned), it runs `leak-check <base>..HEAD` first (from
//! where HEAD meets `<base>`, which must be on a branch of the remote
//! pushed to other than a `ci/` one, so nothing unpublished is left out of the scan; exit 2
//! otherwise), since the
//! commit leaves the computer, and anything it finds (or a check that can't
//! look) exits 3 with what it printed, for Claude's question; `--leak-ok`,
//! the user's answer to push anyway, skips it. Then it force-pushes HEAD to
//! `ci/land-<id>`, `<id>` the first 12 hex digits of `git hash-object` of
//! the branch's name and a newline (the same at every push of a landing,
//! the name itself never published), on the target's remote (`origin`
//! for the default branch), waits for its CI as `wait-ci --same-as` does,
//! beside the default branch's run at the commit the change sits on (the
//! merge base with `--base`), and deletes
//! the branch on every way out, an interrupt (SIGINT, SIGTERM, SIGHUP)
//! included, saying `ci/land-<id> not deleted: <why>` above its last line
//! when the remote refuses. No CI answering (`none`), a push failing,
//! GitHub refusing the token, a remote that isn't GitHub's or a target that
//! tracks no remote run
//! the tests here instead, and `none` marks the record (`ci` `none`), so the
//! rest of the landing runs them here too; `--here` does so by hand (the
//! user's "Run them here").
//!
//! A commit tested again (a run that failed for the default branch's
//! reasons, a job run again by hand) keeps the check runs of its earlier
//! runs, so before anything else leaves it asks for the commit's GitHub
//! Actions runs (`wait_ci::earlier`). When its own earlier pushes' runs
//! passed, each workflow's
//! latest run that wasn't canceled, and the commit's other checks and
//! statuses agree (another CI's among them), it says `passed in CI before,
//! run <id>` and passes with no leak-check and no push, the commit having
//! been pushed and tested as it is. GitHub not answering for those runs
//! runs the tests here, since a wait that didn't know them would answer
//! from them. Otherwise the wait leaves those runs' check
//! runs out, so the answer is the run this push started, and a canceled
//! job is neither passed nor failed there: a run with nothing failed and
//! something canceled is `canceled in CI`, to run again.
//!
//! Tests run here, and a push to CI (which runs the repository's pre-push
//! hook) is made, only where a landing may run the repository's code, as
//! for `beforePush` (`land_push::may_run_its_code`): your own and
//! origin's commits, or a main clone in trusted-dirs, never someone else's
//! PR checked out untrusted; otherwise it exits 7.
//!
//! Every run records `tests` ({ok, passed, failed, total, file}) and the
//! output (a CI run's: its failed jobs' logs, or a line saying it
//! passed), as `land-lock record` does, and sets the phase: `testing`
//! (`testing again` with `--again`), `testing in CI` while it waits, and
//! `tests failed <n of m>` when they fail. Its last line says how it went:
//!
//! ```text
//! passed [<n> of <total>]                     exit 0
//! no tests                                    exit 0, `test` empty
//! failed <n> of <total> <log>                 exit 1, the log's last lines above it
//! failed in CI <sha> <run id> <job names>     exit 1, the change's failures; run id "-" for another CI than GitHub's
//! mixed in CI <sha> <run id> <job names>      exit 1, some or maybe the default branch's
//! base-failing in CI <sha> <run id> <jobs>    exit 4, every failure the default branch's too
//! canceled in CI <sha> <run id> <job names>   exit 8, nothing failed, the run canceled
//! leak-check found <n>                        exit 3, its hits above it
//! not set                                     exit 5
//! untrusted                                   exit 7
//! timeout ci/land-<id>                        exit 6
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};

use super::land_lock::{record_at, set_phase};
use crate::sources::landing::own_git_dir;
use crate::{repos, util, worktree};

const USAGE: &str =
    "usage: land-lock test [--leak-ok] [--command <cmd>] [--base <ref>] [--again] [--here]";
/// The run's whole output, in the worktree's own git directory.
const LOG: &str = "landing-test.log";
/// How many of a failing run's last lines it prints.
const SHOWN: usize = 40;
const SOURCE: &str = "land-lock test";

/// What a run's output says it counted. `first` is the first failing test's
/// file, or its name where the runner names no file (cargo, nextest).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub passed: Option<u64>,
    pub failed: Option<u64>,
    pub total: Option<u64>,
    pub first: Option<String>,
}

/// `target` is `land-lock --target`'s branch, else the worktree's
/// `wtnTarget` names where the landing goes.
pub fn main(args: &[String], target: Option<String>) -> ExitCode {
    // `--command` (a command of Claude's choosing) and `--leak-ok` (a push
    // past leak-check) are read only before every other argument, so a
    // permission rule allowing `land-lock test --base *` never lets
    // either through.
    let first_other = args
        .iter()
        .position(|a| matches!(a.as_str(), "--base" | "--again" | "--here"));
    if let Some(at) = first_other
        && args[at..]
            .iter()
            .any(|a| a == "--command" || a == "--leak-ok")
    {
        eprintln!(
            "land-lock test: --command and --leak-ok come before the other arguments\n{USAGE}"
        );
        return ExitCode::from(2);
    }
    let mut args = args.to_vec();
    let mut value = |flag: &str| -> Option<Option<String>> {
        let at = args.iter().position(|a| a == flag)?;
        if at + 1 >= args.len() {
            return Some(None);
        }
        let v = args.remove(at + 1);
        args.remove(at);
        Some(Some(v))
    };
    let (base, command) = (value("--base"), value("--command"));
    if base == Some(None) || command == Some(None) {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let (base, command) = (base.flatten(), command.flatten());
    let flags = ["--again", "--here", "--leak-ok"];
    if let Some(bad) = args.iter().find(|a| !flags.contains(&a.as_str())) {
        eprintln!("land-lock test: unknown argument {bad}\n{USAGE}");
        return ExitCode::from(2);
    }
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let Some((top, own)) = worktree::git_at(&cwd, &["rev-parse", "--show-toplevel"])
        .ok()
        .map(PathBuf::from)
        .and_then(|top| Some((top.clone(), own_git_dir(&top)?)))
    else {
        eprintln!("not in a git repository");
        return ExitCode::from(2);
    };
    let steps = match repos::steps_at(&top) {
        Ok(steps) => steps,
        Err(e) => {
            eprintln!("land-lock test: {e}");
            return ExitCode::from(2);
        }
    };
    let run = Run {
        top,
        own,
        test: command.or(steps.test),
        again: has("--again"),
        target,
    };
    let ci_none = crate::sources::landing::read_record(&run.top)
        .is_some_and(|r| r.get("ci").and_then(Value::as_str) == Some("none"));
    if steps.test_in != "ci" || has("--here") || ci_none {
        return run.here();
    }
    let Some(base) = base else {
        eprintln!("land-lock test: tests in CI need --base, for leak-check\n{USAGE}");
        return ExitCode::from(2);
    };
    run.in_ci(&base, has("--leak-ok"))
}

struct Run {
    top: PathBuf,
    own: PathBuf,
    test: Option<String>,
    again: bool,
    /// `land-lock --target`'s branch, when given.
    target: Option<String>,
}

impl Run {
    fn testing(&self) -> &'static str {
        if self.again {
            "testing again"
        } else {
            "testing"
        }
    }

    /// Records `words` as `land-lock record` would; a record failing never
    /// stops the landing.
    fn record(&self, words: &[&str]) {
        record_at(&self.top, words);
    }

    /// Keeps `text` as the run's output (`landing-test.log`), recorded for
    /// the Landing view as a run here records its own.
    fn keep_output(&self, text: &str) {
        let log = self.own.join(LOG);
        if util::write_atomic(&log, text).is_ok() {
            self.record(&["output", "tests", &log.to_string_lossy()]);
        }
    }

    /// Records the run, with the commit it ran on (`head`), by which
    /// `land-lock settle` tells a commit already tested from one to test.
    fn record_tests(&self, ok: bool, counts: &Counts) {
        let head = worktree::git_at(&self.top, &["rev-parse", "HEAD"]).unwrap_or_default();
        let mut tests = json!({
            "ok": ok,
            "file": counts.first.clone().unwrap_or_default(),
            "head": head,
        });
        for (key, n) in [
            ("passed", counts.passed),
            ("failed", counts.failed),
            ("total", counts.total),
        ] {
            if let Some(n) = n {
                tests[key] = json!(n);
            }
        }
        self.record(&["tests", &tests.to_string()]);
    }

    /// Runs the tests in the worktree.
    fn here(&self) -> ExitCode {
        let Some(test) = self.test.as_deref() else {
            println!(
                "repos.json sets no test command: run this again with --command and the one the repository's CLAUDE.md names, or with --command '' when it names none"
            );
            println!("not set");
            return ExitCode::from(5);
        };
        if test.trim().is_empty() {
            println!("no tests");
            return ExitCode::SUCCESS;
        }
        if let Some(refused) = self.untrusted() {
            return refused;
        }
        set_phase(self.testing());
        let log = self.own.join(LOG);
        let status = fs::File::create(&log).and_then(|out| {
            Command::new("sh")
                .arg("-c")
                .arg(test)
                .current_dir(&self.top)
                .stdin(Stdio::null())
                .stdout(out.try_clone()?)
                .stderr(out)
                .status()
        });
        let status = match status {
            Ok(s) => s,
            Err(e) => {
                eprintln!("land-lock test: couldn't run {test}: {e}");
                return ExitCode::FAILURE;
            }
        };
        let text = String::from_utf8_lossy(&fs::read(&log).unwrap_or_default()).into_owned();
        let counts = counts(&text);
        let ok = status.success();
        self.record_tests(ok, &counts);
        self.record(&["output", "tests", &log.to_string_lossy()]);
        if ok {
            match (counts.passed, counts.total) {
                (Some(p), Some(t)) => println!("passed {p} of {t}"),
                _ => println!("passed"),
            }
            return ExitCode::SUCCESS;
        }
        let lines: Vec<&str> = text.lines().collect();
        for line in &lines[lines.len().saturating_sub(SHOWN)..] {
            println!("{}", strip_escapes(line));
        }
        let of = of_words(&counts);
        set_phase(format!("tests failed {of}").trim());
        println!("failed {of} {}", log.display());
        ExitCode::FAILURE
    }

    /// `Some` exit, having said why, when the worktree holds code a landing
    /// may not run (`land_push::may_run_its_code`): its tests, or the
    /// repository's pre-push hook, which a push to CI runs.
    fn untrusted(&self) -> Option<ExitCode> {
        let common = worktree::git_at(
            &self.top,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .map(PathBuf::from)
        .unwrap_or_default();
        if super::land_push::may_run_its_code(&common, &self.top) {
            return None;
        }
        println!(
            "the worktree holds code that isn't yours, so neither its tests nor a push of it run; trust the main clone in trusted-dirs to land it"
        );
        println!("untrusted");
        Some(ExitCode::from(7))
    }

    /// Runs the tests in the repository's CI, as the module's comment says.
    fn in_ci(&self, base: &str, leak_ok: bool) -> ExitCode {
        // First, so a target that tracks no remote runs them here with no
        // published base to check.
        let Some(ci) = Ci::of(&self.top, self.target.clone()) else {
            println!("no GitHub remote to run the tests on: running them here");
            return self.here();
        };
        if let Some(refused) = self.untrusted() {
            return refused;
        }
        // leak-check is a rule of Switchboard's own repository alone
        // (src/dev.rs): elsewhere it isn't run, and the base needn't be
        // published.
        let scan = !leak_ok && crate::dev::is_switchboard_clone(&self.top);
        // leak-check scans from where the change leaves what is already
        // public, so a base that isn't on a branch of the remote pushed to
        // (`--base HEAD`, or a commit only another remote has) would let
        // unpublished commits through unscanned. A landing's own `ci/`
        // branch doesn't count: what it holds was pushed for a test.
        let pushed_to = format!("refs/remotes/{}/", ci.remote);
        let published = worktree::git_at(&self.top, &["merge-base", "HEAD", base]).ok();
        let on_a_remote = published.as_deref().is_some_and(|commit| {
            worktree::git_at(
                &self.top,
                &[
                    "for-each-ref",
                    "--contains",
                    commit,
                    "--format=%(refname)",
                    &pushed_to,
                ],
            )
            .is_ok_and(|refs| {
                refs.lines().any(|r| {
                    r.strip_prefix(&pushed_to).is_some_and(|branch| {
                        !branch.trim().is_empty() && !branch.starts_with("ci/")
                    })
                })
            })
        });
        let published = published.filter(|_| on_a_remote);
        if scan && published.is_none() {
            println!(
                "{base} isn't on a remote branch, so leak-check would miss what isn't published: pass the base the commit is on, as fetched"
            );
            return ExitCode::from(2);
        }
        let sha = worktree::git_at(&self.top, &["rev-parse", "HEAD"]).unwrap_or_default();
        crate::integrations::run_by_you();
        let earlier = match super::wait_ci::earlier(
            &ci.repo,
            &sha,
            &ci.branch,
            super::wait_ci::poll(),
        ) {
            Ok(earlier) => earlier,
            Err(e) => {
                println!(
                    "GitHub didn't say which runs the commit already has ({e}): running the tests here"
                );
                return self.here();
            }
        };
        if let Some(runs) = &earlier.passed {
            let ids: Vec<String> = runs.iter().map(u64::to_string).collect();
            let before = format!("passed in CI before, run {}", ids.join(", "));
            self.keep_output(&format!("{before}: every check of {sha} passed\n"));
            self.record_tests(true, &Counts::default());
            println!("{before}");
            println!("passed");
            return ExitCode::SUCCESS;
        }
        if let Some(published) = published.as_ref().filter(|_| scan) {
            let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("switchboard"));
            let out = Command::new(exe)
                .args(["leak-check", &format!("{published}..HEAD")])
                .current_dir(&self.top)
                .stdin(Stdio::null())
                .output();
            let (code, said, err) = match &out {
                Ok(o) => (
                    o.status.code().unwrap_or(2),
                    String::from_utf8_lossy(&o.stdout).into_owned(),
                    String::from_utf8_lossy(&o.stderr).into_owned(),
                ),
                Err(e) => (2, String::new(), e.to_string()),
            };
            let found = said.lines().filter(|l| !l.trim().is_empty()).count();
            self.record(&["leak", &json!({ "found": found }).to_string()]);
            if code != 0 {
                print!("{said}");
                if code != 1 {
                    println!("leak-check couldn't look: {}", err.trim());
                }
                println!("leak-check found {found}");
                return ExitCode::from(3);
            }
        }
        set_phase(self.testing());
        // Before the push, so an interrupt from here on deletes the branch.
        let cleanup = ci.delete_on_interrupt();
        worktree::count_fetch(&self.top, &ci.remote, SOURCE);
        if !worktree::git_ok(
            &self.top,
            &[
                "push",
                "--quiet",
                "--force",
                &ci.remote,
                &format!("HEAD:refs/heads/{}", ci.branch),
            ],
        ) {
            println!("the push to {} failed: running the tests here", ci.branch);
            return self.here();
        }
        // The commit the change sits on, whose CI tells the default
        // branch's failures from the change's.
        let same_as = published;
        set_phase("testing in CI");
        let answer = super::wait_ci::wait(
            &ci.repo,
            &sha,
            same_as.as_deref(),
            Duration::from_secs(45 * 60),
            Duration::from_secs(3 * 60),
            super::wait_ci::poll(),
            Some(&earlier.suites),
        );
        ci.delete();
        drop(cleanup);
        let answer = match answer {
            Ok(answer) => answer,
            Err(e) => {
                println!("GitHub refused the CI's look ({e}): running the tests here");
                return self.here();
            }
        };
        if answer.starts_with("passed") {
            self.keep_output(&format!("{answer}: every check of {} passed\n", ci.branch));
            self.record_tests(true, &Counts::default());
            println!("passed");
            return ExitCode::SUCCESS;
        }
        let failure = ["failed", "change-failing", "base-failing", "mixed"]
            .into_iter()
            .find_map(|word| Some((word, answer.strip_prefix(word)?.strip_prefix(' ')?)));
        if let Some((word, rest)) = failure {
            let jobs = rest.splitn(3, ' ').nth(2).unwrap_or("").to_string();
            let logs = super::wait_ci::failed_logs(&ci.repo, &sha, &earlier.suites);
            self.keep_output(&format!("{answer}\n{logs}"));
            let counts = Counts {
                first: Some(jobs),
                ..counts_by_job(&logs)
            };
            self.record_tests(false, &counts);
            if word == "base-failing" {
                println!("base-failing in CI {rest}");
                return ExitCode::from(4);
            }
            set_phase("tests failed in CI");
            let word = if word == "mixed" { "mixed" } else { "failed" };
            println!("{word} in CI {rest}");
            return ExitCode::FAILURE;
        }
        if let Some(rest) = answer.strip_prefix("canceled ") {
            self.keep_output(&format!("{answer}\n"));
            println!("canceled in CI {rest}");
            return ExitCode::from(8);
        }
        if answer == "none" {
            let mut record = util::read_json_object(self.own.join(crate::sources::landing::RECORD));
            record.insert("ci".into(), json!("none"));
            let _ = util::write_atomic(
                self.own.join(crate::sources::landing::RECORD),
                util::json_compact(&Value::Object(record)),
            );
            println!("no CI ran on {}: running the tests here", ci.branch);
            return self.here();
        }
        println!("timeout {}", ci.branch);
        ExitCode::from(6)
    }
}

/// `<failed> of <total>`, as much as is known.
fn of_words(counts: &Counts) -> String {
    match (counts.failed, counts.total) {
        (Some(f), Some(t)) => format!("{f} of {t}"),
        (Some(f), None) => f.to_string(),
        _ => String::new(),
    }
}

/// Where a landing's tests run in CI.
struct Ci {
    top: PathBuf,
    remote: String,
    /// `owner/name` on GitHub.
    repo: String,
    /// `ci/land-<id>`.
    branch: String,
}

impl Ci {
    /// The landing's CI from the worktree at `top`, landing on `given` (else
    /// its `wtnTarget`, else the default branch on `origin`): `None` when
    /// that target tracks no remote, or the remote isn't on GitHub.
    fn of(top: &Path, given: Option<String>) -> Option<Ci> {
        let name = worktree::git_at(top, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()?;
        let target = worktree::git_at(
            top,
            &["config", "--get", &format!("branch.{name}.wtnTarget")],
        )
        .ok()
        .filter(|t| !t.is_empty());
        let remote = match given.or(target) {
            Some(target) => super::worktree_from_note::tracked(top, &target)?.0,
            None => "origin".to_string(),
        };
        let url =
            worktree::git_at(top, &["config", "--get", &format!("remote.{remote}.url")]).ok()?;
        let repo = repos::repo_of_url(&url)?;
        Some(Ci {
            top: top.to_path_buf(),
            remote,
            repo,
            branch: branch_for(&name),
        })
    }

    /// Deletes the branch, saying so in a line when it couldn't (a ruleset
    /// refusing it, the network down), so it isn't left published unseen.
    fn delete(&self) {
        worktree::count_fetch(&self.top, &self.remote, SOURCE);
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.top)
            .args(["push", "--quiet", &self.remote, "--delete", &self.branch])
            .stdin(Stdio::null())
            .output();
        let why = match out {
            Ok(o) if o.status.success() => return,
            Ok(o) => String::from_utf8_lossy(&o.stderr).trim().to_string(),
            Err(e) => e.to_string(),
        };
        // Gone already is what was wanted.
        if why.contains("remote ref does not exist") {
            return;
        }
        let why = why.lines().last().unwrap_or("").to_string();
        println!("{} not deleted: {why}", self.branch);
    }

    /// Deletes the branch when the process is interrupted while it waits,
    /// then exits; the thread is left to end with the process.
    fn delete_on_interrupt(&self) -> std::thread::JoinHandle<()> {
        let ci = Ci {
            top: self.top.clone(),
            remote: self.remote.clone(),
            repo: self.repo.clone(),
            branch: self.branch.clone(),
        };
        let (ready, started) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            util::block_on(async move {
                use tokio::signal::unix::{SignalKind, signal};
                let (Ok(mut int), Ok(mut term), Ok(mut hup)) = (
                    signal(SignalKind::interrupt()),
                    signal(SignalKind::terminate()),
                    signal(SignalKind::hangup()),
                ) else {
                    let _ = ready.send(());
                    return;
                };
                let _ = ready.send(());
                tokio::select! {
                    _ = int.recv() => {}
                    _ = term.recv() => {}
                    _ = hup.recv() => {}
                }
                ci.delete();
                set_phase("");
                std::process::exit(130);
            });
        });
        // The handlers are in place before the wait starts.
        let _ = started.recv();
        handle
    }
}

/// `ci/land-<id>` for the branch `name`: what `git rev-parse --abbrev-ref
/// HEAD | git hash-object --stdin | cut -c1-12` prints.
pub fn branch_for(name: &str) -> String {
    use sha1::{Digest, Sha1};
    let content = format!("{name}\n");
    let mut hasher = Sha1::new();
    hasher.update(format!("blob {}\0", content.len()).as_bytes());
    hasher.update(content.as_bytes());
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("ci/land-{}", &hex[..12])
}

/// A line without its terminal escape sequences and control characters.
fn strip_escapes(line: &str) -> String {
    let escapes = Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b[@-_]").expect("a regex");
    escapes
        .replace_all(line, "")
        .chars()
        .filter(|c| *c == '\t' || !c.is_control())
        .collect()
}

/// The counts a run's output gives, from the summaries of cargo test,
/// cargo-nextest, jest, vitest and pytest; none known for any other.
/// The counts of `failed_logs`'s jobs, each read on its own and summed, so
/// two partitions' summaries add up rather than the last standing for both.
fn counts_by_job(logs: &str) -> Counts {
    let mut jobs: Vec<String> = Vec::new();
    for line in logs.lines() {
        if jobs.is_empty() || (line.starts_with("==> ") && line.ends_with(" <==")) {
            jobs.push(String::new());
        }
        let job = jobs.last_mut().expect("one at least");
        job.push_str(line);
        job.push('\n');
    }
    let add = |a: Option<u64>, b: Option<u64>| match (a, b) {
        (Some(a), Some(b)) => Some(a + b),
        (a, b) => a.or(b),
    };
    jobs.iter()
        .map(|job| counts(job))
        .fold(Counts::default(), |sum, job| Counts {
            passed: add(sum.passed, job.passed),
            failed: add(sum.failed, job.failed),
            total: add(sum.total, job.total),
            first: sum.first.or(job.first),
        })
}

pub fn counts(output: &str) -> Counts {
    let text: String = output
        .lines()
        .map(strip_escapes)
        .collect::<Vec<_>>()
        .join("\n");
    let number = |caps: &regex::Captures, at: usize| {
        caps.get(at).and_then(|m| m.as_str().parse::<u64>().ok())
    };
    let first_of = |pattern: &str| -> Option<String> {
        Regex::new(pattern)
            .ok()?
            .captures(&text)
            .map(|c| c[1].trim().to_string())
    };
    // nextest: `Summary [ 1.2s] 401 tests run: 399 passed, 2 failed, 3 skipped`.
    let nextest =
        Regex::new(r"(?m)^\s*Summary \[[^\]]*\]\s+(\d+) tests? run: (.*)$").expect("a regex");
    if let Some(caps) = nextest.captures_iter(&text).last() {
        let rest = caps[2].to_string();
        let part = |word: &str| -> Option<u64> {
            Regex::new(&format!(r"(\d+) {word}"))
                .ok()?
                .captures(&rest)
                .and_then(|c| c[1].parse().ok())
        };
        return Counts {
            total: number(&caps, 1),
            passed: part("passed").or(Some(0)),
            failed: Some(part("failed").unwrap_or(0) + part("timed out").unwrap_or(0)),
            first: first_of(r"(?m)^\s*(?:FAIL|TIMEOUT) \[[^\]]*\]\s+(?:\(\s*\d+/\d+\)\s+)?(.+)$"),
        };
    }
    // cargo test: a `test result:` line a binary, summed.
    let cargo = Regex::new(r"test result: \w+\. (\d+) passed; (\d+) failed").expect("a regex");
    let results: Vec<_> = cargo.captures_iter(&text).collect();
    if !results.is_empty() {
        let passed: u64 = results.iter().filter_map(|c| number(c, 1)).sum();
        let failed: u64 = results.iter().filter_map(|c| number(c, 2)).sum();
        return Counts {
            passed: Some(passed),
            failed: Some(failed),
            total: Some(passed + failed),
            first: first_of(r"(?m)^---- (.+) stdout ----$"),
        };
    }
    // jest: `Tests:       1 failed, 2 skipped, 5 passed, 8 total`.
    let jest = Regex::new(r"(?m)^Tests:\s+(.*\d+ total)\s*$").expect("a regex");
    if let Some(caps) = jest.captures_iter(&text).last() {
        let rest = caps[1].to_string();
        let part = |word: &str| -> Option<u64> {
            Regex::new(&format!(r"(\d+) {word}"))
                .ok()?
                .captures(&rest)
                .and_then(|c| c[1].parse().ok())
        };
        return Counts {
            passed: part("passed").or(Some(0)),
            failed: part("failed").or(Some(0)),
            total: part("total"),
            first: first_of(r"(?m)^\s*FAIL\s+(\S+)"),
        };
    }
    // vitest: ` Tests  1 failed | 2 passed (3)`.
    let vitest = Regex::new(r"(?m)^\s*Tests\s+(.*)\((\d+)\)\s*$").expect("a regex");
    if let Some(caps) = vitest.captures_iter(&text).last() {
        let rest = caps[1].to_string();
        let part = |word: &str| -> Option<u64> {
            Regex::new(&format!(r"(\d+) {word}"))
                .ok()?
                .captures(&rest)
                .and_then(|c| c[1].parse().ok())
        };
        return Counts {
            passed: part("passed").or(Some(0)),
            failed: part("failed").or(Some(0)),
            total: number(&caps, 2),
            first: first_of(r"(?m)^\s*FAIL\s+(\S+)"),
        };
    }
    // pytest: `===== 2 failed, 10 passed, 1 skipped in 1.23s =====`.
    let pytest =
        Regex::new(r"(?m)^=+ (.*\d+ (?:passed|failed|errors?).*) in [\d.]+s(?: \([^)]*\))? =+$")
            .expect("a regex");
    if let Some(caps) = pytest.captures_iter(&text).last() {
        let rest = caps[1].to_string();
        let part = |word: &str| -> Option<u64> {
            Regex::new(&format!(r"(\d+) {word}"))
                .ok()?
                .captures(&rest)
                .and_then(|c| c[1].parse().ok())
        };
        let passed = part("passed").unwrap_or(0);
        let failed = part("failed").unwrap_or(0) + part("errors?").unwrap_or(0);
        return Counts {
            passed: Some(passed),
            failed: Some(failed),
            total: Some(passed + failed),
            first: first_of(r"(?m)^(?:FAILED|ERROR) ([^:\s]+)"),
        };
    }
    Counts::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(passed: u64, failed: u64, total: u64, first: Option<&str>) -> Counts {
        Counts {
            passed: Some(passed),
            failed: Some(failed),
            total: Some(total),
            first: first.map(str::to_string),
        }
    }

    #[test]
    fn cargo_s_results_are_summed_over_its_binaries() {
        let out = "running 3 tests\n\
            test a ... ok\n\
            ---- tabs::tests::names stdout ----\n\
            thread panicked\n\
            test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n\
            \n\
            test result: ok. 10 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out\n";
        assert_eq!(counts(out), c(12, 1, 13, Some("tabs::tests::names")));
    }

    #[test]
    fn nextest_s_summary_is_read_and_its_first_failure_named() {
        let out = "        PASS [   0.004s] switchboard::it a::b\n\
            \x1b[31m        FAIL\x1b[0m [   0.100s] switchboard::it land_lock::fails\n\
            ------------\n     Summary [  12.345s] 401 tests run: 399 passed, 2 failed, 3 skipped\n";
        assert_eq!(
            counts(out),
            c(399, 2, 401, Some("switchboard::it land_lock::fails"))
        );
        assert_eq!(
            counts("     Summary [   0.1s] 1 test run: 1 passed, 0 skipped\n"),
            c(1, 0, 1, None)
        );
        let two = "==> Tests (1/2) <==\n     Summary [ 1s] 100 tests run: 98 passed, 2 failed, 0 skipped\n\
            ==> Tests (2/2) <==\n     Summary [ 1s] 100 tests run: 99 passed, 1 failed, 0 skipped\n";
        assert_eq!(
            counts_by_job(two),
            c(197, 3, 200, None),
            "two partitions add up"
        );
        // An Actions log, as `wait_ci::failed_logs` gives it.
        let actions = "2026-11-16T14:33:12.0708011Z      Summary [  12.345s] 401 tests run: 399 passed, 2 failed, 3 skipped\n";
        assert_eq!(
            counts(&crate::sources::ciboard::readable(actions)),
            c(399, 2, 401, None)
        );
    }

    #[test]
    fn jest_vitest_and_pytest_summaries_are_read() {
        let jest = "FAIL src/a.test.ts\n  ● a › b\nPASS src/b.test.ts\n\
            Test Suites: 1 failed, 1 passed, 2 total\n\
            Tests:       1 failed, 2 skipped, 5 passed, 8 total\n";
        assert_eq!(counts(jest), c(5, 1, 8, Some("src/a.test.ts")));
        let vitest = " FAIL  src/x.test.ts > suite > name\n\n Test Files  1 failed | 3 passed (4)\n      Tests  1 failed | 20 passed (21)\n";
        assert_eq!(counts(vitest), c(20, 1, 21, Some("src/x.test.ts")));
        let pytest = "FAILED tests/test_a.py::test_x - assert 1 == 2\n\
            ========== 1 failed, 10 passed, 1 skipped in 1.23s ==========\n";
        assert_eq!(counts(pytest), c(10, 1, 11, Some("tests/test_a.py")));
        assert_eq!(counts("==== 3 passed in 0.10s ====\n"), c(3, 0, 3, None));
    }

    #[test]
    fn an_unknown_runner_gives_no_counts() {
        assert_eq!(counts("all good\n"), Counts::default());
    }

    #[test]
    fn the_ci_branch_is_git_s_hash_of_the_branch_name() {
        // `printf 'main\n' | git hash-object --stdin`
        assert_eq!(branch_for("main"), "ci/land-ba2906d0666c");
    }
}
