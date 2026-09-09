//! Creating a VM.
//!
//! Two doors, the way OpenShift has two: a form for the ordinary case,
//! and YAML for everything else. The form builds a
//! `VirtualMachineInstance` — not a `VirtualMachine` — because nothing
//! turns a definition into an instance yet (stormvm `docs/kube.md`: "a
//! controller turning a running VirtualMachine into an instance, and a
//! scheduler placing it" is on the Left list, and "today an instance is
//! applied with a nodeName"). A form that produced a definition would
//! produce a VM that never starts, and the console would have made a
//! promise the cluster cannot keep.
//!
//! **Import.** Bringing an existing qcow2 or raw disk in is the thing
//! that makes replacing a hypervisor a migration rather than a rebuild,
//! and it is genuinely not here yet: it needs a raw-media path in the
//! registry (stormblock-registry#5) so a disk image lands as an opaque
//! golden that is never unpacked. Until it does, a VM's root disk is a
//! golden that already exists, and the form says so rather than offering
//! a file picker that would fail.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use console_core::{Creator, Field};
use serde_json::{json, Value};

use crate::Inner;

const CREATE: &str = "/api/plugins/vm/create";
const APPLY: &str = "/api/plugins/k8s/apply";

pub fn creators() -> Vec<Creator> {
    vec![
        Creator::form(
            "vm:vm",
            "Virtual machine",
            CREATE,
            vec![
                Field::text("name", "Name").required(),
                Field::text("namespace", "Namespace").default("default"),
                Field::text("node", "Node")
                    .required()
                    .hint("the node to run on — nothing schedules VMs yet, so this is explicit"),
                Field::text("cores", "vCPU").default("2"),
                Field::text("memory", "Memory").default("4Gi"),
                Field::text("golden", "Root disk from golden")
                    .required()
                    .hint("a sealed golden to CoW-clone, e.g. rocky-10-cloud. Importing an existing disk image needs stormblock-registry#5"),
                Field::select("bus", "Disk bus", &["virtio", "nvme", "scsi", "sata"]),
                Field::text("ssh_key", "SSH public key")
                    .hint("goes into the cloud-init seed; a guest with no key and no password is a machine nothing can log into"),
            ],
        )
        .describe("A VM on this cluster, from a golden, on a named node")
        .at(&["#/vms"]),
        Creator::yaml("vm:yaml", "Virtual machine (YAML)", APPLY, VMI)
            .describe("A KubeVirt VirtualMachineInstance, as kubectl would apply it")
            .at(&["#/vms"]),
    ]
}

#[derive(serde::Deserialize)]
pub struct Form {
    name: String,
    #[serde(default)]
    namespace: String,
    #[serde(default)]
    node: String,
    #[serde(default)]
    cores: String,
    #[serde(default)]
    memory: String,
    #[serde(default)]
    golden: String,
    #[serde(default)]
    bus: String,
    #[serde(default)]
    ssh_key: String,
}

/// The form, as an object the apiserver takes. Everything the form does
/// not ask about is left out rather than defaulted invisibly — a field
/// silently accepted is how a VM boots with the wrong disk (stormvm
/// kube.md), and the same goes for one silently supplied.
pub fn instance(f: &Form) -> Result<Value, String> {
    if f.name.trim().is_empty() {
        return Err("a virtual machine needs a name".into());
    }
    if f.golden.trim().is_empty() {
        return Err("a root disk needs a golden to clone from".into());
    }
    if f.node.trim().is_empty() {
        return Err("nothing places VMs yet, so a node has to be named".into());
    }
    let cores: i64 = f.cores.trim().parse().unwrap_or(2);
    if cores < 1 {
        return Err("vCPU must be at least 1".into());
    }
    let memory = if f.memory.trim().is_empty() { "4Gi" } else { f.memory.trim() };
    let bus = if f.bus.trim().is_empty() { "virtio" } else { f.bus.trim() };
    let ns = if f.namespace.trim().is_empty() { "default" } else { f.namespace.trim() };
    let user_data = if f.ssh_key.trim().is_empty() {
        "#cloud-config\n".to_string()
    } else {
        format!("#cloud-config\nssh_authorized_keys:\n  - {}\n", f.ssh_key.trim())
    };
    Ok(json!({
        "apiVersion": "kubevirt.io/v1",
        "kind": "VirtualMachineInstance",
        "metadata": {"name": f.name.trim(), "namespace": ns},
        "spec": {
            "nodeName": f.node.trim(),
            "domain": {
                "cpu": {"cores": cores},
                "memory": {"guest": memory},
                "firmware": {"bootloader": {"efi": {"secureBoot": false}}},
                "devices": {
                    "disks": [
                        {"name": "root", "disk": {"bus": bus}},
                        {"name": "seed", "disk": {"bus": bus}}
                    ],
                    "interfaces": [{"name": "default"}]
                }
            },
            "networks": [{"name": "default", "pod": {}}],
            "volumes": [
                {"name": "root", "dataVolume": {"name": f.golden.trim()}},
                {"name": "seed", "cloudInitNoCloud": {"userData": user_data}}
            ]
        }
    }))
}

pub async fn create(State(inner): State<Arc<Inner>>, Json(form): Json<Form>) -> Response {
    let Some(client) = &inner.client else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "no apiserver"})))
            .into_response();
    };
    let doc = match instance(&form) {
        Ok(d) => d,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
    };
    let ns = doc.pointer("/metadata/namespace").and_then(Value::as_str).unwrap_or("default");
    let path = format!("{}/namespaces/{ns}/virtualmachineinstances", crate::VM_API);
    match client.post_json(&path, &doc).await {
        Ok((status, body)) if status.is_success() => {
            let name = doc.pointer("/metadata/name").and_then(Value::as_str).unwrap_or("");
            let _ = body;
            Json(json!({"message": format!("virtual machine {name} created")})).into_response()
        }
        Ok((status, body)) => {
            let msg = body
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("apiserver returned {}", status.as_u16()));
            (StatusCode::BAD_GATEWAY, Json(json!({"error": msg}))).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

const VMI: &str = "apiVersion: kubevirt.io/v1\nkind: VirtualMachineInstance\nmetadata:\n  name: web-1\n  namespace: default\nspec:\n  # Nothing schedules VMs yet, so the node is named here.\n  nodeName: CHANGE-ME\n  domain:\n    cpu:\n      cores: 2\n    memory:\n      guest: 4Gi\n    firmware:\n      bootloader:\n        efi:\n          secureBoot: false\n    devices:\n      disks:\n        - name: root\n          disk:\n            bus: virtio\n        - name: seed\n          disk:\n            bus: virtio\n      interfaces:\n        - name: default\n  networks:\n    - name: default\n      pod: {}\n  volumes:\n    - name: root\n      dataVolume:\n        name: rocky-10-cloud\n    - name: seed\n      cloudInitNoCloud:\n        userData: |\n          #cloud-config\n";

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> Form {
        Form {
            name: "web-1".into(),
            namespace: String::new(),
            node: "storm-1".into(),
            cores: "4".into(),
            memory: "8Gi".into(),
            golden: "rocky-10-cloud".into(),
            bus: String::new(),
            ssh_key: String::new(),
        }
    }

    #[test]
    fn the_form_builds_an_instance_the_apiserver_takes() {
        let v = instance(&form()).unwrap();
        assert_eq!(v["kind"], "VirtualMachineInstance");
        assert_eq!(v["apiVersion"], "kubevirt.io/v1");
        assert_eq!(v["metadata"]["namespace"], "default", "namespace defaults");
        assert_eq!(v["spec"]["nodeName"], "storm-1");
        assert_eq!(v["spec"]["domain"]["cpu"]["cores"], 4);
        assert_eq!(v["spec"]["domain"]["memory"]["guest"], "8Gi");
        assert_eq!(v["spec"]["domain"]["devices"]["disks"][0]["disk"]["bus"], "virtio");
        assert_eq!(v["spec"]["volumes"][0]["dataVolume"]["name"], "rocky-10-cloud");
    }

    #[test]
    fn an_ssh_key_reaches_the_seed_because_a_guest_without_one_is_unreachable() {
        let mut f = form();
        f.ssh_key = "ssh-ed25519 AAAA gw".into();
        let v = instance(&f).unwrap();
        let seed = v["spec"]["volumes"][1]["cloudInitNoCloud"]["userData"].as_str().unwrap();
        assert!(seed.contains("ssh_authorized_keys"), "{seed}");
        assert!(seed.contains("ssh-ed25519 AAAA gw"), "{seed}");
        // Without one the seed is still valid cloud-config, not empty.
        let plain = instance(&form()).unwrap();
        assert_eq!(plain["spec"]["volumes"][1]["cloudInitNoCloud"]["userData"], "#cloud-config\n");
    }

    #[test]
    fn what_cannot_be_guessed_is_refused_rather_than_defaulted() {
        let mut f = form();
        f.name = String::new();
        assert!(instance(&f).unwrap_err().contains("name"));
        let mut f = form();
        f.golden = String::new();
        assert!(instance(&f).unwrap_err().contains("golden"));
        let mut f = form();
        f.node = String::new();
        assert!(instance(&f).unwrap_err().contains("node"));
        let mut f = form();
        f.cores = "0".into();
        assert!(instance(&f).unwrap_err().contains("vCPU"));
    }

    #[test]
    fn the_yaml_template_is_one_valid_instance() {
        let docs = plugin_kubernetes::apply::parse_documents(VMI).unwrap();
        assert_eq!(docs.len(), 1);
        let (kind, name, path) = plugin_kubernetes::apply::target(&docs[0]).unwrap();
        assert_eq!(kind, "VirtualMachineInstance");
        assert_eq!(name, "web-1");
        assert_eq!(path, "/apis/kubevirt.io/v1/namespaces/default/virtualmachineinstances");
    }
}
