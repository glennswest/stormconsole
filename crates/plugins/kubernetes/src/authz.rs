//! Which namespaces a viewer may see — asked of the apiserver, not
//! decided here.
//!
//! The distinction issue #7 turns on: a filter in the UI is a display
//! choice, and this has to be an authorization result. So the console
//! never decides; it asks rustkube **as the viewer**, with the viewer's
//! own bearer token, and takes the answer.
//!
//! Two questions, in order:
//!
//! 1. `GET /api/v1/namespaces` as the viewer. A `200` is the self-scoped
//!    answer OpenShift's project list gives: exactly what this identity
//!    may list.
//! 2. rustkube's RBAC makes that all-or-nothing — a user with access to
//!    one namespace and no cluster-wide `list namespaces` gets a `403`,
//!    not a short list. Upstream solves this with
//!    `SelfSubjectAccessReview`, which rustkube does not serve (filed
//!    there). Until it does, a `403` falls back to asking about each
//!    namespace the console already knows of: `GET
//!    /api/v1/namespaces/{ns}` as the viewer, one request each, and the
//!    ones that answer `200` are theirs.
//!
//! Answers are cached briefly per token: a console redraws its feed every
//! two seconds and an authorization question per redraw would be a denial
//! of service on the apiserver by way of a UI.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

/// How long an answer stands. Short enough that a RoleBinding takes
/// effect while somebody is still looking at the screen.
const TTL: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq)]
pub struct Allowed {
    pub namespaces: HashSet<String>,
    /// How the answer was reached, for the note the UI shows.
    pub source: Source,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The apiserver listed them for this identity.
    Listed,
    /// The apiserver refused the list, so each was asked about in turn.
    Probed,
    /// The apiserver could not be asked at all. Nothing is claimed and
    /// nothing is hidden — an unreachable authorizer must not silently
    /// become a permissive one, so the caller reports it as unknown.
    Unavailable,
}

struct Entry {
    at: Instant,
    allowed: Allowed,
}

#[derive(Default)]
pub struct Authorizer {
    cache: RwLock<HashMap<String, Entry>>,
}

impl Authorizer {
    /// The namespaces this token may see, out of `known` (the console's
    /// own cluster-wide cache).
    pub async fn allowed(
        &self,
        base: &str,
        http: &reqwest::Client,
        token: &str,
        known: &[String],
    ) -> Allowed {
        if let Some(e) = self.cache.read().await.get(token) {
            if e.at.elapsed() < TTL {
                return e.allowed.clone();
            }
        }
        let allowed = ask(base, http, token, known).await;
        self.cache
            .write()
            .await
            .insert(token.to_string(), Entry { at: Instant::now(), allowed: allowed.clone() });
        allowed
    }
}

async fn ask(base: &str, http: &reqwest::Client, token: &str, known: &[String]) -> Allowed {
    let list = http
        .get(format!("{base}/api/v1/namespaces"))
        .bearer_auth(token)
        .timeout(Duration::from_secs(5))
        .send()
        .await;
    match list {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
            Allowed { namespaces: names(&body), source: Source::Listed }
        }
        Ok(resp) if resp.status().as_u16() == 401 || resp.status().as_u16() == 403 => {
            let mut ok = HashSet::new();
            for ns in known {
                let r = http
                    .get(format!("{base}/api/v1/namespaces/{ns}"))
                    .bearer_auth(token)
                    .timeout(Duration::from_secs(5))
                    .send()
                    .await;
                if matches!(r, Ok(ref resp) if resp.status().is_success()) {
                    ok.insert(ns.clone());
                }
            }
            Allowed { namespaces: ok, source: Source::Probed }
        }
        _ => Allowed { namespaces: HashSet::new(), source: Source::Unavailable },
    }
}

/// `metadata.name` of every item in a list response.
fn names(list: &serde_json::Value) -> HashSet<String> {
    list.get("items")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|o| o.pointer("/metadata/name").and_then(serde_json::Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The line the UI shows about what it is not showing. Silence would read
/// as a short list, which reads as a broken console.
pub fn note(hidden: usize, source: Source) -> String {
    match source {
        Source::Unavailable => {
            "the apiserver could not be asked what you may see".to_string()
        }
        _ if hidden == 1 => "1 namespace you cannot view".to_string(),
        _ => format!("{hidden} namespaces you cannot view"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn names_come_out_of_a_list_response() {
        let v = json!({"items": [
            {"metadata": {"name": "default"}},
            {"metadata": {"name": "kube-system"}},
            {"metadata": {}}
        ]});
        let got = names(&v);
        assert_eq!(got.len(), 2);
        assert!(got.contains("default") && got.contains("kube-system"));
        assert!(names(&json!({})).is_empty());
    }

    #[test]
    fn the_note_counts_and_says_when_it_could_not_ask() {
        assert_eq!(note(1, Source::Listed), "1 namespace you cannot view");
        assert_eq!(note(3, Source::Probed), "3 namespaces you cannot view");
        assert_eq!(
            note(0, Source::Unavailable),
            "the apiserver could not be asked what you may see"
        );
    }

    #[tokio::test]
    async fn an_answer_is_cached_so_a_two_second_redraw_is_not_a_flood() {
        let a = Authorizer::default();
        // No apiserver at this address: the first ask fails and is cached
        // as Unavailable, and the second returns without dialling again.
        let http = reqwest::Client::new();
        let first = a.allowed("http://127.0.0.1:1", &http, "t", &[]).await;
        assert_eq!(first.source, Source::Unavailable);
        let started = Instant::now();
        let second = a.allowed("http://127.0.0.1:1", &http, "t", &[]).await;
        assert_eq!(second, first);
        assert!(started.elapsed() < Duration::from_millis(50), "second ask was not cached");
    }
}
