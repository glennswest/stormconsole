// node web/src/lib/hba.test.mjs
import assert from 'node:assert/strict'
import { body, busy, manifestBody, offer, offerText, offers, opImage, storeHolds, byNode, current, driveIdOf, drivesOnCards, flashWhy, gbit, limitText, phaseText, phaseTone, progress, rateText, requestWhy, running, slow, storeImages, versions } from './hba.js'

// forge1's HBA330 as stormdrive v0.56.0 writes it (card.rs `object`), through the k8s plugin's /hbas.
const sha = 'ab'.repeat(32)
const forge1 = {
  name: 'forge1-0000-01-00-0',
  status: {
    node: 'forge1', present: true, pci: '0000:01:00.0', board: 'HBA330 Mini', chip: 'SAS3008', firmwareVersion: '16.17.00.03', biosVersion: '08.37.00.00',
    phys: [
      { phy: 0, port: 0, rate: '12.0 Gbit', rateMbps: 12000, maxRateMbps: 12000, minRateMbps: 1500, disabled: false, attached: { sasAddress: '5000cca0', kind: 'SAS drive', drive: 'wwn-5000cca0', name: 'sdb' } },
      { phy: 1, port: 1, rate: '6.0 Gbit', rateMbps: 6000, maxRateMbps: 12000, minRateMbps: 1500, disabled: false, attached: { sasAddress: '5000cca1', kind: 'SAS drive', drive: 'wwn-5000cca1', name: 'sdc' } },
      { phy: 2, port: null, rate: 'unknown', rateMbps: null, maxRateMbps: 12000, minRateMbps: 1500, disabled: false, attached: null },
      { phy: 3, port: null, rate: 'disabled', rateMbps: null, maxRateMbps: null, minRateMbps: null, disabled: true, attached: null },
    ],
    devices: [{ handle: 9, sasAddress: '5000cca9', kind: 'SAS drive', drive: 'wwn-behind-expander', slot: 7 }],
  },
  operations: [],
}
const forge2 = { name: 'forge2-0000-02-00-0', status: { node: 'forge2', present: true, phys: [] }, operations: [] }

// A drive's component id, here and on another node, to stormdrive's id.
assert.equal(driveIdOf('drive:drive:wwn-5000cca0'), 'wwn-5000cca0')
assert.equal(driveIdOf('drive:@forge2:drive:wwn-5000cca0'), 'wwn-5000cca0')
assert.equal(driveIdOf('shelf:x'), null)

// Rates said in Gbit; what the card allows; a phy held below it.
assert.equal(gbit(12000), '12 Gbit')
assert.equal(gbit(1500), '1.5 Gbit')
assert.equal(gbit(22500), '22.5 Gbit')
assert.equal(gbit(null), null)
assert.equal(rateText(forge1.status.phys[0]), '12 Gbit')
assert.equal(rateText(forge1.status.phys[2]), 'no link')
assert.equal(rateText(forge1.status.phys[3]), 'disabled')
assert.equal(rateText({ rate: 'negotiation failed' }), 'negotiation failed')
assert.equal(limitText(forge1.status.phys[0]), '1.5–12 Gbit')
assert.equal(limitText(forge1.status.phys[3]), '—')
assert.ok(slow(forge1.status.phys[1]) && !slow(forge1.status.phys[0]))

// Each drive on its card: by the phy, else by the card's device list (behind an expander).
const on = drivesOnCards([forge1, forge2])
assert.deepEqual(on.get('wwn-5000cca1'), { card: 'forge1-0000-01-00-0', node: 'forge1', board: 'HBA330 Mini', phy: 1, rateMbps: 6000, maxRateMbps: 12000 })
assert.equal(on.get('wwn-behind-expander').phy, null)
assert.equal(on.get('wwn-behind-expander').slot, 7)
assert.equal(on.size, 3)

assert.deepEqual(byNode([forge2, forge1]).map((g) => g.node), ['forge1', 'forge2'])
assert.equal(current(forge1, 'firmware'), '16.17.00.03')
assert.equal(current(forge1, 'bios'), '08.37.00.00')

// An operation's life: waiting, running with bytes, each ending said.
assert.ok(running({ phase: '' }) && running({ phase: 'Running' }) && running({ phase: 'Pending' }))
assert.ok(!running({ phase: 'Succeeded' }) && !running({ phase: 'Refused' }) && !running({ phase: 'Planned' }))
assert.deepEqual(progress({ phase: 'Running', step: 'download', bytesDone: 65536, bytesTotal: 262144 }), { fraction: 0.25, text: 'download 25% (65536 of 262144 bytes)' })
assert.deepEqual(progress({ phase: 'Running', step: 'reset' }), { fraction: null, text: 'reset' })
assert.equal(progress({ phase: 'Succeeded' }), null)
assert.match(phaseText({ phase: 'Written' }), /next reset/)
assert.match(phaseText({ phase: 'Failed' }), /keeps running what it ran/)
assert.match(phaseText({ phase: 'Planned' }), /dry run/)
assert.equal(phaseTone({ phase: 'Refused' }), 'error')
assert.equal(phaseTone({ phase: 'Succeeded' }), 'ok')
assert.equal(versions({ before: '16.17.00.03', after: '16.17.01.00' }), '16.17.00.03 → 16.17.01.00')
assert.equal(versions({ before: '16.17.00.03', after: '16.17.00.03' }), '16.17.00.03 (same version)')
assert.equal(versions({ phase: 'Planned', before: '16.17.00.03', imageVersion: '16.17.01.00' }), '16.17.00.03 → 16.17.01.00 (image)')
assert.equal(versions({}), '—')

// Flash is withheld, with the reason: a card gone, one flash going, a refusal of the review.
assert.equal(flashWhy(forge1, { allowed: true }), '')
assert.match(flashWhy({ ...forge1, status: { ...forge1.status, present: false } }), /not seen on forge1/)
assert.match(flashWhy({ ...forge1, operations: [{ name: 'f1', phase: 'Running' }] }, { allowed: true }), /f1 is running: one flash at a time/)
assert.equal(flashWhy({ ...forge1, operations: [{ name: 'f1', phase: 'Succeeded', finished: 't' }] }, { allowed: true }), '')
assert.match(flashWhy(forge1, { allowed: false, reason: 'not a storage-admin: the apiserver does not allow create cardoperations.storage.storm.io' }), /not a storage-admin/)
assert.ok(busy([{ operations: [{ phase: '' }] }]) && !busy([forge1]))

// The images: only the card's node's store, with their sha256.
const imgs = storeImages(forge1, [
  { node: 'forge1', name: 'hba330-16.17.01.00.bin', size: 1048576, sha256: sha.toUpperCase() },
  { node: 'forge2', name: 'other.bin', size: 1, sha256: sha },
  { node: 'forge1', name: 'no-sha.bin', size: 1 },
])
assert.deepEqual(imgs, [{ name: 'hba330-16.17.01.00.bin', size: 1048576, sha256: sha }])

// The request, checked as the server checks it.
const ok = body('firmware', imgs[0], ' https://dl.example/hba330-16.17.01.00.bin ', true)
assert.deepEqual(ok, { region: 'firmware', image: { name: 'hba330-16.17.01.00.bin', sha256: sha, source: 'https://dl.example/hba330-16.17.01.00.bin' }, dryRun: true })
assert.equal(requestWhy(ok), '')
assert.match(requestWhy(body('manufacturing', imgs[0], 'https://x/y', false)), /firmware or bios/)
assert.match(requestWhy(body('firmware', null, 'https://x/y', false)), /firmware store/)
assert.match(requestWhy(body('firmware', imgs[0], 'ftp://x/y', false)), /recorded from/)
assert.match(requestWhy(body('firmware', { name: 'a', sha256: 'abc' }, 'https://x/y', false)), /64 hex/)

// #380: the release manifest's offers (stormdrive#184), per region.
assert.equal(offer(forge1, 'firmware'), null, 'no offers written: stormdrive older than v0.59.0')
assert.equal(offers(forge1), null)
const m = { ...forge1, status: { ...forge1.status, firmwareAvailable: {
  firmware: { current: '16.17.00.03', flashable: true, image: { region: 'firmware', version: '16.17.01.00', file: 'hba330-16.17.01.00.bin', sha256: sha.toUpperCase(), source: 'https://dl.example/hba330.bin', golden: 'hba330-p16' } },
  bios: { current: '08.37.00.00', flashable: true, image: null },
  uefi: { current: null, flashable: false, image: null },
  nvdata: { current: '0e.01.00.07', flashable: false, error: '2 images for HBA330 nvdata at 0e.01.00.07' },
} } }
const fo = offer(m, 'firmware')
assert.deepEqual(fo.image, { version: '16.17.01.00', file: 'hba330-16.17.01.00.bin', sha256: sha, source: 'https://dl.example/hba330.bin', golden: 'hba330-p16' })
assert.equal(offer(m, 'bios').image, null)
assert.deepEqual(offers(m).map((o) => o.region), ['firmware', 'nvdata'], 'only regions with an entry or an error')
assert.equal(offerText(fo), '16.17.01.00 (now 16.17.00.03)')
assert.match(offerText(offers(m)[1]), /^not offered: 2 images/)
assert.match(offerText({ region: 'nvdata', current: 'a', image: { version: 'b' }, flashable: false }), /inside the firmware image/)
// Is it in the node's store?
assert.deepEqual(storeHolds(m, fo.image, [{ node: 'forge1', name: 'copy.bin', sha256: sha }]), { held: true, text: 'In forge1’s firmware store as copy.bin, the same sha256.' })
assert.match(storeHolds(m, fo.image, [{ node: 'forge1', name: 'hba330-16.17.01.00.bin', sha256: 'cd'.repeat(32) }]).text, /its sha256 is cdcd.*not the manifest’s/)
assert.match(storeHolds(m, fo.image, [{ node: 'forge2', name: 'x', sha256: sha }]).text, /^Not in forge1’s firmware store: the node reads it beside the manifest/)
assert.equal(storeHolds(m, null, []).held, false)
// The manifest's request: only the pinned sha256.
const mb = manifestBody('firmware', fo.image, false)
assert.deepEqual(mb, { region: 'firmware', image: { manifest: true, sha256: sha }, dryRun: false })
assert.equal(requestWhy(mb), '')
assert.match(requestWhy(manifestBody('bios', null, true)), /offers this card no image/)
assert.match(requestWhy({ region: 'firmware', image: { manifest: true, sha256: sha, source: 'https://x' } }), /names its own/)
// An operation's image: the node's record for a manifest flash, the typed golden otherwise.
assert.deepEqual(opImage({ image: { manifest: true, sha256: sha }, manifest: { file: 'f.bin', sha256: sha, source: 'https://s', version: '1' } }), { name: 'f.bin', sha256: sha, source: 'https://s', manifest: true, version: '1' })
assert.equal(opImage({ image: { manifest: true, sha256: sha } }).name, 'the release manifest’s entry', 'before the node planned it')
assert.equal(opImage({ image: { name: 'a', sha256: 'b', source: 'c' } }).source, 'c')

console.log('hba.test.mjs: ok')
