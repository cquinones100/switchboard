//! Slack over the network (lib/slack-relay/slack.js's `apiCall`, `putFile`,
//! `getFile` and `socketMode`): the Web API and the files through
//! `integrations::slack`'s client, as the bot, and Socket Mode's WebSocket
//! with tokio-tungstenite, on rustls. Every request is counted in the
//! request audit as it is made.
//!
//! A Web API call goes out form-encoded, which every method accepts, and a
//! rate limit is waited out as Slack asks, up to three times. None of this
//! decides what may be called: `Slack::api` has already refused anything
//! that isn't for the DM before a call gets here.
//!
//! Built without the `net` feature, each of these says Switchboard can't
//! reach Slack, and Socket Mode never connects.

use anyhow::Result;
use futures_util::future::LocalBoxFuture;
use serde_json::Value;

use super::slack::Transport;
use crate::integrations::slack::{Client, Send};

/// The relay's way to Slack: the Web API at `base`, as the bot.
pub fn as_bot(base: String, bot_token: String) -> Client {
    Client {
        waits_out: 3,
        ..Client::new(base, bot_token)
    }
}

impl Transport for Client {
    fn call<'a>(&'a self, method: &'a str, params: &'a Value) -> LocalBoxFuture<'a, Result<Value>> {
        Box::pin(async move {
            let source = format!("relay: {method}");
            Ok(Client::call(self, Send::Form, method, params, &source).await?)
        })
    }

    fn put<'a>(&'a self, url: &'a str, bytes: Vec<u8>) -> LocalBoxFuture<'a, Result<()>> {
        Box::pin(self.upload(url, bytes, "relay: file upload"))
    }

    fn get<'a>(&'a self, url: &'a str) -> LocalBoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(self.download(url, "relay: file download"))
    }
}

// ── Socket Mode ─────────────────────────────────────────────────────────────

/// Socket Mode: Slack sends each event down the socket in an envelope,
/// acknowledged before anything else (see `slack::envelope`). Slack also
/// asks the app to reconnect now and then (a "disconnect"), and a dropped
/// connection is opened again, waiting longer after each failure in a row.
/// Every (re)connection catches up on what was sent while the socket was
/// down. Runs until the relay stops.
#[cfg(feature = "net")]
pub async fn socket_mode(cx: super::Cx, app_token: String) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as Ws;

    use super::slack::{Envelope, RECONNECT_DELAYS_MS, envelope};

    let mut failures = 0usize;
    loop {
        if cx.life.borrow().stopping {
            return;
        }
        let why = match open_socket(&app_token).await {
            Err(e) => e.to_string(),
            Ok(url) => match tokio_tungstenite::connect_async(url.as_str()).await {
                Err(e) => format!("couldn't connect: {e}"),
                Ok((mut socket, _)) => {
                    crate::requests::count("Slack", "WebSocket", "relay: Socket Mode");
                    while let Some(message) = socket.next().await {
                        if cx.life.borrow().stopping {
                            let _ = socket.close(None).await;
                            return;
                        }
                        let text = match message {
                            Ok(Ws::Text(t)) => t.to_string(),
                            Ok(Ws::Close(_)) | Err(_) => break,
                            Ok(_) => continue,
                        };
                        let Some((ack, what)) = envelope(&text) else {
                            continue;
                        };
                        if let Some(ack) = ack
                            && socket.send(Ws::text(ack)).await.is_err()
                        {
                            break;
                        }
                        match what {
                            Envelope::Hello => {
                                failures = 0;
                                cx.log("Slack: connected");
                                super::run::on_connect(&cx);
                            }
                            Envelope::Disconnect => break,
                            Envelope::Event { event, team, app } => {
                                super::run::on_event(&cx, event, team, app)
                            }
                            Envelope::Interactive(payload) => {
                                super::run::on_interactive(&cx, payload)
                            }
                            Envelope::Other => {}
                        }
                    }
                    "connection closed".to_string()
                }
            },
        };
        if cx.life.borrow().stopping {
            return;
        }
        let delay = RECONNECT_DELAYS_MS[failures.min(RECONNECT_DELAYS_MS.len() - 1)];
        failures += 1;
        cx.log(&format!("Slack: {why}; reconnecting in {}s", delay / 1000));
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    }
}

/// The socket's address, from `apps.connections.open` with the app-level
/// token.
#[cfg(feature = "net")]
async fn open_socket(app_token: &str) -> Result<String> {
    let reply = Client::new(crate::integrations::slack::api_base(), app_token.into())
        .call(
            Send::Form,
            "apps.connections.open",
            &Value::Null,
            "relay: apps.connections.open",
        )
        .await?;
    reply
        .get("url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("apps.connections.open: no address"))
}

#[cfg(not(feature = "net"))]
pub async fn socket_mode(cx: super::Cx, _app_token: String) {
    cx.log("Slack: Switchboard was built without network support, so it can't reach Slack.");
}

#[cfg(all(test, feature = "net"))]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_api_client_form_encodes_waits_out_rate_limits_and_reports_refusals() {
        use crate::fake_http::{Fake, Reply, Service};
        // A Slack of the test's own, as strict as Slack about the token and
        // the method, answering each request with the next of these.
        let fake = Fake::canned(
            Service::Slack {
                token: "xoxb-test".into(),
            },
            vec![
                Reply::json(429, json!({ "ok": false, "error": "ratelimited" }))
                    .header("Retry-After", "1"),
                Reply::json(200, json!({ "ok": true, "ts": "1.2" })),
                Reply::json(200, json!({ "ok": false, "error": "channel_not_found" })),
            ],
        );
        let base = format!("{}/api/", fake.url);
        let https = as_bot(base, "xoxb-test".into());
        let slack = crate::relay::slack::Slack::new(Box::new(https));
        *slack.channel.borrow_mut() = Some("D1".into());
        *slack.owner.borrow_mut() = Some("U1".into());
        crate::util::block_on(async {
            let started = std::time::Instant::now();
            let sent = slack
                .api(
                    "chat.postMessage",
                    json!({ "channel": "D1", "text": "hi there" }),
                )
                .await
                .unwrap();
            assert_eq!(sent["ts"], "1.2");
            assert!(
                started.elapsed() >= std::time::Duration::from_millis(900),
                "the limit was waited out"
            );
            let refused = slack
                .api("chat.postMessage", json!({ "channel": "D1", "text": "x" }))
                .await
                .unwrap_err();
            assert_eq!(refused.to_string(), "chat.postMessage: channel_not_found");
            // Another conversation never reaches Slack at all.
            let elsewhere = slack
                .api("chat.postMessage", json!({ "channel": "C2", "text": "x" }))
                .await
                .unwrap_err();
            assert!(elsewhere.to_string().contains("only uses its DM, not C2"));
        });
        let seen = fake.seen();
        assert_eq!(
            (seen[0].method.as_str(), seen[0].path()),
            ("POST", "/api/chat.postMessage")
        );
        assert_eq!(seen[0].header("authorization"), Some("Bearer xoxb-test"));
        assert_eq!(seen[0].text(), "channel=D1&text=hi+there");
        assert_eq!(seen.len(), 3, "three requests in all, none for C2");
    }
}
