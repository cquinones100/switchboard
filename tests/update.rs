//! Updates against a GitHub of the test's own on 127.0.0.1
//! (`SWITCHBOARD_GITHUB_API` moved there, the release's files served from
//! it too): `switchboard update` swapping a copy of the binary for the
//! release's and running its setup, a download whose checksum doesn't match
//! refused with the old binary left, the server's box on attaching and its
//! hourly look's timing, and install.sh, with `uname` stubbed for each
//! platform and run whole against the same stand-in.
//!
//! It sets `SWITCHBOARD_STATE_DIR` in the process's environment, so the file
//! is a binary of its own, apart from tests/it, whose other tests' threads a
//! change to the environment would race.

#![cfg(feature = "net")]

mod common;

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use common::http::{self, Reply, Service};
use common::stand_in;
use common::{BIN, TempDir, TestServer};
use serde_json::{Value, json};
use switchboard::update::{self, Release, install, release};

const INSTALL_SH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/install.sh");
const TARGETS: [&str; 3] = [
    "x86_64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
];

/// Each path's status and body.
type Routes = Arc<Mutex<HashMap<String, (u16, Vec<u8>)>>>;

/// A stand-in for GitHub, as strict as GitHub's API is for a public
/// repository (tests/common/http.rs: a User-Agent, GET, no token needed and
/// a wrong one refused): each path's answer, changeable while it runs, the
/// release's files served from it too, and what each path was asked.
struct Fake {
    url: String,
    routes: Routes,
    /// Whether the token's account has spent its rate limit, so a request
    /// carrying it is answered 403 as GitHub answers one.
    spent: Arc<AtomicBool>,
    http: http::Fake,
}

impl Fake {
    fn start() -> Fake {
        Fake::start_knowing(None)
    }

    /// One that takes `token` as well as no token, refusing any other.
    fn start_knowing(token: Option<&str>) -> Fake {
        let routes: Routes = Arc::default();
        let r = routes.clone();
        let spent = Arc::new(AtomicBool::new(false));
        let s = spent.clone();
        let service = Service::GitHubPublic {
            token: token.map(str::to_string),
        };
        let http = http::Fake::start(service, move |request| {
            if request.method != "GET" {
                return None;
            }
            if s.load(Ordering::Relaxed) && request.header("authorization").is_some() {
                let reset = (std::time::SystemTime::now() + Duration::from_secs(600))
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                return Some(
                    Reply::json(
                        403,
                        json!({ "message": "API rate limit exceeded for user ID 1." }),
                    )
                    .header("x-ratelimit-remaining", "0")
                    .header("x-ratelimit-reset", reset.to_string()),
                );
            }
            let kind = if request.path().starts_with("/dl/") {
                "application/octet-stream"
            } else {
                "application/json; charset=utf-8"
            };
            r.lock()
                .unwrap()
                .get(request.path())
                .map(|(status, body)| Reply::bytes(*status, kind, body.clone()))
        });
        Fake {
            url: http.url.clone(),
            routes,
            spent,
            http,
        }
    }

    fn serve(&self, path: &str, status: u16, body: impl Into<Vec<u8>>) {
        self.routes
            .lock()
            .unwrap()
            .insert(path.to_string(), (status, body.into()));
    }

    fn asked(&self, path: &str) -> usize {
        self.http.seen().iter().filter(|r| r.path() == path).count()
    }

    /// The `Authorization` each ask of `path` carried, in order.
    fn signed(&self, path: &str) -> Vec<Option<String>> {
        self.http
            .seen()
            .iter()
            .filter(|r| r.path() == path)
            .map(|r| r.header("authorization").map(str::to_string))
            .collect()
    }

    /// A release `tag` whose archive for every target holds `binary`, each
    /// with its checksum (or `bad_sum`'s, to show one refused), served at
    /// the releases list and as the latest; gives its JSON.
    fn publish(&self, tag: &str, binary: &[u8], bad_sum: bool) -> Value {
        let version = tag.trim_start_matches('v');
        let mut assets = Vec::new();
        for target in TARGETS {
            let name = format!("switchboard-{version}-{target}.tar.gz");
            let archive = archive_of(
                &format!("switchboard-{version}-{target}/switchboard"),
                binary,
            );
            let sum = if bad_sum {
                install::sha256_hex(b"something else")
            } else {
                install::sha256_hex(&archive)
            };
            self.serve(&format!("/dl/{name}"), 200, archive);
            self.serve(
                &format!("/dl/{name}.sha256"),
                200,
                format!("{sum}  {name}\n"),
            );
            for file in [name.clone(), format!("{name}.sha256")] {
                assets.push(json!({
                    "name": file,
                    "browser_download_url": format!("{}/dl/{file}", self.url),
                }));
            }
        }
        let json = json!({
            "tag_name": tag,
            "name": format!("Switchboard {version}"),
            "body": format!("- Everything in {version} (abc1234)"),
            "draft": false,
            "prerelease": false,
            "published_at": "2026-11-01T12:00:00Z",
            "html_url": format!("https://github.com/{}/releases/tag/{tag}", update::REPO),
            "assets": assets,
        });
        let base = format!("/repos/{}/releases", update::REPO);
        self.serve(&base, 200, json!([json]).to_string());
        self.serve(&format!("{base}/latest"), 200, json.to_string());
        json
    }

    fn list_path() -> String {
        format!("/repos/{}/releases", update::REPO)
    }
}

/// A tar.gz holding one file, at `path`, executable.
fn archive_of(path: &str, bytes: &[u8]) -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    {
        let mut tar = tar::Builder::new(&mut gz);
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, path, bytes).unwrap();
        tar.finish().unwrap();
    }
    gz.finish().unwrap()
}

/// What the release's binary is in these tests: a stand-in release
/// (tests/common/stand_in.rs) that says a version and notes in `$SB_MARK`
/// that its setup ran. Named anything but switchboard, it fails as
/// switchboard 0.2.0 did, reading its own name as a command it doesn't
/// have, so an installer that runs it under another name is caught here.
fn stub(version: &str) -> Vec<u8> {
    stand_in::Release::new(&format!("{version} (stub)"))
        .on_setup("echo \"setup $*\" >> \"$SB_MARK\"")
        .script()
        .into_bytes()
}

#[test]
fn the_binary_is_switchboard_under_any_name_but_a_commands() {
    let dir = TempDir::new("sbn");
    let version = format!("switchboard {} (", env!("CARGO_PKG_VERSION"));
    // As an installer runs it once before renaming it into place.
    for name in [".switchboard.new.123", "switchboard-0.2.0"] {
        let copy = dir.path().join(name);
        std::fs::copy(BIN, &copy).unwrap();
        // A file just written is "busy" on Linux for a moment while a
        // program another test starts still holds it open until its exec,
        // as install.rs's spawn_version knows; tried again meanwhile.
        let out = (0..250)
            .find_map(|_| match Command::new(&copy).arg("--version").output() {
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                    thread::sleep(Duration::from_millis(20));
                    None
                }
                other => Some(other.unwrap()),
            })
            .expect("the copy never stopped being busy");
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && said.starts_with(&version),
            "{name}: {said}{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    // A link named after a command still runs that command.
    let link = dir.path().join("skill-path");
    std::os::unix::fs::symlink(BIN, &link).unwrap();
    let out = Command::new(&link).arg("--help").output().unwrap();
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(said.contains("skill-path"), "{said}");
    assert!(!said.starts_with("switchboard 0"), "{said}");
}

/// A copy of the switchboard under test to update, so the one cargo built
/// is never replaced: a hard link where the folders share a disk (the swap
/// renames a new file over it, which leaves the other name as it was), else
/// a copy.
fn own_copy(dir: &Path) -> PathBuf {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let copy = bin.join("switchboard");
    if std::fs::hard_link(BIN, &copy).is_err() {
        std::fs::copy(BIN, &copy).unwrap();
    }
    copy
}

fn command(program: &Path, fake: &Fake, home: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.env("HOME", home)
        .env("SWITCHBOARD_STATE_DIR", home.join("state"))
        .env("SWITCHBOARD_GITHUB_API", &fake.url)
        .env("SB_MARK", home.join("mark"))
        .env("PATH", stand_in::path_with_gh(&home.join("stand-ins")))
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .env_remove("SWITCHBOARD")
        .stdin(Stdio::null());
    cmd
}

#[test]
fn update_puts_the_release_in_place_and_runs_its_setup() {
    let fake = Fake::start();
    fake.publish("v9.9.9", &stub("9.9.9"), false);
    let dir = TempDir::new("sbu");
    let copy = own_copy(dir.path());
    let before = std::fs::metadata(BIN).unwrap().len();

    // --check says what is out and changes nothing.
    let out = command(&copy, &fake, dir.path())
        .args(["update", "--check"])
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains(&format!(
            "Switchboard 9.9.9 is out; you have {}.",
            env!("CARGO_PKG_VERSION")
        )),
        "{said}"
    );
    assert!(said.contains("• Everything in 9.9.9 (abc1234)"), "{said}");
    assert!(dir.path().join("state/update-checked").exists());

    // A binary cargo built isn't replaced without --force.
    let out = command(&copy, &fake, dir.path())
        .arg("update")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("built from source"));
    assert_ne!(std::fs::read(&copy).unwrap(), stub("9.9.9"));
    // Nor with the release named, as M-p's "Update Switchboard" box runs it.
    let out = command(&copy, &fake, dir.path())
        .args(["update", "--tag", "v9.9.9"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("built from source"));
    assert_ne!(std::fs::read(&copy).unwrap(), stub("9.9.9"));

    let out = command(&copy, &fake, dir.path())
        .args(["update", "--force"])
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{said}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(said.contains("Checked it against"), "{said}");
    assert!(said.contains("to switchboard 9.9.9 (stub)"), "{said}");
    assert_eq!(std::fs::read(&copy).unwrap(), stub("9.9.9"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("mark")).unwrap(),
        "setup setup\n"
    );
    // The binary the tests run is as it was.
    assert_eq!(std::fs::metadata(BIN).unwrap().len(), before);
    // Nothing is left beside it.
    assert_eq!(
        std::fs::read_dir(copy.parent().unwrap()).unwrap().count(),
        1
    );
}

#[test]
fn a_running_workspace_is_told_how_to_restart_on_the_release() {
    let fake = Fake::start();
    fake.publish("v9.9.9", &stub("9.9.9"), false);
    let server = TestServer::start();
    let pid = server.pid();
    let dir = TempDir::new("sbu");
    let copy = own_copy(dir.path());
    // No terminal to answer the question: it isn't asked, nothing restarts,
    // and it says how.
    let out = command(&copy, &fake, dir.path())
        .env("SWITCHBOARD", server.socket())
        .args(["update", "--force"])
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains(
            "The workspace runs 9.9.9 once its server restarts on it: `switchboard upgrade`"
        ),
        "{said}"
    );
    assert!(!said.contains("[Y/n]"), "{said}");
    assert_eq!(server.pid(), pid);
    assert!(!server.log().contains("restarting on"), "{}", server.log());
}

/// A state directory of the test process's own: `install_release` runs in
/// this process, not in a `switchboard` given a home of its own, and counts
/// its downloads in the request audit, which would otherwise be yours.
fn own_state() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("sbu-state-{}", std::process::id()));
        // SAFETY: set once, before this test reads anything of the
        // environment through the library, and only the tests here run in
        // this process.
        unsafe { std::env::set_var("SWITCHBOARD_STATE_DIR", dir) };
    });
}

#[test]
fn a_download_whose_checksum_is_wrong_changes_nothing() {
    own_state();
    let fake = Fake::start();
    let json = fake.publish("v9.9.9", &stub("9.9.9"), true);
    let dir = TempDir::new("sbu");
    let target = dir.path().join("switchboard");
    let old = stub("0.1.0");
    std::fs::write(&target, &old).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
    let chosen = Release::from_json(&json).unwrap();
    let this = release::this_target().unwrap();
    let err = update::install_release(&chosen, this, &target).unwrap_err();
    assert!(
        err.to_string()
            .contains("isn't the file that was published"),
        "{err}"
    );
    assert_eq!(std::fs::read(&target).unwrap(), old);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);

    // An archive that isn't there: the same.
    let mut gone = chosen.clone();
    for asset in &mut gone.assets {
        asset.url = format!("{}/missing/{}", fake.url, asset.name);
    }
    assert!(update::install_release(&gone, this, &target).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), old);

    // And the right one goes in.
    fake.publish("v9.9.9", &stub("9.9.9"), false);
    let said = update::install_release(&chosen, this, &target).unwrap();
    assert_eq!(said, "9.9.9 (stub)");
    assert_eq!(std::fs::read(&target).unwrap(), stub("9.9.9"));
}

fn wait_until(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
    let deadline = common::deadline(timeout);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn the_server_offers_a_release_hourly_until_answered_and_again_for_a_newer_one() {
    let fake = Fake::start();
    fake.publish("v9.9.9", &stub("9.9.9"), false);
    let server = TestServer::start_with_env(&[
        ("SWITCHBOARD_GITHUB_API", &fake.url),
        ("SWITCHBOARD_UPDATES", "on"),
        ("GH_TOKEN", ""),
        ("GITHUB_TOKEN", ""),
    ]);
    let list = Fake::list_path();
    let offered = server.state.join("update-offered");
    let checked = server.state.join("update-checked");

    // The first look (the server's first tick, with no look recorded, or
    // the terminal attaching, whichever starts it; the other finds it under
    // way) offers it on the terminal, its notes beneath.
    let mut client = server.attach(100, 32);
    let screen = client.wait_text("Update now, to 9.9.9");
    assert!(screen.iter().any(|l| l.contains("Not now")));
    assert!(
        screen
            .iter()
            .any(|l| l.contains("• Everything in 9.9.9 (abc1234)")),
        "{}",
        screen.join("\n")
    );
    assert_eq!(fake.asked(&list), 1);

    // Esc is "Not now", an answer.
    client.send(b"\x1b");
    wait_until("the answer recorded", Duration::from_secs(5), || {
        std::fs::read_to_string(&offered).is_ok_and(|t| t.trim() == "v9.9.9")
    });
    client.wait_for("the box closed", |s| {
        !s.iter().any(|l| l.contains("Update now"))
    });

    // Within the hour, the ticks don't look again.
    thread::sleep(Duration::from_secs(5));
    assert_eq!(fake.asked(&list), 1);

    // The hour up, it looks, and says nothing of the release answered.
    let hour_ago = switchboard::util::now_secs() - update::CHECK_EVERY - 1;
    std::fs::write(&checked, format!("{hour_ago}\n")).unwrap();
    wait_until("the hourly look", Duration::from_secs(10), || {
        fake.asked(&list) == 2
    });
    thread::sleep(Duration::from_millis(500));
    assert!(
        !client.screen().iter().any(|l| l.contains("Update now")),
        "the answered release was offered again"
    );
    // It recorded when it looked.
    let at: i64 = std::fs::read_to_string(&checked)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(at > hour_ago);

    // A newer release, the next hour, is offered.
    fake.publish("v9.9.10", &stub("9.9.10"), false);
    std::fs::write(&checked, format!("{hour_ago}\n")).unwrap();
    client.wait_text("Update now, to 9.9.10");
    assert_eq!(fake.asked(&list), 3);
}

/// A gh in `home`'s stand-ins signed in as `gho_stand_in`, writing a line
/// to the file it gives each time it is asked for its token.
fn gh_signed_in(home: &Path) -> PathBuf {
    let runs = home.join("gh-runs");
    stand_in::Gh::new()
        .answer(&format!(
            "printf 'run\\n' >> {}\nprintf 'gho_stand_in\\n'",
            stand_in::sh_quote(&runs.to_string_lossy())
        ))
        .install(&home.join("stand-ins"));
    runs
}

#[test]
fn the_releases_are_read_with_ghs_token_and_without_it_when_refused_spent_or_off() {
    let check = |fake: &Fake, home: &Path| {
        let out = command(&own_copy(home), fake, home)
            .args(["update", "--check"])
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            out.status.success(),
            "{said}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(said.contains("Switchboard 9.9.9 is out"), "{said}");
    };
    let list = Fake::list_path();
    let bearer = Some("Bearer gho_stand_in".to_string());

    // gh signed in: the look signs in as gh, so it counts against your own
    // rate limit rather than the 60 an hour your address shares.
    let fake = Fake::start_knowing(Some("gho_stand_in"));
    fake.publish("v9.9.9", &stub("9.9.9"), false);
    let home = TempDir::new("sbu-gh");
    gh_signed_in(home.path());
    check(&fake, home.path());
    assert_eq!(fake.signed(&list), vec![bearer.clone()]);

    // A token GitHub refuses is no reason to miss a public release: asked
    // again at once without it.
    let mut fake = Fake::start_knowing(None);
    fake.http.expect_refusals();
    fake.publish("v9.9.9", &stub("9.9.9"), false);
    let home = TempDir::new("sbu-refused");
    gh_signed_in(home.path());
    check(&fake, home.path());
    assert_eq!(fake.signed(&list), vec![bearer.clone(), None]);

    // Nor is your account's rate limit, spent by gh elsewhere: the address's
    // requests are asked, as they were before gh's token was sent.
    let fake = Fake::start_knowing(Some("gho_stand_in"));
    fake.spent.store(true, Ordering::Relaxed);
    fake.publish("v9.9.9", &stub("9.9.9"), false);
    let home = TempDir::new("sbu-spent");
    gh_signed_in(home.path());
    check(&fake, home.path());
    assert_eq!(fake.signed(&list), vec![bearer, None]);

    // With the github setting off, gh isn't asked and no token goes.
    let fake = Fake::start_knowing(Some("gho_stand_in"));
    fake.publish("v9.9.9", &stub("9.9.9"), false);
    let home = TempDir::new("sbu-off");
    let runs = gh_signed_in(home.path());
    let settings = home.path().join("settings.json");
    common::write_settings_to(&settings, json!({ "github": false }));
    let out = command(&own_copy(home.path()), &fake, home.path())
        .env("SWITCHBOARD_SETTINGS", &settings)
        .args(["update", "--check"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(fake.signed(&list), vec![None]);
    assert!(!runs.exists(), "gh was asked for its token");
}

#[test]
fn the_server_doesnt_look_with_the_check_off() {
    let fake = Fake::start();
    fake.publish("v9.9.9", &stub("9.9.9"), false);
    // The setting is in place before the server starts, since its first
    // tick would look.
    let settings = TempDir::new("sbs");
    let file = settings.path().join("settings.json");
    common::write_settings_to(&file, serde_json::json!({"update_check": "off"}));
    let server = TestServer::start_with_env(&[
        ("SWITCHBOARD_GITHUB_API", &fake.url),
        ("SWITCHBOARD_UPDATES", "on"),
        ("SWITCHBOARD_SETTINGS", file.to_str().unwrap()),
    ]);
    let mut client = server.attach(100, 32);
    thread::sleep(Duration::from_secs(3));
    assert_eq!(fake.asked(&Fake::list_path()), 0);
    assert!(!client.screen().iter().any(|l| l.contains("Update now")));
}

// ── install.sh ──────────────────────────────────────────────────────────────

/// A folder of stand-ins for `uname` (saying `os` and `arch`) and, on a
/// Mac, `sysctl` (saying whether the shell runs under Rosetta), each as
/// strict as the real one about how it is called.
fn stubbed_platform(dir: &Path, os: &str, arch: &str, translated: &str) -> PathBuf {
    let stubs = dir.join("stubs");
    stand_in::platform(&stubs, os, arch, translated);
    stubs
}

fn with_stubs(stubs: &Path) -> String {
    format!(
        "{}:{}",
        stubs.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

#[test]
fn install_sh_picks_the_archive_for_each_platform() {
    for (os, arch, translated, want) in [
        ("Linux", "x86_64", "", Some("x86_64-unknown-linux-gnu")),
        ("Linux", "amd64", "", Some("x86_64-unknown-linux-gnu")),
        ("Darwin", "arm64", "", Some("aarch64-apple-darwin")),
        ("Darwin", "x86_64", "0", Some("x86_64-apple-darwin")),
        // A shell under Rosetta on Apple silicon gets the native build.
        ("Darwin", "x86_64", "1", Some("aarch64-apple-darwin")),
        ("Linux", "aarch64", "", None),
        ("FreeBSD", "amd64", "", None),
    ] {
        let dir = TempDir::new("sbi");
        let stubs = stubbed_platform(dir.path(), os, arch, translated);
        let out = Command::new("sh")
            .args([INSTALL_SH, "--target"])
            .env("PATH", with_stubs(&stubs))
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&out.stdout).trim().to_string();
        match want {
            Some(target) => {
                assert!(out.status.success(), "{os} {arch}");
                assert_eq!(said, target, "{os} {arch} {translated}");
            }
            None => {
                assert!(!out.status.success(), "{os} {arch} gave {said}");
                assert!(
                    String::from_utf8_lossy(&out.stderr).contains("no release"),
                    "{os} {arch}"
                );
            }
        }
        stand_in::assert_none_refused(&stubs);
    }
}

fn has(program: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {program}")])
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn run_install(fake: &Fake, dir: &Path, stubs: &Path) -> std::process::Output {
    let script = std::fs::read(INSTALL_SH).unwrap();
    // Piped in, as `curl … | sh` runs it.
    let mut child = Command::new("sh")
        .env("HOME", dir)
        .env("PATH", with_stubs(stubs))
        .env("SHELL", "/bin/zsh")
        .env("SWITCHBOARD_GITHUB_API", &fake.url)
        .env("SB_MARK", dir.join("mark"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // sh reads the script as it runs it, so one that stops early (the
    // failure a test looks for) may close the pipe before all is written.
    if let Err(e) = child.stdin.take().unwrap().write_all(&script) {
        assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe, "{e}");
    }
    child.wait_with_output().unwrap()
}

#[test]
fn install_sh_installs_a_checked_release_and_runs_its_setup() {
    if !(has("curl") || has("wget")) || !(has("sha256sum") || has("shasum")) {
        eprintln!("skipped: no curl or wget, or no sha256sum or shasum");
        return;
    }
    let fake = Fake::start();
    let dir = TempDir::new("sbi");
    let stubs = stubbed_platform(dir.path(), "Linux", "x86_64", "");

    // A checksum that doesn't match installs nothing.
    fake.publish("v9.9.9", &stub("9.9.9"), true);
    let out = run_install(&fake, dir.path(), &stubs);
    assert!(
        !out.status.success(),
        "installed a mismatched archive:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("isn't the file that was published"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!dir.path().join(".local/bin/switchboard").exists());

    fake.publish("v9.9.9", &stub("9.9.9"), false);
    let out = run_install(&fake, dir.path(), &stubs);
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{said}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let installed = dir.path().join(".local/bin/switchboard");
    assert_eq!(std::fs::read(&installed).unwrap(), stub("9.9.9"));
    assert!(
        said.contains("Installed switchboard 9.9.9 (stub)"),
        "{said}"
    );
    // ~/.local/bin isn't on the stubbed PATH, so it says how to add it.
    assert!(said.contains("isn't on your PATH"), "{said}");
    assert!(said.contains("~/.zshrc"), "{said}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("mark")).unwrap(),
        "setup setup\n"
    );
    assert_eq!(fake.asked(&format!("{}/latest", Fake::list_path())), 2);
    stand_in::assert_none_refused(&stubs);
    stand_in::assert_none_refused(&dir.path().join(".local/bin"));
}
