// The History page's model (#121), with plain node: `node web/src/lib/history.test.mjs`.
import assert from 'node:assert/strict'
import { kernelLine, kernelLines, timelineRows, slowestPhases, counterTrends, findings, assetChange, assetGroups, bytes, seconds, bootEnd } from './history.js'

// PID 1's own format (stormpump history.rs kmsg_line, gap_line).
const log = [
  '7 [    0.000123] <6> Linux version 6.17',
  '8 [    1.500000] <3> nvme nvme0: I/O timeout',
  '-- 4 kernel record(s) lost before they were saved (seq 9..12) --',
  '13 [    2.000000] <4> a warning',
  'something else',
  '',
].join('\n')
assert.deepEqual(kernelLine('8 [    1.500000] <3> x y'), { seq: 8, time: '1.500000', pri: 3, level: 'err', text: 'x y' })
assert.equal(kernelLine('-- 4 kernel --').gap, true)
assert.equal(kernelLines(log).length, 5)
assert.deepEqual(kernelLines(log, 3).map((l) => l.level), ['err', 'gap'], 'errors, and the gap always')
assert.deepEqual(kernelLines(log, 7, 'NVME').map((l) => l.level), ['err', 'gap'], 'search is case-blind')
assert.deepEqual(kernelLines('', 7), [])

const ev = [
  { t_ms: 1, event: 'boot', boot_id: 'a' },
  { t_ms: 2, event: 'phase', name: 'early', ms: 12 },
  { t_ms: 3, event: 'phase', name: 'modules', ms: 900 },
  { t_ms: 4, event: 'refused', name: 'x' },
  { t_ms: 5, event: 'exited', name: 'y' },
  { t_ms: 6, event: 'started', name: 'z' },
]
assert.deepEqual(timelineRows(ev).map((r) => [r.event, r.tone]), [['phase', ''], ['phase', ''], ['refused', 'bad'], ['exited', 'warn'], ['started', '']])
assert.deepEqual(slowestPhases(ev, 1).map((p) => p.name), ['modules'])

// Newest first, as the API gives them.
const recs = [
  { at: 'c', counters: { media_errors: 33, power_on_hours: 90, cycles: 4 }, findings: [{ counter: 'media_errors', change: 'grew', from: 32, to: 33, since: 'b', other_boot: true }] },
  { at: 'b', counters: { media_errors: 32, power_on_hours: 80, cycles: 2 }, findings: [{ counter: 'cycles', change: 'went_back', from: 9, to: 2, since: 'a', other_boot: false }] },
  { at: 'a', counters: { media_errors: 32, power_on_hours: 70, cycles: 9, same: 1 } },
]
const t = counterTrends(recs)
assert.deepEqual(t.map((c) => c.name), ['cycles', 'media_errors', 'power_on_hours', 'same'], 'backwards, then moved, then still')
assert.deepEqual([t[0].first, t[0].latest, t[0].delta, t[0].back], [9, 4, -5, true])
assert.deepEqual([t[1].first, t[1].latest, t[1].delta, t[1].back], [32, 33, 1, false])
const f = findings(recs)
assert.equal(f[0].message, 'media_errors grew 32 → 33 since b (an earlier boot)')
assert.equal(f[1].bad, true)
assert.match(f[1].message, /went backwards 9 → 2 since a: /)

assert.equal(assetChange({ item: 'dimm/A1', change: 'removed' }), 'dimm/A1 removed')
assert.equal(assetChange({ item: 'hba/x', change: 'changed', fields: ['firmware: 1 → 2'] }), 'hba/x firmware: 1 → 2')
const g = assetGroups({ system: { vendor: 'Dell', x: null }, 'dimm/A1': { size_mb: 8 }, 'dimm/A2': { size_mb: 8 }, 'drive/a/b': 'v' })
assert.deepEqual(g.map((x) => x.group), ['system', 'dimm', 'drive'])
assert.deepEqual(g[0].rows[0].fields, [['vendor', 'Dell']])
assert.equal(g[1].rows.length, 2)
assert.equal(g[2].rows[0].name, 'a/b')

assert.equal(bytes(512), '512 B')
assert.equal(bytes(16 << 20), '16.0 MiB')
assert.equal(bytes(null), '—')
assert.equal(seconds(250), '250 ms')
assert.equal(seconds(9000), '9.0 s')
assert.equal(bootEnd({ end: 'crashed' }).health, 'error')
assert.equal(bootEnd({ end: 'no clean end' }).health, 'warn')
console.log('history.test.mjs PASS')
