use super::*;

fn viewer(roles: &[&str]) -> Viewer {
    Viewer { user: Some("x".into()), roles: roles.iter().map(|r| r.to_string()).collect(), ..Viewer::anonymous() }
}

#[test]
fn reads_are_open_and_every_write_is_an_administrators() {
    assert!(allowed(&Method::GET, &viewer(&[])));
    assert!(!allowed(&Method::POST, &viewer(&[])));
    assert!(!allowed(&Method::POST, &viewer(&["operator"])));
    assert!(allowed(&Method::POST, &viewer(&["admin"])));
}

#[test]
fn only_the_operator_api_is_forwarded() {
    assert!(forwardable("api/v1/components"));
    assert!(forwardable("/api/v1/members/b2/split"));
    assert!(forwardable("api/v1/peers/b3/join"));
    assert!(forwardable("api/v1/operations/form-1/resume"));
    assert!(forwardable("api/v1/operations"));
    // Between stormclusters only.
    assert!(!forwardable("api/v1/record"));
    assert!(!forwardable("api/v1/members/../record"));
    assert!(!forwardable("api/v1/peersx"));
    assert!(!forwardable("healthz"));
}

#[test]
fn a_forwarded_answer_is_the_coordinators_and_names_it() {
    let v = normalise(
        StatusCode::CONFLICT,
        json!({"coordinator": "b1", "response": {"refused": ["b2 is not an SNO", "an even control plane"]}}),
    );
    assert_eq!(v["coordinator"], "b1");
    assert_eq!(v["refused"][1], "an even control plane");
    assert_eq!(v["error"], "refused: b2 is not an SNO; an even control plane (by b1, which coordinates this)");
}

#[test]
fn a_refusal_here_carries_every_reason_as_the_error() {
    let v = normalise(StatusCode::CONFLICT, json!({"refused": ["a", "b"]}));
    assert_eq!(v["error"], "refused: a; b");
    // An error stormcluster wrote itself is kept.
    let v = normalise(StatusCode::BAD_GATEWAY, json!({"error": "b1 coordinates this, and it has not been discovered"}));
    assert_eq!(v["error"], "b1 coordinates this, and it has not been discovered");
    // A failure without words gets some.
    let v = normalise(StatusCode::INTERNAL_SERVER_ERROR, json!({}));
    assert_eq!(v["error"], "stormcluster answered 500 Internal Server Error");
}

#[test]
fn a_plans_steps_are_described_as_stormcluster_would() {
    let v = normalise(
        StatusCode::OK,
        json!({"plan": {"steps": [
            {"step": "cordon", "node": "b3"},
            {"step": "nodeLeave", "node": "b3", "keep_data": false},
            {"step": "recordMember", "node": "b2", "role": "master", "state": "joining"},
            {"step": "publish"},
            {"step": "somethingNew", "node": "b4"},
            {"step": "evict", "node": "b3", "description": "upstream words win"},
        ], "warnings": ["2 masters remain"]}}),
    );
    let d: Vec<&str> = v["plan"]["steps"].as_array().unwrap().iter().map(|s| s["description"].as_str().unwrap()).collect();
    assert_eq!(
        d,
        vec![
            "cordon b3",
            "revert b3 to SNO (wiping its data)",
            "record b2 as master, joining",
            "publish the cluster record to every member",
            "somethingNew · node b4",
            "upstream words win",
        ]
    );
    assert_eq!(v["plan"]["warnings"][0], "2 masters remain");
    assert!(v.get("error").is_none());
}

#[test]
fn a_started_operation_passes_through() {
    let op = json!({"id": "drain-1", "state": "running", "steps": [{"step": {"step": "cordon", "node": "b2"}, "description": "cordon b2", "status": "running"}]});
    assert_eq!(normalise(StatusCode::ACCEPTED, op.clone()), op);
}

#[test]
fn the_nav_puts_membership_first_in_cluster() {
    let p = StormclusterPlugin::new("http://127.0.0.1:9102/", None);
    let nav = p.nav();
    assert_eq!(nav[0].label, "Cluster");
    assert_eq!(nav[0].items[0].href, "#/cluster");
    assert_eq!(p.inner.base, "http://127.0.0.1:9102");
    assert!(StormclusterPlugin::new("http://x", Some(" \n".into())).inner.token.is_none());
}
