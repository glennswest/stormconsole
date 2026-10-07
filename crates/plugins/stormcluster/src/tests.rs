use super::objects::*;
use super::*;

#[test]
fn only_stormclusters_reads_are_forwarded() {
    assert!(forwardable("api/v1/components"));
    assert!(forwardable("api/v1/operations"));
    assert!(forwardable("api/v1/operations/form-1"));
    assert!(forwardable("/api/v1/peers"));
    // Gone with stormcluster#12: a change is an object.
    assert!(!forwardable("api/v1/members/b2/split"));
    // The dry run has its own route; the record is between stormclusters.
    assert!(!forwardable("api/v1/plan"));
    assert!(!forwardable("api/v1/record"));
    assert!(!forwardable("api/v1/peers/../record"));
    assert!(!forwardable("api/v1/peersx"));
    assert!(!forwardable("healthz"));
}

/// A form writes every member, then the `Cluster`: the members already
/// there when the `Cluster` appears join in the same operation.
#[test]
fn a_form_is_its_members_then_the_cluster() {
    let f = Form {
        name: "storm".into(),
        masters: vec!["b1".into(), "b2".into(), "b3".into()],
        workers: vec!["b4".into()],
        storage: vec!["b4".into()],
        forge: false,
    };
    let objs = form_objects(&f, "b1").unwrap();
    let names: Vec<String> =
        objs.iter().map(|o| format!("{} {} {}", o["kind"].as_str().unwrap(), o["metadata"]["name"].as_str().unwrap(), o["spec"])).collect();
    assert_eq!(
        names,
        vec![
            r#"ClusterMember b1 {"role":"master"}"#,
            r#"ClusterMember b2 {"role":"master"}"#,
            r#"ClusterMember b3 {"role":"master"}"#,
            r#"ClusterMember b4 {"role":"worker","storage":true}"#,
            "Cluster storm null",
        ]
    );
    assert_eq!(objs[0]["apiVersion"], "cluster.storm.io/v1alpha1");
    // Seeded where the Cluster is written, so this node must be a master.
    let e = form_objects(&Form { masters: vec!["b2".into()], ..f.clone() }, "b1").unwrap_err();
    assert!(e.contains("seeded on b1"), "{e}");
    let e = form_objects(&Form { workers: vec!["b1".into()], ..f.clone() }, "b1").unwrap_err();
    assert!(e.contains("both a master and a worker"), "{e}");
    assert!(form_objects(&Form { name: "Storm Cluster".into(), ..f }, "b1").unwrap_err().contains("not a name"));
}

#[test]
fn a_join_is_one_member_each_and_a_change_is_a_spec_patch() {
    let objs = join_objects(&Join { nodes: vec!["b5".into(), "b6".into()], role: "master".into(), storage: false }).unwrap();
    assert_eq!(objs.len(), 2);
    assert_eq!(objs[1]["spec"], json!({"role": "master"}));
    assert!(join_objects(&Join { nodes: vec!["b5".into()], role: "boss".into(), storage: false }).unwrap_err().contains("not a role"));
    assert!(join_objects(&Join { nodes: vec![], role: "worker".into(), storage: false }).is_err());

    assert_eq!(change_patch(&Change { drain: Some(true), ..Default::default() }).unwrap(), json!({"spec": {"drain": true}}));
    assert_eq!(change_patch(&Change { role: Some("master".into()), ..Default::default() }).unwrap(), json!({"spec": {"role": "master"}}));
    assert!(change_patch(&Change::default()).is_err());
}

/// Each change is previewed with the dry-run request for the same thing.
#[test]
fn a_change_is_previewed_as_stormcluster_names_it() {
    let role = |r: &str| Change { role: Some(r.into()), ..Default::default() };
    assert_eq!(change_plan("b2", &role("master"), Some("worker")), Some(json!({"op": "promote", "nodes": ["b2"]})));
    assert_eq!(change_plan("b2", &role("worker"), Some("master")), Some(json!({"op": "demote", "node": "b2"})));
    assert_eq!(change_plan("b2", &Change { drain: Some(false), ..Default::default() }, None), Some(json!({"op": "uncordon", "node": "b2"})));
    assert_eq!(
        change_plan("b4", &Change { storage: Some(true), ..Default::default() }, None),
        Some(json!({"op": "storage", "node": "b4", "storage": true}))
    );
}

/// stormcluster's own sentences reach the steps (stormcluster#11 settled);
/// the console no longer keeps a copy of them.
#[test]
fn a_plan_carries_stormclusters_descriptions() {
    let v = plan_answer(
        true,
        json!({"plan": {"steps": [{"step": "cordon", "node": "b3"}, {"step": "nodeLeave", "node": "b3"}, {"step": "new"}], "warnings": ["2 masters remain"]},
               "descriptions": ["cordon b3", "release b3 to SNO, erasing its data"]}),
    );
    let d: Vec<&str> = v["plan"]["steps"].as_array().unwrap().iter().map(|s| s["description"].as_str().unwrap()).collect();
    assert_eq!(d, vec!["cordon b3", "release b3 to SNO, erasing its data", "new"]);
    assert_eq!(v["plan"]["warnings"][0], "2 masters remain");
    assert!(v.get("error").is_none());
}

#[test]
fn a_refused_plan_carries_every_reason_and_who_coordinates() {
    let v = plan_answer(false, json!({"coordinator": "b1", "response": {"refused": ["b2 is not an SNO", "an even control plane"]}}));
    assert_eq!(v["coordinator"], "b1");
    assert_eq!(v["error"], "refused: b2 is not an SNO; an even control plane (by b1, which coordinates this)");
    assert_eq!(plan_answer(false, json!({}))["error"], "stormcluster could not plan this");
}

#[test]
fn an_object_is_read_with_its_status_and_whether_it_is_going() {
    let s = summary(&json!({"metadata": {"name": "b2", "deletionTimestamp": "2026-10-06T00:00:00Z"},
                            "spec": {"role": "worker"}, "status": {"phase": "Leaving", "blockers": []}}));
    assert_eq!(s["name"], "b2");
    assert_eq!(s["deleting"], true);
    assert_eq!(s["status"]["phase"], "Leaving");
    assert_eq!(summary(&json!({"metadata": {"name": "x"}}))["status"], json!({}));
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
