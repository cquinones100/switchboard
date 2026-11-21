//! Web Push to the installed phone app when a tab starts waiting on you:
//! the message encrypted for the phone's browser as RFC 8291 says
//! (`aes128gcm`), and signed with the app's VAPID key (RFC 8292), both
//! with aws-lc-rs, the crypto rustls already builds on. The key is made
//! once and kept as a secret (`platform::set_secret`); the browsers'
//! subscriptions are kept in `~/.local/state/switchboard/phone/push.json`.
//!
//! What is pushed names the tab alone (its number and name), never what
//! Claude asks: a push passes through Apple's or Google's push service,
//! encrypted end to end, and shows on a locked phone. The question is read
//! once the app is open. A subscription is sent to only when its endpoint
//! is one of the browsers' push services, since the address comes from the
//! phone.

use anyhow::{Result, anyhow};
use aws_lc_rs::aead::{AES_128_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use aws_lc_rs::agreement::{self, ECDH_P256, PrivateKey, UnparsedPublicKey};
use aws_lc_rs::hkdf::{HKDF_SHA256, KeyType, Prk, Salt};
use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

use std::path::Path;

use super::config;
use crate::{net, platform, util};

/// The secret the VAPID key is kept as.
const SECRET: &str = "switchboard-phone-vapid";
/// Who the pushes come from, as RFC 8292 asks; the project's page, not
/// yours.
const SUBJECT: &str = "https://github.com/cquinones100/switchboard";
/// The record size written in the header; a push is one record.
const RECORD_SIZE: u32 = 4096;
/// The push services a subscription may name, by the end of its host.
const SERVICES: [&str; 4] = [
    ".push.apple.com",
    "fcm.googleapis.com",
    ".push.services.mozilla.com",
    ".notify.windows.com",
];

/// A browser's subscription: where to send, its key and its secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    pub device: String,
    pub endpoint: String,
    /// The browser's P-256 public key, uncompressed, base64url.
    pub p256dh: String,
    /// Its 16-byte authentication secret, base64url.
    pub auth: String,
}

pub fn subscriptions() -> Vec<Subscription> {
    subscriptions_in(&config::state_dir())
}

fn subscriptions_in(dir: &Path) -> Vec<Subscription> {
    util::read_text(dir.join("push.json"))
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn write_subscriptions(all: &[Subscription]) -> std::io::Result<()> {
    write_subscriptions_in(&config::state_dir(), all)
}

fn write_subscriptions_in(dir: &Path, all: &[Subscription]) -> std::io::Result<()> {
    util::make_dirs(dir)?;
    util::write_private(
        dir.join("push.json"),
        serde_json::to_string_pretty(all).unwrap_or_default(),
        false,
    )
}

/// The subscriptions of the devices phone.json lists: a device taken out of
/// it since it subscribed is sent nothing.
pub fn listed_subscriptions() -> Vec<Subscription> {
    subscriptions()
        .into_iter()
        .filter(|s| super::web::listed(&s.device))
        .collect()
}

/// A device's subscription kept, in place of the one it had.
pub fn subscribe(sub: Subscription) -> Result<()> {
    let host = host_of(&sub.endpoint).ok_or_else(|| anyhow!("not a push service's address"))?;
    if !SERVICES
        .iter()
        .any(|s| host.ends_with(s) || host == s.trim_start_matches('.'))
    {
        return Err(anyhow!("{host} isn't a push service"));
    }
    if b64(&sub.p256dh).map(|k| k.len()) != Some(65) || b64(&sub.auth).map(|a| a.len()) != Some(16)
    {
        return Err(anyhow!("its keys aren't a push subscription's"));
    }
    let mut all = subscriptions();
    all.retain(|s| !s.device.eq_ignore_ascii_case(&sub.device) && s.endpoint != sub.endpoint);
    all.push(sub);
    Ok(write_subscriptions(&all)?)
}

fn host_of(endpoint: &str) -> Option<String> {
    let url = url::Url::parse(endpoint).ok()?;
    (url.scheme() == "https").then(|| url.host_str().map(str::to_string))?
}

fn b64(text: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(text.trim_end_matches('='))
        .ok()
        .or_else(|| STANDARD.decode(text).ok())
}

/// The VAPID key: kept as a secret, made the first time.
pub fn key() -> Result<EcdsaKeyPair> {
    if let Some(kept) = platform::secret(SECRET)
        && let Ok(pkcs8) = STANDARD.decode(kept.trim())
        && let Ok(key) = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &pkcs8)
    {
        return Ok(key);
    }
    let rng = SystemRandom::new();
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
        .map_err(|_| anyhow!("couldn't make a push key"))?;
    platform::set_secret(SECRET, "vapid", &STANDARD.encode(pkcs8.as_ref()))?;
    EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref())
        .map_err(|_| anyhow!("couldn't read the push key"))
}

/// The key's public half, as the page's `applicationServerKey`.
pub fn public_key(key: &EcdsaKeyPair) -> String {
    URL_SAFE_NO_PAD.encode(key.public_key().as_ref())
}

/// The `Authorization` header for a push to `endpoint`: a JWT for its
/// service, good for 12 hours, signed with the key.
pub fn authorization(key: &EcdsaKeyPair, endpoint: &str, now_secs: i64) -> Result<String> {
    let url = url::Url::parse(endpoint)?;
    let aud = format!("{}://{}", url.scheme(), url.host_str().unwrap_or(""));
    let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = URL_SAFE_NO_PAD.encode(
        serde_json::json!({"aud": aud, "exp": now_secs + 12 * 3600, "sub": SUBJECT}).to_string(),
    );
    let signed = format!("{header}.{claims}");
    let signature = key
        .sign(&SystemRandom::new(), signed.as_bytes())
        .map_err(|_| anyhow!("couldn't sign the push"))?;
    Ok(format!(
        "vapid t={signed}.{}, k={}",
        URL_SAFE_NO_PAD.encode(signature.as_ref()),
        public_key(key)
    ))
}

struct Len(usize);

impl KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

fn expand(prk: &Prk, info: &[u8], n: usize) -> Result<Vec<u8>> {
    let mut out = vec![0; n];
    prk.expand(&[info], Len(n))
        .and_then(|okm| okm.fill(&mut out))
        .map_err(|_| anyhow!("HKDF failed"))?;
    Ok(out)
}

/// `payload` encrypted for a browser whose key is `ua_public` and secret
/// `auth`, with this server's one-time key `as_private` and `salt` (RFC
/// 8291, section 3): the header (salt, record size, key) and one record.
pub fn encrypt(
    payload: &[u8],
    ua_public: &[u8],
    auth: &[u8],
    as_private: &PrivateKey,
    salt: &[u8; 16],
) -> Result<Vec<u8>> {
    let as_public = as_private
        .compute_public_key()
        .map_err(|_| anyhow!("no public key"))?;
    let as_public = as_public.as_ref().to_vec();
    let ecdh: Vec<u8> = agreement::agree(
        as_private,
        UnparsedPublicKey::new(&ECDH_P256, ua_public),
        anyhow!("the browser's key isn't a P-256 key"),
        |secret| Ok(secret.to_vec()),
    )?;
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(&as_public);
    let prk_key = Salt::new(HKDF_SHA256, auth).extract(&ecdh);
    let ikm = expand(&prk_key, &key_info, 32)?;
    let prk = Salt::new(HKDF_SHA256, salt).extract(&ikm);
    let cek = expand(&prk, b"Content-Encoding: aes128gcm\0", 16)?;
    let nonce = expand(&prk, b"Content-Encoding: nonce\0", 12)?;
    let sealing = LessSafeKey::new(
        UnboundKey::new(&AES_128_GCM, &cek).map_err(|_| anyhow!("bad content key"))?,
    );
    let mut record = payload.to_vec();
    record.push(2);
    sealing
        .seal_in_place_append_tag(
            Nonce::try_assume_unique_for_key(&nonce).map_err(|_| anyhow!("bad nonce"))?,
            Aad::empty(),
            &mut record,
        )
        .map_err(|_| anyhow!("couldn't encrypt the push"))?;
    let mut out = salt.to_vec();
    out.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    out.push(as_public.len() as u8);
    out.extend_from_slice(&as_public);
    out.extend_from_slice(&record);
    Ok(out)
}

/// What came of a push to one subscription.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The service took it (its status, 2xx).
    Sent(u16),
    /// The service refused it, with its status.
    Refused(u16),
    /// The service says the subscription is gone (404, 410), so it is
    /// forgotten.
    Gone(u16),
    /// No answer came, or the push couldn't be made, and why.
    Failed(String),
}

impl Outcome {
    pub fn words(&self) -> String {
        match self {
            Outcome::Sent(status) => format!("sent ({status})"),
            Outcome::Refused(status) => format!("refused ({status})"),
            Outcome::Gone(status) => {
                format!("gone ({status}), so its subscription is forgotten")
            }
            Outcome::Failed(why) => format!("failed: {why}"),
        }
    }
}

/// Sends `title` (with `body` under it, empty for the app's own
/// "Waiting on you", and the tab it opens, `tab`, empty for none) to every
/// listed subscription, logs each outcome and forgets the gone; gives each
/// device with what came of its push.
pub async fn send(
    key: &EcdsaKeyPair,
    title: &str,
    body: &str,
    tab: &str,
) -> Vec<(String, Outcome)> {
    let payload = serde_json::json!({"title": title, "body": body, "tab": tab}).to_string();
    let sent = deliver(key, listed_subscriptions(), payload.as_bytes()).await;
    record(&config::state_dir(), &sent);
    for (sub, outcome) in &sent {
        eprintln!("phone: a push to {}: {}", sub.device, outcome.words());
    }
    sent.into_iter().map(|(s, o)| (s.device, o)).collect()
}

/// `payload` pushed to each of `subs`. A panic under a push (as macOS's
/// certificate check once gave) is caught and said as its failure, rather
/// than ending the task with nothing said.
async fn deliver(
    key: &EcdsaKeyPair,
    subs: Vec<Subscription>,
    payload: &[u8],
) -> Vec<(Subscription, Outcome)> {
    use futures_util::FutureExt;
    let mut out = Vec::new();
    for sub in subs {
        let pushed = std::panic::AssertUnwindSafe(send_one(key, &sub, payload))
            .catch_unwind()
            .await;
        let outcome = match pushed {
            Ok(Ok(status @ (404 | 410))) => Outcome::Gone(status),
            Ok(Ok(status)) if (200..300).contains(&status) => Outcome::Sent(status),
            Ok(Ok(status)) => Outcome::Refused(status),
            Ok(Err(e)) => Outcome::Failed(format!("{e:#}")),
            Err(panic) => Outcome::Failed(format!("it panicked: {}", panic_words(&*panic))),
        };
        out.push((sub, outcome));
    }
    out
}

fn panic_words(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "no message".into())
}

/// Each outcome written to the log in `dir`, and the gone subscriptions
/// taken out of its push.json.
fn record(dir: &Path, sent: &[(Subscription, Outcome)]) {
    for (sub, outcome) in sent {
        super::log::line_in(dir, &format!("push to {}: {}", sub.device, outcome.words()));
    }
    let gone: Vec<&str> = sent
        .iter()
        .filter(|(_, o)| matches!(o, Outcome::Gone(_)))
        .map(|(s, _)| s.endpoint.as_str())
        .collect();
    if !gone.is_empty() {
        let mut all = subscriptions_in(dir);
        all.retain(|s| !gone.contains(&s.endpoint.as_str()));
        let _ = write_subscriptions_in(dir, &all);
    }
}

async fn send_one(key: &EcdsaKeyPair, sub: &Subscription, payload: &[u8]) -> Result<u16> {
    let ua_public = b64(&sub.p256dh).ok_or_else(|| anyhow!("its key isn't base64"))?;
    let auth = b64(&sub.auth).ok_or_else(|| anyhow!("its secret isn't base64"))?;
    let rng = SystemRandom::new();
    let mut salt = [0u8; 16];
    rng.fill(&mut salt).map_err(|_| anyhow!("no randomness"))?;
    let one_time = PrivateKey::generate(&ECDH_P256).map_err(|_| anyhow!("no key"))?;
    let body = encrypt(payload, &ua_public, &auth, &one_time, &salt)?;
    let authorization = authorization(key, &sub.endpoint, util::now_secs())?;
    let response = net::post_bytes(
        &sub.endpoint,
        &[
            ("Authorization", &authorization),
            ("Content-Encoding", "aes128gcm"),
            ("Content-Type", "application/octet-stream"),
            ("TTL", "3600"),
            ("Urgency", "high"),
        ],
        body,
        net::Audit {
            service: "Web Push",
            protocol: "HTTPS",
            source: "phone app",
        },
    )
    .await?;
    Ok(response.status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(text: &str) -> Vec<u8> {
        URL_SAFE_NO_PAD.decode(text).unwrap()
    }

    /// RFC 8291's example (appendix A), byte for byte.
    #[test]
    fn a_push_is_encrypted_as_rfc_8291_s_example() {
        let as_private = PrivateKey::from_private_key(
            &ECDH_P256,
            &d("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw"),
        )
        .unwrap();
        let ua_public = d(
            "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
        );
        let auth = d("BTBZMqHH6r4Tts7J_aSIgg");
        let salt: [u8; 16] = d("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
        let body = encrypt(
            b"When I grow up, I want to be a watermelon",
            &ua_public,
            &auth,
            &as_private,
            &salt,
        )
        .unwrap();
        assert_eq!(
            URL_SAFE_NO_PAD.encode(body),
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN"
        );
    }

    #[test]
    fn only_a_push_services_address_is_kept() {
        assert_eq!(
            host_of("https://web.push.apple.com/abc").as_deref(),
            Some("web.push.apple.com")
        );
        assert_eq!(host_of("http://web.push.apple.com/abc"), None);
        let sub = |endpoint: &str| Subscription {
            device: "p".into(),
            endpoint: endpoint.into(),
            p256dh: "x".into(),
            auth: "y".into(),
        };
        let refused = subscribe(sub("https://example.com/x")).unwrap_err();
        assert!(refused.to_string().contains("isn't a push service"));
    }

    /// A browser's subscription to `endpoint`, with real keys.
    fn browser(device: &str, endpoint: String) -> Subscription {
        let key = PrivateKey::generate(&ECDH_P256).unwrap();
        let public = key.compute_public_key().unwrap();
        Subscription {
            device: device.into(),
            endpoint,
            p256dh: URL_SAFE_NO_PAD.encode(public.as_ref()),
            auth: URL_SAFE_NO_PAD.encode([7u8; 16]),
        }
    }

    #[test]
    fn a_push_is_sent_logged_and_a_gone_subscription_forgotten() {
        use crate::demo::services::{Fake, Reply, Service};
        let service = Fake::start(Service::Web, |req| match req.path() {
            "/here" => Some(Reply::bytes(201, "text/plain", "")),
            "/gone" => Some(Reply::bytes(410, "text/plain", "")),
            "/no" => Some(Reply::bytes(403, "text/plain", "bad key")),
            _ => None,
        });
        let dir = std::env::temp_dir().join(format!("sb-phone-push-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let subs = vec![
            browser("phone", format!("{}/here", service.url)),
            browser("old-phone", format!("{}/gone", service.url)),
            browser("tablet", format!("{}/no", service.url)),
        ];
        write_subscriptions_in(&dir, &subs).unwrap();
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let key =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref()).unwrap();

        let sent = util::block_on(deliver(&key, subs.clone(), b"{}"));
        record(&dir, &sent);

        let outcomes: Vec<(&str, &Outcome)> =
            sent.iter().map(|(s, o)| (s.device.as_str(), o)).collect();
        assert_eq!(
            outcomes,
            vec![
                ("phone", &Outcome::Sent(201)),
                ("old-phone", &Outcome::Gone(410)),
                ("tablet", &Outcome::Refused(403)),
            ]
        );
        let seen = service.seen();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0].header("content-encoding"), Some("aes128gcm"));
        assert!(
            seen[0]
                .header("authorization")
                .is_some_and(|a| a.starts_with("vapid t=")),
            "signed with the key"
        );
        let log = std::fs::read_to_string(dir.join("phone.log")).unwrap();
        assert!(log.contains("push to phone: sent (201)"), "{log}");
        assert!(log.contains("push to old-phone: gone (410)"), "{log}");
        assert!(log.contains("push to tablet: refused (403)"), "{log}");
        let kept: Vec<String> = subscriptions_in(&dir)
            .into_iter()
            .map(|s| s.device)
            .collect();
        assert_eq!(kept, ["phone", "tablet"], "the gone one is forgotten");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_push_that_reaches_nothing_says_why() {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let key =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref()).unwrap();
        let mut sub = browser("phone", "http://127.0.0.1:9/x".into());
        sub.p256dh = "not a key".into();
        let sent = util::block_on(deliver(&key, vec![sub], b"{}"));
        assert!(
            matches!(&sent[0].1, Outcome::Failed(why) if why.contains("base64")),
            "{sent:?}"
        );
    }
}
