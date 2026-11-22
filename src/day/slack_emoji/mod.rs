//! Emoji in Slack's messages, drawn rather than left as their codes. New
//! with the Rust version, with no tmux-version file behind it.
//!
//! A standard emoji (`:thumbsup:`, `:wave::skin-tone-3:`) becomes its
//! character as a message's text is read (`slack_look::plain`), from the
//! `emojis` crate's shortcodes, with the names Slack spells its own way
//! added here (`SLACK_NAMES`). A code inside a code span or block stays as
//! written, and a name the standard set doesn't know is left for the
//! workspace's own emoji.
//!
//! A workspace's own emoji (`:shipit:`) is a small picture, which the Slack
//! view draws in two columns (`kit::emoji`, `ui::images::emoji`). Which
//! ones a workspace has comes from Slack's `emoji.list`, through your own
//! sign-in (`slack_user`) while one is stored and Settings'
//! `slack_token_reads` is on, which needs the `emoji:read` scope; with
//! none, a workspace's emoji stay their codes. The list is kept in
//! `~/.cache/switchboard/slack-emoji/list.json` and each picture by its
//! name beside it, both read again after `KEEP`, the old ones used while
//! that fails. An alias resolves to what it names. Everything here runs
//! off the server's task (`serve.rs`), and each request is counted in the
//! request audit (src/net.rs). Settings' `slack_emoji` off leaves every
//! code as written and looks nothing up.

pub mod serve;

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, SystemTime};

use image::RgbaImage;
use regex::Regex;
use serde_json::{Value, json};

use crate::net::{self, Audit};
use crate::{paths, settings, util};

pub use serve::{SlackEmojis, SlackEmojisEvent};

/// How long the workspace's list, and each picture, is kept before it is
/// read again.
pub const KEEP: Duration = Duration::from_secs(7 * 24 * 3600);

/// The longest side of a picture as it is drawn, two cells being small.
const SIDE: u32 = 64;

/// What asks, in the request audit.
const SOURCE: &str = "slack emoji";

/// A picture that hasn't arrived whole in this long has failed.
const TIMEOUT: Duration = Duration::from_secs(30);

/// A picture larger than this is refused as it arrives.
const LARGEST: usize = 2 * 1024 * 1024;

/// How many aliases are followed before a name counts as unknown.
const ALIAS_HOPS: usize = 5;

/// Slack's names for standard emoji that the `emojis` crate's shortcodes
/// (GitHub's) spell another way.
const SLACK_NAMES: &[(&str, &str)] = &[
    ("simple_smile", "🙂"),
    ("slightly_smiling_face", "🙂"),
    ("white_frowning_face", "☹️"),
    ("face_with_rolling_eyes", "🙄"),
    ("hugging_face", "🤗"),
    ("thinking_face", "🤔"),
    ("robot_face", "🤖"),
    ("the_horns", "🤘"),
    ("sign_of_the_horns", "🤘"),
    ("hand_with_index_and_middle_fingers_crossed", "🤞"),
    ("face_with_cowboy_hat", "🤠"),
    ("face_palm", "🤦"),
    ("shrug", "🤷"),
    ("heavy_heart_exclamation_mark_ornament", "❣️"),
    ("admission_tickets", "🎟️"),
    ("raised_hand_with_fingers_splayed", "🖐️"),
    ("reversed_hand_with_middle_finger_extended", "🖕"),
    ("speaking_head_in_silhouette", "🗣️"),
    ("man_in_business_suit_levitating", "🕴️"),
    ("star-struck", "🤩"),
    ("zany_face", "🤪"),
    ("face_with_symbols_on_mouth", "🤬"),
    ("shushing_face", "🤫"),
    ("face_with_hand_over_mouth", "🤭"),
    ("exploding_head", "🤯"),
    ("partying_face", "🥳"),
    ("pleading_face", "🥺"),
    ("melting_face", "🫠"),
    ("saluting_face", "🫡"),
    ("heart_on_fire", "❤️‍🔥"),
    ("white_check_mark", "✅"),
    ("heavy_check_mark", "✔️"),
];

/// A code inside a code span or block, kept as written, or an emoji's
/// code: its name and, for a standard one, its skin tone (2 to 6).
static CODES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)```.*?```|`[^`\n]*`|:([a-z0-9_+-]+):(?::skin-tone-([2-6]):)?").unwrap()
});

/// A standard emoji's character by Slack's name for it, with a skin tone
/// (Slack's 2 to 6) where it takes one; `None` for a name it doesn't know.
pub fn standard(name: &str, tone: Option<u8>) -> Option<String> {
    if let Some((_, c)) = SLACK_NAMES.iter().find(|(n, _)| *n == name) {
        let base = emojis::get(c);
        return Some(
            match (base, tone) {
                (Some(e), Some(t)) => toned(e, t).unwrap_or(e).as_str(),
                _ => c,
            }
            .to_string(),
        );
    }
    let e = emojis::get_by_shortcode(name)?;
    Some(
        tone.and_then(|t| toned(e, t))
            .unwrap_or(e)
            .as_str()
            .to_string(),
    )
}

fn toned(e: &'static emojis::Emoji, tone: u8) -> Option<&'static emojis::Emoji> {
    use emojis::SkinTone;
    let tone = match tone {
        2 => SkinTone::Light,
        3 => SkinTone::MediumLight,
        4 => SkinTone::Medium,
        5 => SkinTone::MediumDark,
        6 => SkinTone::Dark,
        _ => return None,
    };
    e.with_skin_tone(tone)
}

/// The emoji codes in `text` outside code spans and blocks, each as the
/// whole match, its name and its skin tone. A name of digits alone is a
/// code only when it is a standard emoji's (`:100:`), so a time
/// (`10:30:45`) is left alone, and a code right after a colon that closed
/// no code is words, so a path (`std::fs::read`) is too.
fn codes(text: &str) -> Vec<(std::ops::Range<usize>, &str, Option<u8>)> {
    let mut out = Vec::new();
    let mut last_end = 0;
    for c in CODES.captures_iter(text) {
        let whole = c.get(0).expect("a match");
        let Some(name) = c.get(1).map(|n| n.as_str()) else {
            continue;
        };
        let tone = c.get(2).and_then(|t| t.as_str().parse().ok());
        let digits_alone = name.bytes().all(|b| b.is_ascii_digit());
        if digits_alone && standard(name, None).is_none() {
            continue;
        }
        let after_a_colon = text[..whole.start()].ends_with(':') && last_end != whole.start();
        if after_a_colon {
            continue;
        }
        last_end = whole.end();
        out.push((whole.range(), name, tone));
    }
    out
}

/// A piece of a message's text: words as they are, or a workspace's own
/// emoji's code, by its name.
#[derive(Debug, PartialEq, Eq)]
pub enum Piece<'a> {
    Text(&'a str),
    Custom(&'a str),
}

/// `text` with each standard emoji's code as its character, a code inside
/// a code span or block left as written, and the codes of the workspace's
/// own emoji left for `pieces`.
pub fn with_standard(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (whole, name, tone) in codes(text) {
        if let Some(drawn) = standard(name, tone) {
            out.push_str(&text[last..whole.start]);
            out.push_str(&drawn);
            last = whole.end;
        }
    }
    out.push_str(&text[last..]);
    out
}

/// `text` as words and the codes of emoji the standard set doesn't know,
/// which may be the workspace's own; a code inside a code span or block
/// is words.
pub fn pieces(text: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut last = 0;
    for (whole, name, tone) in codes(text) {
        if tone.is_some() || standard(name, None).is_some() {
            continue;
        }
        if whole.start > last {
            out.push(Piece::Text(&text[last..whole.start]));
        }
        out.push(Piece::Custom(name));
        last = whole.end;
    }
    if last < text.len() {
        out.push(Piece::Text(&text[last..]));
    }
    out
}

/// The names of the workspace's own emoji `text` may use.
pub fn custom_names(text: &str) -> impl Iterator<Item = String> + '_ {
    pieces(text).into_iter().filter_map(|p| match p {
        Piece::Custom(name) => Some(name.to_string()),
        Piece::Text(_) => None,
    })
}

/// Whether Settings' `slack_emoji` is on: emoji are drawn.
pub fn on() -> bool {
    settings::is_on("slack_emoji")
}

/// What a workspace's emoji is, as the view draws it: its picture, the
/// standard emoji it is an alias of, or nothing the workspace has by that
/// name.
#[derive(Clone, Debug)]
pub enum Custom {
    Picture(Arc<RgbaImage>),
    Standard(String),
    Unknown,
}

/// What a name in `emoji.list`'s answer comes to: its picture's address,
/// the standard emoji an alias names, or nothing.
#[derive(Debug, PartialEq, Eq)]
pub enum Found {
    Picture(String),
    Standard(String),
    Unknown,
}

/// A name's entry in the workspace's list, its aliases followed.
pub fn resolve(list: &HashMap<String, String>, name: &str) -> Found {
    let mut name = name.to_string();
    for _ in 0..=ALIAS_HOPS {
        let Some(value) = list.get(&name) else {
            return standard(&name, None).map_or(Found::Unknown, Found::Standard);
        };
        match value.strip_prefix("alias:") {
            Some(other) => name = other.to_string(),
            None => return Found::Picture(value.clone()),
        }
    }
    Found::Unknown
}

/// The name as a file's name, `None` for one that couldn't be an emoji's.
fn file_name(name: &str) -> Option<String> {
    let fits = !name.is_empty()
        && name.len() <= 100
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_+-".contains(c));
    fits.then(|| format!("{name}.img"))
}

/// The workspace's emoji kept on this computer.
pub struct Store {
    pub dir: PathBuf,
}

impl Store {
    pub fn here() -> Store {
        Store {
            dir: paths::cache_dir().join("slack-emoji"),
        }
    }

    /// Whether `path` was written within `KEEP` of `now`.
    fn fresh(path: &PathBuf, now: SystemTime) -> bool {
        fs::metadata(path)
            .and_then(|m| m.modified())
            .is_ok_and(|t| now.duration_since(t).unwrap_or_default() < KEEP)
    }

    /// The list kept, and whether it is fresh.
    pub fn list(&self, now: SystemTime) -> Option<(HashMap<String, String>, bool)> {
        let path = self.dir.join("list.json");
        let list = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
        Some((list, Store::fresh(&path, now)))
    }

    pub fn write_list(&self, list: &HashMap<String, String>) -> anyhow::Result<()> {
        fs::create_dir_all(&self.dir)?;
        util::write_atomic(
            self.dir.join("list.json"),
            serde_json::to_string(list)?.as_bytes(),
        )?;
        Ok(())
    }

    /// A picture kept, and whether it is fresh.
    pub fn picture(&self, name: &str, now: SystemTime) -> Option<(Vec<u8>, bool)> {
        let path = self.dir.join(file_name(name)?);
        Some((fs::read(&path).ok()?, Store::fresh(&path, now)))
    }

    pub fn write_picture(&self, name: &str, bytes: &[u8]) -> anyhow::Result<()> {
        let Some(file) = file_name(name) else {
            anyhow::bail!("{name} isn't an emoji's name");
        };
        fs::create_dir_all(&self.dir)?;
        util::write_atomic(self.dir.join(file), bytes)?;
        Ok(())
    }
}

/// A picture's bytes as the image drawn, `SIDE` pixels at most, its first
/// frame for one that moves.
pub fn decode(bytes: &[u8]) -> Option<RgbaImage> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = if img.width() > SIDE || img.height() > SIDE {
        img.thumbnail(SIDE, SIDE)
    } else {
        img
    };
    Some(img.to_rgba8())
}

/// `emoji.list`'s answer as each name and its address or `alias:<name>`.
pub fn list_of(answer: &Value) -> HashMap<String, String> {
    answer
        .get("emoji")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
        .collect()
}

/// The workspace's list: the one kept while it is fresh, else read through
/// your own sign-in and kept, else the one kept however old.
async fn workspace_list(store: &Store) -> Result<HashMap<String, String>, String> {
    let now = SystemTime::now();
    let kept = store.list(now);
    if let Some((list, true)) = &kept {
        return Ok(list.clone());
    }
    let read = async {
        let state = paths::state_dir();
        let client = match super::slack_api::client(&state).await {
            Some(Ok(client)) => client,
            Some(Err(why)) => return Err(why),
            None => return Err("no Slack sign-in of your own to read emoji with".to_string()),
        };
        let answer =
            super::slack_user::api(&client, true, "emoji.list", &json!({}), SOURCE).await?;
        Ok(list_of(&answer))
    };
    match read.await {
        Ok(list) => {
            let _ = store.write_list(&list);
            Ok(list)
        }
        Err(why) => kept.map(|(list, _)| list).ok_or(why),
    }
}

/// A redirect is followed only to Slack's own hosts.
fn slack_host(host: &str) -> bool {
    ["slack.com", "slack-edge.com"]
        .iter()
        .any(|h| host == *h || host.ends_with(&format!(".{h}")))
}

/// A picture's bytes from its address, which Slack serves without a
/// sign-in.
async fn download(url: &str) -> Result<Vec<u8>, String> {
    let got = net::get_file_following(
        url,
        &[],
        TIMEOUT,
        Audit {
            service: "Slack",
            protocol: "HTTPS",
            source: SOURCE,
        },
        slack_host,
        LARGEST,
    )
    .await
    .map_err(|e| e.to_string())?;
    if got.status != 200 {
        return Err(format!("Slack answered {} for an emoji", got.status));
    }
    Ok(got.bytes)
}

/// What each of `names` is, those that could be read: from the workspace's
/// list, each picture the one kept while it is fresh, else downloaded and
/// kept, else the one kept however old. A name whose picture couldn't be
/// had is left out, to be asked again.
pub async fn load(store: &Store, names: &[String]) -> Result<Vec<(String, Custom)>, String> {
    let list = workspace_list(store).await?;
    let now = SystemTime::now();
    let mut out = Vec::new();
    for name in names {
        let custom = match resolve(&list, name) {
            Found::Unknown => Custom::Unknown,
            Found::Standard(c) => Custom::Standard(c),
            Found::Picture(url) => {
                let kept = store.picture(name, now);
                let bytes = match kept {
                    Some((bytes, true)) => Some(bytes),
                    kept => match download(&url).await {
                        Ok(bytes) => {
                            let _ = store.write_picture(name, &bytes);
                            Some(bytes)
                        }
                        Err(_) => kept.map(|(bytes, _)| bytes),
                    },
                };
                match bytes.as_deref().and_then(decode) {
                    Some(img) => Custom::Picture(Arc::new(img)),
                    None => continue,
                }
            }
        };
        out.push((name.clone(), custom));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_standard_code_and_a_skin_tone_become_their_characters() {
        assert_eq!(with_standard("nice :thumbsup:"), "nice 👍");
        assert_eq!(with_standard(":+1: and :tada:"), "👍 and 🎉");
        assert_eq!(with_standard("hi :wave::skin-tone-3:"), "hi 👋🏼");
        assert_eq!(with_standard(":simple_smile:"), "🙂");
    }

    #[test]
    fn a_code_in_a_code_span_or_block_stays_as_written() {
        assert_eq!(with_standard("`:thumbsup:` :tada:"), "`:thumbsup:` 🎉");
        assert_eq!(
            with_standard("```\n:thumbsup:\n``` :tada:"),
            "```\n:thumbsup:\n``` 🎉"
        );
        assert!(
            pieces("`:shipit:`")
                .iter()
                .all(|p| matches!(p, Piece::Text(_)))
        );
    }

    #[test]
    fn an_unknown_name_is_left_for_the_workspaces_own() {
        assert_eq!(with_standard("ship :shipit: now"), "ship :shipit: now");
        assert_eq!(
            pieces("ship :shipit: now"),
            vec![
                Piece::Text("ship "),
                Piece::Custom("shipit"),
                Piece::Text(" now")
            ]
        );
        assert_eq!(
            custom_names(":a-b: :tada:").collect::<Vec<_>>(),
            vec!["a-b"]
        );
    }

    #[test]
    fn times_and_words_with_colons_are_left_alone() {
        assert!(custom_names("at 10:30:45, see: this").next().is_none());
        assert_eq!(with_standard("at 10:30:45"), "at 10:30:45");
        assert!(custom_names("std::fs::read and a::b::c").next().is_none());
        assert_eq!(with_standard(":tada::tada: :100:"), "🎉🎉 💯");
    }

    #[test]
    fn an_alias_resolves_to_what_it_names() {
        let list: HashMap<String, String> = [
            ("shipit", "https://emoji.slack-edge.com/T1/shipit/1.png"),
            ("ship", "alias:shipit"),
            ("yes", "alias:thumbsup"),
            ("loop", "alias:loop"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let shipit = Found::Picture("https://emoji.slack-edge.com/T1/shipit/1.png".into());
        assert_eq!(resolve(&list, "ship"), shipit);
        assert_eq!(resolve(&list, "yes"), Found::Standard("👍".into()));
        assert_eq!(resolve(&list, "loop"), Found::Unknown);
        assert_eq!(resolve(&list, "nope"), Found::Unknown);
    }

    #[test]
    fn every_slack_name_draws_an_emoji() {
        for (name, _) in SLACK_NAMES {
            assert!(standard(name, None).is_some(), "{name}");
        }
    }

    #[test]
    fn the_store_keeps_the_list_and_pictures_by_name() {
        let dir = std::env::temp_dir().join(format!(
            "sb-slack-emoji-{}-{}",
            std::process::id(),
            util::now_ms()
        ));
        let store = Store { dir: dir.clone() };
        let now = SystemTime::now();
        assert!(store.list(now).is_none());
        let list = HashMap::from([("a".to_string(), "u".to_string())]);
        store.write_list(&list).unwrap();
        assert_eq!(store.list(now), Some((list, true)));
        store.write_picture("a", b"png").unwrap();
        assert_eq!(store.picture("a", now), Some((b"png".to_vec(), true)));
        let later = now + KEEP + Duration::from_secs(1);
        assert_eq!(store.picture("a", later), Some((b"png".to_vec(), false)));
        assert!(store.write_picture("../a", b"x").is_err());
        let _ = fs::remove_dir_all(dir);
    }
}
