// node web/src/lib/crgrid.test.mjs
import assert from 'node:assert'
import {
  cellText, shownColumns, wideCount, matches, applyChange, toneOf, bar, fractionOf, curatedCell, toGrid,
  filterCrds, conditionTone, fieldRows, isCurated,
} from './crgrid.js'

const cols = [
  { key: 'name', name: 'Name', type: 'string', priority: 0 },
  { key: 'namespace', name: 'Namespace', type: 'string', priority: 0 },
  { key: 'c0', name: 'Node', type: 'string', priority: 0, printer: true },
  { key: 'c1', name: 'Phase', type: 'string', priority: 0, printer: true },
  { key: 'c2', name: 'Message', type: 'string', priority: 1, printer: true },
  { key: 'age', name: 'Age', type: 'date', priority: 0 },
]

// Cells print as the CLI prints them.
assert.strictEqual(cellText({ type: 'string' }, ''), '')
assert.strictEqual(cellText({ type: 'integer' }, 2), '2')
assert.strictEqual(cellText({ type: 'boolean' }, false), 'False')
assert.strictEqual(cellText({ type: 'string' }, { a: 1 }), '{"a":1}')
assert.strictEqual(cellText({ type: 'date' }, new Date(Date.now() - 3 * 3600e3).toISOString()), '3h', 'a date is an age')

// Wide columns only when asked; the project column only across projects.
assert.deepStrictEqual(shownColumns(cols).map((c) => c.name), ['Name', 'Namespace', 'Node', 'Phase', 'Age'])
assert.deepStrictEqual(shownColumns(cols, { wide: true }).map((c) => c.name), ['Name', 'Namespace', 'Node', 'Phase', 'Message', 'Age'])
assert.deepStrictEqual(shownColumns(cols, { project: 'team' }).map((c) => c.name), ['Name', 'Node', 'Phase', 'Age'])
assert.strictEqual(wideCount(cols), 1)

// Search reads what is printed.
const row = { id: 'md0', name: 'md0', namespace: '', c0: 'forge1', c1: 'Rebuilding', c2: '', age: '' }
assert.ok(matches(row, cols, 'FORGE'))
assert.ok(matches(row, cols, ''))
assert.ok(!matches(row, cols, 'nope'))

// The live stream: a snapshot replaces, a change upserts, a delete removes.
let rows = applyChange([], { type: 'snapshot', rows: [{ id: 'b' }, { id: 'c' }] })
rows = applyChange(rows, { type: 'ADDED', row: { id: 'a' } })
assert.deepStrictEqual(rows.map((r) => r.id), ['a', 'b', 'c'], 'kept in order')
rows = applyChange(rows, { type: 'MODIFIED', row: { id: 'b', c1: 'Ready' } })
assert.strictEqual(rows[1].c1, 'Ready')
rows = applyChange(rows, { type: 'DELETED', id: 'c' })
assert.deepStrictEqual(rows.map((r) => r.id), ['a', 'b'])
assert.strictEqual(applyChange(rows, { type: 'error', error: 'x' }), rows, 'an error leaves the rows as they were')

// Tones, bars, fractions.
assert.strictEqual(toneOf('Ready'), 'ok')
assert.strictEqual(toneOf('Rebuilding'), 'warn')
assert.strictEqual(toneOf('Failed'), 'error')
assert.strictEqual(toneOf('forge1'), '')
assert.strictEqual(bar(0.5, 4), '▰▰▱▱')
assert.strictEqual(bar(2, 4), '▰▰▰▰', 'clamped')
assert.strictEqual(fractionOf(42), 0.42)
assert.strictEqual(fractionOf('42%'), 0.42)
assert.strictEqual(fractionOf(0.5), 0.5)
assert.strictEqual(fractionOf('3/4'), 0.75)
assert.strictEqual(fractionOf('fast'), null)

// Our kinds' renderers (#348 §4).
const phase = { name: 'Phase', type: 'string', printer: true }
assert.deepStrictEqual(curatedCell('raidsets.storage.storm.io', phase, 'Rebuilding'), { value: '▰▰▰▱ Rebuilding', tone: 'warn' })
assert.deepStrictEqual(curatedCell('raidsets.storage.storm.io', phase, 'Ready'), { value: '▰▰▰▰ Ready', tone: 'ok' })
assert.deepStrictEqual(curatedCell('drives.storage.storm.io', { name: 'Health', printer: true }, 'OK'), { value: '● OK', tone: 'ok' })
assert.deepStrictEqual(curatedCell('drives.storage.storm.io', { name: 'Link', printer: true }, 'degraded'), { value: '● degraded', tone: 'warn' })
assert.deepStrictEqual(curatedCell('replications.storage.storm.io', { name: 'Progress', printer: true }, '50%'), { value: '▰▰▰▰▱▱▱▱ 50%', tone: 'accent' })
assert.strictEqual(curatedCell('drives.storage.storm.io', { name: 'Model', printer: true }, 'X'), null, 'a column with no renderer is text')
assert.strictEqual(curatedCell('drives.storage.storm.io', { name: 'Health', printer: false }, 'OK'), null, 'only printer columns')
assert.deepStrictEqual(curatedCell('things.example.io', { name: 'Status', printer: true, type: 'string' }, 'Failed'), { value: '● Failed', tone: 'error' }, 'any kind: a state column is coloured')
assert.strictEqual(curatedCell('things.example.io', { name: 'Status', printer: true, type: 'string' }, 'Weird'), null, 'but not a word that means nothing')
assert.ok(isCurated('drives.storage.storm.io') && !isCurated('things.example.io'))

// The grid: coloured columns use `metrics`, sort by their text; the rest are text.
const g = toGrid('raidsets.storage.storm.io', cols, [
  { id: 'md0', name: 'md0', namespace: '', c0: 'forge1', c1: 'Rebuilding', c2: '', age: '2026-11-18T10:00:00Z' },
  { id: 'md1', name: 'md1', namespace: '', c0: 'forge1', c1: '', c2: '', age: '2026-11-18T11:00:00Z' },
])
const byKey = Object.fromEntries(g.columns.map((c) => [c.key, c]))
assert.strictEqual(byKey.c1.render, 'metrics')
assert.strictEqual(byKey.name.render, 'mono')
assert.strictEqual(typeof byKey.c0.render, 'function')
assert.strictEqual(byKey.c0.render(g.rows[0]), 'forge1')
assert.strictEqual(String(g.rows[0].c1), 'Rebuilding', 'a coloured cell sorts by its text')
assert.strictEqual(g.rows[0].c1[0].tone, 'warn')
assert.strictEqual(g.rows[1].c1[0].tone, 'muted', 'an empty one in a coloured column')
assert.ok(g.rows[1].age < g.rows[0].age, 'an age sorts by its time, youngest first')

// The CRD list.
const crds = [
  { name: 'raidsets.storage.storm.io', group: 'storage.storm.io', kind: 'RaidSet', short: ['rs'], count: 2 },
  { name: 'ciliumnodes.cilium.io', group: 'cilium.io', kind: 'CiliumNode', short: [], count: 0 },
]
assert.deepStrictEqual(filterCrds(crds, 'rs').map((k) => k.kind), ['RaidSet'], 'a short name finds it')
assert.deepStrictEqual(filterCrds(crds, '', { only: 'storm' }).map((k) => k.kind), ['RaidSet'])
assert.deepStrictEqual(filterCrds(crds, '', { only: 'instances' }).map((k) => k.kind), ['RaidSet'])
assert.strictEqual(filterCrds(crds, 'CILIUM').length, 1)

// Conditions.
assert.strictEqual(conditionTone({ type: 'Ready', status: 'True' }), 'ok')
assert.strictEqual(conditionTone({ type: 'Ready', status: 'False' }), 'warn')
assert.strictEqual(conditionTone({ type: 'Degraded', status: 'True' }), 'error', 'a bad thing true is bad')
assert.strictEqual(conditionTone({ type: 'Degraded', status: 'False' }), 'ok')

// Spec and status as sections.
assert.deepStrictEqual(fieldRows({ node: 'forge1', parity: 2, tags: ['a', 'b'], drives: [{ wwn: 'x' }] }), [
  { key: 'node', text: 'forge1' },
  { key: 'parity', text: '2' },
  { key: 'tags', text: 'a, b' },
  { key: 'drives', nested: '[\n  {\n    "wwn": "x"\n  }\n]' },
])
console.log('crgrid: ok')
