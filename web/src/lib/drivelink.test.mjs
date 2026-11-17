import assert from 'node:assert/strict'
import { linkLevel, gbit, total, perMin, rateLine, counterRows, levelTone, trendFromHistory, linkAlarms, worstSync, listMetric } from './drivelink.js'

// forge1's bay 3 as the drive plugin's /links row carries it (stormdrive v0.49.0).
const bay3 = {
  component: 'drive:drive:5000cca26a819e69', node: 'forge1', bay: 3, serial: '2EG3K1XJ', hostSide: 'host',
  link: {
    level: 'failing', since: 1000, at: 1120, negotiatedMbps: 12000, hostMaximumMbps: 12000, hostMaximumHwMbps: 12000,
    maximum: 'maximum 12 Gbit (hardware)', policy: null, hostPhy: 'phy-1:2',
    drive: { phys: [{ phy: '0', rateMbps: 12000, invalidDwords: 4294967295, disparityErrors: 4294967295, lossOfSync: 73506628, phyResets: 0 }],
             rates: [{ phy: '0', invalidDwords: 0, disparityErrors: 0, lossOfSync: 500, phyResets: 0 }] },
    host: { phys: [{ phy: 'phy-1:2', rateMbps: 12000, invalidDwords: 0, disparityErrors: 0, lossOfSync: 0, phyResets: 0 }],
            rates: [{ phy: 'phy-1:2', invalidDwords: 0, disparityErrors: 0, lossOfSync: 0, phyResets: 0 }] },
    message: 'drive phy 0 loss of sync 500/min',
    recommendation: "the drive's receiver or its slot: reseat; if it continues, swap bays or replace",
  },
}

assert.equal(linkLevel(bay3, undefined), 'failing', 'the object first')
assert.equal(linkLevel(null, 'degraded'), 'degraded', 'else the feed')
assert.equal(linkLevel(null, '12 Gbit'), null, 'the feed\'s rate is not a level')
assert.equal(gbit(12000), '12 Gbit')
assert.equal(gbit(1500), '1.5 Gbit')
assert.equal(total(4294967295), 'saturated', 'a NetApp shelf\'s years')
assert.equal(total(73506628), '73,506,628')
assert.equal(perMin(500), '500/min')
assert.equal(perMin(0), '0')
assert.equal(perMin(0.5), '0.5/min')
assert.equal(perMin(null), '—', 'no rate on the first sample')
assert.equal(rateLine(bay3.link), '12 Gbit of 12 Gbit (hardware) maximum')
assert.equal(rateLine({ negotiatedMbps: 6000, maximum: 'maximum 6 Gbit (policy)', hostMaximumMbps: 6000, hostMaximumHwMbps: 12000 }),
  '6 Gbit of 6 Gbit (policy) maximum — below the maximum', 'pinned by the policy, said')

const rows = counterRows(bay3)
assert.equal(rows.length, 2)
assert.deepEqual(rows.map((r) => [r.side, r.phy]), [['drive', '0'], ['host', 'phy-1:2']])
const sync = rows[0].counters.find((c) => c.key === 'lossOfSync')
assert.deepEqual([sync.total, sync.rate, sync.moving], [73506628, 500, true])
assert.ok(rows[1].counters.every((c) => !c.moving), 'the HBA side is still')
assert.deepEqual(counterRows({ ...bay3, hostSide: 'expander' }).map((r) => r.side), ['drive', 'expander'])

assert.equal(levelTone('ok'), '', 'ok says nothing')
assert.equal(levelTone('degraded'), 'warn')
assert.equal(levelTone('failing'), 'error')

const t = trendFromHistory([{ at: 1000, drive: 480, host: 0 }, { at: 1060, drive: null, host: null }, { at: 1120, drive: 500, host: 0 }])
assert.deepEqual(t.map((s) => s.name), ['drive', 'host'])
assert.deepEqual(t[0].points, [{ t: 1000000, v: 480 }, { t: 1060000, v: null }, { t: 1120000, v: 500 }], 'a missing rate is a gap')
assert.deepEqual(trendFromHistory([]), [])

const alarms = linkAlarms([
  { id: 'drive:alarm:link:a', kind: 'alarm', health: 'warn', detail: 'Drive link degraded: bay 1', link: '#/drives?drive=a' },
  { id: 'drive:drive:x', kind: 'drive', health: 'error', detail: 'x' },
  { id: 'drive:alarm:link:b', kind: 'alarm', health: 'error', detail: 'Drive link failing: bay 3', link: '#/drives?drive=b' },
])
assert.deepEqual(alarms.map((a) => [a.tone, a.href]), [['error', '#/drives?drive=b'], ['warn', '#/drives?drive=a']], 'failing first, drives only')
assert.equal(worstSync(bay3.link), 500)
assert.deepEqual(listMetric('failing', bay3.link), { label: 'link errors', value: 'failing · 500/min sync', tone: 'error' })
assert.equal(listMetric('ok', bay3.link), null, 'ok says nothing')
assert.deepEqual(listMetric('degraded', null), { label: 'link errors', value: 'degraded', tone: 'warn' })
console.log('drivelink.test.mjs PASS')
