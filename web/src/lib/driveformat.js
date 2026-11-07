// The format page's model (#165). Pure, so it runs under node. The records
// are stormdrive's, from GET /api/plugins/drive/formats; `format` is its
// persisted `status.format`, in unix seconds.

/// "4096 + PI type 1", "520, no PI".
export function formatText(blockSize, protection) {
  if (!blockSize) return 'unknown'
  if (protection === 'type1') return `${blockSize} + PI type 1`
  if (protection === 'none') return `${blockSize}, no PI`
  return `${blockSize}`
}

/// What a drive is formatted as now. A PI type READ CAPACITY reports beyond 1
/// is said by number: stormdrive cannot format to it, and it is still true.
export function currentText(rec) {
  const c = rec.current
  const size = c?.blockSize ?? rec.blockSize
  if (c?.protType > 1) return `${size} + PI type ${c.protType}`
  return formatText(size, c?.protection)
}

/// What the choices on the page format this drive as. `pi` is 'offered' (PI
/// type 1 where the drive has it, else none) or 'none'. stormdrive falls back
/// only from a format equal to its plan; any other is exact, and a refusal
/// ends it, so `fallbacks` is what this run would really do.
export function chosen(rec, blockSize, pi) {
  const p = rec.planned
  const planned = p && !p.error ? p : null
  let protection = 'none'
  if (pi === 'offered') {
    if (planned?.blockSize === blockSize) protection = planned.protection
    else if (rec.offersPi) protection = 'type1'
  }
  const onPlan = planned?.blockSize === blockSize && planned?.protection === protection
  return {
    block_size: blockSize,
    protection,
    basis: onPlan ? planned.basis || '' : '',
    fallbacks: onPlan ? planned.fallbacks || [] : [],
  }
}

export const isRunning = (rec) => rec.format?.state === 'running'
export const selectable = (rec) => !rec.whyNot && !isRunning(rec)

/// "14 h 3 m", "3 m 20 s", "45 s".
export function duration(seconds) {
  if (seconds === null || seconds === undefined || seconds < 0) return '—'
  const s = Math.floor(seconds)
  if (s >= 86400) return `${Math.floor(s / 86400)} d ${Math.floor((s % 86400) / 3600)} h`
  if (s >= 3600) return `${Math.floor(s / 3600)} h ${Math.floor((s % 3600) / 60)} m`
  if (s >= 60) return `${Math.floor(s / 60)} m ${s % 60} s`
  return `${s} s`
}

/// The clock of a run, at `now` (unix seconds). The estimate was taken at
/// `progressAtUnix`, so it counts down between the drive's own reports, and
/// `null` means there is none yet (stormdrive gives none under 0.1%).
export function timing(f, now) {
  const end = f.finishedUnix ?? now
  const elapsed = f.startedUnix ? Math.max(0, end - f.startedUnix) : null
  let left = null
  if (f.state === 'running' && f.etaSeconds !== null && f.etaSeconds !== undefined) {
    left = Math.max(0, f.etaSeconds - Math.max(0, now - (f.progressAtUnix ?? now)))
  }
  return { elapsed, left }
}

/// What the drive reads after the run. `asked` is what the run ended up
/// formatting (a refusal changes it), and `matches` is false when the drive
/// took the format and reads back something else, which stormdrive fails.
export function readBack(f) {
  if (f.blockSizeAfter === null || f.blockSizeAfter === undefined) return null
  const type = f.protTypeAfter ?? 0
  const text = type ? `${f.blockSizeAfter} + PI type ${type}` : `${f.blockSizeAfter}, no PI`
  const wanted = f.protection === 'type1' ? 1 : 0
  return { text, matches: f.blockSizeAfter === f.toBlockSize && type === wanted }
}

/// The phrase that confirms a format; the server checks the same one.
export function phrase(records) {
  if (records.length === 1 && records[0].serial) return records[0].serial
  return `format ${records.length} drives`
}

/// The request's body.
export function request(records, blockSize, pi, confirm) {
  return {
    confirm,
    drives: records.map((r) => {
      const c = chosen(r, blockSize, pi)
      return { node: r.node, id: r.id, block_size: c.block_size, protection: c.protection }
    }),
  }
}

/// Drives by node, then bay, then name: the order they sit in.
export function byNode(records) {
  const nodes = new Map()
  for (const r of records) {
    if (!nodes.has(r.node)) nodes.set(r.node, [])
    nodes.get(r.node).push(r)
  }
  for (const list of nodes.values()) {
    list.sort((a, b) => (a.bay ?? 1e9) - (b.bay ?? 1e9) || String(a.name).localeCompare(String(b.name)))
  }
  return [...nodes.entries()].map(([node, drives]) => ({ node, drives }))
}

/// Finished runs, newest first; running ones are shown apart.
export function finished(records) {
  return records
    .filter((r) => r.format && r.format.state !== 'running')
    .sort((a, b) => (b.format.finishedUnix ?? 0) - (a.format.finishedUnix ?? 0))
}

// --- #308: in use, the dialog's choices, progress per drive ---------------

/// The badge a drive's use earns ('System', 'Data slab', 'RAID member', …), or
/// null for a free one. `use` is the drive plugin's answer (inuse.rs).
export const badgeOf = (rec) => (rec?.use?.in_use ? rec.use.badge || 'In use' : null)
export const inUse = (rec) => !!rec?.use?.in_use

/// "4096 + T10 PI type 1", the confirm step's words.
export function resultText(blockSize, protection) {
  return protection === 'type1' ? `${blockSize} + T10 PI type 1` : `${blockSize}, no PI`
}

const key2 = (bs, p) => `${bs}:${p}`
export const optionKey = (o) => key2(o.block_size, o.protection)
export function parseKey(k) {
  const [bs, p] = String(k).split(':')
  return { block_size: Number(bs), protection: p }
}

/// What this drive can be formatted to, best first (stormdrive's order:
/// 4096 + PI 1, 4096, 512 + PI 1, 512). From `supports` (pi.rs): the listed
/// block lengths when VPD 0xB4 lists them, else 512, and 4096 as a try where
/// the physical block is 4K. stormdrive formats PI type 1 or none, so a drive
/// listing type 2 or 3 gets a note, not an option.
export function options(rec) {
  const s = rec.supports || null
  const pis = s?.pi_types || (rec.offersPi ? [1] : [])
  const out = []
  const add = (bs, p, basis) => {
    if (!out.some((o) => o.block_size === bs && o.protection === p)) out.push({ block_size: bs, protection: p, basis })
  }
  const listed = (s?.block_lengths || []).filter((l) => l.length === 512 || l.length === 4096)
  if (listed.length) {
    for (const bs of [4096, 512]) {
      const l = listed.find((x) => x.length === bs)
      if (!l) continue
      if ((l.pi_types || []).includes(1)) add(bs, 'type1', 'listed')
      if (!l.pi_types?.length || l.pi_types.includes(0)) add(bs, 'none', 'listed')
    }
  } else {
    const fourK = !s || s.physical_block_size === 4096 || rec.planned?.blockSize === 4096
    for (const bs of fourK ? [4096, 512] : [512]) {
      const basis = s && bs === 4096 ? 'try' : s ? 'offered' : 'unknown'
      if (pis.includes(1)) add(bs, 'type1', basis)
      add(bs, 'none', basis)
    }
  }
  const p = rec.planned && !rec.planned.error ? rec.planned : null
  if (p && !out.some((o) => o.block_size === p.blockSize && o.protection === p.protection)) out.unshift({ block_size: p.blockSize, protection: p.protection, basis: p.basis || 'plan' })
  const rank = (o) => (o.block_size === 4096 ? 0 : 2) + (o.protection === 'type1' ? 0 : 1)
  return out.sort((a, b) => rank(a) - rank(b))
}

/// The default: the drive's own plan (plannedFormat), else the best offered.
export function defaultOption(rec) {
  const p = rec.planned && !rec.planned.error ? rec.planned : null
  const opts = options(rec)
  return (p && opts.find((o) => o.block_size === p.blockSize && o.protection === p.protection)) || opts[0] || { block_size: 4096, protection: 'none' }
}

/// PI types the drive has that stormdrive does not format to.
export function piNote(rec) {
  const extra = (rec.supports?.pi_types || []).filter((t) => t > 1)
  if (!extra.length) return ''
  return `the drive also lists PI type ${extra.join(' and ')}; stormdrive formats type 1 or none`
}

/// The run stormdrive would make for a choice: its fallbacks only when the
/// choice is the drive's plan (stormdrive falls back from the plan alone).
export function planFor(rec, o) {
  const p = rec.planned && !rec.planned.error ? rec.planned : null
  const onPlan = p && p.blockSize === o.block_size && p.protection === o.protection
  return { ...o, onPlan: !!onPlan, fallbacks: onPlan ? p.fallbacks || [] : [] }
}

/// "sdb → 4096 + T10 PI type 1", as the confirm step and the owner name it.
export const confirmLine = (rec, o) => `${rec.name} → ${resultText(o.block_size, o.protection)}`

/// The request for a set of drives and the choice for each (by `node/id`).
export function requestFor(records, choice, confirm) {
  return {
    confirm,
    drives: records.map((r) => {
      const o = parseKey(choice[`${r.node}/${r.id}`] || optionKey(defaultOption(r)))
      return { node: r.node, id: r.id, block_size: o.block_size, protection: o.protection }
    }),
  }
}

/// Where a run is, in three steps: probe (stormdrive's prepare and queue),
/// format (FORMAT UNIT running) and read-back (rescan and READ CAPACITY).
export const STEPS = ['probe', 'format', 'read-back']
export function stepOf(f) {
  if (!f) return null
  if (f.state === 'done') return 'done'
  if (f.state && f.state !== 'running') return 'failed'
  switch (f.phase) {
    case 'format':
    case 'formatting':
      return 'format'
    case 'rescan':
    case 'verify':
      return 'read-back'
    default:
      return 'probe'
  }
}

/// The runs to show one row each for: running first, then the ones that ended
/// within `recent` seconds (or every ended one with `recent` null), newest first.
export function progressRows(records, now, recent = null) {
  const running = records.filter(isRunning)
  const ended = finished(records).filter((r) => recent === null || now - (r.format.finishedUnix ?? 0) <= recent)
  return [...running, ...ended]
}
