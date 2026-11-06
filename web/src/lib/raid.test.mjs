// node web/src/lib/raid.test.mjs
import assert from 'node:assert'
import { ordered, where, driveWords, needsHands, progress, memberActions, canScrub, stateWord, HEALTH } from './raid.js'

const set = (name, state) => ({ name, status: { state } })
assert.deepStrictEqual(ordered([set('a', 'clean'), set('z', 'failed'), set('b', 'rebuilding'), set('c', 'degraded')]).map((s) => s.name), ['z', 'c', 'b', 'a'], 'worst first')

assert.strictEqual(where({ slot: 4, bay: '7' }), 'bay 7')
assert.strictEqual(where({ slot: 4, bay: null }), 'slot 4', 'a drive gone has its slot still')
assert.strictEqual(driveWords({ serial: 'ZC1', path: '/dev/sdb' }), 'ZC1')
assert.strictEqual(driveWords({ serial: 'file', path: '/x/d.img' }), '/x/d.img', 'a file-backed drive has no serial to speak of')
assert.strictEqual(driveWords({ path: '' }), 'drive not here')
assert.ok(needsHands({ state: 'failed' }) && needsHands({ state: 'missing' }) && !needsHands({ state: 'rebuilding' }) && !needsHands({ state: 'active' }))

const [r] = progress({ rebuild: { slots: [3], percent: 42.5, running: true, rate_bytes_per_sec: 31457280 } })
assert.strictEqual(r.text, 'Rebuilding slot 3 at 30 MiB/s')
assert.strictEqual(r.percent, 42.5)
assert.deepStrictEqual(progress({ rebuild: { slots: [3], percent: 100, running: false } }), [], 'a finished rebuild is not news')
assert.strictEqual(progress({ rebuild: { slots: [3], percent: 9, running: false, error: 'read failed' } })[0].error, 'read failed', 'a stopped one is')
const [s] = progress({ scrub: { percent: 100, running: false, mismatches: 2, repaired: 0 } })
assert.ok(s.bad && s.text === 'Last scrub: 2 mismatches', s.text)
assert.strictEqual(progress({ scrub: { percent: 5, running: true, mismatches: 0 } })[0].text, 'Scrubbing, no mismatches so far')
assert.deepStrictEqual(progress(undefined), [])

const comps = [{ id: 'sb:member:/dev/a', actions: [{ id: 'fail', path: '/x' }, { id: 'replace', path: '/y' }] }, { id: 'sb:member:/dev/b', actions: [] }]
assert.strictEqual(memberActions(comps, { component: 'sb:member:/dev/a' }).replace.path, '/y')
assert.strictEqual(memberActions(comps, { component: 'sb:member:/dev/b' }).fail, null, 'withheld means no button')
assert.strictEqual(memberActions(comps, { component: 'gone' }).fail, null)

assert.ok(canScrub({ state: 'clean' }))
assert.ok(canScrub({ state: 'degraded' }))
assert.ok(!canScrub({ state: 'clean', scrub: { running: true } }))
assert.ok(!canScrub({ state: 'rebuilding', rebuild: { running: true } }))
assert.ok(!canScrub({ state: 'failed' }))
assert.strictEqual(stateWord('degraded'), 'Degraded')
assert.strictEqual(HEALTH.failed, 'error')
console.log('raid.test PASS')
