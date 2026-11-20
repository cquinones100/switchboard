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

fn subscriptions_file() -> std::path::PathBuf {
    config::state_dir().join("push.json")
}

pub fn subscriptions() -> Vec<Subscription> {
    util::read_text(subscriptions_file())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn write_subscriptions(all: &[Subscription]) -> std::io::Result<()> {
    util::make_dirs(config::state_dir())?;
    util::write_private(
        subscriptions_file(),
        serde_json::to_string_pretty(all).unwrap_or_default(),
        false,
    )
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

/// Sends `text` (and the tab it opens, `tab`) to every subscription; one
/// the service says is gone (404, 410) is forgotten.
pub async fn send(key: &EcdsaKeyPair, title: &str, tab: &str) {
    let payload = serde_json::json!({"title": title, "tab": tab}).to_string();
    let mut gone = Vec::new();
    // A device taken out of phone.json since it subscribed is sent nothing.
    for sub in subscriptions()
        .into_iter()
        .filter(|s| super::web::listed(&s.device))
    {
        match send_one(key, &sub, payload.as_bytes()).await {
            Ok(status) if status == 404 || status == 410 => gone.push(sub.endpoint.clone()),
            Ok(status) if !(200..300).contains(&status) => {
                eprintln!("phone: a push to {} was refused ({status})", sub.device)
            }
            Ok(_) => {}
            Err(e) => eprintln!("phone: a push to {}: {e:#}", sub.device),
        }
    }
    if !gone.is_empty() {
        let mut all = subscriptions();
        all.retain(|s| !gone.contains(&s.endpoint));
        let _ = write_subscriptions(&all);
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
}
