// What the Boot and flow-over page says (#271), kept apart from the page so plain node can test it.
// A boot from PID 1's timeline (stormpump `history.rs`, CLOCK_BOOTTIME ms, a phase's `t_ms` being when it
// began); a flow-over from the engine's open health (stormblock `flowprogress.rs`).

const MAX_PHASES = 9

/// What came before PID 1, then its phases in order, and when everything was running.
export function bootStory(detail) {
  const tl = detail?.timeline || []
  const header = tl.find((e) => e.event === 'boot')
  const rest = tl.filter((e) => e.event !== 'boot' && e.t_ms != null)
  const pid1 = rest.find((e) => e.event === 'pid1')
  const first = pid1?.t_ms ?? (rest.length ? Math.min(...rest.map((e) => e.t_ms)) : null)
  const all = rest.find((e) => e.event === 'all-running')
  const phases = rest
    .filter((e) => e.event === 'phase' && e.ms != null)
    .map((e) => ({ name: e.name || 'phase', at: e.t_ms, ms: e.ms }))
    .sort((a, b) => a.at - b.at)
  const before = first ? [{ name: 'kernel and initramfs', at: 0, ms: first, before: true }] : []
  const ends = [...phases.map((p) => p.at + p.ms), all?.t_ms ?? 0, first ?? 0]
  const span = Math.max(1, ...ends)
  const rows = [...before, ...phases]
  return {
    release: detail?.release || null,
    hostname: detail?.hostname || null,
    kernel: detail?.kernel || null,
    current: !!detail?.current,
    end: detail?.end || null,
    rows: rows.slice(0, MAX_PHASES),
    more: Math.max(0, rows.length - MAX_PHASES),
    span,
    allRunningMs: all?.t_ms ?? null,
    allRunningDetail: all?.detail || null,
    started: detail?.started ?? 0,
    exited: detail?.exited ?? 0,
    refused: detail?.refused ?? 0,
    wall: detail?.at ? Date.parse(detail.at) : null,
    headerMs: header?.t_ms ?? 0,
  }
}

/// Time since the kernel started, for a boot that is still this one. `null` for any other.
export function uptimeMs(story, now = Date.now()) {
  if (!story?.current || story.wall == null) return null
  return Math.max(0, now - story.wall + story.headerMs)
}

const remote = (half) => half === 'remote' || half === 'mixed'

/// Which of the page's states the engine's health is in. Absent `flow_over_remaining` means this engine
/// runs no flow-over, so the slabs say where it runs from; health says "unknown" of those until it can read them.
export function flowStory(health) {
  if (!health) return { state: 'unknown', word: 'The engine has not said yet' }
  const slabs = health.slabs || null
  // Not `diskless` alone: an engine with no volume placed yet says that, with both halves `none`.
  const fromNetwork = !!slabs && (remote(slabs.system) || remote(slabs.data))
  const fromDisk = !!slabs && !fromNetwork && (slabs.system === 'local' || slabs.data === 'local')
  const left = health.flow_over_remaining
  const f = health.flow_over || null
  const moved = f?.moved ?? 0
  const zeroed = f?.zeroed ?? 0
  const base = {
    moved,
    zeroed,
    left: left ?? null,
    bytes: f?.bytes ?? 0,
    average: f?.mb_per_s ?? null,
    seconds: f?.seconds ?? null,
    eta: f?.eta_seconds ?? null,
    validation: f?.validation || null,
    percent: left == null ? null : moved + zeroed + left > 0 ? ((moved + zeroed) / (moved + zeroed + left)) * 100 : 100,
  }
  if (left == null) {
    return fromNetwork
      ? { ...base, state: 'network', word: 'Running from the network', sub: 'This engine runs no flow-over.' }
      : fromDisk
        ? { ...base, state: 'local', word: 'Running from its own disk', sub: 'No flow-over on this boot.' }
        : { ...base, state: 'none', word: 'No flow-over on this boot', sub: slabs ? 'The engine has no volume on a slab yet.' : 'The engine has not said where its slabs are yet.' }
  }
  if (health.flow_over_stalled) return { ...base, state: 'stalled', word: 'The flow-over stopped', sub: health.flow_over_stalled }
  if (left > 0) return { ...base, state: 'moving', word: 'Moving onto its own disk', sub: 'Running from the network while it moves.' }
  const v = base.validation
  if (v?.state === 'corrupt') {
    return { ...base, state: 'corrupt', word: 'A drive did not read back what it was given', sub: `${v.mismatched} extent(s) differ, ${v.unrepaired} not repaired. Those still read from the appliance, and the node does not go local.` }
  }
  if (v?.state === 'running') return { ...base, state: 'checking', word: 'Checking what moved', sub: `${v.validated} of ${v.total} extents read back and compared.` }
  // Slabs are kept for ten seconds, so they can still say the network just after the last extent.
  if (fromNetwork) return { ...base, state: 'settling', word: 'Moved, and settling', sub: 'The last extent has moved; the engine still reports a slab on the network.' }
  return { ...base, state: 'local', word: 'Running from its own disk', sub: f ? 'Its data has moved.' : 'Flow-over finished.' }
}

/// What to say of a finished flow-over. The engine's `seconds` keeps counting after the last extent
/// (stormblock#543), so its average falls for as long as it runs: time and speed are said only from the last
/// reading this page took while extents were left. `watched` is that reading, or null after a reload.
export function flowTotal(flow, watched) {
  if (flow.state !== 'local' || !flow.bytes) return null
  return { bytes: flow.bytes, seconds: watched?.seconds ?? null, average: watched?.average ?? null }
}

/// MB/s between readings of the copied-bytes counter. The engine's own `mb_per_s` is the average since
/// the flow-over began, which would draw a nearly flat line. A counter that fell means a new engine process.
export function rates(samples) {
  const out = []
  for (let i = 1; i < samples.length; i++) {
    const secs = (samples[i].t - samples[i - 1].t) / 1000
    const bytes = samples[i].bytes - samples[i - 1].bytes
    if (secs > 0 && bytes >= 0) out.push(bytes / 1e6 / secs)
  }
  return out
}

/// An SVG polyline's points for those rates, across `w` by `h`. Empty until there are two.
export function sparkline(values, w = 600, h = 80) {
  if (values.length < 2) return ''
  const top = Math.max(...values, 1)
  return values.map((v, i) => `${((i / (values.length - 1)) * w).toFixed(1)},${(h - (v / top) * (h - 4) - 2).toFixed(1)}`).join(' ')
}

export function clock(ms) {
  if (ms == null) return '—'
  const s = Math.floor(ms / 1000)
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  const pad = (n) => String(n).padStart(2, '0')
  return h ? `${h}:${pad(m)}:${pad(s % 60)}` : `${m}:${pad(s % 60)}`
}

export function seconds(ms) {
  if (ms == null) return '—'
  return ms < 10000 ? `${(ms / 1000).toFixed(1)} s` : `${Math.round(ms / 1000)} s`
}
