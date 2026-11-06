import assert from 'node:assert/strict'
import { bootStory, uptimeMs, flowStory, rates, sparkline, clock, seconds } from './boot.js'

const tl = [
  { t_ms: 3000, event: 'boot', wall_unix: 1 },
  { t_ms: 3000, event: 'pid1' },
  { t_ms: 3100, event: 'phase', name: 'modules', ms: 900 },
  { t_ms: 3050, event: 'phase', name: 'early', ms: 40 },
  { t_ms: 5000, event: 'started', name: 'stormdrive' },
  { t_ms: 9000, event: 'all-running', detail: '2 service(s)' },
]
const s = bootStory({ timeline: tl, release: '12.61', hostname: 'n1', current: true, started: 2, at: '2026-11-06T00:00:00Z' })
assert.deepEqual(s.rows.map((r) => r.name), ['firmware end to PID 1', 'early', 'modules'], 'what came before PID 1, then in the order they began')
assert.equal(s.rows[0].ms, 3000)
assert.equal(s.span, 9000)
assert.equal(s.allRunningMs, 9000)
assert.equal(uptimeMs(s, Date.parse('2026-11-06T00:01:00Z')), 63000, 'the wall clock since PID 1, plus when PID 1 started')
assert.equal(uptimeMs({ ...s, current: false }), null, 'an old boot has no running clock')
assert.equal(bootStory(null).rows.length, 0)
assert.equal(bootStory({ timeline: [{ event: 'boot', t_ms: 1 }] }).allRunningMs, null, 'nothing has happened yet')
const many = bootStory({ timeline: Array.from({ length: 14 }, (_, i) => ({ event: 'phase', name: `p${i}`, t_ms: i + 1, ms: 1 })) })
assert.equal(many.rows.length, 9)
assert.equal(many.more, 6, 'the first event is PID 1 by default, so 15 rows, 6 not shown')

const flow = { moved: 300, zeroed: 100, bytes: 1200e6, seconds: 60, mb_per_s: 20, eta_seconds: 90, extents_per_s: 6.6 }
const moving = flowStory({ flow_over_remaining: 600, flow_over: flow, slabs: { diskless: false, system: 'mixed', data: 'mixed' } })
assert.equal(moving.state, 'moving')
assert.equal(moving.percent, 40, 'an all-zero extent counts as done')
assert.equal(moving.eta, 90)
assert.equal(flowStory({ flow_over_remaining: 5, flow_over: flow, flow_over_stalled: 'destination full' }).sub, 'destination full')
assert.equal(flowStory({ flow_over_remaining: 5, flow_over: flow, flow_over_stalled: 'x' }).state, 'stalled')
const local = (extra) => flowStory({ flow_over_remaining: 0, flow_over: { ...flow, ...extra }, slabs: { diskless: false, system: 'local', data: 'local' } })
assert.equal(local({}).state, 'local')
assert.equal(local({}).word, 'Running from its own disk')
assert.equal(local({}).percent, 100)
assert.equal(local({ validation: { state: 'running', validated: 2, total: 9 } }).state, 'checking', 'not its own disk until it has been checked')
assert.equal(local({ validation: { state: 'passed' } }).state, 'local')
assert.equal(local({ validation: { state: 'corrupt', mismatched: 2, unrepaired: 1 } }).state, 'corrupt')
assert.notEqual(local({ validation: { state: 'corrupt', mismatched: 2, unrepaired: 1 } }).word, 'Running from its own disk')
assert.equal(flowStory({ flow_over_remaining: 0, flow_over: flow, slabs: { diskless: false, system: 'mixed', data: 'local' } }).state, 'settling', 'never its own disk while a slab says the network')
assert.equal(flowStory({ slabs: { diskless: false, system: 'local', data: 'local' } }).state, 'local')
assert.equal(flowStory({ slabs: { diskless: true, system: 'remote', data: 'remote' } }).state, 'network')
assert.equal(flowStory({ slabs: { diskless: false, system: 'remote', data: 'local' } }).state, 'network')
assert.equal(flowStory({ slabs: { diskless: true, system: 'none', data: 'none', items: [] } }).state, 'none', 'a real engine with nothing placed: not "the network"')
assert.equal(flowStory({}).state, 'none', 'slabs not known yet claims nothing about where it runs')
assert.equal(flowStory(null).state, 'unknown')
assert.equal(flowStory({ flow_over_remaining: 0 }).percent, 100, 'nothing to move')

assert.deepEqual(rates([{ t: 0, bytes: 0 }, { t: 2000, bytes: 100e6 }, { t: 4000, bytes: 100e6 }]), [50, 0], 'from the counter, not the average')
assert.deepEqual(rates([{ t: 0, bytes: 9e6 }, { t: 2000, bytes: 0 }, { t: 4000, bytes: 2e6 }]), [1], 'a counter that fell is a new engine, not a negative rate')
assert.equal(sparkline([1]), '')
assert.equal(sparkline([0, 10], 100, 10), '0.0,8.0 100.0,2.0')
assert.equal(clock(65000), '1:05')
assert.equal(clock(3725000), '1:02:05')
assert.equal(clock(null), '—')
assert.equal(seconds(1234), '1.2 s')
assert.equal(seconds(61000), '61 s')
console.log('boot.test.mjs PASS')
