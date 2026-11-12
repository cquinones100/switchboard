//! What GitHub says of Switchboard's releases, and which file of one is for
//! this computer. Where repo-update.sh fetched origin's main and counted the
//! commits yours didn't have, an update is now a release: a tag `v<version>`
//! with an archive per platform, `switchboard-<version>-<target>.tar.gz`,
//! and its SHA-256 beside it in `<archive>.sha256`, as
//! `.github/workflows/release.yml` publishes them.
//!
//! The releases are read from GitHub's REST API as every GitHub call is
//! (`github::get_public`): with gh's token when gh is signed in, since the
//! 60 requests an hour GitHub allows without one are shared by every program
//! behind the same address and often spent by the others, and without one
//! when there is none or it won't do (refused, or its account's limit
//! spent), which a public repository's releases don't need.
//! Drafts and pre-releases are left out, so a release is offered only once it
//! is published as one.

use std::cmp::Ordering;
use std::fmt;

use anyhow::{Result, anyhow};
use serde_json::Value;

use crate::integrations::github;
use crate::net::{self, Audit, ErrorKind, ServiceError};

/// The repository releases come from, the one the code lives in: a version
/// bump on its main publishes one (.github/workflows/release.yml).
/// install.sh's `REPO` and the README's install line and clone URL name it
/// too, and change with it.
pub const REPO: &str = "cquinones100/switchboard";

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

    /// This binary's own version, from Cargo.toml.
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
    /// A release from GitHub's JSON for one; `None` for a draft, a
    /// pre-release, or a tag that isn't a version.
    pub fn from_json(json: &Value) -> Option<Release> {
        if json["draft"].as_bool() == Some(true) || json["prerelease"].as_bool() == Some(true) {
            return None;
        }
        let tag = json["tag_name"].as_str()?.to_string();
        let version = Version::parse(&tag)?;
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

    /// The archive for a target and the checksum beside it, when this
    /// release has both.
    pub fn files_for(&self, target: &str) -> Option<(&Asset, &Asset)> {
        let name = archive_name(&self.version, target);
        let sum = format!("{name}.sha256");
        let archive = self.assets.iter().find(|a| a.name == name)?;
        let checksum = self.assets.iter().find(|a| a.name == sum)?;
        Some((archive, checksum))
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

/// An archive's name: `switchboard-<version>-<target>.tar.gz`.
pub fn archive_name(version: &Version, target: &str) -> String {
    format!("switchboard-{version}-{target}.tar.gz")
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

/// What made a request, for the request audit.
fn audit(source: &str) -> Audit<'_> {
    Audit {
        service: "GitHub",
        protocol: "REST",
        source,
    }
}

/// The published releases, the newest thirty GitHub lists. `source` says
/// what asked, for the request audit. The API's address is GitHub's own,
/// moved by `SWITCHBOARD_GITHUB_API` as every GitHub call is.
pub async fn list(source: &str) -> Result<Vec<Release>> {
    let path = format!("repos/{REPO}/releases?per_page=30");
    let response = github::get_public(&path, source).await?;
    if !response.ok() {
        let said = response.body["message"].as_str().unwrap_or("").to_string();
        let kind = match response.status {
            401 => ErrorKind::Unauthorized,
            403 | 429 => ErrorKind::RateLimited {
                reset_at: response
                    .header("x-ratelimit-reset")
                    .and_then(|r| r.parse().ok()),
            },
            404 => ErrorKind::NotFound,
            _ => ErrorKind::Failed,
        };
        let message = if said.is_empty() {
            format!("GitHub answered {} for {REPO}'s releases", response.status)
        } else {
            format!(
                "GitHub answered {} for {REPO}'s releases: {said}",
                response.status
            )
        };
        return Err(ServiceError::new(kind, "GitHub", message).into());
    }
    let listed = response
        .body
        .as_array()
        .ok_or_else(|| anyhow!("GitHub's list of releases wasn't a list"))?;
    Ok(listed.iter().filter_map(Release::from_json).collect())
}

/// How long a release's file may take to arrive.
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// A release's file, downloaded whole; an error for anything but a success.
pub async fn download(asset: &Asset, source: &str) -> Result<Vec<u8>> {
    let (status, bytes) = net::get_bytes(
        &asset.url,
        &[("Accept", "application/octet-stream")],
        DOWNLOAD_TIMEOUT,
        audit(source),
    )
    .await?;
    if !(200..300).contains(&status) {
        return Err(anyhow!("GitHub answered {status} for {}", asset.name));
    }
    Ok(bytes)
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
            assets.push(json!({"name": format!("{name}.sha256"), "browser_download_url": format!("https://x/{name}.sha256")}));
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
            let (archive, sum) = release.files_for(target).unwrap();
            assert_eq!(archive.name, format!("switchboard-0.2.0-{target}.tar.gz"));
            assert_eq!(sum.name, format!("{}.sha256", archive.name));
        }
        assert_eq!(release.files_for("aarch64-unknown-linux-gnu"), None);
        assert_eq!(release.published, Some(1_793_534_400));
        assert_eq!(release.note_lines(), vec!["• One", "• Two"]);
    }

    #[test]
    fn an_archive_without_its_checksum_is_not_offered() {
        let release = Release::from_json(&json!({
            "tag_name": "v0.2.0",
            "assets": [{"name": "switchboard-0.2.0-x86_64-apple-darwin.tar.gz", "browser_download_url": "https://x/a"}]
        }))
        .unwrap();
        assert_eq!(release.files_for("x86_64-apple-darwin"), None);
    }

    #[test]
    fn drafts_and_pre_releases_are_left_out_and_the_newest_comes_first() {
        let releases: Vec<Release> = [
            json!({"tag_name": "v0.2.0"}),
            json!({"tag_name": "v0.4.0", "draft": true}),
            json!({"tag_name": "v0.3.0-rc.1", "prerelease": true}),
            json!({"tag_name": "v0.3.0"}),
            json!({"tag_name": "nightly"}),
            json!({"tag_name": "v0.1.0"}),
        ]
        .iter()
        .filter_map(Release::from_json)
        .collect();
        let tags: Vec<&str> = newer_than(&releases, &v("0.1.0"))
            .iter()
            .map(|r| r.tag.as_str())
            .collect();
        assert_eq!(tags, vec!["v0.3.0", "v0.2.0"]);
        assert!(newer_than(&releases, &v("0.3.0")).is_empty());
    }
}
