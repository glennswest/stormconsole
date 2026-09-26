# stormconsole

The StormCOS web console. One Rust binary (`stormconsole`, axum) with the
browser app (Svelte 5 + [stormview](https://github.com/glennswest/stormview))
embedded, listening on **:9094**. It shows a StormCOS cluster as it is —
projects and what runs in them, virtual machines, storage, drives, bare
metal, the datastore, the fleet and its logs — and acts on it through each
component's own API. Every domain is a **plugin**; the host knows none of
them. The orchestrator is rustkube (+ rustkube-node) only.

It is a view of real running nodes, never an installer (stormcos
`docs/CLUSTER.md`). Version **0.20.0**. Design: [docs/architecture.md](docs/architecture.md).

## What it does today

Each plugin reads one component and contributes navigation, API routes,
create forms and a slice of the component feed every page is drawn from.

| Plugin (id) | Reads | Shows | Acts |
|---|---|---|---|
| kubernetes (`k8s`) | rustkube apiserver, list+watch of 23 kinds; the Cilium agent's `/healthz` | Projects; workloads, services, claims, policies, Cilium; the Cluster section (nodes, namespaces, PVs, storage classes, CRDs, cluster roles); events | projects (request, members, isolation, delete), YAML import/edit, delete — **as the viewer**, so RBAC decides |
| vm (`vm`) | KubeVirt `VirtualMachine`/`VirtualMachineInstance` and `snapshot.kubevirt.io` through the apiserver; stormvm (:9095) for consoles and verbs | Virtual machines: addresses asked vs done, disks, settings, SSH keys, snapshots, serial and framebuffer consoles | create, start/stop/restart, pause/reset/freeze…, settings, disks, snapshot/restore, keys |
| vmimages (`img`) | vmcloud-image-operator (:9099) | VM catalogue, VM goldens, local copies | make a golden, retry, delete (the `CloudImage`, via the apiserver), unmanage |
| fleet (`fleet`) | the stormcast log group's hosts; this node's stormd APIs | Nodes, and on demand one node's services | this node's stormd services start/stop/restart |
| logs (`logs`) | stormcast multicast `239.255.42.1:5514` (RFC 5424) into a redb ring | Fleet logs: filter, search, live follow | — |
| stormdrive (`drive`) | stormdrive (:9092) on this node **and every fleet node** | Drives as a rack map (chassis by bay, heat by health/temp/wear/usage), shelves | locate, fleet join/leave, tests, format, designate |
| stormstorage (`storage`) | stormstorage (:9093) feed | Storage pools | the feed's own actions |
| stormblock (`sb`) | stormblock engine (:9090) | Volumes (attached, with consumer), Unattached, slabs, arrays, exports | delete a volume (not while in use), create volume/export |
| sbregistry (`reg`) | stormblock-registry (:5100) | Images → Catalog (goldens, blanks, media, lineage, clones, download progress), pushed images, pallets | create golden/clone |
| fastetcd (`etcd`) | fastetcd `/health` (:2379) and `/metrics` (:2381) | Datastore: revision, size vs quota, alarms, leader; members and keyspace when the v3 JSON gateway is served | compact, defragment, disarm, snapshot (admin) |
| stormipmi (`ipmi`) | stormipmi (:9097) Machines API | Hardware → Machines: by service tag, BMC, power, the release each boots, default image, adopt, SOL console | power, set release/default, test mark, adopt (admin) |

Pages (hash routes): `#/` overview · `#/projects` · `#/k8s/<kind>` ·
`#/k8s/ns/<name>` (a project's page) · `#/k8s/events` · `#/vms` ·
`#/vm/<ns>/<name>` · `#/images` · `#/machines` · `#/drives` (`?group=shelf`) ·
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
| `verify-vm-net.sh`, `verify-vm-keys.sh`, `verify-vm-snapshots.sh` | VM addresses, SSH keys, snapshots — real fastetcd + rustkube |
| `verify-images.sh` | Volumes vs Images — stormblock v18.1.0 + sbregistry v0.23.0 from their tags, and forge's engine read-only |
| `verify-drives.sh` | the Drives map at 1,600 drives across 10 nodes |
| `verify-machines.sh` | the Machines page — stormipmi's own rig (ipmi_sim, stand-in forge) |
| `verify-etcd.sh` | the datastore — a real etcd and a real fastetcd |
| `verify-auth.sh` | what is open, the bearer, token and reader sessions |

## Run

```
stormconsole [--config <path>]      default /etc/stormconsole/config.toml
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
| `[kubernetes] enabled / server / token / insecure_skip_tls_verify` | on / `https://127.0.0.1:6443` / — / false | the local default is unverified (self-signed stormcert); a configured server is verified unless set |
| `[kubernetes] system_namespaces` | `["cilium"]` | never a project, besides `default`, `openshift`, `kube-*`, `openshift-*` |
| `[fleet] enabled / mcast_group / stormd_host / stormd_ports` | on / `239.255.42.1:5514` / `127.0.0.1` / 9080–9089, 9180–9199, 180, 8269 | |
| `[logs] enabled / mcast_group / db_path / ring_cap / retain_hours / dedup` | on / `239.255.42.1:5514` / `<data_dir>/logs.redb` / 200000 / 168 / true | |
| `[stormdrive] enabled / url / nodes` | on / `http://127.0.0.1:9092` / {} | `nodes` = `host = "url"`, beside the fleet-discovered ones |
| `[stormstorage] enabled / url` | on / `http://127.0.0.1:9093` | |
| `[stormblock] enabled / url / token_file` | on / `http://127.0.0.1:9090` / — | the engine's `<data_dir>/api_token`; a v18 engine answers 401 to reads without it |
| `[sbregistry] enabled / url` | on / `http://127.0.0.1:5100` | |
| `[vm] enabled / url / ssh_keys_namespace` | on / `http://127.0.0.1:9095` / `default` | `url` is stormvm, for the consoles and verbs only |
| `[vmimages] enabled / url` | on / `http://127.0.0.1:9099` | |
| `[fastetcd] enabled / url / metrics_url` | on / `http://127.0.0.1:2379` / `http://127.0.0.1:2381` | |
| `[stormipmi] enabled / url / token_file` | on / `http://127.0.0.1:9097` / — | stormipmi's `api.tokenFile`, held server-side |

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
| metrics | **none**; there is no `/metrics` |

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
  admin-only — Machines writes and typing into a SOL console, the datastore,
  YAML into a system namespace. The write gate is **one check in the host,
  by method**: any non-GET under `/api/plugins/` needs `operator`.
- **Identity upstream**: a user with a `kube_token` is asked about as
  themselves — the namespaces they see and every write they make are the
  apiserver's RBAC answer. Without one the console says so
  (`/api/v1/console/access`).

## Host API

| Route | |
|---|---|
| `GET /api/v1/components` · `WS /ws/components` | the feed, filtered per viewer; the socket pushes each change |
| `GET /api/v1/console/nav` · `/creators` · `/access` | navigation, create forms, what this viewer is not shown |
| `GET /api/v1/console/events?id=` · `/events/recent` | what happened to one object; the dock |
| `/api/v1/auth/login` · `/logout` · `/session` | |
| `/api/plugins/<id>/…` | each plugin's own routes (see [docs/architecture.md](docs/architecture.md)) |

## How it ships

A stormcos **service golden**, `stormconsole` (32 MB), built by stormcos
`deploy/build-goldens.sh`: the static musl binary with the SPA inside,
under stormd, with the flat config (`listen_addr = "0.0.0.0:9094"`,
`data_dir = "/var/lib/stormconsole"`), `stormconsole-data` and
`stormconsole-logs` volumes, stormd's API on 9194, exit 78 not restarted,
started on single-node clusters. Goldens are requested with
`stormcentral component build stormconsole`. Not yet right on the stormcos
side: the golden's liveness path (stormcos#102 — it must be `/healthz`) and
the engine token for Storage (stormcos#94).

`Containerfile` + `config/stormd.toml` build the same thing as a container
on `stormdbase` (stormd on 9080, the console under it, liveness
`/healthz`), for running outside a StormCOS node.

## Not done, and why

- **Pod logs** — nothing serves `kubectl logs` for ordinary pods
  (rustkube#55, rustkube-node#34, closed as duplicates of stormvm#5).
- **Fleet lifecycle** — join, promote, demote, drain are a CLI on the node
  with no API (stormcos#38); the console offers none.
- **Cilium flows, Hubble, agent metrics** — stormpump#11 (#4).
- **VM metrics over time** — cadvisor is not wired (#14); **VM hotplug and
  memory resize** — stormvm#18, stormvm#19; **keys into a running guest** —
  stormvm#41; **VMs on the pod network** (and so isolation covering them) —
  stormvm#16; **snapshot step, disks, size** — stormvm#45.
- **Datastore members, keyspace and verbs on fastetcd** — its v3 JSON
  gateway (fastetcd#28); traffic counters (fastetcd#29).
- **Per-drive usage on other nodes** — stormdrive#12 (this node's comes from
  its engine).
- **Watch deletes of custom resources** — rustkube#100: a deleted VM
  instance or snapshot stays on a watching console until it relists.
- **Registry reads carry no credential** — a registry with an auth file and
  no anonymous pull answers the console 401 (#35).
- **Users and groups without editing a file, certificate identity, audit**
  — #15 steps 3 and 4.
- **Remote plugins** (a component contributing its own UI) — designed, not
  built.

## Status

0.20.0. Every page above is live-verified against the real upstream (or,
for the drives rack, 1,600 stand-in drives over real transport); see
[CHANGELOG.md](CHANGELOG.md) and the work plan in [CLAUDE.md](CLAUDE.md).
