# CLAUDE.md — stormconsole

The StormCOS web console — patterned on the OpenShift console, built in Rust
on stormd and stormview, with a pluggable architecture where each domain
(kubernetes via rustkube, fleet/nodes, logs, stormdrive, stormblock,
sbregistry) is a plugin contributing its own part.

**Hard rule: mkube is NOT part of this project and must never appear in its
design, code, or docs.** The orchestrator is rustkube + rustkube-node only.

## Version

Current: **0.32.0**

Version locations:
- `Cargo.toml` (workspace.package.version)
- `web/package.json`

## Key context

- Design: `docs/architecture.md`
- Build and test with `sc-build` after pushing (it builds the pushed commit
  on dev.g8.lo as `stormbuild` and deletes it); never on this VM, never as
  root. `web/dist/` is committed: rebuild it with
  `SC_BUILD_OUT=dist.tgz SC_BUILD_OUT_TO=tmp/dist.tgz sc-build 'cd web &&
  npm ci && npx vite build && tar czf ../dist.tgz dist'` and unpack it into
  `web/`.
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
- [ ] Pod logs — served upstream: rustkube v0.8.1 `pods/{name}/log`
      (rustkube#55), streamed by rustkube-node v0.3.0 (#34). Not shown here
      yet: there is no pod page (#12). A terminal waits on the kubelet
      serving exec (rustkube-node#56)

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

Upstream since delivered: fastetcd#28 (gateway, v1.8.0) and fastetcd#29
(traffic, v1.7.0). The plugin reads both shapes; checking it live against
such a fastetcd is #64.

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
  The page is walked in Chromium since #58 (`deploy/browser/vm.cjs` net)

### VM Backup tab: snapshots (#25) ✅ v0.15.0 2026-09-25
The objects are KubeVirt's `snapshot.kubevirt.io/v1beta1`
`VirtualMachineSnapshot` / `VirtualMachineRestore`; their status shape is
stormvm-spec `snapshot.rs` (`phase` InProgress/Succeeded/Failed,
`readyToUse`, `indications`, `conditions`, `error.message`,
`creationTime`, `virtualMachineSnapshotContentName` = stormblock group id).
Nothing acts on them yet: the controller is rustkube-node#53, the CRDs
stormpump#28, a real snapshot needs stormblock#130. The status carries no
step, disk list or size — file on stormvm, read them when present.

- [x] Watch both kinds (optional CRDs; "not installed" named, stormpump#28)
- [x] `vm/src/snapshots.rs`: rows (time, phase + step, disks, size, error,
      note, "not picked up" after a minute naming rustkube-node#53), the
      create body (source VM or VMI, name default `<vm>-<utc stamp>`,
      `storm.io/note`), the restore body and its refusals (running, not
      ready, no definition)
- [x] Routes as the viewer: list, create, delete, restore
- [x] UI: Backup tab — Snapshot (name/note), the list, Restore, Delete;
      polls while one is in progress; restores listed
- [x] File stormvm: step / disks / size on the status
- [x] Tests, docs, changelog; live check on dev; release; golden
- Verified with `sc-build deploy/verify-vm-snapshots.sh` (real fastetcd +
  rustkube, the node's status written as stormvm-spec shapes it): no CRDs
  → named; button → the KubeVirt object with source and note; bad name
  400, duplicate refused, unknown VM 404; InProgress "cloning disks…" →
  Succeeded with disks/size; Failed "failed while freezing: …"; restore
  409 running / 409 not ready / 200 stopped, VirtualMachineRestore as
  KubeVirt spells it, complete → "restored"; delete 200 and 404 through
  another machine; viewer 403, operator 200
- Found and filed **rustkube#100** (P1): a CR's DELETED watch event names
  the plural as its namespace, so a watching console never drops a deleted
  VMI or snapshot. The check runs the post-delete steps on a fresh console
- Filed **stormvm#45**: step / disks / size on the snapshot status

### SSH keys once, on every VM (#26) ✅ v0.16.0 2026-09-25
KubeVirt's `accessCredentials` names a Secret **in the VM's namespace**,
so: the user's list lives in Secret `<user>-ssh-keys` in a home namespace
(`[vm] ssh_keys_namespace`, default `default`), one key per data item,
labelled `storm.io/ssh-keys-for`; the console keeps a copy of it in each
namespace where the user creates or keys a VM. Nothing on a node honours
`accessCredentials` yet (stormvm#41), so the cloud-init seed keeps
carrying the keys too — that is what gets a key into a guest today.

- [x] `vm/src/keys.rs`: parse/validate a public key (type, blob, comment),
      Secret name for a user, data-item names, Secret body and reading,
      the create-time `accessCredentials` entry
- [x] Routes: `GET/POST /keys`, `DELETE /keys/{name}`, `GET
      /keys/choices` (for the form); home Secret written as the viewer,
      copies refreshed; config keys shown read-only
- [x] Field kind `checklist` with a `source` (console-core + CreateDialog)
      — the create form's per-key checkboxes
- [x] Create: selected keys → seed (every key, default user + root) and
      `accessCredentials` (`noCloud`) → the user's Secret copy when all
      are chosen, a `<vm>-ssh-keys` Secret for a subset
- [x] VM page: which keys it has (accessCredentials Secrets + the seed),
      "Add my keys" → `qemuGuestAgent` entry, honest about stormvm#41
- [x] Account → SSH keys page (paste or upload `.pub`, name, delete)
- [x] Tests, docs, changelog; live check on dev; release; golden
- Verified with `sc-build deploy/verify-vm-keys.sh` (real fastetcd +
  rustkube, real console with users, real ssh-keygen keys): private key
  and junk refused; named key and an authorized_keys file (duplicate
  skipped) saved to the home Secret with its labels; choices list 4 keys
  ticked; create in `web` → copy of the user's Secret + `web-1-ssh-keys`
  (config key), both `noCloud`, seed holding all 4 for default user and
  root, each accepted by `ssh-keygen -l`; a one-key subset → the
  machine's own Secret; a pasted key alone; no key → the warning; delete
  → home and the `web` copy both lose it; Add my keys → `qemuGuestAgent`
  entry, idempotent; a viewer 403 on save and delete
- Filed **rustkube#101** (stringData not folded into data); commented the
  shapes on **stormvm#41**

### Projects first (#28) ✅ v0.17.0 2026-09-25
rustkube v0.15.0 serves `project.openshift.io/v1` (rustkube#97): `projects`
(namespaces the caller has a RoleBinding in), `projectrequests` (creates the
namespace annotated `openshift.io/requester`, binds the requester `admin`),
and ClusterRoles `admin`/`edit`/`view`. Reserved: `default`, `openshift`,
`kube-*`, `openshift-*`.

- [x] System namespaces: rustkube's reserved set + `[kubernetes]
      system_namespaces` (default `["cilium"]`, the node's services)
- [x] k8s routes as the viewer: `GET/POST /projects`, `DELETE
      /projects/{p}`, members (RoleBindings to admin/edit/view: list, add,
      remove), isolation (`storm-isolate` NetworkPolicies: within the
      namespace only, DNS to kube-system opt-in; badge; remove). Fallback
      when `project.openshift.io` is not served: namespaces
- [x] Masthead: a **Project** selector (the viewer's projects; system ones
      only in an admin group) with New project
- [x] Every create targets a project: `Creator.namespaced`, a project
      picker in the dialog with New project inline (suggested
      `<user>-work`), templates take the chosen project; `/apply` and VM
      create refuse system namespaces (admin YAML excepted for `/apply`)
- [x] Lists: namespaced kinds always show Namespace; grouped by project
      when all are shown. Node daemons leave the nav (their pods are the
      mirror in kube-system); they stay on the node page
- [x] Cluster section (admin): Nodes, PVs, StorageClasses, CRDs,
      ClusterRoles, all Namespaces — new watched kinds
- [x] Namespace page = project page: requester, display name, members,
      isolation, delete
- [x] PVC Pending under `WaitForFirstConsumer`: idle, "provisioned when a
      pod or VM uses it", Attach to a VM
- [x] Tests, docs, changelog; live check (projects as two users); release
- Verified with `sc-build deploy/verify-projects.sh`: real fastetcd +
  rustkube v0.15.0 apiserver and controller-manager (TLS, anonymous off,
  signed tokens), console users alice/bob (operator) and root (admin),
  each with their own kube identity. alice: no projects, `alice-work`
  suggested; system and bad names refused; created with requester alice
  and her admin binding; bob cannot see it (404). Creates: no project →
  refused, `?project=` → 201, `default` → the system sentence, bob's
  namespace → hidden, VM with none/default/cilium refused, in alice-work
  201; root into kube-system 201. bob made view → sees it at once; his
  pod create, isolate and delete are the apiserver's 403s. Isolation with
  and without DNS, the two policies exactly, kube-system refused, removed.
  A 600Gi claim: no project refused; in alice-work Idle, "provisioned
  when a pod or VM uses it", Attach → vm1 has the claim as a volume.
  Rows carry their project; sc/crd/crole in the feed; nav Home+Projects,
  Cluster section, no Node services. Delete default 403, alice-work 200 →
  Terminating
- Filed **rustkube#102** (a claim's phase is not defaulted)
- Walked in Chromium since #58 (`deploy/browser/`)

### Machines page, served by stormipmi (#31) ✅ v0.18.0 2026-09-25
stormipmi v0.4.0 (stormipmi#12) serves the Machines API on :9097:
`GET /api/v1/machines[?test=true]` (`{machines, default, forge}`), power
by tag (`on|off|soft|reboot|cycle`), `PUT …/release` (read back), `GET|PUT
…/intent` (501 until stormblock#148), `PUT …/test`, `POST …/adopt`, `GET|PUT
/api/v1/machines/default`, `GET /api/v1/releases`, a `machine:<tag>` feed,
and the SOL console at `WS /api/v1/hosts/{ns}/{name}/console/serial`
(replay, then live; `?token=`). Reads are open; writes take the bearer
from `api.tokenFile`; admin-only is the console's to enforce.

- [x] `crates/plugins/stormipmi` (name `ipmi`): the feed; `/proxy` with
      reads open and every write `admin` only, the bearer added
      server-side; `/console/{ns}/{name}` relaying SOL (typing admin only)
- [x] Config `[stormipmi] enabled/url/token_file`; nav Hardware → Machines
- [x] `#/machines`: by service tag — BMC, power (confirmed), the release
      each boots (set, confirmed, read back), boot intent (501 said), the
      default image, test marks, new hosts to adopt, SOL console
- [x] Live check: stormipmi's own smoke (fastetcd, apiserver, ipmi_sim,
      stand-in forge) + a console with an admin and an operator
- [x] Docs, changelog, release, golden
- Verified with `sc-build deploy/verify-machines.sh`: stormipmi v0.4.0
  built from its tag on its own rig (fastetcd, rustkube v0.15.0, ipmi_sim,
  stand-in forge), restarted with an `api.tokenFile` (a direct write →
  401). Through the console: ops/admin `me`; the fleet (SIMBOARD0001 on,
  NEWBOX1 to adopt, default 10.1), releases 10.2/10.1; readyz and a
  `..%2F` traversal 404; the feed with proxied power actions. ops refused
  (403) on power, release, test, adopt; admin: soft-off → off, on → on,
  NEWBOX1 power 409, release 10.2 read back, 9.9 404, default → 10.2,
  intent 501 as stormipmi words it, test mark → ?test=true lists it,
  adopt 201 with no password in the API. SOL through the console: both
  viewers got the replay and live output; admin's typing echoed, ops'
  dropped. An audit line per act
- Walked in Chromium since #58 (`deploy/browser/`)

### Images are the registry's, Volumes are what is attached (#19) ✅ v0.19.0 2026-09-25
Owner's scope: a UI point of view only — goldens stay engine volumes. The
data: stormblock v18.1.0 (#138) puts `kind` (volume|golden|blank|media|
snapshot|template), `in_use`, `attachments` and `consumer` on every volume
(and `?kind=`/`?in_use=` filters); sbregistry v0.23.0 (#34/#43) serves
`/v1/catalog/images` (kinds component|blank|media|golden|base|slab_golden|
release_part|sealed; `source`, `digest`, `parent`, `releases`, `clones`,
`clone_names`, `location`, `state`) and `/v1/media/jobs` (phase, source,
percent, fault, golden).

- [x] stormblock plugin: the engine's `kind` first (the old sealed/parent
      rule for an older engine); Volumes = kind volume and in use, with
      consumer and attachment; Unattached volumes apart; image kinds out
      of the Volumes view; Delete disabled while in use
- [x] sbregistry plugin: the catalog as `reg:cat:<name>` (kind, base
      lineage, clones, releases, source, digest, location, sizes); media
      jobs merged in ("downloading from X, n%", failed with its fault);
      an older registry said to predate the catalog
- [x] `#/images`: grouped by kind, lineage and clone counts; nav Images →
      Catalog, and Volumes / Unattached under Storage
- [x] Live check: stormblock v18.1.0 on file-backed disks (templates,
      clones, a claim attached with an owner) + sbregistry v0.23.0 on it,
      and read-only against forge's real engine; docs; release; golden
- Verified with `sc-build deploy/verify-images.sh`: stormblock v18.1.0
  (release build, file-backed raid1) and sbregistry v0.23.0 from their
  tags. The engine's filters agree with the console: Volumes = the claim,
  consumer `PersistentVolumeClaim shop/db` linked to `k8s:pvc:shop/db`,
  `nvme-tcp nsid 2`, Delete disabled (and the engine's 409 through the
  proxy); Unattached = seed and idle-clone ("attached: nothing"); the
  template's blank only under the engine card. Catalog: `pvc-256M` blank,
  2 clones, its engine volume; a held fetch of a missing URL → a row
  "failed (upstream): … 404". Nav: Storage Volumes/Unattached, Images
  Catalog/Pushed/Pallets. forge's real engine (pre-v18), read-only through
  a console alone: 92 volumes (89 clones), 777 images, and the card says
  the split is missing
- Found: a v18 engine answers 401 to reads without its token →
  `[stormblock] token_file`; filed **stormcos#94** to wire it on nodes
- Walked in Chromium since #58 (`deploy/browser/`)

### Drives at rack scale (#32) ✅ v0.20.0 2026-09-26
Target: 160 drives a node, ~1,600 a rack. Data today: stormdrive's feed
per node (bay, hba, temp, wear, spare, smart, capacity, serial, dev,
shelf edge; shelves with PSU/fans/temp); per-drive usage is stormdrive#12
(open) — until then usage comes from this node's engine: slabs name their
drive (stormblock#136), array members their device path and state
(`rebuilding`, `degraded`, `failed`). Rack: a node label.

- [x] Drives plugin, fleet-wide: this node's stormdrive as before
      (`drive:` ids) plus every fleet node's (`drive:@<host>:`, discovered
      from the log hosts' addresses at :9092) and `[stormdrive] nodes`;
      per-node proxies; each drive and shelf says its node
- [x] stormblock: per-drive usage (`sb:use:<serial>`) and array member
      state (`sb:member:<path>`)
- [x] k8s node `rack` from label `topology.storm.io/rack`
- [x] `web/src/lib/drivemap.js` (pure): join, filters (failing, degraded,
      rebuilding, full, out of fleet, spare), group by chassis/node/rack,
      heat colour by health/temperature/wear/usage, totals in PB/EB; a
      node test at 10×160
- [x] DrivesView: chassis map by bay + list, totals, legend, selection
- [x] Live check: 10 synthetic stormdrive feeds (1,600 drives) + a
      synthetic engine; docs; release; golden
- Verified with `sc-build deploy/verify-drives.sh`: ten stand-in
  stormdrives (160 drives each, four 40-bay shelves), a stand-in engine
  for this node, a real rustkube with rack-labelled Nodes, a real console.
  1,600 drives from 10 nodes, 160 each, remote ids `drive:@storm-N:`; the
  card "1600 drives on 10 nodes (1 without stormdrive)"; 8 drive-use and 3
  array-member joins; racks A/B. drivemap over the live feed: 40 chassis,
  11.4 PB raw, 27.4 TB used of 58.4 TB in slabs, failing → storm-3 bay 17,
  rebuilding → /dev/sd21 here, full → here-SN000; 17 ms. Locate through
  storm-3's proxy reached storm-3 only; this node's through the local
  proxy; an unknown node 404. A node killed → 1,440 drives, 9 nodes.
  Feed 1.2 MB in 38 ms (3.2 s before the id fix)
- Walked in Chromium since #58 (`deploy/browser/`)

### Docs from the code (#21) ✅ v0.20.1 2026-09-26
Pattern: stormbootx b1347d9. Facts gathered from the code (config.rs,
main/server/auth, every plugin, the SPA router) and the other components'
code for every port and API referenced.

- [x] README.md rewritten from the code: what it is and does today, build
      (sc-build, never root), every config key with its default, flags,
      ports, health/metrics endpoints, auth and roles, plugins and their
      upstreams, how it ships (stormcos service golden)
- [x] docs/architecture.md: stale removed or corrected, design marked
      where the code does not do it yet
- [x] Crate doc comments that no longer match
- [x] Cross-references checked against the other components' code
- [x] Doc promises the code does not keep → issues; close with the list
- Found in the code on the way, and fixed: a token sign-in was a no-role
  session; bearer compared with `==`; `/api/version` behind auth and a
  phantom `/metrics` on the open list; a VM waiting for its instance said
  nothing places one (rustkube does, #72); "Delete golden" called a route
  the operator does not serve (now the CloudImage via the apiserver); the
  node page probed three ports that serve no feed and missed six that do
- Filed: stormcos#102 (golden health path), #35 (registry credential),
  #36 (scale/cordon/drain the docs promised)
- Verified: `sc-build` (tests), `sc-build deploy/verify-auth.sh`

### A presentation (#22) ✅ 2026-09-26
`docs/presentation.md`, Marp, 8–15 slides, every claim from the code and
the #21 docs. stormcentral's graph: depends_on stormview, rustkube,
stormrfb, stormd; the code also reads stormblock, stormdrive,
stormstorage, sbregistry, fastetcd, stormvm, vmcloud-image-operator,
stormipmi and stormcast — shown, and filed on stormcentral.

- [x] The deck; rendered with marp-cli through sc-build
- [x] README links it; changelog; close; golden
- Rendered with `npx @marp-team/marp-cli@4` through sc-build: 11 slides,
  no unrendered fences. Filed stormcentral#39 (depends_on is 4 of 13)

### Test containers (#27) ✅ v0.21.0 2026-09-27
stormcentral `docs/test-standard.md`: one image from `test/Containerfile`
(repo root context), `/test short|medium|long`, `test/build.sh` builds the
static binary on the build box, JSON lines + exit 0/1/2, everything in the
run's namespace, labelled `storm.io/test-run`, no machine assumptions.
Pattern: stormipmi's `test/`.

- [x] `test/` crate (own workspace): env, report, apiserver + console
      clients, websocket
- [x] short: health, version, SPA, feed, nav, and the main job — a Service
      made through the apiserver appears in the console's feed and leaves
- [x] medium: open/closed surface, plugin cards, creators, websocket push,
      apply into the project, object YAML, edit + 409, delete through the
      console, refusals (no project, system namespace, bad YAML, proxy)
- [x] long: waves of Services sized from the node's pod capacity —
      propagation latency, feed size/latency, residue per wave
- [x] `test/stormconsole-test.yaml` (Job, RBAC, requires), Containerfile,
      build.sh; docs; a harness on dev against a real console + rustkube
- Verified with `sc-build deploy/verify-tests.sh` (real fastetcd +
  rustkube v0.15.0 + console): short 8/8, medium 24/24, long 2 waves
  (30 then 45 Services: shown in 255/1523 ms, dropped in 506/2026 ms,
  feed 1 ms, residue 0), 0 left in every run's namespace; no console →
  skip exit 0; runner missing STORM_API → exit 2; auth on, no token →
  skip; with the token → 8/8; podman built the image (6 MB, `/test`,
  labels). The medium suite found the console answering an unknown API
  path with the app's HTML (fixed: 404)
- Filed **rustkube#113**: a Service create is ~1.5 s against 0.05 s for
  a ConfigMap, serializes, and slows as Services accumulate — the long
  suite paces its creates (5 in flight) and times the console apart

### Docs refresh from the code (2026-09-27) ✅
Since 2026-09-18 the code changed only in `server.rs` after the #21 pass
(unknown `/api/*` and `/ws/*` answer JSON 404). Config keys, defaults and
ports checked against `config.rs` again: all match. What is stale is the
**upstream**: pod logs are served (rustkube v0.8.1, rustkube-node v0.3.0),
rustkube#100 is fixed (v0.15.2), rustkube#59 is served (v0.9.0),
stormblock-registry#5 shipped (v0.19.0), stormpump#11 closed. Owner's
description: PVCs are the built-in `stormblock` driver (the node's kubelet
through the engine), CSI only for other classes.

- [x] README, architecture, presentation, CLAUDE.md: the above corrected;
      the 404 behaviour and `/metrics` (#41); claims via the built-in driver
- [x] Doc promises the code does not keep → issues: #40 (stale docs,
      closed by this), #44 (VM disk import is unblocked), #45 (use
      SelfSubjectAccessReview; RBAC-aware actions), #42 (keys home)
- [x] Changelog; sc-build (exit 0, 4f80ff7); close #40

### Lifecycle from every phase (#37) ✅ 2026-09-28
Filed from stormvm#21: Restart/Stop were enabled only from `Running`, so a
machine whose start failed, or stuck in `Scheduling`, could not be
restarted from its row — exactly when a restart is wanted.

- [x] Row actions: Restart, Stop and Start from every phase. Stop disabled
      only when already stopped, Start only when running. A bare instance
      (no definition) keeps Restart/Start disabled with the reason; its
      Stop is the delete it always was
- [x] Handlers made true to that: Start from a failed/stuck instance
      replaces it (running=true + the instance deleted); Restart sets
      running=true before deleting the instance, so it also recovers a
      definition wanting nothing; Stop sets running=false and deletes a
      lingering instance
- [x] A defined machine's Stop goes through the definition (it deleted the
      instance, which the definition then put back)
- [x] `status.message` beside Failed on the row (reason and message both)
- [x] Tests, changelog, docs; sc-build; live check
- [x] Golden: the first request aborted on the platform (stormcentral#135);
      shipped in golden-stormconsole-e3677bb239e7 (v0.22.0, stormcos#157)
- Verified with `sc-build` (all tests) and `sc-build
  deploy/verify-vm-lifecycle.sh` (real fastetcd v1.2.0 + rustkube v0.15.3,
  the kubelet's status written through `/status`): 24/24 — a Failed
  machine offers Start/Restart/Stop with reason and message on the row;
  Restart and Start from Failed write running=true and delete the dead
  instance; Running has Start off, Stop → running=false + instance gone;
  a stopped definition offers Start/Restart not Stop, Restart brings it
  up; stuck Scheduling restarts; a bare Failed instance refuses
  Start/Restart 409 with the sentence, its Stop (danger) deletes it

### Each drive's usage, slabs and volumes (#29) ✅ v0.22.0 2026-09-28
Data, read from the code (2026-09-28): stormdrive 0.15.0 (#12/#13, golden
`golden-stormdrive-01a422df544f`) serialises per drive in `GET
/api/v1/drives`: `usage` (capacity, in_slabs, used, free_in_slabs,
outside_slabs, free, promisable, committed?, headroom?, `slabs[]` with id,
role, tier, total/allocated/free/committed), `overcommit {enabled, ratio}`,
`drain {state, moved, failed, remaining, reason, then_leave}`; the feed
carries `bay` and `hba` (#3). stormblock ≥ v17.1.0: `GET
/api/v1/volumes?placement=true` → per volume `placement.drives[]` (drive
serial/wwn, slabs, legs, bytes), `placement.slabs[]` (state, drain),
`rebuild`; `GET /api/v1/slabs/pool` (pressure, by tier).

- [x] drive plugin: `GET /api/plugins/drive/usage` — every node's
      `/api/v1/drives` fetched on demand (cached 10 s), reduced to raw
      bytes per drive: usage, slabs, overcommit, drain; unreachable nodes
      named
- [x] sb plugin: `GET /api/plugins/sb/placement` — this node's volumes by
      drive serial (volume, kind, consumer, bytes, legs, shared, slab state,
      rebuild); `GET /api/plugins/sb/pool`
- [x] drivemap.js: usage from stormdrive for every node (the engine's
      `sb:use` only as a fallback for an older stormdrive), `draining`
      filter, pools per node × role × tier
- [x] DrivesView: the picked drive — location incl. controller, usage bar,
      overcommit/committed/headroom, its slabs with their use, its volumes
      with their consumer, drain progress; `#/pools`
- [x] Tests (Rust + drivemap), docs, changelog; live check with stand-ins;
      release; golden
- Verified with `sc-build deploy/verify-drives.sh` (0 failed): ten stand-in
  stormdrives serving `/api/v1/drives` in v0.15.0's serialised shape
  (storm-7 as a pre-usage stormdrive), a stand-in engine with volumes and
  placement in stormblock's shape, a real rustkube and a real console. 1,600
  drives in bytes from 10 nodes; storm-x named; storm-7 "reports no usage";
  storm-4 drive 5 at 95% → full; storm-2 drive 9 draining (30 left);
  slabs with committed where reported; both answers cached (3 more reads →
  no new node read, no new placement walk); here-SN000 → vm-web-root
  (VirtualMachine web/web-1, 100 legs shared) then pvc-db (PVC shop/db),
  here-SN001's leg draining; pools 9 nodes × 2 tiers, no headroom claimed
  where committed is partial, may-promise per drive's ratio; model 24 ms;
  storm-9 killed → named. `drivemap.test.mjs` PASS; 284 Rust tests; the
  `--locked` musl release build
- Walked in Chromium since #58 (`deploy/browser/`)
- Volumes on *another* node's drives are not read (this node's engine only)

### Create: "+ New project…" can be named, and nothing breaks (#56)
Owner on C2NR0Q2 (golden-stormconsole-b5667ebc5e42): Create VM offers New
project but it cannot be named, and the dialog breaks the whole console.
- [x] `deploy/verify-create-project.sh`: real fastetcd + rustkube + console,
      the SPA built from the commit, driven by headless Chromium
      (Playwright): open Create VM, New project, type, create — twice; fail
      on any page error. Run first on unfixed main to reproduce
- [x] Fix `CreateDialog.svelte`: defaults set once per opening, never
      overwriting a choice; effects that do not re-trigger themselves
- [x] web/dist rebuilt; changelog, docs; sc-build; close; golden
- Cause: the reset effect wrote `lists`/`values` and then read them, so it
  re-ran itself until `effect_update_depth_exceeded` — thrown on opening
  Create VM (the only creator with a checklist), after which nothing on
  the page updated. Reproduced on unfixed main by the browser check (the
  VM was created, the dialog never said so, the next click timed out)
- Verified with `sc-build deploy/verify-create-project.sh`: no projects →
  starts on New project with `alice-vms`, kept 4 s, created (namespace
  requester alice, vm1 in it); one project → New project chosen, stays,
  `alice-lab` kept, vm2 created there; Projects lists both; the dialog
  opens and cancels again; no page errors

### Docs refresh from the code (2026-10-02) ✅
Since 2026-09-25, code changed after the 2026-09-28 pass only in #56 (the
Create dialog, documented with it). Stale was upstream and the platform,
collected in #61: fastetcd serves the v3 gateway (v1.8.0) and traffic
counters (v1.7.0); stormcos#94 is done; the node config path is
`/etc/stormconsole/stormconsole.toml`; the default stormd ports missed
9201/9202/8180/8545 (fixed in code with the node page's layout, e289d2d,
sc-build 284 tests).
- [x] README, architecture, presentation, changelog
- [x] Filed #64: the datastore page against a fastetcd ≥ v1.8.0
- [x] The health path: `/admin/healthz` is the app's 200, so the golden's
      probe always passes (stormcos#102, stormcentral#226 — not ours)

### Pod and VM detail: network, image, metadata, traffic, logs (#69)
Owner (2026-10-02): full network info, image + sha + last checked + build
date, all metadata, traffic counters, logs in the UI with the last 5 runs.
Read from the code (2026-10-02): the kubelet writes `image`, `imageID`
(a digest under containerd, the image string again under stormpump),
`restartCount`, `state` (terminated without reason), `podIPs`, `hostIP`;
no `lastState`, no resolved time, no OCI labels, no MTU/routes/CNI
(filed **rustkube-node#130**, **#131**). pods/log forwards `container,
previous, tailLines, timestamps, follow, limitBytes`; `previous` is only
the run before. Counters: the kubelet's `/metrics/cadvisor` on :10250
(rx/tx bytes per pod interface), bearer checked by TokenReview — rustkube
has no node proxy (rustkube#108), so the console dials the kubelet at the
node's address. Golden provenance: stormcentral `GET /api/v1/goldens
?component=` (authenticated). VMs: no tap counters (stormvm#48), serial
replay already on the Serial console tab.
- [x] k8s `pod.rs`: `GET /pods/{ns}/{name}` — metadata, owner chain,
      QoS, priority, SA, conditions, containers (image, digest, pull
      policy, ports, restarts, state, lastState when present), network
      (IPs, hostNetwork, DNS, Services selecting it + endpoints, Cilium
      identity/policies), gaps named
- [x] Logs: `GET /pods/{ns}/{name}/log` (as the viewer, streamed when
      following); the console keeps the last 5 runs per container
      (`previous` fetched when restartCount moves), `GET …/runs`
- [x] Traffic: `GET /pods/{ns}/{name}/traffic` from the node's kubelet
- [x] Golden provenance for `stormpump://` via optional `[stormcentral]`
- [x] `#/pod/:ns/:name`: Overview, Network, Logs, Events, YAML; pod rows
      link to it; VM page: metadata card, image card, traffic gap named
- [x] Tests, docs, changelog; live check; release; golden
- Verified with `sc-build deploy/verify-pod-page.sh` (real fastetcd v1.2.0
  + rustkube v0.15.3 apiserver, a stand-in kubelet on :10250 serving
  containerLogs and /metrics/cadvisor, a stand-in stormcentral, a real
  console with its SPA): 33 API checks and 29 in headless Chromium, no
  page errors; screenshots read. Owners ReplicaSet → Deployment, digests
  from imageID, stormpump without one and its gap named, the cilium
  golden with built_at, Service web (not db) with this pod ready, both
  addresses, DNS; tailLines, timestamps, follow streaming, previous 400 in
  the node's words then the run before, download filename; restarts
  0 → 1 → 4 kept runs 0 and 3 with 2 missed, a missed run 404; traffic
  for this pod only, growing, drawn as a rate; Pause/Resume, search,
  kept runs selectable, the stormpump container's logs, YAML
- Not on a real blade: the owner's acceptance (cilium's pod on 11.6x) is
  the first look at a real kubelet's answers — the stand-ins follow
  rustkube-node's code as read on 2026-10-02
- Left on #12: Terminal (rustkube-node#56), Environment

### Registry images and instances (#70)
Owner: the console calls goldens "registry images"; the CoW clone a
workload runs on is its "instance". UI words only — APIs keep `golden`.
- [x] Backend prose reworded; details built from a kind through
      `console_core::words::term`
- [x] SPA: `ui/words.js` `term()`/`shown()`/`sameRelation()` on kinds,
      metric labels, the kind metric's value, relation names, stormview
      cards; visible text reworded; the pod page's Registry image row and
      its one tooltip naming "golden"
- [x] Tests, dist, docs, changelog; sc-build; browser check; release; golden
- Verified with `sc-build deploy/verify-pod-page.sh` (its words pass): a
  stand-in sbregistry, engine and image operator whose names avoid the
  word and whose kinds and fields keep it, a real console; Chromium read
  the visible text, every title/aria-label/placeholder/option, the opened
  navigator, opened rows and the Create menu on the overview, Registry
  images, the registry's registry-image and instance lists, VM registry
  images, the VM catalogue, volumes, the engine, a card view, a `rel=`
  link in the page's word, and the pod page: no "golden" anywhere except
  the pod page's one tooltip. 29/29 words checks, 305 Rust tests
- Data keeps its names: an object stormcentral or forge named
  `golden-<component>…` is shown as named (catalog rows); the pod page
  shows `component@commit` and puts stormcentral's name in the tooltip

### Cluster page: stormcluster's feed and its operations (#63) ✅ v0.25.0 2026-10-03
stormcluster (stormcluster#1) runs on every node, :9102, and serves a
stormview feed: `system` (the cluster or this SNO), `member:<node>`,
`peer:<node>`, `op:<id>`, each card with its actions as body-less POSTs
(`/api/v1/peers/{n}/form|join?role=`, `/api/v1/members/{n}/promote|demote|
drain|uncordon|split[?keepData=false]`, `/api/v1/operations/{id}/resume`).
`POST /api/v1/operations` takes `{"op":"form"|"join"|…}`; `?dryRun=true`
on all of them answers `{"plan":{"steps":[…],"warnings":[…]}}`; a refusal
is `409 {"refused":[…]}`; a request forwarded to the coordinator comes back
as `{"coordinator", "response"}`. Writes take a bearer when stormcluster's
`token_file` is set. Read from stormcluster 61777dd (docs/api.md, feed.rs,
http.rs, plan.rs).
- [x] `crates/plugins/stormcluster` (name `cluster`): the feed (3 s), a
      proxy limited to the operator API (not `/record`), writes `admin`
      only with the bearer added server-side; answers normalised — a
      forwarded answer unwrapped with its coordinator named, a refusal
      also carried as `error` so a generic row button says the reasons,
      plan steps given a `description` (stormcluster's `describe` wording)
      when the plan does not carry one
- [x] Config `[stormcluster] enabled/url/token_file` (default this node's
      :9102); nav Cluster → Membership `#/cluster`
- [x] `#/cluster`: the cluster, then members, peers, operations (steps of
      each); every action but Resume previews its plan first (steps,
      warnings) and runs on confirm; Split asks keep/wipe data; refusals as
      a list; Form (name, masters 1/3/5, workers) and Join (as worker, or
      as master in pairs) as forms building the operations body
- [x] Tests, docs, changelog; live check against a real stormcluster on
      dev; release; golden
- Verified with `sc-build deploy/verify-cluster.sh` (57 checks, 0 failed):
  three real stormclusters at 61777dd (b1–b3 on 127.0.0.11–13, private
  group 239.255.42.63:25563), stand-in node API and fastetcd gateway, a
  console with admin and ops. Proxy: ops 403 even on a dry run; `/record`
  and non-API paths 404; two masters 409 with the reason as `error`; an
  unknown node named; Form planned in stormcluster's words; a form seeded
  on b2 planned by b2 and named; a console without the token gets 401 in
  words. Chromium: SNO and peers; Form (two masters refused on the page,
  plan, Run, b1 member with the cluster CA, 4/4 steps done); Join b2 →
  failed at the enrollment (no stormcert), failed step shown, Resume;
  with a three-member record: Promote one → refused with its reason,
  Promote a pair planned, Split keep → wipe re-planned, Cancel runs
  nothing, Drain run → failed at cordon (no apiserver); ops sees all,
  offered nothing; no page errors. Demoting the seed refused with why
- Filed **stormcluster#11** (plan steps carry no description)
- Shipped in v0.25.0, golden-stormconsole-84db69059bc5 (stormcos#157)
- Not run against a real node API (stormcos#38) or a real apiserver: a
  form/join/split actually changing a node is stormcluster's to verify

### Destructive storage: storage-admins only, typed confirm, as the user (#82) ✅ v0.26.0 2026-10-05
stormcos#250 ships ClusterRoles `storage-admin` (every verb on
`storage.storm.io`) and `storage-viewer` (get/list/watch); the components'
own checks are stormdrive#45, stormraid#8, stormblock#274 (all open). The
engine already calls these verbs destructive (`serve/api.rs`
`is_destructive`: every DELETE, PUT forge, seal, tar, files, gc, trim
apply, fsck repair) and wants its admin token for them.
- [x] `console_core::storage`: one rule, `classify(method, path, query)` →
      resource + verb in `storage.storm.io`, over the drive (format,
      sanitize, wipe, partition, destructive test, worker jobs), engine
      (the engine's own list + array/slab create) and stormstorage proxies;
      `Reviewer` asks a SelfSubjectAccessReview **as the viewer** (cached
      30 s per token), fails closed (no identity, no apiserver, 404)
- [x] Host middleware: a classified request needs the review's yes (else
      403 with the reason) and `X-Storm-Confirm` equal to the object's
      name — the serial for a drive, the label otherwise (else 428 naming
      it); then the proxy forwards the **viewer's** bearer, never the
      console's or the engine token
- [x] Feed: classified actions stripped for anyone the review refuses;
      `GET /api/v1/console/guard?method=&path=` for the SPA
- [x] SPA: a typed confirm (type the serial) before any guarded action
- [x] Tests, docs, changelog; `deploy/verify-storage-guard.sh` (real
      fastetcd + rustkube with the storage roles, stand-in stormdrive and
      engine recording the bearer, Chromium); release; golden
- Verified with `sc-build deploy/verify-storage-guard.sh` (0 failed): real
  fastetcd v1.2.0 + rustkube v0.15.3 carrying the release's storage roles;
  alice (operator, storage-admin) sees Format/Destructive test/Delete, bob
  (operator, storage-viewer) the same drives and volume with Locate only;
  bob 403 with the reason on format, forge, slab destroy, RAID create,
  nothing reaching a component; carol (reader + storage-admin) 403; the
  console's own token 403 (no kubernetes identity); alice 428 without the
  serial and with `sdb`, 200 with `ZC1234` — stormdrive, storm-b's
  stormdrive and the engine got **alice's** bearer, ordinary writes still
  the node token; root (system:masters) allowed; audit and refusal lines;
  Chromium: OK then the typed prompt naming ZC1234, wrong word stops it,
  serial formats once; bob offered no Format; a binding removed → Format
  gone and 403 within 30 s
- Shipped in golden-stormconsole-1977f1e1aa93 (stormcos#157)
- Waiting on the components to check the bearer themselves: stormdrive#45,
  stormraid#8, stormblock#274 — until #274 the engine refuses a user's
  bearer, so a volume delete in the console stops there

### fastetcd over mutual TLS (#47) ✅ v0.27.0 2026-10-05
stormcos moves fastetcd's :2379 to mTLS with a stormcert pair (stormcos#81,
docs/SECURITY.md); the console's plain client would lose the etcd page.
- [x] `[fastetcd] ca_file`, `cert_file`, `key_file`: the client trusts only
      that CA (no built-in roots) and presents the pair; cert and key come
      together, and TLS files with an `http://` url are a config error (78)
- [x] Rebuilt when a file changes (stormcert renews); a file missing or
      unreadable is the etcd card's error naming it, not a crash loop
- [x] Tests; docs, example config, changelog; `deploy/verify-etcd-tls.sh`
      against a real fastetcd serving mTLS (openssl CA, server + client
      pairs): verified, refused without the pair, wrong CA named, reload;
      release; golden
- Verified with `sc-build deploy/verify-etcd-tls.sh` (0 failed): fastetcd
  v1.13.0 built from its tag, `--client-cert-auth` with an openssl node CA
  (curl: no answer without a pair, none in plaintext). stormcos's shape →
  ok with its member, the gateway, the keyspace and a value; CA only →
  `CertificateRequired`; stranger CA → `UnknownIssuer`; http:// → error; a
  pair missing → the file named, then minted → ok with no restart; the pair
  swapped for an untrusted one → error, renewed → ok; http url + ca_file
  and half a pair → exit 78. `verify-projects.sh` passes with h2 on
- Shipped in golden-stormconsole-fdf1206f5d6f (stormcos#157)
- Found only live: tonic's ALPN is `h2` only (reqwest needed `http2`); one
  level of error chain hid the TLS cause; a shut client port read healthy

### A machine outside policy says so (#51) ✅ v0.27.1 2026-10-06
A VM whose node binding is `user` (NAT inside the hypervisor, stormvm#16)
or a host `bridge` is not a Cilium endpoint: no NetworkPolicy and no
project isolation reaches it. The console said "isolated" anyway.
- [x] One rule (`plugin_kubernetes::network::outside_policy`): outside when
      the binding is `user` or `bridge`, or, where Cilium's endpoints are
      watched, when there is no endpoint under the machine's `ns/name`
- [x] VM row: `policy` metric "none applies (NAT|host bridge)", no
      endpoint reference for such a machine
- [x] VM page: the Network card says no policy or isolation applies, that
      the project is isolated and this machine is outside it, and which
      policies would select it on the pod network
- [x] Isolate answer and project card: "… Except N machines — … —", with
      each one's reason (`outside` on `GET /projects/{p}`)
- [x] Tests, docs, changelog; live check; release; golden
- Verified with `sc-build deploy/verify-vm-policy.sh` (0 failed): real
  fastetcd v1.2.0 + rustkube v0.15.3, KubeVirt and Cilium CRDs, machines'
  status written as rustkube-node writes it; Chromium on the project and
  VM pages, no page errors
- Shipped in golden-stormconsole-e25614720d2f (stormcos#324)
- Left: a passt machine with no endpoint is named in the project's
  exceptions, but its own row/page say nothing (the vm plugin does not
  see Cilium's endpoints); moot until a node runs passt (stormvm#16)

### A network edit that shows (#50) ✅ v0.27.2 2026-10-06
The save writes `storm.io/bridge` on the definition's template; the form's
value and the pending check read only `spec.networks`/`interfaces`, so a
saved edit read back as `pod` and never went pending — "nothing happened".
- [x] `settings::network` reads what stormvm reads (`network::asked`:
      `storm.io/bridge.<iface>`, then `storm.io/bridge`, then the network),
      on the template + object for the definition, the VMI's own for the run
- [x] Save: clears a per-interface `storm.io/bridge.<iface>` that would
      shadow the write; reads the definition back and refuses to call it
      saved when it does not carry the value; the answer says what was
      written and that `spec.networks` is left as it was
- [x] A saved edit is written through to the cache (`Store::observe`), so
      the page's re-read after Save does not race the watch
- [x] Found live: a stopped machine read as pending on every field (the
      run side read from no instance) — fixed
- [x] Tests; `deploy/verify-vm-network-edit.sh`; docs, changelog; release
- Verified with `sc-build deploy/verify-vm-network-edit.sh` (0 failed):
  real fastetcd v1.2.0 + rustkube v0.15.3. test1 (the owner's spec,
  stopped): pod → stormbr0 held by the apiserver as the template
  annotation with `spec.networks` unchanged, the form reading stormbr0 at
  once, the Network card "host bridge stormbr0", back to pod removes it;
  web (running): pending network on the form and the row; pinned
  (`storm.io/bridge.default: br9`): cleared and replaced; Chromium did the
  edit through the Settings tab with no page errors
- Shipped in golden-stormconsole-013795cd465d (stormcos#324)
- So the owner's test1 edit most likely *did* save: the annotation was
  written and nothing read it back

### The apiserver with a token file and a CA (#33) ✅ v0.28.0 2026-10-06
stormcert#27 mints the console's ServiceAccount token into a file that is
renewed in place; stormcos mounts it and the node CA (stormcos#76). The
console took only an inline `token` and verified nothing for the default
server.
- [x] `console_core::apiserver::Conn`: server, bearer (inline or a file
      re-read when its mtime moves), a client trusting only `ca_file`
      (rebuilt when it changes; unreadable → no roots, fail closed, said)
- [x] `[kubernetes] token_file`, `ca_file`; `token`+`token_file`,
      `ca_file`+`insecure_skip_tls_verify`, `ca_file` with http → exit 78;
      the loopback default without a CA warns at start, and the card says
      "certificate not verified" (health unchanged)
- [x] One Conn for every apiserver caller: k8s client/watches, namespace
      probes, vm, vmimages, the storage Reviewer, the release read, the
      kubelet hop; the `/version` probe through it, with the bearer
- [x] Found live: the probe sent no bearer (401 with anonymous off); a
      probe failure said only "client error (Connect)"
- [x] Tests; `deploy/verify-kube-tls.sh`; docs, example config, changelog
- Verified with `sc-build deploy/verify-kube-tls.sh` (0 failed): fastetcd
  v1.2.0 + rustkube v0.15.3 serving TLS from an openssl CA, anonymous off,
  SA-signed tokens. CA + token file → ok, 23/23 kinds, k8s and VM objects;
  a stranger CA and the system roots → `invalid peer certificate:
  UnknownIssuer`, nothing read; a CA not yet there → named, then picked up
  with no restart; an expired token → nothing, renewed in place →
  recovered; skip-verify → works and says so; contradictions exit 78
- Shipped in golden-stormconsole-0a5296dcd21e (stormcos#324)
- RBAC for `kube-system/stormconsole` and the mounts: stormcos#76; the
  resource list it needs: #78

### stormcluster over TLS (#89) ✅ v0.29.0 2026-10-06
stormcluster#5: :9102 is TLS only; plain answers /healthz, everything else
needs a node-CA client certificate or the bearer. No stormcluster has both
TLS and the HTTP writes (#12 landed first), so `verify-cluster.sh` stays
pinned at 61777dd for #63's write flows (#88 moves it to the objects).
- [x] `console_core::tls` (fastetcd's #47 client, section-named messages,
      an HTTP/1.1-only variant, fail-closed before its files build)
- [x] `[stormcluster] ca_file/cert_file/key_file`; https default with a CA;
      78 on half a pair or http; refreshed before every poll and proxy call
- [x] Feed says the upstream's refusal and the whole connect cause
- [x] Found live: stormcluster offers h2 in ALPN and drops an h2 client
      (filed stormcluster#30); the console speaks HTTP/1.1 to it
- [x] `deploy/verify-cluster-tls.sh` against stormcluster 55b69da: 0 failed
- [x] Release; golden; close — golden-stormconsole-743d467dee9c (stormcos#324)

### Cluster page on cluster.storm.io objects (#88) ✅ v0.30.0 2026-10-07
stormcluster#12: the lifecycle API is `cluster.storm.io/v1alpha1` `Cluster`
and `ClusterMember` (cluster-scoped, `status` subresource, finalizer
`cluster.storm.io/split`), reconciled by the seed (on an SNO, by itself on
its own apiserver). :9102 is read-only plus `POST /api/v1/plan` →
`{plan, descriptions[]}` / `409 {refused[]}`. A release always wipes (#14;
`keepData` is refused); a failed operation resumes by itself (no Resume).
Read from stormcluster 139c712 (docs/api.md, reconcile.rs, feed.rs).
- [x] Plugin watches both kinds through the apiserver connection (optional
      CRDs, "not installed" named); `GET /objects`
- [x] Writes as the viewer through the apiserver, no admin gate (RBAC):
      form (members first, then the `Cluster`), join, role, drain,
      storage, release (delete member), dissolve (delete cluster)
- [x] `POST /plan` → stormcluster's dry run, `descriptions[]` on the steps;
      the proxy is read-only plus plan; the copied `describe` goes
- [x] Page: previews every change with the plan; objects' status — phase,
      message, blockers as the refusal list, operation step/error,
      suggested names; no Resume, no keep-data
- [x] `verify-cluster.sh` on objects: three stormclusters at main with TLS,
      a real rustkube they reconcile against, stand-in node API; Chromium
- [x] Docs, changelog; release; golden
- Verified with `sc-build deploy/verify-cluster.sh` (0 failed): three
  stormclusters at 139c712 over TLS, b1 reconciling a real rustkube v0.15.3
  (TLS, anonymous off) that it installed the CRDs on. Plans in
  stormcluster's descriptions (equal to its own `/api/v1/plan`); two
  masters refused with every reason; a reader stopped by the write gate,
  alice (no RBAC) by the apiserver with nothing written; root formed storm
  in Chromium → `ClusterMember b1` + `Cluster storm`, phase Forming → Ready,
  the page became the cluster; Join b2 → its row shows Joining, then Failed
  at the enrollment (no stormcert) with the error; Release b2 behind the
  typed name → deleting; no page errors
- Shipped in golden-stormconsole-325df3f882a1 (stormcos#324), with #39's
  comment fix; the first golden attempt failed `--locked` (#106): the lock
  lacked the plugin's new dependencies, and the version bump's sed had
  rewritten tungstenite 0.29.0 → 0.30.0 — bump only the workspace's own
  `[[package]]` entries

### The console's own bearer from a per-node file (#102) ✅ v0.31.0 2026-10-07
stormcos#200 (P1): the console ships with no `[api] auth_token` and no
users, so :9094 is open as admin. A token in the golden would be one secret
shared by every node and readable on forge, so stormcos mints one per node
at boot (`stormcert-agent sa-token` into tier-0) and points a file at it.
- [x] `[api] auth_token_file`: the bearer from a file, re-read when its
      mtime moves; with it set, auth is on even while the file is missing
      — closed (401 on every non-open route), the reason logged once per
      change, open the moment it appears; not with `auth_token` (78)
- [x] One source for the bearer: viewer, middleware, login
- [x] Tests; `deploy/verify-auth.sh` (or a new one): missing → closed,
      minted → bearer and login work, rotated → old refused, new works,
      no restart; 78; docs, example config, changelog; release; golden
- Verified with `sc-build deploy/verify-auth-file.sh` (0 failed): no
  file → health only, feed/write/empty bearer/sign-in 401, the reason
  logged naming the file, no "open" warning; minted → bearer 200, a
  session opened with it is the token's and writes past the gate; re-minted
  → new 200, old 401 and cannot sign in; removed → 401; with `auth_token`
  too → exit 78. No restarts
- Shipped in golden-stormconsole-a413c4a70737 (stormcos#324)

### stormstorage's api_token on the proxy (#53) ✅ v0.32.0 2026-10-07
stormstorage#6: with `[api] api_token` set, every write but the placement
dry run and self-registration needs `Authorization: Bearer <token>`; the
feed's actions (Publish/Republish, Assemble, Move, Delete) went through the
proxy with none and got 401.
- [x] `FeedPlugin::bearer`: a feed upstream's token, added server-side by
      its proxy (the browser's own is never forwarded); for stormstorage,
      `[stormstorage] token_file`, read at start like stormblock's (#30)
- [x] Destructive storage (a stormstorage DELETE) keeps #82's rule: the
      viewer's bearer — so a tokened stormstorage refuses it until it takes
      a user's bearer (stormstorage#58)
- [x] Tests; `deploy/verify-storage-token.sh` against a real stormstorage
      with a token; docs, example config, changelog; release; golden
- Verified with `sc-build deploy/verify-storage-token.sh` (0 failed): a
  real stormstorage at ec5ac39 with `[api] api_token`; with `token_file`,
  Publish and Move through the proxy answer as stormstorage does to the
  token (404 for no such volume), the token never in the console's log; a
  DELETE stopped by the storage guard, not sent with it; without it, or
  with the browser sending the token, stormstorage's 401; an unreadable
  file warned at start
- Shipped in golden-stormconsole-89fa6be3c1aa (stormcos#324)

### Every page in a browser (#58) ✅ 2026-10-07
Eight pages were checked only through their APIs ("not viewed in a
browser"); #56 showed Chromium runs on dev through sc-build and that a
browser finds what an API check cannot.
- [x] `deploy/browser/`: `lib.cjs` (launch, sign in, fail on any page or
      console error), `run.sh` (Playwright once per run), one walk per area
- [x] Each rig builds the SPA from the commit and ends with its walk:
      drives + pools (`verify-drives.sh`), machines incl. power, release and
      SOL (`verify-machines.sh`), images catalog / volumes / unattached
      (`verify-images.sh`), projects: selector, members, isolation
      (`verify-projects.sh`), VM network / keys / backup / lifecycle rows
      (`verify-vm-{net,keys,snapshots,lifecycle}.sh`)
- [x] Fix whatever they find; drop "not viewed in a browser" from the docs
- [x] Golden: none needed — tests and docs only, nothing in the binary changed
- Verified: all eight rigs on fresh build VMs (`SC_BUILD_VM=1`; dev.g8.lo
  is retired), every walk 0 failed, no page errors. The walks found no page
  bug; what they caught was in the checks (CSS upper-cases headings, so
  matching is case-blind; bare-1 is deleted by its own rig) and on the
  fresh VM (no kubectl, no Python websockets — the rig brings its own)

### Cluster progress, node by node (#84)
Owner on stormcluster#1: "a cluster progress screen as they reconfigure".
stormcluster's `GET /api/v1/operations/{id}` gives `request` (op + its
nodes) and `steps[{step: {step, node?, …}, description, status, error,
note, startedAt, finishedAt}]` — grouping by node needs nothing new
upstream (read from stormcluster cd478e0, exec.rs/plan.rs).
- [ ] `web/src/lib/progress.js` (pure): every node the request names or a
      step names, in order, with its steps, status (pending / running /
      done / failed) and current step; node-less steps as "the cluster";
      a node test
- [ ] Cluster page: after a write, the progress view opens on the
      operation it starts (the first new `op:` card); any operation opens it
      from the list; polled while running; closes on demand
- [ ] `verify-cluster.sh`'s browser walk: Form → b1's steps done; Join →
      b2 failed at the join token with its error; tests, docs, changelog;
      release; golden (stormcentral#521 permitting)

### A flowsdn plugin (#83)
flowsdn#297: the agent serves a read-only HTTP/1.1 listener on the node's
loopback, `http://127.0.0.1:9878` (both edition manifests; in golden
eda35249a55e and later): `GET /v1/endpoint[/{id}[/healthz]]`, `/v1/ipam`,
`/v1/healthz`, `/v1/health/modules`, `/v1/config`, `GET|POST
/v1/statedb/query` (health table only); Kubernetes mode adds `/v1/ip`,
`/v1/identity`, `/v1/service`, `/v1/node/routes` (404 otherwise). One
request per connection, `Connection: close`, 2 s budget; writes 403. IPAM
counts are decimal strings. Endpoint rows carry `status.pod` (namespace,
pod_name, node_name, workloads, containers), `status.pod-networks`,
`status.identity` once allocated (#291). Shapes: flowsdn docs/agent-api.md
and `crates/flowsdn-agent/src/{api,health_api}.rs` (read 8fa0cc8). Loopback
only, so each node's console shows its own agent. No Hubble (flowsdn#293).
- [ ] `crates/plugins/flowsdn` (name `flowsdn`): poll every 5 s into a cache
      that keeps the last good answer per route when the agent is down;
      edition from `/etc/stormcos/release/manifest.json` (`edition`/`network`,
      else its components) — a cilium node says "not this edition" and has
      no nav item
- [ ] Feed: the agent (health from healthz + modules; a module "not
      implemented" is idle, not a warning), one row per endpoint led by
      ns/pod with node + namespace as `belongs_to` (columns), state, IPv4,
      IPv6, identity; one per IPAM pool (u128 counts, warn under 10% free)
- [ ] Access: endpoints and services in namespaces the viewer may not see
      are withheld (feed and routes), as the vm plugin does
- [ ] Routes: `/snapshot`, `/endpoint/{id}` (live, + healthz), `/state/{table}`
      (allowlist: health)
- [ ] `#/flowsdn`: Endpoints (node/namespace filters, ✓/· marks), IPAM,
      Health (per module), Config, Services, Routes, State, Flows (says
      flowsdn#293)
- [ ] Config `[flowsdn] enabled/url/release_manifest`; tests; docs,
      changelog; `deploy/verify-flowsdn.sh` (stand-in agent in the agent's
      shapes and transport, a real console, Chromium); release; golden

### Node health: each API's state and latency, alert on a stall (#123) ✅ 2026-10-10
stormcos#458: stormd (#49) probes each process's declared APIs; PID 1
(stormpump#127) probes its own units' and merges every container's stormd
state file (stormd#52) into `/run/stormpump/health.json` — `{updated,
worst, apis:[{source stormpump|stormd, container?, process, api, url,
state healthy|slow|stalled|down|unknown, since, running, last_ms, p50_ms,
p99_ms, budget_p50_ms, budget_p99_ms, last_error, last_check, checks,
interval_secs?, file_age_secs?, stale?, reported_state?}]}`, rewritten every
5 s. Each change is a line in `/system-data/history/api/<process>.jsonl`
(`ts, process, api, url, from, to, from_secs, latency_ms, p50_ms, p99_ms,
error`). stormd also serves `GET /api/v1/health/apis` → `{items}`. Read
from stormd b30c7b9 (apihealth.rs) and stormpump 272470b (apihealth.rs,
healthd.rs, README § API health). The console's unit binds neither
`/run/stormpump` nor system-data yet: stormcos's to do (filed).
- [x] `crates/plugins/apihealth` (name `health`): every 5 s, the summary
      file; when it is unreadable, each local stormd's
      `/api/v1/health/apis` (the fleet ports), saying which source; a
      summary not rewritten for 30 s is said stale. History: the last
      changes per API from `history_dir`
- [x] Feed: the node (worst), one row per API (state, last/p50/p99 against
      budget, since, error); stalled/down are errors naming service, probe
      and how long
- [x] Routes `/snapshot`, `/history?process=&api=`
- [x] `#/health` page (Compute → API health) and the local node page's
      section; an alert bar on every page for each stalled/down API
- [x] Config `[health] enabled/summary_file/history_dir`; tests; docs,
      changelog; `deploy/verify-api-health.sh` (a real stormd probing a
      stand-in that stalls, a summary file, a real console, Chromium);
      release; golden
- Verified with `sc-build deploy/verify-api-health.sh` (0 failed, 71201ab):
  a real stormd (b30c7b9) probing a stand-in service every second (2 s
  timeout, p99 200 ms) and writing its state file; a console with no
  summary bound in reads the stormd — healthy, then STALLED "for Ns — no
  answer within 2 s" (node row in error, the alert bar on the overview and
  the nodes page, its link opening the API with its changes), slow with
  last/p50/p99 against the budget, DOWN "HTTP 500", healthy again; a 401
  stormd named; history narrowed and newest first, "not mounted" said. A
  console on PID 1's summary (a stand-in merge in stormpump's shape over the
  real state file): the engine's own probe stalled first, the container's
  stormd merged; the stormd frozen → its entry stalled with what it last
  said; the merge stopped → "summary has not been rewritten". Chromium: no
  page errors, no bar once healthy. Unit tests 13/13; clippy clean on the
  crate
- Found on the way: main did not compile (flowsdn `Module` derived
  `Default` over `Health`, from #83) — fixed in 00f842e; workspace clippy
  debt and no clippy gate → #133
- Left: the console's unit binds neither `/run/stormpump` nor `/system-data`
  (stormcos#525); until then a node shows each stormd's view, without PID
  1's own probes, and no history

### Pod page at kubectl-describe parity, with stats over time (#124)
Owner 2026-10-08: "events for itself, and all the stats … upstream
guidance" (describe + top; pod-lifecycle; debug-pods). Most of describe is
here since #69 (conditions, per-container state/lastState/restarts, images
and goldens, owners, labels/annotations, per-container logs with current/
previous/kept runs). Read from rustkube-node 7bfe4d2: `/stats/summary`
serves per container `cpu.usageCoreNanoSeconds` and
`memory.workingSetBytes`, and per pod `network` (bytes, packets, errors,
drops per interface). No per-container rss/page faults/rootfs/logs, no pod
volume[] or ephemeral-storage: filed **rustkube-node#242**, read when
present. The kubelet writes Unhealthy/Killing/BackOff/Started events, and
`ready`/`started` per container.
- [ ] Stats sampler (`kubernetes/src/stats.rs`): every 15 s each node's
      kubelet `/stats/summary` (console credential, the apiserver conn's
      client), one ring per pod (1 h); `GET /pods/{ns}/{name}/stats` as the
      viewer's pod — CPU cores (rate), memory, every reported field, network
      rates; what the node does not report named (rustkube-node#242)
- [ ] Probes per container: configured (startup/liveness/readiness: type,
      target, delay, period, timeout, thresholds, k8s defaults) and last
      observed (ready/started + the latest `Unhealthy` event for that
      container and probe)
- [ ] Volumes: each `spec.volumes` with its type and source, where it is
      mounted (container, path, ro), a claim's phase, size, class and bound
      PV; used bytes when the kubelet reports them
- [ ] Events: first and last seen, count, source, container (fieldPath);
      the Events tab a full table refreshed every 5 s
- [ ] UI: Containers card adds Ready, Started, requests/limits; Probes and
      Volumes cards; a **Stats** tab — charts over 15 m / 1 h with current
      values per container (CPU, memory) and per interface (rx/tx bytes,
      packets, errors, drops)
- [ ] Tests; docs, changelog; `deploy/verify-pod-page.sh` extended (stand-in
      kubelet serving /stats/summary in rustkube-node's shape, Unhealthy
      events, probes, volumes, claims) + Chromium; release; golden

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

### Phase 5 — storage & images plugins ✅
- [x] stormdrive plugin: every node's :9092 (v0.20.0, #32), usage per drive
      (v0.22.0, #29)
- [x] stormblock plugin: volumes/slabs/arrays/exports/drives (v0.4.0),
      Volumes vs Images (v0.19.0, #19), placement per drive (v0.22.0)
- [x] sbregistry plugin: goldens, clones, pallets, images (v0.4.0), the
      catalog and media jobs (v0.19.0)

### Later
- [ ] Dynamic remote plugins (manifest + reverse proxy, OpenShift
      dynamic-plugin style / stormd `[process.ui]` style)
- [x] YAML edit/apply views for rustkube resources (v0.8.0: `/apply`, and
      `GET|PUT /api/plugins/k8s/object/{kind}/{key}` on every watched kind)
- [ ] RBAC-aware UI — rustkube serves the reviews since v0.9.0 (#45)

## Cross-project issues filed

Tracked in `docs/architecture.md` §Integration gaps. File with `gh issue
create` on the owning repo; never fix in this repo (Core Rule 11).

2026-09-27: cadvisor#15 (per-VM stats keyed to the VMI, for #14),
stormvm#31 comment (a snapshot schedule). Here: #40 (stale docs), #41
(`/metrics` answers HTML), #42 (Decide: SSH keys home), #43 (Decide: the
rack label), #44 (VM disk import), #45 (access reviews).

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
chassis). Since delivered: rustkube#55 + rustkube-node#34 (pod logs,
#12), stormblock-registry#5 (media import, #44), stormpump#11 (Hubble
relay, #4), rustkube#59 (access reviews, #45) and stormvm's console
service (v0.10.0).

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
