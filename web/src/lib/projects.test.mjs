// node web/src/lib/projects.test.mjs
import assert from 'node:assert/strict'
import { filterProjects, holdings, holdsLine, projectRows } from './projects.js'

const feed = [
  { id: 'k8s:pod:web/a' },
  { id: 'k8s:pod:web/b' },
  { id: 'k8s:ctr:web/a/app' },
  { id: 'vm:machine:web/m1' },
  { id: 'vm:instance:web/m2' },
  { id: 'vm:vmi-disk:web/m1' },
  { id: 'k8s:pvc:web/c' },
  { id: 'k8s:svc:kube-system/dns' },
  { id: 'k8s:node:n1' },
  { id: 'plugin:vm' },
]
const held = holdings(feed)
assert.deepEqual(held.get('web'), { pods: 2, machines: 2, claims: 1, services: 0 })
assert.deepEqual(held.get('kube-system'), { pods: 0, machines: 0, claims: 0, services: 1 })
assert.equal(held.get('n1'), undefined, 'a cluster-scoped row is in no project')

const rows = projectRows(
  [{ name: 'web', requester: 'alice' }, { name: 'api' }],
  [{ name: 'storm-assets' }, { name: 'kube-system' }],
  feed,
)
assert.deepEqual(rows.map((r) => r.name), ['api', 'web', 'kube-system', 'storm-assets'], "the viewer's first")
assert.deepEqual(rows.map((r) => r.system), [false, false, true, true])
assert.equal(rows[1].holds.pods, 2)
assert.equal(rows[0].holds.pods, 0)

assert.equal(filterProjects(rows).length, 4)
assert.deepEqual(filterProjects(rows, 'mine').map((r) => r.name), ['api', 'web'])
assert.deepEqual(filterProjects(rows, 'system').map((r) => r.name), ['kube-system', 'storm-assets'])
assert.deepEqual(filterProjects(rows, 'all', 'ALICE').map((r) => r.name), ['web'])
assert.deepEqual(filterProjects(rows, 'mine', 'storm'), [])

assert.equal(holdsLine(rows[1].holds), '2 pods · 2 VMs · 1 claim')
assert.equal(holdsLine(rows[0].holds), 'empty')
console.log('PASS projects.test.mjs')
