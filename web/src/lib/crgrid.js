// A grid for every custom resource (#348): what the pages do with what the
// k8s plugin's /crds and /cr/<crd> answer. Kept apart from the pages so it
// is tested on its own (node web/src/lib/crgrid.test.mjs).
import { ago } from './ui/time.js'

/// A cell as text, by its column's type: a date is an age (kubectl prints
/// it so), anything structured is its JSON, missing is empty.
export function cellText(col, v) {
  if (v === undefined || v === null || v === '') return ''
  if (col.type === 'date' && typeof v === 'string') return ago(v)
  if (typeof v === 'boolean') return v ? 'True' : 'False'
  if (typeof v === 'object') return JSON.stringify(v)
  return String(v)
}

/// The columns shown: `priority > 0` only when wide, as `-o wide`; the
/// project column only when no project is chosen.
export function shownColumns(cols, { wide = false, project = '' } = {}) {
  return (cols || []).filter((c) => (wide || !(c.priority > 0)) && !(project && c.key === 'namespace'))
}

/// How many columns `wide` would add.
export function wideCount(cols) {
  return (cols || []).filter((c) => c.priority > 0).length
}

/// Does a row match the search? Any shown cell, as it is printed.
export function matches(row, cols, q) {
  const s = (q || '').trim().toLowerCase()
  if (!s) return true
  return cols.some((c) => cellText(c, row[c.key]).toLowerCase().includes(s))
}

/// One message of the live stream applied to the rows.
export function applyChange(rows, msg) {
  if (!msg || !msg.type) return rows
  if (msg.type === 'snapshot') return msg.rows || []
  if (msg.type === 'DELETED') return rows.filter((r) => r.id !== msg.id)
  if ((msg.type === 'ADDED' || msg.type === 'MODIFIED') && msg.row) {
    const i = rows.findIndex((r) => r.id === msg.row.id)
    if (i < 0) return [...rows, msg.row].sort((a, b) => a.id.localeCompare(b.id))
    const next = rows.slice()
    next[i] = msg.row
    return next
  }
  return rows
}

// What a state word means, for colour. Said once for every kind, so a
// Ready that is green on drives is green on raidsets.
const OK = /^(ok|ready|active|running|healthy|good|bound|succeeded|complete|completed|established|available|passed|online|up|clean|optimal|in ?sync|synced|idle)$/i
const WARN = /^(pending|degraded|rebuilding|resyncing|recovering|progressing|creating|provisioning|updating|draining|unknown|warn|warning|waiting|formatting|scrubbing|reshaping|initializing|starting|stopping|terminating|marginal)$/i
const ERROR = /^(failed|failure|error|critical|failing|offline|down|lost|missing|crashloopbackoff|broken|faulted|bad)$/i

export function toneOf(text) {
  const t = String(text ?? '').trim()
  if (!t) return ''
  if (OK.test(t)) return 'ok'
  if (WARN.test(t)) return 'warn'
  if (ERROR.test(t)) return 'error'
  return ''
}

/// A bar of `n` steps for a fraction, as text the grid can print.
export function bar(fraction, steps = 8) {
  const f = Math.max(0, Math.min(1, Number(fraction) || 0))
  const on = Math.round(f * steps)
  return '▰'.repeat(on) + '▱'.repeat(steps - on)
}

/// A percentage from what a progress column holds: 42, "42%", 0.42, "3/8".
export function fractionOf(v) {
  if (v === null || v === undefined || v === '') return null
  if (typeof v === 'number') return v > 1 ? v / 100 : v
  const s = String(v).trim()
  const of = s.match(/^(\d+(?:\.\d+)?)\s*\/\s*(\d+(?:\.\d+)?)$/)
  if (of) return Number(of[2]) > 0 ? Number(of[1]) / Number(of[2]) : null
  const pc = s.match(/^(\d+(?:\.\d+)?)\s*%?$/)
  if (pc) {
    const n = Number(pc[1])
    return s.endsWith('%') || n > 1 ? n / 100 : n
  }
  return null
}

// A RAID set's (or pool's) phase as how far along it is: a bar that fills
// as the set comes up, the colour of what it means.
const PHASE_STEPS = {
  pending: 1, creating: 2, initializing: 2, assembling: 2, resyncing: 3, rebuilding: 3, reshaping: 3,
  degraded: 3, ready: 4, active: 4, clean: 4, healthy: 4, failed: 4,
}

function phaseBar(v) {
  const t = String(v ?? '')
  if (!t) return null
  const step = PHASE_STEPS[t.toLowerCase()]
  return { value: step ? `${bar(step / 4, 4)} ${t}` : t, tone: toneOf(t) }
}

function dot(v) {
  const t = String(v ?? '')
  return t ? { value: `● ${t}`, tone: toneOf(t) } : null
}

function progressBar(v) {
  const f = fractionOf(v)
  return f === null ? null : { value: `${bar(f)} ${Math.round(f * 100)}%`, tone: f >= 1 ? 'ok' : 'accent' }
}

function link(v) {
  const t = String(v ?? '')
  return t ? { value: t, tone: 'accent' } : null
}

// Our kinds' own renderers, by CRD and column name, on top of the generic
// grid (#348 §4): the same grid with a renderer per kind, never a page of
// its own that drifts.
const CURATED = {
  'drives.storage.storm.io': { Health: dot, Link: dot, Prep: dot },
  'raidsets.storage.storm.io': { Phase: phaseBar },
  'raidpools.storage.storm.io': { Phase: phaseBar },
  'raidspares.storage.storm.io': { Phase: dot },
  'replications.storage.storm.io': { Progress: progressBar, Phase: dot },
  'volumes.storage.storm.io': { Owner: link, Phase: dot },
  'driveoperations.storage.storm.io': { Phase: dot },
  'clustermembers.cluster.storm.io': { Phase: dot },
  'clusterupdates.stormupdate.storm.io': { Phase: dot },
}

// Every other kind: a column that is a state gets its colour.
const STATEY = /^(phase|status|state|health|ready|link|condition)$/i

/// How one cell is drawn: `{value, tone}` for a coloured cell, or null for
/// plain text.
export function curatedCell(crd, col, v) {
  if (!col.printer || v === '' || v === null || v === undefined) return null
  const own = CURATED[crd]?.[col.name]
  if (own) return own(v)
  if (/progress|percent/i.test(col.name)) return progressBar(v)
  if (STATEY.test(col.name) && col.type !== 'boolean') {
    const t = String(v)
    const tone = toneOf(t)
    return tone ? { value: `● ${t}`, tone } : null
  }
  return null
}

/// Is this kind one we curate?
export function isCurated(crd) {
  return !!CURATED[crd]
}

/// A coloured cell for the grid's `metrics` render (it carries a tone).
/// It sorts by its text: the grid compares `String(cell)`.
function toned(value, tone, text) {
  const cell = [{ label: '', value, tone }]
  Object.defineProperty(cell, 'toString', { value: () => text, enumerable: false })
  return cell
}

/// The DataGrid columns and rows for a grid answer.
export function toGrid(crd, cols, rows) {
  const coloured = new Set(cols.filter((c) => rows.some((r) => curatedCell(crd, c, r[c.key]))).map((c) => c.key))
  const columns = cols.map((c) => ({
    key: c.key,
    label: c.name,
    render: c.key === 'name' ? 'mono' : coloured.has(c.key) ? 'metrics' : (row) => row[`${c.key}$text`],
  }))
  const out = rows.map((r) => {
    const o = { id: r.id, href: r.href }
    for (const c of cols) {
      const v = r[c.key]
      const text = cellText(c, v)
      o[`${c.key}$text`] = text
      if (coloured.has(c.key)) {
        const cur = curatedCell(crd, c, v)
        o[c.key] = cur ? toned(cur.value, cur.tone, text) : toned(text, 'muted', text)
      } else if (c.type === 'date' && typeof v === 'string') {
        // Sorting reads row[key]: an age sorts by its time, youngest first.
        o[c.key] = -Date.parse(v) || 0
      } else {
        o[c.key] = v === '' ? null : v
      }
    }
    return o
  })
  return { columns, rows: out }
}

/// The CRD list, searched: name, group, kind, short names.
export function filterCrds(items, q, { only = '' } = {}) {
  const s = (q || '').trim().toLowerCase()
  return (items || []).filter((k) => {
    if (only === 'storm' && !/storm\.io$/.test(k.group)) return false
    if (only === 'instances' && !(k.count > 0)) return false
    if (!s) return true
    return [k.name, k.group, k.kind, ...(k.short || [])].some((x) => String(x || '').toLowerCase().includes(s))
  })
}

/// A condition's status as a tone: True is good unless the condition is a
/// bad thing being true (Degraded, Failing, …).
export function conditionTone(c) {
  const bad = /degraded|fail|error|pressure|unavailable|stalled|disrupt|missing/i.test(c.type || '')
  if (c.status === 'True') return bad ? 'error' : 'ok'
  if (c.status === 'False') return bad ? 'ok' : 'warn'
  return 'warn'
}

/// A value of spec or status for the structured sections: scalars as
/// text, lists of scalars joined, anything deeper is shown as YAML-ish JSON.
export function fieldRows(obj) {
  if (!obj || typeof obj !== 'object') return []
  return Object.entries(obj).map(([k, v]) => {
    if (v === null || v === undefined) return { key: k, text: '' }
    if (typeof v !== 'object') return { key: k, text: String(v) }
    if (Array.isArray(v) && v.every((x) => x === null || typeof x !== 'object')) return { key: k, text: v.join(', ') }
    return { key: k, nested: JSON.stringify(v, null, 2) }
  })
}
