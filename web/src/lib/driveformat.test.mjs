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

// --- #308 -----------------------------------------------------------------
{
  const m = await import('./driveformat.js')
  // forge1: four HGST H7210A521SUN010T behind a PERC, no VPD 0xB4, PI 1 + 2,
  // physical 4096; plannedFormat 4096 + type1 for all four. sda is the system drive.
  const sup = { protect: true, pi_types: [1, 2], block_lengths: [], physical_block_size: 4096 }
  const plan = { blockSize: 4096, protection: 'type1', basis: 'try', fallbacks: [{ blockSize: 4096, protection: 'none' }, { blockSize: 512, protection: 'type1' }] }
  const drive = (name, extra = {}) => ({ node: 'forge1', id: `id-${name}`, name, serial: `SN-${name}`, supports: sup, planned: plan, offersPi: true, use: { in_use: false, use: 'free', badge: null }, ...extra })
  const sda = drive('sda', { current: { blockSize: 512, protection: 'type1', protType: 1 },
    use: { in_use: true, use: 'system', badge: 'System', reason: "sda holds this node's system slab (partition 2 'stormblock'); the node runs from it" },
    whyNot: "sda holds this node's system slab (partition 2 'stormblock'); the node runs from it. The system drive is never formatted from the console" })
  const [sdb, sdc, sdd] = ['sdb', 'sdc', 'sdd'].map((n) => drive(n))

  assert.equal(m.badgeOf(sda), 'System')
  assert.equal(m.badgeOf(sdb), null)
  assert.equal(m.selectable(sda), false, 'the system drive is never picked')
  assert.equal([sdb, sdc, sdd].every(m.selectable), true)

  const opts = m.options(sdb).map(m.optionKey)
  assert.deepEqual(opts, ['4096:type1', '4096:none', '512:type1', '512:none'])
  assert.equal(m.optionKey(m.defaultOption(sdb)), '4096:type1', 'the plan is the default')
  assert.equal(m.piNote(sdb), 'the drive also lists PI type 2; stormdrive formats type 1 or none')
  assert.equal(m.confirmLine(sdb, m.defaultOption(sdb)), 'sdb → 4096 + T10 PI type 1')
  assert.equal(m.planFor(sdb, m.defaultOption(sdb)).fallbacks.length, 2)
  assert.equal(m.planFor(sdb, { block_size: 512, protection: 'none' }).fallbacks.length, 0)

  // VPD 0xB4 listed: exactly what it lists.
  const listed = drive('sde', { planned: null, supports: { pi_types: [1], block_lengths: [{ length: 512, pi_types: [0, 1] }, { length: 520, pi_types: [0] }] } })
  assert.deepEqual(m.options(listed).map(m.optionKey), ['512:type1', '512:none'])
  // A 512-native drive with no PI: 512 only.
  assert.deepEqual(m.options(drive('sdf', { planned: null, offersPi: false, supports: { pi_types: [], block_lengths: [], physical_block_size: 512 } })).map(m.optionKey), ['512:none'])

  // Three drives at 4096 + PI 1, one request per drive.
  const req = m.requestFor([sdb, sdc, sdd], {}, 'format 3 drives')
  assert.deepEqual(req.drives.map((d) => [d.id, d.block_size, d.protection]), [['id-sdb', 4096, 'type1'], ['id-sdc', 4096, 'type1'], ['id-sdd', 4096, 'type1']])
  assert.equal(m.requestFor([sdb], { 'forge1/id-sdb': '512:none' }, 'SN-sdb').drives[0].block_size, 512)

  // Progress, per drive.
  assert.equal(m.stepOf({ state: 'running', phase: 'queued' }), 'probe')
  assert.equal(m.stepOf({ state: 'running', phase: 'formatting' }), 'format')
  assert.equal(m.stepOf({ state: 'running', phase: 'verify' }), 'read-back')
  assert.equal(m.stepOf({ state: 'done' }), 'done')
  assert.equal(m.stepOf({ state: 'interrupted' }), 'failed')
  const rows = m.progressRows([{ ...sdb, format: { state: 'running' } }, { ...sdc, format: { state: 'done', finishedUnix: 100 } }, { ...sdd, format: { state: 'done', finishedUnix: 10 } }], 200, 150)
  assert.deepEqual(rows.map((r) => r.name), ['sdb', 'sdc'])
}
console.log('driveformat #308 ok')
