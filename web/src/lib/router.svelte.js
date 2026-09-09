// A hash router in one file. Routes are '#/path/:param' patterns; the
// current match is reactive state any component can read.

const routes = [
  { pattern: '#/', name: 'overview' },
  { pattern: '#/logs', name: 'logs' },
  // component ids carry ':' and '/', so the grid root travels in the query:
  // #/grid?id=<component>&rel=<relation>
  { pattern: '#/grid', name: 'grid' },
  { pattern: '#/drives', name: 'drives' },
  { pattern: '#/vms', name: 'vmlist' },
  { pattern: '#/nodes', name: 'nodes' },
  { pattern: '#/node/:host', name: 'nodedetail' },
  { pattern: '#/vm/:ns/:name', name: 'vmdetail' },
  { pattern: '#/k8s/events', name: 'k8sevents' },
  // A namespace is a place, not a row: it has a page of its own, and it
  // is matched before the generic kind list because it is longer.
  { pattern: '#/k8s/ns/:name', name: 'namespace' },
  { pattern: '#/k8s/:kind', name: 'k8slist' },
]

function match(hash) {
  if (!hash || hash === '#') hash = '#/'
  const [path, query] = hash.split('?')
  for (const r of routes) {
    // Fresh per candidate: a partial match must not leave its parameters
    // behind for the route that eventually wins.
    const params = {}
    const rp = r.pattern.split('/')
    const hp = path.split('/')
    if (rp.length !== hp.length) continue
    let ok = true
    for (let i = 0; i < rp.length; i++) {
      if (rp[i].startsWith(':')) params[rp[i].slice(1)] = decodeURIComponent(hp[i])
      else if (rp[i] !== hp[i]) { ok = false; break }
    }
    if (ok) {
      return { name: r.name, params, query: new URLSearchParams(query || '') }
    }
  }
  return { name: 'overview', params: {}, query: new URLSearchParams() }
}

export const route = $state({ current: match(location.hash) })

window.addEventListener('hashchange', () => {
  route.current = match(location.hash)
})

export function navigate(hash) {
  location.hash = hash
}
