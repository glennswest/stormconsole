// The Projects page's model (#162): one list, because a project is a
// namespace. The viewer's own and the platform's are the same kind of row,
// the system ones marked. Pure, so it runs under node.

// What a person looks for in a project, counted from the feed's ids:
// `<plugin>:<kind>:<project>/<name>`.
const HOLDS = [
  ['pods', 'k8s:pod:'],
  ['machines', 'vm:'],
  ['claims', 'k8s:pvc:'],
  ['services', 'k8s:svc:'],
]

/// Per project, how many of each it holds. A VM is one row whichever of its
/// two ids it has (`vm:machine:`, or `vm:instance:` with no definition).
export function holdings(components) {
  const out = new Map()
  for (const c of components) {
    for (const [what, prefix] of HOLDS) {
      if (!c.id.startsWith(prefix)) continue
      if (what === 'machines' && !/^vm:(machine|instance):/.test(c.id)) continue
      const key = c.id.split(':').slice(2).join(':')
      const slash = key.indexOf('/')
      if (slash < 1) continue
      const project = key.slice(0, slash)
      const counts = out.get(project) || { pods: 0, machines: 0, claims: 0, services: 0 }
      counts[what]++
      out.set(project, counts)
    }
  }
  return out
}

/// Every project this viewer is told of, theirs first, each alphabetical.
export function projectRows(list, system, components = []) {
  const held = holdings(components)
  const none = { pods: 0, machines: 0, claims: 0, services: 0 }
  const byName = (a, b) => a.name.localeCompare(b.name)
  const row = (isSystem) => (p) => ({ ...p, system: isSystem, holds: held.get(p.name) || none })
  return [...[...list].sort(byName).map(row(false)), ...[...system].sort(byName).map(row(true))]
}

/// `show` is 'all', 'mine' or 'system'; `search` matches names and owners.
export function filterProjects(rows, show = 'all', search = '') {
  const q = search.trim().toLowerCase()
  return rows.filter((p) => {
    if (show === 'mine' && p.system) return false
    if (show === 'system' && !p.system) return false
    return !q || `${p.name} ${p.displayName || ''} ${p.requester || ''}`.toLowerCase().includes(q)
  })
}

/// The line under a project's name: only what it holds, and "empty" rather
/// than a row of zeroes.
export function holdsLine(holds) {
  const parts = []
  if (holds.pods) parts.push(`${holds.pods} ${holds.pods === 1 ? 'pod' : 'pods'}`)
  if (holds.machines) parts.push(`${holds.machines} ${holds.machines === 1 ? 'VM' : 'VMs'}`)
  if (holds.claims) parts.push(`${holds.claims} ${holds.claims === 1 ? 'claim' : 'claims'}`)
  if (holds.services) parts.push(`${holds.services} ${holds.services === 1 ? 'service' : 'services'}`)
  return parts.length ? parts.join(' · ') : 'empty'
}
