// A drive's firmware (#317). Pure, so it runs under node. The records are the
// drive plugin's (`GET /api/plugins/drive/firmware`, firmware.rs), and its
// rules are mirrored here so the page says them before anyone asks: one model
// at a time, the image named for it, never two members of one RAID set, never
// a member of a set that is not clean.

/// A model as it stands in an image name (firmware.rs `model_key`).
export function modelKey(model) {
  return String(model || '')
    .trim()
    .replace(/[^A-Za-z0-9._-]/g, '-')
    .replace(/^-+|-+$/g, '')
}

/// The model an image is named for, `<model>+<file>`, or null.
export function imageModel(name) {
  const at = String(name || '').indexOf('+')
  return at > 0 ? name.slice(0, at) : null
}

/// What an upload of `file` for `model` is called in the store.
export function uploadName(model, file) {
  const m = modelKey(model)
  if (!m) throw new Error('the drive reports no model')
  const base = String(file || '').split(/[/\\]/).pop()
  const f = base.replace(/[^A-Za-z0-9._-]/g, '-').replace(/^[-.]+|[-.]+$/g, '')
  if (!f) throw new Error(`${JSON.stringify(file)} is not a file name`)
  const name = `${m}+${f}`
  if (name.length > 128) throw new Error(`${name}: at most 128 characters`)
  return name
}

const same = (a, b) => String(a).toLowerCase() === String(b).toLowerCase()

/// The images a selection can take, on every node it spans: `fits` named for
/// its model, `unnamed` naming none (to be checked by hand). One named for
/// another model is not offered.
export function imagesFor(recs, images) {
  const nodes = [...new Set(recs.map((r) => r.node))]
  const key = recs[0]?.modelKey || ''
  const byName = new Map()
  for (const i of images || []) {
    if (!byName.has(i.name)) byName.set(i.name, { ...i, nodes: [] })
    byName.get(i.name).nodes.push(i.node)
  }
  const fits = []
  const unnamed = []
  for (const i of byName.values()) {
    const missing = nodes.filter((n) => !i.nodes.includes(n))
    const entry = { ...i, missing }
    if (i.model === null || i.model === undefined) unnamed.push(entry)
    else if (key && same(i.model, key)) fits.push(entry)
  }
  const order = (a, b) => a.missing.length - b.missing.length || String(b.name).localeCompare(String(a.name))
  return { fits: fits.sort(order), unnamed: unnamed.sort(order) }
}

/// Why this selection cannot be updated together, or ''.
export function selectionWhy(recs) {
  if (!recs.length) return 'no drives chosen'
  const bad = recs.find((r) => r.whyNot)
  if (bad) return `${bad.name}: ${bad.whyNot}`
  const models = [...new Set(recs.map((r) => (r.modelKey || '').toLowerCase()))]
  if (models.length > 1) return `one model at a time: ${recs.map((r) => `${r.name} (${r.model || 'no model'})`).join(', ')}`
  if (!models[0]) return 'the drive reports no model, so no image can be matched to it'
  for (const r of recs) {
    if (r.set && r.setState !== 'clean') return `${r.name} is in RAID set ${r.set}, which is ${r.setState}: update it once the set is clean`
  }
  for (let i = 0; i < recs.length; i++) {
    const a = recs[i]
    if (!a.set) continue
    const b = recs.slice(i + 1).find((x) => x.set === a.set)
    if (b) return `${a.name} and ${b.name} are both in RAID set ${a.set}: one at a time, so the set never loses redundancy. Update ${a.name}, then ${b.name} once the set is clean again (stormdrive#126)`
  }
  return ''
}

/// Why this image cannot go on this selection, or ''.
export function imageWhy(recs, image, checked) {
  if (!image) return 'choose an image'
  const key = recs[0]?.modelKey || ''
  if (image.model && !same(image.model, key)) return `${image.name} is for ${image.model}, and these drives are ${key}`
  if (!image.model && !checked) return `${image.name} does not name a model, and stormdrive does not check one (stormdrive#126): tick that you checked it is for ${recs[0]?.model || key}`
  if (image.missing?.length) return `${image.name} is not in the image store of ${image.missing.join(', ')}: upload it there`
  return ''
}

/// The phrase that confirms an update; the server checks the same one.
export function phrase(recs) {
  if (recs.length === 1 && recs[0].serial) return recs[0].serial
  return `update ${recs.length} drives`
}

export const isRunning = (r) => r.run?.state === 'running' || r.run?.state === 'queued'

/// A row for each update running, or ended within `recent` seconds of `now`
/// (a kept record's `finished` is stormdrive's SystemTime: seconds and nanos).
export function rows(recs, now, recent = 3600) {
  const t = (x) => (x && typeof x === 'object' ? x.secs_since_epoch : typeof x === 'number' ? x : null)
  return recs
    .filter((r) => r.run && (isRunning(r) || (t(r.run.finished) !== null && now - t(r.run.finished) <= recent)))
    .map((r) => ({ ...r, back: readBack(r) }))
    .sort((a, b) => Number(isRunning(b)) - Number(isRunning(a)) || String(a.name).localeCompare(String(b.name)))
}

/// What the drive reports after the update: its version now, against what the run says it went to.
export function readBack(r) {
  const run = r.run
  if (!run || isRunning(r)) return null
  if (run.state === 'failed' || run.state === 'interrupted') return { ok: false, text: `still ${r.version}${run.error ? `: ${run.error}` : ''}` }
  if (run.to && r.version === run.to) {
    return { ok: true, text: `reads back ${r.version} (was ${run.from})${run.resetRequired ? '; takes effect at the next reset' : ''}` }
  }
  return { ok: false, text: `reads back ${r.version}, the run says ${run.to || 'nothing'}` }
}
