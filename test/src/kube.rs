//! Just enough of the apiserver: create, read and delete, with the
//! ServiceAccount's token and CA. Everything the suites make is in the run's
//! namespace and carries the run label.

use std::time::Duration;

use serde_json::{json, Value};

use crate::env::Env;

pub const RUN_LABEL: &str = "storm.io/test-run";

pub struct Kube {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
    pub namespace: String,
    pub run_id: String,
}

impl Kube {
    pub fn new(env: &Env) -> Result<Kube, String> {
        // Generous: a busy apiserver is not what these suites measure, and one
        // slow create is not the console failing.
        let mut b = reqwest::Client::builder().timeout(Duration::from_secs(120));
        if let Some(ca) = &env.ca {
            let cert = reqwest::Certificate::from_pem(ca).map_err(|e| format!("ServiceAccount ca.crt: {e}"))?;
            b = b.add_root_certificate(cert);
        }
        Ok(Kube {
            http: b.build().map_err(|e| e.to_string())?,
            base: env.api.trim_end_matches('/').to_string(),
            token: env.token.clone(),
            namespace: env.namespace.clone(),
            run_id: env.run_id.clone(),
        })
    }

    async fn call(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<(u16, Value), String> {
        let mut req = self.http.request(method.clone(), format!("{}{path}", self.base));
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        if let Some(b) = body {
            req = req.header("Content-Type", "application/json").body(b.to_string());
        }
        let r = req.send().await.map_err(|e| format!("{method} {path}: {e}"))?;
        let status = r.status().as_u16();
        let text = r.text().await.unwrap_or_default();
        Ok((status, serde_json::from_str(&text).unwrap_or(Value::String(text))))
    }

    fn fail(method: &str, path: &str, status: u16, body: &Value) -> String {
        let msg = body["message"].as_str().map(str::to_owned).unwrap_or_else(|| body.to_string());
        format!("{method} {path}: HTTP {status}: {}", msg.chars().take(300).collect::<String>())
    }

    pub async fn get(&self, path: &str) -> Result<Option<Value>, String> {
        match self.call(reqwest::Method::GET, path, None).await? {
            (200, v) => Ok(Some(v)),
            (404, _) => Ok(None),
            (s, v) => Err(Self::fail("GET", path, s, &v)),
        }
    }

    pub async fn create(&self, path: &str, obj: &Value) -> Result<Value, String> {
        match self.call(reqwest::Method::POST, path, Some(obj)).await? {
            (200..=299, v) => Ok(v),
            (s, v) => Err(Self::fail("POST", path, s, &v)),
        }
    }

    /// Delete; gone already is fine.
    pub async fn delete(&self, path: &str) -> Result<(), String> {
        match self.call(reqwest::Method::DELETE, path, None).await? {
            (200..=299 | 404, _) => Ok(()),
            (s, v) => Err(Self::fail("DELETE", path, s, &v)),
        }
    }

    pub fn services(&self) -> String {
        format!("/api/v1/namespaces/{}/services", self.namespace)
    }

    /// A ClusterIP Service with no selector: a real object the console
    /// watches and shows, that needs no image, no pod and no capacity.
    pub fn service(&self, name: &str) -> Value {
        json!({
            "apiVersion": "v1",
            "kind": "Service",
            "metadata": {"name": name, "namespace": self.namespace, "labels": {RUN_LABEL: self.run_id}},
            "spec": {"ports": [{"port": 80, "protocol": "TCP"}]}
        })
    }

    /// Everything this run made in its namespace, by label — the runner
    /// deletes the namespace too, but a suite leaves the host as it found it
    /// on its own.
    pub async fn sweep(&self) -> Result<usize, String> {
        let sel = format!("labelSelector={RUN_LABEL}%3D{}", self.run_id);
        let mut n = 0;
        for kind in ["services", "configmaps"] {
            let base = format!("/api/v1/namespaces/{}/{kind}", self.namespace);
            if let Some(list) = self.get(&format!("{base}?{sel}")).await? {
                for item in list["items"].as_array().into_iter().flatten() {
                    if let Some(name) = item.pointer("/metadata/name").and_then(Value::as_str) {
                        self.delete(&format!("{base}/{name}")).await?;
                        n += 1;
                    }
                }
            }
        }
        Ok(n)
    }

    /// How many pods the node under test may hold — the long suite's wave
    /// size comes from the machine, never assumed.
    pub async fn node_pod_capacity(&self, node: &str) -> Option<usize> {
        let list = self.get("/api/v1/nodes").await.ok()??;
        let items = list["items"].as_array()?;
        let pick = items
            .iter()
            .find(|n| {
                n.pointer("/metadata/name").and_then(Value::as_str) == Some(node)
                    || n.pointer("/status/addresses")
                        .and_then(Value::as_array)
                        .is_some_and(|a| a.iter().any(|x| x["address"] == node))
            })
            .or_else(|| items.first())?;
        pick.pointer("/status/allocatable/pods").and_then(Value::as_str)?.parse().ok()
    }
}
