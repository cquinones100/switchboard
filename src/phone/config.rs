//! `~/.config/switchboard/phone.json`, which names who the phone app
//! answers to: `owner`, your Tailscale login, and `devices`, the Tailscale
//! names of the devices allowed to pair, and optionally `port`. It is read
//! through `paths::local_config_dir()`, so it never leaves the computer, and
//! never written for you.

use std::path::PathBuf;

use serde_json::Value;

use crate::{paths, util};

/// The port served on when phone.json names none.
pub const DEFAULT_PORT: u16 = 8443;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub owner: String,
    pub devices: Vec<String>,
    pub port: u16,
}

pub fn file() -> PathBuf {
    paths::local_config_dir().join("phone.json")
}

/// Where the phone app keeps what it holds: the paired devices, a pairing
/// under way, the push subscriptions, the certificate, its lock.
pub fn state_dir() -> PathBuf {
    paths::state_dir().join("phone")
}

/// phone.json read, or what is wrong with it.
pub fn read() -> Result<Config, String> {
    let file = file();
    let text = util::read_text(&file).ok_or_else(|| {
        format!(
            "{} isn't there; write it as {{ \"owner\": \"<your Tailscale login>\", \"devices\": [\"<your phone's Tailscale name>\"] }}",
            file.display()
        )
    })?;
    parse(&text).map_err(|why| format!("{}: {why}", file.display()))
}

pub fn parse(text: &str) -> Result<Config, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let owner = v["owner"]
        .as_str()
        .map(str::trim)
        .filter(|o| !o.is_empty())
        .ok_or("names no owner, your Tailscale login")?
        .to_string();
    let devices: Vec<String> = v["devices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string)
        .collect();
    if devices.is_empty() {
        return Err("lists no devices, your phone's Tailscale name".into());
    }
    let port = match &v["port"] {
        Value::Null => DEFAULT_PORT,
        p => p
            .as_u64()
            .and_then(|p| u16::try_from(p).ok())
            .filter(|p| *p > 0)
            .ok_or("its port isn't a port number")?,
    };
    Ok(Config {
        owner,
        devices,
        port,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phone_json_names_an_owner_and_devices() {
        let c = parse(r#"{"owner": "me@example.com", "devices": ["my-phone"]}"#).unwrap();
        assert_eq!(c.owner, "me@example.com");
        assert_eq!(c.devices, ["my-phone"]);
        assert_eq!(c.port, DEFAULT_PORT);
        assert_eq!(
            parse(r#"{"owner": "a", "devices": ["b"], "port": 9443}"#)
                .unwrap()
                .port,
            9443
        );
        assert!(
            parse(r#"{"devices": ["b"]}"#)
                .unwrap_err()
                .contains("owner")
        );
        assert!(
            parse(r#"{"owner": "a", "devices": []}"#)
                .unwrap_err()
                .contains("devices")
        );
        assert!(parse(r#"{"owner": "a", "devices": ["b"], "port": 70000}"#).is_err());
    }
}
