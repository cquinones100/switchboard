//! What GitHub says of Switchboard's releases, and which file of one is for
//! this computer. Where repo-update.sh fetched origin's main and counted the
//! commits yours didn't have, an update is now a release: a tag `v<version>`
//! with an archive per platform, `switchboard-<version>-<target>.tar.gz`,
//! its SHA-256 beside it in `<archive>.sha256` and its signature in
//! `<archive>.sig`, as `.github/workflows/release.yml` publishes them. What a
//! release is (`Version`, `Release`, which archive is a target's) is the
//! plugin API's `release`, shared with the diff's own update; this module
//! keeps how Switchboard reaches GitHub for one.
//!
//! A release's files are downloaded over HTTPS alone, every redirect
//! included (`download`), with plain HTTP only to this computer's own
//! address, where the tests' and the demo's stand-ins answer.
//!
//! The releases are read from GitHub's REST API as every GitHub call is
//! (`github::get_public`): with gh's token when gh is signed in, since the
//! 60 requests an hour GitHub allows without one are shared by every program
//! behind the same address and often spent by the others, and without one
//! when there is none or it won't do (refused, or its account's limit
//! spent), which a public repository's releases don't need.
//! Drafts and pre-releases are left out, so a release is offered only once it
//! is published as one.

use anyhow::{Result, anyhow};
pub use switchboard_plugin_api::release::{
    Asset, Files, REPO, Release, Version, archive_name, downloadable, newer_than, target_for,
    this_target,
};

use crate::integrations::github;
use crate::net::{self, Audit, ErrorKind, ServiceError};

/// What made a request, for the request audit.
fn audit(source: &str) -> Audit<'_> {
    Audit {
        service: "GitHub",
        protocol: "REST",
        source,
    }
}

/// How many releases GitHub is asked for at a time.
const PER_PAGE: usize = 30;

/// The most pages of the list read to find Switchboard's releases.
const PAGES_MOST: usize = 10;

/// How many of Switchboard's releases the list is read for: enough to
/// offer the newest and to find one a few back by its tag.
const ENOUGH: usize = 10;

/// The published releases of Switchboard, newest first, from the list
/// GitHub gives thirty at a time. The repository holds other releases too
/// (the diff's own and Notes', tagged `diff-v<version>` and
/// `notes-v<version>`, which `Release::from_json` leaves out), so the next page is read while fewer than `ENOUGH` of
/// Switchboard's have come and the list goes on: releases of the diff
/// neither hide a newer Switchboard nor the ones just before it.
/// `source` says what asked, for the request audit. The API's address is
/// GitHub's own, moved by `SWITCHBOARD_GITHUB_API` as every GitHub call is.
pub async fn list(source: &str) -> Result<Vec<Release>> {
    let mut found = Vec::new();
    for page in 1..=PAGES_MOST {
        let (mut releases, listed) = list_page(page, source).await?;
        found.append(&mut releases);
        if found.len() >= ENOUGH || listed < PER_PAGE {
            break;
        }
    }
    Ok(found)
}

/// One page of the list: Switchboard's releases on it, and how many
/// releases of any kind it held.
async fn list_page(page: usize, source: &str) -> Result<(Vec<Release>, usize)> {
    let path = format!("repos/{REPO}/releases?per_page={PER_PAGE}&page={page}");
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
    Ok((
        listed.iter().filter_map(Release::from_json).collect(),
        listed.len(),
    ))
}

/// How long a release's file may take to arrive.
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// The largest file a release may hold.
const LARGEST: usize = 500 * 1024 * 1024;

/// A release's file, downloaded whole over HTTPS, a redirect followed only
/// to another HTTPS address (GitHub sends a release's files from another
/// host); an error for anything but a success.
pub async fn download(asset: &Asset, source: &str) -> Result<Vec<u8>> {
    if !downloadable(&asset.url) {
        return Err(anyhow!(
            "{} isn't offered over HTTPS ({}), so it wasn't downloaded",
            asset.name,
            asset.url
        ));
    }
    let file = net::get_file_following(
        &asset.url,
        &[("Accept", "application/octet-stream")],
        DOWNLOAD_TIMEOUT,
        audit(source),
        |_| true,
        LARGEST,
    )
    .await?;
    if !(200..300).contains(&file.status) {
        return Err(anyhow!(
            "GitHub answered {} for {}",
            file.status,
            asset.name
        ));
    }
    Ok(file.bytes)
}
