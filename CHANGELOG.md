# Changelog

## [Unreleased]

### 2026-09-25
- **fix(build):** `Cargo.lock` gains `plugin-fastetcd`, which the #20
  commit added to the workspace without locking — the golden build runs
  `--locked` and refused (#23).

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
