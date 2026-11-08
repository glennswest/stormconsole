// node web/src/lib/controllers.test.mjs
import assert from 'node:assert/strict'
import { banners, byNode, counts, failedMetric, hbaWhy, isFailed, personalityPath, stateBody, statePath, stateText, stateWhy } from './controllers.js'

// forge1's PERC H730 as the drive plugin serves it (stormdrive v0.43.0): RAID mode, no perccli, sdb failed,
// a disk with no inventory record unconfigured bad.
const forge1 = {
  node: 'forge1', proxy: '/api/plugins/drive/proxy', key: '0000:03:00.0', model: 'PERC H730 Mini', firmware: '25.5.9.0001',
  mode: 'raid', mode_basis: 'inferred: disks in states only RAID mode has (failed ×3)', pending_mode: null, virtual_disks: 0,
  perccli: { ok: false, why: 'perccli is not on this node (stormcentral#1310)' },
  findings: [{ code: 'raid_mode', severity: 'warning', message: 'controller 0000:03:00.0 (PERC H730 Mini) is in RAID mode; HBA mode is mandatory' }],
  drives: [
    { device_id: 0, slot: 0, state: 'jbod', drive: 'a1', name: 'sda', component: 'drive:drive:a1' },
    { device_id: 1, slot: 1, state: 'failed', drive: 'b2', name: 'sdb', component: 'drive:drive:b2' },
    { device_id: 7, slot: 7, state: 'unconfigured_bad', drive: null, component: null },
    { device_id: 8, slot: 8, state: 'online', drive: 'c3', name: 'sde' },
  ],
}

// The words, never "missing".
assert.equal(stateText('failed'), 'Failed by the controller')
assert.ok(isFailed('failed') && isFailed('unconfigured_bad') && isFailed('offline') && !isFailed('jbod'))
assert.ok(failedMetric('unconfigured bad') && failedMetric('failed') && !failedMetric('jbod') && !failedMetric(undefined))

// The banner says HBA is required; once asked for, a reboot.
const b = banners([forge1])
assert.equal(b.length, 1)
assert.equal(b[0].tone, 'warn')
assert.match(b[0].text, /^HBA mode is required/)
const asked = { ...forge1, pending_mode: 'hba', findings: [{ code: 'reboot_pending', severity: 'info', message: '…' }] }
assert.match(banners([asked])[0].text, /a reboot of forge1 applies it/)
assert.equal(banners([{ ...forge1, mode: 'hba', findings: [] }]).length, 0)

// Switch to HBA: off with perccli's own reason (501 tool_missing), and for virtual disks, HBA already, a pending one.
assert.match(hbaWhy(forge1), /perccli is not available on forge1: perccli is not on this node \(stormcentral#1310\)/)
const ready = { ...forge1, perccli: { ok: true, path: '/usr/sbin/perccli64' } }
assert.equal(hbaWhy(ready), '')
assert.match(hbaWhy({ ...ready, virtual_disks: 2 }), /2 virtual disks/)
assert.equal(hbaWhy({ ...ready, mode: 'hba' }), 'already in HBA mode')
assert.match(hbaWhy(asked), /a reboot of forge1/)
assert.equal(hbaWhy(ready, { allowed: false, reason: 'not a storage-admin' }), 'not a storage-admin')

// Set good: the failed drive when it is out of the fleet and free; never a configured disk, the system drive or an in-use one.
const free = { outOfFleet: true, use: { in_use: false } }
const sdb = forge1.drives[1]
assert.equal(stateWhy(sdb, 'good', free), '')
assert.equal(stateWhy(sdb, 'jbod', free), '')
assert.equal(stateWhy(forge1.drives[2], 'good', null), '', 'no inventory record: the controller route, stormdrive decides')
assert.match(stateWhy(forge1.drives[3], 'good', free), /a member or spare/)
assert.equal(stateWhy(forge1.drives[0], 'jbod', free), 'already JBOD')
assert.equal(stateWhy(sdb, 'good', { outOfFleet: true, use: { in_use: true, use: 'system' } }), 'the system drive')
assert.match(stateWhy(sdb, 'good', { outOfFleet: true, use: { in_use: true, reason: 'mounted at /data' } }), /in use: mounted at \/data/)
assert.equal(stateWhy(sdb, 'good', { outOfFleet: false, use: { in_use: false } }), 'in the fleet: leave it first')

// Paths: by the drive when it has one (confirmed by its serial), else the controller's; force is sent.
assert.equal(statePath(forge1, sdb), '/api/plugins/drive/proxy/api/v1/drives/b2/controller-state')
assert.equal(statePath({ ...forge1, proxy: '/api/plugins/drive/node/storm-b/proxy' }, forge1.drives[2]), '/api/plugins/drive/node/storm-b/proxy/api/v1/controllers/0000:03:00.0/drives/7/state')
assert.equal(personalityPath(forge1), '/api/plugins/drive/proxy/api/v1/controllers/0000:03:00.0/personality')
assert.deepEqual(stateBody('good'), { state: 'good', force: true })

// By node, RAID first; totals.
const other = { ...forge1, key: '0000:01:00.0', mode: 'hba', findings: [], drives: [] }
const g = byNode([other, forge1, { ...other, node: 'storm-b' }])
assert.deepEqual(g.map((x) => x.node), ['forge1', 'storm-b'])
assert.equal(g[0].controllers[0].key, '0000:03:00.0')
assert.deepEqual(counts([forge1, other]), { controllers: 2, raid: 1, pending: 0, disks: 4, failed: 2 })

console.log('PASS controllers.test.mjs')
