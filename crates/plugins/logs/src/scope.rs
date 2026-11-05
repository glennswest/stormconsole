//! Which of the hosts heard on the group this console shows (#161).
//!
//! The group is a segment's: every node on it is heard, whichever cluster it
//! belongs to. So everything heard passes through one rule before anything
//! reads it: **this node and its cluster's members, and nobody else**. The
//! ring keeps what it heard. A node that joins has its history, and a node
//! that leaves stops being shown without anything being rewritten.
//!
//! A machine can also be heard under several names: a line from before its
//! hostname was set, or one too broken to parse, which is kept under the
//! sender's address. The address says they are one machine, so they are one
//! row, under the member's name.

use std::collections::{BTreeMap, HashMap};

use console_core::members::Member;

use crate::store::HostSummary;

#[derive(Clone, Debug, Default)]
pub struct Scope {
    /// Every name a line may carry, to the node it means.
    canonical: HashMap<String, String>,
    /// The members, for those that have not been heard yet.
    members: Vec<Member>,
}

impl Scope {
    pub fn build(local: &str, members: &[Member], heard: &[HostSummary]) -> Self {
        // This node, as the apiserver names it if that differs from its
        // hostname: the row is the hostname's, since that is what is "local".
        let local_addr = heard.iter().find(|h| h.host == local).map(|h| h.addr.as_str()).unwrap_or("");
        let is_local = |m: &Member| m.name == local || (!local_addr.is_empty() && m.addrs.iter().any(|a| a == local_addr));
        let name_of = |m: &Member| if is_local(m) { local.to_string() } else { m.name.clone() };

        let mut canonical: HashMap<String, String> = HashMap::new();
        let mut by_addr: HashMap<String, String> = HashMap::new();
        for m in members {
            let name = name_of(m);
            canonical.insert(m.name.clone(), name.clone());
            for a in &m.addrs {
                by_addr.insert(a.clone(), name.clone());
            }
        }
        if !local.is_empty() {
            canonical.insert(local.to_string(), local.to_string());
        }
        if !local_addr.is_empty() {
            by_addr.insert(local_addr.to_string(), local.to_string());
        }
        // Where a member is heard from, even if its Node object lists nothing.
        for h in heard {
            if let (Some(name), false) = (canonical.get(&h.host), h.addr.is_empty()) {
                by_addr.entry(h.addr.clone()).or_insert_with(|| name.clone());
            }
        }
        for h in heard {
            if let Some(name) = by_addr.get(&h.addr) {
                canonical.entry(h.host.clone()).or_insert_with(|| name.clone());
            }
        }
        // An unparseable line is kept under its sender's address.
        for (addr, name) in by_addr {
            canonical.entry(addr).or_insert(name);
        }
        let members = members.iter().map(|m| Member { name: name_of(m), addrs: m.addrs.clone() }).collect();
        Self { canonical, members }
    }

    /// The node a line's host means, or `None` for a machine not shown here.
    pub fn node_of(&self, host: &str) -> Option<&str> {
        self.canonical.get(host).map(String::as_str)
    }

    /// One row per node: what was heard from this cluster, merged, and a
    /// row for a member that has not been heard yet (`count` 0, no `last_ts`).
    pub fn rows(&self, heard: Vec<HostSummary>) -> Vec<HostSummary> {
        let mut rows: BTreeMap<String, HostSummary> = BTreeMap::new();
        for h in heard {
            let Some(node) = self.node_of(&h.host).map(str::to_string) else { continue };
            let own_name = h.host == node;
            match rows.get_mut(&node) {
                None => {
                    rows.insert(node.clone(), HostSummary { host: node, ..h });
                }
                Some(r) => {
                    r.count += h.count;
                    r.entries += h.entries;
                    // RFC 3339 from one clock family sorts as text.
                    let newer = h.last_ts > r.last_ts;
                    if newer {
                        r.last_ts = h.last_ts;
                    }
                    if !h.addr.is_empty() && (own_name || newer || r.addr.is_empty()) {
                        r.addr = h.addr;
                    }
                }
            }
        }
        for m in &self.members {
            rows.entry(m.name.clone()).or_insert_with(|| HostSummary {
                host: m.name.clone(),
                addr: m.addrs.first().cloned().unwrap_or_default(),
                count: 0,
                entries: 0,
                last_ts: String::new(),
            });
        }
        rows.into_values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heard(host: &str, addr: &str, count: i64, last_ts: &str) -> HostSummary {
        HostSummary { host: host.into(), addr: addr.into(), count, entries: count, last_ts: last_ts.into() }
    }
    fn member(name: &str, addrs: &[&str]) -> Member {
        Member { name: name.into(), addrs: addrs.iter().map(|a| a.to_string()).collect() }
    }
    fn names(rows: &[HostSummary]) -> Vec<&str> {
        rows.iter().map(|r| r.host.as_str()).collect()
    }

    /// forge1 as seen on 2026-11-05: itself, and four other clusters' nodes.
    fn forge1() -> Vec<HostSummary> {
        vec![
            heard("storm-94a281", "192.168.31.203", 10, "2026-11-05T00:15:00Z"),
            heard("storm-003101", "192.168.31.10", 5, "2026-11-05T00:15:00Z"),
            heard("pvetest1", "192.168.31.10", 5, "2026-11-05T00:05:00Z"),
            heard("storm-84b7e0", "192.168.16.20", 5, "2026-11-05T00:15:00Z"),
            heard("server1", "192.168.16.20", 5, "2026-11-04T23:15:00Z"),
            heard("storm-84ae30", "192.168.16.115", 5, "2026-11-04T22:55:00Z"),
        ]
    }

    #[test]
    fn a_single_node_shows_only_itself() {
        let h = forge1();
        let s = Scope::build("storm-94a281", &[member("storm-94a281", &["192.168.31.203"])], &h);
        assert_eq!(names(&s.rows(h)), ["storm-94a281"]);
        assert_eq!(s.node_of("storm-003101"), None);
        assert_eq!(s.node_of("server1"), None);
        assert_eq!(s.node_of("192.168.16.115"), None);
    }

    #[test]
    fn with_no_apiserver_answer_it_is_this_node_alone() {
        let h = forge1();
        let s = Scope::build("storm-94a281", &[], &h);
        assert_eq!(names(&s.rows(h)), ["storm-94a281"]);
    }

    #[test]
    fn a_cluster_shows_its_members_one_row_each() {
        let h = forge1();
        let m = [member("storm-94a281", &["192.168.31.203"]), member("storm-003101", &["192.168.31.10"])];
        let s = Scope::build("storm-94a281", &m, &h);
        let rows = s.rows(h);
        assert_eq!(names(&rows), ["storm-003101", "storm-94a281"]);
        assert_eq!(rows[0].count, 10, "the old name's lines are this machine's");
        assert_eq!(rows[0].last_ts, "2026-11-05T00:15:00Z");
        assert_eq!(s.node_of("pvetest1"), Some("storm-003101"));
    }

    #[test]
    fn the_newest_of_the_merged_rows_says_when_it_was_last_heard() {
        let h = vec![heard("old", "10.0.0.2", 1, "2026-11-05T01:00:00Z"), heard("m", "10.0.0.2", 1, "2026-11-05T00:00:00Z")];
        let s = Scope::build("here", &[member("m", &["10.0.0.2"])], &h);
        assert_eq!(s.rows(h).iter().find(|r| r.host == "m").unwrap().last_ts, "2026-11-05T01:00:00Z");
    }

    #[test]
    fn a_member_not_heard_yet_still_has_a_row() {
        let s = Scope::build("here", &[member("here", &[]), member("quiet", &["10.0.0.9"])], &[]);
        let rows = s.rows(vec![]);
        let q = rows.iter().find(|r| r.host == "quiet").unwrap();
        assert_eq!((q.addr.as_str(), q.count, q.last_ts.as_str()), ("10.0.0.9", 0, ""));
    }

    #[test]
    fn a_member_with_no_address_on_its_object_is_still_merged_by_where_it_is_heard_from() {
        let h = vec![heard("m", "10.0.0.2", 1, "t"), heard("old", "10.0.0.2", 1, "t"), heard("far", "10.0.0.3", 1, "t")];
        let s = Scope::build("here", &[member("m", &[])], &h);
        assert_eq!(names(&s.rows(h)), ["m"]);
    }

    #[test]
    fn this_nodes_other_names_are_this_node() {
        // Its Node object under another name, and a line kept under its address.
        let h = vec![heard("here", "10.0.0.1", 2, "t"), heard("10.0.0.1", "10.0.0.1", 1, "t")];
        let s = Scope::build("here", &[member("node-a", &["10.0.0.1"])], &h);
        let rows = s.rows(h);
        assert_eq!(names(&rows), ["here"]);
        assert_eq!(rows[0].count, 3);
        assert_eq!(s.node_of("node-a"), Some("here"));
    }
}
