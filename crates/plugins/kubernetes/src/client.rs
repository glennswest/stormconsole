//! A thin rustkube apiserver client: kube-wire REST paths, bearer auth,
//! list + watch. Resources travel as `serde_json::Value` — the console
//! reads a handful of fields per kind and stays resilient to schema
//! evolution and CRDs.

use std::sync::Arc;

use console_core::Viewer;
use console_core::apiserver::{Bearer, Conn, Trust};
use futures_util::StreamExt;
use serde_json::Value;

/// A thin apiserver client that can act as somebody other than itself.
///
/// The console's own credential is what the watches use — they have to
/// see the whole cluster to serve anybody. A *write* is different: it
/// should be subject to the same authorization as the read that showed
/// the object, which means carrying the viewer's own bearer so the
/// apiserver's RBAC decides. [`As`] says whose credential each write carries, and it is
/// per-request rather than a second client, because a client per session
/// is a connection pool per session.
#[derive(Clone)]
pub struct RkClient {
    base: String,
    /// Who the console is to the apiserver and whose certificate it
    /// trusts (#33) — shared with every other apiserver caller.
    conn: Arc<Conn>,
}

/// Whose credential a write carries (#387). A write names it: there is no
/// `None` that quietly becomes the console's token, which is how a viewer
/// with no kubernetes identity once wrote workloads on the console's role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum As<'a> {
    /// The console's own token: what it does by itself, and root (#165).
    Console,
    /// The viewer's own bearer; the apiserver's RBAC for them decides.
    Viewer(&'a str),
}

/// Why a write that has to be the viewer's own is refused to someone with no
/// kubernetes identity (the node's bearer, a client certificate, a user with no token).
pub const NO_IDENTITY: &str =
    "This goes as you, and this sign-in has no kubernetes identity. Sign in as a user with one.";

impl<'a> As<'a> {
    /// A write that has to be the viewer's own — a workload or a project:
    /// their token, or the console's for root. `None` for anyone else: in a
    /// project the console made, its ServiceAccount is `admin`
    /// (`self-provisioner`), so its role would hold nobody back there
    /// (`docs/rbac.md` §Two credentials).
    pub fn own(viewer: &'a Viewer) -> Option<Self> {
        match viewer.token.as_deref() {
            Some(t) => Some(As::Viewer(t)),
            None if viewer.root => Some(As::Console),
            None => None,
        }
    }

    /// A component's setting — a VM, stormcluster, a cloud image, a new
    /// project: the viewer's token, or with none the console's, whose role is
    /// then all they may do (`docs/rbac.md` §Two credentials).
    pub fn within_role(viewer: &'a Viewer) -> Self {
        viewer.token.as_deref().map_or(As::Console, As::Viewer)
    }

    /// The bearer a gate already chose: `Some` the viewer's, `None` root's,
    /// which is the console's (`console_core::storage::ActAs`).
    pub fn chosen(token: Option<&'a str>) -> Self {
        token.map_or(As::Console, As::Viewer)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RkError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("apiserver returned {0}")]
    Status(reqwest::StatusCode),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

impl RkClient {
    pub fn new(conn: Arc<Conn>) -> Self {
        Self { base: conn.server().to_string(), conn }
    }

    /// An inline token and either no verification or the system roots —
    /// the shape a test or a one-off caller wants.
    pub fn plain(server: &str, token: Option<&str>, insecure: bool) -> Self {
        let bearer = token.map(|t| Bearer::Inline(t.to_string())).unwrap_or(Bearer::None);
        Self::new(Conn::new(server, bearer, if insecure { Trust::Unverified } else { Trust::System }))
    }

    pub fn conn(&self) -> &Arc<Conn> {
        &self.conn
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// The bearer a request carries.
    fn bearer(&self, who: As<'_>) -> Option<String> {
        match who {
            As::Viewer(t) => Some(t.to_string()),
            As::Console => self.conn.token(),
        }
    }

    /// A read's credential: a viewer's bearer narrows what they see; with
    /// none it is the console's read, which `access` filters before it is
    /// served. Writes take an [`As`] and never come through here.
    fn reader(as_viewer: Option<&str>) -> As<'_> {
        as_viewer.map_or(As::Console, As::Viewer)
    }

    fn request(&self, method: reqwest::Method, path: &str, who: As<'_>) -> reqwest::RequestBuilder {
        let req = self.conn.http().request(method, format!("{}{}", self.base, path));
        match self.bearer(who) {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }

    pub async fn get(&self, path: &str) -> Result<Value, RkError> {
        self.get_as(path, None).await
    }

    pub async fn get_as(&self, path: &str, as_viewer: Option<&str>) -> Result<Value, RkError> {
        let resp = self.request(reqwest::Method::GET, path, Self::reader(as_viewer)).send().await?;
        if !resp.status().is_success() {
            return Err(RkError::Status(resp.status()));
        }
        Ok(resp.json().await?)
    }

    /// A text body — a container's log, read as the console.
    pub async fn get_text(&self, path: &str) -> Result<String, RkError> {
        let resp = self.request(reqwest::Method::GET, path, As::Console).send().await?;
        if !resp.status().is_success() {
            return Err(RkError::Status(resp.status()));
        }
        Ok(resp.text().await?)
    }

    /// The response itself, whatever its status — for a body that is
    /// passed through as it arrives (a followed log), and a refusal whose
    /// words are the apiserver's.
    pub async fn get_raw_as(&self, path: &str, as_viewer: Option<&str>) -> Result<reqwest::Response, RkError> {
        Ok(self.request(reqwest::Method::GET, path, Self::reader(as_viewer)).send().await?)
    }

    /// The console's own bearer, for a hop the apiserver does not proxy
    /// (a kubelet checks it with a TokenReview).
    pub fn token(&self) -> Option<String> {
        self.conn.token()
    }

    /// Create: POST a JSON object to a collection. The apiserver's status
    /// and body come back whatever they are — a 409 is the caller's to
    /// report, not an error here.
    pub async fn post_json(&self, path: &str, body: &Value) -> Result<(reqwest::StatusCode, Value), RkError> {
        self.post_json_as(path, body, As::Console).await
    }

    pub async fn post_json_as(
        &self,
        path: &str,
        body: &Value,
        who: As<'_>,
    ) -> Result<(reqwest::StatusCode, Value), RkError> {
        let resp = self.request(reqwest::Method::POST, path, who).json(body).send().await?;
        let status = resp.status();
        let body = resp.json().await.unwrap_or(Value::Null);
        Ok((status, body))
    }

    /// Replace one object — what saving an edited YAML does. The
    /// apiserver's `resourceVersion` check is the concurrency guard: an
    /// edit of a stale object is refused with a 409 rather than
    /// overwriting somebody else's change, so it is passed through as it
    /// comes back.
    pub async fn put_json(
        &self,
        path: &str,
        body: &Value,
        who: As<'_>,
    ) -> Result<(reqwest::StatusCode, Value), RkError> {
        let resp = self.request(reqwest::Method::PUT, path, who).json(body).send().await?;
        let status = resp.status();
        let body = resp.json().await.unwrap_or(Value::Null);
        Ok((status, body))
    }

    /// Merge-patch one object. A KubeVirt `VirtualMachine` is started and
    /// stopped by writing `spec.running`, so this is the whole of a VM's
    /// lifecycle on the apiserver side.
    pub async fn patch_merge(
        &self,
        path: &str,
        body: &Value,
        who: As<'_>,
    ) -> Result<(reqwest::StatusCode, Value), RkError> {
        let resp = self
            .request(reqwest::Method::PATCH, path, who)
            .header(reqwest::header::CONTENT_TYPE, "application/merge-patch+json")
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        let body = resp.json().await.unwrap_or(Value::Null);
        Ok((status, body))
    }

    pub async fn delete(&self, path: &str, who: As<'_>) -> Result<reqwest::StatusCode, RkError> {
        let resp = self.request(reqwest::Method::DELETE, path, who).send().await?;
        Ok(resp.status())
    }

    /// Delete, with the apiserver's body: its refusal is in its own words (#348).
    pub async fn delete_json(&self, path: &str, who: As<'_>) -> Result<(reqwest::StatusCode, Value), RkError> {
        let resp = self.request(reqwest::Method::DELETE, path, who).send().await?;
        let status = resp.status();
        let body = resp.json().await.unwrap_or(Value::Null);
        Ok((status, body))
    }

    /// Delete with `DeleteOptions` — `preconditions.uid`, so what is deleted
    /// is the object that was read, not one made since under its name.
    pub async fn delete_body(
        &self,
        path: &str,
        options: &Value,
        who: As<'_>,
    ) -> Result<(reqwest::StatusCode, Value), RkError> {
        let resp = self.request(reqwest::Method::DELETE, path, who).json(options).send().await?;
        let status = resp.status();
        let body = resp.json().await.unwrap_or(Value::Null);
        Ok((status, body))
    }

    /// Open a watch stream and send each event's `{type, object}` down the
    /// channel, preserving apiserver order. Returns when the connection
    /// ends (the caller re-lists and redials).
    pub async fn watch(
        &self,
        path: &str,
        resource_version: &str,
        shutdown: &tokio_util::sync::CancellationToken,
        events: tokio::sync::mpsc::Sender<(String, Value)>,
    ) -> Result<(), RkError> {
        self.watch_as(path, resource_version, None, shutdown, events).await
    }

    /// [`watch`](Self::watch), as a viewer: a grid's live updates are that
    /// viewer's own watch, so the apiserver's RBAC decides what reaches them
    /// (#348). `None` is the console's own credential.
    pub async fn watch_as(
        &self,
        path: &str,
        resource_version: &str,
        as_viewer: Option<&str>,
        shutdown: &tokio_util::sync::CancellationToken,
        events: tokio::sync::mpsc::Sender<(String, Value)>,
    ) -> Result<(), RkError> {
        let sep = if path.contains('?') { '&' } else { '?' };
        let url = format!(
            "{}{}{}watch=true&resourceVersion={}&allowWatchBookmarks=true",
            self.base, path, sep, resource_version
        );
        // The cache's watch is the console's own read of the whole cluster,
        // so it carries the console's credential; a grid's is the viewer's.
        let req = match self.bearer(Self::reader(as_viewer)) {
            Some(t) => self.conn.http().get(url).bearer_auth(t),
            None => self.conn.http().get(url),
        };
        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(RkError::Status(resp.status()));
        }
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        loop {
            let chunk = tokio::select! {
                c = stream.next() => c,
                _ = shutdown.cancelled() => return Ok(()),
            };
            let Some(chunk) = chunk else { return Ok(()) };
            buf.extend_from_slice(&chunk?);
            while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=nl).collect();
                let line = &line[..line.len() - 1];
                if line.is_empty() {
                    continue;
                }
                let Ok(ev) = serde_json::from_slice::<Value>(line) else { continue };
                let kind = ev.get("type").and_then(Value::as_str).unwrap_or("").to_string();
                let object = ev.get("object").cloned().unwrap_or(Value::Null);
                if events.send((kind, object)).await.is_err() {
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewer(token: Option<&str>, root: bool) -> Viewer {
        Viewer { user: Some("dave".into()), token: token.map(str::to_string), roles: vec!["operator".into()], root, ..Viewer::default() }
    }

    /// #387, finding 39f446993b07: a workload or project write with no
    /// kubernetes identity has no credential to go on, never the console's.
    #[test]
    fn a_write_of_ones_own_never_borrows_the_consoles_credential() {
        let v = viewer(Some("theirs"), false);
        assert_eq!(As::own(&v), Some(As::Viewer("theirs")));
        assert_eq!(As::own(&viewer(None, false)), None);
        // Root has no bearer of its own and goes on the console's (#165).
        assert_eq!(As::own(&viewer(None, true)), Some(As::Console));
    }

    #[test]
    fn a_components_setting_falls_back_to_the_consoles_role_by_name() {
        let v = viewer(Some("theirs"), false);
        assert_eq!(As::within_role(&v), As::Viewer("theirs"));
        assert_eq!(As::within_role(&viewer(None, false)), As::Console);
        assert_eq!(As::chosen(Some("t")), As::Viewer("t"));
        assert_eq!(As::chosen(None), As::Console);
    }

    #[test]
    fn the_bearer_is_the_one_named() {
        let c = RkClient::plain("https://127.0.0.1:1", Some("console"), true);
        assert_eq!(c.bearer(As::Console).as_deref(), Some("console"));
        assert_eq!(c.bearer(As::Viewer("theirs")).as_deref(), Some("theirs"));
        assert_eq!(c.bearer(RkClient::reader(None)).as_deref(), Some("console"));
    }
}
