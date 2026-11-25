//! Slack's Web API, the one client that calls it: the relay as its bot
//! (`relay::http`), the Slack view as you (`day::slack_user`, `slack_api`)
//! and the photos as the relay's bot (`photos`), each with the token it acts
//! with. Every request goes through `crate::net`, so each is counted in the
//! request audit, and only to `api_base`, which the tests point at a server
//! of their own.
//!
//! A call goes out the way its caller asks (`Send`): a GET with its
//! arguments in the query, a POST of them as JSON, or a POST of them as a
//! form (objects and arrays as JSON), which every method accepts. Slack's
//! refusal (`ok: false`) and its rate limit come back as an `Error` each
//! caller words its own way; a client made `waiting_out_limits` waits a
//! limit out as Slack asks, up to that many times, before giving up. None
//! of this decides what may be called: the relay's rules
//! (`relay::slack::refuse`) are checked before a call gets here.

use std::time::Duration;

use serde_json::Value;

use crate::net::{self, Audit};

/// Slack's Web API, or `SWITCHBOARD_SLACK_API`, which the tests point at a
/// server of their own, ending in `/`.
pub fn api_base() -> String {
    let base = net::base_url("SWITCHBOARD_SLACK_API", "https://slack.com/api");
    format!("{base}/")
}

/// How a call's arguments go out.
#[derive(Clone, Copy)]
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
    /// Slack's rate limit, with the seconds it asked to wait when it said.
    RateLimited {
        method: String,
        retry_after: Option<String>,
    },
    /// Slack's `ok: false`: its error (empty when it gave none), the scope
    /// it said was missing, and the HTTP status.
    Refused {
        method: String,
        error: String,
        needed: Option<String>,
        status: u16,
    },
}

impl Error {
    /// Slack's error code, as its answer's `error` says it.
    pub fn code(&self) -> &str {
        match self {
            Error::Unreachable { .. } => "",
            Error::RateLimited { .. } => "ratelimited",
            Error::Refused { error, .. } => error,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unreachable { method, error } => write!(f, "{method}: {error}"),
            Error::RateLimited { method, .. } => write!(f, "{method}: ratelimited"),
            Error::Refused { method, error, .. } if error.is_empty() => {
                write!(f, "{method}: undefined")
            }
            Error::Refused { method, error, .. } => write!(f, "{method}: {error}"),
        }
    }
}

impl std::error::Error for Error {}

/// Slack's Web API at `base` (ending in `/`), with `token`: the bot's
/// (`xoxb-…`), yours (`xoxp-…`) or the relay app's (`xapp-…`).
#[derive(Clone)]
pub struct Client {
    base: String,
    token: String,
    waits_out: u32,
}

impl Client {
    pub fn new(base: String, token: String) -> Client {
        Client {
            base,
            token,
            waits_out: 0,
        }
    }

    /// The same client, waiting out Slack's rate limit up to `times` times
    /// a call, as long as it asks each time (a second when it doesn't say).
    pub fn waiting_out_limits(self, times: u32) -> Client {
        Client {
            waits_out: times,
            ..self
        }
    }

    /// The Web API's address, ending in `/`.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// The token, for a file's download, which isn't a Web API call.
    pub fn token(&self) -> &str {
        &self.token
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
        let audit = || Audit {
            service: "Slack",
            protocol: "REST",
            source,
        };
        let mut waited = 0;
        loop {
            let response = match send {
                Send::Get => {
                    let query = form_body(params);
                    let url = if query.is_empty() {
                        url.clone()
                    } else {
                        format!("{url}?{query}")
                    };
                    net::get_json(&url, &headers, audit()).await
                }
                Send::Json => net::post_json(&url, &headers, params, audit()).await,
                Send::Form => net::post_form(&url, &headers, form_body(params), audit()).await,
            }
            .map_err(|error| Error::Unreachable {
                method: method.to_string(),
                error,
            })?;
            if response.status == 429 {
                let retry_after = response.header("Retry-After").map(|s| s.trim().to_string());
                if waited < self.waits_out {
                    let wait = retry_after
                        .as_deref()
                        .and_then(|s| s.parse::<f64>().ok())
                        .filter(|s| *s > 0.0)
                        .unwrap_or(1.0);
                    tokio::time::sleep(Duration::from_secs_f64(wait)).await;
                    waited += 1;
                    continue;
                }
                return Err(Error::RateLimited {
                    method: method.to_string(),
                    retry_after,
                });
            }
            if response.body.get("ok") == Some(&Value::Bool(true)) {
                return Ok(response.body);
            }
            let text = |k: &str| response.body.get(k).and_then(Value::as_str);
            return Err(Error::Refused {
                method: method.to_string(),
                error: text("error").unwrap_or("").to_string(),
                needed: text("needed").map(str::to_string),
                status: response.status,
            });
        }
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
