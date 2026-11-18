//! A release of Switchboard, or of an app of its own that ships apart from
//! it (the diff, Notes): what GitHub says of one, which of its files is for
//! this computer, and putting it in the place of the running binary. Taken
//! from `switchboard`'s src/update (`release.rs`, `install.rs`) when the
//! diff got an update of its own, so both check a release one way; the
//! `release` feature holds it, since a plugin with no update needs none of
//! its crates.
//!
//! A release is a tag (`v<version>` for Switchboard, `diff-v<version>` for
//! the diff) with an archive per platform, `<app>-<version>-<target>.tar.gz`,
//! its SHA-256 beside it in `<archive>.sha256` and its signature in
//! `<archive>.sig`, as the release workflows publish them.
//!
//! An archive is checked against the SHA-256 published beside it before
//! anything is unpacked, and against its signature, which only the release
//! key makes: the checksum comes from the same release as the archive, so
//! someone able to change one could change the other, while the key's
//! public half is compiled into the binary (`RELEASE_KEYS`), install.sh
//! carrying a copy. The signature is OpenSSH's (`ssh-keygen -Y sign`,
//! namespace `switchboard-release`). The binary in it is written beside the
//! old one, in a folder of its own, and run once (`--version`) to show it
//! starts on this computer, and only then renamed over the old one, which
//! is a single step: whatever fails before it leaves the old binary exactly
//! as it was. The running binary goes on running through the rename (the
//! file it started from stays open until it ends).
//!
//! `fetch` reaches GitHub for a program with no HTTPS client of its own (the
//! diff alone), with the `net` feature: over HTTPS alone, every redirect
//! included, with plain HTTP only to this computer's own address, where the
//! tests' stand-ins answer. `switchboard` reaches it through its own client
//! (src/net.rs), which counts each request in the audit.

use std::cmp::Ordering;
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use sha2::{Digest, Sha256};
use ssh_key::{PublicKey, SshSig};

/// The repository releases come from, the one the code lives in: a version
/// bump on its main publishes one (.github/workflows/release.yml).
/// install.sh's `REPO` and the README's install line and clone URL name it
/// too, and change with it.
pub const REPO: &str = "cquinones100/switchboard";

/// Switchboard's own name, which its archives and the binary in them carry.
pub const SWITCHBOARD: &str = "switchboard";

/// A version as a tag names it (`v0.2.0`, `0.2.0`, `1.0.0-rc.1`): its three
/// numbers and what follows a `-`, compared as semver does, a version
/// with something after the `-` coming before the same one without.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: String,
}

impl Version {
    /// A version from its text, with or without a `v` before it; `None` for
    /// anything else.
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        // What follows a `+` is build metadata, which semver ignores.
        let text = text.split('+').next().unwrap_or(text);
        let (numbers, pre) = match text.split_once('-') {
            Some((n, p)) if !p.is_empty() => (n, p.to_string()),
            Some(_) => return None,
            None => (text, String::new()),
        };
        let mut parts = numbers.split('.').map(|n| {
            if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
                None
            } else {
                n.parse::<u64>().ok()
            }
        });
        let version = Version {
            major: parts.next()??,
            minor: parts.next()??,
            patch: parts.next()??,
            pre,
        };
        parts.next().is_none().then_some(version)
    }

    /// Switchboard's own version, the workspace's, which this crate takes.
    pub fn current() -> Version {
        Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo.toml's version is a version")
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Version) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => compare_pre(&self.pre, &other.pre),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Version) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Two pre-release labels compared part by part, as semver says: numbers as
/// numbers and before words, words as text, and the shorter first when one
/// starts the other.
fn compare_pre(a: &str, b: &str) -> Ordering {
    let mut a = a.split('.');
    let mut b = b.split('.');
    loop {
        match (a.next(), b.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre)?;
        }
        Ok(())
    }
}

/// One file of a release: its name and where it is downloaded from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

/// A release's files for one target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Files<'a> {
    pub archive: &'a Asset,
    /// `<archive>.sha256`.
    pub checksum: &'a Asset,
    /// `<archive>.sig`, the archive signed with the release key
    /// (`verify_signature`).
    pub signature: &'a Asset,
}

/// A published release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Its tag, as `v0.2.0`.
    pub tag: String,
    pub version: Version,
    /// Its notes, as the release workflow wrote them: a line per commit
    /// since the release before.
    pub notes: String,
    /// When it was published, in seconds since the epoch.
    pub published: Option<i64>,
    /// Its page on GitHub.
    pub page: String,
    pub assets: Vec<Asset>,
}

impl Release {
    /// A release of Switchboard from GitHub's JSON for one; `None` for a
    /// draft, a pre-release, or a tag that isn't a version.
    pub fn from_json(json: &Value) -> Option<Release> {
        Release::tagged("", json)
    }

    /// A release whose tag is `prefix` and a version (`diff-v` and `0.2.0`),
    /// from GitHub's JSON for one; `None` for a draft, a pre-release, or
    /// any other tag.
    pub fn tagged(prefix: &str, json: &Value) -> Option<Release> {
        if json["draft"].as_bool() == Some(true) || json["prerelease"].as_bool() == Some(true) {
            return None;
        }
        let tag = json["tag_name"].as_str()?.to_string();
        let version = Version::parse(tag.strip_prefix(prefix)?)?;
        let assets = json["assets"]
            .as_array()
            .map(|assets| {
                assets
                    .iter()
                    .filter_map(|a| {
                        Some(Asset {
                            name: a["name"].as_str()?.to_string(),
                            url: a["browser_download_url"].as_str()?.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let published = json["published_at"]
            .as_str()
            .and_then(|t| t.parse::<jiff::Timestamp>().ok())
            .map(|t| t.as_second());
        Some(Release {
            tag,
            version,
            notes: json["body"].as_str().unwrap_or("").trim().to_string(),
            published,
            page: json["html_url"].as_str().unwrap_or("").to_string(),
            assets,
        })
    }

    /// Switchboard's archive for a target with the checksum and signature
    /// beside it, when this release has all three: one published before
    /// releases were signed has no signature, and is never offered.
    pub fn files_for(&self, target: &str) -> Option<Files<'_>> {
        self.files_of(SWITCHBOARD, target)
    }

    /// `app`'s archive for a target, with its checksum and signature.
    pub fn files_of(&self, app: &str, target: &str) -> Option<Files<'_>> {
        let name = archive_of(app, &self.version, target);
        let named = |file: &str| self.assets.iter().find(|a| a.name == file);
        Some(Files {
            archive: named(&name)?,
            checksum: named(&format!("{name}.sha256"))?,
            signature: named(&format!("{name}.sig"))?,
        })
    }

    /// Its notes as lines to read in a box: a bullet for each of the
    /// workflow's `- ` lines, Markdown's headings and blank lines left out.
    pub fn note_lines(&self) -> Vec<String> {
        self.notes
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(
                |l| match l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")) {
                    Some(item) => format!("• {item}"),
                    None => l.to_string(),
                },
            )
            .collect()
    }
}

/// Switchboard's archive's name: `switchboard-<version>-<target>.tar.gz`.
pub fn archive_name(version: &Version, target: &str) -> String {
    archive_of(SWITCHBOARD, version, target)
}

/// An app's archive's name: `<app>-<version>-<target>.tar.gz`.
pub fn archive_of(app: &str, version: &Version, target: &str) -> String {
    format!("{app}-{version}-{target}.tar.gz")
}

/// The target a release is built for, from an operating system and
/// architecture as Rust names them (`std::env::consts::OS` and `ARCH`): the
/// three Switchboard ships for, and `None` for any other.
pub fn target_for(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        _ => None,
    }
}

/// This computer's target, as this binary was built for it.
pub fn this_target() -> Option<&'static str> {
    target_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// The releases newer than `current`, newest first.
pub fn newer_than<'a>(releases: &'a [Release], current: &Version) -> Vec<&'a Release> {
    let mut newer: Vec<&Release> = releases.iter().filter(|r| r.version > *current).collect();
    newer.sort_by(|a, b| b.version.cmp(&a.version));
    newer
}

/// Whether a release's file may be downloaded from `url`: over HTTPS, or
/// over plain HTTP from this computer's own address (the tests' and the
/// demo's stand-ins).
pub fn downloadable(url: &str) -> bool {
    let Ok(url) = url::Url::parse(url) else {
        return false;
    };
    let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost"));
    let no_credentials = url.username().is_empty() && url.password().is_none();
    no_credentials && (url.scheme() == "https" || (url.scheme() == "http" && local))
}

// ── Checking an archive ─────────────────────────────────────────────────────

/// The public halves of the keys a release may be signed with, one OpenSSH
/// key a line: the one the release workflow signs with now, and, while a key
/// is being replaced, the next. install.sh's `RELEASE_KEYS` holds the same.
/// The file stays in src/update, where the release workflows read it.
pub const RELEASE_KEYS: &str = include_str!("../../../src/update/release-signing-keys.pub");

/// What a release's signature is made for, so no signature the key makes
/// for anything else counts as one.
pub const SIGNATURE_NAMESPACE: &str = "switchboard-release";

/// A file's SHA-256, in lower-case hex, as `shasum -a 256` prints it.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The hash a `.sha256` file gives for `name`: its first word when that is
/// 64 hex digits, as `shasum -a 256 <name>` writes it (`<hash>  <name>`),
/// and the name after it, when there is one, must be the archive's own, so
/// a checksum published for another file is never taken for this one's.
pub fn expected_hash(checksum_file: &str, name: &str) -> Result<String> {
    let line = checksum_file
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .ok_or_else(|| anyhow!("{name}.sha256 is empty"))?;
    let mut words = line.split_whitespace();
    let hash = words.next().unwrap_or("").to_ascii_lowercase();
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("{name}.sha256 doesn't hold a SHA-256");
    }
    if let Some(named) = words.next() {
        // shasum marks a file read as binary with a `*` before its name.
        let named = named.trim_start_matches('*');
        if Path::new(named).file_name().and_then(|n| n.to_str()) != Some(name) {
            bail!("{name}.sha256 is the checksum of {named}, not of {name}");
        }
    }
    Ok(hash)
}

/// Refuses an archive whose SHA-256 isn't the one published beside it.
pub fn verify(archive: &[u8], checksum_file: &str, name: &str) -> Result<()> {
    let expected = expected_hash(checksum_file, name)?;
    let actual = sha256_hex(archive);
    if actual != expected {
        bail!(
            "{name} isn't the file that was published: its SHA-256 is {actual}, and the release says {expected}. Nothing was changed."
        );
    }
    Ok(())
}

/// The keys a signature may be made with: `RELEASE_KEYS`, and in a debug
/// build (the tests') `SWITCHBOARD_RELEASE_KEY`'s too, never in a release.
pub fn release_keys() -> Result<Vec<PublicKey>> {
    let mut lines: Vec<String> = RELEASE_KEYS.lines().map(str::to_string).collect();
    if cfg!(debug_assertions)
        && let Ok(key) = std::env::var("SWITCHBOARD_RELEASE_KEY")
    {
        lines.push(key);
    }
    lines
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| PublicKey::from_openssh(l).map_err(|e| anyhow!("a release key won't read: {e}")))
        .collect()
}

/// Refuses an archive unless `signature` (its `.sig`, as `ssh-keygen -Y
/// sign` writes it) is a signature of it by one of the release keys.
pub fn verify_signature(archive: &[u8], signature: &str, name: &str) -> Result<()> {
    let refused = || {
        anyhow!(
            "{name} isn't signed by Switchboard's release key, so it may not be the file that was published. Nothing was changed."
        )
    };
    let signature = SshSig::from_pem(signature.trim()).map_err(|_| refused())?;
    let signed = release_keys()?
        .iter()
        .any(|key| key.verify(SIGNATURE_NAMESPACE, archive, &signature).is_ok());
    if !signed {
        return Err(refused());
    }
    Ok(())
}

// ── Putting it in place ─────────────────────────────────────────────────────

/// The binary named `app` inside a release's archive, wherever the archive
/// keeps it (the workflow puts it in a folder named after the archive);
/// only a plain file is taken.
pub fn binary_named(archive: &[u8], app: &str) -> Result<Vec<u8>> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries().context("the archive can't be read")? {
        let mut entry = entry.context("the archive can't be read")?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let is_binary = entry
            .path()
            .ok()
            .and_then(|p| p.file_name().map(|n| n == app))
            .unwrap_or(false);
        if is_binary {
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .context("the archive can't be read")?;
            return Ok(bytes);
        }
    }
    bail!("the archive has no {app} in it")
}

/// How long the new binary may take to say its version.
const RUN_TIMEOUT: Duration = Duration::from_secs(20);

/// Puts `binary`, a build of `app`, in the place of the file at `target`,
/// as the module's comment says: written beside it, run once, then renamed
/// over it. Gives what the new binary said of its version.
pub fn replace_named(target: &Path, binary: &[u8], app: &str) -> Result<String> {
    replace_as(target, binary, app, None)
}

/// `replace_named`, refusing before the rename a binary that says another
/// version than `expect` (the release it was downloaded as).
pub fn replace_as(target: &Path, binary: &[u8], app: &str, expect: Option<&str>) -> Result<String> {
    let dir = target
        .parent()
        .ok_or_else(|| anyhow!("{} has no folder", target.display()))?;
    let name = target
        .file_name()
        .ok_or_else(|| anyhow!("{} has no name", target.display()))?
        .to_string_lossy();
    // A folder of its own beside the old binary, on the same disk so the
    // rename is one step, holding it under the app's own name: the binary
    // takes a name that isn't a command's as `switchboard`, but a release
    // from before it did read `.switchboard.new-<pid>` as a command it lacks.
    let folder = dir.join(format!(".{name}.new-{}", std::process::id()));
    let new = folder.join(app);
    // Only a folder this run made is cleaned up after: one there already
    // (another's, or a link) is never touched.
    fs::create_dir(&folder).with_context(|| format!("couldn't write in {}", dir.display()))?;
    let result = write_and_check(&new, binary, app)
        .and_then(|said| match expect {
            Some(want) if Version::parse(&said) != Version::parse(want) => {
                bail!("the archive's {app} says {said}, not {want}. Nothing was changed.")
            }
            _ => Ok(said),
        })
        .and_then(|said| {
            fs::rename(&new, target)
                .with_context(|| format!("couldn't put it in place of {}", target.display()))?;
            Ok(said)
        });
    let _ = fs::remove_file(&new);
    let _ = fs::remove_dir(&folder);
    result
}

fn write_and_check(new: &Path, binary: &[u8], app: &str) -> Result<String> {
    let dir = new.parent().unwrap_or(Path::new("."));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o755)
        .open(new)
        .with_context(|| format!("couldn't write in {}", dir.display()))?;
    file.write_all(binary)
        .and_then(|()| file.sync_all())
        .with_context(|| format!("couldn't write in {}", dir.display()))?;
    drop(file);
    fs::set_permissions(new, fs::Permissions::from_mode(0o755))?;
    says_version_of(new, app)
}

/// Runs a binary with `--version` and gives the version it says, or why it
/// won't do: one that can't start here (built for another computer), fails,
/// or says something other than `app`'s version.
pub fn says_version_of(binary: &Path, app: &str) -> Result<String> {
    let mut child = spawn_version(binary, app)?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > RUN_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!("the new {app} didn't say its version");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    let said = out.lines().next().unwrap_or("").trim().to_string();
    match said.strip_prefix(&format!("{app} ")) {
        Some(version) if status.success() => Ok(version.to_string()),
        _ => bail!("the new {app} didn't say its version"),
    }
}

// Starting a file just written can meet "text file busy" for a moment on
// Linux, while a process forked elsewhere in this one still holds it open
// for writing until it execs; it is tried again for a little while then.
fn spawn_version(binary: &Path, app: &str) -> Result<std::process::Child> {
    let started = Instant::now();
    loop {
        // Named after the app, whatever the file is called, so it runs
        // `--version` rather than look for a command by its file's name.
        let spawned = Command::new(binary)
            .arg0(app)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        match spawned {
            Err(e)
                if e.raw_os_error() == Some(libc::ETXTBSY)
                    && started.elapsed() < Duration::from_secs(2) =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            other => {
                return other
                    .with_context(|| format!("the new {app} won't start on this computer"));
            }
        }
    }
}

// ── Reaching GitHub ─────────────────────────────────────────────────────────

/// GitHub's releases and their files, for a program with no HTTPS client of
/// its own. Without the `net` feature each fails, saying the program was
/// built without network support, so no caller needs a `cfg` of its own.
pub mod fetch {
    use anyhow::{Result, anyhow};
    use serde_json::Value;

    use super::{REPO, Release, downloadable};

    /// How many releases GitHub is asked for at a time, the most pages of
    /// the list read, and how many of an app's releases are enough.
    const PER_PAGE: usize = 30;
    const PAGES_MOST: usize = 10;
    const ENOUGH: usize = 10;

    /// The largest file a release may hold.
    const LARGEST: u64 = 500 * 1024 * 1024;

    /// GitHub's REST API (`SWITCHBOARD_GITHUB_API` moves it, as it does
    /// every GitHub call of Switchboard's).
    pub fn api_url() -> String {
        match std::env::var("SWITCHBOARD_GITHUB_API") {
            Ok(url) if !url.trim().is_empty() => url.trim().trim_end_matches('/').to_string(),
            _ => "https://api.github.com".to_string(),
        }
    }

    /// The published releases tagged `prefix` and a version, newest first,
    /// from the list GitHub gives thirty at a time: the repository holds
    /// other apps' releases too, so the next page is read while fewer than
    /// enough of these have come and the list goes on. `agent` is the user
    /// agent, which GitHub refuses a request without.
    pub fn list(prefix: &str, agent: &str) -> Result<Vec<Release>> {
        let mut found = Vec::new();
        for page in 1..=PAGES_MOST {
            let url = format!(
                "{}/repos/{REPO}/releases?per_page={PER_PAGE}&page={page}",
                api_url()
            );
            // A later page failing keeps what the pages before found: the
            // newest releases are on the first.
            let asked = get(&url, agent, "application/vnd.github+json", 30);
            let (status, body) = match asked {
                Ok(answer) => answer,
                Err(_) if !found.is_empty() => break,
                Err(e) => return Err(e),
            };
            if !(200..300).contains(&status) {
                if !found.is_empty() {
                    break;
                }
                return Err(anyhow!("GitHub answered {status} for {REPO}'s releases"));
            }
            let json: Value = serde_json::from_slice(&body)
                .map_err(|_| anyhow!("GitHub's list of releases wasn't a list"))?;
            let listed = json
                .as_array()
                .ok_or_else(|| anyhow!("GitHub's list of releases wasn't a list"))?;
            found.extend(listed.iter().filter_map(|r| Release::tagged(prefix, r)));
            if found.len() >= ENOUGH || listed.len() < PER_PAGE {
                break;
            }
        }
        Ok(found)
    }

    /// A release's file, downloaded whole; an error for anything but a
    /// success.
    pub fn file(asset: &super::Asset, agent: &str) -> Result<Vec<u8>> {
        let (status, bytes) = get(&asset.url, agent, "application/octet-stream", 600)?;
        if !(200..300).contains(&status) {
            return Err(anyhow!("GitHub answered {status} for {}", asset.name));
        }
        Ok(bytes)
    }

    /// A GET over HTTPS (plain HTTP only to this computer), a redirect
    /// followed only to another HTTPS address (GitHub sends a release's
    /// files from another host): the status and the bytes, refused past
    /// `LARGEST`.
    #[cfg(feature = "net")]
    fn get(url: &str, agent: &str, accept: &str, seconds: u64) -> Result<(u16, Vec<u8>)> {
        use std::io::Read;
        use std::time::Duration;

        // Plain HTTP, to this computer alone, only while the API itself
        // was moved there (a test's stand-in): a release's list never
        // sends this program to this computer over plain HTTP otherwise.
        let plain = |u: &str| url::Url::parse(u).is_ok_and(|u| u.scheme() == "http");
        if !downloadable(url) || (plain(url) && !plain(&api_url())) {
            return Err(anyhow!(
                "{url} isn't offered over HTTPS, so it wasn't asked for"
            ));
        }
        let policy = reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() < 10 && attempt.url().scheme() == "https" {
                attempt.follow()
            } else {
                attempt.stop()
            }
        });
        let client = reqwest::blocking::Client::builder()
            .user_agent(agent)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(seconds))
            .redirect(policy)
            .build()
            .map_err(|e| anyhow!("couldn't set up HTTPS: {e}"))?;
        let response = client
            .get(url)
            .header("Accept", accept)
            .send()
            .map_err(|e| anyhow!("GitHub couldn't be reached: {e}"))?;
        let status = response.status().as_u16();
        let mut bytes = Vec::new();
        response
            .take(LARGEST + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| anyhow!("GitHub's answer stopped short: {e}"))?;
        if bytes.len() as u64 > LARGEST {
            return Err(anyhow!("{url} is larger than a release's file may be"));
        }
        Ok((status, bytes))
    }

    #[cfg(not(feature = "net"))]
    fn get(url: &str, _agent: &str, _accept: &str, _seconds: u64) -> Result<(u16, Vec<u8>)> {
        let _ = (downloadable(url), LARGEST);
        Err(anyhow!("this program was built without network support"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn versions_read_and_compare_as_semver_does() {
        assert_eq!(v("v0.2.0"), v("0.2.0"));
        assert_eq!(v("1.2.3").to_string(), "1.2.3");
        assert!(v("0.10.0") > v("0.9.9"));
        assert!(v("1.0.0") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-rc.2") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-rc.10") > v("1.0.0-rc.9"));
        assert!(v("1.0.0-beta") > v("1.0.0-alpha"));
        assert!(v("1.0.0-alpha.1") > v("1.0.0-alpha"));
        assert_eq!(v("1.0.0+build.5"), v("1.0.0"));
        for bad in [
            "", "v", "1.2", "1.2.3.4", "1.x.3", "latest", "1.2.3-", "01a.2.3",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn each_platform_gets_its_own_archive() {
        assert_eq!(
            target_for("linux", "x86_64"),
            Some("x86_64-unknown-linux-gnu")
        );
        assert_eq!(target_for("macos", "x86_64"), Some("x86_64-apple-darwin"));
        assert_eq!(target_for("macos", "aarch64"), Some("aarch64-apple-darwin"));
        assert_eq!(target_for("linux", "aarch64"), None);
        assert_eq!(target_for("windows", "x86_64"), None);
        assert!(
            this_target().is_some() || cfg!(not(any(target_os = "linux", target_os = "macos")))
        );

        let mut assets = Vec::new();
        for target in [
            "x86_64-unknown-linux-gnu",
            "x86_64-apple-darwin",
            "aarch64-apple-darwin",
        ] {
            let name = format!("switchboard-0.2.0-{target}.tar.gz");
            assets.push(json!({"name": name, "browser_download_url": format!("https://x/{name}")}));
            for end in ["sha256", "sig"] {
                assets.push(json!({"name": format!("{name}.{end}"), "browser_download_url": format!("https://x/{name}.{end}")}));
            }
        }
        let release = Release::from_json(&json!({
            "tag_name": "v0.2.0", "body": "- One\n- Two", "assets": assets,
            "published_at": "2026-11-01T12:00:00Z", "html_url": "https://github.com/x"
        }))
        .unwrap();
        for (os, arch) in [
            ("linux", "x86_64"),
            ("macos", "x86_64"),
            ("macos", "aarch64"),
        ] {
            let target = target_for(os, arch).unwrap();
            let files = release.files_for(target).unwrap();
            let archive = &files.archive.name;
            assert_eq!(*archive, format!("switchboard-0.2.0-{target}.tar.gz"));
            assert_eq!(files.checksum.name, format!("{archive}.sha256"));
            assert_eq!(files.signature.name, format!("{archive}.sig"));
        }
        assert_eq!(release.files_for("aarch64-unknown-linux-gnu"), None);
        assert_eq!(release.published, Some(1_793_534_400));
        assert_eq!(release.note_lines(), vec!["• One", "• Two"]);
    }

    #[test]
    fn an_archive_without_its_checksum_or_signature_is_not_offered() {
        let name = "switchboard-0.2.0-x86_64-apple-darwin.tar.gz";
        for ends in [vec![], vec![".sha256"], vec![".sig"]] {
            let mut assets = vec![json!({"name": name, "browser_download_url": "https://x/a"})];
            for end in ends {
                assets.push(
                    json!({"name": format!("{name}{end}"), "browser_download_url": "https://x/b"}),
                );
            }
            let release =
                Release::from_json(&json!({"tag_name": "v0.2.0", "assets": assets})).unwrap();
            assert_eq!(release.files_for("x86_64-apple-darwin"), None);
        }
    }

    #[test]
    fn files_come_over_https_or_from_this_computer() {
        assert!(downloadable("https://github.com/x/releases/download/v1/a"));
        assert!(downloadable("http://127.0.0.1:4100/dl/a"));
        assert!(!downloadable("http://github.com/x/releases/download/v1/a"));
        assert!(!downloadable("http://127.0.0.1.example.com/a"));
        assert!(!downloadable("ftp://x/a"));
        assert!(!downloadable("http://127.0.0.1:80@example.com/a"));
        assert!(!downloadable("http://localhost:80@example.com/a"));
    }

    #[test]
    fn drafts_and_pre_releases_are_left_out_and_the_newest_comes_first() {
        let listed = [
            json!({"tag_name": "v0.2.0"}),
            json!({"tag_name": "v0.4.0", "draft": true}),
            json!({"tag_name": "v0.3.0-rc.1", "prerelease": true}),
            json!({"tag_name": "v0.3.0"}),
            json!({"tag_name": "nightly"}),
            // A release of the diff alone, which is none of Switchboard's.
            json!({"tag_name": "diff-v9.0.0"}),
            json!({"tag_name": "v0.1.0"}),
        ];
        let releases: Vec<Release> = listed.iter().filter_map(Release::from_json).collect();
        let tags: Vec<&str> = newer_than(&releases, &v("0.1.0"))
            .iter()
            .map(|r| r.tag.as_str())
            .collect();
        assert_eq!(tags, vec!["v0.3.0", "v0.2.0"]);
        assert!(newer_than(&releases, &v("0.3.0")).is_empty());

        // An app's own releases are those with its tag, and none of
        // Switchboard's.
        let diffs: Vec<Release> = listed
            .iter()
            .filter_map(|r| Release::tagged("diff-v", r))
            .collect();
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].version, v("9.0.0"));
        let name = "switchboard-diff-9.0.0-x86_64-apple-darwin.tar.gz";
        assert_eq!(
            archive_of("switchboard-diff", &diffs[0].version, "x86_64-apple-darwin"),
            name
        );
    }

    fn archive_of_files(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        {
            let mut tar = tar::Builder::new(&mut gz);
            for (path, bytes) in files {
                let mut header = tar::Header::new_gnu();
                header.set_size(bytes.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                tar.append_data(&mut header, path, *bytes).unwrap();
            }
            tar.finish().unwrap();
        }
        gz.finish().unwrap()
    }

    #[test]
    fn the_checksum_must_match_and_name_the_archive() {
        let archive = b"the archive";
        let hash = sha256_hex(archive);
        let name = "switchboard-0.2.0-x86_64-apple-darwin.tar.gz";
        assert!(verify(archive, &format!("{hash}  {name}\n"), name).is_ok());
        assert!(verify(archive, &format!("{hash} *{name}\n"), name).is_ok());
        assert!(verify(archive, &format!("{}\n", hash.to_uppercase()), name).is_ok());
        let bad = verify(b"another archive", &format!("{hash}  {name}\n"), name);
        assert!(bad.unwrap_err().to_string().contains("isn't the file"));
        assert!(verify(archive, &format!("{hash}  other.tar.gz\n"), name).is_err());
        assert!(verify(archive, "", name).is_err());
        assert!(verify(archive, "not-a-hash  x", name).is_err());
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn the_binary_is_found_in_its_folder_by_its_apps_name() {
        let archive = archive_of_files(&[
            ("switchboard-0.2.0-x/README", b"read me"),
            ("switchboard-0.2.0-x/switchboard", b"the binary"),
            ("switchboard-0.2.0-x/switchboard-diff", b"the diff"),
        ]);
        assert_eq!(
            binary_named(&archive, "switchboard").unwrap(),
            b"the binary"
        );
        assert_eq!(
            binary_named(&archive, "switchboard-diff").unwrap(),
            b"the diff"
        );
        let empty = archive_of_files(&[("switchboard-0.2.0-x/README", b"read me")]);
        assert!(binary_named(&empty, "switchboard").is_err());
        assert!(binary_named(b"not gzip at all", "switchboard").is_err());
    }
}
