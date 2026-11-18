import assert from 'node:assert/strict'
import { fromName, registryName, versionOf, imageLine, buildLine } from './imagename.js'

// The same names as holdings::from_name's test.
assert.deepEqual(fromName('golden-stormlb-aaaaaaaaaaaa'), { repository: 'stormlb', tag: 'aaaaaaaaaaaa' })
assert.deepEqual(fromName('stormlb-bbbbbbbbbbbb'), { repository: 'stormlb', tag: 'bbbbbbbbbbbb' })
assert.deepEqual(fromName('fstemplate-golden-stormlb-aaaaaaaaaaaa-raw'), { repository: 'stormlb', tag: 'aaaaaaaaaaaa' })
assert.deepEqual(fromName('stormlb-0123456789abcdef'), { repository: 'stormlb', tag: '0123456789abcdef' })
assert.deepEqual(fromName('rocky-10-x86_64'), { repository: 'rocky-10-x86_64', tag: '' })
assert.equal(registryName('golden-stormlb-aaaaaaaaaaaa'), 'registry/stormlb:aaaaaaaaaaaa')
assert.equal(registryName('fedora-44'), '', 'not a component image: left as named')
assert.equal(registryName(''), '')

// From the node's own objects (#347): golden name, image-build, image-digest.
const D = `sha256:${'c'.repeat(64)}`
assert.deepEqual(imageLine({ clonedFrom: 'cilium-abc123def456', imageBuild: { version: '1.16.3' }, digest: D }), { ref: 'registry/cilium:1.16.3', digest: D, version: '1.16.3' })
assert.equal(imageLine({ clonedFrom: 'golden-app-aaaaaaaaaaaa' }).ref, 'registry/app:aaaaaaaaaaaa', 'no version: its tag')
assert.equal(imageLine({ clonedFrom: 'golden-app-aaaaaaaaaaaa' }).digest, '')
assert.equal(imageLine({ clonedFrom: 'x', digest: 'ab' }).digest, 'sha256:ab', 'prefixed once')
assert.equal(imageLine({ clonedFrom: 'x' }).ref, 'registry/x')
assert.equal(imageLine({}), null)
assert.equal(imageLine({ digest: D }), null, 'no registry image named, none invented')
assert.equal(versionOf({}), '')
assert.equal(
  buildLine({ imageBuild: { 'org.opencontainers.image.created': '2026-11-18', revision: 'abc123def4567890', version: '1.16.3' } }),
  'version 1.16.3 · revision abc123def456 · created 2026-11-18'
)
assert.equal(buildLine({ imageBuild: null }), '')
assert.equal(buildLine({}), '')
console.log('imagename.test.mjs PASS')
