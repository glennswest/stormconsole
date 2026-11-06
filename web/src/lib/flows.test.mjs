import assert from 'node:assert/strict'
import { verdictWord, verdictTone, dropWord, endWord, portWord, addFlow, flowQuery } from './flows.js'

assert.equal(verdictWord('FORWARDED'), 'allowed')
assert.equal(verdictWord('DROPPED'), 'dropped')
assert.equal(verdictWord(undefined), 'unknown')
assert.equal(verdictTone('DROPPED'), 'bad')
assert.equal(dropWord('POLICY_DENIED'), 'policy denied')
assert.equal(dropWord(null), '')
assert.equal(endWord({ project: 'shop', pod: 'web' }), 'shop/web')
assert.equal(endWord({ hidden: true }), 'another project')
assert.equal(endWord({ labels: ['reserved:world'], ip: '203.0.113.9' }), 'world 203.0.113.9')
assert.equal(endWord({ ip: '10.0.0.1', labels: [] }), '10.0.0.1')
assert.equal(portWord({ protocol: 'TCP', port: 443 }), 'TCP/443')
assert.equal(portWord({ protocol: 'ICMPv4' }), 'ICMPv4')
assert.deepEqual(addFlow([1, 2, 3], 0, 3), [0, 1, 2])
assert.equal(flowQuery({ ns: 'shop', pod: 'web', verdict: 'DROPPED', last: 50 }), 'ns=shop&pod=web&verdict=DROPPED&last=50')
assert.equal(flowQuery({}), '')
console.log('flows.test.mjs PASS')
