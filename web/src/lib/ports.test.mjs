// node web/src/lib/ports.test.mjs
import assert from 'node:assert'
import {
  portRows, wantWord, moduleWord, moduleLines, dbm, appliedWords, counterLabel, counterRows,
  draftOf, changeOf, changeLine, lastChangeLine,
} from './ports.js'

// A row as GET /api/plugins/k8s/nodes/<n>/ports gives it (crates/plugins/kubernetes/src/ports.rs).
const p25 = {
  name: 'forge1-ens1f0np0', ifname: 'ens1f0np0', mac: 'e8:eb:d3:94:a2:80', health: 'warn', state: 'down',
  speed: null, speedWord: '—', fec: { want: 'rs', source: 'spec', configured: 'baser', active: 'baser' },
  module: { present: true, kind: 'SFP', vendor: 'Mellanox', part: 'MCP2M00-A002', compliance: ['25GBASE-CR CA-N', 'passive copper'],
    lengthM: 2, temperatureC: 35.5, rxPowerMw: [0.5], txPowerMw: [0.6] },
  applied: { status: 'Unknown', reason: 'Applying', message: 'fec baser -> rs' }, pending: false, spec: { fec: 'rs' },
}
const r = portRows([p25])[0]
assert.deepStrictEqual([r.id, r.link, r.fecText, r.want, r.module, r.applied],
  ['forge1-ens1f0np0', 'down', 'baser', 'rs (this object)', 'MCP2M00-A002 · Mellanox', 'applying'])
assert.deepStrictEqual(portRows(null), [])

assert.strictEqual(wantWord(null), 'unmanaged')
assert.strictEqual(wantWord({ want: 'rs', source: 'table' }), "rs (port-fec.toml's speed table)")
assert.strictEqual(moduleWord({ present: false }), 'empty cage')
assert.strictEqual(moduleWord(null), '—', 'a BASE-T port has no module at all')

const lines = Object.fromEntries(moduleLines(p25.module))
assert.strictEqual(lines.Compliance, '25GBASE-CR CA-N, passive copper')
assert.strictEqual(lines.Length, '2 m')
assert.strictEqual(lines['Rx power'], '0.500 mW (-3.0 dBm)')
assert.ok(!('Serial' in lines), 'nothing said for a field the module does not have')
assert.deepStrictEqual(moduleLines({ present: false }), [])
assert.strictEqual(Object.fromEntries(moduleLines({ present: true, rxPowerMw: [1, 0] }))['Rx power'], 'lane 0: 1.000 mW (0.0 dBm), lane 1: 0.000 mW (— dBm)')
assert.strictEqual(dbm(0), null)

assert.strictEqual(appliedWords({ reason: 'Applied' }).tone, 'ok')
assert.strictEqual(appliedWords({ reason: 'Refused', message: 'nothing applied: x' }).word, 'refused')
assert.strictEqual(appliedWords({ reason: 'NotSupported' }).word, 'partly applied')
assert.strictEqual(appliedWords({ reason: 'Applied' }, true).word, 'waiting for the node', 'a newer generation the node has not read')
assert.strictEqual(appliedWords(null).word, '—')

assert.strictEqual(counterLabel('fecUncorrectableBlocks'), 'fec uncorrectable blocks')
const rows = counterRows({ txBytes: 10, rxCrcErrors: 2, rxPackets: 100, fecCorrectedBlocks: 5, pauseRxFrames: 0, odd: 'x' }, { rxPackets: 40, rxCrcErrors: 3 }, 3)
assert.deepStrictEqual(rows.map((c) => c.key), ['rxCrcErrors', 'rxPackets', 'txBytes', 'fecCorrectedBlocks', 'pauseRxFrames'])
assert.strictEqual(rows.find((c) => c.key === 'rxPackets').rate, 20)
assert.strictEqual(rows.find((c) => c.key === 'rxCrcErrors').rate, null, 'a counter that went back (a reset) has no rate')
assert.strictEqual(rows.find((c) => c.key === 'rxCrcErrors').bad, true)
assert.strictEqual(rows.find((c) => c.key === 'fecCorrectedBlocks').bad, false, 'corrected is what FEC is for')

// The form: only what changed goes, an emptied setting is null.
const spec = { fec: 'baser', mtu: 9000, pause: { rx: true } }
const d = draftOf(spec)
assert.deepStrictEqual(d, { fec: 'baser', mtu: '9000', speed: '', autoneg: '', pauseRx: 'on', pauseTx: '', ringRx: '', ringTx: '' })
assert.deepStrictEqual(changeOf(d, spec), {}, 'nothing changed')
assert.deepStrictEqual(changeOf({ ...d, fec: 'rs' }, spec), { fec: 'rs' })
assert.deepStrictEqual(changeOf({ ...d, fec: '', mtu: '' }, spec), { fec: null, mtu: null })
assert.deepStrictEqual(changeOf({ ...d, pauseRx: '' }, spec), { pause: null })
assert.deepStrictEqual(changeOf({ ...d, pauseTx: 'off' }, spec), { pause: { rx: true, tx: false } })
assert.deepStrictEqual(changeOf({ ...draftOf({}), ringRx: '4096' }, {}), { rings: { rx: 4096 } })
assert.deepStrictEqual(changeOf({ ...draftOf({}), autoneg: 'off', speed: '25000' }, {}), { speed: 25000, autoneg: false })
assert.deepStrictEqual(changeOf({ ...d, mtu: 'big' }, spec), { error: 'MTU: a whole number' })
assert.strictEqual(changeLine({ fec: 'rs', mtu: null }), 'fec rs, mtu left to the node')

assert.strictEqual(lastChangeLine({ setting: 'fec', from: 'baser', to: 'rs', by: 'networkport/n-x generation 4', at: 't' }), 'fec baser → rs by networkport/n-x generation 4 at t')
assert.strictEqual(lastChangeLine({ setting: 'fec', from: 'baser', to: 'rs', source: 'table', error: 'EOPNOTSUPP' }), 'fec baser → rs from table, failed: EOPNOTSUPP')
assert.strictEqual(lastChangeLine(null), '')
console.log('PASS ports.test.mjs')
