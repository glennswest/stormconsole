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
nav rendered from the nav feed, and a **namespace selector** in the top bar
(OpenShift's project selector; the selection scopes namespaced views and
persists per browser).

Most pages are *generic*: list pages are `ComponentGrid`/`DataGrid` over a
feed slice (`?kind=…`), detail pages are `ComponentCard` + relation
navigation. Plugins earn custom views only where generic rendering isn't
enough — the log viewer, the YAML editor, node topology.

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
  inference already done at the emitter), stores into a SQLite ring
  (bounded by size/age, WAL, one writer);
- query API patterned on mcastsyslog's proven shape:
  `GET /api/plugins/logs/events?host=&min_severity=&last=&search=`,
  `…/around?at=&window=`, `…/summary`, and SSE `…/stream` for follow;
- the viewer UI: severity/host/search filters, live follow, and deep links
  every other plugin can target (`#/logs?host=storm-a1`).

Per-entity logs stay at their source: a node's stormd serves its own
process logs (`:9080/api/v1/logs`), reachable through the fleet plugin's
node proxy — the console does not re-store what a node already stores.

### fleet (nodes)

There is no fleet-inventory service, by design: **nodes announce themselves
by existing** on the multicast group. The fleet plugin derives the node set
from log traffic (host field + source address), tracks presence/last-seen,
and drills into a node's own daemons through the proxy (stormd :9080,
stormdrive :9092, stormblock :9090, kubelet :10250 read endpoints).

CLUSTER.md calls for a **small periodic capability beacon** alongside the
logs (cores, memory, drives, pallets booted, join state) rather than an
inventory protocol. The beacon is the console's to define — proposed as an
issue on stormcos (below); until it exists, capabilities come from per-node
API calls after discovery.

Fleet actions follow CLUSTER.md: join, promote, demote, drain — each a
call against the node's own control surface, rendered as stormview actions
with `danger` where appropriate.

### stormdrive

One stormdrive per storage node on :9092. The plugin fans out over the
fleet's storage nodes, aggregates `/api/v1/drives` and summaries, and
namespaces them per node (`drive:{node}:{id}`). Drive detail: SMART/health,
wear, thermal, location (enclosure/bay), locate-LED toggle, test and fleet
lifecycle actions (fleet/reserved/spare/failed) — all proxied.

### stormblock

The block engine's management API on :9090: volumes, exports, LUNs,
fstemplates, slabs, arrays, sessions, moves. Rendered generically as
components with `belongs_to`/`has_many` edges (array → slabs → volumes →
exports). Storage health is the engine's own readiness — never a cached
row.

### sbregistry

The image side: goldens (`/v1/goldens`), clones (`/v1/clones`), pallets
(`/v1/pallets`), drives/images (`/v1/drives`, `/v1/images`), warm-up
(`/v1/warmup`), plus the OCI distribution surface (`/v2/`). The console
shows the golden/clone graph (golden `has_many` clones), pallet contents,
and warm-up state. sbregistry does not yet serve a stormview components
feed — issue filed; until then the plugin maps its JSON to components
itself.

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
| stormd | [#2](https://github.com/glennswest/stormd/issues/2) `no_restart_exit_codes` — a process exiting 78 (EX_CONFIG) should not be restarted. **Done in stormd v0.7.0**; `config/stormd.toml` uses it | a bad config failing once, loudly, instead of looping |

## Phasing

1. **Skeleton** — workspace, core, binary, SPA shell, auth, themes,
   aggregated feed, Containerfile. The console runs and shows itself.
2. **kubernetes** — watch cache, namespace views, workloads, nodes, events.
3. **logs** — collector + viewer; fleet log deep links.
4. **fleet** — discovery, node pages, beacon proposal, day-2 actions.
5. **storage & images** — stormdrive, stormblock, sbregistry plugins.
6. **Remote plugins** — dynamic registration + proxy.
