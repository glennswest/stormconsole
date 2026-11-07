// node web/src/lib/drivefirmware.test.mjs
import assert from 'node:assert/strict'
import { imageModel, imagesFor, imageWhy, modelKey, phrase, readBack, rows, selectionWhy, uploadName } from './drivefirmware.js'

// The same names as firmware.rs.
assert.equal(modelKey('Samsung SSD 870 EVO'), 'Samsung-SSD-870-EVO')
assert.equal(uploadName('H7210A521SUN010T', 'C:\\fw\\A3Z0 build.bin'), 'H7210A521SUN010T+A3Z0-build.bin')
assert.equal(imageModel('H7210A521SUN010T+A3Z2.bin'), 'H7210A521SUN010T')
assert.equal(imageModel('loose.bin'), null)
assert.throws(() => uploadName('', 'x.bin'), /no model/)

const rec = (name, extra = {}) => ({ node: 'forge1', id: name, name, serial: `S-${name}`, model: 'H7210A521SUN010T', modelKey: 'H7210A521SUN010T', version: 'A3Z0', whyNot: null, ...extra })
const sdb = rec('sdb')
const sdc = rec('sdc')
const far = rec('sdx', { node: 'storm-b' })
const other = rec('sdz', { model: 'ST4000', modelKey: 'ST4000' })
const k1 = rec('sdk', { set: 'ds0-a', setState: 'clean' })
const k2 = rec('sdm', { set: 'ds0-a', setState: 'clean' })

assert.equal(selectionWhy([sdb, sdc, far]), '')
assert.match(selectionWhy([sdb, other]), /^one model at a time/)
assert.match(selectionWhy([sdb, rec('sdq', { whyNot: 'it is formatting' })]), /^sdq: it is formatting/)
assert.match(selectionWhy([k1, k2]), /^sdk and sdm are both in RAID set ds0-a: one at a time.*stormdrive#126/)
assert.equal(selectionWhy([k1, sdb]), '', 'one member and a free drive')
assert.match(selectionWhy([rec('sdk', { set: 'ds0-a', setState: 'degraded' })]), /which is degraded/)
assert.equal(selectionWhy([]), 'no drives chosen')

const images = [
  { node: 'forge1', name: 'H7210A521SUN010T+A3Z2.bin', model: 'H7210A521SUN010T' },
  { node: 'storm-b', name: 'H7210A521SUN010T+A3Z2.bin', model: 'H7210A521SUN010T' },
  { node: 'forge1', name: 'H7210A521SUN010T+A3Z1.bin', model: 'H7210A521SUN010T' },
  { node: 'forge1', name: 'ST4000+SN04.bin', model: 'ST4000' },
  { node: 'forge1', name: 'loose.bin', model: null },
]
const im = imagesFor([sdb, far], images)
assert.deepEqual(im.fits.map((i) => [i.name, i.missing]), [['H7210A521SUN010T+A3Z2.bin', []], ['H7210A521SUN010T+A3Z1.bin', ['storm-b']]])
assert.deepEqual(im.unnamed.map((i) => i.name), ['loose.bin'])
assert.equal(imageWhy([sdb, far], im.fits[0], false), '')
assert.match(imageWhy([sdb, far], im.fits[1], false), /not in the image store of storm-b/)
assert.match(imageWhy([sdb], { name: 'loose.bin', model: null, missing: [] }, false), /stormdrive#126.*tick/)
assert.equal(imageWhy([sdb], { name: 'loose.bin', model: null, missing: [] }, true), '')
assert.match(imageWhy([sdb], { name: 'ST4000+SN04.bin', model: 'ST4000', missing: [] }, true), /is for ST4000/)
assert.equal(phrase([sdb]), 'S-sdb')
assert.equal(phrase([sdb, sdc]), 'update 2 drives')

// Runs: running first, then those that ended within the hour, read back against the drive's version now.
const t = (s) => ({ secs_since_epoch: s, nanos_since_epoch: 0 })
const going = rec('sdb', { run: { state: 'running', phase: 'download', percent: 40 } })
const ok = rec('sdc', { version: 'A3Z2', run: { state: 'done', from: 'A3Z0', to: 'A3Z2', finished: t(1000) } })
const old = rec('sdd', { run: { state: 'done', from: 'A', to: 'B', finished: t(1) } })
const bad = rec('sde', { run: { state: 'failed', from: 'A3Z0', error: 'DOWNLOAD MICROCODE: ILLEGAL REQUEST', finished: t(1100) } })
const r = rows([ok, old, going, bad], 1200, 600)
assert.deepEqual(r.map((x) => x.name), ['sdb', 'sdc', 'sde'])
assert.equal(r[0].back, null)
assert.deepEqual(readBack(ok), { ok: true, text: 'reads back A3Z2 (was A3Z0)' })
assert.equal(readBack(bad).ok, false)
assert.match(readBack(bad).text, /^still A3Z0: DOWNLOAD MICROCODE/)
assert.equal(readBack(rec('sdq', { version: 'A3Z0', run: { state: 'done', to: 'A3Z2', from: 'A3Z0' } })).ok, false, 'a version that did not move is said')
console.log('PASS drivefirmware.test.mjs')
