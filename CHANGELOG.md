# Changelog

## [Unreleased]
<!-- New unreleased changes go here -->

### 2026-10-06 — A network edit that shows (#50)
- **fix:** the VM settings form's Network value and the pending check read
  the `storm.io/bridge` annotation the save writes (as stormvm reads it:
  `storm.io/bridge.<iface>`, then `storm.io/bridge`, then `spec.networks`).
  A saved edit read back as `pod` and never went pending, which looked like
  "nothing happened".
- **fix:** a network save clears a per-interface `storm.io/bridge.<iface>`
  that would shadow it, reads the returned definition back and errors
  instead of saying "written" when it does not carry the value, and says
  what it wrote and that `spec.networks` is left as it was.
- **docs:** architecture's settings section no longer says the network is
  refused.

## [v0.27.1] — 2026-10-06

### 2026-10-06 — A machine outside policy says so (#51)
- **fix:** isolating a project no longer calls a VM behind the
  hypervisor's NAT (stormvm#16) or on a host bridge isolated: the answer
  and the project card name the machines isolation does not reach
  ("Except 2 machines — vm1 and vm2 — behind the hypervisor's NAT …").
  `GET /api/plugins/k8s/projects/{p}` carries them as `outside`.
- **fix:** such a VM's row says `policy = none applies (NAT)` (or host
  bridge) and no longer points at a Cilium endpoint, so no policy is drawn
  as selecting it; the VM page says no network policy or isolation applies,
  whether the project is isolated, and which policies would select it on
  the pod network.

## [v0.27.0] — 2026-10-05

### 2026-10-05 — fastetcd over mutual TLS (#47)
- **feat:** `[fastetcd] ca_file`, `cert_file`, `key_file`: the client
  verifies fastetcd against that CA only and presents the pair; reread when
  a file changes; a missing or bad file is the datastore card's error
  naming it, not a failed start. Half a pair, or certificates with a
  non-`https://` url, is a config error (exit 78).
- **fix:** reqwest offers `h2`: fastetcd's TLS port (tonic) advertises only
  `h2` in ALPN, and an http/1.1-only client could never complete the
  handshake. `Cargo.lock` gains `h2`.
- **fix:** the datastore card says the whole cause of a failed request
  (`invalid peer certificate: UnknownIssuer`, `CertificateRequired`), not
  "client error (Connect)".
- **fix:** the client port not answering while the metrics listener does is
  an error ("the client port did not answer: …"), not a healthy store.
- **test:** `deploy/verify-etcd-tls.sh`.

## [v0.26.0] — 2026-10-05

### 2026-10-05 — destructive storage: storage-admins only, typed, as the user (#82)
- **feat:** `console_core::storage`: one rule naming which requests are
  destructive storage (drive format/sanitize/wipe/partition/destructive
  test/worker jobs on any node; the engine's own destructive list; RAID
  set create and member changes; slab create; forge) and the
  `storage.storm.io` resource and verb, and a SelfSubjectAccessReview asked
  **as the viewer**, cached 30 s, failing closed.
- **feat:** the feed drops those actions for anyone the review refuses:
  storage-viewers and everyone else see every drive, set, slab and volume
  read-only.
- **feat:** the host refuses them (403, the reason) without the review's
  yes — for every console role, `admin` and the console's `auth_token`
  included — and without `X-Storm-Confirm` equal to the drive's serial or
  the object's name (428 naming it); each one done or refused is logged.
- **feat:** the proxies send the **user's own bearer** for them, never the
  console's or the engine's node token, so the component's own check
  decides. **BREAKING:** deleting a volume through the console now needs
  storage-admin, and the engine refuses a user's bearer until
  stormblock#274.
- **feat:** `GET /api/v1/console/guard?method=&path=`; the page asks for
  the typed serial after the OK on any action answered 428.
- **test:** `deploy/verify-storage-guard.sh`.

## [v0.25.0] — 2026-10-03

### 2026-10-03 — the Cluster page, from stormcluster (#63)
- **feat:** `crates/plugins/stormcluster` (name `cluster`): stormcluster's
  feed (`[stormcluster] url`, default this node's :9102) — the cluster or
  this SNO, members, discovered peers, the last operations — polled every
  3 s. Its proxy forwards only the operator API (not `/api/v1/record`);
  reads are open, every write is admin-only with stormcluster's
  `token_file` bearer added server-side and an audit line per act.
- **feat:** answers a page can read: a request forwarded to its
  coordinator is unwrapped and names it; a `409 {"refused": [...]}` keeps
  its reasons and carries them as `error` too; a dry-run plan's steps get
  stormcluster's own sentence (filed stormcluster#11 to serve it).
- **feat:** `#/cluster` (Cluster → Membership): the cluster, members (role,
  state, address, hardware, the cluster CA), the nodes discovered, and
  operations with their steps. Every action but Resume shows stormcluster's
  plan (`?dryRun=true`) — steps and warnings — before it runs; Split asks
  keep or wipe; a refusal is shown as its list of reasons. Form a cluster
  (name, masters 1/3/5, workers), Join nodes (as workers, or masters in
  pairs) and Promote workers (in pairs) are forms that build `POST
  /api/v1/operations`.
- **test:** `deploy/verify-cluster.sh`: three real stormclusters on
  loopback addresses and a private multicast group, stand-ins for the node
  lifecycle API and fastetcd's gateway, a console with an admin and an
  operator; the proxy with curl and the page in Chromium.

## [v0.24.0] — 2026-10-02

### 2026-10-02 — registry images and instances (#70)
- **feat:** the console calls a golden a **registry image** everywhere it
  shows one, and the copy-on-write clone a pod, VM or boot runs on its
  **instance** ("instance of <registry image>", "clone of …"). Nav
  (Images → Registry images, VM registry images), the Images page and its
  groups, Create (Registry image, Instance, VM registry image), action
  labels (Make / Delete registry image), form labels and hints, details,
  the VM page's disks and images, the Drives page, the pod page (a
  Registry image row: `cilium@<commit> (sealed digest …, built … from
  repo@commit)`) and error messages.
- **feat:** APIs keep `golden`: ids, kinds, relation and metric names, form
  fields, JSON. The SPA translates the tokens it shows
  (`web/src/lib/ui/words.js`; `console_core::words` for details the backend
  builds from a kind), and a `rel=` link in either vocabulary works. The
  API's word is named once, in the pod page's Registry image tooltip,
  which also carries stormcentral's own name for the object.
- **test:** `verify-pod-page.sh` gains a words pass in Chromium over every
  page that shows a registry image or an instance (#70).

## [v0.23.1] — 2026-10-02

### Fixed
- `Cargo.lock` lists plugin-kubernetes's new `chrono` dependency: v0.23.0's
  lock was stale, so the golden's `--locked` build refused it (#71).

## [v0.23.0] — 2026-10-02

### 2026-10-02 — the pod page (#69)
- **feat:** a pod has a page, `#/pod/<ns>/<name>`, and its row links to it
  (plus a Logs action). Overview: phase, node, every pod address, QoS,
  priority, service account, restart policy, the owner chain (ReplicaSet →
  Deployment, Job → CronJob), conditions with times, labels, annotations,
  events; a Containers table (init and ephemeral included) with state,
  restarts and last termination; an Images card per container — the
  reference, the `sha256:` digest from the node's `imageID`, the image ID,
  pull policy, when it was last checked, and how it was built. Network:
  addresses, host network, DNS (policy and config), Cilium (datapath,
  identity, the policies that select it), the interfaces when the CNI
  records them, traffic, the Services that select the pod with their
  cluster IPs and ports and whether this pod is among their ready
  Endpoints, and the container ports. Logs, Events, YAML.
- **feat:** logs in the UI, as OpenShift shows them: per container, opening
  on the last 1,000 lines and following live (pause holds new lines and
  says how many; resume adds them), search with highlight, wrap,
  timestamps, download; the previous run from the node; and **the last 5
  runs per container kept by the console** — it watches restart counts and
  fetches `previous` as each run ends, saying how many it missed between
  two looks. `GET /api/plugins/k8s/pods/{ns}/{name}/log` streams the
  apiserver's `pods/log` as the viewer; `…/runs/{container}/{run}`.
- **feat:** traffic counters — `GET …/pods/{ns}/{name}/traffic` reads the
  kubelet's `/metrics/cadvisor` on the pod's node (:10250), rx/tx per
  interface; the page polls every 5 s and draws rates as sparklines.
  rustkube has no node proxy (rustkube#108), so the console dials the
  kubelet with the viewer's bearer or its own.
- **feat:** `[stormcentral] url / token_file` (optional): a `stormpump://`
  image shows the newest golden stormcentral built for that component —
  built at, by, commit, build id, tar sha256 — labelled as the newest, since
  the node does not say which it runs.
- **feat:** the VM page gains Images (each disk's source and digest),
  Metadata (labels, annotations, conditions, boot time) and the gaps — no
  per-VM counters yet (stormvm#48) — with the serial log on its console tab.
- **docs:** what the node does not report is named on the page where it
  would be, with its issue — filed **rustkube-node#130** (lastState and
  termination reason, a digest for `stormpump://`, when an image was
  resolved, OCI build info) and **rustkube-node#131** (packets, errors and
  drops; network-status with MTU, gateway, routes and CNI; runs before the
  previous one). README, architecture, presentation, example config.
- **test:** `deploy/verify-pod-page.sh` — a real fastetcd + rustkube
  v0.15.3, a stand-in kubelet and stormcentral, a real console; the API
  checked with curl and every tab driven in headless Chromium.

### 2026-10-02
- **fix:** the fleet's default `[fleet] stormd_ports` and the node page's
  port layout missed six service goldens' stormd APIs — stormrdp 9201,
  stormcluster 9202, nextnfs 8180, minismbd 8545 (ports outside
  9180–9199), and, on the node page only, stormupdate 9188 and nfsop 9198.
  Checked against stormcos `build-goldens.sh` and stormcentral's component
  registry (#61).
- **fix:** fastetcd's gap lines name the release that closes them — the v3
  JSON gateway since fastetcd v1.8.0 (fastetcd#28), the traffic counters
  since v1.7.0 (fastetcd#29) — instead of saying fastetcd does not serve
  them yet.
- **docs:** refreshed from the code. Since 2026-09-25 the only code change
  after the 2026-09-28 pass was #56 (the Create dialog), already
  documented; what was stale was upstream and the platform (#61):
  - fastetcd serves the gateway and the counters now; the gaps table no
    longer lists them, and #64 is the live check that has not been run;
  - stormcos#94 is done: on a node stormcos sets `[stormblock] token_file
    = "/run/stormblock/engine/api_token"`, removed from the open gaps;
  - the node config path is `/etc/stormconsole/stormconsole.toml`
    (`--config`, from stormcentral's component registry);
    `/etc/stormconsole/config.toml` is only the default off a node;
  - how it ships names the component registry entry;
  - the golden's `/admin/healthz` probe is not a probe that restarts a
    healthy console, as the deck said: the console answers it with the
    app's 200, so it always passes (stormcos#102, stormcentral#226);
  - the deck's status line: v0.22.0, and that most pages have not been
    viewed in a browser (#58).

### 2026-09-30
- **fix:** Create dialog (#56) — "+ New project…" could not be named, and
  opening Create VM stopped the whole console. The dialog's reset effect
  wrote state it then read (`lists = {}`, then `lists[f.name] = …`), so it
  re-ran itself until Svelte threw `effect_update_depth_exceeded`, after
  which nothing on the page updated. Its writes are now untracked and run
  once per opening. The project picker's default no longer overrides a
  choice: a project-list reload fills it only while it is empty or names a
  project that has gone, and the new-project name is suggested once.
- **test:** `deploy/verify-create-project.sh` + `create-project.browser.cjs`
  — the SPA built from the commit, driven by headless Chromium against a
  real console, fastetcd and rustkube. It failed on the unfixed code with
  that error and passes on the fix.

### 2026-09-28
- **docs:** refreshed from the code again. Since the 2026-09-27 pass, only
  #37 (VM lifecycle) and #29 (drive usage) changed code, and `config.rs`
  did not change at all, so config keys, defaults and ports are as
  documented. Corrected:
  - `docs/architecture.md` still said VM lifecycle was `spec.running` alone;
  - the README's live-check table lacked `verify-vm-lifecycle.sh` and the
    #29 half of `verify-drives.sh`;
  - CLAUDE.md said to build on `root@dev.g8.lo`. It now says sc-build, and
    how the committed `web/dist` is rebuilt;
  - CLAUDE.md listed Phase 5 and YAML edit as open, though both shipped.

  No new doc promises the code does not keep were found.

## [v0.22.0] — 2026-09-28

### 2026-09-28
- **feat:** each drive's usage, slabs and volumes, on every node (#29). The
  Drives page reads every node's stormdrive usage in bytes (stormdrive
  v0.13.0+, #12/#13) through a new `GET /api/plugins/drive/usage`, fetched
  while the page is open and cached 10 s. A picked drive shows:
  - where it is, including its controller (stormdrive#3's `hba`);
  - what is left;
  - used / free in slabs / not in a slab;
  - overcommit, committed and headroom;
  - a drain in progress;
  - its slabs with their use;
  - the volumes on it, from this node's engine placement
    (`GET /api/plugins/sb/placement`, stormblock#136, cached 15 s). Each
    volume has its consumer linked, its legs (shared ones marked) and the
    worst state of its slabs there.

  Totals now cover every node, and there is a Draining filter. **Pools**
  (`#/drives?group=pool`, Hardware → Pools) sum every slab per node, role
  and tier. Committed and headroom are summed only where every slab
  reports them (stormblock#152), and the engine's pool pressure is shown. A
  drive with no usage says why (its node's stormdrive did not answer, or
  predates usage). A drive on another node says its volumes are not read
  here.
- **test:** `deploy/verify-drives.sh` checks all of the above against
  stand-ins in stormdrive v0.15.0's and stormblock's own shapes: 25 checks,
  plus a node going away.
### 2026-09-27
- **fix:** VM Restart, Stop and Start are offered from every phase, not
  only `Running` — a machine whose start failed, or stuck scheduling, can
  be restarted from its row (#37, from stormvm#21). Stop is disabled only
  when the machine is stopped, Start only while it runs. The verbs are made
  true from every phase: Start from a failed or stuck instance replaces
  it; Restart writes `running: true` before deleting the instance, so it
  also brings up a stopped machine; Stop writes `running: false` and
  deletes a lingering instance. A defined machine's Stop now goes through
  its definition (it deleted the instance, which the definition put
  straight back). A Failed row shows `status.message` beside the reason
- **docs:** refreshed from the code and the upstreams as they are today
  (#40). Config keys, defaults and ports re-checked against `config.rs`:
  unchanged. Corrected:
  - pod logs are served upstream (rustkube v0.8.1, rustkube-node v0.3.0)
    and not shown here for want of a pod page (#12); a terminal waits on
    rustkube-node#56;
  - rustkube#100 is fixed in v0.15.2, rustkube#59 (access reviews) in
    v0.9.0, stormblock-registry#5 (media import) in v0.19.0, and
    stormpump#11 (Hubble relay, agent metrics) is closed. The gaps table
    moves them to the console's own issues (#45, #44, #4);
  - claims of the `stormblock` class are served by the built-in stormblock
    driver (the node's kubelet through its engine), CSI only for other
    classes;
  - unknown `/api/*` and `/ws/*` paths answer a JSON 404, and `/metrics`
    still falls through to the app (#41).

  Filed what the docs promised and the code does not do: #44 (VM disk
  import), #45 (access reviews, RBAC-aware actions), #42 (where SSH keys
  live, a decision) and cadvisor#15 (per-VM stats).

## [v0.21.0] — 2026-09-27

### 2026-09-27
- **test:** short, medium and long test containers per the stormcos test
  standard (#27): `test/` (its own crate and lock), `test/build.sh`,
  `test/Containerfile` (scratch, `/test`), `test/stormconsole-test.yaml`
  (the Job, namespaced RBAC, `requires: [service: stormconsole]`). JSON
  lines and exit 0/1/2; everything in the run's namespace and removed;
  skip where the console is absent or needs a token it was not given.
  `deploy/verify-tests.sh` runs them against a real console and rustkube.
  Filed rustkube#113 (Service creates ~30× a ConfigMap, serialized).
- **fix(server):** an API path nothing serves (`/api/plugins/<a plugin
  that is off>/…`) answers 404, not the app's HTML with 200 — found by the
  medium suite.

### 2026-09-26
- **docs:** `docs/presentation.md` — the console's purpose and
  functionality in eleven Marp slides, from the code and the #21 docs:
  the problem, where it sits (stormcentral's graph and what the code
  reads), how it works, what it does today, interfaces, how it ships,
  planned work marked as planned, status and the open issues that matter
  (#22).

## [v0.20.1] — 2026-09-26

### 2026-09-26
- **fix(auth):** signing in with the `auth_token` made a session named
  "admin" with no roles, which could not write; it is now the token's
  session, an administrator's, as the bearer always was (#21).
- **fix(auth):** the bearer token is compared in constant time, as the
  password already was.
- **fix(auth):** `/api/version` answers without a session, as its comment
  said and the masthead needs; `/metrics` is no longer on the open list —
  the console serves none.
- **fix(vm):** a definition wanting to run with no instance no longer says
  "nothing places one yet" — rustkube's controller-manager and scheduler
  do (rustkube#72); it says what makes one and what to check.
- **fix(images):** "Delete golden" deletes the `CloudImage`
  (storm.io/v1alpha1, cluster-scoped) through the apiserver as the viewer;
  it called the operator's `DELETE /api/v1/images/{name}`, which does not
  exist (405).
- **fix(fleet):** a node's page probes only ports that serve a feed — not
  stormblock 9090, stormvm 9095 or sbregistry 5100, which were always
  "silent" — and adds stormipmi 9097, the stormd APIs 9195–9199 and
  stormlb's 180; this node's stormd scan adds 180 and 8269 (stormimds).
  Checked against stormcos `build-goldens.sh`.
- **docs:** the README rewritten from the code (#21): every plugin and
  its upstream, every config key with its default, flags and exit codes,
  ports and health (no metrics), auth and roles, the host API, how it
  ships, and what is not done with its issue. `docs/architecture.md`
  corrected against the code — the plugin contract, watched kinds, actions
  that exist (scale/cordon/drain do not), pod logs, logs API, fleet
  drill-down, stormdrive fleet-wide, the VM lifecycle, deployment,
  repository layout; Drives, Images, Machines and Projects have their own
  sections; remote plugins and phasing marked as design and history; the
  upstream gaps table current. Crate doc comments brought up to the code.
  Filed stormcos#102 (the golden's health path) and #35.
- **docs:** config comments corrected (`--hash-password`, the redb log
  ring); the example config shows `password_hash`, roles and ssh_keys.

## [v0.20.0] — 2026-09-26

### 2026-09-26
- **feat(drives):** the Drives page at rack scale (#32). Every node's
  drives, not one's: this node's stormdrive plus each fleet host's at
  :9092 and `[stormdrive] nodes`, each drive stamped with its node, remote
  actions through per-node proxies. The page is a map: each chassis bay by
  bay, coloured by health, temperature, wear or usage; grouped by chassis,
  node or rack; filters for failing, degraded, rebuilding, full, hot, out
  of fleet and spares; totals up to EB; click a bay for the drive, its
  actions and its usage. A list view remains.
- **feat(storage):** per-drive usage from this node's engine
  (`sb:use:<serial>`) and array member state (`sb:member:<dev>`), until
  stormdrive reports usage (stormdrive#12).
- **feat(k8s):** a node's `rack`, from its `topology.storm.io/rack` label.
- **chore:** `web/src/lib/drivemap.js` + `drivemap.test.mjs` (1,600
  drives, plain node) and `deploy/verify-drives.sh`.

## [v0.19.0] — 2026-09-25

### 2026-09-25
- **feat(storage):** Volumes are what is attached, and images are the
  registry's (#19). Storage → Volumes lists only volumes in use (stormblock
  v18.1.0's `kind`/`in_use`), each with its consumer — linked to the PVC or
  VM — and how it is served; Delete is disabled while in use. Unattached
  volumes have their own item; goldens, blanks, media, snapshots and
  templates are not volumes here. An older engine keeps every unsealed
  volume in Volumes and says why.
- **feat(images):** Images → Catalog (`#/images`) from sbregistry's
  `/v1/catalog/images`: grouped by kind, base lineage, clones, releases,
  digest, location, the engine volume underneath; media jobs merged in
  ("downloading from X, n%", failures with their fault).
- **feat(storage):** `[stormblock] token_file` — a guarded engine is read
  and proxied with its token (v18 answers 401 to reads without it).
- **feat(core):** `proxy::router_as`, a proxy carrying a bearer.
- **chore:** `deploy/verify-images.sh`. Filed stormcos#94 (the token on
  nodes).

## [v0.18.0] — 2026-09-25

### 2026-09-25
- **feat(ipmi):** the Machines page, served by stormipmi (#31). A new
  plugin over stormipmi's Machines API: the fleet by service tag — BMC,
  power, state, the release each machine boots (set it, confirmed, read
  back), boot intent, test marks, the default image, new hosts to adopt —
  and each machine's SOL console relayed through the console. Reads are
  open; every write and console typing are admin-only, enforced in the
  plugin, which carries stormipmi's write token server-side and logs who
  acted. Config `[stormipmi] enabled/url/token_file`.
- **feat(core):** `proxy::forward_as`, a forward carrying the console's
  own bearer for an upstream.
- **chore:** `deploy/verify-machines.sh`, the live check on stormipmi's
  own rig (ipmi_sim, stand-in forge) with an admin and an operator.

## [v0.17.0] — 2026-09-25

### 2026-09-25
- **feat(k8s):** projects are the top of the console (#28). The masthead's
  selector is a Project selector over the viewer's projects (rustkube's
  `project.openshift.io/v1`); a Projects page and New project
  (`ProjectRequest`, the requester made admin); a Project tab on each —
  requester, members (admin/edit/view RoleBindings, added and removed),
  isolation, delete. All as the viewer, so RBAC decides.
- **feat(k8s):** Isolate a project: NetworkPolicies `storm-isolate` (within
  the namespace only) and, opt-in, `storm-isolate-dns`; shown as a badge,
  removable.
- **BREAKING (pre-1.0):** every create targets a project, chosen in the
  dialog with New project inline. Nothing is created in `default` by
  omission: templates no longer name a namespace, `/apply` takes
  `?project=` and refuses a namespaced document with neither, VM create
  refuses a missing namespace. System namespaces (`default`, `openshift`,
  `kube-*`, `openshift-*`, `[kubernetes] system_namespaces`, default
  `["cilium"]`) are refused as targets (`/apply` allows `admin`).
- **feat(k8s):** a Cluster section (admin) — Nodes, Namespaces,
  PersistentVolumes, StorageClasses, CRDs, ClusterRoles, newly watched.
- **feat(k8s):** a claim waiting for its first consumer is Idle, says
  "Pending — provisioned when a pod or VM uses it", and offers Attach to a
  VM. A claim with no phase (rustkube#102) is Pending.
- **feat(ui):** lists always show a namespaced row's project, grouped by
  project when they span several.
- **refactor(fleet):** "Node services" left the navigator; a node's daemons
  are on its page.
- **chore:** `deploy/verify-projects.sh`, the live check as three real
  identities. Filed rustkube#102.

## [v0.16.0] — 2026-09-25

### 2026-09-25
- **feat(vm):** SSH keys uploaded once and given to every machine (#26).
  Account → SSH keys (the key icon in the masthead): paste or upload a
  `.pub` or an `authorized_keys` file, name, delete; a private key is
  refused by name. Kept as Secret `<user>-ssh-keys` in
  `[vm] ssh_keys_namespace`, with a copy in each namespace a machine of
  theirs is in, all refreshed on every change. The create form lists your
  keys as checkboxes, all ticked; every chosen key goes into the
  cloud-init seed and into `accessCredentials` (`noCloud`). The VM page
  shows which keys a machine has and from where, with "Add my keys"
  (`qemuGuestAgent`). The console config's keys are offered too.
- **feat(ui):** a `checklist` create-form field whose options are fetched
  as the viewer.
- **chore:** `deploy/verify-vm-keys.sh`, the live check. It found
  rustkube#101: `stringData` is stored as written, not folded into `data`.

## [v0.15.0] — 2026-09-25

### 2026-09-25
- **feat(vm):** a Backup tab (#25). Snapshot schedules a
  `snapshot.kubevirt.io/v1beta1` `VirtualMachineSnapshot` (optional name
  and note) and returns; the tab lists the machine's snapshots — time,
  state and the step it is on, disks, size, indications, the reason on
  failure — with Restore (a `VirtualMachineRestore`, refused with a
  sentence while running, not ready, or with no definition) and Delete,
  and the restores made from them. Re-reads every 2 s while anything is
  moving. Without the CRDs it says so (stormpump#28); a snapshot nothing
  has picked up in a minute says so (rustkube-node#53); step, disks and
  size are read when the node reports them (stormvm#45).
- **chore:** `deploy/verify-vm-snapshots.sh`, the live check. It found
  rustkube#100: a custom resource's DELETED watch event names the plural as
  its namespace, so watchers never drop it.

## [v0.14.0] — 2026-09-25

### 2026-09-25
- **feat(vm):** a VM's addresses, asked against done (#24). Per interface:
  what the spec asked for (pod network and binding, host bridge, multus),
  what the node did (`storm.io/binding`), the MAC, every address, and
  whether the address is reachable, in one sentence. A pod network run as
  a NAT inside the hypervisor says so and names stormvm#16. The list row
  carries every address (`ip`), "no address yet" for a running machine
  without one, and `network = NAT, not pod`; the page's Network card is a
  table with copy buttons.
- **feat(ui):** `ResourceTable` puts a copy button on any metric whose
  value is IP or MAC addresses.
- **chore:** `deploy/verify-vm-net.sh`, the live check over a real
  fastetcd + rustkube.

## [v0.13.0] — 2026-09-25

### 2026-09-25
- **fix(build):** `Cargo.lock` gains `plugin-fastetcd`, which the #20
  commit added to the workspace without locking — the golden build runs
  `--locked` and refused (#23).
- **fix(etcd):** an unreachable store no longer reports `alarms = none` —
  nothing answered, so no alarm is known to be absent.
- **fix(verify):** `deploy/verify-etcd.sh` used f-strings that need
  python 3.12; dev has an older one, so the live check had never run.
  It now runs clean end to end (#20).

### 2026-09-24
- **feat(etcd):** a fastetcd plugin (#20). The datastore rustkube stands
  on was the one part of the control plane the console showed nothing
  about. From `/metrics`: revision, compact revision, DB size against
  quota and space used, what a defrag would free, disk free, snapshots,
  NOSPACE, leader and leader changes — an alarm or no leader is an error,
  80% of quota a warning. From etcd's v3 JSON gateway when it answers:
  members as rows with the leader marked, raft term and index, alarms on
  the member that raised them, and compact / defragment / disarm as
  confirmed actions. fastetcd does not serve that gateway yet, so the
  store's row says what it cannot show and names fastetcd#28, rather than
  drawing an empty member table. The store `serves` the apiserver, as a
  reference.
- **feat(etcd):** a keyspace browser at `#/etcd/keys` — the flat keyspace
  grouped into directories with counts, a value opened decoded (JSON, the
  Kubernetes protobuf envelope by its type, text, bytes), and a snapshot
  download that `etcdutl` reads back. `admin` only, checked in the route:
  the keyspace is every object beneath Kubernetes RBAC, Secrets included.
- **chore:** `deploy/verify-etcd.sh`, the live check — a real etcd for the
  gateway path and a real fastetcd for today's, run with
  `sc-build deploy/verify-etcd.sh`.

## [v0.9.0 – v0.12.0] — 2026-09-10 to 2026-09-22

### 2026-09-22
- **feat(events):** what happened to **this**, on the thing itself. The
  console had events in two places and neither answered the question
  people ask: a cluster-wide list is where you go when you do not know
  what is wrong, and "what happened to this" is where you go when you do.
  So a VM that would not start said `Scheduling` forever and the reason —
  no node with enough memory, a golden that would not clone, a bridge that
  does not exist on the node it was pinned to — was in an event and
  nowhere a person would find it. It is a **plugin contract**, not a view:
  a plugin is asked for one component id and answers `None` if it is not
  its own, so a new plugin with an event source needs no change anywhere
  else and one without needs no change at all. "Nothing happened" and
  "nothing records events for this" render differently, because they are
  different facts — stormblock, stormdrive and the registry write none,
  and a volume with no events is not a volume nothing has happened to.
- **feat(events):** the **bottom dock**, which vSphere and Proxmox both
  have and are right about: you press Create, and the question for the
  next ten seconds is "did that work". Answering it should not cost a
  navigation — by the time somebody has found the Events page the thing
  has happened or not and they have lost the thread. Two sources on
  purpose: what this console *did*, appended the instant an action
  returns, because nothing upstream knows a button was pressed and this is
  the only half that can say "your request was sent"; and what the cluster
  did about it, polled, which is the half carrying the reason when it did
  not work. Shut, the bar still shows the last line — a dock that hides
  everything when closed is a dock people leave open.
- **feat(k8s):** `ResourceSpec` carries `api_kind`, because an event is
  matched on `involvedObject` and that needs `PersistentVolumeClaim`, not
  the console's `pvc`. Stated rather than derived: deriving it from the
  title gives "Pod" for Pods and "Network policie" for the next one along.
  Kind is matched as well as name because a Service and a Deployment
  routinely share one, and an event about the wrong object is worse than
  none — it gets acted on. A container's events are its pod's, narrowed by
  `fieldPath`, so a crash-looping sidecar's `BackOff` reaches the
  container that is crashing and not its healthy neighbour.
- **feat(vm):** a machine's events come from two objects that share a name
  — the `VirtualMachine` the controller writes about and the
  `VirtualMachineInstance` the kubelet writes about — and somebody asking
  "did my start work" does not care which, so they are merged rather than
  left to the kubernetes plugin, which would answer for one of them.
- **fix(vm):** a stopped machine has no pending disks. Every disk was
  flagged "added — at next boot" when nothing was running, which is true
  in a useless sense and the same mistake as warning somebody editing a
  stopped machine about a restart it does not need.

### 2026-09-22
- **feat(nav):** a virtual machine is a workload, next to Pods. It had a
  section of its own holding one item, which said the thing a console
  should not: that running a machine is a different kind of activity from
  running a pod. On this platform it is the same activity — a VM *is* a
  kube object the kubelet reconciles — and somebody choosing what to run
  wants the choice in front of them, not in another part of the navigator.
  Workloads is built by two plugins now, so its items are numbered with
  gaps: `.item()` counts 0, 1, 2, and there is no integer between 0 and 1.
- **feat(vm):** add and remove a machine's disks. A disk is two things
  that have to agree — an entry in `domain.devices.disks` and one in
  `volumes` — and they are written and removed together, for the same
  reason `stormvm_node::plan::Sockets` exists over there. The arrays go
  whole, because a merge patch replaces an array rather than merging into
  it. **Not hotplug, and it does not pretend to be:** stormvm serves no
  device verb, so the disk is written to the definition and the guest sees
  it at its next boot — said in the answer, in the form, and before the
  button is pressed. Filed as stormvm#18. The root disk and the seed
  refuse to be removed: one leaves a machine that cannot start and the
  other leaves one whose next boot has no user and no key, and both look
  like a machine that broke rather than one somebody edited.
- **fix(vm):** a disk you add does not vanish from the page. The card read
  the running instance, so a disk added to the definition disappeared the
  moment it was added — written, correct, and reported as not there.
  Reading the definition instead would only move the lie, since a removed
  disk would vanish while the guest still had it. Both now, merged by
  name, each saying whether it is **attached**: written-and-not-yet-there,
  or removed-and-still-there-until-restart.
- **feat(vm):** the memory floor, which is the one decision that governs
  whether a machine's memory can ever change without a restart — and which
  the console could neither see nor set. KubeVirt's `memory.guest` with a
  *lower* resource request is exactly ballooning, and stormvm reads it that
  way: a floor below the size makes it build a `virtio-balloon-pci` or pass
  `--balloon`. It is a setting now and says which of the two situations a
  machine is in. A request equal to the size is not a floor — a balloon
  with nothing to deflate into is not adjustable memory, and reporting it
  as one would promise something that is not there. Moving the balloon once
  it exists still needs a verb stormvm does not have (stormvm#19).

### 2026-09-22
- **feat(vm):** the verbs the hypervisor serves, which nothing was calling
  (#13, #14, #18). stormvm has served `pause`, `unpause`, `softreboot`,
  `reset`, `freeze` and `thaw` beside the console doors since the doors
  landed, and reports per machine which of them it can take. The console
  called none. The probe already fetched `/api/v1/vms` to decide whether
  stormvm was answering and threw the body away — it is the only place
  the control verbs are reported, so it is kept now, and a machine is
  offered only what it can actually take. A button that 404s makes a
  client report that *the VM* refused, which sends whoever pressed it
  looking at the guest (stormvm#9). Ordered by what a guest survives: a
  soft reboot is a request the guest can honour and sits with Pause; a
  reset is the button on the front of the box and is shelved with the
  destructive ones. Freeze and thaw come as a pair, because a guest left
  frozen has every write blocked and from inside that looks like a hang.
- **feat(vm):** settings that say **when the change lands**, and a machine
  that admits it has diverged (#14). Changing a running VM has four
  different answers depending on the field, and a page that appears to
  apply everything and quietly applies some is worse than one that
  refuses. Every field carries when it takes effect, and the answer
  depends on the machine: on a stopped one everything simply applies, and
  warning somebody about a restart they do not need is how real warnings
  stop being read. The network binding and the SSH key are refused on
  purpose and say where to go instead — one moves the guest's address and
  this form cannot say what the new one will be, and the other is read
  once at first boot. And **pending changes**: a VirtualMachine is the
  definition, a VirtualMachineInstance is what is running, and nothing
  makes them agree — edit one and they diverge silently until somebody
  reboots and finds out. A diverged machine now says which fields, what
  was asked for beside what is running, on its page and as a metric on
  its row.
- **feat(vm):** a token when the node wants one (#13). stormvm admits
  loopback without one, but `--require-token` exists for a node that has
  decided its own loopback is not a boundary, and there the plain dial was
  refused with a 401 that reads as "the console is broken". Mint first,
  use it if minting worked, dial plainly if it did not.
- **feat(vm):** read-only is a capability, not an accident (#13, #15). A
  serial console is a root shell on most guests, so watching one and
  driving it are different permissions — and the second came free with the
  first. The relay drops what a read-only browser sends rather than
  refusing the door, because watching a guest boot is the whole point of
  it; keepalives still pass. And the replay is said out loud: stormvm
  sends the tail of the guest's console log on attach, so a console opened
  ten minutes into a boot prints the boot, and a reader who does not know
  that is reading history as though it were now.
- **feat(net):** what the network knows, where people already look (#17).
  Cilium knew which identity a workload had, whether its datapath was
  programmed and which policies selected it — as five lists under
  Networking, which is not where anybody is when they are asking why a pod
  cannot be reached. Now on the pod, the node and the machine: the
  identity **resolved** (12345 answers nothing, `app=web tier=frontend`
  does, and it is what policy is written against), the datapath state (a
  pod can be Running with an endpoint still regenerating, and during that
  window nothing reaches it), the policies that select it — evaluated
  against the labels Cilium itself decided the workload has, because those
  are the labels policy is applied to — and the addresses left in a node's
  pod CIDR, which is the number that predicts pods stuck Pending. A
  selector carrying `matchExpressions` reports as selecting nothing rather
  than being guessed at. All from CRDs the plugin already watches: the
  agent is a DaemonSet and they can disagree, and a CRD is the cluster's
  own record. Flows and per-module agent health stay gated on stormpump#11
  (tracked on #4).
- **feat(auth):** a reader may not write, enforced **once and by method**
  (#15). The roles landed and almost nothing consulted them: two routes
  checked, and delete-a-pod, apply-YAML, replace-an-object, raw delete,
  every VM verb, create, and everything behind the storage and registry
  proxies did not. Per-route is how this is usually done and how it goes
  wrong — one route added without the check is the whole hole, and the
  proxies are `any` and could never be enumerated. One check in the host,
  by method, scoped to `/api/plugins/`. The refusal names who is signed in
  and which role is missing, because "forbidden" on a console somebody is
  already logged into reads as a broken console.
- **fix(auth):** a console with no credentials configured is not a
  read-only console. Roles now decide things and an anonymous viewer holds
  none, so turning authentication *off* had made the console read-only —
  a silent break for every deployment that has never configured a user. A
  refusal has to come from having decided to enforce something, not from
  having decided nothing.
- **feat(table):** a reference shows every target rather than the first
  (the policies selecting a pod are several, and showing one is worse than
  showing none because nothing says there were others), and a placement
  column takes only single-target edges that resolve in the feed — a
  placement is singular by definition, and a plugin publishes an edge
  without being able to know whether the other side exists on this
  console.
- **fix:** three constructors that listed every field of a struct and
  stopped compiling when it grew one — `Viewer` twice and the VM create
  `Form` three times in a day, each break taking the whole workspace's
  tests with it. They spread the default now.

### 2026-09-22
- **feat(nav):** sections declare whether they are **work** or
  **administration**, and the administration ones start shut (#16). First
  login was a wall: ten sections, every one expanded, thirty-five items,
  most of it infrastructure nobody is looking at when they sit down to do
  something. Nothing is removed — the default state was the problem. The
  split is not "basic" against "advanced", which ages badly and is faintly
  insulting; it is the person creating a VM against the person deciding
  whether a drive is failing, and the second one knows where to look. So
  Home, Workloads, Virtualization and Storage stay open, and Compute,
  Networking, Observe, Images, Hardware and Administration start shut. The
  kind is declared by the plugin that contributes the section, so a new
  plugin classifies itself and the SPA still renders whatever it is given;
  `Work` is the default, so nothing is hidden that was not deliberately
  classified. A section contributed by two plugins is work if *either*
  calls it that — which is how Storage stays open for the PVCs somebody
  asked for while stormblock's engine internals sit in it.
- **feat(nav):** a shut section carries its total ("Storage 119"), so
  shutting it hides nothing you needed in order to decide whether to open
  it. The stored map holds only choices somebody actually made, so a
  section that changes kind on the server changes for everybody who never
  touched it, and an explicit choice always wins.

### 2026-09-22
- **fix(console):** a relation is containment or context, never a
  destination — in every plugin (#18). A VM pushed its node as `has_one`
  and nothing else, and both renderers read more into that than it says:
  the table as something the row contained, the card as where following
  the row went. Opening a virtual machine landed you in *node details*.
  203d5b8 patched the symptom for VMs by publishing the node as a metric
  as well. The shape was in every plugin, so the rule moved instead:
  `has_many` nests, and everything else is a reference — a link in the
  opened row and never where the line leads. The sweep, all of it the
  same direction error: a VM's node, a local image's node and volume, a
  golden's catalogue entry, a clone's volume, a volume's **parent** (a
  table that expanded upwards through its own ancestry), a Cilium
  endpoint's pod and identity, a CiliumNode's node.
- **feat(console):** placement is a column, read off the `belongs_to`
  edge. There were two such columns, namespace and node, written out by
  hand in the table; every other placement in the feed was invisible. Any
  placement whose values differ down the list earns a column and a sort
  now — a VM's node and definition, a local image's node, a drive's
  shelf, a volume's array — and so do the `FeedPlugin` upstreams whose
  components this repo does not write. One that is the same on every row
  (the `engine` on every stormblock volume) earns nothing, because a
  column of one repeated value is a column of noise.
- **feat(console):** every row opens, and the opened row is the object:
  the detail unabbreviated, every metric as a labelled fact rather than
  crushed into one line, the references as links, and **every** action —
  including the destructive ones the row keeps behind its menu. Clicking
  a line goes to that component's own page where it has one and opens it
  in place where it has none, which is the only detail a pod, a container
  or a volume has until #12 lands. The nested table also spans the whole
  width at last; the colspan was a literal `7` that knew about Kind and
  nothing else, so every placement column left the nested content short of
  the right edge.
- **fix(vm):** one Stop per machine. 203d5b8 added a Stop to the row
  without removing the one already there, so an instance published two —
  the same path, different `danger` — and the row showed one inline and
  one in the menu that asked for confirmation first.
- **feat(vm):** Restart, which did not exist. `POST
  …/machines/{ns}/{name}/restart` deletes the instance and lets the
  definition put it back, because KubeVirt has no verb that reboots a VMI
  in place. Without a definition that is a delete wearing a reassuring
  name: refused with `409` and the sentence saying why, and offered
  disabled on the row rather than not at all, since "why can I not
  restart this" is a question the row should answer. Stopping an instance
  nothing will restart is published `danger` for the same reason.
- **fix(vm):** the detail line stops repeating its own columns — the
  node, the vCPU and the memory each have one now.
- **fix(vm):** the test form builds again, and the seed assertion matches
  the hostname 203d5b8 made unconditional. `network` and `hostname` were
  added to `Form` without being carried into the test constructor, so the
  workspace had not compiled its tests since.

### 2026-09-20
- **feat(vmimages):** a plugin for cloud images: the catalogue a cluster could
  golden from, the goldens the fleet has, and which nodes carry a local copy —
  all of it from `vmcloud-image-operator`, which is a door onto the cluster's
  own `CloudImage` and `CloudImagePlacement` objects, so a golden made here is
  the object `kubectl` shows. A catalogue row carries a **Make golden**
  action, because the operator's `POST /api/v1/catalog/{reference}` takes no
  body precisely so a stormview action — a method and a path, and nothing
  else — can be wired to it. The create forms offer the live catalogue, the
  built goldens and the known nodes rather than asking anybody to type a
  reference. A local copy points at the stormblock volume it became
  (`sb:volume:…`) and the node it is on (`k8s:node:…`) rather than describing
  either a second time. Config: `[vmimages] enabled`, `url` (default
  `http://127.0.0.1:9099`). Nav items land under **Images**, beside
  sbregistry's, because both answer "where does an image come from".
- **fix(vm):** the node is optional now that something schedules VMs. The form
  required one — "nothing places VMs yet, so a node has to be named" — and the
  YAML template shipped `nodeName: CHANGE-ME`. Both were true, and both were
  the workaround somebody had to perform to get a VM to run at all;
  rustkube#72 gave the scheduler VirtualMachineInstances. Blank now means "the
  scheduler picks"; a named node still pins the machine there. The key is
  **absent** rather than empty when none was asked for, because `spec.nodeName`
  is a pin and the scheduler leaves a VMI carrying one alone — an empty string
  would pin the machine to a node called `""`, which is the same shape of bug
  as `CHANGE-ME` and harder to see.
- **feat(k8s):** a namespace shows its annotations, which is where the
  descriptive fields actually live. Labels never carried who asked for a
  namespace or what it is for — `openshift.io/requester`,
  `openshift.io/description` and `openshift.io/display-name` are annotations,
  and so is anything an operator adds to explain a namespace to the next
  person. They were fetched with the object and thrown away. Those three get
  a line of their own so they read as prose; the rest stay a list; and
  `kubectl.kubernetes.io/last-applied-configuration` is dropped in the plugin
  rather than in the view, because it is the whole object as a JSON string
  and every consumer would otherwise have to know to drop it.
- **fix(k8s):** a pod contains containers, not a node. Three faults wearing
  one symptom — open a pod, get a node, which gets you back to the pods, and
  the thing a pod actually contains was nowhere.
  `Relation::has_one("node", …)` was the wrong direction: the table reads the
  direction to decide what nests inside what, so a pod expanded into its node
  and the node's `has_many pods` expanded straight back. It is `belongs_to`
  now — a pod does not own its node, it is placed on one.
  Containers were not in the feed at all; `containerStatuses` was read once,
  to sum restarts. Every container is now a component of its own
  (`k8s:container:<ns>/<pod>/<name>`) carrying the image in full, the
  `imageID` actually running, restarts, ports and the state in the words
  `kubectl describe` uses — `waiting · CrashLoopBackOff` is the whole
  diagnosis in most cases and should not need the YAML tab. Read from `spec`,
  not `status`, so a pod that has not started still lists them; init
  containers first, because that is the order they run in; spec and status
  matched by name, because the kubelet does not promise the arrays agree and
  pairing a container with somebody else's state is worse than showing none.
- **fix(k8s):** a pod says which namespace it is in. It is in the detail line,
  and the table grew a Namespace column — read from the `belongs_to namespace`
  edge every namespaced component already publishes rather than parsed out of
  an id, and shown only when some row has one, so cluster-scoped lists get no
  empty column. Generic, so deployments, services and the rest gained it at
  the same time.
- **fix(registry):** an image is more than a name and the word "digest".
  Images went through `generic()`, which looks for whichever of
  `state/status/digest/size_human/created/role` it finds and takes three; a
  `PushRec` has none of those but `digest`, so an image rendered as its ref,
  the text "digest sha256:…", no metrics and no edges. Everything now shown
  was already in the record: the **command** (`Entrypoint` + `Cmd`
  concatenated — `Cmd` alone is the default *arguments* when an Entrypoint is
  set, and showing it by itself reads as the command), the user (blank means
  root, worth seeing without opening anything), workdir, env count, and the
  twelve hex that name its golden. The golden built from it is now an edge:
  `img-<first 12 hex>` is the naming rule the golden/clone model coordinates
  on, and a malformed digest yields no edge rather than pointing at a template
  that could never exist.

### 2026-09-09
- **fix:** the VM console doors work, now that stormvm serves them
  (stormvm#5). Three bugs found only by running both ends together: the
  stormvm probe sat behind the apiserver guard, so a node with no rustkube
  reported its consoles shut while stormvm answered on the same machine;
  doors were reported per stormvm rather than per VM, offering a
  framebuffer to a machine whose spec never asked for one; and a refusal
  reached the viewer as "HTTP error: 409 Conflict", because a websocket
  upgrade carries only a status. Verified end to end on dev — the relay is
  byte-identical to dialling stormvm directly, and the browser terminal
  takes keystrokes to the guest
- **feat:** a node can be opened (Phase 4). `#/node/<host>` probes that
  node's known ports concurrently and shows what answered — the daemon's
  own `system` card over the port layout's guess, with silence explained
  rather than reported as a fault. Drill-in is on demand: twenty nodes'
  components in the pushed feed would be thousands of rows nobody is
  looking at
- **feat:** `#/nodes` lists the fleet. "Nodes" in the navigator pointed at
  the plugin card — a page showing one row, with a badge that said 1
  however many nodes were on the segment
- **fix:** the log collector was throwing away the address. It had the
  datagram's source and used it only as a fallback *name* for lines that
  failed to parse, so a node that identified itself properly left nothing
  to dial — which is the one thing CLUSTER.md says the console needs
  ("everything else it can ask the node's own API for once it has an
  address"). `LogEvent.addr` is always the sender; the host summary keeps
  the last seen, so a node that moves corrects itself
- **fix:** the local node had no page — the only node without one, because
  its fallback component carried no link and the detail route only knew
  hosts the group had heard
- **fix:** a feed with no `system` card was reported Unknown, which says
  "did not answer" about something that answered fully
- **chore:** stormcos#38 filed — fleet lifecycle (join, promote, demote,
  drain) is CLI-only, so the console can show a node and not act on it

## [v0.8.0] — 2026-09-09

### 2026-09-09
- **feat:** a **VM plugin** (#9, #2). stormvm's `docs/kube.md` settles where
  a VM lives — *"stormvm is libraries, the kubelet is the loop"* — so a VM
  is a KubeVirt `VirtualMachine`/`VirtualMachineInstance` in the apiserver
  and the plugin watches `kubevirt.io/v1` the way the Cilium view watches
  `cilium.io/v2`, with no second source of truth. Both objects are shown,
  because they answer different questions; vCPU is read however the spec
  spelled it; lifecycle is `spec.running` and nothing else; a definition
  that wants to run with no instance says so rather than being drawn as
  broken. A VM page carries disks with what backs each, network, the
  reason it did not start, YAML, and both console doors
- **feat:** the **console doors** (#2) — serial and framebuffer, relayed
  through the console's own origin as websockets and addressed by VM
  rather than node, so the browser never learns a node address and the URL
  survives a live migration. The serial door is a terminal that sends
  keystrokes back; the framebuffer is noVNC (MPL-2.0), lazily loaded in
  its own chunk. Neither upstream serves them yet — stormvm's console
  service is unbuilt, and the pod-log route needs rustkube#55 and
  rustkube-node#34 — so the page names the missing upstream instead of
  showing a terminal that will never print
- **feat:** **namespace as a dimension** (#5). The selection travels in the
  URL as `?ns=`, so a pasted link shows what the sender was looking at;
  what is namespaced comes from a server-side kind catalogue
  (`GET /api/plugins/k8s/kinds`, from `cache::RESOURCES`) instead of three
  hardcoded lists in the SPA; a cluster-scoped list says it is
  cluster-scoped rather than leaving a reader to wonder why the selector
  changed nothing
- **feat:** a **namespace page** (#6) at `#/k8s/ns/<name>` — inventory,
  quota, limit ranges, labels, resources, events, YAML — whose every count
  is a link into that kind filtered to this namespace. "No quota" is shown
  as the answer it is: nothing here is bounded
- **feat:** **per-viewer authorization** (#7). `[[api.users]] kube_token`
  gives a user a kubernetes identity; the console asks rustkube *as them*
  which namespaces they may see, and filters the feed before it leaves the
  process — so a hidden object is unreachable by REST, by websocket and by
  following a relation, and a plugin route answers 404 for it. Every write
  carries the viewer's own bearer, so the apiserver's RBAC decides. With no
  identity nothing is enforced and `/api/v1/console/access` says so
- **feat:** **drives are hardware, not storage** (#8). A Hardware nav
  section; `#/drives` grouped by shelf and ordered by bay with stormdrive's
  real actions on the rows and the shelf's on the group;
  `#/drives?group=shelf` for the other question, which enclosure is in
  trouble. Enrolling a discovered disk into the fleet was reachable from
  nowhere and is now a button
- **feat:** **YAML that can be saved** (#4). `PUT
  /api/plugins/k8s/object/{kind}/{key}` replaces the object, so the
  `resourceVersion` it was loaded with is the concurrency guard — a 409
  rather than a silent overwrite. A rename is refused rather than
  performed
- **feat:** the **Cilium agent's own verdict** (#4) — `127.0.0.1:9879/healthz`
  on the node, taken as the worse of it and the CRD view, because they
  disagree exactly when it matters
- **feat:** row actions behind a menu. A drive carries nine operations;
  nine buttons per row is a wall, and it put "Destructive test" one
  mis-click from "Locate"
- **fix:** no card renders a loopback address as if a browser could use it
  (#10). `console_core::upstream` separates the address the console dials
  from the one a viewer could use; a card says "on this node :9092"
- **fix:** an unserved CRD leaves the snapshot rather than emptying, so
  "absent" and "present and empty" are tellable apart — a machine that has
  never run Cilium was getting a red Cilium card
- **fix:** a partial route match no longer leaves its parameters behind for
  whichever route eventually wins
- **docs:** README §The namespace is a dimension, §Who sees what,
  §Virtual machines, §Hardware and storage; architecture §Who sees what and
  §vm; `[api.users] kube_token` and `[vm]` in the example config
- **chore:** cross-project issues filed — rustkube#59 (no
  SelfSubjectAccessReview, so scoping costs a probe per namespace) and
  stormdrive#3 (bay and controller only in the rendered detail string).
  stormconsole#1 is closed by stormdrive v0.4.0 / stormstorage v0.2.0,
  which the console already consumes as feed plugins

## [v0.7.1] — 2026-09-02

### 2026-09-02
- **fix:** dedup normalises a leading timestamp out of the key. Emitters
  forward a process's own log line verbatim and tracing writes its
  timestamp at the front of it, so the message text carries a microsecond
  clock that changes on every occurrence — keyed on raw text, the flood
  dedup was written for stayed one entry per arrival and nothing
  collapsed. Verified against the live fleet: 646 arrivals of the same
  line are now one entry showing `×646`. Only the key is normalised; the
  stored text is whatever last arrived, and a message that is nothing but
  a timestamp keeps it
- **fix:** a stale ring is named rather than reported as "invalid data" —
  a config pointing `[logs] db_path` at the SQLite file from 0.6 now says
  what the file is and that it can be deleted

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

### 2026-09-20
- **feat:** the fleet view reads the node capability beacon (stormcos#26).
  Node cards now carry cores, memory, drives, workloads up/down and pallet
  count, read off the `[storm-beacon@0 …]` element every node puts on the log
  group every 30 s. No new socket, no polling: the collector already sees
  every datagram. Beacons are kept beside the log ring rather than in it,
  because a beacon is state and the ring is a bounded, age-pruned log —
  one that aged out would take a node's capabilities off the fleet view
  while the node was still announcing them.
- Every beacon parameter is retained rather than parsed into a fixed struct,
  so a field stormcos adds reaches the node card without a release here.
  Absent fields render as absent: the emitter omits what it cannot read so a
  reader can tell "no role" from "role unknown".
- **fix:** a select option now submits a value rather than its label. A VM was
  created whose root disk was named "alma 10 x86_64 — not goldened yet, will
  be built": the option carried one string, so the form posted the sentence a
  person read and the server mapped it back afterwards — a mapping that missed
  as soon as the catalogue changed between rendering and submitting.
- **fix:** the cloud-init payload goes in a Secret (`userDataSecretRef`)
  instead of inline in the VMI spec. A public key is not confidential, but
  `userData` is the field that grows passwords and it travels in a spec that
  anyone with `get` on virtualmachineinstances can read.
- **fix:** the VM create form submitted an image *name* where the engine wants
  a *volume*, so a machine created from the dropdown failed at start with
  `cloning golden fedora-43 for disk root: 404 Not Found: {"error":"no volume
  fedora-43"}`. The operator answers three names for one image — `fedora-43`
  (the object), `fedora-43-x86_64` (`status.localName`, what a person calls
  it) and `media-846574c8a97c` (`status.golden`, the volume the engine
  actually holds) — and only the last can be cloned. The dropdown now shows
  the readable name and submits the volume. Goldening a catalogue reference
  waits for the digest to resolve (seconds: it comes from the published
  checksum file, not the download) rather than returning a name that will
  never be a volume.
- **fix:** the root-disk dropdown emptied itself whenever the image operator
  was briefly silent, and degraded to a free-text box asking for a golden name
  from memory — the exact thing the dropdown exists to remove. The console and
  the operator start together on a node, so that was the first minute after
  every boot. An empty refresh now keeps the last good list and says it may be
  stale, and until the first good answer arrives the poll retries every 3s
  instead of every 60.
- **fix(vm):** a null field no longer empties the image list. `status.golden`
  is null while an image is still building, and `#[serde(default)]` covers an
  *absent* field, not a present-and-null one — so one downloading image made
  the whole `ImageList` fail to deserialise and every fleet golden vanished
  from the create form at once. Reported as "rawhide is not showing", where
  rawhide was Available, had a golden, and was not the image at fault. Every
  string field in the image and catalogue models now tolerates null; one row
  that cannot be read costs that row, never the list.
- **fix(vm):** creating a VM from an image that is still downloading no longer
  fails. Any non-empty `status.message` was treated as a failure, and the
  operator writes progress there (`importing into http://…`), so the form
  returned a URL as an error and created nothing. Only a phase that means
  failure fails now. And because `status.golden` does not exist until the image
  is `Available` — after the download, decode and seal — the machine is created
  against `status.localName`, the name its local copy will carry, and the
  kubelet waits for it exactly as a pod waits for an image that is still
  pulling.
- **fix(vm):** the create form makes a `VirtualMachine`, not a bare
  `VirtualMachineInstance`. A VMI applied on its own has no durable definition
  behind it, and all three of these were that one missing object: every setting
  in the drawer was read-only ("nothing durable to write to"), delete asked for
  a `virtualmachines/<name>` that had never existed and returned 404, and stop
  would have destroyed the machine instead of stopping it.
- **feat(vm):** the display is editable — adapter and framebuffer memory, with
  the graphics device turned on or off to match. There was no field at all, on
  a console whose main use for a VM that will not boot is to look at it.
- **feat:** the masthead reads `StormCOS <release>` instead of the console's
  own name, and `/api/version` answers the same thing without a session. The
  release comes from the nodes (`nodeInfo.osImage`, which the kubelet fills
  from the manifest the image carries), so it is what *booted* rather than
  what was published. Nodes that disagree are all named — that is what a
  half-finished rollout looks like.
- **fix(ui):** Console and Start take the line, ahead of restart and stop. The
  inline slots went to the first two actions declared, which buried a VM's
  Console behind the kebab — the one action people open the list for, two
  clicks away, while rarely-used ones sat in the open.
