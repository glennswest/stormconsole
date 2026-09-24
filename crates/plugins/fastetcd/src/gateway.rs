//! etcd's v3 JSON gateway: `POST /v3/…` on the client port.
//!
//! The same surface etcd itself serves beside gRPC, so this reads fastetcd
//! once fastetcd#28 lands and reads etcd today. Keys and values are base64
//! and 64-bit integers arrive as strings, per protobuf's JSON mapping; the
//! readers here take either, and either spelling of a field name, because
//! grpc-gateway's `OrigName` setting has changed between etcd releases.

use std::time::Duration;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde::Serialize;
use serde_json::{json, Value};

pub struct Gateway<'a> {
    pub client: &'a reqwest::Client,
    pub base: &'a str,
}

/// Why a gateway call did not answer.
#[derive(Debug)]
pub enum Error {
    /// The server is there and does not serve `/v3/` — fastetcd before #28.
    NotServed(String),
    /// It is not there at all, or it answered with a failure.
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotServed(s) | Error::Failed(s) => f.write_str(s),
        }
    }
}

impl Gateway<'_> {
    pub async fn call(&self, path: &str, body: Value) -> Result<Value, Error> {
        let url = format!("{}{path}", self.base.trim_end_matches('/'));
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| Error::Failed(reason(&e)))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| Error::Failed(reason(&e)))?;
        // 404 and 405 are "no such route", and so is a 2xx that is not
        // JSON: fastetcd's gRPC router answers an HTTP/1 POST it does not
        // know with an empty or non-JSON body rather than a clean 404.
        if status.as_u16() == 404 || status.as_u16() == 405 {
            return Err(Error::NotServed(format!("{path} responded {status}")));
        }
        let v: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) if status.is_success() => {
                return Err(Error::NotServed(format!("{path} did not answer JSON")))
            }
            Err(_) => return Err(Error::Failed(format!("{path} responded {status}"))),
        };
        if !status.is_success() {
            let msg = v.get("message").or_else(|| v.get("error")).and_then(Value::as_str).unwrap_or("");
            return Err(Error::Failed(format!("{path} responded {status}: {msg}")));
        }
        Ok(v)
    }

    pub async fn status(&self) -> Result<Status, Error> {
        self.call("/v3/maintenance/status", json!({})).await.map(|v| Status::from(&v))
    }

    pub async fn members(&self) -> Result<Vec<Member>, Error> {
        let v = self.call("/v3/cluster/member/list", json!({})).await?;
        Ok(v.get("members").and_then(Value::as_array).map(|a| a.iter().map(Member::from).collect()).unwrap_or_default())
    }

    pub async fn alarms(&self) -> Result<Vec<Alarm>, Error> {
        let v = self.call("/v3/maintenance/alarm", json!({"action": "GET"})).await?;
        Ok(v.get("alarms")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|x| Alarm {
                        member_id: id(x, &["memberID", "member_id", "memberId"]),
                        alarm: x.get("alarm").and_then(Value::as_str).unwrap_or("NONE").to_string(),
                    })
                    // `NONE` is how etcd says "no alarm" inside the list.
                    .filter(|a| a.alarm != "NONE")
                    .collect()
            })
            .unwrap_or_default())
    }

    pub async fn disarm(&self, member_id: &str, alarm: &str) -> Result<Value, Error> {
        self.call(
            "/v3/maintenance/alarm",
            json!({"action": "DEACTIVATE", "memberID": member_id, "alarm": alarm}),
        )
        .await
    }

    pub async fn compact(&self, revision: u64) -> Result<Value, Error> {
        self.call("/v3/kv/compaction", json!({"revision": revision.to_string(), "physical": true})).await
    }

    pub async fn defragment(&self) -> Result<Value, Error> {
        self.call("/v3/maintenance/defragment", json!({})).await
    }

    /// Every key under `prefix`, keys only, at most `limit`.
    pub async fn keys(&self, prefix: &str, limit: u64) -> Result<Keys, Error> {
        let v = self
            .call(
                "/v3/kv/range",
                json!({
                    "key": B64.encode(prefix),
                    "range_end": B64.encode(prefix_end(prefix.as_bytes())),
                    "keys_only": true,
                    "limit": limit.to_string(),
                }),
            )
            .await?;
        let keys = kvs(&v).iter().filter_map(|kv| bytes(kv, "key")).map(|k| String::from_utf8_lossy(&k).into_owned()).collect();
        Ok(Keys { keys, count: num(&v, &["count"]).unwrap_or(0), more: v.get("more").and_then(Value::as_bool).unwrap_or(false) })
    }

    /// One key, whole.
    pub async fn get(&self, key: &str) -> Result<Option<Kv>, Error> {
        let v = self.call("/v3/kv/range", json!({"key": B64.encode(key)})).await?;
        Ok(kvs(&v).first().map(|kv| Kv {
            key: key.to_string(),
            value: bytes(kv, "value").unwrap_or_default(),
            create_revision: num(kv, &["create_revision", "createRevision"]).unwrap_or(0),
            mod_revision: num(kv, &["mod_revision", "modRevision"]).unwrap_or(0),
            version: num(kv, &["version"]).unwrap_or(0),
            lease: num(kv, &["lease"]).unwrap_or(0),
        }))
    }
}

/// The end of the range that holds every key with this prefix: the prefix
/// with its last byte incremented, carrying — etcd's `clientv3.GetPrefixRangeEnd`.
pub fn prefix_end(prefix: &[u8]) -> Vec<u8> {
    let mut end = prefix.to_vec();
    while let Some(last) = end.pop() {
        if last < 0xff {
            end.push(last + 1);
            return end;
        }
    }
    // All 0xff, or empty: the range runs to the end of the keyspace.
    vec![0]
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Status {
    pub member_id: String,
    pub cluster_id: String,
    pub leader: String,
    pub version: String,
    pub revision: u64,
    pub raft_term: u64,
    pub raft_index: u64,
    pub raft_applied_index: u64,
    pub db_size: u64,
    pub db_size_in_use: u64,
    pub is_learner: bool,
    pub errors: Vec<String>,
}

impl From<&Value> for Status {
    fn from(v: &Value) -> Self {
        let h = v.get("header").cloned().unwrap_or(Value::Null);
        Status {
            member_id: id(&h, &["member_id", "memberId"]),
            cluster_id: id(&h, &["cluster_id", "clusterId"]),
            leader: id(v, &["leader"]),
            version: v.get("version").and_then(Value::as_str).unwrap_or("").to_string(),
            revision: num(&h, &["revision"]).unwrap_or(0),
            raft_term: num(v, &["raftTerm", "raft_term"]).or_else(|| num(&h, &["raft_term", "raftTerm"])).unwrap_or(0),
            raft_index: num(v, &["raftIndex", "raft_index"]).unwrap_or(0),
            raft_applied_index: num(v, &["raftAppliedIndex", "raft_applied_index"]).unwrap_or(0),
            db_size: num(v, &["dbSize", "db_size"]).unwrap_or(0),
            db_size_in_use: num(v, &["dbSizeInUse", "db_size_in_use"]).unwrap_or(0),
            is_learner: v.get("isLearner").or_else(|| v.get("is_learner")).and_then(Value::as_bool).unwrap_or(false),
            errors: v
                .get("errors")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Member {
    pub id: String,
    pub name: String,
    pub peer_urls: Vec<String>,
    pub client_urls: Vec<String>,
    pub is_learner: bool,
}

impl From<&Value> for Member {
    fn from(v: &Value) -> Self {
        let urls = |keys: &[&str]| {
            keys.iter()
                .find_map(|k| v.get(*k).and_then(Value::as_array))
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default()
        };
        Member {
            id: id(v, &["ID", "id"]),
            name: v.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
            peer_urls: urls(&["peerURLs", "peer_urls", "peerUrls"]),
            client_urls: urls(&["clientURLs", "client_urls", "clientUrls"]),
            is_learner: v.get("isLearner").or_else(|| v.get("is_learner")).and_then(Value::as_bool).unwrap_or(false),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Alarm {
    pub member_id: String,
    pub alarm: String,
}

pub struct Keys {
    pub keys: Vec<String>,
    /// How many keys are in the range, which is more than `keys` when
    /// `more` is set.
    pub count: u64,
    pub more: bool,
}

pub struct Kv {
    pub key: String,
    pub value: Vec<u8>,
    pub create_revision: u64,
    pub mod_revision: u64,
    pub version: u64,
    pub lease: u64,
}

fn kvs(v: &Value) -> Vec<Value> {
    v.get("kvs").and_then(Value::as_array).cloned().unwrap_or_default()
}

fn bytes(v: &Value, key: &str) -> Option<Vec<u8>> {
    v.get(key).and_then(Value::as_str).and_then(|s| B64.decode(s).ok())
}

/// A 64-bit integer, as a JSON string (protobuf's mapping) or number.
pub fn num(v: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|k| match v.get(*k)? {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_u64(),
        _ => None,
    })
}

/// A member or cluster id, printed the way `etcdctl` prints them: hex.
fn id(v: &Value, keys: &[&str]) -> String {
    num(v, keys).map(|n| format!("{n:x}")).unwrap_or_default()
}

fn reason(e: &reqwest::Error) -> String {
    use std::error::Error as _;
    e.source().map(|s| s.to_string()).unwrap_or_else(|| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `curl -X POST :2379/v3/maintenance/status -d '{}'` against etcd 3.5.
    #[test]
    fn status_in_etcds_own_shape() {
        let v = json!({
            "header": {"cluster_id": "14841639068965178418", "member_id": "10276657743932975437",
                       "revision": "412", "raft_term": "2"},
            "version": "3.5.17", "dbSize": "25001984", "leader": "10276657743932975437",
            "raftIndex": "431", "raftTerm": "2", "raftAppliedIndex": "431",
            "dbSizeInUse": "24981504", "errors": ["memberID:1 alarm:NOSPACE "]
        });
        let s = Status::from(&v);
        assert_eq!(s.member_id, "8e9e05c52164694d");
        assert_eq!(s.leader, s.member_id);
        assert_eq!(s.revision, 412);
        assert_eq!(s.raft_term, 2);
        assert_eq!(s.raft_index, 431);
        assert_eq!(s.db_size, 25001984);
        assert_eq!(s.db_size_in_use, 24981504);
        assert_eq!(s.version, "3.5.17");
        assert_eq!(s.errors.len(), 1);
        assert!(!s.is_learner);
    }

    #[test]
    fn members_in_either_spelling() {
        let v = json!({"ID": "10276657743932975437", "name": "default",
                       "peerURLs": ["http://localhost:2380"], "clientURLs": ["http://localhost:2379"]});
        let m = Member::from(&v);
        assert_eq!(m.id, "8e9e05c52164694d");
        assert_eq!(m.client_urls, vec!["http://localhost:2379"]);
        let v = json!({"id": 1, "name": "b", "peer_urls": ["p"], "is_learner": true});
        let m = Member::from(&v);
        assert_eq!((m.id.as_str(), m.peer_urls.len(), m.is_learner), ("1", 1, true));
    }

    #[test]
    fn a_prefix_range_ends_one_past_the_prefix() {
        assert_eq!(prefix_end(b"/registry/"), b"/registry0".to_vec());
        assert_eq!(prefix_end(b"a\xff"), b"b".to_vec());
        assert_eq!(prefix_end(b""), vec![0]);
    }
}
