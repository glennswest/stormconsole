// A shelf's RAID sets as the page shows them (#67). The engine's words, sorted for a person: what is
// wrong comes first, and a failed member has to be findable by its bay.

export const HEALTH = { clean: 'ok', rebuilding: 'warn', degraded: 'warn', failed: 'error' }

const RANK = { failed: 0, degraded: 1, rebuilding: 2, clean: 3 }

export function stateWord(s) {
  return s ? s[0].toUpperCase() + s.slice(1) : 'Unknown'
}

/// Worst first, then by name.
export function ordered(sets) {
  return [...sets].sort(
    (a, b) => (RANK[a.status?.state] ?? 4) - (RANK[b.status?.state] ?? 4) || String(a.name).localeCompare(String(b.name))
  )
}

/// Where a member is, as far as anything has said. A failed member with no drive here still has its slot.
export function where(m) {
  if (m.bay != null) return `bay ${m.bay}`
  return `slot ${m.slot}`
}

/// What identifies the drive to pull.
export function driveWords(m) {
  const serial = m.serial && m.serial !== 'file' ? m.serial : ''
  return serial || m.path || 'drive not here'
}

export function needsHands(m) {
  return m.state === 'failed' || m.state === 'missing'
}

function bytes(n) {
  if (!n) return ''
  const units = ['B/s', 'KiB/s', 'MiB/s', 'GiB/s']
  let i = 0
  while (n >= 1024 && i < units.length - 1) {
    n /= 1024
    i++
  }
  return `${n >= 10 || i === 0 ? Math.round(n) : n.toFixed(1)} ${units[i]}`
}

/// Rebuild and scrub, each a line with a percent, only while there is something to say.
export function progress(status) {
  const out = []
  const r = status?.rebuild
  if (r && (r.running || r.error)) {
    const rate = bytes(r.rate_bytes_per_sec)
    out.push({
      kind: 'rebuild',
      percent: r.percent ?? 0,
      error: r.error || '',
      text: `Rebuilding slot${(r.slots || []).length === 1 ? '' : 's'} ${(r.slots || []).join(', ')}${rate ? ` at ${rate}` : ''}`,
    })
  }
  const s = status?.scrub
  if (s) {
    const found = s.mismatches ? `${s.mismatches} mismatch${s.mismatches === 1 ? '' : 'es'}${s.repaired ? `, ${s.repaired} repaired` : ''}` : 'no mismatches'
    out.push({
      kind: 'scrub',
      percent: s.percent ?? 0,
      error: s.error || '',
      bad: s.mismatches > 0,
      text: s.running ? `Scrubbing, ${found} so far` : `Last scrub: ${found}`,
    })
  }
  return out
}

/// What the feed offers this viewer on a member: withheld from anyone the storage guard refuses (#82).
export function memberActions(components, m) {
  const c = components.find((x) => x.id === m.component)
  const of = (id) => (c?.actions || []).find((a) => a.id === id) || null
  return { fail: of('fail'), replace: of('replace') }
}

export function canScrub(status) {
  return !status?.scrub?.running && !status?.rebuild?.running && status?.state !== 'failed'
}
