// The neutral network pages' model (#168), with plain node: `node web/src/lib/network.test.mjs`.
import assert from 'node:assert/strict'
import { notHere, mergeEndpoints, inProject, search, rulesLine, enforcement, selectsLine, fromLine } from './network.js'

const obj = { project: 'shop', name: 'web-1', ingress: null, from: 'flowsdn', state: 'ready' }
const agent = { project: 'shop', name: 'web-1', ingress: true, from: 'flowsdn', state: 'ready' }
const other = { project: 'a', name: 'x', from: 'flowsdn', ips: ['10.1.0.9'], node: 'n1' }
const m = mergeEndpoints([obj, other], [agent])
assert.equal(m.length, 2, 'one endpoint once')
assert.equal(m[1].ingress, true, "the agent's says more")
assert.equal(m[0].name, 'x', 'sorted')
assert.deepEqual(mergeEndpoints(null, null), [])

const own = { project: 'shop', name: 'allow', appliesTo: [{ project: 'shop' }] }
const wide = { project: '', name: 'all', appliesTo: [{ project: 'shop' }, { project: 'b' }] }
const elsewhere = { project: '', name: 'b-only', appliesTo: [{ project: 'b' }] }
const theirs = { project: 'b', name: 'theirs', appliesTo: [] }
const rows = [own, wide, elsewhere, theirs]
assert.deepEqual(inProject(rows, 'shop').map((r) => r.name), ['allow', 'all'], 'its own, and cluster-wide ones that reach in')
assert.equal(inProject(rows, '').length, 4, 'the cluster-wide view')

assert.deepEqual(search([other, obj], '10.1.0').map((r) => r.name), ['x'])
assert.deepEqual(search([other, obj], 'READY').map((r) => r.name), ['web-1'], 'case-blind')
assert.equal(search([other], '').length, 1)
assert.equal(search([{ name: 'p', identity: 77 }], '77').length, 1)

assert.equal(rulesLine({ ingress: 2, egress: 0 }, 'ingress'), '2 rules')
assert.equal(rulesLine({ ingress: 1 }, 'ingress'), '1 rule')
assert.equal(rulesLine({ ingress: 0, types: ['Ingress'] }, 'ingress'), 'all denied')
assert.equal(rulesLine({ egress: 0, types: ['Ingress'] }, 'egress'), '—', 'a direction not named is left alone')

assert.equal(enforcement(true), 'enforced')
assert.equal(enforcement(false), 'not enforced')
assert.equal(enforcement(null), '—')

assert.equal(selectsLine({ selects: ['app=web'], project: 'p' }), 'app=web')
assert.equal(selectsLine({ selects: [], project: 'p' }), 'every pod in the project')
assert.equal(selectsLine({ selects: [], project: '' }), 'every endpoint')

assert.equal(fromLine([{ from: 'cilium' }, { from: 'cilium' }], ''), 'from: cilium')
assert.equal(fromLine([], 'flowsdn'), 'from: flowsdn')
assert.equal(fromLine([], ''), '')
console.log('network.test.mjs PASS')

// The other network's addresses land on this one's (#285).
assert.equal(notHere('#/flowsdn', 'flowsdn'), null)
assert.equal(notHere('#/flowsdn', 'cilium'), '#/network/endpoints')
assert.equal(notHere('#/k8s/fsid', 'cilium'), '#/k8s/cid')
assert.equal(notHere('#/network/flows', 'cilium'), null)
assert.equal(notHere('#/network/flows', 'flowsdn'), '#/flowsdn')
assert.equal(notHere('#/k8s/cid', 'flowsdn'), '#/k8s/fsid')
assert.equal(notHere('#/k8s/cn', 'flowsdn'), '#/flowsdn')
assert.equal(notHere('#/flowsdn', 'unknown'), '#/network/endpoints')
assert.equal(notHere('#/network/flows', 'unknown'), '#/network/endpoints')
assert.equal(notHere('#/flowsdn', ''), null, 'not said yet: stays')
assert.equal(notHere('#/k8s/pod', 'flowsdn'), null)
