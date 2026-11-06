import assert from 'node:assert/strict'
import { capability, result, what } from './audit.js'

assert.equal(capability('console'), 'Opened a console')
assert.equal(capability('other'), 'other')
assert.deepEqual(result(101), { word: 'done', tone: 'ok' }, 'a console that opened')
assert.equal(result(200).tone, 'ok')
assert.equal(result(403).word, 'refused (403)')
assert.equal(result(428).tone, 'warn', 'stopped for the typed name')
assert.equal(result(502).word, 'failed (502)')
assert.equal(what({ capability: 'console', object: 'x', may_write: false }), 'x (watching only)')
assert.equal(what({ capability: 'console', object: 'x', may_write: true }), 'x')
assert.equal(what({ capability: 'delete', object: 'x', may_write: false }), 'x')
console.log('audit.test.mjs PASS')
