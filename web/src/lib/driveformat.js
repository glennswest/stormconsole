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
