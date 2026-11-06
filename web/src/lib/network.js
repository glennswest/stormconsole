// The Network Policies and Network Endpoints pages' model (#168), apart from the page so
// plain node can test it. Whichever network this is, the row is the same.

/// What the two answers amount to: the objects' rows, and a flowsdn agent's, which say
/// more about the same endpoint (whether it is enforcing), so an agent's row wins.
export function mergeEndpoints(objects, agent) {
  const rows = new Map()
  for (const r of objects || []) rows.set(`${r.project}/${r.name}`, r)
  for (const r of agent || []) rows.set(`${r.project}/${r.name}`, r)
  return [...rows.values()].sort((a, b) => `${a.project}/${a.name}`.localeCompare(`${b.project}/${b.name}`))
}

/// In a project, or all of them. A cluster-wide policy applies inside a project when it
/// selects anything there, so it is shown there too.
export function inProject(rows, project) {
  if (!project) return rows
  return rows.filter((r) => r.project === project || (r.project === '' && (r.appliesTo || []).some((a) => a.project === project)))
}

export function search(rows, needle) {
  const n = (needle || '').toLowerCase()
  if (!n) return rows
  return rows.filter((r) =>
    [r.name, r.project, r.node, r.state, r.identityLabels, r.kind, r.sort, ...(r.ips || []), ...(r.selects || []), r.identity]
      .filter((x) => x != null)
      .join(' ')
      .toLowerCase()
      .includes(n),
  )
}

/// What a direction's rules come to. A standard policy naming a direction with no rules
/// denies it all, which is what a default-deny is.
export function rulesLine(p, dir) {
  const n = dir === 'ingress' ? p.ingress : p.egress
  const word = dir === 'ingress' ? 'Ingress' : 'Egress'
  if (n) return `${n} ${n === 1 ? 'rule' : 'rules'}`
  return (p.types || []).includes(word) ? 'all denied' : '—'
}

/// Whether a direction is enforced. Not said is not "no".
export function enforcement(v) {
  return v === true ? 'enforced' : v === false ? 'not enforced' : '—'
}

export function selectsLine(p) {
  if (!p.selects?.length) return p.project ? 'every pod in the project' : 'every endpoint'
  return p.selects.join('; ')
}

/// The small detail: where these rows came from.
export function fromLine(rows, network) {
  const froms = [...new Set(rows.map((r) => r.from).filter(Boolean))]
  if (froms.length) return `from: ${froms.join(', ')}`
  return network ? `from: ${network}` : ''
}

/// Where the address of the other network's page lands on this node (#285). Nothing while the
/// node has not said.
export function notHere(path, network) {
  if (!network) return null
  if (network === 'cilium') return { '#/flowsdn': '#/network/endpoints', '#/k8s/fsid': '#/k8s/cid' }[path] || null
  if (network === 'flowsdn') return { '#/network/flows': '#/flowsdn', '#/k8s/cn': '#/flowsdn', '#/k8s/cid': '#/k8s/fsid' }[path] || null
  return { '#/flowsdn': '#/network/endpoints', '#/network/flows': '#/network/endpoints', '#/k8s/cn': '#/network/endpoints', '#/k8s/cid': '#/network/endpoints', '#/k8s/fsid': '#/network/endpoints' }[path] || null
}
