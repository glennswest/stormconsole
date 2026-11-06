//! What a guest is told when it asks who it is (#11).
//!
//! On a node a guest's answer at 169.254.169.254 comes from the kubelet, not from stormimds's own store
//! (stormimds#12), so `/admin/instances` is empty everywhere it matters and this asks what stormimds asks:
//! `GET :10250/vmInstance/{address}`, for each address the machine holds. The answer is the guest's identity.
//! No admin key: the kubelet takes any bearer the apiserver vouches for.
//!
//! The interesting answers are the ones where there is nothing to read inside the guest: nobody at that
//! address (it boots with no datasource, which looks like a broken network from inside), another machine at
//! it, or a node that has not synced (the guest is told to wait).

use serde_json::{json, Value};

/// Every address a machine holds, as the node reports them, v4 first: each is a separate question.
pub fn addresses(instance: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for i in instance.pointer("/status/interfaces").and_then(Value::as_array).into_iter().flatten() {
        let many = i.get("ipAddresses").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str);
        for a in many.chain(i.get("ipAddress").and_then(Value::as_str)) {
            if !a.is_empty() && !out.iter().any(|x| x == a) {
                out.push(a.to_string());
            }
        }
    }
    out.sort_by_key(|a| a.contains(':'));
    out
}

/// The node's InternalIP, out of its object.
pub fn node_address(node: &Value) -> Option<String> {
    node.pointer("/status/addresses")?
        .as_array()?
        .iter()
        .find(|a| a["type"] == "InternalIP")?["address"]
        .as_str()
        .map(str::to_string)
}

/// One address's answer, in words. `uid` is the machine asked about, because an address can answer as another.
pub fn answer(address: &str, status: u16, body: &Value, uid: &str) -> Value {
    let said = body.get("error").and_then(Value::as_str).unwrap_or("").to_string();
    match status {
        200 => {
            let id = body["instance_id"].as_str().unwrap_or("");
            let theirs = !uid.is_empty() && id != uid;
            json!({
                "address": address,
                "state": if theirs { "another" } else { "answered" },
                "sentence": if theirs {
                    format!("This address answers as another machine ({id}), so this guest would be told that machine's identity.")
                } else {
                    "The guest is told this.".to_string()
                },
                "instanceId": id,
                "hostname": body.get("hostname"),
                "localIpv4": body.get("local_ipv4"),
                "region": body.get("region"),
                "zone": body.get("zone"),
                "tags": body.get("tags"),
                "labels": body.get("labels"),
                "launchedAt": body.get("launched_at"),
                "serviceAccount": body.get("service_account"),
                "interfaces": body.pointer("/network/interfaces"),
                "told": told(body),
            })
        }
        404 => json!({
            "address": address,
            "state": "nobody",
            "sentence": "The node has no answer for this address, so a guest asking from it is told nobody is here: no identity and no keys, and cloud-init reports no datasource.",
            "said": said,
        }),
        503 => json!({
            "address": address,
            "state": "wait",
            "sentence": "The node cannot answer yet, and a guest asking now is told to wait and ask again.",
            "said": said,
        }),
        401 | 403 => json!({
            "address": address,
            "state": "refused",
            "sentence": format!("The kubelet refused the console's credential ({status}), so what the guest is told cannot be read from here. It checks the bearer with the apiserver."),
        }),
        s => json!({"address": address, "state": "unknown", "sentence": format!("The kubelet answered {s}."), "said": said}),
    }
}

/// Keys and user-data. Neither is in the answer yet: `public_keys` and `user_data` are what rustkube-node#168
/// proposes, and stormimds has to map them too (stormimds#20). A key's type and comment only, and user-data's
/// size only: this says whether it arrives, and a machine's YAML is where its content is.
fn told(body: &Value) -> Value {
    let keys: Vec<String> = body
        .get("public_keys")
        .and_then(Value::as_array)
        .map(|l| {
            l.iter()
                .filter_map(Value::as_str)
                .map(|k| {
                    let mut f = k.split_whitespace();
                    let kind = f.next().unwrap_or("");
                    let comment = f.nth(1).unwrap_or("no comment");
                    format!("{kind} {comment}")
                })
                .collect()
        })
        .unwrap_or_default();
    let user_data = body.get("user_data").and_then(Value::as_str).map(str::len);
    json!({
        "keys": keys,
        "keysSentence": if keys.is_empty() {
            "Only the node's default keys, if it has any. The node does not pass on the keys this machine asked for yet (rustkube-node#168, stormimds#20). A key in the machine's seed disk still arrives that way."
        } else {
            "The keys this machine asked for."
        },
        "userDataBytes": user_data,
        "userDataSentence": if user_data.is_some() {
            "Its user-data."
        } else {
            "None from metadata yet (rustkube-node#168, stormimds#20). User-data in the machine's seed disk still arrives that way."
        },
    })
}

/// Which of these a guest gets depends on the address it asks from, so the worst is the headline: a machine
/// with one answer and one nobody is still a machine that works from one of them, and says so per address.
pub fn verdict(running: bool, rows: &[Value]) -> Value {
    let any = |s: &str| rows.iter().any(|r| r["state"] == s);
    let (state, sentence) = if !running {
        ("stopped", "Not running: there is no guest to ask.")
    } else if rows.is_empty() {
        ("noaddress", "The node reports no address for this guest yet. The metadata service knows a guest by the address it asks from, so until it has one it has no identity.")
    } else if any("answered") && !any("another") {
        ("answered", "The node answers for this guest.")
    } else if any("another") {
        ("another", "An address of this guest answers as another machine.")
    } else if any("refused") {
        ("refused", "This could not be read: the kubelet refused the console's credential.")
    } else if any("wait") {
        ("wait", "The node has not synced: a guest asking now is told to wait.")
    } else if any("nobody") {
        ("nobody", "The node has no answer for this guest, so it will come up with no identity and no keys from metadata.")
    } else {
        ("unknown", "The kubelet could not be asked.")
    };
    json!({"state": state, "sentence": sentence})
}

/// The stormimds service, for its row. A node's guests are the kubelet's to describe, so this says only that
/// the service guests talk to is there.
pub fn service_sentence(up: bool, not_started: bool, why: &str) -> String {
    if up {
        "Serves guests' metadata at 169.254.169.254, from the kubelet's answers".to_string()
    } else if not_started {
        "stormimds is not started on this node, so a guest here gets no answer at 169.254.169.254 and uses its seed disk".to_string()
    } else {
        format!("stormimds did not answer: {why}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_address_once_v4_first() {
        let i = json!({"status": {"interfaces": [
            {"ipAddress": "fd00::1", "ipAddresses": ["fd00::1", "10.0.0.1"]},
            {"ipAddress": ""},
            {"ipAddress": "10.0.0.2"},
        ]}});
        assert_eq!(addresses(&i), ["10.0.0.1", "10.0.0.2", "fd00::1"]);
        assert!(addresses(&json!({})).is_empty());
    }

    #[test]
    fn a_nodes_internal_address() {
        let n = json!({"status": {"addresses": [{"type": "Hostname", "address": "n1"}, {"type": "InternalIP", "address": "10.0.0.9"}]}});
        assert_eq!(node_address(&n).as_deref(), Some("10.0.0.9"));
        assert_eq!(node_address(&json!({})), None);
    }

    // As rustkube-node's vm_manager.rs builds it.
    fn real() -> Value {
        json!({"instance_id": "u1", "hostname": "web-1", "local_ipv4": "10.0.0.1", "region": "storm", "zone": "n1",
               "tags": {"namespace": "shop", "name": "web-1"}, "labels": {}, "network": {"interfaces": []}, "launched_at": 1})
    }

    #[test]
    fn the_answers_in_words() {
        let a = answer("10.0.0.1", 200, &real(), "u1");
        assert_eq!((a["state"].as_str(), a["hostname"].as_str()), (Some("answered"), Some("web-1")));
        assert_eq!(a["told"]["keys"], json!([]));
        assert!(a["told"]["keysSentence"].as_str().unwrap().contains("rustkube-node#168"));
        assert_eq!(a["told"]["userDataBytes"], Value::Null);
        assert_eq!(answer("10.0.0.1", 200, &real(), "other")["state"], "another");
        assert_eq!(answer("10.0.0.1", 404, &json!({"error": "no machine"}), "u1")["state"], "nobody");
        assert_eq!(answer("10.0.0.1", 503, &json!({"error": "not synced"}), "u1")["state"], "wait");
        assert_eq!(answer("10.0.0.1", 401, &json!({}), "u1")["state"], "refused");
        assert_eq!(answer("10.0.0.1", 500, &json!({}), "u1")["state"], "unknown");
    }

    #[test]
    fn keys_when_the_node_passes_them_show_who_and_not_the_key() {
        let mut b = real();
        b["public_keys"] = json!(["ssh-ed25519 AAAAsecret alice@laptop", "ssh-rsa AAAAother"]);
        b["user_data"] = json!("#cloud-config\npassword: hunter2\n");
        let a = answer("10.0.0.1", 200, &b, "u1");
        assert_eq!(a["told"]["keys"], json!(["ssh-ed25519 alice@laptop", "ssh-rsa no comment"]));
        assert_eq!(a["told"]["userDataBytes"], 33);
        let text = a.to_string();
        assert!(!text.contains("AAAA") && !text.contains("hunter2"), "{text}");
    }

    #[test]
    fn the_headline() {
        let row = |s: &str| json!({"state": s});
        let v = |run, rows: &[Value]| verdict(run, rows)["state"].as_str().unwrap().to_string();
        assert_eq!(v(false, &[]), "stopped");
        assert_eq!(v(true, &[]), "noaddress");
        assert_eq!(v(true, &[row("answered"), row("nobody")]), "answered", "a guest asks from its v4, and its v6 has no answer");
        assert_eq!(v(true, &[row("answered"), row("another")]), "another");
        assert_eq!(v(true, &[row("nobody")]), "nobody");
        assert_eq!(v(true, &[row("wait")]), "wait");
        assert_eq!(v(true, &[row("refused"), row("nobody")]), "refused", "not known is not nobody");
    }
}
