//! What Tailscale says, read from its JSON (`platform::tailscale_status`
//! and `tailscale_whois`): this computer's tailnet addresses and name, and
//! whether certificates can be had for that name; and who a connection
//! comes from. Plain functions of the JSON, so each refusal is tested
//! without Tailscale.

use std::net::IpAddr;

use serde_json::Value;

/// This computer on the tailnet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// Its Tailscale addresses, the only ones served on.
    pub ips: Vec<IpAddr>,
    /// Its `*.ts.net` name, without the trailing dot.
    pub name: String,
}

/// Who a connection comes from: the login signed in on the device, and the
/// device's Tailscale name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    pub login: String,
    pub device: String,
}

/// `tailscale status --json` read, or which of the three things the phone
/// app needs is missing.
pub fn status_from(v: &Value) -> Result<Status, String> {
    let state = v["BackendState"].as_str().unwrap_or("");
    if state != "Running" {
        let said = if state.is_empty() {
            String::new()
        } else {
            format!(" (it says {state})")
        };
        return Err(format!(
            "Tailscale isn't running{said}: start it and sign in"
        ));
    }
    let ips: Vec<IpAddr> = v["Self"]["TailscaleIPs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|ip| ip.parse().ok())
        .collect();
    if ips.is_empty() {
        return Err("this computer has no tailnet address".into());
    }
    let name = v["Self"]["DNSName"]
        .as_str()
        .unwrap_or("")
        .trim_end_matches('.')
        .to_string();
    let certs: Vec<&str> = v["CertDomains"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if name.is_empty() || !certs.contains(&name.as_str()) {
        return Err(
            "the tailnet has HTTPS certificates off: turn them on in the Tailscale admin console's DNS page"
                .into(),
        );
    }
    Ok(Status { ips, name })
}

/// `tailscale whois --json` read: its login and the device's name (its
/// computed name, else the first part of its DNS name).
pub fn peer_from(v: &Value) -> Option<Peer> {
    let login = v["UserProfile"]["LoginName"].as_str()?.trim();
    let device = v["Node"]["ComputedName"]
        .as_str()
        .filter(|n| !n.is_empty())
        .or_else(|| v["Node"]["Name"].as_str()?.split('.').next())?
        .trim();
    (!login.is_empty() && !device.is_empty()).then(|| Peer {
        login: login.to_string(),
        device: device.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn running() -> Value {
        json!({
            "BackendState": "Running",
            "Self": {"DNSName": "studio.tail1.ts.net.", "TailscaleIPs": ["100.64.0.1", "fd7a::1"]},
            "CertDomains": ["studio.tail1.ts.net"],
        })
    }

    #[test]
    fn a_running_tailnet_gives_its_addresses_and_name() {
        let s = status_from(&running()).unwrap();
        assert_eq!(s.name, "studio.tail1.ts.net");
        assert_eq!(s.ips.len(), 2);
    }

    #[test]
    fn each_missing_piece_is_named() {
        let mut stopped = running();
        stopped["BackendState"] = json!("Stopped");
        assert!(status_from(&stopped).unwrap_err().contains("isn't running"));
        let mut no_ips = running();
        no_ips["Self"]["TailscaleIPs"] = json!([]);
        assert!(
            status_from(&no_ips)
                .unwrap_err()
                .contains("no tailnet address")
        );
        let mut no_certs = running();
        no_certs["CertDomains"] = Value::Null;
        assert!(
            status_from(&no_certs)
                .unwrap_err()
                .contains("certificates off")
        );
    }

    #[test]
    fn whois_names_the_login_and_the_device() {
        let v = json!({
            "Node": {"Name": "my-phone.tail1.ts.net.", "ComputedName": "my-phone"},
            "UserProfile": {"LoginName": "me@example.com"},
        });
        assert_eq!(
            peer_from(&v),
            Some(Peer {
                login: "me@example.com".into(),
                device: "my-phone".into()
            })
        );
        let v = json!({"Node": {"Name": "pad.tail1.ts.net."}, "UserProfile": {"LoginName": "x"}});
        assert_eq!(peer_from(&v).unwrap().device, "pad");
        assert_eq!(peer_from(&json!({})), None);
    }
}
