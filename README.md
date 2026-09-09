# stormconsole

**The StormCOS console** — patterned on the OpenShift console, built the
storm way: a single static Rust binary under [stormd](https://github.com/glennswest/stormd),
rendering everything through the [stormview](https://github.com/glennswest/stormview)
contract, with a pluggable architecture where every domain contributes its
own part.

- **kubernetes** — namespaces, workloads, nodes, events, network policies
  via [rustkube](https://github.com/glennswest/rustkube) (kube-wire-compatible
  apiserver, watch-backed cache); **Cilium** endpoints, nodes, identities
  and policies through its CRDs, under one Cilium card that also carries
  the agent's own verdict on this node
- **vm** — virtual machines as KubeVirt `VirtualMachine` /
  `VirtualMachineInstance` objects, which is where
  [stormvm](https://github.com/glennswest/stormvm) puts them: the kubelet
  is the loop, so the console watches the CRDs. Lifecycle, disks, network,
  and both console doors — serial and framebuffer
- **logs** — the fleet log collector: stormcast multicast
  (`239.255.42.1:5514`) → a deduplicating redb ring → query API + live
  follow. Repeats collapse into one entry with a count, and entries expire
  on both an age and a size bound
- **fleet** — nodes discovered by their own announcements; drill into each
  node's stormd/stormdrive/stormblock; join, promote, demote, drain
- **stormdrive** — physical drives fleet-wide: SMART, wear, thermal,
  locate, lifecycle
- **stormblock** — volumes, exports, slabs, arrays from the block engine
- **sbregistry** — goldens, clones, pallets, warm-up

Not part of this project: mkube. The orchestrator side is rustkube and
rustkube-node only.

The console both consumes and produces the stormview component feed: it
aggregates every plugin's components at `/api/v1/components` (+
`/ws/components`), so any stormview renderer can show the whole cluster.

## Status

v0.8.0 — a working console on a StormCOS node with no config: rustkube
(with Cilium), virtual machines, fleet nodes and services, fleet logs,
stormdrive, stormstorage, stormblock, sbregistry, OpenShift-style create
and edit, the namespace dimension, and per-viewer authorization. Design in
[docs/architecture.md](docs/architecture.md); work plan in
[CLAUDE.md](CLAUDE.md).

## The namespace is a dimension

A namespace is the unit of ownership, quota and policy, so it is a
persistent control in the masthead rather than a menu item — the
OpenShift project selector. It scopes every namespaced view, says nothing
about the cluster-scoped ones (which say so on their own pages), and
**travels in the URL** as `?ns=`, so a link somebody pastes shows what
they were looking at. Which kinds it applies to comes from the plugin
that owns them (`GET /api/plugins/k8s/kinds`), not from a list kept in
the SPA.

A namespace also has a page of its own — `#/k8s/ns/<name>` — with an
inventory whose **every count is a link** into that kind filtered to this
namespace, its quota as used-against-hard, its limit ranges, its events
and its YAML. "No quota" is shown as the answer it is: nothing here is
bounded.

## Who sees what

The console aggregates kubernetes, fleet, logs, drives, volumes and the
registry into one feed, which makes it the broadest read surface on the
platform. So a viewer sees what they may see, and the decision is not the
console's:

- a user configured with `kube_token` has a kubernetes identity; the
  console asks **rustkube, as them** which namespaces they may see (list,
  falling back to a per-namespace probe, since rustkube serves no
  `SelfSubjectAccessReview` — [rustkube#59](https://github.com/glennswest/rustkube/issues/59))
- the feed is filtered **before it leaves the process**, so a hidden
  object is unreachable by REST, by websocket and by following a
  relation; a plugin route answers for it exactly as it would for one
  that does not exist
- every **write** carries the viewer's own bearer, so the apiserver's
  RBAC decides — a viewer whose Role has no `delete` verb is refused by
  the apiserver, not by the console's guess about them
- with no identity configured nothing is enforced, and
  `GET /api/v1/console/access` says so (`identified: false`) rather than
  implying a check that is not happening
- what is withheld is counted and named — "4 namespaces you cannot view"
  beside the selector — because a short list with no explanation reads as
  a broken console

## Virtual machines

A VM here is a KubeVirt object in the apiserver that rustkube-node's
kubelet reconciles (stormvm `docs/kube.md`: *"stormvm is libraries, the
kubelet is the loop"*), so the plugin watches `kubevirt.io/v1` the way the
Cilium view watches `cilium.io/v2`. Both objects are shown, because they
answer different questions: a `VirtualMachine` is what should exist and
whether it should run, a `VirtualMachineInstance` is the machine that is
running — its node, its phase, its disks, and the reason it did not start
when it did not.

Lifecycle is `spec.running` and nothing else. A definition that wants to
run and has no instance says exactly that: nothing places one yet, which
is stormvm's own outstanding work, not a fault here.

The two console doors are websockets relayed through the console's own
origin and addressed **by VM rather than by node**, so the browser never
learns a node address and the URL survives a live migration. Neither
upstream serves them yet — stormvm's console service is unbuilt, and the
other route to a guest's serial (the pod log the kubelet already writes)
needs [rustkube#55](https://github.com/glennswest/rustkube/issues/55) and
[rustkube-node#34](https://github.com/glennswest/rustkube-node/issues/34)
— so the page names the missing upstream instead of showing a terminal
that will never print. The framebuffer is
[noVNC](https://github.com/novnc/noVNC) (MPL-2.0), lazily loaded in its
own chunk so it is not in the console's first paint.

## Hardware, and storage

They are different sections because they are different things. A **drive**
is a physical object with a serial, a shelf and a bay that somebody
eventually walks up to and pulls; a **volume** is an allocation on top of
one. `#/drives` groups by shelf and orders by bay, with stormdrive's own
operations — locate, join fleet, designate, format — on the rows and the
shelf's on the group. `#/drives?group=shelf` is the other question: which
enclosure is in trouble, since a shelf fails as a unit.

## Build

Build on `root@dev.g8.lo`, never on a Mac:

```bash
cargo build --release --target x86_64-unknown-linux-musl
cd web && npm install && npm run build   # SPA, embedded at cargo build
```

## Run

```bash
stormconsole --config /etc/stormconsole/config.toml
# UI + API on :9094
```

## Configuration

One TOML file, every section optional; no file at all runs on defaults
(`config/config.toml` documents them). Two shapes are accepted:

```toml
# The console's own, sectioned shape
[api]
bind = "0.0.0.0:9094"
[logs]
db_path = "/var/lib/stormconsole/logs.redb"

# The flat StormCOS node-service shape — what a golden writes, the same
# two keys stormdrive and stormstorage take
listen_addr = "0.0.0.0:9094"
data_dir    = "/var/lib/stormconsole"
```

`listen_addr` is `[api] bind` and wins over it; the log ring lives at
`<data_dir>/logs.redb` (default `/var/lib/stormconsole`) unless `[logs]
db_path` says otherwise. Unknown keys are errors.

### The log ring

`[logs]` takes three more keys, all optional:

| key | default | what it does |
|---|---|---|
| `dedup` | `true` | Collapse repeats of the same host/app/severity/message into one entry carrying a count |
| `ring_cap` | `200000` | Most distinct entries kept; the oldest are dropped first |
| `retain_hours` | `168` | Drop an entry this long after it was **last** seen; `0` leaves `ring_cap` as the only bound |

Both bounds are swept on a timer as well as on insert, so a fleet that
goes quiet still expires what it left behind. A repeat refreshes an
entry's last-seen time, so a line that keeps arriving keeps its place.

Upgrading from ≤0.6: the ring changed format, so it changed filename. The
old `logs.db` is a SQLite file that nothing reads any more and can be
deleted.

**Every upstream defaults to this node's own daemon**, so a StormCOS node
lights up with the two-line config above and nothing else:

| plugin | default | override |
|---|---|---|
| kubernetes | `https://127.0.0.1:6443`, TLS unverified (stormcert self-signed, no CA in the golden; sno is anonymous-admin) | `[kubernetes] server`, `token`, `insecure_skip_tls_verify` — a configured server is verified unless told otherwise |
| stormblock | `http://127.0.0.1:9090` | `[stormblock] url` |
| sbregistry | `http://127.0.0.1:5100` | `[sbregistry] url` |
| stormdrive | `http://127.0.0.1:9092` (its stormview feed) | `[stormdrive] url` |
| stormstorage | `http://127.0.0.1:9093` (its stormview feed) | `[stormstorage] url` |
| vm | the apiserver above for the objects; `http://127.0.0.1:9095` for the console doors only | `[vm] url` |
| fleet | stormd instances probed on `127.0.0.1` ports 9080–9089 and 9180–9199 (the StormCOS layout) | `[fleet] stormd_host`, `stormd_ports` |

Any plugin can be turned off with `enabled = false`. To run the console
somewhere else and look at one node, point every `url`/`server` and
`stormd_host` at that node — that is how it is verified from dev.

## Creating things

Every list view has a **+ Create**, and the top bar has one that lists
everything — OpenShift's pattern. What each does is declared by the plugin
that owns the resource (`GET /api/v1/console/creators`): a YAML editor
seeded with a template, or a small form, posting to a plugin path. The SPA
knows nothing about pods or volumes.

- **kubernetes** — *Import YAML* (any documents, `---`-separated, like
  `oc apply -f`) and a template per kind (Pod, Deployment, StatefulSet,
  DaemonSet, Job, CronJob, Service, PVC, Namespace, NetworkPolicy,
  CiliumNetworkPolicy, CiliumClusterwideNetworkPolicy). `POST
  /api/plugins/k8s/apply` turns each document into JSON and creates it in
  the collection its `apiVersion`/`kind`/`namespace` name; every document
  gets a line in the result, a failure on one does not stop the rest, and a
  conflict is reported as the apiserver phrased it.
- **stormblock** — Volume (name, size, and one of array id / redundancy
  policy / template) and Export (volume, NVMe/TCP or iSCSI), through the
  proxy to the engine's own API.
- **sbregistry** — Golden (repository + tag/digest) and Clone (of a
  golden), likewise.
- **vm** — a Virtual machine form (name, namespace, node, vCPU, memory, a
  golden for the root disk, bus, SSH key) and a `VirtualMachineInstance`
  YAML template. The form builds an *instance*, not a definition, because
  nothing turns a definition into one yet — a form producing a definition
  would produce a VM that never starts. Importing an existing qcow2 or raw
  disk needs a raw-media path in the registry
  ([stormblock-registry#5](https://github.com/glennswest/stormblock-registry/issues/5))
  and is not pretended at.

Editing exists too: any cached object's YAML tab saves back with `PUT
/api/plugins/k8s/object/{kind}/{key}`. The write is a replace, so the
`resourceVersion` it was loaded with is the concurrency guard — an edit of
something changed since comes back 409 rather than quietly overwriting
somebody. A rename is refused rather than performed, because saving under
a new name would create a second object and leave the first.

Actions on cards — delete a volume, restart a stormd process, locate a
drive — are carried by the feed and invoked with the method the feed
declares, through the owning plugin's proxy.

When the console cannot start it prints exactly one line on stderr —
`stormconsole: fatal: config /etc/stormconsole/stormconsole.toml: line 2:
unknown field `port` …` — and exits **78** (`EX_CONFIG`) for a config it
cannot run on, **1** for a port it cannot bind. A supervisor reading the
code can tell the one a restart will not fix from the one it might.
