// The progress view's model (#84), with plain node: `node web/src/lib/progress.test.mjs`.
import assert from 'node:assert/strict'
import { byNode, requestNodes, moving } from './progress.js'

const st = (step, node, status, extra = {}) => ({ step: node ? { step, node } : { step }, description: `${step} ${node || ''}`.trim(), status, ...extra })

// A join of b2 and b3: b2 failed at its token, b3 not reached, the cluster's
// own steps after them.
const join = {
  id: 'join-1',
  state: 'failed',
  request: { op: 'join', nodes: ['b2', 'b3'], role: 'master' },
  steps: [
    st('checkJoinable', 'b2', 'done'),
    st('checkJoinable', 'b3', 'done'),
    st('enroll', 'b2', 'failed', { error: 'stormcert did not answer' }),
    st('nodeJoin', 'b2', 'pending'),
    st('enroll', 'b3', 'pending'),
    st('nodeJoin', 'b3', 'pending'),
    st('publish', null, 'pending'),
  ],
}
const rows = byNode(join)
assert.deepEqual(rows.map((r) => [r.label, r.status, r.done, r.total]), [
  ['b2', 'failed', 1, 3],
  ['b3', 'pending', 1, 3],
  ['the cluster', 'pending', 0, 1],
])
assert.equal(rows[0].current.description, 'enroll b2')
assert.equal(rows[0].current.error, 'stormcert did not answer')
assert.equal(rows[1].current.description, 'enroll b3', 'the step b3 waits at')
assert.equal(rows[2].node, null)

// A form running: b1 done, b4 at its join, a worker named only by the request.
const form = {
  state: 'running',
  request: { op: 'form', name: 'storm', masters: ['b1'], workers: ['b4', 'b5'] },
  steps: [st('seed', 'b1', 'done'), st('ensureEndpoint', null, 'done'), st('nodeJoin', 'b4', 'running')],
}
const f = byNode(form)
assert.deepEqual(f.map((r) => [r.label, r.status]), [['b1', 'done'], ['b4', 'running'], ['b5', 'pending'], ['the cluster', 'done']])
assert.equal(f[1].current.description, 'nodeJoin b4')
assert.equal(f[2].total, 0, 'a node the request names before any step of it')
assert.ok(moving(form) && !moving(join))

// A node only steps name (no request), and a demote's single node.
assert.deepEqual(requestNodes({ op: 'demote', node: 'b3' }), ['b3'])
assert.deepEqual(byNode({ steps: [st('cordon', 'b9', 'done')] }).map((r) => r.label), ['b9'])
assert.deepEqual(byNode(null), [])
console.log('progress.test.mjs: ok')
