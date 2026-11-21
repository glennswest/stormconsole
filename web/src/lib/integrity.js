// A RAID set's integrity (#368, stormraid#80): bit rot found and rewritten
// from parity, per set and per member drive, lifetime and for the last day.
// The drive plugin reads every RaidSet once a minute (GET
// /api/plugins/drive/integrity) and keeps a day of samples; this is the
// model the drive page's Integrity panel, the Integrity page and the alarm
// bar draw from.

// The counters, in the order a person reads them: what was found, what
// parity gave back, what parity could not, then the I/O errors.
export const COUNTERS = [
  ['mismatches', 'Checksum mismatches', 'Blocks that did not match their checksum, or that parity or the chunk hash showed wrong: bit rot found'],
  ['repaired', 'Rewritten from parity', 'Blocks rewritten on the drive with what parity gave back'],
  ['unrecoverableRows', 'Rows past parity', 'Rows with more bad blocks than parity: data lost (the set’s only)'],
  ['piErrors', 'PI check failures', 'I/O that failed its T10 PI check (guard, application or reference tag)'],
  ['readErrors', 'Read errors', 'Reads the drive failed'],
  ['writeErrors', 'Write errors', 'Writes the drive failed'],
]

// Where a mismatch was found.
export const WHERE = [
  ['l1', 'L1 read path'],
  ['l2', 'L2 write verify'],
  ['l3', 'L3 scrub'],
]

// The rows of a counter table: lifetime, the last day's rise, and when it last rose.
// `last` maps a counter to stormraid's `last…` time of it; the set's rows-past-parity
// have none per member, so a member leaves it out.
export function counterRows(counts, day, last = {}, { member = false } = {}) {
  if (!counts) return []
  const lastKey = { mismatches: 'mismatch', repaired: 'repair', unrecoverableRows: 'unrecoverable', piErrors: 'piError', readErrors: 'readError', writeErrors: 'writeError' }
  return COUNTERS.filter(([k]) => !(member && k === 'unrecoverableRows')).map(([key, word, title]) => {
    const life = counts[key] ?? 0
    const rose = day?.counts ? (day.counts[key] ?? 0) : null
    return { key, word, title, life, day: rose, last: last?.[lastKey[key]] || null, tone: tone(key, life, rose) }
  })
}

// Red for data lost, amber for rot found or errors, in the last day; a
// lifetime count with nothing new is history, and stays plain.
export function tone(key, life, rose) {
  const n = rose === null ? life : rose
  if (!n) return ''
  if (key === 'unrecoverableRows') return 'error'
  return rose === null ? '' : 'warn'
}

// Where the mismatches were found, as a line: "L1 read path 1 · L3 scrub 7".
export function whereLine(counts) {
  if (!counts) return ''
  return WHERE.filter(([k]) => counts[k]).map(([k, w]) => `${w} ${counts[k]}`).join(' · ')
}

// What the day column covers: "last 24 h", or since when the console began keeping.
export function dayWords(day) {
  if (!day) return 'nothing kept yet'
  if (day.full) return 'last 24 h'
  const t = new Date(day.since * 1000)
  return `since ${t.toISOString().slice(11, 16)} UTC (this console has kept ${hours(Date.now() / 1000 - day.since)})`
}

function hours(s) {
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))} min`
  return `${Math.round(s / 3600)} h`
}

// The scrub, in a line: running or not, how often, passes done, the next.
export function scrubLine(integ) {
  if (!integ) return ''
  const parts = []
  parts.push(integ.scrubbing ? 'scrubbing now' : 'not scrubbing')
  if (integ.scrubEvery) parts.push(integ.scrubEvery === 'off' ? 'passes back to back' : `a pass every ${integ.scrubEvery}`)
  parts.push(`${integ.scrubPasses ?? 0} pass${integ.scrubPasses === 1 ? '' : 'es'} done`)
  if (integ.nextScrub) parts.push(`next ${at(integ.nextScrub)}`)
  return parts.join(' · ')
}

// The last background pass, in a line.
export function passLine(p) {
  if (!p) return ''
  const when = p.completedAt ? `${at(p.startedAt)} → ${at(p.completedAt)}` : `started ${at(p.startedAt)}, running`
  return `${when} · ${p.stripes} stripes · ${p.mismatches} mismatch${p.mismatches === 1 ? '' : 'es'} · ${p.repaired} repaired${p.unrecoverableRows ? ` · ${p.unrecoverableRows} rows past parity` : ''}`
}

export function at(t) {
  return t ? String(t).replace('T', ' ').replace(/(\.\d+)?Z$/, ' UTC') : '—'
}

// The set and member a drive is, by its feed id, or null.
export function memberOf(answer, component) {
  for (const set of answer?.sets || []) {
    for (const m of set.members || []) {
      if (m.drive?.component === component) return { set, member: m }
    }
  }
  return null
}

// Whether `BitRot` is on.
export function rotting(set) {
  return set?.bitrot?.status === 'True'
}

// The set's state at a glance: lost (rows past parity in the last day),
// bitrot, ok, or unknown (no integrity: stormraid predates v0.27.0).
export function setLevel(set) {
  if (!set?.integrity) return 'unknown'
  const lost = set.integrity.day?.counts?.unrecoverableRows
  if (lost) return 'lost'
  if (rotting(set)) return 'bitrot'
  return 'ok'
}

export function levelTone(level) {
  return level === 'lost' ? 'error' : level === 'bitrot' ? 'warn' : ''
}

// The bit-rot alarms in the feed (the drive plugin's `alarm` rows), red first.
export function bitrotAlarms(components) {
  const rank = { error: 0, warn: 1 }
  return components
    .filter((c) => c.kind === 'alarm' && c.id.includes(':alarm:bitrot:'))
    .sort((a, b) => (rank[a.health] ?? 2) - (rank[b.health] ?? 2) || a.detail.localeCompare(b.detail))
    .map((c) => ({ id: c.id, text: c.detail, href: c.link || '#/drives/integrity', tone: c.health === 'error' ? 'error' : 'warn' }))
}

// The trend (#385): stormraid's counters over days, from the ironprom of the set's node, asked by name of the
// metrics plugin (`entity=raidset` for the set, `entity=member` for one slot of it). `null` with the reason
// when it cannot be asked: the series are labelled by set and slot, and kept on the set's node.
export function setTrend(set) {
  if (!set?.name) return { query: null, why: 'The set has no name.' }
  if (!set.node) return { query: null, why: 'The set names no node, so there is no ironprom to ask for its trend.' }
  return { query: { entity: 'raidset', node: set.node, set: set.name }, node: set.node, why: '' }
}

export function memberTrend(set, member) {
  if (!set?.name || !set.node) return setTrend(set)
  if (member?.slot === null || member?.slot === undefined) return { query: null, why: 'The member has no slot, and stormraid labels its series by slot.' }
  return { query: { entity: 'member', set: set.name, slot: String(member.slot) }, node: set.node, why: '' }
}

// The window an integrity trend opens on: a week, since a mismatch is rare.
export const TREND_WINDOW = 604800
