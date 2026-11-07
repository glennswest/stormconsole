// node web/src/lib/drivereplace.test.mjs
import assert from 'node:assert/strict'
import { candidates, kindOf, plan, proxyOf, replaceWhy, rows, stormdriveId, asked } from './drivereplace.js'

// forge1's layout as drivemap builds it: sda the system drive, sdb–sdd free, plus a fleet drive,
// a RAID member and an LVM disk, and a free drive on another node.
const act = (id, enabled = true, why = '') => ({ id, label: id, method: 'POST', path: `/api/plugins/drive/proxy/api/v1/drives/x/${id}`, enabled, why })
const drive = (name, serial, bay, extra = {}) => ({
  id: `drive:drive:${name}id`, node: 'forge1', local: true, shelfId: 'drive:shelf:0', bay, serial, dev: `/dev/${name}`,
  capacity: 10e12, outOfFleet: true, c: { label: `${name} · M`, detail: 'out of fleet · idle', actions: [act('locate-on'), act('fleet-join')] },
  use: { in_use: false, use: 'free' }, ...extra,
})
const sda = drive('sda', 'SYS001', 0, { use: { in_use: true, use: 'system', badge: 'System', reason: "sda holds this node's system slab" } })
const sdb = drive('sdb', 'NET002', 3)
const sdc = drive('sdc', 'NET003', 4)
const sdd = drive('sdd', 'NET004', 7, { capacity: 4e12 })
const sde = drive('sde', 'FLT005', 5, { outOfFleet: false, use: { in_use: true, use: 'slab_member', badge: 'Slab member' },
  c: { label: 'sde · M', detail: 'in fleet · idle', actions: [act('locate-on'), act('fleet-leave')] }, usage: { used: 2e12 } })
const sdf = drive('sdf', 'LVM006', 6, { use: { in_use: true, use: 'lvm', badge: 'In use', reason: 'sdf holds LVM2_member' } })
const sdk = drive('sdk', 'RD0011', 11, { set: 'ds0-a', use: { in_use: true, use: 'raid_member', badge: 'RAID member' } })
const sdl = drive('sdl', 'NEW012', 12, { shelfId: 'drive:shelf:1' })
const far = drive('sdx', 'BEE001', 1, { node: 'storm-b', local: false, id: 'drive:@storm-b:drive:b1' })
const records = [sda, sdb, sdc, sdd, sde, sdf, sdk, sdl, far]

assert.deepEqual(records.map(kindOf), ['system', 'free', 'free', 'free', 'slab', 'other', 'raid', 'free', 'free'])
assert.equal(stormdriveId(far), 'b1')
assert.equal(stormdriveId(sdb), 'sdbid')
assert.equal(proxyOf(sdb), '/api/plugins/drive/proxy')
assert.equal(proxyOf({ node: 'storm-b', local: false, c: { actions: [] } }), '/api/plugins/drive/node/storm-b/proxy')

// The system drive: refused, with the issue that would let it.
assert.match(replaceWhy(sda), /system drive.*stormblock#563.*Nothing is changed/)
assert.equal(plan(sda, sdb).ok, false)
// Held outside the engine: said in its own words.
assert.match(replaceWhy(sdf), /^sdf holds LVM2_member: .*nothing here can move it/)

// A free drive: nothing to move, recorded.
const f = plan(sdb, sdc)
assert.equal(f.ok, true)
assert.deepEqual(f.call, { method: 'POST', path: '/api/plugins/drive/replace', body: { node: 'forge1', old: 'sdbid', new: 'sdcid' } })
assert.match(f.steps.join(' '), /stormdrive#127/)
assert.match(plan(sdb, sdb).why, /itself/)
assert.match(plan(sdb, far).why, /same node/)
assert.match(plan(sdb, sde).why, /in use \(slab member\)/)
const fmting = new Map([[sdc.id, { format: { state: 'running' } }]])
assert.match(plan(sdb, sdc, { formats: fmting }).why, /being formatted/)

// Candidates: same node, free, same shelf first, by bay; the small one said.
const cs = candidates(sde, records)
assert.deepEqual(cs.map((x) => x.d.dev), ['/dev/sdb', '/dev/sdc', '/dev/sdd', '/dev/sdl'])
assert.match(cs[2].p.why, /smaller/)

// A slab member: join the new drive, then drain the old one out of the fleet.
const s = plan(sde, sdb)
assert.equal(s.ok, true)
assert.deepEqual(s.join, { method: 'POST', path: '/api/plugins/drive/proxy/api/v1/drives/x/fleet-join' })
assert.equal(s.call.path, '/api/plugins/drive/proxy/api/v1/drives/sdeid/drain?leave=true')
assert.match(s.steps.join(' '), /stormblock#563/)
const noJoin = { ...sdc, c: { ...sdc.c, actions: [act('fleet-join', false, 'designated reserved')] } }
assert.match(plan(sde, noJoin).why, /cannot join the fleet: designated reserved/)
assert.match(plan({ ...sde, drain: { state: 'running' } }, sdb).why, /being drained already/)

// A RAID member: the engine's live replace, on a blank drive it has open, at the set's format.
const member = { slot: 2, state: 'active', serial: 'RD0011', path: '/dev/sdk', component: 'sb:member:/dev/sdk' }
const raid = {
  shelves: [{ sets: [{ name: 'ds0-a', status: { state: 'clean' }, members: [member] }] }],
  free: [{ uuid: 'u-sdc', serial: 'NET003', path: '/dev/sdc' }],
}
const components = [{ id: 'sb:member:/dev/sdk', actions: [{ id: 'replace', method: 'POST', path: '/api/plugins/sb/proxy/api/v1/arrays/a1/members/2/replace' }] }]
const formats = new Map([
  [sdk.id, { current: { blockSize: 4096, protection: 'type1', protType: 1 } }],
  [sdc.id, { current: { blockSize: 4096, protection: 'type1', protType: 1 } }],
  [sdb.id, { current: { blockSize: 512, protection: 'type1', protType: 1 } }],
])
const ctx = { raid, components, formats }
const r = plan(sdk, sdc, ctx)
assert.equal(r.ok, true, r.why)
assert.equal(r.live, true)
assert.deepEqual(r.call, { method: 'POST', path: '/api/plugins/sb/proxy/api/v1/arrays/a1/members/2/replace', body: { drive_uuid: 'u-sdc' }, confirm: true })
assert.match(r.steps[1], /keeps serving.*rebuild from parity.*never loses redundancy/)
assert.match(r.steps[2], /superblock is wiped and it is released/)
// Not open in the engine; at another format; the set busy; no replace offered (not a storage-admin).
assert.match(plan(sdk, sdb, ctx).why, /not a blank drive this node's engine has open/)
const sdbOpen = { ...ctx, raid: { ...raid, free: [{ uuid: 'u-sdb', serial: 'NET002' }] } }
const fm = plan(sdk, sdb, sdbOpen)
assert.match(fm.why, /^sdb is 512 \+ PI type 1, and the set's sdk is 4096 \+ PI type 1: format sdb to 4096 \+ PI type 1 first$/)
assert.deepEqual(fm.format, { blockSize: 4096, protection: 'type1' })
const busy = { ...ctx, raid: { ...raid, shelves: [{ sets: [{ name: 'ds0-a', status: { state: 'rebuilding', rebuild: { running: true, slots: [0] } }, members: [member] }] }] } }
assert.match(plan(sdk, sdc, busy).why, /rebuilding already/)
assert.match(plan(sdk, sdc, { ...ctx, components: [] }).why, /storage-admin/)
assert.match(plan({ ...sdk, local: false, node: 'forge1' }, sdc, ctx).why, /its own node's console/)
// A failed slot: a rebuild, said as one short already.
const failedSet = { ...ctx, raid: { ...raid, shelves: [{ sets: [{ name: 'ds0-a', status: { state: 'degraded' }, members: [{ ...member, state: 'failed' }] }] }] } }
const fr = plan(sdk, sdc, failedSet)
assert.equal(fr.live, false)
assert.match(fr.steps[0], /already one drive short/)

// Progress rows, read again from what holds each one.
const askR = asked(r, sdk, sdc, 1)
const copying = { shelves: [{ sets: [{ name: 'ds0-a', status: { state: 'clean', rebuild: { running: true, slots: [2], percent: 41.6 } }, members: [member] }] }] }
assert.deepEqual([rows([askR], { raid: copying })[0].state, rows([askR], { raid: copying })[0].percent], ['running', 42])
const swapped = { shelves: [{ sets: [{ name: 'ds0-a', status: { state: 'clean' }, members: [{ ...member, serial: 'NET003', path: '/dev/sdc' }] }] }] }
const done = rows([askR], { raid: swapped })[0]
assert.equal(done.state, 'done')
assert.equal(done.back, 'slot 2 holds sdc (NET003), active; set clean')
assert.equal(rows([askR], { raid: { shelves: [{ sets: [{ name: 'ds0-a', status: { state: 'clean' }, members: [member] }] }] } })[0].state, 'failed')

const askS = asked(s, sde, sdb, 1)
const draining = { ...sde, drain: { state: 'running', moved: 30, remaining: 70, failed: 0 } }
assert.deepEqual([rows([askS], { records: [draining] })[0].percent, rows([askS], { records: [draining] })[0].state], [30, 'running'])
const empty = { ...sde, outOfFleet: true, drain: { state: 'empty', moved: 100, remaining: 0, failed: 0 } }
assert.match(rows([askS], { records: [empty] })[0].back, /empty \(100 moved\) and out of the fleet: it can be pulled/)
assert.equal(rows([askS], { records: [{ ...sde, drain: { state: 'stuck', moved: 1, remaining: 2, failed: 1, errors: ['no room'] } }] })[0].back, 'no room')
assert.equal(rows([asked(f, sdb, sdc, 1)])[0].state, 'done')
console.log('PASS drivereplace.test.mjs')
