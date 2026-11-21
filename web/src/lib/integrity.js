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

// The drive's own counters (#384): what it says of itself, from its Drive object's
// status.health.smart, as the drive plugin reads them with the links (GET
// /api/plugins/drive/counters) and keeps a day of. In the order a person reads them:
// what the drive could not correct, what it remapped, then what it did correct.
// `bad`: an error a rise of which is red (stormblock#648's rule: an uncorrected
// error is failing); the rest amber. Corrected errors climb on a healthy drive in
// normal running, so they are never coloured.
export const DRIVE_COUNTERS = [
  ['readUncorrected', 'Read errors, uncorrected', 'Reads the drive could not correct (error counter log page 0x03): media errors', 'bad'],
  ['writeUncorrected', 'Write errors, uncorrected', 'Writes the drive could not correct (log page 0x02)', 'bad'],
  ['verifyUncorrected', 'Verify errors, uncorrected', 'Verifies the drive could not correct (log page 0x05)', 'bad'],
  ['reportedUncorrectable', 'Reported uncorrectable', 'Errors the drive could not correct on a host read (ATA 187)', 'bad'],
  ['offlineUncorrectable', 'Offline uncorrectable', 'Sectors the drive’s own scan could not read (ATA 198)', 'bad'],
  ['grownDefects', 'Grown defects', 'Blocks the drive has remapped since it left the factory (the grown defect list)', 'warn'],
  ['reallocated', 'Reallocated sectors', 'Sectors the drive has remapped (ATA 5)', 'warn'],
  ['pending', 'Pending sectors', 'Sectors waiting to be remapped (ATA 197)', 'warn'],
  ['crcErrors', 'Interface CRC errors', 'Transfers the link corrupted (ATA 199): the cable or the backplane', 'warn'],
  ['readCorrected', 'Read errors, corrected', 'Reads the drive corrected (ECC, retries): normal in running', ''],
  ['writeCorrected', 'Write errors, corrected', 'Writes the drive corrected', ''],
  ['verifyCorrected', 'Verify errors, corrected', 'Verifies the drive corrected', ''],
]

// The drive's row in the counters answer, by its feed id.
export function driveCountersOf(answer, component) {
  return (answer?.drives || []).find((d) => d.component === component) || null
}

// The rows of the drive's table: only the counters the drive gives.
export function driveRows(row) {
  const counts = row?.counts
  if (!counts) return []
  return DRIVE_COUNTERS.filter(([k]) => counts[k] !== undefined && counts[k] !== null).map(([key, word, title, bad]) => {
    const life = counts[key]
    const rose = row.day?.counts ? (row.day.counts[key] ?? 0) : null
    return { key, word, title, life, day: rose, tone: rose ? (bad === 'bad' ? 'error' : bad) : '' }
  })
}

// Where the counters come from, in a person's words.
export function sourceWords(source) {
  if (source === 'log_sense') return 'the drive’s own log pages (LOG SENSE) and grown defect list'
  if (source === 'ata_smart') return 'the drive’s SMART attributes'
  return source ? `the drive (${source})` : 'the drive'
}
