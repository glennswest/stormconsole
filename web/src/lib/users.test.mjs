import assert from 'node:assert/strict'
import { means, offer, nameProblem, newUserProblem, groupNameProblem, viaGroups, memberChoices } from './users.js'

assert.equal(means('cluster-admin'), 'administrator: every action, every page')
assert.equal(means('storage-admin'), 'format drives and RAID sets, as themselves')
assert.equal(means('storage-viewer'), 'reads')
assert.deepEqual(offer(['zeta', 'view', 'storage-admin', 'alpha']), ['storage-admin', 'view', 'alpha', 'zeta'], 'suggested first, only those there')
assert.deepEqual(offer(undefined), [])
assert.equal(nameProblem('alice'), '')
assert.equal(nameProblem('storage-1'), '')
assert.match(nameProblem('Alice'), /lower-case/)
assert.match(nameProblem('-a'), /lower-case/)
assert.match(nameProblem(''), /required/)
assert.match(nameProblem('x'.repeat(64)), /63/)
const ok = { name: 'bob', password: 'p', again: 'p', roles: ['storage-admin'] }
assert.equal(newUserProblem(ok), '')
assert.match(newUserProblem({ ...ok, again: 'q' }), /differ/)
assert.match(newUserProblem({ ...ok, roles: [] }), /role or a group/)
assert.equal(newUserProblem({ ...ok, roles: [], groups: ['drives'] }), '', 'a group stands in for a role')
assert.match(newUserProblem({ ...ok, password: '' }), /password/)
// Groups (#334).
assert.equal(groupNameProblem('drives'), '')
assert.match(groupNameProblem('a'.repeat(58)), /57/)
assert.match(groupNameProblem('Drives'), /lower-case/)
const groups = [
  { name: 'drives', members: ['alice', 'bob'], roles: ['storage-admin', 'view'] },
  { name: 'readers', members: ['alice'], roles: ['view'] },
  { name: 'ops', members: ['carol'], roles: ['edit'] },
]
assert.deepEqual(viaGroups({ name: 'alice', own: ['view'] }, groups), [{ role: 'storage-admin', from: ['drives'] }], 'a role held in its own right is not said twice')
assert.deepEqual(viaGroups({ name: 'alice', own: [] }, groups), [{ role: 'storage-admin', from: ['drives'] }, { role: 'view', from: ['drives', 'readers'] }])
assert.deepEqual(viaGroups({ name: 'dave', own: [] }, groups), [])
assert.deepEqual(memberChoices([{ name: 'admin', builtin: true }, { name: 'alice' }]), ['alice'], 'admin is in no group')
console.log('users.test.mjs PASS')
