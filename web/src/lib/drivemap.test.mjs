// The Drives page's model at rack scale (#32): 10 nodes × 160 drives, run
// with plain node — `node web/src/lib/drivemap.test.mjs` — no browser, no
// bundler. It checks the joins and the grouping and measures them.
import assert from 'node:assert/strict'
import { build, groups, totals, heat, cells, columns, parseBytes, formatBytes, passes, pools, noUsage } from './drivemap.js'

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

console.log(`1,600 drives on ${all.nodes} nodes, ${formatBytes(all.capacity)} raw: ` +
  `${byChassis.length} chassis, ${byRack.length} racks, ${all.failing} failing, ${all.degraded} degraded, ${all.rebuilding} rebuilding, ${all.full} full`)
console.log(`with usage and placement: build ${(t5 - t4).toFixed(1)} ms, ${ps.length} pools`)
console.log(`build ${(t1 - t0).toFixed(1)} ms · three groupings ${(t2 - t1).toFixed(1)} ms · totals ${(t3 - t2).toFixed(1)} ms`)
console.log('PASS')
