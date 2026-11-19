// A node's Network page (#336): its host ports from stormpump's NetworkPort objects
// (stormpump#236), apart from the page so plain node can test it.

/// The grid's rows.
export function portRows(ports) {
  return (ports || []).map((p) => ({
    id: p.name,
    name: p.name,
    health: p.health,
    ifname: p.ifname,
    link: p.state,
    speed: p.speed ?? null,
    speedText: p.speedWord,
    fec: p.fec?.active ?? null,
    fecText: p.fec?.active || '—',
    want: wantWord(p.fec),
    module: moduleWord(p.module),
    applied: appliedWords(p.applied, p.pending).word,
    mac: p.mac || '—',
  }))
}

/// What the port's FEC is held to, and by what.
export function wantWord(fec) {
  if (!fec || !fec.want) return 'unmanaged'
  const by = { spec: 'this object', port: "the node's port-fec.toml", table: "port-fec.toml's speed table" }[fec.source]
  return by ? `${fec.want} (${by})` : fec.want
}

export function moduleWord(m) {
  if (!m || !m.present) return m && m.present === false ? 'empty cage' : '—'
  return [m.part || m.kind, m.vendor].filter(Boolean).join(' · ') || 'a module'
}

/// mW → dBm, the unit optics are read in. Nothing for no light.
export function dbm(mw) {
  if (typeof mw !== 'number' || mw <= 0) return null
  return 10 * Math.log10(mw)
}

const perLane = (a) =>
  (Array.isArray(a) ? a : [a])
    .map((v, i) => (typeof v === 'number' ? `${a.length > 1 ? `lane ${i}: ` : ''}${v.toFixed(3)} mW (${dbm(v)?.toFixed(1) ?? '—'} dBm)` : null))
    .filter(Boolean)
    .join(', ')

/// The module's lines, as label and value. A BASE-T port has none (stormpump docs/ports.md).
export function moduleLines(m) {
  if (!m || !m.present) return []
  const out = []
  const add = (label, v) => {
    if (v !== null && v !== undefined && v !== '' && !(Array.isArray(v) && !v.length)) out.push([label, v])
  }
  add('Kind', m.kind)
  add('Vendor', m.vendor)
  add('Part', m.part)
  add('Serial', m.serial)
  add('Compliance', Array.isArray(m.compliance) ? m.compliance.join(', ') : m.compliance)
  add('Length', typeof m.lengthM === 'number' ? `${m.lengthM} m` : null)
  add('Temperature', typeof m.temperatureC === 'number' ? `${m.temperatureC.toFixed(1)} °C` : null)
  add('Rx power', m.rxPowerMw == null ? null : perLane(m.rxPowerMw) || null)
  add('Tx power', m.txPowerMw == null ? null : perLane(m.txPowerMw) || null)
  return out
}

/// What the Applied condition says, in a word and a tone (stormpump docs/ports.md's table).
export function appliedWords(a, pending) {
  if (pending) return { word: 'waiting for the node', tone: 'warn', message: 'the node has not read this change yet' }
  const reason = a?.reason
  const message = a?.message || ''
  switch (reason) {
    case 'Applied':
      return { word: 'applied', tone: 'ok', message }
    case 'NodeSetting':
      return { word: "node's setting", tone: 'ok', message }
    case 'Pending':
    case 'Applying':
      return { word: reason === 'Pending' ? 'waiting for the node' : 'applying', tone: 'warn', message }
    case 'NotSupported':
      return { word: 'partly applied', tone: 'warn', message }
    case 'Refused':
      return { word: 'refused', tone: 'bad', message }
    case 'Absent':
      return { word: 'port gone', tone: 'bad', message }
    default:
      return { word: '—', tone: 'dim', message }
  }
}

/// `rxCrcErrors` → `rx crc errors`, `fecCorrectedBlocks` → `fec corrected blocks`.
export function counterLabel(k) {
  return k.replace(/([A-Z])/g, ' $1').toLowerCase()
}

const ORDER = ['rx', 'tx', 'fec', 'pause']
const BAD = /error|drop|missed|uncorrectable/i

/// The counters as rows, each with its rate since the last read when there was one.
export function counterRows(counters, prev, secs) {
  const rows = Object.entries(counters || {})
    .filter(([, v]) => typeof v === 'number')
    .map(([k, v]) => {
      const before = prev?.[k]
      const rate = typeof before === 'number' && secs > 0 && v >= before ? (v - before) / secs : null
      return { key: k, label: counterLabel(k), value: v, rate, bad: BAD.test(k) && v > 0 }
    })
  const rank = (k) => {
    const i = ORDER.findIndex((p) => k.startsWith(p))
    return i < 0 ? ORDER.length : i
  }
  return rows.sort((a, b) => rank(a.key) - rank(b.key) || a.key.localeCompare(b.key))
}

const onOff = (v) => (v === true ? 'on' : v === false ? 'off' : '')
const num = (v) => (typeof v === 'number' ? String(v) : '')

/// The form, from the spec: '' is "left as the port has it".
export function draftOf(spec) {
  const s = spec || {}
  return {
    fec: s.fec || '',
    mtu: num(s.mtu),
    speed: num(s.speed),
    autoneg: onOff(s.autoneg),
    pauseRx: onOff(s.pause?.rx),
    pauseTx: onOff(s.pause?.tx),
    ringRx: num(s.rings?.rx),
    ringTx: num(s.rings?.tx),
  }
}

/// The settings the form changes, as the PATCH body: only what differs from the spec, a setting
/// emptied is `null` (handed back to the node). `{ error }` for what is not a number.
export function changeOf(draft, spec) {
  const was = draftOf(spec)
  const out = {}
  const bool = (v) => (v === '' ? null : v === 'on')
  const int = (label, v) => {
    if (v === '') return null
    const n = Number(v)
    if (!Number.isInteger(n)) throw new Error(`${label}: a whole number`)
    return n
  }
  try {
    if (draft.fec !== was.fec) out.fec = draft.fec || null
    if (draft.mtu !== was.mtu) out.mtu = int('MTU', draft.mtu.trim())
    if (draft.speed !== was.speed) out.speed = int('Speed', draft.speed.trim())
    if (draft.autoneg !== was.autoneg) out.autoneg = bool(draft.autoneg)
    const pair = (a, b, wa, wb, conv) => {
      if (a === wa && b === wb) return undefined
      if (a === '' && b === '') return null
      const o = {}
      if (a !== wa || a !== '') o.rx = conv('rx', a)
      if (b !== wb || b !== '') o.tx = conv('tx', b)
      return o
    }
    const p = pair(draft.pauseRx, draft.pauseTx, was.pauseRx, was.pauseTx, (_, v) => bool(v))
    if (p !== undefined) out.pause = p
    const r = pair(draft.ringRx.trim(), draft.ringTx.trim(), was.ringRx, was.ringTx, (side, v) => int(`Ring ${side}`, v))
    if (r !== undefined) out.rings = r
  } catch (e) {
    return { error: e.message }
  }
  return out
}

/// How a change reads before it is sent: "fec rs, mtu left to the node".
export function changeLine(change) {
  return Object.entries(change)
    .map(([k, v]) => (v === null ? `${k} left to the node` : `${k} ${typeof v === 'object' ? JSON.stringify(v) : v}`))
    .join(', ')
}

/// The last change, in words.
export function lastChangeLine(c) {
  if (!c) return ''
  const what = `${c.setting || 'setting'} ${c.from ?? '—'} → ${c.to ?? '—'}`
  const by = c.by ? ` by ${c.by}` : c.source ? ` from ${c.source}` : ''
  const err = c.error ? `, failed: ${c.error}` : ''
  return `${what}${by}${c.at ? ` at ${c.at}` : ''}${err}`
}
