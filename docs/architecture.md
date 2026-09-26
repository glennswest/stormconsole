# stormconsole — Architecture

The StormCOS console, patterned on the OpenShift console: one place to see
and operate a cluster — workloads, nodes, storage, images, logs — but built
the storm way: a single static Rust binary under stormd, rendering
everything through the stormview contract, with a pluggable architecture in
which every domain is a plugin that contributes its own part.

**Scope rule: the orchestrator is rustkube only** — the Rust
kube-apiserver/controller-manager/scheduler over fastetcd — and
rustkube-node (kubelet/kube-proxy/CNI). This console talks to no other.

This document is the design. Where the code does not do something yet it
says so, and names the issue; everything else here is what the code does
(v0.20.1). The README is the operational reference — config, ports, auth.

## What the console is (and is not)

stormcos's `docs/CLUSTER.md` sets the bar: the console is **a view of real
nodes running real services**. Every check is against the running thing —
storage health is the engine's own readiness, not a database row. Its
actions are day-2 actions. **Replacing a disk** is here — locate, leave the
fleet, test, format, designate, through stormdrive's own actions. **Join,
promote, demote and drain are not**: they are a CLI on the node with no
API (stormcos#38), and a button that cannot work is worse than none. It is
not an installer; a console that is only an installer is abandoned the day
the cluster exists.

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
  `/api/v1/components` — stormd, stormdrive, stormstorage, stormipmi —
  appear in the console with no per-daemon UI work. The rest (rustkube,
  stormblock, sbregistry, fastetcd, vmcloud-image-operator) are mapped by
  their plugins from their own APIs.
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
    /// Stable short name; prefixes component ids and the API mount point.
    fn name(&self) -> &'static str;
    /// Nav contribution: sections and items (label, hash route, order).
    fn nav(&self) -> Vec<NavSection> { … }
    /// Create forms and YAML templates (§Creating things).
    fn creators(&self) -> Vec<Creator> { … }
    /// API routes, mounted at /api/plugins/{name}/…
    fn routes(&self) -> axum::Router { … }
    /// This plugin's slice of the aggregated components feed.
    async fn components(&self) -> Vec<ComponentSummary>;
    /// Its own health and one line — the `plugin:<name>` card and /readyz.
    async fn health(&self) -> Health { … }
    async fn detail(&self) -> String { … }
    /// What one viewer may see of this plugin's slice.
    async fn access(&self, viewer: &Viewer) -> Access { … }
    /// What happened to one of its objects, and recently (§Events).
    async fn events(&self, viewer: &Viewer, id: &str) -> Option<Events> { … }
    async fn recent_events(&self, viewer: &Viewer) -> Option<Events> { … }
    /// Background work (watches, multicast listeners, pollers).
    async fn run(&self, shutdown: CancellationToken) { … }
}
```

Every component id must start with `<name>:` — the registry warns once per
component that does not, on every refresh, which at rack scale is enough
to slow the feed a hundredfold (#32).

The host (console-core) provides:

- **Registry** — plugins are registered at startup from config; a disabled
  plugin simply isn't constructed. All plugins are compiled in; the trait
  is the seam where dynamically registered remote plugins would attach
  (§Remote plugins — design, not built).
- **Aggregated feed** — concatenation of every plugin's `components()`,
  cached, pushed as full snapshots on `/ws/components` exactly like stormd.
- **Nav feed** — `GET /api/v1/console/nav` returns the merged navigation;
  the SPA renders whatever it is given, so a new plugin appears in the nav
  with no frontend change.
- **Proxy helpers** — reverse-proxy plumbing (`console_core::proxy`) so a
  plugin can expose its upstream through the console origin,
  `/api/plugins/{name}/proxy/…`, carrying the console's own bearer where
  the upstream needs one (`router_as`, `forward_as`: stormblock,
  stormipmi). The browser only ever talks to the console; upstream
  addresses and credentials stay server-side.
- **Access** — a `Viewer` is the identity on a request; each plugin answers
  `access()` with what that identity may see of its own slice, and the
  registry applies the answer *before a snapshot leaves the process*. That
  is the whole point: a filter in the UI is a display choice, and this has
  to be an authorization result. A plugin with nothing to authorize says
  `Unrestricted`, and `GET /api/v1/console/access` reports whether anything
  is being enforced at all — the console never implies a check it is not
  doing. See §Who sees what.
- **Upstream addresses** — the address the console *dials* and the address a
  browser could *use* are different things (`console_core::upstream`). The
  console runs on the node, so its dial addresses are loopback and correct;
  printed on a card read from a laptop, `127.0.0.1:9092` is that laptop. A
  card says where an upstream is in words — "on this node :9092" — and
  anything meant to be clicked goes through the plugin proxy.

### Who sees what

The console aggregates kubernetes, fleet, logs, drives, volumes and the
registry into one feed, which makes it the broadest read surface on the
platform. The authorization model, in the order it runs:

1. **Identity.** `[[api.users]] kube_token` gives a console user a
   kubernetes bearer. The auth middleware puts the `Viewer` on every
   request's extensions; console-core makes it an axum extractor, so a
   plugin route reads it without knowing anything about sessions.
2. **The question.** `NamespaceAccess` asks rustkube **as the viewer**:
   `GET /api/v1/namespaces` first, since a 200 is the self-scoped answer
   OpenShift's project list gives. rustkube's RBAC makes that
   all-or-nothing, and it serves no `SelfSubjectAccessReview`
   ([rustkube#59](https://github.com/glennswest/rustkube/issues/59)), so a
   403 falls back to one probe per known namespace — `GET
   /api/v1/namespaces/{ns}/pods?limit=1`, **not** the Namespace object,
   because a Namespace is cluster-scoped and a RoleBinding cannot grant
   access to one. Answers are cached 30s: a feed that redraws every two
   seconds would otherwise be a denial of service on the apiserver by way
   of a UI. One `NamespaceAccess` is shared by the kubernetes and vm
   plugins, so the same question is asked once and cannot be answered two
   ways.
3. **Reads.** `/api/v1/components` and `/ws/components` are filtered per
   viewer, with surviving relations re-pointed, so a hidden object is
   unreachable by REST, by socket and by following an edge. A plugin route
   answers 404 for a hidden namespace — the same answer an absent one
   gets, because 403 would confirm it is there.
4. **Writes.** Every mutation carries the viewer's own bearer, so the
   apiserver's RBAC decides. A viewer whose Role has no `delete` verb is
   refused by the apiserver rather than by the console's guess about them.
   The *watches* keep the console's own credential — they have to see the
   whole cluster to serve anybody.
5. **Honesty.** What is withheld is counted and named ("4 namespaces you
   cannot view"), because a short list with no explanation reads as a
   broken console. An apiserver that cannot be asked hides nothing and
   says so: an unreachable authorizer must not quietly become a permissive
   one, and must not blank the console either.

#### Remote plugins — design, not built

A service would register a manifest (name, nav items, upstream URL,
optional components URL); the core would proxy its UI under the console
origin and merge its components feed (OpenShift dynamic-plugin style,
stormd `[process.ui]` style). That would make the console extensible by
components it has never heard of — same philosophy as stormview's open
`kind`. Nothing implements it; today every plugin is compiled in.

### Frontend model

Svelte 5 + Vite, `stormview` npm package, embedded in the binary
(rust-embed) like stormd's SPA — no node at runtime. The app shell owns:
hash router, login (stormview `LoginPanel`, session cookies), theme picker,
nav rendered from the nav feed, and a **Project selector** in the masthead
(the viewer's projects, system namespaces apart for administrators; the
selection scopes namespaced views, travels in the URL as `?ns=` and
persists per browser — §Projects).

Most pages are *generic*: list pages are a `ResourceTable` (or a
`ComponentCard` grid) over a feed slice, detail pages are relation
navigation. Plugins earn custom views only where generic rendering isn't
enough — the log viewer, the YAML editor, a VM's page, the drives rack map
(`web/src/lib/drivemap.js`), machines, images, projects.

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
- **A reference shows every target**, not the first: a placement is
  usually one thing (the node a VM is on) but the policies selecting a pod
  are several, and showing one of them is worse than showing none, because
  nothing says there were others. A reference whose target is not in this
  console's feed is dropped where it is drawn — a plugin publishes an edge
  without being able to know whether the other side exists here.
- **Placement is a column**, read off the `belongs_to` edges rather than
  written out per kind. Namespace, node, shelf, array, definition — any
  placement whose values differ down the list earns a column and a sort;
  one that is the same on every row (an "engine" on every stormblock
  volume) earns nothing. Upstreams reached through a `FeedPlugin` get the
  same columns without this repo writing their components.
- **Every row opens**, and what opens is the object: the detail
  unabbreviated, every metric, the references as links, and every action
  including the destructive ones the row keeps behind its menu. Clicking
  a line goes to that component's own page where it has one, and opens it
  in place where it does not.

Everything else — multi-select with bulk lifecycle actions, sorting —
matches `ComponentGrid`'s behaviour.

#### A relation is containment or context, never a destination

`has_many` is containment: a pod's containers, a node's pods, a golden's
local copies. It nests into a table inside the row.

`has_one` and `belongs_to` are context: the node a VM runs on, the volume
a clone is stored in, the parent a snapshot was cut from, the identity a
Cilium endpoint carries. They are references — a chip in the opened row
that links to that object — and never where the line leads.

Getting this wrong is not cosmetic. A VM published its node as `has_one`
and nothing else; the table read that as something the machine contained
and the card as where following the row went, so opening a virtual
machine landed in node details (#18). A volume published its parent the
same way and expanded upwards through its own ancestry. Plugins therefore
publish an edge in the direction the thing actually points, and the
renderers do not have to guess.

### Navigation: work and administration

Virtual machines live in **Workloads**, beside Pods, rather than in a
section of their own. A VM here is a kube object the kubelet reconciles,
so running one is the same activity as running a pod, and a navigator
that separates them says otherwise. Workloads is therefore built by two
plugins and numbers its items with gaps.


A `NavSection` declares a `kind` — `work` or `admin` — and the SPA starts
the administration ones shut, with their total beside them (#16). The
split is not "basic" against "advanced", which ages badly and is faintly
insulting; it is the person creating a virtual machine against the person
deciding whether a drive is failing, and the second one knows where to
look.

The kind is declared by the plugin that contributes the section, because
the plugin is the thing that knows, and `Work` is the default, so a
section nobody classified stays open and nothing is hidden that was not
deliberately classified. A section two plugins contribute to is work if
*either* calls it that — which is how Storage stays open for the PVCs
somebody asked for while stormblock's slabs sit in the same section.

Shut is a third state, not the absence of a choice: the stored map holds
only choices somebody actually made, so an explicit choice always wins and
a section that changes kind on the server changes for everybody who never
touched it.

### Events: what happened, as opposed to what is

An object's fields say its state; only an event says how it got there. The
console had events in two places — a cluster-wide list and a namespace's
tab — and neither answers "what happened to *this*", which is the question
actually being asked.

It is a **plugin contract**. `ConsolePlugin::events(viewer, id)` is asked
for one component id and answers `None` if the id is not its own; the host
takes the first plugin that claims it (`/api/v1/console/events?id=`). A
plugin with an event source needs no change anywhere else, and one without
needs no change at all.

`Events { available, reason, items }` keeps two answers apart that a
single empty list would conflate: **nothing happened** and **nothing
records events for this**. stormblock, stormdrive and the registry write
none, and a volume with no events is not a volume nothing has happened to.

Matching is on `involvedObject`, which is why `ResourceSpec` carries
`api_kind` — an event says `PersistentVolumeClaim`, not the console's
`pvc`, and deriving one from the title works for "Pods" and produces
"Network policie" for the next one along. Kind is matched as well as name
because a Service and a Deployment routinely share one, and an event about
the wrong object is worse than none: it gets acted on. A container has no
events of its own — the kubelet records them against the pod with the
container in `fieldPath` — so a container's box is its pod's events
narrowed by that path.

A virtual machine's events are recorded against two objects that share a
name, the `VirtualMachine` and the `VirtualMachineInstance`, and somebody
asking "did my start work" does not care which; the vm plugin merges them.

#### The dock

`/api/v1/console/events/recent` merges every plugin's recent activity, and
the SPA docks it at the bottom of every screen — vSphere's Recent Tasks
and Proxmox's task log, for the reason both exist: you press Create, and
the question for the next ten seconds is "did that work". Answering it
should not cost a navigation.

Two sources, deliberately. **What this console did** is appended in the
browser the instant an action returns — nothing upstream knows a button
was pressed, so this is the only half that can say "your request was
sent", and it is acknowledged in the same frame rather than after the next
poll. **What the cluster did about it** is polled every five seconds, and
is the half that carries the reason when it did not work. Shut, the bar
still shows the last line, because a dock that hides everything when
closed is a dock people leave open.

### Who may do what

Authentication is optional and off by default on a single node. When it is
off, every viewer holds `admin` — a console with no credentials configured
is one where everybody who can reach the port is an administrator, and
`main` warns about exactly that on every start. A refusal has to come from
having decided to enforce something, not from having decided nothing.

When it is on, a viewer carries **roles**, named for what the console
offers rather than for Kubernetes verbs: "may open a console" and "may
delete a volume" do not line up with get/list/watch, and pretending they
do produces a model nobody can reason about. `admin` holds every role, so
no check has to remember to list it.

The read/write boundary is enforced **once, in the host, by method**: any
POST, PUT, PATCH or DELETE under `/api/plugins/` needs `operator`. Not
per route — one route added without the check is the whole hole, and the
storage and registry proxies are `any` and could never be enumerated. A
route that read with a POST would be refused, and would be a route worth
changing rather than an exception worth carving. Namespace scoping
(`access()`) is a separate and narrower question and is unchanged.

Still open (#15): users and groups that can be managed without editing a
file on an immutable root, certificate identity from `stormcert`, and an
audit of the capabilities that matter — consoles, deletes, goldens.

### The network's view, inside the views people use

Cilium's facts are attached to the pod, the node and the machine rather
than living only in the five lists under Networking (#17): the identity
*resolved* to the labels policy is written against, the datapath state,
the policies that select the workload, and the addresses left in a node's
pod CIDR.

Two constraints shape it. The agent is a DaemonSet — every node has one
and they can disagree — so everything here comes from a CRD, which is the
cluster's own record and says the same thing to everybody. And a policy
selector is evaluated against the labels *Cilium* decided the workload
has, because those are the labels policy is applied to; a match computed
against the pod's own labels would differ from the datapath's on exactly
the workloads where it matters. A selector carrying `matchExpressions` is
reported as selecting nothing rather than guessed at.

Flows — allowed and denied, the actual answer to "why can nothing reach
this" — and per-module agent health come from Hubble and the agent's own
REST API, neither enabled in the image yet (stormpump#11, tracked on #4).

## Built-in plugins

### kubernetes (rustkube)

Talks to the rustkube apiserver (`https://…:6443`) with a bearer token or
client cert from config. rustkube is kube-wire-compatible (core v1,
apps/v1, batch/v1, RBAC, CRDs, watch streams with bookmarks), so the client
is a thin typed layer over the standard REST paths.

- **Watch-backed cache** (`cache::RESOURCES`): list+watch on namespaces,
  nodes, pods, deployments, statefulsets, daemonsets, jobs, cronjobs,
  services, PVCs, configmaps, network policies, resource quotas, limit
  ranges, and — optional, synced empty when not served — PVs, storage
  classes, CRDs, cluster roles and the Cilium kinds. Events are read on
  demand (`GET /api/v1/events`), not watched. The cache serves the UI
  instantly and emits the plugin's components slice (pods and workloads
  with `belongs_to` namespace edges, `has_many` pod edges, health derived
  from status/conditions). ReplicaSets are not watched.
- **Namespace views**: the Project selector scopes every namespaced page; a
  namespace's page (`#/k8s/ns/<name>`) is the project's — inventory whose
  every count is a link, quota, limit ranges, events, YAML, and the
  Project tab (§Projects).
- **Actions** (as the viewer): delete a pod, delete any object through
  `DELETE /api/plugins/k8s/raw/{/api|/apis…}`, import YAML (`/apply`,
  always into a project), edit an object (`PUT /object/{kind}/{key}`,
  `resourceVersion` as the guard), and the project verbs. **Not built:**
  scale, cordon/uncordon, drain (#36).
- **Cilium**: the agent's API is a unix socket and Hubble is gRPC, neither
  reachable from a golden, so Cilium is read through its CRDs on the
  apiserver — `cilium.io/v2` endpoints (state, address, identity, edge to
  the pod), nodes, identities, network policies (selector and rule counts,
  DELETE) — plus core NetworkPolicy, under one `k8s:cilium` card. CRD kinds
  are optional in the watch cache: a 404 is "not installed", synced and
  empty, re-checked each minute. **Not built:** Hubble flows — they need a
  relay the console can reach (stormpump#11, #4).
- **Pod logs — not available.** rustkube has no `/log` subresource and
  rustkube-node no `/containerLogs`, although the node writes CRI log files
  under `/var/log/pods/…` (rustkube#55 and rustkube-node#34, closed as
  duplicates of stormvm#5, which gives a VM's serial instead). Nothing
  links a pod to its logs; a node's page links to that node's fleet logs.

### logs

The fleet already emits: stormcast sends RFC 5424 over UDP to multicast
`239.255.42.1:5514` from initramfs onward, and there is no production
collector. The logs plugin **is** the collector:

- joins the multicast group, parses RFC 5424 (stormcast dialect: severity
  inference already done at the emitter), stores into a **deduplicating
  redb ring**;
- query API patterned on mcastsyslog's proven shape:
  `GET /api/plugins/logs/events?host=&app=&min_severity=&last=&search=`,
  `…/summary`, and SSE `…/stream` for follow;
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

The text is *fingerprinted* before it is keyed, and there is one
normalisation: a leading timestamp comes off. Emitters here forward a
process's own log line verbatim, and tracing writes its timestamp at the
front of it, so the message carries a microsecond clock that changes on
every occurrence —

```text
2026-09-02T18:26:30.258373Z  WARN plugin_logs::collector: store insert failed …
2026-09-02T18:26:44.545181Z  WARN plugin_logs::collector: store insert failed …
```

— and keyed on raw text those are two entries. The first build of this
ran against a real node and deduplicated nothing at all for exactly that
reason. A leading timestamp is redundant with the event's own `ts` and can
never be what distinguishes two messages, so it is stripped for keying
only; the stored text is whatever last arrived. Nothing else is
normalised — collapsing numbers or identifiers would merge messages that
genuinely differ.

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
logs rather than an inventory protocol, and it now exists (stormcos#26).
Every node puts an RFC 5424 element — `[storm-beacon@0 cores="8"
mem_bytes="…" drives="3" pallets="…" running="18" failed="0" …]` — on the
same multicast group as its logs, every thirty seconds. The collector
already sees every datagram, so reading it costs no socket, no discovery
and no polling: capabilities no longer come from per-node API calls after
discovery, they arrive with the traffic that made the node visible at all.

Two properties are load-bearing on the reading side:

- **Beacons are kept beside the ring, not in it.** A beacon is *state* —
  the current shape of a node — and the ring is a bounded, age-pruned,
  deduplicating log. A beacon that fell out of a busy ring would take a
  node's capabilities off the fleet view while the node was still
  announcing them every thirty seconds.
- **Every parameter is kept, not parsed into a struct.** stormcos owns the
  shape; a fixed struct here would mean every new field needed a release in
  this repo before it could be seen. Unknown fields reach the node card.

Absent fields are rendered as absent. The emitter omits what it cannot read
rather than sending an empty value, precisely so a reader can tell "no
role" from "role unknown", and nothing on this side defaults them to zero.

Another node's services are drilled into on demand — `GET
/api/plugins/fleet/nodes/{host}` probes its port layout (`node::NODE_PORTS`,
checked against stormcos `build-goldens.sh`: only ports that serve a feed)
and `…/nodes/{addr}/{port}/…` proxies to one, for an address the collector
has heard and a port in the layout. **Not built:** the fleet actions —
join, promote, demote, drain — which have no API (stormcos#38).

### stormdrive and stormstorage — feed consumers

Both daemons serve the stormview components feed themselves, so neither is
mapped: poll `GET {url}/api/v1/components`, re-prefix ids and relation
targets, route actions through the plugin's proxy, and take health and
detail from the upstream's own `system` card. stormstorage is a plain
`FeedPlugin` (`storage:…`, every 3 s). stormdrive is fleet-wide
(`DrivesPlugin`, every 5 s): this node's feed as `drive:…` and every other
node's as `drive:@<host>:…` with its own proxy — §Drives at rack scale.

### vm (KubeVirt objects, not a daemon)

stormvm's `docs/kube.md` settles where a VM lives: *"stormvm is libraries,
the kubelet is the loop"*. A VM is a `VirtualMachine` /
`VirtualMachineInstance` in the apiserver: rustkube's controller-manager
makes the instance from the definition, its scheduler places it
(rustkube#72), and rustkube-node's kubelet reconciles the instances
assigned to its node. stormvm on :9095 serves only what needs the running
process — the serial and framebuffer doors and the control verbs — not the
objects. So the plugin watches
`kubevirt.io/v1` with the same client and list+watch loop the Cilium view
uses — `plugin-kubernetes` exports `Client`, `KubeStore` and `watch` for
it — and there is no second source of truth to reconcile.

It is a plugin rather than two more kinds in the kubernetes plugin because
a VM is a domain: its own navigation, its own creation forms, its own
lifecycle verbs, and two console doors that are websockets rather than
component actions.

Both objects are surfaced, because they answer different questions: the
definition is what should exist and whether it should run, the instance is
the machine that *is* running. vCPU is read however the spec spelled it
(`cores`, the sockets×cores×threads product, or a resource request) — a
page that understands one spelling is wrong for every VM that used
another. Lifecycle is `spec.running` and nothing else; stopping an
instance is deleting it, since a VMI *is* the running machine.

The verbs are on the row, so the common ones need no detail page first —
`POST …/machines/{ns}/{name}/{start,stop,restart}` and `DELETE
…/machines/{ns}/{name}`. Restart is the instance deleted with a
definition there to put it back, because KubeVirt has no verb that
reboots a VMI in place. Without a definition that is a delete wearing a
reassuring name, so it is refused with `409` and the sentence explaining
why rather than performed; the row offers it disabled for the same
reason, since "why can I not restart this" is a question the row should
answer. Stopping an instance that nothing will restart is destructive and
is published `danger`, which is what keeps it behind the row menu beside
Delete.

**The verbs beside the doors.** stormvm serves `pause`, `unpause`,
`softreboot`, `reset`, `freeze` and `thaw`, and reports per machine which
of them it can take — `control.lifecycle` for the control socket,
`control.freeze` for the guest's own agent. The console proxies them
(`POST …/machines/{ns}/{name}/verb/{verb}`) rather than reimplementing
them: pausing a guest is QMP or cloud-hypervisor's HTTP API depending on
which hypervisor started it, and which one that is was recorded at start
precisely so nothing else has to guess. A machine is offered only what it
can take, because a button that 404s makes a client report that *the VM*
refused.

**Settings, and when they land.** `GET`/`PUT
…/vms/{ns}/{name}/settings` gives every editable field with the answer to
"when does this take effect" — which depends on the machine, since on a
stopped one everything simply applies. Edits are merge patches against the
`VirtualMachine`, never the instance: a patch to a running VMI's spec is
read by nothing and lost when it stops, so a machine with no definition is
refused rather than half-changed. Two fields are refused on purpose and
say where to go instead — the network binding moves the guest's address
and a one-field form cannot say what the new one will be, and the SSH key
lives in a cloud-init seed the guest reads once at first boot.

**Disks.** `POST`/`DELETE …/vms/{ns}/{name}/disks[/{disk}]` add and
remove a disk — both halves together, since a disk is an entry in
`domain.devices.disks` and a matching one in `volumes`, and two
`format!`s in two places is a machine that boots with a disk pointing at
nothing. The arrays are sent whole because a merge patch replaces rather
than merges, which also means the console refuses to work from a spec it
could not read rather than replacing a machine's disks with a list of
one. **This is not hotplug**: stormvm serves no device verb
(stormvm#18), so the guest sees a new disk at its next boot and every
layer says so. The root disk and the seed refuse to be removed.

The card merges the definition's disks with the running instance's, each
saying whether it is `attached`. Reading either alone can state only half
the truth: the instance alone loses a disk the moment it is added, and
the definition alone loses one that is still in the guest after being
removed.

**SSH keys, uploaded once (#26).** `vm/src/keys.rs` (parse and validate a
public key — type, base64 body whose embedded type must match, comment; a
private key refused by name; Secret and item names; KubeVirt's
`accessCredentials` shape) and `keystore.rs` (the Secrets, as the viewer).
KubeVirt's `accessCredentials` can only name a Secret in the machine's own
namespace, so the user's list is Secret `<user>-ssh-keys` in
`[vm] ssh_keys_namespace` (default `default`), labelled
`storm.io/ssh-keys-for=<user>` and `storm.io/ssh-keys-home=true`, and a copy
with the first label is kept in every namespace where they create or key a
machine; saving or deleting on the Account page rewrites the original and
every copy (a replace carries `resourceVersion`, and clears `data` so a
deleted key does not survive in it). Routes: `GET|POST /keys`,
`DELETE /keys/{item}`, `GET /keys/choices`, `GET|POST
/vms/{ns}/{name}/keys`. The create form's per-key checkboxes are a generic
field kind, `checklist`, whose options the dialog fetches from `source` as
the viewer — options that depend on who is looking cannot be declared once
per plugin. At create: every chosen key goes into the cloud-init seed (the
image's default user and root) — which is what puts a key in a guest today,
since no node honours `accessCredentials` yet (stormvm#41) — and into
`accessCredentials` (`noCloud`): the user's Secret copy when the whole saved
list was chosen, so the machine follows that list, and a `<vm>-ssh-keys`
Secret for anything else (a subset, the config file's keys, a pasted one).
"Add my keys" on an existing machine names the user's Secret with
`qemuGuestAgent` (users `[root]`), the only path to a running guest, and
says that nothing acts on it yet. Keys from the console's config file
(`[[api.users]] ssh_keys`) are shown read-only and offered too. rustkube
stores `stringData` as written rather than folding it into `data`
(rustkube#101); every reader here takes both. `deploy/verify-vm-keys.sh` is
the live check.

**Snapshots: the Backup tab (#25).** `vm/src/snapshots.rs` over KubeVirt's
own `snapshot.kubevirt.io/v1beta1` `VirtualMachineSnapshot` and
`VirtualMachineRestore`, watched as optional kinds (`vmsnap`, `vmrestore`),
so `virtctl`/`oc` see exactly what the console made. The Snapshot button
creates the object as the viewer and returns: the node idles the
filesystems, pauses, group-clones every disk and resumes (stormvm#28), and
writes the status in the shape `stormvm-spec` `snapshot_status` produces
(`phase`, `readyToUse`, `indications`, `error.message`, `creationTime`,
`virtualMachineSnapshotContentName` = the stormblock group). Rows read the
step (`storm.io/step`), disks (`snapshotVolumes.includedVolumes`) and size
(`storm.io/sizeBytes`) when present, and say "not reported" otherwise
(stormvm#45). An object with no status for a minute says nothing has
picked it up and names rustkube-node#53. Restore is refused, with the
sentence, when the snapshot is not ready, the machine is running, or there
is no `VirtualMachine` to restore into; a snapshot is only deleted or
restored through the machine it belongs to. Routes:
`GET|POST …/vms/{ns}/{name}/snapshots`, `DELETE …/snapshots/{snap}`,
`POST …/snapshots/{snap}/restore`. Without the CRDs the tab says so and
names stormpump#28. `deploy/verify-vm-snapshots.sh` is the live check.
Known upstream gap: rustkube#100 — a CR's DELETED watch event carries the
plural as its namespace, so a deleted snapshot (or a stopped VM's
instance) stays in the console's cache until it relists.

**Addresses, asked against done (#24).** `vm/src/network.rs` gives one row
per interface from two sources that disagree today. What was *asked* is
read the way stormvm reads it: `storm.io/bridge.<iface>`, then
`storm.io/bridge`, win over the network; otherwise the network of the same
name — `pod` with the binding on the interface (`masquerade` by default,
`bridge`, `passt`) or `multus`. What the node *did* is `status.interfaces[]`
as rustkube-node writes it: `mac`, `ipAddress`/`ipAddresses` (guest agent,
else the node's neighbour table) and `storm.io/binding` — `bridge` for a
tap on a real bridge, `user` for qemu's NAT inside the hypervisor. Each row
carries a `reach` verdict (`reachable`, `nat`, `none` = no address yet,
`pending` = not reported, `stopped`) and a sentence. A spec asking for the
pod network that runs as `user` says so and names stormvm#16, so "pod" never
reads as a working pod network. The list row carries every address as `ip`
(warn when behind the NAT, "no address yet" when running without one) and
the binding as `network` ("NAT, not pod"). The page shows the table with a
copy button per MAC and address; `ResourceTable` puts one on any metric
whose value is IP or MAC addresses, decided on the value so feed upstreams
get it too. `deploy/verify-vm-net.sh` is the live check.

**Memory and the balloon.** `memory.guest` with a *lower* resource
request is ballooning — stormvm reads it that way and builds a
`virtio-balloon-pci` or passes `--balloon`. That floor is the only
mechanism by which a machine's memory ever changes without a restart, so
it is a setting, and it says which of the two situations a machine is in.
A request equal to the size is not a floor. Moving the balloon once it
exists needs a verb stormvm does not have (stormvm#19).

**Pending changes.** A `VirtualMachine` is the definition and a
`VirtualMachineInstance` is the machine that is running, and nothing makes
them agree. A diverged machine reports which fields, what was asked for
beside what is actually running, rather than leaving somebody to find out
at the next reboot.

The console doors are relayed through the console's own origin
(`/api/plugins/vm/console/{ns}/{name}/{serial,vnc}`) and addressed by VM
rather than node, so the browser never learns a node address and the URL
survives a live migration. A token is minted per attach and presented if
minting worked, so a `--require-token` node works; on an ordinary node it
is one extra request that changes nothing. **Read-only is a capability**:
a serial console is a root shell on most guests, so a viewer without
`operator` has what the browser sends dropped at the relay rather than the
door refused — watching a guest boot is the whole point of the door.
Keepalives still pass. The **replay** (stormvm sends the tail of the
guest's console log on attach) is labelled, because a reader who does not
know the first screenful is history is reading it as though it were now. stormvm is probed for its **VM collection**,
not `/healthz` — every daemon here answers `/healthz`, and a health probe
would have the console offering a terminal that dials a stranger.

### Drives at rack scale (#32)

 160 drives a node, ~1,600 a rack. The
drives plugin (`crates/plugins/stormdrive`) reads this node's stormdrive
(ids `drive:…`, unchanged) and every other node's: each host the log
collector has an address for, at :9092, plus `[stormdrive] nodes`. A remote
node's ids are `drive:@<host>:…` (under the plugin's prefix, as the
registry requires) and its actions go through
`/api/plugins/drive/node/<host>/proxy`. Every drive and shelf carries a `node` metric; a host with no
stormdrive adds no rows and is counted on the card. Per-drive usage is
stormdrive#12; until then the stormblock plugin publishes, for this node,
`sb:use:<serial>` (slab bytes and free, summed over the slabs that name the
drive, stormblock#136) and `sb:member:<dev>` (a drive-level array member's
state), and the kubernetes plugin a node's `rack` from its label
`topology.storm.io/rack`. The page's model is `web/src/lib/drivemap.js`
(pure; `drivemap.test.mjs` runs it at 10×160 with plain node): it joins
those by serial and device path (this node's drives only — another node's
`/dev/sd5` is not this engine's), filters (failing, degraded, rebuilding,
full ≥ 90%, hot ≥ 50 °C, out of fleet, spares), groups by chassis (a shelf
is a node's), node or rack, totals up to EB, and colours a bay by health,
temperature, wear or usage — no data drawn hatched, never green. The Drives
page is a map by default: each chassis a grid of its bays (12 across up to
60 bays, 15 above), empty bays kept, rebuilding outlined; a totals band
whose counts are filters; a list view capped at 200 a group. The live check
is `deploy/verify-drives.sh`.

### Images are the registry's; Volumes are what is attached (#19)

 A UI
point of view only — goldens stay the engine's volumes. The stormblock plugin
reads the engine's own `kind` (volume|golden|blank|media|snapshot|template),
`in_use`, `attachments` and `consumer` (stormblock v18.1.0, #138): Storage →
**Volumes** is `kind volume` and in use, each with its consumer (a PVC links
to `k8s:pvc:…`, a VM to `vm:machine:…`, a mount says where) and how it is
served (`nvme-tcp nsid 2`, `ublk /dev/ublkb0 → /data`), Delete disabled while
in use; **Unattached volumes** apart; image kinds are not in either. An older
engine carries neither field: its unsealed volumes all go to Volumes and the
engine card says why the split is missing. A guarded engine (v18 guards
reads too) needs `[stormblock] token_file`, used for the poll and the proxy
(stormcos#94 wires it on nodes). The sbregistry plugin reads the catalog
(`/v1/catalog/images`, sbregistry v0.23.0) as `reg:cat:<name>` — kind,
component/source, sizes, clones and clone names, releases, digest, location,
the engine volume underneath, and its base as an upward edge, which is the
lineage — and merges `/v1/media/jobs` in: an image arriving reads
"downloading from <host>, n%", a failed fetch "failed (<fault>): <error>",
and a job with no image yet is a row of its own. An older registry's 404 is
said on the card. `#/images` groups the catalog by kind. The live check is
`deploy/verify-images.sh`: a v18.1.0 engine on file-backed disks and a
v0.23.0 registry on it, then forge's real engine read-only through a console
only.

### Machines, from stormipmi (#31)

 `crates/plugins/stormipmi` (name
`ipmi`) fronts stormipmi's Machines API (stormipmi#12) at `[stormipmi] url`
(default this node's :9097; usually a bastion). Its `machine:<tag>` feed
comes in as `ipmi:machine:<tag>`, power actions routed through the plugin's
proxy. `/api/plugins/ipmi/proxy/*` forwards **only** the Machines surface
(`api/v1/machines…`, `api/v1/releases`, `api/v1/hosts…`,
`api/v1/components`; `..` refused), reads open, **every write `admin`
only** — stormipmi leaves that to the console — with stormipmi's
`api.tokenFile` bearer (`[stormipmi] token_file`) added server-side, and an
audit line per act naming the user. `/api/plugins/ipmi/console/{ns}/{host}`
relays the SOL console (replay, then live), read-only unless the viewer is an
admin: a serial console is a root shell. `/api/plugins/ipmi/me` tells the
page whether to offer the buttons. `#/machines` (Hardware): by service tag —
BMC address/vendor/model/firmware and the credentials Secret's name, power
as the BMC last said it (and what was asked, when they differ), state, the
release each boots (`pinnedFromDefault`, `dangling`, since), Set release
(confirmed; stormipmi answers once read back; takes effect at next boot),
boot intent (stormipmi's 501 shown as it says it), test marks, the default
image, and new hosts to adopt with their BMC. `deploy/verify-machines.sh` is
the live check on stormipmi's own rig.

### Projects (#28)

 A project is a namespace with an owner, served by
rustkube as `project.openshift.io/v1` (rustkube#97). `kubernetes/src/
projects.rs` asks everything **as the viewer**: `GET /projects` (the
viewer's projects; system namespaces only for `admin`, separately),
`POST /projects` (a `ProjectRequest` — the apiserver annotates the requester
and binds them `admin`; on an apiserver without the API, a Namespace with
the same annotations), `GET|DELETE /projects/{p}` (ownership, members,
isolation), `POST /projects/{p}/members` and `DELETE …/members/{binding}`
(RoleBindings to the ClusterRoles `admin`/`edit`/`view`; a member change
clears the namespace-access cache so the person let in sees it at once), and
`POST|DELETE /projects/{p}/isolate` (NetworkPolicies `storm-isolate` — every
pod in the namespace to every other, nothing else in or out — and, opt-in,
`storm-isolate-dns` to port 53 in kube-system; enforced by Cilium, and a VM
is covered only once it is a pod-network endpoint, stormvm#16). A refusal
from the apiserver stays a 403 with its reason.

**System namespaces** — `default`, `openshift`, `kube-*`, `openshift-*`,
and `[kubernetes] system_namespaces` (default `["cilium"]`, the node's
services) — are never a project: not in the viewer's list, not a create
target (`/apply` refuses them except for `admin`, VM create always), not
deleted or isolated from the console. **Every create targets a project**:
`Creator.project` makes the dialog show a project picker with New project
inline (suggested `<user>-work`, `<user>-vms` for machines); YAML goes to
`/apply?project=`, and a namespaced document that names no namespace goes
there — never to `default`, which is refused with a sentence when nothing
was chosen. The masthead selector is a **Project** selector. Lists always
show the Namespace column on namespaced rows and group by project when they
span several. The **Cluster** admin section holds what no project owns —
Nodes, all Namespaces, PersistentVolumes, StorageClasses, CRDs,
ClusterRoles, all now watched (optional kinds). The fleet's "Node services"
left the navigator: a node's daemons are on its page, and to the cluster
they are the mirror pods in kube-system. A claim Pending under a class with
`volumeBindingMode: WaitForFirstConsumer` (its own class, or the default)
is Idle, reads "Pending — provisioned when a pod or VM uses it", and offers
Attach to a VM (`#/attach/{ns}/{claim}`: the project's machines, each one
click, as a disk). rustkube does not default a claim's phase (rustkube#102);
a missing phase is read as the API's default, Pending.
`deploy/verify-projects.sh` is the live check, as three real identities.

### stormblock

The block engine's management API on :9090 has no stormview feed (its UI
is server-rendered), so this plugin maps rather than consumes, every 5 s:
volumes (the engine's own `kind`, health, size, allocated, shared,
redundancy, consumer and attachments; parent, array and consumer edges;
DELETE, disabled while in use), slabs (health from free space), arrays,
exports and the engine's drives, all under an `sb:engine` card whose
`has_many` groups are what the Storage nav items open — `volumes`
(attached), `unattached`, `images` (the engine card only), `slabs`,
`arrays`, `exports`. It also publishes `sb:use:<serial>` and
`sb:member:<dev>` for the Drives page. A guarded engine is read and proxied
with its token (`[stormblock] token_file`). Creates: a volume form and an
export form, through the proxy.

### fastetcd (the datastore, #20)

The store rustkube stands on, and until this plugin the one component of
the control plane the console said nothing about. fastetcd serves two
things over HTTP, and the plugin reads both:

- **`/metrics`** on its own listener (loopback :2381 by default, which is
  why the console reads it from the node): revision, compact revision, DB
  size and in-use, the effective quota and how much of it is used, disk
  free, snapshots, NOSPACE, has-leader and leader changes. Enough for the
  store's health — an alarm or no leader is an error, 80% of quota a
  warning — and for the one derived number worth having, **what a defrag
  would free** (size minus in-use).
- **etcd's v3 JSON gateway**, `POST /v3/…` on the client port: status
  (leader, raft term and index, version), the member list, alarms by
  member, `range` for the keyspace, and the maintenance verbs. etcd serves
  it; fastetcd does not yet ([fastetcd#28]). Everything that needs it is
  read from it when it answers and **said to be missing** when it does not
  — the store's row names the issue, and there is no member table at all
  rather than an empty one that reads as a cluster with no members.

No gRPC client, deliberately: that was the owner's call on #20, and
#28 is the alternative — one HTTP surface every tool can use, in etcd's
own shape so `curl` recipes written for etcd work unchanged. The gateway
readers take either spelling of a field and 64-bit integers as strings or
numbers, since protobuf's JSON mapping and grpc-gateway's `OrigName` have
both moved between etcd releases.

Components: `etcd:store` (a `has_many` to its members, and a `has_one`
**serves** reference to `plugin:k8s` — the apiserver depends on the store;
it is not *in* it), and one `etcd:member:<hex id>` per member with its role
(leader, follower, learner), URLs, and its own alarms. Actions — compact to
the current revision, defragment (this node's member from the store, any
member through its client URL), disarm an alarm — are all `danger`, so
confirmed, and exist only when the gateway does: a button that cannot work
is worse than none.

**The keyspace is beneath Kubernetes RBAC.** Reading it is reading every
Secret in the cluster, so key names, values and the snapshot need the
`admin` role, checked in the route; a count is on the card for everyone.
`#/etcd/keys` groups the flat keyspace on the next `/` into directories
with counts (a keys-only scan, bounded at 50,000 per prefix and saying so
when it is cut short), and opens a value decoded — JSON as JSON, the
upstream `k8s\0` protobuf envelope by the type its `TypeMeta` names (the
body needs a schema and is shown as bytes), text as text. rustkube writes
JSON. The snapshot is streamed through, un-base64'd chunk by chunk, so the
file a browser saves is one `etcdutl snapshot status` reads.

Traffic — puts/ranges/txns per second, watchers, lagging watchers — is
computed from counter deltas between polls when `/metrics` exports the
counters, and fastetcd does not yet ([fastetcd#29]).

`deploy/verify-etcd.sh` (run with `sc-build deploy/verify-etcd.sh`) runs
the console against a real etcd for the gateway path and a real fastetcd
for today's.

[fastetcd#28]: https://github.com/glennswest/fastetcd/issues/28
[fastetcd#29]: https://github.com/glennswest/fastetcd/issues/29

### sbregistry

The image side on :5100, every 10 s: readiness and warm-up (`/readyz` —
ready with a failed warm-up step is a warning that names the step), the
**catalog** (`/v1/catalog/images`, §Images) with media jobs merged in, and
golden records, clones, pallets and pushed images. Creates: golden and
clone forms. sbregistry serves no stormview feed, so the plugin maps its
JSON. **Not built:** a credential — a registry with an auth file and no
anonymous pull answers these reads 401 (#35).

### Creating things — the `Creator` contract

OpenShift puts a **+ Create** on every list and an *Import YAML* in the
top bar. Here the same two things are declared, not built: a plugin's
`creators()` returns `Creator { id, label, at: [hash routes], mode: yaml |
form, method, path, template | fields }`, the host stamps the owner and
serves them at `/api/v1/console/creators`, and the SPA offers each one on
the routes it names (`"*"` for everywhere). A YAML creator posts the editor
text as `application/yaml`; a form creator posts its fields as one JSON
object. A creator whose objects live in a project says `project: true`,
and the dialog asks which of the viewer's projects, with New project
inline — `?project=` on a YAML post, `namespace` on a form (§Projects). A
form field may be a `checklist` whose options the dialog fetches from
`source` as the viewer (the VM form's SSH keys). The kubernetes plugin's
`/apply` splits a YAML stream, converts each document to JSON and POSTs it
as the viewer to the collection its `apiVersion`/`kind`/namespace name —
the document's own namespace, else the chosen project, never `default` by
omission — reporting per document. Empty lists say so and offer the
create, rather than showing nothing.

## Cross-cutting services (console-core + binary)

- **Auth** — stormd-compatible: `[[api.users]]` (argon2 `password_hash`,
  roles, SSH keys, an optional `kube_token`) + an optional `auth_token`
  bearer; HttpOnly in-memory sessions (24 h); everything except `/healthz`,
  `/readyz`, `/api/version`, `/api/summary`, the auth endpoints and static
  assets requires a session or bearer; comparisons in constant time. The
  write gate is one check by method (§Who may do what).
- **Config** — one TOML, unknown keys refused; every key and default is in
  the README. Fleet-discovered endpoints (every node's stormdrive) need
  none.
- **Health** — `/healthz` (process, `ok`), `/readyz` (plugins; 503 on
  Error). **No metrics endpoint.**
- **stormd summary** — `GET /api/summary` in stormd's plugin-card shape, so
  the console's own container card shows plugin count, node count, and
  health.

## Deployment

On StormCOS the console is a **service golden** that stormcos's
`deploy/build-goldens.sh` builds (`service_golden stormconsole 32M … 9094`):
the static musl binary with the SPA embedded, under stormd (whose own API
is on 9194), a **flat** config — `listen_addr = "0.0.0.0:9094"`, `data_dir =
"/var/lib/stormconsole"` — the `stormconsole-data` and `stormconsole-logs`
volumes, exit 78 not restarted, started on single-node clusters. The
console accepts the flat shape as well as its own sectioned one. The
golden's liveness path is `/admin/healthz` today, which the console does
not serve — stormcos#102 (it must be `/healthz`).

Outside StormCOS, `Containerfile` builds the same thing on `stormdbase`:

```
FROM registry.gt.lo:5000/stormdbase:latest
COPY stormconsole /app/stormconsole
COPY config/stormd.toml /etc/stormd/config.toml
COPY config/config.toml /etc/stormconsole/config.toml
EXPOSE 9080 9094 22
ENTRYPOINT ["/stormd"]
```

— stormd on 9080 supervising the console with an HTTP liveness probe on
`:9094/healthz`, `no_restart_exit_codes = [78]` and a `[process.ui]` proxy.
The multicast listener needs the fleet network (host or macvlan): a
bridged/NAT'd container cannot join the group.

Startup failures are one line on stderr and a distinct exit status: 78
(`EX_CONFIG`) for a config the console cannot run on, 1 for a port it
cannot bind. stormd archives the run's output, so that line is what a
person without a shell on the node will eventually read.

## Repository layout

```
stormconsole/
  Cargo.toml                 # workspace
  crates/
    console-core/            # plugin trait, registry, access, feeds, proxy, creators, events
    stormconsole/            # binary: axum server, config, auth, SPA embed
    plugins/
      kubernetes/            # rustkube client, watch cache, components, projects, apply
      vm/                    # KubeVirt objects, doors, settings, disks, keys, snapshots, network
      vmimages/              # vmcloud-image-operator: catalogue, goldens, local copies
      fleet/                 # nodes from the log group, node drill-down, local stormd services
      logs/                  # multicast collector, redb ring, query API, SSE
      stormdrive/            # every node's drives (Hardware)
      stormstorage/          # storage pools (a feed)
      stormblock/            # the block engine
      sbregistry/            # the registry: catalog, goldens, clones, pallets, images
      fastetcd/              # the datastore: /metrics + etcd's v3 gateway
      stormipmi/             # bare metal: the Machines API and SOL
  web/                       # Svelte 5 SPA (stormview npm); web/dist committed, embedded
  config/                    # config.toml (example), stormd.toml (Containerfile)
  deploy/                    # verify-*.sh — live checks run with sc-build
  Containerfile
  docs/architecture.md       # this file
```

## Open upstream gaps (Core Rule 11)

What the console needs from other components and does not have, each
filed on its owner. The console says so on the page where the gap shows.

| Repo | Issue | What waits on it |
|------|-------|------------------|
| rustkube | [#59](https://github.com/glennswest/rustkube/issues/59) no `SelfSubjectAccessReview` | one call per viewer instead of a probe per namespace; showing an action only when it would be allowed |
| rustkube | [#100](https://github.com/glennswest/rustkube/issues/100) a custom resource's DELETED watch event names the plural as its namespace | a deleted VM instance or snapshot leaving a watching console |
| rustkube | [#101](https://github.com/glennswest/rustkube/issues/101) `stringData` not folded into `data`; [#102](https://github.com/glennswest/rustkube/issues/102) a claim's phase not defaulted | readers taking `data` alone; worked around here |
| rustkube, rustkube-node | #55, #34 (closed as duplicates of stormvm#5) | `kubectl logs` for ordinary pods — reopen if wanted |
| stormcos | [#38](https://github.com/glennswest/stormcos/issues/38) fleet lifecycle has no API | join, promote, demote, drain |
| stormcos | [#94](https://github.com/glennswest/stormcos/issues/94) the engine token; [#102](https://github.com/glennswest/stormcos/issues/102) the golden's health path | Storage on a v18 engine; a golden stormd does not restart |
| stormpump | [#11](https://github.com/glennswest/stormpump/issues/11) Cilium metrics, Hubble, relay | the flow view (#4) |
| stormvm | #16 pod network, #18 device verb, #19 memory resize, #41 accessCredentials, #45 snapshot step/disks/size | VMs under isolation; hotplug; memory changes; keys into a running guest; the Backup tab's detail |
| rustkube-node | #53 snapshot controller | a snapshot being taken |
| stormblock-registry | [#5](https://github.com/glennswest/stormblock-registry/issues/5) raw media | importing an existing VM disk |
| stormdrive | #12 per-drive usage | usage on other nodes' drives |
| fastetcd | [#28](https://github.com/glennswest/fastetcd/issues/28) v3 JSON gateway, [#29](https://github.com/glennswest/fastetcd/issues/29) traffic counters | members, keyspace and verbs on fastetcd; traffic on its card |
| stormconsole | #36 scale, cordon, drain; #35 registry credential; #15 users without a file, certificate identity, audit; #14 VM metrics over time (cadvisor) | — |

## Phasing (history)

How the console was built; kept for the order, not as a plan. Phases 1–5
are done; the fleet's day-2 actions (4) wait on stormcos#38; remote plugins
(6) are designed above and not started.

1. **Skeleton** — workspace, core, binary, SPA shell, auth, themes,
   aggregated feed, Containerfile.
2. **kubernetes** — watch cache, namespace views, workloads, nodes, events.
3. **logs** — collector + viewer; fleet log deep links.
4. **fleet** — discovery, node pages, beacon; day-2 actions not built.
5. **storage & images** — stormdrive, stormblock, sbregistry, vmimages.
6. **Remote plugins** — not started.
