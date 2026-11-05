// The offered drives' model (#153), with plain node: `node web/src/lib/driveoffer.test.mjs`.
import assert from 'node:assert/strict'
import { engineDrives, offerFor, riskText, planSentence, statusLine } from './driveoffer.js'

const drive = (id, metrics, actions = []) => ({ id: `sb:drive:${id}`, label: `/dev/${id}`, metrics: Object.entries(metrics).map(([label, value]) => ({ label, value })), actions })
const fmt = { id: 'format', method: 'POST', path: '/x/format' }
const feed = [
  drive('a', { serial: 'S1', path: '/dev/a', offer: 'foreign', holds: 'an XFS filesystem', confirm: 'S1' }, [fmt]),
  drive('b', { serial: 'S2', path: '/dev/b', offer: 'blank', confirm: 'S2' }),
  drive('c', { serial: 'S3', path: '/dev/c', offer: 'ours', slabs: '2', volumes: '5' }),
  drive('d', { serial: 'S4', path: '/dev/d' }),
  { id: 'sb:volume:v', metrics: [{ label: 'offer', value: 'blank' }] },
]
const o = engineDrives(feed)
assert.deepEqual(o.map((d) => d.state), ['foreign', 'blank', 'ours'], 'a drive with no offer is not guessed at')
assert.deepEqual(o.map((d) => d.offered), [true, true, false])
assert.equal(o[0].sentence, 'holds an XFS filesystem')
assert.equal(o[1].sentence, 'blank, nothing found on it')
assert.equal(o[2].sentence, 'ours: 2 slab(s), 5 volume(s), adopted')
assert.equal(o[0].confirm, 'S1')
assert.ok(o[0].action, 'the action, for who may')
assert.equal(o[1].action, null, 'withheld by the guard, so nothing to press')

assert.equal(offerFor(o, { serial: 'S2', dev: '/dev/zzz' }).id, 'sb:drive:b', 'by serial, whatever the path')
assert.equal(offerFor(o, { serial: '', dev: '/dev/a' }).id, 'sb:drive:a')
assert.equal(offerFor(o, { serial: 'no', dev: 'no' }), null)
assert.equal(offerFor(o, { serial: '', dev: '' }), null, 'an empty name never matches')

// The engine's own wording (stormblock drive_offer.rs risk).
const said = 'formatting /dev/sdq (ST2000DM008, serial ZFL1ABC, 2000398934016 bytes) as a slab erases what it holds: an XFS filesystem. A power cut while it is formatted leaves the drive unusable until it is formatted again. Nothing formats a drive on its own (#154): to go ahead, send "confirm": "ZFL1ABC"'
const r = riskText(said)
assert.match(r, /erases what it holds: an XFS filesystem/)
assert.match(r, /power cut/)
assert.doesNotMatch(r, /send|confirm/, 'not the API instruction')
assert.equal(riskText('something else'), 'something else', 'other wording is kept whole')

assert.match(planSentence({ moves: 0, bytes: 0, left: 0 }), /Nothing to move/)
assert.equal(planSentence({ moves: 12, bytes: 3 << 20, left: 0 }), '12 extent(s) would move, 3.0 MiB in all.')
assert.match(planSentence({ moves: 1, bytes: 512, left: 4 }), /512 B in all\. 4 extent\(s\) stay/)

assert.equal(statusLine(null).running, false)
assert.deepEqual(statusLine({ state: 'running', moved: 3, planned: 9, bytes: 1024, skipped: 0, failed: 0 }), { running: true, failed: false, line: 'Rebalance running: 3 of 9 moved, 1.0 KiB' })
const done = statusLine({ state: 'done', moved: 8, planned: 9, bytes: 0, skipped: 0, failed: 1 })
assert.equal(done.running, false)
assert.equal(done.failed, true)
assert.match(done.line, /1 failed/)
console.log('driveoffer.test.mjs PASS')
