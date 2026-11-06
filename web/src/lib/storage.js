// What the Storage page makes of the storage.storm.io objects (#96), apart from the page so a node test can
// read it. Shapes: stormblock src/mgmt/api/kube.rs; the reduced drives and enclosures of plugin-stormdrive.

const st = (o) => o?.status || {}
const sp = (o) => o?.spec || {}

/// How a health word reads. The components' words differ; these are the ones that want attention.
export function tone(word) {
  const w = String(word || '').toLowerCase()
  if (/fail|critical|offline|unreadable|lost/.test(w)) return 'bad'
  if (/degrad|warn|rebuild|noncritical|unrecoverable/.test(w)) return 'warn'
  return ''
}

export function volumeRows(items = []) {
  return items
    .map((o) => {
      const s = st(o)
      const missing = s.legsMissing || 0
      return {
        id: o.metadata?.name,
        name: sp(o).name || o.metadata?.name,
        sizeBytes: sp(o).sizeBytes,
        redundancy: sp(o).redundancy,
        role: sp(o).role,
        retention: sp(o).retention,
        family: sp(o).namespace,
        sealed: !!sp(o).sealed,
        access: sp(o).access,
        health: s.health,
        writable: s.writable,
        legsExpected: s.legsExpected,
        legsMissing: missing,
        unreadable: !!s.unreadable,
        failedSlabs: (s.failedSlabs || []).length,
        allocatedBytes: s.allocatedBytes,
        physicalBytes: s.physicalBytes,
        children: (s.children || []).length,
        attach: s.attach?.transport || '',
        trouble: missing > 0 || !!s.unreadable || (s.failedSlabs || []).length > 0 || tone(s.health) !== '',
      }
    })
    .sort((a, b) => b.trouble - a.trouble || String(a.name).localeCompare(String(b.name)))
}

export function slabRows(items = []) {
  return items
    .map((o) => {
      const s = st(o)
      return {
        id: o.metadata?.name,
        tier: sp(o).tier,
        role: sp(o).role,
        domain: sp(o).domain,
        path: sp(o).drive?.path,
        serial: sp(o).drive?.serial,
        capacityBytes: s.capacityBytes || 0,
        freeBytes: s.freeBytes || 0,
        quarantined: !!s.quarantined,
        metadata: !!s.holdsMetadata,
      }
    })
    .sort((a, b) => b.quarantined - a.quarantined || String(a.path).localeCompare(String(b.path)))
}

export function nodeRows(items = []) {
  return items
    .map((o) => {
      const s = st(o)
      return {
        name: o.metadata?.name,
        local: !!s.local,
        totalBytes: s.totalBytes || 0,
        freeBytes: s.freeBytes || 0,
        version: s.engineVersion || '',
        topology: (sp(o).topologyChain || Object.keys(sp(o).topology || {})).map((k) => `${k}=${sp(o).topology?.[k] ?? ''}`).join(' · '),
      }
    })
    .sort((a, b) => b.local - a.local || a.name.localeCompare(b.name))
}

/// One line per drive. The engine knows its slabs and drain, stormdrive its bay and health; a serial is what
/// they share. A drive only one of them knows still has a line, because that is itself a finding: a drive
/// the engine has that stormdrive does not, or one on another node, whose slabs this engine does not hold.
export function driveRows(engine = [], physical = []) {
  const rows = physical.map((d) => ({
    key: `${d.node}/${d.serial || d.name}`,
    node: d.node,
    path: d.path || d.name,
    serial: d.serial,
    model: d.model,
    capacityBytes: d.capacityBytes,
    enclosure: d.enclosure,
    bay: d.bay,
    membership: d.membership,
    designation: d.designation,
    health: d.health,
    temperatureC: d.temperatureC,
    wearPct: d.wearPct,
    slabs: null,
    drain: null,
    system: false,
    seenBy: 'stormdrive',
  }))
  for (const o of engine) {
    const s = st(o)
    // A file-backed drive's serial is literally "file", so a serial joins only where it names one thing.
    const one = physical.filter((d) => d.serial && d.serial === s.serial).length === 1 && engine.filter((e) => st(e).serial === s.serial).length === 1
    const row = one ? rows.find((r) => r.serial === s.serial) : null
    const fromEngine = { slabs: (s.slabs || []).length, drain: s.drain?.state || null, system: !!s.system }
    if (row) {
      Object.assign(row, fromEngine, { seenBy: 'both' })
    } else {
      rows.push({
        key: `engine/${o.metadata?.name}`,
        node: '',
        path: sp(o).path,
        serial: s.serial,
        model: s.model,
        capacityBytes: s.capacityBytes,
        enclosure: null,
        bay: null,
        membership: null,
        designation: null,
        health: s.smart ? (s.smart.healthy ? 'healthy' : 'failing') : null,
        temperatureC: s.smart?.temperatureC ?? null,
        wearPct: null,
        ...fromEngine,
        seenBy: 'engine',
      })
    }
  }
  return rows.sort((a, b) => String(a.node).localeCompare(String(b.node)) || String(a.path).localeCompare(String(b.path)))
}

export function enclosureRows(items = []) {
  return [...items].sort((a, b) => String(a.node).localeCompare(String(b.node)) || String(a.name).localeCompare(String(b.name)))
}

/// `{ok, total}` as the page says it, or a dash when the enclosure reports none.
export const okOf = (c) => (c && c.total ? `${c.ok} of ${c.total}` : '—')

export function totals({ volumes = [], slabs = [], nodes = [] }) {
  const sum = (l, k) => l.reduce((n, r) => n + (r[k] || 0), 0)
  return {
    volumes: volumes.length,
    trouble: volumes.filter((v) => v.trouble).length,
    slabs: slabs.length,
    quarantined: slabs.filter((s) => s.quarantined).length,
    capacityBytes: sum(nodes, 'totalBytes'),
    freeBytes: sum(nodes, 'freeBytes'),
    nodes: nodes.length,
  }
}

/// A row's text, for the search box.
export const matches = (row, q) => !q || JSON.stringify(row).toLowerCase().includes(q.toLowerCase())
