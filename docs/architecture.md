# stormconsole — Architecture

The StormCOS console, patterned on the OpenShift console: one place to see
and operate a cluster — workloads, nodes, storage, images, logs — but built
the storm way: a single static Rust binary under stormd, rendering
everything through the stormview contract, with a pluggable architecture in
which every domain is a plugin that contributes its own part.

**Scope rule: mkube is not part of this system.** The kubernetes side is
rustkube (Rust kube-apiserver/controller-manager/scheduler over fastetcd)
and rustkube-node (kubelet/kube-proxy/CNI). Nothing in this console talks
to, references, or borrows from mkube.

## What the console is (and is not)

stormcos's `docs/CLUSTER.md` sets the bar: the console is **a view of real
nodes running real services**. Every check is against the running thing —
storage health is the engine's own readiness, not a database row. Its
actions are day-2 actions: **join, promote, demote, drain, replace a
disk**. It is not an installer; a console that is only an installer is
abandoned the day the cluster exists.

The OpenShift console is the pattern for *shape*: left navigation grouped
by domain, a namespace (project) selector scoping the workload pages, list
pages with filters, detail pages with tabs (Details / YAML / Events /
Logs), an Overview that shows cluster health at a glance, and a plugin
mechanism so components ship their own console surface instead of the
console knowing every component.

## Foundations

### stormd — the runtime

stormconsole ships as one static musl binary in a `FROM stormdbase` scratch
container, supervised by stormd. stormd provides the init, SSH, per-process
logging, liveness probing, and the OCI updater; the console provides a
`summary` endpoint so its card in stormd's own dashboard is live. The
console's web server listens on **:9094** (9080 stormd, 9090 stormblock,
9092 stormdrive, 6443 rustkube, 10250 kubelet are taken).

### stormview — the contract and the UI system

Everything the console shows is, wherever possible, a
`ComponentSummary` — `{id, kind, label, health, detail, metrics, actions,
relations, link}` — the shape defined by the stormview Rust crate and
rendered by the stormview npm package (themes, `DataGrid`,
`ComponentCard`, `ComponentGrid`, `RelationPicker`, `HealthDot`,
`LoginPanel`). Two consequences:

- **The console is a stormview consumer**: daemons that already serve
  `/api/v1/components` (stormd today; stormdrive and sbregistry once their
  issues land) appear in the console with no per-daemon UI work.
- **The console is a stormview producer**: it aggregates every plugin's
  components into its own `/api/v1/components` + `/ws/components` feed, so
  stormsh's TUI, stormd, or any other stormview renderer can show the whole
  cluster through the console. The UIs cannot drift because none of them
  owns the model.

Component ids are namespaced by plugin to keep the aggregate feed
collision-free: `k8s:pod:default/web`, `fleet:node:storm-a1`,
`drive:nvme-eui.0025...`, `reg:golden:img-ab12cd34ef56`.

## Pluggable architecture

The core knows nothing about kubernetes, drives, or images. Every domain is
a **console plugin**:

```rust
#[async_trait]
pub trait ConsolePlugin: Send + Sync {
    /// Stable short name; prefixes component ids and API mount point.
    fn name(&self) -> &'static str;
    /// Nav contribution: sections and items (label, hash route, order).
    fn nav(&self) -> Vec<NavSection>;
    /// API routes, mounted at /api/plugins/{name}/…
    fn routes(&self) -> axum::Router<AppState>;
    /// This plugin's slice of the aggregated components feed.
    async fn components(&self) -> Vec<ComponentSummary>;
    /// Plugin's own health — surfaces as a component and in /readyz.
    async fn health(&self) -> Health;
    /// Background work (watches, multicast listeners, pollers).
    async fn run(&self, shutdown: CancellationToken);
}
```

The host (console-core) provides:

- **Registry** — plugins are registered at startup from config; a disabled
  plugin simply isn't constructed. Compiled-in plugins now; the trait is
  the seam where dynamically registered remote plugins attach later.
- **Aggregated feed** — concatenation of every plugin's `components()`,
  cached, pushed as full snapshots on `/ws/components` exactly like stormd.
- **Nav feed** — `GET /api/v1/console/nav` returns the merged navigation;
  the SPA renders whatever it is given, so a new plugin appears in the nav
  with no frontend change.
- **Proxy helpers** — authenticated reverse-proxy plumbing so plugins can
  expose upstream daemons (a node's stormdrive, rustkube) through the
  console origin: `/api/plugins/{name}/proxy/…`. The browser only ever
  talks to the console; upstream credentials stay server-side.

Later, **remote plugins** (OpenShift dynamic-plugin style, stormd
`[process.ui]` style): a service registers a manifest (name, nav items,
upstream URL, optional components URL); the core proxies its UI under the
console origin and merges its components feed. That makes the console
extensible by components the console has never heard of — same philosophy
as stormview's open `kind`.

### Frontend model

Svelte 5 + Vite, `stormview` npm package, embedded in the binary
(rust-embed) like stormd's SPA — no node at runtime. The app shell owns:
hash router, login (stormview `LoginPanel`, session cookies), theme picker,
nav rendered from the nav feed, and a **namespace selector** in the
masthead (OpenShift's project selector; the selection scopes namespaced
views and persists per browser).

Most pages are *generic*: list pages are a `ResourceTable` (or a
`ComponentCard` grid) over a feed slice, detail pages are relation
navigation. Plugins earn custom views only where generic rendering isn't
enough — the log viewer, the YAML editor, node topology.

#### The design layer: two axes

`web/src/lib/ui/console.css` is the console's chrome, layered on top of
stormview's palette and loaded after it. There are two independent axes,
and neither constrains the other:

| axis | attribute | owner | what it controls |
|---|---|---|---|
| **theme** | `data-theme` | stormview | the palette — twelve of them |
| **style** | `data-style` | this file | the chrome — two of them |

A style is not a colour scheme. It is proportion and structure: how tall
the masthead is, how tight a table row is, how square a corner is, how
dense the navigator is, whether a button shouts. Both styles therefore
work on all twelve palettes, and switching palette never changes the
console's shape. Both selectors live in the masthead and persist per
browser; `initStyle()` runs before mount so the first paint is already in
the chosen style.

- **`openshift`** (default) — the OpenShift console. Comfortable density
  (8px rows, 236px navigator, 22px page title), a near-black masthead
  over a panel-coloured navigator, a 3px accent rail on the active nav
  item, 4px radii, sentence case throughout, rows separated by hairlines.
- **`esxi`** — the ESXi host client (VMware Clarity). Compact density (5px
  rows, 212px navigator, 18px page title, 40px header), a dark teal
  header, 2px radii, zebra-striped tables, uppercase action labels. The
  whole navigator tree fits on one screen, which is the point.

Everything density-dependent reads from a token (`--sc-row-py`,
`--sc-nav-py`, `--sc-nav-font`, `--sc-gutter`, `--sc-zebra`,
`--sc-btn-case`, …), so a component's scoped CSS never has to know which
style is active.

Both styles put a **dark bar at the top whatever the palette below it** —
which is what both references do in light mode too. The masthead
therefore carries its own foreground tokens (`--sc-masthead-fg`,
`--sc-masthead-dim`, `--sc-masthead-line`) instead of inheriting the
theme's, and lifts the palette's state colours toward white; otherwise a
light theme paints dark green on near-black. Its `--sc-masthead` is
blended a little toward `--panel` so it still belongs to the palette it
sits on.

Beyond the two styles, the layer supplies the page grammar every view
shares (`.sc-page`, `.sc-crumbs`, `.sc-pagehead`, `.sc-toolbar`,
`.sc-empty`, `.sc-seg`, `.sc-status`), tabular numerals, focus rings,
reduced motion and scrollbars. Concretely: every view opens the same way
— breadcrumb, then title with scope and count, then a toolbar (search,
state filter, table/card switch, result count), then the data — and every
empty screen names what is missing, says why in one line, and carries the
action that fixes it.

#### stormview components, and the console's own

The console renders `ComponentCard` from stormview directly. Cards are
mounted inside a `.sc-cards` wrapper: stormview styles its components with
scoped rules of one class plus one element, so selecting through a wrapper
class out-specifies them and the console can retune shape without forking
stormview.

Tables are the exception. `ResourceTable` (`web/src/lib/components/`) is
the console's own, because a console needs things a shared grid should not
assume:

- **Status, not health.** The table says "Ready", "Degraded", "Failed";
  the feed's `ok`/`warn`/`error` is a wire value, not a word for an
  operator. `StatusPill` carries a glyph as well as a colour, so state
  survives colour blindness and greyscale.
- **Kind is conditional.** A column that reads `k8s-pod` seventeen times
  on a pod list is noise, so Kind appears only when the rows differ.
- **A header that stays put.** stormview's `DataGrid` sets a sticky header
  inside an `overflow-x` wrapper, where it can never fire; the console's
  table bounds its own height so the header actually sticks.
- **Name first, destructive actions last** and right-aligned.

Everything else — nested relation expansion, multi-select with bulk
lifecycle actions, sorting — matches `ComponentGrid`'s behaviour.

## Built-in plugins

### kubernetes (rustkube)

Talks to the rustkube apiserver (`https://…:6443`) with a bearer token or
client cert from config. rustkube is kube-wire-compatible (core v1,
apps/v1, batch/v1, RBAC, CRDs, watch streams with bookmarks), so the client
is a thin typed layer over the standard REST paths.

- **Watch-backed cache**: list+watch on namespaces, nodes, pods,
  deployments, replicasets, statefulsets, daemonsets, jobs, cronjobs,
  services, PVCs, events. The cache serves the UI instantly and emits the
  plugin's components slice (pods and workloads become components with
  `belongs_to` namespace edges, `has_many` pod edges, health derived from
  status/conditions).
- **Namespace views**: the selector scopes every namespaced page; a
  namespace detail page shows its workloads, events, and resource counts —
  the OpenShift project dashboard.
- **Actions**: delete pod (danger), scale via deployment update (rustkube
  has no `/scale` subresource yet — update the spec directly), cordon/
  uncordon and drain via eviction API.
- **Cilium**: the agent's API is a unix socket and Hubble is gRPC, neither
  reachable from a golden, so Cilium is read through its CRDs on the
  apiserver — `cilium.io/v2` endpoints (state, address, identity, edge to
  the pod), nodes, identities, network policies (selector and rule counts,
  DELETE) — plus core NetworkPolicy, under one `k8s:cilium` card. CRD kinds
  are optional in the watch cache: a 404 is "not installed", synced and
  empty, re-checked each minute. Hubble flows are the next step and need
  a relay the console can reach.
- **Pod logs**: rustkube today has **no** `/log` subresource and
  rustkube-node has **no** `/containerLogs` endpoint, although the node
  writes CRI log files under `/var/log/pods/…`. Issues are filed (below).
  Until they land the pod detail page links to fleet logs filtered to the
  pod's node/host, which the logs plugin serves.

### logs

The fleet already emits: stormcast sends RFC 5424 over UDP to multicast
`239.255.42.1:5514` from initramfs onward, and there is no production
collector. The logs plugin **is** the collector:

- joins the multicast group, parses RFC 5424 (stormcast dialect: severity
  inference already done at the emitter), stores into a **deduplicating
  redb ring**;
- query API patterned on mcastsyslog's proven shape:
  `GET /api/plugins/logs/events?host=&min_severity=&last=&search=`,
  `…/around?at=&window=`, `…/summary`, and SSE `…/stream` for follow;
- the viewer UI: severity/host/search filters, live follow, and deep links
  every other plugin can target (`#/logs?host=storm-a1`).

Per-entity logs stay at their source: a node's stormd serves its own
process logs (`:9080/api/v1/logs`), reachable through the fleet plugin's
node proxy — the console does not re-store what a node already stores.

#### The ring

Three things shape the store, and all three came from watching a real node
misbehave — sptest emitted one identical line thousands of times a second
while its own ring returned `SQLITE_FULL` on a filesystem with 1.7 TB
free, and the flood made the log view unusable in the browser.

**redb, not SQLite.** Pure Rust, no C toolchain in the golden, and no page
ceiling to hit while the disk is empty. The trade is that there is no
`GROUP BY`, so the per-host and per-severity summaries the components feed
asks for every few seconds are *maintained* by insert and prune rather
than computed by scanning.

**Dedup on arrival.** An entry is keyed by what an operator would call the
same message — host, app, severity, text — and a repeat bumps a `count`
and a last-seen time instead of appending. A flood of one line costs one
entry; the viewer renders it as `×N`, and a lifetime `duplicates` counter
on the collector component shows how much is being absorbed. `[logs]
dedup = false` stores every arrival separately.

**Two automatic bounds.** An entry is dropped when it falls outside the
retention window (`retain_hours`, measured from *last* seen, so a line
that keeps arriving keeps its place) or when the ring exceeds `ring_cap`
distinct entries. A timer sweeps as well as inserts, so a quiet fleet
still expires what it left behind.

Ordering is receive order, not the wire timestamp — emitters disagree
about clocks — and a repeat re-inserts at a fresh sequence number, which
keeps sequence order and last-seen order identical. That is what lets
pruning stop at the first live entry instead of walking the whole ring.

Two things follow from dedup that are easy to miss. The live tail
*updates* a row the viewer already holds rather than appending one, so a
chattering line is one row and a rising number instead of a thousand DOM
nodes; and repeats are throttled to at most one broadcast per second per
line, so a flood is not a flood on the wire and in every open viewer.
Finally, a store error is deliberately **not** logged per failure: the
console's own warnings go out over this same multicast group, so a broken
ring that logs every failure floods the fleet it is meant to observe. The
fault surfaces as component health instead.

The ring changed format at v0.7.0 and therefore changed filename
(`logs.db` → `logs.redb`); the old SQLite file is inert and can be
deleted.

### fleet (nodes)

There is no fleet-inventory service, by design: **nodes announce themselves
by existing** on the multicast group. The fleet plugin's node list is the
log collector's host list (a `LogHosts` handle shared from the logs
plugin), with recency as health — heard in the last two minutes is ok,
ten is a warning, longer is an error — and each node card links to the
logs filtered to it.

This node's **services** are its stormd instances, discovered by probing
the StormCOS port layout on loopback (control plane 9081–9085; node
services at port + 100: stormdrive 9192, stormstorage 9193, console 9194).
Each one's own stormview feed is folded in under `fleet:svc:<name>` —
the system card (as kind `service`) and its processes, with start/stop/
restart carried through `/api/plugins/fleet/proxy/{port}/…`. Mounts and
log cards are filtered out; sixty of them per service is noise on an
overview.

CLUSTER.md calls for a **small periodic capability beacon** alongside the
logs (cores, memory, drives, pallets booted, join state) rather than an
inventory protocol. The beacon is the console's to define — proposed as an
issue on stormcos (below); until it exists, capabilities come from per-node
API calls after discovery. Drilling into *another* node's services, and
the fleet actions (join, promote, demote, drain), are the remaining phase 4
work.

### stormdrive and stormstorage — feed plugins

Both daemons serve the stormview components feed themselves (stormconsole#1),
so each is a `FeedPlugin`: poll `GET {url}/api/v1/components` every 3 s,
re-prefix ids and relation targets (`drive:…`, `storage:…`), route actions
through the plugin's proxy, and take health and detail from the upstream's
own `system` card. stormdrive's locate / fleet / test / designation actions
and stormstorage's pool → node → volume graph arrive with no mapping here.
Fleet-wide drive aggregation across nodes rides on fleet discovery later;
one node first.

### stormblock

The block engine's management API on :9090 has no stormview feed yet (its
UI is server-rendered), so this is the one storage plugin that maps rather
than consumes: volumes (health from the engine's own `healthy | degraded |
failed`; size, allocated, physical, redundancy; parent and array edges; a
DELETE action), slabs (health from free space), arrays, exports (edge to
their volume) and the engine's drives, all under an `sb:engine` card whose
`has_many` relations are what the Storage nav items open. Creates: a
volume form and an export form, posted through the proxy.

### sbregistry

The image side on :5100: readiness and warm-up (`/readyz` — ready with a
failed warm-up step is a warning that names the step, because a node whose
PVC ladder was never cut works, slowly, and should say so), and goldens,
clones (edges to their golden and their stormblock volume), pallets and
images as components. Creates: golden and clone forms. sbregistry does not
serve a stormview feed — issue filed; until then the plugin maps its JSON
itself.

### Creating things — the `Creator` contract

OpenShift puts a **+ Create** on every list and an *Import YAML* in the
top bar. Here the same two things are declared, not built: a plugin's
`creators()` returns `Creator { id, label, at: [hash routes], mode: yaml |
form, method, path, template | fields }`, the host stamps the owner and
serves them at `/api/v1/console/creators`, and the SPA offers each one on
the routes it names (`"*"` for everywhere). A YAML creator posts the editor
text as `application/yaml`; a form creator posts its fields as one JSON
object. The kubernetes plugin's `/apply` splits a YAML stream, converts
each document to JSON and POSTs it to the collection its
`apiVersion`/`kind`/`namespace` name (cluster-scoped kinds known, plurals
derived with the usual exceptions), reporting per document. Empty lists
say so and offer the create, rather than showing nothing.

## Cross-cutting services (console-core + binary)

- **Auth** — stormd-compatible: `[[api.users]]` + optional `auth_token`
  bearer; HttpOnly in-memory sessions (24 h); everything except `/healthz`,
  `/readyz`, `/metrics`, auth endpoints and static assets requires a
  session or bearer. stormview's `LoginPanel` renders it.
- **Config** — one TOML: bind address, users/token, theme default,
  multicast group, rustkube endpoint + credentials, stormblock/sbregistry
  endpoints, per-plugin enable flags. Fleet-discovered endpoints
  (stormdrive per node) need no config.
- **Health** — `/healthz` (process), `/readyz` (plugins report), Prometheus
  `/metrics`.
- **stormd summary** — `GET /api/summary` in stormd's plugin-card shape, so
  the console's own container card shows plugin count, node count, and
  health.

## Deployment

```
FROM registry.gt.lo:5000/stormdbase:latest
COPY stormconsole /app/stormconsole
COPY config.toml /etc/stormconsole/config.toml
EXPOSE 9080 9094 22
ENTRYPOINT ["/stormd"]
```

stormd supervises the console with an HTTP liveness probe on
`:9094/healthz` and a `[process.ui]` proxy so the console is also reachable
as a tab on its own stormd. Build: cross-compile
`x86_64/aarch64-unknown-linux-musl` **on dev.g8.lo**, image via podman.

The multicast listener needs the container on the fleet network (host or
macvlan networking) — a bridged/NAT'd container cannot join the group.

On StormCOS the console is a stormdbase golden that stormpump's
`build-goldens.sh` stages exactly like stormdrive and stormstorage: the
musl binary, a stormd config with an HTTP liveness probe on `/healthz`,
and a **flat** `/etc/stormconsole/stormconsole.toml` — `listen_addr` and
`data_dir`, nothing else — with the data volume mounted at
`/var/lib/stormconsole`. The console accepts that shape as well as its own
sectioned one (see README §Configuration); rejecting it was issue #3, a
crash loop that made the node's serial console unreadable.

Startup failures are one line on stderr and a distinct exit status: 78
(`EX_CONFIG`) for a config the console cannot run on, 1 for a port it
cannot bind. stormd archives the run's output to
`/var/log/stormd/<name>.<run>.failed.log`, so that line is what a person
without a shell on the node will eventually read.

## Repository layout

```
stormconsole/
  Cargo.toml                 # workspace
  crates/
    console-core/            # plugin trait, registry, feeds, proxy, auth types
    stormconsole/            # binary: axum server, config, SPA embed
    plugins/
      kubernetes/            # rustkube client, watch cache, k8s components
      fleet/                 # multicast discovery, node proxy, fleet actions
      logs/                  # collector, ring store, query API, SSE
      stormdrive/            # per-node drive aggregation
      stormblock/            # block engine views
      sbregistry/            # goldens/clones/pallets views
  web/                       # Svelte 5 SPA (stormview npm), embedded at build
  config/                    # example config.toml
  Containerfile
  docs/architecture.md       # this file
```

## Integration gaps — issues to file (Core Rule 11)

| Repo | Issue | Needed for |
|------|-------|------------|
| rustkube | No `GET /api/v1/namespaces/{ns}/pods/{name}/log` subresource (apiserver → kubelet proxy) | pod logs in the console (and `kubectl logs` generally) |
| rustkube-node | Kubelet has no `/containerLogs/{ns}/{pod}/{container}` endpoint though CRI log files exist under `/var/log/pods/…` | same |
| stormblock-registry (sbregistry) | Serve a stormview components feed (`/api/v1/components` + `/ws/components`) for goldens/clones/pallets/warm-up | generic rendering in stormconsole and stormsh |
| stormdrive | Serve the stormview components feed (planned in stormview README, not present in src) | fleet-wide drive aggregation without bespoke mapping |
| stormcos | Define the node capability beacon (periodic, alongside stormcast logs: cores, memory, drives, pallets, join state) | fleet inventory without an inventory protocol |
| stormpump | [#7](https://github.com/glennswest/stormpump/issues/7) put stormconsole back in the image — the crash loop (stormconsole#3) is fixed in v0.3.0 | the console booting on a StormCOS node at all |
| stormpump | [#11](https://github.com/glennswest/stormpump/issues/11) Cilium observability — agent `prometheus-serve-addr`, enable Hubble + relay (+ ui) in the image | agent metrics on the Cilium card; the flow view (stormconsole#4) |
| stormd | [#2](https://github.com/glennswest/stormd/issues/2) `no_restart_exit_codes` — a process exiting 78 (EX_CONFIG) should not be restarted. **Done in stormd v0.7.0**; `config/stormd.toml` uses it | a bad config failing once, loudly, instead of looping |

## Phasing

1. **Skeleton** — workspace, core, binary, SPA shell, auth, themes,
   aggregated feed, Containerfile. The console runs and shows itself.
2. **kubernetes** — watch cache, namespace views, workloads, nodes, events.
3. **logs** — collector + viewer; fleet log deep links.
4. **fleet** — discovery, node pages, beacon proposal, day-2 actions.
5. **storage & images** — stormdrive, stormblock, sbregistry plugins.
6. **Remote plugins** — dynamic registration + proxy.
