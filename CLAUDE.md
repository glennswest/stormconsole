# CLAUDE.md — stormconsole

The StormCOS web console — patterned on the OpenShift console, built in Rust
on stormd and stormview, with a pluggable architecture where each domain
(kubernetes via rustkube, fleet/nodes, logs, stormdrive, stormblock,
sbregistry) is a plugin contributing its own part.

**Hard rule: mkube is NOT part of this project and must never appear in its
design, code, or docs.** The orchestrator is rustkube + rustkube-node only.

## Version

Current: **0.1.0** (bootstrap)

Version locations:
- `Cargo.toml` (workspace.package.version)
- `web/package.json`

## Key context

- Design: `docs/architecture.md`
- Build on `root@dev.g8.lo`, never on the Mac (see parent CLAUDE.md).
- UI system and contract: [stormview](https://github.com/glennswest/stormview)
  — Rust crate (ComponentSummary et al.) + npm package (themes, DataGrid,
  ComponentCard, ComponentGrid, RelationPicker, HealthDot, LoginPanel).
- Reference host app: stormd's `web/` (Svelte 5 + Vite, embedded SPA).
- Console listens on **:9094** (stormd 9080, stormblock 9090, stormdrive
  9092, rustkube 6443, kubelet 10250 are taken).
- Fleet discovery: stormcast multicast group `239.255.42.1:5514` (RFC 5424).
- stormcos `docs/CLUSTER.md` defines what the console must be: a view of
  real running nodes — join, promote, demote, drain, replace-a-disk — never
  an installer.

## Work plan

### Phase 1 — skeleton (v0.1.x) ✦ in progress
- [x] Repo, .gitignore, GitHub (private)
- [x] Design doc (`docs/architecture.md`)
- [ ] Cargo workspace: `crates/console-core`, `crates/stormconsole`,
      `crates/plugins/*`
- [ ] console-core: `ConsolePlugin` trait, registry, aggregated
      `/api/v1/components` + `/ws/components`, nav feed
- [ ] stormconsole binary: axum server on :9094, config TOML, auth
      (sessions + bearer, stormd-compatible), SPA embed, plugin mounting
- [ ] web/: Svelte 5 + stormview SPA shell — nav from `/api/v1/console/nav`,
      themes, login, Overview (ComponentCard grid), generic list/detail
      via ComponentGrid
- [ ] Containerfile (FROM stormdbase, stormd supervises the binary)
- [ ] Build + test on dev.g8.lo

### Phase 2 — kubernetes plugin (rustkube)
- [ ] rustkube client (reqwest, bearer/cert auth, watch=true streaming)
- [ ] Namespace views: namespace selector, per-namespace overview
- [ ] Workloads: pods, deployments, replicasets, statefulsets, daemonsets,
      jobs/cronjobs — list + detail + events
- [ ] Cluster: nodes, namespaces, PVs/PVCs, services
- [ ] Watch-backed cache → components feed contribution
- [ ] Pod logs — blocked on rustkube `/log` subresource and rustkube-node
      `/containerLogs` (issues filed; interim: stormlogs / direct node)

### Phase 3 — logs plugin
- [ ] Embedded fleet collector: join `239.255.42.1:5514`, parse RFC 5424,
      SQLite ring store
- [ ] Query API (`/api/plugins/logs/events?host=&min_severity=&last=`) +
      SSE follow
- [ ] Log viewer UI (severity filter, host filter, search, follow)

### Phase 4 — fleet/nodes plugin
- [ ] Node discovery from multicast presence
- [ ] Node detail: drill into that node's stormd/stormdrive/stormblock APIs
- [ ] Define the capability beacon (issue filed on stormcos)
- [ ] Fleet actions per CLUSTER.md: join, promote, demote, drain

### Phase 5 — storage & images plugins
- [ ] stormdrive plugin: aggregate per-node :9092 (drives, SMART, wear,
      locate, fleet lifecycle)
- [ ] stormblock plugin: :9090 volumes/exports/luns/slabs/arrays
- [ ] sbregistry plugin: goldens, clones, pallets, images (components feed
      issue filed)

### Later
- [ ] Dynamic remote plugins (manifest + reverse proxy, OpenShift
      dynamic-plugin style / stormd `[process.ui]` style)
- [ ] YAML edit/apply views for rustkube resources
- [ ] RBAC-aware UI (rustkube SSAR-equivalent)

## Cross-project issues filed

Tracked in `docs/architecture.md` §Integration gaps. File with `gh issue
create` on the owning repo; never fix in this repo (Core Rule 11).
