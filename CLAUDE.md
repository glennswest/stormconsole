# CLAUDE.md — stormconsole

The StormCOS web console — patterned on the OpenShift console, built in Rust
on stormd and stormview, with a pluggable architecture where each domain
(kubernetes via rustkube, fleet/nodes, logs, stormdrive, stormblock,
sbregistry) is a plugin contributing its own part.

**Hard rule: mkube is NOT part of this project and must never appear in its
design, code, or docs.** The orchestrator is rustkube + rustkube-node only.

## Version

Current: **0.14.0**

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
- [x] Pod logs — superseded: rustkube#55 and rustkube-node#34 were closed
      as duplicates of stormvm#5, which gives a VM's serial as an
      interactive socket rather than a log read. `kubectl logs` for
      ordinary pods is consequently unserved by anything; reopen those two
      if it is wanted

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

### Console chrome (v0.5.0) ✅ 2026-09-02
The SPA read as a dashboard: one type size, no breadcrumbs, a flat nav,
list pages that were a bare heading over a grid, and empty states that
said "No pods." with nothing to do about it. Reference points: the
OpenShift console and the ESXi host client.
- [x] `web/src/lib/ui/console.css` — the console's design layer over
      stormview's palette: 4px radii, near-flat elevation, a type scale,
      tabular numerals, focus rings, reduced motion, scrollbars, and the
      page grammar. Tokens only, so all twelve themes keep working;
      `--sc-masthead` is derived from `--panel`/`--bg` rather than fixed
- [x] Masthead: brand mark, namespace selector as the working scope, live
      health pill, one primary Create, navigator toggle
- [x] Navigator: collapsible groups, an icon per item matched from the nav
      feed, an object count per countable route, accent rail when active
- [x] Every view: breadcrumb → title/scope/count → toolbar → data;
      table/card switch persisted
- [x] Overview: status band (counts + one proportional rule) that filters
      everything below; plugin cards; each plugin's objects capped at 8
- [x] `ResourceTable` — the console's own table (Status not health, Kind
      only when rows differ, a header that actually sticks, Name first).
      stormview's `ComponentCard` still renders cards, restyled through a
      `.sc-cards` wrapper rather than forked
- [x] Empty states that name what is missing and carry the fix
- [x] Built and tested on dev (33/33); reviewed live against sptest
      through http://dev.g8.lo:9094/ in a dark and a light theme

### Two console styles (v0.6.0) ✅ 2026-09-02
- [x] `data-style` as an axis independent of stormview's `data-theme`:
      `openshift` (default) and `esxi`, selectable in the masthead and
      persisted. A style is proportion and structure, not colour, so both
      work on all twelve palettes
- [x] Density-dependent CSS moved onto style tokens (`--sc-row-py`,
      `--sc-nav-py`, `--sc-nav-font`, `--sc-gutter`, `--sc-zebra`,
      `--sc-btn-case`, …) so scoped component CSS stays style-agnostic
- [x] The masthead owns its foreground (`--sc-masthead-fg`/`-dim`/
      `-line`): both styles put a dark bar over the content whatever the
      palette, so it can no longer inherit a light theme's dark text
- [x] Reviewed live on dev in both styles × dark and light palettes

### Fleet log ring on redb (v0.7.0) ✅ 2026-09-02
- [x] SQLite → redb. Pure Rust, no C toolchain in the golden, no page
      ceiling. Fixes the `SQLITE_FULL` insert failures seen on sptest with
      1.7 TB free. File is now `<data_dir>/logs.redb`; the old `logs.db`
      is inert and can be deleted
- [x] Dedup on arrival keyed on host/app/severity/message: a repeat bumps
      `count` and last-seen instead of appending
- [x] Duplicate counts surfaced — `×N` per line, a lifetime `duplicates`
      metric on the collector component, `duplicates`/`received` on
      `/summary`
- [x] Automatic expiry on two bounds: `retain_hours` (168, from *last*
      seen) and `ring_cap` (200000 entries), swept on a 60s timer as well
      as on insert. `dedup = false` opts out
- [x] The flood fix, both ends: the tail updates a row in place instead of
      appending, and repeats broadcast at most once a second per line
- [x] Store failures no longer log per occurrence — the console's warnings
      go out over the group it collects from, so a broken ring was
      flooding the fleet it observes
- [x] Per-host/per-severity aggregates maintained on insert and prune
      (redb has no GROUP BY, and the feed asks every few seconds)
- [x] Dedup keys on a *fingerprint*: a leading timestamp is stripped,
      because emitters forward tracing's own line and its microsecond
      clock made every flooded line distinct. Found only by running
      against the live fleet — the first build deduplicated nothing
- [x] 44/44 tests on dev, twelve new covering dedup, fingerprinting,
      throttling, both bounds, aggregate bookkeeping under eviction, and
      the stale-ring error
- [x] Verified on dev against the real group: 646 arrivals of sptest's
      flooding line → 1 entry, `×646`, live tail responsive

### Namespaces, access, hardware and VMs (v0.8.0) ✅ 2026-09-09
Nine issues filed after a live review, all closed but #4 (half of it is
gated on stormpump#11). Verified against a real rustkube + fastetcd on
dev with a real ServiceAccount and a synthetic stormdrive feed; 84 tests.

- [x] **#5 namespace as a dimension** — the kind catalogue moved to the
      server (`/api/plugins/k8s/kinds`, from `cache::RESOURCES`), so the
      SPA stopped carrying three hardcoded lists of what is namespaced;
      the selection travels in the URL (`?ns=`); cluster-scoped kinds say
      they are, and the masthead greys the selector where it does not
      apply
- [x] **#6 namespace detail** — `#/k8s/ns/<name>` with tabs, an inventory
      whose every count is a link, quota as used-against-hard, limit
      ranges, events and YAML. "No quota" is shown as the answer it is
- [x] **#7 access-scoped namespaces** — `console_core::access` (a
      `Viewer` on every request, an `Access` answer per plugin, filtering
      before the snapshot leaves the process) and a k8s answer that is an
      authorization result. Four things only the live run found: the
      probe was asking about the Namespace object, which is cluster-scoped
      and which no RoleBinding can grant — so every ordinary project
      member saw *nothing*; the VM plugin was not scoped at all; plugin
      routes were an open door around the filtered feed; and writes were
      going out on the console's credential rather than the viewer's.
      Filed rustkube#59 for the SelfSubjectAccessReview that would
      replace the probe
- [x] **#8 drives are hardware, not storage** — a Hardware nav section,
      `#/drives` grouped by shelf and ordered by bay with the feed's real
      actions, `#/drives?group=shelf` for the enclosure question, and row
      actions behind a menu (nine buttons per row put a destructive one a
      mis-click from a harmless one). Filed stormdrive#3 for bay and
      controller as metrics instead of prose
- [x] **#9 + #2 VM plugin** — the whole shape came from reading
      stormvm's `docs/kube.md` rather than assuming a daemon: a VM is a
      KubeVirt object the kubelet reconciles, so the plugin watches the
      CRDs. Lifecycle, disks, network, YAML, and both console doors —
      built, probed, and honest about stormvm's console service being
      unbuilt. Import stays blocked on stormblock-registry#5
- [x] **#10 loopback addresses** — `console_core::upstream`; verified no
      component mentions `127.0.0.1` any more
- [x] **#4 Cilium, the ungated half** — the agent's health server on the
      node, taken as the worse of it and the CRD view; and YAML
      view/edit (`PUT /api/plugins/k8s/object/…`, a replace, so
      `resourceVersion` is the concurrency guard and a rename is
      refused). Metrics, hubble-ui and the flow view stay gated on
      stormpump#11 — #4 stays open for them
- [x] **#1** — closed: stormdrive v0.4.0 and stormstorage v0.2.0 are
      already consumed as `FeedPlugin`s, and #8 was the consumer-side
      work that was left

### The VM console doors, live (2026-09-09)
stormvm#5 landed, so the doors stopped being theoretical. Running both
ends together found three bugs no amount of reading either side would
have: the stormvm probe was behind the apiserver guard (a node with no
rustkube reported its consoles shut while stormvm answered on the same
machine); doors were reported per stormvm rather than per VM; and a
refusal reached the viewer as a bare status line. Verified against a real
`stormvm serve` — the relay is byte-identical to dialling stormvm direct,
and the browser terminal takes keystrokes to the guest.

Next: Cilium's gated half (stormpump#11), the capability beacon
(stormcos#26) and fleet lifecycle (stormcos#38).

### Relations are references, not destinations (#18) ✅ v0.9.0 2026-09-22
A VM row pushed its node as a relation and nothing else; the table read
that as something the row *contains* and the card as where the row
*leads*, so opening a machine landed in node details. 203d5b8 patched it
for VMs with a metric. The shape is everywhere, so the fix belongs in the
rule, not in one plugin.

- [x] `ResourceTable`: only `has_many` nests. A `has_one` or `belongs_to`
      is context — the node a VM is on, the volume a clone is stored in —
      and is rendered as a reference chip, never as containment
- [x] Placement becomes a **column**, generically: any `belongs_to` whose
      values actually differ down the list earns one (so namespace, node,
      array, shelf appear; "engine", the same on every row, does not).
      Replaces the hardcoded namespace/node pair, and works for the
      `FeedPlugin` upstreams whose components this repo cannot edit
- [x] Every row expands, and the expanded row is worth expanding: the
      detail in full, every metric, the references as links, and **all**
      the actions — the destructive ones included and labelled. Clicking
      the line opens that object's own page where it has one; where it
      does not, the expansion is the detail
- [x] Plugin sweep: every `has_one` that points at a container, an owner
      or a placement becomes `belongs_to` (vm, vmimages, sbregistry,
      stormblock, kubernetes/cilium)
- [x] VM lifecycle on the row: the duplicate Stop 203d5b8 left behind, and
      a Restart that exists at all — refused with a sentence, and disabled
      on the row, where there is no definition to restart from
- [x] Verified live on dev against a real fastetcd + rustkube seeded with
      two nodes, four pods across two namespaces (one crashlooping), the
      KubeVirt CRDs, a defined VM and an instance applied on its own:
      Namespace/Node/Definition columns off the edges, the opened row
      carrying references and every action, clicking a machine opening the
      machine, the node chip reaching the node, restart 200 with the VMI
      gone and 409 with the sentence for the undefined one. 154 tests.
      Live on dev while that cluster is up: http://dev.g8.lo:9094/ — the
      seed and the restart script are /build/cache/sc18/

Left open on #18: what is still not editable — cores, memory, disk bus and
network (#14), a disk added to a running machine, and the address the guest
actually holds, which needs the agent at `agent.sock` that nothing reads

### The batch filed 2026-09-22 (#13–#17) ✅ v0.10.0
Read stormvm's `docs/console.md` before writing any of the VM half: it is
the authority, and it says three things this repo was guessing at — the
replay is already served on attach, minting is loopback-only and exists
for `--require-token` nodes, and there is a whole set of **control verbs**
beside the doors that nothing here has ever called.

- [x] **#16 collapse the navigation.** A `kind` on `NavSection` —
      `work` or `admin` — declared by the plugin that contributes it, not
      a list in the SPA. Admin sections start shut; an explicit choice
      wins and persists. A collapsed section carries its total, so it
      stays discoverable
- [x] **#13 the console doors, the rest of them.** Mint a token and
      present it, so a `--require-token` node works; surface `replay` and
      say in the terminal where the history ends and the live stream
      begins; read-only as an explicit capability rather than a side
      effect of being able to see the VM
- [x] **#13/#14/#18 the control verbs.** `pause`, `unpause`,
      `softreboot`, `reset`, `freeze`, `thaw` — served by stormvm on every
      node, reported per machine (`control.lifecycle`, `control.freeze`),
      and called by nothing. This is most of what "a person can see a VM
      exists and cannot power it off" was asking for
- [x] **#14 settings, honestly.** An edit form that says per field
      whether it applies now or at next boot, and a machine that reports
      it has **pending changes** rather than silently diverging from its
      spec. Metrics: what is actually measurable today, and an honest
      absence where it is not (cadvisor is not wired here yet)
- [x] **#17 the network's view, inside the views people use.** The half
      that comes from CRDs the kubernetes plugin already watches —
      identity and what it resolves to, endpoint state, the policies that
      select a workload — on the pod and VM views. Flows stay gated on
      stormpump#11 (tracked on #4)
- [x] **#15 identity.** Step 1 landed in parallel (7408a47: argon2,
      roles, per-user SSH keys). Step 2 is the write gate — **one check,
      in the host, by method**, not per route, because the proxies are
      `any` and could never be enumerated

**Verified live on dev** against the seeded fastetcd + rustkube: the nav
collapsed to four open sections with totals on the shut ones; a settings
edit written to the definition, the row turning warn with `pending:
cores` and the page reading "Waiting for a restart. vCPU has been
changed"; refusals for a fractional vCPU, the network binding and the SSH
key; 409 for a machine with no definition; and with two users configured,
a reader getting 403 on both a settings PUT and a pod delete while an
operator got 200. Cilium seeded as CRDs: `datapath = ready` and
`regenerating`, `identity = app=web tier=frontend`, the right policy
selected and the `app=db` one not, and `addresses = 1 free of 20` warning
on the node. 172 tests.

### Events, and a dock that says what happened (v0.12.0) ✅ 2026-09-22
- [x] `ConsolePlugin::events(viewer, id)` — a contract, not a view. The
      host asks every plugin and takes the first that claims the id
- [x] **"Nothing happened" and "nothing records events for this" are
      different answers.** `Events { available, reason, items }`
- [x] `ResourceSpec::api_kind`, because an event is matched on
      `involvedObject` and a Service and a Deployment share names
- [x] A container's events are its pod's, narrowed by `fieldPath`
- [x] A machine's are merged across `VirtualMachine` and
      `VirtualMachineInstance` — one machine to whoever is looking
- [x] The **bottom dock**: what this console did (appended on the spot,
      because nothing upstream knows a button was pressed) over what the
      cluster did about it (polled). Shut, the bar still shows the last
      line
- [x] Verified live with seeded events: a VM's box showing
      `FailedScheduling ×14` beside `Started`, a crashing sidecar's
      `BackOff` reaching that container and not its healthy neighbour, a
      volume answering "the storage engine does not write any", and a
      Restart click landing at the top of the dock at 0s

### Disks, memory and where virtual machines live (v0.11.0) ✅ 2026-09-22
- [x] A virtual machine is a **workload**, next to Pods. A VM here is a
      kube object the kubelet reconciles, so running one is the same
      activity as running a pod; a navigator that separates them says
      otherwise. `item_at` exists because Workloads is built by two
      plugins now and `.item()` counts 0, 1, 2
- [x] Add and remove a disk — both halves together, whole arrays, and
      honest that the guest sees it at its next boot. The card merges the
      definition's disks with the instance's, because reading either
      alone states only half the truth: a disk added vanishes, or a disk
      removed vanishes while the guest still has it
- [x] The **memory floor**, the one decision governing whether memory can
      ever change without a restart, which the console could neither see
      nor set. stormvm builds the balloon from it already
- [x] Filed the two upstream gaps: **stormvm#18** (a device verb — qemu's
      `device_del` is a *request* the guest may ignore, chv's
      `vm.remove-device` is not, and the asymmetry is worth reporting
      rather than smoothing over) and **stormvm#19** (a memory resize
      verb — the balloon is built and nothing can move it)

**Left open, and why.** #14's metrics half — per-VM CPU, memory, disk and
network over time — needs cadvisor, which runs on nodes as a pallet and
is wired to nothing here; the container↔VM matching cannot be verified
without a node actually running machines, and a metrics client shipped
unverified is worse than none. #17's flows and per-module agent health
are gated on stormpump#11 (tracked on #4). #15's steps 3 and 4 — users
and groups manageable without editing a file on an immutable root,
certificate identity from stormcert, and an audit of consoles, deletes
and goldens — are their own pass

### The datastore: a fastetcd plugin (#20) ✅ v0.13.0 2026-09-25
fastetcd serves gRPC + `/health` on :2379 and Prometheus `/metrics` on
127.0.0.1:2381, and nothing else over HTTP. The owner's steer on #20 is
no gRPC client in the console: file what is missing on fastetcd. Filed
**fastetcd#28** (etcd's v3 JSON gateway — status, members, alarms,
range, compact/defrag/disarm/snapshot) and **fastetcd#29** (traffic,
watch and slow-watcher metrics).

- [x] `crates/plugins/fastetcd` (name `etcd`): `/metrics` parsed for
      revision, compact revision, DB size/in-use/quota, disk, NOSPACE,
      has-leader, leader changes; `/health` on the client port
- [x] The v3 gateway when it answers (etcd's own shape): status, member
      list, alarms — one component per member, the leader marked. When it
      does not, the store row says so and names fastetcd#28
- [x] Keyspace browser: `/api/plugins/etcd/keys?prefix=`, `/value?key=`
      decoded (JSON, k8s protobuf envelope type, text, hex); `#/etcd/keys`,
      read-only, admin only
- [x] Actions (admin, confirmed): compact, defrag, disarm, snapshot
- [x] Relation: the store `serves` `plugin:k8s`
- [x] Config `[fastetcd] enabled/url/metrics_url`, docs, changelog
- [x] Verified with `sc-build deploy/verify-etcd.sh` (2026-09-25): real
      etcd 3.5.17 — members/leader/raft, keyspace counts, three decodings,
      snapshot read back by `etcdutl`, NOSPACE raised by filling a 16 MB
      quota then cleared through the console (compact, defrag, disarm);
      real fastetcd v1.2.0 — metrics path, fastetcd#28 named, unreachable
      after kill. The script had never run before (py3.12 f-strings); the
      lock was missing the crate (#23)

Left for upstream: members, keyspace and verbs on *fastetcd* wait on
fastetcd#28; traffic (puts/txns per second, lagging watchers) on
fastetcd#29. The plugin already reads both shapes, so they light up
without a console change.

### A VM's addresses, asked against done (#24) ✅ v0.14.0 2026-09-25
rustkube-node now writes `status.interfaces[]` per NIC: `name`, `mac`,
`ipAddress`, `ipAddresses` (guest agent first, the node's ARP table as a
fallback) and `storm.io/binding` (`bridge` = tap on a real bridge, `user` =
SLIRP NAT inside qemu, `passt`). The spec says what was *asked*:
`networks[].pod` (+ interface `masquerade`/`bridge`/`passt`), `multus`,
or the `storm.io/bridge[.<iface>]` annotation, which wins (stormvm
`kube.rs`). Today `pod` renders as `user` (stormvm#16), so a spec saying
"pod" must never read as a working pod network.

- [x] `vm/src/network.rs`: one row per interface merging spec and status —
      asked (network + binding), did (binding), MAC, every address, and a
      reach verdict with a sentence (reachable / NAT, not reachable /
      no address yet / not reported yet / stopped)
- [x] List: every address on the row (`ip`), "no address yet" when running
      without one, NAT flagged
- [x] Detail: Network card as a table — interface, asked, node did, MAC,
      addresses, reach — with copy buttons
- [x] ResourceTable: a copy button on any metric whose value is IP
      addresses (generic, so FeedPlugin upstreams get it too)
- [x] Tests, docs, changelog; verify on dev; release; golden
- Verified with `sc-build deploy/verify-vm-net.sh`: real fastetcd v1.2.0
  + rustkube v0.14.1, KubeVirt CRDs, status written through `/status` as
  the kubelet writes it. NAT (asked pod) → `ip=10.155.0.15` warn,
  `network=NAT, not pod`, sentence naming stormvm#16; bridged → v4+v6,
  reachable on stormbr0; quiet → "no address yet"; stopped → stopped.
  The page's rendering was not viewed in a browser (no browser here); the
  bundle built and the API answers what it renders

### Phase 4 — fleet/nodes plugin
- [x] Node discovery from multicast presence — and the piece that was
      actually missing: the **address**. The collector had the datagram's
      source and used it only as a fallback name for unparseable lines, so
      a node that identified itself properly left nothing to dial.
      `LogEvent.addr` is always the sender; the host summary keeps the last
      one seen
- [x] Node detail: `#/node/<host>` probes that node's fifteen known ports
      concurrently and renders what answered — the daemon's own `system`
      card over the port layout's guess, with silence explained rather
      than reported as failure. **On demand, not aggregated**: twenty
      nodes' components in the pushed feed is thousands of rows nobody is
      looking at
- [x] `#/nodes` — the navigator's "Nodes" pointed at the plugin card, a
      page showing one row with a badge that said 1 however many nodes
      were on the segment
- [ ] Define the capability beacon — stormcos#26, still open
- [ ] Fleet actions per CLUSTER.md: join, promote, demote, drain —
      **blocked**, filed as stormcos#38. They are a CLI on the node
      (`stormcos join <endpoint> --token …`), with no HTTP surface; a
      button that cannot work is worse than no button. The transport to
      reach a node exists now, so what is missing is only something to
      call

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

2026-09-22: stormvm#18 (disk hotplug — no device verb beside the console
doors, and `Caps` carries no hotplug flag although `DESIGN.md` says it
should), stormvm#19 (memory resize — the balloon device is built from
`Memory::min` and there is no verb to move it).

2026-09-09: stormcos#38 (fleet lifecycle has no API — join/promote/demote/
drain are CLI-only, and the refusal a join can give is the interesting
answer, so it has to arrive as data a console can render), rustkube#59 (no SelfSubjectAccessReview / SelfSubjectRulesReview
— the RBAC engine decides correctly on every request but there is no way to
*ask*, so scoping the namespace list costs one probe per namespace per
viewer, and deciding whether to show an action before it 403s is
impossible), stormdrive#3 (bay and controller live only in the rendered
`detail` string, so a UI has to regex prose to place a drive in a
chassis). Still waiting: rustkube#55 + rustkube-node#34 (pod logs — and
with them a VM's serial, which the kubelet already writes to the pod log),
stormblock-registry#5 (raw media, and with it VM disk import), stormpump#11
(Cilium metrics, Hubble, relay), and stormvm's own console service.

2026-09-02: stormcast#1 (repetition collapse compares raw lines, so
tracing's leading timestamp defeats it — one looping service put 10,920
copies of one line on the fleet bus; same bug class stormconsole hit in
its own dedup and fixed in v0.7.1), rustkube#57 (store-unavailable
returns 500 with a raw Rust Debug dump; should be 503 with a concise
reason). fastetcd's ENOSPC → read-barrier deadlock is already covered by
fastetcd#14/#15 — not re-filed.

2026-08-30: stormpump#7 (re-enable the console in the image, #3 fixed),
stormd#2 (non-retryable exit codes; the console exits 78 for config errors).
Also stormpump#11 (Cilium agent metrics addr + Hubble/relay enablement) —
the image-side half of stormconsole#4 (full Cilium).
