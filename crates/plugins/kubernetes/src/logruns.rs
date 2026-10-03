//! The last runs of every container, kept by the console (#69).
//!
//! OpenShift shows the runs before the current one, and a crash-looping
//! container's earlier attempts are where the reason usually is. The node
//! keeps every run on disk, but its API serves only the current one and
//! `previous` — the one before (rustkube-node#131). So the console keeps
//! them: it looks at every pod's `restartCount` on a timer, and when one
//! moves it asks for `previous`, which at that moment is the run that just
//! ended. Five are kept per container, the newest last.
//!
//! What this cannot do, and says: a run that ended and was itself
//! replaced between two looks is gone by the time the console asks, so
//! each kept run carries how many it `missed` before it. Runs are held in
//! memory — a console restart starts the record over — and a pod that is
//! deleted takes its runs with it.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::cache::Store;
use crate::client::RkClient;

/// Runs kept per container.
pub const KEEP: usize = 5;
/// How much of one run is kept: the end of it, where the reason is.
pub const RUN_BYTES: usize = 256 * 1024;
/// How often restart counts are compared.
const EVERY: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    /// Which run this was, counting from 0 — the restartCount the
    /// container had while it ran.
    pub run: i64,
    /// When the console fetched it, just after the run ended.
    pub kept_at: String,
    /// Runs that ended between two looks and could not be fetched.
    pub missed: i64,
    pub bytes: usize,
    /// Whether the start of the run was cut to keep the end.
    pub truncated: bool,
    #[serde(skip)]
    pub text: String,
}

#[derive(Default)]
struct Track {
    last: i64,
    runs: VecDeque<Run>,
}

/// The kept runs, by pod uid and container.
#[derive(Default)]
pub struct LogRuns {
    tracks: RwLock<HashMap<(String, String), Track>>,
}

/// One container to look at: (pod uid, ns, pod, container, restartCount).
pub type Seen = (String, String, String, String, i64);

/// The containers a pod list holds, with their restart counts.
pub fn seen(pods: &HashMap<String, Value>) -> Vec<Seen> {
    let mut out = Vec::new();
    for (key, pod) in pods {
        let Some((ns, name)) = key.split_once('/') else { continue };
        let uid = pod.pointer("/metadata/uid").and_then(Value::as_str).unwrap_or(key).to_string();
        for ptr in ["/status/initContainerStatuses", "/status/containerStatuses"] {
            for c in pod.pointer(ptr).and_then(Value::as_array).into_iter().flatten() {
                let Some(cname) = c.get("name").and_then(Value::as_str) else { continue };
                let rc = c.get("restartCount").and_then(Value::as_i64).unwrap_or(0);
                out.push((uid.clone(), ns.into(), name.into(), cname.into(), rc));
            }
        }
    }
    out
}

/// What to fetch after a look: the containers whose count moved (and
/// those seen for the first time already restarted), with how many runs
/// were missed. Updates the counts; forgets pods that are gone.
pub async fn due(runs: &LogRuns, seen: &[Seen]) -> Vec<(Seen, i64)> {
    let mut tracks = runs.tracks.write().await;
    let live: std::collections::HashSet<(String, String)> =
        seen.iter().map(|(u, _, _, c, _)| (u.clone(), c.clone())).collect();
    tracks.retain(|k, _| live.contains(k));
    let mut out = Vec::new();
    for s in seen {
        let key = (s.0.clone(), s.3.clone());
        let rc = s.4;
        match tracks.get_mut(&key) {
            None => {
                tracks.insert(key, Track { last: rc, runs: VecDeque::new() });
                if rc > 0 {
                    out.push((s.clone(), 0));
                }
            }
            Some(t) if rc > t.last => {
                let missed = rc - t.last - 1;
                t.last = rc;
                out.push((s.clone(), missed));
            }
            Some(t) => t.last = t.last.max(rc),
        }
    }
    out
}

/// Keep one fetched run: the end of it, five per container.
pub async fn keep(runs: &LogRuns, s: &Seen, missed: i64, text: String) {
    let truncated = text.len() > RUN_BYTES;
    let text = if truncated {
        let mut cut = text.len() - RUN_BYTES;
        while !text.is_char_boundary(cut) {
            cut += 1;
        }
        text[cut..].to_string()
    } else {
        text
    };
    let mut tracks = runs.tracks.write().await;
    let t = tracks.entry((s.0.clone(), s.3.clone())).or_default();
    t.runs.push_back(Run {
        run: s.4 - 1,
        kept_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        missed,
        bytes: text.len(),
        truncated,
        text,
    });
    while t.runs.len() > KEEP {
        t.runs.pop_front();
    }
}

impl LogRuns {
    /// The runs kept for one pod, by container, oldest first.
    pub async fn of(&self, uid: &str) -> HashMap<String, Vec<Run>> {
        let tracks = self.tracks.read().await;
        tracks
            .iter()
            .filter(|((u, _), _)| u == uid)
            .map(|((_, c), t)| (c.clone(), t.runs.iter().cloned().collect()))
            .collect()
    }

    /// One kept run's text.
    pub async fn text(&self, uid: &str, container: &str, run: i64) -> Option<String> {
        let tracks = self.tracks.read().await;
        let t = tracks.get(&(uid.to_string(), container.to_string()))?;
        t.runs.iter().find(|r| r.run == run).map(|r| r.text.clone())
    }
}

/// The keeper: look, fetch what moved, repeat.
pub async fn run(runs: Arc<LogRuns>, store: Arc<Store>, client: RkClient, shutdown: CancellationToken) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(EVERY) => {}
            _ = shutdown.cancelled() => return,
        }
        let pods = store.kind("pod").await;
        for (s, missed) in due(&runs, &seen(&pods)).await {
            let (_, ns, pod, container, _) = &s;
            let path = format!(
                "/api/v1/namespaces/{ns}/pods/{pod}/log?container={container}&previous=true&limitBytes={}",
                RUN_BYTES * 2
            );
            match client.get_text(&path).await {
                Ok(text) => keep(&runs, &s, missed, text).await,
                Err(e) => tracing::debug!(%ns, %pod, %container, "previous run not kept: {e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pods(rc: i64) -> HashMap<String, Value> {
        let mut m = HashMap::new();
        m.insert(
            "shop/web-1".to_string(),
            json!({"metadata":{"uid":"u1"},"status":{"containerStatuses":[{"name":"app","restartCount":rc}]}}),
        );
        m
    }

    #[tokio::test]
    async fn a_moving_restart_count_fetches_the_run_that_ended() {
        let r = LogRuns::default();
        // First sight, never restarted: nothing to fetch.
        assert!(due(&r, &seen(&pods(0))).await.is_empty());
        // One restart: the run that ended is `previous`.
        let d = due(&r, &seen(&pods(1))).await;
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].1, 0);
        // Three more between two looks: two of them are missed.
        let d = due(&r, &seen(&pods(4))).await;
        assert_eq!(d[0].1, 2);
        // No change: nothing.
        assert!(due(&r, &seen(&pods(4))).await.is_empty());
    }

    #[tokio::test]
    async fn first_sight_of_a_restarted_container_fetches_once() {
        let r = LogRuns::default();
        assert_eq!(due(&r, &seen(&pods(7))).await.len(), 1);
        assert!(due(&r, &seen(&pods(7))).await.is_empty());
    }

    #[tokio::test]
    async fn five_are_kept_newest_last_and_the_end_of_each() {
        let r = LogRuns::default();
        for rc in 1..=7 {
            let s: Seen = ("u1".into(), "shop".into(), "web-1".into(), "app".into(), rc);
            keep(&r, &s, 0, format!("run {}", rc - 1)).await;
        }
        let of = r.of("u1").await;
        let runs: Vec<i64> = of["app"].iter().map(|r| r.run).collect();
        assert_eq!(runs, vec![2, 3, 4, 5, 6]);
        assert_eq!(r.text("u1", "app", 6).await.unwrap(), "run 6");
        assert!(r.text("u1", "app", 1).await.is_none());

        let s: Seen = ("u1".into(), "shop".into(), "web-1".into(), "big".into(), 1);
        keep(&r, &s, 0, format!("HEAD{}TAIL", "x".repeat(RUN_BYTES))).await;
        let big = &r.of("u1").await["big"][0];
        assert!(big.truncated);
        assert_eq!(big.bytes, RUN_BYTES);
        assert!(r.text("u1", "big", 0).await.unwrap().ends_with("TAIL"));
    }

    #[tokio::test]
    async fn a_deleted_pod_takes_its_runs_with_it() {
        let r = LogRuns::default();
        due(&r, &seen(&pods(1))).await;
        let s: Seen = ("u1".into(), "shop".into(), "web-1".into(), "app".into(), 1);
        keep(&r, &s, 0, "x".into()).await;
        due(&r, &[]).await;
        assert!(r.of("u1").await.is_empty());
    }
}
