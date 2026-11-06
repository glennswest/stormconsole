// node web/src/lib/rack.test.mjs
import assert from 'node:assert'
import { rackState } from './rack.js'

const m = (node, rack) => ({ tag: 'T1', name: 'h1', node, location: rack ? { rack } : null })
let s = rackState('n1', 'r1', null)
assert.deepStrictEqual([s.owner, s.rack, s.note], ['hand', 'r1', ''], 'no stormipmi here: a formed cluster')
s = rackState('n1', '', [m('other', 'r9')])
assert.deepStrictEqual([s.owner, s.machine], ['hand', null], 'stormipmi has no machine for this node')
s = rackState('n1', 'r1', [m('n1', 'r1')])
assert.deepStrictEqual([s.owner, s.rack, s.note], ['stormipmi', 'r1', ''], 'its own')
s = rackState('n1', '', [m('n1', null)])
assert.deepStrictEqual([s.owner, s.rack, s.note], ['stormipmi', '', ''], 'a machine with no location yet: still recorded there')
s = rackState('n1', '', [m('n1', 'r1')])
assert.ok(/not on the Node yet/.test(s.note), s.note)
s = rackState('n1', 'hand', [m('n1', 'r1')])
assert.ok(/leaves a label it did not set/.test(s.note), s.note)
console.log('PASS rack.test.mjs')
