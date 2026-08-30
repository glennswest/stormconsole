# stormconsole

**The StormCOS console** — patterned on the OpenShift console, built the
storm way: a single static Rust binary under [stormd](https://github.com/glennswest/stormd),
rendering everything through the [stormview](https://github.com/glennswest/stormview)
contract, with a pluggable architecture where every domain contributes its
own part.

- **kubernetes** — namespaces, workloads, nodes, events, network policies
  via [rustkube](https://github.com/glennswest/rustkube) (kube-wire-compatible
  apiserver, watch-backed cache); **Cilium** endpoints, nodes, identities
  and policies through its CRDs, under one Cilium card
- **logs** — the fleet log collector: stormcast multicast
  (`239.255.42.1:5514`) → SQLite ring → query API + live follow
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

v0.4.0 — a working console on a StormCOS node with no config: rustkube
(with Cilium), fleet nodes and services, fleet logs, stormdrive,
stormstorage, stormblock, sbregistry, and OpenShift-style create. Design in
[docs/architecture.md](docs/architecture.md); work plan in
[CLAUDE.md](CLAUDE.md).

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
db_path = "/var/lib/stormconsole/logs.db"

# The flat StormCOS node-service shape — what a golden writes, the same
# two keys stormdrive and stormstorage take
listen_addr = "0.0.0.0:9094"
data_dir    = "/var/lib/stormconsole"
```

`listen_addr` is `[api] bind` and wins over it; the log ring lives at
`<data_dir>/logs.db` (default `/var/lib/stormconsole`) unless `[logs]
db_path` says otherwise. Unknown keys are errors.

**Every upstream defaults to this node's own daemon**, so a StormCOS node
lights up with the two-line config above and nothing else:

| plugin | default | override |
|---|---|---|
| kubernetes | `https://127.0.0.1:6443`, TLS unverified (stormcert self-signed, no CA in the golden; sno is anonymous-admin) | `[kubernetes] server`, `token`, `insecure_skip_tls_verify` — a configured server is verified unless told otherwise |
| stormblock | `http://127.0.0.1:9090` | `[stormblock] url` |
| sbregistry | `http://127.0.0.1:5100` | `[sbregistry] url` |
| stormdrive | `http://127.0.0.1:9092` (its stormview feed) | `[stormdrive] url` |
| stormstorage | `http://127.0.0.1:9093` (its stormview feed) | `[stormstorage] url` |
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

Actions on cards — delete a volume, restart a stormd process, locate a
drive — are carried by the feed and invoked with the method the feed
declares, through the owning plugin's proxy.

When the console cannot start it prints exactly one line on stderr —
`stormconsole: fatal: config /etc/stormconsole/stormconsole.toml: line 2:
unknown field `port` …` — and exits **78** (`EX_CONFIG`) for a config it
cannot run on, **1** for a port it cannot bind. A supervisor reading the
code can tell the one a restart will not fix from the one it might.
