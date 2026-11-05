import assert from 'node:assert/strict'
import { fromName, registryName, versionOf, imageLine } from './imagename.js'

// The same names as holdings::from_name's test.
assert.deepEqual(fromName('golden-stormlb-aaaaaaaaaaaa'), { repository: 'stormlb', tag: 'aaaaaaaaaaaa' })
assert.deepEqual(fromName('stormlb-bbbbbbbbbbbb'), { repository: 'stormlb', tag: 'bbbbbbbbbbbb' })
assert.deepEqual(fromName('fstemplate-golden-stormlb-aaaaaaaaaaaa-raw'), { repository: 'stormlb', tag: 'aaaaaaaaaaaa' })
assert.deepEqual(fromName('stormlb-0123456789abcdef'), { repository: 'stormlb', tag: '0123456789abcdef' })
assert.deepEqual(fromName('rocky-10-x86_64'), { repository: 'rocky-10-x86_64', tag: '' })
assert.equal(registryName('golden-stormlb-aaaaaaaaaaaa'), 'registry/stormlb:aaaaaaaaaaaa')
assert.equal(registryName('fedora-44'), '', 'not a component image: left as named')
assert.equal(registryName(''), '')

const rec = { component: 'cilium', version: '1.16.3', commit: 'abc123def4567890', name: 'cilium-abc123def456', deviceSha256: 'f'.repeat(64) }
assert.deepEqual(imageLine({ golden: rec }), { ref: 'registry/cilium:1.16.3', digest: `sha256:${'f'.repeat(64)}`, version: '1.16.3' })
assert.equal(imageLine({ golden: rec, imageBuild: { version: '1.16.4' } }).ref, 'registry/cilium:1.16.4', "the node's word first")
assert.equal(imageLine({ golden: { ...rec, deviceSha256: 'sha256:ab' } }).digest, 'sha256:ab', 'not prefixed twice')
assert.equal(imageLine({ clonedFrom: 'golden-app-aaaaaaaaaaaa' }).ref, 'registry/app:aaaaaaaaaaaa', 'no record, no version: its tag')
assert.equal(imageLine({ clonedFrom: 'golden-app-aaaaaaaaaaaa' }).digest, '')
assert.equal(imageLine({ golden: { component: 'x', commit: 'c'.repeat(40) } }).ref, `registry/x:${'c'.repeat(12)}`, 'the commit, last')
assert.equal(imageLine({ golden: { component: 'x' } }).ref, 'registry/x')
assert.equal(imageLine({}), null)
assert.equal(imageLine({ golden: { available: false } }), null)
assert.equal(versionOf({}), '')
console.log('imagename.test.mjs PASS')
