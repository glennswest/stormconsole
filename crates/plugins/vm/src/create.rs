//! Creating a VM.
//!
//! Two doors, the way OpenShift has two: a form for the ordinary case,
//! and YAML for everything else. The form builds a
//! `VirtualMachineInstance` — not a `VirtualMachine` — because nothing
//! turns a definition into an instance yet. A form that produced a
//! definition would produce a VM that never starts, and the console would
//! have made a promise the cluster cannot keep.
//!
//! **The node is optional now.** It was required, with the hint "nothing
//! schedules VMs yet, so this is explicit", and the YAML template shipped
//! `nodeName: CHANGE-ME` — which was true and is the workaround somebody
//! had to perform to get a VM at all. rustkube#72 gave the scheduler
//! VirtualMachineInstances, so a VM with no node is placed like anything
//! else.
//!
//! What has *not* changed is what `spec.nodeName` means: it is a **pin**,
//! and the scheduler leaves a VMI that carries one alone. So the field is
//! absent unless somebody asked for it — an empty string would pin the
//! machine to a node called `""` and it would never run, which is the same
//! shape of bug as `CHANGE-ME` and harder to see.
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
use console_core::{Creator, Field, Viewer};

use crate::images::{self, Catalogue};
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::Inner;

const CREATE: &str = "/api/plugins/vm/create";
const APPLY: &str = "/api/plugins/k8s/apply";

pub fn creators(catalogue: &Catalogue) -> Vec<Creator> {
    vec![
        Creator::form(
            "vm:vm",
            "Virtual machine",
            CREATE,
            vec![
                Field::text("name", "Name").required(),
                Field::text("namespace", "Namespace").default("default"),
                Field::text("node", "Node")
                    .hint("leave blank and the scheduler picks one; name a node to pin it there"),
                Field::text("cores", "vCPU").default("2"),
                Field::text("memory", "Memory").default("4Gi"),
                root_disk(catalogue),
                Field::select("bus", "Disk bus", &["virtio", "nvme", "scsi", "sata"]),
                Field::text("ssh_key", "SSH public key")
                    .hint("goes into the cloud-init seed; a guest with no key and no password is a machine nothing can log into"),
            ],
        )
        .describe("A VM on this cluster, from a golden")
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
    let cores: i64 = f.cores.trim().parse().unwrap_or(2);
    if cores < 1 {
        return Err("vCPU must be at least 1".into());
    }
    let memory = if f.memory.trim().is_empty() { "4Gi" } else { f.memory.trim() };
    let bus = if f.bus.trim().is_empty() { "virtio" } else { f.bus.trim() };
    let ns = if f.namespace.trim().is_empty() { "default" } else { f.namespace.trim() };
    let _ = cloud_init(f);
    let mut vmi = json!({
        "apiVersion": "kubevirt.io/v1",
        "kind": "VirtualMachineInstance",
        "metadata": {"name": f.name.trim(), "namespace": ns},
        "spec": {
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
                // The seed by reference, not inline.
                //
                // An SSH *public* key is not confidential — that is what
                // makes it a public key — but `userData` is the field that
                // grows passwords, and it travels in the VMI spec where
                // anyone with read on virtualmachineinstances can see it.
                // KubeVirt has `userDataSecretRef` for exactly this, so the
                // cloud-init payload goes in a Secret and the machine points
                // at it. The secret is namespaced with the VM and named after
                // it, so deleting the VM leaves one obvious thing behind
                // rather than an anonymous blob.
                {"name": "seed", "cloudInitNoCloud": {
                    "userDataSecretRef": {"name": format!("{}-cloudinit", f.name.trim())}
                }}
            ]
        }
    });
    // A node only when one was asked for.
    //
    // `spec.nodeName` is a **pin**, not a hint: the scheduler treats a VMI
    // that carries one as already placed and leaves it alone, which is right
    // for a machine somebody deliberately put somewhere and wrong for every
    // other machine. Writing an empty string would be writing a field the
    // user did not set, so the key is absent unless it means something.
    if !f.node.trim().is_empty() {
        vmi["spec"]["nodeName"] = json!(f.node.trim());
    }
    Ok(vmi)
}

/// The root-disk field: a list when the image operator has answered, free
/// text when it has not.
///
/// Free text was the old behaviour and it is the right fallback rather than
/// an empty dropdown — a console on a cluster with no image operator should
/// keep the box that worked, not show a control with nothing in it.
///
/// The list holds three kinds in one field, ordered by how soon the VM can
/// start: goldens already on the node, goldens the fleet has that would be
/// copied, and catalogue entries that must be downloaded and built. Each
/// label says which, because the difference is minutes.
/// The cloud-init payload for a machine, which goes into a Secret.
pub fn cloud_init(f: &Form) -> String {
    if f.ssh_key.trim().is_empty() {
        "#cloud-config\n".to_string()
    } else {
        format!("#cloud-config\nssh_authorized_keys:\n  - {}\n", f.ssh_key.trim())
    }
}

/// The Secret name a machine's seed lives under.
pub fn seed_secret_name(name: &str) -> String {
    format!("{}-cloudinit", name.trim())
}

fn root_disk(catalogue: &Catalogue) -> Field {
    if catalogue.choices.is_empty() {
        let hint = if catalogue.note.is_empty() {
            "a sealed golden to CoW-clone, e.g. rocky-10-cloud".to_string()
        } else {
            catalogue.note.clone()
        };
        return Field::text("golden", "Root disk").required().hint(&hint);
    }
    // The golden name is submitted; the sentence is only read.
    //
    // These were one string until a machine was created whose root disk was
    // named "alma 10 x86_64 — not goldened yet, will be built": the option
    // carried the label, and the server mapped it back to a value after the
    // fact. That mapping missed the moment the catalogue changed between
    // rendering the form and submitting it, and there is no mapping now.
    let options = catalogue
        .choices
        .iter()
        .map(|c| console_core::FieldOption::new(c.value.clone(), c.label.clone()))
        .collect();
    Field::choices("golden", "Root disk", options)
        .required()
        .hint("already on the node boots at once; anything else is built or copied first")
}

/// What the form submitted, with one piece of belt and braces.
///
/// The select now posts the golden name directly, so this is normally the
/// identity. It still maps a *label* back, because a form rendered by an
/// older console — or held open across an upgrade — posts what it was given,
/// and a machine created with a sentence for a disk name is a bad way to
/// find that out.
pub fn value_of(catalogue: &Catalogue, submitted: &str) -> String {
    if catalogue.choices.iter().any(|c| c.value == submitted) {
        return submitted.to_string();
    }
    catalogue
        .choices
        .iter()
        .find(|c| c.label == submitted)
        .map(|c| c.value.clone())
        .unwrap_or_else(|| submitted.to_string())
}

pub async fn create(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Json(form): Json<Form>,
) -> Response {
    // Every exit from here says what happened, and says it on the log group
    // rather than only into the dialog that asked.
    //
    // A create that fails in a modal is gone the moment it is dismissed:
    // there is nothing to go back to, nothing to show somebody else, and
    // nothing for anyone who was not the person who clicked. The dialog
    // still shows the reason — this is the copy that outlives it.
    let Some(client) = &inner.client else {
        warn!("vm create refused: no apiserver configured for this console");
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "no apiserver"})))
            .into_response();
    };
    // Resolve what the form showed back to what it means, and golden it if
    // it is not a golden yet.
    //
    // One field carries three kinds of answer — a local golden, a fleet
    // golden, a catalogue reference nobody has built — because they are the
    // same decision from the person's side: what should this machine boot.
    // Which of the three it was is the operator's problem, not theirs.
    let mut form = form;
    let catalogue = inner.images.get();
    form.golden = value_of(&catalogue, form.golden.trim());
    if images::is_reference(&form.golden) {
        match golden_from_reference(&inner, &form.golden, &form.node).await {
            Ok(name) => form.golden = name,
            Err(e) => {
                warn!(reference = %form.golden, error = %e, "vm create: could not golden the image");
                return (StatusCode::BAD_GATEWAY, Json(json!({"error": e}))).into_response()
            }
        }
    }

    // Make sure the node has the disk before creating a machine that clones
    // it.
    //
    // A golden in the registry is the fleet's; a node holds a *placement* of
    // it. Nothing created placements, so this form offered "will be copied to
    // the node" and then created a VM whose root disk did not exist —
    // `no volume fedora-43-x86_64`, from the kubelet, minutes later, with
    // nothing connecting it back to the choice made here.
    let mut placing = false;
    if let Some(base) = inner.image_operator.clone() {
        match images::ensure_local(&inner.http, &base, form.node.trim(), &form.golden).await {
            Ok(true) => {}
            Ok(false) => placing = !form.node.trim().is_empty(),
            Err(e) => {
                warn!(golden = %form.golden, node = %form.node, error = %e,
                      "vm create: could not place the root disk");
                return (StatusCode::BAD_GATEWAY, Json(json!({"error": e}))).into_response();
            }
        }
    }

    let doc = match instance(&form) {
        Ok(d) => d,
        Err(e) => {
            warn!(name = %form.name, error = %e, "vm create rejected");
            return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response();
        }
    };
    let ns = doc.pointer("/metadata/namespace").and_then(Value::as_str).unwrap_or("default");
    if let Some(refusal) = crate::refuse_hidden(&inner, &viewer, ns).await {
        return refusal;
    }
    // The seed first: a machine whose secret does not exist boots with no
    // cloud-init at all, which is a VM with no login and no obvious reason.
    //
    // `stringData` so the payload is written as text rather than base64 by
    // hand — the apiserver does the encoding, and a hand-encoded secret that
    // is wrong is unreadable in a way nobody debugs quickly.
    let secret = json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": {
            "name": seed_secret_name(&form.name),
            "namespace": ns,
            "labels": {"storm.io/vm": form.name.trim()},
        },
        "type": "Opaque",
        "stringData": {"userdata": cloud_init(&form)},
    });
    let secret_path = format!("/api/v1/namespaces/{ns}/secrets");
    match client.post_json_as(&secret_path, &secret, viewer.token.as_deref()).await {
        // 409 is fine: recreating a machine of the same name reuses its seed.
        Ok((status, _)) if status.is_success() || status.as_u16() == 409 => {}
        Ok((status, body)) => {
            let msg = body
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("apiserver returned {}", status.as_u16()));
            warn!(name = %form.name, error = %msg, "vm create: could not write the cloud-init secret");
            return (StatusCode::BAD_GATEWAY, Json(json!({"error": msg}))).into_response();
        }
        Err(e) => {
            warn!(name = %form.name, error = %e, "vm create: could not write the cloud-init secret");
            return (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()}))).into_response();
        }
    }

    let path = format!("{}/namespaces/{ns}/virtualmachineinstances", crate::VM_API);
    match client.post_json_as(&path, &doc, viewer.token.as_deref()).await {
        Ok((status, body)) if status.is_success() => {
            let name = doc.pointer("/metadata/name").and_then(Value::as_str).unwrap_or("");
            let _ = body;
            info!(name, namespace = ns, golden = %form.golden, placing, "virtual machine created");
            // Say that the disk is still arriving, rather than letting it
            // look created-and-broken for the minutes an import takes.
            let message = if placing {
                format!(
                    "virtual machine {name} created — its root disk is still being \
                     copied to {}, so it will start once that finishes",
                    form.node.trim()
                )
            } else {
                format!("virtual machine {name} created")
            };
            Json(json!({"message": message})).into_response()
        }
        Ok((status, body)) => {
            // The apiserver's own words when it has any. A bare status code
            // is the answer that sends somebody to read source, and the
            // message is usually the whole diagnosis — a missing CRD, a
            // field it will not take, a namespace that does not exist.
            let msg = body
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("apiserver returned {}", status.as_u16()));
            warn!(
                name = %form.name, namespace = ns, status = status.as_u16(), error = %msg,
                "vm create: apiserver refused it"
            );
            (StatusCode::BAD_GATEWAY, Json(json!({"error": msg}))).into_response()
        }
        Err(e) => {
            warn!(name = %form.name, error = %e, "vm create: could not reach the apiserver");
            (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()}))).into_response()
        }
    }
}

/// Ask the image operator to golden a catalogue reference, and say what the
/// VM should then boot from.
///
/// **This returns as soon as the object exists, not when the golden is
/// built.** The operator answers 202 and reconciles: a download, a decode
/// and a seal, which is minutes for a cloud image. The VM is created against
/// the name the golden will have, and starts when it is there — which is the
/// same shape as a pod scheduled before its image is pulled, and the reason
/// the form says "will be built" rather than pretending it is instant.
///
/// `localOn` is the node the VM is pinned to, when it is pinned. A VM the
/// scheduler will place has no node to name yet, so the image is goldened
/// fleet-wide and the copy follows wherever it lands.
async fn golden_from_reference(
    inner: &Arc<Inner>,
    reference: &str,
    node: &str,
) -> Result<String, String> {
    let Some(base) = inner.image_operator.clone() else {
        return Err(format!(
            "{reference} is a catalogue reference and there is no image operator to build it"
        ));
    };
    let mut body = json!({"reference": reference});
    if !node.trim().is_empty() {
        body["localOn"] = json!([node.trim()]);
    }
    let url = format!("{}/api/v1/images", base.trim_end_matches('/'));
    let r = inner
        .http
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("image operator at {base}: {e}"))?;
    let status = r.status();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let msg = v
            .get("error")
            .or_else(|| v.get("message"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("image operator returned {}", status.as_u16()));
        return Err(msg);
    }
    // `localName` is what a VM spec refers to; it is only there once the
    // object has been resolved, so fall back to the object's own name.
    let name = v
        .pointer("/status/localName")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| v.get("name").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    if name.is_empty() {
        return Err(format!("image operator accepted {reference} but named no golden"));
    }
    Ok(name)
}

const VMI: &str = "apiVersion: kubevirt.io/v1\nkind: VirtualMachineInstance\nmetadata:\n  name: web-1\n  namespace: default\nspec:\n  domain:\n    cpu:\n      cores: 2\n    memory:\n      guest: 4Gi\n    firmware:\n      bootloader:\n        efi:\n          secureBoot: false\n    devices:\n      disks:\n        - name: root\n          disk:\n            bus: virtio\n        - name: seed\n          disk:\n            bus: virtio\n      interfaces:\n        - name: default\n  networks:\n    - name: default\n      pod: {}\n  volumes:\n    - name: root\n      dataVolume:\n        name: rocky-10-cloud\n    - name: seed\n      cloudInitNoCloud:\n        userData: |\n          #cloud-config\n";

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
        let seed = cloud_init(&f);
        assert!(seed.contains("ssh_authorized_keys"), "{seed}");
        assert!(seed.contains("ssh-ed25519 AAAA gw"), "{seed}");
        // Without one the seed is still valid cloud-config, not empty.
        assert_eq!(cloud_init(&form()), "#cloud-config\n");
    }

    #[test]
    fn the_seed_is_referenced_not_inlined() {
        // A public key is not confidential, but userData is the field that
        // grows passwords and it travels in the VMI spec, readable by anyone
        // with get on virtualmachineinstances. KubeVirt has
        // userDataSecretRef; the payload belongs there.
        let mut f = form();
        f.ssh_key = "ssh-ed25519 AAAA gw".into();
        let v = instance(&f).unwrap();
        let seed = &v["spec"]["volumes"][1]["cloudInitNoCloud"];
        assert_eq!(seed["userDataSecretRef"]["name"], "web-1-cloudinit");
        assert!(seed["userData"].is_null(), "the payload must not be inline: {seed}");
        // And the key must not appear anywhere in the machine's spec.
        let whole = serde_json::to_string(&v).unwrap();
        assert!(!whole.contains("ssh-ed25519"), "the key leaked into the VMI: {whole}");
    }

    #[test]
    fn the_secret_is_named_after_the_machine() {
        // So deleting a VM leaves one obvious thing behind rather than an
        // anonymous blob nobody will ever identify.
        assert_eq!(seed_secret_name("web-1"), "web-1-cloudinit");
        assert_eq!(seed_secret_name("  spaced  "), "spaced-cloudinit");
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
        f.cores = "0".into();
        assert!(instance(&f).unwrap_err().contains("vCPU"));
    }

    #[test]
    fn no_node_means_the_scheduler_picks_one() {
        // It used to be required — "nothing places VMs yet, so a node has to
        // be named". Something does now (rustkube#72), and the field is a
        // pin rather than an obligation.
        let mut f = form();
        f.node = String::new();
        let v = instance(&f).expect("a VM with no node is a VM the scheduler places");
        assert!(
            v["spec"]["nodeName"].is_null(),
            "the key must be absent, not empty: the scheduler treats a VMI \
             carrying spec.nodeName as already placed, so an empty string \
             would pin the VM to a node called \"\" and it would never run"
        );
    }

    #[test]
    fn a_named_node_still_pins_the_machine() {
        let v = instance(&form()).unwrap();
        assert_eq!(v["spec"]["nodeName"], "storm-1");
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

    use crate::images::Choice;

    fn catalogue() -> Catalogue {
        Catalogue {
            choices: vec![
                Choice {
                    value: "fedora-43-x86_64".into(),
                    label: "fedora-43-x86_64 — on this node".into(),
                },
                Choice {
                    value: "rocky-10-x86_64".into(),
                    label: "rocky-10-x86_64 — goldened, will be copied to the node".into(),
                },
                Choice {
                    value: "debian:13".into(),
                    label: "debian 13 x86_64 — not goldened yet, will be built".into(),
                },
            ],
            note: String::new(),
        }
    }

    #[test]
    fn the_root_disk_is_a_list_when_the_operator_has_answered() {
        let f = root_disk(&catalogue());
        assert_eq!(f.kind, "select");
        assert_eq!(f.options.len(), 3);
        assert!(f.required);
    }

    #[test]
    fn the_root_disk_falls_back_to_free_text_without_an_operator() {
        // The old behaviour, kept: a console on a cluster with no image
        // operator should keep the box that worked rather than show an
        // empty dropdown.
        let f = root_disk(&Catalogue::default());
        assert_eq!(f.kind, "text");
        assert!(f.required);
        assert!(f.hint.contains("golden"));
    }

    #[test]
    fn an_empty_list_explains_itself() {
        let c = Catalogue { choices: vec![], note: "no answer from the image operator".into() };
        assert_eq!(root_disk(&c).hint, "no answer from the image operator");
    }

    #[test]
    fn a_label_maps_back_to_its_value() {
        let c = catalogue();
        assert_eq!(value_of(&c, "fedora-43-x86_64 — on this node"), "fedora-43-x86_64");
        assert_eq!(value_of(&c, "debian 13 x86_64 — not goldened yet, will be built"), "debian:13");
    }

    #[test]
    fn a_typed_golden_passes_through_unchanged() {
        // What keeps the free-text path working, and what stops a console
        // whose cache is empty from mangling a name somebody typed.
        assert_eq!(value_of(&Catalogue::default(), "rocky-10-cloud"), "rocky-10-cloud");
        assert_eq!(value_of(&catalogue(), "something-else"), "something-else");
    }

    #[test]
    fn only_a_reference_needs_goldening() {
        let c = catalogue();
        assert!(!images::is_reference(&value_of(&c, "fedora-43-x86_64 — on this node")));
        assert!(images::is_reference(&value_of(
            &c,
            "debian 13 x86_64 — not goldened yet, will be built"
        )));
    }
}
