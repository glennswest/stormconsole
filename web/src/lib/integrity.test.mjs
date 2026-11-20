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
