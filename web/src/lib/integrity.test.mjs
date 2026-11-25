import assert from 'node:assert/strict'
import { counterRows, whereLine, dayWords, scrubLine, passLine, memberOf, setLevel, levelTone, bitrotAlarms, at } from './integrity.js'

// md0 on forge1 as the drive plugin's /integrity answers it (stormraid v0.27.0).
const counts = (l3, repaired, rows = 0) => ({ mismatches: 1 + l3, l1: 1, l2: 0, l3, repaired, piErrors: 2, readErrors: 3, writeErrors: 0, unrecoverableRows: rows })
const md0 = {
  name: 'md0', node: 'forge1', phase: 'Ready',
  integrity: {
    totals: counts(7, 7), day: { since: 1000, full: true, counts: counts(2, 2) }, last: { mismatch: '2026-11-20T10:00:00Z', repair: '2026-11-20T10:00:00Z' },
    scrubbing: false, scrubPasses: 4, scrubEvery: '7d', nextScrub: '2026-11-27T10:00:00Z',
    lastScrubPass: { startedAt: '2026-11-20T09:00:00Z', completedAt: '2026-11-20T10:00:00Z', stripes: 1000, mismatches: 7, repaired: 7, unrecoverableRows: 0 },
  },
  bitrot: { type: 'BitRot', status: 'True', message: 'slot 2 (/dev/sdc): 8 mismatch(es)' },
  members: [
    { slot: 0, path: '/dev/sdb', drive: { component: 'drive:drive:61' }, counts: counts(0, 0), day: { since: 1000, full: true, counts: counts(0, 0) } },
    { slot: 2, path: '/dev/sdc', drive: { component: 'drive:drive:69', serial: 'NET003' }, counts: counts(7, 7), day: { since: 1000, full: true, counts: counts(2, 2) } },
  ],
}

// The counter rows: lifetime, the day, when it last rose.
const rows = counterRows(md0.integrity.totals, md0.integrity.day, md0.integrity.last)
assert.deepEqual(rows.map((r) => r.key), ['mismatches', 'repaired', 'unrecoverableRows', 'piErrors', 'readErrors', 'writeErrors'])
assert.deepEqual(rows.map((r) => [r.life, r.day]), [[8, 3], [7, 2], [0, 0], [2, 2], [3, 3], [0, 0]])
assert.equal(rows[0].last, '2026-11-20T10:00:00Z')
assert.equal(rows[0].tone, 'warn', 'rot found in the last day is amber')
assert.equal(rows[5].tone, '', 'nothing is plain')
const quiet = counterRows(counts(7, 7), { since: 1, full: true, counts: counts(0, 0) })
assert.equal(quiet[1].tone, '', 'a lifetime count with nothing new is history')
assert.equal(counterRows(counts(0, 0, 2), { counts: counts(0, 0, 1) })[2].tone, 'error', 'rows past parity are red')
assert.equal(counterRows(counts(1, 1), null)[0].day, null, 'no day kept: no day column value')
assert.ok(!counterRows(counts(1, 1), null, {}, { member: true }).some((r) => r.key === 'unrecoverableRows'), 'a member has no rows of its own')
assert.deepEqual(counterRows(null, null), [])

assert.equal(whereLine(md0.integrity.totals), 'L1 read path 1 · L3 scrub 7')
assert.equal(dayWords({ since: 1000, full: true }), 'last 24 h')
assert.match(dayWords({ since: Date.now() / 1000 - 7200, full: false }), /^since \d\d:\d\d UTC \(this console has kept 2 h\)$/)
assert.equal(dayWords(null), 'nothing kept yet')
assert.equal(scrubLine(md0.integrity), 'not scrubbing · a pass every 7d · 4 passes done · next 2026-11-27 10:00:00 UTC')
assert.equal(scrubLine({ scrubbing: true, scrubEvery: 'off', scrubPasses: 1 }), 'scrubbing now · passes back to back · 1 pass done')
assert.equal(passLine(md0.integrity.lastScrubPass), '2026-11-20 09:00:00 UTC → 2026-11-20 10:00:00 UTC · 1000 stripes · 7 mismatches · 7 repaired')
assert.equal(at('2026-11-20T10:00:00.123Z'), '2026-11-20 10:00:00 UTC')

// A drive finds its set and member by its feed id.
const ans = { sets: [md0] }
assert.equal(memberOf(ans, 'drive:drive:69').member.slot, 2)
assert.equal(memberOf(ans, 'drive:drive:69').set.name, 'md0')
assert.equal(memberOf(ans, 'drive:drive:none'), null)
assert.equal(memberOf(null, 'x'), null)

// The set's level.
assert.equal(setLevel(md0), 'bitrot')
assert.equal(levelTone('bitrot'), 'warn')
assert.equal(setLevel({ ...md0, integrity: { ...md0.integrity, day: { counts: counts(0, 0, 1) } } }), 'lost')
assert.equal(levelTone('lost'), 'error')
assert.equal(setLevel({ ...md0, bitrot: { status: 'False' } }), 'ok')
assert.equal(setLevel({ ...md0, integrity: null }), 'unknown')

// The alarm bar: the bit-rot rows only, red first.
const feed = [
  { id: 'drive:alarm:link:x', kind: 'alarm', health: 'error', detail: 'Drive link failing', link: '#/drives?drive=x' },
  { id: 'drive:alarm:bitrot:md1', kind: 'alarm', health: 'warn', detail: 'Bit rot on RAID set md1', link: '#/drives/integrity?set=md1' },
  { id: 'drive:alarm:bitrot:md0', kind: 'alarm', health: 'error', detail: 'Bit rot on RAID set md0 — 2 row(s) past parity: data lost', link: '#/drives/integrity?set=md0' },
]
assert.deepEqual(bitrotAlarms(feed).map((a) => [a.tone, a.href]), [['error', '#/drives/integrity?set=md0'], ['warn', '#/drives/integrity?set=md1']])

console.log('integrity: ok')

// The trend (#385): asked of the set's node's ironprom by set, and by set and slot for one member.
{
  const { setTrend, memberTrend, TREND_WINDOW } = await import('./integrity.js')
  assert.deepEqual(setTrend(md0), { query: { entity: 'raidset', node: 'forge1', set: 'md0' }, node: 'forge1', why: '' })
  assert.deepEqual(memberTrend(md0, md0.members[1]), { query: { entity: 'member', set: 'md0', slot: '2' }, node: 'forge1', why: '' })
  assert.deepEqual(memberTrend(md0, md0.members[0]).query, { entity: 'member', set: 'md0', slot: '0' }, 'slot 0 is a slot')
  assert.equal(memberTrend(md0, { path: '/dev/sdx' }).query, null)
  assert.match(memberTrend(md0, { path: '/dev/sdx' }).why, /no slot/)
  assert.match(setTrend({ name: 'md1' }).why, /no node/)
  assert.match(memberTrend({ name: 'md1' }, md0.members[0]).why, /no node/)
  assert.equal(TREND_WINDOW, 604800)
}

// The drive's own counters (#384): only what the drive gives, an uncorrected rise red, corrected never coloured.
{
  const { driveRows, driveCountersOf, sourceWords } = await import('./integrity.js')
  const net003 = {
    component: 'drive:drive:5000cca26a819e69', source: 'log_sense',
    counts: { readUncorrected: 2, writeUncorrected: 0, grownDefects: 5, readCorrected: 120, writeCorrected: 4 },
    day: { since: 1000, full: true, counts: { readUncorrected: 1, writeUncorrected: 0, grownDefects: 2, readCorrected: 40, writeCorrected: 0 } },
  }
  assert.equal(driveCountersOf({ drives: [net003] }, 'drive:drive:5000cca26a819e69'), net003)
  assert.equal(driveCountersOf({ drives: [net003] }, 'drive:drive:x'), null)
  const rows = driveRows(net003)
  assert.deepEqual(rows.map((r) => r.key), ['readUncorrected', 'writeUncorrected', 'grownDefects', 'readCorrected', 'writeCorrected'], 'what the drive gives, in reading order')
  const by = Object.fromEntries(rows.map((r) => [r.key, r]))
  assert.deepEqual([by.readUncorrected.life, by.readUncorrected.day, by.readUncorrected.tone], [2, 1, 'error'])
  assert.equal(by.writeUncorrected.tone, '', 'nothing new: plain')
  assert.equal(by.grownDefects.tone, 'warn')
  assert.equal(by.readCorrected.tone, '', 'corrected is never coloured')
  assert.deepEqual(driveRows({ counts: { pending: 1 } }).map((r) => [r.key, r.day, r.tone]), [['pending', null, '']], 'no day kept: no rise, no colour')
  assert.deepEqual(driveRows(null), [])
  assert.match(sourceWords('log_sense'), /LOG SENSE/)
  assert.match(sourceWords('ata_smart'), /SMART/)
}

// What the node's engine sees of the drive (#394, stormdrive#160): its counters with their own day and rate,
// a PI/EEDP/medium rise red and the rest amber, the level in a line, its alarms in the feed.
{
  const { engineRows, driveRows, errorsLine, errorsTone, errorsLevelWords, perMinute, errorsAlarms } = await import('./integrity.js')
  const row = {
    component: 'drive:drive:5000cca26a819e69', source: 'log_sense',
    counts: { readUncorrected: 1, grownDefects: 4 },
    day: { since: 1000, full: true, counts: { readUncorrected: 1, grownDefects: 0 } },
    errors: {
      level: 'failing', since: 1763899940, streak: 2, why: ['pi_guard 0.5/min', 'read_uncorrected 0.0/min'],
      defectsPerDay: 1, pendingPerDay: null, perMin: { pi_guard: 0.5, read_uncorrected: 0.03, recovered_error: 0, grown: 0 },
    },
    errorCounts: { piGuard: 2, piAppTag: 0, piRefTag: 1, eedpGuard: 1, eedpAppTag: 0, eedpRefTag: 0, mediumError: 1, recoveredError: 0, hardwareError: 0, readRetryCorrected: 2, writeRetryCorrected: 0, nonMedium: 3 },
    errorsDay: { since: 2000, full: false, counts: { piGuard: 2, piRefTag: 0, eedpGuard: 0, readRetryCorrected: 1 } },
  }
  const rows = engineRows(row)
  assert.deepEqual(rows.map((r) => r.key), ['piGuard', 'piAppTag', 'piRefTag', 'eedpGuard', 'eedpAppTag', 'eedpRefTag', 'mediumError', 'hardwareError', 'recoveredError', 'readRetryCorrected', 'writeRetryCorrected', 'nonMedium'], 'what it gives, PI first; no verify page, no row')
  const by = Object.fromEntries(rows.map((r) => [r.key, r]))
  assert.deepEqual([by.piGuard.life, by.piGuard.day, by.piGuard.rate, by.piGuard.tone], [2, 2, 0.5, 'error'])
  assert.deepEqual([by.piRefTag.day, by.piRefTag.tone], [0, ''], 'counted before, nothing new: plain')
  assert.equal(by.readRetryCorrected.tone, 'warn', 'retry-corrected rose: amber')
  assert.equal(by.recoveredError.rate, 0)
  assert.equal(by.nonMedium.day, 0, 'not in the day: none rose')
  assert.deepEqual(engineRows({ counts: {} }), [], 'no engine block: no rows')
  const own = Object.fromEntries(driveRows(row).map((r) => [r.key, r.rate]))
  assert.deepEqual(own, { readUncorrected: 0.03, grownDefects: 0 }, 'the drive\'s own counters get the engine\'s rate')
  assert.equal(errorsLine(row.errors), `failing · since ${new Date(1763899940000).toISOString().replace('T', ' ').replace(/(\.\d+)?Z$/, ' UTC')} · 2 samples running with a PI check or medium error · 1 defect grown in the last day`)
  assert.equal(errorsLine({ level: 'ok', since: 0, streak: 0 }), 'ok')
  assert.equal(errorsLine(null), '')
  assert.deepEqual([errorsTone('failing'), errorsTone('degraded'), errorsTone('ok'), errorsTone('settling')], ['error', 'warn', '', ''])
  assert.match(errorsLevelWords('failing'), /two samples running/)
  assert.deepEqual([perMinute(0), perMinute(0.03), perMinute(0.5), perMinute(12.4), perMinute(null)], ['0', '0.03/min', '0.5/min', '12/min', '—'])
  const alarms = errorsAlarms([
    { id: 'drive:alarm:drive-errors:a', kind: 'alarm', health: 'warn', detail: 'Drive errors degraded: bay 1', link: '#/drives?drive=a' },
    { id: 'drive:alarm:link:x', kind: 'alarm', health: 'error', detail: 'Drive link failing: bay 2' },
    { id: 'drive:alarm:drive-errors:b', kind: 'alarm', health: 'error', detail: 'Drive errors failing: bay 3', link: '#/drives?drive=b' },
  ])
  assert.deepEqual(alarms.map((a) => [a.tone, a.href]), [['error', '#/drives?drive=b'], ['warn', '#/drives?drive=a']], 'red first, not the link\'s')
}
