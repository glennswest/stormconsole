// node web/src/lib/driveactions.test.mjs
import assert from 'node:assert/strict'
import { commonActions, groupActions } from './driveactions.js'

// stormdrive's drive actions (components.rs), forge1's sdb.
const a = (id, label, danger = false, enabled = true) => ({ id, label, danger, enabled, method: 'POST', path: `/x/${id}` })
const feed = [
  a('locate-on', 'Locate'), a('locate-off', 'LED off'), a('fleet-join', 'Join fleet'), a('test-smoke', 'Smoke test'),
  a('test-scan', 'Read scan'), a('test-destructive', 'Destructive test', true), a('enroll', 'Enrol (data slab)', true),
  a('format-4k', 'Format 4K', true), a('overcommit-2x', 'Overcommit 2×', true), a('mark-spare', 'Mark spare'),
]
const g = groupActions(feed, { format: { enabled: true } })
assert.deepEqual(g.map((x) => x.name), ['Locate', 'Test', 'Fleet', 'Designate', 'Destructive'])
const last = g.at(-1)
assert.equal(last.danger, true)
assert.deepEqual(last.items.map((i) => i.label), ['Format…', 'Destructive test', 'Enrol (data slab)', 'Overcommit 2×'])
assert.ok(!g.flatMap((x) => x.items).some((i) => i.key === 'format-4k'), "the feed's Format 4K is never offered")
assert.ok(g.slice(0, -1).every((x) => x.items.every((i) => !i.danger)), 'nothing destructive before the last group')

// An in-use drive: Format… shown, off, with why.
const off = groupActions(feed, { format: { enabled: false, why: 'sda is the system drive' } }).at(-1).items[0]
assert.deepEqual([off.enabled, off.why], [false, 'sda is the system drive'])
assert.ok(!groupActions(feed).flatMap((x) => x.items).some((i) => i.format), 'no Format… unless asked')

// Replace… and Update firmware… (#317): their own group, before the destructive one, off with why.
const m = groupActions(feed, { format: { enabled: true }, replace: { enabled: false, why: 'the system drive' }, firmware: { enabled: true } })
assert.deepEqual(m.map((x) => x.name), ['Locate', 'Test', 'Fleet', 'Designate', 'Maintain', 'Destructive'])
assert.deepEqual(m[4].items.map((i) => [i.label, i.enabled, i.danger, i.why]), [['Replace…', false, false, 'the system drive'], ['Update firmware…', true, false, '']])
assert.ok(m[4].items[0].replace && m[4].items[1].firmware)
// A selection: what any of them offers, counted.
const c = commonActions([{ actions: [a('locate-on', 'Locate'), a('fleet-join', 'Join fleet', false, false)] }, { actions: [a('locate-on', 'Locate'), a('fleet-join', 'Join fleet')] }])
assert.deepEqual(c.map((x) => [x.id, x.enabled, x.count]), [['locate-on', true, 2], ['fleet-join', true, 1]])
assert.deepEqual(commonActions([]), [])
console.log('PASS driveactions.test.mjs')
