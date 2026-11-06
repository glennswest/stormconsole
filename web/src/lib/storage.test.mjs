// node web/src/lib/storage.test.mjs
import assert from 'node:assert'
import { volumeRows, slabRows, nodeRows, driveRows, totals, tone, okOf, matches } from './storage.js'

const vol = (name, status = {}) => ({ metadata: { name: `u-${name}` }, spec: { name, sizeBytes: 10, redundancy: 'raid1', role: 'data' }, status: { health: 'healthy', legsExpected: 2, legsMissing: 0, ...status } })
const vs = volumeRows([vol('b'), vol('a'), vol('c', { legsMissing: 1 }), vol('d', { failedSlabs: ['s'] })])
assert.deepStrictEqual(vs.map((v) => v.name), ['c', 'd', 'a', 'b'], 'trouble first')
assert.ok(vs[0].trouble && vs[1].trouble && !vs[2].trouble)

const eng = (serial, slabs) => ({ metadata: { name: `e-${serial}` }, spec: { path: `/dev/${serial}` }, status: { serial, model: 'm', capacityBytes: 5, slabs, smart: { healthy: true, temperatureC: 30 } } })
const phy = (node, serial) => ({ node, name: 'sda', path: '/dev/sda', serial, capacityBytes: 5, enclosure: 'shelf0', bay: 1, health: 'healthy' })
let ds = driveRows([eng('Z1', ['s1', 's2']), eng('ONLY', [])], [phy('n1', 'Z1'), phy('n2', 'Z2')])
assert.strictEqual(ds.length, 3)
const z1 = ds.find((d) => d.serial === 'Z1')
assert.deepStrictEqual([z1.seenBy, z1.slabs, z1.bay], ['both', 2, 1], 'one drive, both halves')
assert.strictEqual(ds.find((d) => d.serial === 'Z2').slabs, null, 'another node: its slabs are not this engine\'s to say')
assert.strictEqual(ds.find((d) => d.serial === 'ONLY').seenBy, 'engine')
// A file-backed drive's serial is "file": not joined with another of the same word.
ds = driveRows([eng('file', ['a']), eng('file', ['b'])], [phy('n1', 'file'), phy('n1', 'file')])
assert.strictEqual(ds.filter((d) => d.seenBy === 'both').length, 0)
assert.strictEqual(ds.length, 4)
ds = driveRows([eng('file', ['a']), eng('file', ['b'])], [phy('n1', 'file')])
assert.strictEqual(ds.filter((d) => d.seenBy === 'both').length, 0, 'two engine drives are not one physical one')

const ss = slabRows([{ metadata: { name: 'a' }, spec: { drive: { path: '/a' } }, status: { capacityBytes: 8, freeBytes: 2 } }, { metadata: { name: 'b' }, spec: { drive: { path: '/b' } }, status: { quarantined: true, holdsMetadata: true } }])
assert.deepStrictEqual(ss.map((s) => [s.id, s.quarantined, s.metadata]), [['b', true, true], ['a', false, false]])

const ns = nodeRows([{ metadata: { name: 'z' }, spec: {}, status: { local: true, totalBytes: 9, freeBytes: 3 } }, { metadata: { name: 'a' }, spec: { topology: { rack: 'r1' }, topologyChain: ['rack'] }, status: { totalBytes: 1, freeBytes: 1 } }])
assert.deepStrictEqual(ns.map((n) => n.name), ['z', 'a'], 'this node first')
assert.strictEqual(ns[1].topology, 'rack=r1')

const t = totals({ volumes: vs, slabs: ss, nodes: ns })
assert.deepStrictEqual([t.volumes, t.trouble, t.quarantined, t.capacityBytes, t.freeBytes], [4, 2, 1, 10, 4])
assert.deepStrictEqual([tone('Degraded'), tone('FAILING'), tone('healthy'), tone(null)], ['warn', 'bad', '', ''])
assert.deepStrictEqual([okOf({ ok: 1, total: 2 }), okOf(null), okOf({ ok: 0, total: 0 })], ['1 of 2', '—', '—'])
assert.ok(matches({ a: 'Hello' }, 'hell') && !matches({ a: 'x' }, 'y') && matches({}, ''))
console.log('PASS storage.test.mjs')
