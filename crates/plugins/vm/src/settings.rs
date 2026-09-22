//! What can be changed about a machine, and **when the change lands**.
//!
//! The design in this is not the form; it is the honesty (#14). Changing a
//! running VM has four different answers depending on the field, and a
//! settings page that appears to apply everything and quietly applies some
//! is worse than one that refuses:
//!
//! | field | while running |
//! |---|---|
//! | cores, memory | needs hotplug or a restart — say which, and do not pretend |
//! | disk bus | restart |
//! | network binding | restart, and it moves the guest's address |
//! | SSH key / cloud-init | next boot only — the seed is read once |
//!
//! So every field carries when it takes effect, and the answer depends on
//! what the machine actually is: on a *stopped* machine everything simply
//! applies, and saying "needs a restart" to somebody editing a machine that
//! is not running is a warning about nothing.
//!
//! The second half is **pending changes**. A `VirtualMachine` is the
//! definition and a `VirtualMachineInstance` is the machine that is
//! running, and nothing makes them agree: edit the definition of a running
//! machine and the two silently diverge, with the console showing the
//! definition and the guest running the old numbers. A machine that has
//! diverged says so here, field by field, rather than leaving somebody to
//! find out at the next reboot.

use console_core::{ComponentSummary, Health, Metric};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::components::{memory, vcpus};

/// When an edit to one field takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Applies {
    /// The machine is not running: there is nothing to apply it to yet, so
    /// it is simply what the machine will be.
    WhenItStarts,
    /// Takes effect immediately on the running machine.
    Now,
    /// Written now, in force after the machine is restarted.
    OnRestart,
    /// Written now, in force at the guest's next boot — and only then,
    /// because the thing that reads it is read once.
    NextBoot,
}

impl Applies {
    pub fn note(self) -> &'static str {
        match self {
            Applies::WhenItStarts => "applies when the machine starts",
            Applies::Now => "applies immediately",
            Applies::OnRestart => "written now, in force after a restart",
            Applies::NextBoot => "read by the guest at its next boot, and only then",
        }
    }
}

/// One editable field: what it is now, what changing it costs, and whether
/// the definition and the running machine already disagree about it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Setting {
    pub name: &'static str,
    pub label: &'static str,
    pub value: Value,
    /// What the machine is actually running, when that differs from the
    /// value above. `null` when they agree or nothing is running.
    pub running: Value,
    pub applies: Applies,
    pub note: String,
}

impl Setting {
    fn new(name: &'static str, label: &'static str, value: Value, applies: Applies) -> Self {
        Self { name, label, value, running: Value::Null, applies, note: applies.note().into() }
    }

    /// A note of its own, where "applies on restart" is true and not the
    /// interesting half of the answer.
    fn noting(mut self, note: &str) -> Self {
        self.note = format!("{} · {note}", self.applies.note());
        self
    }

    fn against(mut self, running: Option<Value>) -> Self {
        if let Some(r) = running {
            if r != self.value && !r.is_null() {
                self.running = r;
            }
        }
        self
    }
}

/// Every setting for one machine.
///
/// `machine` is the `VirtualMachine`, `instance` the running
/// `VirtualMachineInstance`; either may be absent. A machine with no
/// definition cannot be edited at all — there is nothing durable to write
/// to, and patching a VMI's spec is a change the hypervisor will never
/// read.
pub fn of(machine: Option<&Value>, instance: Option<&Value>) -> Settings {
    let defined = machine.is_some();
    let live = instance.is_some();
    let spec = machine
        .and_then(|v| v.pointer("/spec/template/spec").cloned())
        .or_else(|| instance.and_then(|v| v.get("spec").cloned()))
        .unwrap_or(Value::Null);
    let domain = spec.get("domain").cloned().unwrap_or(Value::Null);
    let run_spec = instance.and_then(|v| v.get("spec").cloned()).unwrap_or(Value::Null);
    let run_domain = run_spec.get("domain").cloned().unwrap_or(Value::Null);
    // Annotations live on the *template* of a definition and on the object
    // itself for a running instance: the instance is what the template
    // became, so the same keys are in two places depending which you hold.
    let annotations = machine
        .and_then(|v| v.pointer("/spec/template/metadata/annotations").cloned())
        .or_else(|| instance.and_then(|v| v.pointer("/metadata/annotations").cloned()))
        .unwrap_or(Value::Null);
    let run_annotations = instance
        .and_then(|v| v.pointer("/metadata/annotations").cloned())
        .unwrap_or(Value::Null);

    // On a stopped machine every edit is simply what it will be. Telling
    // somebody editing a machine that is not running that a field "needs a
    // restart" is a warning about nothing, and warnings about nothing are
    // how real ones stop being read.
    let when = |running: Applies| if live { running } else { Applies::WhenItStarts };

    let fields = vec![
        Setting::new(
            "cores",
            "vCPU",
            vcpus(&domain).map(Value::from).unwrap_or(Value::Null),
            // KubeVirt has CPU hotplug behind a feature gate and nothing on
            // this platform turns it on, so the honest answer is the one
            // that is true here rather than the one that is true upstream.
            when(Applies::OnRestart),
        )
        .against(vcpus(&run_domain).map(Value::from)),
        Setting::new(
            "memory",
            "Memory",
            memory(&domain).map(Value::from).unwrap_or(Value::Null),
            when(Applies::OnRestart),
        )
        .against(memory(&run_domain).map(Value::from)),
        // The floor a balloon may deflate to, which is the only mechanism
        // by which a machine's memory ever changes without a restart.
        //
        // KubeVirt's `memory.guest` with a *lower* resource request is
        // exactly ballooning, and stormvm reads it that way — a floor
        // below the size makes it build a `virtio-balloon-pci` (qemu) or
        // pass `--balloon` (cloud-hypervisor). No floor, no balloon, and
        // then memory cannot be changed at all until the machine
        // restarts. The console ignored the field entirely, so the one
        // decision that governs whether memory is adjustable was
        // invisible and unreachable.
        //
        // Adding or moving the floor still needs a restart, because the
        // device is built at start. What it buys is everything after that.
        Setting::new(
            "memory_floor",
            "Memory floor",
            Value::from(floor(&domain)),
            when(Applies::OnRestart),
        )
        .against(Some(Value::from(floor(&run_domain))))
        .noting(if floor(&domain).is_empty() {
            "no floor, so this machine has no balloon and its memory cannot change while it \
             runs. A floor below the size gives it one."
        } else {
            "the machine has a balloon: its memory may be squeezed to this floor without a \
             restart, once something asks it to"
        }),
        Setting::new("bus", "Disk bus", Value::from(bus(&domain)), when(Applies::OnRestart))
            .against(Some(Value::from(bus(&run_domain)))),
        Setting::new(
            "network",
            "Network",
            Value::from(network(&spec)),
            when(Applies::OnRestart),
        )
        .against(Some(Value::from(network(&run_spec)))),
        Setting::new("hostname", "Hostname", Value::from(hostname(&spec)), when(Applies::NextBoot))
            .against(Some(Value::from(hostname(&run_spec)))),
        Setting::new("ssh_key", "SSH key", Value::from(ssh_key(&spec)), when(Applies::NextBoot)),
        // The screen.
        //
        // This was not here at all, so the answer to "why can I not edit the
        // display" was that no field existed — on a console whose whole
        // purpose for a VM that will not boot is to let somebody look at it.
        //
        // Blank means no adapter; setting one turns the screen on, which also
        // moves the machine from cloud-hypervisor to qemu, because a
        // framebuffer is the thing cloud-hypervisor does not have.
        Setting::new("display", "Display", Value::from(display(&spec, &annotations)), when(Applies::OnRestart))
            .against(Some(Value::from(display(&run_spec, &run_annotations))))
            .noting(
                "blank for no screen. A screen makes this machine run under qemu rather than \
                 cloud-hypervisor, which is the only hypervisor here with a framebuffer",
            ),
        Setting::new("vga_memory", "Display memory", Value::from(vga_memory(&annotations)), when(Applies::OnRestart))
            .against(Some(Value::from(vga_memory(&run_annotations))))
            .noting(
                "MiB, and only the VGA-family adapters have it — virtio sizes itself from what \
                 the guest asks to draw",
            ),
    ];

    let pending: Vec<&'static str> =
        fields.iter().filter(|f| !f.running.is_null()).map(|f| f.name).collect();

    Settings {
        editable: defined,
        // Why not, in the words of the thing that is missing. An instance
        // applied on its own has nothing durable behind it: a patch to a
        // VMI's spec is read by nothing, and the machine would go back to
        // what it was the moment it restarted.
        why: if defined {
            String::new()
        } else if live {
            "this machine is a VirtualMachineInstance applied on its own, so there is nothing \
             durable to write to — a change to a running instance is read by nothing and is lost \
             when it stops"
                .into()
        } else {
            "no such machine".into()
        },
        running: live,
        pending: pending.iter().map(|s| s.to_string()).collect(),
        fields,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Settings {
    pub editable: bool,
    pub why: String,
    pub running: bool,
    /// Fields the definition and the running machine already disagree
    /// about — changes written but not yet in force.
    pub pending: Vec<String>,
    pub fields: Vec<Setting>,
}

/// The display adapter a machine asks for, from its template annotations.
///
/// Empty means no screen: `autoattachGraphicsDevice` false, or simply never
/// asked for. That is a real answer and is offered as one, because turning a
/// screen on afterwards is the common case — somebody made a VM, it will not
/// boot, and they want to see why.
fn display(spec: &Value, annotations: &Value) -> String {
    let on = spec
        .pointer("/domain/devices/autoattachGraphicsDevice")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !on {
        return String::new();
    }
    annotations
        .get("storm.io/vga")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("virtio")
        .to_string()
}

/// How much framebuffer memory, for the adapters that have any.
fn vga_memory(annotations: &Value) -> String {
    annotations
        .get("storm.io/vga-memory")
        .and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_i64().map(|n| n.to_string())))
        .unwrap_or_default()
}

/// The balloon floor: a memory *request* lower than the guest size.
///
/// Equal or absent is not a floor — a request that matches the size gives
/// a balloon nothing to deflate into, and reporting it as one would
/// promise adjustable memory that is not.
fn floor(domain: &Value) -> String {
    let req = domain
        .pointer("/resources/requests/memory")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let size = memory(domain).unwrap_or_default();
    if req.is_empty() || req == size {
        return String::new();
    }
    req.to_string()
}

/// The first disk's bus, which is the one a form can honestly offer: a
/// machine whose disks differ is described by its YAML, not by a dropdown.
fn bus(domain: &Value) -> String {
    domain
        .pointer("/devices/disks/0/disk/bus")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Which network the guest is on: the bridge it was pinned to, or the pod
/// network. `storm.io/bridge` is the override the create form writes.
fn network(spec: &Value) -> String {
    spec.pointer("/networks/0/storm.io/bridge")
        .or_else(|| spec.pointer("/domain/devices/interfaces/0/storm.io~1bridge"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if spec.pointer("/networks/0").is_some() { "pod".into() } else { String::new() }
        })
}

fn hostname(spec: &Value) -> String {
    spec.get("hostname").and_then(Value::as_str).unwrap_or_default().to_string()
}

/// The key is in the seed, and the seed is cloud-config. Reported as
/// present or not rather than echoed: it is not a secret, but a form that
/// round-trips a whole cloud-config through a text box is a form that
/// loses everything else in it.
fn ssh_key(spec: &Value) -> String {
    let seed = spec
        .pointer("/volumes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|v| v.pointer("/cloudInitNoCloud/userData").and_then(Value::as_str))
        .unwrap_or_default();
    seed.lines()
        .find(|l| l.trim_start().starts_with("- ssh-"))
        .map(|l| l.trim_start().trim_start_matches("- ").to_string())
        .unwrap_or_default()
}

/// The patch one edit makes to the `VirtualMachine`.
///
/// A merge patch against `spec.template.spec`, so everything not named is
/// left exactly as it was — which matters because this form knows about
/// six fields and a VM spec has dozens.
pub fn patch(field: &str, value: &str) -> Result<Value, String> {
    let template = |inner: Value| json!({"spec": {"template": {"spec": inner}}});
    match field {
        "cores" => {
            let n: i64 = value.trim().parse().map_err(|_| {
                format!("{value:?} is not a number of vCPU — a machine cannot have a fraction of one")
            })?;
            if n < 1 {
                return Err("a machine needs at least one vCPU".into());
            }
            Ok(template(json!({"domain": {"cpu": {"cores": n}}})))
        }
        "memory" => {
            let m = value.trim();
            if m.is_empty() {
                return Err("memory cannot be blank".into());
            }
            // Kubernetes quantities, as somebody types them. Validated
            // loosely on purpose: the apiserver is the authority and its
            // refusal names the field, where a regex here would reject
            // something valid the day the quantity grammar grows.
            if !m.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                return Err(format!("{m:?} is not a memory quantity — try 4Gi"));
            }
            Ok(template(json!({"domain": {"memory": {"guest": m}}})))
        }
        "bus" => {
            if !["virtio", "sata", "scsi", "usb"].contains(&value) {
                return Err(format!("{value:?} is not a disk bus this console offers"));
            }
            Ok(template(json!({"domain": {"devices": {"disks": [{"disk": {"bus": value}}]}}})))
        }
        "memory_floor" => {
            let m = value.trim();
            if m.is_empty() {
                // Removing the floor removes the balloon at the next
                // restart, which is a real choice: a balloon returns
                // memory by taking it away from a guest that thought it
                // had it, and that is wrong for anything latency-
                // sensitive.
                return Ok(template(json!({"domain": {"resources": {"requests":
                    {"memory": Value::Null}}}})));
            }
            if !m.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                return Err(format!("{m:?} is not a memory quantity — try 2Gi"));
            }
            Ok(template(json!({"domain": {"resources": {"requests": {"memory": m}}}})))
        }
        "hostname" => Ok(template(json!({"hostname": value.trim()}))),
        // The display is two edits: the annotation naming the adapter, and
        // the device flag that decides whether there is a screen at all.
        // Writing only the annotation leaves a model named on a machine with
        // no graphics device, which is a setting that appears to be saved and
        // does nothing.
        "display" => {
            let want = value.trim();
            let on = !want.is_empty();
            if on && !matches!(want, "virtio" | "vga" | "std" | "qxl") {
                return Err(format!(
                    "{want:?} is not a display this platform has — virtio, vga or qxl"
                ));
            }
            Ok(json!({"spec": {"template": {
                "metadata": {"annotations": {
                    "storm.io/vga": if on { json!(want) } else { Value::Null }
                }},
                "spec": {"domain": {"devices": {"autoattachGraphicsDevice": on}}}
            }}}))
        }
        "vga_memory" => {
            let v = value.trim();
            if v.is_empty() {
                return Ok(json!({"spec": {"template": {"metadata": {"annotations": {
                    "storm.io/vga-memory": Value::Null
                }}}}}));
            }
            let mb: i64 = v
                .parse()
                .map_err(|_| format!("{v:?} is not a number of MiB"))?;
            if mb < 1 {
                return Err("display memory must be at least 1 MiB".into());
            }
            Ok(json!({"spec": {"template": {"metadata": {"annotations": {
                "storm.io/vga-memory": mb.to_string()
            }}}}}))
        }
        // Deliberately not patchable here. Changing the binding moves the
        // guest's address, and doing that through a one-field form gives
        // no way to say what the new address will be — which is the whole
        // question somebody asks immediately afterwards. The YAML tab
        // takes it, with the whole spec visible.
        "network" => Err("the network binding is changed in the YAML, not here: it moves the \
                          guest's address, and this form has no way to tell you what the new \
                          one will be"
            .into()),
        "ssh_key" => Err("the SSH key lives in the cloud-init seed, which the guest reads once \
                          at first boot. Changing it here would change nothing on a machine that \
                          has already booted; add the key to the guest, or rebuild the machine"
            .into()),
        other => Err(format!("{other:?} is not a setting")),
    }
}

/// A machine whose definition and running instance disagree says so on its
/// row, not only on its page.
pub fn pending_metric(c: &mut ComponentSummary, s: &Settings) {
    if s.pending.is_empty() {
        return;
    }
    c.metrics.push(Metric::new("pending", s.pending.join(", ")).tone("warn"));
    if c.health == Health::Ok {
        c.health = Health::Warn;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine(cores: i64, mem: &str) -> Value {
        json!({"spec": {"template": {"spec": {
            "hostname": "web-1",
            "domain": {"cpu": {"cores": cores}, "memory": {"guest": mem},
                       "devices": {"disks": [{"name": "root", "disk": {"bus": "virtio"}}]}},
            "networks": [{"name": "default", "pod": {}}]
        }}}})
    }

    fn instance(cores: i64, mem: &str) -> Value {
        json!({"spec": {
            "hostname": "web-1",
            "domain": {"cpu": {"cores": cores}, "memory": {"guest": mem},
                       "devices": {"disks": [{"name": "root", "disk": {"bus": "virtio"}}]}},
            "networks": [{"name": "default", "pod": {}}]
        }})
    }

    fn field<'a>(s: &'a Settings, name: &str) -> &'a Setting {
        s.fields.iter().find(|f| f.name == name).unwrap()
    }

    /// The whole point of the issue: a field says when it lands, and the
    /// answer depends on whether the machine is running.
    #[test]
    fn a_stopped_machine_is_not_warned_about_restarts_it_does_not_need() {
        let m = machine(2, "4Gi");
        let stopped = of(Some(&m), None);
        assert_eq!(field(&stopped, "cores").applies, Applies::WhenItStarts);
        assert_eq!(field(&stopped, "ssh_key").applies, Applies::WhenItStarts);

        let i = instance(2, "4Gi");
        let live = of(Some(&m), Some(&i));
        assert_eq!(field(&live, "cores").applies, Applies::OnRestart);
        assert_eq!(field(&live, "hostname").applies, Applies::NextBoot);
    }

    /// A definition edited while the machine runs diverges from it
    /// silently. It does not any more.
    #[test]
    fn a_machine_that_has_diverged_says_which_fields() {
        let m = machine(8, "16Gi");
        let i = instance(2, "4Gi");
        let s = of(Some(&m), Some(&i));
        assert_eq!(s.pending, vec!["cores", "memory"]);
        assert_eq!(field(&s, "cores").value, json!(8), "the form shows what was asked for");
        assert_eq!(field(&s, "cores").running, json!(2), "and what is actually running");
        // Fields that agree say nothing, rather than repeating themselves.
        assert!(field(&s, "bus").running.is_null());

        let s = of(Some(&m), Some(&instance(8, "16Gi")));
        assert!(s.pending.is_empty());
    }

    /// An instance applied on its own has nothing durable to write to.
    #[test]
    fn an_instance_with_no_definition_is_not_editable_and_says_why() {
        let i = instance(2, "4Gi");
        let s = of(None, Some(&i));
        assert!(!s.editable);
        assert!(s.why.contains("lost when it stops"), "{}", s.why);
    }

    /// The floor is the only mechanism by which memory ever changes
    /// without a restart, and the console could neither see nor set it.
    #[test]
    fn the_memory_floor_says_whether_this_machine_has_a_balloon() {
        let none = machine(2, "4Gi");
        let s = of(Some(&none), None);
        let f = field(&s, "memory_floor");
        assert_eq!(f.value, json!(""));
        assert!(f.note.contains("no balloon"), "{}", f.note);

        let mut ballooned = machine(2, "4Gi");
        ballooned["spec"]["template"]["spec"]["domain"]["resources"] =
            json!({"requests": {"memory": "1Gi"}});
        let s = of(Some(&ballooned), None);
        let f = field(&s, "memory_floor");
        assert_eq!(f.value, json!("1Gi"));
        assert!(f.note.contains("squeezed to this floor"), "{}", f.note);

        // A request equal to the size is not a floor: a balloon with
        // nothing to deflate into is not adjustable memory.
        let mut equal = machine(2, "4Gi");
        equal["spec"]["template"]["spec"]["domain"]["resources"] =
            json!({"requests": {"memory": "4Gi"}});
        assert_eq!(field(&of(Some(&equal), None), "memory_floor").value, json!(""));
    }

    #[test]
    fn the_floor_is_written_and_can_be_taken_away() {
        let p = patch("memory_floor", "2Gi").unwrap();
        assert_eq!(
            p.pointer("/spec/template/spec/domain/resources/requests/memory"),
            Some(&json!("2Gi"))
        );
        // Blank removes it, which removes the balloon at the next restart
        // — a real choice, not a no-op.
        let p = patch("memory_floor", "").unwrap();
        assert_eq!(
            p.pointer("/spec/template/spec/domain/resources/requests/memory"),
            Some(&Value::Null)
        );
        assert!(patch("memory_floor", "lots").unwrap_err().contains("2Gi"));
    }

    #[test]
    fn a_patch_touches_only_the_field_it_names() {
        let p = patch("cores", "4").unwrap();
        assert_eq!(p.pointer("/spec/template/spec/domain/cpu/cores"), Some(&json!(4)));
        assert!(p.pointer("/spec/template/spec/domain/memory").is_none());
        assert_eq!(
            patch("memory", "8Gi").unwrap().pointer("/spec/template/spec/domain/memory/guest"),
            Some(&json!("8Gi"))
        );
    }

    #[test]
    fn a_refusal_says_what_to_do_instead() {
        assert!(patch("cores", "half").unwrap_err().contains("fraction"));
        assert!(patch("cores", "0").unwrap_err().contains("at least one"));
        assert!(patch("memory", "lots").unwrap_err().contains("4Gi"));
        assert!(patch("bus", "ide").unwrap_err().contains("not a disk bus"));
        // The two that are refused on purpose point somewhere that works.
        assert!(patch("network", "br0").unwrap_err().contains("YAML"));
        assert!(patch("ssh_key", "ssh-ed25519 x").unwrap_err().contains("reads once"));
    }
}
