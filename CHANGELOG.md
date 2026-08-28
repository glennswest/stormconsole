# Changelog

## [Unreleased]
<!-- New unreleased changes go here -->

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
