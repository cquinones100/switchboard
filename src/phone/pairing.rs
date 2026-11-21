//! Pairing a phone, and the phones paired, in `~/.local/state/switchboard/
//! phone/`. A listed device's first visit gets a code (`pending.json`: the
//! device, the code, when it runs out); you type that code in the
//! workspace (the box `switchboard phone` asks the server to open, or
//! `switchboard phone pair <code>`), which marks the pairing approved; the
//! phone's next look takes it, gets a token as its cookie, and the token's
//! SHA-256 is kept in `devices.json` beside the device's name. Typing the
//! code, rather than answering a yes or no, means a stray Enter in the
//! workspace can't pair anything.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::config;
use crate::util;

/// How long a code lasts.
pub const CODE_LIFE_MS: i64 = 5 * 60 * 1000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub device: String,
    pub code: String,
    pub expires: i64,
    #[serde(default)]
    pub approved: bool,
}

/// A paired phone: its Tailscale name, its token's SHA-256 in hex, and when
/// it was paired (ms).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub device: String,
    pub token: String,
    pub paired: i64,
}

fn pending_file() -> PathBuf {
    config::state_dir().join("pending.json")
}

fn devices_file() -> PathBuf {
    config::state_dir().join("devices.json")
}

/// The code as typed, its digits alone: "482 917" and "482917" are one.
pub fn digits(code: &str) -> String {
    code.chars().filter(char::is_ascii_digit).collect()
}

/// A code shown in two halves, as a phone shows it.
pub fn shown(code: &str) -> String {
    let (a, b) = code.split_at(code.len() / 2);
    format!("{a} {b}")
}

pub fn pending() -> Option<Pending> {
    serde_json::from_str(&util::read_text(pending_file())?).ok()
}

/// A pairing started for `device` with `code`, in place of any before it.
pub fn start(device: &str, code: &str, now: i64) -> std::io::Result<()> {
    let p = Pending {
        device: device.to_string(),
        code: code.to_string(),
        expires: now + CODE_LIFE_MS,
        approved: false,
    };
    write_private(
        pending_file(),
        &serde_json::to_string(&p).unwrap_or_default(),
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Approval {
    /// Approved; the phone takes it on its next look.
    Paired(String),
    WrongCode,
    NoneWaiting,
}

impl Approval {
    pub fn words(&self) -> String {
        match self {
            Approval::Paired(device) => format!("Paired {device}."),
            Approval::WrongCode => "That isn't the code the phone shows.".into(),
            Approval::NoneWaiting => "No phone is waiting to pair.".into(),
        }
    }
}

/// What typing `code` does to `pending`: approve it, when it is the code
/// and hasn't run out.
pub fn judge(pending: Option<&Pending>, code: &str, now: i64) -> Approval {
    match pending {
        Some(p) if p.expires <= now => Approval::NoneWaiting,
        Some(p) if !p.code.is_empty() && digits(code) == p.code => {
            Approval::Paired(p.device.clone())
        }
        Some(_) => Approval::WrongCode,
        None => Approval::NoneWaiting,
    }
}

/// The code typed in the workspace: the pairing waiting approved when it
/// matches.
pub fn approve(code: &str, now: i64) -> Approval {
    let pending = pending();
    let approval = judge(pending.as_ref(), code, now);
    if let (Approval::Paired(_), Some(mut p)) = (&approval, pending) {
        p.approved = true;
        if write_private(
            pending_file(),
            &serde_json::to_string(&p).unwrap_or_default(),
        )
        .is_err()
        {
            return Approval::NoneWaiting;
        }
    }
    approval
}

/// Whether `device`'s pairing with `code` was approved; it is taken (the
/// pending file removed) when it was.
pub fn take_approved(device: &str, code: &str, now: i64) -> bool {
    let taken = pending().is_some_and(|p| {
        p.approved && p.expires > now && p.code == code && p.device.eq_ignore_ascii_case(device)
    });
    if taken {
        let _ = std::fs::remove_file(pending_file());
    }
    taken
}

/// A pairing refused or run out, taken away.
pub fn cancel() {
    let _ = std::fs::remove_file(pending_file());
}

pub fn token_hash(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn devices() -> Vec<Device> {
    util::read_text(devices_file())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn write_devices(devices: &[Device]) -> std::io::Result<()> {
    write_private(
        devices_file(),
        &serde_json::to_string_pretty(devices).unwrap_or_default(),
    )
}

/// `device` paired with `token`, in place of a pairing it had before.
pub fn add(device: &str, token: &str, now: i64) -> std::io::Result<()> {
    let mut all = devices();
    all.retain(|d| !d.device.eq_ignore_ascii_case(device));
    all.push(Device {
        device: device.to_string(),
        token: token_hash(token),
        paired: now,
    });
    write_devices(&all)
}

/// `device`'s pairing taken away, and its push subscriptions with it;
/// whether it had one.
pub fn revoke(device: &str) -> std::io::Result<bool> {
    let mut all = devices();
    let before = all.len();
    all.retain(|d| !d.device.eq_ignore_ascii_case(device));
    if all.len() == before {
        return Ok(false);
    }
    write_devices(&all)?;
    let pushes = config::state_dir().join("push.json");
    if let Some(serde_json::Value::Array(mut subs)) = util::read_json(&pushes) {
        subs.retain(|s| {
            !s["device"]
                .as_str()
                .unwrap_or("")
                .eq_ignore_ascii_case(device)
        });
        write_private(
            pushes,
            &serde_json::to_string_pretty(&subs).unwrap_or_default(),
        )?;
    }
    Ok(true)
}

/// Whether `token` is the one `device` was paired with.
pub fn token_matches(devices: &[Device], device: &str, token: &str) -> bool {
    let hash = token_hash(token);
    devices.iter().any(|d| {
        d.device.eq_ignore_ascii_case(device)
            && d.token.len() == hash.len()
            && d.token
                .bytes()
                .zip(hash.bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
    })
}

fn write_private(file: PathBuf, text: &str) -> std::io::Result<()> {
    util::make_dirs(config::state_dir())?;
    let tmp = file.with_extension("tmp");
    util::write_private(&tmp, text, false)?;
    std::fs::rename(tmp, file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waiting(expires: i64) -> Pending {
        Pending {
            device: "my-phone".into(),
            code: "482917".into(),
            expires,
            approved: false,
        }
    }

    #[test]
    fn only_the_code_shown_before_it_runs_out_pairs() {
        let p = waiting(1000);
        assert_eq!(
            judge(Some(&p), "482 917", 10),
            Approval::Paired("my-phone".into())
        );
        assert_eq!(judge(Some(&p), "482918", 10), Approval::WrongCode);
        assert_eq!(judge(Some(&p), "", 10), Approval::WrongCode);
        assert_eq!(judge(Some(&p), "482917", 1000), Approval::NoneWaiting);
        assert_eq!(judge(None, "482917", 10), Approval::NoneWaiting);
    }

    #[test]
    fn a_token_matches_its_own_device_alone() {
        let devices = vec![Device {
            device: "My-Phone".into(),
            token: token_hash("secret"),
            paired: 0,
        }];
        assert!(token_matches(&devices, "my-phone", "secret"));
        assert!(!token_matches(&devices, "my-phone", "other"));
        assert!(!token_matches(&devices, "pad", "secret"));
    }

    #[test]
    fn a_code_shows_in_two_halves() {
        assert_eq!(shown("482917"), "482 917");
        assert_eq!(digits(" 48-2 917\n"), "482917");
    }
}
