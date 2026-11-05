// A hash router in one file. Routes are '#/path/:param' patterns; the
// current match is reactive state any component can read.

const routes = [
  { pattern: '#/', name: 'overview' },
  { pattern: '#/logs', name: 'logs' },
  // component ids carry ':' and '/', so the grid root travels in the query:
  // #/grid?id=<component>&rel=<relation>
  { pattern: '#/grid', name: 'grid' },
  { pattern: '#/drives', name: 'drives' },
  // Formatting drives, and following a format (#165).
  { pattern: '#/drives/format', name: 'driveformat' },
  { pattern: '#/vms', name: 'vmlist' },
  { pattern: '#/nodes', name: 'nodes' },
  // Every API on this node, its state and latency (#123).
  { pattern: '#/health', name: 'health' },
  // What the cluster runs, an update's steps, history (#164).
  { pattern: '#/update', name: 'update' },
  // This node's boots, installs, drives and hardware, kept across installs
  // (#121); the part and the record travel in the query.
  { pattern: '#/history', name: 'history' },
  // The datastore's keyspace; the prefix and an opened key travel in the
  // query because both are full of '/'.
  { pattern: '#/etcd/keys', name: 'etcdkeys' },
  // Your own SSH keys, given to every machine you create (#26).
  { pattern: '#/account/keys', name: 'accountkeys' },
  // Projects are the top of the console (#28).
  { pattern: '#/projects', name: 'projects' },
  // Bare metal by service tag, from stormipmi (#31).
  { pattern: '#/machines', name: 'machines' },
  // What the cluster is made of, from stormcluster (#63).
  { pattern: '#/cluster', name: 'cluster' },
  // This node's pod network on the flowsdn edition (#83).
  { pattern: '#/flowsdn', name: 'flowsdn' },
  // The registry's catalog: goldens, blanks and media are images (#19).
  { pattern: '#/images', name: 'images' },
  // Repositories, tags, and who holds a clone (#163).
  { pattern: '#/registry', name: 'registry' },
  { pattern: '#/attach/:ns/:name', name: 'attachclaim' },
  { pattern: '#/node/:host', name: 'nodedetail' },
  { pattern: '#/vm/:ns/:name', name: 'vmdetail' },
  // One pod, all of it (#69).
  { pattern: '#/pod/:ns/:name', name: 'poddetail' },
  { pattern: '#/k8s/events', name: 'k8sevents' },
  // A project is a place, not a row: it has a page of its own, and it is
  // matched before the generic kind list because it is longer. The path is
  // the API's word, as in `#/k8s/pod`; the page's word is project (#162).
  { pattern: '#/k8s/ns/:name', name: 'project' },
  { pattern: '#/k8s/:kind', name: 'k8slist' },
]

// Where a page went. A project is a namespace, so the list of namespaces is
// the Projects page (#162); an old link lands there.
const MOVED = { '#/k8s/ns': '#/projects' }

function follow() {
  const to = MOVED[(location.hash || '').split('?')[0]]
  if (to) location.replace(to)
  return !!to
}

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

follow()
export const route = $state({ current: match(location.hash) })

window.addEventListener('hashchange', () => {
  if (!follow()) route.current = match(location.hash)
})

export function navigate(hash) {
  location.hash = hash
}
