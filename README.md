# stormconsole

The StormCOS web console. One Rust binary (`stormconsole`, axum) with the
browser app (Svelte 5 + [stormview](https://github.com/glennswest/stormview))
embedded, listening on **:9094**. It shows a StormCOS cluster as it is —
projects and what runs in them, virtual machines, storage, drives, bare
metal, the datastore, the fleet and its logs — and acts on it through each
component's own API. Every domain is a **plugin**; the host knows none of
them. The orchestrator is rustkube (+ rustkube-node) only.

It is a view of real running nodes, never an installer (stormcos
`docs/CLUSTER.md`). Version **0.21.0**. Design: [docs/architecture.md](docs/architecture.md).
In eleven slides: [docs/presentation.md](docs/presentation.md) (Marp —
`npx @marp-team/marp-cli docs/presentation.md`).

## What it does today

Each plugin reads one component and contributes navigation, API routes,
create forms and a slice of the component feed every page is drawn from.

| Plugin (id) | Reads | Shows | Acts |
|---|---|---|---|
| kubernetes (`k8s`) | rustkube apiserver, list+watch of 23 kinds; the Cilium agent's `/healthz`; each node's kubelet `/metrics/cadvisor` (:10250) for a pod's counters; stormcentral's registry images (goldens), when `[stormcentral]` is set | Projects; workloads, services, claims, policies, Cilium; the Cluster section (nodes, namespaces, PVs, storage classes, CRDs, cluster roles); events; **a pod's page** — owners, containers, image and digest, the golden's build, addresses, DNS, the Services that select it and whether it is in their Endpoints, Cilium, traffic, logs (current, previous, and the last 5 runs the console kept) | projects (request, members, isolation, delete), YAML import/edit, delete — **as the viewer**, so RBAC decides |
| vm (`vm`) | KubeVirt `VirtualMachine`/`VirtualMachineInstance` and `snapshot.kubevirt.io` through the apiserver; stormvm (:9095) for consoles and verbs | Virtual machines: addresses asked vs done, disks, images, metadata, settings, SSH keys, snapshots, serial and framebuffer consoles | create, start/stop/restart, pause/reset/freeze…, settings, disks, snapshot/restore, keys |
| vmimages (`img`) | vmcloud-image-operator (:9099) | VM catalogue, VM registry images, local copies | make a registry image, retry, delete (the `CloudImage`, via the apiserver), unmanage |
| fleet (`fleet`) | the stormcast log group's hosts; this node's stormd APIs | Nodes, and on demand one node's services | this node's stormd services start/stop/restart |
| logs (`logs`) | stormcast multicast `239.255.42.1:5514` (RFC 5424) into a redb ring | Fleet logs: filter, search, live follow | — |
| stormdrive (`drive`) | stormdrive (:9092) on this node **and every fleet node** | Drives as a rack map (chassis by bay, heat by health/temp/wear/usage); each drive's usage in bytes, its slabs, the volumes on it and any drain; shelves; pools per node, role and tier | locate, fleet join/leave, drain, overcommit, tests, format, designate |
| stormstorage (`storage`) | stormstorage (:9093) feed | Storage pools | the feed's own actions |
| stormblock (`sb`) | stormblock engine (:9090) | Volumes (attached, with consumer), Unattached, slabs, arrays, exports | delete a volume (not while in use), create volume/export |
| sbregistry (`reg`) | stormblock-registry (:5100) | Images → Registry images (component, container and slab registry images, blanks, media, lineage, instances, download progress), pushed images, pallets | create a registry image or an instance |
| fastetcd (`etcd`) | fastetcd `/health` (:2379) and `/metrics` (:2381) | Datastore: revision, size vs quota, alarms, leader; members and keyspace when the v3 JSON gateway is served (etcd; fastetcd v1.8.0+) | compact, defragment, disarm, snapshot (admin) |
| stormipmi (`ipmi`) | stormipmi (:9097) Machines API | Hardware → Machines: by service tag, BMC, power, the release each boots, default image, adopt, SOL console | power, set release/default, test mark, adopt (admin) |
| stormcluster (`cluster`) | stormcluster (:9102) feed and operations API, on this node — any node's answers for the cluster | Cluster → Membership: the cluster (or this SNO), members with role, state and the cluster CA, discovered nodes, the last operations and their steps | form, join (as workers, or masters in pairs), promote (in pairs), demote, drain, uncordon, split to SNO (keep or wipe), resume — each previewed as stormcluster's plan first (admin) |

**Claims.** A PersistentVolumeClaim of the built-in `stormblock` class
(provisioner `stormblock.storm.io`, binding `WaitForFirstConsumer`) is
served by the **built-in stormblock driver**: the node's own kubelet clones
and attaches it through its stormblock engine — no CSI. Every other class
goes through its CSI driver. The console shows the claim in its project
("Pending — provisioned when a pod or VM uses it" until something does,
with Attach to a VM) and the engine volume under Storage → Volumes with
the claim as its consumer.

**Words** (#70). What the platform's APIs call a *golden* — a sealed,
immutable image on forge — the console calls a **registry image**, and
the copy-on-write clone a pod, VM or boot runs on is its **instance**.
Only the page's words changed: ids, kinds, relation and metric names, form
fields and JSON keep `golden`, and the SPA translates the tokens it shows
(`web/src/lib/ui/words.js`, mirrored by `console_core::words`). The pod
page names the API's word once, in the Registry image row's tooltip.

Pages (hash routes): `#/` overview · `#/projects` · `#/k8s/<kind>` ·
`#/k8s/ns/<name>` (a project's page) · `#/k8s/events` · `#/vms` ·
`#/vm/<ns>/<name>` · `#/pod/<ns>/<name>` (`?tab=Logs&container=`) · `#/images` · `#/machines` · `#/drives` (`?group=shelf`, `?group=pool`) ·
`#/nodes` · `#/node/<host>` · `#/logs` · `#/etcd/keys` · `#/account/keys` ·
`#/attach/<ns>/<claim>` · `#/grid?id=&rel=`. The masthead carries the
Project selector, Create, cluster health, the key (your SSH keys),
appearance (12 palettes × two styles) and the session.

## Build

Never on a Mac, never on the stormcentral VM, never as root: push, then

```bash
sc-build                                       # cargo build && cargo test on dev.g8.lo
sc-build 'cargo build --release --locked --target x86_64-unknown-linux-musl'
sc-build 'node web/src/lib/drivemap.test.mjs'  # the Drives page model at 1,600 drives
```

The SPA is built with Vite (`web/`, `npm ci && npx vite build`) into
`web/dist/`, which is **committed** and embedded by `rust-embed`
(`crates/stormconsole/src/server.rs`). A release build embeds it; a debug
build reads `web/dist/` from disk. Rebuild and commit `web/dist/` with any
change under `web/src/`. The live checks run the same way, each standing up
the real upstreams it needs on dev and deleting them after:

| `sc-build deploy/…` | Checks |
|---|---|
| `verify-projects.sh` | projects, members, isolation, project-first create — as three real rustkube identities |
| `verify-create-project.sh` | the Create dialog in a headless Chromium (Playwright): Create VM → New project → name → create, with no page errors — real fastetcd + rustkube |
| `verify-vm-net.sh`, `verify-vm-keys.sh`, `verify-vm-snapshots.sh`, `verify-vm-lifecycle.sh` | VM addresses, SSH keys, snapshots, Start/Stop/Restart from every phase — real fastetcd + rustkube |
| `verify-vm-policy.sh` | machines outside policy (#51): NAT, host bridge and no-endpoint machines named in the isolate answer, the project card, the VM row and page — real fastetcd + rustkube, Chromium |
| `verify-vm-network-edit.sh` | the VM settings network edit (#50): stopped, running and per-interface-pinned machines — the definition read back from the apiserver, the form and pending, the Settings tab in Chromium — real fastetcd + rustkube |
| `verify-kube-tls.sh` | the console's apiserver credential (#33): rustkube over TLS with an openssl CA and anonymous off — `ca_file` + `token_file`, a stranger CA, a CA minted late, an expired token renewed in place, skip-verify, system roots, config contradictions |
| `verify-images.sh` | Volumes vs Images — stormblock v18.1.0 + sbregistry v0.23.0 from their tags, and forge's engine read-only |
| `verify-drives.sh` | the Drives map at 1,600 drives across 10 nodes; each drive's usage, slabs, volumes and pools — stand-ins in stormdrive v0.15.0's and stormblock's shapes |
| `verify-machines.sh` | the Machines page — stormipmi's own rig (ipmi_sim, stand-in forge) |
| `verify-cluster.sh` | the Cluster page — three real stormclusters (b1–b3) on loopback addresses and a private multicast group, stand-ins for the node lifecycle API and fastetcd's gateway; the proxy with curl, then form, join, resume, a refusal, promote in pairs, split, drain in Chromium as an admin and an operator |
| `verify-etcd.sh` | the datastore — a real etcd and a real fastetcd |
| `verify-etcd-tls.sh` | the datastore over mutual TLS (#47) — fastetcd built from its tag with `--client-cert-auth`, an openssl node CA and pairs (ECDSA and RSA, PKCS#8, as stormcert writes them); stormcos's shape healthy with members and the keyspace, each misconfiguration an error with its cause, a pair minted late and renewed picked up without a restart, the two config errors |
| `verify-auth.sh` | what is open, the bearer, token and reader sessions |
| `verify-storage-guard.sh` | destructive storage (#82) — real fastetcd + rustkube with the release's `storage-admin`/`storage-viewer` roles, stand-in stormdrives (here and storm-b) and engine recording each write's bearer; who is shown and refused what, the typed serial, whose bearer arrives, the audit line, a binding removed; Format in Chromium as a storage-admin and a storage-viewer |
| `verify-pod-page.sh` | the pod page — real fastetcd + rustkube v0.15.3, a stand-in kubelet (containerLogs, `/metrics/cadvisor`) and stormcentral; the API with curl and every tab in Chromium, screenshots in `shots.tgz`; and the words (#70) — stand-in registry, engine and image operator, every page that shows a registry image searched for “golden” |

## Tests on a node

The stormcos test standard (stormcentral `docs/test-standard.md`): one image
from `test/Containerfile` (repo root as context; `test/build.sh` builds the
static binary first), run by stormcentral as a Job in the run's own
namespace as `/test short|medium|long` — JSON lines on stdout and in
`/results`, exit 0 all passed, 1 a failure, 2 could not run. The Job and its
RBAC are `test/stormconsole-test.yaml`.

| Suite | Budget | Proves |
|---|---|---|
| `short` | < 2 min | up on the node — health, readiness, version, the app, the feed, the navigation — and its main job: a Service made through the apiserver appears in the console's feed, and leaves it |
| `medium` | < 30 min | the short suite, then: what is open without a session; every plugin card; the create forms; the websocket pushing a change; YAML into the project, refused with no project, refused when malformed; an object's YAML; an edit, and a stale edit refused (409); events; delete through the console; projects; a VM refused in a system namespace; a proxy kept to its upstream; an unknown API path 404 |
| `long` | the night | waves of Services sized from the node's allocatable pods: how long the console takes to show and drop each wave, the feed's size and answer time, what each wave leaves — a slowdown beyond 3× or any residue fails |

`requires: [service: stormconsole]`, nothing else: a node without the console
reports every test skipped, and a console with authentication on needs
`STORMCONSOLE_TOKEN` from the runner or reports skipped. Everything it makes
is in the run's namespace, labelled `storm.io/test-run`, and removed.
`sc-build deploy/verify-tests.sh` runs all three the way stormcentral does,
against a real console and rustkube, plus the edges and a podman build.

## Run

```
stormconsole [--config <path>]      default /etc/stormconsole/config.toml
                                    (on a node: /etc/stormconsole/stormconsole.toml)
printf %s 'pw' | stormconsole --hash-password   an argon2 password_hash, then exit
```

With no config file it runs on defaults and says so. `RUST_LOG` sets the
log level (default `info`). Exit codes: **78** (EX_CONFIG) for a config
that does not parse or validate, or a `--hash-password` with nothing on
stdin; **1** when the listen address cannot be bound or the server fails;
every fatal exit prints one `stormconsole: fatal: <what>` line on stderr.
stormd is told not to restart on 78.

## Configuration

TOML. **Unknown keys are errors** (named with their line), and every
section is optional. Two shapes are accepted: the flat node-service one
stormcos writes (`listen_addr`, `data_dir`) and the sectioned one below.
Full example: [config/config.toml](config/config.toml).

| Key | Default | |
|---|---|---|
| `listen_addr` | — | wins over `[api] bind` |
| `data_dir` | `/var/lib/stormconsole` | the log ring lives here |
| `[general] name` | `stormconsole` | |
| `[general] theme` | — | default palette; a viewer's pick wins |
| `[api] bind` | `0.0.0.0:9094` | |
| `[api] auth_token` | — | a machine credential (`Authorization: Bearer`); signing in with it is an admin session |
| `[[api.users]]` | none | `name`; `password_hash` (argon2 PHC; `password` plaintext still read, warned about); `roles` (`viewer` default, `operator`, `admin`); `ssh_keys`; `kube_token` (the user's own rustkube identity) |
| `[kubernetes] enabled / server / token / insecure_skip_tls_verify` | on / `https://127.0.0.1:6443` / — / false | without `ca_file` the local default is unverified (warned at start, said on the card); a configured server is verified against the system roots unless set |
| `[kubernetes] token_file` | — | the console's bearer from a file (stormcert's ServiceAccount token), re-read when it changes; not with `token` |
| `[kubernetes] ca_file` | — | PEM CA the apiserver is checked against — only that CA; re-read when it changes; unreadable fails closed with the file named on the card; not with `insecure_skip_tls_verify` (exit 78) |
| `[kubernetes] system_namespaces` | `["cilium"]` | never a project, besides `default`, `openshift`, `kube-*`, `openshift-*` |
| `[fleet] enabled / mcast_group / stormd_host / stormd_ports` | on / `239.255.42.1:5514` / `127.0.0.1` / 9080–9089, 9180–9199, 9201, 9202, 180, 8180, 8269, 8545 | a service golden's stormd is its port + 100 |
| `[logs] enabled / mcast_group / db_path / ring_cap / retain_hours / dedup` | on / `239.255.42.1:5514` / `<data_dir>/logs.redb` / 200000 / 168 / true | |
| `[stormdrive] enabled / url / nodes` | on / `http://127.0.0.1:9092` / {} | `nodes` = `host = "url"`, beside the fleet-discovered ones |
| `[stormstorage] enabled / url` | on / `http://127.0.0.1:9093` | |
| `[stormblock] enabled / url / token_file` | on / `http://127.0.0.1:9090` / — | the engine's `<data_dir>/api_token`; a v18 engine answers 401 to reads without it. On a node stormcos sets `/run/stormblock/engine/api_token` |
| `[sbregistry] enabled / url` | on / `http://127.0.0.1:5100` | |
| `[vm] enabled / url / ssh_keys_namespace` | on / `http://127.0.0.1:9095` / `default` | `url` is stormvm, for the consoles and verbs only |
| `[vmimages] enabled / url` | on / `http://127.0.0.1:9099` | |
| `[fastetcd] enabled / url / metrics_url` | on / `http://127.0.0.1:2379` / `http://127.0.0.1:2381` | |
| `[fastetcd] ca_file / cert_file / key_file` | none | mutual TLS on the client port (#47): fastetcd is verified against `ca_file` only (no built-in roots) and the console presents the pair. `cert_file` and `key_file` go together; any of them with a non-`https://` url is a config error (exit 78). Reread when a file changes; a missing or bad file is said on the datastore's card, naming it, and the console runs on. On a node: the stormcert node CA and the `stormconsole-etcd` pair under `/data/stormcert`, `url = "https://127.0.0.1:2379"` |
| `[stormipmi] enabled / url / token_file` | on / `http://127.0.0.1:9097` / — | stormipmi's `api.tokenFile`, held server-side |
| `[stormcluster] enabled / url / token_file` | on / `http://127.0.0.1:9102` / — | stormcluster's `token_file`, held server-side; without it a guarded stormcluster refuses every write with 401 |
| `[stormcentral] url / token_file` | — / — | stormcentral, for a `stormpump://` image's golden (build, commit, built by) on the pod page; off unless set — its golden list is authenticated |

An upstream that is not there is not an error: its card says which address
did not answer.

## Ports, health, metrics

| | |
|---|---|
| **9094/tcp** | everything: the SPA, the API, the websocket |
| `GET /healthz` | `ok` — liveness, no session needed |
| `GET /readyz` | `{health, plugins}`; **503** when overall health is Error |
| `GET /api/version` | `{console, release}` — the release from the nodes' `osImage` |
| `GET /api/summary` | the stormd plugin card |
| metrics | **none** — `/metrics` is not a route and falls through to the app with 200 HTML (#41) |
| outbound | each pod's node at **:10250** (the kubelet's `/metrics/cadvisor`, with the viewer's or the console's bearer — rustkube has no node proxy, rustkube#108) |
| unknown `/api/…`, `/ws/…` | JSON **404** `{"error": "no such route: …"}`; any other path is the app |

## Authentication and roles

Off until `[api] auth_token` or a user is configured — then every request
not on the open list (`/healthz`, `/readyz`, `/api/version`, `/api/summary`,
`/api/v1/auth/*`, static assets) needs a session or the bearer. With it
off, everybody who reaches the port is an administrator, and the console
warns about that on every start.

- **Sessions**: `POST /api/v1/auth/login {username, password}`, cookie
  `stormconsole_session` (HttpOnly, SameSite=Strict, 24 h, in memory — a
  restart signs everyone out). Tokens and passwords are compared in
  constant time.
- **Roles**: `viewer` reads; `operator` writes; `admin` also gets what is
  admin-only — Machines writes and typing into a SOL console, every cluster
  membership operation, the datastore,
  YAML into a system namespace. The write gate is **one check in the host,
  by method**: any non-GET under `/api/plugins/` needs `operator`.
- **Identity upstream**: a user with a `kube_token` is asked about as
  themselves — the namespaces they see and every write they make are the
  apiserver's RBAC answer. Without one the console says so
  (`/api/v1/console/access`).
- **Destructive storage is for storage-admins** (#82, stormcos#250):
  format, sanitize, wipe, partition and the destructive test on a drive (on
  any node), the drive worker's jobs; RAID set create, destroy, member
  add/fail/replace; slab create and destroy; forge on/off; and everything
  else the engine calls destructive (every DELETE — so deleting a volume —
  seal, tar, files, gc, trim with apply, fsck with repair). For these, no
  console role is enough, `admin` and the console's own `auth_token`
  included:
  - the console asks a **SelfSubjectAccessReview as the user** (their
    `kube_token`) for `storage.storm.io`, the resource and the verb — the
    release's `storage-admin` ClusterRole holds every verb there,
    `storage-viewer` only reads. No identity (authentication off, the
    token, a user without `kube_token`), no apiserver, or an apiserver
    without the review: refused, with the reason. Answers stand 30 s;
  - those actions are **not in the feed** for anyone the review refuses —
    they see every drive, set, slab and volume, read-only;
  - the request needs `X-Storm-Confirm` set to the **drive's serial** (the
    object's name otherwise): 428 says what to type, and the page asks for
    it after the OK;
  - the proxy sends the **user's own bearer** upstream, never the
    console's or the engine's node token, so the component's own
    SubjectAccessReview decides too (stormdrive#45, stormraid#8,
    stormblock#274). Until they do, stormdrive takes the request on the
    console's check alone and the engine refuses it (it wants its admin
    token): deleting a volume in the console waits on stormblock#274;
  - each one done is logged — `storage: <verb> <resource> on <object> as
    <user>` — and each refusal as `storage: refused`.

## Host API

| Route | |
|---|---|
| `GET /api/v1/components` · `WS /ws/components` | the feed, filtered per viewer; the socket pushes each change |
| `GET /api/v1/console/nav` · `/creators` · `/access` | navigation, create forms, what this viewer is not shown |
| `GET /api/v1/console/events?id=` · `/events/recent` | what happened to one object; the dock |
| `GET /api/v1/console/guard?method=&path=` | is this request destructive storage, may this viewer, what to type (#82) |
| `/api/v1/auth/login` · `/logout` · `/session` | |
| `/api/plugins/<id>/…` | each plugin's own routes (see [docs/architecture.md](docs/architecture.md)) |

## How it ships

A stormcos **service golden**, `stormconsole` (32 MB), described by its
entry in stormcentral's component registry (port 9094, argv `--config
/etc/stormconsole/stormconsole.toml`, its config text, `stormconsole-data`
64 MB and `stormconsole-logs` 64 MB) and built by stormcos
`deploy/build-goldens.sh`: the static musl binary with the SPA inside,
under stormd (its API on 9194), exit 78 not restarted, started on
single-node clusters. The config it is given is the flat shape —
`listen_addr = "0.0.0.0:9094"`, `data_dir = "/var/lib/stormconsole"` — plus
`[stormblock] token_file = "/run/stormblock/engine/api_token"`, with the
host's `/run/stormblock` mounted read-only (stormcos#94, done). Goldens are
requested with `stormcentral component build stormconsole`; a release
installs them.

Not yet right on the platform side: the golden's liveness path is
`/admin/healthz`, which the console does not route — it falls through to
the app's HTML with a 200, so the probe always passes and detects nothing.
It must be `/healthz` (stormcos#102, stormcentral#226).

`Containerfile` + `config/stormd.toml` build the same thing as a container
on `stormdbase` (stormd on 9080, the console under it, liveness
`/healthz`), for running outside a StormCOS node.

## Not done, and why

- **On the pod page, what the node does not report** — a container's
  `lastState` and termination reason, a digest for a `stormpump://` image,
  when an image was last resolved, and its OCI build info
  (rustkube-node#130); the interface's MTU, gateway, routes and CNI,
  packets/errors/drops, and runs before the previous one
  (rustkube-node#131). Each is read when present and named where it is
  not. The console keeps the last 5 runs per container itself, in memory.
  Which registry image a node's instance was cloned from is not on the pod, so the page shows the newest
  stormcentral built for the component and says so. A **terminal** waits
  on the kubelet answering exec (rustkube-node#56); **Environment** (#12).
  A VM's traffic counters: stormvm#48.
- **Fleet lifecycle** — join, promote, demote, drain are a CLI on the node
  with no API (stormcos#38); the console offers none.
- **Scale a workload, cordon/uncordon and drain a node** — not built (#36).
- **Cilium flows, Hubble, agent metrics** — the image ships them
  (stormpump#11, closed); the console does not read them yet (#4).
- **VM metrics over time** — cadvisor is not wired (#14; per-VM stats
  keyed to the VMI are cadvisor#15); **importing a VM disk** — sbregistry's
  media path is served (v0.19.0) and the console does not offer it (#44); **VM hotplug and
  memory resize** — stormvm#18, stormvm#19; **keys into a running guest** —
  stormvm#41; **VMs on the pod network** (and so isolation covering them) —
  stormvm#16; until then a NAT'd or bridged machine says no policy applies
  to it, and an isolated project names the machines it does not reach (#51); **snapshot step, disks, size** — stormvm#45.
- **Datastore members, keyspace, verbs and traffic on fastetcd** — fastetcd
  serves the v3 JSON gateway since v1.8.0 (fastetcd#28) and etcd's traffic
  counters since v1.7.0 (fastetcd#29), and the plugin reads both in etcd's
  shape; it has been checked live against etcd 3.5 and fastetcd v1.2.0 only
  (#64). An older fastetcd gets a line naming the release that has them.
- **Volumes on another node's drives** — the console reads only this node's
  engine, so a drive on another node lists no volumes, and says so. Each
  drive's *usage* comes from its own node's stormdrive (v0.13.0+).
- **Committed and headroom** per slab and pool need the engine to report
  committed bytes (stormblock#152); until every slab of a pool does, the
  pool claims no headroom.
- **Watch deletes of custom resources** need rustkube ≥ v0.15.2
  (rustkube#100): against an older apiserver a deleted VM instance or
  snapshot stays on a watching console until it relists.
- **Actions the viewer may not take are still offered** and answered by
  the apiserver's 403; rustkube serves `SelfSubjectRulesReview` (v0.9.0)
  and the console does not ask it yet (#45).
- **Where SSH keys live** — `[vm] ssh_keys_namespace` defaults to
  `default`, a system namespace where a project-only user cannot write
  (#42, a decision).
- **Registry reads carry no credential** — a registry with an auth file and
  no anonymous pull answers the console 401 (#35).
- **Users and groups without editing a file, certificate identity, audit**
  — #15 steps 3 and 4.
- **Remote plugins** (a component contributing its own UI) — designed, not
  built.

## Status

0.21.0. Every page above is live-verified against the real upstream (or,
for the drives rack, 1,600 stand-in drives over real transport); see
[CHANGELOG.md](CHANGELOG.md) and the work plan in [CLAUDE.md](CLAUDE.md).
