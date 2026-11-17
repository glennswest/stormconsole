// A drive's SAS link (#339, stormdrive#149), apart from the page so plain node
// can test it (drivelink.test.mjs).
//
// stormdrive samples each SAS drive's link once a minute, both sides — the
// drive's own phy counters (log page 0x18) and the host phy facing it (the
// HBA's, or a shelf expander's) — with a rate per minute and a level:
// `settling` (first sample after a reset), `ok`, `degraded` (anything moving),
// `failing` (loss of sync or phy resets moving two samples running). The
// lifetime totals are history (a NetApp shelf's years, saturated at 2^32−1);
// the rate is what says "now".

export const COUNTERS = [
  ['invalidDwords', 'Invalid DWORDs'],
  ['disparityErrors', 'Disparity errors'],
  ['lossOfSync', 'Loss of DWORD sync'],
  ['phyResets', 'Phy reset problems'],
]

const SATURATED = 4294967295

/// The level a drive's link is at: its Drive object's, else the feed's word
/// (`link errors`: degraded or failing; ok says nothing there).
export function linkLevel(row, feedWord) {
  const l = row?.link?.level
  if (l) return l
  if (feedWord === 'degraded' || feedWord === 'failing') return feedWord
  return null
}

/// 12000 → "12 Gbit", 1500 → "1.5 Gbit".
export function gbit(mbps) {
  if (mbps === null || mbps === undefined) return '—'
  return mbps % 1000 === 0 ? `${mbps / 1000} Gbit` : `${(mbps / 1000).toFixed(1)} Gbit`
}

/// A lifetime total: a saturated counter says so.
export function total(n) {
  if (n === null || n === undefined) return '—'
  if (n >= SATURATED) return 'saturated'
  return Number(n).toLocaleString('en-US')
}

/// A rate per minute: nothing moving reads 0, a fraction keeps a digit.
export function perMin(v) {
  if (v === null || v === undefined) return '—'
  if (v === 0) return '0'
  return v >= 10 ? `${Math.round(v).toLocaleString('en-US')}/min` : `${v.toFixed(1)}/min`
}

/// The negotiated rate against the maximum, as the panel's first line.
export function rateLine(link) {
  if (!link) return ''
  const neg = gbit(link.negotiatedMbps)
  const max = link.hostMaximumHwMbps ?? link.hostMaximumMbps
  // stormdrive's words for the maximum, e.g. "maximum 6 Gbit (policy)".
  const said = link.maximum || (max ? `maximum ${gbit(max)}` : '')
  const below = link.negotiatedMbps && max && link.negotiatedMbps < max
  return `${neg}${said ? ` of ${said.replace(/^maximum /, '')} maximum` : ''}${below ? ' — below the maximum' : ''}`
}

/// The panel's table: one row per side and phy, each counter's total and rate.
export function counterRows(row) {
  const link = row?.link
  if (!link) return []
  const out = []
  const host = row.hostSide || 'host'
  for (const [key, side] of [['drive', 'drive'], ['host', host]]) {
    const s = link[key]
    if (!s) continue
    for (const p of s.phys || []) {
      const r = (s.rates || []).find((x) => x.phy === p.phy) || null
      out.push({
        side,
        phy: p.phy,
        rate: p.rateMbps ?? null,
        counters: COUNTERS.map(([k, word]) => ({
          key: k,
          word,
          total: p[k],
          rate: r ? r[k] : null,
          moving: !!r && r[k] > 0,
        })),
      })
    }
  }
  return out
}

/// What the level means, for the panel and the bay's tooltip.
export function levelWords(level) {
  return (
    {
      ok: 'ok: nothing moving',
      settling: 'settling: the first sample since a reset, no rate yet',
      degraded: 'degraded: errors moving',
      failing: 'failing: loss of sync or phy resets moving every minute',
    }[level] || 'not reported'
  )
}

/// The tone a level is shown in: ok says nothing.
export function levelTone(level) {
  return { degraded: 'warn', failing: 'error' }[level] || ''
}

/// The trend when ironprom has none: the console's own last samples of the
/// loss-of-sync rate, one line per side, in LineChart's shape.
export function trendFromHistory(history, hostSide = 'host') {
  const pts = history || []
  if (!pts.length) return []
  const line = (k, name, slot) => ({ name, slot, points: pts.map((p) => ({ t: p.at * 1000, v: p[k] ?? null })) })
  return [line('drive', 'drive', 0), line('host', hostSide, 1)].filter((s) => s.points.some((p) => p.v !== null))
}

/// The drive's link alarms in the feed (#339), for the alarm bar: failing
/// first, each with its text and the link to the drive.
export function linkAlarms(components) {
  const rank = { error: 0, warn: 1 }
  return components
    .filter((c) => c.kind === 'alarm' && c.id.includes(':alarm:link:'))
    .sort((a, b) => (rank[a.health] ?? 2) - (rank[b.health] ?? 2) || a.detail.localeCompare(b.detail))
    .map((c) => ({ id: c.id, text: c.detail, href: c.link || '#/drives', tone: c.health === 'error' ? 'error' : 'warn' }))
}

/// The worst loss-of-sync rate of the last sample, either side, or null.
export function worstSync(link) {
  const rates = [...(link?.drive?.rates || []), ...(link?.host?.rates || [])]
  if (!rates.length) return null
  return Math.max(...rates.map((r) => (r.lossOfSync || 0) + (r.phyResets || 0)))
}

/// The list's word for a drive's link: its level with the rate, toned; null
/// when it is ok (ok says nothing) or not reported.
export function listMetric(level, link) {
  if (level !== 'degraded' && level !== 'failing') return null
  const r = worstSync(link)
  return { label: 'link errors', value: r ? `${level} · ${perMin(r)} sync` : level, tone: levelTone(level) }
}
