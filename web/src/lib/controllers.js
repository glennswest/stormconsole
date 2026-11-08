// RAID controllers (#325, stormdrive#132). Pure, so it runs under node.
//
// The drive plugin's `GET /api/plugins/drive/controllers` is every node's
// stormdrive `GET /api/v1/controllers`, each controller carrying its node and
// the proxy its verbs go through, each disk the drive row it is (or null).
// A controller in RAID mode keeps its disks from Linux behind its own
// configuration: HBA mode is required, and stormdrive raises `raid_mode`
// until HBA is asked for, then `reboot_pending` until a reboot applies it.
// What the controller holds against a disk — failed, unconfigured bad,
// offline — is what makes a drive "missing" to Linux, so it is said as that.

/// A disk state, in words. perccli's own word for the rest.
export function stateText(state) {
  switch (state) {
    case 'failed':
      return 'Failed by the controller'
    case 'unconfigured_bad':
      return 'Unconfigured bad (controller)'
    case 'offline':
      return 'Offline (controller)'
    case 'unconfigured_good':
      return 'Unconfigured good: hidden from Linux until JBOD'
    case 'jbod':
      return 'JBOD: Linux sees it'
    case 'online':
      return 'Online: in a virtual disk'
    case 'hot_spare':
      return 'Hot spare'
    case 'rebuild':
      return 'Rebuilding'
    case 'copyback':
      return 'Copyback'
    default:
      return state ? String(state).replace(/_/g, ' ') : 'unknown'
  }
}

/// A state the controller holds against the drive.
export function isFailed(state) {
  return state === 'failed' || state === 'unconfigured_bad' || state === 'offline'
}

/// The same from a drive row's `controller` metric, which stormdrive writes
/// with spaces ("unconfigured bad").
export function failedMetric(value) {
  return !!value && isFailed(String(value).trim().replace(/ /g, '_'))
}

export function modeText(mode) {
  return { raid: 'RAID', hba: 'HBA', jbod: 'JBOD personality', unknown: 'not known' }[mode] || String(mode || 'not known')
}

/// The banner(s): one per controller in RAID mode with HBA not asked for, and
/// one per controller waiting for a reboot. `[{tone, node, key, text}]`.
export function banners(controllers) {
  const out = []
  for (const c of controllers || []) {
    for (const f of c.findings || []) {
      if (f.code === 'raid_mode') {
        out.push({ tone: 'warn', node: c.node, key: c.key, code: f.code, text: `HBA mode is required: ${c.node}'s ${c.model || 'controller'} (${c.key}) is in RAID mode, which hides its disks from Linux.`, detail: f.message })
      } else if (f.code === 'reboot_pending') {
        out.push({ tone: 'info', node: c.node, key: c.key, code: f.code, text: `${c.node}'s ${c.model || 'controller'} (${c.key}): HBA asked for; a reboot of ${c.node} applies it.`, detail: f.message })
      }
    }
  }
  return out
}

/// Why Switch to HBA is not offered, or '' when it is. `guard` is the
/// console's answer for the path (`{allowed, reason}`), when read.
export function hbaWhy(c, guard) {
  if (c.mode === 'hba') return 'already in HBA mode'
  if (c.pending_mode === 'hba') return `HBA already asked for: a reboot of ${c.node} applies it`
  if (c.virtual_disks > 0) return `it has ${c.virtual_disks} virtual disk${c.virtual_disks === 1 ? '' : 's'}: they are the owner's data and are not deleted from here`
  if (c.perccli && !c.perccli.ok) return `perccli is not available on ${c.node}: ${c.perccli.why}`
  if (c.error) return `the controller could not be read: ${c.error}`
  if (guard && !guard.allowed) return guard.reason || 'only a storage-admin may'
  return ''
}

/// The disk states stormdrive's plan takes to good / JBOD (`raidctl::plan_state`).
const TO_GOOD = ['failed', 'unconfigured_bad', 'unconfigured_good', 'jbod']

/// Why a disk cannot be set `target` ('good' | 'jbod'), or ''. `row` is its
/// drive record (drivemap), when it has one: stormdrive refuses a drive in
/// use or in the fleet, and the system drive.
export function stateWhy(disk, target, row, guard) {
  const st = disk.state
  if (!TO_GOOD.includes(st)) return `${stateText(st).toLowerCase()}: a member or spare of the controller's configuration, not changed from here`
  if (target === 'good' && st === 'unconfigured_good') return 'already good'
  if (target === 'jbod' && st === 'jbod') return 'already JBOD'
  if (row?.use?.use === 'system') return 'the system drive'
  if (row?.use?.in_use) return `in use: ${row.use.reason || row.use.badge || ''}`.trim()
  if (row && !row.outOfFleet) return 'in the fleet: leave it first'
  if (guard && !guard.allowed) return guard.reason || 'only a storage-admin may'
  return ''
}

/// Where a disk's state change goes: by the drive when it has a record (so
/// the guard confirms it by its serial), else the controller's own route.
export function statePath(c, disk) {
  return disk.drive
    ? `${c.proxy}/api/v1/drives/${encodeURIComponent(disk.drive)}/controller-state`
    : `${c.proxy}/api/v1/controllers/${encodeURIComponent(c.key)}/drives/${disk.device_id}/state`
}

export function personalityPath(c) {
  return `${c.proxy}/api/v1/controllers/${encodeURIComponent(c.key)}/personality`
}

/// The body Set good / Set JBOD sends: force, because from Failed stormdrive
/// (and perccli) need it, and the confirmation is the person's.
export function stateBody(target) {
  return { state: target, force: true }
}

/// Controllers by node, for the page: `[{node, controllers}]`, nodes in the
/// order given, controllers with a RAID-mode finding first.
export function byNode(controllers) {
  const m = new Map()
  for (const c of controllers || []) {
    if (!m.has(c.node)) m.set(c.node, [])
    m.get(c.node).push(c)
  }
  const raid = (c) => ((c.findings || []).some((f) => f.code === 'raid_mode') ? 0 : 1)
  return [...m].map(([node, list]) => ({ node, controllers: [...list].sort((a, b) => raid(a) - raid(b) || String(a.key).localeCompare(String(b.key))) }))
}

/// Totals for the band.
export function counts(controllers) {
  const all = controllers || []
  const disks = all.flatMap((c) => c.drives || [])
  return {
    controllers: all.length,
    raid: all.filter((c) => (c.findings || []).some((f) => f.code === 'raid_mode')).length,
    pending: all.filter((c) => c.pending_mode).length,
    disks: disks.length,
    failed: disks.filter((d) => isFailed(d.state)).length,
  }
}
