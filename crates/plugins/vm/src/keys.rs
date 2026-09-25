//! A user's SSH public keys, uploaded once and given to every machine they
//! make (#26).
//!
//! **Where they live.** KubeVirt's `spec.accessCredentials` names a Secret,
//! and a Secret is namespaced: a machine can only point at one in its own
//! namespace. So the user's list is a Secret `<user>-ssh-keys` in a home
//! namespace (`[vm] ssh_keys_namespace`), one key per data item, and the
//! console keeps a copy of it in every namespace where that user creates or
//! keys a machine. The copies carry a label naming the user, so an edit on
//! the Account page reaches every one of them.
//!
//! **How they reach a guest.** Two ways, both written:
//!
//! - `accessCredentials` with `propagationMethod: noCloud` — the standard
//!   field, so `virtctl` and `oc` see the keys, and what the node will act on
//!   once stormvm#41 lands; `qemuGuestAgent` for a key added to a machine
//!   that is already running, which is the only way one reaches a live guest.
//! - The cloud-init seed, as before. Nothing on a node honours
//!   `accessCredentials` yet, so the seed is what actually puts a key in a
//!   guest today. A key in both lands in `authorized_keys` once.

use std::collections::BTreeMap;

use base64::Engine;
use serde_json::{json, Value};

/// The label every key Secret carries, home or copy: whose keys these are.
pub const LABEL_FOR: &str = "storm.io/ssh-keys-for";
/// On the home Secret only, so a copy is never mistaken for the original.
pub const LABEL_HOME: &str = "storm.io/ssh-keys-home";

/// The key types OpenSSH accepts in `authorized_keys`.
const TYPES: &[&str] = &[
    "ssh-ed25519",
    "ssh-rsa",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "sk-ssh-ed25519@openssh.com",
    "sk-ecdsa-sha2-nistp256@openssh.com",
];

/// One public key, understood.
#[derive(Debug, Clone, PartialEq)]
pub struct PublicKey {
    pub kind: String,
    pub blob: String,
    pub comment: String,
}

impl PublicKey {
    /// The one line `authorized_keys` takes.
    pub fn line(&self) -> String {
        if self.comment.is_empty() {
            format!("{} {}", self.kind, self.blob)
        } else {
            format!("{} {} {}", self.kind, self.blob, self.comment)
        }
    }

    /// Short and stable, for telling two keys apart on a page: the type and
    /// the blob's last characters, which is what people compare by eye.
    pub fn short(&self) -> String {
        let tail: String = self.blob.chars().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect();
        format!("{} …{tail}", self.kind)
    }
}

/// Read one public key line. A private key pasted by mistake is refused by
/// name, because the paste box is exactly where that happens.
pub fn parse(line: &str) -> Result<PublicKey, String> {
    let line = line.trim();
    if line.contains("PRIVATE KEY") {
        return Err("that is a private key. Paste the .pub file — the private half never leaves your machine".into());
    }
    let mut parts = line.split_whitespace();
    // An authorized_keys line may lead with options (`from="…" ssh-ed25519 …`);
    // a key for a console to hand out should not carry somebody else's.
    let kind = parts.next().unwrap_or_default();
    if !TYPES.contains(&kind) {
        return Err(format!(
            "{:?} is not an SSH public key type. Expected a line like `ssh-ed25519 AAAA… you@host`",
            kind.chars().take(40).collect::<String>()
        ));
    }
    let blob = parts.next().unwrap_or_default();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(blob)
        .map_err(|_| "the key's body is not base64 — was the line cut short?".to_string())?;
    // The blob opens with its own type, length-prefixed. A body that names a
    // different type than the line does is a line assembled by hand.
    let inner = decoded
        .get(4..)
        .and_then(|rest| {
            let n = u32::from_be_bytes(decoded.get(..4)?.try_into().ok()?) as usize;
            rest.get(..n)
        })
        .and_then(|t| std::str::from_utf8(t).ok());
    if inner != Some(kind) {
        return Err(format!("the key's body is not a {kind} key"));
    }
    Ok(PublicKey { kind: kind.into(), blob: blob.into(), comment: parts.collect::<Vec<_>>().join(" ") })
}

/// Every key in a pasted or uploaded file: one per non-blank, non-comment
/// line, so an `authorized_keys` file goes in whole.
pub fn parse_all(text: &str) -> Result<Vec<PublicKey>, String> {
    let keys: Vec<PublicKey> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(parse)
        .collect::<Result<_, _>>()?;
    if keys.is_empty() {
        return Err("no key in what was given".into());
    }
    Ok(keys)
}

/// A DNS-1123 label from anything: what a user's name becomes in a Secret's
/// name and a label value.
pub fn dns(s: &str) -> String {
    let mut out: String = s
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() { "user".into() } else { out.chars().take(50).collect() }
}

pub fn secret_name(user: &str) -> String {
    format!("{}-ssh-keys", dns(user))
}

/// A key's name as a Secret data item: `[-._a-zA-Z0-9]`, never empty.
pub fn item_name(name: &str, key: &PublicKey) -> String {
    let base = if name.trim().is_empty() {
        if key.comment.is_empty() { key.kind.clone() } else { key.comment.clone() }
    } else {
        name.trim().to_string()
    };
    let out: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_') { c } else { '-' })
        .collect();
    let out = out.trim_matches(|c| c == '-' || c == '.').to_string();
    if out.is_empty() { "key".into() } else { out.chars().take(63).collect() }
}

/// The keys a Secret holds, by item name. `data` is base64 as the
/// apiserver returns it; `stringData` is read too, for an object that has
/// not been round-tripped.
pub fn from_secret(secret: &Value) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some(d) = secret.get("data").and_then(Value::as_object) {
        for (k, v) in d {
            if let Some(text) = v
                .as_str()
                .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
                .and_then(|b| String::from_utf8(b).ok())
            {
                out.insert(k.clone(), text.trim().to_string());
            }
        }
    }
    if let Some(d) = secret.get("stringData").and_then(Value::as_object) {
        for (k, v) in d {
            if let Some(t) = v.as_str() {
                out.insert(k.clone(), t.trim().to_string());
            }
        }
    }
    out
}

/// The whole Secret for a user's keys in `ns`. `home` marks the original.
pub fn secret(ns: &str, name: &str, user: &str, home: bool, keys: &BTreeMap<String, String>) -> Value {
    let mut labels = json!({ LABEL_FOR: dns(user) });
    if home {
        labels[LABEL_HOME] = json!("true");
    }
    json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": {
            "name": name,
            "namespace": ns,
            "labels": labels,
            "annotations": {"storm.io/ssh-keys-user": user},
        },
        "type": "Opaque",
        "stringData": keys,
    })
}

/// A Secret of the keys a machine was given when that was not all of the
/// user's: named after the machine, and so not refreshed when the user's
/// list changes — which is what choosing a subset means.
pub fn machine_secret_name(vm: &str) -> String {
    format!("{}-ssh-keys", vm.trim())
}

/// One `accessCredentials` entry.
///
/// `noCloud` at create: the keys go into the cloud-init the guest reads at
/// first boot. `qemuGuestAgent` for a key added later: the agent writes
/// `authorized_keys` for the named users while the guest runs, and follows
/// the Secret when it changes. Root and nothing else, because that is who
/// the seed authorizes and who people log in as here; the image's own user
/// has its name baked into the image and is not knowable from the spec.
pub fn access_credential(secret: &str, agent: bool) -> Value {
    let method = if agent {
        json!({"qemuGuestAgent": {"users": ["root"]}})
    } else {
        json!({"noCloud": {}})
    };
    json!({
        "sshPublicKey": {
            "source": {"secret": {"secretName": secret}},
            "propagationMethod": method,
        }
    })
}

/// The Secrets a machine's spec names for SSH keys, and how each reaches
/// the guest.
pub fn credentials(spec: &Value) -> Vec<(String, String)> {
    spec.pointer("/accessCredentials")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let secret = c.pointer("/sshPublicKey/source/secret/secretName")?.as_str()?;
            let m = c.pointer("/sshPublicKey/propagationMethod")?.as_object()?;
            let how = m.keys().next().cloned().unwrap_or_default();
            Some((secret.to_string(), how))
        })
        .collect()
}

/// The keys a cloud-init user-data authorizes, read back: every
/// `ssh_authorized_keys` entry, once.
pub fn from_seed(userdata: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in userdata.lines() {
        let t = l.trim().trim_start_matches("- ").trim();
        if TYPES.iter().any(|k| t.starts_with(&format!("{k} "))) && !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // A real ed25519 public key body (the type string, length-prefixed, then 32 bytes).
    fn ed25519(comment: &str) -> String {
        let mut b = vec![0, 0, 0, 11];
        b.extend_from_slice(b"ssh-ed25519");
        b.extend_from_slice(&[0, 0, 0, 32]);
        b.extend_from_slice(&[7u8; 32]);
        format!("ssh-ed25519 {} {comment}", base64::engine::general_purpose::STANDARD.encode(b)).trim().to_string()
    }

    #[test]
    fn a_public_key_is_read_and_a_private_one_refused() {
        let k = parse(&ed25519("gw@mac")).unwrap();
        assert_eq!(k.kind, "ssh-ed25519");
        assert_eq!(k.comment, "gw@mac");
        assert_eq!(k.line(), ed25519("gw@mac"));
        assert!(parse("-----BEGIN OPENSSH PRIVATE KEY-----\nb3Blb").unwrap_err().contains("private key"));
        assert!(parse("hello world").unwrap_err().contains("not an SSH public key type"));
        assert!(parse("ssh-ed25519 !!!").unwrap_err().contains("base64"));
        // An rsa line whose body says ed25519: assembled by hand, refused.
        let body = ed25519("").split_whitespace().nth(1).unwrap().to_string();
        assert!(parse(&format!("ssh-rsa {body}")).unwrap_err().contains("not a ssh-rsa key"));
    }

    #[test]
    fn a_file_of_keys_goes_in_whole() {
        let text = format!("# mine\n{}\n\n{}\n", ed25519("laptop"), ed25519("desk"));
        let ks = parse_all(&text).unwrap();
        assert_eq!(ks.len(), 2);
        assert!(parse_all("\n# nothing\n").unwrap_err().contains("no key"));
    }

    #[test]
    fn names_are_what_the_apiserver_accepts() {
        assert_eq!(secret_name("Glenn West"), "glenn-west-ssh-keys");
        assert_eq!(secret_name(""), "user-ssh-keys");
        let k = parse(&ed25519("gw@mac book")).unwrap();
        assert_eq!(item_name("", &k), "gw-mac-book");
        assert_eq!(item_name("my laptop!", &k), "my-laptop");
    }

    #[test]
    fn a_secret_round_trips_and_says_whose_it_is() {
        let mut keys = BTreeMap::new();
        keys.insert("laptop".to_string(), ed25519("laptop"));
        let s = secret("default", "gw-ssh-keys", "gw", true, &keys);
        assert_eq!(s["metadata"]["labels"][LABEL_FOR], "gw");
        assert_eq!(s["metadata"]["labels"][LABEL_HOME], "true");
        assert_eq!(from_secret(&s), keys);
        // As the apiserver hands it back: base64 under data.
        let back = json!({"data": {"laptop": base64::engine::general_purpose::STANDARD.encode(ed25519("laptop"))}});
        assert_eq!(from_secret(&back), keys);
        let copy = secret("web", "gw-ssh-keys", "gw", false, &keys);
        assert!(copy["metadata"]["labels"].get(LABEL_HOME).is_none(), "a copy is not the original");
    }

    #[test]
    fn access_credentials_are_kubevirts_shape() {
        let c = access_credential("gw-ssh-keys", false);
        assert_eq!(c["sshPublicKey"]["source"]["secret"]["secretName"], "gw-ssh-keys");
        assert!(c["sshPublicKey"]["propagationMethod"]["noCloud"].is_object());
        let a = access_credential("gw-ssh-keys", true);
        assert_eq!(a["sshPublicKey"]["propagationMethod"]["qemuGuestAgent"]["users"], json!(["root"]));
        let spec = json!({"accessCredentials": [c, a]});
        assert_eq!(
            credentials(&spec),
            vec![("gw-ssh-keys".into(), "noCloud".into()), ("gw-ssh-keys".into(), "qemuGuestAgent".into())]
        );
    }

    #[test]
    fn the_seed_is_read_back_for_its_keys() {
        let k = ed25519("gw");
        let seed = format!("#cloud-config\nssh_authorized_keys:\n  - {k}\nusers:\n  - default\n  - name: root\n    ssh_authorized_keys:\n      - {k}\n");
        assert_eq!(from_seed(&seed), vec![k]);
    }
}
