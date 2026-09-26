// The Drives page's model (#32), kept out of the component so it can be run
// and measured at rack scale — 160 drives a node, 1,600 a rack — without a
// browser (drivemap.test.mjs).
//
// Everything here is read from the component feed:
//   - stormdrive's drives and shelves, from every node (`drive:` for this
//     node, `drive@<host>:` for the others), each with a `node` metric;
//   - this node's engine, per drive: `sb:use:<serial>` (slab bytes, free)
//     and `sb:member:<dev>` (a drive-level array member's state) — the only
//     usage and rebuild state there is until stormdrive reports usage itself
//     (stormdrive#12), so other nodes' drives say they have none;
//   - each kube node's `rack` metric (label `topology.storm.io/rack`).

const UNITS = { B: 1, KB: 1024, MB: 1024 ** 2, GB: 1024 ** 3, TB: 1024 ** 4, PB: 1024 ** 5, EB: 1024 ** 6 }

export function metric(c, label) {
  const m = (c?.metrics || []).find((x) => x.label === label)
  return m ? m.value : undefined
}

/// "7.3 TB" → bytes, the way stormdrive prints it (1024-based).
export function parseBytes(s) {
  const m = /^\s*([\d.]+)\s*([KMGTPE]?B)\s*$/i.exec(s || '')
  if (!m) return 0
  return Math.round(Number(m[1]) * (UNITS[m[2].toUpperCase()] || 1))
}

/// Bytes → the largest unit that keeps it readable, up to EB: a rack of
/// 1,600 drives is petabytes, and a room of them is not far off an exabyte.
export function formatBytes(n) {
  if (!n) return '0 B'
  const order = ['B', 'KB', 'MB', 'GB', 'TB', 'PB', 'EB']
  let i = 0
  let v = n
  while (v >= 1024 && i < order.length - 1) {
    v /= 1024
    i++
  }
  return i === 0 ? `${n} B` : `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} ${order[i]}`
}

function bayOf(c) {
  const b = metric(c, 'bay')
  if (b !== undefined && b !== '') return Number(b)
  const m = /bay (\d+)/.exec(c.detail || '')
  return m ? Number(m[1]) : null
}

/// One record per drive, joined to what else is known about it.
export function build(components) {
  const byId = new Map()
  const use = new Map()
  const member = new Map()
  const rack = new Map()
  const drives = []
  const shelves = []
  for (const c of components) {
    byId.set(c.id, c)
    if (c.kind === 'drive') drives.push(c)
    else if (c.kind === 'shelf') shelves.push(c)
    else if (c.kind === 'drive-use') use.set(metric(c, 'serial'), c)
    else if (c.kind === 'array-member') member.set(metric(c, 'path'), c)
    else if (c.kind === 'k8s-node' && metric(c, 'rack')) rack.set(c.label, metric(c, 'rack'))
  }
  const records = drives.map((c) => {
    const node = metric(c, 'node') || 'this node'
    const local = c.id.startsWith('drive:')
    const shelfId = (c.relations || []).find((r) => r.name === 'shelf')?.targets?.[0] || null
    const shelf = shelfId ? byId.get(shelfId) : null
    const capacity = parseBytes(metric(c, 'capacity'))
    const u = local ? use.get(metric(c, 'serial')) : null
    let usage = null
    if (u && capacity) {
      const slab = Number(metric(u, 'slab bytes') || 0)
      const free = Number(metric(u, 'free bytes') || 0)
      usage = { slab, free, used: slab - free, frac: Math.min(1, (slab - free) / capacity), unslabbed: Math.max(0, capacity - slab) }
    }
    const m = local ? member.get(metric(c, 'dev')) : null
    const detail = c.detail || ''
    return {
      id: c.id,
      c,
      node,
      local,
      rack: rack.get(node) || null,
      shelfId,
      shelfLabel: shelf ? shelf.label : null,
      bay: bayOf(c),
      serial: metric(c, 'serial') || '',
      dev: metric(c, 'dev') || '',
      capacity,
      temp: metric(c, 'temp') !== undefined ? Number(metric(c, 'temp')) : null,
      wear: metric(c, 'wear') !== undefined ? Number(metric(c, 'wear')) : null,
      health: c.health,
      usage,
      member: m ? metric(m, 'state') : null,
      outOfFleet: detail.includes('out of fleet'),
      spare: /(^|·\s)spare(\s|·|$)/.test(detail),
      failedMark: /(^|·\s)failed(\s|·|$)/.test(detail),
    }
  })
  return { records, shelves, byId }
}

export const FILTERS = [
  ['', 'All drives'],
  ['failing', 'Failing'],
  ['degraded', 'Degraded'],
  ['rebuilding', 'Rebuilding'],
  ['full', 'Full (90%+)'],
  ['hot', 'Hot (50 °C+)'],
  ['out', 'Out of fleet'],
  ['spare', 'Spares'],
]

export function passes(d, filter) {
  switch (filter) {
    case 'failing':
      return d.health === 'error' || d.failedMark || d.member === 'failed'
    case 'degraded':
      return d.health === 'warn'
    case 'rebuilding':
      return d.member === 'rebuilding' || d.member === 'degraded'
    case 'full':
      return !!d.usage && d.usage.frac >= 0.9
    case 'hot':
      return d.temp !== null && d.temp >= 50
    case 'out':
      return d.outOfFleet
    case 'spare':
      return d.spare
    default:
      return true
  }
}

export function matches(d, search) {
  if (!search) return true
  const q = search.toLowerCase()
  return `${d.c.label} ${d.c.detail || ''} ${d.serial} ${d.dev} ${d.node} ${d.shelfLabel || ''} bay ${d.bay ?? ''}`
    .toLowerCase()
    .includes(q)
}

/// Group key and label for a drive.
export function groupOf(d, by) {
  if (by === 'node') return [d.node, d.node]
  if (by === 'rack') return [d.rack || '', d.rack ? `rack ${d.rack}` : 'no rack label']
  // chassis: a shelf is a node's, so two nodes' "shelf 1" are two chassis.
  const key = d.shelfId || `${d.node}/`
  return [key, d.shelfLabel ? `${d.shelfLabel} · ${d.node}` : `${d.node} · no enclosure`]
}

export function totals(list) {
  const t = { drives: 0, capacity: 0, failing: 0, degraded: 0, rebuilding: 0, full: 0, hot: 0, nodes: new Set(), withUsage: 0, used: 0, slab: 0 }
  for (const d of list) {
    t.drives++
    t.capacity += d.capacity
    t.nodes.add(d.node)
    if (passes(d, 'failing')) t.failing++
    else if (passes(d, 'degraded')) t.degraded++
    if (passes(d, 'rebuilding')) t.rebuilding++
    if (passes(d, 'full')) t.full++
    if (passes(d, 'hot')) t.hot++
    if (d.usage) {
      t.withUsage++
      t.used += d.usage.used
      t.slab += d.usage.slab
    }
  }
  return { ...t, nodes: t.nodes.size }
}

/// Groups of drives, each with its totals and its drives ordered by bay.
/// Chassis groups are listed by node, then shelf; the worst-off group is not
/// moved to the top, because a map that reshuffles as health changes is one
/// nobody can learn.
export function groups(records, { by = 'chassis', filter = '', search = '' } = {}) {
  const out = new Map()
  for (const d of records) {
    if (!passes(d, filter) || !matches(d, search)) continue
    const [key, label] = groupOf(d, by)
    if (!out.has(key)) out.set(key, { key, label, node: d.node, drives: [] })
    out.get(key).drives.push(d)
  }
  const list = [...out.values()]
  for (const g of list) {
    g.drives.sort((a, b) => (a.bay ?? 1e9) - (b.bay ?? 1e9) || a.c.label.localeCompare(b.c.label))
    g.totals = totals(g.drives)
  }
  list.sort((a, b) => a.node.localeCompare(b.node) || a.label.localeCompare(b.label, undefined, { numeric: true }))
  return list
}

/// How many bays wide to draw a chassis: its bays as a grid the shape the
/// common enclosures are — 12 across up to 60 bays, 15 across above.
export function columns(drives) {
  const bays = drives.reduce((m, d) => (d.bay !== null ? Math.max(m, d.bay + 1) : m), 0) || drives.length
  if (bays <= 12) return Math.max(bays, 1)
  return bays <= 60 ? 12 : 15
}

/// Cells for a chassis, one per bay in order, with empty bays kept empty:
/// a missing drive is a hole in the map, which is the point of a map.
export function cells(drives) {
  const withBay = drives.filter((d) => d.bay !== null)
  const without = drives.filter((d) => d.bay === null)
  const max = withBay.reduce((m, d) => Math.max(m, d.bay), -1)
  const slots = Array.from({ length: max + 1 }, () => null)
  for (const d of withBay) slots[d.bay] = d
  return [...slots, ...without]
}

export const MODES = [
  ['health', 'Health'],
  ['temp', 'Temperature'],
  ['wear', 'Wear'],
  ['usage', 'Usage'],
]

/// A cell's colour for a mode: a CSS colour, and the value it stands for.
/// `null` means the mode says nothing about this drive — drawn hatched, not
/// green, because no data is not good news.
export function heat(d, mode) {
  if (!d) return { css: 'transparent', value: null, text: 'empty bay' }
  switch (mode) {
    case 'temp':
      if (d.temp === null) return none('no temperature reported')
      return { css: ramp((d.temp - 25) / 35), value: d.temp, text: `${d.temp} °C` }
    case 'wear':
      if (d.wear === null) return none('no wear reported')
      return { css: ramp(d.wear / 100), value: d.wear, text: `${d.wear}% worn` }
    case 'usage':
      if (!d.usage) {
        return none(d.local ? 'no slab on this drive' : 'usage is read from this node’s engine only (stormdrive#12)')
      }
      return { css: ramp(d.usage.frac), value: d.usage.frac, text: `${Math.round(d.usage.frac * 100)}% used` }
    default: {
      const css = { ok: 'var(--ok)', warn: 'var(--warn-strong)', error: 'var(--error)', idle: 'var(--text-faint)' }[d.health]
      return { css: css || 'var(--text-faint)', value: d.health, text: d.health }
    }
  }
}

function none(text) {
  return { css: null, value: null, text }
}

/// Cool to hot: green, amber, red, on a 0..1 scale.
export function ramp(x) {
  const v = Math.max(0, Math.min(1, Number.isFinite(x) ? x : 0))
  const hue = 130 - 130 * v
  return `hsl(${hue.toFixed(0)} 65% 45%)`
}
