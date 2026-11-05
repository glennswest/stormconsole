// What the History page makes of the node's records (#121), kept apart from
// the page so plain node can test it.

const LEVELS = ['emerg', 'alert', 'crit', 'err', 'warning', 'notice', 'info', 'debug']

/// A line of a kernel log as PID 1 keeps it: `seq [sec.usec] <pri> text`, or a
/// `-- N kernel record(s) lost … --` gap line, which is always worth showing.
export function kernelLine(line) {
  const m = /^(\d+) \[\s*(\d+\.\d+)\] <(\d)> (.*)$/.exec(line)
  if (m) return { seq: +m[1], time: m[2], pri: +m[3], level: LEVELS[+m[3]], text: m[4] }
  if (line.startsWith('-- ')) return { gap: true, pri: 0, level: 'gap', text: line }
  return { pri: 6, level: 'info', text: line }
}

/// The lines at or above a level, and matching a search. A gap always shows:
/// it says the log is not whole.
export function kernelLines(text, maxPri = 7, search = '') {
  const needle = search.toLowerCase()
  const out = []
  for (const l of (text || '').split('\n')) {
    if (!l) continue
    const k = kernelLine(l)
    if (k.gap || (k.pri <= maxPri && (!needle || k.text.toLowerCase().includes(needle)))) out.push(k)
  }
  return out
}

/// A timeline's rows: the `boot` header is the page's heading, not a row.
export function timelineRows(events) {
  return (events || []).filter((e) => e.event !== 'boot').map((e) => ({
    ...e,
    tone: e.event === 'refused' || e.event === 'crashed' || e.event === 'crash' ? 'bad' : e.event === 'exited' || e.event === 'dropped' ? 'warn' : '',
  }))
}

/// The slowest phases of a boot, for the question a boot's history is kept
/// for: where did the time go.
export function slowestPhases(events, n = 5) {
  return (events || [])
    .filter((e) => e.event === 'phase' && e.ms != null)
    .sort((a, b) => b.ms - a.ms)
    .slice(0, n)
}

/// Each counter over a drive's records (given newest first): first and latest
/// value, how it moved. A counter that moved comes first; one that went
/// backwards before one that grew.
export function counterTrends(records) {
  const seen = new Map()
  for (const r of [...(records || [])].reverse()) {
    for (const [name, v] of Object.entries(r.counters || {})) {
      const c = seen.get(name)
      if (c) {
        if (v < c.latest) c.back = true
        c.latest = v
        c.points++
      } else seen.set(name, { name, first: v, latest: v, points: 1, back: false })
    }
  }
  const rank = (c) => (c.back ? 0 : c.latest !== c.first ? 1 : 2)
  return [...seen.values()]
    .map((c) => ({ ...c, delta: c.latest - c.first }))
    .sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name))
}

/// Every finding in a drive's records, newest first, in stormdrive's words.
export function findings(records) {
  const out = []
  for (const r of records || []) {
    for (const f of r.findings || []) {
      const when = f.other_boot ? `since ${f.since} (an earlier boot)` : `since ${f.since}`
      out.push({
        at: r.at,
        bad: f.change !== 'grew',
        message:
          f.change === 'grew'
            ? `${f.counter} grew ${f.from} → ${f.to} ${when}`
            : `${f.counter} went backwards ${f.from} → ${f.to} ${when}: the drive's counters were reset, or it is not the drive it was`,
      })
    }
  }
  return out
}

/// One hardware change, in stormdrive's words.
export function assetChange(c) {
  return c.change === 'changed' ? `${c.item} ${(c.fields || []).join(', ')}` : `${c.item} ${c.change}`
}

/// A hardware record's items by what they are: `dimm/A1` under `dimm`.
export function assetGroups(items) {
  const groups = new Map()
  for (const [key, value] of Object.entries(items || {})) {
    const [group, ...rest] = key.split('/')
    if (!groups.has(group)) groups.set(group, [])
    groups.get(group).push({ key, name: rest.join('/') || group, fields: fields(value) })
  }
  return [...groups.entries()].map(([group, rows]) => ({ group, rows }))
}

function fields(v) {
  if (v === null || typeof v !== 'object') return [['', String(v)]]
  return Object.entries(v)
    .filter(([, x]) => x !== null && x !== undefined)
    .map(([k, x]) => [k, typeof x === 'object' ? JSON.stringify(x) : String(x)])
}

export function bytes(n) {
  if (n == null) return '—'
  const u = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB']
  let i = 0
  let v = n
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024
    i++
  }
  return `${i ? v.toFixed(1) : v} ${u[i]}`
}

export function seconds(ms) {
  if (ms == null) return '—'
  return ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(1)} s`
}

/// What to say about how a boot ended, and its tone.
export function bootEnd(b) {
  return (
    {
      running: { label: 'running', health: 'ok' },
      'shut down': { label: 'shut down', health: 'idle' },
      crashed: { label: 'crashed', health: 'error' },
      'no clean end': { label: 'no clean end', health: 'warn' },
      'no timeline': { label: 'no timeline', health: 'idle' },
    }[b.end] || { label: b.end, health: 'unknown' }
  )
}
