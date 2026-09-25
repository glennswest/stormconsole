//! The key Secrets, read and written as the viewer (#26). The pure half —
//! parsing, names, bodies — is `keys.rs`; this is the apiserver half.

use std::collections::BTreeMap;

use console_core::Viewer;
use serde_json::{json, Value};

use crate::keys;
use crate::Inner;

/// Whose keys: the signed-in user, or `admin` on a console with no users —
/// the name that console's login gives, so the Secret is the same one
/// whichever way somebody arrives.
pub fn user_of(viewer: &Viewer) -> String {
    viewer.user.clone().unwrap_or_else(|| "admin".into())
}

fn secrets(ns: &str) -> String {
    format!("/api/v1/namespaces/{ns}/secrets")
}

fn message(status: reqwest::StatusCode, body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("apiserver returned {}", status.as_u16()))
}

/// One Secret, or `None` when it does not exist.
pub async fn read(inner: &Inner, viewer: &Viewer, ns: &str, name: &str) -> Result<Option<Value>, String> {
    let Some(client) = &inner.client else { return Err("no apiserver".into()) };
    match client.get_as(&format!("{}/{name}", secrets(ns)), viewer.token.as_deref()).await {
        Ok(v) => Ok(Some(v)),
        Err(plugin_kubernetes::RkError::Status(s)) if s.as_u16() == 404 => Ok(None),
        Err(e) => Err(format!("could not read Secret {ns}/{name}: {e}")),
    }
}

/// Create or replace a Secret. A replace carries the `resourceVersion` it
/// read, so two edits at once is a 409 rather than one silently lost.
async fn write(inner: &Inner, viewer: &Viewer, body: &Value, existing: Option<&Value>) -> Result<(), String> {
    let Some(client) = &inner.client else { return Err("no apiserver".into()) };
    let ns = body["metadata"]["namespace"].as_str().unwrap_or("default");
    let name = body["metadata"]["name"].as_str().unwrap_or_default();
    let (status, b) = match existing {
        None => client.post_json_as(&secrets(ns), body, viewer.token.as_deref()).await,
        Some(old) => {
            let mut body = body.clone();
            body["metadata"]["resourceVersion"] = old.pointer("/metadata/resourceVersion").cloned().unwrap_or(Value::Null);
            // A replace with `stringData` alone would leave the old `data`
            // items in place: a deleted key would survive its deletion.
            body["data"] = json!({});
            client.put_json(&format!("{}/{name}", secrets(ns)), &body, viewer.token.as_deref()).await
        }
    }
    .map_err(|e| e.to_string())?;
    if status.is_success() {
        Ok(())
    } else {
        Err(format!("could not write Secret {ns}/{name}: {}", message(status, &b)))
    }
}

/// The user's saved keys, from the home Secret.
pub async fn saved(inner: &Inner, viewer: &Viewer) -> Result<BTreeMap<String, String>, String> {
    let name = keys::secret_name(&user_of(viewer));
    Ok(read(inner, viewer, &inner.keys_ns, &name).await?.map(|s| keys::from_secret(&s)).unwrap_or_default())
}

/// Replace the user's list: the home Secret, then every copy. Returns the
/// namespaces whose copies were refreshed.
pub async fn store(inner: &Inner, viewer: &Viewer, list: &BTreeMap<String, String>) -> Result<Vec<String>, String> {
    let user = user_of(viewer);
    let name = keys::secret_name(&user);
    let home = &inner.keys_ns;
    let old = read(inner, viewer, home, &name).await?;
    write(inner, viewer, &keys::secret(home, &name, &user, true, list), old.as_ref()).await?;
    let mut refreshed = Vec::new();
    for ns in copies(inner, viewer).await {
        if &ns == home {
            continue;
        }
        let old = read(inner, viewer, &ns, &name).await?;
        write(inner, viewer, &keys::secret(&ns, &name, &user, false, list), old.as_ref()).await?;
        refreshed.push(ns);
    }
    Ok(refreshed)
}

/// Where this user's copies are: every Secret labelled with their name.
///
/// A cluster-wide list, as the viewer. A viewer who may not list Secrets
/// across the cluster gets no copies refreshed from here — each copy is
/// still refreshed the next time a machine is created or keyed in its
/// namespace, which is when it is read.
pub async fn copies(inner: &Inner, viewer: &Viewer) -> Vec<String> {
    let Some(client) = &inner.client else { return vec![] };
    let sel = format!("{}={}", keys::LABEL_FOR, keys::dns(&user_of(viewer)));
    let Ok(list) = client.get_as(&format!("/api/v1/secrets?labelSelector={sel}"), viewer.token.as_deref()).await else {
        return vec![];
    };
    let name = keys::secret_name(&user_of(viewer));
    let mut out: Vec<String> = list["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s.pointer("/metadata/name").and_then(Value::as_str) == Some(name.as_str()))
        .filter_map(|s| s.pointer("/metadata/namespace").and_then(Value::as_str).map(str::to_string))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Make sure `ns` holds a current copy of the user's Secret, and name it.
/// The home namespace holds the original, which is current by definition.
pub async fn ensure_copy(inner: &Inner, viewer: &Viewer, ns: &str, list: &BTreeMap<String, String>) -> Result<String, String> {
    let user = user_of(viewer);
    let name = keys::secret_name(&user);
    let old = read(inner, viewer, ns, &name).await?;
    let current = old.as_ref().map(keys::from_secret);
    if current.as_ref() != Some(list) {
        write(inner, viewer, &keys::secret(ns, &name, &user, ns == inner.keys_ns, list), old.as_ref()).await?;
    }
    Ok(name)
}

/// A Secret of exactly these keys for one machine, when it was given
/// something other than the user's whole list.
pub async fn machine_secret(inner: &Inner, viewer: &Viewer, ns: &str, vm: &str, list: &BTreeMap<String, String>) -> Result<String, String> {
    let name = keys::machine_secret_name(vm);
    let old = read(inner, viewer, ns, &name).await?;
    let mut body = keys::secret(ns, &name, &user_of(viewer), false, list);
    // Not the user's list: a machine's own, so an edit on the Account page
    // does not rewrite it, and deleting the machine leaves it findable.
    body["metadata"]["labels"] = json!({"storm.io/vm": vm});
    write(inner, viewer, &body, old.as_ref()).await?;
    Ok(name)
}
