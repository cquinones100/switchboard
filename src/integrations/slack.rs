//! Slack's Web API, the one client that calls it: the relay as its bot
//! (`relay::http`, its files sent and fetched too), the Slack view as you
//! (`day::slack_user`, `slack_api`) and the photos as the relay's bot
//! (`photos`), each with the token it acts with. Every request goes through
//! `crate::net`, so each is counted in the request audit, and only to
//! `api_base`, which the tests point at a server of their own.
//!
//! A call goes out the way its caller asks (`Send`): a GET with its
//! arguments in the query, a POST of them as JSON, or a POST of them as a
//! form (objects and arrays as JSON), which every method accepts. Slack's
//! refusal (`ok: false`) and its rate limit come back as an `Error` each
//! caller words its own way. None of this decides what may be called: the
//! relay's rules (`relay::slack::refuse`) are checked before a call gets
//! here.

use std::time::Duration;

use anyhow::bail;
use serde_json::Value;

use crate::net::{self, Audit, Body};

/// A file sent or fetched can be a video of up to 200 MB.
const FILE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Slack's Web API, or `SWITCHBOARD_SLACK_API`, which the tests point at a
/// server of their own, ending in `/`.
pub fn api_base() -> String {
    let base = net::base_url("SWITCHBOARD_SLACK_API", "https://slack.com/api");
    format!("{base}/")
}

/// How a call's arguments go out.
pub enum Send {
    Get,
    Json,
    Form,
}

/// Why a call came back without Slack's answer.
#[derive(Debug)]
pub enum Error {
    /// No answer came: the network, or a timeout.
    Unreachable {
        method: String,
        error: anyhow::Error,
    },
    /// Slack's `ok: false`, or its rate limit (status 429): its error
    /// (empty when it gave none), the scope it said was missing, the HTTP
    /// status and the seconds it asked to wait.
    Refused {
        method: String,
        error: String,
        needed: Option<String>,
        status: u16,
        retry_after: Option<String>,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unreachable { method, error } => write!(f, "{method}: {error}"),
            Error::Refused { method, error, .. } if error.is_empty() => {
                write!(f, "{method}: undefined")
            }
            Error::Refused { method, error, .. } => write!(f, "{method}: {error}"),
        }
    }
}

impl std::error::Error for Error {}

/// Slack's Web API at `base` (ending in `/`), with `token`: the bot's
/// (`xoxb-…`), yours (`xoxp-…`) or the relay app's (`xapp-…`), waiting
/// out Slack's rate limit up to `waits_out` times a call, as long as it asks
/// each time (a second when it doesn't say).
#[derive(Clone)]
pub struct Client {
    pub base: String,
    pub token: String,
    pub waits_out: u32,
}

impl Client {
    pub fn new(base: String, token: String) -> Client {
        Client {
            base,
            token,
            waits_out: 0,
        }
    }

    /// One call: Slack's answer when it says `ok`, counted in the request
    /// audit under `source`.
    pub async fn call(
        &self,
        send: Send,
        method: &str,
        params: &Value,
        source: &str,
    ) -> Result<Value, Error> {
        let url = format!("{}{method}", self.base);
        let bearer = format!("Bearer {}", self.token);
        let headers = [("Authorization", bearer.as_str())];
        let audit = || audit("REST", source);
        let mut waited = 0;
        loop {
            let response = match send {
                Send::Get => {
                    let url = match form_body(params) {
                        query if query.is_empty() => url.clone(),
                        query => format!("{url}?{query}"),
                    };
                    net::get_json(&url, &headers, audit()).await
                }
                Send::Json => net::post(&url, &headers, Body::Json(params), audit()).await,
                Send::Form => {
                    net::post(&url, &headers, Body::Form(form_body(params)), audit()).await
                }
            }
            .map_err(|error| Error::Unreachable {
                method: method.to_string(),
                error,
            })?;
            let retry_after = response.header("Retry-After").map(|s| s.trim().to_string());
            if response.status == 429 && waited < self.waits_out {
                let wait = retry_after
                    .as_deref()
                    .and_then(|s| s.parse::<f64>().ok())
                    .filter(|s| *s > 0.0)
                    .unwrap_or(1.0);
                tokio::time::sleep(Duration::from_secs_f64(wait)).await;
                waited += 1;
                continue;
            }
            if response.status != 429 && response.body.get("ok") == Some(&Value::Bool(true)) {
                return Ok(response.body);
            }
            let text = |k: &str| response.body.get(k).and_then(Value::as_str);
            return Err(Error::Refused {
                method: method.to_string(),
                error: text("error").unwrap_or("").to_string(),
                needed: text("needed").map(str::to_string),
                status: response.status,
                retry_after,
            });
        }
    }

    /// A file's bytes sent to the address Slack gave for it, which needs no
    /// token.
    pub async fn upload(&self, url: &str, bytes: Vec<u8>, source: &str) -> anyhow::Result<()> {
        let body = Body::File(bytes, FILE_TIMEOUT);
        let sent = net::post(url, &[], body, audit("HTTPS", source)).await;
        let status = sent
            .map_err(|e| anyhow::anyhow!("file upload: {e}"))?
            .status;
        if !(200..300).contains(&status) {
            bail!("file upload: HTTP {status}");
        }
        Ok(())
    }

    /// A file Slack keeps, fetched with the token.
    pub async fn download(&self, url: &str, source: &str) -> anyhow::Result<Vec<u8>> {
        let bearer = format!("Bearer {}", self.token);
        let headers = [("Authorization", bearer.as_str())];
        let got = net::get_file(url, &headers, FILE_TIMEOUT, audit("HTTPS", source)).await;
        let got = got.map_err(|e| anyhow::anyhow!("file download: {e}"))?;
        if !(200..300).contains(&got.status) {
            bail!("file download: HTTP {}", got.status);
        }
        Ok(got.bytes)
    }
}

fn audit<'a>(protocol: &'a str, source: &'a str) -> Audit<'a> {
    Audit {
        service: "Slack",
        protocol,
        source,
    }
}

/// A call's arguments as a form: each present one, an object or array as
/// JSON, a string as it is (`URLSearchParams`).
pub fn form_body(params: &Value) -> String {
    let Some(map) = params.as_object() else {
        return String::new();
    };
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in map.iter().filter(|(_, v)| !v.is_null()) {
        let text = match v {
            Value::String(s) => s.clone(),
            Value::Object(_) | Value::Array(_) => crate::util::json_compact(v),
            other => other.to_string(),
        };
        form.append_pair(k, &text);
    }
    form.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn arguments_go_form_encoded_with_objects_as_json() {
        let body = form_body(&json!({
            "channel": "D1",
            "text": "a b&c=é",
            "thread_ts": null,
            "unfurl_links": false,
            "blocks": [{ "type": "markdown" }],
        }));
        assert_eq!(
            body,
            "channel=D1&text=a+b%26c%3D%C3%A9&unfurl_links=false&blocks=%5B%7B%22type%22%3A%22markdown%22%7D%5D"
        );
    }
}
