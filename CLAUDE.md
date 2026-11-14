# CLAUDE.md — stormconsole

The StormCOS web console — an operator console built in Rust
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
  on a fresh build VM as `stormbuild`, then destroys it); never on this VM,
  never as root. `web/dist/` is committed: rebuild it with
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

## Work plan and history

Moved to [docs/work-plan.md](docs/work-plan.md) (CLAUDE.md is loaded into every session; its size made the session's context thrash). Nothing was dropped. Not loaded by default: `grep -n '^##' docs/work-plan.md`, then read only the section you need. Write new intentions and progress there, not here.

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
