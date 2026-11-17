import assert from 'node:assert/strict'
import { means, offer, nameProblem, newUserProblem } from './users.js'

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
assert.match(newUserProblem({ ...ok, roles: [] }), /role/)
assert.match(newUserProblem({ ...ok, password: '' }), /password/)
console.log('users.test.mjs PASS')
