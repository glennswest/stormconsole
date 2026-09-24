use super::*;

/// fastetcd today: /health and /metrics, no gateway.
fn fastetcd(metrics: &str) -> Seen {
    Seen {
        alive: Some(true),
        alive_err: String::new(),
        metrics: Some(Samples::parse(metrics)),
        metrics_err: String::new(),
        status: None,
        members: vec![],
        alarms: vec![],
        gateway_err: Some(GwError::NotServed("/v3/maintenance/status responded 404".into())),
        objects: None,
        rates: vec![],
    }
}

const HEALTHY: &str = "\
etcd_server_has_leader 1
etcd_server_leader_changes_seen_total_total 1
etcd_mvcc_db_total_size_in_bytes 4194304
etcd_mvcc_db_total_size_in_use_in_bytes 1048576
etcd_server_quota_backend_bytes 2147483648
fastetcd_store_space_used_ratio 0.002
fastetcd_disk_available_bytes 107374182400
fastetcd_nospace_alarm_active 0
etcd_debugging_mvcc_current_revision 412
etcd_debugging_mvcc_compact_revision 12
";

fn metric<'a>(c: &'a ComponentSummary, label: &str) -> Option<&'a str> {
    c.metrics.iter().find(|m| m.label == label).map(|m| m.value.as_str())
}

#[test]
fn fastetcd_today_is_healthy_and_says_what_it_cannot_show() {
    let (h, line, cs) = build(&fastetcd(HEALTHY), Some("plugin:k8s"));
    assert_eq!(h, Health::Ok);
    assert_eq!(cs.len(), 1, "no members invented from nothing");
    let store = &cs[0];
    assert_eq!(store.id, STORE);
    assert_eq!(metric(store, "revision"), Some("412"));
    assert_eq!(metric(store, "compacted to"), Some("12"));
    assert_eq!(metric(store, "db size"), Some("4.0 MB"));
    assert_eq!(metric(store, "defrag frees"), Some("3.0 MB"));
    assert_eq!(metric(store, "alarms"), Some("none"));
    assert_eq!(metric(store, "leader"), Some("yes"));
    assert_eq!(metric(store, "leader changes"), Some("1"));
    assert_eq!(metric(store, "members"), None, "a count it does not have is not 0");
    assert!(store.detail.contains("fastetcd#28"), "{}", store.detail);
    assert!(store.detail.contains("fastetcd#29"), "{}", store.detail);
    assert!(!line.contains("Not shown"), "the card line stays short: {line}");
    assert!(store.actions.is_empty(), "no verb without something to perform it");
    assert!(store
        .relations
        .iter()
        .any(|r| r.name == "serves" && r.targets == vec!["plugin:k8s".to_string()]));
}

#[test]
fn nospace_from_the_scrape_is_an_error_that_says_writes_are_refused() {
    let m = HEALTHY.replace("fastetcd_nospace_alarm_active 0", "fastetcd_nospace_alarm_active 1");
    let (h, line, cs) = build(&fastetcd(&m), None);
    assert_eq!(h, Health::Error);
    assert!(line.contains("NOSPACE") && line.contains("writes are refused"), "{line}");
    assert_eq!(metric(&cs[0], "alarms"), Some("NOSPACE"));
}

#[test]
fn no_leader_is_an_error() {
    let m = HEALTHY.replace("etcd_server_has_leader 1", "etcd_server_has_leader 0");
    let (h, line, _) = build(&fastetcd(&m), None);
    assert_eq!(h, Health::Error);
    assert!(line.contains("no leader"), "{line}");
}

#[test]
fn nearly_full_warns() {
    let m = HEALTHY.replace("fastetcd_store_space_used_ratio 0.002", "fastetcd_store_space_used_ratio 0.85");
    let (h, _, cs) = build(&fastetcd(&m), None);
    assert_eq!(h, Health::Warn);
    assert_eq!(metric(&cs[0], "space used"), Some("85.0%"));
}

#[test]
fn metrics_off_is_a_warning_not_an_outage() {
    let mut s = fastetcd(HEALTHY);
    s.metrics = None;
    s.metrics_err = "Connection refused (os error 111)".into();
    let (h, line, _) = build(&s, None);
    assert_eq!(h, Health::Warn);
    assert!(line.contains("metrics not reachable"), "{line}");
}

#[test]
fn nothing_answering_is_unreachable() {
    let mut s = fastetcd(HEALTHY);
    s.alive = None;
    s.alive_err = "Connection refused (os error 111)".into();
    s.metrics = None;
    s.gateway_err = Some(GwError::Failed("refused".into()));
    let (h, line, cs) = build(&s, None);
    assert_eq!(h, Health::Error);
    assert!(line.starts_with("unreachable"), "{line}");
    assert!(!cs[0].detail.contains("fastetcd#28"), "an unreachable store has no gaps to explain");
}

fn gateway(alarms: Vec<Alarm>) -> Seen {
    let mut s = fastetcd(HEALTHY);
    s.gateway_err = None;
    s.status = Some(Status {
        member_id: "a1".into(),
        cluster_id: "c".into(),
        leader: "b2".into(),
        version: "3.5.17".into(),
        revision: 500,
        raft_term: 3,
        raft_index: 900,
        raft_applied_index: 900,
        db_size: 8 << 20,
        db_size_in_use: 2 << 20,
        is_learner: false,
        errors: vec![],
    });
    let mem = |id: &str, name: &str, learner: bool| Member {
        id: id.into(),
        name: name.into(),
        peer_urls: vec![format!("http://{name}:2380")],
        client_urls: vec![format!("http://{name}:2379")],
        is_learner: learner,
    };
    s.members = vec![mem("a1", "n1", false), mem("b2", "n2", false), mem("c3", "n3", true)];
    s.alarms = alarms;
    s.objects = Some(1234);
    s
}

#[test]
fn with_the_gateway_every_member_is_a_row_and_the_leader_is_marked() {
    let (h, _, cs) = build(&gateway(vec![]), None);
    assert_eq!(h, Health::Ok);
    let store = &cs[0];
    assert!(!store.detail.contains("fastetcd#28"), "{}", store.detail);
    assert_eq!(metric(store, "revision"), Some("500"), "the gateway's revision wins");
    assert_eq!(metric(store, "raft term"), Some("3"));
    assert_eq!(metric(store, "members"), Some("3"));
    assert_eq!(metric(store, "objects"), Some("1234"));
    assert_eq!(metric(store, "version"), Some("3.5.17"));
    let rel = store.relations.iter().find(|r| r.name == "members").unwrap();
    assert_eq!(rel.targets.len(), 3);
    let role = |id: &str| metric(cs.iter().find(|c| c.id == member_id(id)).unwrap(), "role").unwrap().to_string();
    assert_eq!(role("a1"), "follower");
    assert_eq!(role("b2"), "leader");
    assert_eq!(role("c3"), "learner");
    let me = cs.iter().find(|c| c.id == member_id("a1")).unwrap();
    assert_eq!(metric(me, "answering"), Some("this node's store"));
    let ids: Vec<&str> = store.actions.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, vec!["compact", "defragment"]);
    assert!(store.actions.iter().all(|a| a.danger), "every verb here is confirmed");
    assert_eq!(store.actions[0].path, "/api/plugins/etcd/compact?revision=500");
}

#[test]
fn an_alarm_lands_on_its_member_with_a_way_to_disarm_it() {
    let (h, _, cs) = build(&gateway(vec![Alarm { member_id: "b2".into(), alarm: "CORRUPT".into() }]), None);
    assert_eq!(h, Health::Error);
    let n2 = cs.iter().find(|c| c.id == member_id("b2")).unwrap();
    assert_eq!(n2.health, Health::Error);
    assert!(n2.actions.iter().any(|a| a.path == "/api/plugins/etcd/disarm?member=b2&alarm=CORRUPT"));
    let n1 = cs.iter().find(|c| c.id == member_id("a1")).unwrap();
    assert_eq!(n1.health, Health::Ok, "the alarm is b2's, not everyone's");
}

#[test]
fn the_keyspace_reads_as_directories() {
    let keys: Vec<String> = [
        "/registry/namespaces/default",
        "/registry/namespaces/kube-system",
        "/registry/pods/default/web-1",
        "/registry/pods/default/web-2",
        "/registry/apps/deployments/default/web",
        "/registry/ranges",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let t = tree("/registry/", &keys, 6, false);
    let kids = t["children"].as_array().unwrap();
    let by = |n: &str| kids.iter().find(|c| c["name"] == n).unwrap().clone();
    assert_eq!(by("namespaces/")["count"], 2);
    assert_eq!(by("pods/")["count"], 2);
    assert_eq!(by("apps/")["path"], "/registry/apps/");
    assert_eq!(by("ranges")["leaf"], true);
    assert_eq!(t["total"], 6);
}

#[test]
fn only_admin_reads_the_keyspace() {
    let op = Viewer { user: Some("o".into()), roles: vec!["operator".into()], ..Viewer::anonymous() };
    let ad = Viewer { user: Some("a".into()), roles: vec!["admin".into()], ..Viewer::anonymous() };
    assert!(admin_only(&op).is_some());
    assert!(admin_only(&ad).is_none());
}
