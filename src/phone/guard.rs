//! Who the phone app answers: every rule a request passes before anything
//! is done for it, as plain functions so each refusal is tested alone. A
//! connection's address must be a tailnet peer Tailscale names (`whois`,
//! each answer kept for a minute), signed in as phone.json's owner, on a
//! device phone.json lists; then, but for pairing, it must carry the cookie
//! that device was paired with; and a write must come from the app's own
//! page (`Origin`), so no other site can make a phone's browser send one.
//! Like the Slack relay's `from_owner`, the owner comes from phone.json
//! alone, never from a request.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use super::pairing::{self, Device};
use super::tailnet::Peer;

/// How long a `whois` answer is kept.
pub const WHOIS_TTL: Duration = Duration::from_secs(60);

/// The cookie a paired device carries.
pub const COOKIE: &str = "sb_phone";

/// Why a request was refused, for the log. The page a refused device sees
/// says none of it, and the first two look alike there, so a device learns
/// nothing of who is allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Tailscale named no device at its address.
    NotOnTailnet,
    /// Signed in to Tailscale as someone else.
    NotOwner,
    /// The owner's, but not a device phone.json lists.
    NotListed,
    /// No cookie, or not the one its device was paired with.
    NotPaired,
    /// A write from another page than the app's.
    ForeignOrigin,
}

impl Refusal {
    pub fn why(self) -> &'static str {
        match self {
            Refusal::NotOnTailnet => "not a device Tailscale names",
            Refusal::NotOwner => "signed in to Tailscale as someone else",
            Refusal::NotListed => "a device phone.json doesn't list",
            Refusal::NotPaired => "not paired",
            Refusal::ForeignOrigin => "a write from another origin",
        }
    }
}

/// What a request is checked against.
#[derive(Clone, Debug)]
pub struct Rules {
    pub owner: String,
    pub devices: Vec<String>,
    /// The app's own origin, `https://<name>:<port>`.
    pub origin: String,
}

/// The device a peer is, when it is the owner's and listed.
pub fn admit(rules: &Rules, peer: Option<&Peer>) -> Result<String, Refusal> {
    let peer = peer.ok_or(Refusal::NotOnTailnet)?;
    if !peer.login.eq_ignore_ascii_case(&rules.owner) {
        return Err(Refusal::NotOwner);
    }
    if !rules
        .devices
        .iter()
        .any(|d| d.eq_ignore_ascii_case(&peer.device))
    {
        return Err(Refusal::NotListed);
    }
    Ok(peer.device.clone())
}

/// A cookie's value in a `Cookie` header.
pub fn cookie<'a>(header: Option<&'a str>, name: &str) -> Option<&'a str> {
    header?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// Whether `device` carries the cookie it was paired with.
pub fn paired(
    devices: &[Device],
    device: &str,
    cookie_header: Option<&str>,
) -> Result<(), Refusal> {
    match cookie(cookie_header, COOKIE) {
        Some(token) if pairing::token_matches(devices, device, token) => Ok(()),
        _ => Err(Refusal::NotPaired),
    }
}

/// A request that changes anything must name the app's own origin; one
/// that only reads needs none.
pub fn same_origin(rules: &Rules, method: &str, origin: Option<&str>) -> Result<(), Refusal> {
    if matches!(method, "GET" | "HEAD") || origin == Some(rules.origin.as_str()) {
        Ok(())
    } else {
        Err(Refusal::ForeignOrigin)
    }
}

/// Tailscale's `whois` answers, each kept for `WHOIS_TTL`.
#[derive(Default)]
pub struct WhoisCache {
    answers: HashMap<IpAddr, (Instant, Option<Peer>)>,
}

impl WhoisCache {
    /// The answer for `ip`, asked of `whois` only when none is kept from
    /// the last minute.
    pub fn peer(
        &mut self,
        ip: IpAddr,
        now: Instant,
        whois: impl FnOnce(IpAddr) -> Option<Peer>,
    ) -> Option<Peer> {
        if let Some((at, peer)) = self.answers.get(&ip)
            && now.duration_since(*at) < WHOIS_TTL
        {
            return peer.clone();
        }
        let peer = whois(ip);
        self.answers
            .retain(|_, (at, _)| now.duration_since(*at) < WHOIS_TTL);
        self.answers.insert(ip, (now, peer.clone()));
        peer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn rules() -> Rules {
        Rules {
            owner: "me@example.com".into(),
            devices: vec!["my-phone".into()],
            origin: "https://studio.tail1.ts.net:8443".into(),
        }
    }

    fn peer(login: &str, device: &str) -> Peer {
        Peer {
            login: login.into(),
            device: device.into(),
        }
    }

    #[test]
    fn the_owners_listed_device_is_admitted() {
        assert_eq!(
            admit(&rules(), Some(&peer("Me@Example.com", "MY-PHONE"))),
            Ok("MY-PHONE".into())
        );
    }

    #[test]
    fn an_address_tailscale_doesnt_name_is_refused() {
        assert_eq!(admit(&rules(), None), Err(Refusal::NotOnTailnet));
    }

    #[test]
    fn another_tailnet_user_is_refused() {
        assert_eq!(
            admit(&rules(), Some(&peer("them@example.com", "my-phone"))),
            Err(Refusal::NotOwner)
        );
    }

    #[test]
    fn an_unlisted_device_is_refused() {
        assert_eq!(
            admit(&rules(), Some(&peer("me@example.com", "laptop"))),
            Err(Refusal::NotListed)
        );
    }

    #[test]
    fn a_device_without_its_cookie_is_refused() {
        let devices = vec![Device {
            device: "my-phone".into(),
            token: pairing::token_hash("t0ken"),
            paired: 0,
        }];
        assert_eq!(
            paired(&devices, "my-phone", Some("a=1; sb_phone=t0ken")),
            Ok(())
        );
        assert_eq!(paired(&devices, "my-phone", None), Err(Refusal::NotPaired));
        assert_eq!(
            paired(&devices, "my-phone", Some("sb_phone=wrong")),
            Err(Refusal::NotPaired)
        );
        assert_eq!(
            paired(&devices, "pad", Some("sb_phone=t0ken")),
            Err(Refusal::NotPaired),
            "another device's cookie is no use"
        );
    }

    #[test]
    fn a_write_from_another_origin_is_refused() {
        let r = rules();
        assert_eq!(same_origin(&r, "GET", None), Ok(()));
        assert_eq!(same_origin(&r, "POST", Some(&r.origin.clone())), Ok(()));
        assert_eq!(
            same_origin(&r, "POST", Some("https://evil.example")),
            Err(Refusal::ForeignOrigin)
        );
        assert_eq!(same_origin(&r, "POST", None), Err(Refusal::ForeignOrigin));
    }

    #[test]
    fn whois_is_asked_once_a_minute() {
        let asked = Cell::new(0);
        let whois = |_| {
            asked.set(asked.get() + 1);
            Some(peer("me@example.com", "my-phone"))
        };
        let mut cache = WhoisCache::default();
        let ip: IpAddr = "100.64.0.2".parse().unwrap();
        let t0 = Instant::now();
        assert!(cache.peer(ip, t0, whois).is_some());
        assert!(
            cache
                .peer(ip, t0 + Duration::from_secs(30), whois)
                .is_some()
        );
        assert_eq!(asked.get(), 1);
        cache.peer(ip, t0 + WHOIS_TTL, whois);
        assert_eq!(asked.get(), 2);
    }
}
