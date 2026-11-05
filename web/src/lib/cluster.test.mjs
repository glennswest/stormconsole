// node web/src/lib/cluster.test.mjs (#161)
import assert from 'node:assert/strict'
import { networkOf, freeNodes, clusterCard } from './cluster.js'

const peer = (label, detail, health, metrics = {}) => ({
  id: `peer:${label}`,
  kind: 'peer',
  label,
  detail,
  health,
  metrics: Object.entries(metrics).map(([l, value]) => ({ label: l, value })),
})

// The release's flavour, over what the node announced (storm-003102 on forge1).
assert.equal(networkOf(peer('a', '', 'idle', { release: '12.61-flowsdn', network: 'cilium' })), 'flowsdn')
assert.equal(networkOf(peer('a', '', 'idle', { release: '12.61', network: 'flowsdn' })), 'cilium')
assert.equal(networkOf(peer('a', '', 'idle', { network: 'flowsdn' })), 'flowsdn')
assert.equal(networkOf(undefined), '')

const peers = [
  peer('self', 'single-node cluster, available', 'idle'),
  peer('free', 'single-node cluster, available', 'idle'),
  peer('stale', 'single-node cluster, available', 'error'),
  peer('taken', 'worker in other', 'unknown'),
  peer('master', 'master in other', 'unknown'),
  peer('laptop', 'laptop: a single-node cluster only, never joined', 'unknown'),
  peer('quiet', 'beacon only: stormcluster is not running there', 'unknown'),
]
assert.deepEqual(freeNodes(peers, 'self').map((p) => p.label), ['free'])
assert.deepEqual(freeNodes(undefined, 'self'), [])

const card = clusterCard({ id: 'system', detail: 'b1 is a single-node cluster; 4 peer(s) discovered', metrics: [{ label: 'peers', value: '4' }] })
assert.equal(card.detail, 'b1 is a single-node cluster')
assert.deepEqual(card.metrics, [])
assert.equal(clusterCard({ detail: 'storm: 3 members', metrics: [] }).detail, 'storm: 3 members')
assert.equal(clusterCard(undefined), undefined)
console.log('PASS cluster')
