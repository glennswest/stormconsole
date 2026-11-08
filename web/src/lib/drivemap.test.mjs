// The Drives page's model at rack scale (#32): 10 nodes × 160 drives, run
// with plain node — `node web/src/lib/drivemap.test.mjs` — no browser, no
// bundler. It checks the joins and the grouping and measures them.
import assert from 'node:assert/strict'
import { build, groups, setName, totals, heat, cells, columns, parseBytes, formatBytes, passes, pools, noUsage, enclosures, unreadShelves, unreadShelfKeys } from './drivemap.js'

function drive(host, i, extra = {}) {
  const local = host === null
  const prefix = local ? 'drive' : `drive:@${host}`
  const shelf = Math.floor(i / 40)
  const bay = i % 40
  const metrics = [
    { label: 'bay', value: String(bay) },
    { label: 'serial', value: `${host || 'here'}-SN${i}` },
    { label: 'dev', value: `/dev/sd${i}` },
    { label: 'capacity', value: '7.3 TB' },
    { label: 'temp', value: String(30 + (i % 30)) },
    { label: 'wear', value: String(i % 100) },
    { label: 'node', value: host || 'here' },
  ]
  return {
    id: `${prefix}:drive:${i}`,
    kind: 'drive',
    label: `sd${i} · HGST`,
    health: extra.health || 'ok',
    detail: extra.detail || 'sas hdd · 7.3 TB · fleet',
    metrics,
    actions: [],
    relations: [{ kind: 'belongs_to', name: 'shelf', targets: [`${prefix}:shelf:${shelf}`] }],
  }
}
function shelf(host, n) {
  const prefix = host === null ? 'drive' : `drive:@${host}`
  return { id: `${prefix}:shelf:${n}`, kind: 'shelf', label: `DS4246 #${n}`, health: 'ok', detail: '', metrics: [{ label: 'node', value: host || 'here' }], relations: [] }
}

const comps = []
const hosts = [null, ...Array.from({ length: 9 }, (_, k) => `storm-${k + 1}`)]
for (const h of hosts) {
  for (let s = 0; s < 4; s++) comps.push(shelf(h, s))
  for (let i = 0; i < 160; i++) {
    const extra = {}
    if (h === 'storm-3' && i === 7) extra.health = 'error'
    if (h === 'storm-4' && i === 11) extra.health = 'warn'
    if (h === 'storm-5' && i === 2) extra.detail = 'sas hdd · 7.3 TB · out of fleet'
    comps.push(drive(h, i, extra))
  }
}
// This node's engine: usage for two drives, a rebuilding member.
comps.push({ id: 'sb:use:here-SN0', kind: 'drive-use', label: 'here-SN0', metrics: [
  { label: 'serial', value: 'here-SN0' }, { label: 'slab bytes', value: String(parseBytes('7.3 TB')) }, { label: 'free bytes', value: '0' }] })
comps.push({ id: 'sb:use:here-SN1', kind: 'drive-use', label: 'here-SN1', metrics: [
  { label: 'serial', value: 'here-SN1' }, { label: 'slab bytes', value: String(2 * 1024 ** 4) }, { label: 'free bytes', value: String(1024 ** 4) }] })
// A drive on another node with the same serial as a local slab must not
// borrow it: the engine is this node's.
comps.push({ id: 'sb:member:/dev/sd5', kind: 'array-member', label: '/dev/sd5', metrics: [{ label: 'path', value: '/dev/sd5' }, { label: 'state', value: 'rebuilding' }] })
// Racks from kube node labels.
for (const h of hosts) comps.push({ id: `k8s:node:${h || 'here'}`, kind: 'k8s-node', label: h || 'here', metrics: [{ label: 'rack', value: ['here', 'storm-1', 'storm-2', 'storm-3', 'storm-4'].includes(h || 'here') ? 'A' : 'B' }] })

// #166: a node's backplane behind a RAID controller is its internal shelf.
{
  const d = (id, node, hba, extra = {}) => ({ id, kind: 'drive', label: id, metrics: [{ label: 'node', value: node }, ...(hba ? [{ label: 'hba', value: hba }] : []), { label: 'capacity', value: '1 TB' }, ...(extra.metrics || [])], relations: extra.relations || [] })
  const forge = build([d('drive:drive:1', 'forge1', 'PERC H730'), d('drive:drive:2', 'forge1', 'PERC H730')]).records
  const g = groups(forge, { by: 'chassis' })
  assert.equal(g.length, 1, 'one shelf')
  assert.equal(g[0].label, 'forge1 · internal shelf behind PERC H730, not read yet')
  assert.ok(!/no (shelf|enclosure)/.test(g[0].label))
  assert.equal(enclosures(forge), 1)
  assert.deepEqual(unreadShelves(forge).map((u) => [u.node, u.drives, u.words]), [['forge1', 2, 'internal shelf behind PERC H730, not read yet']])
  assert.deepEqual(unreadShelfKeys([d('a', 'forge1', 'PERC H730'), d('b', 'forge1', 'PERC H730')]), ['unread:forge1/PERC H730'])

  // Read through the controller (stormdrive#65): the same box, with a name.
  const inShelf = (id, s, pos) => d(id, 'n', 'HBA', { relations: [{ name: 'shelf', targets: [s] }], metrics: [{ label: 'position', value: pos }] })
  const read = build([
    { id: 'shelf:a', kind: 'shelf', label: 'DELL BP14G', metrics: [{ label: 'position', value: 'internal' }] },
    { id: 'shelf:b', kind: 'shelf', label: 'DELL rear', metrics: [{ label: 'position', value: 'rear' }] },
    { id: 'shelf:c', kind: 'shelf', label: 'DS4246', metrics: [{ label: 'position', value: 'external' }] },
    inShelf('drive:drive:a', 'shelf:a', 'internal'), inShelf('drive:drive:b', 'shelf:b', 'rear'), inShelf('drive:drive:c', 'shelf:c', 'external'),
  ]).records
  assert.deepEqual(groups(read, { by: 'chassis' }).map((x) => x.label).sort(), ['DELL BP14G · internal shelf · n', 'DELL rear · rear shelf · n', 'DS4246 · external shelf · n'])
  assert.equal(enclosures(read), 2, 'the node with two shelves in it, and the JBOD')
  assert.equal(unreadShelves(read).length, 0)

  // Two controllers are two shelves, in one box.
  const two = build([d('drive:drive:1', 'n', 'A'), d('drive:drive:2', 'n', 'B')]).records
  assert.equal(groups(two, { by: 'chassis' }).length, 2)
  assert.equal(enclosures(two), 1)
}

const t0 = performance.now()
const { records } = build(comps)
const t1 = performance.now()
const byChassis = groups(records, { by: 'chassis' })
const byNode = groups(records, { by: 'node' })
const byRack = groups(records, { by: 'rack' })
const t2 = performance.now()
const all = totals(records)
const t3 = performance.now()

assert.equal(records.length, 1600)
assert.equal(all.nodes, 10)
assert.equal(byChassis.length, 40, 'four shelves on each of ten nodes, not four shelves')
assert.equal(byNode.length, 10)
assert.deepEqual(byRack.map((g) => [g.label, g.drives.length]), [['rack A', 800], ['rack B', 800]])
assert.equal(byChassis[0].drives.length, 40)
assert.deepEqual(byChassis[0].drives.slice(0, 3).map((d) => d.bay), [0, 1, 2], 'ordered by bay')
assert.equal(columns(byChassis[0].drives), 12)
assert.equal(cells(byChassis[0].drives).length, 40)

// Joins: usage and rebuild state only for this node's drives.
const here0 = records.find((d) => d.id === 'drive:drive:0')
const here1 = records.find((d) => d.id === 'drive:drive:1')
assert.ok(here0.usage && here0.usage.frac > 0.99, 'a full drive')
assert.ok(Math.abs(here1.usage.frac - 1 / 7.3) < 0.01, 'one TB used of 7.3')
assert.equal(records.find((d) => d.id === 'drive:drive:5').member, 'rebuilding')
assert.equal(records.find((d) => d.id === 'drive:@storm-1:drive:5').member, null, "another node's /dev/sd5 is not this engine's")
assert.equal(heat(records.find((d) => d.id === 'drive:@storm-1:drive:0'), 'usage').css, null, 'no data is drawn as no data')

// #29: stormdrive's usage in bytes for every node, and this node's volumes
// by drive. storm-2 answers for all 160 drives; storm-6 is silent.
const TB = 1024 ** 4
const usage = { drives: [], nodes: [{ node: 'storm-2', ok: true }, { node: 'storm-6', ok: false, error: 'refused' }] }
for (let i = 0; i < 160; i++) {
  const hot = i === 3
  usage.drives.push({
    component: `drive:@storm-2:drive:${i}`, node: 'storm-2', serial: `storm-2-SN${i}`, capacity: parseBytes('7.3 TB'),
    overcommit: { enabled: i < 80, ratio: 2 },
    ...(i === 9 ? { drain: { state: 'running', moved: 4, failed: 0, remaining: 6, reason: 'operator', then_leave: true } } : {}),
    usage: {
      capacity: parseBytes('7.3 TB'), in_slabs: 7 * TB, used: hot ? 7 * TB : 1 * TB, free_in_slabs: hot ? 0 : 6 * TB,
      outside_slabs: parseBytes('7.3 TB') - 7 * TB, free: parseBytes('7.3 TB') - (hot ? 7 : 1) * TB, promisable: 14 * TB,
      committed: 3 * TB, headroom: 11 * TB,
      slabs: [{ id: `s${i}`, role: 'data', tier: i < 80 ? 'hot' : 'warm', total: 7 * TB, used: hot ? 7 * TB : TB, free: hot ? 0 : 6 * TB, committed: 3 * TB }],
    },
  })
}
// A drive stormdrive knows but has no usage for yet.
usage.drives[159] = { component: 'drive:@storm-2:drive:159', node: 'storm-2', serial: 'storm-2-SN159', capacity: 1 }
const placement = { volumes: 2, placed: 2, drives: { 'storm-2-SN4': [
  { component: 'sb:volume:v1', id: 'v1', name: 'db', kind: 'volume', consumer: 'PersistentVolumeClaim shop/db', consumer_link: 'k8s:pvc:shop/db', bytes: 4096, legs: 2, shared_legs: 0, state: 'ok', slabs: [] }] } }
const t4 = performance.now()
const joined = build(comps, { usage, placement }).records
const t5 = performance.now()
const s2 = (i) => joined.find((d) => d.id === `drive:@storm-2:drive:${i}`)
assert.equal(s2(0).usage.source, 'stormdrive', "another node's usage, from its own stormdrive")
assert.ok(Math.abs(s2(0).usage.frac - 1 / 7.3) < 0.01)
assert.equal(s2(3).usage.frac > 0.9, true)
assert.equal(s2(9).drain.remaining, 6)
assert.equal(s2(4).volumes[0].consumer_link, 'k8s:pvc:shop/db', 'the volumes on a drive, joined by serial')
assert.equal(s2(159).usage, null)
assert.match(noUsage(s2(159)), /no usage for it yet/)
assert.match(noUsage(joined.find((d) => d.id === 'drive:@storm-6:drive:0')), /did not answer/)
assert.equal(joined.find((d) => d.id === 'drive:drive:0').usage.source, 'engine', 'an older stormdrive: the engine still answers')
const jc = (f) => joined.filter((d) => passes(d, f)).length
assert.equal(jc('draining'), 1)
assert.equal(jc('full'), 2, "this node's full drive and storm-2's")
const jt = totals(joined)
assert.equal(jt.withUsage, 159 + 2)
const ps = pools(joined)
const hot = ps.find((p) => p.node === 'storm-2' && p.tier === 'hot')
const warm = ps.find((p) => p.node === 'storm-2' && p.tier === 'warm')
assert.equal(hot.drives, 80)
assert.equal(hot.promisable, 80 * 14 * TB, 'overcommit 2× doubles what the hot slabs may promise')
assert.equal(warm.promisable, 79 * 7 * TB, 'warm drives are not overcommitted')
assert.equal(hot.committed, 80 * 3 * TB)
assert.equal(hot.headroom, 80 * 11 * TB)
assert.equal(ps.filter((p) => p.node === 'here').length, 0, 'the engine fallback carries no slab list, so no pool is guessed')

// Filters.
const count = (f) => records.filter((d) => passes(d, f)).length
assert.equal(count('failing'), 1)
assert.equal(count('degraded'), 1)
assert.equal(count('rebuilding'), 1)
assert.equal(count('full'), 1)
assert.equal(count('out'), 1)
assert.equal(groups(records, { filter: 'failing' }).length, 1, 'filters narrow the map to the chassis that matter')
assert.equal(groups(records, { search: 'storm-7' }).reduce((n, g) => n + g.drives.length, 0), 160)

// Totals in rack units.
assert.equal(formatBytes(all.capacity), '11.4 PB')
assert.equal(formatBytes(1024 ** 6 * 2), '2.0 EB')
assert.equal(heat(here0, 'temp').text, '30 °C')

// #43: RAID sets named in the room, `<rack>.<shelf>.<set>`. A DS2246 on this
// node as stormblock#252 lays it out: two RAID-6 sets of 11 and two spares,
// the members naming their drive by serial (the device path is stale on
// purpose, so the join has to be by serial).
const raid = []
for (let s = 0; s < 1; s++) raid.push(shelf(null, s))
for (let i = 0; i < 24; i++) raid.push(drive(null, i))
for (let i = 0; i < 24; i++) {
  const set = i < 11 ? 'ds1-a' : i < 22 ? 'ds1-b' : null
  const metrics = [{ label: 'path', value: `/dev/old${i}` }, { label: 'state', value: set ? 'active' : 'spare' }, { label: 'serial', value: `here-SN${i}` }]
  if (set) metrics.push({ label: 'set', value: set }, { label: 'shelf', value: 'ds1' })
  if (set) raid.push({ id: `sb:member:/dev/old${i}`, kind: 'array-member', label: `/dev/old${i}`, metrics })
}
raid.push({ id: 'k8s:node:here', kind: 'k8s-node', label: 'here', metrics: [{ label: 'rack', value: 'rack3' }] })
const rr = build(raid).records
assert.equal(setName(rr[0]), 'rack3.ds1.ds1-a')
assert.equal(setName(rr[11]), 'rack3.ds1.ds1-b')
assert.equal(setName(rr[23]), null, 'a spare is in no set')
const bySet = groups(rr, { by: 'set' })
assert.deepEqual(bySet.map((g) => [g.label, g.drives.length]),
  [['here · in no RAID set', 2], ['set rack3.ds1.ds1-a', 11], ['set rack3.ds1.ds1-b', 11]])
// No rack label: the name says so rather than borrowing another's.
const unracked = build(raid.filter((c) => c.kind !== 'k8s-node')).records
assert.equal(setName(unracked[0]), 'no-rack.ds1.ds1-a')

console.log(`1,600 drives on ${all.nodes} nodes, ${formatBytes(all.capacity)} raw: ` +
  `${byChassis.length} chassis, ${byRack.length} racks, ${all.failing} failing, ${all.degraded} degraded, ${all.rebuilding} rebuilding, ${all.full} full`)
// #52: each node's stormdrive reports the volumes on its drives.
{
  const c = (id, serial, node) => ({ id, kind: 'drive', label: serial, metrics: [{ label: 'serial', value: serial }, ...(node ? [{ label: 'node', value: node }] : [])], relations: [] })
  const vol = (id, extra = {}) => ({ id, name: id, kind: 'volume', bytes: 1, legs: 1, shared_legs: 0, state: 'ok', rebuild: 'none', consumer: null, consumer_link: null, component: null, ...extra })
  const u = (id, node, volumes) => ({ component: id, node, serial: id, capacity: 100, usage: { capacity: 100, in_slabs: 0, used: 0, free_in_slabs: 0, outside_slabs: 0, free: 100, promisable: 0, slabs: [], ...(volumes ? { volumes, volumes_at: 5 } : {}) } })
  const comps = [c('drive:drive:0', 'HERE'), c('drive:@n2:drive:1', 'FAR', 'n2'), c('drive:@n2:drive:2', 'EMPTY', 'n2'), c('drive:@n3:drive:3', 'OLD', 'n3'), c('drive:drive:4', 'FALLBACK')]
  const usageAnswer = { drives: [u('drive:drive:0', 'here', [vol('v1', { component: 'sb:volume:v1' })]), u('drive:@n2:drive:1', 'n2', [vol('far', { consumer: 'Pod a/b', consumer_link: 'k8s:pod:a/b' })]), u('drive:@n2:drive:2', 'n2', []), u('drive:@n3:drive:3', 'n3', null), u('drive:drive:4', 'here', null)], nodes: [] }
  const placed = { volumes: 2, placed: 2, drives: { HERE: [{ id: 'v1', name: 'old-word', slabs: [{ id: 's1', state: 'draining' }] }], FALLBACK: [{ id: 'p1', name: 'from-the-engine', slabs: [] }] } }
  const rec = (id, placement = placed) => build(comps, { usage: usageAnswer, placement }).records.find((x) => x.id === id)
  assert.equal(rec('drive:drive:0').volumes[0].name, 'v1', 'stormdrive first, on this node too')
  assert.deepEqual(rec('drive:drive:0').volumes[0].slabs, [{ id: 's1', state: 'draining' }], 'a slab state still from the engine')
  assert.deepEqual(rec('drive:@n2:drive:1').volumes.map((v) => [v.name, v.consumer_link]), [['far', 'k8s:pod:a/b']], 'another node')
  assert.deepEqual(rec('drive:@n2:drive:1').volumes[0].slabs, [], 'no slabs to walk, not undefined: the slab table flatMaps them')
  assert.deepEqual([rec('drive:@n2:drive:2').volumesReported, rec('drive:@n2:drive:2').volumes], [true, []], 'nothing here is an answer')
  assert.deepEqual([rec('drive:@n3:drive:3').volumesReported, rec('drive:@n3:drive:3').volumes], [false, null], 'not reported is not')
  assert.equal(rec('drive:drive:4').volumes[0].name, 'from-the-engine', 'the engine as the fallback')
  assert.deepEqual([rec('drive:drive:0', null).volumes[0].name, rec('drive:drive:0', null).volumes[0].slabs], ['v1', []], 'no placement answer at all')
}
console.log(`with usage and placement: build ${(t5 - t4).toFixed(1)} ms, ${ps.length} pools`)
console.log(`build ${(t1 - t0).toFixed(1)} ms · three groupings ${(t2 - t1).toFixed(1)} ms · totals ${(t3 - t2).toFixed(1)} ms`)
// The engine's row for a drive is not a second drive, and its offer joins on.
{
  const m = (label, value) => ({ label, value })
  const sd = { id: 'drive:a', kind: 'drive', label: 'sdq', detail: '', metrics: [m('serial', 'ZFL1'), m('dev', '/dev/sdq')], relations: [] }
  const remote = { id: 'drive:@b:x', kind: 'drive', label: 'sdq', detail: '', metrics: [m('serial', 'ZFL1'), m('node', 'b')], relations: [] }
  const en = { id: 'sb:drive:u1', kind: 'drive', label: '/dev/sdq', metrics: [m('serial', 'ZFL1'), m('path', '/dev/sdq'), m('offer', 'foreign'), m('holds', 'an XFS filesystem'), m('confirm', 'ZFL1')], actions: [] }
  const { records } = build([sd, remote, en])
  assert.deepEqual(records.map((r) => r.id), ['drive:a', 'drive:@b:x'])
  assert.equal(records[0].offer.sentence, 'holds an XFS filesystem')
  assert.equal(records[1].offer, null, "this node's engine does not answer for another node's drive")
  assert.ok(passes(records[0], 'offered'))
  assert.ok(!passes(records[1], 'offered'))
}

// #308: forge1's sda is in use (System), sdb free; the filters say so.
{
  const d = (id) => ({ id, kind: 'drive', label: id, metrics: [{ label: 'node', value: 'forge1' }, { label: 'serial', value: id }] })
  const usage = { drives: [
    { component: 'drive:drive:sda', node: 'forge1', serial: 'sda', use: { in_use: true, use: 'system', badge: 'System', reason: "sda holds this node's system slab" } },
    { component: 'drive:drive:sdb', node: 'forge1', serial: 'sdb', use: { in_use: false, use: 'free', badge: null } },
  ] }
  const [a, b] = build([d('drive:drive:sda'), d('drive:drive:sdb')], { usage }).records
  assert.equal(a.use.badge, 'System')
  assert.ok(passes(a, 'inuse') && !passes(a, 'free'))
  assert.ok(passes(b, 'free') && !passes(b, 'inuse'))
}
// #325: a drive the controller has failed reads so, is failing and is drawn as an error, never "missing".
{
  const d = (id, controller) => ({ id, kind: 'drive', label: id, health: 'unknown', metrics: [{ label: 'node', value: 'forge1' }, ...(controller ? [{ label: 'controller', value: controller }] : [])] })
  const [ok, bad, ub] = build([d('drive:drive:sda', 'jbod'), d('drive:drive:sdb', 'failed'), d('drive:drive:sdc', 'unconfigured bad')]).records
  assert.equal(ok.controllerFailed, false)
  assert.ok(bad.controllerFailed && ub.controllerFailed)
  assert.ok(passes(bad, 'failing') && !passes(ok, 'failing'))
  assert.equal(heat(bad, 'health').text, 'Failed by the controller')
  assert.equal(heat(bad, 'health').css, 'var(--error)')
  assert.equal(totals([ok, bad, ub]).failing, 2)
}
console.log('PASS')
