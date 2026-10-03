---
marp: true
title: stormconsole
description: The StormCOS web console — purpose and functionality
paginate: true
---

# stormconsole

**The StormCOS web console**

One Rust binary, the browser app inside it, on **:9094**.
It shows a StormCOS cluster as it is, and acts on it through each
component's own API.

v0.21.0 · `npx @marp-team/marp-cli docs/presentation.md`

---

## The problem

A StormCOS cluster is a dozen components: an orchestrator, a datastore, a
block engine, a registry, a drive manager, a hypervisor, bare metal, a log
bus. Each has its own API, and none is a place to *look at the cluster*.

stormconsole is that place — **a view of real running nodes** (stormcos
`docs/CLUSTER.md`): projects and what runs in them, VMs, storage, drives,
machines, the datastore, the fleet and its logs, with the day-2 actions
each component already offers. **Not an installer.**

---

## Where it sits

stormcentral's graph: **depends on** `stormview` (the UI contract and
widgets), `rustkube` (the orchestrator), `stormrfb` (the VNC client) and
`stormd` (its supervisor). Nothing depends on it; stormcos ships it.

What the code **reads at runtime**, one plugin each:

| | | |
|---|---|---|
| rustkube :6443 | stormblock :9090 | stormdrive :9092 (every node) |
| stormstorage :9093 | sbregistry :5100 | fastetcd :2379 / :2381 |
| stormvm :9095 | vmcloud-image-operator :9099 | stormipmi :9097 |
| stormcast `239.255.42.1:5514` | stormd APIs 9081–9199 | Cilium agent :9879 |

An absent upstream is not an error: its card says which address did not
answer.

---

## How it works

```
   browser ── SPA (Svelte 5 + stormview, embedded) ──┐
                                                     │  /api/v1/components  /ws/components
 ┌───────────────────── stormconsole :9094 ──────────▼──────────────────────┐
 │  host: auth · write gate · per-viewer filter · nav · creators · events   │
 │  ┌────────┐ ┌────┐ ┌────┐ ┌─────┐ ┌────┐ ┌───┐ ┌─────┐ ┌────┐ ┌──────┐   │
 │  │  k8s   │ │ vm │ │img │ │drive│ │ sb │ │reg│ │etcd │ │ipmi│ │fleet │…  │
 │  └───┬────┘ └─┬──┘ └─┬──┘ └──┬──┘ └─┬──┘ └─┬─┘ └──┬──┘ └─┬──┘ └──┬───┘   │
 └──────┼────────┼──────┼───────┼──────┼──────┼──────┼──────┼───────┼───────┘
     rustkube  stormvm  image  stormdrive stormblock sbregistry fastetcd stormipmi
     (watch)  (doors)  operator (every node)                        stormcast
```

Every domain is a **plugin**: navigation, routes, create forms, a slice of
one component feed, and what a viewer may see of it. The host knows none of
them.

---

## What it does today — workloads

- **Projects first.** A project is a namespace with an owner (rustkube's
  `project.openshift.io`). Selector, New project, members (admin/edit/view),
  **Isolate** (NetworkPolicies), delete. Every create asks which project;
  nothing lands in `default`.
- **Kubernetes.** 23 kinds watched: workloads, services, claims, policies,
  Cilium; the Cluster section (nodes, namespaces, PVs, storage classes,
  CRDs, cluster roles). YAML import and edit. All **as the viewer** — RBAC
  decides.
- **Virtual machines.** KubeVirt objects. Create from a golden, lifecycle
  and control verbs, settings (now vs next boot), disks, addresses asked vs
  done, **SSH keys once**, **snapshots**, serial and framebuffer consoles.

---

## What it does today — storage and hardware

- **Images → Catalog.** The registry's goldens, blanks and media, grouped by
  kind, with base lineage, clones, releases, and download progress.
- **Storage → Volumes.** Only what is attached to something running, with
  its consumer; Unattached apart; delete disabled while in use.
- **Drives, at rack scale.** Every node's drives as a map: each chassis bay
  by bay, coloured by health, temperature, wear or usage; grouped by chassis,
  node or rack; failing / degraded / rebuilding / draining / full filters;
  totals to EB. Each drive: what is left, its slabs, the volumes on it and
  who uses them, a drain in progress; pools per node, role and tier.
  Checked at 1,600 drives.
- **Machines.** Bare metal by service tag from stormipmi: BMC, power, the
  release each boots, default image, adopt, SOL console — admin-only writes.
- **Datastore.** fastetcd's revision, size vs quota, alarms, leader.

---

## What it does today — the fleet and the chrome

- **Nodes** from the stormcast log group; one node's services on demand.
- **Fleet logs** in a deduplicating redb ring: filter, search, live follow.
- **Events** on every object, and a dock of what this console did.
- **Who sees what**: the feed filtered per viewer before it leaves the
  process; a user's own `kube_token` makes the apiserver's answer theirs.
- **Who may do what**: viewer / operator / admin, one write gate by method.
- Two styles × 12 palettes; OpenShift-shaped navigation, collapsible
  admin sections.

Each of these has a live check: `sc-build deploy/verify-*.sh` stands the
real upstreams up on dev.

---

## Interfaces

| | |
|---|---|
| Port | **9094** — SPA, API, websocket |
| Health | `/healthz` (liveness), `/readyz` (503 on Error), `/api/version` |
| Metrics | **none** |
| API | `/api/v1/components`, `/ws/components`, `/api/v1/console/{nav,creators,access,events}`, `/api/v1/auth/*`, `/api/plugins/<id>/…` |
| CLI | `stormconsole [--config <path>]` · `--hash-password` |
| Config | `--config`; on a node `/etc/stormconsole/stormconsole.toml` (default `/etc/stormconsole/config.toml`), TOML, unknown keys refused; one section per plugin (`enabled`, `url`, tokens) |
| Exit | 78 bad config (stormd does not restart it), 1 cannot bind |

Every key and default: README §Configuration.

---

## How it ships and runs

- **Built** with `sc-build` on dev: musl static binary, SPA embedded
  (`web/dist` committed).
- **Shipped** as a stormcos **service golden**, `stormconsole` (32 MB), from
  its entry in stormcentral's component registry, built by stormcos
  `deploy/build-goldens.sh`: under stormd (API 9194), flat config
  (`listen_addr`, `data_dir`, the engine's `token_file`), data and log
  volumes, exit 78 not restarted, started on single-node clusters.
- **Updated** by a new golden: `stormcentral component build stormconsole`
  → a stormcos release request → the next release carries it.
- **Outside StormCOS:** the `Containerfile` on `stormdbase`, stormd on 9080.

---

## Planned — not built

- Scale a workload; cordon, uncordon, drain a node — **#36**
- Fleet lifecycle: join, promote, demote, drain — no API (**stormcos#38**)
- A pod's terminal and environment — exec waits on the kubelet (**rustkube-node#56**, **#12**)
- Cilium flows and Hubble — shipped in the image, not read yet (**#4**)
- Import a VM disk by URL — the registry serves it (**#44**)
- VM metrics over time — cadvisor not wired (**#14**)
- Users and groups without a file, certificate identity, audit — **#15**
- Remote plugins — a component contributing its own UI — designed only

---

## Status and what matters

**0.24.0**, every page's API live-verified against its real upstream; most pages not yet viewed in a browser (**#58**).

Open, and why it matters:
- **stormcos#102** / stormcentral#226 — the golden's liveness path is
  `/admin/healthz`, which the console does not route: it answers the app
  with a 200, so the probe always passes and a hung console is never seen.
- **#42** (a decision) — SSH keys live in `default`, where a
  project-only user cannot write them.
- **#35** — the registry plugin sends no credential.
- **#64** — fastetcd serves the v3 gateway and traffic counters now
  (v1.8.0); the datastore page has not yet been checked against it.
- Upstream data still coming: stormblock#152, stormvm#16, #41, #45.

Docs: README (operational), `docs/architecture.md` (design), CHANGELOG.
