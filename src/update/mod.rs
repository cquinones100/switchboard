//! Keeping Switchboard current: `switchboard update`, and the server's look
//! for a new release, hourly and as a terminal attaches, offering it in a
//! box. Ported from tmux/repo-update.sh, which fetched `~/switchboard`'s
//! origin and fast-forwarded main; with no clone any more, what it offers
//! is a release (`release`), downloaded, checked and put in place of the
//! binary (`install`), then `switchboard setup` run for what the new
//! version brings, as repo-update.sh ran the guided setup.sh after a pull.
//! The workspace's server goes on with the build it started with, so a
//! running one is then offered a restart on the new one in place, keeping
//! every pane (`switchboard upgrade`, src/mux/upgrade.rs), while Settings'
//! "Offer to restart after an update" (`upgrade_after_update`) is on.
//!
//! As before, the look runs in the background and never on a key press; it
//! records when it last looked (`update-checked`) and which release you
//! answered (`update-offered`) in the state directory, so the hourly look
//! asks again each hour until the box is answered, "Update now" or "Not
//! now", and then not until a newer release comes out, while a look as a
//! terminal attaches asks whenever there is something newer than this
//! binary. The update_check setting chooses which looks run: both
//! ("hourly"), only on attaching ("open"), or none ("off"); M-p's "Update
//! Switchboard" looks whatever it says.
//!
//! The server looks by itself only in a binary built by the release
//! workflow (`SWITCHBOARD_RELEASE` set as it builds), so a binary built from
//! a clone with cargo, which git and cargo keep current, isn't offered a
//! release in its place; `SWITCHBOARD_UPDATES=on` (or `off`) in the
//! server's environment says otherwise, which is how the tests turn it on.
//!
//! - `release`: the releases on GitHub, versions, and which archive is this
//!   computer's.
//! - `install`: the checksum, the archive unpacked, and the swap.
//! - `serve`: the server's side: the looks, the box, and M-p's commands.

pub mod install;
pub mod release;
mod serve;

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Result, anyhow};

pub use release::{REPO, Release, Version};
pub use serve::{Command, UpdateEvent, Updates};

use crate::{paths, util};

/// This binary's version and the commit it was built from, as `switchboard
/// --version` prints them after its name: `0.2.0 (1a2b3c4d5e6f)`.
pub fn version_line() -> String {
    format!(
        "{} ({})",
        env!("CARGO_PKG_VERSION"),
        env!("SWITCHBOARD_COMMIT")
    )
}

/// Whether this binary was built by the release workflow.
pub fn is_release_build() -> bool {
    option_env!("SWITCHBOARD_RELEASE").is_some_and(|v| !v.is_empty() && v != "0")
}

/// Whether the server looks for releases by itself, as the module's comment
/// says.
pub fn looks_by_itself() -> bool {
    match std::env::var("SWITCHBOARD_UPDATES").as_deref() {
        Ok("on") => true,
        Ok("off") => false,
        _ => is_release_build(),
    }
}

// ── When to look, and what to say ───────────────────────────────────────────

/// How often the hourly look looks, in seconds (repo-update.sh's CHECK_EVERY).
pub const CHECK_EVERY: i64 = 3600;

/// What started a look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// A terminal attached (tmux's `client-attached` running `check`).
    Attach,
    /// The hour came round (health.sh starting `periodic`).
    Hourly,
    /// Asked for, from M-p (`now`).
    Asked,
}

/// Whether the update_check setting lets a look of this kind run: "off"
/// lets none but one asked for, "open" leaves out the hourly one.
pub fn allowed(why: Why, setting: &str) -> bool {
    match why {
        Why::Asked => true,
        Why::Attach => setting != "off",
        Why::Hourly => setting != "off" && setting != "open",
    }
}

/// Whether the hour is up since the last look (`None` for never).
pub fn hour_up(last: Option<i64>, now: i64) -> bool {
    last.is_none_or(|last| now - last >= CHECK_EVERY || now < last)
}

/// What a look comes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Nothing to show.
    Quiet,
    /// Asked for, and there is nothing newer.
    UpToDate,
    /// The releases newer than this binary, newest first, to offer.
    Offer(Vec<Release>),
}

/// What a look that found `releases` shows: the newer ones, unless the
/// hourly look finds the newest already answered (`offered`); "up to date"
/// only to a look asked for. A release with no archive for this computer
/// counts as not there, since it can't be installed here.
pub fn decide(
    why: Why,
    releases: &[Release],
    current: &Version,
    target: Option<&str>,
    offered: Option<&str>,
) -> Decision {
    let newer: Vec<Release> = release::newer_than(releases, current)
        .into_iter()
        .filter(|r| target.is_some_and(|t| r.files_for(t).is_some()))
        .cloned()
        .collect();
    match newer.first() {
        None if why == Why::Asked => Decision::UpToDate,
        None => Decision::Quiet,
        Some(newest) if why == Why::Hourly && offered == Some(newest.tag.as_str()) => {
            Decision::Quiet
        }
        Some(_) => Decision::Offer(newer),
    }
}

/// When the last look was, in seconds since the epoch.
pub fn checked_file() -> PathBuf {
    paths::state_dir().join("update-checked")
}

/// The release last answered, by its tag.
pub fn offered_file() -> PathBuf {
    paths::state_dir().join("update-offered")
}

pub fn last_checked() -> Option<i64> {
    util::read_text(checked_file()).and_then(|t| t.trim().parse().ok())
}

pub fn mark_checked(now: i64) {
    let _ = std::fs::create_dir_all(paths::state_dir());
    let _ = util::write_atomic(checked_file(), format!("{now}\n"));
}

pub fn last_offered() -> Option<String> {
    util::read_text(offered_file())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

pub fn mark_offered(tag: &str) {
    let _ = std::fs::create_dir_all(paths::state_dir());
    let _ = util::write_atomic(offered_file(), format!("{tag}\n"));
}

/// The box's text: what is out, and each newer release's notes, newest
/// first, as repo-update.sh listed the new commits with their ages.
pub fn offer_text(newer: &[Release], current: &Version, now_secs: i64) -> String {
    let Some(newest) = newer.first() else {
        return String::new();
    };
    let mut text = format!("Switchboard {} is out; you have {current}.", newest.version);
    if newer.len() > 1 {
        text.push_str(&format!(" {} new releases, newest first:", newer.len()));
    }
    for r in newer {
        let ago = crate::sidebar::text::ago(r.published, now_secs);
        text.push_str("\n\n");
        text.push_str(&r.version.to_string());
        if !ago.is_empty() {
            text.push_str(&format!(" · {ago} ago"));
        }
        for line in r.note_lines() {
            text.push('\n');
            text.push_str(&line);
        }
    }
    text
}

/// What a binary built from source says of a newer release rather than
/// installing it: how it is kept current instead, and how to take the
/// release all the same.
pub fn from_source_text(version: &Version) -> String {
    format!(
        "Switchboard {version} is out. This switchboard was built from source, so git and cargo \
         keep it current (git pull, cargo build --release, then switchboard setup); \
         `switchboard update --force` puts the release in its place."
    )
}

// ── switchboard update ──────────────────────────────────────────────────────

fn usage() -> ExitCode {
    eprintln!(
        "Usage: switchboard update            install the newest release, then the guided setup\n\
         \x20      switchboard update --check    say whether there is a newer release, changing nothing\n\
         \x20      switchboard update --force    install it in place of a switchboard built from source\n\
         \n\
         Releases come from github.com/{REPO}. A switchboard built from source\n\
         is replaced only with --force."
    );
    ExitCode::from(2)
}

/// `switchboard update [--check] [--force]`, and what the server's box runs
/// in a tab of its own: `--tag <tag>` installs that release (the one the box
/// offered), `--hold` waits for Enter before the tab closes, and `--setup`
/// runs the guided setup alone (M-p's "Run Switchboard's setup"). A binary
/// built from source is replaced only with `--force`, `--tag` or not, so
/// the box never swaps a cargo build for a release unasked.
pub fn main(args: Vec<String>) -> ExitCode {
    let mut check = false;
    let mut force = false;
    let mut hold = false;
    let mut setup_only = false;
    let mut tag = None;
    let mut rest = args.into_iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--check" => check = true,
            "--force" => force = true,
            "--hold" => hold = true,
            "--setup" => setup_only = true,
            "--tag" => match rest.next() {
                Some(t) => tag = Some(t),
                None => return usage(),
            },
            _ => return usage(),
        }
    }
    let code = if setup_only {
        run_setup(&exe_or_self(), "Running Switchboard's setup…")
    } else {
        match update(check, force, tag.as_deref()) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("switchboard update: {e:#}");
                ExitCode::FAILURE
            }
        }
    };
    if hold {
        wait_for_enter();
    }
    code
}

fn update(check: bool, force: bool, tag: Option<&str>) -> Result<ExitCode> {
    let current = Version::current();
    let target = release::this_target()
        .ok_or_else(|| anyhow!("there are no releases of Switchboard for this computer"))?;
    println!("Looking for a newer Switchboard on github.com/{REPO}…");
    let releases = util::block_on(release::list("switchboard update"))?;
    mark_checked(util::now_secs());
    let newer = match decide(Why::Asked, &releases, &current, Some(target), None) {
        Decision::Offer(newer) => newer,
        _ => Vec::new(),
    };
    let chosen = match tag {
        Some(tag) => releases
            .iter()
            .find(|r| r.tag == tag)
            .ok_or_else(|| anyhow!("there is no release {tag}"))?,
        None => match newer.first() {
            Some(newest) => newest,
            None => {
                println!("Switchboard {current} is up to date.");
                return Ok(ExitCode::SUCCESS);
            }
        },
    };
    if check {
        println!(
            "{}",
            offer_text(&newer, &current, util::now_secs())
                .lines()
                .map(|l| format!("  {l}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        println!("\n`switchboard update` installs it.");
        return Ok(ExitCode::SUCCESS);
    }
    if !is_release_build() && !force {
        println!("{}", from_source_text(&chosen.version));
        return Ok(ExitCode::SUCCESS);
    }
    let binary = install::installed_binary()?;
    install_release(chosen, target, &binary)?;
    mark_offered(&chosen.tag);
    println!();
    let code = run_setup(
        &binary,
        "Running the setup, for what the new version brings…",
    );
    offer_upgrade(&chosen.version);
    Ok(code)
}

/// The setting that has `switchboard update` offer the running workspace a
/// restart on the release it put in place.
pub const OFFER_UPGRADE: &str = "upgrade_after_update";

/// After a release is put in place: a workspace running asks whether to
/// restart its server on it now, keeping every tab and what runs in it
/// (`switchboard upgrade`), while `OFFER_UPGRADE` is on and there is a
/// terminal to answer; Enter is yes, a closed input no. Otherwise it says
/// how to.
fn offer_upgrade(version: &Version) {
    if std::os::unix::net::UnixStream::connect(crate::mux::socket::socket_path()).is_err() {
        return;
    }
    let how = format!(
        "The workspace runs {version} once its server restarts on it: `switchboard upgrade` \
         (M-p's \"Restart Switchboard on the new build\") does that, keeping every tab and \
         what runs in it."
    );
    if !crate::settings::is_on(OFFER_UPGRADE) || !std::io::stdin().is_terminal() {
        println!("\n{how}");
        return;
    }
    println!();
    if !crate::setup::ask(
        false,
        &format!("Restart the workspace on {version} now, keeping every tab and what runs in it?"),
    ) {
        println!("\n{how}");
        return;
    }
    let _ = crate::mux::socket::run_command(vec!["upgrade".to_string()]);
}

/// Downloads a release's archive for `target` and its checksum, checks one
/// against the other, and puts the binary in it in place of `binary`,
/// saying what it does as it goes. Whatever fails leaves `binary` as it was.
pub fn install_release(chosen: &Release, target: &str, binary: &Path) -> Result<String> {
    let (archive, checksum) = chosen.files_for(target).ok_or_else(|| {
        anyhow!(
            "release {} has no archive for {target} with its checksum",
            chosen.tag
        )
    })?;
    println!("Downloading {}…", archive.name);
    let (bytes, sum) = util::block_on(async {
        let bytes = release::download(archive, "switchboard update").await?;
        let sum = release::download(checksum, "switchboard update").await?;
        anyhow::Ok((bytes, sum))
    })?;
    install::verify(&bytes, &String::from_utf8_lossy(&sum), &archive.name)?;
    println!("Checked it against {}.", checksum.name);
    let new = install::binary_from(&bytes)?;
    let said = install::replace(binary, &new)?;
    println!("Updated {} to switchboard {said}.", paths::tilde(binary));
    Ok(said)
}

/// The guided setup, from the binary at `binary` (the new one, after an
/// update), with this terminal, `why` said first; gives how it ended.
fn run_setup(binary: &Path, why: &str) -> ExitCode {
    println!("{why}\n");
    match std::process::Command::new(binary).arg("setup").status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("switchboard update: couldn't run the setup: {e}");
            ExitCode::FAILURE
        }
    }
}

fn exe_or_self() -> PathBuf {
    util::exe().unwrap_or_else(|_| PathBuf::from("switchboard"))
}

/// Waits for Enter, so a tab that ran an update keeps what it said until
/// it is read; at once when there is no terminal to read.
fn wait_for_enter() {
    if !std::io::stdin().is_terminal() {
        return;
    }
    print!("\nPress Enter to close.");
    let _ = std::io::stdout().flush();
    let _ = std::io::stdin().lock().read_line(&mut String::new());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn release(tag: &str, notes: &str) -> Release {
        let version = Version::parse(tag).unwrap();
        let mut assets = Vec::new();
        for target in ["x86_64-unknown-linux-gnu", "aarch64-apple-darwin"] {
            let name = release::archive_name(&version, target);
            assets.push(json!({"name": name, "browser_download_url": "https://x/a"}));
            assets.push(
                json!({"name": format!("{name}.sha256"), "browser_download_url": "https://x/b"}),
            );
        }
        Release::from_json(&json!({
            "tag_name": tag, "body": notes, "assets": assets,
            "published_at": "2026-11-01T00:00:00Z"
        }))
        .unwrap()
    }

    const LINUX: Option<&str> = Some("x86_64-unknown-linux-gnu");

    #[test]
    fn the_setting_chooses_which_looks_run() {
        for (setting, attach, hourly) in [
            ("hourly", true, true),
            ("open", true, false),
            ("off", false, false),
        ] {
            assert_eq!(allowed(Why::Attach, setting), attach, "{setting}");
            assert_eq!(allowed(Why::Hourly, setting), hourly, "{setting}");
            assert!(allowed(Why::Asked, setting));
        }
    }

    #[test]
    fn the_hourly_look_waits_an_hour() {
        assert!(hour_up(None, 1000));
        assert!(!hour_up(Some(1000), 1000 + CHECK_EVERY - 1));
        assert!(hour_up(Some(1000), 1000 + CHECK_EVERY));
        // A clock put back doesn't keep it from ever looking again.
        assert!(hour_up(Some(5000), 1000));
    }

    #[test]
    fn a_release_answered_isnt_asked_about_again_until_a_newer_one() {
        let current = Version::parse("0.1.0").unwrap();
        let one = vec![release("v0.2.0", "- A")];
        assert!(matches!(
            decide(Why::Hourly, &one, &current, LINUX, None),
            Decision::Offer(_)
        ));
        assert_eq!(
            decide(Why::Hourly, &one, &current, LINUX, Some("v0.2.0")),
            Decision::Quiet
        );
        // On attaching it asks while there is something newer, answered or not.
        assert!(matches!(
            decide(Why::Attach, &one, &current, LINUX, Some("v0.2.0")),
            Decision::Offer(_)
        ));
        let two = vec![release("v0.2.0", "- A"), release("v0.3.0", "- B")];
        match decide(Why::Hourly, &two, &current, LINUX, Some("v0.2.0")) {
            Decision::Offer(newer) => {
                assert_eq!(newer[0].tag, "v0.3.0");
                assert_eq!(newer.len(), 2);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn up_to_date_is_said_only_when_asked() {
        let current = Version::parse("0.2.0").unwrap();
        let one = vec![release("v0.2.0", "")];
        assert_eq!(
            decide(Why::Asked, &one, &current, LINUX, None),
            Decision::UpToDate
        );
        assert_eq!(
            decide(Why::Attach, &one, &current, LINUX, None),
            Decision::Quiet
        );
        // A release with nothing for this computer isn't offered.
        let old = Version::parse("0.1.0").unwrap();
        assert_eq!(
            decide(Why::Attach, &one, &old, Some("x86_64-apple-darwin"), None),
            Decision::Quiet
        );
    }

    #[test]
    fn the_box_lists_each_release_newest_first() {
        let current = Version::parse("0.1.0").unwrap();
        let newer = vec![
            release(
                "v0.3.0",
                "## What changed\n\n- Faster tabs (abc1234)\n- A fix",
            ),
            release("v0.2.0", "- The first"),
        ];
        let now = 1_793_491_200 + 2 * 86_400;
        let text = offer_text(&newer, &current, now);
        assert_eq!(
            text,
            "Switchboard 0.3.0 is out; you have 0.1.0. 2 new releases, newest first:\n\n\
             0.3.0 · 2d ago\n• Faster tabs (abc1234)\n• A fix\n\n\
             0.2.0 · 2d ago\n• The first"
        );
    }

    #[test]
    fn the_version_names_its_commit() {
        let line = version_line();
        assert!(line.starts_with(env!("CARGO_PKG_VERSION")));
        assert!(line.ends_with(')') && line.contains(" ("));
    }
}
