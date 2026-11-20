// SAS cards and their flash (#376, stormdrive#166). Pure, so it runs under node.
//
// The k8s plugin's `GET /api/plugins/k8s/hbas` is every `HostBusAdapter` this
// viewer may read, as the apiserver gives it (stormdrive card.rs `object`),
// each with its `CardOperation`s newest first. A flash is `POST
// /api/plugins/k8s/hbas/<name>/flash`, a storage-admin's, with the card's name
// typed; the node's own checks come back on the operation's status.

/// A drive's component id → stormdrive's drive id (`drive:drive:<id>` here,
/// `drive:@<host>:drive:<id>` on another node).
export function driveIdOf(component) {
  const m = String(component || '').match(/^drive:(?:@[^:]+:)?drive:(.+)$/)
  return m ? m[1] : null
}

/// Mb/s as the page says it: "12 Gbit", "1.5 Gbit", "22.5 Gbit".
export function gbit(mbps) {
  if (mbps === null || mbps === undefined || mbps === '') return null
  const g = Number(mbps) / 1000
  return Number.isFinite(g) ? `${Number(g.toFixed(1))} Gbit` : null
}

/// A phy's rate, against what the card is set to allow.
export function rateText(p) {
  if (p.disabled) return 'disabled'
  if (p.rateMbps) return gbit(p.rateMbps)
  const r = p.rate || 'unknown'
  return r === 'unknown' ? 'no link' : r
}

/// What the card is set to on a phy (SAS IO Unit Page 1): "1.5–12 Gbit".
export function limitText(p) {
  const lo = gbit(p.minRateMbps)
  const hi = gbit(p.maxRateMbps)
  if (!lo && !hi) return '—'
  return `${lo ? lo.replace(' Gbit', '') : '?'}–${hi || '?'}`
}

/// Is a phy running below what the card allows? (A drive or a cable held back.)
export function slow(p) {
  return !!(p.rateMbps && p.maxRateMbps && p.rateMbps < p.maxRateMbps)
}

/// Drive id → where it hangs off a card: `{card, node, board, phy, rateMbps, maxRateMbps}`.
export function drivesOnCards(cards) {
  const out = new Map()
  for (const c of cards || []) {
    const st = c.status || {}
    for (const p of st.phys || []) {
      const d = p.attached?.drive
      if (!d || out.has(d)) continue
      out.set(d, { card: c.name, node: st.node, board: st.board || st.adapter || null, phy: p.phy, rateMbps: p.rateMbps ?? null, maxRateMbps: p.maxRateMbps ?? null })
    }
    // A drive behind an expander has no phy of the card's own: the device list still names it.
    for (const d of st.devices || []) {
      if (!d.drive || out.has(d.drive)) continue
      out.set(d.drive, { card: c.name, node: st.node, board: st.board || st.adapter || null, phy: null, slot: d.slot ?? null, rateMbps: null, maxRateMbps: null })
    }
  }
  return out
}

/// The cards grouped by node, in order.
export function byNode(cards) {
  const m = new Map()
  for (const c of cards || []) {
    const n = c.status?.node || 'no node'
    if (!m.has(n)) m.set(n, [])
    m.get(n).push(c)
  }
  return [...m.entries()].sort((a, b) => a[0].localeCompare(b[0])).map(([node, cards]) => ({ node, cards }))
}

const RUNNING = new Set(['Running', 'Pending', ''])

/// Is the operation still going (nothing has ended it)?
export function running(op) {
  return RUNNING.has(op?.phase || '') && !op?.finished
}

/// An operation's progress: `{fraction, text}` while it writes, else null.
export function progress(op) {
  if (op?.phase !== 'Running') return null
  const done = Number(op.bytesDone) || 0
  const total = Number(op.bytesTotal) || 0
  const step = op.step || 'download'
  if (!total) return { fraction: null, text: step }
  const f = Math.max(0, Math.min(1, done / total))
  return { fraction: f, text: `${step} ${Math.round(f * 100)}% (${done} of ${total} bytes)` }
}

/// Each phase, said: what it means for the card.
export function phaseText(op) {
  switch (op?.phase) {
    case '':
    case undefined:
    case null:
      return 'waiting for the node'
    case 'Pending':
      return 'waiting: the node cannot re-check who asked yet'
    case 'Planned':
      return 'dry run: the image and the guards pass, nothing written'
    case 'Running':
      return 'flashing'
    case 'Succeeded':
      return 'flashed, the card reset and running it'
    case 'Written':
      return 'written and read back; it runs at the card’s next reset (no moment without I/O came)'
    case 'Refused':
      return 'refused'
    case 'Failed':
      return 'failed: the card keeps running what it ran'
    case 'Interrupted':
      return 'interrupted: stormdrive stopped during the flash; read the regions and flash again'
    default:
      return String(op.phase)
  }
}

/// A phase's tone.
export function phaseTone(op) {
  switch (op?.phase) {
    case 'Succeeded':
    case 'Planned':
      return 'ok'
    case 'Written':
    case 'Pending':
      return 'warn'
    case 'Refused':
    case 'Failed':
    case 'Interrupted':
      return 'error'
    default:
      return 'accent'
  }
}

/// "16.17.00.03 → 16.17.01.00", or what is known of it.
export function versions(op) {
  const b = op?.before || null
  const a = op?.after || null
  if (b && a) return b === a ? `${a} (same version)` : `${b} → ${a}`
  if (b && op?.imageVersion) return `${b} → ${op.imageVersion}${op.phase === 'Running' ? '' : ' (image)'}`
  return b || a || op?.imageVersion || '—'
}

/// The version a region runs now.
export function current(card, region) {
  const st = card?.status || {}
  return region === 'bios' ? st.biosVersion || null : st.firmwareVersion || null
}

/// Why Flash is not offered on this card, or ''. `guard` is the console's
/// answer for the path (`{allowed, reason}`), when read.
export function flashWhy(card, guard) {
  const st = card?.status || {}
  if (st.present === false) return `not seen on ${st.node || 'its node'} now`
  if (!st.node) return 'stormdrive has not written its status'
  const going = (card.operations || []).find(running)
  if (going) return `${going.name} is ${going.phase ? going.phase.toLowerCase() : 'waiting for the node'}: one flash at a time`
  if (guard && !guard.allowed) return guard.reason || 'only a storage-admin may'
  return ''
}

/// The images a flash of this card can name: the firmware store of the card's
/// node (the drive plugin's `/firmware` answer), newest name first.
export function storeImages(card, images) {
  const node = card?.status?.node
  return (images || [])
    .filter((i) => i.node === node && i.sha256)
    .map((i) => ({ name: i.name, size: i.size ?? null, sha256: String(i.sha256).toLowerCase() }))
    .sort((a, b) => String(b.name).localeCompare(String(a.name)))
}

/// Why this flash request cannot go, or ''. As the server checks it.
export function requestWhy(req) {
  if (!['firmware', 'bios'].includes(req?.region)) return 'choose firmware or bios'
  if (!req.image?.name) return 'choose an image from the node’s firmware store'
  if (!/^[0-9a-f]{64}$/i.test(String(req.image.sha256 || '').trim())) return 'the image’s sha256 is not 64 hex digits'
  const src = String(req.image.source || '').trim()
  if (!/^https?:\/\/\S{3,}$/.test(src)) return 'name the http(s) URL the image was recorded from'
  return ''
}

/// The request body.
export function body(region, image, source, dryRun) {
  return { region, image: { name: image?.name || '', sha256: image?.sha256 || '', source: String(source || '').trim() }, dryRun: !!dryRun }
}

/// Does the page need to read again soon (an operation of anybody's is going)?
export function busy(cards) {
  return (cards || []).some((c) => (c.operations || []).some(running))
}
