// The Registry page's model (#163), with plain node: `node web/src/lib/registry.test.mjs`.
import assert from 'node:assert/strict'
import { holderWord, holderName, imageKind, ref, shortDigest, clonesOf, holderCounts, narrowed, byHolder, bytes } from './registry.js'

const clone = (name, kind, hname, extra = {}) => ({ id: name, name, holder: { kind, name: hname, ...extra }, allocated_bytes: 1, virtual_size_bytes: 9, in_use: false })
const h = {
  repositories: [
    { name: 'stormlb', clones: 3, orphans: 1, images: [
      { name: 'stormlb-aaaaaaaaaaaa', repository: 'stormlb', tag: 'aaaaaaaaaaaa', digest: 'sha256:abcdef0123456789', kind: 'component', clones: [clone('boothost-r230-2', 'boot-host', 'r230'), clone('c1', 'nobody', '')] },
      { name: 'stormlb-bbbbbbbbbbbb', repository: 'stormlb', tag: 'bbbbbbbbbbbb', digest: '', kind: 'component', clones: [clone('p1', 'pod', 'web', { namespace: 'shop' })] },
    ] },
    { name: 'pvc-1G', clones: 0, orphans: 0, images: [{ name: 'pvc-1G', repository: 'pvc-1G', tag: '', digest: '', kind: 'blank', clones: [] }] },
  ],
  unparented: [clone('lost', 'nobody', '')],
  by_holder: { 'boot-host': 1, pod: 1, nobody: 2 },
}

assert.equal(holderWord('release-disk'), 'Release disk')
assert.equal(holderName({ kind: 'pod', name: 'web', namespace: 'shop' }), 'shop/web')
assert.equal(holderName({ kind: 'boot-host', name: 'r230' }), 'r230')
assert.equal(holderName({ kind: 'nobody' }), 'nobody')
assert.equal(holderName({ kind: 'claim', withheld: true }), 'in a project you cannot see')
assert.equal(imageKind('golden'), 'container image', 'not the word')
assert.equal(ref(h.repositories[0].images[0]), 'stormlb:aaaaaaaaaaaa')
assert.equal(ref(h.repositories[1].images[0]), 'pvc-1G')
assert.equal(shortDigest('sha256:abcdef0123456789'), 'sha256:abcdef012345')
assert.equal(shortDigest(''), '')

assert.equal(clonesOf(h).length, 4)
assert.deepEqual(holderCounts(h), [{ kind: 'boot-host', label: 'Boot hosts', count: 1 }, { kind: 'pod', label: 'Pods', count: 1 }, { kind: 'nobody', label: 'Nobody', count: 2 }])

// No filter: every repository, a tag with no clones too.
assert.deepEqual(narrowed(h).map((r) => r.name), ['stormlb', 'pvc-1G'])
// Orphans only: images with one, and only those clones.
const o = narrowed(h, { kind: 'nobody' })
assert.equal(o.length, 1)
assert.deepEqual(o[0].images.map((i) => i.tag), ['aaaaaaaaaaaa'])
assert.deepEqual(o[0].images[0].clones.map((c) => c.name), ['c1'])
assert.equal(o[0].images[0].all, 2, 'and how many there are in all')
assert.deepEqual(narrowed(h, { repo: 'pvc-1G' }).map((r) => r.name), ['pvc-1G'])
// A search on a tag keeps its clones; on a holder, only that clone.
assert.equal(narrowed(h, { search: 'bbbbbbbb' })[0].images[0].clones.length, 1)
const s = narrowed(h, { search: 'r230' })
assert.deepEqual(s[0].images[0].clones.map((c) => c.holder.name), ['r230'])

// The reverse view: the machine, and what it runs on.
const r = byHolder(h, { search: 'r230' })
assert.equal(r.length, 1)
assert.equal(ref(r[0].image), 'stormlb:aaaaaaaaaaaa')
assert.equal(byHolder(h, { kind: 'pod' })[0].holder.name, 'web')
assert.equal(byHolder(h, { kind: 'nobody' }).length, 2, 'a clone with no parent left is here too')
assert.equal(byHolder(h, { kind: 'nobody' }).find((c) => c.name === 'lost').image, null)

assert.equal(bytes(0), '0 B')
assert.equal(bytes(2097152), '2.0 MiB')
console.log('registry.test.mjs PASS')
