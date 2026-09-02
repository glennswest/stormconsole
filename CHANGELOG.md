# Changelog

## [Unreleased]
<!-- New unreleased changes go here -->

## [v0.7.0] — 2026-09-02

### 2026-09-02
- **BREAKING:** the fleet log ring moved from SQLite to **redb**, and with
  it from `<data_dir>/logs.db` to `<data_dir>/logs.redb`. Nothing reads
  the old file; it is inert and can be deleted. redb is pure Rust (no C
  toolchain in the golden) and has no page ceiling to hit — the console's
  ring had been failing every insert with `SQLITE_FULL` on a node with
  1.7 TB free
- **feat:** the ring deduplicates on arrival. An entry is keyed on
  host/app/severity/message; a repeat bumps a `count` and a last-seen time
  instead of appending a row, so a service emitting one line thousands of
  times a second costs one entry rather than a million
- **feat:** duplicate counts are visible — `×N` on the line in the viewer
  (with first-seen in the tooltip), a lifetime `duplicates` counter on the
  collector component beside `events` and `received`, and
  `duplicates`/`received` on `GET /api/plugins/logs/summary`
- **feat:** entries expire automatically on two bounds, whichever bites
  first: `[logs] retain_hours` (default 168, measured from *last* seen, so
  a line that keeps arriving keeps its place) and `[logs] ring_cap`
  (default 200000 distinct entries). A timer sweeps every 60s as well as
  pruning on insert, so a fleet that goes quiet still expires what it left
  behind. `[logs] dedup = false` opts out of merging
- **fix:** the live tail updates a row the viewer already holds instead of
  appending one, and the collector broadcasts a repeating line at most
  once a second. A flooding node made the log view unresponsive because
  every duplicate crossed the wire and became a DOM node
- **fix:** a store failure is no longer logged per occurrence. The
  console's own warnings go out over the same multicast group it collects
  from, so a broken ring flooded the fleet it was meant to observe; the
  fault surfaces as component health, with the first failure and every
  ten-thousandth logged
- **perf:** per-host and per-severity summaries are maintained by insert
  and prune rather than computed, since the components feed asks for them
  every few seconds and redb has no `GROUP BY`
- **docs:** README gains a log-ring section; architecture explains the
  store's three constraints and what follows from dedup

## [v0.6.0] — 2026-09-02

### 2026-09-02
- **feat:** two console styles, selectable in the masthead and persisted
  per browser. Theme and style are independent axes: a theme
  (`data-theme`, stormview) is the palette, a style (`data-style`) is the
  chrome — masthead height, row density, corner radius, navigator
  tightness, whether a button shouts. Both styles work on all twelve
  palettes, and switching palette never changes the console's shape
  - `openshift` (default): comfortable density, a near-black masthead
    over a panel-coloured navigator, a 3px accent rail on the active nav
    item, 4px radii, sentence case, hairline-separated rows
  - `esxi`: compact density, a dark teal 40px header, a tighter
    navigator that fits its whole tree on one screen, 2px radii,
    zebra-striped tables, uppercase action labels
- **feat:** the masthead carries its own foreground tokens rather than
  inheriting the theme's — both styles put a dark bar above the content
  whatever the palette, so a light theme was painting dark text on
  near-black. State colours in the health pill are lifted toward white
  for the same reason
- **refactor:** everything density-dependent (navigator, tables, page
  header, empty states, buttons) reads from a style token, so a
  component's scoped CSS never has to know which style is active

## [v0.5.0] — 2026-09-02

### 2026-09-02
- **feat:** enterprise console chrome, in the idiom of the OpenShift
  console and the ESXi host client. A stormconsole-local design layer
  (`web/src/lib/ui/console.css`) sits on top of stormview's palette and
  supplies shape: 4px radii, near-flat elevation, a type scale, tabular
  numerals, focus rings, reduced motion, thin scrollbars. It consumes
  stormview tokens only, so all twelve themes keep working, and stormview
  itself is untouched
- **feat:** masthead — brand mark and wordmark, the namespace selector set
  off as the working scope, a live cluster-health pill (n healthy /
  degraded / failed) beside the feed's connection state, Create as the one
  primary button, and a navigator toggle
- **feat:** navigator — collapsible groups (persisted per browser), a
  stroked icon per item matched from the server's nav feed by label and
  route, an object count on every countable route, an accent rail on the
  active item
- **feat:** one page grammar everywhere — breadcrumb, then title with
  scope and count, then a toolbar (search, state filter, table/card
  switch, result count), then the data. Table is the default view and the
  choice persists
- **feat:** overview — a status band showing the fleet's health as large
  tabular counts over one proportional rule; clicking a state filters
  everything below it. Plugin cards, then each plugin's objects capped at
  eight with Show all
- **feat:** `ResourceTable`, the console's own table: Status says "Ready"
  rather than the feed's `ok`, Kind is a column only when the rows differ,
  the header stays put over a long list, Name is first and destructive
  actions are last, and sorting covers name, status (worst first), kind
  and detail
- **feat:** empty states name what is missing, say why in one line, and
  carry the action that fixes it; failures quote what the plugin returned
- **feat:** `StatusPill` carries a glyph as well as a colour, so state
  survives colour blindness and greyscale
- **fix:** the previous sticky table header never fired — it was scoped to
  an `overflow-x` wrapper that never scrolled vertically
- **fix:** the search field's padding lost a specificity tie with the
  generic control rule, putting the magnifier on top of the placeholder
- **fix:** Overview and Workloads shared one navigator glyph
- **docs:** architecture — the design layer, and why the console keeps its
  own table while rendering stormview's cards directly

### 2026-08-30
- **docs:** filed stormpump#11 (Cilium agent metrics address, Hubble +
  relay enablement) and stormconsole#4 (agent probe, hubble-ui proxy,
  native flows, policy YAML edit); recorded in the integration-gaps table

## [v0.4.0] — 2026-08-30

### 2026-08-30
- **feat:** a working console on a node with no config — every upstream
  defaults to this node's own daemon: rustkube `https://127.0.0.1:6443`
  (unverified TLS; sno is anonymous-admin), stormblock :9090, sbregistry
  :5100, stormdrive :9092, stormstorage :9093, stormd instances on the
  StormCOS port layout. Verified against sptest (192.168.8.106) from dev:
  133 components — 10/10 rustkube kinds, 84 volumes, 8 services
- **feat:** console-core `Feed`/`FeedPlugin` (poll an upstream stormview
  feed, re-prefix ids and relations, actions through the plugin proxy),
  `proxy::forward` + router (method, query, content-type, body pass
  through), `value` helpers
- **feat:** stormdrive and stormstorage (new) are feed plugins over the
  node's own feeds; stormblock maps volumes/slabs/arrays/exports/drives
  with health, metrics, edges and a DELETE action; sbregistry maps
  readiness + warm-up (a failed step is a named warning), goldens, clones,
  pallets, images
- **feat:** fleet — nodes from the log collector's hosts (recency health,
  link to logs), this node's stormd services discovered on loopback with
  their processes and start/stop/restart via proxy; `[fleet] stormd_host`
  to look at one node from elsewhere
- **feat:** create, the OpenShift way — `Creator` contract
  (`/api/v1/console/creators`), kubernetes *Import YAML* + per-kind
  templates through `POST /api/plugins/k8s/apply` (per-document results,
  conflicts reported), stormblock volume/export and sbregistry golden/clone
  forms; **+ Create** on every list and in the top bar; empty lists say so
  and offer the create
- **fix:** the UI invokes actions with the method the feed declares (a
  stormblock delete is a DELETE); the logs view takes `?host=` so node
  cards deep-link
- **docs:** README configuration defaults table and *Creating things*,
  architecture plugin sections rewritten to what is built, example config
- **feat:** Cilium — endpoints (state, address, identity, edge to the pod),
  nodes, identities (namespace + labels), CiliumNetworkPolicy /
  Clusterwide and core NetworkPolicy (selector + rule counts, DELETE
  action) through the apiserver's `cilium.io/v2`, under a Cilium card
  (endpoints ready, identities, nodes, policies); Networking nav items;
  policy creators. Optional CRD kinds count as synced-empty when the CRD
  is not served, so a cluster without Cilium stays honest
- **chore:** `config/stormd.toml` sets `no_restart_exit_codes = [78]` — stormd
  v0.7.0 (stormd#2, done) marks the console failed once on a bad config
  instead of restarting it
- **docs:** record stormpump#7 and stormd#2, filed for #3's follow-through,
  in the integration-gaps table and work plan

## [v0.3.0] — 2026-08-30

### 2026-08-30
- **fix:** #3 crash loop on StormCOS — the golden's flat node-service
  config (`listen_addr`, `data_dir`) was rejected by `deny_unknown_fields`
  and the console exited 1 on every start. Both keys are now accepted;
  `listen_addr` wins over `[api] bind`, and `[logs] db_path` defaults to
  `<data_dir>/logs.db` (`/var/lib/stormconsole`, the golden's data volume)
- **fix:** startup failures print one line on stderr
  (`stormconsole: fatal: …`, naming the file and line for config errors)
  and exit 78 (EX_CONFIG) for a bad config, 1 for a port that cannot be
  bound — instead of anyhow's Debug dump and exit 1 for everything
- **test:** config parsing — stormpump's exact golden file, the example
  file, defaults, precedence, unknown key named with its line, bad address
- **docs:** README §Configuration, example config, architecture deployment
  note on the StormCOS golden and exit statuses

### 2026-08-28
- **feat:** logs plugin phase 3 — the fleet log collector: multicast join
  (socket2, SO_REUSEADDR), lenient RFC 5424 parse with source-IP fallback,
  SQLite ring store (WAL, 200k-row cap), query/summary APIs and SSE live
  stream; collector component with events/hosts metrics
- **feat:** fleet log viewer UI — host/severity/search filters, live
  follow over EventSource, severity coloring
- **chore:** Verified live on dev: synthetic stormcast datagrams parsed,
  stored, filtered, summarized, and delivered over SSE

## [v0.2.0] — 2026-08-28

### 2026-08-28
- **chore:** Live verification on dev against a real rustkube apiserver
  (fastetcd-backed): 10/10 kinds synced, correct health derivation,
  delete-pod action + watch removal confirmed
- **feat:** kubernetes plugin phase 2 — rustkube client (Value-based,
  bearer auth, NDJSON `?watch=true` streaming with ordered apply and
  re-list backoff), watch-backed cache over ns/node/pod/deploy/sts/ds/
  job/cronjob/svc/pvc, components mapping with kubectl-grade health
  derivation and namespace/node relations, delete-pod action route,
  events endpoint
- **feat:** UI — namespace selector (top bar, persisted), `#/k8s/:kind`
  list views over the feed, `#/k8s/events` table with Warning
  highlighting and auto-refresh
- **fix:** enable reqwest `stream` feature for watch streaming

## [v0.1.0] — 2026-08-28

### 2026-08-28
- **feat:** Cargo workspace — console-core (ConsolePlugin trait, nav merge,
  Registry with aggregated stormview feed + ws snapshot push, upstream
  Probe), stormconsole binary (axum :9094, TOML config, stormd-compatible
  auth, embedded SPA, stormd card summary), six plugin skeletons (k8s,
  fleet, logs, drive, sb, reg)
- **feat:** Svelte 5 SPA shell on stormview — server-driven nav (top bar +
  sidebar), Overview grouped by plugin, GridView, LoginPanel gate, themes
- **feat:** Containerfile (FROM stormdbase) + stormd supervisor config with
  liveness probe, plugin UI proxy, and card summary
- **chore:** Cross-project issues filed: stormblock-registry#24 and
  stormdrive#1 (stormview components feeds), rustkube#55 (pod /log
  subresource), rustkube-node#34 (kubelet /containerLogs), stormcos#26
  (node capability beacon)
- **chore:** Verified on dev.g8.lo — build, tests, live endpoint + auth
  smoke, 5.9 MB musl release with embedded SPA

### 2026-08-26
- **docs:** Initial architecture design (`docs/architecture.md`) — pluggable
  console on stormd + stormview; plugins: kubernetes (rustkube), logs
  (stormcast collector), fleet, stormdrive, stormblock, sbregistry
- **docs:** Work plan (`CLAUDE.md`), README
- **chore:** Repository bootstrap, .gitignore, private GitHub repo
