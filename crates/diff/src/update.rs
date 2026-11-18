//! Keeping `switchboard-diff` current when it runs alone: its look for a
//! newer release, once a day, and `switchboard-diff update`, which puts one
//! in the binary's place. New with the diff as an app of its own; in the
//! workspace neither runs, Switchboard's own update (src/update) covering
//! the diff built into it.
//!
//! The diff's releases are tagged `diff-v<version>` in Switchboard's
//! repository (.github/workflows/release-diff.yml), each with an archive
//! per platform, its SHA-256 and its signature. What a release is, the
//! checks and the swap are the plugin API's `release`, the same code
//! `switchboard update` runs: an archive is refused unless its checksum
//! matches and it is signed with a release key compiled in, and the binary
//! in it is run once before it takes the old one's place.
//!
//! The look runs off the screen's thread after the diff has drawn, at most
//! once a day: what it found is kept in the diff's cache (`update.json`),
//! so a day's runs make one request (a run closed before GitHub answered
//! holds the next look off for an hour, not the day). It sends GitHub a request for the
//! repository's releases and nothing of yours. It runs only in a binary
//! built by the release workflow (`SWITCHBOARD_RELEASE` set as it builds),
//! since git and cargo keep one built from a clone current;
//! `SWITCHBOARD_UPDATES=on` (or `off`) says otherwise, which is how the
//! tests turn it on. The `diff_update_check` setting turns it off.

use std::path::PathBuf;
use std::process::ExitCode;

use serde::{Deserialize, Serialize};
use switchboard_plugin_api::release::{self, Release, Version, fetch};

use crate::{UPDATE_CHECK, host, paths, requests, settings, util};

/// The app's name, which its archives and the binary in them carry, and
/// what its releases' tags start with.
pub const APP: &str = "switchboard-diff";
pub const TAG: &str = "diff-v";

/// How long a look's answer is kept, in seconds, and how long a look with
/// no answer yet (a run closed while GitHub was asked) holds the next off.
const DAY: i64 = 86_400;
const HOUR: i64 = 3600;

/// This binary's version.
pub fn current() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo.toml's version is a version")
}

/// Whether this binary was built by the release workflow.
pub fn is_release_build() -> bool {
    option_env!("SWITCHBOARD_RELEASE").is_some_and(|v| !v.is_empty() && v != "0")
}

/// Whether the diff looks for releases by itself, as the module's comment
/// says.
fn looks_by_itself() -> bool {
    match std::env::var("SWITCHBOARD_UPDATES").as_deref() {
        Ok("on") => true,
        Ok("off") => false,
        _ => is_release_build(),
    }
}

/// Whether the look runs: alone, in a build that looks, with its setting
/// on and Switchboard's own update check not off.
pub fn allowed() -> bool {
    host::is_alone() && looks_by_itself() && turned_on()
}

/// Whether its setting leaves the look on. Off, a release found before
/// is no longer said.
pub fn turned_on() -> bool {
    settings::is_on(UPDATE_CHECK)
}

/// The user agent, which GitHub refuses a request without.
fn agent() -> String {
    format!(
        "switchboard-diff/{} (+https://github.com/{})",
        env!("CARGO_PKG_VERSION"),
        release::REPO
    )
}

/// A release newer than this binary: its version, and when it was
/// published, in seconds since the epoch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Newer {
    pub version: String,
    pub published: Option<i64>,
}

impl Newer {
    /// The dim line that says it: `0.2.0 is out · switchboard-diff update`.
    pub fn line(&self) -> String {
        format!("{} is out · {APP} update", self.version)
    }
}

/// What the last look found, and when it looked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Looked {
    #[serde(default)]
    checked: i64,
    #[serde(default)]
    newer: Option<Newer>,
    /// GitHub was asked and the answer hasn't come: the run may have been
    /// closed first, so the look is tried again sooner than a day.
    #[serde(default)]
    pending: bool,
}

fn cache_file() -> PathBuf {
    paths::cache_dir().join("update.json")
}

fn kept() -> Option<Looked> {
    serde_json::from_str(&util::read_text(cache_file())?).ok()
}

/// A release kept from an earlier look, while it is still newer than this
/// binary (one since installed is not).
fn still_newer(newer: Option<Newer>) -> Option<Newer> {
    // Newer than what is in place, an update since this run included; a
    // binary replaced since this run started by other means (install.sh)
    // is of a version unknown here, so nothing is offered over it.
    if replaced_since_start() && installed_version().is_none() {
        return None;
    }
    let have = installed_version().unwrap_or_else(current);
    newer.filter(|n| Version::parse(&n.version).is_some_and(|v| v > have))
}

/// The newest release with an archive for this computer, when it is newer
/// than this binary.
fn newest(releases: &[Release]) -> Option<&Release> {
    if replaced_since_start() && installed_version().is_none() {
        return None;
    }
    let have = installed_version().unwrap_or_else(current);
    latest(releases).filter(|r| r.version > have)
}

/// The newest release with an archive for this computer, whatever version
/// this binary is: what the day's look keeps, which each copy of the diff
/// under the same home then compares with its own.
fn latest(releases: &[Release]) -> Option<&Release> {
    let target = release::this_target()?;
    release::newer_than(releases, &Version::parse("0.0.0")?)
        .into_iter()
        .find(|r| r.files_of(APP, target).is_some())
}

/// The look: the release newer than this binary, from what the last look
/// kept while that is under a day old, else asked of GitHub and kept. A
/// look that fails is kept too, so a computer off the network asks once a
/// day, not on every run.
pub fn look(now: i64) -> Option<Newer> {
    if let Some(looked) = kept()
        && now >= looked.checked
        && now - looked.checked < if looked.pending { HOUR } else { DAY }
    {
        return still_newer(looked.newer);
    }
    // The look is kept before GitHub is asked, as one with no answer yet,
    // so runs closed while the request is out ask at most once an hour.
    let keep = |newer: Option<Newer>, pending: bool| {
        let looked = Looked {
            checked: now,
            newer,
            pending,
        };
        let _ = util::make_dirs(paths::cache_dir());
        let _ = util::write_atomic(cache_file(), format!("{}\n", util::json_compact(&looked)));
    };
    keep(None, true);
    requests::count("GitHub", "REST", "switchboard-diff: update check");
    let newer = fetch::list(TAG, &agent()).ok().and_then(|releases| {
        latest(&releases).map(|r| Newer {
            version: r.version.to_string(),
            published: r.published,
        })
    });
    keep(newer.clone(), false);
    still_newer(newer)
}

/// The version the last `update` put in place, kept in the diff's state.
fn installed_file() -> PathBuf {
    paths::state_dir().join("installed-version")
}

/// The binary's file as this run found it as it started (its size and
/// modified time), so an update can tell one put in its place since by
/// any means, `install.sh` included.
static AT_START: std::sync::OnceLock<Option<(u64, std::time::SystemTime)>> =
    std::sync::OnceLock::new();

fn stamp(binary: &std::path::Path) -> Option<(u64, std::time::SystemTime)> {
    let meta = std::fs::metadata(binary).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

/// Whether this run's binary file was replaced since it started.
pub fn replaced_since_start() -> bool {
    let Some(binary) = this_binary() else {
        return false;
    };
    AT_START
        .get()
        .and_then(|s| *s)
        .is_some_and(|start| stamp(&binary) != Some(start))
}

/// Notes the binary's file as this run starts.
pub fn note_start() {
    AT_START.get_or_init(|| this_binary().and_then(|b| stamp(&b)));
}

/// This run's binary, its links followed: the file an update replaces.
fn this_binary() -> Option<PathBuf> {
    let exe = util::exe().ok()?;
    Some(std::fs::canonicalize(&exe).unwrap_or(exe))
}

/// A binary's file as an update left it: its path, size, modified time
/// and version, apart by tabs, a line for each copy of the diff updated
/// under this home.
fn record_of(binary: &std::path::Path, version: &Version) -> Option<String> {
    let (len, modified) = stamp(binary)?;
    let at = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(format!("{}\t{len}\t{at}\t{version}", binary.display()))
}

/// The version an update put at `binary`, when its line is of that very
/// file as it is now (one replaced since by other means says nothing).
fn version_at(binary: &std::path::Path) -> Option<Version> {
    let kept = util::read_text(installed_file())?;
    let path = format!("{}\t", binary.display());
    let line = kept.lines().find(|l| l.starts_with(&path))?;
    let version = Version::parse(line.rsplit('\t').next()?)?;
    (record_of(binary, &version)? == line).then_some(version)
}

/// Keeps what an update put at `binary`, the other copies' lines kept.
fn keep_version_at(binary: &std::path::Path, version: &Version) {
    let Some(record) = record_of(binary, version) else {
        return;
    };
    let path = format!("{}\t", binary.display());
    let kept = util::read_text(installed_file()).unwrap_or_default();
    let mut lines: Vec<&str> = kept.lines().filter(|l| !l.starts_with(&path)).collect();
    lines.push(&record);
    let _ = util::make_dirs(paths::state_dir());
    let _ = util::write_atomic(installed_file(), format!("{}\n", lines.join("\n")));
}

/// The version in this run's binary's place, as the last update of that
/// file kept it, when it is newer than this run's.
pub fn installed_version() -> Option<Version> {
    version_at(&this_binary()?).filter(|v| *v > current())
}

/// What a binary built from a clone says rather than replace itself.
fn from_source_text(version: &Version) -> String {
    format!(
        "{APP} {version} is out. This {APP} was built from a clone, so git and cargo keep it \
         current (cargo build --release -p {APP}); `{APP} update --force` puts the release in \
         its place."
    )
}

/// What came of asking for an update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing newer than this binary.
    UpToDate,
    /// A release is out and wasn't installed: only asked about (`--check`),
    /// or this binary was built from a clone. What to say.
    Out(String),
    /// The release is in the binary's place: the version it said.
    Installed(String),
    /// The binary was replaced since this run started (from another
    /// terminal, or install.sh), so nothing is put over it.
    Replaced,
    /// The release asked for is no longer offered.
    Withdrawn(String),
}

/// Looks for the newest release and puts it in this binary's place:
/// downloaded, refused unless its checksum matches and it is signed with a
/// release key, run once, then swapped in. With `check`, only says what is
/// out; a binary built from a clone is replaced only with `force`.
/// With `want`, the release of that version (the one a row offered) rather
/// than whichever is newest by now. `say` is told each step. Whatever
/// fails leaves the binary as it was.
pub fn update(
    check: bool,
    force: bool,
    want: Option<&str>,
    say: &dyn Fn(&str),
) -> Result<Outcome, String> {
    let target = release::this_target()
        .ok_or_else(|| format!("there are no releases of {APP} for this computer"))?;
    say(&format!(
        "Looking for a newer {APP} on github.com/{}…",
        release::REPO
    ));
    requests::count("GitHub", "REST", "switchboard-diff update");
    let releases = fetch::list(TAG, &agent()).map_err(|e| format!("{e:#}"))?;
    let wanted = want.and_then(|version| {
        let version = Version::parse(version)?;
        // With this computer's archive, its checksum and its signature:
        // one that lost any is withdrawn for this computer.
        releases.iter().find(|r| {
            r.version == version && r.version > current() && r.files_of(APP, target).is_some()
        })
    });
    // A version asked for that the list no longer holds is said, never
    // answered with another release.
    if let (Some(version), None) = (want, wanted) {
        // What the day's look kept named it: looked for anew next time.
        let _ = std::fs::remove_file(cache_file());
        return Ok(Outcome::Withdrawn(version.to_string()));
    }
    let Some(chosen) = wanted.or_else(|| newest(&releases)) else {
        return Ok(Outcome::UpToDate);
    };
    if check {
        return Ok(Outcome::Out(format!(
            "{APP} {} is out; you have {}. `{APP} update` installs it.",
            chosen.version,
            current()
        )));
    }
    if !is_release_build() && !force {
        return Ok(Outcome::Out(from_source_text(&chosen.version)));
    }
    let files = chosen.files_of(APP, target).ok_or_else(|| {
        format!(
            "release {} has no archive for {target} with its checksum and signature",
            chosen.tag
        )
    })?;
    let name = &files.archive.name;
    say(&format!("Downloading {name}…"));
    let get = |asset| {
        requests::count("GitHub", "REST", "switchboard-diff update");
        fetch::file(asset, &agent()).map_err(|e| format!("{e:#}"))
    };
    let (bytes, sum, signature) = (
        get(files.archive)?,
        get(files.checksum)?,
        get(files.signature)?,
    );
    let checked = release::verify(&bytes, &String::from_utf8_lossy(&sum), name).and_then(|()| {
        release::verify_signature(&bytes, &String::from_utf8_lossy(&signature), name)
    });
    checked.map_err(|e| format!("{e:#}"))?;
    say("Checked its checksum, and its signature by Switchboard's release key.");
    let new = release::binary_named(&bytes, APP).map_err(|e| format!("{e:#}"))?;
    let exe = util::exe().map_err(|e| format!("couldn't find the running {APP}: {e}"))?;
    let binary = std::fs::canonicalize(&exe).unwrap_or(exe);
    // The binary in place may be newer than this run of it (updated from
    // another terminal since), as the last update kept, read rather than
    // run: never put an older release over it.
    if version_at(&binary).is_some_and(|v| v >= chosen.version) {
        return Ok(Outcome::UpToDate);
    }
    // Replaced since this run started, by an update or install.sh, with
    // no record saying it is older: never put this run's offer over it.
    let replaced = replaced_since_start();
    if replaced && version_at(&binary).is_none_or(|v| v >= chosen.version) {
        return Ok(Outcome::Replaced);
    }
    let version = chosen.version.to_string();
    let said =
        release::replace_as(&binary, &new, APP, Some(&version)).map_err(|e| format!("{e:#}"))?;
    // What is in place now is kept for the next update to read; the day's
    // look stays, its release compared with this one (`still_newer`).
    keep_version_at(&binary, &chosen.version);
    say(&format!(
        "Updated {} to {APP} {said}.",
        util::tilde(&binary)
    ));
    Ok(Outcome::Installed(said))
}

fn usage() -> ExitCode {
    eprintln!(
        "Usage: {APP} update            install the newest release of {APP}\n\
         \x20      {APP} update --check    say whether there is a newer release, changing nothing\n\
         \x20      {APP} update --force    install it in place of a {APP} built from a clone\n\
         \n\
         Releases come from github.com/{}, tagged {TAG}<version>.",
        release::REPO
    );
    ExitCode::from(2)
}

/// `switchboard-diff update [--check] [--force]`.
pub fn main(args: &[String]) -> ExitCode {
    let (mut check, mut force) = (false, false);
    for arg in args {
        match arg.as_str() {
            "--check" => check = true,
            "--force" => force = true,
            "-h" | "--help" => {
                usage();
                return ExitCode::SUCCESS;
            }
            _ => return usage(),
        }
    }
    match update(check, force, None, &|step| println!("{step}")) {
        Ok(Outcome::UpToDate) => {
            println!("{APP} {} is up to date.", current());
            ExitCode::SUCCESS
        }
        Ok(Outcome::Out(text)) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Ok(Outcome::Installed(_)) => ExitCode::SUCCESS,
        Ok(Outcome::Withdrawn(version)) => {
            eprintln!("{APP} update: {APP} {version} is no longer offered");
            ExitCode::FAILURE
        }
        Ok(Outcome::Replaced) => {
            println!("{APP} was replaced since this one started; nothing was changed.");
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("{APP} update: {why}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_kept_counts_only_while_it_is_newer_than_this_binary() {
        let newer = |version: &str| {
            Some(Newer {
                version: version.into(),
                published: None,
            })
        };
        assert_eq!(still_newer(newer("999.0.0")), newer("999.0.0"));
        assert_eq!(still_newer(newer("0.0.1")), None);
        assert_eq!(still_newer(newer("not a version")), None);
        assert_eq!(still_newer(None), None);
        assert_eq!(
            newer("0.2.0").unwrap().line(),
            "0.2.0 is out · switchboard-diff update"
        );
    }

    #[test]
    fn in_the_workspace_or_a_build_from_a_clone_nothing_is_looked_for() {
        // The unit tests' host is no `switchboard-diff` run alone.
        assert!(!allowed());
    }
}
