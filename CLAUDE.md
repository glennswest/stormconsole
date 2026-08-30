# CLAUDE.md — stormconsole

The StormCOS web console — patterned on the OpenShift console, built in Rust
on stormd and stormview, with a pluggable architecture where each domain
(kubernetes via rustkube, fleet/nodes, logs, stormdrive, stormblock,
sbregistry) is a plugin contributing its own part.

**Hard rule: mkube is NOT part of this project and must never appear in its
design, code, or docs.** The orchestrator is rustkube + rustkube-node only.

## Version

Current: **0.4.0**

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

### Phase 1 — skeleton (v0.1.x) ✅ complete 2026-08-28
- [x] Repo, .gitignore, GitHub (private)
- [x] Design doc (`docs/architecture.md`)
- [x] Cargo workspace: `crates/console-core`, `crates/stormconsole`,
      `crates/plugins/*`
- [x] console-core: `ConsolePlugin` trait, registry, aggregated
      `/api/v1/components` + `/ws/components`, nav feed
- [x] stormconsole binary: axum server on :9094, config TOML, auth
      (sessions + bearer, stormd-compatible), SPA embed, plugin mounting
- [x] web/: Svelte 5 + stormview SPA shell — nav from `/api/v1/console/nav`,
      themes, login, Overview (ComponentCard grid), generic list/detail
      via ComponentGrid
- [x] Containerfile (FROM stormdbase, stormd supervises the binary) +
      `config/stormd.toml`
- [x] Build + test on dev.g8.lo — clean build, tests pass, live smoke of
      /healthz, /readyz, nav, components, summary, SPA, auth (401 → login
      → 200). musl release: 5.9 MB static binary with embedded SPA at
      `/build/cargo/stormconsole/x86_64-unknown-linux-musl/release/stormconsole`
      (dev uses `CARGO_TARGET_DIR=/build/cargo/stormconsole`)

Notes: the stormpump session packages the console as a golden guarded on
that binary path — one file plus /etc/stormconsole/config.toml, no
Containerfile needed on that path.

### Phase 2 — kubernetes plugin (rustkube) ✅ complete 2026-08-28
- [x] rustkube client (reqwest, bearer auth, list + `?watch=true` NDJSON
      streaming, reconnect with fresh list on failure) — serde_json::Value
      based, no generated types
- [x] Watch-backed cache over: namespaces, nodes, pods, deployments,
      statefulsets, daemonsets, jobs, cronjobs, services, PVCs
- [x] Components mapping with health derivation (pod phase/readiness,
      deployment ready/desired, node Ready condition) and relations
      (pod belongs_to ns + has_one node; ns has_many workloads)
- [x] Actions as POST routes under /api/plugins/k8s (delete pod first)
- [x] Events: on-demand REST `GET /api/plugins/k8s/events?namespace=`
- [x] UI: namespace selector (top bar, persisted), `#/k8s/:kind` list view
      over the feed, Events view; nav items per kind
- [x] Live verification against a real rustkube (fastetcd + kube-apiserver
      debug builds on dev, `--insecure --dev-anonymous-admin`): 10/10 kinds
      synced, namespaces/deployment/service in the feed with correct health
      (deploy 0/2 ready → error with no controllers running), delete-pod
      action through the console worked and the watch removed the pod from
      the feed. rustkube emits no events without controller-manager — the
      events view shows an honest empty list.
- [ ] Pod logs — blocked on rustkube#55 and rustkube-node#34 (interim:
      fleet logs deep link)

### Phase 3 — logs plugin ✅ complete 2026-08-28
- [x] Collector: socket2 multicast join on `239.255.42.1:5514`, lenient
      RFC 5424 parse (stormcast dialect; PRI → facility/severity;
      unparseable lines kept whole with source IP as host)
- [x] SQLite ring store (rusqlite bundled, WAL): events(ts, host, app,
      severity, msg), pruned to 200k rows; live tail on a broadcast channel
- [x] Query API: `GET /api/plugins/logs/events?host=&min_severity=&last=
      &search=`, `GET /summary` (hosts, counts), SSE `GET /stream`
- [x] Components: collector health/metrics (events stored, hosts seen)
- [x] LogsView UI: host/severity/search filters, recent table, live follow
      via EventSource
- [x] Verified on dev with synthetic RFC 5424 datagrams to the group:
      parse/fallback/severity filter/summary/component metrics all correct,
      SSE delivered a live datagram end to end

### Issue #3 — crash loop on StormCOS ✅ fixed 2026-08-30
Root cause: stormpump's `build-goldens.sh` writes the console's config in
the flat node-service shape (`listen_addr = …`, `data_dir = …`, no
sections) that stormdrive/stormstorage use; `Config` had
`deny_unknown_fields` and no such keys, so `toml::from_str` failed and
`main` returned `Err` → exit 1 on every start.
- [x] Accept `listen_addr` and `data_dir` at top level; `logs.db_path`
      defaults to `<data_dir>/logs.db` (`/var/lib/stormconsole`, the
      golden's writable volume)
- [x] Fatal path: one line on stderr naming what failed; exit 78
      (EX_CONFIG) for config errors, 1 for runtime errors
- [x] Tests: stormpump's exact file, the example file, unknown key named
- [x] Docs (README config section, example config, architecture), changelog
- [x] Build + test on dev (15/15); smoke: stormpump's verbatim config →
      alive 15 s, /healthz 200, ring at /var/lib/stormconsole/logs.db;
      unknown key → exit 78 naming file/line/key; busy port → exit 1
- [x] Release v0.3.0; close #3; file stormpump (re-enable
      `STORMCONSOLE_START`) and stormd (non-retryable exit codes) issues

### Make it a console (v0.4.0) ✅ 2026-08-30
Seen on sptest (192.168.8.106): every plugin idle, nothing configured, the
storage/registry plugins still phase-1 stubs — while rustkube has 15 pods,
stormblock 84 volumes, stormdrive/stormstorage serve feeds, and eight stormd
instances (9081–9085, 9192–9194) each serve a feed. Zero-config, node-local:
- [x] console-core: `Feed` (poll an upstream `/api/v1/components`, re-prefix
      ids/relations, route actions through the plugin proxy), `FeedPlugin`,
      `proxy::forward` + router, value helpers
- [x] Defaults when unset: rustkube `https://127.0.0.1:6443` (insecure —
      stormcert self-signed, no CA mounted; sno is anonymous-admin),
      stormblock :9090, sbregistry :5100, stormdrive :9092, stormstorage :9093
- [x] drive + storage (new) = FeedPlugin over the node's stormdrive/stormstorage
- [x] sb: volumes/slabs/arrays/exports/drives → components with health,
      metrics, delete action via proxy; nav Storage → Volumes/Slabs/…
- [x] reg: readyz + warm-up → registry health; goldens/clones/pallets/images
      → components; nav Images
- [x] fleet: nodes from the log collector's hosts (recency health, link to
      logs) + this node's stormd services discovered on the local port
      layout, system+process components with start/stop/restart via proxy
- [x] web: actions use their method (DELETE), logs view takes ?host=
- [x] Create, OpenShift-style: `Creator` contract + `/api/v1/console/
      creators`, k8s Import YAML + per-kind templates via `/apply`,
      stormblock/sbregistry forms, + Create menus, honest empty states
- [x] Built + tested on dev (all green); run on dev against sptest:
      133 components, 10/10 kinds, 84 volumes, 8 services, proxies 200.
      Live demo while the node is up: http://dev.g8.lo:9094/ (config at
      /build/cache/sc-live/config.toml points every upstream at
      192.168.8.106). The node is under active development and reboots;
      YAML create/conflict/delete verified against a local rustkube on dev
- [x] Cilium via CRDs: endpoints/nodes/identities/policies + card,
      creators, DELETE; optional CRD kinds synced-empty when absent
- [x] Release v0.4.0; update stormpump#7
- [ ] Next: cAdvisor container stats plugin (user integrating cadvisor),
      stormvm feed (:9095, FeedPlugin) + VM consoles (#2), per-node drill
      into other nodes, fleet actions

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

2026-08-30: stormpump#7 (re-enable the console in the image, #3 fixed),
stormd#2 (non-retryable exit codes; the console exits 78 for config errors).
Also stormpump#11 (Cilium agent metrics addr + Hubble/relay enablement) —
the image-side half of stormconsole#4 (full Cilium).
