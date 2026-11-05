// What the engine found on each drive it holds (stormblock#154), and what the
// Drives page says about it. Pure, so plain node can test it.
//
// The owner, 2026-10-13: "on auto format, no, since we may be doing a test, and
// a power off could destroy a drive". A drive is ours (adopted), blank or
// foreign; the last two are offers, and only a person takes one up.

const metric = (c, label) => (c.metrics || []).find((m) => m.label === label)?.value

function sentence(state, holds, slabs, volumes) {
  if (state === 'blank') return 'blank, nothing found on it'
  if (state === 'foreign') return `holds ${holds || 'something that is not ours'}`
  if (state === 'ours') return `ours: ${slabs ?? '?'} slab(s), ${volumes ?? '?'} volume(s), adopted`
  return ''
}

/// Every drive the engine reports what it found on. A drive opened at start,
/// or an older engine, says nothing, and nothing is guessed for it.
export function engineDrives(components) {
  const out = []
  for (const c of components || []) {
    if (!c.id?.startsWith('sb:drive:')) continue
    const state = metric(c, 'offer')
    if (!state) continue
    const holds = metric(c, 'holds')
    const slabs = metric(c, 'slabs')
    const volumes = metric(c, 'volumes')
    out.push({
      id: c.id,
      label: c.label,
      serial: metric(c, 'serial') || '',
      path: metric(c, 'path') || '',
      model: metric(c, 'model') || '',
      capacity: metric(c, 'capacity') || '',
      state,
      holds: holds || '',
      offered: state === 'blank' || state === 'foreign',
      sentence: sentence(state, holds, slabs, volumes),
      // The word a format needs: the engine's, never worked out here.
      confirm: metric(c, 'confirm') || '',
      // Withheld from anyone the storage guard refuses (#82).
      action: (c.actions || []).find((a) => a.id === 'format') || null,
    })
  }
  return out
}

/// One of stormdrive's drives, on this node, in the engine's list: by serial,
/// which is the drive's own, then by device path.
export function offerFor(offers, { serial, dev }) {
  return (serial && offers.find((o) => o.serial === serial)) || (dev && offers.find((o) => o.path === dev)) || null
}

/// The 412's text for a person: the risk, without the sentence telling an API
/// client what to send.
export function riskText(error) {
  return (error || '').replace(/\s*Nothing formats a drive on its own[\s\S]*$/, '').trim()
}

function bytes(n) {
  const u = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB']
  let v = n || 0
  let i = 0
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024
    i++
  }
  return `${i ? v.toFixed(1) : v} ${u[i]}`
}

/// What a dry run planned, in a sentence.
export function planSentence(plan) {
  if (!plan) return ''
  const stay = plan.left ? ` ${plan.left} extent(s) stay where they are: their legs must stay apart, they have parity, or they are kept on one drive.` : ''
  if (!plan.moves) return `Nothing to move: the slabs are as even as they can be.${stay}`
  return `${plan.moves} extent(s) would move, ${bytes(plan.bytes)} in all.${stay}`
}

/// How a rebalance is going, and whether it is still going.
export function statusLine(s) {
  if (!s) return { running: false, line: '' }
  const running = String(s.state).toLowerCase() === 'running'
  const extra = [s.skipped ? `${s.skipped} left where they were` : '', s.failed ? `${s.failed} failed` : ''].filter(Boolean).join(', ')
  return {
    running,
    failed: !!s.failed,
    line: `Rebalance ${String(s.state).toLowerCase()}: ${s.moved} of ${s.planned} moved, ${bytes(s.bytes)}${extra ? `, ${extra}` : ''}`,
  }
}
