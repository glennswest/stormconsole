// node web/src/lib/driveformat.test.mjs
import assert from 'node:assert/strict'
import { byNode, chosen, currentText, duration, finished, formatText, isRunning, phrase, readBack, request, selectable, timing } from './driveformat.js'

// forge1's H7210A521SUN010T, as stormdrive#108 reads it.
const netapp = {
  node: 'forge1', id: 'a', name: 'sdb', serial: 'SN1', bay: 2, blockSize: 520, offersPi: false,
  current: { blockSize: 520, protection: 'none', protType: 0 },
  planned: { blockSize: 4096, protection: 'type1', basis: 'try', fallbacks: [{ blockSize: 4096, protection: 'none' }, { blockSize: 512, protection: 'type1' }] },
}

assert.equal(formatText(4096, 'type1'), '4096 + PI type 1')
assert.equal(formatText(4096, 'none'), '4096, no PI')
assert.equal(currentText(netapp), '520, no PI')
assert.equal(currentText({ blockSize: 512, current: { blockSize: 512, protType: 2 } }), '512 + PI type 2')
assert.equal(currentText({ blockSize: 512 }), '512', 'not probed: no claim about PI')

// The plan, and its fallbacks only when the run would use them.
let c = chosen(netapp, 4096, 'offered')
assert.deepEqual([c.block_size, c.protection, c.fallbacks.length], [4096, 'type1', 2])
c = chosen(netapp, 4096, 'none')
assert.deepEqual([c.protection, c.fallbacks.length], ['none', 0], 'a format off the plan is exact')
c = chosen(netapp, 512, 'offered')
assert.deepEqual([c.protection, c.fallbacks.length], ['none', 0], 'not offered, no PI')
c = chosen({ ...netapp, offersPi: true }, 512, 'offered')
assert.equal(c.protection, 'type1')
c = chosen({ planned: { error: 'not probed yet' }, offersPi: false }, 4096, 'offered')
assert.deepEqual([c.protection, c.fallbacks.length], ['none', 0], 'a plan that is an error is no plan')

// Who may be chosen.
assert.equal(selectable(netapp), true)
assert.equal(selectable({ ...netapp, whyNot: 'in the fleet' }), false)
assert.equal(selectable({ ...netapp, format: { state: 'running' } }), false)
assert.equal(isRunning({ ...netapp, format: { state: 'done' } }), false)
assert.equal(selectable({ ...netapp, format: { state: 'failed' } }), true, 'a failed one can go again')

// The clock: an estimate counts down between the drive's reports.
assert.equal(duration(50400 + 180), '14 h 3 m')
assert.equal(duration(200), '3 m 20 s')
assert.equal(duration(45), '45 s')
assert.equal(duration(90000), '1 d 1 h')
assert.equal(duration(null), '—')
let f = { state: 'running', startedUnix: 1000, progressAtUnix: 1500, etaSeconds: 3000 }
assert.deepEqual(timing(f, 1600), { elapsed: 600, left: 2900 })
assert.equal(timing({ ...f, etaSeconds: 10 }, 1600).left, 0, 'never below zero')
assert.equal(timing({ ...f, etaSeconds: null }, 1600).left, null, 'no estimate is not zero')
assert.deepEqual(timing({ state: 'done', startedUnix: 1000, finishedUnix: 1100, etaSeconds: 5 }, 9999), { elapsed: 100, left: null })

// The result read back.
assert.equal(readBack({ toBlockSize: 4096 }), null, 'nothing read yet')
assert.deepEqual(readBack({ toBlockSize: 4096, protection: 'type1', blockSizeAfter: 4096, protTypeAfter: 1 }), { text: '4096 + PI type 1', matches: true })
assert.deepEqual(readBack({ toBlockSize: 4096, protection: 'none', blockSizeAfter: 4096, protTypeAfter: 0 }), { text: '4096, no PI', matches: true })
assert.deepEqual(readBack({ toBlockSize: 4096, protection: 'type1', blockSizeAfter: 512, protTypeAfter: 0 }), { text: '512, no PI', matches: false }, 'took it, reads otherwise')

// The confirmation, and what is sent.
assert.equal(phrase([netapp]), 'SN1')
assert.equal(phrase([netapp, { ...netapp, serial: 'SN2' }]), 'format 2 drives')
assert.deepEqual(request([netapp], 4096, 'offered', 'SN1'), { confirm: 'SN1', drives: [{ node: 'forge1', id: 'a', block_size: 4096, protection: 'type1' }] })

// The order.
const g = byNode([{ node: 'b', name: 'x' }, { node: 'a', name: 'z', bay: 4 }, { node: 'a', name: 'y', bay: 1 }, { node: 'a', name: 'w' }])
assert.deepEqual(g.map((n) => n.node), ['b', 'a'])
assert.deepEqual(g[1].drives.map((d) => d.name), ['y', 'z', 'w'], 'by bay, with no bay last')
assert.deepEqual(finished([{ name: '1', format: { state: 'done', finishedUnix: 5 } }, { name: '2', format: { state: 'running' } }, { name: '3', format: { state: 'failed', finishedUnix: 9 } }, { name: '4' }]).map((r) => r.name), ['3', '1'])
console.log('PASS driveformat.test.mjs')
