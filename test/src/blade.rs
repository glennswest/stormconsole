//! `blade` (declared, 15 min): #69's done-when on the real node the suite
//! runs on (#76).
//!
//! #69's pod and VM pages were verified only against a stand-in kubelet,
//! whose answers follow rustkube-node as it was read on 2026-10-02. This
//! asks the node's own console the same questions a person opening the
//! pages asks, of the node's own objects:
//!
//! - **cilium's pod** (`kube-system/cilium-…`): its addresses, each image
//!   with its digest and build date, live counters, its logs (the current
//!   run, and the previous one or the node's reason there is none), events
//!   and probes;
//! - **a bridged VM** made for the run (`fedora-44-x86_64`, its NIC on the
//!   node's `stormbr0`, a seed that prints its address to the serial
//!   console): Running, its address on stormbr0, its image, its counters,
//!   its serial log through the console's door.
//!
//! What the node does not report yet is named by the page with its issue
//! (rustkube-node#130, #131, #242, stormvm#48). Those are recorded as
//! **skip** with the page's own words — expected, not passed and not
//! failed. A fail is the page not answering, or answering something a real
//! node contradicts.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::report::{Outcome, Report};
use crate::Ctx;

pub async fn run(ctx: &Ctx, r: &mut Report) {
    cilium(ctx, r).await;
    bridged_vm(ctx, r).await;
}

// ---- cilium's pod

/// The first cilium agent pod in the feed: `cilium-…` but not the operator
/// or envoy.
async fn find_cilium(ctx: &Ctx) -> Option<(String, String)> {
    let feed = ctx.console.components().await.ok()?;
    feed.iter()
        .filter_map(|c| c["id"].as_str()?.strip_prefix("k8s:pod:").map(String::from))
        .filter_map(|key| key.split_once('/').map(|(n, p)| (n.to_string(), p.to_string())))
        .find(|(_, p)| p.starts_with("cilium-") && !p.starts_with("cilium-operator") && !p.starts_with("cilium-envoy"))
}

async fn cilium(ctx: &Ctx, r: &mut Report) {
    let Some((ns, name)) = find_cilium(ctx).await else {
        r.record("cilium-pod", Outcome::Skip("no cilium agent pod in the console's feed: not the cilium edition".into()), 0, None);
        return;
    };
    let base = format!("/api/plugins/k8s/pods/{ns}/{name}");
    let d = match ctx.console.get(&base).await {
        Ok(a) if a.status == 200 => a.body,
        Ok(a) => {
            r.record("cilium-pod", Outcome::Fail(format!("{base}: HTTP {}: {}", a.status, short(&a.text))), 0, None);
            return;
        }
        Err(e) => {
            r.record("cilium-pod", Outcome::Fail(e), 0, None);
            return;
        }
    };
    r.record("cilium-pod", Outcome::Pass(format!("{ns}/{name} on {}", d["metadata"]["node"].as_str().unwrap_or("?"))), 0, None);
    let gap = |what: &str| -> Option<String> {
        d["gaps"].as_array()?.iter().find(|g| g["what"] == what).map(|g| {
            format!("{} ({})", g["why"].as_str().unwrap_or(""), g["issue"].as_str().unwrap_or(""))
        })
    };

    // Addresses: its own, or the node's when it is on the host network.
    let net = &d["network"];
    let ips: Vec<&str> = net["podIPs"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let host_net = net["hostNetwork"].as_bool() == Some(true);
    let host_ip = net["hostIP"].as_str().unwrap_or("");
    r.record(
        "cilium-addresses",
        if !ips.is_empty() {
            Outcome::Pass(format!("{}{}", ips.join(", "), if host_net { " (the host network)" } else { "" }))
        } else if host_net && !host_ip.is_empty() {
            Outcome::Pass(format!("the host network: the node's {host_ip}"))
        } else {
            Outcome::Fail(format!("no address: podIPs {} hostIP {host_ip:?}", net["podIPs"]))
        },
        0,
        None,
    );

    // Each image, with its digest or the gap that says why not.
    let containers = d["containers"].as_array().cloned().unwrap_or_default();
    let mut lines = Vec::new();
    let mut bad = Vec::new();
    for c in &containers {
        let n = c["name"].as_str().unwrap_or("?");
        let image = c["image"].as_str().unwrap_or("");
        match c["digest"].as_str() {
            Some(dg) => lines.push(format!("{n}: {image} {dg}")),
            None if gap("image digest").is_some() || c["reported"] == false => {
                lines.push(format!("{n}: {image}, no digest (imageID {:?})", c["imageID"].as_str().unwrap_or("")))
            }
            None => bad.push(format!("{n}: {image} with no digest and no gap named")),
        }
    }
    r.record(
        "cilium-image",
        if containers.is_empty() {
            Outcome::Fail("no containers".into())
        } else if bad.is_empty() {
            Outcome::Pass(lines.join(" · "))
        } else {
            Outcome::Fail(bad.join(" · "))
        },
        0,
        Some(json!({"digest_gap": gap("image digest")})),
    );

    // The build date: a golden's, from stormcentral, when the console has
    // one to ask; else why not.
    let built: Vec<String> = containers
        .iter()
        .filter_map(|c| c["golden"]["builtAt"].as_i64().map(|t| format!("{}: built {t} ({})", c["name"].as_str().unwrap_or("?"), c["golden"]["name"].as_str().unwrap_or(""))))
        .collect();
    let why_not: Vec<String> = containers
        .iter()
        .filter_map(|c| c["golden"]["reason"].as_str().map(|w| format!("{}: {w}", c["name"].as_str().unwrap_or("?"))))
        .chain(gap("build info"))
        .collect();
    r.record(
        "cilium-build-date",
        if !built.is_empty() { Outcome::Pass(built.join(" · ")) } else { Outcome::Skip(format!("not shown: {}", why_not.join(" · "))) },
        0,
        None,
    );

    // Counters: the kubelet's, per interface; the host network has none.
    match ctx.console.get(&format!("{base}/traffic")).await {
        Ok(a) if a.body["available"] == true => {
            let ifs: Vec<String> = a.body["interfaces"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|i| format!("{} rx {} tx {}", i["interface"].as_str().unwrap_or("?"), i["rxBytes"], i["txBytes"]))
                .collect();
            r.record("cilium-counters", Outcome::Pass(ifs.join(" · ")), 0, Some(json!({"missing": a.body["missing"]})));
        }
        Ok(a) if host_net && a.body["reason"].as_str().is_some_and(|w| w.contains("host network")) => {
            r.record("cilium-counters", Outcome::Pass(format!("said: {}", a.body["reason"].as_str().unwrap_or(""))), 0, None)
        }
        Ok(a) => r.record("cilium-counters", Outcome::Fail(format!("HTTP {}: {}", a.status, a.body["reason"].as_str().unwrap_or(&short(&a.text)))), 0, None),
        Err(e) => r.record("cilium-counters", Outcome::Fail(e), 0, None),
    };
    // Stats over time (#124): the console scrapes the kubelet every 15 s,
    // so a minute is long enough for a rate.
    let t = Instant::now();
    let stats = loop {
        let a = ctx.console.get(&format!("{base}/stats?window=900")).await;
        let ready = matches!(&a, Ok(a) if a.body["available"] == true
            && a.body["series"]["containers"].as_array().is_some_and(|cs| cs.iter().any(|c| c["points"].as_array().is_some_and(|p| p.iter().any(|p| !p["cpu"].is_null())))));
        if ready || t.elapsed() > Duration::from_secs(75) {
            break a;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    };
    match stats {
        Ok(a) if a.body["available"] == true => {
            let cs: Vec<String> = a.body["series"]["containers"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| {
                    let last = c["points"].as_array().and_then(|p| p.last()).cloned().unwrap_or(Value::Null);
                    format!("{} cpu {} ws {}", c["name"].as_str().unwrap_or("?"), last["cpu"], last["workingSet"])
                })
                .collect();
            r.record("cilium-stats", Outcome::Pass(cs.join(" · ")), 0, Some(json!({"missing": a.body["missing"], "samples": a.body["samples"]})));
        }
        Ok(a) => r.record("cilium-stats", Outcome::Fail(a.body["reason"].as_str().map(String::from).unwrap_or_else(|| short(&a.text))), 0, None),
        Err(e) => r.record("cilium-stats", Outcome::Fail(e), 0, None),
    };

    // Logs: the current run of the agent container, and the previous one —
    // or the node's own words for why there is none.
    let main = containers
        .iter()
        .find(|c| c["role"] == "container" && c["name"] == "cilium-agent")
        .or_else(|| containers.iter().find(|c| c["role"] == "container"));
    if let Some(c) = main {
        let cn = c["name"].as_str().unwrap_or("");
        match ctx.console.get(&format!("{base}/log?container={cn}&tailLines=20")).await {
            Ok(a) if a.status == 200 && !a.text.trim().is_empty() => r.record(
                "cilium-logs-current",
                Outcome::Pass(format!("{cn}: {} lines, last: {}", a.text.lines().count(), short(a.text.lines().last().unwrap_or("")))),
                0,
                None,
            ),
            Ok(a) => r.record("cilium-logs-current", Outcome::Fail(format!("{cn}: HTTP {} {}", a.status, short(&a.text))), 0, None),
            Err(e) => r.record("cilium-logs-current", Outcome::Fail(e), 0, None),
        };
        let restarts = c["restartCount"].as_i64().unwrap_or(0);
        match ctx.console.get(&format!("{base}/log?container={cn}&previous=true&tailLines=20")).await {
            Ok(a) if a.status == 200 && restarts > 0 => {
                r.record("cilium-logs-previous", Outcome::Pass(format!("{cn} restarted {restarts}×: {} lines of the run before", a.text.lines().count())), 0, None)
            }
            Ok(a) if a.status >= 400 && restarts == 0 => {
                r.record("cilium-logs-previous", Outcome::Pass(format!("no restart, and the node says so: {}", short(&a.text))), 0, None)
            }
            Ok(a) => r.record(
                "cilium-logs-previous",
                Outcome::Fail(format!("{cn} restartCount {restarts} but previous answered HTTP {}: {}", a.status, short(&a.text))),
                0,
                None,
            ),
            Err(e) => r.record("cilium-logs-previous", Outcome::Fail(e), 0, None),
        };
    }

    match ctx.console.get(&format!("{base}/events")).await {
        Ok(a) if a.body["available"] == true => {
            let items = a.body["items"].as_array().cloned().unwrap_or_default();
            let reasons: Vec<String> = items.iter().take(5).map(|e| format!("{}×{}", e["reason"].as_str().unwrap_or("?"), e["count"])).collect();
            r.record("cilium-events", Outcome::Pass(format!("{} events: {}", items.len(), reasons.join(", "))), 0, None);
        }
        Ok(a) => r.record("cilium-events", Outcome::Fail(a.body["reason"].as_str().map(String::from).unwrap_or_else(|| short(&a.text))), 0, None),
        Err(e) => r.record("cilium-events", Outcome::Fail(e), 0, None),
    };

    let probes: Vec<String> = containers
        .iter()
        .flat_map(|c| {
            let n = c["name"].as_str().unwrap_or("?").to_string();
            c["probes"].as_object().into_iter().flatten().map(move |(k, p)| {
                format!("{n} {k} {} {}: {}", p["type"].as_str().unwrap_or(""), p["target"].as_str().unwrap_or(""), p["result"].as_str().unwrap_or("not observed"))
            })
        })
        .collect();
    r.record(
        "cilium-probes",
        if probes.is_empty() { Outcome::Skip("its containers declare no probes".into()) } else { Outcome::Pass(probes.join(" · ")) },
        0,
        None,
    );
}

// ---- a bridged VM

/// The golden a VM boots from on a test machine (stormvm's medium suite uses
/// the same): `STORM_VM_GOLDEN`, else `fedora-44-x86_64`.
fn golden() -> String {
    std::env::var("STORM_VM_GOLDEN").ok().filter(|g| !g.is_empty()).unwrap_or_else(|| "fedora-44-x86_64".into())
}

/// The guest: the golden as its root, its NIC on the node's bridge, a seed
/// that prints a marker and its address to the serial console — the one
/// channel that works before there is a network or a login.
pub fn guest(ns: &str, name: &str, run_id: &str, golden: &str) -> Value {
    let user_data = [
        "#cloud-config",
        "runcmd:",
        "  - [ sh, -c, \"echo STORMCONSOLE-BLADE-OK > /dev/console\" ]",
        "  - [ sh, -c, \"ip -4 -o addr show scope global | sed 's/^/STORMCONSOLE-BLADE-ADDR /' > /dev/console\" ]",
    ]
    .join("\n")
        + "\n";
    json!({
        "apiVersion": "kubevirt.io/v1",
        "kind": "VirtualMachineInstance",
        "metadata": {
            "name": name,
            "namespace": ns,
            "labels": {"storm.io/test-run": run_id, "app.kubernetes.io/managed-by": "stormconsole-test"},
            "annotations": {"storm.io/bridge": "stormbr0"}
        },
        "spec": {
            "domain": {
                "cpu": {"cores": 1},
                "resources": {"requests": {"memory": "1Gi"}},
                "devices": {
                    "disks": [{"name": "root", "disk": {"bus": "virtio"}}, {"name": "seed", "disk": {"bus": "virtio"}}],
                    // As stormvm's own medium guest: the annotation puts the
                    // NIC on the node's bridge whatever the binding says.
                    "interfaces": [{"name": "default", "masquerade": {}}]
                }
            },
            "networks": [{"name": "default", "pod": {}}],
            "volumes": [
                {"name": "root", "dataVolume": {"name": golden}},
                {"name": "seed", "cloudInitNoCloud": {"userData": user_data}}
            ]
        }
    })
}

async fn bridged_vm(ctx: &Ctx, r: &mut Report) {
    let ns = &ctx.env.namespace;
    if !matches!(ctx.kube.get("/apis/kubevirt.io/v1").await, Ok(Some(_))) {
        r.record("vm-running", Outcome::Skip("kubevirt.io/v1 is not served here: no VMs on this node".into()), 0, None);
        return;
    }
    let name = ctx.env.name("bridged");
    let golden = golden();
    let path = format!("/apis/kubevirt.io/v1/namespaces/{ns}/virtualmachineinstances");
    if let Err(e) = ctx.kube.create(&path, &guest(ns, &name, &ctx.env.run_id, &golden)).await {
        r.record("vm-running", Outcome::Fail(format!("the apiserver refused the VM: {e}")), 0, None);
        return;
    }
    let page = format!("/api/plugins/vm/vms/{ns}/{name}");
    let detail = || {
        let page = page.clone();
        async move { ctx.console.get(&page).await.ok().filter(|a| a.status == 200).map(|a| a.body) }
    };

    // Running, through the console's own page.
    let t = Instant::now();
    let mut last = Value::Null;
    let running = loop {
        if let Some(d) = detail().await {
            if d["phase"] == "Running" {
                break true;
            }
            last = d;
        }
        if t.elapsed() > Duration::from_secs(480) {
            break false;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    };
    if !running {
        let why = format!("phase {} — {}", last["phase"], last["reason"].as_str().unwrap_or(""));
        let outcome = if why.contains("golden") || why.contains(&golden) {
            Outcome::Skip(format!("the golden {golden} is not on this node: {why}"))
        } else {
            Outcome::Fail(format!("not Running in 8 min: {why}"))
        };
        r.record("vm-running", outcome, t.elapsed().as_millis(), None);
        let _ = ctx.kube.delete(&format!("{path}/{name}")).await;
        return;
    }
    r.record("vm-running", Outcome::Pass(format!("{ns}/{name} Running in {}s", t.elapsed().as_secs())), t.elapsed().as_millis(), None);

    // Its address on stormbr0: the guest agent's or the node's ARP table's,
    // written by the kubelet into status.interfaces (rustkube-node, #24).
    let t = Instant::now();
    let mut d = Value::Null;
    let addressed = loop {
        if let Some(x) = detail().await {
            d = x;
            let i = &d["interfaces"][0];
            if i["addresses"].as_array().is_some_and(|a| !a.is_empty()) {
                break true;
            }
        }
        if t.elapsed() > Duration::from_secs(240) {
            break false;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    };
    let i = &d["interfaces"][0];
    let said = format!(
        "asked {} · node did {} · {} · reach {} ({})",
        i["asked"].as_str().unwrap_or("?"),
        i["did"].as_str().unwrap_or("not reported"),
        i["addresses"],
        i["reach"].as_str().unwrap_or("?"),
        i["note"].as_str().unwrap_or("")
    );
    r.record(
        "vm-address",
        if addressed && i["did"] == "bridge" { Outcome::Pass(said) } else { Outcome::Fail(said) },
        t.elapsed().as_millis(),
        Some(json!({"interface": i})),
    );

    // Its image.
    let img = &d["images"][0];
    r.record(
        "vm-image",
        if img["source"] == golden.as_str() {
            Outcome::Pass(format!("{} {} · digest {}", img["kind"].as_str().unwrap_or(""), golden, img["digest"]))
        } else {
            Outcome::Fail(format!("images {}", d["images"]))
        },
        0,
        None,
    );

    // Counters: none yet for a machine (stormvm#48); the page must say so.
    let counters_gap = d["gaps"].as_array().and_then(|g| g.iter().find(|g| g["what"] == "traffic counters")).cloned();
    r.record(
        "vm-counters",
        match counters_gap {
            Some(g) => Outcome::Skip(format!("the page says: {} ({})", g["why"].as_str().unwrap_or(""), g["issue"].as_str().unwrap_or(""))),
            None => Outcome::Fail("no counters and no gap named".into()),
        },
        0,
        None,
    );

    // The serial log, through the console's door.
    let t = Instant::now();
    let serial = ctx.console.read_ws(&format!("/api/plugins/vm/console/{ns}/{name}/serial"), Duration::from_secs(120), "STORMCONSOLE-BLADE-ADDR").await;
    r.record(
        "vm-serial",
        match serial {
            Ok(text) if text.contains("STORMCONSOLE-BLADE-ADDR") => {
                let line = text.lines().find(|l| l.contains("STORMCONSOLE-BLADE-ADDR")).unwrap_or("");
                Outcome::Pass(format!("{} bytes; the guest said: {}", text.len(), short(line.trim())))
            }
            Ok(text) if text.contains("[console]") => Outcome::Fail(format!("the door refused: {}", short(text.trim()))),
            Ok(text) if !text.is_empty() => Outcome::Pass(format!("{} bytes of the serial log (no address line yet): {}", text.len(), short(text.trim()))),
            Ok(_) => Outcome::Fail("the door opened and said nothing in 2 min".into()),
            Err(e) => Outcome::Fail(e),
        },
        t.elapsed().as_millis(),
        None,
    );

    let _ = ctx.kube.delete(&format!("{path}/{name}")).await;
}

fn short(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() > 200 {
        format!("{}…", s.chars().take(200).collect::<String>())
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guest_is_bridged_and_boots_the_golden() {
        let g = guest("t-ns", "t-bridged", "r1", "fedora-44-x86_64");
        assert_eq!(g["metadata"]["annotations"]["storm.io/bridge"], "stormbr0");
        assert_eq!(g["metadata"]["labels"]["storm.io/test-run"], "r1");
        assert_eq!(g["spec"]["volumes"][0]["dataVolume"]["name"], "fedora-44-x86_64");
        assert!(g["spec"]["volumes"][1]["cloudInitNoCloud"]["userData"].as_str().unwrap().contains("STORMCONSOLE-BLADE-ADDR"));
    }
}
