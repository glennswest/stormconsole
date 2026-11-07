// Replacing a drive (#317). Pure, so it runs under node.
//
// What a replace does depends on what the old drive holds, and each case is
// done by what holds it, never by the console:
//
// - a RAID member: the engine's own replace (stormblock `POST
//   /api/v1/arrays/{id}/members/{slot}/replace`, #256). An active member is
//   copied onto the new drive while it keeps serving: every write goes to both,
//   and a read that fails on the old drive turns the copy into a rebuild from
//   parity, so the set never runs one copy short. The new drive then takes the
//   slot, and the old one's superblock is wiped. A failed slot is rebuilt.
// - a slab member: the new drive joins the fleet, then stormdrive drains the
//   old one through the engine and takes it out of the fleet (`drain?leave=true`).
//   The engine puts the slabs where it has room, the new drive or another
//   (stormblock#563 asks for a drain onto a named drive).
// - the system drive: refused. The engine will not drain the system disk or the
//   volume metadata (stormblock#563).
// - a free drive: nothing to move. The console records it (stormdrive#127).
//
// `d` is a drivemap record (drivemap.js `build`); `ctx` carries what else the
// page has read: `formats` (the drive plugin's format records, by component),
// `raid` (the sb plugin's `/raid` answer) and `components` (the feed).

import { formatText } from './driveformat.js'

/// What the old drive is, for a replace.
export function kindOf(d) {
  const u = d?.use || {}
  if (u.use === 'system') return 'system'
  if (d?.set || u.use === 'raid_member') return 'raid'
  if (u.in_use && (u.use === 'slab_member' || u.use === 'data')) return 'slab'
  if (u.in_use) return 'other'
  if (d && d.outOfFleet === false && d.c?.detail?.includes('in fleet')) return 'slab'
  return 'free'
}

export const KIND_WORDS = {
  raid: 'RAID member',
  slab: 'Slab member',
  system: 'System drive',
  other: 'In use',
  free: 'Free',
}

const nameOf = (d) => d?.dev?.replace(/^\/dev\//, '') || d?.c?.label?.split(' · ')[0] || d?.serial || d?.id || 'the drive'

/// stormdrive's id for the drive: the last part of its component id.
export function stormdriveId(d) {
  const id = d?.id || ''
  const at = id.lastIndexOf(':drive:')
  return at >= 0 ? id.slice(at + ':drive:'.length) : id
}

/// The proxy a drive's stormdrive is reached by: this node's or another's.
export function proxyOf(d) {
  for (const a of d?.c?.actions || []) {
    const at = (a.path || '').indexOf('/api/v1/')
    if (at > 0) return a.path.slice(0, at)
  }
  return d?.local === false ? `/api/plugins/drive/node/${encodeURIComponent(d.node)}/proxy` : '/api/plugins/drive/proxy'
}

/// Every set the engine has, on shelves or not.
export function setsOf(raid) {
  return (raid?.shelves || []).flatMap((s) => s.sets || []).concat(raid?.arrays || [])
}

/// The set and member a drive is, by serial first (a file-backed drive's serial
/// is `file`, which names nothing), then by device.
export function memberOf(d, raid) {
  const good = d?.serial && d.serial !== 'file'
  for (const s of setsOf(raid)) {
    for (const m of s.members || []) {
      if ((good && m.serial === d.serial) || (d?.dev && m.path === d.dev)) return { set: s, member: m }
    }
  }
  return null
}

/// The blank drive the engine has open for this one, which is what it replaces onto.
export function engineDrive(d, raid) {
  const good = d?.serial && d.serial !== 'file'
  return (raid?.free || []).find((f) => (good && f.serial === d.serial) || (d?.dev && f.path === d.dev)) || null
}

/// A format record's format in comparable terms.
function fmt(rec) {
  const c = rec?.current
  if (!c && !rec?.blockSize) return null
  const blockSize = c?.blockSize ?? rec.blockSize
  const pi = c?.protType ?? (c?.protection === 'type1' ? 1 : c?.protection === 'none' ? 0 : null)
  return { blockSize, pi, protection: pi === 1 ? 'type1' : pi === 0 ? 'none' : null }
}
const fmtWords = (f) => (f.pi > 1 ? `${f.blockSize} + PI type ${f.pi}` : formatText(f.blockSize, f.protection))

const refused = (kind, why, extra = {}) => ({ kind, ok: false, why, steps: [], ...extra })

/// What replacing `old` by `neu` would do, or why it would not. `neu` may be
/// null: then only what the old drive allows is answered.
export function plan(old, neu, ctx = {}) {
  const kind = kindOf(old)
  const o = nameOf(old)
  if (kind === 'system') {
    return refused(kind, `${o} is the system drive: the engine cannot yet move the system slab, the volume metadata and the ESP onto another drive (stormblock#563). Nothing is changed.`)
  }
  if (kind === 'other') {
    return refused(kind, `${old.use?.reason || `${o} is in use`}: it is held by something outside the engine, so nothing here can move it.`)
  }
  if (old.drain?.state === 'running' || old.drain?.state === 'pending') return refused(kind, `${o} is being drained already`)
  if (!neu) return { kind, ok: true, why: '', steps: [] }
  const n = nameOf(neu)
  if (neu.id === old.id) return refused(kind, 'a drive cannot replace itself')
  if (neu.node !== old.node) return refused(kind, `${n} is on ${neu.node} and ${o} on ${old.node}: a replacement goes in the same node`)
  if (neu.use?.in_use) return refused(kind, `${n} is in use (${(neu.use.badge || 'in use').toLowerCase()}): pick a free drive`)
  const nrec = ctx.formats?.get?.(neu.id)
  if (nrec?.format?.state === 'running') return refused(kind, `${n} is being formatted`)

  if (kind === 'free') {
    return {
      kind,
      ok: true,
      why: '',
      call: { method: 'POST', path: '/api/plugins/drive/replace', body: { node: old.node, old: stormdriveId(old), new: stormdriveId(neu) } },
      steps: [`Nothing on ${o} to move.`, `${o} is recorded as replaced by ${n} in the console's audit (stormdrive does not keep it yet, stormdrive#127). Then ${o} can be pulled.`],
    }
  }

  if (neu.capacity && old.capacity && neu.capacity < old.capacity) {
    return refused(kind, `${n} is smaller than ${o}`)
  }

  if (kind === 'raid') {
    if (!old.local) return refused(kind, `a RAID set is replaced from its own node's console: open ${old.node}'s`)
    const at = memberOf(old, ctx.raid)
    if (!at) return refused(kind, `${o} is not a member this node's engine reports${ctx.raid ? '' : ' (its RAID sets have not been read)'}`)
    const { set, member } = at
    const setName = set.name || set.id
    if (set.status?.rebuild?.running) return refused(kind, `RAID set ${setName} is rebuilding already: replace ${o} once it is done`)
    if (!['active', 'failed', 'missing'].includes(member.state)) return refused(kind, `slot ${member.slot} of ${setName} is ${member.state}`)
    const e = engineDrive(neu, ctx.raid)
    if (!e) return refused(kind, `${n} is not a blank drive this node's engine has open, and the engine replaces onto those only`)
    const of = fmt(ctx.formats?.get?.(old.id))
    const nf = fmt(nrec)
    if (of && nf && (of.blockSize !== nf.blockSize || (of.pi !== null && nf.pi !== null && of.pi !== nf.pi))) {
      return refused(kind, `${n} is ${fmtWords(nf)}, and the set's ${o} is ${fmtWords(of)}: format ${n} to ${fmtWords(of)} first`, {
        format: { blockSize: of.blockSize, protection: of.protection },
      })
    }
    const action = (ctx.components || []).find((c) => c.id === member.component)?.actions?.find((a) => a.id === 'replace')
    if (!action) return refused(kind, `the engine offers you no replace for ${o}: replacing a set member needs storage-admin`)
    const live = member.state === 'active'
    return {
      kind,
      ok: true,
      why: '',
      live,
      set: setName,
      slot: member.slot,
      call: { method: 'POST', path: action.path, body: { drive_uuid: e.uuid }, confirm: true },
      steps: live
        ? [
            `${n} is free, big enough${nf ? ` and ${fmtWords(nf)}, as the set is` : ''}.`,
            `The engine copies slot ${member.slot} of ${setName} from ${o} onto ${n} while ${o} keeps serving. Every write goes to both. If a read on ${o} fails, the copy becomes a rebuild from parity, so the set never loses redundancy.`,
            `${n} takes slot ${member.slot}; ${o}'s superblock is wiped and it is released.`,
          ]
        : [
            `Slot ${member.slot} of ${setName} is ${member.state}: the set is already one drive short.`,
            `The engine rebuilds slot ${member.slot} onto ${n} from the other members.`,
          ],
      ...(of || nf ? {} : { note: 'The formats of these drives have not been read; the engine checks the size.' }),
    }
  }

  // A slab member.
  const steps = []
  let join = null
  if (neu.outOfFleet !== false || !(neu.c?.detail || '').includes('in fleet')) {
    join = (neu.c?.actions || []).find((a) => a.id === 'fleet-join') || null
    if (!join) return refused(kind, `${n} cannot join the fleet from here (stormdrive offers no Join on it)`)
    if (!join.enabled) return refused(kind, `${n} cannot join the fleet: ${join.why || 'stormdrive says not now'}`)
    steps.push(`${n} joins the fleet: stormdrive registers it with the engine.`)
  }
  const used = old.usage?.used
  steps.push(
    `The engine moves every slab off ${o}. Redundant volumes keep their other copies while their extents move. It puts them where it has room: ${n}, or another fleet drive (stormblock#563).`,
    `Once ${o} is empty it leaves the fleet and can be pulled.`,
  )
  return {
    kind,
    ok: true,
    why: '',
    join: join ? { method: join.method, path: join.path } : null,
    call: { method: 'POST', path: `${proxyOf(old)}/api/v1/drives/${encodeURIComponent(stormdriveId(old))}/drain?leave=true` },
    steps,
    ...(used && neu.capacity && neu.capacity < used ? { note: `${n} holds less than is on ${o}; the rest goes to other fleet drives.` } : {}),
  }
}

/// The drives `old` could be replaced by, each with whether it can and why
/// not: same node, free; same shelf first, then by bay.
export function candidates(old, records, ctx = {}) {
  return records
    .filter((d) => d.id !== old.id && d.node === old.node && !d.use?.in_use)
    .map((d) => ({ d, p: plan(old, d, ctx) }))
    .sort(
      (a, b) =>
        Number(b.d.shelfId === old.shelfId) - Number(a.d.shelfId === old.shelfId) ||
        Number(b.p.ok) - Number(a.p.ok) ||
        (a.d.bay ?? 1e9) - (b.d.bay ?? 1e9) ||
        String(a.d.dev).localeCompare(String(b.d.dev)),
    )
}

/// Why Replace… is off for this drive, or '' when it can be asked.
export function replaceWhy(d) {
  const p = plan(d, null)
  return p.ok ? '' : p.why
}

// --- progress, from what holds each replace --------------------------------

/// What a page keeps of a replace it asked for, so the rows can say what was
/// asked; the state is read again from the engine and stormdrive.
export function asked(p, old, neu, at) {
  return { kind: p.kind, node: old.node, set: p.set ?? null, slot: p.slot ?? null, live: !!p.live, at,
    old: { id: old.id, name: nameOf(old), serial: old.serial }, neu: { id: neu.id, name: nameOf(neu), serial: neu.serial } }
}

/// A row for each replace asked for: where it is, from the engine (a set's
/// copy) or stormdrive (a drain), and the result read back.
export function rows(asks, { records = [], raid = null } = {}) {
  const byId = new Map(records.map((r) => [r.id, r]))
  return asks.map((a) => {
    const base = { ...a, percent: null, state: 'running', text: '', back: '' }
    if (a.kind === 'free') return { ...base, state: 'done', text: 'nothing to move', back: `${a.old.name} recorded as replaced by ${a.neu.name}` }
    if (a.kind === 'raid') {
      const s = setsOf(raid).find((x) => (x.name || x.id) === a.set)
      if (!s) return { ...base, state: 'unknown', text: raid ? `RAID set ${a.set} is not reported` : 'reading the RAID sets…' }
      const m = (s.members || []).find((x) => x.slot === a.slot)
      const r = s.status?.rebuild
      if (r?.running && (r.slots || []).includes(a.slot)) {
        return { ...base, percent: Math.round(r.percent ?? 0), text: `${a.live ? 'copying' : 'rebuilding'} slot ${a.slot} of ${a.set}` }
      }
      if (m && m.serial && m.serial === a.neu.serial && m.state === 'active') {
        return { ...base, state: 'done', percent: 100, text: `slot ${a.slot} of ${a.set}`, back: `slot ${a.slot} holds ${a.neu.name} (${a.neu.serial}), active; set ${s.status?.state || 'state not said'}` }
      }
      if (r?.error) return { ...base, state: 'failed', text: `slot ${a.slot} of ${a.set}`, back: r.error }
      return { ...base, state: 'failed', text: `slot ${a.slot} of ${a.set}`, back: `slot ${a.slot} holds ${m?.serial || 'no drive'} (${m?.state || 'unknown'}), not ${a.neu.name}: the engine stopped the replace` }
    }
    const d = byId.get(a.old.id)
    const dr = d?.drain
    if (!d) return { ...base, state: 'done', text: `${a.old.name} is gone`, back: `${a.old.name} is no longer reported: pulled` }
    if (!dr) return { ...base, state: 'unknown', text: 'no drain is recorded on it yet' }
    const total = (dr.moved || 0) + (dr.remaining || 0) + (dr.failed || 0)
    const percent = total ? Math.round(((dr.moved || 0) / total) * 100) : null
    if (dr.state === 'running' || dr.state === 'pending') {
      return { ...base, percent, text: `draining ${a.old.name}: ${dr.moved || 0} moved, ${dr.remaining || 0} left${dr.state === 'pending' ? ' (waiting for the engine)' : ''}` }
    }
    if (dr.state === 'empty') {
      const out = d.outOfFleet
      return { ...base, state: 'done', percent: 100, text: `drained ${a.old.name}`, back: `${a.old.name} is empty (${dr.moved || 0} moved)${out ? ' and out of the fleet: it can be pulled' : ', leaving the fleet'}` }
    }
    return { ...base, state: 'failed', percent, text: `drain of ${a.old.name} ${dr.state}`, back: (dr.errors || []).join('; ') || `${dr.failed || 0} failed` }
  })
}
