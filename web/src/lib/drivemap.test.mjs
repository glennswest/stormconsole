// The Drives page's model at rack scale (#32): 10 nodes × 160 drives, run
// with plain node — `node web/src/lib/drivemap.test.mjs` — no browser, no
// bundler. It checks the joins and the grouping and measures them.
import assert from 'node:assert/strict'
import { build, groups, totals, heat, cells, columns, parseBytes, formatBytes, passes } from './drivemap.js'

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
console.log(`build ${(t1 - t0).toFixed(1)} ms · three groupings ${(t2 - t1).toFixed(1)} ms · totals ${(t3 - t2).toFixed(1)} ms`)
console.log('PASS')
