//! Opening a link in the browser, as tmux/open-url.sh did for every link
//! Switchboard opened (mission control's menus, the link view, M-r, M-l, the
//! ticket and merge queue commands): in the Chrome profile
//! `~/.config/switchboard/browser.json` gives the link's site, else in its
//! default profile, else with whatever the desktop opens links with
//! (`platform::open`). The file is yours, never in a repository:
//!
//! ```json
//! { "hosts": { "linear.app": "Profile 2", "slack.com": "Profile 2" }, "default": "Profile 1" }
//! ```
//!
//! A site matches itself and its subdomains, the longest match winning, and
//! a host ends at a `/`, `:`, `?` or `#`, so a link's fragment can't pick
//! another site's profile. A profile is the folder Chrome keeps it in, as
//! chrome://version names it ("Profile 2"); one written as the name Chrome
//! shows ("Work") is found by that name in Chrome's Local State, which the
//! Settings view's Default Chrome profile row lists too. With the
//! chrome_profiles setting off, every link opens in the default browser.
//!
//! On macOS Chrome opens with `open -na "Google Chrome" --args
//! --profile-directory=…`, as chrome-profile.sh ran it; on Linux the first
//! of google-chrome and Chromium installed is run with the same flag. Where
//! neither is, the link opens in the default browser, its profile unchosen.
//! Opening a link reaches no service itself, so nothing is counted in the
//! request audit, as open-url.sh counted nothing.

use std::path::PathBuf;

use anyhow::{Result, bail};
use serde_json::Value;

use crate::usage::own::{Profile, profiles_in};
use crate::{paths, platform, settings, util};

/// Where a link opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Browser {
    /// Chrome, in the profile kept in this folder.
    Chrome(String),
    /// Whatever the desktop opens links with.
    Default,
}

/// `~/.config/switchboard/browser.json`.
pub fn browser_file() -> PathBuf {
    paths::config_dir().join("browser.json")
}

/// A link's host, in lower case: what follows `scheme://` up to the first
/// `/`, `?` or `#`, without a user before an `@` or a port after a `:`.
/// `None` for anything but an http or https link.
pub fn host_of(url: &str) -> Option<String> {
    let scheme = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    let rest = if scheme.starts_with("https://") {
        &url[8..]
    } else if scheme.starts_with("http://") {
        &url[7..]
    } else {
        return None;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(bracketed) = host.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or(bracketed)
    } else {
        host.split(':').next().unwrap_or(host)
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// The profile browser.json gives a host: the longest of its `hosts` the
/// host is, or is a subdomain of, else its `default`.
pub fn profile_for(host: &str, config: &Value) -> Option<String> {
    let named = config
        .get("hosts")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(site, profile)| {
            let site = site.trim().to_ascii_lowercase();
            let profile = profile.as_str()?.trim();
            let matches = !site.is_empty() && (host == site || host.ends_with(&format!(".{site}")));
            (matches && !profile.is_empty()).then(|| (site.len(), profile.to_string()))
        })
        .max_by_key(|(len, _)| *len)
        .map(|(_, profile)| profile);
    named.or_else(|| {
        config
            .get("default")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string)
    })
}

/// A profile as Chrome keeps it: a folder Local State lists, as given; a
/// name Chrome shows for one, as that one's folder; anything else as given,
/// since Chrome makes a profile it doesn't know yet.
pub fn profile_dir(profile: &str, profiles: Option<&[Profile]>) -> String {
    let Some(profiles) = profiles else {
        return profile.to_string();
    };
    if profiles.iter().any(|p| p.dir == profile) {
        return profile.to_string();
    }
    profiles
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(profile))
        .map_or_else(|| profile.to_string(), |p| p.dir.clone())
}

/// Where a link opens: the profile `config` (browser.json) gives its site,
/// found among Chrome's `profiles`, while `profiles_on` (the chrome_profiles
/// setting); else the default browser.
pub fn choose(
    url: &str,
    config: &Value,
    profiles: Option<&[Profile]>,
    profiles_on: bool,
) -> Browser {
    if !profiles_on {
        return Browser::Default;
    }
    match host_of(url).and_then(|host| profile_for(&host, config)) {
        Some(profile) => Browser::Chrome(profile_dir(&profile, profiles)),
        None => Browser::Default,
    }
}

/// Opens an http or https link as the module's comment says. Blocking for
/// as long as it takes to hand the link over (a moment): the server calls
/// it on a blocking thread (`open_in_background`).
pub fn open_url(url: &str) -> Result<()> {
    open_url_as(url, None)
}

/// `open_url`, the profile chosen as for a link on the site `as_site`
/// rather than the link's own, when given: the AWS login's, whose pages are
/// on AWS's sign-in hosts but whose profile is the one browser.json gives
/// its access portal, `awsapps.com`, as aws-login.sh chose it.
pub fn open_url_as(url: &str, as_site: Option<&str>) -> Result<()> {
    let url = url.trim();
    if host_of(url).is_none() {
        bail!("{url} isn't a web link");
    }
    let config = util::read_json(browser_file()).unwrap_or(Value::Null);
    let profiles = platform::chrome_local_state().and_then(|f| profiles_in(&f));
    let chosen_by = as_site.map(|site| format!("https://{site}/"));
    let profiles_on = settings::is_on("chrome_profiles");
    match choose(
        chosen_by.as_deref().unwrap_or(url),
        &config,
        profiles.as_deref(),
        profiles_on,
    ) {
        Browser::Chrome(profile) => match open_in_chrome(url, &profile) {
            Ok(()) => Ok(()),
            // No Chrome to run: the link still opens, in the default browser.
            Err(_) => platform::open(url),
        },
        // The AWS login with no profile named for it still opens in Chrome,
        // in the profile Chrome picks, as aws-login.sh's BROWSER did.
        Browser::Default if as_site.is_some() && profiles_on => {
            open_in_chrome(url, "").or_else(|_| platform::open(url))
        }
        Browser::Default => platform::open(url),
    }
}

/// `open_url` off the server's task, on the async runtime's blocking pool,
/// for the server and what it runs. Off a runtime (a unit test driving the
/// server's state) it opens nothing, so no test opens a browser. What went
/// wrong is dropped, for callers with nowhere to say it.
pub fn open_in_background(url: String) {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn_blocking(move || {
            let _ = open_url(&url);
        });
    }
}

#[cfg(target_os = "macos")]
fn open_in_chrome(url: &str, profile: &str) -> Result<()> {
    // No profile: the one Chrome picks itself.
    let chosen = format!("--profile-directory={profile}");
    let mut args = vec!["-na", "Google Chrome", "--args"];
    if !profile.is_empty() {
        args.push(&chosen);
    }
    args.push(url);
    util::run("open", args).map(|_| ())
}

#[cfg(not(target_os = "macos"))]
fn open_in_chrome(url: &str, profile: &str) -> Result<()> {
    use std::process::{Command, Stdio};
    // Chromium first when its folder is where the profiles were read from.
    let chromium_first =
        platform::chrome_local_state().is_some_and(|f| f.to_string_lossy().contains("/chromium/"));
    let mut names = vec!["google-chrome", "google-chrome-stable"];
    let chromium = ["chromium", "chromium-browser"];
    if chromium_first {
        names.splice(0..0, chromium);
    } else {
        names.extend(chromium);
    }
    let Some(chrome) = names.iter().find_map(|n| util::which(n)) else {
        bail!("Chrome isn't installed");
    };
    let mut command = Command::new(chrome);
    // No profile: the one Chrome picks itself.
    if !profile.is_empty() {
        command.arg(format!("--profile-directory={profile}"));
    }
    let mut child = command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // A Chrome already running takes the link and this one ends at once; a
    // Chrome started here runs on. Either way it is waited for elsewhere, so
    // it never lingers as a zombie of the server's.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// `switchboard open-url <url>`: what open-url.sh was, for scripts, skills
/// and gh's `--web` (as its `GH_BROWSER`, which every pane's environment
/// sets). `--as <site>` chooses the profile as for a link on that site.
pub fn main(args: Vec<String>) -> std::process::ExitCode {
    let (as_site, url) = match args.as_slice() {
        [url] => (None, url),
        [flag, site, url] if flag == "--as" => (Some(site.as_str()), url),
        _ => {
            eprintln!("Usage: switchboard open-url [--as <site>] <url>");
            return std::process::ExitCode::from(2);
        }
    };
    match open_url_as(url, as_site) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("open-url: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profiles() -> Vec<Profile> {
        vec![
            Profile {
                dir: "Default".into(),
                name: "Person 1".into(),
            },
            Profile {
                dir: "Profile 2".into(),
                name: "Work".into(),
            },
        ]
    }

    #[test]
    fn a_host_ends_where_a_link_says_it_does() {
        assert_eq!(
            host_of("https://Linear.app/x/issue/A-1"),
            Some("linear.app".into())
        );
        assert_eq!(host_of("http://localhost:3000/a"), Some("localhost".into()));
        assert_eq!(
            host_of("https://github.com#linear.app"),
            Some("github.com".into())
        );
        assert_eq!(
            host_of("https://github.com?x=linear.app"),
            Some("github.com".into())
        );
        assert_eq!(host_of("https://me@linear.app/"), Some("linear.app".into()));
        assert_eq!(host_of("HTTPS://GitHub.com"), Some("github.com".into()));
        assert_eq!(host_of("file:///tmp/x"), None);
        assert_eq!(host_of("https://"), None);
    }

    #[test]
    fn a_link_opens_in_its_sites_profile_else_the_default_else_the_browser() {
        let config = json!({ "hosts": { "linear.app": "Profile 2", "app.linear.app": "Work" }, "default": "Profile 1" });
        let local = profiles();
        let at = |url: &str| choose(url, &config, Some(&local), true);
        assert_eq!(
            at("https://linear.app/x/issue/ABC-1"),
            Browser::Chrome("Profile 2".into())
        );
        assert_eq!(
            at("https://team.linear.app/"),
            Browser::Chrome("Profile 2".into()),
            "a subdomain of a site named"
        );
        assert_eq!(
            at("https://app.linear.app/"),
            Browser::Chrome("Profile 2".into()),
            "the longest site wins, and a name Chrome shows is its folder"
        );
        assert_eq!(
            at("https://github.com/o/r/pull/12345"),
            Browser::Chrome("Profile 1".into())
        );
        assert_eq!(
            at("https://notlinear.app/"),
            Browser::Chrome("Profile 1".into()),
            "only the site and its subdomains"
        );
        assert_eq!(
            at("https://github.com/#linear.app"),
            Browser::Chrome("Profile 1".into()),
            "a fragment can't pick another site's profile"
        );

        let no_default = json!({ "hosts": { "linear.app": "Profile 2" } });
        assert_eq!(
            choose("https://github.com/o/r", &no_default, None, true),
            Browser::Default
        );
        assert_eq!(
            choose("https://linear.app/x", &config, Some(&local), false),
            Browser::Default,
            "chrome_profiles off"
        );
        assert_eq!(
            choose("https://linear.app/x", &Value::Null, None, true),
            Browser::Default,
            "no browser.json"
        );
    }

    #[test]
    fn a_profile_is_found_by_its_folder_or_its_name() {
        let local = profiles();
        assert_eq!(profile_dir("Profile 2", Some(&local)), "Profile 2");
        assert_eq!(profile_dir("work", Some(&local)), "Profile 2");
        assert_eq!(profile_dir("Profile 9", Some(&local)), "Profile 9");
        assert_eq!(profile_dir("Work", None), "Work");
    }
}
