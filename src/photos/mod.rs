//! People's photos, for the avatar atom (src/ui/images.rs's `avatar`):
//! someone's picture from Slack, Linear or GitHub, fetched once, kept in
//! `~/.cache/switchboard/photos/<service>/<id>`, and counted in the request
//! audit as each request goes out (src/net.rs). New to the Rust port; the
//! tmux version drew no one's face.
//!
//! Where each comes from:
//!
//! - Slack: `users.info` with the Slack relay's bot token, which needs the
//!   `users:read` scope in the relay's app (docs/guide.md's manifest), its
//!   `profile.image_192`. The relay's rules (`relay::slack::refuse`) pass
//!   it, since it names no conversation; a profile that kept Slack's own
//!   picture (`is_custom_image` false) counts as no photo. Slack's
//!   connector says nothing of a picture, so without the relay there is no
//!   Slack photo.
//! - Linear: the user's `avatarUrl` from its GraphQL API, with the key and
//!   under the `linear` setting as every Linear request (src/integrations).
//! - GitHub: the login's picture from `avatars.githubusercontent.com`,
//!   which needs no token, under the `github` setting.
//!
//! A photo is kept as the bytes the service sent, and someone with none as
//! an empty `<id>.none`, so neither is asked for again for `KEEP_DAYS`;
//! after that it is fetched again, the old one used while that fails. What
//! is used is the photo shrunk to `SIDE` pixels at most, since an avatar is
//! two cells wide. The server asks for one only as something draws it
//! (`serve.rs`), off its task, and only while the `photos` setting is on.

pub mod serve;

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::{Result, bail};
use image::RgbaImage;
use serde_json::{Value, json};

use crate::net::{self, Audit};
use crate::{integrations, paths, util};

pub use serve::{Photos, PhotosEvent};

/// How long a photo, or knowing there is none, is kept before it is
/// fetched again.
pub const KEEP_DAYS: u64 = 30;

/// The longest side of a photo as it is used.
pub const SIDE: u32 = 128;

/// What asks, in the request audit.
const SOURCE: &str = "photos";

/// A download that hasn't arrived whole in this long has failed.
const TIMEOUT: Duration = Duration::from_secs(30);

/// A photo larger than this is refused as it arrives.
const LARGEST: usize = 20 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Service {
    Slack,
    Linear,
    GitHub,
}

impl Service {
    fn dir(self) -> &'static str {
        match self {
            Service::Slack => "slack",
            Service::Linear => "linear",
            Service::GitHub => "github",
        }
    }
}

/// Someone, by their id in a service: a Slack member ID (`U…`), a Linear
/// user's id, a GitHub login.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Who {
    pub service: Service,
    pub id: String,
}

impl Who {
    pub fn new(service: Service, id: &str) -> Who {
        Who {
            service,
            id: id.to_string(),
        }
    }

    /// The id as a file's name, `None` for one that couldn't be a service's
    /// id (empty, too long, or with anything but letters, digits, `-` and
    /// `_`), which is never looked up.
    fn file_name(&self) -> Option<&str> {
        let fits = !self.id.is_empty()
            && self.id.len() <= 100
            && self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        fits.then_some(self.id.as_str())
    }
}

/// What the cache holds of someone, with how long it stays fresh (zero
/// once it is `KEEP_DAYS` old).
#[derive(Debug, PartialEq)]
pub enum Kept {
    Photo { bytes: Vec<u8>, left: Duration },
    NoPhoto { left: Duration },
    Nothing,
}

/// How long something is kept before it is fetched again.
pub const KEEP: Duration = Duration::from_secs(KEEP_DAYS * 24 * 3600);

/// The photos kept on this computer.
pub struct Store {
    pub dir: PathBuf,
}

impl Store {
    pub fn here() -> Store {
        Store {
            dir: paths::cache_dir().join("photos"),
        }
    }

    fn path(&self, who: &Who, none: bool) -> Option<PathBuf> {
        let name = who.file_name()?;
        let dir = self.dir.join(who.service.dir());
        Some(if none {
            dir.join(format!("{name}.none"))
        } else {
            dir.join(name)
        })
    }

    pub fn read(&self, who: &Who, now: SystemTime) -> Kept {
        let left = |path: &PathBuf| {
            fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                // Written after `now` (a moment ago) is as fresh as can be.
                .map(|t| KEEP.saturating_sub(now.duration_since(t).unwrap_or_default()))
                .unwrap_or_default()
        };
        let (Some(photo), Some(none)) = (self.path(who, false), self.path(who, true)) else {
            return Kept::Nothing;
        };
        if let Ok(bytes) = fs::read(&photo) {
            return Kept::Photo {
                left: left(&photo),
                bytes,
            };
        }
        if none.exists() {
            return Kept::NoPhoto { left: left(&none) };
        }
        Kept::Nothing
    }

    /// Keeps what the service answered: a photo's bytes, or that there is
    /// none, either taking the other's place.
    pub fn write(&self, who: &Who, photo: Option<&[u8]>) -> Result<()> {
        let (Some(path), Some(none)) = (self.path(who, false), self.path(who, true)) else {
            bail!("{} isn't an id to keep a photo by", who.id);
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        match photo {
            Some(bytes) => {
                util::write_atomic(&path, bytes)?;
                let _ = fs::remove_file(none);
            }
            None => {
                util::write_atomic(&none, b"")?;
                let _ = fs::remove_file(path);
            }
        }
        Ok(())
    }
}

/// A photo's bytes as the image an avatar draws, `SIDE` pixels at most.
pub fn decode(bytes: &[u8]) -> Option<RgbaImage> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = if img.width() > SIDE || img.height() > SIDE {
        img.thumbnail(SIDE, SIDE)
    } else {
        img
    };
    Some(img.to_rgba8())
}

/// What a load found: the photo (`None` for someone without one), and how
/// long until it is due to be fetched again: what is left of a kept one's
/// `KEEP`, all of it for one just fetched, and zero for an old one kept
/// because fetching it again failed, which is to be tried again soon.
#[derive(Debug)]
pub struct Loaded {
    pub photo: Option<RgbaImage>,
    pub fresh_for: Duration,
}

/// Someone's photo: the one kept while it is fresh, else fetched and kept,
/// else (the service failing) the one kept however old.
pub async fn load(store: &Store, who: &Who) -> Result<Loaded> {
    let kept = store.read(who, SystemTime::now());
    let fresh = |photo, fresh_for| Loaded { photo, fresh_for };
    match &kept {
        Kept::Photo { bytes, left } if !left.is_zero() => {
            return Ok(fresh(decode(bytes), *left));
        }
        Kept::NoPhoto { left } if !left.is_zero() => return Ok(fresh(None, *left)),
        _ => {}
    }
    match fetch(who).await {
        Ok(bytes) => {
            let _ = store.write(who, bytes.as_deref());
            Ok(fresh(bytes.as_deref().and_then(decode), KEEP))
        }
        Err(e) => {
            let photo = match kept {
                Kept::Photo { bytes, .. } => decode(&bytes),
                Kept::NoPhoto { .. } => None,
                Kept::Nothing => return Err(e),
            };
            Ok(fresh(photo, Duration::ZERO))
        }
    }
}

/// The photo's bytes from its service, `None` when they have none.
pub async fn fetch(who: &Who) -> Result<Option<Vec<u8>>> {
    if who.file_name().is_none() {
        bail!("{} isn't an id to look a photo up by", who.id);
    }
    match who.service {
        Service::Slack => {
            let Some(token) = crate::relay::slack::token(crate::relay::slack::BOT_TOKEN_SERVICE)
            else {
                bail!("The Slack relay isn't set up, so there are no Slack photos.");
            };
            let Some(url) =
                slack_photo_url(&crate::relay::slack::api_base(), &token, &who.id).await?
            else {
                return Ok(None);
            };
            download(&url, &[], "Slack").await
        }
        Service::Linear => {
            let data = integrations::linear::graphql(
                "query($id: String!) { user(id: $id) { avatarUrl } }",
                json!({ "id": who.id }),
                SOURCE,
            )
            .await?;
            let Some(url) = data["user"]["avatarUrl"].as_str().filter(|u| !u.is_empty()) else {
                return Ok(None);
            };
            // Linear's own hosts may want the key, as its uploads do.
            let key;
            let headers: Vec<(&str, &str)> = if linear_host(url) {
                key = integrations::linear::api_key().await?;
                vec![("Authorization", key.as_str())]
            } else {
                Vec::new()
            };
            download(url, &headers, "Linear").await
        }
        Service::GitHub => {
            integrations::ensure_on("github", "GitHub")?;
            let url = format!("{}/{}?s={SIDE}", github_avatars(), who.id);
            download(&url, &[], "GitHub").await
        }
    }
}

/// Where GitHub's avatars are (`SWITCHBOARD_GITHUB_AVATARS` moves it).
pub fn github_avatars() -> String {
    net::base_url(
        "SWITCHBOARD_GITHUB_AVATARS",
        "https://avatars.githubusercontent.com",
    )
}

/// Whether `url` is HTTPS to Linear's own host, read as a browser reads it.
fn linear_host(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https"
            && u.domain()
                .is_some_and(|host| host == "linear.app" || host.ends_with(".linear.app"))
    })
}

/// The address of a Slack member's own photo, from `users.info` at `base`
/// (Slack's Web API, its `/` at the end) with the bot's token. `None` when
/// they kept Slack's picture.
pub async fn slack_photo_url(base: &str, token: &str, id: &str) -> Result<Option<String>> {
    let params = json!({ "user": id });
    crate::relay::slack::refuse("users.info", &params, None, None)?;
    let auth = format!("Bearer {token}");
    let audit = Audit {
        service: "Slack",
        protocol: "REST",
        source: SOURCE,
    };
    let response = net::get_json(
        &format!("{base}users.info?user={id}"),
        &[("Authorization", auth.as_str())],
        audit,
    )
    .await?;
    slack_photo_of(&response.body)
}

/// `users.info`'s answer read: the largest of its pictures we use, or why
/// Slack refused.
fn slack_photo_of(answer: &Value) -> Result<Option<String>> {
    if answer["ok"] != Value::Bool(true) {
        let why = answer["error"].as_str().unwrap_or("no reason given");
        if why == "missing_scope" {
            bail!(
                "Slack's users.info needs the users:read scope in the Slack relay's app, so there are no Slack photos."
            );
        }
        bail!("Slack's users.info refused: {why}");
    }
    let profile = &answer["user"]["profile"];
    if profile["is_custom_image"] == Value::Bool(false) {
        return Ok(None);
    }
    Ok(["image_192", "image_72", "image_48"]
        .iter()
        .find_map(|k| profile[*k].as_str().filter(|u| !u.is_empty()))
        .map(str::to_string))
}

/// A photo's bytes from `url`: `None` for a 404 (someone with no photo) or
/// a host (or redirect) of an integration turned off, which is never reached;
/// an error for anything else that isn't a picture.
async fn download(url: &str, headers: &[(&str, &str)], service: &str) -> Result<Option<Vec<u8>>> {
    let audit = Audit {
        service,
        protocol: "HTTPS",
        source: SOURCE,
    };
    // An address (or a redirect) on a host of an integration turned off is
    // never reached: a Linear user's photo may be kept on GitHub's.
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default();
    if !crate::views::pictures::reachable(&host) {
        return Ok(None);
    }
    let file = net::get_file_following(
        url,
        headers,
        TIMEOUT,
        audit,
        crate::views::pictures::reachable,
        LARGEST,
    )
    .await?;
    // Not found, or a redirect to a host of an integration turned off.
    if file.status == 404 || (300..400).contains(&file.status) {
        return Ok(None);
    }
    if file.status != 200 {
        bail!("{service} answered {} for a photo", file.status);
    }
    if decode(&file.bytes).is_none() {
        bail!("What {service} sent for a photo isn't a picture");
    }
    Ok(Some(file.bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Root(PathBuf);

    impl Root {
        fn new(name: &str) -> Root {
            let dir = std::env::temp_dir().join(format!("sb-photos-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Root(dir)
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn png(side: u32) -> Vec<u8> {
        let img = RgbaImage::from_pixel(side, side, image::Rgba([200, 10, 10, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn a_photo_or_none_is_kept_and_goes_stale_after_the_days_it_is_kept() {
        let root = Root::new("store");
        let store = Store {
            dir: root.0.clone(),
        };
        let ana = Who::new(Service::Slack, "UANA1234");
        let now = SystemTime::now();
        assert_eq!(store.read(&ana, now), Kept::Nothing);
        store.write(&ana, None).unwrap();
        assert!(
            matches!(store.read(&ana, now), Kept::NoPhoto { left } if left > KEEP - Duration::from_secs(60))
        );
        store.write(&ana, Some(b"photo")).unwrap();
        assert!(matches!(
            store.read(&ana, now),
            Kept::Photo { bytes, left } if bytes == b"photo" && left > KEEP - Duration::from_secs(60)
        ));
        // What is left is what remains of its days, not all of them again.
        let day_29 = now + KEEP - Duration::from_secs(24 * 3600);
        assert!(matches!(
            store.read(&ana, day_29),
            Kept::Photo { left, .. } if left <= Duration::from_secs(24 * 3600 + 60)
        ));
        assert!(
            !root.0.join("slack/UANA1234.none").exists(),
            "the photo takes its place"
        );
        let later = now + Duration::from_secs((KEEP_DAYS + 1) * 24 * 3600);
        assert!(matches!(
            store.read(&ana, later),
            Kept::Photo { left, .. } if left.is_zero()
        ));
        // The same id in another service is someone else.
        assert_eq!(
            store.read(&Who::new(Service::GitHub, "UANA1234"), now),
            Kept::Nothing
        );
    }

    #[test]
    fn an_id_that_could_name_another_file_is_never_kept_or_looked_up() {
        let root = Root::new("ids");
        let store = Store {
            dir: root.0.clone(),
        };
        for id in ["", "../x", "a/b", "a.b", &"x".repeat(101)] {
            let who = Who::new(Service::GitHub, id);
            assert!(store.write(&who, Some(b"x")).is_err(), "{id:?}");
            assert_eq!(store.read(&who, SystemTime::now()), Kept::Nothing);
        }
        assert!(
            store
                .write(&Who::new(Service::GitHub, "some-login_2"), None)
                .is_ok()
        );
    }

    #[test]
    fn a_large_photo_is_shrunk_and_anything_else_isnt_a_photo() {
        let img = decode(&png(400)).unwrap();
        assert_eq!(img.dimensions(), (SIDE, SIDE));
        assert_eq!(decode(&png(40)).unwrap().dimensions(), (40, 40));
        assert!(decode(b"<html>").is_none());
    }

    #[test]
    fn slacks_answer_gives_the_largest_picture_and_none_for_its_own() {
        let answer = |profile: Value| json!({ "ok": true, "user": { "profile": profile } });
        assert_eq!(
            slack_photo_of(&answer(json!({
                "is_custom_image": true,
                "image_72": "https://a/72.jpg",
                "image_192": "https://a/192.jpg",
            })))
            .unwrap()
            .as_deref(),
            Some("https://a/192.jpg")
        );
        assert_eq!(
            slack_photo_of(&answer(json!({
                "is_custom_image": false,
                "image_192": "https://a/192.jpg",
            })))
            .unwrap(),
            None
        );
        let missing = slack_photo_of(&json!({ "ok": false, "error": "missing_scope" }))
            .unwrap_err()
            .to_string();
        assert!(missing.contains("users:read"), "{missing}");
    }

    #[test]
    fn only_linears_own_hosts_are_sent_the_key() {
        assert!(linear_host("https://public.linear.app/a/b.png"));
        assert!(linear_host("https://linear.app/a.png"));
        assert!(!linear_host("https://evil.example/linear.app/a.png"));
        assert!(!linear_host("https://notlinear.app/a.png"));
        assert!(!linear_host("http://public.linear.app/a.png"));
        assert!(!linear_host("https://evil.example?host=.linear.app"));
        assert!(!linear_host("https://evil.example#.linear.app"));
        assert!(!linear_host("https://public.linear.app@evil.example/a.png"));
        assert!(linear_host("https://evil.example@public.linear.app/a.png"));
    }

    #[cfg(feature = "net")]
    #[test]
    fn a_slack_photo_is_asked_with_the_bots_token_and_downloaded() {
        use crate::fake_http::{Fake, Reply, Service as Fs};
        let web = Fake::start(Fs::Web, |request| match request.path() {
            "/ana.png" => Some(Reply::bytes(200, "image/png", png(16))),
            _ => None,
        });
        let photo = format!("{}/ana.png", web.url);
        let slack = Fake::start(
            Fs::Slack {
                token: "xoxb-1".into(),
            },
            move |request| {
                assert_eq!(request.path(), "/api/users.info");
                assert!(request.target.contains("user=UANA1234"));
                Some(Reply::json(
                    200,
                    json!({ "ok": true, "user": { "profile": {
                    "is_custom_image": true, "image_192": photo,
                } } }),
                ))
            },
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let base = format!("{}/api/", slack.url);
        let url = runtime
            .block_on(slack_photo_url(&base, "xoxb-1", "UANA1234"))
            .unwrap()
            .unwrap();
        let bytes = runtime
            .block_on(download(&url, &[], "Slack"))
            .unwrap()
            .unwrap();
        assert_eq!(decode(&bytes).unwrap().dimensions(), (16, 16));
        let missing = format!("{}/nobody.png", web.url);
        assert_eq!(
            runtime.block_on(download(&missing, &[], "Slack")).unwrap(),
            None
        );
    }
}
