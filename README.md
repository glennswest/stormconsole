# stormconsole

**The StormCOS console** — patterned on the OpenShift console, built the
storm way: a single static Rust binary under [stormd](https://github.com/glennswest/stormd),
rendering everything through the [stormview](https://github.com/glennswest/stormview)
contract, with a pluggable architecture where every domain contributes its
own part.

- **kubernetes** — namespaces, workloads, nodes, events via
  [rustkube](https://github.com/glennswest/rustkube) (kube-wire-compatible
  apiserver, watch-backed cache)
- **logs** — the fleet log collector: stormcast multicast
  (`239.255.42.1:5514`) → SQLite ring → query API + live follow
- **fleet** — nodes discovered by their own announcements; drill into each
  node's stormd/stormdrive/stormblock; join, promote, demote, drain
- **stormdrive** — physical drives fleet-wide: SMART, wear, thermal,
  locate, lifecycle
- **stormblock** — volumes, exports, slabs, arrays from the block engine
- **sbregistry** — goldens, clones, pallets, warm-up

Not part of this project: mkube. The orchestrator side is rustkube and
rustkube-node only.

The console both consumes and produces the stormview component feed: it
aggregates every plugin's components at `/api/v1/components` (+
`/ws/components`), so any stormview renderer can show the whole cluster.

## Status

v0.1.0 — bootstrap. Design complete (see
[docs/architecture.md](docs/architecture.md)); Phase 1 (skeleton) in
progress. Work plan in [CLAUDE.md](CLAUDE.md).

## Build

Build on `root@dev.g8.lo`, never on a Mac:

```bash
cargo build --release --target x86_64-unknown-linux-musl
cd web && npm install && npm run build   # SPA, embedded at cargo build
```

## Run

```bash
stormconsole --config /etc/stormconsole/config.toml
# UI + API on :9094
```
